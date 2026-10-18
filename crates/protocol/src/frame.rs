//! 字节流上的自研分帧（研究稿 §3）。
//!
//! ```text
//! offset  size  field      说明
//! 0       4     length     u32 LE —— 本帧**除 length 字段外**的剩余字节数（含 type..payload）
//! 4       2     type       u16 LE —— 见 [`FrameKind`]
//! 6       2     flags      u16 LE —— 见 [`Flags`]
//! 8       4     session    u32 LE —— 0 = 连接级；否则为 daemon 分配的**连接内会话句柄**
//! 12      8     seq        u64 LE —— 会话内单调递增（数据 / ack 用）；不适用时 0
//! 20      N     payload    数据帧 = 原始字节；控制帧 = UTF-8 JSON
//! ```
//!
//! > 命名：帧头那个槽位研究稿叫 `sessionId`；Rust 侧一律叫 [`Frame::handle`]——它是**连接内**
//! > 的会话句柄，而「会话」在 `CONTEXT.md` 里是另一层概念（逻辑 `sid` 是不透明字符串，只出现在
//! > 控制 payload 里）。
//!
//! 三条设计约束（研究稿 §3.1）：
//! 1. **长度前缀是唯一可靠的边界来源**——不做转义、不猜；
//! 2. payload 怎么解由 `type` 决定，**不靠猜**；
//! 3. **长度必须校验上界**（[`MAX_FRAME_BYTES`]）：损坏的长度字段绝不能换来一次超大分配。

use std::io::{self, Read, Write};

/// 定长头大小。
pub const HEADER_BYTES: usize = 20;

/// 单帧字节数上限（含头）。研究稿 §3.2 的 `MAX_FRAME = 1 MiB`。
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// 帧类型（研究稿 §3.4）。
///
/// [`FrameKind::Unknown`] 保留**未知类型**：解码不因此失败（研究稿 §6.1 的兼容策略——未知帧
/// 一律忽略而不是断连），交给上层决定跳不跳过。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameKind {
    // —— 连接级（`handle = 0`）
    /// UI→D：握手（版本区间 + 能力位 + 令牌）。
    Hello,
    /// D→UI：握手结果。
    Welcome,
    /// D→UI：错误（含**版本不匹配**这种明确拒绝）。
    Error,
    /// 双向：心跳。
    Ping,
    /// 双向：心跳应答。
    Pong,
    /// UI→D：枚举会话（重连后重建页签）。
    ListSessions,
    /// D→UI：会话清单。
    Sessions,
    /// UI→D：优雅停机（终止所有会话后 daemon 退出）。
    Shutdown,
    /// D→UI：停机确认（会话已清空，daemon 即将退出）。
    ShutdownAck,
    // —— 会话管理
    /// UI→D：建会话。
    CreateSession,
    /// D→UI：建好了（回逻辑 `sid` 与连接内句柄）。
    SessionCreated,
    /// UI→D：接回已有会话。
    Attach,
    /// D→UI：接上了（从哪个 `seq` 起）。
    Attached,
    /// UI→D：seq 断档，请发一份当前 checkpoint。
    Checkpoint,
    /// D→UI：checkpoint（ANSI 快照 + 对应 `seq`）。
    Snapshot,
    /// UI→D：断开（**≠ 终止**，会话继续活在 daemon 里）。
    Detach,
    /// UI→D：真正结束会话。
    Terminate,
    /// D→UI：会话进程退出。
    SessionExited,
    /// UI→D：改终端尺寸。
    Resize,
    /// UI→D：累计已消费字节数。
    Ack,
    /// shim→D / D→UI：agent 事件（负载形状见 [`crate::AgentEvent`]）。
    AgentEvent,
    // —— 会话级（原始字节）
    /// D→UI：PTY 输出。
    Data,
    /// UI→D：键盘输入。
    Input,
    /// 未知类型（保留原值，以便无损转交）。
    Unknown(u16),
}

impl FrameKind {
    /// 原始 `type` 值。
    pub fn as_u16(self) -> u16 {
        match self {
            FrameKind::Hello => 0x0001,
            FrameKind::Welcome => 0x0002,
            FrameKind::Error => 0x0003,
            FrameKind::Ping => 0x0004,
            FrameKind::Pong => 0x0005,
            FrameKind::ListSessions => 0x0010,
            FrameKind::Sessions => 0x0011,
            FrameKind::Shutdown => 0x0012,
            FrameKind::ShutdownAck => 0x0013,
            FrameKind::CreateSession => 0x0020,
            FrameKind::SessionCreated => 0x0021,
            FrameKind::Attach => 0x0022,
            FrameKind::Attached => 0x0023,
            FrameKind::Checkpoint => 0x0028,
            FrameKind::Snapshot => 0x0029,
            FrameKind::Detach => 0x0024,
            FrameKind::Terminate => 0x0025,
            FrameKind::SessionExited => 0x0026,
            FrameKind::Resize => 0x0027,
            FrameKind::AgentEvent => 0x0030,
            FrameKind::Data => 0x0100,
            FrameKind::Input => 0x0101,
            FrameKind::Ack => 0x0102,
            FrameKind::Unknown(raw) => raw,
        }
    }

    /// 由原始 `type` 值还原；不认识的进 [`FrameKind::Unknown`]。
    pub fn from_u16(raw: u16) -> Self {
        match raw {
            0x0001 => FrameKind::Hello,
            0x0002 => FrameKind::Welcome,
            0x0003 => FrameKind::Error,
            0x0004 => FrameKind::Ping,
            0x0005 => FrameKind::Pong,
            0x0010 => FrameKind::ListSessions,
            0x0011 => FrameKind::Sessions,
            0x0012 => FrameKind::Shutdown,
            0x0013 => FrameKind::ShutdownAck,
            0x0020 => FrameKind::CreateSession,
            0x0021 => FrameKind::SessionCreated,
            0x0022 => FrameKind::Attach,
            0x0023 => FrameKind::Attached,
            0x0028 => FrameKind::Checkpoint,
            0x0029 => FrameKind::Snapshot,
            0x0024 => FrameKind::Detach,
            0x0025 => FrameKind::Terminate,
            0x0026 => FrameKind::SessionExited,
            0x0027 => FrameKind::Resize,
            0x0030 => FrameKind::AgentEvent,
            0x0100 => FrameKind::Data,
            0x0101 => FrameKind::Input,
            0x0102 => FrameKind::Ack,
            other => FrameKind::Unknown(other),
        }
    }

    /// 是否是**原始字节**负载（数据面）而非 JSON 控制面。
    ///
    /// 控制优先队列据此分流（[`crate::queue`]）。
    pub fn is_raw_payload(self) -> bool {
        matches!(self, FrameKind::Data | FrameKind::Input)
    }
}

/// 帧标志位（研究稿 §3.2）。
///
/// 保留原样 u16：未知位也要能往返，便于将来加位而不破坏转发。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Flags(u16);

impl Flags {
    /// payload 是原始字节（数据帧）；0 = JSON 控制帧。
    pub const PAYLOAD_IS_RAW: u16 = 1 << 0;
    /// 回放环形缓冲里的历史数据（UI 据此插入「已重新连接」分隔）。
    pub const IS_REPLAY: u16 = 1 << 1;
    /// payload 是否压缩（v1 恒 0，预留）。
    pub const COMPRESSED: u16 = 1 << 2;
    /// 分片帧：后面还有同 `type` 的续帧。
    pub const MORE: u16 = 1 << 3;

    /// 空标志。
    pub fn empty() -> Self {
        Self(0)
    }

    /// 由原始位构造（未知位保留）。
    pub fn from_bits(bits: u16) -> Self {
        Self(bits)
    }

    /// 原始位。
    pub fn bits(self) -> u16 {
        self.0
    }

    /// 是否置了某个（或某几个）位。
    pub fn contains(self, bits: u16) -> bool {
        self.0 & bits == bits
    }

    /// 置位。
    pub fn with(self, bits: u16) -> Self {
        Self(self.0 | bits)
    }
}

/// 一个帧。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// 帧类型。
    pub kind: FrameKind,
    /// 标志位。
    pub flags: Flags,
    /// 0 = 连接级；否则是 daemon 分配的**连接内会话句柄**（逻辑 `sid` 是不透明字符串，只在控制
    /// payload 里出现——见 [`crate::message`]）。研究稿把这个槽位叫 `sessionId`。
    pub handle: u32,
    /// 会话内单调递增序号；不适用时 0。
    pub seq: u64,
    /// 负载。
    pub payload: Vec<u8>,
}

impl Frame {
    /// 控制帧（JSON 负载）。
    pub fn control(kind: FrameKind, handle: u32, seq: u64, payload: Vec<u8>) -> Self {
        Self {
            kind,
            flags: Flags::empty(),
            handle,
            seq,
            payload,
        }
    }

    /// 数据帧（原始字节负载，置 `PAYLOAD_IS_RAW`）。
    pub fn raw(kind: FrameKind, handle: u32, seq: u64, payload: Vec<u8>) -> Self {
        Self {
            kind,
            flags: Flags::empty().with(Flags::PAYLOAD_IS_RAW),
            handle,
            seq,
            payload,
        }
    }

    /// 整帧字节数（含头）。
    pub fn encoded_len(&self) -> usize {
        HEADER_BYTES + self.payload.len()
    }

    /// 编码进 `out`（追加，不覆盖）。
    pub fn encode(&self, out: &mut Vec<u8>) -> Result<(), FrameError> {
        let total = self.encoded_len();
        if total > MAX_FRAME_BYTES {
            return Err(FrameError::TooLarge {
                size: total,
                max: MAX_FRAME_BYTES,
            });
        }
        out.reserve(total);
        // `length` 不含它自己那 4 字节。
        out.extend_from_slice(&((total - 4) as u32).to_le_bytes());
        out.extend_from_slice(&self.kind.as_u16().to_le_bytes());
        out.extend_from_slice(&self.flags.bits().to_le_bytes());
        out.extend_from_slice(&self.handle.to_le_bytes());
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.extend_from_slice(&self.payload);
        Ok(())
    }

    /// 编码并整帧写出。
    ///
    /// **不调 `flush`**：`write_all` 返回时数据已经交出去了，而 `flush` 对管道/file 的语义各不相同
    /// （在命名管道上 `FlushFileBuffers` 会「等对端读完才返回」，反而制造耦合）。
    /// 需要落盘/刷新的调用方自己负责——帧层只管把一帧完整写出去。
    pub fn write_to(&self, writer: &mut impl Write) -> Result<(), FrameError> {
        let mut bytes = Vec::with_capacity(self.encoded_len());
        self.encode(&mut bytes)?;
        writer.write_all(&bytes)?;
        Ok(())
    }

    /// 读一个帧。**流在帧边界处正常结束**时返回 `Ok(None)`。
    ///
    /// 上界校验发生在**分配之前**：损坏的长度字段只会得到一个错误，不会换来一次大分配。
    pub fn read_from(reader: &mut impl Read) -> Result<Option<Self>, FrameError> {
        let mut length_bytes = [0u8; 4];
        let got = read_full(reader, &mut length_bytes)?;
        if got == 0 {
            // 流在帧边界处正常结束。
            return Ok(None);
        }
        if got != 4 {
            return Err(FrameError::Incomplete { expected: 4, got });
        }
        let length = u32::from_le_bytes(length_bytes) as usize;
        let total = length + 4;

        // 先判上界，再分配。
        let min_total = HEADER_BYTES;
        if total < min_total || total > MAX_FRAME_BYTES {
            return Err(FrameError::TooLarge {
                size: total,
                max: MAX_FRAME_BYTES,
            });
        }

        let mut rest = vec![0u8; length];
        let got = read_full(reader, &mut rest)?;
        if got != length {
            return Err(FrameError::Incomplete {
                expected: length,
                got,
            });
        }

        let kind = FrameKind::from_u16(u16::from_le_bytes([rest[0], rest[1]]));
        let flags = Flags::from_bits(u16::from_le_bytes([rest[2], rest[3]]));
        let handle = u32::from_le_bytes([rest[4], rest[5], rest[6], rest[7]]);
        let seq = u64::from_le_bytes([
            rest[8], rest[9], rest[10], rest[11], rest[12], rest[13], rest[14], rest[15],
        ]);
        let payload = rest.split_off(HEADER_BYTES - 4);

        Ok(Some(Self {
            kind,
            flags,
            handle,
            seq,
            payload,
        }))
    }
}

/// 把一个逻辑负载切成若干帧——**除最后一帧外都置 [`Flags::MORE`]**。
///
/// 研究稿 §3.2：超过 [`MAX_FRAME_BYTES`] 的块由**发送侧**切分，接收侧按 `handle` + `seq` 顺序
/// 拼接。各分片**共用同一个 `seq`**：它们是**一个**逻辑消息，不是多条消息。
///
/// 帧层只负责把这个语义做对（每帧都在上界内、`MORE` 置位正确）；真正的消费者在数据路径——
/// ConPTY 输出与大快照（#67 / #71）。
pub fn fragments(kind: FrameKind, handle: u32, seq: u64, payload: &[u8]) -> Vec<Frame> {
    let max_payload = MAX_FRAME_BYTES - HEADER_BYTES;
    // 空负载也要占一帧，否则接收侧看不到这个逻辑消息。
    if payload.is_empty() {
        return vec![Frame::raw(kind, handle, seq, Vec::new())];
    }

    let mut chunks = payload.chunks(max_payload).peekable();
    let mut frames = Vec::new();
    while let Some(chunk) = chunks.next() {
        let mut frame = Frame::raw(kind, handle, seq, chunk.to_vec());
        if chunks.peek().is_some() {
            frame.flags = frame.flags.with(Flags::MORE);
        }
        frames.push(frame);
    }
    frames
}

/// 读满整个缓冲，返回实际读到的字节数（EOF 时可能少于缓冲长度）。
fn read_full(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..])? {
            0 => break,
            read => filled += read,
        }
    }
    Ok(filled)
}

/// 分帧错误。
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// 帧超过 [`MAX_FRAME_BYTES`]（含 `length` 字段不可信的情形）。
    #[error("帧过大：{size} 字节（上限 {max}）")]
    TooLarge { size: usize, max: usize },

    /// 帧没读全（对端在帧中间断了）。
    #[error("帧不完整：期望 {expected} 字节，实际 {got}")]
    Incomplete { expected: usize, got: usize },

    /// 底层 IO 失败。
    #[error("帧 IO 失败：{0}")]
    Io(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn kinds() -> impl Strategy<Value = FrameKind> {
        prop_oneof![
            Just(FrameKind::Hello),
            Just(FrameKind::Welcome),
            Just(FrameKind::Error),
            Just(FrameKind::Ping),
            Just(FrameKind::Pong),
            Just(FrameKind::CreateSession),
            Just(FrameKind::Data),
            Just(FrameKind::Input),
            Just(FrameKind::Ack),
            // 避开已占用的类型码；否则 "Unknown(0x0001)" 解码成 Hello，往返必然失败。
            (0x2000u16..=0xFFFF).prop_map(FrameKind::Unknown),
        ]
    }

    proptest! {
        /// 验收 1：帧往返性质。
        #[test]
        fn round_trips(
            kind in kinds(),
            flags in any::<u16>(),
            session in any::<u32>(),
            seq in any::<u64>(),
            payload in proptest::collection::vec(any::<u8>(), 0..4096),
        ) {
            let frame = Frame { kind, flags: Flags::from_bits(flags), handle: session, seq, payload };
            let mut bytes = Vec::new();
            frame.encode(&mut bytes).unwrap();
            prop_assert_eq!(bytes.len(), frame.encoded_len());

            let mut cursor = bytes.as_slice();
            let decoded = Frame::read_from(&mut cursor).unwrap().unwrap();
            prop_assert_eq!(&decoded, &frame);
            // 帧边界被准确消费：后面没有残留。
            prop_assert!(cursor.is_empty());
        }

        /// 损坏 / 随机输入不得 panic（长度上界在分配之前生效）。
        #[test]
        fn decoding_arbitrary_bytes_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..8192)) {
            let mut cursor = bytes.as_slice();
            let _ = Frame::read_from(&mut cursor);
        }

        /// 连续多帧能按序逐个读出。
        #[test]
        fn reads_frames_in_sequence(
            frames in proptest::collection::vec(
                (kinds(), any::<u32>(), any::<u64>(), proptest::collection::vec(any::<u8>(), 0..256)),
                1..8,
            ),
        ) {
            let mut bytes = Vec::new();
            let mut expected = Vec::new();
            for (kind, handle, seq, payload) in frames {
                let frame = Frame::control(kind, handle, seq, payload);
                frame.encode(&mut bytes).unwrap();
                expected.push(frame);
            }
            let mut cursor = bytes.as_slice();
            for want in expected {
                let got = Frame::read_from(&mut cursor).unwrap().unwrap();
                prop_assert_eq!(got, want);
            }
            prop_assert!(Frame::read_from(&mut cursor).unwrap().is_none());
        }
    }

    #[test]
    fn empty_stream_is_a_clean_end() {
        let mut cursor = [].as_slice();
        assert!(Frame::read_from(&mut cursor).unwrap().is_none());
    }

    #[test]
    fn oversized_length_is_rejected_before_allocating() {
        // 伪造一个长度字段极大、但后面没有数据的头。
        let mut bytes = u32::MAX.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[0u8; 8]);
        let error = Frame::read_from(&mut bytes.as_slice()).unwrap_err();
        assert!(matches!(error, FrameError::TooLarge { .. }), "{error:?}");
    }

    #[test]
    fn too_short_length_is_rejected() {
        // length 连头都装不下。
        let bytes = 3u32.to_le_bytes().to_vec();
        let error = Frame::read_from(&mut bytes.as_slice()).unwrap_err();
        assert!(matches!(error, FrameError::TooLarge { .. }), "{error:?}");
    }

    #[test]
    fn truncated_payload_is_an_incomplete_frame() {
        let frame = Frame::control(FrameKind::Ping, 0, 0, vec![1, 2, 3, 4]);
        let mut bytes = Vec::new();
        frame.encode(&mut bytes).unwrap();
        bytes.truncate(bytes.len() - 2);
        let error = Frame::read_from(&mut bytes.as_slice()).unwrap_err();
        assert!(matches!(error, FrameError::Incomplete { .. }), "{error:?}");
    }

    #[test]
    fn data_frames_carry_the_raw_flag() {
        let frame = Frame::raw(FrameKind::Data, 7, 9, b"hi".to_vec());
        assert!(frame.flags.contains(Flags::PAYLOAD_IS_RAW));
        assert!(frame.kind.is_raw_payload());
        assert!(!FrameKind::Resize.is_raw_payload());
    }

    #[test]
    fn fragments_split_oversized_payloads_and_mark_the_continuation() {
        let payload = vec![0xAB; MAX_FRAME_BYTES + 1234];
        let frames = fragments(FrameKind::Data, 5, 77, &payload);

        assert_eq!(frames.len(), 2, "一帧装不下就该切成两帧");
        for frame in &frames {
            assert!(
                frame.encoded_len() <= MAX_FRAME_BYTES,
                "每一片都必须在帧上界内"
            );
            assert_eq!(frame.handle, 5);
            assert_eq!(frame.seq, 77, "各分片共用一个 seq：它们是一个逻辑消息");
        }
        assert!(frames[0].flags.contains(Flags::MORE), "非末片要置 MORE");
        assert!(!frames[1].flags.contains(Flags::MORE), "末片不置 MORE");

        // 接收侧的拼接规则：按序把 payload 接起来就还原。
        let reassembled: Vec<u8> = frames.iter().flat_map(|f| f.payload.clone()).collect();
        assert_eq!(reassembled, payload);

        // 每片都能按序从字节流里读回来。
        let mut bytes = Vec::new();
        for frame in &frames {
            frame.encode(&mut bytes).unwrap();
        }
        let mut cursor = bytes.as_slice();
        for want in &frames {
            assert_eq!(&Frame::read_from(&mut cursor).unwrap().unwrap(), want);
        }
    }

    #[test]
    fn fragments_keep_small_payloads_as_one_frame() {
        let frames = fragments(FrameKind::Data, 1, 1, b"short");
        assert_eq!(frames.len(), 1);
        assert!(!frames[0].flags.contains(Flags::MORE));

        let empty = fragments(FrameKind::Data, 1, 2, b"");
        assert_eq!(empty.len(), 1, "空负载也要占一帧");
        assert!(empty[0].payload.is_empty());
    }
}

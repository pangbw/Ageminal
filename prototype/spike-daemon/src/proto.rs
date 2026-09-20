//! 长度前缀帧（B2 #5 的最小化）。
//!
//! 线上格式：`[u8 kind][u32 len LE][payload]`
//! PoC 只实现判定所需的最小子集；版本协商与能力位按 B2 #5 用 Hello/HelloAck 表达。

use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const KIND_DATA: u8 = 0x01; // daemon → client：PTY 输出
pub const KIND_INPUT: u8 = 0x02; // client → daemon：键盘输入
pub const KIND_RESIZE: u8 = 0x03; // client → daemon：[u16 cols][u16 rows]
pub const KIND_HELLO: u8 = 0x04; // client → daemon：[u32 proto_version]
pub const KIND_HELLO_ACK: u8 = 0x05; // daemon → client：[u32 proto][u32 caps][u64 buffered_bytes][u32 pid][u32 alive]
pub const KIND_REPLAY: u8 = 0x06; // daemon → client：环形缓冲快照
pub const KIND_PING: u8 = 0x07; // client → daemon：[u64 seq]
pub const KIND_PONG: u8 = 0x08; // daemon → client：[u64 seq]
pub const KIND_STATUS: u8 = 0x09; // client → daemon：请求状态
pub const KIND_STATUS_REPLY: u8 = 0x0a; // daemon → client：JSON 文本
pub const KIND_DETACH: u8 = 0x0b; // client → daemon：优雅断开
pub const KIND_SHUTDOWN: u8 = 0x0c; // client → daemon：结束所有会话并退出
pub const KIND_GAP: u8 = 0x0d; // daemon → client：广播滞后（有丢失）

pub const PROTO_VERSION: u32 = 1;

/// 能力位（B2 #5：版本区间 + 能力位）
pub const CAP_REPLAY: u32 = 1 << 0;
pub const CAP_RESIZE: u32 = 1 << 1;
pub const CAP_PING: u32 = 1 << 2;

pub fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(5 + payload.len());
    v.push(kind);
    v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    v.extend_from_slice(payload);
    v
}

pub async fn write_frame<W: AsyncWrite + Unpin>(
    w: &mut W,
    kind: u8,
    payload: &[u8],
) -> io::Result<()> {
    w.write_all(&frame(kind, payload)).await
}

pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<(u8, Vec<u8>)> {
    let mut head = [0u8; 5];
    r.read_exact(&mut head).await?;
    let kind = head[0];
    let len = u32::from_le_bytes([head[1], head[2], head[3], head[4]]) as usize;
    // 防呆：单帧上限 16 MiB
    if len > 16 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut payload = vec![0u8; len];
    if len > 0 {
        r.read_exact(&mut payload).await?;
    }
    Ok((kind, payload))
}

pub fn u16s(payload: &[u8]) -> (u16, u16) {
    let c = u16::from_le_bytes([payload[0], payload[1]]);
    let r = u16::from_le_bytes([payload[2], payload[3]]);
    (c, r)
}

pub fn u32s(payload: &[u8]) -> u32 {
    u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]])
}

pub fn u64s(payload: &[u8]) -> u64 {
    u64::from_le_bytes(payload[..8].try_into().unwrap())
}

pub fn put_u64(v: u64) -> [u8; 8] {
    v.to_le_bytes()
}

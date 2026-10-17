//! 出站写线程：一条**控制优先**的帧队列 + 一条把队列排空的线程。
//!
//! 为什么需要它：会话输出是**多个线程**（每个会话一个）产生的，而管道只有一个。
//! 直接让每个线程各自 `WriteFile` 会把帧写花（字节流上没有帧边界保护），所以要一个独占写端的
//! 线程 + 一个受锁保护的队列。
//!
//! 队列用的是 `ageminal-protocol` 的 [`ControlFirstQueue`]（#65）：**控制面永远先于数据面**
//! 出队——否则持续输出会把 `SessionExited` / 响应这类控制消息饿死（研究稿 §五 记的 WezTerm 翻车）。

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};

use ageminal_protocol::{ControlFirstQueue, Frame, Message};

use crate::{logging, DaemonError};

/// 一条连接的出站口。
#[derive(Debug)]
pub struct Outbound {
    queue: Mutex<ControlFirstQueue>,
    wake: Condvar,
    closed: AtomicBool,
    /// 写失败/退出要立刻落日志——只在 `join` 时记的话，连接可能还挂着，线索就丢了。
    log_path: PathBuf,
}

impl Outbound {
    /// 新建（未关闭）。
    pub fn new(log_path: PathBuf) -> Self {
        Self {
            queue: Mutex::default(),
            wake: Condvar::new(),
            closed: AtomicBool::new(false),
            log_path,
        }
    }

    /// 投一条**控制**消息（响应 / 事件）。
    pub fn send_control(&self, message: &Message) -> Result<(), DaemonError> {
        let frame = message.to_frame().map_err(|error| DaemonError::Protocol {
            reason: error.to_string(),
        })?;
        self.push(frame);
        Ok(())
    }

    /// 投一条**数据**消息（PTY 输出）。
    ///
    /// 连接已经关了就**直接丢掉**——不能让后台会话的输出线程无限往一条死连接里塞。
    pub fn send_data(&self, message: &Message) {
        if let Ok(frame) = message.to_frame() {
            self.push(frame);
        }
    }

    fn push(&self, frame: Frame) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        let mut queue = self.queue.lock().expect("outbound queue poisoned");
        queue.push(frame);
        self.wake.notify_one();
    }

    /// 写线程主体：队列空了就睡，关了就（排空后）退。
    pub fn run(&self, writer: &mut impl Write) -> Result<(), DaemonError> {
        logging::append(&self.log_path, "写线程启动");
        loop {
            let frame = {
                let mut queue = self.queue.lock().expect("outbound queue poisoned");
                while queue.is_empty() {
                    if self.closed.load(Ordering::SeqCst) {
                        return Ok(());
                    }
                    queue = self.wake.wait(queue).expect("outbound wait poisoned");
                }
                queue.pop().expect("非空队列")
            };
            let kind = frame.kind;
            let encoded = frame.encoded_len();
            if let Err(error) = frame.write_to(writer) {
                logging::append(&self.log_path, &format!("写出失败（{kind:?}）：{error}"));
                return Err(match error {
                    ageminal_protocol::FrameError::Io(source) => DaemonError::Io(source),
                    other => DaemonError::Protocol {
                        reason: other.to_string(),
                    },
                });
            }
            // 控制面留痕（数据面太吵，不记）：daemon 的 stdio 是 NUL，日志是唯一线索。
            if !kind.is_raw_payload() {
                logging::append(&self.log_path, &format!("-> {kind:?} {encoded}B"));
            }
        }
    }

    /// 关掉：写线程会先排空已入队的帧再退出；之后的 `push` 一律丢弃。幂等。
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.wake.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ageminal_protocol::{Frame, FrameKind, Message};

    use super::*;

    /// 把写入的字节攒起来，供测试解码。
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl Write for Sink {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("sink").extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// 控制消息要能从写线程里**先**出去，哪怕数据帧先入队；`close` 之后排空再退。
    #[test]
    fn control_frames_leave_before_data_and_close_drains() {
        let outbound = Arc::new(Outbound::new(
            std::env::temp_dir().join("ageminal-outbound-test.log"),
        ));
        let sink = Arc::new(Mutex::new(Vec::new()));

        let writer = {
            let outbound = outbound.clone();
            let sink = sink.clone();
            std::thread::spawn(move || outbound.run(&mut Sink(sink)))
        };

        outbound.send_data(&Message::Data {
            handle: 1,
            seq: 0,
            bytes: b"one".to_vec(),
        });
        outbound.send_data(&Message::Data {
            handle: 1,
            seq: 3,
            bytes: b"two".to_vec(),
        });
        outbound.send_control(&Message::Ping).unwrap();

        // `close` 之后队列里还剩两帧——写线程要先排空才退。
        outbound.close();
        writer.join().unwrap().unwrap();

        let bytes = sink.lock().expect("sink").clone();
        let mut cursor = bytes.as_slice();
        let first = Frame::read_from(&mut cursor).unwrap().unwrap();
        assert_eq!(first.kind, FrameKind::Ping, "控制帧必须先出");
        assert_eq!(
            Frame::read_from(&mut cursor).unwrap().unwrap().kind,
            FrameKind::Data
        );
        assert_eq!(
            Frame::read_from(&mut cursor).unwrap().unwrap().kind,
            FrameKind::Data
        );
        assert!(Frame::read_from(&mut cursor).unwrap().is_none());
    }

    /// 关掉之后投递的消息直接丢弃——不能给死连接攒内存。
    #[test]
    fn pushing_after_close_is_dropped() {
        let outbound = Outbound::new(std::env::temp_dir().join("ageminal-outbound-test.log"));
        outbound.close();
        outbound.send_control(&Message::Ping).unwrap();
        outbound.send_data(&Message::Ping);

        let mut bytes = Vec::new();
        outbound.run(&mut bytes).unwrap();
        assert!(bytes.is_empty(), "关闭后不该再写任何字节");
    }
}

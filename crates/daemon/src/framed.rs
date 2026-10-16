//! 帧读写：daemon 侧对 [`ageminal_protocol`] 的薄封装。
//!
//! 服务端与客户端都经这里收发，**不自己碰字节**（#65 验收 4：两侧只用协议 crate 的类型）。

use std::io::{Read, Write};

use ageminal_protocol::{Frame, FrameError, Message, MessageError};

use crate::DaemonError;

/// 读一条消息。**`Ok(None)` = 对端在帧边界处正常关闭**。
///
/// **不认识的帧类型一律跳过、不断连**（研究稿 §6.1 的兼容策略）——这样加新帧类型不会把旧对端
/// 打死。已知类型但负载是坏 JSON 仍算错误（对方说的是我们懂的协议，只是说坏了）。
pub fn read_message(reader: &mut impl Read) -> Result<Option<Message>, DaemonError> {
    loop {
        let Some(frame) = Frame::read_from(reader).map_err(frame_error)? else {
            return Ok(None);
        };
        match Message::from_frame(&frame) {
            Ok(message) => return Ok(Some(message)),
            Err(MessageError::UnsupportedKind(_)) => continue,
            Err(error) => {
                return Err(DaemonError::Protocol {
                    reason: error.to_string(),
                })
            }
        }
    }
}

/// 编码并整帧写出。
pub fn send(writer: &mut impl Write, message: &Message) -> Result<(), DaemonError> {
    let frame = message.to_frame().map_err(|error| DaemonError::Protocol {
        reason: error.to_string(),
    })?;
    frame.write_to(writer).map_err(frame_error)
}

/// 分帧错误里，**IO 失败**与**协议违规**要分开：前者是链路问题，后者是对端发错了。
fn frame_error(error: FrameError) -> DaemonError {
    match error {
        FrameError::Io(source) => DaemonError::Io(source),
        other => DaemonError::Protocol {
            reason: other.to_string(),
        },
    }
}

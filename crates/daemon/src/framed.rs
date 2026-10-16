//! 帧读写：daemon 侧对 [`ageminal_protocol`] 的薄封装。
//!
//! 服务端与客户端都经这里收发，**不自己碰字节**（#65 验收 4：两侧只用协议 crate 的类型）。

use std::io::{Read, Write};

use ageminal_protocol::{Frame, FrameError, Message};

use crate::DaemonError;

/// 读一条消息。**`Ok(None)` = 对端在帧边界处正常关闭**。
pub fn read_message(reader: &mut impl Read) -> Result<Option<Message>, DaemonError> {
    let Some(frame) = Frame::read_from(reader).map_err(frame_error)? else {
        return Ok(None);
    };
    Message::from_frame(&frame)
        .map_err(|error| DaemonError::Protocol {
            reason: error.to_string(),
        })
        .map(Some)
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

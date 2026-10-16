//! 最小线格式：**一行一条 JSON**（`\n` 结束）。
//!
//! 只服务 #64 的 token 握手 + ping/pong，刻意保持最小。#65（协议层：帧编解码 + 控制优先 +
//! 版本协商）会用**正式帧层取代它**（长度前缀、控制帧 / 数据帧、控制优先队列），届时本模块要么
//! 被替换，要么下沉到 `ageminal-protocol`。
//!
//! 泛型开在 `Read` / `Write` 上，所以 Linux 上也能用 `Cursor` 直接单测编解码——不必起真管道。

use std::io::{self, BufRead, Read, Write};

use serde::de::DeserializeOwned;
use serde::Serialize;

/// 单条消息的行长度上限：挡住「对端发一条超长行打爆内存」。
pub const MAX_LINE_BYTES: u64 = 64 * 1024;

/// 读一条消息。对端关闭连接时返回 [`io::ErrorKind::UnexpectedEof`]。
pub fn read_message<R: BufRead, T: DeserializeOwned>(reader: &mut R) -> io::Result<T> {
    let mut line = String::new();
    let read = reader.by_ref().take(MAX_LINE_BYTES).read_line(&mut line)?;
    if read == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "对端已关闭连接",
        ));
    }
    if !line.ends_with('\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "消息行超过上限或缺少换行",
        ));
    }
    serde_json::from_str(line.trim_end_matches(['\n', '\r']))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// 写一条消息并 flush——握手是请求-应答，不能攒在缓冲里等对端超时。
pub fn write_message<W: Write, T: Serialize>(writer: &mut W, message: &T) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(message)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    bytes.push(b'\n');
    writer.write_all(&bytes)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Cursor};

    use ageminal_protocol::ClientMessage;

    use super::*;

    fn reader(bytes: &[u8]) -> BufReader<Cursor<Vec<u8>>> {
        BufReader::new(Cursor::new(bytes.to_vec()))
    }

    #[test]
    fn round_trips_a_message() {
        let mut bytes = Vec::new();
        write_message(&mut bytes, &ClientMessage::Ping { nonce: 7 }).unwrap();

        let message: ClientMessage = read_message(&mut reader(&bytes)).unwrap();
        assert_eq!(message, ClientMessage::Ping { nonce: 7 });
    }

    #[test]
    fn reads_messages_one_line_at_a_time() {
        let mut bytes = Vec::new();
        write_message(&mut bytes, &ClientMessage::Ping { nonce: 1 }).unwrap();
        write_message(&mut bytes, &ClientMessage::Ping { nonce: 2 }).unwrap();

        let mut reader = reader(&bytes);
        assert_eq!(
            read_message::<_, ClientMessage>(&mut reader).unwrap(),
            ClientMessage::Ping { nonce: 1 }
        );
        assert_eq!(
            read_message::<_, ClientMessage>(&mut reader).unwrap(),
            ClientMessage::Ping { nonce: 2 }
        );
    }

    #[test]
    fn reports_eof_when_peer_closed() {
        let error = read_message::<_, ClientMessage>(&mut reader(b"")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn rejects_an_overlong_line() {
        let bytes = vec![b'x'; MAX_LINE_BYTES as usize + 1];
        let error = read_message::<_, ClientMessage>(&mut reader(&bytes)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn rejects_malformed_json() {
        let error = read_message::<_, ClientMessage>(&mut reader(b"not json\n")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn tolerates_crlf() {
        let bytes = b"{\"type\":\"ping\",\"nonce\":3}\r\n";
        let message: ClientMessage = read_message(&mut reader(bytes)).unwrap();
        assert_eq!(message, ClientMessage::Ping { nonce: 3 });
    }
}

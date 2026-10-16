//! app-core ↔ daemon 的帧协议（Rust 单一来源，见 REQUIREMENTS.md §19）。
//!
//! 分工：本 crate 只放**两边都要认识的消息形状**。
//! - **#64（本阶段）**：token 握手 + `Ping` / `Pong`。传输是**一行一条 JSON**
//!   （见 `ageminal_daemon::wire`），刻意保持最小——它服务的只是「拉起 daemon、确认它还活着」。
//! - **#65**：把它扩成正式帧层（控制帧 / 数据帧、控制优先队列、版本协商的完整语义）。
//!   届时 `ClientMessage` / `ServerMessage` 会跟着长；**线格式变更 = 抬 [`PROTOCOL_VERSION`]**。

use serde::{Deserialize, Serialize};

/// 协议版本。与 app 版本解耦，仅在帧形状变化时递增。
pub const PROTOCOL_VERSION: u32 = 1;

/// app-core（或 agent shim）→ daemon 的消息。
///
/// 握手顺序固定：连接后的**第一条**必须是 [`ClientMessage::Hello`]；通过后才发别的。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// 握手：`token` 取自 daemon 落盘的 `daemon.json`（或会话注入的 `AGEMINAL_TOKEN`）；
    /// `protocol` 是客户端的 [`PROTOCOL_VERSION`]。
    Hello { token: String, protocol: u32 },

    /// 存活探测。`nonce` 原样回显，用来把应答和请求对上（避免复用管道时的错位）。
    Ping { nonce: u64 },
}

/// daemon → 客户端的消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// 握手通过。`protocol` 是 daemon 的版本，客户端据此判断是否继续。
    Welcome {
        protocol: u32,
        pid: u32,
        version: String,
    },

    /// 握手被拒（令牌错 / 版本不匹配）。daemon 发完即断开。
    Rejected { reason: String },

    /// [`ClientMessage::Ping`] 的应答。
    Pong { nonce: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_starts_at_one() {
        assert_eq!(PROTOCOL_VERSION, 1);
    }

    #[test]
    fn client_messages_round_trip() {
        for message in [
            ClientMessage::Hello {
                token: "abc".to_owned(),
                protocol: PROTOCOL_VERSION,
            },
            ClientMessage::Ping { nonce: 42 },
        ] {
            let json = serde_json::to_string(&message).unwrap();
            let back: ClientMessage = serde_json::from_str(&json).unwrap();
            assert_eq!(back, message);
        }
    }

    #[test]
    fn server_messages_round_trip() {
        for message in [
            ServerMessage::Welcome {
                protocol: PROTOCOL_VERSION,
                pid: 7,
                version: "0.0.0".to_owned(),
            },
            ServerMessage::Rejected {
                reason: "令牌不匹配".to_owned(),
            },
            ServerMessage::Pong { nonce: 42 },
        ] {
            let json = serde_json::to_string(&message).unwrap();
            let back: ServerMessage = serde_json::from_str(&json).unwrap();
            assert_eq!(back, message);
        }
    }

    #[test]
    fn message_tag_is_snake_case() {
        let json = serde_json::to_string(&ClientMessage::Ping { nonce: 1 }).unwrap();
        assert_eq!(json, r#"{"type":"ping","nonce":1}"#);
    }
}

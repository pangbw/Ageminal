//! Agent 事件接入的**真进程**集成测试（issue #79）。
//!
//! 这里不启动真正的 `ageminal-notify.exe`（shim 的解析 / 投递有独立单测与真机验收），而是用
//! 第二个协议客户端模拟它：验证 daemon 的**多客户端口**（UI 长连接 + 短连接）与事件广播。

#![cfg(windows)]

mod support;

use std::time::Duration;

use ageminal_daemon::client;
use ageminal_protocol::{AgentEvent, Message};
use support::{Fixture, Spawned};

#[test]
fn a_short_client_can_deliver_an_agent_event_to_the_ui_connection() {
    let fixture = Fixture::new("agent-events");
    let daemon = Spawned::start(&fixture);

    // UI 先占住一条长连接；shim 再连时不能被它卡住（管道实例池）。
    let mut ui = client::connect(&daemon.info, Duration::from_secs(5)).expect("UI 连接");
    let mut notifier =
        client::connect(&daemon.info, Duration::from_secs(5)).expect("通知客户端连接");

    notifier
        .send_agent_event(AgentEvent {
            sid: "wt/main/shell/1".to_owned(),
            incarnation_id: Some("inc-1".to_owned()),
            agent: "claude".to_owned(),
            event: "Stop".to_owned(),
            native: Some("Stop".to_owned()),
            ts: 1_700_000_000_000,
            payload: Some(serde_json::json!({"hook_event_name": "Stop"})),
            truncated: false,
            worktree: Some(r"D:\code\wt".to_owned()),
        })
        .expect("短客户端投递事件");

    let received = ui
        .next_event(Duration::from_secs(5))
        .expect("UI 收事件")
        .expect("UI 应收到 AgentEvent");
    match received {
        Message::AgentEvent(event) => {
            assert_eq!(event.sid, "wt/main/shell/1");
            assert_eq!(event.agent, "claude");
            assert_eq!(event.event, "Stop");
            assert_eq!(event.incarnation_id.as_deref(), Some("inc-1"));
            assert_eq!(
                event
                    .payload
                    .as_ref()
                    .and_then(|payload| payload.get("hook_event_name")),
                Some(&serde_json::Value::String("Stop".to_owned()))
            );
        }
        other => panic!("期待 AgentEvent，收到 {other:?}"),
    }
}

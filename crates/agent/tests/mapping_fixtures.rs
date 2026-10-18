//! 逐事件映射夹具（#80 验收 1）。
//!
//! 夹具只描述「事件 → 状态」，不重复 reducer 的版本 / 防抖测试；后者在
//! `sequences.rs` 与 `reducer.rs` 的 proptest 里。

use ageminal_agent::{AgentReducer, AgentState};
use ageminal_protocol::AgentEvent;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
struct Fixture {
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    agent: String,
    event: String,
    #[serde(default)]
    native: Option<String>,
    #[serde(default)]
    payload: Value,
    expect: String,
}

#[test]
fn every_fixture_event_maps_to_the_expected_state() {
    let fixture: Fixture =
        serde_json::from_str(include_str!("fixtures/events.json")).expect("映射夹具 JSON");
    assert!(!fixture.cases.is_empty());

    for (index, case) in fixture.cases.iter().enumerate() {
        let event = AgentEvent {
            sid: "fixture-session".to_owned(),
            incarnation_id: Some("fixture-inc".to_owned()),
            agent: case.agent.clone(),
            event: case.event.clone(),
            native: case.native.clone(),
            ts: 1_700_000_000_000 + index as u64,
            payload: (!case.payload.is_null()).then(|| case.payload.clone()),
            truncated: false,
            worktree: Some(r"D:\code\fixture".to_owned()),
        };
        let mut reducer = AgentReducer::new("fixture-session");
        let update = reducer.apply_event(&event);

        if case.expect == "ignored" {
            assert!(
                update.is_none(),
                "{}: 期待忽略，实际得到 {:?}",
                case.name,
                reducer.current()
            );
            assert!(reducer.current().is_none(), "{}", case.name);
            continue;
        }

        let expected: AgentState =
            serde_json::from_value(Value::String(case.expect.clone())).expect("合法状态名");
        let update = update.unwrap_or_else(|| panic!("{}: 期待状态，实际被忽略", case.name));
        assert!(update.changed, "{}: 首条事件必须改变状态", case.name);
        assert_eq!(update.current.state, expected, "{}", case.name);
        assert_eq!(update.current.source, ageminal_agent::StatusSource::Native);
        assert_eq!(
            update.current.incarnation_id.as_deref(),
            Some("fixture-inc")
        );
    }
}

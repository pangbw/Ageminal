//! 归约行为夹具：done 常驻、优先级、版本、幂等、防抖、子代理与退出语义（#80）。

use ageminal_agent::{AgentReducer, AgentState, StatusBook, StatusSource, DEBOUNCE_MS};
use ageminal_protocol::{AgentEvent, SessionExited};
use serde_json::{json, Value};

fn event(
    reducer_sid: &str,
    incarnation: &str,
    agent: &str,
    name: &str,
    payload: Value,
    ts: u64,
) -> AgentEvent {
    AgentEvent {
        sid: reducer_sid.to_owned(),
        incarnation_id: Some(incarnation.to_owned()),
        agent: agent.to_owned(),
        event: name.to_owned(),
        native: None,
        ts,
        payload: (!payload.is_null()).then_some(payload),
        truncated: false,
        worktree: None,
    }
}

fn exit(sid: &str, incarnation: &str, code: Option<i32>) -> SessionExited {
    SessionExited {
        sid: sid.to_owned(),
        incarnation_id: incarnation.to_owned(),
        exit_code: code,
    }
}

#[test]
fn done_stays_done_until_new_activity() {
    let mut reducer = AgentReducer::new("s");
    reducer
        .apply_event(&event(
            "s",
            "inc-1",
            "claude",
            "UserPromptSubmit",
            json!({}),
            1_000,
        ))
        .expect("running");
    reducer
        .apply_event(&event("s", "inc-1", "claude", "Stop", json!({}), 2_000))
        .expect("done");
    assert_eq!(
        reducer.current().map(|current| current.state),
        Some(AgentState::Done)
    );

    // 没有事件 = 没有迁移；本模块没有 done→idle 的定时器。
    assert_eq!(
        reducer.current().map(|current| current.state),
        Some(AgentState::Done)
    );

    reducer
        .apply_event(&event(
            "s",
            "inc-1",
            "claude",
            "UserPromptSubmit",
            json!({}),
            9_000,
        ))
        .expect("new turn");
    assert_eq!(
        reducer.current().map(|current| current.state),
        Some(AgentState::Running)
    );
}

#[test]
fn late_done_does_not_regress_a_new_running_turn() {
    let mut reducer = AgentReducer::new("s");
    reducer
        .apply_event(&event(
            "s",
            "inc-1",
            "claude",
            "UserPromptSubmit",
            json!({}),
            2_000,
        ))
        .unwrap();
    assert!(
        reducer
            .apply_event(&event("s", "inc-1", "claude", "Stop", json!({}), 1_500))
            .is_none(),
        "旧时间戳的 Stop 必须被丢弃"
    );
    assert_eq!(
        reducer.current().map(|current| current.state),
        Some(AgentState::Running)
    );
}

#[test]
fn priority_is_exited_error_permission_waiting_running() {
    let mut reducer = AgentReducer::new("s");
    assert_eq!(
        reducer
            .apply_event(&event("s", "inc-1", "claude", "Stop", json!({}), 1_000))
            .map(|update| update.current.state),
        Some(AgentState::Done)
    );
    assert_eq!(
        reducer
            .apply_event(&event(
                "s",
                "inc-1",
                "claude",
                "PermissionRequest",
                json!({}),
                1_001
            ))
            .map(|update| update.current.state),
        Some(AgentState::Permission)
    );
    assert!(
        reducer
            .apply_event(&event(
                "s",
                "inc-1",
                "opencode",
                "question.asked",
                json!({}),
                1_002
            ))
            .is_none(),
        "waiting 不能压过 permission"
    );
    assert_eq!(
        reducer
            .apply_event(&event(
                "s",
                "inc-1",
                "claude",
                "StopFailure",
                json!({"error":"api"}),
                1_003
            ))
            .map(|update| update.current.state),
        Some(AgentState::Error)
    );
    assert!(
        reducer
            .apply_event(&event(
                "s",
                "inc-1",
                "claude",
                "UserPromptSubmit",
                json!({}),
                1_004
            ))
            .is_none(),
        "running 不能压过 error"
    );
    let exited = reducer
        .apply_exit(&exit("s", "inc-1", Some(3)), 1_005)
        .expect("exited 是硬事实");
    assert_eq!(exited.current.state, AgentState::Exited);
}

#[test]
fn non_zero_exit_is_exited_with_code_not_error() {
    let mut reducer = AgentReducer::new("s");
    reducer
        .apply_event(&event(
            "s",
            "inc-1",
            "codex",
            "UserPromptSubmit",
            json!({}),
            1_000,
        ))
        .unwrap();
    let update = reducer
        .apply_exit(&exit("s", "inc-1", Some(7)), 2_000)
        .expect("退出事件");
    assert_eq!(update.current.state, AgentState::Exited);
    assert_eq!(update.current.exit_code, Some(7));
    assert_eq!(update.current.source, StatusSource::Process);
    assert_ne!(update.current.state, AgentState::Error);
}

#[test]
fn error_reason_survives_the_transition_to_exited() {
    let mut reducer = AgentReducer::new("s");
    reducer
        .apply_event(&event(
            "s",
            "inc-1",
            "claude",
            "StopFailure",
            json!({"error": "overloaded"}),
            1_000,
        ))
        .unwrap();
    assert_eq!(
        reducer
            .current()
            .and_then(|current| current.reason.as_deref()),
        Some("overloaded")
    );
    let update = reducer
        .apply_exit(&exit("s", "inc-1", None), 2_000)
        .unwrap();
    assert_eq!(update.current.state, AgentState::Exited);
    assert_eq!(update.current.reason.as_deref(), Some("overloaded"));
}

#[test]
fn stable_identity_makes_duplicate_events_idempotent() {
    let mut reducer = AgentReducer::new("s");
    let first = event(
        "s",
        "inc-1",
        "claude",
        "PreToolUse",
        json!({"tool_use_id": "tool-1"}),
        1_000,
    );
    let update = reducer.apply_event(&first).expect("first");
    assert_eq!(update.current.state, AgentState::Running);

    let duplicate = event(
        "s",
        "inc-1",
        "claude",
        "PreToolUse",
        json!({"tool_use_id": "tool-1"}),
        9_000,
    );
    assert!(
        reducer.apply_event(&duplicate).is_none(),
        "稳定 id 必须幂等"
    );
    assert_eq!(
        reducer.current().map(|current| current.state),
        Some(AgentState::Running)
    );
}

#[test]
fn anonymous_events_are_debounced_for_300_ms() {
    let mut reducer = AgentReducer::new("s");
    let waiting = event("s", "inc-1", "opencode", "question.asked", json!({}), 1_000);
    assert!(reducer.apply_event(&waiting).is_some());

    let within = event(
        "s",
        "inc-1",
        "opencode",
        "question.asked",
        json!({}),
        1_000 + DEBOUNCE_MS,
    );
    assert!(
        reducer.apply_event(&within).is_none(),
        "同一指纹在防抖窗口内只处理一次"
    );

    let after = event(
        "s",
        "inc-1",
        "opencode",
        "question.asked",
        json!({}),
        1_000 + DEBOUNCE_MS + 1,
    );
    let update = reducer.apply_event(&after).expect("窗口外可再次处理");
    assert_eq!(update.current.state, AgentState::Waiting);
    assert!(!update.changed, "重复的相同状态只刷新版本");
}

#[test]
fn logical_sequence_wins_even_when_the_timestamp_is_older() {
    let mut reducer = AgentReducer::new("s");
    reducer
        .apply_event(&event(
            "s",
            "inc-1",
            "pi",
            "agent_settled",
            json!({"seq": 10}),
            2_000,
        ))
        .unwrap();
    assert_eq!(
        reducer.current().map(|current| current.state),
        Some(AgentState::Done)
    );

    assert!(
        reducer
            .apply_event(&event(
                "s",
                "inc-1",
                "pi",
                "agent_start",
                json!({"seq": 9}),
                2_001,
            ))
            .is_none(),
        "逻辑序号更小的事件是迟到的"
    );
    assert_eq!(
        reducer
            .apply_event(&event(
                "s",
                "inc-1",
                "pi",
                "agent_start",
                json!({"seq": 11}),
                1_000,
            ))
            .map(|update| update.current.state),
        Some(AgentState::Running),
        "逻辑序号更大时即使时间戳更旧也应生效"
    );
}

#[test]
fn subagent_lineage_is_filtered_by_default() {
    let mut reducer = AgentReducer::new("s");
    reducer
        .apply_event(&event(
            "s",
            "inc-1",
            "opencode",
            "session.status",
            json!({"type":"busy"}),
            1_000,
        ))
        .unwrap();

    // 根会话创建子会话；谱系事件本身不展示。
    assert!(
        reducer
            .apply_event(&event(
                "s",
                "inc-1",
                "opencode",
                "session.created",
                json!({"sessionID": "child-1", "parentID": "root"}),
                1_001,
            ))
            .is_none(),
        "谱系事件本身不展示"
    );
    // 子会话的授权事件不能把根会话渲染成 permission。
    assert!(reducer
        .apply_event(&event(
            "s",
            "inc-1",
            "opencode",
            "permission.asked",
            json!({"sessionID": "child-1"}),
            1_002,
        ))
        .is_none());
    assert_eq!(
        reducer.current().map(|current| current.state),
        Some(AgentState::Running)
    );

    // 根会话自己的授权仍然生效。
    reducer
        .apply_event(&event(
            "s",
            "inc-1",
            "opencode",
            "permission.asked",
            json!({"sessionID": "root"}),
            1_003,
        ))
        .unwrap();
    assert_eq!(
        reducer.current().map(|current| current.state),
        Some(AgentState::Permission)
    );
}

#[test]
fn a_new_incarnation_resets_the_old_generation() {
    let mut reducer = AgentReducer::new("s");
    reducer
        .apply_event(&event(
            "s",
            "inc-1",
            "claude",
            "UserPromptSubmit",
            json!({}),
            1_000,
        ))
        .unwrap();
    reducer
        .apply_event(&event("s", "inc-2", "claude", "Stop", json!({}), 2_000))
        .unwrap();
    assert_eq!(
        reducer.current().map(|current| current.state),
        Some(AgentState::Done)
    );
    assert_eq!(
        reducer
            .current()
            .and_then(|current| current.incarnation_id.as_deref()),
        Some("inc-2")
    );
}

#[test]
fn status_book_keeps_sessions_apart_and_is_sorted() {
    let mut book = StatusBook::default();
    book.apply_event(&event("z", "inc-z", "claude", "Stop", json!({"id":"z"}), 1))
        .unwrap();
    book.apply_event(&event(
        "a",
        "inc-a",
        "pi",
        "agent_settled",
        json!({"id":"a"}),
        2,
    ))
    .unwrap();

    let snapshots = book.snapshots();
    assert_eq!(
        snapshots
            .iter()
            .map(|snapshot| snapshot.sid.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "z"]
    );
    assert_eq!(
        book.snapshot("a").map(|snapshot| snapshot.state),
        Some(AgentState::Done)
    );

    book.apply_exit(&exit("z", "inc-z", Some(1)), 3)
        .expect("z exited");
    assert_eq!(
        book.snapshot("z").and_then(|snapshot| snapshot.exit_code),
        Some(1)
    );
    assert_eq!(
        book.remove("a").map(|snapshot| snapshot.sid),
        Some("a".to_owned())
    );
    assert!(!book.contains("a"));
}

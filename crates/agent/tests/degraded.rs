//! 降级模式夹具（#81）：进程树识别 + 输出静默 + T_interrupt。

use ageminal_agent::{
    AgentReducer, AgentState, AgentStatusSnapshot, DegradedObservation, StatusSource, T_IDLE_MS,
    T_INTERRUPT_MS,
};
use ageminal_protocol::{AgentEvent, SessionExited};
use serde_json::json;

fn observation(
    process_agent_id: Option<&str>,
    configured_agent_id: Option<&str>,
    last_output_ms: u64,
    now_ms: u64,
) -> DegradedObservation {
    DegradedObservation {
        root_alive: true,
        process_agent_id: process_agent_id.map(str::to_owned),
        configured_agent_id: configured_agent_id.map(str::to_owned),
        incarnation_id: Some("inc-1".to_owned()),
        last_output_ms,
        now_ms,
    }
}

fn native(name: &str, payload: serde_json::Value, ts: u64) -> AgentEvent {
    AgentEvent {
        sid: "s".to_owned(),
        incarnation_id: Some("inc-1".to_owned()),
        agent: "claude".to_owned(),
        event: name.to_owned(),
        native: None,
        ts,
        payload: Some(payload),
        truncated: false,
        worktree: None,
    }
}

fn status(reducer: &AgentReducer) -> &AgentStatusSnapshot {
    reducer.current().expect("status")
}

#[test]
fn manual_agent_is_detected_as_degraded_running() {
    let mut reducer = AgentReducer::new("s");
    let update = reducer
        .observe_degraded(&observation(Some("claude"), None, 1_000, 1_000))
        .expect("识别到进程后创建状态");
    assert_eq!(update.current.state, AgentState::Running);
    assert_eq!(update.current.source, StatusSource::Heuristic);
    assert!(update.current.degraded);
    assert_eq!(update.current.agent.as_deref(), Some("claude"));
}

#[test]
fn idle_needs_two_polls_after_the_silence_threshold() {
    let mut reducer = AgentReducer::new("s");
    reducer
        .observe_degraded(&observation(Some("claude"), None, 1_000, 1_000))
        .unwrap();
    let at_threshold = 1_000 + T_IDLE_MS;
    assert!(
        reducer
            .observe_degraded(&observation(Some("claude"), None, 1_000, at_threshold))
            .is_none(),
        "第一次达到阈值只记 streak"
    );
    let update = reducer
        .observe_degraded(&observation(Some("claude"), None, 1_000, at_threshold + 1))
        .expect("连续两次达到阈值后转 idle");
    assert_eq!(update.current.state, AgentState::Idle);
    assert!(update.current.degraded);
}

#[test]
fn output_immediately_returns_degraded_idle_to_running() {
    let mut reducer = AgentReducer::new("s");
    reducer
        .observe_degraded(&observation(Some("claude"), None, 1_000, 1_000))
        .unwrap();
    assert!(
        reducer
            .observe_degraded(&observation(Some("claude"), None, 1_000, 1_000 + T_IDLE_MS))
            .is_none(),
        "第一次达到阈值只记 streak"
    );
    reducer
        .observe_degraded(&observation(
            Some("claude"),
            None,
            1_000,
            1_000 + T_IDLE_MS + 1,
        ))
        .unwrap();
    assert_eq!(status(&reducer).state, AgentState::Idle);

    let update = reducer
        .note_output(1_000 + T_IDLE_MS + 2)
        .expect("输出立即回 running");
    assert_eq!(update.current.state, AgentState::Running);
    assert_eq!(update.current.source, StatusSource::Heuristic);
}

#[test]
fn configured_agent_without_a_detected_process_is_not_declared_exited() {
    let mut reducer = AgentReducer::new("s");
    reducer
        .observe_degraded(&observation(None, Some("claude"), 1_000, 1_000))
        .expect("一等入口的命令可能还在启动");
    assert_eq!(status(&reducer).state, AgentState::Running);

    reducer.observe_degraded(&observation(None, Some("claude"), 1_000, 2_000));
    assert_eq!(
        status(&reducer).state,
        AgentState::Running,
        "从未识别到 agent 进程时不能把启动期误判成 exited"
    );
}

#[test]
fn a_detected_agent_disappearing_becomes_exited() {
    let mut reducer = AgentReducer::new("s");
    reducer
        .observe_degraded(&observation(Some("codex"), None, 1_000, 1_000))
        .unwrap();
    let update = reducer
        .observe_degraded(&observation(None, None, 2_000, 2_000))
        .expect("agent 进程消失");
    assert_eq!(update.current.state, AgentState::Exited);
    assert_eq!(update.current.source, StatusSource::Heuristic);
    assert!(update.current.degraded);
}

#[test]
fn a_heuristic_exit_is_refined_by_the_real_process_exit_code() {
    let mut reducer = AgentReducer::new("s");
    reducer
        .observe_degraded(&observation(Some("claude"), None, 1_000, 1_000))
        .unwrap();
    reducer
        .observe_degraded(&observation(None, None, 2_000, 2_000))
        .unwrap();
    assert_eq!(status(&reducer).state, AgentState::Exited);

    let update = reducer
        .apply_exit(
            &SessionExited {
                sid: "s".to_owned(),
                incarnation_id: "inc-1".to_owned(),
                exit_code: Some(7),
            },
            3_000,
        )
        .expect("真实退出码应补全");
    assert_eq!(update.current.state, AgentState::Exited);
    assert_eq!(update.current.source, StatusSource::Process);
    assert_eq!(update.current.exit_code, Some(7));
    assert!(!update.current.degraded);
}

#[test]
fn native_mode_does_not_accept_heuristic_done_or_waiting() {
    let mut reducer = AgentReducer::new("s");
    reducer
        .apply_event(&native("UserPromptSubmit", json!({}), 1_000))
        .expect("native running");
    reducer.observe_degraded(&observation(Some("claude"), Some("claude"), 1_000, 2_000));
    assert_eq!(status(&reducer).state, AgentState::Running);
    assert_eq!(status(&reducer).source, StatusSource::Native);
    assert!(!status(&reducer).degraded);
}

#[test]
fn native_running_falls_back_to_idle_after_t_interrupt_and_recovers_on_output() {
    let mut reducer = AgentReducer::new("s");
    reducer
        .apply_event(&native("UserPromptSubmit", json!({}), 1_000))
        .unwrap();
    let before = 1_000 + T_INTERRUPT_MS - 1;
    assert!(
        reducer
            .observe_degraded(&observation(Some("claude"), Some("claude"), 1_000, before))
            .map_or(true, |update| update.current.state == AgentState::Running),
        "未超过阈值不能降级"
    );
    let update = reducer
        .observe_degraded(&observation(
            Some("claude"),
            Some("claude"),
            1_000,
            before + 1,
        ))
        .expect("超过 T_interrupt 后降为 idle");
    assert_eq!(update.current.state, AgentState::Idle);
    assert_eq!(update.current.source, StatusSource::Heuristic);
    assert!(update.current.degraded);

    let update = reducer
        .note_output(before + 2)
        .expect("输出立即恢复原生 running");
    assert_eq!(update.current.state, AgentState::Running);
    assert_eq!(update.current.source, StatusSource::Native);
    assert!(!update.current.degraded);
}

#[test]
fn a_new_native_event_ends_the_fallback() {
    let mut reducer = AgentReducer::new("s");
    reducer
        .apply_event(&native("UserPromptSubmit", json!({}), 1_000))
        .unwrap();
    reducer
        .observe_degraded(&observation(
            Some("claude"),
            Some("claude"),
            1_000,
            1_000 + T_INTERRUPT_MS + 1,
        ))
        .unwrap();
    assert_eq!(status(&reducer).state, AgentState::Idle);

    let update = reducer
        .apply_event(&native("Stop", json!({}), 1_000 + T_INTERRUPT_MS + 2))
        .expect("原生事件覆盖 fallback");
    assert_eq!(update.current.state, AgentState::Done);
    assert_eq!(update.current.source, StatusSource::Native);
    assert!(!update.current.degraded);
}

#[test]
fn native_agent_process_disappearance_is_still_exited() {
    let mut reducer = AgentReducer::new("s");
    reducer
        .apply_event(&native("UserPromptSubmit", json!({}), 1_000))
        .unwrap();
    assert!(
        reducer
            .observe_degraded(&observation(Some("claude"), Some("claude"), 1_000, 2_000))
            .is_none(),
        "首次识别到进程不改变 running 状态"
    );
    let update = reducer
        .observe_degraded(&observation(None, Some("claude"), 2_000, 3_000))
        .expect("进程消失是硬事实");
    assert_eq!(update.current.state, AgentState::Exited);
    assert_eq!(update.current.source, StatusSource::Heuristic);
}

#[test]
fn root_disappearance_marks_heuristic_exited() {
    let mut reducer = AgentReducer::new("s");
    reducer
        .observe_degraded(&observation(Some("pi"), None, 1_000, 1_000))
        .unwrap();
    let update = reducer
        .observe_degraded(&DegradedObservation {
            root_alive: false,
            process_agent_id: None,
            configured_agent_id: None,
            incarnation_id: Some("inc-1".to_owned()),
            last_output_ms: 1_000,
            now_ms: 2_000,
        })
        .expect("根进程消失");
    assert_eq!(update.current.state, AgentState::Exited);
    assert!(update.current.degraded);
}

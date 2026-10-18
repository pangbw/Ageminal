//! 原生事件 → 统一状态的逐 agent 映射（#80）。
//!
//! 映射只依赖 **事件名 + 原生 JSON 负载**，不解析终端输出、不读时钟。来源依据见
//! `docs/research/agent-event-mapping.md` §三；`Notification` / `session.status` 等需要二次
//! 判定的事件在这里拆开。
//!
//! 未知事件返回 [`Mapping::Ignore`]，不会创建状态；子代理事件的过滤在 reducer 里做（需要
//! 维护 opencode 的父子谱系）。

use ageminal_protocol::AgentEvent;
use serde_json::Value;

use crate::AgentState;

/// 映射成功的状态事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MappedEvent {
    pub state: AgentState,
    pub seq: u64,
    pub reason: Option<String>,
}

/// 一条原生事件的映射结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Mapping {
    /// 不改变状态（未知事件 / 通知类型不关心 / 谱系事件等）。
    Ignore,
    /// 改变状态。
    State(MappedEvent),
}

/// 把协议事件映射成统一状态。
///
/// `event.event` 是 shim 的主事件名；`event.native` 是可选的原始别名。先试主事件名，再试
/// 别名，避免某个注入物把「统一事件名」和「原生事件名」放反。
pub(crate) fn map_event(event: &AgentEvent) -> Mapping {
    let payload = event.payload.as_ref().unwrap_or(&Value::Null);
    let primary = event.event.trim();
    if let Some(mapped) = map_name(primary, payload) {
        return mapped;
    }
    if let Some(native) = event.native.as_deref().map(str::trim) {
        if !native.is_empty() && !native.eq_ignore_ascii_case(primary) {
            if let Some(mapped) = map_name(native, payload) {
                return mapped;
            }
        }
    }
    Mapping::Ignore
}

fn map_name(raw: &str, payload: &Value) -> Option<Mapping> {
    let name = raw.to_ascii_lowercase();
    let state =
        match name.as_str() {
            // —— 回合 / 活动 ——
            "sessionstart" | "session_start" => {
                if text_any(payload, &[&["source"]])
                    .is_some_and(|source| source.eq_ignore_ascii_case("resume"))
                {
                    AgentState::Idle
                } else {
                    AgentState::Running
                }
            }
            "userpromptsubmit"
            | "user_prompt_submit"
            | "before_agent_start"
            | "agent_start"
            | "turn_start"
            | "pretooluse"
            | "pre_tool_use"
            | "posttooluse"
            | "post_tool_use"
            | "posttoolusefailure"
            | "post_tool_use_failure"
            | "posttoolbatch"
            | "post_tool_batch"
            | "tool_call"
            | "tool_execution_start"
            | "tool_execution_update"
            | "tool_execution_end"
            | "message_start"
            | "message_update"
            | "message_end"
            | "agent_end"
            | "session.compacted"
            | "session_compacted"
            | "queue_update"
            | "auto_retry_start"
            | "summarization_retry_start"
            | "summarization_retry_end" => AgentState::Running,

            // —— 回合结束 ——
            "stop"
            | "agent-turn-complete"
            | "agent_turn_complete"
            | "agent_completed"
            | "agent_settled"
            | "session.idle"
            | "session_idle" => AgentState::Done,

            // —— 等用户 / 授权 ——
            "permissionrequest" | "permission_request" | "permission.asked"
            | "permission_asked" => AgentState::Permission,
            "permissiondenied" | "permission_denied" | "permission.replied"
            | "permission_replied" => AgentState::Running,
            "question.asked" | "question_asked" => AgentState::Waiting,
            "question.replied" | "question_replied" | "question.rejected" | "question_rejected" => {
                AgentState::Running
            }
            "interrupt" => AgentState::Waiting,
            "ui_prompt_start" | "ui_prompt_started" => {
                if asks_for_permission(payload) {
                    AgentState::Permission
                } else {
                    AgentState::Waiting
                }
            }
            "ui_prompt_end" | "ui_prompt_ended" => AgentState::Running,
            "tool_result" => {
                if bool_any(payload, &[&["isError"], &["is_error"]]) == Some(true) {
                    AgentState::Error
                } else {
                    AgentState::Running
                }
            }

            // —— 错误 ——
            "stopfailure"
            | "stop_failure"
            | "session.error"
            | "session_error"
            | "extension_error"
            | "session_compact_failed" => AgentState::Error,
            "auto_retry_end" => {
                if bool_any(payload, &[&["success"]]) == Some(false)
                    || text_any(payload, &[&["finalError"], &["final_error"]]).is_some()
                {
                    AgentState::Error
                } else {
                    AgentState::Running
                }
            }

            // —— 进程 / 会话结束 ——
            "sessionend" | "session_end" | "session_shutdown" => AgentState::Exited,

            // —— 需要看负载的事件 ——
            "notification" => notification_state(payload)?,
            "session.status" | "session_status" | "sessionstatus" => session_status(payload)?,

            // —— 子代理：默认不展示；reducer 还会按谱系过滤 ——
            "subagentstart" | "subagent_start" | "subagentstop" | "subagent_stop"
            | "subagent.started" | "subagent.finished" => return None,
            "session.created" | "session_created" | "session.updated" | "session_updated" => {
                return None
            }

            // 某些自定义集成可能直接发统一状态名。
            "running" => AgentState::Running,
            "waiting" => AgentState::Waiting,
            "permission" => AgentState::Permission,
            "done" => AgentState::Done,
            "idle" => AgentState::Idle,
            "error" => AgentState::Error,
            "exited" => AgentState::Exited,

            _ if is_activity_name(&name) => AgentState::Running,
            _ => return None,
        };

    let reason = if state == AgentState::Error {
        error_reason(payload)
    } else {
        None
    };
    Some(Mapping::State(MappedEvent {
        state,
        seq: sequence_of(payload),
        reason,
    }))
}

fn notification_state(payload: &Value) -> Option<AgentState> {
    let kind = text_any(
        payload,
        &[
            &["notification_type"],
            &["notificationType"],
            &["notification", "type"],
            &["notification", "notification_type"],
            &["type"],
        ],
    )?;
    let kind = kind.to_ascii_lowercase();
    Some(match kind.as_str() {
        "permission_prompt" | "permission-prompt" => AgentState::Permission,
        "agent_needs_input"
        | "agent-needs-input"
        | "elicitation_dialog"
        | "elicitation-dialog"
        | "elicitation_url_dialog"
        | "elicitation-url-dialog" => AgentState::Waiting,
        "idle_prompt" | "idle-prompt" => AgentState::Idle,
        "agent_completed" | "agent-completed" => {
            if has_failure(payload) {
                AgentState::Error
            } else {
                AgentState::Done
            }
        }
        _ => return None,
    })
}

fn session_status(payload: &Value) -> Option<AgentState> {
    let status = text_any(
        payload,
        &[
            &["status", "type"],
            &["properties", "type"],
            &["properties", "status", "type"],
            &["data", "status", "type"],
            &["data", "type"],
            &["type"],
            &["status"],
            &["properties", "status"],
            &["data", "status"],
        ],
    )?;
    Some(match status.to_ascii_lowercase().as_str() {
        "busy" | "running" | "working" | "retry" | "retrying" => AgentState::Running,
        "idle" | "done" | "completed" => AgentState::Done,
        "error" | "failed" | "failure" => AgentState::Error,
        _ => return None,
    })
}

fn asks_for_permission(payload: &Value) -> bool {
    if bool_any(
        payload,
        &[
            &["permission"],
            &["requiresPermission"],
            &["requires_permission"],
            &["isPermission"],
            &["is_permission"],
        ],
    ) == Some(true)
    {
        return true;
    }
    text_any(payload, &[&["kind"], &["type"]])
        .map(|kind| {
            matches!(
                kind.to_ascii_lowercase().as_str(),
                "permission" | "approval" | "authorization" | "authorisation" | "gate"
            )
        })
        .unwrap_or(false)
}

fn has_failure(payload: &Value) -> bool {
    payload.get("failed") == Some(&Value::Bool(true))
        || payload.get("is_error") == Some(&Value::Bool(true))
        || payload.get("isError") == Some(&Value::Bool(true))
        || payload.get("success") == Some(&Value::Bool(false))
        || text_any(payload, &[&["error"], &["failure"]]).is_some()
}

fn error_reason(payload: &Value) -> Option<String> {
    let raw = text_any(
        payload,
        &[
            &["error", "message"],
            &["error"],
            &["message"],
            &["reason"],
            &["error_type"],
            &["errorType"],
        ],
    )?;
    Some(truncate(raw, 240))
}

fn is_activity_name(name: &str) -> bool {
    name.starts_with("message.")
        || name.starts_with("message_")
        || name.starts_with("file.")
        || name.starts_with("file_")
        || name.starts_with("todo.")
        || name.starts_with("todo_")
        || name.starts_with("tool.")
        || name.starts_with("tool_")
}

fn sequence_of(payload: &Value) -> u64 {
    for path in [
        &["seq"][..],
        &["sequence"],
        &["event_seq"],
        &["eventSeq"],
        &["event", "seq"],
        &["properties", "seq"],
        &["properties", "sequence"],
        &["info", "seq"],
    ] {
        if let Some(number) = integer_at(payload, path) {
            return number;
        }
    }
    0
}

/// 稳定事件 id：用于「同一事件重复投递」的幂等。
///
/// 只收 agent 真正提供的标识；没有时返回 `None`，reducer 会退回到短时防抖。
pub(crate) fn stable_identity(event: &AgentEvent) -> Option<String> {
    let payload = event.payload.as_ref().unwrap_or(&Value::Null);
    let mut parts = vec![
        event.agent.trim().to_ascii_lowercase(),
        event.event.trim().to_ascii_lowercase(),
    ];
    for path in [
        &["id"][..],
        &["event_id"],
        &["eventId"],
        &["uuid"],
        &["event_uuid"],
        &["eventUuid"],
        &["tool_use_id"],
        &["toolUseId"],
        &["requestID"],
        &["requestId"],
        &["turn_id"],
        &["turnId"],
        &["thread-id"],
        &["thread_id"],
        &["threadId"],
        &["seq"],
        &["sequence"],
        &["event", "id"],
    ] {
        if let Some(value) = text_at(payload, path) {
            parts.push(value);
        }
    }
    (parts.len() > 2).then(|| parts.join("|"))
}

/// 无稳定 id 时用于防抖的指纹（不包含时间戳）。
pub(crate) fn debounce_key(event: &AgentEvent) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    event.agent.hash(&mut hasher);
    event.event.hash(&mut hasher);
    event.native.hash(&mut hasher);
    event
        .payload
        .as_ref()
        .map(|payload| payload.to_string())
        .unwrap_or_default()
        .hash(&mut hasher);
    hasher.finish()
}

/// opencode 的父子谱系：`session.created/updated` 的 `parentID` + 子会话 id。
pub(crate) fn lineage_child(event: &AgentEvent) -> Option<(String, String)> {
    let name = event.event.trim().to_ascii_lowercase();
    if !matches!(
        name.as_str(),
        "session.created" | "session_created" | "session.updated" | "session_updated"
    ) {
        return None;
    }
    let payload = event.payload.as_ref()?;
    let parent = parent_session_id(payload)?;
    let child = session_id(payload).or_else(|| {
        text_any(
            payload,
            &[
                &["id"],
                &["info", "id"],
                &["data", "id"],
                &["properties", "id"],
            ],
        )
    })?;
    Some((child, parent))
}

/// 事件负载里的 agent 内部会话 id（不是 Ageminal 的终端 sid）。
pub(crate) fn event_session_id(event: &AgentEvent) -> Option<String> {
    event.payload.as_ref().and_then(session_id)
}

/// 是否明确是子代理事件（事件名或 Claude 的 `agent_id` / `agent_type`）。
pub(crate) fn is_subagent_event(event: &AgentEvent) -> bool {
    let name = event.event.trim().to_ascii_lowercase();
    if name.contains("subagent") || name.starts_with("subagent.") || name.starts_with("subagent_") {
        return true;
    }
    let Some(payload) = event.payload.as_ref() else {
        return false;
    };
    text_any(
        payload,
        &[&["agent_id"], &["agentId"], &["agent_type"], &["agentType"]],
    )
    .is_some()
        || parent_session_id(payload).is_some()
}

fn session_id(payload: &Value) -> Option<String> {
    text_any(
        payload,
        &[
            &["sessionID"],
            &["sessionId"],
            &["session_id"],
            &["session", "id"],
            &["data", "sessionID"],
            &["data", "sessionId"],
            &["properties", "sessionID"],
            &["properties", "sessionId"],
        ],
    )
}

fn parent_session_id(payload: &Value) -> Option<String> {
    text_any(
        payload,
        &[
            &["parentID"],
            &["parentId"],
            &["parent_id"],
            &["session", "parentID"],
            &["session", "parentId"],
            &["info", "parentID"],
            &["info", "parentId"],
            &["info", "parent_id"],
            &["data", "parentID"],
            &["data", "parentId"],
            &["data", "parent_id"],
            &["properties", "parentID"],
            &["properties", "parentId"],
            &["properties", "parent_id"],
        ],
    )
}

fn text_any(value: &Value, paths: &[&[&str]]) -> Option<String> {
    paths.iter().find_map(|path| text_at(value, path))
}

fn text_at(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for part in path {
        current = current.get(*part)?;
    }
    as_text(current)
}

fn as_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_owned())
        }
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(boolean) => Some(boolean.to_string()),
        // 错误对象 / 嵌套详情保留成紧凑 JSON，交给 reducer 截断。
        Value::Object(_) | Value::Array(_) => serde_json::to_string(value).ok(),
        Value::Null => None,
    }
}

fn bool_any(value: &Value, paths: &[&[&str]]) -> Option<bool> {
    paths.iter().find_map(|path| {
        let mut current = value;
        for part in *path {
            current = current.get(*part)?;
        }
        current.as_bool()
    })
}

fn integer_at(value: &Value, path: &[&str]) -> Option<u64> {
    let mut current = value;
    for part in path {
        current = current.get(*part)?;
    }
    current.as_u64().or_else(|| current.as_str()?.parse().ok())
}

fn truncate(mut text: String, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text;
    }
    text = text.chars().take(limit).collect();
    text.push('…');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(name: &str, payload: Value) -> AgentEvent {
        AgentEvent {
            sid: "s".to_owned(),
            incarnation_id: Some("inc".to_owned()),
            agent: "claude".to_owned(),
            event: name.to_owned(),
            native: None,
            ts: 1,
            payload: Some(payload),
            truncated: false,
            worktree: None,
        }
    }

    #[test]
    fn notification_types_are_disambiguated() {
        for (kind, expected) in [
            ("permission_prompt", AgentState::Permission),
            ("agent_needs_input", AgentState::Waiting),
            ("elicitation_dialog", AgentState::Waiting),
            ("elicitation_url_dialog", AgentState::Waiting),
            ("idle_prompt", AgentState::Idle),
            ("agent_completed", AgentState::Done),
        ] {
            let mapping = map_event(&event(
                "Notification",
                serde_json::json!({ "notification_type": kind }),
            ));
            assert_eq!(
                mapping,
                Mapping::State(MappedEvent {
                    state: expected,
                    seq: 0,
                    reason: None
                }),
                "{kind}"
            );
        }
    }

    #[test]
    fn opencode_status_and_question_events_are_split() {
        assert_eq!(
            map_event(&event(
                "session.status",
                serde_json::json!({ "type": "busy" })
            )),
            Mapping::State(MappedEvent {
                state: AgentState::Running,
                seq: 0,
                reason: None
            })
        );
        assert_eq!(
            map_event(&event(
                "session.status",
                serde_json::json!({ "type": "idle" })
            )),
            Mapping::State(MappedEvent {
                state: AgentState::Done,
                seq: 0,
                reason: None
            })
        );
        assert_eq!(
            map_event(&event("question.asked", serde_json::json!({}))),
            Mapping::State(MappedEvent {
                state: AgentState::Waiting,
                seq: 0,
                reason: None
            })
        );
    }

    #[test]
    fn pi_prompt_distinguishes_permission_from_waiting() {
        assert_eq!(
            map_event(&event(
                "ui_prompt_start",
                serde_json::json!({ "kind": "confirm" })
            )),
            Mapping::State(MappedEvent {
                state: AgentState::Waiting,
                seq: 0,
                reason: None
            })
        );
        assert_eq!(
            map_event(&event(
                "ui_prompt_start",
                serde_json::json!({ "kind": "permission" })
            )),
            Mapping::State(MappedEvent {
                state: AgentState::Permission,
                seq: 0,
                reason: None
            })
        );
    }

    #[test]
    fn unknown_and_subagent_events_are_ignored() {
        assert_eq!(
            map_event(&event("SomethingNew", serde_json::json!({}))),
            Mapping::Ignore
        );
        assert_eq!(
            map_event(&event("SubagentStart", serde_json::json!({}))),
            Mapping::Ignore
        );
    }

    #[test]
    fn stable_identity_collects_agent_identifiers() {
        let mut value = event(
            "PreToolUse",
            serde_json::json!({ "tool_use_id": "tool-7", "seq": 3 }),
        );
        value.agent = "claude".to_owned();
        assert_eq!(
            stable_identity(&value).as_deref(),
            Some("claude|pretooluse|tool-7|3")
        );
        assert!(stable_identity(&event("Stop", serde_json::json!({}))).is_none());
    }
}

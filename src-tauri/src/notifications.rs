//! 桌面通知（#82）。
//!
//! 只在**状态真正进入** `waiting` / `permission` / `done` / `error` 时发一次；`done → idle`
//! 不再通知。通知由 Rust 侧发送，WebView 隐藏 / 页面重载不影响；点击 toast 后的窗口激活由
//! Windows / Tauri notification 插件完成（#48 真机结论：无精确跳转回调，点击只激活窗口）。

use std::sync::Mutex;

use ageminal_agent::{AgentState, StatusUpdate};
use tauri::Manager;
use tauri_plugin_notification::NotificationExt;

use crate::agents;
use crate::storage::Store;

/// 这条状态迁移是否应发通知。
///
/// 四类目标状态各通知一次；相同状态（即使来源从 heuristic 变成 native）不重复，
/// `running` / `idle` / `exited` 不发。
pub fn should_notify(update: &StatusUpdate) -> bool {
    update.changed
        && update.previous != Some(update.current.state)
        && matches!(
            update.current.state,
            AgentState::Waiting | AgentState::Permission | AgentState::Done | AgentState::Error
        )
}

/// 中文 / 英文状态短语。语言来自 `settings.general.language`；未设置按中文（产品默认）。
fn state_label(state: AgentState, english: bool) -> &'static str {
    match (state, english) {
        (AgentState::Waiting, false) => "等待输入",
        (AgentState::Permission, false) => "需要授权",
        (AgentState::Done, false) => "已完成",
        (AgentState::Error, false) => "出错",
        (AgentState::Waiting, true) => "Waiting for input",
        (AgentState::Permission, true) => "Permission required",
        (AgentState::Done, true) => "Done",
        (AgentState::Error, true) => "Error",
        (AgentState::Running, false) => "运行中",
        (AgentState::Idle, false) => "空闲",
        (AgentState::Exited, false) => "已退出",
        (AgentState::Running, true) => "Running",
        (AgentState::Idle, true) => "Idle",
        (AgentState::Exited, true) => "Exited",
    }
}

fn state_body(state: AgentState, english: bool) -> &'static str {
    match (state, english) {
        (AgentState::Waiting, false) => "agent 正在等待你的输入",
        (AgentState::Permission, false) => "agent 正在等待你的授权",
        (AgentState::Done, false) => "本轮任务已完成",
        (AgentState::Error, false) => "agent 执行出错，请查看终端",
        (AgentState::Waiting, true) => "The agent is waiting for your input.",
        (AgentState::Permission, true) => "The agent needs your permission.",
        (AgentState::Done, true) => "The turn is complete.",
        (AgentState::Error, true) => "The agent reported an error. Check the terminal.",
        _ => "",
    }
}

fn english(app: &tauri::AppHandle) -> bool {
    app.try_state::<Mutex<Store>>()
        .and_then(|store| {
            store
                .lock()
                .ok()
                .and_then(|guard| guard.settings().general.language.clone())
        })
        .is_some_and(|language| language.to_ascii_lowercase().starts_with("en"))
}

fn agent_name(app: &tauri::AppHandle, id: Option<&str>) -> String {
    let Some(id) = id else {
        return "Agent".to_owned();
    };
    app.try_state::<Mutex<Store>>()
        .and_then(|store| {
            store
                .lock()
                .ok()
                .and_then(|guard| agents::resolve(guard.settings(), id))
        })
        .map(|config| config.name)
        .unwrap_or_else(|| id.to_owned())
}

/// 发一条通知；失败只记录，不影响状态/终端。
pub fn notify(app: &tauri::AppHandle, update: &StatusUpdate) {
    if !should_notify(update) {
        return;
    }
    let english = english(app);
    let name = agent_name(app, update.current.agent.as_deref());
    let label = state_label(update.current.state, english);
    let body = if update.current.degraded {
        if english {
            "Status is incomplete (degraded mode)."
        } else {
            "状态不完整（降级模式）。"
        }
    } else {
        state_body(update.current.state, english)
    };
    let title = format!("{name} · {label}");
    if let Err(error) = app.notification().builder().title(title).body(body).show() {
        tracing::warn!("[notification] 发送失败：{error}");
    }
}

#[cfg(test)]
mod tests {
    use ageminal_agent::{AgentStatusSnapshot, StatusSource};

    use super::*;

    fn update(previous: Option<AgentState>, state: AgentState, changed: bool) -> StatusUpdate {
        StatusUpdate {
            previous,
            current: AgentStatusSnapshot {
                sid: "s".to_owned(),
                agent: Some("claude".to_owned()),
                incarnation_id: Some("inc-1".to_owned()),
                state,
                source: StatusSource::Native,
                degraded: false,
                timestamp_ms: 1,
                seq: 0,
                exit_code: None,
                reason: None,
            },
            changed,
        }
    }

    #[test]
    fn four_attention_states_notify_once_on_transition() {
        for state in [
            AgentState::Waiting,
            AgentState::Permission,
            AgentState::Done,
            AgentState::Error,
        ] {
            assert!(
                should_notify(&update(None, state, true)),
                "{}",
                state.as_str()
            );
            assert!(should_notify(&update(
                Some(AgentState::Running),
                state,
                true
            )));
            assert!(!should_notify(&update(Some(state), state, true)));
            assert!(!should_notify(&update(
                Some(AgentState::Running),
                state,
                false
            )));
        }
    }

    #[test]
    fn running_idle_and_exited_do_not_notify() {
        for state in [AgentState::Running, AgentState::Idle, AgentState::Exited] {
            assert!(
                !should_notify(&update(None, state, true)),
                "{}",
                state.as_str()
            );
        }
    }

    #[test]
    fn labels_cover_both_languages() {
        assert_eq!(state_label(AgentState::Waiting, false), "等待输入");
        assert_eq!(state_label(AgentState::Permission, false), "需要授权");
        assert_eq!(state_label(AgentState::Done, false), "已完成");
        assert_eq!(state_label(AgentState::Error, false), "出错");
        assert_eq!(state_label(AgentState::Running, false), "运行中");
        assert_eq!(state_label(AgentState::Idle, false), "空闲");
        assert_eq!(state_label(AgentState::Exited, false), "已退出");
        assert_eq!(
            state_label(AgentState::Permission, true),
            "Permission required"
        );
        assert_eq!(state_label(AgentState::Done, true), "Done");
    }
}

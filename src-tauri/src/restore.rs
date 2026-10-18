//! 启动恢复与四象限对账（issue #78）。
//!
//! `state.json` 里的页签骨架 + launch spec 是**用户意图**；daemon 的 `ListSessions` 是
//! **进程事实**。启动时两边按 `sid` 求交，绝不从「list 里没看到」直接推断进程死亡：
//!
//! | 持久页签 | daemon 存活 | 动作 |
//! | --- | --- | --- |
//! | 有 | 有 | [`RestoreAction::Attach`]：live reattach |
//! | 有 | 无，agent 且有 `agentSessionId` | [`RestoreAction::Resume`]：CLI resume |
//! | 有 | 无，其它终端的 | [`RestoreAction::Rebuild`]：重建 shell / 重新起 agent |
//! | 无 | 有 | [`RestoreAction::Adopt`]：收养孤儿会话，避免状态文件丢失后会话不可见 |
//! | 无 | 无 | 不产生页签 |
//!
//! 对账本身是纯函数 [`reconcile`]；命令层只负责读写 `state.json` 与询问 daemon。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;

use crate::path_text;
use crate::storage::{LaunchSpec, SavedTab, SavedTabKind, Store, WorktreeTabs};
use crate::terminal::{TerminalService, TerminalSession};

/// 前端保存页签骨架时的页签类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum TabsTabKind {
    /// 终端。
    Terminal,
    /// 文件预览。
    Preview,
}

/// 前端保存的一条页签。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TabsTab {
    /// 类型。
    pub kind: TabsTabKind,
    /// 显示标题。
    pub name: String,
    /// 终端 `sid`；预览为 `None`。
    #[serde(default)]
    pub sid: Option<String>,
    /// 预览绝对路径；终端为 `None`。
    #[serde(default)]
    pub path: Option<String>,
    /// 终端 cwd；预览为 `None`。
    #[serde(default)]
    pub cwd: Option<String>,
    /// 一等入口 agent id；普通终端为 `None`。
    #[serde(default)]
    pub agent_id: Option<String>,
}

/// 一个 worktree 的页签骨架（前端 → Rust）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TabsWorktree {
    /// worktree 根。
    pub root: String,
    /// 激活页签 id。
    #[serde(default)]
    pub active: Option<String>,
    /// 页签顺序。
    #[serde(default)]
    pub tabs: Vec<TabsTab>,
}

/// 对账前的存活会话（daemon 事实）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSession {
    /// 逻辑会话 id。
    pub sid: String,
    /// 会话 cwd（daemon 权威）。
    pub cwd: String,
    /// 本次 spawn 世代。
    pub incarnation_id: String,
    /// 一等入口 agent id。
    pub agent_id: Option<String>,
}

impl From<TerminalSession> for LiveSession {
    fn from(session: TerminalSession) -> Self {
        Self {
            sid: session.sid,
            cwd: session.cwd,
            incarnation_id: session.incarnation_id,
            agent_id: session.agent_id,
        }
    }
}

/// 对账结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum RestoreAction {
    /// 两侧都有：live reattach。
    Attach,
    /// 仅 UI 有，agent 有可恢复的会话 id：CLI resume。
    Resume,
    /// 仅 UI 有：纯 shell 或没有 agentSessionId 的 agent，重新建立会话。
    Rebuild,
    /// 仅 daemon 有：收养孤儿。
    Adopt,
    /// 文件预览，不需要会话。
    Preview,
}

/// 对账后要交回前端的一条页签。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RestoredTab {
    /// 类型。
    pub kind: TabsTabKind,
    /// 显示标题；孤儿可能为空，由前端填缺省名。
    pub name: String,
    /// 终端 `sid`。
    #[serde(default)]
    pub sid: Option<String>,
    /// 预览路径。
    #[serde(default)]
    pub path: Option<String>,
    /// 终端 cwd。
    #[serde(default)]
    pub cwd: Option<String>,
    /// 一等入口 agent id。
    #[serde(default)]
    pub agent_id: Option<String>,
    /// 对账动作。
    pub action: RestoreAction,
}

/// 对账后的一个 worktree 分组。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RestoredWorktree {
    /// worktree 根。
    pub root: String,
    /// 激活页签 id；无效时回落到最后一个页签。
    #[serde(default)]
    pub active: Option<String>,
    /// 页签顺序。
    pub tabs: Vec<RestoredTab>,
}

/// 纯对账核心。
///
/// 输入顺序 = 持久化顺序；孤儿追加到同 cwd 的分组，没有分组时新建。
pub fn reconcile(
    saved: &[WorktreeTabs],
    specs: &[LaunchSpec],
    live: &[LiveSession],
) -> Vec<RestoredWorktree> {
    let specs: HashMap<&str, &LaunchSpec> =
        specs.iter().map(|spec| (spec.sid.as_str(), spec)).collect();
    let live_by_sid: HashMap<&str, &LiveSession> = live
        .iter()
        .map(|session| (session.sid.as_str(), session))
        .collect();
    let mut consumed = HashSet::new();
    let mut restored: Vec<RestoredWorktree> = Vec::new();

    for group in saved {
        let mut tabs = Vec::new();
        for tab in &group.tabs {
            match tab.kind {
                SavedTabKind::Preview => {
                    let Some(path) = tab.path.as_ref() else {
                        continue;
                    };
                    tabs.push(RestoredTab {
                        kind: TabsTabKind::Preview,
                        name: tab.name.clone(),
                        sid: None,
                        path: Some(path_text(path)),
                        cwd: None,
                        agent_id: None,
                        action: RestoreAction::Preview,
                    });
                }
                SavedTabKind::Terminal => {
                    let Some(sid) = tab.sid.as_deref() else {
                        continue;
                    };
                    let spec = specs.get(sid).copied();
                    let live_session = live_by_sid.get(sid).copied();
                    if live_session.is_some() {
                        consumed.insert(sid.to_owned());
                    }
                    let agent_id = live_session
                        .and_then(|session| session.agent_id.clone())
                        .or_else(|| spec.and_then(|spec| spec.agent_id.clone()))
                        .or_else(|| tab.agent_id.clone());
                    let action = match live_session {
                        Some(_) => RestoreAction::Attach,
                        None if agent_id.is_some()
                            && spec.is_some_and(|spec| spec.agent_session_id.is_some()) =>
                        {
                            RestoreAction::Resume
                        }
                        None => RestoreAction::Rebuild,
                    };
                    let cwd = live_session
                        .map(|session| session.cwd.clone())
                        .or_else(|| tab.cwd.as_ref().map(|path| path_text(path)))
                        .or_else(|| spec.map(|spec| path_text(&spec.cwd)));
                    tabs.push(RestoredTab {
                        kind: TabsTabKind::Terminal,
                        name: tab.name.clone(),
                        sid: Some(sid.to_owned()),
                        path: None,
                        cwd,
                        agent_id,
                        action,
                    });
                }
            }
        }

        if tabs.is_empty() {
            continue;
        }
        let active = resolve_active(group.active.clone(), &tabs);
        restored.push(RestoredWorktree {
            root: path_text(&group.root),
            active,
            tabs,
        });
    }

    // 仅 daemon 有：收养。按 cwd 并回已有分组；否则新建一个分组。
    for session in live {
        if consumed.contains(&session.sid) {
            continue;
        }
        let tab = RestoredTab {
            kind: TabsTabKind::Terminal,
            name: String::new(),
            sid: Some(session.sid.clone()),
            path: None,
            cwd: Some(session.cwd.clone()),
            agent_id: session.agent_id.clone(),
            action: RestoreAction::Adopt,
        };
        if let Some(group) = restored.iter_mut().find(|group| group.root == session.cwd) {
            group.tabs.push(tab);
            if group.active.is_none() {
                group.active = Some(session.sid.clone());
            }
        } else {
            restored.push(RestoredWorktree {
                root: session.cwd.clone(),
                active: Some(session.sid.clone()),
                tabs: vec![tab],
            });
        }
    }

    restored
}

/// 激活项不存在时回落到最后一个页签。
fn resolve_active(active: Option<String>, tabs: &[RestoredTab]) -> Option<String> {
    let ids: HashSet<&str> = tabs.iter().filter_map(tab_id).collect();
    if let Some(active) = active.filter(|active| ids.contains(active.as_str())) {
        return Some(active);
    }
    tabs.last().and_then(|tab| tab_id(tab).map(str::to_owned))
}

fn tab_id(tab: &RestoredTab) -> Option<&str> {
    match tab.kind {
        TabsTabKind::Terminal => tab.sid.as_deref(),
        TabsTabKind::Preview => tab.path.as_deref(),
    }
}

/// 保存前端页签骨架；未知字段从旧状态按 root / tab id 合并保留。
#[tauri::command]
#[specta::specta]
pub fn save_tabs(
    store: State<'_, Mutex<Store>>,
    worktrees: Vec<TabsWorktree>,
) -> Result<(), String> {
    let mut next = to_saved(&worktrees);
    let mut store = store.lock().map_err(|_| "状态锁已被污染".to_owned())?;
    let previous = store.state().tabs.clone();
    merge_extra(&mut next, &previous);
    let live_sids: HashSet<String> = next
        .iter()
        .flat_map(|group| group.tabs.iter())
        .filter_map(|tab| tab.sid.clone())
        .collect();
    store
        .update_state(
            |state| {
                state.tabs = next;
                // 页签已不存在的 launch spec 没有恢复价值；关页签时已清理，这里兜底。
                state
                    .launch_specs
                    .retain(|spec| live_sids.contains(&spec.sid));
            },
            Instant::now(),
        )
        .map_err(|error| error.to_string())
}

/// 启动对账：读持久骨架 + 问 daemon → 纯函数 → 回写 incarnation / 占位 spec。
#[tauri::command]
#[specta::specta]
pub async fn restore_tabs(
    store: State<'_, Mutex<Store>>,
    terminal: State<'_, TerminalService>,
) -> Result<Vec<RestoredWorktree>, String> {
    let (saved, specs) = {
        let store = store.lock().map_err(|_| "状态锁已被污染".to_owned())?;
        (
            store.state().tabs.clone(),
            store.state().launch_specs.clone(),
        )
    };
    let service = terminal.inner().clone();
    let live = match tauri::async_runtime::spawn_blocking(move || service.list()).await {
        Ok(Ok(sessions)) => sessions
            .into_iter()
            .map(LiveSession::from)
            .collect::<Vec<_>>(),
        Ok(Err(error)) => {
            // list 失败 ≠ 进程死亡：按「仅 UI 有」恢复，真正建会话前 terminal_open 还会再查一次。
            tracing::warn!("[restore] 枚举 daemon 会话失败，按持久骨架恢复：{error}");
            Vec::new()
        }
        Err(error) => {
            tracing::warn!("[restore] 枚举会话的线程失败：{error}");
            Vec::new()
        }
    };
    let restored = reconcile(&saved, &specs, &live);

    let mut store = store.lock().map_err(|_| "状态锁已被污染".to_owned())?;
    store
        .update_state(
            |state| {
                merge_live_state(state, &live, &restored);
            },
            Instant::now(),
        )
        .map_err(|error| error.to_string())?;
    Ok(restored)
}

/// 更新 agentSessionId（#79 的事件通道会调用）。
///
/// 只在 launch spec 已存在时更新；未知 `sid` 返回错误，避免静默丢事件。
#[tauri::command]
#[specta::specta]
pub fn terminal_set_agent_session(
    store: State<'_, Mutex<Store>>,
    sid: String,
    agent_session_id: Option<String>,
) -> Result<(), String> {
    if agent_session_id
        .as_deref()
        .is_some_and(|id| id.contains(['\r', '\n']))
    {
        return Err("agent 会话 id 不能包含换行".to_owned());
    }
    let mut store = store.lock().map_err(|_| "状态锁已被污染".to_owned())?;
    let mut found = false;
    store
        .update_state(
            |state| {
                if let Some(spec) = state.launch_specs.iter_mut().find(|spec| spec.sid == sid) {
                    spec.agent_session_id = agent_session_id
                        .as_deref()
                        .map(str::trim)
                        .filter(|id| !id.is_empty())
                        .map(str::to_owned);
                    spec.updated_at_ms = Some(crate::now_ms());
                    found = true;
                }
            },
            Instant::now(),
        )
        .map_err(|error| error.to_string())?;
    if found {
        Ok(())
    } else {
        Err(format!("未知会话：{sid}"))
    }
}

fn to_saved(worktrees: &[TabsWorktree]) -> Vec<WorktreeTabs> {
    worktrees
        .iter()
        .filter_map(|group| {
            if group.root.trim().is_empty() {
                return None;
            }
            let tabs: Vec<SavedTab> = group
                .tabs
                .iter()
                .filter_map(|tab| match tab.kind {
                    TabsTabKind::Terminal => {
                        let sid = tab.sid.as_deref()?.trim();
                        if sid.is_empty() {
                            return None;
                        }
                        Some(SavedTab {
                            kind: SavedTabKind::Terminal,
                            name: tab.name.clone(),
                            sid: Some(sid.to_owned()),
                            path: None,
                            cwd: Some(PathBuf::from(tab.cwd.as_deref().unwrap_or(&group.root))),
                            agent_id: tab.agent_id.clone(),
                            extra: Default::default(),
                        })
                    }
                    TabsTabKind::Preview => {
                        let path = tab.path.as_deref()?.trim();
                        if path.is_empty() {
                            return None;
                        }
                        Some(SavedTab {
                            kind: SavedTabKind::Preview,
                            name: tab.name.clone(),
                            sid: None,
                            path: Some(PathBuf::from(path)),
                            cwd: None,
                            agent_id: None,
                            extra: Default::default(),
                        })
                    }
                })
                .collect();
            if tabs.is_empty() {
                return None;
            }
            Some(WorktreeTabs {
                root: PathBuf::from(&group.root),
                active: group.active.clone(),
                tabs,
                extra: Default::default(),
            })
        })
        .collect()
}

fn merge_extra(next: &mut [WorktreeTabs], previous: &[WorktreeTabs]) {
    for group in next {
        let Some(old_group) = previous.iter().find(|old| old.root == group.root) else {
            continue;
        };
        group.extra = old_group.extra.clone();
        for tab in &mut group.tabs {
            let Some(key) = saved_tab_key(tab) else {
                continue;
            };
            if let Some(old_tab) = old_group
                .tabs
                .iter()
                .find(|old| saved_tab_key(old) == Some(key))
            {
                tab.extra = old_tab.extra.clone();
            }
        }
    }
}

fn saved_tab_key(tab: &SavedTab) -> Option<&str> {
    match tab.kind {
        SavedTabKind::Terminal => tab.sid.as_deref(),
        SavedTabKind::Preview => tab.path.as_deref().and_then(Path::to_str),
    }
}

/// 把 daemon 的世代 / cwd 回写到 launch spec，并给没有 spec 的恢复页签补占位。
fn merge_live_state(
    state: &mut crate::storage::AppState,
    live: &[LiveSession],
    restored: &[RestoredWorktree],
) {
    for session in live {
        if let Some(spec) = state
            .launch_specs
            .iter_mut()
            .find(|spec| spec.sid == session.sid)
        {
            spec.cwd = PathBuf::from(&session.cwd);
            spec.incarnation_id = Some(session.incarnation_id.clone());
            if spec.agent_id.is_none() {
                spec.agent_id = session.agent_id.clone();
            }
            spec.updated_at_ms = Some(crate::now_ms());
        } else {
            state.launch_specs.push(LaunchSpec {
                sid: session.sid.clone(),
                cwd: PathBuf::from(&session.cwd),
                shell: None,
                env: Vec::new(),
                command: None,
                program: None,
                args: None,
                agent_id: session.agent_id.clone(),
                agent_session_id: None,
                incarnation_id: Some(session.incarnation_id.clone()),
                updated_at_ms: Some(crate::now_ms()),
                extra: Default::default(),
            });
        }
    }

    for group in restored {
        for tab in &group.tabs {
            let Some(sid) = tab.sid.as_deref() else {
                continue;
            };
            if state.launch_specs.iter().any(|spec| spec.sid == sid) {
                continue;
            }
            state.launch_specs.push(LaunchSpec {
                sid: sid.to_owned(),
                cwd: PathBuf::from(tab.cwd.as_deref().unwrap_or(&group.root)),
                shell: None,
                env: Vec::new(),
                command: None,
                program: None,
                args: None,
                agent_id: tab.agent_id.clone(),
                agent_session_id: None,
                incarnation_id: None,
                updated_at_ms: Some(crate::now_ms()),
                extra: Default::default(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use super::*;

    fn tab(sid: &str, name: &str, agent: Option<&str>) -> SavedTab {
        SavedTab {
            kind: SavedTabKind::Terminal,
            name: name.to_owned(),
            sid: Some(sid.to_owned()),
            path: None,
            cwd: Some(PathBuf::from(r"D:\code\wt")),
            agent_id: agent.map(str::to_owned),
            extra: Map::new(),
        }
    }

    fn preview(path: &str) -> SavedTab {
        SavedTab {
            kind: SavedTabKind::Preview,
            name: "README.md".to_owned(),
            sid: None,
            path: Some(PathBuf::from(path)),
            cwd: None,
            agent_id: None,
            extra: Map::new(),
        }
    }

    fn group(active: Option<&str>, tabs: Vec<SavedTab>) -> WorktreeTabs {
        WorktreeTabs {
            root: PathBuf::from(r"D:\code\wt"),
            active: active.map(str::to_owned),
            tabs,
            extra: Map::new(),
        }
    }

    fn spec(sid: &str, agent: Option<&str>, agent_session: Option<&str>) -> LaunchSpec {
        LaunchSpec {
            sid: sid.to_owned(),
            cwd: PathBuf::from(r"D:\code\wt"),
            shell: None,
            env: Vec::new(),
            command: Some("claude".to_owned()),
            program: Some("claude".to_owned()),
            args: None,
            agent_id: agent.map(str::to_owned),
            agent_session_id: agent_session.map(str::to_owned),
            incarnation_id: Some("inc-1".to_owned()),
            updated_at_ms: None,
            extra: Map::new(),
        }
    }

    fn live(sid: &str) -> LiveSession {
        LiveSession {
            sid: sid.to_owned(),
            cwd: r"D:\code\wt".to_owned(),
            incarnation_id: "inc-2".to_owned(),
            agent_id: Some("claude".to_owned()),
        }
    }

    #[test]
    fn four_quadrants_and_actions() {
        let saved = vec![
            group(
                Some("t-live"),
                vec![
                    tab("t-live", "live", None),
                    tab("t-shell", "shell", None),
                    tab("t-resume", "resume", Some("claude")),
                    tab("t-agent", "agent", Some("claude")),
                    preview(r"D:\code\wt\README.md"),
                ],
            ),
            group(None, vec![tab("t-dead-live", "orphan-source", None)]),
        ];
        let specs = vec![
            spec("t-live", None, None),
            spec("t-shell", None, None),
            spec("t-resume", Some("claude"), Some("agent-42")),
            spec("t-agent", Some("claude"), None),
        ];
        let live = vec![live("t-live"), live("t-dead-live")];
        let restored = reconcile(&saved, &specs, &live);
        let first = &restored[0];
        let actions: Vec<RestoreAction> = first.tabs.iter().map(|tab| tab.action).collect();
        assert_eq!(
            actions,
            vec![
                RestoreAction::Attach,
                RestoreAction::Rebuild,
                RestoreAction::Resume,
                RestoreAction::Rebuild,
                RestoreAction::Preview,
            ]
        );
        assert_eq!(first.active.as_deref(), Some("t-live"));
        assert_eq!(first.tabs[2].agent_id.as_deref(), Some("claude"));

        // t-dead-live 在持久化分组里，但 daemon 仍活着：Attach，而不是孤儿。
        assert_eq!(restored[1].tabs[0].action, RestoreAction::Attach);
    }

    #[test]
    fn daemon_only_session_is_adopted_under_its_cwd() {
        let saved = vec![group(None, vec![tab("t1", "terminal", None)])];
        let mut orphan_session = live("orphan");
        orphan_session.cwd = r"D:\code\other".to_owned();
        let restored = reconcile(&saved, &[], &[orphan_session]);
        assert_eq!(restored.len(), 2);
        let orphan = &restored[1];
        assert_eq!(orphan.root, r"D:\code\other");
        assert_eq!(orphan.active.as_deref(), Some("orphan"));
        assert_eq!(orphan.tabs[0].action, RestoreAction::Adopt);
        assert_eq!(orphan.tabs[0].sid.as_deref(), Some("orphan"));
    }

    #[test]
    fn active_falls_back_to_last_tab_and_preview_ids_are_paths() {
        let saved = vec![group(
            Some("missing"),
            vec![tab("t1", "one", None), preview(r"D:\code\wt\README.md")],
        )];
        let restored = reconcile(&saved, &[], &[]);
        assert_eq!(restored[0].active.as_deref(), Some(r"D:\code\wt\README.md"));
    }

    #[test]
    fn empty_groups_are_dropped() {
        let saved = vec![group(Some("x"), Vec::new())];
        assert!(reconcile(&saved, &[], &[]).is_empty());
    }

    #[test]
    fn tabs_are_converted_and_invalid_rows_dropped() {
        let input = vec![TabsWorktree {
            root: r"D:\code\wt".to_owned(),
            active: Some("t1".to_owned()),
            tabs: vec![
                TabsTab {
                    kind: TabsTabKind::Terminal,
                    name: "term".to_owned(),
                    sid: Some("t1".to_owned()),
                    path: None,
                    cwd: Some(r"D:\code\wt".to_owned()),
                    agent_id: None,
                },
                TabsTab {
                    kind: TabsTabKind::Terminal,
                    name: "bad".to_owned(),
                    sid: None,
                    path: None,
                    cwd: None,
                    agent_id: None,
                },
                TabsTab {
                    kind: TabsTabKind::Preview,
                    name: "README.md".to_owned(),
                    sid: None,
                    path: Some(r"D:\code\wt\README.md".to_owned()),
                    cwd: None,
                    agent_id: None,
                },
            ],
        }];
        let saved = to_saved(&input);
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].tabs.len(), 2);
        assert_eq!(saved[0].tabs[0].sid.as_deref(), Some("t1"));
        assert_eq!(saved[0].tabs[1].kind, SavedTabKind::Preview);
    }
}

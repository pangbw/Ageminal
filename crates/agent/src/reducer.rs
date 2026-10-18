//! 状态归约：版本化、优先级、幂等、防抖与子代理过滤（#80）。
//!
//! 归约器是**纯内存**的：不读时钟、不做 I/O。事件 timestamp 由 shim 带入；进程退出的
//! timestamp 由调用方传入。这样夹具与 proptest 完全可重复。
//!
//! # 版本
//!
//! 每个快照带 `timestamp_ms` + `seq`。比较规则：
//! - 两边都有逻辑 `seq` 时，**seq 优先**；seq 相同才看时间；
//! - 只有一边有 seq 时，有 seq 的一边视为更可靠（除非它的时间明显更旧？不——按契约
//!   「逻辑序优先」，直接以 seq 为准）；
//! - 两边都无 seq 时用时间戳；
//! - `exited` 是硬事实，绕过版本直接获胜（但不会覆盖已经 exited 的状态）。
//!
//! # 优先级
//!
//! `exited > error > permission > waiting > running`；`done` / `idle` 不能把 `running`
//! 打回。`done` 不会自动迁移到 `idle`——本模块没有时钟，也没有 done→idle 的定时器。
//!
//! # 幂等 / 防抖 / 子代理
//!
//! - 有稳定 id 的事件（tool_use_id / turn-id / event id / seq…）重复投递时永远忽略；
//! - 没有稳定 id 的事件，用「agent + event + payload」指纹做 **300 ms** 防抖；
//! - 子代理事件（Claude `agent_id` / Codex `Subagent*`）与 opencode 子会话（由
//!   `session.created` 的 `parentID` 建谱系）默认不进入根会话状态。

use std::collections::{HashMap, HashSet};

use ageminal_protocol::{AgentEvent, SessionExited};

use crate::mapping::{self, Mapping};
use crate::{AgentState, AgentStatusSnapshot, StatusSource, StatusUpdate};

/// 没有稳定 id 的同源事件在这个窗口内只处理一次。
pub const DEBOUNCE_MS: u64 = 300;

/// 稳定 id 的保留上限；超过后按时间清理，避免长会话内存无限增长。
const MAX_STABLE_IDS: usize = 4096;

/// 稳定 id 的保留时长；超过这个时间没有再次出现的 id 可以丢弃。
const STABLE_ID_RETENTION_MS: u64 = 10 * 60 * 1000;

/// 单个 agent 会话的归约器。
#[derive(Debug)]
pub struct AgentReducer {
    sid: String,
    incarnation_id: Option<String>,
    current: Option<AgentStatusSnapshot>,
    stable_seen: HashMap<String, u64>,
    debounce_seen: HashMap<u64, u64>,
    subagent_sessions: HashSet<String>,
}

impl AgentReducer {
    /// 建一个会话归约器。
    pub fn new(sid: impl Into<String>) -> Self {
        Self {
            sid: sid.into(),
            incarnation_id: None,
            current: None,
            stable_seen: HashMap::new(),
            debounce_seen: HashMap::new(),
            subagent_sessions: HashSet::new(),
        }
    }

    /// 当前快照。
    pub fn current(&self) -> Option<&AgentStatusSnapshot> {
        self.current.as_ref()
    }

    /// 归约一条 agent 原生事件。
    ///
    /// 未知事件、子代理事件、重复事件返回 `None`。返回 `Some` 时 `changed` 可能为
    /// `false`（同一状态的新活动事件只刷新版本）。
    pub fn apply_event(&mut self, event: &AgentEvent) -> Option<StatusUpdate> {
        if event.sid != self.sid {
            return None;
        }
        self.begin_incarnation(event.incarnation_id.as_deref());
        self.observe_lineage(event);
        if mapping::is_subagent_event(event)
            || mapping::event_session_id(event)
                .is_some_and(|session| self.subagent_sessions.contains(&session))
        {
            return None;
        }
        let Mapping::State(mapped) = mapping::map_event(event) else {
            return None;
        };
        if self.is_duplicate(event) {
            return None;
        }
        self.merge(Observation {
            state: mapped.state,
            source: StatusSource::Native,
            timestamp_ms: event.ts,
            seq: mapped.seq,
            agent: Some(event.agent.clone()),
            reason: mapped.reason,
            exit_code: None,
        })
    }

    /// 归约进程退出事实。
    ///
    /// `timestamp_ms` 由调用方提供（app-core 用收到 `SessionExited` 的时刻）。非零退出码
    /// 只写进 `exit_code`，状态仍是 `exited`，不会擅自改成 `error`。
    pub fn apply_exit(&mut self, exit: &SessionExited, timestamp_ms: u64) -> Option<StatusUpdate> {
        if exit.sid != self.sid {
            return None;
        }
        if let Some(current) = self.incarnation_id.as_deref() {
            if current != exit.incarnation_id {
                // 迟到的旧世代退出事件；当前世代还在，不能把它标死。
                return None;
            }
        }
        if self
            .current
            .as_ref()
            .is_some_and(|current| current.state.is_terminal())
        {
            return None;
        }
        self.incarnation_id = Some(exit.incarnation_id.clone());
        let previous = self.current.as_ref().map(|current| current.state);
        let current = AgentStatusSnapshot {
            sid: self.sid.clone(),
            agent: self
                .current
                .as_ref()
                .and_then(|current| current.agent.clone()),
            incarnation_id: self.incarnation_id.clone(),
            state: AgentState::Exited,
            source: StatusSource::Process,
            timestamp_ms,
            seq: self
                .current
                .as_ref()
                .map(|current| current.seq)
                .unwrap_or(0),
            exit_code: exit.exit_code,
            reason: self
                .current
                .as_ref()
                .and_then(|current| current.reason.clone()),
        };
        let changed = previous != Some(AgentState::Exited)
            || self
                .current
                .as_ref()
                .is_some_and(|old| old.exit_code != current.exit_code);
        self.current = Some(current.clone());
        Some(StatusUpdate {
            previous,
            current,
            changed,
        })
    }

    /// 新一次 spawn（同 sid 复用）会重置所有旧世代的幂等 / 谱系 / 状态。
    fn begin_incarnation(&mut self, incarnation_id: Option<&str>) {
        let Some(incarnation_id) = incarnation_id.map(str::trim).filter(|id| !id.is_empty()) else {
            return;
        };
        if self.incarnation_id.as_deref() == Some(incarnation_id) {
            return;
        }
        // daemon 已按世代过滤迟到事件；这里只在确实收到新世代时重置。
        self.incarnation_id = Some(incarnation_id.to_owned());
        self.current = None;
        self.stable_seen.clear();
        self.debounce_seen.clear();
        self.subagent_sessions.clear();
    }

    /// opencode 的子会话谱系：记录 child → parent，后续 child 的状态事件直接丢。
    fn observe_lineage(&mut self, event: &AgentEvent) {
        if let Some((child, _parent)) = mapping::lineage_child(event) {
            self.subagent_sessions.insert(child);
        }
    }

    fn is_duplicate(&mut self, event: &AgentEvent) -> bool {
        let now = event.ts;
        if let Some(identity) = mapping::stable_identity(event) {
            if self.stable_seen.contains_key(&identity) {
                return true;
            }
            if self.stable_seen.len() >= MAX_STABLE_IDS {
                let cutoff = now.saturating_sub(STABLE_ID_RETENTION_MS);
                self.stable_seen.retain(|_, seen| *seen >= cutoff);
                // 极端情况下（10 分钟内 4096 个不同事件）整批清掉，宁可多处理一次也不无界。
                if self.stable_seen.len() >= MAX_STABLE_IDS {
                    self.stable_seen.clear();
                }
            }
            self.stable_seen.insert(identity, now);
            return false;
        }

        let key = mapping::debounce_key(event);
        if let Some(previous) = self.debounce_seen.get(&key) {
            if now.saturating_sub(*previous) <= DEBOUNCE_MS {
                return true;
            }
        }
        self.debounce_seen.insert(key, now);
        false
    }

    fn merge(&mut self, incoming: Observation) -> Option<StatusUpdate> {
        let previous = self.current.as_ref();
        if !accept(previous, &incoming) {
            return None;
        }

        let previous_state = previous.map(|current| current.state);
        let current = AgentStatusSnapshot {
            sid: self.sid.clone(),
            agent: incoming
                .agent
                .or_else(|| previous.and_then(|current| current.agent.clone())),
            incarnation_id: self.incarnation_id.clone(),
            state: incoming.state,
            source: incoming.source,
            timestamp_ms: incoming.timestamp_ms,
            seq: if incoming.seq > 0 {
                incoming.seq
            } else {
                previous.map(|current| current.seq).unwrap_or(0)
            },
            exit_code: incoming.exit_code,
            reason: if incoming.state == AgentState::Exited {
                incoming
                    .reason
                    .or_else(|| previous.and_then(|current| current.reason.clone()))
            } else {
                incoming.reason
            },
        };
        let changed = previous_state != Some(current.state)
            || previous.is_some_and(|old| old.exit_code != current.exit_code);
        self.current = Some(current.clone());
        Some(StatusUpdate {
            previous: previous_state,
            current,
            changed,
        })
    }
}

/// 一个 app-core 生命周期内的所有会话状态。
#[derive(Debug, Default)]
pub struct StatusBook {
    sessions: HashMap<String, AgentReducer>,
}

impl StatusBook {
    /// 归约一条事件；未知事件 / 重复事件返回 `None`。
    pub fn apply_event(&mut self, event: &AgentEvent) -> Option<StatusUpdate> {
        if event.sid.trim().is_empty() {
            return None;
        }
        self.sessions
            .entry(event.sid.clone())
            .or_insert_with(|| AgentReducer::new(event.sid.clone()))
            .apply_event(event)
    }

    /// 归约进程退出。
    pub fn apply_exit(&mut self, exit: &SessionExited, timestamp_ms: u64) -> Option<StatusUpdate> {
        if exit.sid.trim().is_empty() {
            return None;
        }
        self.sessions
            .entry(exit.sid.clone())
            .or_insert_with(|| AgentReducer::new(exit.sid.clone()))
            .apply_exit(exit, timestamp_ms)
    }

    /// 某个会话是否有状态（用于让普通 shell 的退出不创建无意义状态）。
    pub fn contains(&self, sid: &str) -> bool {
        self.sessions.contains_key(sid)
    }

    /// 某个会话的当前快照。
    pub fn snapshot(&self, sid: &str) -> Option<&AgentStatusSnapshot> {
        self.sessions.get(sid).and_then(AgentReducer::current)
    }

    /// 所有快照，按 sid 排序，保证 UI 投影稳定。
    pub fn snapshots(&self) -> Vec<AgentStatusSnapshot> {
        let mut snapshots: Vec<_> = self
            .sessions
            .values()
            .filter_map(AgentReducer::current)
            .cloned()
            .collect();
        snapshots.sort_by(|left, right| left.sid.cmp(&right.sid));
        snapshots
    }

    /// 关闭页签 / 会话销毁时移除。
    pub fn remove(&mut self, sid: &str) -> Option<AgentStatusSnapshot> {
        self.sessions
            .remove(sid)
            .and_then(|reducer| reducer.current().cloned())
    }
}

#[derive(Debug, Clone)]
struct Observation {
    state: AgentState,
    source: StatusSource,
    timestamp_ms: u64,
    seq: u64,
    agent: Option<String>,
    reason: Option<String>,
    exit_code: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Order {
    Older,
    Same,
    Newer,
}

/// 判断候选状态能否覆盖当前状态。
fn accept(current: Option<&AgentStatusSnapshot>, incoming: &Observation) -> bool {
    let Some(current) = current else {
        return true;
    };
    if current.state == AgentState::Exited {
        return false;
    }
    if incoming.state == AgentState::Exited {
        // 进程消失是硬事实，绕过时间 / seq；当前已经 exited 的错误路径在上面处理。
        return true;
    }

    let order = compare_version(current, incoming);
    if order == Order::Older {
        return false;
    }
    if order == Order::Same {
        return incoming.state.priority() > current.state.priority();
    }

    transition_allowed(current.state, incoming.state)
}

fn compare_version(current: &AgentStatusSnapshot, incoming: &Observation) -> Order {
    match (current.seq, incoming.seq) {
        (current_seq, incoming_seq) if current_seq > 0 && incoming_seq > 0 => {
            if incoming_seq > current_seq {
                Order::Newer
            } else if incoming_seq < current_seq {
                Order::Older
            } else {
                compare_time(current.timestamp_ms, incoming.timestamp_ms)
            }
        }
        (0, incoming_seq) if incoming_seq > 0 => Order::Newer,
        _ => compare_time(current.timestamp_ms, incoming.timestamp_ms),
    }
}

fn compare_time(current: u64, incoming: u64) -> Order {
    if incoming > current {
        Order::Newer
    } else if incoming < current {
        Order::Older
    } else {
        Order::Same
    }
}

fn transition_allowed(current: AgentState, incoming: AgentState) -> bool {
    use AgentState::*;

    if current == incoming {
        return true;
    }
    match incoming {
        Exited => true,
        Error => current != Exited,
        Permission => !matches!(current, Error | Exited),
        Waiting => !matches!(current, Error | Permission | Exited),
        Running => !matches!(current, Error | Exited),
        // 只有**较新的** done/idle 才能结束 running；迟到的回合结束事件由版本比较
        // （seq / timestamp）挡掉，不能在这里无条件拒绝。
        Done => !matches!(current, Error | Exited),
        Idle => !matches!(current, Error | Exited),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn snapshot(state: AgentState, timestamp_ms: u64, seq: u64) -> AgentStatusSnapshot {
        AgentStatusSnapshot {
            sid: "s".to_owned(),
            agent: Some("claude".to_owned()),
            incarnation_id: Some("inc-1".to_owned()),
            state,
            source: StatusSource::Native,
            timestamp_ms,
            seq,
            exit_code: None,
            reason: None,
        }
    }

    fn reducer_with(state: AgentState, timestamp_ms: u64, seq: u64) -> AgentReducer {
        let mut reducer = AgentReducer::new("s");
        reducer.current = Some(snapshot(state, timestamp_ms, seq));
        reducer
    }

    fn observation(state: AgentState, timestamp_ms: u64, seq: u64) -> Observation {
        Observation {
            state,
            source: StatusSource::Native,
            timestamp_ms,
            seq,
            agent: Some("claude".to_owned()),
            reason: None,
            exit_code: None,
        }
    }

    fn event(name: &str, payload: serde_json::Value, ts: u64) -> AgentEvent {
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

    fn state_strategy() -> impl Strategy<Value = AgentState> {
        prop_oneof![
            Just(AgentState::Running),
            Just(AgentState::Idle),
            Just(AgentState::Waiting),
            Just(AgentState::Permission),
            Just(AgentState::Done),
            Just(AgentState::Error),
            Just(AgentState::Exited),
        ]
    }

    proptest! {
        #[test]
        fn exited_is_terminal_against_every_later_state(
            later in state_strategy(),
            timestamp in 0u64..10_000,
        ) {
            let mut reducer = reducer_with(AgentState::Exited, 100, 0);
            let result = reducer.merge(observation(later, timestamp, 0));
            prop_assert!(result.is_none());
            prop_assert_eq!(reducer.current().map(|current| current.state), Some(AgentState::Exited));
        }

        #[test]
        fn error_is_not_downgraded_by_non_exit(
            later in state_strategy(),
            timestamp in 0u64..10_000,
        ) {
            prop_assume!(later != AgentState::Exited);
            let mut reducer = reducer_with(AgentState::Error, 100, 0);
            let _ = reducer.merge(observation(later, timestamp, 0));
            prop_assert_eq!(reducer.current().map(|current| current.state), Some(AgentState::Error));
        }

        #[test]
        fn late_done_and_idle_never_regress_running(
            terminal in prop_oneof![Just(AgentState::Done), Just(AgentState::Idle)],
            timestamp in 0u64..=100,
        ) {
            let mut reducer = reducer_with(AgentState::Running, 100, 0);
            let result = reducer.merge(observation(terminal, timestamp, 0));
            prop_assert!(result.is_none());
            prop_assert_eq!(reducer.current().map(|current| current.state), Some(AgentState::Running));
        }

        #[test]
        fn permission_beats_waiting_at_the_same_version(
            seq in 1u64..100,
            timestamp in 0u64..10_000,
        ) {
            let mut reducer = reducer_with(AgentState::Waiting, timestamp, seq);
            let result = reducer.merge(observation(AgentState::Permission, timestamp, seq));
            prop_assert!(result.is_some());
            prop_assert_eq!(reducer.current().map(|current| current.state), Some(AgentState::Permission));
        }

        #[test]
        fn logical_sequence_wins_over_newer_timestamp(
            sequence in 2u64..100,
            timestamp in 0u64..10_000,
        ) {
            let mut reducer = reducer_with(AgentState::Done, timestamp, sequence);
            let stale = reducer.merge(observation(AgentState::Running, timestamp + 1, sequence - 1));
            prop_assert!(stale.is_none());
            prop_assert_eq!(reducer.current().map(|current| current.state), Some(AgentState::Done));
        }

        #[test]
        fn stable_id_is_idempotent_at_any_later_time(
            timestamp in 1u64..10_000,
            later in 0u64..10_000,
            tool in "[a-z0-9]{1,12}",
        ) {
            let mut reducer = AgentReducer::new("s");
            let first = event(
                "PreToolUse",
                serde_json::json!({ "tool_use_id": tool, "seq": 1 }),
                timestamp,
            );
            prop_assert!(reducer.apply_event(&first).is_some());
            let state = reducer.current().map(|current| current.state);
            let duplicate = event(
                "PreToolUse",
                serde_json::json!({ "tool_use_id": tool, "seq": 1 }),
                timestamp.saturating_add(later),
            );
            prop_assert!(reducer.apply_event(&duplicate).is_none());
            prop_assert_eq!(reducer.current().map(|current| current.state), state);
        }

        #[test]
        fn anonymous_duplicate_within_debounce_is_ignored(
            timestamp in 1u64..10_000,
            delta in 0u64..=DEBOUNCE_MS,
        ) {
            let mut reducer = AgentReducer::new("s");
            let first = event("question.asked", serde_json::json!({}), timestamp);
            prop_assert!(reducer.apply_event(&first).is_some());
            let duplicate = event(
                "question.asked",
                serde_json::json!({}),
                timestamp.saturating_add(delta),
            );
            prop_assert!(reducer.apply_event(&duplicate).is_none());
            prop_assert_eq!(
                reducer.current().map(|current| current.state),
                Some(AgentState::Waiting)
            );
        }

        #[test]
        fn anonymous_duplicate_after_debounce_can_apply(
            timestamp in 1u64..10_000,
            delta in (DEBOUNCE_MS + 1)..10_000,
        ) {
            let mut reducer = AgentReducer::new("s");
            let first = event("question.asked", serde_json::json!({}), timestamp);
            prop_assert!(reducer.apply_event(&first).is_some());
            let later = event(
                "question.asked",
                serde_json::json!({}),
                timestamp.saturating_add(delta),
            );
            let update = reducer.apply_event(&later).expect("窗口外可再次处理");
            prop_assert!(!update.changed, "相同状态只刷新版本");
            prop_assert_eq!(update.current.state, AgentState::Waiting);
        }
    }
}

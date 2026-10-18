//! 状态归约：版本化、优先级、幂等、防抖与子代理过滤（#80）。
//!
//! 归约器是**纯内存**的：不读时钟、不做 I/O。事件 timestamp 由 shim 带入；进程退出与
//! 降级轮询的 timestamp 由调用方传入。这样夹具与 proptest 完全可重复。
//!
//! # 降级（#81）
//!
//! [`AgentReducer::observe_degraded`] 接收进程树识别结果与 daemon 的最后输出时间。
//! 降级模式只产生 `running` / `idle` / `exited`，全部 `source=heuristic` 且
//! `degraded=true`；`t_idle=60s` 需连续两次轮询，输出立即回 `running`。原生模式只用
//! `T_interrupt=120s` 兜底，绝不覆盖原生 `waiting` / `permission` / `done` / `error`。
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

/// 降级模式：输出静默达到这个阈值后进入 `idle`（需连续两次轮询）。
pub const T_IDLE_MS: u64 = 60_000;

/// 原生模式：`running` 且无事件 / 输出达到这个阈值后降为 `idle`（明确标注不完整）。
pub const T_INTERRUPT_MS: u64 = 120_000;

/// 一次降级轮询的输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DegradedObservation {
    /// 会话根进程是否还活着；`false` 时状态归为 `exited`。
    pub root_alive: bool,
    /// `sysinfo` 进程树里实际识别到的 agent id。
    pub process_agent_id: Option<String>,
    /// 一等入口 / 会话元数据里已知的 agent id（进程尚未被识别时的回退）。
    pub configured_agent_id: Option<String>,
    /// 本次 spawn 的世代 id（app-core 从 SessionCreated / SessionInfo 传入）。
    pub incarnation_id: Option<String>,
    /// daemon 记录的最后输出时间（Unix 毫秒）；隐藏会话也准确。
    pub last_output_ms: u64,
    /// 本次轮询时间（Unix 毫秒）。
    pub now_ms: u64,
}

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
    /// 是否已收到过原生事件（一旦 native，不再让降级 running/idle 覆盖）。
    native_mode: bool,
    /// 最近一次输出活动（daemon 时间或本地观察）。
    last_output_ms: u64,
    /// 最近一次原生事件时间。
    last_native_event_ms: u64,
    /// 降级模式连续满足静默阈值的轮询次数。
    idle_streak: u8,
    /// 是否曾识别到 agent 进程（用于区分「进程退出」与「尚未启动」）。
    seen_agent: bool,
    /// 原生模式下是否已因 T_interrupt 降为启发式 idle。
    fallback_idle: bool,
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
            native_mode: false,
            last_output_ms: 0,
            last_native_event_ms: 0,
            idle_streak: 0,
            seen_agent: false,
            fallback_idle: false,
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
        // T_interrupt 的启发式 idle 会被任何新原生事件立即恢复；若能映射出具体状态，
        // 则让该事件直接覆盖（例如 Stop → done），只有未知 / 重复事件才单独恢复 running。
        let was_fallback = std::mem::take(&mut self.fallback_idle);
        // 原生事件是「集成已生效」的证据；即使事件本身被忽略（未知 / 子代理），也说明
        // 不再处于纯降级模式。
        self.native_mode = true;
        self.last_native_event_ms = event.ts;
        self.observe_lineage(event);
        if mapping::is_subagent_event(event)
            || mapping::event_session_id(event)
                .is_some_and(|session| self.subagent_sessions.contains(&session))
        {
            return self.recover_fallback(was_fallback, event.ts);
        }
        let Mapping::State(mapped) = mapping::map_event(event) else {
            return self.recover_fallback(was_fallback, event.ts);
        };
        if self.is_duplicate(event) {
            return self.recover_fallback(was_fallback, event.ts);
        }
        self.merge(Observation {
            state: mapped.state,
            source: StatusSource::Native,
            timestamp_ms: event.ts,
            seq: mapped.seq,
            agent: Some(event.agent.clone()),
            reason: mapped.reason,
            exit_code: None,
            degraded: false,
        })
        .or_else(|| self.recover_fallback(was_fallback, event.ts))
    }

    fn recover_fallback(&mut self, was_fallback: bool, timestamp_ms: u64) -> Option<StatusUpdate> {
        if !was_fallback
            || self.current.as_ref().map(|current| current.state) != Some(AgentState::Idle)
        {
            return None;
        }
        self.force_state(
            AgentState::Running,
            StatusSource::Native,
            timestamp_ms,
            false,
            None,
        )
    }

    /// 输出活动：降级模式下立即回 `running`；原生模式下只处理 T_interrupt 兜底后的恢复。
    ///
    /// `timestamp_ms` 来自 app-core 收到数据帧的时间，或 daemon 的 `lastOutputMs`。
    pub fn note_output(&mut self, timestamp_ms: u64) -> Option<StatusUpdate> {
        let timestamp_ms = timestamp_ms.max(self.last_output_ms);
        self.last_output_ms = timestamp_ms;
        self.idle_streak = 0;

        if self.native_mode {
            if self.fallback_idle {
                self.fallback_idle = false;
                return self.force_state(
                    AgentState::Running,
                    StatusSource::Native,
                    timestamp_ms,
                    false,
                    None,
                );
            }
            return None;
        }

        let state = self.current.as_ref()?.state;
        if matches!(state, AgentState::Running | AgentState::Exited) {
            return None;
        }
        self.force_state(
            AgentState::Running,
            StatusSource::Heuristic,
            timestamp_ms,
            true,
            None,
        )
    }

    /// 一次降级轮询：进程树识别 + 输出静默。
    ///
    /// - 降级模式只产生 `running` / `idle` / `exited`，全部 `source=heuristic` 且 `degraded=true`；
    /// - 原生模式只用 `T_interrupt` 兜底，不覆盖原生 `waiting` / `permission` / `done` / `error`；
    /// - agent 进程从有到无 → `exited`；从未识别到（例如一等入口的命令还没起来）不误判。
    pub fn observe_degraded(&mut self, obs: &DegradedObservation) -> Option<StatusUpdate> {
        self.begin_incarnation(obs.incarnation_id.as_deref());
        let output_changed = obs.last_output_ms > self.last_output_ms;
        self.last_output_ms = self.last_output_ms.max(obs.last_output_ms);

        if !obs.root_alive {
            return self.force_exited(obs.now_ms);
        }

        let process_agent = non_empty(obs.process_agent_id.as_deref());
        let configured_agent = non_empty(obs.configured_agent_id.as_deref());
        if process_agent.is_some() {
            self.seen_agent = true;
        }

        if self.native_mode {
            if self.seen_agent && process_agent.is_none() {
                return self.force_exited(obs.now_ms);
            }
            if self.fallback_idle && output_changed {
                self.fallback_idle = false;
                return self.force_state(
                    AgentState::Running,
                    StatusSource::Native,
                    obs.now_ms,
                    false,
                    None,
                );
            }
            let current = self.current.as_ref()?;
            if current.state == AgentState::Running
                && obs.now_ms.saturating_sub(self.last_activity_ms()) >= T_INTERRUPT_MS
            {
                self.fallback_idle = true;
                return self.force_state(
                    AgentState::Idle,
                    StatusSource::Heuristic,
                    obs.now_ms,
                    true,
                    None,
                );
            }
            return None;
        }

        if self.seen_agent && process_agent.is_none() {
            return self.force_exited(obs.now_ms);
        }
        let agent = process_agent.or(configured_agent);
        let agent = agent?;

        let Some(current) = self.current.as_ref() else {
            return self.force_new(&agent, obs);
        };
        if current.state == AgentState::Exited {
            return self.force_new(&agent, obs);
        }

        if output_changed && current.state != AgentState::Running {
            self.idle_streak = 0;
            return self.force_state(
                AgentState::Running,
                StatusSource::Heuristic,
                obs.now_ms,
                true,
                None,
            );
        }
        if current.state == AgentState::Running {
            if output_changed {
                self.idle_streak = 0;
                return None;
            }
            if obs.now_ms.saturating_sub(self.last_output_ms) >= T_IDLE_MS {
                self.idle_streak = self.idle_streak.saturating_add(1);
                if self.idle_streak >= 2 {
                    return self.force_state(
                        AgentState::Idle,
                        StatusSource::Heuristic,
                        obs.now_ms,
                        true,
                        None,
                    );
                }
            } else {
                self.idle_streak = 0;
            }
        }
        None
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
        if self.current.as_ref().is_some_and(|current| {
            current.state.is_terminal() && current.source != StatusSource::Heuristic
        }) {
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
            degraded: false,
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
            || self.current.as_ref().is_some_and(|old| {
                old.exit_code != current.exit_code || old.source != current.source
            });
        self.fallback_idle = false;
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
        self.native_mode = false;
        self.last_output_ms = 0;
        self.last_native_event_ms = 0;
        self.idle_streak = 0;
        self.seen_agent = false;
        self.fallback_idle = false;
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
            degraded: incoming.degraded,
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
            || previous.is_some_and(|old| {
                old.exit_code != current.exit_code
                    || old.source != current.source
                    || old.degraded != current.degraded
            });
        self.current = Some(current.clone());
        Some(StatusUpdate {
            previous: previous_state,
            current,
            changed,
        })
    }
}

impl AgentReducer {
    fn last_activity_ms(&self) -> u64 {
        self.last_output_ms.max(self.last_native_event_ms)
    }

    fn force_new(&mut self, agent: &str, obs: &DegradedObservation) -> Option<StatusUpdate> {
        let previous = self.current.as_ref().map(|current| current.state);
        let current = AgentStatusSnapshot {
            sid: self.sid.clone(),
            agent: Some(agent.to_owned()),
            incarnation_id: self.incarnation_id.clone(),
            state: AgentState::Running,
            source: StatusSource::Heuristic,
            degraded: true,
            timestamp_ms: obs.now_ms,
            seq: self
                .current
                .as_ref()
                .map(|current| current.seq)
                .unwrap_or(0),
            exit_code: None,
            reason: None,
        };
        // 刚识别到 agent 时给一个完整静默窗口；否则会把 shell 启动前的旧时间算进来。
        self.last_output_ms = obs.last_output_ms.max(obs.now_ms);
        self.idle_streak = 0;
        self.current = Some(current.clone());
        Some(StatusUpdate {
            previous,
            current,
            changed: previous != Some(AgentState::Running),
        })
    }

    fn force_state(
        &mut self,
        state: AgentState,
        source: StatusSource,
        timestamp_ms: u64,
        degraded: bool,
        reason: Option<String>,
    ) -> Option<StatusUpdate> {
        let previous_snapshot = self.current.as_ref()?;
        let previous = previous_snapshot.state;
        let current = AgentStatusSnapshot {
            sid: self.sid.clone(),
            agent: previous_snapshot.agent.clone(),
            incarnation_id: self.incarnation_id.clone(),
            state,
            source,
            degraded,
            timestamp_ms,
            seq: previous_snapshot.seq,
            exit_code: None,
            reason,
        };
        let changed = previous != current.state
            || previous_snapshot.source != current.source
            || previous_snapshot.degraded != current.degraded;
        self.current = Some(current.clone());
        Some(StatusUpdate {
            previous: Some(previous),
            current,
            changed,
        })
    }

    fn force_exited(&mut self, timestamp_ms: u64) -> Option<StatusUpdate> {
        let previous_snapshot = self.current.as_ref()?;
        if previous_snapshot.state == AgentState::Exited {
            return None;
        }
        let previous = previous_snapshot.state;
        let current = AgentStatusSnapshot {
            sid: self.sid.clone(),
            agent: previous_snapshot.agent.clone(),
            incarnation_id: self.incarnation_id.clone(),
            state: AgentState::Exited,
            source: StatusSource::Heuristic,
            degraded: true,
            timestamp_ms,
            seq: previous_snapshot.seq,
            exit_code: None,
            reason: previous_snapshot.reason.clone(),
        };
        let changed =
            previous != AgentState::Exited || previous_snapshot.source != StatusSource::Heuristic;
        self.current = Some(current.clone());
        Some(StatusUpdate {
            previous: Some(previous),
            current,
            changed,
        })
    }
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
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

    /// 输出活动；没有状态的普通 shell 直接忽略。
    pub fn note_output(&mut self, sid: &str, timestamp_ms: u64) -> Option<StatusUpdate> {
        self.sessions.get_mut(sid)?.note_output(timestamp_ms)
    }

    /// 一次降级轮询；普通 shell（无 agent 进程、无 configured agent）不创建状态。
    pub fn observe_degraded(
        &mut self,
        sid: &str,
        obs: &DegradedObservation,
    ) -> Option<StatusUpdate> {
        if !self.sessions.contains_key(sid) {
            if obs.process_agent_id.is_none() && obs.configured_agent_id.is_none() {
                return None;
            }
            self.sessions.insert(sid.to_owned(), AgentReducer::new(sid));
        }
        self.sessions
            .get_mut(sid)
            .and_then(|reducer| reducer.observe_degraded(obs))
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
    degraded: bool,
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
    // 原生事实永远优先于启发式；否则轮询时间戳可能比事件晚几百毫秒，把刚到的原生状态挡掉。
    if current.source == StatusSource::Heuristic && incoming.source == StatusSource::Native {
        return transition_allowed(current.state, incoming.state);
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
            degraded: false,
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
            degraded: false,
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

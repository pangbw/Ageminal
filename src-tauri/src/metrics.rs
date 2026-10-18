//! 本地数据采集（#88）：`logs/metrics.jsonl`。
//!
//! 只记录计数与状态迁移，不写终端内容 / 对话；`metrics.jsonl` 可随时重置，不参与
//! `state.json` / `settings.json` 的 schema 迁移。

use std::sync::atomic::{AtomicU64, Ordering};

use ageminal_agent::{AgentState, StatusSource, StatusUpdate};
use serde_json::json;

use crate::logging;

const MAX_METRICS_BYTES: u64 = 10 * 1024 * 1024;
const GENERATIONS: u32 = 5;
static PENDING_BYTES: AtomicU64 = AtomicU64::new(0);

fn path() -> std::path::PathBuf {
    logging::logs_dir().join("metrics.jsonl")
}

fn append(line: &serde_json::Value) {
    let path = path();
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    if std::fs::metadata(&path)
        .map(|metadata| metadata.len() >= MAX_METRICS_BYTES)
        .unwrap_or(false)
    {
        rotate(&path);
    }
    use std::io::Write;
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    let _ = writeln!(file, "{line}");
}

fn rotate(path: &std::path::Path) {
    let base = path.to_string_lossy();
    let _ = std::fs::remove_file(format!("{base}.{GENERATIONS}"));
    for generation in (1..GENERATIONS).rev() {
        let _ = std::fs::rename(
            format!("{base}.{generation}"),
            format!("{base}.{}", generation + 1),
        );
    }
    let _ = std::fs::rename(path, format!("{base}.1"));
}

fn state_name(state: AgentState) -> &'static str {
    match state {
        AgentState::Running => "running",
        AgentState::Waiting => "waiting",
        AgentState::Permission => "permission",
        AgentState::Done => "done",
        AgentState::Idle => "idle",
        AgentState::Error => "error",
        AgentState::Exited => "exited",
    }
}

fn source_name(source: StatusSource) -> &'static str {
    match source {
        StatusSource::Native => "native",
        StatusSource::Process => "process",
        StatusSource::Heuristic => "heuristic",
    }
}

/// 记录一次统一状态迁移。
pub fn record_status(update: &StatusUpdate) {
    append(&json!({
        "ts": crate::now_ms(),
        "event": "agent_status",
        "sid": update.current.sid,
        "incarnation": update.current.incarnation_id,
        "state": state_name(update.current.state),
        "source": source_name(update.current.source),
        "degraded": update.current.degraded,
        "changed": update.changed,
    }));
}

/// 累计 PTY 吞吐；每轮降级轮询落一次，避免每个数据帧写文件。
pub fn add_bytes(bytes: usize) {
    PENDING_BYTES.fetch_add(bytes as u64, Ordering::Relaxed);
}

/// 把当前累计吞吐写成一行。
pub fn flush_throughput() {
    let bytes = PENDING_BYTES.swap(0, Ordering::Relaxed);
    if bytes == 0 {
        return;
    }
    append(&json!({
        "ts": crate::now_ms(),
        "event": "throughput",
        "bytes": bytes,
    }));
}

/// 清空指标；UI「重置指标」用。返回是否成功。
pub fn reset() -> Result<(), String> {
    let path = path();
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// 重置本地指标采集文件（#88）。
#[tauri::command]
#[specta::specta]
pub fn metrics_reset() -> Result<(), String> {
    reset()
}

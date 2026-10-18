//! 有界落盘历史（issue #74）。
//!
//! 每个会话一个目录：`<config>/history/<encoded-sid>/`，包含
//! - `checkpoint.json`：某个 `seq` 上的 ANSI 屏幕快照（原子替换）；
//! - `output.log`：`magic + version + generation + start_seq` 头部 + 定长帧
//!   （`kind(1) + payload_len(4, LE) + seq(8, LE) + bytes`）。
//!
//! # 为什么不是把 4 MiB 环形缓冲直接 dump 到磁盘
//!
//! 环形缓冲只服务「热重连」；UI detach 后仍要继续产出，daemon 也可能活很久。落盘日志按
//! **累计字节偏移 `seq`** 追加，`Attach(last_seq)` 在内存环覆盖不到时可以从日志精确续放。
//! 日志与 checkpoint 用 `generation` 配对，避免把旧快照配到新日志。
//!
//! # 有界
//!
//! - VT / xterm 的 scrollback 是 10k 行（内存态）；
//! - 单会话落盘上限 [`CHECKPOINT_MAX`]（64 MiB，含 checkpoint 与日志；日志满时写新
//!   checkpoint 并重置日志）；
//! - 所有历史目录总预算 [`HISTORY_BUDGET`]（256 MiB），超出时按最近更新时间从旧到新 GC
//!   **非活跃**会话；活跃会话由单会话上限兜底。
//!
//! # 残尾 / 断档
//!
//! 崩溃可能留下半截帧：恢复时只读到最后一个完整帧，残尾直接忽略。帧的 `seq` 必须连续；
//! 出现断档就判为不可读，由调用方退回 ANSI 快照——绝不把半个转义序列画到屏幕上。
//!
//! # 目录名
//!
//! `sid` 是逻辑 id（形如 `terminal:/ws/a:1`），**不能直接当路径**。这里用百分号编码
//! 只保留 `[A-Za-z0-9_-]`，其余字节编码成 `%XX`；真实 `sid` 同时写进 checkpoint。

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// 单会话历史（checkpoint + 日志）的上限，也是 checkpoint 文件的安全网。
pub const CHECKPOINT_MAX: u64 = 64 * 1024 * 1024;

/// 所有历史目录的总预算；超出时 GC 最旧的非活跃会话。
pub const HISTORY_BUDGET: u64 = 256 * 1024 * 1024;

/// checkpoint 的 schema 版本。
pub const CHECKPOINT_SCHEMA: u32 = 1;

/// 每写多少输出补一份 checkpoint（不截断日志；轮转时才截断）。
pub const CHECKPOINT_EVERY: u64 = 1024 * 1024;

/// 日志头版本。
pub const LOG_VERSION: u8 = 1;

const LOG_MAGIC: [u8; 4] = *b"AGMH";

/// 日志头：magic(4) + version(1) + reserved(3) + generation(4) + start_seq(8)。
const HEADER_BYTES: usize = 20;

/// 日志帧类型：输出字节。
const FRAME_OUTPUT: u8 = 1;

/// 单个日志帧的 payload 上限（`MAX_FRAME_BYTES` 留出余量）。
const MAX_FRAME_PAYLOAD: usize = 1024 * 1024 - 8;

const CHECKPOINT_FILE: &str = "checkpoint.json";
const LOG_FILE: &str = "output.log";

/// 历史层错误。调用方（会话层）只记录并**关闭持久化**，绝不让历史 IO 把会话弄死。
#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    /// 文件系统 / IO 失败。
    #[error("历史 IO 失败：{0}")]
    Io(#[from] io::Error),
    /// checkpoint JSON 失败。
    #[error("checkpoint 序列化失败：{0}")]
    Json(#[from] serde_json::Error),
    /// checkpoint 超过单会话安全网。
    #[error("checkpoint {bytes} 字节，超过上限 {max}")]
    CheckpointTooLarge { bytes: u64, max: u64 },
    /// 单帧超过协议层的安全上限。
    #[error("历史帧 {bytes} 字节，超过上限 {max}")]
    FrameTooLarge { bytes: usize, max: usize },
    /// 追加后需要轮转，但调用方没有提供快照。
    #[error("历史轮转缺少 checkpoint 快照")]
    RotationNeedsCheckpoint,
    /// 日志内容与内存状态对不上（残尾之外的结构性问题）。
    #[error("历史损坏：{0}")]
    Corrupt(String),
}

/// 历史限额；生产用 [`HistoryLimits::default`]，测试用更小的值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryLimits {
    /// 单会话（checkpoint + 日志）上限。
    pub checkpoint_max: u64,
    /// 全部历史目录的总预算。
    pub total_budget: u64,
}

impl Default for HistoryLimits {
    fn default() -> Self {
        Self {
            checkpoint_max: CHECKPOINT_MAX,
            total_budget: HISTORY_BUDGET,
        }
    }
}

/// 要从 VT 内核落进 checkpoint 的状态。
#[derive(Debug, Clone, Copy)]
pub struct Snapshot<'a> {
    /// ANSI 屏幕快照。
    pub ansi: &'a str,
    /// 列数。
    pub cols: u16,
    /// 行数。
    pub rows: u16,
    /// 是否处于 alt-screen。
    pub alt_screen: bool,
}

/// `checkpoint.json` 的落盘形状。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    /// schema 版本（[`CHECKPOINT_SCHEMA`]）。
    pub schema_version: u32,
    /// 逻辑会话 id（目录名是编码后的形态，这里保留原文）。
    pub sid: String,
    /// 与日志头配对的世代。
    pub generation: u32,
    /// 快照对应的累计字节偏移。
    pub seq: u64,
    /// 列数。
    pub cols: u16,
    /// 行数。
    pub rows: u16,
    /// 是否处于 alt-screen。
    pub alt_screen: bool,
    /// ANSI 快照。
    pub ansi: String,
    /// 写入时刻（Unix 毫秒）。
    pub updated_at_ms: u64,
}

/// 所有会话共享的历史根与限额。
#[derive(Debug, Clone)]
pub struct HistoryStore {
    root: PathBuf,
    limits: HistoryLimits,
    /// daemon 日志落点；历史层只在这里留失败线索。
    log_path: Option<PathBuf>,
}

impl HistoryStore {
    /// 用生产限额建 store。
    pub fn new(root: PathBuf) -> Self {
        Self::with_limits(root, HistoryLimits::default())
    }

    /// 显式限额（测试 / 将来的设置项）。
    pub fn with_limits(root: PathBuf, limits: HistoryLimits) -> Self {
        Self {
            root,
            limits,
            log_path: None,
        }
    }

    /// 把历史层的失败写进 daemon.log（stdio 是 NUL，不能 eprintln）。
    pub fn with_log_path(mut self, log_path: PathBuf) -> Self {
        self.log_path = Some(log_path);
        self
    }

    /// 历史根。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 为 `sid` 开一份新历史（同 sid 的旧目录会被清掉）。
    pub fn create(&self, sid: &str, snapshot: Snapshot<'_>) -> Result<History, HistoryError> {
        fs::create_dir_all(&self.root)?;
        let dir = self.root.join(encode_sid(sid));
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        fs::create_dir_all(&dir)?;

        let log_path = dir.join(LOG_FILE);
        let log = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&log_path)?;
        let mut history = History {
            sid: sid.to_owned(),
            dir,
            log_path,
            log,
            generation: 1,
            start_seq: 0,
            end_seq: 0,
            log_size: HEADER_BYTES as u64,
            checkpoint_size: 0,
            last_checkpoint_seq: 0,
            limits: self.limits,
            log_path_for_warnings: self.log_path.clone(),
        };
        if let Err(error) = history.initialize(snapshot) {
            let _ = fs::remove_dir_all(&history.dir);
            return Err(error);
        }
        Ok(history)
    }

    /// 按总预算清理历史。`active` 里的 `sid`（解码前的原文）绝不删除。
    pub fn collect_garbage(&self, active: &[String]) -> GcReport {
        let mut report = GcReport::default();
        let active: Vec<String> = active.iter().map(|sid| encode_sid(sid)).collect();
        let Ok(entries) = fs::read_dir(&self.root) else {
            report.remaining_bytes = 0;
            return report;
        };

        let mut candidates = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let (bytes, updated) = directory_usage(&path);
            report.remaining_bytes = report.remaining_bytes.saturating_add(bytes);
            candidates.push(Candidate {
                name,
                path,
                bytes,
                updated,
            });
        }

        // 最旧的先走；同一时刻按目录名兜底，保证可重复。
        candidates.sort_by(|left, right| {
            left.updated
                .cmp(&right.updated)
                .then_with(|| left.name.cmp(&right.name))
        });
        for candidate in candidates {
            if report.remaining_bytes <= self.limits.total_budget {
                break;
            }
            if active.contains(&candidate.name) {
                continue;
            }
            match fs::remove_dir_all(&candidate.path) {
                Ok(()) => {
                    report.remaining_bytes = report.remaining_bytes.saturating_sub(candidate.bytes);
                    report.removed_dirs += 1;
                    report.removed_bytes = report.removed_bytes.saturating_add(candidate.bytes);
                }
                Err(error) => self.warn(&format!(
                    "GC 删除 {} 失败：{error}",
                    candidate.path.display()
                )),
            }
        }
        report.over_budget = report.remaining_bytes > self.limits.total_budget;
        if report.over_budget {
            self.warn(&format!(
                "历史总预算仍超出：{} > {}（活跃会话受单会话上限保护）",
                report.remaining_bytes, self.limits.total_budget
            ));
        }
        report
    }

    pub(crate) fn warn(&self, message: &str) {
        #[cfg(windows)]
        if let Some(path) = &self.log_path {
            crate::logging::append(path, &format!("[history] {message}"));
        }
        #[cfg(not(windows))]
        let _ = (&self.log_path, message);
    }
}

/// GC 的结果。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct GcReport {
    /// 扫描到的历史总字节数。
    pub remaining_bytes: u64,
    /// 删除的目录数。
    pub removed_dirs: usize,
    /// 删除的字节数。
    pub removed_bytes: u64,
    /// 清理后是否仍超预算（活跃会话可能撑住）。
    pub over_budget: bool,
}

/// 一个会话的历史写入端。
#[derive(Debug)]
pub struct History {
    sid: String,
    dir: PathBuf,
    log_path: PathBuf,
    log: File,
    generation: u32,
    start_seq: u64,
    end_seq: u64,
    log_size: u64,
    checkpoint_size: u64,
    last_checkpoint_seq: u64,
    limits: HistoryLimits,
    log_path_for_warnings: Option<PathBuf>,
}

impl History {
    /// 历史目录。
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 当前日志头部世代。
    pub fn generation(&self) -> u32 {
        self.generation
    }

    /// 日志能覆盖的起点（轮转后等于 checkpoint 的 `seq`）。
    pub fn start_seq(&self) -> u64 {
        self.start_seq
    }

    /// 已写入的累计字节偏移。
    pub fn end_seq(&self) -> u64 {
        self.end_seq
    }

    /// 当前日志文件大小。
    pub fn log_size(&self) -> u64 {
        self.log_size
    }

    /// 当前 checkpoint 文件大小。
    pub fn checkpoint_size(&self) -> u64 {
        self.checkpoint_size
    }

    /// 再追加 `incoming` 字节是否就到了下一份 checkpoint 的间隔。
    pub fn needs_checkpoint(&self, incoming: usize) -> bool {
        self.end_seq
            .saturating_add(incoming as u64)
            .saturating_sub(self.last_checkpoint_seq)
            >= CHECKPOINT_EVERY
    }

    /// 在当前 `seq` 写一份 checkpoint；不截断日志（供周期性检查点用）。
    pub fn checkpoint(&mut self, seq: u64, snapshot: Snapshot<'_>) -> Result<(), HistoryError> {
        if seq != self.end_seq {
            return Err(HistoryError::Corrupt(format!(
                "checkpoint 偏移 {seq} 与当前末尾 {} 不一致",
                self.end_seq
            )));
        }
        self.write_checkpoint(seq, self.generation, snapshot)
    }

    /// 再追加 `incoming` 字节是否就需要轮转。
    pub fn would_rotate(&self, incoming: usize) -> bool {
        self.log_size
            .saturating_add(self.checkpoint_size)
            .saturating_add(frame_bytes(incoming as u64))
            > self.limits.checkpoint_max
    }

    /// 追加一段**连续**输出。`seq` 必须是当前末尾。
    ///
    /// 若调用前 [`Self::would_rotate`] 为真，`checkpoint` 必须提供；轮转时它会成为新 checkpoint。
    pub fn append(
        &mut self,
        seq: u64,
        bytes: &[u8],
        checkpoint: Option<Snapshot<'_>>,
    ) -> Result<(), HistoryError> {
        if seq != self.end_seq {
            return Err(HistoryError::Corrupt(format!(
                "追加偏移 {seq} 与当前末尾 {} 不连续",
                self.end_seq
            )));
        }
        if bytes.is_empty() {
            return Ok(());
        }

        let mut offset = 0usize;
        for chunk in bytes.chunks(MAX_FRAME_PAYLOAD - 8) {
            self.write_output_frame(seq + offset as u64, chunk)?;
            offset += chunk.len();
        }
        self.end_seq += bytes.len() as u64;

        if self.log_size.saturating_add(self.checkpoint_size) > self.limits.checkpoint_max {
            let snapshot = checkpoint.ok_or(HistoryError::RotationNeedsCheckpoint)?;
            self.rotate(snapshot)?;
        }
        Ok(())
    }

    /// 从 `from_seq` 精确读出日志尾巴；不在覆盖范围 / 有断档时返回 `None`。
    ///
    /// 残尾（半截帧）会被忽略；如果残尾之后还有完整帧才判为损坏。
    pub fn replay_from(&self, from_seq: u64) -> Result<Option<Vec<u8>>, HistoryError> {
        if from_seq < self.start_seq || from_seq > self.end_seq {
            return Ok(None);
        }
        let mut file = File::open(&self.log_path)?;
        let Some((generation, start_seq)) = read_log_header(&mut file)? else {
            return Ok(None);
        };
        if generation != self.generation || start_seq != self.start_seq {
            return Ok(None);
        }
        if from_seq < start_seq {
            return Ok(None);
        }

        let mut cursor = start_seq;
        let mut started = false;
        let mut out = Vec::with_capacity((self.end_seq - from_seq) as usize);
        loop {
            let mut frame_head = [0u8; 5];
            match read_exact_or_eof(&mut file, &mut frame_head)? {
                ReadStatus::Eof | ReadStatus::Partial => break,
                ReadStatus::Full => {}
            }
            let kind = frame_head[0];
            let length = u32::from_le_bytes(frame_head[1..5].try_into().expect("u32")) as usize;
            if !(8..=MAX_FRAME_PAYLOAD).contains(&length) {
                return Ok(None);
            }
            let mut payload = vec![0u8; length];
            if !matches!(
                read_exact_or_eof(&mut file, &mut payload)?,
                ReadStatus::Full
            ) {
                break;
            }
            if kind != FRAME_OUTPUT {
                return Ok(None);
            }
            let frame_seq = u64::from_le_bytes(payload[..8].try_into().expect("u64"));
            let Some(frame_end) = frame_seq.checked_add((length - 8) as u64) else {
                return Ok(None);
            };
            if frame_seq != cursor {
                return Ok(None);
            }
            cursor = frame_end;
            if frame_end <= from_seq {
                continue;
            }
            if !started && frame_seq > from_seq {
                return Ok(None);
            }
            started = true;
            let skip = from_seq.saturating_sub(frame_seq) as usize;
            out.extend_from_slice(&payload[8 + skip..]);
        }

        if cursor != self.end_seq {
            return Ok(None);
        }
        Ok(Some(out))
    }

    fn initialize(&mut self, snapshot: Snapshot<'_>) -> Result<(), HistoryError> {
        self.write_checkpoint(0, self.generation, snapshot)?;
        self.write_log_header()?;
        Ok(())
    }

    fn write_log_header(&mut self) -> Result<(), HistoryError> {
        self.log.seek(SeekFrom::Start(0))?;
        let mut header = [0u8; HEADER_BYTES];
        header[..4].copy_from_slice(&LOG_MAGIC);
        header[4] = LOG_VERSION;
        header[8..12].copy_from_slice(&self.generation.to_le_bytes());
        header[12..20].copy_from_slice(&self.start_seq.to_le_bytes());
        self.log.write_all(&header)?;
        self.log_size = HEADER_BYTES as u64;
        Ok(())
    }

    fn write_output_frame(&mut self, seq: u64, bytes: &[u8]) -> Result<(), HistoryError> {
        let payload = 8u32
            .checked_add(bytes.len() as u32)
            .ok_or(HistoryError::FrameTooLarge {
                bytes: bytes.len(),
                max: MAX_FRAME_PAYLOAD,
            })?;
        if payload as usize > MAX_FRAME_PAYLOAD {
            return Err(HistoryError::FrameTooLarge {
                bytes: bytes.len(),
                max: MAX_FRAME_PAYLOAD,
            });
        }
        self.log.seek(SeekFrom::End(0))?;
        self.log.write_all(&[FRAME_OUTPUT])?;
        self.log.write_all(&payload.to_le_bytes())?;
        self.log.write_all(&seq.to_le_bytes())?;
        self.log.write_all(bytes)?;
        self.log_size = self.log_size.saturating_add(5 + payload as u64);
        Ok(())
    }

    fn rotate(&mut self, snapshot: Snapshot<'_>) -> Result<(), HistoryError> {
        self.generation = self.generation.wrapping_add(1).max(1);
        let seq = self.end_seq;
        self.write_checkpoint(seq, self.generation, snapshot)?;

        self.log.set_len(0)?;
        self.log.seek(SeekFrom::Start(0))?;
        self.start_seq = seq;
        self.write_log_header()?;
        Ok(())
    }

    fn write_checkpoint(
        &mut self,
        seq: u64,
        generation: u32,
        snapshot: Snapshot<'_>,
    ) -> Result<(), HistoryError> {
        let checkpoint = Checkpoint {
            schema_version: CHECKPOINT_SCHEMA,
            sid: self.sid.clone(),
            generation,
            seq,
            cols: snapshot.cols,
            rows: snapshot.rows,
            alt_screen: snapshot.alt_screen,
            ansi: snapshot.ansi.to_owned(),
            updated_at_ms: now_ms(),
        };
        let bytes = serde_json::to_vec(&checkpoint)?;
        if bytes.len() as u64 > self.limits.checkpoint_max {
            return Err(HistoryError::CheckpointTooLarge {
                bytes: bytes.len() as u64,
                max: self.limits.checkpoint_max,
            });
        }

        let temporary = self.dir.join("checkpoint.json.tmp");
        {
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        replace_file(&temporary, &self.dir.join(CHECKPOINT_FILE))?;
        self.checkpoint_size = bytes.len() as u64;
        self.last_checkpoint_seq = seq;
        Ok(())
    }

    #[allow(dead_code)]
    pub(crate) fn warn(&self, message: &str) {
        #[cfg(windows)]
        if let Some(path) = &self.log_path_for_warnings {
            crate::logging::append(path, &format!("[history] {message}"));
        }
        #[cfg(not(windows))]
        let _ = (&self.log_path_for_warnings, message);
    }
}

/// 把逻辑 `sid` 编成安全的目录名（只保留 ASCII 字母数字、`-`、`_`）。
pub fn encode_sid(sid: &str) -> String {
    let mut out = String::with_capacity(sid.len());
    for byte in sid.bytes() {
        if byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    if out.is_empty() {
        out.push_str("%00");
    }
    out
}

/// 目录项 + 统计。
struct Candidate {
    name: String,
    path: PathBuf,
    bytes: u64,
    updated: SystemTime,
}

/// 递归统计目录字节数与最近修改时间（失败项忽略）。
fn directory_usage(path: &Path) -> (u64, SystemTime) {
    let mut bytes = 0u64;
    let mut updated = UNIX_EPOCH;
    let Ok(entries) = fs::read_dir(path) else {
        return (0, updated);
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if let Ok(modified) = metadata.modified() {
            updated = updated.max(modified);
        }
        if metadata.is_dir() {
            let (child_bytes, child_updated) = directory_usage(&entry.path());
            bytes = bytes.saturating_add(child_bytes);
            updated = updated.max(child_updated);
        } else {
            bytes = bytes.saturating_add(metadata.len());
        }
    }
    (bytes, updated)
}

fn frame_bytes(incoming: u64) -> u64 {
    let chunk = (MAX_FRAME_PAYLOAD - 8) as u64;
    let frames = incoming.div_ceil(chunk);
    incoming + frames * 13
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

enum ReadStatus {
    Eof,
    Partial,
    Full,
}

fn read_exact_or_eof(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<ReadStatus> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..]) {
            Ok(0) if filled == 0 => return Ok(ReadStatus::Eof),
            Ok(0) => return Ok(ReadStatus::Partial),
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(ReadStatus::Full)
}

fn read_log_header(file: &mut File) -> Result<Option<(u32, u64)>, HistoryError> {
    let mut header = [0u8; HEADER_BYTES];
    if !matches!(read_exact_or_eof(file, &mut header)?, ReadStatus::Full) {
        return Ok(None);
    }
    if header[..4] != LOG_MAGIC || header[4] != LOG_VERSION {
        return Ok(None);
    }
    let generation = u32::from_le_bytes(header[8..12].try_into().expect("u32"));
    let start_seq = u64::from_le_bytes(header[12..20].try_into().expect("u64"));
    Ok(Some((generation, start_seq)))
}

#[cfg(windows)]
fn replace_file(temporary: &Path, destination: &Path) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    use std::os::windows::ffi::OsStrExt;

    let temporary: Vec<u16> = temporary
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let replaced = unsafe {
        MoveFileExW(
            temporary.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(temporary: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(temporary, destination)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::thread;
    use std::time::Duration;

    use super::*;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_store(tag: &str, limits: HistoryLimits) -> HistoryStore {
        let unique = format!(
            "{}-{tag}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let root = std::env::temp_dir().join(format!("ageminal-history-{unique}"));
        let _ = fs::remove_dir_all(&root);
        HistoryStore::with_limits(root, limits)
    }

    fn snapshot(ansi: &str) -> Snapshot<'_> {
        Snapshot {
            ansi,
            cols: 80,
            rows: 24,
            alt_screen: false,
        }
    }

    #[test]
    fn encoded_sids_cannot_escape_the_history_root() {
        assert_eq!(encode_sid("terminal:/ws/a:1"), "terminal%3A%2Fws%2Fa%3A1");
        assert_eq!(encode_sid(".."), "%2E%2E");
        assert_eq!(encode_sid(r"a\b"), "a%5Cb");
        assert_ne!(encode_sid(""), "");
    }

    #[test]
    fn writes_a_checkpoint_and_replays_by_absolute_offset() {
        let store = temp_store("replay", HistoryLimits::default());
        let mut history = store.create("terminal:/s", snapshot("ready")).unwrap();
        history.append(0, b"0123456789", None).unwrap();
        history.append(10, b"abcdef", None).unwrap();

        let dir = history.dir().to_path_buf();
        assert!(dir.join(CHECKPOINT_FILE).is_file());
        assert!(dir.join(LOG_FILE).is_file());
        assert_eq!(history.end_seq(), 16);
        assert_eq!(history.replay_from(4).unwrap().unwrap(), b"456789abcdef");
        assert!(history.replay_from(16).unwrap().unwrap().is_empty());
        assert_eq!(history.replay_from(17).unwrap(), None);
        assert_eq!(history.replay_from(3).unwrap().unwrap(), b"3456789abcdef");
        drop(history);
        let _ = fs::remove_dir_all(store.root());
    }

    #[test]
    fn a_torn_tail_is_ignored_but_a_gap_is_not() {
        let store = temp_store("tail", HistoryLimits::default());
        let mut history = store.create("s", snapshot("ready")).unwrap();
        history.append(0, b"hello", None).unwrap();

        // 手工写半帧（5 字节头 + 3 字节 payload），模拟崩溃撕开最后一条 append。
        let mut log = OpenOptions::new()
            .append(true)
            .open(history.dir().join(LOG_FILE))
            .unwrap();
        log.write_all(&[FRAME_OUTPUT, 100, 0, 0, 0, 1, 2, 3])
            .unwrap();
        drop(log);
        assert_eq!(history.replay_from(0).unwrap().unwrap(), b"hello");

        // 一个结构完整的帧如果 seq 不连续，必须整份判为不可读。
        let mut log = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(history.dir().join(LOG_FILE))
            .unwrap();
        let mut header = [0u8; HEADER_BYTES];
        header[..4].copy_from_slice(&LOG_MAGIC);
        header[4] = LOG_VERSION;
        header[8..12].copy_from_slice(&1u32.to_le_bytes());
        log.write_all(&header).unwrap();
        log.write_all(&[FRAME_OUTPUT, 13, 0, 0, 0]).unwrap();
        log.write_all(&0u64.to_le_bytes()).unwrap();
        log.write_all(b"hello").unwrap();
        log.write_all(&[FRAME_OUTPUT, 11, 0, 0, 0]).unwrap();
        log.write_all(&99u64.to_le_bytes()).unwrap();
        log.write_all(b"gap").unwrap();
        drop(log);
        assert_eq!(history.replay_from(0).unwrap(), None);

        let dir = history.dir().to_path_buf();
        drop(history);
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(store.root());
    }

    #[test]
    fn periodic_checkpoints_advance_without_truncating_the_log() {
        let store = temp_store("periodic", HistoryLimits::default());
        let mut history = store.create("s", snapshot("ready")).unwrap();
        let chunk = vec![b'p'; 16 * 1024];
        let mut seq = 0u64;
        while seq < CHECKPOINT_EVERY + (chunk.len() as u64) {
            let checkpoint = history
                .needs_checkpoint(chunk.len())
                .then(|| snapshot("current"));
            history.append(seq, &chunk, None).unwrap();
            if let Some(current) = checkpoint {
                history
                    .checkpoint(history.end_seq(), current)
                    .expect("periodic checkpoint");
            }
            seq += chunk.len() as u64;
        }

        let checkpoint: Checkpoint =
            serde_json::from_slice(&fs::read(history.dir().join(CHECKPOINT_FILE)).unwrap())
                .unwrap();
        assert!(
            checkpoint.seq >= CHECKPOINT_EVERY,
            "周期 checkpoint 应推进到新的 seq，实际 {}",
            checkpoint.seq
        );
        assert_eq!(checkpoint.generation, history.generation());
        assert_eq!(
            history.replay_from(0).unwrap().unwrap().len() as u64,
            history.end_seq(),
            "周期 checkpoint 不应截断日志"
        );

        let dir = history.dir().to_path_buf();
        drop(history);
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(store.root());
    }

    #[test]
    fn rotation_bounds_a_session_and_drops_older_offsets() {
        let limits = HistoryLimits {
            checkpoint_max: 4 * 1024,
            total_budget: HISTORY_BUDGET,
        };
        let store = temp_store("rotate", limits);
        let mut history = store.create("s", snapshot("ready")).unwrap();
        let chunk = vec![b'x'; 1024];
        let mut seq = 0;
        for _ in 0..12 {
            let rotate = history.would_rotate(chunk.len());
            history
                .append(seq, &chunk, rotate.then_some(snapshot("rotated")))
                .unwrap();
            seq += chunk.len() as u64;
        }
        assert!(history.log_size() + history.checkpoint_size() <= limits.checkpoint_max + 4096);
        assert_eq!(history.end_seq(), seq);
        assert_eq!(
            history.replay_from(0).unwrap(),
            None,
            "轮转后起点之前的偏移必须报告缺口"
        );
        let tail = history
            .replay_from(history.start_seq())
            .unwrap()
            .expect("轮转后的起点必须还能读");
        assert_eq!(tail.len() as u64, history.end_seq() - history.start_seq());
        assert!(tail.iter().all(|byte| *byte == b'x'));
        let dir = history.dir().to_path_buf();
        drop(history);
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(store.root());
    }

    #[test]
    fn gc_removes_the_oldest_inactive_history_and_keeps_active() {
        let store = temp_store(
            "gc",
            HistoryLimits {
                checkpoint_max: CHECKPOINT_MAX,
                total_budget: HISTORY_BUDGET,
            },
        );
        let mut dirs = Vec::new();
        for sid in ["old", "middle", "new"] {
            let history = store.create(sid, snapshot("ready")).unwrap();
            dirs.push(history.dir().to_path_buf());
            drop(history);
            thread::sleep(Duration::from_millis(15));
        }
        let total: u64 = dirs.iter().map(|dir| directory_usage(dir).0).sum();
        assert!(total > 3, "测试目录应非空");

        // 比当前总量少 1 字节：至少得删掉一个目录才能回到预算内。
        let budget = total - 1;
        let gc = HistoryStore::with_limits(
            store.root().to_path_buf(),
            HistoryLimits {
                checkpoint_max: CHECKPOINT_MAX,
                total_budget: budget,
            },
        );
        let report = gc.collect_garbage(&["new".to_owned()]);
        assert!(report.removed_dirs >= 1, "预算超出时必须删旧目录");
        assert!(report.remaining_bytes <= budget);
        assert!(dirs[2].exists(), "活跃会话的历史不能删");
        assert!(!dirs[0].exists(), "最旧的目录应先被删");
        let _ = fs::remove_dir_all(store.root());
    }

    #[test]
    fn too_large_a_checkpoint_is_rejected_instead_of_written_unbounded() {
        let limits = HistoryLimits {
            checkpoint_max: 64,
            total_budget: HISTORY_BUDGET,
        };
        let store = temp_store("large", limits);
        let error = store
            .create("s", snapshot(&"x".repeat(1024)))
            .expect_err("超过 checkpoint 上限必须失败");
        assert!(matches!(error, HistoryError::CheckpointTooLarge { .. }));
        let _ = fs::remove_dir_all(store.root());
    }
}

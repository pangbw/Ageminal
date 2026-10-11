//! 已打开文件的变更监听（#61；REQUIREMENTS.md §9「文件变更实时刷新」）。
//!
//! 只监听**调用方显式给定的文件**（= 前端当前打开着的预览页签），不去递归整个工作树
//! （#61 决策）：事件量因此有界，不必碰 `node_modules` / `target`，也不需要忽略过滤。
//!
//! 做法：对每个被监听文件的**父目录**做**非递归**监听（编辑器的原子保存 = 写临时文件再改名，
//! 只盯文件本身会漏），事件回调把命中集合内路径的变更累积起来，工作线程按 [`DEBOUNCE`] 去抖
//! ——窗口内没有新事件才把这一批（去重后）交给 `sink`。
//!
//! `Access` 类事件（读 / 打开）**一律忽略**：我们自己 `read_preview` 一个文件就会触发它，否则
//! 会自我触发成刷新回环。
//!
//! 本模块**无 UI 依赖**：`sink` 是普通回调，Tauri 的 `emit` 由调用方接。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};

use crate::error::FilesError;

/// 变更去抖窗口：窗口内没有新事件才回推，突发写合并成一批（#61）。
pub const DEBOUNCE: Duration = Duration::from_millis(150);

/// 监听线程与事件回调之间共享的状态。
#[derive(Default)]
struct State {
    /// 被监听文件的规范化键。
    files: HashSet<String>,
    /// 自上次回推以来命中的变更路径。
    pending: Vec<PathBuf>,
    /// 置位后工作线程退出。
    stopping: bool,
}

/// 已打开文件的变更监听器。
///
/// 生命周期由持有者（Tauri 的 `State`）决定：`set_files` 替换监听集合，`Drop` 停线程。
pub struct FileWatcher {
    shared: Arc<(Mutex<State>, Condvar)>,
    watcher: Mutex<RecommendedWatcher>,
    /// 当前已监听的目录（`set_files` 的差量依据）。
    dirs: Mutex<HashSet<PathBuf>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl FileWatcher {
    /// 起一个监听器；变更批次经 `sink` 回推（去重后的绝对路径）。
    pub fn start<F>(sink: F) -> Result<Self, FilesError>
    where
        F: Fn(Vec<PathBuf>) + Send + Sync + 'static,
    {
        let shared = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let sink: Arc<dyn Fn(Vec<PathBuf>) + Send + Sync> = Arc::new(sink);

        let handler = {
            let shared = Arc::clone(&shared);
            move |result: notify::Result<Event>| {
                let Ok(event) = result else { return };
                // 读 / 打开文件也会产生 Access——忽略，免得自己读预览就把自己刷起来。
                if matches!(event.kind, EventKind::Access(_)) {
                    return;
                }
                let (lock, condvar) = &*shared;
                let Ok(mut state) = lock.lock() else { return };
                for path in event.paths {
                    if state.files.contains(&key(&path)) {
                        state.pending.push(path);
                    }
                }
                if !state.pending.is_empty() {
                    condvar.notify_one();
                }
            }
        };

        let watcher =
            notify::recommended_watcher(handler).map_err(|source| FilesError::Watch { source })?;

        let worker = {
            let shared = Arc::clone(&shared);
            let sink = Arc::clone(&sink);
            thread::spawn(move || run(&shared, sink.as_ref()))
        };

        Ok(Self {
            shared,
            watcher: Mutex::new(watcher),
            dirs: Mutex::new(HashSet::new()),
            worker: Mutex::new(Some(worker)),
        })
    }

    /// 用 `files` 替换整个监听集合（只增删有变化的目录）。
    pub fn set_files(&self, files: &[PathBuf]) -> Result<(), FilesError> {
        {
            let (lock, _) = &*self.shared;
            let mut state = lock.lock().unwrap_or_else(|poison| poison.into_inner());
            state.files = files.iter().map(|file| key(file)).collect();
        }

        let wanted: HashSet<PathBuf> = files
            .iter()
            .filter_map(|file| file.parent().map(Path::to_path_buf))
            .collect();

        let mut dirs = self
            .dirs
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let mut watcher = self
            .watcher
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());

        let mut applied = HashSet::new();
        let mut first_error = None;
        for dir in &wanted {
            if dirs.contains(dir) {
                applied.insert(dir.clone());
                continue;
            }
            match watcher.watch(dir, RecursiveMode::NonRecursive) {
                Ok(()) => {
                    applied.insert(dir.clone());
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        for dir in dirs.difference(&wanted) {
            let _ = watcher.unwatch(dir);
        }
        *dirs = applied;

        match first_error {
            Some(source) => Err(FilesError::Watch { source }),
            None => Ok(()),
        }
    }
}

impl Drop for FileWatcher {
    fn drop(&mut self) {
        {
            let (lock, condvar) = &*self.shared;
            let mut state = lock.lock().unwrap_or_else(|poison| poison.into_inner());
            state.stopping = true;
            condvar.notify_all();
        }
        if let Some(worker) = self
            .worker
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take()
        {
            let _ = worker.join();
        }
    }
}

/// 工作线程：等第一批事件，再等到一个静默窗口结束，回推这一批。
fn run(shared: &(Mutex<State>, Condvar), sink: &(dyn Fn(Vec<PathBuf>) + Send + Sync)) {
    let (lock, condvar) = shared;
    loop {
        let mut state = lock.lock().unwrap_or_else(|poison| poison.into_inner());
        while state.pending.is_empty() && !state.stopping {
            state = condvar
                .wait(state)
                .unwrap_or_else(|poison| poison.into_inner());
        }
        if state.stopping {
            return;
        }
        // 去抖：只要窗口内还有新事件就继续等。
        while !state.stopping {
            let (next, timeout) = condvar
                .wait_timeout(state, DEBOUNCE)
                .unwrap_or_else(|poison| poison.into_inner());
            state = next;
            if timeout.timed_out() {
                break;
            }
        }
        if state.stopping {
            return;
        }
        let batch = std::mem::take(&mut state.pending);
        drop(state);
        if !batch.is_empty() {
            sink(dedup(batch));
        }
    }
}

/// 按路径去重，保留首次出现的顺序。
fn dedup(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter(|path| seen.insert(key(path)))
        .collect()
}

/// 路径比较键：Windows 不区分大小写，且把分隔符统一（notify 可能回 `\`）。
#[cfg(windows)]
fn key(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\").to_lowercase()
}

/// 路径比较键：其它平台用原文本。
#[cfg(not(windows))]
fn key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

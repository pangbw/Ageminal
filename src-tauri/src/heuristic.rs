//! 降级状态的外部适配（#81）：`sysinfo` 进程快照 + 当前设置里的识别清单。
//!
//! 纯状态机在 `ageminal-agent`；这里只负责把系统进程与 `settings.json` 转成它认识的形状。
//! **不解析终端输出内容、不读窗口标题、不看 CPU**。

use std::sync::Mutex;
use std::time::Duration;

use ageminal_agent::{ProcessNode, RecognitionRule};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use tauri::Manager;

use crate::agents;
use crate::storage::Store;

/// 进程树轮询间隔（#26：1–2 s）。
pub const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// 复用的 `sysinfo::System`；每次只刷新进程，不拉 CPU / 内存全量。
pub struct ProcessScanner {
    system: System,
}

impl Default for ProcessScanner {
    fn default() -> Self {
        Self {
            system: System::new(),
        }
    }
}

impl ProcessScanner {
    /// 拍一张全系统进程快照，转换成纯逻辑 crate 的 [`ProcessNode`]。
    pub fn scan(&mut self) -> Vec<ProcessNode> {
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing()
                .with_cmd(UpdateKind::OnlyIfNotSet)
                .with_exe(UpdateKind::OnlyIfNotSet),
        );
        self.system
            .processes()
            .values()
            .map(|process| ProcessNode {
                pid: process.pid().as_u32(),
                parent_pid: process.parent().map(|pid| pid.as_u32()),
                name: process.name().to_string_lossy().into_owned(),
                exe: process
                    .exe()
                    .map(|path| path.to_string_lossy().into_owned()),
                cmd: process
                    .cmd()
                    .iter()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect(),
            })
            .collect()
    }
}

/// 当前设置下的识别清单；设置页保存后，下一轮轮询自动生效。
pub fn recognition_rules(app: &tauri::AppHandle) -> Vec<RecognitionRule> {
    app.try_state::<Mutex<Store>>()
        .and_then(|store| {
            store
                .lock()
                .ok()
                .map(|guard| agents::recognition_rules(guard.settings()))
        })
        .unwrap_or_else(agents::default_recognition_rules)
}

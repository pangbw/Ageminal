//! 文档加载 / 保存的通用机制：`schemaVersion`、迁移链、前向兼容、损坏恢复。
//!
//! 迁移与恢复都作用在**原始 JSON**（而不是反序列化后的结构）上，
//! 这样未知字段能被原样保留（见 REQUIREMENTS.md §14 前向兼容）。

use std::fs;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use super::atomic::{self, BackupKind};
use super::{BackupInfo, Notice, StoreError};

/// 缺失 `schemaVersion` 时视为 1。
pub(crate) const DEFAULT_SCHEMA_VERSION: u32 = 1;

/// JSON 中版本字段的键名（各文档结构都按 camelCase 序列化）。
pub(crate) const SCHEMA_VERSION_KEY: &str = "schemaVersion";

/// 一步迁移：把文档从 `from` 迁到 `from + 1`。
pub(crate) struct Migration {
    pub from: u32,
    pub apply: fn(&mut Value) -> Result<(), String>,
}

/// 可持久化文档。
pub(crate) trait Document: Serialize + DeserializeOwned + Default {
    /// 文件名（如 `settings.json`）。
    const FILE: &'static str;
    /// 当前代码能写出的版本。
    const CURRENT_VERSION: u32;
    /// 有序迁移链（每次只走一步）。
    fn migrations() -> &'static [Migration];
    fn schema_version(&self) -> u32;
}

pub(crate) fn path_for<T: Document>(root: &Path) -> PathBuf {
    root.join(T::FILE)
}

fn backup_dir(root: &Path) -> PathBuf {
    root.join("backups")
}

/// 读取文档；缺失则写入默认值，损坏 / 不可读 / 迁移失败则备份并重建。
///
/// 返回文档与加载期间产生的提示（由上层决定何时告知用户）。
pub(crate) fn load<T: Document>(root: &Path) -> Result<(T, Vec<Notice>), StoreError> {
    let path = path_for::<T>(root);
    let file = T::FILE.to_owned();
    let mut notices = Vec::new();

    if !path.exists() {
        let default = T::default();
        save(root, &default)?;
        return Ok((default, notices));
    }

    // 读失败、非 UTF-8、非法 JSON、根不是对象 —— 一律按「损坏」处理。
    let parsed = fs::read(&path)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_object);

    let Some(mut value) = parsed else {
        notices.push(quarantine::<T>(root, &path, &file)?);
        return rebuild::<T>(root, notices);
    };

    let found = value
        .get(SCHEMA_VERSION_KEY)
        .and_then(Value::as_u64)
        .map(|raw| u32::try_from(raw).unwrap_or(u32::MAX))
        .unwrap_or(DEFAULT_SCHEMA_VERSION);

    if found > T::CURRENT_VERSION {
        // 前向兼容：可写、保留未知字段，仅提示一次。
        notices.push(Notice::WrittenByNewerVersion {
            file: file.clone(),
            found,
            current: T::CURRENT_VERSION,
        });
    } else if found < T::CURRENT_VERSION {
        // 先备份，再逐步迁移，成功后原子写回。
        let backup = atomic::copy_to_backup(&path, &backup_dir(root), BackupKind::Migrate)
            .map_err(|source| StoreError::io(&file, source))?;
        if let Err(reason) = run_chain::<T>(&mut value, found) {
            let failed = atomic::rename_backup(&backup, BackupKind::MigrateFailed)
                .map_err(|source| StoreError::io(&file, source))?;
            notices.push(Notice::MigrationFailed {
                backup: BackupInfo {
                    file: file.clone(),
                    path: failed,
                },
                reason,
            });
            return rebuild::<T>(root, notices);
        }
        write_value(&path, &value, &file)?;
    }

    let document = match serde_json::from_value::<T>(value) {
        Ok(document) => document,
        Err(_) => {
            notices.push(quarantine::<T>(root, &path, &file)?);
            return rebuild::<T>(root, notices);
        }
    };

    // 本轮可能新增了迁移前的安全备份，统一收口。
    let _ = atomic::gc_backups(&backup_dir(root));
    Ok((document, notices))
}

/// 原子写回文档，并把 `schemaVersion` 归到文档自身的版本。
pub(crate) fn save<T: Document>(root: &Path, document: &T) -> Result<(), StoreError> {
    let file = T::FILE.to_owned();
    let mut value =
        serde_json::to_value(document).map_err(|source| StoreError::serialize(&file, source))?;
    if let Some(object) = value.as_object_mut() {
        object.insert(
            SCHEMA_VERSION_KEY.to_owned(),
            Value::from(document.schema_version()),
        );
    }
    write_value(&path_for::<T>(root), &value, &file)
}

fn write_value(path: &Path, value: &Value, file: &str) -> Result<(), StoreError> {
    let mut bytes =
        serde_json::to_vec_pretty(value).map_err(|source| StoreError::serialize(file, source))?;
    bytes.push(b'\n');
    atomic::write_atomic(path, &bytes).map_err(|source| StoreError::io(file, source))
}

/// 把损坏 / 不可读的文档挪进备份目录。
fn quarantine<T: Document>(root: &Path, path: &Path, file: &str) -> Result<Notice, StoreError> {
    let path = atomic::move_to_backup(path, &backup_dir(root), BackupKind::Corrupt)
        .map_err(|source| StoreError::io(file, source))?;
    Ok(Notice::CorruptRecovered {
        backup: BackupInfo {
            file: file.to_owned(),
            path,
        },
    })
}

/// 用默认文档重建，并收口备份数量。
fn rebuild<T: Document>(root: &Path, notices: Vec<Notice>) -> Result<(T, Vec<Notice>), StoreError> {
    let default = T::default();
    save(root, &default)?;
    let _ = atomic::gc_backups(&backup_dir(root));
    Ok((default, notices))
}

/// 按 `from` 逐步跑迁移链，最后把 `schemaVersion` 写成当前版本。
fn run_chain<T: Document>(value: &mut Value, from: u32) -> Result<(), String> {
    let mut version = from;
    while version < T::CURRENT_VERSION {
        let step = T::migrations()
            .iter()
            .find(|migration| migration.from == version)
            .ok_or_else(|| format!("缺少 {version} → {} 的迁移", version + 1))?;
        (step.apply)(value).map_err(|reason| format!("{version} → {}：{reason}", version + 1))?;
        version += 1;
    }
    if let Some(object) = value.as_object_mut() {
        object.insert(
            SCHEMA_VERSION_KEY.to_owned(),
            Value::from(T::CURRENT_VERSION),
        );
    }
    Ok(())
}

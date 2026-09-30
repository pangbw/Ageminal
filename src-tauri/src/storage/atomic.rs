//! 原子写与备份（REQUIREMENTS.md §14）。
//!
//! 写路径：临时文件 → fsync → rename。Windows 上 `std::fs::rename` 会替换已存在目标。

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 每个文档最多保留的备份份数。
pub(crate) const MAX_BACKUPS: usize = 20;

/// 备份类型，决定文件名里的标记段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackupKind {
    /// 文档损坏或不可读。
    Corrupt,
    /// 迁移前的安全备份。
    Migrate,
    /// 迁移失败。
    MigrateFailed,
}

impl BackupKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Corrupt => "corrupt",
            Self::Migrate => "migrate",
            Self::MigrateFailed => "migrate-failed",
        }
    }
}

/// 原子写：先写临时文件并 fsync，再整体替换目标文件。
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)
}

/// 复制一份到 `backups/<file>.<kind>-<ts>`（保留原文件）。
pub(crate) fn copy_to_backup(
    path: &Path,
    backup_dir: &Path,
    kind: BackupKind,
) -> io::Result<PathBuf> {
    transfer_to_backup(
        |from, to| fs::copy(from, to).map(|_| ()),
        path,
        backup_dir,
        kind,
    )
}

/// 移动到 `backups/<file>.<kind>-<ts>`（原文件不再存在）。
pub(crate) fn move_to_backup(
    path: &Path,
    backup_dir: &Path,
    kind: BackupKind,
) -> io::Result<PathBuf> {
    // 用闭包包一层：`fs::rename` 对 `AsRef<Path>` 泛型，不能直接当函数值传。
    transfer_to_backup(|from, to| fs::rename(from, to), path, backup_dir, kind)
}

/// 把已有备份改名为另一种类型（迁移前的安全备份 → 迁移失败备份）。
pub(crate) fn rename_backup(from: &Path, kind: BackupKind) -> io::Result<PathBuf> {
    let dir = from.parent().unwrap_or_else(|| Path::new("."));
    let file = from
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document".to_owned());
    // 名字形如 `<file>.<old-kind>-<ts>`，只剥掉最后一段。
    let (stem, _) = file.rsplit_once('.').unwrap_or((file.as_str(), ""));
    let dest = uniquify(dir.join(format!("{stem}.{}-{}", kind.as_str(), timestamp())));
    fs::rename(from, &dest)?;
    Ok(dest)
}

/// 备份目录里只保留最新的 [`MAX_BACKUPS`] 份。
pub(crate) fn gc_backups(backup_dir: &Path) -> io::Result<()> {
    let mut entries: Vec<(SystemTime, PathBuf)> = match fs::read_dir(backup_dir) {
        Ok(dir) => dir
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_file())
            .map(|entry| {
                let modified = entry
                    .metadata()
                    .and_then(|meta| meta.modified())
                    .unwrap_or(UNIX_EPOCH);
                (modified, entry.path())
            })
            .collect(),
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };

    if entries.len() <= MAX_BACKUPS {
        return Ok(());
    }
    entries.sort_by_key(|entry| entry.0);
    for (_, path) in entries.iter().take(entries.len() - MAX_BACKUPS) {
        let _ = fs::remove_file(path);
    }
    Ok(())
}

fn transfer_to_backup(
    transfer: impl FnOnce(&Path, &Path) -> io::Result<()>,
    path: &Path,
    backup_dir: &Path,
    kind: BackupKind,
) -> io::Result<PathBuf> {
    fs::create_dir_all(backup_dir)?;
    let dest = unique_destination(backup_dir, path, kind);
    transfer(path, &dest)?;
    Ok(dest)
}

fn unique_destination(backup_dir: &Path, path: &Path, kind: BackupKind) -> PathBuf {
    let file = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document".to_owned());
    uniquify(backup_dir.join(format!("{file}.{}-{}", kind.as_str(), timestamp())))
}

/// 同一秒内可能产生同名备份；加序号避免覆盖。
fn uniquify(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let extension = path
        .extension()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    for index in 1..1000u32 {
        let candidate = path.with_extension(format!("{extension}-{index}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    path
}

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

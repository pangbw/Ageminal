//! 原子写与备份（REQUIREMENTS.md §14）。
//!
//! 写路径：临时文件 → fsync → rename。Windows 上 `std::fs::rename` 会替换已存在目标。

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 每个文档最多保留的备份份数（含各类损坏 / 迁移失败备份）。
pub(crate) const MAX_BACKUPS: usize = 20;

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

/// 把文件复制一份到 `backups/<file>.<kind>-<ts>`（保留原文件）。
pub(crate) fn copy_to_backup(path: &Path, backup_dir: &Path, kind: &str) -> io::Result<PathBuf> {
    let dest = unique_destination(backup_dir, path, kind)?;
    fs::copy(path, &dest)?;
    Ok(dest)
}

/// 把文件移动到 `backups/<file>.<kind>-<ts>`（原文件不再存在）。
pub(crate) fn move_to_backup(path: &Path, backup_dir: &Path, kind: &str) -> io::Result<PathBuf> {
    let dest = unique_destination(backup_dir, path, kind)?;
    fs::rename(path, &dest)?;
    Ok(dest)
}

/// 把已有的备份改名为另一种类型（例如迁移前的安全备份 → 迁移失败备份）。
pub(crate) fn rename_backup(from: &Path, kind: &str) -> io::Result<PathBuf> {
    let dir = from.parent().unwrap_or_else(|| Path::new("."));
    let file = from
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document".to_owned());
    // 已有的名字形如 `<file>.<old-kind>-<ts>`，只剥掉最后一段 `<old-kind>-<ts>`。
    let (stem, _) = file.rsplit_once('.').unwrap_or((file.as_str(), ""));
    let dest = dir.join(format!("{stem}.{kind}-{}", timestamp()));
    let dest = uniquify(dest);
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

fn unique_destination(backup_dir: &Path, path: &Path, kind: &str) -> io::Result<PathBuf> {
    fs::create_dir_all(backup_dir)?;
    let file = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document".to_owned());
    Ok(uniquify(
        backup_dir.join(format!("{file}.{kind}-{}", timestamp())),
    ))
}

/// 同一秒内可能产生同名备份；加序号避免覆盖。
fn uniquify(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    for index in 1..1000u32 {
        let candidate = path.with_extension(format!(
            "{}-{index}",
            path.extension()
                .map(|s| s.to_string_lossy())
                .unwrap_or_default()
        ));
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

//! 文件写操作：新建 / 重命名 / 删除（#62；REQUIREMENTS.md §9）。
//!
//! 全部先做**路径校验**（须在 worktree 根内，§15），再落盘；删除走**回收站**（`trash`），
//! 回收站不可用时把 `trash` 的错误原样带出——**绝不静默永久删除**（§9）。
//!
//! 名称参数单独收（不是整条路径）：这样「名字里混进分隔符 / `..`」在进 OS 之前就被挡住，
//! 而不是拼进路径后靠 `relative_to` 兜底。

use std::path::{Path, PathBuf};

use crate::error::FilesError;

/// 新建一个空文件；返回其绝对路径。目标已存在或名称非法时报错。
pub fn create_file(root: &Path, dir: &Path, name: &str) -> Result<PathBuf, FilesError> {
    crate::relative_to(root, dir)?;
    let path = dir.join(validate_name(name)?);
    if path.exists() {
        return Err(FilesError::AlreadyExists { path });
    }
    std::fs::write(&path, b"").map_err(|source| crate::io_error(&path, source))?;
    Ok(path)
}

/// 新建一个目录（父目录须已存在）；返回其绝对路径。
pub fn create_dir(root: &Path, dir: &Path, name: &str) -> Result<PathBuf, FilesError> {
    crate::relative_to(root, dir)?;
    let path = dir.join(validate_name(name)?);
    if path.exists() {
        return Err(FilesError::AlreadyExists { path });
    }
    std::fs::create_dir(&path).map_err(|source| crate::io_error(&path, source))?;
    Ok(path)
}

/// 就地重命名（改 basename，不移动）；返回新路径。
pub fn rename(root: &Path, path: &Path, new_name: &str) -> Result<PathBuf, FilesError> {
    crate::relative_to(root, path)?;
    if !path.exists() {
        return Err(FilesError::NotFound {
            path: path.to_path_buf(),
        });
    }
    let parent = path.parent().ok_or_else(|| FilesError::InvalidName {
        name: new_name.to_owned(),
    })?;
    let target = parent.join(validate_name(new_name)?);
    // 目标仍须落在 worktree 内（重命名 worktree 根时会走到这里被挡下）。
    crate::relative_to(root, &target)?;
    if target.exists() {
        return Err(FilesError::AlreadyExists { path: target });
    }
    std::fs::rename(path, &target).map_err(|source| crate::io_error(&target, source))?;
    Ok(target)
}

/// 移入**回收站**（可恢复）。路径须在 worktree 内且存在。
pub fn delete(root: &Path, path: &Path) -> Result<(), FilesError> {
    crate::relative_to(root, path)?;
    if !path.exists() {
        return Err(FilesError::NotFound {
            path: path.to_path_buf(),
        });
    }
    trash::delete(path).map_err(|source| FilesError::Trash {
        path: path.to_path_buf(),
        source,
    })
}

/// 校验一个「名字」（单一路径分量）：非空、不是 `.` / `..`、不含路径分隔符。
fn validate_name(name: &str) -> Result<&str, FilesError> {
    let invalid = || FilesError::InvalidName {
        name: name.to_owned(),
    };
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return Err(invalid());
    }
    // 控制字符在 Windows 文件系统里非法，也一并挡住。
    if name.chars().any(|ch| ch.is_control()) {
        return Err(invalid());
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("ageminal-ops-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn creates_a_file_and_a_directory() {
        let root = workspace("create");

        let file = create_file(&root, &root, "a.txt").unwrap();
        assert!(file.is_file());

        let dir = create_dir(&root, &root, "sub").unwrap();
        assert!(dir.is_dir());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_creating_over_an_existing_entry() {
        let root = workspace("create-exists");
        std::fs::write(root.join("a.txt"), b"x").unwrap();

        let error = create_file(&root, &root, "a.txt").unwrap_err();
        assert!(matches!(error, FilesError::AlreadyExists { .. }));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_a_name_with_a_separator_or_dotdot() {
        let root = workspace("create-name");

        for name in ["", ".", "..", "a/b", "a\\b"] {
            let error = create_file(&root, &root, name).unwrap_err();
            assert!(
                matches!(error, FilesError::InvalidName { .. }),
                "名字 {name:?} 应被拒"
            );
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_creating_outside_the_root() {
        let root = workspace("create-outside");
        let outside = std::env::temp_dir();

        let error = create_file(&root, &outside, "a.txt").unwrap_err();
        assert!(matches!(error, FilesError::OutsideRoot { .. }));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn renames_in_place() {
        let root = workspace("rename");
        let from = root.join("old.txt");
        std::fs::write(&from, b"x").unwrap();

        let to = rename(&root, &from, "new.txt").unwrap();

        assert_eq!(to, root.join("new.txt"));
        assert!(!from.exists());
        assert!(to.is_file());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_renaming_onto_an_existing_entry() {
        let root = workspace("rename-exists");
        std::fs::write(root.join("a.txt"), b"a").unwrap();
        std::fs::write(root.join("b.txt"), b"b").unwrap();

        let error = rename(&root, &root.join("a.txt"), "b.txt").unwrap_err();
        assert!(matches!(error, FilesError::AlreadyExists { .. }));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_renaming_a_missing_file() {
        let root = workspace("rename-missing");

        let error = rename(&root, &root.join("nope.txt"), "x.txt").unwrap_err();
        assert!(matches!(error, FilesError::NotFound { .. }));

        let _ = std::fs::remove_dir_all(&root);
    }

    // 删除的「进回收站 / 可恢复」是手动验收（会真进回收站）；这里只测**进 trash 之前**的校验。

    #[test]
    fn rejects_deleting_a_missing_file_before_touching_the_trash() {
        let root = workspace("delete-missing");

        let error = delete(&root, &root.join("nope.txt")).unwrap_err();
        assert!(matches!(error, FilesError::NotFound { .. }));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_deleting_outside_the_root_before_touching_the_trash() {
        let root = workspace("delete-outside");
        let outside = std::env::temp_dir().join("ageminal-ops-outside.txt");

        let error = delete(&root, &outside).unwrap_err();
        assert!(matches!(error, FilesError::OutsideRoot { .. }));

        let _ = std::fs::remove_dir_all(&root);
    }
}

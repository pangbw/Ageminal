//! 诊断导出（#88）：列出、脱敏并打包 `%LOCALAPPDATA%\\Ageminal\\logs\\`。
//!
//! 默认脱敏：
//! - 用户主目录 → `<user>`；
//! - 项目路径 → `<project-N>`；
//! - daemon token → `<token>`。
//!
//! 默认**不包含**终端历史 / 对话内容。勾选「包含原始路径」后只保留原始路径，仍抹 token。

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;
use specta::Type;
use tauri::State;
use zip::write::SimpleFileOptions;

use crate::logging;
use crate::storage::Store;

/// 导出前的一个文件。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsFileView {
    /// 文件名。
    pub name: String,
    /// 绝对路径。
    pub path: String,
    /// 字节数；TS number。
    pub size: f64,
}

/// 列出可导出的日志文件（含滚动历史），不包含 terminal history。
#[tauri::command]
#[specta::specta]
pub fn diagnostics_plan() -> Vec<DiagnosticsFileView> {
    let dir = logging::logs_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut files: Vec<DiagnosticsFileView> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let metadata = entry.metadata().ok()?;
            if !metadata.is_file() {
                return None;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if !(name.starts_with("app.log")
                || name.starts_with("daemon.log")
                || name.starts_with("notify-failures.log")
                || name.starts_with("notify-success.log")
                || name.starts_with("metrics.jsonl"))
            {
                return None;
            }
            Some(DiagnosticsFileView {
                name,
                path: path.to_string_lossy().into_owned(),
                size: metadata.len() as f64,
            })
        })
        .collect();
    files.sort_by(|left, right| left.name.cmp(&right.name));
    files
}

fn redact(
    text: &str,
    include_raw_paths: bool,
    projects: &[PathBuf],
    token: Option<&str>,
) -> String {
    let mut output = text.to_owned();
    if !include_raw_paths {
        let mut projects = projects.to_vec();
        projects.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
        for (index, project) in projects.iter().enumerate() {
            output = output.replace(
                project.to_string_lossy().as_ref(),
                &format!("<project-{}>", index + 1),
            );
        }
        if let Some(home) = std::env::var_os("USERPROFILE") {
            let home = home.to_string_lossy();
            output = output.replace(home.as_ref(), "<user>");
        }
    }
    if let Some(token) = token.filter(|token| !token.is_empty()) {
        output = output.replace(token, "<token>");
    }
    output
}

/// 导出 zip；返回导出路径。默认脱敏，`include_raw_paths` 只关闭路径替换。
#[tauri::command]
#[specta::specta]
pub fn diagnostics_export(
    app: tauri::AppHandle,
    store: State<'_, Mutex<Store>>,
    include_raw_paths: bool,
) -> Result<String, String> {
    let config = ageminal_daemon::paths::config_dir().map_err(|error| error.to_string())?;
    let files = diagnostics_plan();
    if files.is_empty() {
        return Err("没有可导出的日志文件".to_owned());
    }

    let projects: Vec<PathBuf> = store
        .lock()
        .map_err(|_| "状态锁已被污染".to_owned())?
        .state()
        .projects
        .iter()
        .flat_map(|project| {
            std::iter::once(project.path.clone()).chain(project.pinned.iter().cloned())
        })
        .collect();
    let token = ageminal_protocol::DaemonInfo::read(&config)
        .ok()
        .map(|info| info.token);

    let out_dir = config.join("diagnostics");
    std::fs::create_dir_all(&out_dir).map_err(|error| error.to_string())?;
    let out_path = out_dir.join(format!("diagnostics-{}.zip", crate::now_ms()));
    let file = std::fs::File::create(&out_path).map_err(|error| error.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let mut summary = String::new();
    summary.push_str(&format!(
        "Ageminal diagnostics\nversion: {}\nexported: {}\ninclude_raw_paths: {}\n\n",
        app.package_info().version,
        crate::now_ms(),
        include_raw_paths
    ));
    for entry in &files {
        summary.push_str(&format!("{} {} bytes\n", entry.name, entry.size));
        let bytes = std::fs::read(&entry.path).unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes);
        let redacted = redact(&text, include_raw_paths, &projects, token.as_deref());
        zip.start_file(&entry.name, options)
            .map_err(|error| error.to_string())?;
        zip.write_all(redacted.as_bytes())
            .map_err(|error| error.to_string())?;
    }
    zip.start_file("summary.txt", options)
        .map_err(|error| error.to_string())?;
    zip.write_all(summary.as_bytes())
        .map_err(|error| error.to_string())?;
    zip.finish().map_err(|error| error.to_string())?;
    Ok(out_path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_redaction_removes_home_projects_and_tokens() {
        let home = std::env::var("USERPROFILE").unwrap_or_else(|_| r"C:\Users\me".to_owned());
        let project = PathBuf::from(&home).join("project");
        let text = format!(
            "{}\\other.txt\r\n{}\\a.txt token=secret",
            home,
            project.display()
        );
        let redacted = redact(&text, false, &[project], Some("secret"));
        assert!(!redacted.contains(&home));
        assert!(redacted.contains("<user>"));
        assert!(redacted.contains("<project-1>"));
        assert!(redacted.contains("<token>"));
    }

    #[test]
    fn raw_paths_keep_paths_but_still_remove_tokens() {
        let home = std::env::var("USERPROFILE").unwrap_or_else(|_| r"C:\Users\me".to_owned());
        let text = format!("{}\\a.txt token=secret", home);
        let redacted = redact(&text, true, &[], Some("secret"));
        assert!(redacted.contains(&home));
        assert!(redacted.contains("<token>"));
        assert!(!redacted.contains("secret"));
    }
}

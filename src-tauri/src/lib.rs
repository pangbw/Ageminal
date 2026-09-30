//! Ageminal 桌面壳与 app-core（见 REQUIREMENTS.md §19）。
//!
//! Rust → TS 的类型边界由 tauri-specta 唯一产出：生成物 `src/bindings.ts`
//! 入版本控制，由 `pnpm check:bindings` 校验它与源码之间无 diff。

use serde::Serialize;
use specta::Type;
use tauri_specta::{collect_commands, Builder};

/// 生成绑定的落点。基于 crate 根定位，不依赖运行时的 cwd。
pub fn bindings_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/bindings.ts")
}

/// 应用与运行时的基础信息。
#[derive(Debug, Serialize, Type)]
pub struct AppInfo {
    /// 产品名。
    pub name: String,
    /// 应用版本（单一来源 = 根 package.json）。
    pub version: String,
    /// 运行平台（`std::env::consts::OS`）。
    pub os: String,
}

/// 首个贯通 Rust → TS 的命令。
#[tauri::command]
#[specta::specta]
fn app_info(app: tauri::AppHandle) -> AppInfo {
    AppInfo {
        name: "Ageminal".to_owned(),
        version: app.package_info().version.to_string(),
        os: std::env::consts::OS.to_owned(),
    }
}

/// 命令注册的单一来源：运行与导出绑定共用。
pub fn builder() -> Builder<tauri::Wry> {
    Builder::new().commands(collect_commands![app_info])
}

/// 导出 TypeScript 绑定到 `src/bindings.ts`。
pub fn export_bindings() {
    builder()
        .export(specta_typescript::Typescript::default(), bindings_path())
        .expect("failed to export TypeScript bindings");
}

/// 启动 Tauri 应用。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(builder().invoke_handler())
        .run(tauri::generate_context!())
        .expect("failed to run Ageminal");
}

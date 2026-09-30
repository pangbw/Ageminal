//! Ageminal 桌面壳与 app-core（见 REQUIREMENTS.md §19）。
//!
//! Rust → TS 的类型边界由 tauri-specta 唯一产出：生成物 `src/bindings.ts`
//! 入版本控制，由 `pnpm check:bindings` 校验它与源码之间无 diff。

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use specta::Type;
use tauri::Manager;
use tauri_specta::{collect_commands, Builder};

pub mod storage;

/// 状态类文档的巡检间隔（去抖窗口是 500 ms，巡检比它更密）。
const STORAGE_TICK: Duration = Duration::from_millis(250);

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

/// 系统 locale（BCP-47）。
///
/// 直接调 `tauri_plugin_os` 的 Rust 函数，**不注册插件**：前端不拿 `os:*` 权限，
/// 也就不需要插件那层 JS 命令面（capabilities 保持最小）。
/// 语言检测链的第一跳；`None` 交给 `navigator.language` 兜底（见 issue #54）。
#[tauri::command]
#[specta::specta]
fn system_locale() -> Option<String> {
    tauri_plugin_os::locale()
}

/// 已持久化的界面语言；`None` = 未设置，前端按系统检测决定（issue #54）。
#[tauri::command]
#[specta::specta]
fn get_language(store: tauri::State<'_, Mutex<storage::Store>>) -> Option<String> {
    store.lock().ok()?.settings().general.language.clone()
}

/// 写入界面语言并**立即落盘**；`None` = 恢复「跟随系统」。
#[tauri::command]
#[specta::specta]
fn set_language(
    store: tauri::State<'_, Mutex<storage::Store>>,
    language: Option<String>,
) -> Result<(), String> {
    let mut guard = store.lock().map_err(|_| "设置锁已被污染".to_owned())?;
    guard
        .update_settings(|settings| settings.general.language = language)
        .map_err(|error| error.to_string())
}

/// 命令注册的单一来源：运行与导出绑定共用。
pub fn builder() -> Builder<tauri::Wry> {
    Builder::new().commands(collect_commands![
        app_info,
        system_locale,
        get_language,
        set_language
    ])
}

/// 导出 TypeScript 绑定到 `src/bindings.ts`。
pub fn export_bindings() {
    builder()
        .export(specta_typescript::Typescript::default(), bindings_path())
        .expect("failed to export TypeScript bindings");
}

/// 状态类文档去抖落盘：后台线程按 tick 巡检，去抖窗口到了才写。
fn spawn_state_flusher(handle: tauri::AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(STORAGE_TICK);
        let Some(store) = handle.try_state::<Mutex<storage::Store>>() else {
            break;
        };
        let guarded = store.lock();
        match guarded {
            Ok(mut guard) => {
                if let Err(error) = guard.flush_state_if_due(Instant::now()) {
                    eprintln!("[storage] 状态落盘失败：{error}");
                }
            }
            Err(_) => {
                eprintln!("[storage] 状态锁已被污染，停止巡检");
                break;
            }
        }
    });
}

/// 启动 Tauri 应用。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(builder().invoke_handler())
        .setup(|app| {
            match storage::Store::open_default() {
                Ok(mut store) => {
                    // 损坏 / 迁移失败 / 高版本写入等提示，取走并只报一次（#88 接管日志）。
                    for notice in store.take_notices() {
                        eprintln!("[storage] {notice:?}");
                    }
                    app.manage(Mutex::new(store));
                    spawn_state_flusher(app.handle().clone());
                }
                Err(error) => eprintln!("[storage] 初始化失败：{error}"),
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build Ageminal")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                if let Some(store) = app.try_state::<Mutex<storage::Store>>() {
                    if let Ok(mut store) = store.lock() {
                        if let Err(error) = store.flush() {
                            eprintln!("[storage] 退出前 flush 失败：{error}");
                        }
                    }
                }
            }
        });
}

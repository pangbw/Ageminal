//! Ageminal 桌面壳与 app-core（见 REQUIREMENTS.md §19）。
//!
//! Rust → TS 的类型边界由 tauri-specta 唯一产出：生成物 `src/bindings.ts`
//! 入版本控制，由 `pnpm check:bindings` 校验它与源码之间无 diff。

use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use specta::Type;
use tauri::{Emitter, Manager};
use tauri_specta::{collect_commands, Builder};

pub mod agents;
pub mod daemon;
pub mod diagnostics;
pub mod files;
pub mod heuristic;
pub mod integrations;
pub mod logging;
pub mod metrics;
pub mod notifications;
pub mod restore;
pub mod settings_api;
pub mod storage;
pub mod terminal;
pub mod tray;
pub mod watch;
pub mod workspace;

/// 状态类文档的巡检间隔（去抖窗口是 500 ms，巡检比它更密）。
const STORAGE_TICK: Duration = Duration::from_millis(250);

/// Unix 毫秒；启动对账 / launch spec 的时间戳用（#78）。
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// 生成绑定的落点。基于 crate 根定位，不依赖运行时的 cwd。
pub fn bindings_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/bindings.ts")
}

/// `Path` → 前端字符串：统一成**平台原生分隔符**。
///
/// Windows 上 git 给的 worktree 路径是 `/`，而 `PathBuf::join` 又追加 `\`——不统一的话 UI 里
/// 会出现 `D:/…\file` 这种混合路径，前端的前缀匹配 / 相对路径也会跟着错。
pub(crate) fn path_text(path: &std::path::Path) -> String {
    path.to_string_lossy()
        .replace('/', std::path::MAIN_SEPARATOR_STR)
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

/// Windows build number（xterm `windowsPty.buildNumber`，#85）。
#[tauri::command]
#[specta::specta]
fn windows_build_number() -> Option<u32> {
    #[cfg(windows)]
    {
        use tauri_plugin_os::Version;
        match tauri_plugin_os::version() {
            Version::Semantic(_, _, build) => u32::try_from(build).ok(),
            _ => None,
        }
    }
    #[cfg(not(windows))]
    {
        None
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

/// 关闭运行中的终端页签前是否二次确认（#69；默认 `true`）。
#[tauri::command]
#[specta::specta]
fn get_confirm_close(store: tauri::State<'_, Mutex<storage::Store>>) -> bool {
    store
        .lock()
        .map(|guard| guard.settings().general.confirm_close)
        .unwrap_or(true)
}

/// 写入「关闭运行中页签需确认」并**立即落盘**（#69）。
#[tauri::command]
#[specta::specta]
fn set_confirm_close(
    store: tauri::State<'_, Mutex<storage::Store>>,
    enabled: bool,
) -> Result<(), String> {
    let mut guard = store.lock().map_err(|_| "设置锁已被污染".to_owned())?;
    guard
        .update_settings(|settings| settings.general.confirm_close = enabled)
        .map_err(|error| error.to_string())
}

/// 命令注册的单一来源：运行与导出绑定共用。
pub fn builder() -> Builder<tauri::Wry> {
    Builder::new().commands(collect_commands![
        app_info,
        windows_build_number,
        system_locale,
        get_language,
        set_language,
        get_confirm_close,
        set_confirm_close,
        settings_api::general_settings,
        settings_api::set_general_settings,
        settings_api::appearance_settings,
        settings_api::set_appearance_settings,
        settings_api::project_settings_view,
        settings_api::set_project_settings,
        settings_api::open_logs_dir,
        diagnostics::diagnostics_plan,
        diagnostics::diagnostics_export,
        metrics::metrics_reset,
        agents::get_agents,
        agents::set_agent,
        integrations::integration_list,
        integrations::integration_preview,
        integrations::integration_apply,
        integrations::integration_disable,
        integrations::integration_test_delivery,
        restore::save_tabs,
        restore::restore_tabs,
        restore::terminal_set_agent_session,
        workspace::workspace_snapshot,
        workspace::add_project,
        workspace::remove_project,
        workspace::set_project_collapsed,
        workspace::set_worktree_pinned,
        workspace::rename_worktree,
        workspace::rename_user_workspace,
        workspace::set_active_worktree,
        files::read_dir,
        files::search_files,
        files::read_preview,
        files::pick_file,
        files::open_in_editor,
        files::create_entry,
        files::rename_entry,
        files::delete_entry,
        files::write_clipboard,
        files::read_clipboard,
        files::open_url,
        files::copy_files,
        files::reveal_in_explorer,
        files::open_terminal,
        terminal::terminal_open,
        terminal::terminal_list,
        terminal::terminal_write,
        terminal::terminal_resize,
        terminal::terminal_ack,
        terminal::terminal_set_visible,
        terminal::terminal_close,
        terminal::agent_statuses,
        watch::set_watched_files
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
                    tracing::warn!("[storage] 状态落盘失败：{error}");
                }
            }
            Err(_) => {
                tracing::warn!("[storage] 状态锁已被污染，停止巡检");
                break;
            }
        }
    });
}

/// 把当前窗口几何写进 `state.json`（去抖落盘）。
///
/// 用物理像素保存 / 恢复：同一台机器同一缩放下精确；DPI 改变时仍按尺寸恢复，位置若
/// 已不在任何显示器内会被忽略。最小化时不写，避免把 0×0 或托盘尺寸存进去。
fn persist_window_geometry(window: &tauri::Window) {
    if window.is_minimized().unwrap_or(false) {
        return;
    }
    let Ok(size) = window.inner_size() else {
        return;
    };
    if size.width == 0 || size.height == 0 {
        return;
    }
    let position = window.outer_position().ok();
    let maximized = window.is_maximized().unwrap_or(false);
    let Some(store) = window.app_handle().try_state::<Mutex<storage::Store>>() else {
        return;
    };
    let Ok(mut store) = store.lock() else {
        return;
    };
    let result = store.update_state(
        |state| {
            let previous = state.window.take();
            // 最大化时保留「还原后」的普通几何，否则下次启动取消最大化会得到一个最大化尺寸。
            let (width, height, x, y) = if maximized {
                previous
                    .as_ref()
                    .map(|window| (window.width, window.height, window.x, window.y))
                    .unwrap_or((
                        f64::from(size.width),
                        f64::from(size.height),
                        position.map(|position| f64::from(position.x)),
                        position.map(|position| f64::from(position.y)),
                    ))
            } else {
                (
                    f64::from(size.width),
                    f64::from(size.height),
                    position.map(|position| f64::from(position.x)),
                    position.map(|position| f64::from(position.y)),
                )
            };
            state.window = Some(storage::WindowState {
                width,
                height,
                x,
                y,
                maximized,
                extra: previous.map(|window| window.extra).unwrap_or_default(),
            });
        },
        Instant::now(),
    );
    if let Err(error) = result {
        tracing::warn!("[window] 保存窗口几何失败：{error}");
    }
}

/// 启动时应用上次的窗口尺寸 / 位置 / 最大化状态。
fn restore_window_geometry(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let Some(saved) = app.try_state::<Mutex<storage::Store>>().and_then(|store| {
        store
            .lock()
            .ok()
            .and_then(|store| store.state().window.clone())
    }) else {
        return;
    };
    if !saved.width.is_finite()
        || !saved.height.is_finite()
        || saved.width < 200.0
        || saved.height < 200.0
    {
        return;
    }
    let size = tauri::PhysicalSize::new(
        saved.width.round().clamp(200.0, 16_384.0) as u32,
        saved.height.round().clamp(200.0, 16_384.0) as u32,
    );
    if let Err(error) = window.set_size(size) {
        tracing::warn!("[window] 恢复尺寸失败：{error}");
    }
    if let (Some(x), Some(y)) = (saved.x, saved.y) {
        if x.is_finite() && y.is_finite() {
            let position = tauri::PhysicalPosition::new(x.round() as i32, y.round() as i32);
            if window_position_visible(&window, position, size) {
                if let Err(error) = window.set_position(position) {
                    tracing::warn!("[window] 恢复位置失败：{error}");
                }
            }
        }
    }
    if saved.maximized {
        if let Err(error) = window.maximize() {
            tracing::warn!("[window] 恢复最大化失败：{error}");
        }
    }
}

/// 位置至少与某个显示器相交才恢复，避免换显示器后窗口落到屏幕外。
fn window_position_visible(
    window: &tauri::WebviewWindow,
    position: tauri::PhysicalPosition<i32>,
    size: tauri::PhysicalSize<u32>,
) -> bool {
    let Ok(monitors) = window.available_monitors() else {
        return true;
    };
    if monitors.is_empty() {
        return true;
    }
    let left = i64::from(position.x);
    let top = i64::from(position.y);
    let right = left + i64::from(size.width);
    let bottom = top + i64::from(size.height);
    monitors.iter().any(|monitor| {
        let origin = monitor.position();
        let extent = monitor.size();
        let monitor_left = i64::from(origin.x);
        let monitor_top = i64::from(origin.y);
        let monitor_right = monitor_left + i64::from(extent.width);
        let monitor_bottom = monitor_top + i64::from(extent.height);
        left < monitor_right && right > monitor_left && top < monitor_bottom && bottom > monitor_top
    })
}

/// 启动 Tauri 应用。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// 无窗口 CLI 模式（卸载钩子等）；返回 `true` 时已处理并应直接退出。
pub fn run_cli() -> bool {
    if std::env::args().any(|arg| arg == "--uninstall-cleanup") {
        integrations::uninstall_cleanup();
        return true;
    }
    false
}

pub fn run() {
    logging::init();
    tauri::Builder::default()
        // 必须最先注册：重复启动时在进入 setup / 拉起 daemon 之前就聚焦已有窗口并退出。
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main_window(app);
        }))
        .plugin(tauri_plugin_notification::init())
        // 目录选择器只从 Rust 侧调用（#32 D3）；capabilities 不放开它的命令。
        .plugin(tauri_plugin_dialog::init())
        // 外部打开同样只从 Rust 侧调用（#60 D6 #20）；capabilities 不放开 plugin:opener。
        .plugin(tauri_plugin_opener::init())
        // 剪贴板同样只从 Rust 侧调用（#62）；capabilities 不放开 clipboard-manager。
        .plugin(tauri_plugin_clipboard_manager::init())
        .invoke_handler(builder().invoke_handler())
        .on_window_event(|window, event| match event {
            // 关窗先记录几何；托盘退出路径放行，普通关窗改成隐藏（#76）。
            tauri::WindowEvent::CloseRequested { api, .. } => {
                persist_window_geometry(window);
                if !tray::exit_allowed() {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
            // 最小化也收进托盘（Windows 没有单独的 Minimized 事件，靠 Resized 查状态）。
            tauri::WindowEvent::Resized(_) => {
                if window.is_minimized().unwrap_or(false) {
                    let _ = window.hide();
                } else {
                    persist_window_geometry(window);
                }
            }
            tauri::WindowEvent::Moved(_) => persist_window_geometry(window),
            _ => {}
        })
        .setup(|app| {
            let mut acrylic = false;
            match storage::Store::open_default() {
                Ok(mut store) => {
                    // 损坏 / 迁移失败 / 高版本写入等提示，取走并只报一次（#88 接管日志）。
                    for notice in store.take_notices() {
                        tracing::warn!("[storage] {notice:?}");
                    }
                    acrylic = store.settings().appearance.acrylic;
                    app.manage(Mutex::new(store));
                    spawn_state_flusher(app.handle().clone());
                    restore_window_geometry(app.handle());
                }
                Err(error) => tracing::warn!("[storage] 初始化失败：{error}"),
            }
            // git 启动定位一次；失败记在 GitTool 里，由快照转成每个项目的提示（§15）。
            app.manage(workspace::GitTool::locate());
            // 文件搜索的取消状态（#19 D8）。
            app.manage(std::sync::Arc::new(files::SearchEpoch::default()));
            // 已打开预览文件的变更监听（#61）；前端经 `set_watched_files` 下发集合。
            let handle = app.handle().clone();
            match ageminal_files::watch::FileWatcher::start(move |paths| {
                let paths = paths
                    .into_iter()
                    .map(|path| crate::path_text(&path))
                    .collect();
                let _ = handle.emit("fs-change", watch::FsChange { paths });
            }) {
                Ok(watcher) => {
                    app.manage(watch::FileWatch(watcher));
                }
                Err(error) => tracing::warn!("[watch] 初始化失败：{error}"),
            }
            // 窗口材质（#87）：transparent 是创建期属性；Acrylic 只在启动时按设置应用。
            #[cfg(windows)]
            if acrylic {
                if let Some(window) = app.get_webview_window("main") {
                    if let Err(error) =
                        window_vibrancy::apply_acrylic(&window, Some((8, 9, 9, 180)))
                    {
                        tracing::warn!("[window] Acrylic 应用失败：{error}");
                    }
                }
            }
            // 集成注入物使用稳定 shim 路径（#31 / #83）；复制失败只记录，不阻塞启动。
            integrations::ensure_shim();
            // 守护进程常驻（#64）：后台确保它在跑（复用优先），不阻塞窗口创建。
            daemon::spawn_ensure(app.package_info().version.to_string());
            // 终端服务（#68）：连接 daemon、按会话合批转发到前端 Channel。
            // 同一线程也归约 agent 状态（#80），并把更新作为 `agent-status` 全局事件推给 UI。
            app.manage(terminal::TerminalService::start(app.handle().clone()));
            // 托盘（#76）：关窗 / 最小化隐藏，菜单承载两层显式退出。
            if let Err(error) = tray::setup(app.handle()) {
                tracing::warn!("[tray] 初始化失败：{error}");
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
                            tracing::warn!("[storage] 退出前 flush 失败：{error}");
                        }
                    }
                }
            }
        });
}

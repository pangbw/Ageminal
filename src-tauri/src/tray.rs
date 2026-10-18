//! 托盘与三层退出（issue #76）。
//!
//! 三条路径必须分得清：
//! 1. **关窗** = 隐藏到托盘；UI 进程、终端和 daemon 都继续跑。
//! 2. **退出界面** = 结束 UI 进程；连接断开后 daemon 只是 `detach_all`，会话继续活。
//! 3. **结束所有会话并退出** = 唯一的真正结束入口：先让 daemon 终止所有会话，再退出 daemon
//!    与 UI。
//!
//! 托盘菜单只暴露 2 / 3 两条；1 是窗口关闭按钮的行为。单实例由 `tauri-plugin-single-instance`
//! 处理，重复启动时聚焦已有窗口。

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::terminal::TerminalService;

const MENU_SHOW: &str = "tray.show";
const MENU_EXIT_UI: &str = "tray.exit-ui";
const MENU_EXIT_ALL: &str = "tray.exit-all";

/// 只有 `exit_ui` / `exit_all` 会把它置位；置位后关闭窗口不再被拦成隐藏。
static EXIT_ALLOWED: AtomicBool = AtomicBool::new(false);

/// 托盘菜单动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayAction {
    /// 显示并聚焦主窗口。
    Show,
    /// 退出 UI，但保留 daemon / 会话。
    ExitUi,
    /// 结束所有会话并退出 daemon / UI。
    ExitAll,
}

/// 菜单 id → 动作。抽成纯函数，单测覆盖菜单接线。
fn action_for(id: &str) -> Option<TrayAction> {
    match id {
        MENU_SHOW => Some(TrayAction::Show),
        MENU_EXIT_UI => Some(TrayAction::ExitUi),
        MENU_EXIT_ALL => Some(TrayAction::ExitAll),
        _ => None,
    }
}

/// 创建托盘图标与菜单。失败只记录，不应让应用起不来。
pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, MENU_SHOW, "显示 Ageminal", true, None::<&str>)?;
    let exit_ui = MenuItem::with_id(
        app,
        MENU_EXIT_UI,
        "退出界面（会话保留）",
        true,
        None::<&str>,
    )?;
    let exit_all = MenuItem::with_id(app, MENU_EXIT_ALL, "结束所有会话并退出", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&show, &separator, &exit_ui, &exit_all])?;

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("Ageminal")
        .menu(&menu)
        // 左键显示窗口，右键才弹菜单；与常见托盘应用一致。
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| handle_menu(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| match event {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }
            | TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } => show_main_window(tray.app_handle()),
            _ => {}
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    builder.build(app)?;
    Ok(())
}

fn handle_menu(app: &AppHandle, id: &str) {
    match action_for(id) {
        Some(TrayAction::Show) => show_main_window(app),
        Some(TrayAction::ExitUi) => exit_ui(app),
        Some(TrayAction::ExitAll) => exit_all(app),
        None => {}
    }
}

/// 显示并聚焦主窗口；单实例回调也走这里。
pub fn show_main_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

/// 关闭按钮是否应该真的关闭（而不是隐藏）。只有两条退出路径会置位。
pub fn exit_allowed() -> bool {
    EXIT_ALLOWED.load(Ordering::SeqCst)
}

/// 退出 UI（保留 daemon 与会话）。
pub fn exit_ui(app: &AppHandle) {
    EXIT_ALLOWED.store(true, Ordering::SeqCst);
    app.exit(0);
}

/// 结束所有会话并退出。阻塞工作在后台线程，托盘菜单回调不卡事件循环。
pub fn exit_all(app: &AppHandle) {
    EXIT_ALLOWED.store(true, Ordering::SeqCst);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }

    let app = app.clone();
    std::thread::spawn(move || {
        let result = match app.try_state::<TerminalService>() {
            Some(service) => service.shutdown_all(),
            None => Err("终端服务不存在".to_owned()),
        };
        if let Err(error) = result {
            eprintln!("[tray] 结束所有会话失败：{error}；尝试强制结束 daemon");
            crate::daemon::force_stop();
        }
        app.exit(0);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_map_to_the_three_layers() {
        assert_eq!(action_for(MENU_SHOW), Some(TrayAction::Show));
        assert_eq!(action_for(MENU_EXIT_UI), Some(TrayAction::ExitUi));
        assert_eq!(action_for(MENU_EXIT_ALL), Some(TrayAction::ExitAll));
        assert_eq!(action_for("tray.unknown"), None);
    }
}

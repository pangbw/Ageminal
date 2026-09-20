// Spike 探针：冷启动计时（Rust 侧）
// 背景：C1 前端框架选型 spike（#47）。冷启动必须用 **release build** 的 exe 测，
// `tauri dev` 的 dev server + 未优化 bundle 不代表真实冷启动。
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::Manager;
use tauri_plugin_notification::NotificationExt;

#[derive(Clone, Serialize)]
struct StartupReport {
    /// 进程入口 -> Tauri setup 完成（Rust 侧建窗/插件初始化）
    app_to_setup_ms: u128,
    /// 进程入口的 Unix 毫秒墙钟；前端用它对齐 `performance.timeOrigin`
    t0_wall_ms: u128,
    /// setup 完成时的 Unix 毫秒墙钟
    setup_wall_ms: u128,
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_millis()
}

/// 前端在首帧就绪后调用，取回 Rust 侧时间戳以拼出完整冷启动分解。
#[tauri::command]
fn startup_report(state: tauri::State<'_, StartupReport>) -> StartupReport {
    state.inner().clone()
}

/// 把前端测得的报告写到临时目录，便于无人值守运行后直接读取。
/// 路径：`%TEMP%\agm-spike-report.json`
#[tauri::command]
fn save_report(json: String) -> Result<String, String> {
    let path = std::env::temp_dir().join("agm-spike-report.json");
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}

/// #48 探针：从 **Rust 侧**发一条桌面通知（与 G2 #32「插件只从 Rust 调」的决议一致）。
/// 目的是实测：点击这条通知后，前端能否收到任何回调。
#[tauri::command]
fn send_test_notification(app: tauri::AppHandle, title: String, body: String) -> Result<(), String> {
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|e| e.to_string())
}

/// #42 探针：读回 `spike-daemon vt-parity` 写出的四份产物。
/// （daemon 与 app 同机，直接读 `%TEMP%\agm-vt-parity\` 即可，不必过 IPC。）
#[derive(Serialize)]
struct VtParityFiles {
    meta: String,
    snap: String,
    full: Vec<u8>,
    tail: Vec<u8>,
}

#[tauri::command]
fn read_vt_parity() -> Result<VtParityFiles, String> {
    let dir = std::env::temp_dir().join("agm-vt-parity");
    let read = |n: &str| {
        std::fs::read(dir.join(n))
            .map_err(|e| format!("读 {} 失败：{e}（请先跑 spike-daemon vt-parity）", dir.join(n).display()))
    };
    Ok(VtParityFiles {
        meta: String::from_utf8_lossy(&read("meta.json")?).to_string(),
        snap: String::from_utf8_lossy(&read("snap.ansi")?).to_string(),
        full: read("full.bin")?,
        tail: read("tail.bin")?,
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 越靠近进程入口越好
    let t0 = Instant::now();
    let t0_wall = now_ms();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_os::init())
        .setup(move |app| {
            app.manage(StartupReport {
                app_to_setup_ms: t0.elapsed().as_millis(),
                t0_wall_ms: t0_wall,
                setup_wall_ms: now_ms(),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            startup_report,
            save_report,
            send_test_notification,
            read_vt_parity
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

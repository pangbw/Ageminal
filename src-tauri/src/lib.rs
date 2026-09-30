//! Ageminal 桌面壳与 app-core（见 REQUIREMENTS.md §19）。
//!
//! 本阶段只负责把窗口与前端骨架跑起来（issue #50）；
//! 命令面 / 类型边界见 issue #51，布局见 #55。

/// 启动 Tauri 应用。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("failed to run Ageminal");
}

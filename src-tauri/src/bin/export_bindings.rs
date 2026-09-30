//! 生成 TypeScript 绑定。
//!
//! 用法：`cargo run -p ageminal-desktop --bin export_bindings`
//!
//! 之所以是 bin 而不是 example：tauri-build 生成的 Windows 应用清单只嵌入 bin 目标，
//! example 缺少 comctl32 v6 清单会以 STATUS_ENTRYPOINT_NOT_FOUND 启动失败。

fn main() {
    ageminal_desktop_lib::export_bindings();
}

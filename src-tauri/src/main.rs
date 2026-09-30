// 发布构建下不额外弹出控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    ageminal_desktop_lib::run()
}

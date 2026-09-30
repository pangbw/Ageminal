//! Ageminal 会话守护进程入口。
//!
//! 本阶段（issue #50）只保证 crate 可编译；守护进程生命周期与握手见 issue #64。

fn main() {
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!(
            "ageminal-daemon {} (protocol {})",
            env!("CARGO_PKG_VERSION"),
            ageminal_protocol::PROTOCOL_VERSION
        );
        return;
    }

    eprintln!(
        "ageminal-daemon {} — 骨架，尚未实现（见 issue #64）",
        env!("CARGO_PKG_VERSION")
    );
}

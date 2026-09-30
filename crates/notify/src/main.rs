//! Ageminal agent 事件 hook shim 入口。
//!
//! 本阶段（issue #50）只保证 crate 可编译；事件投递见 issue #79。

fn main() {
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!(
            "ageminal-notify {} (protocol {})",
            env!("CARGO_PKG_VERSION"),
            ageminal_protocol::PROTOCOL_VERSION
        );
        return;
    }

    eprintln!(
        "ageminal-notify {} — 骨架，尚未实现（见 issue #79）",
        env!("CARGO_PKG_VERSION")
    );
}

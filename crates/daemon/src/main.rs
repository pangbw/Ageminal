//! Ageminal 会话守护进程入口（issue #64）。
//!
//! 由 UI 以 `DETACHED_PROCESS` 拉起、stdio 指向 NUL，所以**运行期绝不写 stdout**（见
//! `logging` 模块）：只有 `--version` 会碰 stdout，而它只用于交互式查询。

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // `--version` 是唯一会写 stdout 的路径：它只用于交互式查询（与 sidecar release 冒烟），
    // 不会出现在 detached 运行里。运行期一律走文件日志（见 logging 模块）。
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!(
            "ageminal-daemon {} (protocol {})",
            env!("CARGO_PKG_VERSION"),
            ageminal_protocol::PROTOCOL_VERSION
        );
        return ExitCode::SUCCESS;
    }

    // 卸载钩子：不写 stdout，优雅结束会话后退出。
    if args.iter().any(|arg| arg == "--shutdown") {
        #[cfg(windows)]
        {
            if let Ok(config) = ageminal_daemon::paths::config_dir() {
                let _ = ageminal_daemon::launch::shutdown_running(&config);
            }
        }
        return ExitCode::SUCCESS;
    }

    serve(&args)
}

/// Windows：跑管道服务端。其他平台：本 crate 编译得过，但没有实现（命名管道 + ConPTY 是
/// Windows 专有）。
#[cfg(windows)]
fn serve(args: &[String]) -> ExitCode {
    ageminal_daemon::server::run(args)
}

#[cfg(not(windows))]
fn serve(_args: &[String]) -> ExitCode {
    // 不写 stdout（可能为 NUL）；这里只可能是有人在非 Windows 上手跑。
    eprintln!("ageminal-daemon 仅支持 Windows（issue #64）");
    ExitCode::FAILURE
}

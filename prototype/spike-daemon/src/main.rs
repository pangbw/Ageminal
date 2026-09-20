//! #40 ConPTY 守护进程 PoC（Ageminal 决策地图 · ticket H3）
//!
//! 用法：
//!   spike-daemon daemon   [--pipe NAME] [--shell PATH] [--cols N] [--rows N] [--ring-bytes N] [--csi6n-reply]
//!   spike-daemon attach   [--pipe NAME]                    # 手动交互（Ctrl+] detach）
//!   spike-daemon status   [--pipe NAME]
//!   spike-daemon selftest [--pipe NAME] [--json PATH] [--keep] [--no-responder] [--ring-bytes N] [--shell PATH]
//!   spike-daemon version

mod conpty;
mod logbuf;
mod marker;
mod proto;
mod session;

#[cfg(feature = "vt-parity")]
mod vt_parity;

#[cfg(windows)]
mod probe;

#[cfg(windows)]
mod client;
#[cfg(windows)]
mod daemon;
#[cfg(windows)]
mod selftest;

use std::collections::HashMap;

fn args_map(argv: &[String]) -> (String, HashMap<String, String>, Vec<String>) {
    let mut cmd = String::from("selftest");
    let mut map = HashMap::new();
    let mut flags = Vec::new();
    let mut i = 0;
    if !argv.is_empty() {
        cmd = argv[0].clone();
        i = 1;
    }
    while i < argv.len() {
        let a = &argv[i];
        if let Some(rest) = a.strip_prefix("--") {
            if i + 1 < argv.len() && !argv[i + 1].starts_with("--") {
                map.insert(rest.to_string(), argv[i + 1].clone());
                i += 2;
            } else {
                flags.push(rest.to_string());
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    (cmd, map, flags)
}

fn get_u16(map: &HashMap<String, String>, k: &str, d: u16) -> u16 {
    map.get(k).and_then(|v| v.parse().ok()).unwrap_or(d)
}

fn get_usize(map: &HashMap<String, String>, k: &str, d: usize) -> usize {
    map.get(k).and_then(|v| v.parse().ok()).unwrap_or(d)
}

fn default_shell() -> String {
    std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())
}

#[tokio::main]
async fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, map, flags) = args_map(&argv);

    match cmd.as_str() {
        "version" => {
            #[cfg(windows)]
            println!("spike-daemon {}", selftest::BUILD);
            #[cfg(not(windows))]
            println!("spike-daemon (non-windows build, ConPTY 不可用)");
        }
        "daemon" => {
            #[cfg(windows)]
            {
                if let Some(p) = map.get("log") {
                    logbuf::init(std::path::Path::new(p));
                }
                let pipe = map
                    .get("pipe")
                    .cloned()
                    .unwrap_or_else(|| "\\\\.\\pipe\\ageminal-proto".to_string());
                let shell = map.get("shell").cloned().unwrap_or_else(default_shell);
                let req = conpty::PtySpawn {
                    shell,
                    cols: get_u16(&map, "cols", 120),
                    rows: get_u16(&map, "rows", 30),
                };
                let responder = flags.iter().any(|f| f == "csi6n-reply");
                let ring = get_usize(&map, "ring-bytes", 2 * 1024 * 1024);
                match session::Session::spawn(&req, ring, responder) {
                    Ok(s) => {
                        // ⚠️ 不走 println!：daemon 的 stdout 可能是 NULL/invalid，而 println! 失败会 panic
                        logbuf::log(&format!(
                            "[daemon] pipe={pipe} shell_pid={} responder={responder}",
                            s.pty.pid()
                        ));
                        if let Err(e) = daemon::serve(pipe, std::sync::Arc::clone(&s)).await {
                            eprintln!("[daemon] serve 结束：{e}");
                        }
                        let _ = s.pty.close();
                        std::process::exit(0);
                    }
                    Err(e) => {
                        eprintln!("[daemon] 起 ConPTY 失败：{e}");
                        std::process::exit(1);
                    }
                }
            }
            #[cfg(not(windows))]
            eprintln!("ConPTY 仅 Windows 可用");
        }
        "attach" => {
            #[cfg(windows)]
            {
                let pipe = map
                    .get("pipe")
                    .cloned()
                    .unwrap_or_else(|| "\\\\.\\pipe\\ageminal-proto".to_string());
                if let Err(e) = client::attach_interactive(&pipe).await {
                    eprintln!("attach 失败：{e}");
                    std::process::exit(1);
                }
            }
            #[cfg(not(windows))]
            eprintln!("ConPTY 仅 Windows 可用");
        }
        "status" => {
            #[cfg(windows)]
            {
                let pipe = map
                    .get("pipe")
                    .cloned()
                    .unwrap_or_else(|| "\\\\.\\pipe\\ageminal-proto".to_string());
                match client::attach(&pipe, 3000).await {
                    Ok(mut a) => match client::status_json(&mut a, 3000).await {
                        Ok(s) => println!("{s}"),
                        Err(e) => {
                            eprintln!("status 失败：{e}");
                            std::process::exit(1);
                        }
                    },
                    Err(e) => {
                        eprintln!("连接失败：{e}");
                        std::process::exit(1);
                    }
                }
            }
            #[cfg(not(windows))]
            eprintln!("ConPTY 仅 Windows 可用");
        }
        "vt-parity" => {
            #[cfg(feature = "vt-parity")]
            {
                let out = map
                    .get("out")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| std::env::temp_dir().join("agm-vt-parity"));
                let cols = get_usize(&map, "cols", 100);
                let rows = get_usize(&map, "rows", 30);
                let tail = get_usize(&map, "tail", 2048);
                let _ = std::fs::create_dir_all(&out);
                if let Some(f) = map.get("emit-fixture") {
                    match std::fs::write(f, vt_parity::fixture_script(cols, rows)) {
                        Ok(()) => println!("fixture 已写出：{f}（{cols}x{rows}）"),
                        Err(e) => {
                            eprintln!("写 fixture 失败：{e}");
                            std::process::exit(1);
                        }
                    }
                    std::process::exit(0);
                }
                if let Some(input) = map.get("replay") {
                    std::process::exit(vt_parity::replay(
                        std::path::PathBuf::from(input),
                        out,
                        cols,
                        rows,
                        tail,
                    ));
                }
                #[cfg(windows)]
                {
                    let seconds = get_u16(&map, "seconds", 3) as u64;
                    std::process::exit(vt_parity::conpty(out, cols, rows, tail, seconds));
                }
                #[cfg(not(windows))]
                {
                    eprintln!("非 Windows：请用 `--replay <file>`，或先用 `--emit-fixture <file>` 生成 fixture");
                    std::process::exit(64);
                }
            }
            #[cfg(not(feature = "vt-parity"))]
            eprintln!("需要以 --features vt-parity 构建");
        }
        "probe" => {
            #[cfg(windows)]
            {
                if let Some(p) = map.get("log") {
                    logbuf::init(std::path::Path::new(p));
                }
                let seconds = get_u16(&map, "seconds", 4) as u64;
                let resize = flags.iter().any(|f| f == "resize");
                let nul_stdio = flags.iter().any(|f| f == "nul-stdio");
                let shell = map.get("shell").cloned();
                std::process::exit(probe::run(shell, seconds, resize, nul_stdio));
            }
            #[cfg(not(windows))]
            eprintln!("ConPTY 仅 Windows 可用");
        }
        "selftest" => {
            #[cfg(windows)]
            {
                let pid = std::process::id();
                let pipe = map
                    .get("pipe")
                    .cloned()
                    .unwrap_or_else(|| format!("\\\\.\\pipe\\ageminal-proto-selftest-{pid}"));
                let json = map
                    .get("json")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| std::env::temp_dir().join("agm-daemon-report.json"));
                let keep = flags.iter().any(|f| f == "keep");
                let responder = !flags.iter().any(|f| f == "no-responder");
                let ring = get_usize(&map, "ring-bytes", 2 * 1024 * 1024);
                let shell = map.get("shell").cloned();
                let code = selftest::run(pipe, json, keep, responder, ring, shell).await;
                std::process::exit(code);
            }
            #[cfg(not(windows))]
            eprintln!("ConPTY 仅 Windows 可用");
        }
        other => {
            eprintln!("未知子命令：{other}");
            eprintln!("可用：daemon | attach | status | probe | selftest | vt-parity | version");
            std::process::exit(64);
        }
    }
}

//! `ageminal-notify`：agent hook / 插件调用的事件 shim（issue #79）。
//!
//! 它只做一件事：把 **agent 原生事件**经 daemon 的命名管道投递过去。统一状态机映射归 #80，
//! 所以这里不解析事件语义，只透传 `agent` / `event` / `native` / `payload`。
//!
//! # 契约（E1 #22）
//!
//! ```text
//! ageminal-notify --agent <id> --event <name> [--session <sid>] [--json <payload>]
//!                 [--native <原名>] [--strict]
//! ```
//!
//! - 命令行 > 环境变量：`--session` > `AGEMINAL_SESSION_ID`，`--agent` > `AGEMINAL_AGENT`。
//! - payload 优先级：`--json` > **最后一个像 JSON 的 argv**（Codex）> 非 TTY stdin（Claude）。
//! - 失败默认 **exit 0**（best-effort），写 `logs\notify-failures.log`；`--strict` 调试时非零退出。
//! - 不改 PATH、不写全局配置。
//!
//! # 为什么不是 daemon 的子命令
//!
//! hook 每次触发都是一个新进程。独立 exe 省掉 dispatch，也不把 Tauri / daemon 的完整依赖链
//! 拖进 hook 热路径（E1 D1 / D7）。

use std::collections::BTreeMap;
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ageminal_protocol::{env, AgentEvent, PROTOCOL_VERSION};
#[cfg(windows)]
use ageminal_protocol::{
    Caps, Frame, Hello, Message, MessageError, ProtocolError, PROTOCOL_VERSION_MIN,
};
#[cfg(windows)]
use std::time::Instant;

/// 单次 shim 的硬超时（连接 + 握手 + 投递）。
const DELIVERY_TIMEOUT: Duration = Duration::from_millis(1_000);
/// 等 stdin 的短超时：hook 的 stdin 可能是继承来的空管道，绝不能无限阻塞。
const STDIN_TIMEOUT: Duration = Duration::from_millis(250);
/// 失败日志的单文件上限（#36 会做完整轮转；这里先保证不会无限长）。
const FAILURE_LOG_LIMIT: u64 = 10 * 1024 * 1024;
/// payload 上限（D6）。
const MAX_PAYLOAD_BYTES: usize = 64 * 1024;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!(
            "ageminal-notify {} (protocol {})",
            env!("CARGO_PKG_VERSION"),
            PROTOCOL_VERSION
        );
        return ExitCode::SUCCESS;
    }

    let cli = Cli::parse(&args);
    let outcome = run(&cli);
    if !outcome.delivered {
        log_failure(&outcome.reason);
    }
    if cli.strict && !outcome.delivered {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// 命令行参数。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Cli {
    agent: Option<String>,
    event: Option<String>,
    session: Option<String>,
    json: Option<String>,
    native: Option<String>,
    strict: bool,
    /// 未被已知 flag 消费的参数；Codex 把 JSON 放在最后一项，从后往前找。
    positionals: Vec<String>,
}

impl Cli {
    fn parse(args: &[String]) -> Self {
        let mut cli = Self::default();
        let mut index = 0;
        while index < args.len() {
            let argument = &args[index];
            let (flag, inline) = argument
                .split_once('=')
                .map_or((argument.as_str(), None), |(flag, value)| {
                    (flag, Some(value.to_owned()))
                });
            match flag {
                "--agent" | "--event" | "--session" | "--json" | "--native" => {
                    let value = match inline {
                        Some(value) => value,
                        None => {
                            index += 1;
                            match args.get(index) {
                                Some(value) => value.clone(),
                                None => {
                                    index += 1;
                                    continue;
                                }
                            }
                        }
                    };
                    match flag {
                        "--agent" => cli.agent = Some(value),
                        "--event" => cli.event = Some(value),
                        "--session" => cli.session = Some(value),
                        "--json" => cli.json = Some(value),
                        "--native" => cli.native = Some(value),
                        _ => unreachable!(),
                    }
                }
                "--strict" => cli.strict = true,
                other if other.starts_with("--") => {
                    // 未知 flag：best-effort 忽略。Codex 还可能把事件 JSON 作为裸参数传来。
                }
                other => cli.positionals.push(other.to_owned()),
            }
            index += 1;
        }
        cli
    }
}

/// 解析后的关联上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Context {
    session_id: String,
    incarnation_id: Option<String>,
    agent: String,
    pipe: String,
    token: String,
    worktree: Option<String>,
}

/// 一次投递的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Outcome {
    delivered: bool,
    reason: String,
}

impl Outcome {
    fn ok() -> Self {
        Self {
            delivered: true,
            reason: String::new(),
        }
    }

    fn failed(reason: impl Into<String>) -> Self {
        Self {
            delivered: false,
            reason: reason.into(),
        }
    }
}

fn run(cli: &Cli) -> Outcome {
    let Some(event_name) = non_empty(cli.event.as_deref()) else {
        return Outcome::failed("缺少 --event");
    };
    let environment = environment_map();
    let context = match resolve_context(cli, &environment) {
        Ok(context) => context,
        Err(reason) => return Outcome::failed(reason),
    };
    let payload = match choose_payload(cli) {
        Ok(payload) => payload,
        Err(reason) => return Outcome::failed(reason),
    };
    let (payload, truncated) = normalize_payload(payload);
    let event = AgentEvent {
        sid: context.session_id,
        incarnation_id: context.incarnation_id,
        agent: context.agent,
        event: event_name,
        native: non_empty(cli.native.as_deref()),
        ts: now_ms(),
        payload,
        truncated,
        worktree: context.worktree,
    };
    match deliver(&event, &context.pipe, &context.token) {
        Ok(()) => Outcome::ok(),
        Err(reason) => Outcome::failed(reason),
    }
}

/// 命令行 > 环境变量。pipe / token 只能来自环境（D2）。
fn resolve_context(cli: &Cli, environment: &BTreeMap<String, String>) -> Result<Context, String> {
    let session_id = non_empty(cli.session.as_deref())
        .or_else(|| non_empty(environment.get(env::SESSION_ID).map(String::as_str)))
        .ok_or_else(|| format!("缺少会话 id：--session / {}", env::SESSION_ID))?;
    let agent = non_empty(cli.agent.as_deref())
        .or_else(|| non_empty(environment.get(env::AGENT).map(String::as_str)))
        .ok_or_else(|| format!("缺少 agent id：--agent / {}", env::AGENT))?;
    let pipe = non_empty(environment.get(env::PIPE).map(String::as_str))
        .ok_or_else(|| format!("缺少 {}", env::PIPE))?;
    let token = non_empty(environment.get(env::TOKEN).map(String::as_str))
        .ok_or_else(|| format!("缺少 {}", env::TOKEN))?;
    Ok(Context {
        session_id,
        incarnation_id: non_empty(environment.get(env::INCARNATION).map(String::as_str)),
        agent,
        pipe,
        token,
        worktree: non_empty(environment.get(env::WORKTREE).map(String::as_str)),
    })
}

/// payload 优先级：`--json` > 最后一个像 JSON 的 argv > 非 TTY stdin。
fn choose_payload(cli: &Cli) -> Result<Option<serde_json::Value>, String> {
    if let Some(raw) = cli.json.as_deref() {
        return parse_payload(raw).map(Some);
    }
    if let Some(raw) = cli
        .positionals
        .iter()
        .rev()
        .find(|argument| serde_json::from_str::<serde_json::Value>(argument).is_ok())
    {
        return parse_payload(raw).map(Some);
    }
    match read_stdin_payload() {
        Some(raw) if !raw.trim().is_empty() => parse_payload(&raw).map(Some),
        _ => Ok(None),
    }
}

fn parse_payload(raw: &str) -> Result<serde_json::Value, String> {
    serde_json::from_str(raw).map_err(|error| format!("payload 不是合法 JSON：{error}"))
}

/// 读取非 TTY stdin；有界等待，避免继承来的空管道把 hook 卡死。
fn read_stdin_payload() -> Option<String> {
    if std::io::stdin().is_terminal() {
        return None;
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut raw = String::new();
        let _ = std::io::stdin().read_to_string(&mut raw);
        let _ = sender.send(raw);
    });
    receiver.recv_timeout(STDIN_TIMEOUT).ok()
}

/// 超过 64 KiB 的 payload 截断为一个 JSON 字符串，并打 `truncated` 标记。
fn normalize_payload(payload: Option<serde_json::Value>) -> (Option<serde_json::Value>, bool) {
    let Some(value) = payload else {
        return (None, false);
    };
    let Ok(bytes) = serde_json::to_vec(&value) else {
        return (None, true);
    };
    if bytes.len() <= MAX_PAYLOAD_BYTES {
        return (Some(value), false);
    }
    let prefix = String::from_utf8_lossy(&bytes[..MAX_PAYLOAD_BYTES]).into_owned();
    (Some(serde_json::Value::String(prefix)), true)
}

fn environment_map() -> BTreeMap<String, String> {
    std::env::vars_os()
        .filter_map(|(name, value)| {
            Some((
                name.into_string().ok()?,
                value.to_string_lossy().into_owned(),
            ))
        })
        .collect()
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// 失败日志：`%LOCALAPPDATA%\Ageminal\logs\notify-failures.log`。
fn log_failure(reason: &str) {
    let Some(local) = std::env::var_os("LOCALAPPDATA") else {
        return;
    };
    let logs = PathBuf::from(local).join("Ageminal").join("logs");
    let _ = append_failure(&logs, reason);
}

fn append_failure(logs: &Path, reason: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(logs)?;
    let path = logs.join("notify-failures.log");
    if std::fs::metadata(&path)
        .map(|metadata| metadata.len() > FAILURE_LOG_LIMIT)
        .unwrap_or(false)
    {
        let rotated = logs.join("notify-failures.log.1");
        let _ = std::fs::remove_file(&rotated);
        let _ = std::fs::rename(&path, &rotated);
    }
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{} notify: {reason}", now_ms())
}

// ───────────────────────────── Windows 投递 ─────────────────────────────

#[cfg(windows)]
fn deliver(event: &AgentEvent, pipe: &str, token: &str) -> Result<(), String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{ERROR_PIPE_BUSY, HANDLE};
    use windows_sys::Win32::System::Pipes::PeekNamedPipe;

    fn open(pipe: &str, deadline: Instant) -> Result<std::fs::File, String> {
        loop {
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(pipe)
            {
                Ok(file) => return Ok(file),
                Err(error)
                    if Instant::now() < deadline
                        && (error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32)
                            || error.kind() == std::io::ErrorKind::NotFound
                            || error.kind() == std::io::ErrorKind::PermissionDenied) =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(format!("打开 daemon 管道失败：{error}")),
            }
        }
    }

    fn readable(file: &std::fs::File, deadline: Instant) -> Result<bool, String> {
        loop {
            let mut available = 0u32;
            let ok = unsafe {
                PeekNamedPipe(
                    file.as_raw_handle() as HANDLE,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    &mut available,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(format!("PeekNamedPipe 失败：GetLastError={}", unsafe {
                    windows_sys::Win32::Foundation::GetLastError()
                }));
            }
            if available > 0 {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn read_message(reader: &mut impl Read) -> Result<Option<Message>, String> {
        loop {
            let frame = Frame::read_from(reader).map_err(|error| error.to_string())?;
            let Some(frame) = frame else {
                return Ok(None);
            };
            match Message::from_frame(&frame) {
                Ok(message) => return Ok(Some(message)),
                Err(MessageError::UnsupportedKind(_)) => continue,
                Err(error) => return Err(error.to_string()),
            }
        }
    }

    let deadline = Instant::now() + DELIVERY_TIMEOUT;
    let mut stream = open(pipe, deadline)?;
    let hello = Message::Hello(Hello {
        proto_min: PROTOCOL_VERSION_MIN,
        proto_max: PROTOCOL_VERSION,
        caps: Caps::supported(),
        token: token.to_owned(),
        client_instance_id: format!("notify-{}-{}", std::process::id(), now_ms()),
        resume: Vec::new(),
    });
    hello
        .to_frame()
        .map_err(|error| error.to_string())?
        .write_to(&mut stream)
        .map_err(|error| error.to_string())?;

    if !readable(&stream, deadline)? {
        return Err("等待 daemon welcome 超时".to_owned());
    }
    match read_message(&mut stream)? {
        Some(Message::Welcome(_)) => {}
        Some(Message::Error(ProtocolError { code, message })) => {
            return Err(format!("daemon 拒绝握手：{code:?}：{message}"));
        }
        Some(other) => return Err(format!("握手后收到意外消息：{other:?}")),
        None => return Err("daemon 在握手前关闭了连接".to_owned()),
    }

    if Instant::now() >= deadline {
        return Err("投递超时".to_owned());
    }
    Message::AgentEvent(event.clone())
        .to_frame()
        .map_err(|error| error.to_string())?
        .write_to(&mut stream)
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(not(windows))]
fn deliver(_event: &AgentEvent, _pipe: &str, _token: &str) -> Result<(), String> {
    Err("ageminal-notify 仅支持 Windows 命名管道".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn parses_flags_inline_values_and_positionals() {
        let cli = Cli::parse(&args(&[
            "notify",
            "--agent",
            "claude",
            "--event=Stop",
            "--session",
            "s1",
            "--strict",
            r#"{"type":"agent-turn-complete"}"#,
        ]));
        assert_eq!(cli.agent.as_deref(), Some("claude"));
        assert_eq!(cli.event.as_deref(), Some("Stop"));
        assert_eq!(cli.session.as_deref(), Some("s1"));
        assert!(cli.strict);
        assert_eq!(cli.positionals.len(), 2);
    }

    #[test]
    fn command_line_wins_over_environment() {
        let cli = Cli::parse(&args(&[
            "--agent",
            "codex",
            "--event",
            "Stop",
            "--session",
            "flag-session",
        ]));
        let context = resolve_context(
            &cli,
            &env(&[
                (env::AGENT, "claude"),
                (env::SESSION_ID, "env-session"),
                (env::PIPE, r"\\.\pipe\ageminal-test"),
                (env::TOKEN, "tok"),
                (env::INCARNATION, "inc-1"),
                (env::WORKTREE, r"D:\code\wt"),
            ]),
        )
        .unwrap();
        assert_eq!(context.agent, "codex");
        assert_eq!(context.session_id, "flag-session");
        assert_eq!(context.incarnation_id.as_deref(), Some("inc-1"));
    }

    #[test]
    fn environment_is_the_default_when_flags_are_absent() {
        let cli = Cli::parse(&args(&["--event", "Stop"]));
        let context = resolve_context(
            &cli,
            &env(&[
                (env::AGENT, "claude"),
                (env::SESSION_ID, "env-session"),
                (env::PIPE, r"\\.\pipe\ageminal-test"),
                (env::TOKEN, "tok"),
            ]),
        )
        .unwrap();
        assert_eq!(context.agent, "claude");
        assert_eq!(context.session_id, "env-session");
    }

    #[test]
    fn missing_pipe_or_token_is_a_safe_failure() {
        let cli = Cli::parse(&args(&["--agent", "claude", "--event", "Stop"]));
        let error = resolve_context(
            &cli,
            &env(&[(env::SESSION_ID, "s1"), (env::AGENT, "claude")]),
        )
        .unwrap_err();
        assert!(error.contains(env::PIPE), "{error}");
    }

    #[test]
    fn payload_priority_is_flag_then_tail_json_then_stdin() {
        let cli = Cli::parse(&args(&[
            "--agent",
            "codex",
            "--event",
            "Stop",
            "--json",
            r#"{"source":"flag"}"#,
            r#"{"source":"argv"}"#,
        ]));
        let value = choose_payload(&cli).unwrap().unwrap();
        assert_eq!(value["source"], "flag");

        let cli = Cli::parse(&args(&[
            "--agent",
            "codex",
            "--event",
            "Stop",
            r#"{"source":"argv"}"#,
        ]));
        let value = choose_payload(&cli).unwrap().unwrap();
        assert_eq!(value["source"], "argv");
    }

    #[test]
    fn oversized_payload_is_truncated_and_marked() {
        let huge = serde_json::json!({"raw": "x".repeat(MAX_PAYLOAD_BYTES)});
        let (payload, truncated) = normalize_payload(Some(huge));
        assert!(truncated);
        assert!(matches!(payload, Some(serde_json::Value::String(_))));
    }

    #[cfg(windows)]
    #[test]
    fn delivers_a_handshake_and_agent_event_over_a_real_pipe() {
        use std::os::windows::io::FromRawHandle;
        use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
        use windows_sys::Win32::System::Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
            PIPE_TYPE_BYTE, PIPE_WAIT,
        };

        let pipe = format!(
            r"\\.\pipe\ageminal-notify-test-{}-{}",
            std::process::id(),
            now_ms()
        );
        let name = pipe
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                8 * 1024,
                8 * 1024,
                0,
                std::ptr::null(),
            )
        };
        assert!(!handle.is_null(), "建测试管道失败");

        struct SendHandle(isize);
        unsafe impl Send for SendHandle {}
        let raw = SendHandle(handle as isize);
        let (error_sender, error_receiver) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let handle = raw.0 as windows_sys::Win32::Foundation::HANDLE;
            unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) };
            let file = unsafe { std::fs::File::from_raw_handle(handle) };
            let mut reader = &file;
            let mut writer = &file;
            let hello = Frame::read_from(&mut reader).expect("读 hello");
            let Some(hello) = hello else {
                error_sender.send("连接提前关闭".to_owned()).unwrap();
                return;
            };
            match Message::from_frame(&hello) {
                Ok(Message::Hello(hello)) => assert_eq!(hello.token, "tok"),
                other => error_sender
                    .send(format!("期待 Hello，收到 {other:?}"))
                    .unwrap(),
            }
            Message::Welcome(ageminal_protocol::Welcome {
                proto: PROTOCOL_VERSION,
                caps: Caps::supported(),
                daemon_version: "test".to_owned(),
            })
            .to_frame()
            .unwrap()
            .write_to(&mut writer)
            .unwrap();

            let event = Frame::read_from(&mut reader).expect("读 event");
            let Some(event) = event else {
                error_sender.send("没有收到 AgentEvent".to_owned()).unwrap();
                return;
            };
            match Message::from_frame(&event) {
                Ok(Message::AgentEvent(event)) => {
                    assert_eq!(event.sid, "s1");
                    assert_eq!(event.event, "Stop");
                }
                other => error_sender
                    .send(format!("期待 AgentEvent，收到 {other:?}"))
                    .unwrap(),
            }
            unsafe { DisconnectNamedPipe(handle) };
        });

        let event = AgentEvent {
            sid: "s1".to_owned(),
            incarnation_id: Some("inc-1".to_owned()),
            agent: "claude".to_owned(),
            event: "Stop".to_owned(),
            native: None,
            ts: 1,
            payload: Some(serde_json::json!({"probe": true})),
            truncated: false,
            worktree: None,
        };
        deliver(&event, &pipe, "tok").expect("shim 投递应成功");
        server.join().unwrap();
        if let Ok(error) = error_receiver.try_recv() {
            panic!("假 daemon 报错：{error}");
        }
    }

    #[test]
    fn failure_log_is_written_under_the_logs_directory() {
        let root = std::env::temp_dir().join(format!(
            "ageminal-notify-log-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let logs = root.join("logs");
        append_failure(&logs, "missing AGEMINAL_PIPE").unwrap();
        let text = std::fs::read_to_string(logs.join("notify-failures.log")).unwrap();
        assert!(text.contains("missing AGEMINAL_PIPE"));
        let _ = std::fs::remove_dir_all(root);
    }
}

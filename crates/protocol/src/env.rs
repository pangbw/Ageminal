//! 会话关联环境变量协议（E4 #25 / #79）。
//!
//! daemon 在 spawn **每一个** PTY 会话时注入这些变量；agent / hook / 扩展进程自动继承。
//! 注入物用绝对路径，不改 PATH、不写全局配置。命令行参数（`--session` / `--agent`）优先于
//! 环境变量，`--agent` 才是权威；`AGEMINAL_AGENT` 只是缺省回退。

/// 逻辑会话 id。
pub const SESSION_ID: &str = "AGEMINAL_SESSION_ID";
/// 本次 spawn 的世代 id。
pub const INCARNATION: &str = "AGEMINAL_INCARNATION";
/// daemon 命名管道完整路径（`\\.\pipe\…`）。
pub const PIPE: &str = "AGEMINAL_PIPE";
/// daemon 握手令牌（每次 daemon 启动重生成）。
pub const TOKEN: &str = "AGEMINAL_TOKEN";
/// worktree 根绝对路径。
pub const WORKTREE: &str = "AGEMINAL_WORKTREE";
/// agent id（仅一等入口已知时作为缺省回退）。
pub const AGENT: &str = "AGEMINAL_AGENT";

/// 所有由 daemon 管理的变量名。
pub const ALL: &[&str] = &[SESSION_ID, INCARNATION, PIPE, TOKEN, WORKTREE, AGENT];

/// 是否是 daemon 保留的变量（调用方追加的 env 不得覆盖它们）。
pub fn is_reserved(name: &str) -> bool {
    ALL.iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(name))
}

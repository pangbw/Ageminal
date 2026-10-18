//! Agent 一等启动入口的配置（issue #77）。
//!
//! `settings.json` 只保存**覆盖项**（[`AgentOverride`]）；四家 agent 的 id、缺省名称 / 命令 /
//! 颜色在代码里固定，因此：
//! - 首次启动没有配置时也有完整清单；
//! - 设置页保存后只写用户真正改过的字段，将来新增字段能带缺省值；
//! - `dsh` 不在 [`AGENT_DEFAULTS`] 里，任何入口都不会意外出现它。
//!
//! 启动本身仍走普通 PTY 会话：app-core 把配置拼成**一行 shell 命令**，daemon 在 shell 起来后
//! 写进 PTY（cwd 已经是 worktree）。这样 `.cmd` / `.exe` / Git Bash 脚本都能由 shell 解析，
//! 不依赖 `CreateProcessW` 对批处理文件的特殊处理。

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use specta::Type;
use tauri::State;

use ageminal_agent::RecognitionRule;
use ageminal_workspace::ProjectSettings;

use crate::storage::{Settings, Store};

/// 一等入口支持的 agent id（顺序 = 菜单顺序）。
///
/// **不要加 dsh**：它暂不支持（REQUIREMENTS.md §10.2）。
pub const AGENT_IDS: [&str; 4] = ["claude", "opencode", "codex", "pi"];

/// 一个 agent 的完整可编辑配置（Rust → 前端）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    /// 稳定 id（`claude` / `opencode` / `codex` / `pi`）。
    pub id: String,
    /// 菜单、页签与设置页显示的名称。
    pub name: String,
    /// 可执行命令（只填程序；参数放 [`Self::args`]）。
    pub command: String,
    /// 原样追加在命令后的参数串（例如 `--model sonnet`）。
    pub args: String,
    /// 图标颜色（十六进制，如 `#d97757`）。
    pub color: String,
}

/// `settings.json` 里的覆盖项：`None` / 缺省 = 用代码里的缺省值。
///
/// `extra` 让前向兼容在**嵌套层**也成立（与 `ProjectSettings` 一致）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentOverride {
    /// 显示名称。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 命令。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// 参数串。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    /// 图标颜色。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// 未知字段原样保留。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl AgentOverride {
    /// 从完整配置生成覆盖项；空值不写，避免把缺省值固化成用户配置。
    pub fn from_config(config: &AgentConfig, default: &AgentDefault) -> Self {
        let mut over = Self::default();
        let name = config.name.trim();
        if !name.is_empty() && name != default.name {
            over.name = Some(name.to_owned());
        }
        let command = config.command.trim();
        if !command.is_empty() && command != default.command {
            over.command = Some(command.to_owned());
        }
        let args = config.args.trim();
        if !args.is_empty() {
            over.args = Some(args.to_owned());
        }
        let color = config.color.trim();
        if !color.is_empty() && !color.eq_ignore_ascii_case(default.color) {
            over.color = Some(color.to_owned());
        }
        over
    }

    /// 是否没有任何有效字段（不值得留在 `settings.json` 里）。
    pub fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.command.is_none()
            && self.args.is_none()
            && self.color.is_none()
            && self.extra.is_empty()
    }
}

/// 代码里的缺省项。
pub struct AgentDefault {
    /// 稳定 id。
    pub id: &'static str,
    /// 显示名称。
    pub name: &'static str,
    /// 命令。
    pub command: &'static str,
    /// 图标颜色。
    pub color: &'static str,
}

/// 四家 agent 的缺省配置（dsh 刻意缺席）。
pub const AGENT_DEFAULTS: [AgentDefault; 4] = [
    AgentDefault {
        id: "claude",
        name: "Claude Code",
        command: "claude",
        color: "#d97757",
    },
    AgentDefault {
        id: "opencode",
        name: "opencode",
        command: "opencode",
        color: "#f5a742",
    },
    AgentDefault {
        id: "codex",
        name: "Codex",
        command: "codex",
        color: "#10a37f",
    },
    AgentDefault {
        id: "pi",
        name: "pi",
        command: "pi",
        color: "#8b5cf6",
    },
];

/// 默认的四家 agent 识别规则（`settings.json` 缺失时也能用）。
pub fn default_recognition_rules() -> Vec<RecognitionRule> {
    AGENT_DEFAULTS
        .iter()
        .map(|agent| RecognitionRule {
            agent_id: agent.id.to_owned(),
            command: agent.command.to_owned(),
        })
        .collect()
}

/// 当前设置下的进程识别清单。
///
/// 同时保留**缺省命令**与**用户覆盖后的命令**：用户把 `claude` 改成 `claude-wrapper` 时，
/// 直接运行旧的 `claude` 仍应被识别为同一 agent（#81 的手动入口）。
pub fn recognition_rules(settings: &Settings) -> Vec<RecognitionRule> {
    let mut rules = default_recognition_rules();
    for config in resolve_all(settings) {
        rules.push(RecognitionRule {
            agent_id: config.id,
            command: config.command,
        });
    }
    rules.sort_by(|left, right| {
        (left.agent_id.as_str(), left.command.to_ascii_lowercase())
            .cmp(&(right.agent_id.as_str(), right.command.to_ascii_lowercase()))
    });
    rules.dedup_by(|left, right| {
        left.agent_id.eq_ignore_ascii_case(&right.agent_id)
            && left.command.eq_ignore_ascii_case(&right.command)
    });
    rules
}

/// 把 `settings` 里的覆盖项合并到缺省配置。
pub fn resolve(settings: &Settings, id: &str) -> Option<AgentConfig> {
    let default = AGENT_DEFAULTS.iter().find(|agent| agent.id == id)?;
    let over = settings.agents.get(id);
    Some(AgentConfig {
        id: default.id.to_owned(),
        name: over
            .and_then(|item| item.name.as_deref())
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(default.name)
            .to_owned(),
        command: over
            .and_then(|item| item.command.as_deref())
            .map(str::trim)
            .filter(|command| !command.is_empty())
            .unwrap_or(default.command)
            .to_owned(),
        args: over
            .and_then(|item| item.args.as_deref())
            .map(str::trim)
            .unwrap_or("")
            .to_owned(),
        color: over
            .and_then(|item| item.color.as_deref())
            .map(str::trim)
            .filter(|color| valid_color(color))
            .unwrap_or(default.color)
            .to_owned(),
    })
}

/// 项目级覆盖后的 agent 配置（#86）。
///
/// 项目只覆盖命令 / 参数；名称与颜色仍继承全局。
pub fn resolve_in_project(
    settings: &Settings,
    project: Option<&ProjectSettings>,
    id: &str,
) -> Option<AgentConfig> {
    let mut config = resolve(settings, id)?;
    if let Some(over) = project.and_then(|project| project.agents.get(id)) {
        if let Some(command) = over
            .command
            .as_deref()
            .map(str::trim)
            .filter(|command| !command.is_empty())
        {
            config.command = command.to_owned();
        }
        if let Some(args) = over.args.as_deref().map(str::trim) {
            config.args = args.to_owned();
        }
    }
    Some(config)
}

/// 项目级覆盖后的启动命令。
pub fn launch_command_in_project(
    settings: &Settings,
    project: Option<&ProjectSettings>,
    id: &str,
) -> Result<String, String> {
    let config =
        resolve_in_project(settings, project, id).ok_or_else(|| format!("未知的 agent：{id}"))?;
    if config.command.contains(['\r', '\n']) || config.args.contains(['\r', '\n']) {
        return Err("Agent 命令 / 参数不能包含换行".to_owned());
    }
    let mut command = quote_program(&config.command);
    let args = config.args.trim();
    if !args.is_empty() {
        command.push(' ');
        command.push_str(args);
    }
    Ok(command)
}

/// 全部四家（菜单 / 设置页用）。
pub fn resolve_all(settings: &Settings) -> Vec<AgentConfig> {
    AGENT_IDS
        .iter()
        .filter_map(|id| resolve(settings, id))
        .collect()
}

/// 拼出要写进 PTY 的启动命令。
///
/// 只对命令本身加引号（路径可能带空格）；参数是用户原样输入的一段 shell 片段，不再解析。
pub fn launch_command(settings: &Settings, id: &str) -> Result<String, String> {
    let config = resolve(settings, id).ok_or_else(|| format!("未知的 agent：{id}"))?;
    if config.command.contains(['\r', '\n']) || config.args.contains(['\r', '\n']) {
        return Err("Agent 命令 / 参数不能包含换行".to_owned());
    }
    let mut command = quote_program(&config.command);
    let args = config.args.trim();
    if !args.is_empty() {
        command.push(' ');
        command.push_str(args);
    }
    Ok(command)
}

/// 拼出 agent CLI 的 resume 命令（#78）。
///
/// `program` / `args` 是 launch spec 里保存的**首次启动**值：恢复时必须重传原始参数
/// （例如 Claude 的 `--settings`），不能只依赖当前设置。缺省时才回落到当前设置。
pub fn resume_command(
    settings: &Settings,
    id: &str,
    session_id: &str,
    program: Option<&str>,
    args: Option<&str>,
) -> Result<String, String> {
    let config = resolve(settings, id).ok_or_else(|| format!("未知的 agent：{id}"))?;
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err("agent 会话 id 为空，无法恢复".to_owned());
    }
    if session_id.contains(['\r', '\n']) {
        return Err("agent 会话 id 不能包含换行".to_owned());
    }

    let program = program
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&config.command);
    let args = args.unwrap_or(&config.args);
    if program.contains(['\r', '\n']) || args.contains(['\r', '\n']) {
        return Err("Agent 命令 / 参数不能包含换行".to_owned());
    }

    let mut command = quote_program(program);
    match id {
        "claude" => {
            command.push_str(" --resume ");
        }
        "codex" => {
            command.push_str(" resume ");
        }
        "opencode" | "pi" => {
            command.push_str(" --session ");
        }
        _ => return Err(format!("Agent {id} 不支持 CLI resume")),
    }
    command.push_str(&quote_argument(session_id));

    let args = args.trim();
    if !args.is_empty() {
        command.push(' ');
        command.push_str(args);
    }
    Ok(command)
}

/// 取全部 agent 配置（设置页 / 菜单初始化）。
#[tauri::command]
#[specta::specta]
pub fn get_agents(store: State<'_, Mutex<Store>>) -> Result<Vec<AgentConfig>, String> {
    let store = store.lock().map_err(|_| "设置锁已被污染".to_owned())?;
    Ok(resolve_all(store.settings()))
}

/// 保存一个 agent 的配置，返回合并后的完整清单。
#[tauri::command]
#[specta::specta]
pub fn set_agent(
    store: State<'_, Mutex<Store>>,
    config: AgentConfig,
) -> Result<Vec<AgentConfig>, String> {
    let default = AGENT_DEFAULTS
        .iter()
        .find(|agent| agent.id == config.id)
        .ok_or_else(|| format!("未知的 agent：{}", config.id))?;
    if config.command.contains(['\r', '\n']) || config.args.contains(['\r', '\n']) {
        return Err("Agent 命令 / 参数不能包含换行".to_owned());
    }

    let normalized = AgentConfig {
        id: default.id.to_owned(),
        name: normalize_or(&config.name, default.name),
        command: normalize_or(&config.command, default.command),
        args: config.args.trim().to_owned(),
        color: if valid_color(config.color.trim()) {
            config.color.trim().to_owned()
        } else {
            default.color.to_owned()
        },
    };

    let mut store = store.lock().map_err(|_| "设置锁已被污染".to_owned())?;
    store
        .update_settings(|settings| {
            let mut over = AgentOverride::from_config(&normalized, default);
            // 前向兼容：用户手写 / 新版本写入的未知字段不因一次保存被抹掉。
            if let Some(previous) = settings.agents.get(&normalized.id) {
                over.extra = previous.extra.clone();
            }
            if over.is_empty() {
                settings.agents.remove(&normalized.id);
            } else {
                settings.agents.insert(normalized.id.clone(), over);
            }
        })
        .map_err(|error| error.to_string())?;
    Ok(resolve_all(store.settings()))
}

fn normalize_or(value: &str, fallback: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        fallback.to_owned()
    } else {
        value.to_owned()
    }
}

fn quote_program(program: &str) -> String {
    if program
        .chars()
        .any(|character| character.is_whitespace() || character == '"')
    {
        format!("\"{}\"", program.replace('"', "\\\""))
    } else {
        program.to_owned()
    }
}

/// 会话 id 通常是 UUID / `ses_…`；只在含 shell 元字符时加引号。
fn quote_argument(argument: &str) -> String {
    if argument
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "-_./:@".contains(character))
    {
        argument.to_owned()
    } else {
        format!("\"{}\"", argument.replace('"', "\\\""))
    }
}

/// 图标颜色只接受十六进制；`type=color` 与手写 JSON 都走同一条校验。
fn valid_color(color: &str) -> bool {
    let color = color.trim();
    matches!(color.len(), 4 | 7 | 9)
        && color.starts_with('#')
        && color[1..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
}

/// `Settings.agents` 在 `storage` 里用这个缺省类型，避免每个设置项都写 `Option`。
pub type AgentOverrides = BTreeMap<String, AgentOverride>;

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_with(id: &str, over: AgentOverride) -> Settings {
        let mut settings = Settings::default();
        settings.agents.insert(id.to_owned(), over);
        settings
    }

    #[test]
    fn recognition_rules_include_defaults_and_configured_commands() {
        let mut settings = Settings::default();
        settings.agents.insert(
            "claude".to_owned(),
            AgentOverride {
                command: Some("my-claude".to_owned()),
                ..AgentOverride::default()
            },
        );
        let rules = recognition_rules(&settings);
        assert!(rules
            .iter()
            .any(|rule| rule.agent_id == "claude" && rule.command == "claude"));
        assert!(rules
            .iter()
            .any(|rule| rule.agent_id == "claude" && rule.command == "my-claude"));
        assert!(rules
            .iter()
            .any(|rule| rule.agent_id == "codex" && rule.command == "codex"));
        assert!(rules.iter().any(|rule| rule.agent_id == "opencode"));
        assert!(rules.iter().any(|rule| rule.agent_id == "pi"));
    }

    #[test]
    fn project_overrides_replace_command_and_args_only() {
        let settings = Settings::default();
        let mut project =
            ageminal_workspace::ProjectSettings::new(std::path::PathBuf::from("C:\\ws"));
        project.agents.insert(
            "claude".to_owned(),
            ageminal_workspace::ProjectAgentOverride {
                command: Some("claude-project".to_owned()),
                args: Some("--model haiku".to_owned()),
                extra: Default::default(),
            },
        );
        let config = resolve_in_project(&settings, Some(&project), "claude").unwrap();
        assert_eq!(config.command, "claude-project");
        assert_eq!(config.args, "--model haiku");
        assert_eq!(config.name, "Claude Code");
        assert_eq!(
            launch_command_in_project(&settings, Some(&project), "claude").unwrap(),
            "claude-project --model haiku"
        );
        let opencode = resolve_in_project(&settings, Some(&project), "opencode").unwrap();
        assert_eq!(opencode.command, "opencode");
    }

    #[test]
    fn defaults_are_complete_and_do_not_include_dsh() {
        let configs = resolve_all(&Settings::default());
        let ids: Vec<&str> = configs.iter().map(|agent| agent.id.as_str()).collect();
        assert_eq!(ids, AGENT_IDS);
        assert!(!ids.contains(&"dsh"));
        assert!(configs.iter().all(|agent| !agent.command.is_empty()));
        assert_eq!(configs[0].name, "Claude Code");
        assert_eq!(configs[0].command, "claude");
    }

    #[test]
    fn overrides_merge_field_by_field_and_empty_values_fall_back() {
        let settings = settings_with(
            "claude",
            AgentOverride {
                name: Some("My Claude".to_owned()),
                command: Some("custom-claude".to_owned()),
                args: Some("--model sonnet".to_owned()),
                color: Some("#123456".to_owned()),
                ..AgentOverride::default()
            },
        );
        let config = resolve(&settings, "claude").expect("claude");
        assert_eq!(config.name, "My Claude");
        assert_eq!(config.command, "custom-claude");
        assert_eq!(config.args, "--model sonnet");
        assert_eq!(config.color, "#123456");

        let settings = settings_with(
            "claude",
            AgentOverride {
                name: Some("   ".to_owned()),
                command: Some(String::new()),
                color: Some("not-a-color".to_owned()),
                ..AgentOverride::default()
            },
        );
        let config = resolve(&settings, "claude").expect("claude");
        assert_eq!(config.name, "Claude Code");
        assert_eq!(config.command, "claude");
        assert_eq!(config.color, "#d97757");
    }

    #[test]
    fn launch_command_quotes_paths_and_keeps_raw_arguments() {
        let settings = settings_with(
            "claude",
            AgentOverride {
                command: Some(r"C:\Program Files\Claude\claude.exe".to_owned()),
                args: Some("--model sonnet".to_owned()),
                ..AgentOverride::default()
            },
        );
        assert_eq!(
            launch_command(&settings, "claude").unwrap(),
            r#""C:\Program Files\Claude\claude.exe" --model sonnet"#
        );
    }

    #[test]
    fn newlines_are_rejected_before_they_reach_the_pty() {
        let settings = settings_with(
            "claude",
            AgentOverride {
                args: Some("--flag\r\necho injected".to_owned()),
                ..AgentOverride::default()
            },
        );
        assert!(launch_command(&settings, "claude").is_err());
        assert!(launch_command(&settings, "dsh").is_err());
    }

    #[test]
    fn resume_command_keeps_the_original_launch_arguments() {
        let settings = settings_with(
            "claude",
            AgentOverride {
                args: Some("--settings \"C:\\tmp\\ageminal.json\"".to_owned()),
                ..AgentOverride::default()
            },
        );
        assert_eq!(
            resume_command(&settings, "claude", "abc-123", None, None).unwrap(),
            "claude --resume abc-123 --settings \"C:\\tmp\\ageminal.json\""
        );

        // launch spec 里的原始 program / args 优先于当前设置（恢复时重传首次参数）。
        assert_eq!(
            resume_command(
                &settings,
                "codex",
                "thr_1",
                Some("codex"),
                Some("--dangerously-bypass-hook-trust"),
            )
            .unwrap(),
            "codex resume thr_1 --dangerously-bypass-hook-trust"
        );
        assert_eq!(
            resume_command(
                &settings,
                "opencode",
                "ses_1",
                Some("opencode.exe"),
                Some("")
            )
            .unwrap(),
            "opencode.exe --session ses_1"
        );
        assert_eq!(
            resume_command(&settings, "pi", "pi-id", Some("pi"), None).unwrap(),
            "pi --session pi-id"
        );
    }

    #[test]
    fn resume_command_rejects_bad_ids_and_unknown_agents() {
        let settings = Settings::default();
        assert!(resume_command(&settings, "claude", "", None, None).is_err());
        assert!(resume_command(&settings, "claude", "a\rb", None, None).is_err());
        assert!(resume_command(&settings, "dsh", "id", None, None).is_err());
    }

    #[test]
    fn color_validation_accepts_only_hex() {
        assert!(valid_color("#abc"));
        assert!(valid_color("#a1b2c3"));
        assert!(valid_color("#a1b2c3dd"));
        assert!(!valid_color("red"));
        assert!(!valid_color("#xyz"));
    }
}

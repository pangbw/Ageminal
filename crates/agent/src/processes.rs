//! 会话进程树 → agent 进程识别（#81）。
//!
//! 纯函数：输入是 `sysinfo` 适配器拍出的进程快照 + 当前设置里的识别清单；不读时钟、不拉进程。
//! app-core 只负责把 `sysinfo::Process` 转成 [`ProcessNode`]。
//!
//! 匹配只看**可执行文件 / 命令行前两个 token 的路径组件**，不解析终端输出、不看窗口标题。
//! 直接二进制（`claude.exe`）与 Node 包装（`node ...\@earendil-works\pi\...`）都能识别。

use std::collections::{HashMap, VecDeque};

/// 一个进程的快照（app-core 从 `sysinfo` 适配）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessNode {
    /// 进程 id。
    pub pid: u32,
    /// 父进程 id；根进程可能没有。
    pub parent_pid: Option<u32>,
    /// 进程名（Windows 通常含 `.exe`）。
    pub name: String,
    /// 可执行文件路径。
    pub exe: Option<String>,
    /// 命令行（第一个是程序）。
    pub cmd: Vec<String>,
}

/// 设置里的识别清单：agent id → 命令（程序名或路径）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecognitionRule {
    /// agent id（`claude` / `codex` / `opencode` / `pi` / 自定义）。
    pub agent_id: String,
    /// 配置里的程序名 / 路径。
    pub command: String,
}

/// 识别出的 agent 后代进程。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedAgent {
    /// 命中的 agent id。
    pub agent_id: String,
    /// 进程 id。
    pub pid: u32,
}

/// 在 `root_pid` 的进程树里找最近的 agent 进程。
///
/// 采用 BFS，因此「shell → shim → node」中的 node 不会盖过更近的直接 agent；同一层按 pid
/// 排序，结果可重复。
pub fn detect_agent(
    root_pid: u32,
    nodes: &[ProcessNode],
    rules: &[RecognitionRule],
) -> Option<DetectedAgent> {
    if rules.is_empty() {
        return None;
    }
    let by_pid: HashMap<u32, &ProcessNode> = nodes.iter().map(|node| (node.pid, node)).collect();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for node in nodes {
        if let Some(parent) = node.parent_pid {
            children.entry(parent).or_default().push(node.pid);
        }
    }
    for pids in children.values_mut() {
        pids.sort_unstable();
    }

    let normalised: Vec<(&str, String)> = rules
        .iter()
        .filter_map(|rule| {
            let needle = normalise(&rule.command)?;
            Some((rule.agent_id.as_str(), needle))
        })
        .collect();
    if normalised.is_empty() {
        return None;
    }

    let mut queue = VecDeque::new();
    let mut seen = std::collections::HashSet::new();
    queue.push_back(root_pid);
    seen.insert(root_pid);
    while let Some(pid) = queue.pop_front() {
        let Some(node) = by_pid.get(&pid) else {
            continue;
        };
        if let Some(agent_id) = match_node(node, &normalised) {
            return Some(DetectedAgent { agent_id, pid });
        }
        if let Some(children) = children.get(&pid) {
            for child in children {
                if seen.insert(*child) {
                    queue.push_back(*child);
                }
            }
        }
    }
    None
}

fn match_node(node: &ProcessNode, rules: &[(&str, String)]) -> Option<String> {
    let mut candidates = Vec::new();
    if let Some(stem) = normalise(&node.name) {
        candidates.push(stem);
    }
    if let Some(exe) = &node.exe {
        if let Some(stem) = normalise(exe) {
            candidates.push(stem);
        }
    }
    if let Some(program) = node.cmd.first() {
        if let Some(stem) = normalise(program) {
            candidates.push(stem);
        }
    }
    // Node / Python / shell 包装：agent 名通常在脚本路径里，而不是解释器进程名里。
    if let Some(script) = node.cmd.get(1) {
        for component in path_components(script) {
            if let Some(component) = normalise_component(&component) {
                candidates.push(component);
            }
        }
    }
    for (agent_id, needle) in rules {
        if candidates
            .iter()
            .any(|candidate| matches_rule(candidate, needle))
        {
            return Some((*agent_id).to_owned());
        }
    }
    None
}

fn matches_rule(candidate: &str, rule: &str) -> bool {
    candidate == rule
        || candidate
            .strip_prefix(rule)
            .is_some_and(|rest| rest.starts_with('-') || rest.starts_with('_'))
}

fn normalise(value: &str) -> Option<String> {
    let value = value.trim().trim_matches('"').trim_matches('\'');
    if value.is_empty() {
        return None;
    }
    let component = value.rsplit(['/', '\\']).next().unwrap_or(value);
    normalise_component(component)
}

fn normalise_component(value: &str) -> Option<String> {
    let mut value = value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_ascii_lowercase();
    if value.is_empty() {
        return None;
    }
    for suffix in [
        ".exe", ".cmd", ".bat", ".com", ".ps1", ".js", ".mjs", ".cjs", ".ts", ".py", ".sh",
    ] {
        if let Some(stripped) = value.strip_suffix(suffix) {
            value = stripped.to_owned();
            break;
        }
    }
    (!value.is_empty()).then_some(value)
}

fn path_components(path: &str) -> Vec<String> {
    path.split(['/', '\\'])
        .filter(|component| !component.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(pid: u32, parent: Option<u32>, name: &str, cmd: &[&str]) -> ProcessNode {
        ProcessNode {
            pid,
            parent_pid: parent,
            name: name.to_owned(),
            exe: Some(format!(r"C:\bin\{name}")),
            cmd: cmd.iter().map(|value| (*value).to_owned()).collect(),
        }
    }

    fn rule(id: &str, command: &str) -> RecognitionRule {
        RecognitionRule {
            agent_id: id.to_owned(),
            command: command.to_owned(),
        }
    }

    #[test]
    fn finds_a_direct_child_executable() {
        let processes = vec![
            node(1, None, "bash.exe", &["bash"]),
            node(2, Some(1), "claude.exe", &["claude"]),
        ];
        assert_eq!(
            detect_agent(1, &processes, &[rule("claude", "claude")]),
            Some(DetectedAgent {
                agent_id: "claude".to_owned(),
                pid: 2
            })
        );
    }

    #[test]
    fn finds_a_node_wrapped_agent_by_script_path() {
        let processes = vec![
            node(1, None, "cmd.exe", &["cmd"]),
            node(
                2,
                Some(1),
                "node.exe",
                &[
                    "node",
                    r"C:\Users\me\AppData\Roaming\npm\node_modules\@earendil-works\pi\dist\cli.js",
                ],
            ),
        ];
        assert_eq!(
            detect_agent(1, &processes, &[rule("pi", "pi")]),
            Some(DetectedAgent {
                agent_id: "pi".to_owned(),
                pid: 2
            })
        );
    }

    #[test]
    fn custom_command_and_nearest_child_are_used() {
        let processes = vec![
            node(1, None, "bash.exe", &["bash"]),
            node(2, Some(1), "bash.exe", &["bash"]),
            node(3, Some(2), "my-agent.exe", &["my-agent"]),
        ];
        assert_eq!(
            detect_agent(1, &processes, &[rule("custom", r"C:\tools\my-agent.exe")]),
            Some(DetectedAgent {
                agent_id: "custom".to_owned(),
                pid: 3
            })
        );
    }

    #[test]
    fn ignores_unrelated_descendants_and_arguments() {
        let processes = vec![
            node(1, None, "bash.exe", &["bash"]),
            node(2, Some(1), "git.exe", &["git", "commit", "-m", "claude"]),
        ];
        assert_eq!(
            detect_agent(1, &processes, &[rule("claude", "claude")]),
            None
        );
    }

    #[test]
    fn matches_versioned_command_names() {
        let processes = vec![node(1, None, "claude-code.exe", &["claude-code"])];
        assert_eq!(
            detect_agent(1, &processes, &[rule("claude", "claude")]),
            Some(DetectedAgent {
                agent_id: "claude".to_owned(),
                pid: 1
            })
        );
    }
}

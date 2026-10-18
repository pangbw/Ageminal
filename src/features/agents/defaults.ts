/**
 * Agent 缺省清单（#77）的**前端兜底**。
 *
 * 生产环境的单一事实来源仍是 Rust 的 `get_agents`（含用户覆盖）；这里只在 IPC 尚未返回 /
 * `pnpm dev:web` 没有 Tauri 宿主时让菜单与设置页仍可用。字段与 Rust 的 `AgentDefaults` 保持一致。
 * `dsh` 刻意不在清单里（REQUIREMENTS.md §10.2）。
 */
import type { AgentConfig } from "../../bindings";

export const FALLBACK_AGENTS: readonly AgentConfig[] = [
  { id: "claude", name: "Claude Code", command: "claude", args: "", color: "#d97757" },
  { id: "opencode", name: "opencode", command: "opencode", args: "", color: "#f5a742" },
  { id: "codex", name: "Codex", command: "codex", args: "", color: "#10a37f" },
  { id: "pi", name: "pi", command: "pi", args: "", color: "#8b5cf6" },
];

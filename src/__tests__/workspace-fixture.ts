/**
 * 工作区测试的替身与夹具。
 *
 * `src/bindings.ts` 是 specta 生成物，测试里不该真的 `invoke`；store / 组件的用例统一
 * 用这里的**同形状替身**模块（`vi.mock` 配 async factory 使用）。
 */
import { vi } from "vitest";

import type {
  AgentConfig,
  PreviewView,
  ProjectView,
  WorkspaceSnapshot,
  WorktreeView,
} from "../bindings";
import type { CommandResult } from "../lib/ipc";

/** 与 Rust 缺省值一致的四家 agent（测试里不依赖 Tauri）。 */
export function defaultAgents(): AgentConfig[] {
  return [
    { id: "claude", name: "Claude Code", command: "claude", args: "", color: "#d97757" },
    { id: "opencode", name: "opencode", command: "opencode", args: "", color: "#f5a742" },
    { id: "codex", name: "Codex", command: "codex", args: "", color: "#10a37f" },
    { id: "pi", name: "pi", command: "pi", args: "", color: "#8b5cf6" },
  ];
}

/** 与 `src/bindings.ts` 同形状的替身模块。 */
export function createBindingsMock() {
  return {
    commands: {
      appInfo: vi.fn().mockResolvedValue({ name: "Ageminal", version: "0.0.0", os: "windows" }),
      windowsBuildNumber: vi.fn().mockResolvedValue(22631),
      systemLocale: vi.fn().mockResolvedValue(null),
      getLanguage: vi.fn().mockResolvedValue(null),
      setLanguage: vi.fn().mockResolvedValue({ status: "ok", data: null }),
      getConfirmClose: vi.fn().mockResolvedValue(true),
      setConfirmClose: vi.fn().mockResolvedValue({ status: "ok", data: null }),
      generalSettings: vi.fn().mockResolvedValue({
        status: "ok",
        data: {
          defaultShell: null,
          confirmClose: true,
          startupBehavior: "restore",
          language: null,
          logsDir: "C:/logs",
        },
      }),
      setGeneralSettings: vi.fn().mockImplementation((input: Record<string, unknown>) =>
        Promise.resolve({
          status: "ok",
          data: {
            defaultShell: input.defaultShell ?? null,
            confirmClose: input.confirmClose ?? true,
            startupBehavior: input.startupBehavior ?? "restore",
            language: input.language ?? null,
            logsDir: "C:/logs",
          },
        }),
      ),
      appearanceSettings: vi.fn().mockResolvedValue({
        status: "ok",
        data: {
          theme: "one-dark-pro-glass",
          fontSize: 13,
          lineHeight: 1.2,
          cursorStyle: "block",
          acrylic: false,
        },
      }),
      setAppearanceSettings: vi.fn().mockImplementation((input: Record<string, unknown>) =>
        Promise.resolve({ status: "ok", data: input }),
      ),
      projectSettingsView: vi.fn().mockImplementation((path: string) =>
        Promise.resolve({ status: "ok", data: { path, defaultShell: null, env: [], agents: [] } }),
      ),
      setProjectSettings: vi.fn().mockImplementation((input: Record<string, unknown>) =>
        Promise.resolve({ status: "ok", data: input }),
      ),
      openLogsDir: vi.fn().mockResolvedValue({ status: "ok", data: null }),
      diagnosticsPlan: vi.fn().mockResolvedValue([]),
      diagnosticsExport: vi.fn().mockResolvedValue({ status: "ok", data: "C:/diag.zip" }),
      metricsReset: vi.fn().mockResolvedValue({ status: "ok", data: null }),
      getAgents: vi.fn().mockResolvedValue({ status: "ok", data: defaultAgents() }),
      saveTabs: vi.fn().mockResolvedValue({ status: "ok", data: null }),
      restoreTabs: vi.fn().mockResolvedValue({ status: "ok", data: [] }),
      terminalSetAgentSession: vi.fn().mockResolvedValue({ status: "ok", data: null }),
      setAgent: vi.fn().mockImplementation((config: AgentConfig) =>
        Promise.resolve({
          status: "ok",
          data: defaultAgents().map((agent) => (agent.id === config.id ? { ...config } : agent)),
        }),
      ),
      workspaceSnapshot: vi.fn(),
      addProject: vi.fn(),
      removeProject: vi.fn(),
      setProjectCollapsed: vi.fn(),
      setWorktreePinned: vi.fn(),
      renameWorktree: vi.fn(),
      renameUserWorkspace: vi.fn(),
      setActiveWorktree: vi.fn(),
      readDir: vi.fn(),
      searchFiles: vi.fn(),
      readPreview: vi.fn(),
      pickFile: vi.fn().mockResolvedValue({ status: "ok", data: null }),
      openInEditor: vi.fn(),
      createEntry: vi.fn(),
      renameEntry: vi.fn(),
      deleteEntry: vi.fn().mockResolvedValue({ status: "ok", data: null }),
      writeClipboard: vi.fn().mockResolvedValue({ status: "ok", data: null }),
      readClipboard: vi.fn().mockResolvedValue({ status: "ok", data: "" }),
      openUrl: vi.fn().mockResolvedValue({ status: "ok", data: null }),
      copyFiles: vi.fn().mockResolvedValue({ status: "ok", data: null }),
      revealInExplorer: vi.fn().mockResolvedValue({ status: "ok", data: null }),
      openTerminal: vi.fn().mockResolvedValue({ status: "ok", data: null }),
      terminalList: vi.fn().mockResolvedValue({ status: "ok", data: [] }),
      agentStatuses: vi.fn().mockResolvedValue([]),
      integrationList: vi.fn().mockResolvedValue({ status: "ok", data: [] }),
      integrationPreview: vi.fn(),
      integrationApply: vi.fn(),
      integrationDisable: vi.fn(),
      integrationTestDelivery: vi.fn(),
      setWatchedFiles: vi.fn().mockResolvedValue({ status: "ok", data: null }),
    },
  };
}

/** 成功的命令结果。 */
export function ok<T>(data: T): Promise<CommandResult<T>> {
  return Promise.resolve({ status: "ok", data });
}

/** 失败的命令结果；文案照 Rust 侧原样给。 */
export function fail(error: string): Promise<CommandResult<never>> {
  return Promise.resolve({ status: "error", error });
}

function basename(path: string): string {
  return path.split("/").pop() ?? path;
}

/** 一个 worktree 视图；只改用例关心的字段。 */
export function worktree(path: string, overrides: Partial<WorktreeView> = {}): WorktreeView {
  return {
    path,
    name: basename(path),
    branchLabel: null,
    isMain: false,
    bare: false,
    locked: false,
    detached: false,
    stale: false,
    pinned: true,
    ...overrides,
  };
}

/** 一个项目视图。 */
export function project(
  path: string,
  worktrees: WorktreeView[],
  overrides: Partial<ProjectView> = {},
): ProjectView {
  return {
    path,
    name: basename(path),
    collapsed: false,
    worktrees,
    error: null,
    ...overrides,
  };
}

/** 一份工作区快照。 */
export function snapshot(
  projects: ProjectView[],
  activeWorktree: string | null = null,
): WorkspaceSnapshot {
  return {
    projects,
    userWorkspace: { path: "/home/user", name: "~" },
    activeWorktree,
  };
}

/** 一个文件预览视图；只改用例关心的字段。 */
export function previewView(overrides: Partial<PreviewView> = {}): PreviewView {
  return {
    kind: "text",
    totalBytes: 0,
    text: "",
    lossy: false,
    truncated: false,
    lines: 1,
    longLine: false,
    imageDataUrl: null,
    imageTooLarge: false,
    ...overrides,
  };
}

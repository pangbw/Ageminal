/**
 * 工作区测试的替身与夹具。
 *
 * `src/bindings.ts` 是 specta 生成物，测试里不该真的 `invoke`；store / 组件的用例统一
 * 用这里的**同形状替身**模块（`vi.mock` 配 async factory 使用）。
 */
import { vi } from "vitest";

import type { ProjectView, WorkspaceSnapshot, WorktreeView } from "../bindings";
import type { CommandResult } from "../lib/ipc";

/** 与 `src/bindings.ts` 同形状的替身模块。 */
export function createBindingsMock() {
  return {
    commands: {
      appInfo: vi.fn(),
      systemLocale: vi.fn().mockResolvedValue(null),
      getLanguage: vi.fn().mockResolvedValue(null),
      setLanguage: vi.fn().mockResolvedValue({ status: "ok", data: null }),
      workspaceSnapshot: vi.fn(),
      addProject: vi.fn(),
      removeProject: vi.fn(),
      setProjectCollapsed: vi.fn(),
      setWorktreePinned: vi.fn(),
      renameWorktree: vi.fn(),
      setActiveWorktree: vi.fn(),
      readDir: vi.fn(),
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
  return { projects, activeWorktree };
}

/**
 * 工作区投影（C8 #16 D2 / D4 / D5 / D6；CONTEXT.md §0）。
 *
 * Rust 是**单一事实来源**：启动 `hydrate` 一次快照，之后每个改动命令都返回**新的完整快照**，
 * 这里整体替换（MVP 不做乐观更新）。侧栏与设置页是同一份投影的两个视图（D6），
 * 靠 getter 派生——不手动同步多处。
 */
import { isTauri } from "@tauri-apps/api/core";
import { defineStore } from "pinia";
import { computed, ref } from "vue";

import {
  commands,
  type ProjectView,
  type WorkspaceSnapshot,
  type WorktreeView,
} from "../bindings";
import { unwrap, type CommandResult } from "../lib/ipc";

/** 窗口标题需要的两个字段（§6）。`branch` 为 `null` = 没有分支可显示（如 bare 主工作树）。 */
export type ActiveWorktree = {
  name: string;
  branch: string | null;
};

export const useWorkspaceStore = defineStore("workspace", () => {
  const projects = ref<ProjectView[]>([]);
  const activePath = ref<string | null>(null);
  /** 最近一次命令的失败原因（Rust 侧文案）；下次成功即清空。 */
  const lastError = ref<string | null>(null);

  /** Rust 快照 → 内存投影（启动 hydrate 与每个改动命令的返回值都走这里）。 */
  function hydrate(snapshot: WorkspaceSnapshot): void {
    projects.value = snapshot.projects;
    activePath.value = snapshot.activeWorktree;
  }

  function messageOf(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
  }

  /** 跑一条返回新快照的命令；失败记进 `lastError`，组件不必层层 try。 */
  async function apply(pending: Promise<CommandResult<WorkspaceSnapshot>>): Promise<boolean> {
    try {
      hydrate(unwrap(await pending));
      lastError.value = null;
      return true;
    } catch (error) {
      lastError.value = messageOf(error);
      return false;
    }
  }

  /** 拉一次完整快照。`pnpm dev:web` 没有 Tauri 宿主，安静跳过（同 `shell/window.ts`）。 */
  async function refresh(): Promise<void> {
    if (!isTauri()) return;
    await apply(commands.workspaceSnapshot());
  }

  /** 添加项目（Rust 侧弹目录选择器）。返回 `false` = 用户取消或失败。 */
  async function addProject(): Promise<boolean> {
    try {
      const snapshot = unwrap(await commands.addProject());
      if (snapshot === null) return false;
      hydrate(snapshot);
      lastError.value = null;
      return true;
    } catch (error) {
      lastError.value = messageOf(error);
      return false;
    }
  }

  function removeProject(path: string): Promise<boolean> {
    return apply(commands.removeProject(path));
  }

  function setProjectCollapsed(projectPath: string, collapsed: boolean): Promise<boolean> {
    return apply(commands.setProjectCollapsed(projectPath, collapsed));
  }

  function toggleProject(project: ProjectView): Promise<boolean> {
    return setProjectCollapsed(project.path, !project.collapsed);
  }

  function setWorktreePinned(
    projectPath: string,
    worktreePath: string,
    pinned: boolean,
  ): Promise<boolean> {
    return apply(commands.setWorktreePinned(projectPath, worktreePath, pinned));
  }

  /** 改显示名；`name` 为 `null` = 回到默认名。 */
  function renameWorktree(
    projectPath: string,
    worktreePath: string,
    name: string | null,
  ): Promise<boolean> {
    return apply(commands.renameWorktree(projectPath, worktreePath, name));
  }

  function setActive(worktreePath: string | null): Promise<boolean> {
    return apply(commands.setActiveWorktree(worktreePath));
  }

  /** 路径 → worktree 的路由表：跨栏查找都走它（D6）。 */
  const byPath = computed(() => {
    const index = new Map<string, WorktreeView>();
    for (const project of projects.value) {
      for (const worktree of project.worktrees) index.set(worktree.path, worktree);
    }
    return index;
  });

  const activeWorktree = computed<ActiveWorktree | null>(() => {
    const path = activePath.value;
    const worktree = path === null ? undefined : byPath.value.get(path);
    return worktree ? { name: worktree.name, branch: worktree.branchLabel } : null;
  });

  /** 侧栏只渲染**已加入左侧栏**的 worktree；一个都没加入的项目整组不出现（§6）。 */
  const sidebarProjects = computed<ProjectView[]>(() =>
    projects.value
      .map((project) => ({
        ...project,
        worktrees: project.worktrees.filter((worktree) => worktree.pinned),
      }))
      .filter((project) => project.worktrees.length > 0),
  );

  function isActive(worktreePath: string): boolean {
    return activePath.value === worktreePath;
  }

  return {
    projects,
    activePath,
    lastError,
    byPath,
    activeWorktree,
    sidebarProjects,
    hydrate,
    refresh,
    addProject,
    removeProject,
    setProjectCollapsed,
    toggleProject,
    setWorktreePinned,
    renameWorktree,
    setActive,
    isActive,
  };
});

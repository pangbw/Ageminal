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
import { errorMessage, unwrap, type CommandResult } from "../lib/ipc";

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

  /** 跑一条命令并把返回的快照整体换上；失败记进 `lastError`，组件不必层层 try。 */  async function applyCommand(
    pending: Promise<CommandResult<WorkspaceSnapshot | null>>,
  ): Promise<boolean> {
    try {
      const snapshot = unwrap(await pending);
      // `null`（如「添加项目」被取消）不是错误，也不该动投影。
      if (snapshot === null) return false;
      hydrate(snapshot);
      lastError.value = null;
      return true;
    } catch (error) {
      lastError.value = errorMessage(error);
      return false;
    }
  }

  /** 拉一次完整快照。`pnpm dev:web` 没有 Tauri 宿主，安静跳过（同 `shell/window.ts`）。 */
  async function refresh(): Promise<void> {
    if (!isTauri()) return;
    await applyCommand(commands.workspaceSnapshot());
  }

  /** 添加项目（Rust 侧弹目录选择器）。返回 `false` = 用户取消或失败。 */
  function addProject(): Promise<boolean> {
    return applyCommand(commands.addProject());
  }

  function removeProject(path: string): Promise<boolean> {
    return applyCommand(commands.removeProject(path));
  }

  function toggleProject(project: ProjectView): Promise<boolean> {
    return applyCommand(commands.setProjectCollapsed(project.path, !project.collapsed));
  }

  function setWorktreePinned(
    projectPath: string,
    worktreePath: string,
    pinned: boolean,
  ): Promise<boolean> {
    return applyCommand(commands.setWorktreePinned(projectPath, worktreePath, pinned));
  }

  /** 改显示名；`name` 为 `null` = 回到默认名。 */
  function renameWorktree(
    projectPath: string,
    worktreePath: string,
    name: string | null,
  ): Promise<boolean> {
    return applyCommand(commands.renameWorktree(projectPath, worktreePath, name));
  }

  function setActive(worktreePath: string | null): Promise<boolean> {
    return applyCommand(commands.setActiveWorktree(worktreePath));
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

  /**
   * 侧栏要渲染的项目：`worktrees` 只留**已加入左侧栏**的那些；一个都没加入的项目
   * 整组不出现（§6「仅渲染已加入侧栏的 worktree」，与原型 `filter(w => w.pinned)` +
   * 空组不渲染一致）。设置页拿的是 `projects`（全量），两个视图同源不同投影（#16 D6）。
   */
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
    toggleProject,
    setWorktreePinned,
    renameWorktree,
    setActive,
    isActive,
  };
});

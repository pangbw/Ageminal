/**
 * 页签模型（#59 的最小形态；完整页签模型见 #69）。
 *
 * 按 worktree 分组，维护每组的页签集合与激活项。当前有两种页签：
 * - 文件预览（#60）：`{ kind: "preview", path }`
 * - 终端（#68）：`{ kind: "terminal", sid }`，背后的 ConPTY 会话由 daemon 持有。
 *
 * `state.json` 的页签骨架由 #78 持久化：本 store 仍是运行期投影，`App.vue` 在恢复后把
 * 每次变化 debounce 回 Rust。拖拽排序、右键菜单、关闭其它页签归 #69。
 */
import { defineStore } from "pinia";
import { ref } from "vue";

import type { TabsTab, TabsWorktree } from "../bindings";
import { basename, isSameOrUnder } from "../lib/path";

/** 一个文件预览页签。 */
export type PreviewTab = {
  kind: "preview";
  path: string;
  name: string;
  /** 后台期间文件变了、但还没重读（#61）：激活时才刷新。 */
  stale: boolean;
};

/** Agent 终端页签的显示元数据（#77）；普通终端不填。 */
export type AgentTabMeta = {
  agentId: string;
  agentColor: string;
};

/** 一个终端页签（#68）。`sid` 是 daemon 会话的逻辑 id。 */
export type TerminalTab = {
  kind: "terminal";
  sid: string;
  cwd: string;
  name: string;
  /** 一等入口的 agent id；普通终端 / 手动入口为 `undefined`。 */
  agentId?: string;
  /** 打开时的图标颜色快照；来自设置（#77）。 */
  agentColor?: string;
};

/** 一个页签。 */
export type Tab = PreviewTab | TerminalTab;

/** 启动恢复后交给 store 的一组页签（#78）。 */
export type HydratedWorktree = {
  root: string;
  active: string | null;
  tabs: Tab[];
};

export const useTabsStore = defineStore("tabs", () => {
  /** worktree 根 → 页签列表。 */
  const tabsByRoot = ref<Record<string, Tab[]>>({});
  /** worktree 根 → 激活页签的 id（预览用路径，终端用 sid）。 */
  const activeByRoot = ref<Record<string, string | null>>({});

  function tabsFor(root: string): Tab[] {
    return tabsByRoot.value[root] ?? [];
  }

  function findPreview(root: string, path: string): PreviewTab | undefined {
    return tabsFor(root).find(
      (tab): tab is PreviewTab => tab.kind === "preview" && tab.path === path,
    );
  }

  function findTerminal(root: string, sid: string): TerminalTab | undefined {
    return tabsFor(root).find(
      (tab): tab is TerminalTab => tab.kind === "terminal" && tab.sid === sid,
    );
  }

  function activeTab(root: string): Tab | null {
    const active = activeByRoot.value[root];
    if (!active) return null;
    return tabsFor(root).find((tab) => tabId(tab) === active) ?? null;
  }

  /** 当前激活的预览页签；激活的是终端时为 `null`。 */
  function activePreview(root: string): PreviewTab | null {
    const tab = activeTab(root);
    return tab?.kind === "preview" ? tab : null;
  }

  /** 当前激活的终端页签；激活的是预览时为 `null`。 */
  function activeTerminal(root: string): TerminalTab | null {
    const tab = activeTab(root);
    return tab?.kind === "terminal" ? tab : null;
  }

  /** 页签的稳定 id：预览用路径，终端用 sid（两者互不冲突：sid 带 `terminal:` 前缀）。 */
  function tabId(tab: Tab): string {
    return tab.kind === "preview" ? tab.path : tab.sid;
  }

  /** 激活指定页签；id 不存在时不动。 */
  function setActive(root: string, id: string): void {
    if (tabsFor(root).some((tab) => tabId(tab) === id)) {
      activeByRoot.value[root] = id;
    }
  }

  /** 启动恢复：整体替换页签骨架与激活项（#78）。 */
  function hydrate(groups: readonly HydratedWorktree[]): void {
    const nextTabs: Record<string, Tab[]> = {};
    const nextActive: Record<string, string | null> = {};
    for (const group of groups) {
      if (!group.root) continue;
      const seen = new Set<string>();
      const tabs = group.tabs.filter((tab) => {
        const id = tabId(tab);
        if (seen.has(id)) return false;
        seen.add(id);
        return true;
      });
      if (tabs.length === 0) continue;
      nextTabs[group.root] = tabs;
      const requested = group.active;
      nextActive[group.root] =
        requested !== null && seen.has(requested)
          ? requested
          : tabId(tabs[tabs.length - 1]!);
    }
    tabsByRoot.value = nextTabs;
    activeByRoot.value = nextActive;
  }

  /** 持久化投影：只保留 Rust 需要的字段，顺序 = 当前页签顺序。 */
  function snapshot(): TabsWorktree[] {
    const worktrees: TabsWorktree[] = [];
    for (const [root, tabs] of Object.entries(tabsByRoot.value)) {
      const items: TabsTab[] = [];
      for (const tab of tabs) {
        if (tab.kind === "terminal") {
          items.push({
            kind: "terminal",
            name: tab.name,
            sid: tab.sid,
            cwd: tab.cwd,
            agentId: tab.agentId ?? null,
          });
        } else {
          items.push({
            kind: "preview",
            name: tab.name,
            path: tab.path,
          });
        }
      }
      if (items.length > 0) {
        worktrees.push({
          root,
          active: activeByRoot.value[root] ?? null,
          tabs: items,
        });
      }
    }
    return worktrees;
  }

  /** 打开（或激活已有的）文件预览页签。 */
  function openFile(root: string, path: string, name: string): void {
    const tabs = (tabsByRoot.value[root] ??= []);
    if (!tabs.some((tab) => tab.kind === "preview" && tab.path === path)) {
      tabs.push({ kind: "preview", path, name, stale: false });
    }
    activeByRoot.value[root] = path;
  }

  /** 打开（或激活已有的）终端页签；`agent` 只由一等入口传入。 */
  function openTerminal(
    root: string,
    sid: string,
    cwd: string,
    name: string,
    agent?: AgentTabMeta,
  ): TerminalTab {
    const tabs = (tabsByRoot.value[root] ??= []);
    const existing = findTerminal(root, sid);
    if (existing) {
      if (agent) {
        existing.agentId = agent.agentId;
        existing.agentColor = agent.agentColor;
      }
      activeByRoot.value[root] = sid;
      return existing;
    }
    const tab: TerminalTab = { kind: "terminal", sid, cwd, name, ...agent };
    tabs.push(tab);
    activeByRoot.value[root] = sid;
    return tab;
  }

  /** 后台变更：标记 stale，等激活时才刷新（#61）。 */
  function markStale(root: string, path: string): void {
    const tab = findPreview(root, path);
    if (tab) tab.stale = true;
  }

  /** 已刷新，清掉 stale。 */
  function clearStale(root: string, path: string): void {
    const tab = findPreview(root, path);
    if (tab) tab.stale = false;
  }

  function isStale(root: string, path: string): boolean {
    return findPreview(root, path)?.stale === true;
  }

  /** 所有 worktree 上打开的预览文件路径（交给 Rust 监听，#61）。 */
  function allPaths(): string[] {
    return Object.values(tabsByRoot.value).flatMap((tabs) =>
      tabs.flatMap((tab) => (tab.kind === "preview" ? [tab.path] : [])),
    );
  }

  /** 按路径标 stale（不关心落在哪个 worktree）——后台变更可能属于非激活的 worktree。 */
  function markStaleByPath(path: string): void {
    for (const tabs of Object.values(tabsByRoot.value)) {
      const tab = tabs.find(
        (candidate): candidate is PreviewTab =>
          candidate.kind === "preview" && candidate.path === path,
      );
      if (tab) {
        tab.stale = true;
        return;
      }
    }
  }

  /** 关闭一个预览页签；关掉的是激活项时，挑一个邻居顶上。 */
  function closeFile(root: string, path: string): void {
    closeTabs(root, [path]);
  }

  /** 关闭一个终端页签；关掉的是激活项时，挑一个邻居顶上。 */
  function closeTerminal(root: string, sid: string): void {
    closeTabs(root, [sid]);
  }

  /** 批量关闭；激活项被关掉时，挑原位置右侧（没有则左侧）的邻居顶上。 */
  function closeTabs(root: string, ids: readonly string[]): void {
    const tabs = tabsByRoot.value[root];
    if (!tabs || ids.length === 0) return;
    const closing = new Set(ids);
    const firstIndex = tabs.findIndex((tab) => closing.has(tabId(tab)));
    if (firstIndex === -1) return;
    const kept = tabs.filter((tab) => !closing.has(tabId(tab)));
    tabsByRoot.value[root] = kept;
    const active = activeByRoot.value[root];
    if (typeof active === "string" && closing.has(active)) {
      const neighbour = kept[Math.min(firstIndex, kept.length - 1)] ?? null;
      activeByRoot.value[root] = neighbour ? tabId(neighbour) : null;
    }
  }

  /** 拖拽排序：把 `fromId` 移到 `toIndex`（clamp 到有效范围）。 */
  function moveTab(root: string, fromId: string, toIndex: number): void {
    const tabs = tabsByRoot.value[root];
    if (!tabs) return;
    const from = tabs.findIndex((tab) => tabId(tab) === fromId);
    if (from === -1) return;
    const [tab] = tabs.splice(from, 1);
    const target = Math.max(0, Math.min(toIndex, tabs.length));
    tabs.splice(target, 0, tab);
  }

  /** 关闭落在 `prefix`（含其自身）之下的预览页签（#62：删除文件 / 目录后收尾）。 */
  function closeUnder(root: string, prefix: string): void {
    const tabs = tabsByRoot.value[root];
    if (!tabs) return;
    const kept = tabs.filter(
      (tab) => tab.kind === "terminal" || !isSameOrUnder(tab.path, prefix),
    );
    if (kept.length === tabs.length) return;
    tabsByRoot.value[root] = kept;
    const active = activeByRoot.value[root];
    if (typeof active === "string" && isSameOrUnder(active, prefix)) {
      activeByRoot.value[root] = kept[0] ? tabId(kept[0]) : null;
    }
  }

  /** 重命名后把落在 `from`（含其自身）之下的预览页签路径改写到 `to`（#62）。 */
  function movePath(root: string, from: string, to: string): void {
    for (const tab of tabsFor(root)) {
      if (tab.kind !== "preview" || !isSameOrUnder(tab.path, from)) continue;
      tab.path = to + tab.path.slice(from.length);
      tab.name = basename(tab.path);
    }
    const active = activeByRoot.value[root];
    if (typeof active === "string" && isSameOrUnder(active, from)) {
      activeByRoot.value[root] = to + active.slice(from.length);
    }
  }

  return {
    tabsByRoot,
    activeByRoot,
    tabsFor,
    tabId,
    activeTab,
    activePreview,
    activeTerminal,
    setActive,
    hydrate,
    snapshot,
    openFile,
    openTerminal,
    closeFile,
    closeTerminal,
    closeTabs,
    moveTab,
    closeUnder,
    movePath,
    markStale,
    clearStale,
    isStale,
    allPaths,
    markStaleByPath,
  };
});

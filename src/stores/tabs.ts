/**
 * 预览页签（#59 的最小形态；完整页签模型见 #69）。
 *
 * 按 worktree 分组（照 #69 的方向），但只做「打开 / 激活 / 关闭文件预览」这一件事：
 * #60 把预览面板体换成 CodeMirror，#69 再接上终端页签、分组、拖拽与右键菜单。
 *
 * 纯 UI 状态，不落盘（§14：页签骨架归属 state.json 是 #69 / #30 的事）。
 */
import { defineStore } from "pinia";
import { ref } from "vue";

/** 一个文件预览页签。 */
export type PreviewTab = {
  path: string;
  name: string;
  /** 后台期间文件变了、但还没重读（#61）：激活时才刷新。 */
  stale: boolean;
};

export const useTabsStore = defineStore("tabs", () => {
  /** worktree 根 → 页签列表。 */
  const tabsByRoot = ref<Record<string, PreviewTab[]>>({});
  /** worktree 根 → 激活页签的路径。 */
  const activeByRoot = ref<Record<string, string | null>>({});

  function tabsFor(root: string): PreviewTab[] {
    return tabsByRoot.value[root] ?? [];
  }

  function findTab(root: string, path: string): PreviewTab | undefined {
    return tabsFor(root).find((tab) => tab.path === path);
  }

  function activeTab(root: string): PreviewTab | null {
    const active = activeByRoot.value[root];
    if (!active) return null;
    return findTab(root, active) ?? null;
  }

  /** 打开（或激活已有的）文件预览页签。 */
  function openFile(root: string, path: string, name: string): void {
    const tabs = (tabsByRoot.value[root] ??= []);
    if (!tabs.some((tab) => tab.path === path)) tabs.push({ path, name, stale: false });
    activeByRoot.value[root] = path;
  }

  /** 后台变更：标记 stale，等激活时才刷新（#61）。 */
  function markStale(root: string, path: string): void {
    const tab = findTab(root, path);
    if (tab) tab.stale = true;
  }

  /** 已刷新，清掉 stale。 */
  function clearStale(root: string, path: string): void {
    const tab = findTab(root, path);
    if (tab) tab.stale = false;
  }

  function isStale(root: string, path: string): boolean {
    return findTab(root, path)?.stale === true;
  }

  /** 所有 worktree 上打开的预览文件路径（交给 Rust 监听，#61）。 */
  function allPaths(): string[] {
    return Object.values(tabsByRoot.value).flatMap((tabs) => tabs.map((tab) => tab.path));
  }

  /** 按路径标 stale（不关心落在哪个 worktree）——后台变更可能属于非激活的 worktree。 */
  function markStaleByPath(path: string): void {
    for (const tabs of Object.values(tabsByRoot.value)) {
      const tab = tabs.find((candidate) => candidate.path === path);
      if (tab) {
        tab.stale = true;
        return;
      }
    }
  }

  /** 关闭一个预览页签；关掉的是激活项时，挑一个邻居顶上。 */
  function closeFile(root: string, path: string): void {
    const tabs = tabsByRoot.value[root];
    if (!tabs) return;
    const index = tabs.findIndex((tab) => tab.path === path);
    if (index === -1) return;
    tabs.splice(index, 1);
    if (activeByRoot.value[root] === path) {
      const neighbour = tabs[index] ?? tabs[index - 1];
      activeByRoot.value[root] = neighbour ? neighbour.path : null;
    }
  }

  return {
    tabsByRoot,
    activeByRoot,
    tabsFor,
    activeTab,
    openFile,
    closeFile,
    markStale,
    clearStale,
    isStale,
    allPaths,
    markStaleByPath,
  };
});

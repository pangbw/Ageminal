/**
 * 文件树投影（#59；REQUIREMENTS.md §9 / #19 D5 / D7 / D8）。
 *
 * **懒加载**：首屏只读当前 worktree 的根目录，展开哪个目录才读哪个。Rust 的 `read_dir` 返回
 * 全部条目 + 可见性标记（#58），过滤与排序的投影都在这里；`visibleRows` 是扁平化后的可见行，
 * 供组件做虚拟滚动。
 *
 * **搜索**是 **Rust 全树搜**（#19 D8）：本 store 只管去抖、代次防过期与状态。纯 UI 状态
 * （展开集合、搜索框、选中项）只在前端，不落盘（§14）。
 */
import { defineStore } from "pinia";
import { computed, ref, watch } from "vue";

import { commands, type DirEntryView, type SearchHitView } from "../bindings";
import { isVisible } from "../features/files/visibility";
import { unwrap } from "../lib/ipc";
import { ancestorsOf } from "../lib/path";
import { useUiStore } from "./ui";

/** 搜索去抖（#19 D8：~200 ms）。 */
const SEARCH_DEBOUNCE_MS = 200;

/** 树里的一行：一个可见节点，或一条「目录过大」提示。 */
export type FileRow = {
  path: string;
  name: string;
  depth: number;
  isDir: boolean;
  hidden: boolean;
  ignored: boolean;
  expanded: boolean;
  /** 目录的子项是否已读到。 */
  loaded: boolean;
  /** 提示行（不是真实条目，不可点击）。 */
  notice: boolean;
};

export const useFilesStore = defineStore("files", () => {
  const ui = useUiStore();

  const root = ref<string | null>(null);
  /** 目录 → 已读到的条目。 */
  const children = ref<Record<string, DirEntryView[]>>({});
  /** 目录 → 是否被截断（超过 5000 条）。 */
  const truncated = ref<Record<string, boolean>>({});
  const expanded = ref<Record<string, boolean>>({});
  /** 目录 → 该目录已发出的请求代次（丢弃过期响应，#19 D5）。 */
  const generation = ref<Record<string, number>>({});
  const selected = ref<string | null>(null);
  const lastError = ref<string | null>(null);

  const query = ref("");
  /** `null` = 不在搜索态（显示树）；否则显示命中列表。 */
  const results = ref<SearchHitView[] | null>(null);
  const searching = ref(false);
  const searchTruncated = ref(false);
  const searchTimedOut = ref(false);
  let searchGeneration = 0;
  let searchTimer: ReturnType<typeof setTimeout> | undefined;

  /** 换工作树：清空所有派生状态并读根目录。 */
  function setRoot(next: string | null): void {
    if (next === root.value) return;
    root.value = next;
    children.value = {};
    truncated.value = {};
    expanded.value = {};
    generation.value = {};
    selected.value = null;
    lastError.value = null;
    clearSearch();
    if (next !== null) void loadDir(next);
  }

  async function loadDir(path: string): Promise<void> {
    const current = root.value;
    if (current === null) return;
    const ticket = (generation.value[path] ?? 0) + 1;
    generation.value[path] = ticket;
    try {
      const listing = unwrap(await commands.readDir(current, path));
      if (generation.value[path] !== ticket) return; // 过期响应，丢弃
      children.value[path] = listing.entries;
      truncated.value[path] = listing.truncated;
      lastError.value = null;
    } catch (error) {
      lastError.value = error instanceof Error ? error.message : String(error);
    }
  }

  /** 展开一个目录（需要时先读它）。 */
  async function expandDir(path: string): Promise<void> {
    expanded.value[path] = true;
    if (!children.value[path]) await loadDir(path);
  }

  async function toggleDir(path: string): Promise<void> {
    if (expanded.value[path]) {
      expanded.value[path] = false;
      return;
    }
    await expandDir(path);
  }

  /** 展开到某个路径（逐级读出祖先目录），用于「搜索定位」。 */
  async function reveal(path: string): Promise<void> {
    const current = root.value;
    if (current === null) return;
    for (const ancestor of ancestorsOf(current, path)) {
      expanded.value[ancestor] = true;
      if (!children.value[ancestor]) await loadDir(ancestor);
    }
    selected.value = path;
  }

  function clearSearch(): void {
    clearTimeout(searchTimer);
    query.value = "";
    results.value = null;
    searching.value = false;
    searchTruncated.value = false;
    searchTimedOut.value = false;
  }

  /** 搜索框输入：去抖后跑一次 Rust 搜索。 */
  function setQuery(value: string): void {
    query.value = value;
    clearTimeout(searchTimer);
    if (value.trim() === "") {
      results.value = null;
      searching.value = false;
      return;
    }
    searchTimer = setTimeout(() => void runSearch(), SEARCH_DEBOUNCE_MS);
  }

  async function runSearch(): Promise<void> {
    const current = root.value;
    const needle = query.value.trim();
    if (current === null || needle === "") {
      results.value = null;
      return;
    }
    searchGeneration += 1;
    const ticket = searchGeneration;
    searching.value = true;
    try {
      const result = unwrap(
        await commands.searchFiles(current, needle, ui.showHidden, ui.showIgnored),
      );
      // 期间又发了新查询 / 输入变了 → 丢弃。
      if (searchGeneration !== ticket || query.value.trim() !== needle) return;
      results.value = result.hits;
      searchTruncated.value = result.truncated;
      searchTimedOut.value = result.timedOut;
    } finally {
      if (searchGeneration === ticket) searching.value = false;
    }
  }

  // 两个开关一动，搜索态要跟着重算（树态由 `visibleRows` 自动重算）。
  watch(
    () => [ui.showHidden, ui.showIgnored],
    () => {
      if (results.value !== null) void runSearch();
    },
  );

  /** 扁平化的可见行（虚拟滚动的数据源）：已展开的目录逐层展开，其余只出目录自身。 */
  const visibleRows = computed<FileRow[]>(() => {
    const rows: FileRow[] = [];
    const toggles = { showHidden: ui.showHidden, showIgnored: ui.showIgnored };
    const walk = (dir: string, depth: number): void => {
      for (const entry of children.value[dir] ?? []) {
        if (!isVisible(entry, toggles)) continue;
        const isExpanded = expanded.value[entry.path] === true;
        rows.push({
          path: entry.path,
          name: entry.name,
          depth,
          isDir: entry.isDir,
          hidden: entry.hidden,
          ignored: entry.ignored,
          expanded: isExpanded,
          loaded: children.value[entry.path] !== undefined,
          notice: false,
        });
        if (entry.isDir && isExpanded) walk(entry.path, depth + 1);
      }
      // 目录被截断时补一条提示（#19 D5：提示是**每目录**的，不只根目录）。
      if (truncated.value[dir] === true) {
        rows.push({
          path: `${dir}::truncated`,
          name: "",
          depth,
          isDir: false,
          hidden: false,
          ignored: false,
          expanded: false,
          loaded: false,
          notice: true,
        });
      }
    };
    if (root.value !== null) walk(root.value, 0);
    return rows;
  });

  function isSelected(path: string): boolean {
    return selected.value === path;
  }

  return {
    root,
    selected,
    lastError,
    query,
    results,
    searching,
    searchTruncated,
    searchTimedOut,
    visibleRows,
    setRoot,
    loadDir,
    expandDir,
    toggleDir,
    reveal,
    clearSearch,
    setQuery,
    isSelected,
  };
});

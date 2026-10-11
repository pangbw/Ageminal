/**
 * 预览页签的 `EditorState` 缓存（#60 / §9 / `CONTEXT.md` §5）。
 *
 * 多预览页签**共享一个 `EditorView`**，每个页签缓存自己的 `EditorState`——折叠与选区本就
 * 在 `EditorState` 里，滚动位置这里**显式**存。LRU 上限 [`PREVIEW_LRU`]（12），超出淘汰最久
 * 未用的页签（连同它的滚动位置）。
 *
 * 刻意不是 Pinia store：`CONTEXT.md` §5 明确 `EditorView` / 编辑器状态**不进状态库**。
 */
import type { EditorState } from "@codemirror/state";

import type { PreviewContent } from "./content";

/** LRU 上限（#60 / §9）。 */
export const PREVIEW_LRU = 12;

/** 一个预览页签的缓存条目。 */
export type PreviewEntry = {
  /** 已加载的内容（文本不含正文）。 */
  content: PreviewContent;
  /** 文本页签的编辑器状态（含折叠 / 选区）；图片 / 二进制为 `null`。 */
  state: EditorState | null;
  /** 显式保存的滚动位置。 */
  scroll: number;
};

/** 一个按「最近使用」排序的定容缓存。 */
export class PreviewCache {
  private readonly entries = new Map<string, PreviewEntry>();

  constructor(private readonly capacity: number = PREVIEW_LRU) {}

  /** 取一条并标记为最近使用。 */
  get(path: string): PreviewEntry | undefined {
    const entry = this.entries.get(path);
    if (!entry) return undefined;
    this.entries.delete(path);
    this.entries.set(path, entry);
    return entry;
  }

  /** 取一条但**不**改变使用顺序。 */
  peek(path: string): PreviewEntry | undefined {
    return this.entries.get(path);
  }

  set(path: string, entry: PreviewEntry): void {
    this.entries.delete(path);
    this.entries.set(path, entry);
    this.evict();
  }

  /** 更新滚动位置（不算「使用」，不改变淘汰顺序）。 */
  setScroll(path: string, scroll: number): void {
    const entry = this.entries.get(path);
    if (entry) entry.scroll = scroll;
  }

  get size(): number {
    return this.entries.size;
  }

  /** 当前键，按「最久未用 → 最近使用」排列。 */
  keys(): string[] {
    return [...this.entries.keys()];
  }

  private evict(): void {
    while (this.entries.size > this.capacity) {
      const oldest = this.entries.keys().next().value;
      if (oldest === undefined) return;
      this.entries.delete(oldest);
    }
  }
}

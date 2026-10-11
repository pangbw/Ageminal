/**
 * 预览面板的控制器（#60）：**单个 `EditorView`** + 每页签 `EditorState` 缓存 + 懒加载。
 *
 * 组件只负责把宿主容器交给 [`PreviewController.mount`]，之后切换页签时调
 * [`PreviewController.activate`]——切走的页签先把滚动位置存回缓存、切回的页签直接命中缓存
 * 上屏（`setState`），**不做 DOM re-parent**（§9 / `CONTEXT.md` §5）。
 *
 * 纯逻辑 / 视图切换在这里，便于用例直接驱动（`EditorView` 在 happy-dom 下可用）。
 */
import type { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";

import { commands } from "../../../bindings";
import { unwrap } from "../../../lib/ipc";
import { PreviewCache, PREVIEW_LRU, type PreviewEntry } from "./cache";
import { toPreviewContent } from "./content";
import { createTextState, emptyState, languageFor } from "./editor";

/**
 * 图片入缓存的上限：超过它只读**不缓存**。
 *
 * 缓存按**条数**（12）而非字节限容，一张 20 MiB 图片的 `data:` URL 约 27 MiB；若不管大小，
 * 12 张大图能把内存撑到几百 MiB。超过此阈值的图片每次激活时重读（少见，且换来内存有界）。
 */
export const IMAGE_CACHE_MAX_BYTES = 4 * 1024 * 1024;

export class PreviewController {
  private readonly cache: PreviewCache;
  private view: EditorView | null = null;
  private activePath: string | null = null;
  /** 丢弃过期加载（快速切页签时，慢的那次不该再上屏）。 */
  private token = 0;

  constructor(capacity: number = PREVIEW_LRU) {
    this.cache = new PreviewCache(capacity);
  }

  /** 把共享 `EditorView` 挂到宿主容器；重复调用是空操作。 */
  mount(host: HTMLElement): void {
    if (this.view) return;
    this.view = new EditorView({ state: emptyState(), parent: host });
    // 若首次加载先于挂载完成，这里把已激活的状态补上屏。
    const entry = this.activePath ? this.cache.peek(this.activePath) : undefined;
    if (entry) this.apply(entry);
  }

  /** 销毁 `EditorView`（组件卸载时调用），并保存当前滚动位置。 */
  unmount(): void {
    this.saveScroll();
    this.view?.destroy();
    this.view = null;
    this.activePath = null;
  }

  /** 当前编辑器状态（无视图时为 `null`）；主要给用例断言状态确实换上了。 */
  get state(): EditorState | null {
    return this.view?.state ?? null;
  }

  /**
   * 切到 `path` 的预览：命中缓存直接上屏，否则读 Rust、建状态、入缓存。
   *
   * 返回该页签的加载内容（供面板渲染图片 / 二进制与警示条）。晚到的加载结果不会覆盖当前页签。
   */
  async activate(root: string, path: string): Promise<PreviewEntry> {
    this.saveScroll();
    this.activePath = path;

    const cached = this.cache.get(path);
    if (cached) return cached;

    const token = (this.token += 1);
    const view = unwrap(await commands.readPreview(root, path));
    const content = toPreviewContent(view);
    const state =
      content.kind === "text" ? createTextState(view.text ?? "", await languageFor(path)) : null;
    const entry: PreviewEntry = { content, state, scroll: 0 };
    if (this.cacheable(entry)) this.cache.set(path, entry);

    if (this.token === token && this.activePath === path) this.apply(entry);
    return entry;
  }

  /** 页签关闭时从缓存移除。 */
  forget(path: string): void {
    this.cache.delete(path);
  }

  /** 宿主容器变为可见后重新测量并恢复滚动（隐藏时 CodeMirror 量不到高度）。 */
  refresh(): void {
    const view = this.view;
    if (!view) return;
    view.requestMeasure();
    const entry = this.activePath ? this.cache.peek(this.activePath) : undefined;
    if (entry) view.scrollDOM.scrollTop = entry.scroll;
  }

  /** 图片超过缓存阈值时不入缓存（见 [`IMAGE_CACHE_MAX_BYTES`]）。 */
  private cacheable(entry: PreviewEntry): boolean {
    if (entry.content.kind !== "image") return true;
    return entry.content.url !== null && entry.content.totalBytes <= IMAGE_CACHE_MAX_BYTES;
  }

  private apply(entry: PreviewEntry): void {
    const view = this.view;
    if (!view || !entry.state) return;
    view.setState(entry.state);
    view.requestMeasure();
    view.scrollDOM.scrollTop = entry.scroll;
  }

  private saveScroll(): void {
    if (this.view && this.activePath) {
      this.cache.setScroll(this.activePath, this.view.scrollDOM.scrollTop);
    }
  }
}

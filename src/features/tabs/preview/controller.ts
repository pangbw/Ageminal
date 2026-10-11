/**
 * 预览面板的控制器（#60）：**单个 `EditorView`** + 每页签 `EditorState` 缓存 + 懒加载。
 *
 * 组件只负责把宿主容器交给 [`PreviewController.mount`]，之后切换页签时调
 * [`PreviewController.activate`]——切走的页签先把**当前** `EditorState`（含折叠 / 选区）与滚动
 * 位置存回缓存、切回的页签直接命中缓存 `setState` 上屏，**不做 DOM re-parent**
 * （§9 / `CONTEXT.md` §5）。
 *
 * 缓存只收文本页签（§9「每页签缓存 `EditorState`」）：图片 / 二进制没有 `EditorState`，
 * 每次激活重读，避开把多张大图的 `data:` URL 一起留在内存里。
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

  /** 销毁 `EditorView`（组件卸载时调用），并把当前状态存回缓存。 */
  unmount(): void {
    this.saveActive();
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
    this.saveActive();
    this.activePath = path;

    const cached = this.cache.get(path);
    if (cached) {
      this.apply(cached);
      return cached;
    }

    const token = (this.token += 1);
    const view = unwrap(await commands.readPreview(root, path));
    const content = toPreviewContent(view);
    const state =
      content.kind === "text" ? createTextState(view.text ?? "", await languageFor(path)) : null;
    const entry: PreviewEntry = { content, state, scroll: 0 };
    if (content.kind === "text") this.cache.set(path, entry);

    if (this.token === token && this.activePath === path) this.apply(entry);
    return entry;
  }

  /** 宿主容器变为可见后重新测量并恢复滚动（隐藏时 CodeMirror 量不到高度）。 */
  refresh(): void {
    const view = this.view;
    if (!view) return;
    view.requestMeasure();
    const entry = this.activePath ? this.cache.peek(this.activePath) : undefined;
    if (entry) view.scrollDOM.scrollTop = entry.scroll;
  }

  private apply(entry: PreviewEntry): void {
    const view = this.view;
    if (!view || !entry.state) return;
    view.setState(entry.state);
    view.requestMeasure();
    view.scrollDOM.scrollTop = entry.scroll;
  }

  /**
   * 把当前页签的**实时**状态与滚动位置存回缓存。
   *
   * 存 `view.state`（不是加载时那份）才留得住切换途中产生的折叠与选区——`EditorState`
   * 不可变，用户每次折叠 / 选择都会换出一份新状态。
   */
  private saveActive(): void {
    const view = this.view;
    if (!view || !this.activePath) return;
    const entry = this.cache.peek(this.activePath);
    if (entry && entry.state) entry.state = view.state;
    this.cache.setScroll(this.activePath, view.scrollDOM.scrollTop);
  }
}

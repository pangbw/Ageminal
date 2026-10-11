/**
 * 文件树的可见性过滤（#18 D3 / D4）。
 *
 * Rust 列举目录时给每个条目带回两个标记（`hidden` / `ignored`），**过滤在前端按开关做** ——
 * 切换开关无需重扫目录。三个可见性维度里，`.git` 由 Rust 直接不返回（永远不显示），
 * 这里只管 dotfile 与「被规则命中」这两个开关。
 */
import type { DirEntryView } from "../../bindings";

/** 两个可见性开关（默认都关：#18 D3）。 */
export type VisibilityToggles = {
  /** 「显示隐藏文件」：打开才显示 dotfile。 */
  showHidden: boolean;
  /** 「显示忽略文件」：打开才显示被规则命中的条目（展示时用弱化样式区分）。 */
  showIgnored: boolean;
};

/**
 * 一个条目在当前开关下是否可见。
 *
 * 两个维度**互相独立**：`node_modules`（被忽略、但不是 dotfile）只需「显示忽略文件」；
 * `.env`（dotfile、但未被忽略）只需「显示隐藏文件」。
 */
export function isVisible(entry: DirEntryView, toggles: VisibilityToggles): boolean {
  if (entry.hidden && !toggles.showHidden) return false;
  if (entry.ignored && !toggles.showIgnored) return false;
  return true;
}

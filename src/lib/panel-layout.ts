/**
 * 三栏布局里左右面板的宽度约束（REQUIREMENTS.md §6；原型 `prototype/index.html` 的拖拽上下限 180–560）。
 *
 * 面板宽度是**纯前端 UI 状态**，不落盘（§14：面板宽度、弹层、搜索框只在前端），
 * 所以这里是纯函数 + 常量：不碰 store、不碰 DOM，拖拽组件与 store 各取所需。
 */

export type PanelSide = "left" | "right";

/** 拖拽时面板宽度的上下限。 */
export const PANEL_MIN_WIDTH = 180;
export const PANEL_MAX_WIDTH = 560;

/** 初始宽度：左栏容得下「项目名 + 路径」，右栏容得下文件名。 */
export const DEFAULT_PANEL_WIDTH: Record<PanelSide, number> = { left: 252, right: 260 };

/** 键盘微调一次的像素数（方向键）。 */
export const PANEL_RESIZE_STEP = 16;

/** 把任意宽度收进 [最小值, 最大值]；非整数一律取整，免得面板宽度出现小数。 */
export function clampPanelWidth(width: number): number {
  return Math.min(PANEL_MAX_WIDTH, Math.max(PANEL_MIN_WIDTH, Math.round(width)));
}

/** 拖拽增量 → 新宽度：左栏随指针向右变宽，右栏随指针向左变宽。 */
export function panelWidthAfterDrag(side: PanelSide, startWidth: number, deltaX: number): number {
  return clampPanelWidth(side === "left" ? startWidth + deltaX : startWidth - deltaX);
}

/**
 * 方向键 → 新宽度：按「把分隔条往哪挪」理解，左右两栏一致
 * （右栏的分隔条在它左侧，方向键向右 = 分隔条右移 = 右栏变窄）。
 */
export function panelWidthAfterStep(side: PanelSide, width: number, direction: -1 | 1): number {
  return clampPanelWidth(width + direction * PANEL_RESIZE_STEP * (side === "left" ? 1 : -1));
}

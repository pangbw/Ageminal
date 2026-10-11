/**
 * 纯前端 UI 状态（C8 #16 D5；REQUIREMENTS.md §14）。
 *
 * 面板宽度、折叠、弹层开关**只活在前端**，不落盘、不走 Rust——它们是「视图长什么样」，
 * 不是工作区事实。工作区事实（项目 / worktree / 页签）在 `workspace.ts` 与后续 store 里。
 */
import { defineStore } from "pinia";
import { ref } from "vue";

import { clampPanelWidth, DEFAULT_PANEL_WIDTH, type PanelSide } from "../lib/panel-layout";

export const useUiStore = defineStore("ui", () => {
  const leftWidth = ref(DEFAULT_PANEL_WIDTH.left);
  const rightWidth = ref(DEFAULT_PANEL_WIDTH.right);
  const leftCollapsed = ref(false);
  const rightCollapsed = ref(false);

  // 弹层 / 对话框开关。实体分别归后续票：#69 新建页签菜单、#86 设置页；
  // 「添加项目」不走前端对话框（#57 由 Rust 侧弹系统目录选择器，见 #32 D1）。
  const newTabMenuOpen = ref(false);
  const settingsOpen = ref(false);

  // 文件树的两个可见性开关（§9；#18 D3 默认都关）。纯前端状态：Rust 已经把标记带回，
  // 过滤在前端做，切开关不重扫目录（#58）。
  const showHidden = ref(false);
  const showIgnored = ref(false);

  function panelWidth(side: PanelSide): number {
    return side === "left" ? leftWidth.value : rightWidth.value;
  }

  function setPanelWidth(side: PanelSide, width: number): void {
    const target = side === "left" ? leftWidth : rightWidth;
    target.value = clampPanelWidth(width);
  }

  function setPanelCollapsed(side: PanelSide, collapsed: boolean): void {
    const target = side === "left" ? leftCollapsed : rightCollapsed;
    target.value = collapsed;
  }

  function collapsePanel(side: PanelSide): void {
    setPanelCollapsed(side, true);
  }

  function expandPanel(side: PanelSide): void {
    setPanelCollapsed(side, false);
  }

  function toggleNewTabMenu(): void {
    newTabMenuOpen.value = !newTabMenuOpen.value;
  }

  function toggleSettings(): void {
    settingsOpen.value = !settingsOpen.value;
  }

  function toggleShowHidden(): void {
    showHidden.value = !showHidden.value;
  }

  function toggleShowIgnored(): void {
    showIgnored.value = !showIgnored.value;
  }

  return {
    leftWidth,
    rightWidth,
    leftCollapsed,
    rightCollapsed,
    newTabMenuOpen,
    settingsOpen,
    showHidden,
    showIgnored,
    panelWidth,
    setPanelWidth,
    setPanelCollapsed,
    collapsePanel,
    expandPanel,
    toggleNewTabMenu,
    toggleSettings,
    toggleShowHidden,
    toggleShowIgnored,
  };
});

# UI 缩放走 WebView2 `ZoomFactor`，不做根字号（rem）重构

## 状态

accepted（2026-09-21）

## 决定

「界面缩放」由 Tauri 的 `Webview.setZoom(scaleFactor)`（底层 WebView2 `ZoomFactor`）实现，全局单值、落盘到 `settings.json` 的 `appearance.ui_zoom`，范围 0.8–2.0、步进 0.1、跨度 0.1，键位 `Ctrl+=` / `Ctrl+-` / `Ctrl+0`。

**不设独立的「界面字号」**：UI 文本大小一律由缩放承担，字号只保留终端一个（xterm `fontSize`）。

## Considered Options

- **根字号 rem 重构** —— 把 `tokens.css` 的 px 令牌与组件里所有 px 改成 rem，靠 `html { font-size }` 缩放。否掉：改动面覆盖全部 Vue 组件，且 codicon 图标字体、xterm 的 canvas 字符度量都不在 rem 体系里，最终还是要额外缩放，等于两套机制并行。
- **只做终端字号，不做界面缩放** —— 否掉：用户要的是"整体变大"，含图标、栏宽、间距。

## Consequences

- **CSS 视口随缩放缩小**，而 `tauri.conf.json` 的 `minWidth: 900 / minHeight: 600` 是物理像素。`zoom = 2.0` 时 900px 窗口只剩 450 CSS px，而两侧栏默认宽 252 + 260 → 主区宽度为负。**因此最小窗口尺寸必须随缩放放大**（`setMinSize(logical × zoom)`），并加一条：算出的最小尺寸超过屏幕工作区时，把缩放上限压到刚好放下。布局约束（`panel-layout.ts`）一个字都不用改。
- 缩放变化后终端需要一次重新 `fit()` —— `ResizeObserver` 观测的是 CSS px 尺寸，缩放会改变它，但**时序不受保证**，不要依赖它自己触发。
- 不钳制「终端字号 × 缩放」的乘积（最坏 32 × 2.0）：两个旋钮各自有合法域，交叉钳制会让"我把缩放调回来字怎么没变"变成新的困惑。

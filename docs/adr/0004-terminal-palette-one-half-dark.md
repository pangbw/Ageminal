# 终端调色板取 One Half Dark，浅色主题下修正三个语义锚点

## 状态

accepted（2026-09-21）

## 决定

终端 16 个 ANSI 色统一取 Windows Terminal 内置的 **One Half Dark**（`microsoft/terminal` 的 `src/cascadia/TerminalSettingsModel/defaults.json`），**深浅两套主题共用同一组色值**。唯一例外：**浅色主题下** `white` / `brightWhite` 映射为 `#282C34`、`cursor` 取当前主题前景色，其余 13 色不动。

## 上下文

16 个 ANSI 色不能从主题的少数几个令牌派生，必须单列。实测 One Half Dark 在白底（`#FAFAFA`）上：`white` **1.28:1**（等于消失）、`yellow` 1.65:1、`green` 1.93:1、`blue` 2.26:1、`cyan` 2.27:1、`purple` 2.82:1、`red` 3.06:1 —— **8 色中 6 色低于 WCAG AA**。任何输出 `\x1b[37m` / `\x1b[97m` 的文本（`ls` 高亮、表格表头、CLI 强调行）会在浅色下直接消失，`cursor: #FFFFFF` 也等于没有光标。

## Considered Options

- **严格照办，16 色一概不动** —— 否掉：浅色下白字消失是硬故障，不是审美问题。
- **浅色改用 One Half Light 整套 palette** —— 否掉：等于维护两套 palette，与"深浅共用一套色值"的决定相反。

修正的三个色是**语义锚点**（"最亮的文字"与"光标"），浅色下换值保住的是语义，不是换配色。

## Consequences

- 深色主题下 `\x1b[30m`（black `#282C34`）对本项目近黑底 `#080909` 只有 **1.42:1** —— 这是 One Half Dark 的设计（它的 black 就是它的背景），**不要"修"**。
- One Half Dark 的 bright 8 色与普通 8 色**是同一组值**（仅 `brightBlack` 不同），所以 `\x1b[1;31m` 与 `\x1b[31m` 视觉无差别；依赖 bright 做层级的 TUI 会失效。现代 agent TUI 多用 24-bit truecolor，影响有限。

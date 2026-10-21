# OSC 52 支持，但默认关闭

## 状态

accepted（2026-09-21）

## 决定

实现 `OSC 52`（`term.parser.registerOscHandler(52, …)`），**默认关闭**，用户须在设置页显式开启。只接受 target `c`（系统剪贴板），**拒绝 `p`**（Windows 没有 primary selection 概念）。`copy-on-select`（终端替应用复制）作为**独立可选项，同样默认关闭**。

## 上下文

`pi --tui-mode fullscreen` 每次拖选都会通过 `OSC 52`（target `c`）写系统剪贴板并闪 "Copied!"，且**没有关闭开关**（[pi#7837](https://github.com/earendil-works/pi/issues/7837)）。而 xterm.js 不内建 OSC 52，本仓也没有注册 handler —— 于是 pi 的 "Copied!" 是一句谎话。这是「在 TUI 里选中后剪贴板没变」的真因，不是缺自动复制功能。详见 `docs/research/tui-selection-and-scroll.md`。

## Considered Options

- **默认开、静默** —— 否掉：启用 OSC 52 等于允许 PTY 里的**任意程序**写系统剪贴板，包括经 SSH/容器转发进来的远端程序。静默开启是不可接受的安全面。
- **默认开、首次请求问一次** —— 否掉：无法可靠区分"用户刚启动的 agent"与"远端不可信程序"，问询频率会失控。
- **不实现 OSC 52，改做 copy-on-select** —— 否掉：pi 已在做复制，再叠一层等于两次写剪贴板；且行业默认值站在"copy-on-select 关"一边（kitty `copy_on_select` 默认 `no`、Windows Terminal `copyOnSelect` 默认 `false`）。

## Consequences

- 默认状态下 pi 的复制**仍然无效**——设置页必须有一条明确说明，否则用户永远不知道开关在哪。
- `Shift+drag` 越过鼠标上报拿本地选择是 xterm.js 在 Windows/Linux 上的**硬编码默认行为**（`shouldForceSelection`），不需要写代码，但需要告知用户。
- 滚动键位（`Shift+PageUp/PageDown`、`Shift+Home/End`、`Shift+滚轮` 强制翻本地 scrollback）属于宿主责任，xterm.js 一个都不绑，见同一份调研的 §五 / §八。

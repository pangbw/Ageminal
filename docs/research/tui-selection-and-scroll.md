# 调研：TUI 里的选中、复制与滚动 —— 鼠标上报的仲裁

> **状态**：调研完成，**决策未定**（等 `/grill-with-docs` 第三轮）。本文只提供事实与备选，不代替决策。
>
> 调研日期：**2026-09-21**
> 背景：REQUIREMENTS §7.4 / §13 的「需解决与 agent TUI 自带快捷键的冲突」一直挂着；新增需求里的「选中自动复制」「快捷键优先级」「滚动」三项，根子都在**鼠标上报与 alt-screen** 上。
> 记号：**确定事实**来自官方文档 / 官方源码 / 一手 issue；**⚠️推测**为无直接来源的推断；**[建议]** 为本文给出的设计提案。
> 版本基线：`@xterm/xterm` **6.0.0**（本仓 `package.json`），源码位置以 `node_modules/@xterm/xterm/lib/xterm.js` 压缩产物为准。

---

## 一、结论（TL;DR）

1. **「选中不了」和「选中了但没复制」是两个不同的问题。** 前者是**鼠标上报**抢走了拖拽（终端侧问题），后者是**没人接 OSC 52**（宿主侧问题）。本仓两个都存在。

2. **pi 的全屏 TUI 自己就在做 copy-on-select**——每次拖选都通过 `OSC 52`（target `c`）写系统剪贴板并闪 "Copied!"，且**没有关闭开关**（issue #7837）。而 xterm.js **不实现 OSC 52**，本仓也没有注册该 handler。**所以「在 pi 里选中后剪贴板没变」的真因极可能是 OSC 52 被静默丢弃，而不是"缺少自动复制功能"。** 这条改变了整件事的性质。

3. **逃逸修饰键的行业惯例是 `Shift`**：Windows Terminal、Ghostty、WezTerm 默认值、以及 xterm.js 在 Windows/Linux 上**硬编码**的 `shouldForceSelection` 全是 `Shift`；只有 macOS 系（Terminal.app / iTerm2 / xterm.js 的 `macOptionClickForcesSelection`）用 `Option`。

4. **copy-on-select 的行业默认是「关」**：kitty `copy_on_select` 默认 `no`，Windows Terminal `copyOnSelect` 默认 `false`；iTerm2 是显著例外（默认**开**）。这给了一个明确的默认值依据。

5. **alt-screen 滚轮→方向键，xterm.js 已经无条件做了**（`!buffer.hasScrollback` 时把 wheel 转成 `ESC [ A` / `ESC [ B`，应用光标键模式时用 `ESC O A/B`）。所以"TUI 里滚不动"不是缺这个能力，而是**应用自己开了鼠标上报把 wheel 吞了**。真正缺的是**宿主侧的滚动键位**（`Shift+PageUp/PageDown/Home/End`），xterm.js 一概不绑，全由宿主负责。

---

## 二、问题本质：三层抢夺

```
拖拽 / 滚轮
   ├─ 应用开了 鼠标上报（DECSET 1000 / 1002 / 1003）
   │     → xterm.js 把事件编码成 SGR 序列发给 PTY，本地选择根本不会发生
   ├─ 否则 alt-screen 且无回滚 → xterm.js 把滚轮转成方向键（已实现）
   └─ 否则 → xterm.js 做本地选择 / 滚 scrollback
```

**确定事实**：

- xterm.js `shouldForceSelection` 的实现（`lib/xterm.js`，压缩产物）：

  ```js
  shouldForceSelection(e){
    return c.isMac
      ? e.altKey && this._optionsService.rawOptions.macOptionClickForcesSelection
      : e.shiftKey
  }
  ```

  即 **Windows / Linux 上 `Shift` 是硬编码的、不可配置**；macOS 上默认**关**（`macOptionClickForcesSelection` 默认 `false`），需要用户自己开。
  来源：<https://github.com/xtermjs/xterm.js/blob/master/src/browser/services/SelectionService.ts>，选项文档 <http://xtermjs.org/docs/api/terminal/interfaces/iterminaloptions/>

- 上游有长期争议：`macOptionClickForcesSelection` 默认不生效、以及"force selection 修饰键跨平台不一致"。
  来源：<https://github.com/xtermjs/xterm.js/issues/4329>、<https://github.com/xtermjs/xterm.js/pull/4783>

- `Shift` 与"扩展选区"共用同一个修饰键，是真实存在的设计冲突（Ghostty 选它是"唯一修饰键"，结果 `Shift+双击` 会**扩展**上一次选区而不是重新开始选）。
  来源：<https://github.com/ghostty-org/ghostty/discussions/5800>

- 协议层还有一层：`XTSHIFTESCAPE`（`CSI > Ps s`）让宿主声明**是否允许鼠标上报吃掉 `Shift` 位**。因为 SGR 鼠标报文本身带修饰位，应用有可能想知道你按了 shift。
  来源：<https://ghostty.org/docs/vt/csi/xtshiftescape>

---

## 三、各家的逃逸键与 copy-on-select 默认值

| 产品 | 逃逸修饰键（TUI 中强制本地选择） | copy-on-select 默认 | 备注 / 来源 |
| --- | --- | --- | --- |
| **Windows Terminal** | `Shift`（文档明确：`Shift` 是"扩展选区到某点"；tmux 鼠标模式下官方答复按住 `Shift` 越过） | `copyOnSelect` = **`false`** | [Selection](https://learn.microsoft.com/en-us/windows/terminal/selection)、[Interaction](https://learn.microsoft.com/en-us/windows/terminal/customize-settings/interaction)、[#12638 官方答复](https://github.com/microsoft/terminal/issues/12638)；`Alt` 是**块选择** |
| **iTerm2** | `Option`（macOS 惯例） | **开**（文档："by default text is copied to the clipboard immediately upon being selected"） | [General Usage](https://iterm2.com/documentation-general-usage.html)；另有独立的「Enable mouse reporting」总开关 |
| **kitty** | `Shift`（[建议] 按惯例；官方 issue 要求为 `copy_on_select` 加修饰键未实现） | `copy_on_select` = **`no`** | [issue #2925](https://github.com/kovidgoyal/kitty/issues/2925)、[kitty.conf](https://sw.kovidgoyal.net/kitty/conf/) |
| **WezTerm** | `bypass_mouse_reporting_modifiers` = **`"SHIFT"`**（配置项名即默认值） | 无内建 copy-on-select | [Mouse Binding](https://wezterm.org/config/mouse.html)、[bypass_...](https://wezterm.org/config/lua/config/bypass_mouse_reporting_modifiers.html)；⚠️ 页面直取被代理拦截，仅凭搜索摘要 |
| **Ghostty** | `Shift`（**唯一**修饰键，作者明确选择） | 无内建 copy-on-select | [discussion #5800](https://github.com/ghostty-org/ghostty/discussions/5800)、[XTSHIFTESCAPE](https://ghostty.org/docs/vt/csi/xtshiftescape) |
| **xterm.js（库）** | Windows/Linux 硬编码 `Shift`；macOS 需开 `macOptionClickForcesSelection` | 不提供，需宿主用 `onSelectionChange` 自己实现 | 见 §二 |
| **VS Code** | 走 xterm.js 的 `Shift` + `macOptionClickForcesSelection` 设置 | `terminal.integrated.copyOnSelection` = **`false`** | ⚠️ 默认值来自设置项常识，未在本次直取官方页确认 |

**结论**：`Shift` 逃逸 + copy-on-select 默认关，是"跟行业对齐"的一侧；iTerm2 的默认开是一枝独秀。

---

## 四、关键发现：OSC 52 —— pi 已经在复制了，是我们丢了

**确定事实**（一手 issue，原文引用）：

> In `--tui-mode fullscreen`, dragging with the primary mouse button to select text immediately writes the selection to the system clipboard via OSC 52 (target `c`, not `p`) and flashes "Copied!" — on every selection, with no modifier key and no setting to change it.
>
> — [earendil-works/pi issue #7837](https://github.com/earendil-works/pi/issues/7837)（标题：*Fullscreen TUI: mouse selection silently overwrites the system clipboard (OSC 52, target `c`); no opt-out*）

同一 issue 里列的对标默认值：kitty `copy_on_select` 默认 `no`、Windows Terminal `copyOnSelect` 默认 `false`。

**本仓事实**：`src/` 与 `src-tauri/src/` 里**没有任何 OSC 52 handler**（`grep -rn "OSC 52\|osc52\|52;" ` 无命中）。xterm.js 6.0.0 不内建 OSC 52；它提供的是钩子：

```ts
registerOscHandler(ident: number, callback: (data: string) => boolean | Promise<boolean>): IDisposable;
```

（`node_modules/@xterm/xterm/typings/xterm.d.ts:1864`）

**⚠️ 推论（需要真机验证）**：在 Ageminal 里跑 `pi --tui-mode fullscreen`，拖选后 pi 会闪 "Copied!"，但**剪贴板不会变**——因为 OSC 52 没人接。这既是"选中后没复制"的最可能解释，也是**安全面**：一旦我们实现 OSC 52，就等于允许 PTY 里的任意程序写系统剪贴板，必须默认关或在首次请求时询问。

**⚠️ 另一条待验**：opencode 是否也走 OSC 52（未查证）。若不走，则它在 Ageminal 里"选中不复制"的原因是 §二 的第一层（鼠标上报），需要本地 copy-on-select。

---

## 五、alt-screen 滚轮：xterm.js 已经做了，但被鼠标上报抢走

**确定事实**：xterm.js 6.0.0 的 wheel 监听器（`lib/xterm.js`）：

```js
if (!this.buffer.hasScrollback) {
  if (0 === t.deltaY) return !1;
  if (0 === e.coreMouseService.consumeWheelEvent(...)) return this.cancel(t, !0);
  const i = ESC + (this.coreService.decPrivateModes.applicationCursorKeys ? "O" : "[")
                + (t.deltaY < 0 ? "A" : "B");
  return this.coreService.triggerDataEvent(i, !0), this.cancel(t, !0);
}
```

要点三条：

1. **只在「没有回滚」的 buffer 上生效**——即 alt-screen（或 normal buffer 被设成 0 回滚）。滚轮被翻成 `↑` / `↓`（应用光标键模式下是 `ESC O A/B`）。
2. **死路在前一行**：若应用开启了鼠标上报且 `consumeWheelEvent` 认为该 wheel 属于应用，事件被吞，轮不到转方向键。
3. **不看 `DECSET 1007`**：上游有 1007（alt-scroll 显式开关）规范，xterm.js 里 `grep 1007` **零命中**——它是无条件按 buffer 类型判断的。而 VS Code 上游早年为这个行为发过 PR。
   来源：[xterm.js PR #1310](https://github.com/xtermjs/xterm.js/pull/1310)、[DECSET 1007 规范](https://terminfo.dev/modes/decset-1007-alt-scroll)

**Windows Terminal 侧对照**：官方 issue #3321 请求"alt buffer 里滚轮发上下键"，长期未被采纳；iTerm2 有「Save lines to scrollback when an app status bar is present」这类周边旋钮。说明这块各家都不完全一致。

**滚动键位是宿主的责任**：xterm.js **不绑定** `Shift+PageUp/PageDown`、`Shift+Home/End`、`Ctrl+Shift+↑/↓` 任何一个；它只提供 `scrollLines` / `scrollPages` / `scrollToTop` / `scrollToBottom` API。VS Code 与 Windows Terminal 的这些键位都在应用层。

---

## 六、快捷键仲裁：VS Code 的白名单模型

**确定事实**：

> The configurable `terminal.integrated.commandsToSkipShell` setting determines which command's keyboard shortcuts should always "skip the shell" and instead be handled by VS Code's keyboard shortcut system. By default, it contains a hard-coded list of commands…
>
> — [VS Code · Terminal Advanced](https://code.visualstudio.com/docs/terminal/advanced)

配套还有一个相反的粗粒度开关 `terminal.integrated.sendKeybindingsToShell`（"大部分键位直接给 shell，少数仍然跳过"）。已知的坑：两者叠加时 `sendKeybindingsToShell` 会**覆盖** `commandsToSkipShell`，导致"我明明加了白名单却不生效"。

来源：[vscode#164116](https://github.com/microsoft/vscode/issues/164116)、[SO: 把 Ctrl+F 送给 shell](https://stackoverflow.com/questions/68421287/how-to-send-ctrlf-to-shell-in-vs-codes-integrated-terminal)

**[建议]** 这正好是 §13「终端聚焦时哪些键不拦截」的现成模型：

- 终端聚焦时，键位默认**全部进 PTY**；
- 另有一张**应用白名单**（`Ctrl+W` 关页签、`Ctrl+Tab` 切页签、`Ctrl+1..9`、`Ctrl+B`、`Ctrl+,`、`Ctrl+=/-/0`）即使终端聚焦也拦截；
- 白名单**必须可覆盖**（用户加 `-` 前缀移出，用于 `Ctrl+W` 这种与 vim/emacs 冲突的键）；
- 不做 `sendKeybindingsToShell` 那种第二个总开关——它是 vscode#164116 那类"为什么不生效"的根源。

xterm.js 侧的落点是 `term.attachCustomKeyEventHandler(fn)`（本仓已在用，见 `src/features/terminal/runtime.ts:176`），返回 `false` 即表示"我拦了，不要送进 PTY"。

---

## 七、本仓现状速查

| 项 | 现状 | 位置 |
| --- | --- | --- |
| 终端快捷键 | 只有 5 个：`Ctrl+Shift+F/C/V/A`，加「有选区时 `Ctrl+C` 复制」 | `src/features/terminal/runtime.ts:176–206` |
| 应用级快捷键（§13） | **完全未实现**；设置页「快捷键」页是只读文案 | `src/features/settings/SettingsView.vue:106–110` |
| OSC 52 | **未实现**（无 handler） | — |
| copy-on-select | 未实现 | — |
| 滚动键位 | 未实现（无宿主侧绑定） | — |
| 滚轮在 alt-screen | 依赖 xterm.js 内建行为（见 §五） | `@xterm/xterm` 内 |
| scrollback | 10 000 行 | `runtime.ts:138` |
| 右键 | `rightClickSelectsWord: false`；有选区复制、无选区弹菜单 | `runtime.ts:148`、`TerminalPanel.vue:104` |
| `altClickMovesCursor` / `macOptionClickForcesSelection` | 未设置（取 xterm 默认） | `runtime.ts:136-165` |

---

## 八、给 Ageminal 的备选（非决策）

**A. OSC 52 支持**（默认关，或首次请求弹一次确认）

- 收益：pi 的 "Copied!" 真的能兑现；TUI 里 app 自己的复制交互立刻可用。
- 代价：安全面（PTY 里任意程序可写剪贴板）。参考行业做法：默认关、或按 host/once/always 记忆。
- 落点：`term.parser.registerOscHandler(52, ...)`，只接受 `c` target（拒绝 `p`，Windows 无 primary 概念）。

**B. 本地 copy-on-select**（默认关，设置页可开）

- 收益：对**不开鼠标上报**的 TUI 与普通 shell 生效。
- 与 A 的关系：A 管"应用自己带复制"，B 管"终端替应用复制"。两者不替代。
- 落点：`term.onSelectionChange` → `navigator.clipboard` / Tauri clipboard。

**C. `Shift+drag` 逃逸**：xterm.js 在 Windows 上已默认是这样，**不需要写代码**——只需要在文档/空态里告诉用户。真正要决定的是**要不要为 `XTSHIFTESCAPE` 做任何事**（[建议] 做：声明"不允许鼠标上报吃 Shift 位"，让 `Shift` 永远保留给本地选择）。

**D. 滚动键位**：`Shift+PageUp/PageDown`（翻页）、`Shift+Home/End`（顶/底）、`Shift+滚轮`（强制翻本地 scrollback，即使应用开了鼠标上报）。最后一条需要用 `attachCustomWheelEventHandler`（xterm.js 提供）拦截。

**E. 快捷键仲裁**：照 §六 的白名单模型实现 §13。

---

## 九、未验证 / 需要真机确认

1. **在 Ageminal 里跑 `pi --tui-mode fullscreen` 拖选，剪贴板是否真的不变**（验证 §四 的推论）。
2. **opencode 的选中复制走不走 OSC 52**——不走的话，它的痛点落在 §二 的第一层，必须靠 B。
3. **`Shift+滚轮` 在开了鼠标上报的 pi 里，wheel 事件会不会先被 `consumeWheelEvent` 吞掉**（决定 D 的最后一条是否需要在 `attachCustomWheelEventHandler` 里先手拦截，而非事后补救）。
4. WezTerm 的 `bypass_mouse_reporting_modifiers` 默认值与 VS Code 的 `copyOnSelection` 默认值：**官方页面直取被本机代理拦截**，仅凭搜索摘要，落地前建议复核。
5. WebView2 的 `ZoomFactor` 对 `clientWidth`（CSS px）的影响，需要在本仓 e2e 里实测——这决定 `FitAddon` 重算 cols/rows 的时机是否需要额外一次 `fit()`。

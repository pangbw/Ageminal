# 技术路线调研：UI 框架与编辑器内核（Tauri vs GPUI）

> **状态回写（2026-09-20）**：**路线已定**——自研 **Tauri 2** 应用（GPUI 分支已封存），前端 **Vue 3**（C1 #9），编辑器内核 **CodeMirror 6**（C4 #12），终端 **xterm.js 6**（C3 #11）。
>
> 本文对 `alacritty_terminal`「第三方需实测 ⚠️」的疑虑**已由 #42 澄清**：以 `default-features = false` 引入时依赖树仅 **68 行**（只带 `vte`，**不拖 winit / crossfont**）；但其 **0.26 把 `TermSize` 关进了 `#[cfg(test)]`**，外部构造 `Term` 只能借 `Grid` 当 `Dimensions`。

> 调研日期：2026-09-19
> 背景：Ageminal 原计划 Tauri + Web 前端；用户提出「用 Zed 的 GPUI 做 UI，并复用 Zed 编辑器核心做语法高亮」。本文核实该构想在纯 Windows 上的可行性。
> 记号：**确定事实**来自官方文档 / crates.io API / GitHub issue 原文；**⚠️推测**为无直接来源的推断。

---

## 一、结论（TL;DR）

1. **GPUI 已是可独立使用的第三方框架，许可证也允许商用，但仍是 pre-1.0。**
   `gpui` 已发布到 crates.io（`0.2.2`，2025-10-22），许可证 **Apache-2.0**（与 Zed 应用的 GPL-3.0 严格区分），可 `gpui = "0.2"` 直接依赖；但官方明说仍处活跃开发、pre-1.0、版本间常有破坏性改动。

2. **纯 Windows 上「用 GPUI 做 UI」可行，但属高成本、高风险的自走路。**
   Zed 官方 Windows 版（2025-10 发布，DirectX 11 + DirectWrite）证明 gpui 的 Windows 后端能跑；门槛是「支持 DirectX 11 的 GPU」（Win7 起即有）。但无障碍全平台基本缺失，Windows 中文 IME 有多个未关闭的真实 bug。

3. **GPUI 与 Tauri 无法有意义地共存，必须二选一。**
   Tauri 不支持嵌入任意原生视图；「WebView 叠加在原生 GPU 内容上」的 feature request 已关闭未实现。GPUI 自带窗口、事件循环与 GPU surface，没有对外提供「渲染进别人 HWND」的公共 API。

4. **「复用 Zed 编辑器核心做只读语法高亮」基本不成立——只有 `gpui` 能被复用。**
   `editor` / `language` / `rope` / `text` / `theme` 全部是 **GPL-3.0-or-later**、未发布到 crates.io、与 Zed 应用强耦合。正确做法是自己用 **tree-sitter + `tree-sitter-highlight`（MIT）** 或 **`syntect`（MIT）** 重写只读高亮。

5. **若坚持原生 Rust + GPUI，Windows 上的现成终端件要么不成熟、要么绕路；若留在 Tauri，xterm.js + CodeMirror 6 是成熟得多、但 IME 仍有已知瑕疵的组合。**

**可行性总判**：用户构想中，**「GPUI 做 UI」可行但代价高，「复用 Zed 编辑器核心」基本不可行**。综合 Windows 成熟度、IME、无障碍与开发成本，**建议继续走 Tauri**。

---

## 二、GPUI 现状

| 项目 | 值 |
|---|---|
| crate 名 | `gpui`（配套 `gpui_collections`、`gpui_http_client`、`gpui-macros`、`gpui_refineable`、`gpui_sum_tree`、`gpui_util`、`gpui_media` 等，同为 0.2.2）|
| 发布形态 | 在 crates.io，最新稳定 `0.2.2`（2025-10-22），历史下载约 288k |
| 许可证 | **Apache-2.0**（Zed 应用主体 GPL-3.0-or-later；「components intended for reuse, such as gpui」为 Apache-2.0）|
| 仓库 | github.com/zed-industries/zed（`crates/gpui`）|
| 官网/文档 | <https://gpui.rs/> · <https://docs.rs/crate/gpui/latest> |
| 状态 | 官方 README：仍活跃开发、pre-1.0、版本间常有破坏性改动 |

注意：
- docs.rs 上 gpui 的 README 仍写「需要 macOS 或 Linux」，属**过时文档**——实际 crate 已带 Windows 后端与 `windows-manifest` 默认 feature。文档滞后本身就是第三方使用风险信号。
- 社区分叉 **gpui-ce（GPUI Community Edition）**（Apache-2.0，目标脱离 Zed 独立演进，自述 API 大致兼容但仍在变化），以及 Longbridge 的 **gpui-component / gpui-kit**（60+ 组件）。⚠️ 分叉长期维护性不确定。

**许可证结论（确定）**：`gpui` 用 Apache-2.0，可商用、可闭源；但**不等于** Zed 编辑器其余部分可商用。

---

## 三、Windows 成熟度

**官方支持已落地：**
- Zed for Windows 于 **2025-10-15** 正式发布，渲染栈 DirectX 11 + DirectWrite。
- 官方解释选 DirectX 11 是因为它从 Windows 7 起保证可用（含 VM），覆盖面最大。
- 安装要求：x64/arm64，支持 DirectX 11 的 GPU；VM 可用但性能下降。
- gpui 0.2.2 默认 feature 含 `windows-manifest`，依赖 `windows / windows-core / windows-numerics / windows-registry (0.61/0.5)`。

**已知 Windows 短板（开放中的真实 issue）：**
- **中文 IME 不稳定**：#59882「Chinese IME 在编辑器 / Agent 面板 / Git commit 输入框间歇性失效」（`platform:windows`、`severity:S2`、`state:needs repro`，仍 open）；相关 #56149（终端 IME 候选窗出现在屏幕顶部）、#62084（候选窗闪烁）。
- **无障碍几乎为零**：#41138「Windows 上屏幕阅读器完全不可用」，且 macOS/Linux 同样不支持；官方解释 GPUI 自研、拿不到系统原生 a11y 能力。
- **字体渲染瑕疵**：#7992 LoDPI 文字偏糊、#9403 文本渲染、#49833 Windows on ARM（Snapdragon）UI 渲染破碎。

**第三方用 GPUI 做成的真实应用**（存在证明，见 awesome-gpui）：
tty7（GPU 终端，原生支持 Windows）、termy（GPUI + alacritty_terminal）、Steward（Windows 启动器）、TokenMonitor（原生 Windows）、Waku、Zeron/comet、hadron（多 worktree + 多 agent）、Codux 等。
⚠️ awesome-gpui 中大量项目标记为 dormant/quiet，生态「有活力但项目死亡率高」。

**判断**：
- 确定：GPUI 在 Windows 上不再是「不可用」，Zed 自己是最大规模生产验证。
- ⚠️推测：对第三方而言「生产可用」≠「开箱即用」；文档滞后、IME/a11y 缺陷、pre-1.0 breaking change 决定它是高投入自走路线。终端里频繁输入中文时，**Windows IME 是最大具体风险点**。

---

## 四、GPUI 与 Tauri 的互斥性

确定事实：
1. GPUI 自带完整平台层（`Application::new().run(...)` 自己拥有窗口、事件循环、GPU surface）。
2. Tauri 维护者明确回复：原生视图嵌入「勉强可以，且只在桌面端，把它们想成图层，不是元素/视图」；移动端与 native-ish embed 尚未考虑。
3. 「WebView 叠加在原生 GPU 内容上」的 feature request（#8246）已关闭且未实现。
4. Tauri 2 在 Windows 用 WebView2（IME 成熟的系统级渲染层）。

**结论**：GPUI 与 Tauri 无法共存。改用 GPUI 意味着放弃：整个 Web 前端生态、Tauri 插件与打包体系（updater / dialog / fs / store / installer / sidecar）、WebView2 带来的成熟 IME/无障碍/字体渲染、大量现成 UI 组件与文档；换来单一语言（Rust）、原生 GPU 性能、与 Zed 内核潜在同构。

---

## 五、编辑器内核复用与原生终端

### 5.1 Zed 编辑器内核能复用吗？——不能

| crate | license | 结论 |
|---|---|---|
| `gpui` | **Apache-2.0** | ✅ 可复用、可商用、可闭源 |
| `editor` | **GPL-3.0-or-later** | ❌ |
| `language` | **GPL-3.0-or-later** | ❌ |
| `rope` | **GPL-3.0-or-later** | ❌ |
| `text` | **GPL-3.0-or-later** | ❌ |
| `theme` | **GPL-3.0-or-later** | ❌ |

补充：这些 crate 未发布到 crates.io（crates.io 上的 `rope` 是另一个项目）；与 GPUI、Zed app context 深度耦合；生态里的复用者只复用 `gpui`，没有复用 editor/language。

**正确做法（只读语法高亮，低风险）**：用 **tree-sitter + `tree-sitter-highlight`（MIT）** 自写高亮（Zed 也走 tree-sitter，但其 `language` 封装是 GPL）；或退回更省事的 **`syntect`（MIT）**。

> 注：若 Ageminal **仅自己用、不分发**，GPL 传染不构成法律问题（GPL 只约束分发）。当前项目已定为**完全开源**，GPL 顾虑进一步降低，但工程上仍不建议复用（未发布、强耦合）。

### 5.2 原生 Rust 终端模拟器选型

| 方案 | 许可证 | 定位 | Windows 现状 |
|---|---|---|---|
| `alacritty_terminal` | Apache-2.0 | VTE 解析 + 终端状态 | Alacritty 支持 Windows；docs.rs 只构建 Linux，第三方需实测 ⚠️ |
| `wezterm-term` | MIT | WezTerm 终端状态机 | WezTerm 在 Windows 用 ConPTY，成熟；crate 未发布 crates.io（需 git 依赖）⚠️ |
| `vte` | Apache-2.0 | 仅 ANSI 解析器，无屏幕模型 | 跨平台；需自实现终端状态 ⚠️ |
| `portable-pty` | MIT | 跨平台 PTY（Windows ConPTY） | ✅ Windows 一等公民，最该直接用的 PTY 层 |
| libghostty / libghostty-vt | MIT | Ghostty 的 VT 内核，可嵌入 | libghostty 1.3.0 起支持 Windows |

**GPUI 现成终端组件：**

| 组件 | 技术栈 | Windows |
|---|---|---|
| `gpui-terminal`（0.1.0） | `alacritty_terminal` + `portable-pty` | 文档只列 Linux；鼠标选择/滚动回看仍 planned ⚠️ |
| `gpui-libghostty`（0.2.5） | libghostty 原生嵌入，alpha | **明确仅 macOS / Linux（Wayland），不支持 Windows** ❌ |
| tty7 / termy | GPUI + 自研 / `alacritty_terminal` | 有 Windows 构建，但属应用而非可依赖组件 ⚠️ |

**中文 IME 现实**：GPUI 的 IME 文档极其稀疏，zTerm 作者原话是「中日韩输入法的光标定位、候选窗位置、组合状态各平台行为不同，GPUI 这里文档很少，我最后去读 Zed 源码」；Zed 自身在 Windows 终端也有 IME 候选窗位置/闪烁 bug。**结论：原生 GPUI 终端 + 中文 IME 是当前最硬的一块骨头。**

---

## 六、对照路线：留在 Tauri

### 6.1 编辑器高亮

| 方案 | 定位 | 成本 | 评价 |
|---|---|---|---|
| **CodeMirror 6** | 可编辑/只读编辑器，Lezer 增量解析 | 小 | ✅ 只读预览首选 |
| Monaco | VS Code 内核 | ~5MB，懒加载困难 | 功能最强但重；只读大材小用 |
| Shiki | 静态高亮（TextMate + Oniguruma WASM），非编辑器 | 中 | 高亮质量最高，无编辑器交互模型 |

对 Ageminal：只读语法高亮 → **CodeMirror 6（只读模式）** 最平衡。

### 6.2 xterm.js 的 IME 与性能

- 性能：xterm.js 是 VS Code 集成终端同款，工业级成熟。
- IME 已知 bug（真实的公开 issue）：xterm.js #5454（中文 IME 输入框远离光标）、VS Code #255285（gemini TUI 下 IME 导致视口左移）、Codex #35438（Windows ConPTY + xterm.js 光标跳动）。
- ⚠️推测：Tauri 侧 IME 也有 bug，但 WebView2 用系统级 IME，整体比 GPUI 自研 IME 路径成熟，且通常有 workaround。

### 6.3 系统门槛

- Tauri/WebView2：Win10 SAC 1709+，Win11 预装；老系统由安装器引导装 WebView2 Runtime。
- GPUI：需 DX11 GPU，与 Tauri 门槛相当。

---

## 七、成本/风险对比

| 维度 | Tauri + Web 前端 | 原生 Rust + GPUI |
|---|---|---|
| UI 开发速度 | 快（HTML/CSS/组件生态、热重载、DevTools）| 慢（Rust 编译、组件早期、文档稀疏）|
| 终端成熟度 | 高（xterm.js）| 中低（组件 alpha 或缺 Windows 支持）|
| 中文 IME | WebView2 系统级，零散 bug 可 workaround | ⚠️ 高风险（自研 IME，Windows 多个 S2 open issue）|
| 无障碍 | 较好 | ❌ 基本缺失 |
| 语法高亮 | CodeMirror 6 / Shiki 开箱即用 | 需自写 tree-sitter/syntect（Zed 内核 GPL 不可复用）|
| 许可证 | 全可控（MIT/Apache）| gpui 是 Apache（OK）；碰 Zed 编辑器内核则 GPL |
| 升级/维护负担 | 生态稳定、文档全 | pre-1.0、breaking change、文档滞后、生态死亡率高 |
| Windows 确定性 | 高（WebView2 是一等公民）| 中（能跑，工程细节需自担）|
| 架构一致性 | 前后端两套（TS + Rust），边界清晰 | 单语言 Rust，长期天花板更高 |
| 开发成本（估算）⚠️推测 | 1× | 2–4× 起步（终端组件、IME、自绘列表、打包）|

**风险汇总**：
- Tauri：主要风险是性能/原生感与 xterm.js 个别 IME bug——已知、可控、有先例。
- GPUI：主要风险是完成度与 Windows IME/无障碍——未知、需自研、有真实未修 bug，且要放弃整个 Web 生态。对「个人开发者、Windows-only、终端是核心」的 ADE，风险收益不划算。

---

## 八、对用户构想的明确回答

> 「用 GPUI 做 UI + 复用 Zed 编辑器核心，在纯 Windows 上是否可行、可行到什么程度？」

1. **用 GPUI 做 UI：可行，但不是「省事」的可行。** gpui 0.2.2 在 crates.io、Apache-2.0、Windows 后端经 Zed 生产版验证；代价是 pre-1.0、文档滞后、需自解决布局/组件/打包，且 Windows 中文 IME 与无障碍是硬伤。
2. **复用 Zed 编辑器核心做只读高亮：基本不可行。** `editor/language/rope/text/theme` 是 GPL-3.0-or-later、未发布、强耦合；唯一可复用的是 Apache-2.0 的 `gpui`。只读高亮正解是自建 tree-sitter + `tree-sitter-highlight`（或 `syntect`）。
3. **GPUI + Tauri 共存：不可行，必须二选一。**
4. **若坚持原生路线**：先做 **Windows + 中文 IME 的终端 spike**（GPUI + `portable-pty` + `alacritty_terminal`，或评估 libghostty），**在投入 UI 之前先验证 IME 与终端渲染**——这是整条路线的成败点。

**最终建议**：保留 **Tauri + xterm.js + CodeMirror 6**。GPUI 值得作为长期重构目标观察，尤其待 `gpui-ce` 成熟、`gpui-libghostty` 支持 Windows 之后。

---

## 九、本项目的落地决策（2026-09-19 确认）

- 主路线：**Tauri + WebView2 + xterm.js + CodeMirror 6**，先做初版。
- **GPUI 方案转入延后分支验证**：独立分支 spike，只验证「Windows 中文 IME 终端输入」；不通过则不采用。
- **不复用 Zed 编辑器核心**（GPL / 未发布 / 强耦合）。
- 完全开源。

---

## 附：关键来源

- GPUI：<https://crates.io/crates/gpui> · <https://docs.rs/crate/gpui/latest> · <https://gpui.rs/>
- 许可证：<https://zed.dev/software-overview> · 各 crate `Cargo.toml`（editor/language/rope/text/theme = GPL-3.0-or-later；gpui = Apache-2.0）
- Windows 要求：<https://zed.dev/docs/windows> · <https://zed.dev/blog/windows-progress-report>
- IME / a11y 缺陷：zed #59882 / #56149 / #62084 / #41138 / #6576 / #7992 / #9403 / #49833
- GPUI 生态：<https://github.com/zed-industries/awesome-gpui> · <https://github.com/gpui-ce/gpui-ce> · <https://github.com/longbridge/gpui-component>
- GPUI 终端：<https://docs.rs/gpui-terminal> · <https://docs.rs/crate/gpui-libghostty/latest> · <https://github.com/l0ng-ai/tty7> · <https://github.com/lassejlv/termy>
- 终端底层：<https://crates.io/crates/alacritty_terminal> · <https://crates.io/crates/vte> · <https://crates.io/crates/portable-pty> · <https://ghostty.org/docs/install/release-notes/1-3-0>
- Tauri 互斥：#11918 · #8246 · <https://v2.tauri.app/reference/webview-versions/>
- Web 编辑器/终端：<https://sourcegraph.com/blog/migrating-monaco-codemirror> · <https://shiki.style/> · xterm.js #5454 · vscode #255285 · codex #35438

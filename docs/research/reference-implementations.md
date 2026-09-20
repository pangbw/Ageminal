# 参考实现调研：orca / pebrel / hrack

> **状态回写（2026-09-20）**：**根决策已定 —— B1 #4**：**自研 Rust 守护进程**，VT 内核选 **`alacritty_terminal`**（Apache-2.0）。
>
> 文中「守护进程要不要内置完整 VT 状态机」这一待定根决策，已由 **#42 实测判定：必须内置（路线 C）**——有界尾巴回放无法重建屏幕（同一 fixture 下 **30 行全错、卡在普通屏**），而**内核快照经 xterm.js 重放逐格 0 差异**。

> 调研日期：**2026-09-19**
> 背景：为 wayfinder 决策地图（issue #41）核实三个强相关参考实现的**终端与进程/会话方案**，评估 Ageminal 可借鉴/可复用的部分。三个仓库均以 `--depth 1` 浅克隆到 `/tmp/opencode/refs/`（只读）。
> 待定根决策：**会话守护进程形态（B1）**，以及「守护进程要不要内置完整 VT 状态机」。
> 记号：**确定事实**来自源码 / 官方文档 / 仓库文件；**⚠️推测**为无直接来源的推断；**[建议]** 为本文设计提案（非既定事实）。
> 与既有调研的关系：[`session-daemon.md`](./session-daemon.md)（B1 候选对比）、[`daemon-ipc-protocol.md`](./daemon-ipc-protocol.md)、[`agent-event-mapping.md`](./agent-event-mapping.md)（E 组状态机）。本文不替用户拍板，只提供一手证据。

---

## 一、结论（TL;DR）

1. **orca 的终端引擎是 xterm.js，不是 libghostty / ghostty-web。**
   渲染侧 `@xterm/xterm 6.1.0-beta.303` + `@xterm/addon-webgl`；守护进程侧 `@xterm/headless 6.1.0-beta.302` + `@xterm/addon-serialize`；PTY 层 `node-pty ^1.1.0`（打了补丁）。仓库里唯一的 "ghostty" 是 `src/main/ghostty/`——一个把 Ghostty 配置文件**导入**成 Orca 设置的模块，与终端引擎无关。README 的 "Ghostty-class terminals" 是**营销措辞**（`README.md:65`）。

2. **orca 确实有一个独立于 UI 的终端守护进程，而且守护进程里跑完整 xterm 状态机。**
   每个本地 PTY 由 fork 出来的 detached **terminal daemon** 持有；UI 侧 `orcad`（或 Electron 主进程）只负责 RPC/git/worktree/持久化。守护进程用 `@xterm/headless` 解析 PTY 输出、用 `SerializeAddon` 生成屏幕快照，并主动应答 ConPTY 的 DA1 查询（`src/main/daemon/headless-emulator.ts`）。

3. **「scrollback that survives restarts」= checkpoint 快照 + 追加式增量日志，落盘在 `terminal-history/`。**
   会话目录含 `checkpoint.json`（ANSI 序列化的屏幕快照）、`output.log`（带 magic/版本/seq 的帧日志，可检测残尾）、`meta.json`、`scrollback.bin`。冷恢复 = 读 checkpoint → 按 seq 续放 output.log，seq 断档就拒绝重放。**这套设计可直接借鉴。**

4. **orca 的守护进程与 Electron/Node 强绑定。**
   daemon 是 **同一个 `Orca.exe` 被 `ELECTRON_RUN_AS_NODE=1` 当纯 Node 跑**，通过 `child_process.fork(detached:true)` 脱离；Windows 上还把整份镜像复制到 `%LOCALAPPDATA%\Orca\daemon-host\<version>\` 以躲开安装器按**镜像路径**做的进程清扫。因此：**架构可复用，代码不可直接搬到 Tauri/Rust。**

5. **orca 是本项目 xterm.js 路线的最强背书，也对 `session-daemon.md` 的「daemon 用 `alacritty_terminal`」建议构成实质性挑战。**
   orca 故意让守护进程与渲染器用**同一个 xterm 内核**（代码注释反复强调 "renderer parity / must match the renderer"）。Ageminal 若渲染器用 xterm.js、守护进程用 Rust 的 `alacritty_terminal`/`vte`，就会引入 orca 一直在规避的**跨引擎一致性成本**。这是 B1 当前最需要正视的点。

6. **pebrel 走的是「进程内驻留」而非守护进程，且是 GPL-3.0。**
   Rust + GPUI + 自维护的 Alacritty 终端核心（`nebula_terminal`，GPL-3.0-or-later），Windows 用 `windows-sys` 直连 ConPTY 并**旁加载 `conpty.dll` + `OpenConsole.exe`**。关窗只是 hide，PTY 留在常驻 GUI 进程里；没有独立 daemon，进程退出会话即终止。**只能作参考实现，不能复用代码。**

7. **hrack 与终端持久化无关，但它是 Agent 状态/集成层（E 组）的最佳范本。**
   Electron + xterm.js + node-pty（**进程内，无 daemon**，`PtyHistory` 仅内存环形缓冲）。其价值在观察者架构：`AgentEvent`（事实）→ `AgentEventReducer` → 会话投影（六态 `working/needs-you/done/error/idle/exited`）+ 能力位/置信度/观察者健康度；CLI 事件经 adapter 归一，**观察者失效绝不阻断 PTY**；只读 workspace 通过受限 IPC 暴露。

**可行性总判**：orca 的终端方案**在 Tauri 2 + WebView2 下可以复现其理念与大部分前端组件，但守护进程这一层必须用 Rust 重写或引入 Node sidecar**。orca 的持久化格式可直接照抄；orca 的 daemon-side xterm 选择则要求 Ageminal 在「同引擎一致性」和「纯 Rust」之间做取舍。

---

## 二、orca 终端栈与进程模型

### 2.1 终端引擎（确定事实）

| 层 | 组件 | 证据 |
|---|---|---|
| 渲染器终端 | `@xterm/xterm 6.1.0-beta.303` | `package.json:249` |
| 渲染器 GPU | `@xterm/addon-webgl 0.20.0-beta.299`（另有 fit/image/ligatures/search/unicode11/web-links） | `package.json:242-249` |
| **守护进程 VT** | `@xterm/headless 6.1.0-beta.302` | `package.json:181` |
| 守护进程快照 | `@xterm/addon-serialize 0.15.0-beta.300` | `package.json:180` |
| PTY | `node-pty ^1.1.0`（`patchedDependencies` 里对 1.1.0 打了补丁） | `package.json:186`、`pnpm-workspace.yaml` |
| Ghostty | **仅配置文件导入**（discovery/mapper/parser/theme），非引擎 | `src/main/ghostty/` |

- **渲染方式**：WebGL（`@xterm/addon-webgl`），有完整的 atlas 预算与 context-loss 回退逻辑（`src/renderer/src/lib/pane-manager/pane-webgl-renderer.ts`）。
- **无 `libghostty` / `libghostty-vt` / `ghostty-web`(WASM)**：全仓库检索 `ghostty` 只命中 i18n 文案与 `src/main/ghostty/` 配置导入模块；`package.json` 无相关依赖。
- 使用 xterm.js 的 **6.1 beta** 通道，并对 6 个 xterm 包各自打了补丁（`pnpm-workspace.yaml` 的 `patchedDependencies`），说明团队深度定制该栈。

### 2.2 进程模型：两个长命进程（确定事实）

`docs/reference/orcad-operations.md:7-30` 明确：一次部署是 **orcad** + **terminal daemon** 两个进程。

| | orcad | terminal daemon |
|---|---|---|
| 由谁启动 | supervisor（systemd 等） | **orcad，detached** |
| 拥有 | RPC、git、worktree、持久化 | **每一个本地 PTY** |
| 生命周期 | 一次受监督运行 | **脱离 orcad，非其服务** |
| 端点 | `ws://<bind>:<port>` | `<data-root>/daemon/daemon-v<N>.sock` |

- **启动**：`child_process.fork(forkEntryPath, ..., { detached: true, stdio: ['ignore','ignore','pipe','ipc'], env: { ELECTRON_RUN_AS_NODE: '1', ... } })`（`src/main/daemon/daemon-launched-child.ts:59-101`）。stderr 只收集启动窗口，避免管道 ref 住父进程。
- **脱附语义**：orcad 用 `disconnectDaemon()` 而非 `shutdownDaemon()`；PID-scoped 更新/回滚不杀 daemon 与 PTY（`orcad-operations.md:18-22`）。但同一 systemd cgroup 的整体 stop 仍会 reap daemon（`orcad-operations.md:24-30`）——诚实标注的局限。
- **接管**：新 orcad 优先 adopt 已应答的 daemon，只有 unhealthy/foreign/无活会话的过期 bundle 才替换（`orcad-operations.md:134-147`）。
- **崩溃环治理**：60s 滚动窗口内最多 5 次 launch，超出报 `daemon_crash_loop`（`orcad-operations.md:139-143`）。
- **健康自检**：daemon 真正 spawn 一个短命 PTY 才算绿色；win32 上退化为仅握手（`coverage: 'handshake'`），并单独标注不虚报（`orcad-operations.md:179-195`）。

### 2.3 守护进程侧 VT 状态机（确定事实）

`src/main/daemon/headless-emulator.ts` 是核心证据：

- `:2` `import { Terminal } from '@xterm/headless'`；`:94-98` 加载 `SerializeAddon` 与 `Unicode11Addon`；`:55` 默认 scrollback 5000。
- **应答 ConPTY 查询**：`:108-120` `installConptyPrimaryDeviceAttributesOverride()`，注释写明「**ConPTY 1.22+ blocks at spawn awaiting a DA1 reply**」；`src/main/daemon/startup-device-attributes-responder.ts` 对启动期与持续期分别处理（fish 会等 DA1 10 秒）。
- **快照**：`:252-284` `getSnapshot()` 用 `serializeWithAbsoluteCursor()` 产出 `snapshotAnsi` + `scrollbackAnsi`，附带 OSC-8 链接范围、rehydrate 序列、鼠标/键盘模式、cwd、标题、以及**未完成的 escape 尾巴**（避免重放时把半个转义序列渲染成字面量）。
- **同引擎一致性是显式设计目标**：`:56`「Keep in sync with the renderer twin」、`:90`「renderer parity」、`:97`「must match the renderer's char-width measurement, else emoji rows mismeasure」。还有 `src/shared/terminal-restore-parity-fixture.ts` 与 `headless-emulator-fidelity.fuzz.test.ts` 做逐字节一致性验证。

### 2.4 IPC 与持久化（确定事实）

**IPC**
- POSIX：`<runtimeDir>/daemon-v<N>.sock`；Windows：`\\?\pipe\orca-terminal-host-v<N>-<sha12(runtimeDir)>`（`src/main/daemon/daemon-spawner.ts:124-136`）。协议版本进端点名，避免旧 daemon 被复用。
- 另有 `daemon-v<N>.token`（随机 token）与 `daemon-v<N>.pid`（PID + launchNonce + 启动时间，用 rename claim 协议做所有权，`daemon-spawner.ts:138-215`）。
- 传输是 NDJSON 分帧；RPC 方法包括 `createOrAttach / write / resize / getSnapshot / takePendingOutput / kill / detach / clearScrollback / listSessions / shutdownIfIdle` 等（`src/main/daemon/daemon-request-router.ts`）。
- 端点所有权用「bind 私有 `.p<hex>` → 独占 `link` → 证明旧主已死 → 单次 `rename`」的协议（`src/main/daemon/AGENTS.md:3-51`），专门解决「离场 daemon 误删继任者 socket」类缺陷。

**持久化（`terminal-history/`）**
- 目录按 worktree 哈希切分：`<userData>/terminal-history/<worktreeHash>/`（`src/main/terminal-history-paths.ts:12-18`；WSL 另有 `terminal-history-wsl/<distro>/`）。
- 会话文件：`checkpoint.json`、`output.log`、`meta.json`、`scrollback.bin`（`src/main/daemon/terminal-history-session-files.ts:8-13`）。
- `checkpoint.json` 字段：`snapshotAnsi`、`scrollbackAnsi`、`oscLinks`、`rehydrateSequences`、`pendingEscapeTailAnsi`、`cwd`、`cols/rows`、`modes`、`lastTitle`、`generation`、`pendingOutputSeq` 等（`src/main/daemon/daemon-checkpoint-file.ts:9-28`）。
- `output.log` 定长帧格式：header = magic `'OCKL'` + `u8 formatVersion` + `u32le generation`；frame = `u8 kind` + `u32le payloadLength` + payload，kind ∈ batch/output/resize/clear（`src/main/daemon/terminal-history-log.ts:3-16`）。
  - **残尾检测**：崩溃撕开的最后一个 append 会因长度前缀不完整而被识别，恢复时截断到最后一个完整帧，而不是重放半个转义序列（`:13-16`）。
  - **seq 断档检测**：batch 序号不连续即判定字节流有洞，整份日志判为不可读（`:80-118`）。
- checkpoint 与增量日志通过 `generation` 配对（`daemon-checkpoint-file.ts:22-24`）；checkpoint 采用「冷却 + 脏标记 + 延迟重试」调度，避免打洞（`daemon-pty-checkpoint-persistence.ts:9-90`）。

### 2.5 Windows 支持与打包（确定事实）

- **不依赖 Tauri sidecar 式独立二进制**：daemon 就是应用自身镜像的 Node 模式副本。
- **更新存活机制**（`docs/reference/windows-daemon-host-relocation.md:4-7, 31-33`）：fork 之前把 runtime 精简复制到 `%LOCALAPPDATA%\Orca\daemon-host\<app version>\`，从那里 fork。原因是 electron-builder 的安装器**按进程镜像路径**清扫 `$INSTDIR` 下的进程；daemon 镜像在 `%LOCALAPPDATA%` 就躲开了清扫。**存活是路径属性，不是文件名属性**。
- **不重命名 exe**：早期版本改名为 `orca-terminal-daemon.exe` 以躲开 `taskkill /IM` 回退分支，被 Defender 标为 MITRE **T1036 Masquerading**（`:41-47`）。现改为逐字节保留原名，代价是回退分支下 daemon 会被杀、终端走 cold-restore（`:49-56`）。
- **随包分发的原生物**（`native/`）：`windows-registry`（node addon，pnpm workspace）、`windows-cli-launcher`（C#）、`computer-use-windows`（PowerShell）、macOS 的 Swift helper 等；`node-pty` 用 `electron-rebuild` 重建并按目标架构打包（`config/electron-builder.config.cjs`）。构建脚本：`build:native`、`smoke:windows-pty-native-capability`、`win-crash-survival-e2e`、`win-update-survival-e2e`。

### 2.6 许可证与组件化程度（确定事实）

- **MIT**（`LICENSE`：`Copyright (c) 2026 Lovecast Inc.`）。
- **组件化程度：仅整机。** 没有发布任何可依赖的终端/daemon 库；daemon、`orcad`、渲染器都是应用内模块。可复用性来自**架构与做法**，不是依赖或包。

### 2.7 能否在 Tauri 2 + WebView2 下复现

| 部分 | 结论 | 依据 |
|---|---|---|
| 渲染器终端（xterm.js + WebGL + addons） | ✅ **可直接复用**（纯 Web 技术，WebView2 同源） | `package.json:242-249` |
| 守护进程侧 `@xterm/headless` 快照 + `addon-serialize` | ⚠️ **语义可复用，代码不可**：这是 JS 库，Rust 守护进程无法直接 require | `headless-emulator.ts` |
| `node-pty` | ❌ **不能直接在 Tauri Rust 核里用**：Node 原生 addon；Tauri 侧要么用 `portable-pty`/`windows-sys` 重写 ConPTY，要么保留 Node sidecar | `package.json:186` |
| daemon 进程模型 / detached fork / adopt / PID+token+socket | ⚠️ **架构可移植**，但 orca 用 `ELECTRON_RUN_AS_NODE` 复用 Electron 镜像，Tauri 没有等价物 | `daemon-launched-child.ts:59-101` |
| Windows 镜像搬迁躲安装器清扫 | ✅ **机制可移植到 Tauri/NSIS**（Tauri 安装器同样按安装目录清扫） | `windows-daemon-host-relocation.md` |
| checkpoint + 帧日志持久化格式 | ✅ **可直接照抄协议**（与语言无关） | `terminal-history-log.ts` |

---

## 三、pebrel

### 3.1 技术栈与终端实现形态（确定事实）

- **Rust + GPUI**（产品 UI 走 `gpui-shell`，默认 feature；旧壳仅 `legacy-shell`），GPU 走 **OpenGL ES 2.0+**（`README.md` badges、`nebula_app/Cargo.toml:265,292-298`）。
- **终端核心是自维护的 Alacritty 派生**：`nebula_terminal`，描述为 "Nebula terminal emulation core"，**GPL-3.0-or-later**（`nebula_terminal/Cargo.toml:1-6`）。
  - 依赖 `vte 0.15.0`（ANSI 解析）+ 自写 tty 层（`nebula_terminal/src/tty/`）。
  - Windows：`windows-sys 0.59`（`Win32_System_Console` 等）+ `piper`/`miow`（`Cargo.toml:39-51`）。
  - **ConPTY 旁加载**：优先加载随包 `conpty.dll` + `OpenConsole.exe`，否则回落内核 `kernel32` ConPTY；注释说明捆绑版规避了内置 ConPTY 的 resize 视口重发怪癖（`nebula_terminal/src/tty/windows/conpty.rs:39-48, 76-90`）。`openconsole=off` 可退回内置以加快 pane spawn。
  - 这一「旁加载 conpty.dll 规避版本碎片化」正是 [`session-daemon.md` §六.3 的推测](./session-daemon.md) 在生产项目里的落地。

### 3.2 Windows「background residency」的真实机制（确定事实）

- 关窗处置：`residency_close_action(keep_session, has_live_panes, tray)` —— 只要 `keep_session && has_live_panes` 就 **Hide**，与托盘开关无关（`nebula_app/src/gpui_shell/workspace/residency.rs:39-50, 997-1035`）。注释说明旧壳是 `detach_panes`（PTY 仍在进程里）再销毁 HWND，GPUI 用 **hide + mux ATTACH reveal** 对齐同一可见行为。
- **没有任何独立 daemon**：PTY 就活在常驻的 Nebula GUI 进程里。「第二次启动回到同一套会话」靠单实例 **mux 服务**：常驻进程持有 live session，普通二次启动经 **loopback TCP** 交接到常驻实例；发现文件是 `%APPDATA%\Nebula\mux.port`，内含 `<port> <token>`，token 仅用于隔离其他本机用户（`nebula_app/src/mux.rs:1-22`）。生产 GUI 服务由 `runtime_api::RuntimeServer` 提供（`mux.port` 的同族 `runtime.port`）。
- **边界**：后台驻留保活的是**同一个进程内**的会话。**进程退出（崩溃 / 显式 Quit / 重启）后 PTY 即终止**。
- **会话快照**：`SessionPersistence` 存的是 workspace/layout 快照，含 `clean_exit` 标志；保存时机为 Checkpoint / WindowClose / TabsClosed / Quit（`nebula_app/src/gpui_shell/workspace/session_persistence.rs:1-110`）。注释/文档明确：这些快照「保存布局与启动身份，不保存命令历史或输出」（`docs/architecture-decisions.md:248`）。
- **「进程退出后恢复对话」是另一件事**：README 原文——关闭窗口后的驻留在 Windows 上保活会话；**进程退出后恢复对话是独立功能，需要受支持的 CLI 和可用的 session identity**（`README.md:62-64`）。即它依赖 agent CLI 自身的 resume（如 Claude/Codex 的 session id），不是重建 PTY。

### 3.3 GPL-3.0 对 Ageminal 的约束（确定事实 + 判断）

- `LICENSE` 是 GNU GPL v3；`nebula_terminal/Cargo.toml:3` 为 `GPL-3.0-or-later`。
- **代码复用不可行**：Ageminal 已定完全开源（可能 MIT/Apache）。把 GPL-3.0 代码链接进 Ageminal 会使整体受 GPL 传染（分发时须以 GPL 兼容方式开源全部衍生作品）。终端核心、`nebula_app` 的会话/驻留代码同属 GPL 树。
- **借鉴（clean-room 重实现）不受限**：GPL 约束的是复制/衍生，不约束独立实现的相同思想。pebrel 可作为：ConPTY 旁加载策略、驻留/单实例交接的产品行为、会话快照边界的**参考**。
- ⚠️注：若 Ageminal「仅自己用、不分发」，GPL 不构成法律问题；但项目已定**完全开源**，故按分发对待，**不复用其代码**。

---

## 四、hrack

### 4.1 定位与技术栈（确定事实）

- Electron + TypeScript，**Apache-2.0**（`package.json:license`、`LICENSE`）。
- 终端：`@xterm/xterm 6.1.0-beta.292` + `@xterm/addon-webgl 0.20.0-beta.291` + `@xterm/addon-fit` + `node-pty ^1.1.0`（`package.json:155-157, 162`）。
- **进程内 PTY，无 daemon**：全仓库源码检索 `daemon` **零命中**（排除 test）。`PTYManager` 跑在 Electron main，`PtyHistory` 是**内存**环形缓冲（默认 16 MB / 20000 事件，超出从最旧端整段淘汰并标 `complete=false`），**不落盘、不跨重启**（`electron/pty/PtyHistory.ts:1-38`）。所以 hrack 的会话**不满足**「关 UI 后存活」。
- README 自述重点在「保住原生 TUI，只在其周围加状态层」：`CLI ── PTY ─> native TUI` 与 `Hooks/SSE/extension → adapter → status` 是**两条独立通道**（`README.md:45-50`）。

### 4.2 观察者架构与统一状态词汇（确定事实）

三层契约（`shared/agent-events.ts:5-7`）：**AgentEvent 是事实，SessionStatus 是事实的投影，HistoryEvent 是事实的低敏摘要，三者不可合并**。

- 事件来源 `AgentEventSource`：`native-stream | jsonl | rpc | acp | hook | transcript | lifecycle | fixture`（`shared/agent-events.ts:13-21`）。
- 每条事件带 `sessionId / adapterId / installationId / seq`（**排序只认 seq，不认时间戳**）/ `occurredAt / source / nativeId`（用于 hook 重放/RPC 重连去重）。
- 状态投影（`AgentEventReducer.ts:1-9`）：纯函数 `reduceAgentSession(previous, event)`，无副作用、不读时钟、不依赖 adapter 私有状态；乱序/重复/并行 tool/多 pending approval/退出终态都在这里确定性处理。
- **六态词汇**：`working | needs-you | done | error | idle | exited`（`shared/agent-events.ts:187-193`）；另有 `AgentStatusConfidence = high | low` 与观察者健康度 `AgentObserverHealth = unconfirmed | healthy | stale | lifecycle-only`。
  - README 面向用户的五词：`thinking · tool call · needs you · completed · error`。
- 事件种类含 `ToolStarted/ToolProgress/ToolCompleted/ToolFailed`、`ApprovalRequested/Resolved`、`InputRequested`、`SessionIdle`（带 `confidence`）等；归约器维护 `AgentCorrelationState` 做 callId/turnId 关联、并行 tool、pending 集合、退出终态（`AgentEventReducer.ts:11-30`）。
- **降级不中断会话**：`README.md:55`「If an observer fails, the PTY keeps running. HRack degrades the status display instead of breaking the CLI session.」

### 4.3 适配器与 DSH 集成（确定事实）

- 每个 CLI 一个 adapter 目录：`claude / codex / opencode / pi / kimi / grok`（`electron/agents/adapters/`），统一经 `AgentEventNormalizer` / `AgentEventReducer` / `AgentEventProjector` 归一。
- 接入方式按 CLI 能力分档（`README.md:113-119`）：DeepSeek Harness 走「官方 Web surface + runtime bridge」；Claude/Codex/Kimi/Grok 走官方 Hooks；OpenCode 走 Server + SSE；Pi 走 Extension API。宿主与 WSL 双运行时。
- **DSH 集成**：`electron/dsh-host/`（`DshRuntime`、`DshHostManager`、`DshTunnelClient`、`DshWireProxy`）+ `electron/dsh-surface/`（`DshWebSurfaceController` 用 Electron `WebContentsView` **内嵌 DSH 官方 Web 界面**，并捕获其运行时 bridge `globalThis.__HRACK_DSH_EMBED__` 与 `__HRACK_DSH_HOST_BRIDGE__`）+ `electron/remote/dshRemoteSessionSource.ts`。DSH 仅在发现本地/WSL 安装后才出现。
- 有较完整的远程/多节点规格（`docs/SPEC-REMOTE.md`、`PLAN-REMOTE-P0..P8`、`SPEC-REMOTE-DSH-WEB-TUNNEL.md`）。

### 4.4 只读 workspace（确定事实）

- 受限 IPC 面：`Describe / List / Read / onChanged`（`shared/workspace-reader.ts:1-44`），`WorkspaceTextFile` 带 `truncated`、`eol`、`languageHint`；UI 在 `src/workspace-reader/`（文件树 + 只读高亮 + Markdown 预览）。设计目标是「与 agent 的 native TUI 并排查看代码而不修改 workspace」。

### 4.5 对 Ageminal E 组的借鉴价值

- **契约分层可直接采纳**：事实（事件）/ 投影（状态）/ 摘要（历史）三分离，比「一个可选字段大对象」更适合多 agent、乱序、重放场景。
- **词汇映射**：hrack 六态与 Ageminal REQUIREMENTS §10.3 的 `running/waiting/permission/done/idle/error/exited`（见 [`agent-event-mapping.md`](./agent-event-mapping.md) §二）几乎一一对应——`working→running`、`needs-you→waiting|permission`、`done→done`、`error→error`、`idle→idle`、`exited→exited`。可直接作为 E 组实现的参照蓝本。
- **能力位 + 置信度 + 观察者健康度**三件套，正好回应 [`agent-event-mapping.md`](./agent-event-mapping.md) §五「纯降级无法可靠推断 waiting/permission」的问题：显式降级状态集合而非猜测。
- **观察者失效不影响 PTY** 的隔离原则，与 Ageminal「终端与状态层解耦」一致。

---

## 五、横向对比

| 维度 | **orca** | **pebrel** | **hrack** |
|---|---|---|---|
| 形态 / 语言 | Electron + TS | Rust + GPUI | Electron + TS |
| 终端引擎 | **xterm.js**（渲染）+ **@xterm/headless**（daemon） | 自维护 Alacritty 派生（`vte`） | xterm.js |
| 渲染 | WebGL（xterm addon） | GPUI / OpenGL ES 2.0+ | WebGL（xterm addon） |
| PTY 层 | `node-pty`（Node addon） | `windows-sys` 直连 ConPTY + 旁加载 `conpty.dll`/`OpenConsole.exe` | `node-pty`（Node addon） |
| **会话持久化** | **checkpoint 快照 + 帧日志落盘 + 冷恢复** | 进程内驻留（关窗 hide）+ workspace 快照；进程退出即终止 | 仅内存环形缓冲（不跨重启） |
| 进程模型 | **独立 detached terminal daemon** 持有全部本地 PTY | **常驻 GUI 进程**持有 PTY + 单实例 mux 交接 | **Electron main 进程内** |
| IPC | UDS / Named Pipe + NDJSON + token + PID | loopback TCP + token（`mux.port`） | Electron IPC（进程内） |
| daemon 侧 VT | **有**（@xterm/headless，应答 ConPTY DA1、产出快照） | N/A（无 daemon） | N/A |
| Agent 状态接入 | 有 agent-hooks / agent-status；但非本调研重点 | CLI 识别 + 活动状态（Claude/Codex 图标） | **最完整**：adapter + 事件归约 + 六态 |
| 许可证 | **MIT** | **GPL-3.0-or-later** | **Apache-2.0** |
| 可复用性 | 仅整机；架构/格式可借鉴 | 仅整机；GPL 不可复用 | 仅整机；架构/契约可借鉴 |
| Windows 存活 UI 退出 | ✅（daemon 独立 + 镜像搬迁） | ✅ 仅关窗（进程在时）；❌ 进程退出 | ❌ |

---

## 六、对 Ageminal 已定路线的冲击与建议

Ageminal 已定：**Tauri 2 + WebView2 + xterm.js + CodeMirror 6，Windows-only，git worktree 一等公民**。

### 6.1 冲击一：orca 强背书 xterm.js，不构成挑战（确定事实）

- 用户最看好的 orca，其终端引擎就是 xterm.js（含 6.1 beta + WebGL），并有大量为性能/一致性写的补丁与测试。**没有任何 libghostty / ghostty-web 的引擎级使用证据。**
- → **xterm.js 选择被强化，不是被挑战。** 继续走 xterm.js 是正确方向。

### 6.2 冲击二：libghostty/ghostty-web「同时充当 daemon VT 与前端渲染」的设想（⚠️推断）

- 就 orca 这一手证据而言，**没有**它用 libghostty 系做 daemon VT 的事实；它用的是 `@xterm/headless`。
- 泛化判断（⚠️推测）：`libghostty-vt`（Rust、MIT、1.3.0 起支持 Windows）能在 Rust daemon 里做 VT 状态机；`ghostty-web`(WASM) 能在浏览器渲染。但「同一份引擎两端共用」的现实可行性取决于：
  - libghostty-vt 与 ghostty-web 是否共享同一 VT 语义版本；
  - web 端能否拿到与原生一致的 cell/delta 或 ANSI 序列化能力；
  - 它们相对 xterm.js 的成熟度与 IME/GPU 表现。
- **结论**：libghostty 系**目前不构成对 xterm.js 的实质性挑战**，但值得作为一个待验证的分支（需自建 spike），不宜作为既定路线。

### 6.3 冲击三（最重要）：daemon 侧 VT 的「同引擎 vs 跨引擎」

- [`session-daemon.md`](./session-daemon.md) §七.4 建议 daemon 用 `alacritty_terminal`（Apache-2.0）做 VT 内核。**orca 的证据指向另一个结论**：它刻意让 daemon 与渲染器**共用同一个 xterm 内核**，并为此写 parity fixture 与 fuzz 测试。原因很实在——两个不同 VTE 实现对同一字节流的**屏幕重建结果可能不同**（字符宽度、Unicode 版本、鼠标/键盘模式、alt-screen、转义边界），而 attach 恢复要求两端逐格一致。
- → 若 Ageminal 渲染器用 xterm.js、daemon 用 Rust VTE，就要承担 orca 一直在规避的**跨引擎一致性成本**。这是 B1 当前的实质性挑战。
- 可选路线（**待用户拍板** → ✅ 已定：自研 daemon，见本文顶部）：
  1. **同引擎**：daemon 内嵌 JS/WASM 跑 `@xterm/headless`（如 `deno_core`/`quickjs`/V8），渲染器用 `@xterm/xterm`。一致性最好，代价是 Rust daemon 里塞 JS 运行时。
  2. **Node sidecar daemon**：直接复用 orca 形态（Node + `@xterm/headless` + `node-pty`），UI 是 Tauri。工程最快、行为最接近 orca，代价是项目要背一个 Node 运行时与 Node 原生模块（`node-pty`）。
  3. **Rust daemon + Rust VTE**（`alacritty_terminal`/`vte`/`libghostty-vt`），接受跨引擎 parity 测试负担。
  4. **最小应答器**：daemon 只应答 ConPTY 查询、原始字节回放。省事，但 alt-screen 全屏 TUI 恢复会失真（[`session-daemon.md`](./session-daemon.md) §5.2 已论证）。

### 6.4 可直接借鉴的成品设计（不涉及代码许可）

1. **持久化格式照抄 orca**（MIT，且与语言无关）：
   - `checkpoint.json` = 屏幕快照（ANSI 序列化）+ 模式/大小/cwd/标题/未完成转义尾巴；
   - `output.log` = `magic + u8 version + u32le generation` 头部 + `u8 kind + u32le len + payload` 帧，batch 带 `u32le seq`；
   - 恢复 = 读 checkpoint → 按 seq 续放 → **残尾截断**、**seq 断档拒绝重放**。
2. **ConPTY 查询必须有人应答**：orca 用 `installDeviceAttributesResponder` 处理 DA1，并区分「启动期」与「持续期」所有权。这正是 [`session-daemon.md`](./session-daemon.md) §5.2 的实证。
3. **Windows daemon 镜像搬迁躲安装器清扫**：Tauri 的 NSIS 安装器同样会按安装目录清理进程；把 daemon 镜像放 `%LOCALAPPDATA%` 是廉价且有效的存活手段。
4. **崩溃环治理**：60s 窗口 5 次上限 + 显式「操作员重启清除」。
5. **端点所有权协议**：bind 私有名 → 独占 link → 证明旧主死亡 → 单次 rename；不删别人的名字。
6. **旁加载 `conpty.dll` + `OpenConsole.exe`**：pebrel 已在 Windows 生产落地，规避内置 ConPTY 版本碎片。[`session-daemon.md`](./session-daemon.md) §六.3 的推测得到印证。
7. **hrack 的状态契约**：事实/投影/摘要三分离 + 能力位 + 置信度 + 观察者健康度；观察者失效不阻断 PTY。

---

## 七、可复用性总表

| 项目 | 许可证 | 平台 | 是否库 / 可嵌入 | 可否直接依赖 | 可借鉴的部分 |
|---|---|---|---|---|---|
| **orca** | MIT | Win/mac/Linux | ❌ 仅整机（应用内模块） | ❌ | 终端架构、daemon 形态、checkpoint+帧日志格式、Windows 镜像搬迁、ConPTY DA1 应答 |
| **pebrel** | **GPL-3.0-or-later** | Win/mac/Linux | ❌ 仅整机 | ❌（且 GPL 传染） | ConPTY 旁加载、驻留/单实例交接行为、会话快照边界（clean-room 参考） |
| **hrack** | Apache-2.0 | Win/mac/Linux | ❌ 仅整机 | ❌ | Agent 事件契约、六态词汇、adapter 模式、只读 workspace IPC 设计 |
| xterm.js / @xterm/* | MIT | Web | ✅ 库 | ✅（已选） | orca 证明其可承载 daemon 侧 headless 使用 |
| node-pty | MIT | Win/mac/Linux | ✅ Node addon | ⚠️ 仅 Node 宿主 | orca/hrack 的 PTY 层；Tauri Rust 核不可直接用 |

> 三个参考实现**没有任何一个提供可依赖的终端/daemon 库**——与 [`session-daemon.md`](./session-daemon.md) 对 Contour/WezTerm/Zellij 的结论一致：只能作参考实现。

---

## 八、待用户拍板的点

> ✅ **已定 —— B1 #4**：自研 Rust daemon；VT 内核 `alacritty_terminal`；**「是否内置完整 VT 状态机」由 #42 实测判定：必须内置**。

1. **B1 的 daemon VT 引擎路线**（本调研最关键的待决项）：同引擎（daemon 内嵌 xterm-headless，需 JS/WASM 运行时）、Node sidecar（复刻 orca）、Rust VTE（接受 parity 成本）、还是最小应答器（接受 alt-screen 失真）？
2. **是否接受项目引入 Node 运行时**：orca 形态在工程上最省、行为最有保证，但与「Tauri + Rust daemon」的既定气质冲突。
3. **是否照抄 orca 的 checkpoint + 帧日志持久化格式**（含残尾/seq 断档语义），以及 scrollback 是否落盘。
4. **是否采用「daemon 镜像搬迁到 `%LOCALAPPDATA%`」以活过 Tauri NSIS 自动更新**。
5. **是否采纳 hrack 的 Agent 状态契约**（事实/投影/摘要 + 六态 + 能力位/置信度/健康度）作为 E 组蓝本，并与 REQUIREMENTS §10.3 词汇对齐。
6. **确认 pebrel 仅作参考、不复用其 GPL 代码**。

---

## 九、尚存不确定点

- orca 使用 xterm **6.1 beta** 并自行维护 6 个补丁；⚠️这条 beta 通道的稳定性与升级成本未知，不应直接把「用 beta」当成 Ageminal 的结论。
- `@xterm/headless` 的快照 + 增量能否**完整**覆盖 alt-screen 全屏 TUI 的冷恢复，orca 仍在用 parity fixture 与 fuzz 测试验证（`headless-emulator-fidelity.fuzz.test.ts`）；⚠️这暗示该问题并非「已完全解决」。
- orca 的 `orcad`（Node 服务端）主要用于 headless Linux / 远程；桌面版是 Electron 主进程直接 fork daemon。两者共享 daemon，但**桌面端的确切 kill/存活边界**（安装器、崩溃、系统关机）需以 `win-crash-survival-e2e` / `win-update-survival-e2e` 的断言为准，本文未逐条运行。
- hrack 的 DSH 隧道/远程多节点规格（`SPEC-REMOTE*.md`）体量很大，本文只核到集成机制，未逐条评估其远程方案。
- libghostty-vt / ghostty-web 的实际一致性能力未做一手验证（仅有 [`ui-editor-route.md`](./ui-editor-route.md) 的间接信息）；若要考虑该分支，需独立 spike。

---

## 十、来源链接（含源码路径）

> 本地浅克隆（只读）：`/tmp/opencode/refs/{orca,pebrel,hrack}`。行号对应该克隆的 HEAD（orca `b0ec11f`、pebrel `a95b6c6`、hrack `13a5a8c`）。

**orca（`github.com/stablyai/orca`，MIT）**
- 终端依赖：`package.json:180-181`（`@xterm/addon-serialize`、`@xterm/headless`）、`:186`（`node-pty`）、`:242-249`（`@xterm/addon-*`、`@xterm/xterm`）
- 补丁依赖与工作区：`pnpm-workspace.yaml`（`patchedDependencies`、`allowBuilds`）
- README 营销措辞：`README.md:65`
- daemon 侧 VT：`src/main/daemon/headless-emulator.ts:2,55-56,90,94-98,108-120,252-284`
- ConPTY DA1：`src/main/daemon/startup-device-attributes-responder.ts:2-24`
- 进程模型/端点/健康：`docs/reference/orcad-operations.md:7-30,134-147,179-195`
- 启动与脱附：`src/main/daemon/daemon-launched-child.ts:59-101`
- socket/pipe/token/PID：`src/main/daemon/daemon-spawner.ts:124-215`
- 端点所有权协议：`src/main/daemon/AGENTS.md:3-51`
- RPC 方法：`src/main/daemon/daemon-request-router.ts`
- 持久化：`src/main/daemon/terminal-history-session-files.ts:8-13`、`terminal-history-log.ts:3-16,80-118`、`daemon-checkpoint-file.ts:9-28`、`terminal-history-paths.ts:12-18`、`daemon-pty-checkpoint-persistence.ts:9-90`
- Windows 镜像搬迁：`docs/reference/windows-daemon-host-relocation.md:4-7,31-56`、`src/main/daemon/daemon-host-relocation.ts:1-47`
- 打包/原生物：`config/electron-builder.config.cjs`、`native/`、`src/main/ghostty/`

**pebrel（`github.com/Kuddev/pebrel`，GPL-3.0-or-later）**
- README（驻留与对话恢复边界）：`README.md:60-64,134`
- 终端核心许可与依赖：`nebula_terminal/Cargo.toml:1-6,26,39-51`
- Windows ConPTY 旁加载：`nebula_terminal/src/tty/windows/conpty.rs:39-48,76-90`
- 驻留：`nebula_app/src/gpui_shell/workspace/residency.rs:1-5,39-50,997-1035`
- 单实例 mux：`nebula_app/src/mux.rs:1-22`
- 会话快照：`nebula_app/src/gpui_shell/workspace/session_persistence.rs:1-110`、`docs/architecture-decisions.md:248`
- 许可：`LICENSE`（GPL v3）

**hrack（`github.com/UniRound-Tec/hrack`，Apache-2.0）**
- README（双通道架构、状态词、只读 workspace）：`README.md:41-62,87,113-119`
- 终端依赖：`package.json:155-157,162,20`
- 事件契约与六态：`shared/agent-events.ts:5-7,13-38,187-203`
- 归约器：`electron/agents/AgentEventReducer.ts:1-30`
- PTY 与内存历史：`electron/pty/PTYManager.ts`、`electron/pty/PtyHistory.ts:1-38`
- 适配器：`electron/agents/adapters/{claude,codex,opencode,pi,kimi,grok}/`
- DSH 集成：`electron/dsh-host/`、`electron/dsh-surface/DshWebSurfaceController.ts`、`electron/dsh-surface/officialRuntimeCapture.ts`、`electron/remote/dshRemoteSessionSource.ts`
- 只读 workspace IPC：`shared/workspace-reader.ts:1-44`、`src/workspace-reader/`
- 远端规格：`docs/SPEC-REMOTE.md`、`PLAN-REMOTE-P0..P8`、`SPEC-REMOTE-DSH-WEB-TUNNEL.md`

**本项目既有调研**
- [`session-daemon.md`](./session-daemon.md)（B1 候选、ConPTY 约束、daemon 侧 VT）
- [`daemon-ipc-protocol.md`](./daemon-ipc-protocol.md)（Named Pipe + 长度前缀帧）
- [`agent-event-mapping.md`](./agent-event-mapping.md)（E 组状态机映射）

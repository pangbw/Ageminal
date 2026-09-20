# 技术路线调研：会话守护进程（session daemon）形态选型

> **状态回写（2026-09-20）**：**§八「待用户拍板的 6 点」已全部定案**（见下表）；**§七建议中 `portable-pty` 未被采用**（B1 #4 选了 `alacritty_terminal` 的 tty；PoC 里裸 ConPTY 也证明了 `Pty` trait 可替换）。
>
> **三条 ConPTY 硬约束**由 **#40 真机实测**得出，已写入 `REQUIREMENTS.md` §5.1：① `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE` 的 `lpValue` 是**句柄值本身**；② `CreatePipe` 两端要等 **`CreateProcess` 之后**才释放；③ 必须给子进程显式 `STARTF_USESTDHANDLES` + **三句柄 NULL**（否则继承 daemon 的 NUL std handle → cmd 立刻 EOF **干净退 0** 的沉默失败）。
>
> **实测**：UI 关闭后进程与输出存活 + 重开回放 ✅（断开 5 s 后重连，回放 964 字节 / 7 个断开期间产生的 TICK，shell 存活）。
>
> | §八 待拍板 | 结论 |
> | --- | --- |
> | 1 daemon 的终端模拟深度 | **完整 VT 状态机（路线 C）** —— B1 #4 定，**#42 实测达标** |
> | 2 PTY 层 | 封在 `Pty` trait 后（B1 #4）；正式选择 `alacritty_terminal`（`default-features = false` 时依赖树仅 **68 行**，不拖 winit/crossfont） |
> | 3 IPC 抽象 | 只做 Windows 的 `tokio` Named Pipe（B2 #5） |
> | 4 自启与退出语义 | **默认不自启**；退出入口唯一；daemon 崩溃**只重建骨架**（B4 #7 / B5 #8） |
> | 5 输出缓冲策略 | 内存环形缓冲 + **ANSI checkpoint 与帧化日志落盘**、有界 GC（B3 #6） |
> | 6 修正 §5.3 的 wmux 描述 | **已修正**（Electron/TS；架构先例，不可复用） |

> 调研日期：2026-09-19
> 背景：Ageminal 需要「UI 关闭后终端进程与输出继续存活，重开 UI 按 session id 重 attach 并回放缓冲」。本文核实常驻会话守护进程应自研还是复用现成方案，以及 Windows 上的落地形态。
> 对应：REQUIREMENTS §5、§18.5、§18.9；issue #4
> 记号：**确定事实**来自官方文档 / crates.io API / 源码 / GitHub issue 原文 / GitHub API；**⚠️推测**为无直接来源的推断。

---

## 一、结论（TL;DR）

1. **守护进程必须是一个独立于 UI 的进程，这是需求硬约束，不是可选实现。**
   ConPTY 的伪控制台句柄（HPCON）由创建它的进程持有；MS 官方文档明确：调用 `ClosePseudoConsole` 会终止所有已附加的子进程及其进程树。因此「UI 直接持有 PTY」无法满足「关 UI 后进程存活」；而守护进程本身崩溃时，其持有的 ConPTY 随之关闭，被托管的进程也会死——**这解释了 §5.4「跨守护进程崩溃只能尽力而为、进程无法复活」的根因**。

2. **自研 Rust 守护进程（ConPTY + 本地 IPC）是正确方向，且是业界一致做法。**
   三个成熟先例——Contour daemon（C++）、WezTerm mux server（Rust）、以及同为 Windows AI-agent 终端的 wmux（Electron+TS）——**无一例外**都采用「独立后台进程持有 PTY + 环形缓冲 + attach/detach RPC」。自研不是另辟蹊径，而是复刻已被验证的架构。

3. **「复用 wmux」不成立——REQUIREMENTS §5.3 对 wmux 的描述与一手事实不符。**
   当前叫 `wmux` 且最主流的两个项目（`amirlehmam/wmux` 385★、`openwong2kim/wmux` 389★）都是 **Electron + TypeScript + node-pty**，不是 Rust，也没有可依赖的库形态。Rust 同名项目都极小（≤17★）、互不相关。wmux 只能作为**架构先例**，不能作为依赖复用。

4. **Contour / WezTerm / Zellij 都不能作为组件复用，只能作为参考实现。**
   三者都是完整终端应用（非库）；Contour 是 C++23，复用等于采用其整机或重实现其协议；WezTerm 的 mux 是应用内特性、未发布为 crate；Zellij 的**原生 Windows 支持 2026 年才刚落地**，且仍有一串未修 Windows issue。

5. **真正的技术难点不在「怎么让进程活着」，而在守护进程要不要内置终端模拟器。**
   Windows ConPTY 会向「终端侧」发查询（如 `CSI 6n` 光标位置报告请求）；无 UI 附着时若无人应答，被托管的程序会**阻塞**（WezTerm #6783 维护者原话）。同时 alt-screen 全屏 TUI 的「重放」无法靠原始字节流忠实重建。因此**守护进程侧需要解析/持有终端状态**（Contour 明确「server emulates, client renders」），这是本选型最大的成本项与待拍板点。

**可行性总判**：**自研 Rust 守护进程可行，且是唯一符合需求的路线**；跨平台现成方案没有一个是能直接复用的守护进程库。建议：常驻二进制 + ConPTY + Named Pipe + 守护进程侧 VT 状态；打包进 Tauri 安装器但**自行 detached 拉起**，不依赖 Tauri sidecar 生命周期。

---

## 二、候选方案对比表

| 候选 | 形态 / 语言 | 许可证 | 最近活跃 | Windows 成熟度 | 可否作为依赖复用 | 「UI 关闭后存活」 |
|---|---|---|---|---|---|---|
| **自研 Rust**（ConPTY + Named Pipe） | 常驻二进制 / Rust | 自选（crate 均 MIT/Apache） | — | 取决于选用 crate；ConPTY 是系统 API | ✅ 自持代码 | ✅ 独立进程天生满足 |
| **复用 wmux** | 应用（Electron/TS） | MIT | 极活跃（今日仍在推） | 高，但**非 Rust、无库** | ❌ | 主流 wmux 之间不一致（见 §三） |
| **Contour daemon** | 应用（C++23） | Apache-2.0 | 活跃 | daemon 模式 0.7.0 新出、**实验性** | ❌（整机/协议） | ✅ daemon 独立，但**会话不跨 daemon** |
| **WezTerm mux server** | 应用（Rust） | MIT（portable-pty 抽离部分） | 仓库活跃，**稳定版停在 2024-02** | 有 Windows，mux 自述「年轻、快速演进」 | ❌（mux 未发布 crate；可复用仅 `portable-pty`） | ✅ mux server 独立于 GUI |
| **Zellij** | 应用（Rust） | MIT | 活跃 | **原生 Windows 支持刚落地**，issue #4745 未关 | ❌（不支持内嵌） | Unix 上可，Windows 上不稳 |
| 进程内 PTY + 独立恢复进程 | 架构 | — | — | — | — | ❌ 关 UI 即终止会话，违背需求 |
| daemon 做成 Tauri sidecar | 打包方式 | — | — | 打包成熟 | — | 取决于拉起方式（见 §五） |

> 星标/活跃为 2026-09-19 的 GitHub API 快照。

---

## 三、Windows 成熟度（分候选，确定事实优先）

### 3.1 自研 Rust：可用的构建块

**ConPTY（系统层）**
- 引入于 **Windows 10 版本 1809（2018-10）**；`portable-pty` 在加载失败时的报错原文即「Windows 10 October 2018 or newer is required」。Ageminal 已把系统下限定为 1809，正好对齐。
- API 三件套：`CreatePseudoConsole` / `ResizePseudoConsole` / `ClosePseudoConsole`；通信通道是**同步**管道（不能用 OVERLAPPED），子进程通过 `STARTUPINFOEXW` + `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE` 创建。
- **关闭语义**：`ClosePseudoConsole` 会终止附加的客户端进程及整棵进程树；官方还警告单线程同步使用会死锁（收尾时仍有最后一帧要排空）。→ 守护进程必须多线程/异步排空通道。

**PTY 封装 crate**

| crate | 版本 / 更新 | 许可 | 评价 |
|---|---|---|---|
| `portable-pty` | 0.9.0 / 2025-02-11 | MIT | wezterm 抽离，Windows 走 ConPTY，是最省事的 v1 选择；但 Windows 细粒度问题不少（见下） |
| `windows` / `windows-sys` | 0.62.2 / 0.61.2 / 2025-10-06 | MIT OR Apache-2.0 | 微软官方，极活跃；提供 ConPTY 原始绑定。自己写管道/生命周期，控制力最强 |
| `conpty` | 0.7.0 / 2024-09-23 | （见 crate） | 轻量 ConPTY 封装，更新较久 |
| `pty-process` | 0.5.3 / 2025-07-12 | （见 crate） | 面向「在 pty 上 spawn 命令」，非 daemon 库 |

- `portable-pty` 源码事实：Windows 实现在 `pty/src/win/`；若同目录存在 **sideload 的 `conpty.dll`** 就优先用它，否则回落 `kernel32.dll`；创建时带 `PSEUDOCONSOLE_INHERIT_CURSOR | PSEUDOCONSOLE_RESIZE_QUIRK | PSEUDOCONSOLE_WIN32_INPUT_MODE`。
- `portable-pty` 的 Windows 未关 issue（截至 2026-09-19）：
  - **#6783（open）**「0.9.0 在 Windows 下 read 返回乱码」——wez 回复：那是 ConPTY 启动时的 `CSI 6n` 光标位置查询，**你不回它就死锁**；并明言「我如今极少用 Windows，不太可能有时间在 Windows 上直接调试」。
  - **#7025（open）**「Windows 上命令直接不启动」、**#4205（open）**「Windows 上 `CommandBuilder` 丢用户自定义 PATH」、**#4206（open）**「drop slave 后无法写入」。
  - **#7782（open，2026-05）**「Windows 启动慢 15–50 秒，日志显示卡在 `portable_pty::cmdbuilder`」——严重但疑似个体环境。
  - **#7742（open）** spawn 失败时子进程 abort、真实错误丢失（Unix 路径为主）。
- **⚠️推测**：wezterm 自身 nightly 在 Windows 用这套代码是「working correctly」（wez 原话），所以严重 bug 多与调用方未实现终端语义有关；但「维护者基本不用 Windows + 大量未关 Windows issue」意味着**选它就要接受自己兜底修**。

**本地 IPC**
- `tokio::net::windows::named_pipe`（Windows-only，feature `net`，MIT）：`NamedPipeServer`/`Client`，`ServerOptions` 提供
  - `first_pipe_instance(true)` → **单例**（重复创建返回 `PermissionDenied`）；
  - `reject_remote_clients` 默认拒绝远程客户端；
  - `create_with_security_attributes_raw(SECURITY_ATTRIBUTES)` → **自定义 DACL，做按用户隔离**；
  - `pipe_mode` / `max_instances` / `in_buffer_size` / `out_buffer_size`。
- `interprocess` 2.4.4（2026-09-03，**0BSD OR Apache-2.0**，维护状态「passive」但持续发版）：旗舰能力 `local_socket`，Windows 端即 Named Pipe，支持 Tokio 异步；提供跨平台统一抽象。⚠️ 其按用户 ACL 的显式程度不如 tokio 直连 Win32；作者声明不接受 LLM 生成代码的贡献（不影响作为依赖使用，但影响未来向上游提 PR 的意愿）。

### 3.2 wmux：先例存在，但不存在「可复用的 Rust wmux」

- `amirlehmam/wmux`（**385★，TypeScript，MIT，今日仍在推送**）：自述 “The original Windows terminal multiplexer for AI agents”，**Electron + xterm.js + node-pty**。仓库内检索不到 daemon/detach/persist 实现，**会话随应用退出而终止**（`node-pty` 跑在 Electron main 进程里）。
- `openwong2kim/wmux`（**389★，TypeScript，MIT，今日仍在推送**）：自述「Windows & macOS，git worktree fan-out，**reboot-surviving sessions**」。其 `ARCHITECTURE.md` 明确：
  - 有独立 **Daemon（background）**，含 `SessionManager + RingBuffer`、`ProcessMonitor + Watchdog`，职责写明 **“Keep PTYs alive after app exit”**；
  - Daemon RPC 提供 `create / attach / detach / destroy / resize`，按会话独立数据管道，事件 `session:died` / `session:output`；
  - 落盘：`~/.wmux/session.json`、`~/.wmux/daemon.pid`、`~/.wmux/scrollback/`（**每个 surface 的 scrollback dump**）。
  - 也就是说：**它已经实现了 Ageminal 想要的守护进程架构**，只是语言/技术栈是 Electron+TS 而非 Tauri+Rust。
- Rust 同名项目：`fernandomenuk/wmux`（17★，「tmux for Windows + JSON-RPC socket」）、`shreshthkapai/wmux`（10★，「persistent multiplexer」）、`newdee/wmux`（1★）、`bahri-hirfanoglu/wmux`（3★）等——**均极小、互不相关、无生产验证**。
- **结论**：REQUIREMENTS §5.3 所称「wmux（Rust，同技术栈，MIT/Apache-2.0）」查无实据；**不能依赖 wmux 作为组件**。但它（尤其 openwong2kim 版）是 Ageminal 架构的**强先例与竞品参照**。

### 3.3 Contour daemon（C++23）

- `contour-terminal/contour`：3,023★，Apache-2.0，仓库活跃（2026-09-18 仍在推）。**daemon mode 是 0.7.0 的新特性**（0.7.0 发布于 **2026-08-17**），文档自己标注 **“Experimental”**，命令行与线协议「可能随版本变化」；客户端与服务端握手要求**构建版本精确一致**。
- Windows 行为（官方文档确定事实）：
  - **Windows 10 1803+ 起可用**；用 **AF_UNIX（`afunix.h`）** 而非 Named Pipe，权限由 **NTFS ACL** 而非 POSIX mode bits 管辖；
  - 会话由 **ConPTY** 支撑，与 GUI 的本地会话同一套代码；
  - `--background` 通过 **`DETACHED_PROCESS`** 脱离调用终端（POSIX 用 `setsid()`）；
  - `contour daemon-service install|uninstall|start|stop|...` 注册为后台服务。**默认用 Task Scheduler 2.0 + 「用户登录」触发器**（per-user，**免提权、免密码**）；SCM 服务方式需提权 + 密码且跑在 session 0（GUI 不可见），官方**尚未实现**。
- **关键限制**：文档原话 「**Sessions do not survive the daemon**」——持久化是相对 client，不是相对 daemon；daemon 停止即所有 shell 结束。
- **可否复用**：❌。它是完整 C++ 终端模拟器，不是库；与 Tauri + xterm.js 集成等于放弃自研渲染或重写其 cells+deltas 协议。**其 Windows 服务/任务计划方案值得借鉴**。

### 3.4 WezTerm mux server（Rust）

- `wezterm/wezterm`：28,951★，仓库活跃（2026-09-17），但**最新稳定 release 停在 2024-02-03**；`portable-pty` 作为独立 crate 保持发版。
- 官方 multiplexing 文档：**“multiplexing is still a young feature and is evolving rapidly”**；unix domain **「supported on all systems, even Windows」**（AF_UNIX）。mux server 独立于 GUI，因此天然满足「关 UI 存活」。
- **可否复用**：❌ 作为 daemon 库——mux 是应用内特性，未发布为 crate（`wezterm-term` 在 crates.io 返回 404）。**唯一可复用的是 `portable-pty`**。守护进程侧持有 VT 状态（`wezterm-term`）这一设计，是 Ageminal 应照抄的思路。

### 3.5 Zellij（Rust）

- `zellij-org/zellij`：35,461★，MIT，活跃；最新 v0.45.1（2026-08-28）。
- **原生 Windows 支持是 2026 年才刚 merge 的新事物**：CHANGELOG 中 “platform: native Windows support” 对应 PR #4720–#4768；官方仍开着 **issue #4745「Windows implementation issues」**（2026-02 创建，正文称「Windows 支持分支正在合并中」），且此后持续有 Windows 修复（#5335/#5336/#5364 等）。
- 定位是「终端复用器」，需要在另一个终端里运行，**不提供嵌入式库 API**。
- **结论**：现在不能作为 Ageminal 的守护进程组件；Windows 成熟度也不够。

---

## 四、许可证

| 组件 | 许可证 | 对 Ageminal（完全开源） |
|---|---|---|
| `portable-pty` 0.9.0 | **MIT** | ✅ |
| `windows` / `windows-sys` | **MIT OR Apache-2.0** | ✅ |
| `tokio`（named pipe 模块） | **MIT** | ✅ |
| `interprocess` 2.4.4 | **0BSD OR Apache-2.0** | ✅（0BSD 几乎等价公有领域） |
| `conpty` | 见 crate（MIT ⚠️待核） | ⚠️ 使用前核对 |
| `alacritty_terminal` 0.26.0 | **Apache-2.0** | ✅ |
| Contour | **Apache-2.0** | ✅（但非库） |
| WezTerm / portable-pty | **MIT** | ✅ |
| Zellij | **MIT** | ✅（但非库） |
| `openconsole.exe` / `conpty.dll`（microsoft/terminal 发行） | **MIT** | ✅ 可随包分发（wezterm 即这么做） |

> 注意：`interprocess` 作者的 “Anti-LLM notice” 只针对**向其上游贡献代码**，不构成使用许可限制。

---

## 五、「UI 关闭后存活」实现机制（逐候选）

### 5.1 自研 daemon：进程独立性如何落地

需求本质：**让持有 HPCON 的进程不是 UI 进程**。Windows 上子进程默认不会因父进程退出而被杀（除非被显式放进 Job Object）。可行做法：

1. **daemon 是独立 exe**，由 UI 以 **detached** 方式拉起：`DETACHED_PROCESS`（必要时 `CREATE_BREAKAWAY_FROM_JOB`），且**不要**让 daemon 把 stdout/stderr 绑到 UI 的管道上（否则 UI 一退管道即断）。Contour 的 `--background` 正是这么做的。
2. **单例与按用户隔离**：
   - 单例：Named Pipe `first_pipe_instance(true)`（重复创建 `PermissionDenied`）+ 一个具名 Mutex；
   - 按用户：管道名内嵌当前用户 SID（如 `\\.\pipe\ageminal-<sid>-...`），并用 `create_with_security_attributes_raw` 设置只允许该用户/系统访问的 DACL；`reject_remote_clients` 保持默认拒绝远程。
3. **自启（可选）**：优先 **Task Scheduler 2.0 + 当前用户登录触发器**（per-user、免提权/密码），而非 SCM 服务（需提权 + 密码、跑 session 0、GUI 不可见）。Contour 的默认策略即此，且把触发器绑定到安装用户的 SAM 名。
4. **显式退出**：由「结束所有会话并退出」命令关闭 daemon，daemon 关闭所有 HPCON → 终止全部会话。

### 5.2 ConPTY 侧的硬约束（决定 daemon 必须懂终端）

- **谁持有 HPCON 谁就是「终端」**：`ClosePseudoConsole` 终止子进程树；daemon 崩溃 = 句柄关闭 = 会话进程死亡。→ **无法在 daemon 崩溃后复活进程**，只能重建骨架（对应 §5.4、§14）。
- **必须应答终端查询**：ConPTY 会向终端侧发 `CSI 6n` 等查询并等待回复；`portable-pty` #6783 里 wez 明确指出不应答就死锁。UI 在时由 xterm.js 应答；**UI 不在时必须有别的东西应答**——只能是 daemon。
- **重放不能是裸字节流**：全屏 TUI（vim/agent TUI）使用 alternate screen 与绝对定位；原始字节按序回放无法忠实重建当前屏幕。Contour 明确「**server emulates, client renders**」，其 native 协议传输的是**行列 delta**；WezTerm 同样在 mux 侧持有终端状态。Contour 文档也直说：attach 到正跑 `vim`/`htop` 的会话时「你得到的是那个程序的屏幕」。
- **结论**：daemon 侧至少需要**一个 VT 解析器/屏幕模型**，用来 (a) 无 UI 时应答查询、避免上游阻塞，(b) 为 attach 提供屏幕快照。渲染仍由 xterm.js 负责。这是最大的一笔增量成本。

### 5.3 wmux

- `openwong2kim/wmux`：**daemon 独立进程 + Named Pipe RPC + RingBuffer + 落盘 scrollback**，主进程退出后 PTY 仍活。是「关 UI 存活」的先例证明（但用 node-pty/TS）。
- `amirlehmam/wmux`：未发现 daemon/持久化实现，PTY 在 Electron main 内，**关应用即结束**。

### 5.4 Contour

- daemon 独立进程，client 可 detach/reattach，历史最多保留 `history.limit` 行；**会话不跨 daemon**。Windows 用 AF_UNIX + ConPTY。

### 5.5 WezTerm

- mux server 独立于 GUI，unix domain（含 Windows AF_UNIX）可 attach/detach；mux 侧持有终端状态。

### 5.6 Tauri sidecar 方式

- Tauri `externalBin` 把二进制**打包**进安装器；shell plugin 用 `shared_child` + `os_pipe` 启动，Windows 上加 `CREATE_NO_WINDOW`；**插件与 Tauri core 都没有 kill-on-exit / Job Object 逻辑**（core 的 `process.rs` 只管 `current_binary`/`restart`）。
- 佐证：Tauri issue **#8689** 用户报告「关闭应用 exe 后，node sidecar 仍在后台运行」并求怎么关掉它——说明 **sidecar 默认会活过 UI 退出**。
- **但**：sidecar 的 stdio 被 Tauri 持有为管道，daemon 若依赖该 stdout 通信，UI 退出即断管。**⚠️推测**：把 sidecar 作为「打包手段」可以，**不应把它作为「生命周期管理手段」**；应由 UI 用 detached 方式自行拉起 daemon，Tauri 只负责安装/卸载。

### 5.7 进程内 PTY + 独立恢复进程

- 进程内 PTY 意味着 HPCON 在 UI 进程里，**关 UI 即 `ClosePseudoConsole` = 会话终结**，直接违背核心需求。所谓「独立恢复进程」只能做崩溃后的骨架重建，不能保住运行中的进程。**不满足需求，排除。**

---

## 六、关键风险

1. **daemon 侧终端模拟的复杂度被低估**（最大风险）。若只做「哑管道」，无 UI 时上游可能因 `CSI 6n` 等查询阻塞；alt-screen 重放也会失真。需要引入/自研 VT 状态机，并定义「快照 + 增量」的 attach 协议。
2. **`portable-pty` 的 Windows 兜底成本**：维护者很少用 Windows，且有一批未关 Windows issue（#6783/#7025/#4205/#7782）。选它=接受必要时自己修，或把 PTY 层抽象成可替换 trait。
3. **ConPTY 版本碎片化**：Win10 各版本的 inbox ConPTY 行为有差异；wezterm 通过**随包分发更新版 `conpty.dll` + `openconsole.exe`** 规避（其 issue #7774 专门跟踪「更新内置 ConPTY」）。⚠️推测：Ageminal 迟早也要走这条路。
4. **进程无法跨 daemon 崩溃/重启恢复**：这是 ConPTY 语义决定的，不是实现缺陷。需在 UI 上诚实表达（区分「重连」与「重建」）。
5. **IPC 协议与背压**：字节流需长度前缀成帧；Named Pipe 缓冲区有限，daemon 输出环形缓冲需处理慢客户端/无客户端场景（Contour 用写队列 + 快照覆盖同会话未写帧，可借鉴）。
6. **安全**：管道 DACL 必须限当前用户；hook/notify 端点仅 `127.0.0.1` + 会话令牌（§15）；注意 pipe 名可被枚举，勿在名字里泄露敏感信息。
7. **Git Bash 交互**：`bash.exe --login -i` 在 ConPTY 下的登录/交互行为需实测（可能与 MSYS 环境、PATH 注入相关，见 #4205 类问题）。
8. **先例数据可能误导**：`openwong2kim/wmux` 自称 “reboot-surviving”，需注意其含义应是「活过应用重启」而非「活过操作系统重启」——后者在 Windows 上不可能。

---

## 七、对 Ageminal 的建议

1. **确定自研 Rust daemon**，架构对齐已验证先例：`UI(Tauri/WebView2) ⇄ NamedPipe(长度前缀帧) ⇄ SessionDaemon(持有 ConPTY + 环形缓冲 + 会话元数据) ⇄ shell/agent`。
2. **daemon 作为独立 exe**：通过 Tauri `externalBin` 打包分发，但由 UI **detached** 拉起（`DETACHED_PROCESS`，不接管 stdio）；单例用 `first_pipe_instance` + 具名 Mutex；按用户隔离用 SID 命名 + SECURITY_ATTRIBUTES/DACL。
3. **PTY 层**：v1 用 `portable-pty` 换速度，但封在一个内部 `Pty` trait 后面，保留切换到 `windows-sys` 原始 ConPTY 的余地；早期就把 `conpty.dll`/`openconsole.exe` 分发作**可选增强**评估。
4. **daemon 必须持有 VT 状态**：至少能应答 DSR/CPR、DA 等查询，并为 attach 产出屏幕快照。候选内核：`alacritty_terminal`（Apache-2.0，crates.io 有发布）；⚠️ 若追求与 WezTerm 同构，可参考 `wezterm-term`（未发布，只能借鉴）。
5. **IPC**：首选 `tokio::net::windows::named_pipe`（显式单例/ACL/远程拒绝）；若未来要跨平台，再评估用 `interprocess::local_socket` 包一层。
6. **自启**：默认不开；开启时用 **Task Scheduler 2.0 用户登录任务**，不用 SCM 服务。
7. **不要**把 wmux / Contour / Zellij 作为组件依赖；它们只作参考。**不要**用进程内 PTY，**不要**依赖 Tauri sidecar 的生命周期来保活。
8. **修正文档**：REQUIREMENTS §5.3 / 附录 A 中「wmux（Rust，同技术栈，MIT/Apache-2.0）」应改为「wmux 为 Electron/TS 应用（架构先例）；Rust 生态无可复用的同名项目」。

---

## 八、待用户拍板的点

> ✅ **本节 6 点已全部定案**（逐条去向见本文顶部状态回写表）。

1. **daemon 的终端模拟深度**：（a）完整 VT 状态机（可精确快照、支持多客户端、无 UI 时完整应答），还是（b）最小应答器（仅回答 ConPTY 查询，靠原始字节回放 + xterm.js）？前者成本高但正确，后者省事但有 alt-screen 失真的真实缺陷。
2. **PTY 层**：`portable-pty`（快，Windows 需兜底）vs `windows-sys` 原生 ConPTY（慢，全控）vs `conpty` crate（轻但更新久）。
3. **IPC 抽象**：只做 Windows 的 `tokio` Named Pipe，还是用 `interprocess::local_socket` 保留跨平台余地（未来 WSL 支持）。
4. **自启与退出语义**：是否提供「登录自启 daemon」；「结束所有会话并退出」之外是否要有 daemon 崩溃自动重启（只重建骨架）。
5. **输出缓冲策略**：仅内存环形缓冲，还是像 `openwong2kim/wmux` 一样把 scrollback **落盘**（`~/.wmux/scrollback/` 先例）以支持更长的无 UI 回放。
6. **是否确认修正 §5.3 的 wmux 描述**（涉及 earlier 调研报告 `ui-editor-route.md` 的一处未附来源的表述）。

---

## 九、来源链接

**ConPTY / Windows 官方**
- Creating a Pseudoconsole session — <https://learn.microsoft.com/en-us/windows/console/creating-a-pseudoconsole-session>
- CreatePseudoConsole / ResizePseudoConsole / ClosePseudoConsole — <https://learn.microsoft.com/en-us/windows/console/createpseudoconsole>
- Named pipes — <https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipes>
- microsoft/terminal（OpenConsole/conpty.dll，MIT）— <https://github.com/microsoft/terminal>

**crates.io（2026-09-19 查询）**
- portable-pty 0.9.0 — <https://crates.io/crates/portable-pty>
- windows 0.62.2 / windows-sys 0.61.2 — <https://crates.io/crates/windows> · <https://crates.io/crates/windows-sys>
- tokio named pipe 模块 — <https://docs.rs/tokio/latest/tokio/net/windows/named_pipe/index.html> · <https://docs.rs/tokio/latest/tokio/net/windows/named_pipe/struct.ServerOptions.html>
- interprocess 2.4.4 — <https://crates.io/crates/interprocess> · <https://docs.rs/interprocess/latest/interprocess/>
- conpty 0.7.0 — <https://crates.io/crates/conpty> · pty-process 0.5.3 — <https://crates.io/crates/pty-process>
- alacritty_terminal 0.26.0 — <https://crates.io/crates/alacritty_terminal>

**portable-pty / WezTerm 源码与 issue**
- Windows 实现 — `pty/src/win/{conpty,pseudocon,procthreadattr}.rs`（<https://github.com/wezterm/wezterm/tree/main/pty/src/win>）
- WezTerm multiplexing 文档 — <https://wezterm.org/multiplexing.html>
- wezterm #6783「portable-pty 0.9.0 doesn't work on windows」（含维护者对 `CSI 6n` 的说明）
- wezterm #7025 / #4205 / #4206 / #7742 / #7782（portable-pty Windows 问题）
- wezterm #7774「Update bundled Windows ConPTY pair」

**Contour**
- 仓库 — <https://github.com/contour-terminal/contour>（Apache-2.0）
- Persistent sessions（daemon mode）— <https://contour-terminal.org/persistent-sessions/>
- Daemon internals（含 Windows / 服务 / DETACHED_PROCESS）— <https://contour-terminal.org/internals/vthost/>
- Release 0.7.0 — <https://github.com/contour-terminal/contour/releases/tag/v0.7.0.8982>

**wmux**
- amirlehmam/wmux（Electron/TS，MIT）— <https://github.com/amirlehmam/wmux>
- openwong2kim/wmux（Electron/TS，MIT，含 daemon 架构）— <https://github.com/openwong2kim/wmux> · `ARCHITECTURE.md`

**Zellij**
- 仓库 — <https://github.com/zellij-org/zellij>
- Windows 实现 issue #4745 — <https://github.com/zellij-org/zellij/issues/4745>
- CHANGELOG（native Windows support PR #4720–#4768）— <https://github.com/zellij-org/zellij/blob/main/CHANGELOG.md>

**Tauri**
- Embedding External Binaries（sidecar）— <https://v2.tauri.app/develop/sidecar/>
- tauri-apps/tauri issue #8689（sidecar 在应用退出后仍在后台运行）— <https://github.com/tauri-apps/tauri/issues/8689>
- shell plugin 源码（`CREATE_NO_WINDOW`，无 kill-on-exit）— <https://github.com/tauri-apps/plugins-workspace/tree/dev/plugins/shell/src>

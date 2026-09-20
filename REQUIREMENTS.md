# Ageminal 需求文档

> 状态：**v6 · 开工前决策全部定稿**（原型阶段 · wayfinder 地图 #1 已清）
> 图例：✅ 已确认 ｜ 🕓 延后（含分支验证 / 专项调研）｜ ❓ 待调研
>
> v1 整理自前期原型与 grill 讨论；v2 补齐终端、进程模型、Agent 集成等空白；v3 敲定技术路线与应用级行为；v4 做一致性清理并回写架构/协议决议；v5 回写 C 组与工程决议；**v6 回写 wayfinder 地图（issue #1）的**全部**决议**——进程拓扑与守护进程（B1–B5）、前端与终端栈（C1–C8）、Git 与文件系统（D1–D5）、Agent 集成（E1–E6）、持久化（F1–F3）、工程/测试/CI/分发（G1–G7，见 §19–§22），以及**真机 spike 结论**（H1 中文 IME、#39 窗口材质、#40 ConPTY 守护进程 PoC、#42 VT parity、#48 通知点击）。
> 当前交付物仍为**布局原型（静态 HTML）**，不含真实实现。调研报告见 `docs/research/`。

---

## 0. 一句话

Ageminal 是一个面向个人开发者的 **ADE（Agent Development Environment）**，以 **git worktree 为一等公民**，在一处并置多个仓库 / 多个 worktree 的终端、文件与预览，专门服务于「**同时运行多个 coding agent**」。

---

## 1. 产品定位 ✅

- **名称**：Ageminal
- **形态**：ADE，具备多工作区管理能力
- **目标用户**：个人开发者
- **核心场景**：同时运行多个 coding agent（Claude Code / Codex / opencode / pi / DeepSeek Harness 等），在一处管理多个仓库、多个 git worktree 的终端、文件与预览，而不必开一堆 VS Code 窗口或 tmux 会话
- **差异化**：以 workspace / worktree 为一等公民、多项目并置、**以 agent 终端为中心**；而非像 VS Code 那样以单文件编辑器为中心
- **开源**：完全开源 ✅（许可证待定，见 §3）

### 1.1 典型工作流

1. 添加 2–3 个本地仓库，自动发现其 worktree；把在用的 worktree 加入左侧栏
2. 对某 worktree 点「启动 Claude Code」→ Ageminal 在该 worktree 目录开一个 agent 终端
3. 另一个 worktree 同时跑 Codex；第三个跑 pi / dsh
4. agent 需要输入或任务完成时，收到**通知**；点通知跳到对应终端
5. 切换 worktree 时页签**整组替换**；右侧文件树跟随当前 worktree
6. 右侧点文件 → 只读预览；需要编辑时「用外部编辑器打开」
7. **关掉 Ageminal 再打开**：daemon 保活使运行中的会话仍在（live reattach）；若进程已死，则用 **agent CLI 自身的 resume** 恢复对话（见 §5.2）

---

## 2. 运行环境 ✅

- **当前仅考虑纯 Windows**（不做跨平台）
- **前提依赖**：Git for Windows（提供 Git Bash 与 `bash.exe`）；无 Git Bash 时回退 **PowerShell**
- **Git 版本下限**：**2.36+** ✅（`git worktree list --porcelain -z` 需要；也是 worktree 发现的唯一来源，见 D1 #17）
- **系统下限**：Windows 10 **1809+**（ConPTY 与 WebView2 各自的门槛都在此之下）
- **WSL 支持延后**（现有 WSL 仅作为开发环境，不作为运行目标）
- **隐藏系统标题栏**，使用自绘标题栏；窗口操作按钮（最小化 / 最大化 / 关闭）置于页签栏右侧
- **深色主题**，界面文案 **中文**（i18n 见 §11）

---

## 3. 技术选型

| 项目 | 决策 | 状态 |
| --- | --- | --- |
| 正式产品形态 | **Tauri 2 桌面应用**（Windows 走 WebView2） | ✅ 已确认 |
| **GPUI 原生方案** | **延后：独立分支做 spike**，只验证「Windows 中文 IME 终端输入」这一点；不通过则不采用 | 🕓 延后 |
| 开源 | **完全开源** | ✅ 已确认 |
| **许可证** | **Apache-2.0**（含显式专利授权与商标条款） | ✅ 已确认（G5 #35） |
| **终端渲染** | **xterm.js 6.0.0**（VS Code 同款）；addon = `fit` / `search` / `web-links` / `unicode11`；DOM 基线 + 可见终端挂 WebGL（失败回退 DOM） | ✅ 已确认（C3 #11） |
| **文件高亮** | **CodeMirror 6**（只读预览）；语言覆盖用 `@codemirror/language-data` 懒加载 | ✅ 已确认（C4 #12） |
| **终端字体** | **Ageminal Mono** —— 由 Maple Mono NF CN v7.9 的四个样式自转 woff2（**25.1 MB**），因 OFL 保留字名而改名 | ✅ 已确认（C3 #11） |
| 编辑器内核 | **不复用 Zed 编辑器核心**（GPL-3.0、未发布、强耦合，见调研报告） | ✅ 已确认 |
| **会话守护进程** | **自研 Rust 常驻进程**（ConPTY + Named Pipe + 服务端 VT 内核）；独立 exe、UI detached 拉起 | ✅ 已确认（B1 #4） |
| 终端会话持久化（tmux？） | **不引入 tmux**；用「daemon 保活 + agent CLI resume」两条路径（见 §5.2/§5.4） | ✅ 已确认（B1/B4） |
| **前端框架** | **Vue 3**（官方 `create-tauri-app` vue-ts 模板 = 纯 Vite SPA；选型经 #47 spike 实证） | ✅ 已确认（C1 #9） |
| **前端状态管理** | **Pinia 4**（setup store）；高频数据（PTY 流 / xterm 池 / CodeMirror 视图）**不进 store** | ✅ 已确认（C8 #16） |
| **i18n** | **vue-i18n 11** + `@intlify/unplugin-vue-i18n` 预编译 | ✅ 已确认（C6 #14） |
| **Rust ↔ TS 类型生成** | **`tauri-specta`（钉 `=2.0.0-rc.25`）**；app ↔ daemon 用 `crates/protocol`（Rust 单一来源，无 TS） | ✅ 已确认（G1 #31） |
| **持久化方式** | **单一根 `%LOCALAPPDATA%\Ageminal\` + JSON**（`settings.json` / `state.json` / `sessions.json`）；非 SQLite；按文件分权、原子写、`schemaVersion` | ✅ 已确认（F1 #28，见 §14） |
| **Tauri ↔ 前端通信** | **`invoke`（命令）/ `Channel`（每会话一条：`Raw` 数据 + `Json` 控制，严格保序）/ `emit`（全局低频事件）**；PTY 合批 **≥16 KiB 或 ≥8 ms**（详见 §7.2） | ✅ 已确认（C2 #10） |
| 守护进程 ↔ UI 通信 | **Named Pipe 单连接 + 控制优先队列**；控制=JSON、数据=原始二进制帧（详见 §5.1） | ✅ 已确认（B2 #5） |
| **仓库结构与构建** | Tauri 默认布局 + Rust workspace；三个二进制（desktop / daemon / notify） | ✅ 已确认（G1 #31，见 §19） |

> **GPUI 结论摘要**：`gpui` 已发布 crates.io（Apache-2.0，可商用），Windows 后端被 Zed 正式版验证；但 pre-1.0、文档滞后、**Windows 中文 IME 有未关闭的 S2 级 bug**、无障碍缺失；且 **GPUI 与 Tauri 互斥**。故初版走 Tauri，GPUI 放到延后分支验证。详见 `docs/research/ui-editor-route.md`。

**Tauri 插件集合** ✅（G2 #32）——**全部只从 Rust 侧调用**，前端不直接调插件命令（capabilities 最小化，只给 main 窗口）：

- **采用**：`opener`（外部打开 / 资源管理器定位）· `dialog`（文件选择器）· `single-instance`（单实例）· `os`（`locale()` + Windows build）· `notification`（桌面通知）· `log`（本地日志 + 收拢前端 console）
- **不采用**：`window-state`（F1 #28 自建）· `store`（F1 #28 自建）· `fs`（Rust `std::fs` + 自研命令）· `shell`（`std::process`，缩小攻击面）· `clipboard-manager`（#21 用 `clipboard-win`，需 CF_HDROP）· `updater`（MVP 手动更新）· `global-shortcut`（快捷键应用内）· `autostart`（🕓）

**许可证** ✅（G5 #35）：项目 = **Apache-2.0**（依赖矩阵与 Apache-2.0 **兼容**，无 GPL/AGPL）。
- **强制审计进 CI**：Rust 用 **`cargo-deny`**、前端用 **`license-checker`**；**拒绝 GPL / AGPL / SSPL / 未知许可**。
- **`@iconify-json/codicon` 是 CC-BY-4.0**（内容许可）→ 按资产单独履约；**Ageminal Mono 为 OFL-1.1** → 同样履约。
- **新增「第三方许可 / 致谢」页**（设置页「关于 / 许可」）：codicon 署名 + 许可证全文、字体 OFL + 版权 + 派生说明、自动生成的依赖许可清单。

---

## 4. 核心概念与数据模型

- **Project（项目）**：一个本地 git 仓库，包含 名称 + 路径（`<path>`）
- **Worktree（工作树）**：即 **git worktree**；一个 worktree 对应一个 agent 上下文
  - worktree 列表**不落盘**，动态取自 `git worktree list --porcelain`（唯一真实来源是 git）
- **Tab（页签）**：归属于某个 worktree；类型 = 终端 / 文件预览 / 浏览器 / 其他
- **Session（会话）**：一个终端页签背后的**常驻 PTY 会话**（由守护进程持有），生命周期可长于 UI
- **用户工作区**：置顶的特殊工作区 ✅

层级主线：`Project → Worktree → Tab (→ Session)`

### 4.1 数据模型细节

- **worktree 名称来源** ✅：名称 = worktree **目录 basename**；若该 worktree 就是仓库根，则用项目名。允许在设置页重命名（仅本地显示名，不写回 git）。
- **分支显示** ✅：取 porcelain 的分支名；**detached HEAD** 显示短 SHA；bare 仓库主 worktree 单独标注。
- **「用户工作区」** ✅（取代 v1 的「默认项目 `~`」）：指向 `%USERPROFILE%` 的**非 git 工作区**，图标区别于普通项目，**不显示分支、不参与 worktree 发现**，用于跑无仓库归属的临时任务。`~` 仅作为可选显示名。

---

## 5. 进程与运行拓扑 ✅（已定稿）

> 本节已由调研 + wayfinder 决议定稿（B1 #4 / B2 #5 / B3 #6 / B4 #7 / B5 #8）。要点：自研 Rust 守护进程、服务端 VT 内核、单连接控制优先协议、有界落盘历史、单例与版本化搬迁。

### 5.1 两个进程

```
┌──────────────┐   Named Pipe    ┌──────────────────┐   ConPTY   ┌──────────────┐
│  Ageminal UI │◄───────────────►│ Session Daemon   │◄──────────►│ Shell / Agent│
│  (Tauri Web) │  控制优先帧(B2) │ (常驻, 持有 PTY) │            │  进程        │
└──────────────┘                 └──────────────────┘            └──────────────┘
        ▲                                  ▲
        │ 本地事件                        │ ageminal notify（CLI shim）
        └──────────────── Agent Hooks ─────┘
```

- **UI 进程**：Tauri 窗口，负责渲染与交互，**不直接持有 PTY**。
- **Session Daemon**：常驻后台进程（独立 exe），持有所有 PTY 与**服务端 VT 内核**、管理会话与历史缓冲、接收 Agent 事件、维护会话元数据；由 UI 以 **detached** 方式拉起（单例 + 按当前用户隔离）。
- **通信**：Named Pipe **单连接 + 控制优先队列**；控制消息用 JSON、PTY 数据用**原始二进制帧**（B2 #5）。
- **daemon 启动规格** ✅（#40 实测细化）：`DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP` 拉起、**stdio 全部指向 NUL**（与 app 管道彻底脱钩）；**daemon 不得依赖 stdout**（stdout 可能为 NULL/invalid，`println!` 失败会 panic），日志一律走文件。
- **ConPTY 三条硬约束** ✅（#40 真机实测，已与 MS 官方样例 / `portable-pty` 一手实现核对）：
  1. `UpdateProcThreadAttribute(PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE)` 的 `lpValue` 是**句柄值本身**，不是指向句柄的指针（传地址会让 Windows **另开一个控制台窗口**）；
  2. `CreatePipe` 的两端要等 **`CreateProcess` 之后**才释放；
  3. 必须给子进程显式 `STARTF_USESTDHANDLES` + **三句柄 NULL** —— 否则 `CreateProcessW` 会把 daemon 的 NUL std handle 复制给子进程，cmd 读 stdin 即 EOF、**干净退 0**（沉默失败：只有 16 字节开场序列、无任何错误）。

### 5.2 会话生命周期 ✅

**两条恢复路径（关键语义）**
- **live reattach**：UI 关闭 → 会话不销毁，守护进程继续持有进程与输出；UI 重开按 `sid` 重新 attach，回放缓冲 + 续接实时流。
- **agent resume**：进程已死（daemon 崩溃 / OS 重启）→ 用 **agent CLI 自身的 resume** 恢复**对话**（`claude --resume <id>` / `codex resume <id>` / `opencode -s <id>` / `pi --session <id>`），由 agent 自行重绘；纯 shell 页签则重建。恢复时**必须重传首次启动的注入参数**，否则「恢复了但事件不再回调」。

**标识**：持久逻辑 id = `string`（含 worktree 归属）；每次 spawn 生成 `incarnationId` 区分进程世代，迟到 exit 事件按世代丢弃。
**操作**：`CreateSession` / `Attach` / `Detach`（≠ Terminate）/ `Terminate`；关闭运行中页签二次确认（§7.3）。
**守护进程常驻** ✅：与托盘共存，直到「结束所有会话并退出」（§12）。

**live reattach 实测** ✅（#40 · Win11 26200）：断开 **5000 ms** 后重连，回放 **964 字节 / 7 个断开期间产生的 TICK**，shell 存活；**纯 IPC Ping/Pong p50 0.016 ms / p99 0.028 ms**；一次命令往返（含 shell 与 ConPTY）p50 0.88 ms / p99 1.34 ms。

### 5.3 为什么不引入 tmux 🔎

- Git for Windows **不自带 tmux**；在 Git Bash 跑 tmux 属社区 hack：需装 MSYS2、把 `tmux.exe` + `msys-event`/libevent DLL 拷进 `C:\Program Files\Git\usr\bin`，需管理员权限，且**随 Git 升级失效**——不适合做产品依赖。
- Windows 上已有成熟替代：**ConPTY（Windows 10 1809+）+ 常驻守护进程 + Named Pipe**。可参考的先例：**Contour Terminal daemon 模式**、**`wmux`（Electron/TS 应用，架构先例，内含 RingBuffer/Watchdog/落盘 scrollback）**；注意 **Rust 生态并无可复用的同名 wmux**。

### 5.4 持久化边界（会话）✅

- **跨应用重启**：✅ live reattach（进程与输出都在守护进程里）。
- **跨守护进程崩溃**：⚠️ 进程必死（ConPTY：句柄关闭即终止子进程树）；**agent 页签用 resume 恢复对话**、纯 shell 重建。
- **跨机器重启**：❌ 进程不存活；重启后 agent 页签用 resume、纯 shell 重建。
- **终端历史落盘**（B3 #6）：`%LOCALAPPDATA%\Ageminal\history\<sid>\`（checkpoint + 帧化日志）；有界（`CHECKPOINT_MAX=64 MiB` 安全网、10k 行 scrollback、总预算 GC）。
- **快照 parity 已实测** ✅（#42）：daemon 侧 Rust VT 内核产出的 ANSI 快照，经 xterm.js 重放后与内核视角**逐格一致**（alt-screen 与光标亦一致）；反证同一 fixture 下**有界尾巴回放无法重建屏幕**（30 行全错、卡在普通屏）。⇒ **服务端 VT 内核是必需项，不是优化项**。
- **守护进程镜像**：`%LOCALAPPDATA%\Ageminal\daemon-host\<version>\`（活过 NSIS 更新，保留最近 2 版）。
- **开机自启**：默认关闭。

---

## 6. 布局

- **整体** ✅：三栏 + 顶部页签
  - **左侧项目栏**：**全高，延伸到窗口顶部**（页签栏从其右侧开始）
  - **中间主区**
  - **右侧文件树**
  - **三栏可拖拽调宽，左右栏可折叠** ✅
- **左侧项目栏** ✅
  - 仅渲染**已加入侧栏**的 worktree
  - 项目节点可折叠
  - **项目名后紧跟项目路径**（左对齐，空间不足才截断）
  - worktree 行显示：名称 + 分支 + agent 状态
  - 🕓 拖拽排序；🕓 worktree 右键菜单
- **顶部页签栏** ✅
  - 从左侧栏右侧开始；页签**按 worktree 分组**，切换 worktree 时整组替换
  - 页签可关闭，超出可横向滚动
  - **新建页签 `+` 位于最后一个页签之后**，点击弹出类型菜单
- **主区** ✅：**单视图**（同一时刻只显示一个页签内容）；**分屏延后**
- **右侧文件树** ✅：根为当前选中 worktree 的根目录；顶部搜索框；**单击文件即预览**
- **设置页** ✅：全窗口视图，自带左导航（**通用 / 外观 / 快捷键 / Agent / 项目 → 各项目**）
- **窗口** ✅：标题 `Ageminal — <激活 worktree 名> (<分支>)`；最小尺寸 **900×600**
- **多窗口** 🕓 延后

---

## 7. 终端能力

### 7.1 后端

- 每个终端页签 = 一个独立 ConPTY 会话 ✅
- **默认 shell**：Git Bash（`bash.exe --login -i`），可回退 PowerShell；shell 路径自动发现 + 设置项手动覆盖
- **默认 cwd**：当前 worktree 根目录
- 终端尺寸变化（含改窗）需向 PTY 同步 resize

### 7.2 渲染与交互 ✅（C3 #11）

- 渲染层：**xterm.js 6.0.0**；addon 组合 = `fit` / `search` / `web-links` / `unicode11`（需 `allowProposedApi`，用于 **CJK 宽度**）
  - **不收**：`serialize` / `headless`（VT 与快照在 Rust daemon 侧做，B1 路线 C）、`attach`（走 Tauri IPC 非 WebSocket）、`canvas`（不兼容 xterm 6）、`ligatures` / `image` / `clipboard`(OSC 52) / `progress`（均延后）
- **渲染器策略**：**DOM 为常驻基线**；激活时给**当前可见终端**挂 `addon-webgl`，context lost / 挂载失败**回退 DOM**（同时可见的终端只有 1 个）
- **中文输入法（IME）** 必须可用（硬性验收项）：走 **WebView2 系统级 IME**，不自研。✅ **H1 #38 真机实测通过**：中文经 `onData` 正确送达（码点级验证）、**候选窗锚点精确跟随光标**（各列 x 完全相等，未复现 xterm.js #5454）、**等宽网格不被破坏**；剩余风险收窄为 **ConPTY 交互（#40）** 与第三方 IME 差异
- 复制 / 粘贴 / 全选；**右键语义**：有选中 → 复制，无选中 → 粘贴菜单；`Ctrl+C` 有选区复制、无选区发 SIGINT；`Ctrl+Shift+C/V` 同义
- 链接识别与点击：URL 在**外部浏览器**打开；**文件路径跳预览 🕓 延后**；OSC 8 走同一 handler
- **终端内搜索**：`addon-search` + `Ctrl+Shift+F`；范围 = 渲染端缓冲（10k 行）
- **字体外观**：**Ageminal Mono**（等宽，含简繁日 CJK）；默认字号 13 / 行高 1.2 / 光标 block + 闪烁；可调项归「外观」
- 滚动缓冲上限（**默认 10k 行**，可配置）✅
- **终端背景**：默认**不透明** `#080909`；透出 OS 毛玻璃是「外观」开关（`allowTransparency`，默认关）
- **resize 同步**：`CreateSession` 带初始 `cols/rows`；`ResizeObserver` 去抖 **~120 ms** → `resize` 帧 → ConPTY；隐藏 / 0 尺寸不发；重新可见先 fit 再发
- 其他选项：`convertEol: false`、`windowsPty: { backend: 'conpty', buildNumber }`、`rightClickSelectsWord: false`
- **终端标题**：支持 OSC 序列动态改标题，并在页签上显示
- **数据通路**（C2 #10）：终端数据走 **每会话一条 `Channel`** 的 `Raw` 帧，**合批 ≥16 KiB 或 ≥8 ms**（Tauri 的 `Raw` 帧 <1024 字节会退化为 eval + JSON 数字数组）；该会话的控制消息（exit / title / 状态）走同一条 Channel 的 `Json` 帧以**保序**

### 7.3 关闭与确认

- 关闭**运行中**的终端页签 → 二次确认（可设置关闭）
- 关闭窗口 ≠ 关闭会话（见 §5、§12）

### 7.4 快捷键（终端内）

- 见 §13；需解决与 agent TUI 自带快捷键的冲突

### 7.5 会话恢复的呈现

- live reattach 时先回放缓冲，再续流；在回放与实时流之间插入分隔标记（「—— 已重新连接 ——」）✅
- **渲染侧流程**（C3/C4）：写 **Snapshot(ANSI)** → 写分隔线 → 续实时帧；**seq 断档则丢弃并请求 checkpoint**（**绝不渲染半截转义序列**）；`clearScrollback` → `term.clear()`
- **隐藏会话不实时推进**（C2 #10）：只有可见 / 最近可见的会话流入并 ack；切回时走 B3 的 `Snapshot + 日志尾` 刷新

---

## 8. 页签与 worktree 交互

- 每个 worktree 独立维护**页签集合**与**当前激活页签**；切换 worktree 整组替换，切回时恢复其激活页签 ✅
- **最后一个页签关闭后**：主区显示「没有打开的页签」空态 ✅
- 新建页签菜单 ✅：终端 / 文件预览 / 浏览器 / 其他
  - **浏览器项置灰并标「即将推出」** ✅（§16.3 明确不含浏览器页签）
  - 文件预览从菜单新建时，**弹文件选择器**（根 = 当前 worktree）；取消则不建页签 ✅
- **页签操作** ✅：
  - 右键菜单：**关闭 / 关闭其他 / 关闭右侧**
  - **拖拽排序**
  - 🕓 重命名（后续刚需再加）
  - 不设硬数量上限；溢出靠横向滚动
  - 🕓 溢出「更多」下拉

---

## 9. 文件树与文件预览

- 右侧文件树，根 = 当前 worktree 根目录；顶部搜索框；**单击文件即预览** ✅
- **`.gitignore` 过滤** ✅：默认隐藏 `.gitignore` 命中的文件与 `node_modules`、`.git` 等
- **懒加载** ✅（按需读目录，大仓库性能）
- **两个开关** ✅：「显示隐藏文件」与「显示忽略文件」
- MVP **只读 `.gitignore`**，不自建忽略规则编辑器 ✅
- 预览 ✅（C4 #12）：
  - **只读**；提供「**用外部编辑器打开**」入口（系统默认程序）
  - 语法高亮用 **CodeMirror 6**；扩展集取**最小集**（行号 / 折叠 / 括号匹配 / 语法高亮 / 搜索 `Ctrl+F`；**不启用** history / activeLine / autocompletion）；只读下隐藏光标，保留选择与复制
  - 语言覆盖：官方 **`@codemirror/language-data`**（Lezer 语言包 + `legacy-modes`，**全部懒加载**）；未命中 → 纯文本
  - 支持：**纯文本 + 语法高亮 + 图片 + 二进制提示**
  - Markdown 渲染 🕓 延后
  - 大文件：**> 2 MB 截断**（另设 **50k 行**上限）并提示；**Rust 侧只回传前 N 字节**，不进内存
  - **内容判定顺序**（D4 #20）：图片扩展名 → `<img>`；**已知二进制扩展名黑名单** → 二进制；**含 NUL** → 二进制；严格 UTF-8（剥 BOM）→ 文本；**无效 UTF-8 → lossy 解码 + 顶部警告条**。GBK 🕓 延后
  - 图片走 `<img>`（上限 20 MB）；**`.svg` 当文本看（XML 高亮）**，图片渲染 🕓
  - **换行**：只读不写回；`\n` / `\r\n` / `\r` 直接按分隔符处理，不规范化、不改文件
  - **超长行护栏**：单行 > 100 KB → 顶部提示「渲染可能较慢」
  - **外部打开**（D6 #20）：`tauri-plugin-opener` 的 `open_path`；「在资源管理器中显示」= `reveal_item_in_dir`；capability 限定在工作区路径内
  - 主题：以 `@codemirror/theme-one-dark` **覆写**为 One Dark Pro Glass
  - **预览页签模型**：多个预览页签（逻辑层）+ **单个 `EditorView` + 每页签缓存 `EditorState`（含折叠 / 选区）+ 显式存滚动**；LRU 上限 **12**；**不做终端那样的 DOM re-parent**
  - **文件变更实时刷新** ✅（fs watch → 事务替换文档并恢复滚动；角落轻提示「文件已更新」）——agent 场景关键
  - 后台文件变更**标记 stale、激活时才刷新**；若正可见则立即刷新
- **右键菜单** ✅（`|` 表示分组，GUI 用**弱分组**样式区分）：
  1. 新建文件 ｜ 新建文件夹
  2. 重命名 ｜ 在文件资源管理器中显示
  3. 复制文件 ｜ 复制路径 ｜ 复制相对路径
  4. **删除**（红色强调）
  - 另有：**在终端中打开** ✅
- **删除语义** ✅：移到**回收站**（可恢复）+ **二次确认**；删除失败/文件已消失给出明确提示；永久删除（Shift+Delete）🕓 延后

---

## 10. Agent 集成

### 10.1 支持清单 ✅

**首批支持**：**Claude Code、opencode**（优先做深）；**Codex**（`notify` + 降级）；**pi**（`-e` 扩展）。
**暂不支持**：**DeepSeek Harness（`dsh`）** —— developer preview（官方保证 breaking changes）+ 官方无终端级一等 resume/TUI；标记实验性、后续评估。
✅ `dsh` = 官方 `deepseek-ai/deepseek-harness`（everything-is-plugin / Cordis；TypeScript SDK、stdio JSON-RPC）。

### 10.2 启动入口 ✅（两者都支持）

- **一等入口**：worktree 行 / 新建页签菜单提供「启动 Claude Code / opencode / Codex / pi」（**dsh 暂不支持**），自动 `cd` 到 worktree 并运行对应命令
  - 命令、参数、图标/颜色、Agent 名称 → 在设置页「Agent」中配置
- **手动入口**：保留普通终端，用户自己敲命令；识别与状态展示仍生效（能力降级见 §10.5）

### 10.3 状态与通知 ✅

- 状态机：`运行中(running)` → `等待输入(waiting)` / `需要授权(permission)` → `完成(done)` / `空闲(idle)` / `出错(error)` / `已退出(exited)`
  - **语义**：`waiting` = 等**非授权类**用户输入；`permission` = 卡在**授权**弹窗；`done` = 回合正常结束；`idle` = 空闲；`exited` = 背后进程结束 —— 明细见 `docs/research/agent-event-mapping.md`
  - **`done` 不自动迁移到 `idle`**（常驻到新活动事件把它带回 `running`）✅（E3 #24）
  - **非零进程退出 → `exited` + 标记非零退出码**，不擅自判 `error` ✅（E3 #24）
- **降级模式**（未装原生集成 / 手动启动）✅（E3 #24）：**只暴露 `running` / `idle` / `exited`**（+ 可选 `error`），并在 worktree 行/页签**显式标注「状态不完整」**；不伪装 `waiting` / `permission` / `done`
- **事件合并规则** ✅（E3 #24）：版本化状态（`state+source+timestamp+seq`，**逻辑序优先、时间序兜底**）；冲突优先级 `exited` > `error` > `permission` > `waiting` > `running`，且 `done`/`idle` **不得把 `running` 打回**；按 agent 稳定 id **幂等 + 200–500 ms 防抖**；**子代理状态默认不展示**（根会话只由根事件决定）
- 通知 ✅：进入「等待输入 / 需要授权 / 完成 / 出错」时发**桌面通知**，并高亮对应 worktree 行与页签；**`done → idle` 不重复通知**
  - **「点击通知」** ✅（#48 真机实测）：**点击通知会激活/聚焦窗口**（已实测）；官方插件在 Windows **无任何点击回调**（desktop 只注册 `notify`/`request_permission`/`is_permission_granted`，源码确认），Web `Notification` 路径在 WebView2 下**事件链不通**（`onclick`/`onshow`/`onclose` 均不触发）。因此**不做「精确跳转到对应终端」，🕓 延后**（将来需自研 WinRT toast 激活）；定位依赖**应用内高亮**（worktree 行 + 页签彩点）
- 展现位置：左侧 worktree 行（彩点 + 名称）、终端页签标题/圆点 ✅；**托盘角标 = 延后**

### 10.4 事件接入机制（调研结论）

原则：**优先用各 Agent 原生事件，不解析终端输出**。

| Agent | 完成事件 | 等待输入 / 授权事件 | 接入方式 | Windows 可用性 |
| --- | --- | --- | --- | --- |
| **Claude Code** | `Stop` / `StopFailure` / `Notification(agent_completed)` | `Notification(agent_needs_input / permission_prompt)`、`PermissionRequest`（**`idle_prompt` 归 `idle`**，E3 #24） | `settings.json` hooks：`command` 或 **HTTP hook** | ✅ |
| **Codex** | `notify`（`agent-turn-complete`，JSON payload） | hooks 事件丰富，但 **Windows 下 `PreToolUse` 有缺陷**（#24453）；首批取 `notify` | `codex -c 'notify=[...]'`（不写全局） | ⚠️ 仅完成（+ 降级） |
| **opencode** | `session.status`(idle)（`session.idle` 已废弃） | `permission.asked` / `permission.replied` | 插件订阅 `event`（v2：`define/setup/context.event.subscribe`） | ✅ |
| **pi** | `agent_settled`（扩展事件） | `ui_prompt_start` / `ui_prompt_end` | `pi -e <文件>` 扩展 | ✅ |
| **dsh** | —（**暂不支持**） | — | 官方 TypeScript SDK（stdio JSON-RPC），后续评估 | 🕓 |

### 10.5 统一投递与降级

- **统一通道** ✅：CLI shim `ageminal notify --event <name> --agent <id>`（需容忍 Codex 把 JSON 放**最后一个 argv**）；shim 经 Named Pipe 投给守护进程。
- **会话关联** ✅（E4 #25）：由 **daemon 在 spawn 会话时写入**下列环境变量（**所有会话都注入**，含普通 shell 页签；不改 `PATH`、不写全局配置）：

  | 变量 | 含义 |
  | --- | --- |
  | `AGEMINAL_SESSION_ID` | 逻辑会话 id（opaque string） |
  | `AGEMINAL_INCARNATION` | 本次 spawn 的世代 id → 丢弃上个世代的迟到事件 |
  | `AGEMINAL_PIPE` | daemon 管道**完整路径**（`\\.\pipe\…`） |
  | `AGEMINAL_TOKEN` | 握手令牌（**每次 daemon 启动重生成**） |
  | `AGEMINAL_WORKTREE` | worktree 根绝对路径 |
  | `AGEMINAL_AGENT` | agent id —— **仅作缺省回退**，`--agent` 才是权威 |

  **优先级**：命令行显式参数 > 环境变量（`--session` / `--agent`）。**被 agent 自身配置覆盖时以覆盖值为准**；关键变量缺失时 shim 安全失败（exit 0 + 日志）。
  ~~`AGEMINAL_HOOK_PORT`~~ **废弃**（E2 #23 已改 `command`+`async`，不需要 HTTP hook）。
- **不自动注入上下文** ✅：仅在启动时 `cd` 到 worktree 并暴露环境变量。
- **注入方式** ✅（**E6 #27 取代 E2 #23 的按会话机制**）：**写全局配置，由用户在设置页手动开启**；未开启时走降级（E5 #26）。

  | agent | 全局写法 | 卸载 |
  | --- | --- | --- |
  | **Claude Code** | 投放插件目录 **`~/.claude/skills/ageminal/`**（`.claude-plugin/plugin.json` + `hooks/hooks.json`）——官方 **skills-directory plugin**，**下个会话自动加载、个人作用域无信任限制** | 删目录（或 `claude plugin disable ageminal@skills-dir`） |
  | **opencode** | 投放 `~/.config/opencode/plugins/ageminal.ts`（Windows 路径 ⚠️待实测） | 删文件 |
  | **pi** | 投放 `~/.pi/agent/extensions/ageminal.ts` | 删文件 |
  | **Codex** | **文本级追加/替换** `~/.codex/config.toml` 的 `notify = [...]`（`notify` 项目级被忽略，只能用户级） | 移除该键 |
  | **dsh** | 暂不支持 | — |

  - **只有 Codex 需要改用户既有文件**（文本级追加/替换，**不丢注释**）；其余是「新增文件 / 目录」。
  - **安全边界**：启用前 **diff 预览**；改前**自动备份** `<file>.ageminal-backup-<ts>`；**停用 = 回滚**；**幂等**；**不写 `PATH`**；**绝不触碰用户其它配置键**。
  - **生效时机**：Claude 的 `hooks/` 改动需 **`/reload-plugins` 或重启**；其余下次启动生效 → 启用后**提示重启对应 agent**。
  - **状态**：`未启用` / `已启用` / `集成有更新` / `部分应用（X/Y 失败）` / `不受支持`（dsh）。
  - **测试投递**：一键跑一次 `ageminal-notify`，直接看诊断计数。
- **恢复时** ✅：全局配置已持久生效，**无需重传命令行参数**；新会话由 daemon 注入同一套环境变量（E4 #25）即可继续回调。
- **一键安装边界** ✅：安装器只做标准应用安装；Agent 配置注入走**设置页单独引导页**（默认零全局污染；「写入全局」为显式 opt-in，diff 预览 + 备份 + 回滚；**不写 PATH**）。
- **降级** ✅（E5 #26）：**只用两个信号**——`sysinfo` 枚举**会话进程树 + 识别清单**（`claude`/`codex`/`opencode`/`pi` + 自定义），与**输出静默**。
  - **只暴露 `running` / `idle` / `exited`**，并在 worktree 行/页签**显式标注「状态不完整」**；绝不声称 `waiting` / `permission` / `done`。
  - `t_idle = 60 s`（静默→`idle`，需连续 2 次轮询；有输出**立即**回 `running`）；`T_interrupt = 120 s`（**原生模式下**为「Claude 无中断事件」缺口兜底：`running` + 静默无事件超时 → 降 `idle` 并标注，新事件即恢复）。
  - **不用终端标题、不用 CPU**；**不解析输出内容**。阈值由**本地数据采集**校准（G6 #36）。
- **识别清单** ✅：命令名匹配（`claude` / `codex` / `opencode` / `pi`）+ 可配置自定义命令。

### 10.6 协作模型 ✅

- **一个 worktree 允许多个 agent 并存**（靠页签区分）
- 「深度集成」（结构化任务列表、diff 汇总）🕓 延后

---

## 11. 设置页

- 全窗口视图，自带左导航 ✅：**通用 / 外观 / 快捷键 / Agent / 项目 → 各项目**
- **通用** ✅：默认 shell、关闭确认、启动行为、语言、日志入口
- **外观** ✅：默认主题 **One Dark Pro Glass**（近黑底 `#080909`；边框 `#212121`；交互灰 `#2c313a`）；**默认不透明**（见下条）；仅深色（浅色/跟随系统 🕓 延后）；字号、密度、光标样式可调
- **窗口材质 / 毛玻璃** ✅（**#39**，**取代 C7 #15 的「按 build 自动选 Mica」**）：**默认实色、不透明近黑 `#080909`**；设置页「外观 → 窗口材质」可**开启 Acrylic**（实测唯一明显可见的效果；**需重启应用生效**，因 `transparent` 是窗口创建期属性）。正文对比度**不依赖** OS 材质与用户壁纸（实测：近黑壁纸下 Mica 与实色无差别）。**弃用 Mica / Mica Alt**（可见性取决于壁纸）与 **Blur**（官方标注 22621+ 拖动/缩放性能差）。**`backdrop-filter` 不得用于「桌面模糊」**——WebView2 采不到桌面像素（#4945，实测 A/B 对照无差异），仅可用于应用内叠层。面板 alpha 取 `0.55`~`0.85`（在不透明底上也可读），终端区始终不透明。
- **快捷键** ✅：MVP **只读展示**（不可自定义）；命令面板 🕓 延后
- **关于 / 许可** ✅（G5 #35）：**第三方许可与致谢**——codicon（CC-BY-4.0）署名 + 许可证全文、`Ageminal Mono`（OFL-1.1）版权 + 派生说明、依赖许可清单
- **Agent** ✅（**MVP 必做**）：各 agent 名称 / 命令 / 参数 / 图标颜色
  - **集成状态** ✅（E6 #27）：`未启用` / `已启用` / `集成有更新` / `部分应用（X/Y 失败）` / `不受支持`；附**上次投递成功时间**与**失败计数**
  - **操作**：**手动启用 / 停用**（启用前 **diff 预览**、改前**自动备份**、停用即**回滚**、幂等）；「**测试投递**」按钮
- **项目 → 各项目** ✅：worktree 列表 + 「加入左侧栏」开关，**与左侧栏实时联动**
  - **移除项目**（v1 遗漏项，需补）
  - 项目级设置：默认 shell 覆盖、环境变量、Agent 命令覆盖、worktree 显示名重命名
- **i18n** ✅（C6 #14）：**vue-i18n 11**（`legacy: false`）+ `@intlify/unplugin-vue-i18n`（**消息预编译**）；按功能域命名空间分 JSON（`common` / `shell` / `terminal` / `tabs` / `files` / `agents` / `settings` / `errors`）；以 **zh-CN 为类型源**（key 拼错编译期报错）
  - **两道 CI 强制**：① 扫 `src/**` 的**中文字面量**（白名单：注释、`i18n/locales/`）；② 校验 **en-US 与 zh-CN 的 key 集合完全一致**（值可空）
  - **库内建文案纳入范围**：CodeMirror 搜索面板 **17 条**（Find / Replace / next / previous / all / match case / regexp / by word / replace / replace all / close / go / …）+ `@codemirror/language` 3 条 + autocomplete 1 条 + commands 1 条；xterm **2 条**（`promptLabel` / `tooMuchOutput`）
  - **语言列表**：MVP **只列 `zh-CN`**（en-US 先对齐 key 结构，内容补齐后再入下拉）；检测走 `plugin-os.locale()`（BCP-47）→ `navigator.language` → 回退 `zh-CN`
  - **术语不译**：worktree / agent / commit / rebase / stash / Claude Code / Codex / opencode…（`docs/i18n-glossary.md`）
  - 语言选择的持久化走 **Rust 侧应用设置**（§14），**不用 localStorage**

---

## 12. 应用级行为

- **启动** ✅：恢复上次工作区（项目、侧栏、激活 worktree、页签骨架）；按 §5 对账——live reattach 或 agent resume。
- **单实例** ✅：重复启动聚焦已有窗口（多窗口 🕓 延后）
- **托盘** ✅：最小化到托盘
- **三层退出语义** ✅：
  1. **关闭窗口** → 隐藏到托盘，UI 与守护进程都活着
  2. **退出 UI** → 只关界面，**守护进程与所有会话继续在后台跑**
  3. **托盘「结束所有会话并退出」** → 唯一真正结束守护进程与会话的入口
- **自动更新** ✅：MVP 不接 updater（**手动更新**）——只有 stable 通道，下载新 NSIS **覆盖安装**；安装时**旧守护进程继续跑**（B4 #7 的镜像搬迁），新 app 启动按版本拉起新镜像（保留 2 版）；详见 §22
- **日志与诊断** ✅（G6 #36）：本地日志在 **`%LOCALAPPDATA%\Ageminal\logs\`**
  - **栈**：Rust `tracing` + `tracing-subscriber`；`tauri-plugin-log` 只负责把**前端 console 收口**到 Rust 日志（`tracing-log` 桥接）；**daemon 与 app 各自落盘，不走管道传日志**
  - **文件**：`app.log` · `daemon.log` · `notify-failures.log`（shim 失败）；**按大小滚动 10 MB × 5 份** + 总上限 GC
  - **跨进程关联**：日志带 **`sid` + `incarnation`**，关键 IPC 带 **`reqId`** → app 与 daemon 的日志可对齐
  - **格式**：文本单行 + 结构化字段（`<ts> <level> <process> <target> <sid?> <msg> key=value…`）
  - **「导出诊断」= 默认脱敏**：`%USERPROFILE%` → `<user>`、项目路径 → `<project-N>`（无映射表）、**令牌抹除**、**默认不含终端历史/对话内容**；另给「包含原始路径」勾选（默认关）；**导出前列出将包含的文件与体积供确认**
  - **`metrics.jsonl`**：本地数据采集（静默时长分布 / 降级切换 / 吞吐计数），供 E5 #26 校准阈值；可重置、不参与 schema 迁移
- **外部集成** 🕓 **全部延后**：拖拽添加项目、命令行打开、资源管理器右键
- **窗口标题** ✅：见 §6
- **远端/命令面板** 🕓 延后

---

## 13. 快捷键体系

🔎 建议默认（MVP **只读展示**，不可自定义）：

| 动作 | 建议键位 |
| --- | --- |
| 新建终端页签 | `Ctrl+Shift+T` |
| 关闭当前页签 | `Ctrl+W` |
| 下一个 / 上一个页签 | `Ctrl+Tab` / `Ctrl+Shift+Tab` |
| 按序号切页签 | `Ctrl+1..9` |
| 下一个 / 上一个 worktree | `Ctrl+Alt+↓` / `Ctrl+Alt+↑` |
| 切换左侧栏 / 右侧文件树 | `Ctrl+B` / `Ctrl+Shift+B` |
| 打开设置 | `Ctrl+,` |
| 终端内搜索 | `Ctrl+F` |
| 命令面板 | 🕓 延后 |

⚠️ 需处理与 agent TUI 的键位冲突；终端聚焦时非管理类快捷键不拦截。

---

## 14. 持久化（范围 + 方式）

**存什么**：

- 项目列表（名称 + 路径）、工作区列表与显示名
- 已加入左侧栏的 worktree；项目折叠状态
- 每个 worktree 的页签集合（类型 / 标题 / 文件 / URL / agent）与激活页签
- **会话元数据（launch spec）归 app 的 `state.json`** ✅（F3 #30）：session id、incarnationId、cwd、shell、启动命令、agentSessionId、页签骨架 → 用于对账 / 重建 / resume
  - 理由：**重建是 app 的职责，且必须在 daemon 已死时仍可用**（daemon 崩溃后不自行重启）
  - daemon 的 `sessions.json` **只存运行态**（`sid`/`incarnationId`/`exited`/`exitCode`/`startedAt`），用于**陈旧残留清理**与启动对账；**派生数据不双存**（「会话是否还活着」问 daemon，不落盘）
- 设置项（通用 / 外观 / 快捷键 / Agent）
- 窗口尺寸与位置、最后激活的 worktree

**不持久化**：worktree 列表（来自 git）。终端历史按 §5.4 落盘（有界）。

**位置与格式** ✅（F1 #28）：**单一根 `%LOCALAPPDATA%\Ageminal\`**，**JSON**（非 SQLite）。

| 文件 | 归属 | 内容 |
| --- | --- | --- |
| `settings.json` | app 独占写 | 通用 / 外观 / 快捷键 / Agent / 项目级设置（shell 覆盖、环境变量、agent 覆盖、worktree 显示名） |
| `state.json` | app 独占写 | 项目列表、侧栏、每 worktree 的页签骨架、激活项、**窗口尺寸/位置**、`agentSessionId` |
| `sessions.json` | daemon 独占写 | 运行态：sid、incarnationId、cwd、shell、启动命令、exited / exit code |
| `history\<sid>\` | daemon | 终端历史（B3 #6） |
| `logs\` | app | 日志（G6 #36） |

- **原子写**（临时文件 → fsync → rename）；状态类去抖 **~500 ms**、设置类**立即写**；退出前 flush；读失败 / 损坏则备份为 `*.corrupt-<ts>` 并重建，**不阻塞启动**。
- 每文件带 **整数 `schemaVersion`**（缺失视为 1，**与 app 版本/协议版本解耦**，仅在形状变化时 +1）✅（F2 #29）
  - **迁移链**：`migrate_N_to_N+1` 有序幂等；加载时 `< current` 逐步跑链 → **先备份** → 原子写回
  - **前向兼容**（`N > current`）✅（F2 #29）：**可写 + key 级保留未知字段** + 一次性警告「配置由更新版本写入」（不做硬只读，避免降级后改不了设置）
  - **失败/损坏备份**统一进 **`%LOCALAPPDATA%\Ageminal\backups\`**（`.migrate-failed-<ts>` / `.corrupt-<ts>`），带保留上限 + GC
  - **`sessions.json` 不做迁移**（可重建的运行态，schema 变化直接重置）；只有 `settings.json` / `state.json` 需要迁移链
  - **发布前策略**：首个公开发布前 schema 变化**备份 + 重置、不写迁移**；发布后**每次变更必须附迁移**（CI 可用历史夹具校验）
- **按文件分权**（app ↔ daemon 无共享写）→ **不需要文件锁**。

**单一事实来源 = Rust 侧持久化** ✅（C8 #16）：前端 Pinia store 只是**内存投影**——启动由 Rust 下发快照 hydrate，修改走命令 → Rust 落盘 → 回推事件。**MVP 不做乐观更新**；**不用** localStorage / `pinia-plugin-persistedstate`。纯 UI 状态（面板宽度、弹层、搜索框）只在前端。

---

## 15. 非功能需求

- **国际化** ✅：上 i18n 框架，见 §11
- **性能目标** ✅：冷启动 < 2s；20 worktree × 3 页签下流畅；终端高吞吐不丢字；文件树首屏 < 300ms
- **兼容** ✅：路径含空格 / 中文 / Unicode 必须支持；超长路径（MAX_PATH）、网络驱动器、权限不足需明确降级或报错
- **错误边界** ✅：未安装 Git for Windows、路径失效、非 git 目录、`git worktree` 失败，都要有明确提示
- **安全** ✅：Named Pipe ACL 限当前用户；hook 端点仅 `127.0.0.1` + 会话令牌；路径规范化校验
- **无障碍**：🔎 不设目标（Tauri/WebView2 本身支持较好，但 MVP 不做专项）
- **安装器体积** ✅：应用本体之外主要是终端字体 **Ageminal Mono ≈25 MB**（woff2 已压缩，安装器压不动）；见 §19

---

## 16. 范围划分

### 16.1 原型 MVP（必须）✅

- 左项目树
- 终端页签（可多开）
- 右侧文件树
- 文件预览页签

### 16.2 纳入正式 MVP 的方向 ✅

- **会话恢复**：两条路径——**daemon 保活（live reattach）** + **agent CLI resume**（B1/B4）；不引入 tmux
- **Agent 一等启动入口**（**首批 Claude Code、opencode**；Codex、pi 次之；dsh 暂不支持）
- **Agent 状态 + 等待输入/完成通知**
- **文件树默认过滤 `.gitignore` / `node_modules`**
- **文件预览只读 + 外部编辑器打开 + 变更自动刷新**
- **启动恢复上次工作区**
- 三栏可调宽 / 可折叠
- 单实例 + 托盘 + 三层退出语义

### 16.3 明确延后 🕓

- **GPUI 原生方案验证**（独立分支 spike，只验证 Windows 中文 IME 终端）
- 浏览器页签 / 内嵌网页
- 主区分屏
- worktree 的创建 / 删除（原型只做「发现」）
- agent 深度集成（结构化事件、任务列表、diff 汇总）
- 外观浅色 / 跟随系统
- 快捷键自定义、命令面板
- 左侧栏拖拽排序、worktree 右键菜单
- 多窗口
- 外部集成（拖拽添加 / 命令行 / 资源管理器右键）
- 自动更新、开机自启
- 页签重命名、Markdown 渲染、GBK 编码、永久删除
- **Snap Layouts**（悬停最大化按钮；与 900×600 冲突、需自写 Win32，C7）
- **dsh 集成**（E2；developer preview + 无官方终端 TUI）
- **把预览升级为可编辑**（C4；CM6 已保留官方路径 `lsp-client` / `merge` / `collab`，但 §9 的「预览只读、编辑走外部编辑器」定位未变）

---

## 17. 原型交付物与验收

- **交付物**：单个自包含 `index.html`，深色主题、中文 ✅ **（已完成）**
- **主题**：**Zed「One Dark Pro Glass」**（近黑底 `#080909` + `#212121` 边框）；**默认不透明**，可开启 Acrylic（#39）；UI 文字按可读性提亮（正文 `#e6e9f0` / 次级 `#b8bfcc` / 弱化 `#949cab`）✅
- **已 mock 的 11 个屏幕** ✅
  1. 主界面（左项目树 + 页签 + 终端 + 右文件树）
  2. 空状态（无任何项目）
  3. 设置页 ·「项目」分页（工作树列表 + 加入左栏开关）
  4. 设置页 ·「通用 / 外观」
  5. 添加项目对话框
  6. 新建页签类型菜单（含 Agent 入口）
  7. 文件树右键菜单（四组 + 弱分组 + 红色删除）
  8. Agent 启动入口（worktree 行 ✦ 按钮 → 菜单）
  9. Agent 状态 / 通知（三色通知卡 + 彩点）
  10. 会话恢复提示（重连横幅 + 回放分隔线）
  11. 三栏拖拽调宽 / 折叠
- **交互为模拟**：拖拽调宽、折叠、页签关闭、菜单、通知均为可用交互；数据内联、无持久化
- **验收清单** ✅
  - 添加一个项目路径
  - 左树显示其 git worktree
  - 为某 worktree 开多个终端页签、可执行命令
  - 右侧可浏览该 worktree 文件树
  - 单击文件在预览页签只读打开
- 细节留待开发阶段完善；由用户负责验收

---

## 18. 待办 / 后续调研

> wayfinder 地图（issue #1）已定案多项；下表标注状态。

1. 前端框架选型 → ✅ **C1（#9）**：Vue 3
2. 持久化方式（位置 / 格式 / 迁移）→ ✅ **F1–F3（#28–30）**
3. Tauri ↔ 前端通信 → ✅ **C2（#10）**：`invoke` / `Channel` / `emit` 三分工 + PTY 合批
4. 页签与 PTY 生命周期管理（含重连协议）→ ✅ **B5（#8）**
5. 会话持久化实现（守护进程 / 帧协议 / 缓冲 / 单例 / 崩溃）→ ✅ **B1 #4 / B2 #5 / B3 #6 / B4 #7**（**#40 已在 Windows 真机验证**：存活/重放 + 延迟 + 三条 ConPTY 约束）
6. Agent 集成注入方式 → ✅ **E2（#23）**
7. 各 Agent 事件 → 统一状态机映射 → ✅ **E3（#24）**（见 `docs/research/agent-event-mapping.md`）
8. 终端 IME 实测 → ✅ **H1（#38）**（真机通过：码点正确、候选窗锚点跟随光标、等宽网格未破坏）
9. 正式阶段运行拓扑与进程模型 → ✅ **B1–B5**
10. 【延后分支】GPUI 方案 spike → 🕓
11. 开源许可证选型 → ✅ **G5（#35）**：Apache-2.0
12. 窗口毛玻璃实现 → ✅ **C7（#15）· #39**（已改为「默认实色 + 可选 Acrylic」）
13. Agent 会话 resume 机制 → ✅ **#46**（`docs/research/agent-resume.md`）
14. xterm.js 集成（addon / 渲染器 / 字体 / resize）→ ✅ **C3（#11）**
15. CodeMirror 6 集成与语言覆盖 → ✅ **C4（#12）**
16. 组件与样式方案 → ✅ **C5（#13）**
17. i18n 框架选型 → ✅ **C6（#14）**
18. 前端状态管理 → ✅ **C8（#16）**
19. 仓库结构与构建编排 → ✅ **G1（#31）**（见 §19）
20. daemon 快照 → xterm.js 一致性（VT parity）→ ✅ **#42**：**达标，保持路线 C（不回退 Node sidecar）**。真机：内核快照 `snap.ansi` 经 xterm.js 重放，与内核视角**逐格 0 差异、alt-screen 与光标一致**；同一 fixture 下**哑管道基线（512 B 有界尾巴）30 行全错、样式 128 格错且卡在普通屏**——同时证明了「为什么必须有快照」与「快照是 faithful 的」。

---

## 19. 工程结构与构建 ✅（G1 #31）

**目录布局**：Tauri 默认布局 + Rust workspace

```
Ageminal/
├─ Cargo.toml            # [workspace] members = ["src-tauri", "crates/*"]
├─ package.json          # 根脚本 + 唯一版本来源
├─ src/                  # Vue 3 前端
├─ src-tauri/            # Tauri app crate（含 app-core）
│  └─ binaries/          # sidecar 构建产物（-$TARGET_TRIPLE.exe，gitignore）
├─ crates/
│  ├─ protocol/          # app-core ↔ daemon 的帧协议（Rust 单一来源）
│  ├─ daemon/            # ageminal-daemon.exe
│  └─ notify/            # ageminal-notify.exe（hook shim）
├─ scripts/              # build-sidecars.mjs 等
├─ docs/{research,adr}
└─ prototype/
```

- **三个二进制**：`ageminal-desktop`（Tauri 主程序）、`ageminal-daemon`（B1 #4）、`ageminal-notify`（E2 #23 的 hook shim，**独立 exe**，不为一次回调启动 GUI）。
- **两个类型边界**：
  1. **app-core ↔ daemon**（Rust ↔ Rust，Named Pipe）→ **`crates/protocol`**，**Rust 单一来源、不涉及 TS**；
  2. **app ↔ 前端**（Rust ↔ TS）→ **`tauri-specta` 钉 `=2.0.0-rc.25`**，生成物入版本控制并做无 diff 校验（备选 `ts-rs`）。
- **sidecar 打包**：`scripts/build-sidecars.mjs` → `cargo build --release -p ageminal-daemon -p ageminal-notify` → 复制为 `src-tauri/binaries/<name>-<target-triple>.exe`；`bundle.externalBin` 打包。
- **运行时落点**：daemon → `%LOCALAPPDATA%\Ageminal\daemon-host\<app 版本>\`（B4 #7，躲安装器清扫）；**shim → 稳定路径 `%LOCALAPPDATA%\Ageminal\bin\`**，使注入的 hook 配置**跨应用更新无需重写**。
- **版本**：单一来源 = 根 `package.json`（`tauri.conf.json` 用 `"version": "../package.json"`）；**app 版本与协议版本分离**。
- **构建编排**：根 `package.json` 的 `sidecars` → `tauri build`；`tauri.conf.json` 的 `beforeBuildCommand` 只负责前端构建。
- **前端目录**：`features/{terminal,tabs,files,agents,settings}/`、`stores/`、`i18n/`、`styles/`、`lib/`、`assets/fonts/`。

---

## 20. 测试策略 ✅（G3 #33）

| 层 | 工具 | 覆盖 |
| --- | --- | --- |
| Rust 单元 | `cargo test` | 协议编解码、事件→状态映射、迁移链、忽略封装、路径校验 |
| **Rust 集成（无 UI）** | `cargo test`，**daemon 以真实进程启动** | Named Pipe 握手、CreateSession、PTY 输出、ack/流控、崩溃清理 |
| 属性 / 快照 | `proptest` + `insta` | 协议帧**往返性质**、ANSI 快照、迁移链幂等 |
| 前端单元 / 组件 | `vitest` + `@vue/test-utils` + `happy-dom` | store、组件、**i18n key 完整性 + 中文字面量检查** |
| 前端集成 | WebdriverIO **browser mode** | 前端在 Chrome 跑、`invoke()` 被 mock |
| **e2e（真应用）** | WebdriverIO + `@wdio/tauri-service`（**embedded** provider，Windows 可用、无需外部 driver） | **仅冒烟**：启动 + 开终端页签 + 关窗 |
| **VT 一致性** | **#42 专用夹具** ✅ | daemon 快照 → xterm.js **逐格** parity（真机达标：0 差异；哑管道基线同 fixture 下 30 行全错） |

- **架构约束** ✅：**`crates/daemon` 不依赖 tauri**（才能脱离 UI 独立测试）；协议与纯逻辑**跨平台可测**；**PTY/ConPTY 测试标 `#[cfg(windows)]`**。
- **明确不进 e2e**（走真机 spike）：中文 IME（K #38）、毛玻璃（#39）、ConPTY 极端行为（#40）、通知点击（#48）。
- **夹具**统一 `tests/fixtures/`：迁移（F2 #29）、忽略规则 + `git check-ignore` 对照（D2 #18）、事件映射（E3 #24）、ANSI 快照（#42）。
- **纪律**：每个 bug 修复必须带回归测试；关键不变量用 `proptest`；**不设覆盖率数字门槛**。

---

## 21. CI 与发布 ✅（G4 #34）

| 工作流 | 触发 | 内容 |
| --- | --- | --- |
| `ci.yml` | PR / push | Rust `fmt --check` + `clippy -D warnings` + `cargo test`（跨平台单元 + `#[cfg(windows)]` 集成）；前端 `lint` + `tsc --noEmit` + `vitest run`；**四道校验**：i18n 中文字面量、i18n key 结构一致、`git check-ignore` 保真度、**specta 生成物无 diff**；迁移夹具 |
| `e2e.yml` | **main push + tag** | 构建 + WebdriverIO **冒烟**（启动 + 开终端页签 + 关窗） |
| `release.yml` | tag | `sidecars` → `tauri build` → 上传 NSIS 安装器到 GitHub Release + **SHA256SUMS** |

- **Runner**：`windows-latest`（含 MSVC / WebView2）；`Swatinem/rust-cache` 缓存 `target/`；`actions/setup-node` 缓存 pnpm store；开启 `core.longpaths`。
- **签名** ✅：**MVP 不签名**（README 明示 SmartScreen 会提示「未知发布者」）；**首个公开发布后申请 SignPath Foundation**（开源免费，条件为「已发布 + 活跃 + 只签自己」）。Azure Artifact Signing 对**个人开发者仅限美/加**，不适用。
- **必过检查**：`ci.yml` 设为 main 分支保护；fork PR 无 secrets → 签名/发布只在 tag 上。
- **不做**：macOS / Linux 构建（§2 纯 Windows）、updater 签名（§12 手动更新）、arm64 🕓（MVP 只 x64）。

---

## 22. 打包与分发 ✅（G7 #37）

- **安装器 = NSIS**，**per-user（`currentUser`）** → 装到 `%LOCALAPPDATA%\Programs\Ageminal`，**免 UAC**；**不写 PATH、不做开机自启**（安装器只做标准应用安装，E2 #23）。
- **WebView2 引导 = `embedBootstrapper`**（约 1.5 MB 嵌入安装器，缺失时联网下载 runtime）；完全离线的 `offlineInstaller` 🕓。
- **sidecar 与首次运行**：`externalBin` 随包 → 首次运行把 daemon 复制到 `daemon-host\<版本>\`、shim 复制到稳定 `bin\`（G1 #31）；安装器创建 **Start Menu 快捷方式**（也是通知 AUMID 的前提）。
- **卸载**：NSIS `installerHooks` **先优雅结束守护进程与会话**、**清理我们写入的 agent 全局集成**（**仅在内容仍带我们的标记时**移除），再删应用文件；**保留用户数据** `%LOCALAPPDATA%\Ageminal\`（文档说明）。
- **更新 = 手动**（§12 不接 updater）：只有 **stable 通道**，下载新 NSIS **覆盖安装**；**旧 daemon 继续跑**（`daemon-host\<版本>\` 搬迁正为此），新 app 启动按版本拉起新镜像（保留 2 版）；「关于」页显示版本，「检查更新」🕓。
- **产物**：NSIS 安装器 + **`SHA256SUMS`**（G4 #34）；**MVP 不签名**，发布后申请 SignPath Foundation。
- **「关于 / 许可」页**：字体 OFL（C3 #11）+ codicon CC-BY 署名（G5 #35）+ 依赖许可清单。

---

## 附录 A：调研结论（2026-09-19）

完整报告见 **`docs/research/ui-editor-route.md`**。要点：

**UI/编辑器路线（Tauri vs GPUI）**

- `gpui` 已发布 crates.io（`0.2.2`，Apache-2.0，可商用/闭源），Windows 后端由 Zed 正式版（DirectX 11 + DirectWrite）验证；但 pre-1.0、文档滞后、Windows 中文 IME 有未关闭的 S2 级 bug、无障碍全平台缺失，且社区生态项目死亡率高。
- **GPUI 与 Tauri 互斥**（Tauri 不支持嵌入任意原生视图，相关 feature request 已关闭未实现）。
- Zed 编辑器核心 `editor` / `language` / `rope` / `text` / `theme` 均为 **GPL-3.0-or-later**、未发布、强耦合 → **不可复用**；只读高亮的正解是 CodeMirror 6（Web）或 tree-sitter-highlight / syntect（原生）。
- GPUI 现成终端组件在 Windows 上要么不支持（`gpui-libghostty` 仅 macOS/Linux）、要么 alpha 缺滚动/选择（`gpui-terminal`）。
- **结论**：主路线保留 **Tauri + xterm.js + CodeMirror 6**；GPUI 转为延后分支验证。

**会话持久化：tmux vs 自研守护进程**

- Git for Windows 不自带 tmux；在 Git Bash 使用 tmux 需 MSYS2 + 拷贝 `tmux.exe` 与 libevent DLL，随 Git 升级失效 → 不作为产品依赖。
- 采用方向：**ConPTY（Win10 1809+）+ 常驻守护进程 + Named Pipe**；先例 Contour Terminal daemon 模式、`wmux`（Electron/TS，架构先例；**Rust 生态无可复用同名项目**）。

**Agent 事件机制**

- Claude Code：hooks 事件含 `SessionStart` / `Stop` / `StopFailure` / `Notification`（`agent_needs_input`、`agent_completed`、`permission_prompt`、`idle_prompt`）/ `PermissionRequest`；handler 支持 `command` 与 **HTTP**
- Codex：`notify`（`agent-turn-complete`，可按会话 `codex -c` 覆盖）；**Windows hooks 已启用**（v0.120+），但 `PreToolUse` 有缺陷（#24453）
- opencode：插件事件 `session.idle` / `session.status` / `permission.asked` / `permission.replied`
- pi（`@earendil-works/pi-coding-agent`）：`--mode json` 事件流、stdio RPC、TypeScript 扩展/SDK
- DeepSeek Harness（`dsh`）：官方 `deepseek-ai/deepseek-harness`，TypeScript SDK / stdio JSON-RPC

---

## 附：现有原型

- 布局原型文件：`prototype/index.html`（单文件自包含，双击浏览器打开）
- 主题：Zed「One Dark Pro Glass」；左下角「原型导航」可在 **11 个屏幕**间跳转
- 交互为模拟实现；窗口材质见 §11（**默认实色**，可开启 Acrylic；C7 #15 的「按 build 自动选 Mica」已被 #39 取代）

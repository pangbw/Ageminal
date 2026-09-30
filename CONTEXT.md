# CONTEXT.md — Ageminal 领域词汇

> 本文件是**术语的单一事实来源**：把项目里反复出现、又最容易混的词钉死。
> 架构与行为约束见 `REQUIREMENTS.md`；难以回退的决策见 `docs/adr/`（待补，见 §8）。
> **维护约定**：术语先改这里，再改代码与其它文档。

## 0. 三条贯穿全局的约定

1. **单一事实来源 = Rust 侧持久化**。前端 store 只是**内存投影**；修改走「命令 → Rust 落盘 → 回推事件」，**MVP 不做乐观更新**。
2. **高频数据不进状态库**：PTY 字节流、xterm 实例池、CodeMirror `EditorView` 都不进 Pinia。
3. **绝不从「列表里没有」推断进程已死**（启动对账走四象限，见 §2）。

---

## 1. 工作区与仓库

| 术语 | 定义 | 别混用 |
| --- | --- | --- |
| **项目（Project）** | 用户加入 Ageminal 的一个 git 仓库。 | ≠ worktree |
| **worktree（工作树）** | `git worktree` 语义下的一个工作树（含主工作树）。左侧栏按项目分组列出。 | ≠ 项目；≠ 页签 |
| **主工作树** | `git worktree list` 的第一条。**bare 仓库单独标注**。 | — |
| **locked / prunable / detached** | worktree 的标注状态；**只标注，绝不自动 prune**。 | — |

## 2. 会话与进程

| 术语 | 定义 |
| --- | --- |
| **会话（Session）** | 一个终端页签背后的**常驻 PTY 会话**，由守护进程持有，生命周期可长于 UI。 |
| **`sid`** | 会话的**持久逻辑 id**（字符串，含 worktree 归属）。跨 UI 重启不变。 |
| **`incarnationId`** | **每次 spawn** 生成的世代 id；用于丢弃迟到的 exit 事件。 |
| **启动规格（launch spec）** | `sid` / `incarnationId` / cwd / shell / 启动命令 / `agentSessionId` / 页签骨架。**只存在 app 的 `state.json`**——daemon 死了也要能重建。 |
| **页签（Tab）** | UI 概念：会话在界面上的呈现。**关页签 ≠ 会话终止**。 |
| **面板（Pane）** | 页签内承载终端的稳定容器；终端实例的宿主 `<div>` 被 re-parent 进来。 |
| **守护进程（daemon）** | 常驻独立进程，持有所有 ConPTY 与**服务端 VT 内核**。 |
| **daemon-host** | 版本化镜像目录 `%LOCALAPPDATA%\Ageminal\daemon-host\<version>\`，活过 NSIS 更新（保留 2 版）。 |

### 2.1 attach / detach / terminate（★ 极易混）

| 词 | 含义 | 关键语义 |
| --- | --- | --- |
| **attach** | UI 连上已有会话 | 先回放**快照**，再续实时流 |
| **detach** | UI 断开，**会话继续跑** | **Detach ≠ Terminate**；关窗、退出 UI 都是 detach |
| **terminate** | 真正结束会话 | `ClosePseudoConsole` → 整个进程树终止；唯一入口是显式结束 / 托盘「结束所有会话并退出」 |

### 2.2 恢复 vs 重建（★ 本项目里「会话恢复」专指 agent 对话）

| 词 | 适用 | 做了什么 |
| --- | --- | --- |
| **恢复（resume）** | **agent 页签** | 用 agent CLI 自身的 resume 恢复**对话**（`claude --resume` / `codex resume` / `opencode -s` / `pi --session`）。**恢复时必须重传首次启动的注入参数**，否则「恢复了但事件不再回调」。 |
| **重建（rebuild）** | **纯 shell 页签** | 进程已死（daemon 崩溃 / OS 重启）→ 只能重建骨架，**不是恢复** |

> ⚠️ 「会话恢复」**不是**「把死掉的终端屏幕恢复出来」。终端屏幕的重建走 §3 的快照。

## 3. 终端数据路径

| 术语 | 定义 |
| --- | --- |
| **快照 / checkpoint** | 服务端 VT 内核产出的 **ANSI 快照**，用于 attach 时重建屏幕（路线 C）。 |
| **回放（replay）** | 环形缓冲里按**单调偏移**把「快照之后的字节」补上；与实时流去重衔接。 |
| **scrollback** | 可滚动历史；10k 行。 |
| **流控** | VS Code 式 **high/low 水位 + 累积 ack**（ack 来自 xterm.js 的 write 回调），档位 256/64/32 KiB。 |
| **合批（batching）** | Tauri `Channel` 的 `Raw` 帧 **< 1024 字节会退化成 JSON 数组**，故 **≥16 KiB 或 ≥8 ms** 才 flush。 |
| **隐藏会话不推流** | 不可见的页签不接收数据；切回时用**快照 attach**。 |
| **PTY 层** | 藏在 `Pty` trait 之后（可替换实现）；关闭 HPCON 即终止该会话的进程树。 |
| **服务端 VT 内核** | 守护进程侧（`alacritty_terminal`）：既产出快照，也保证**有界回放**不会重建出错——**它是必需项，不是优化项**（#42）。 |

## 4. Agent、事件与集成

| 术语 | 定义 |
| --- | --- |
| **注入（injection）** | 把**首次启动参数**传给 agent，使它能回调我们。 |
| **集成（integration）** | 往 agent 的配置里写东西（插件 / hook 文件），**用户须在设置页手动启用**；配 diff 预览 + 自动备份 + 停用即回滚。 |
| **集成状态** | `未启用` / `已启用` / `集成有更新` / `部分应用（X/Y 失败）` / `不受支持` |
| **统一状态** | 由 agent 事件 + 进程事实归并出的会话状态：`running` / `idle` / `waiting` / `permission` / `done` / `error` / `exited` |
| **降级模式** | 只拿到进程事实（无事件）时，只暴露 `running` / `idle` / `exited`，并显式标注「不完整」 |
| **会话树识别** | 通过 `sysinfo` 枚举会话进程树 + 识别名单 + 输出静默；**不解析输出**、不读窗口标题。 |

## 5. 外观与材质

| 术语 | 定义 |
| --- | --- |
| **默认实色** | 窗口与工作区**不透明**；正文对比度**不依赖** OS 材质与用户壁纸（#39）。 |
| **Acrylic（可选）** | 设置页开关，**需重启生效**（`transparent` 是窗口**创建期**属性）。 |
| **材质（Mica / Acrylic）** | OS 提供的真实背景模糊。**Mica 弃用**（可见性取决于壁纸）、**Blur 弃用**（22621+ 性能差）。 |
| **`backdrop-filter`** | **不得**用于「桌面模糊」——WebView2 采不到桌面像素（WebView2Feedback #4945）；只可用于应用内叠层。 |
| **同一 `EditorView`** | 预览页签共享一个 CodeMirror `EditorView`，每个页签缓存自己的 `EditorState`（LRU 12），**不做 DOM re-parent**。 |

## 6. 文件与可见性

| 术语 | 定义 |
| --- | --- |
| **三个可见性维度** | ① `.git\` 永远隐藏 ② dotfile ③ 被忽略（`.gitignore` / `.git/info/exclude` / 全局 excludes）。**Rust 返回标志位，前端过滤**。 |
| **「显示被忽略」** | 不改变遍历，只是不再忽略并**变暗显示**。 |
| **预览页签** | 文件**只读**预览（CodeMirror 6）；编辑走外部编辑器。 |
| **无效 UTF-8** | **lossy 解码 + 警告**（不必当二进制、也不报错）。 |
| **回收站删除** | 走 `trash`；不可用时**显式报错，绝不静默永久删除**。 |

## 7. 不翻译的术语（i18n 约定）

`worktree` · `agent` · `daemon` · `commit` · `checkpoint` · `scrollback` · `pane` · `Acrylic` · `Mica` · `VT` · `ConPTY` · `SGR` · `alt-screen` · `pipe` · `attach` / `detach` · `token`

> 其余 UI 文案以 **zh-CN 为类型来源**；键结构须与其它语言一致（CI 两道检查：CJK 字面量 + 键结构 parity）。
> 英文化的落笔（不译词补集 + 中英对照）见 `docs/i18n-glossary.md`。

## 8. 待补

- **`docs/adr/`**：把最难回退的决策固化为 ADR——自研 daemon、路线 C 的服务端 VT 内核、默认实色、JSON 而非 SQLite、git CLI 而非 `gix`、Apache-2.0 等。当前决策全集见 `REQUIREMENTS.md` 与 issue #1 的 `Decisions so far`。

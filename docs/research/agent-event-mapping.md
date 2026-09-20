# 调研：Agent 原生事件 → Ageminal 统一状态机映射

> **状态回写（2026-09-20）**：**已定 —— E3（#24）**。采纳本文的逐事件映射表；要点：`idle_prompt` → `idle`；**`done` 不自动转 `idle`**；**非零退出 → `exited` + 退出码**；降级模式只暴露 运行/空闲/退出 并显式标注「不完整」；合并规则按「版本化状态 + 逻辑序优先 + 优先级 `exited>error>permission>waiting>running` + 幂等 + 200–500 ms 去抖」；**子 agent 不进 UI**。
>
> 未闭合（实现期验收）：各 agent 在**纯 Windows** 上的事件触发情况（本文末尾「Windows 原生支持验证」原样保留）。

> 调研日期：**2026-09-19**
> 背景：对应 issue #24「Agent 事件 → 统一状态机映射」（REQUIREMENTS §18.7 / §10.3–§10.5）。目标是为首批五个 agent（Claude Code、Codex、opencode、pi、DeepSeek Harness `dsh`）建立**逐事件**到统一状态机的映射，并给出无原生事件时的降级策略。
> 记号：**确定事实**来自官方文档 / 官方源码 / GitHub issue 原文；**⚠️推测**为无直接来源的推断；**[建议]** 为本文给出的设计提案（非既定事实）。
> 版本基线：各 agent 均在快速迭代，本文标注了调研时的版本门槛；事件名以正文来源为准。

---

## 一、结论（TL;DR）

1. **五个 agent 的原生事件成熟度差异很大，Claude Code 最完整，dsh 与 pi 最需要自建集成。**
   Claude Code 有覆盖会话/回合/授权/通知的完整 hooks；opencode 有事件总线（`session.status` / `permission.asked` / `question.asked` / `session.error`）；Codex 既有一个「仅完成」的 `notify`，也有 v0.120 起在 Windows 也可用的完整 hooks；pi 与 dsh 则要靠**扩展 / SDK 协议**，其中 dsh 当前**没有授权（permission）事件**、pi **没有内建授权弹窗**。

2. **REQUIREMENTS §10.4 关于「Codex hooks 在 Windows 被禁用」的结论已过时。**
   Codex 在 `0.118` 曾被临时禁用 Windows hooks（PR #15252，2026-03-20），随后在 **v0.120.0（2026-04-11 发布）** 由 PR #17268「remove windows gate that disables hooks」恢复；官方 hooks 文档现已给出 `commandWindows` / `windows_managed_dir` 等 Windows 配置项。**结论：Codex ≥ 0.120.0 在 Windows 上 hooks 可用**，`notify` 不再是唯一接入点。

3. **`waiting` 与 `permission` 的区分有统一原则：授权有专门的授权事件（Claude `PermissionRequest`、Codex `PermissionRequest`、opencode `permission.asked`），其余「等用户」用提问/输入事件（Claude `agent_needs_input` / `elicitation_*`、opencode `question.asked`、pi `ui_prompt_start`）。**
   Claude Code 的 `Notification(permission_prompt)` 是**延迟约 6 秒**的兜底信号，`PermissionRequest` 才是即时的授权信号，且沙箱网络请求**只**发 `permission_prompt`。

4. **`done` 与 `idle` 需要显式区分，不能由同一事件兼任。**
   建议 `done` = 刚结束一个回合、等待下一条输入；`idle` = `done` 后静默超过阈值仍无新输入（或会话启动后从未活动）。Claude 的 `idle_prompt`（完成约 60 秒后触发）语义更接近 `idle` 而非 `waiting`，与 REQUIREMENTS §10.4 把它列进「等待输入/授权」存在冲突（见 §七）。

5. **纯降级（进程检测 + 空闲启发式）无法可靠推断 `waiting` / `permission` / `done` / `error`，只能给出 `running` / `idle` / `exited` 三类。**
   静默既可能是「等输入」，也可能是长工具调用、长构建或模型在等 API；CPU 低不等于完成（网络/流式等待就是低 CPU）。降级应显式降级状态集合并告知用户，而不是猜测。

6. **跨 agent 仍有两个硬缺口：**
   - **Claude Code 没有「用户中断」事件**（`Stop` 在用户按 Esc 时不触发），事件驱动下可能停留在 `running`，需要进程/超时兜底。
   - **dsh 当前协议没有授权事件，也没有「已退出」通知**；`pi` 的授权只能由第三方扩展定义。这两者的 `permission` 状态在首版可能不可得。

---

## 二、统一状态机定义（供映射参照）

REQUIREMENTS §10.3 的状态集合：`running` → `waiting` / `permission` → `done` / `idle` / `error` / `exited`。下表是本文采用的**语义定义**（用于消歧，非新增需求）：

| 状态 | 语义 | 判定依据 |
|---|---|---|
| `running` 运行中 | 进程存在，且 agent 正在推进（模型调用 / 工具执行 / 流式输出 / 重试中） | 原生「开始/活动」事件；或进程存在 + 最近有输出 |
| `waiting` 等待输入 | agent 主动停下、等用户**非授权类**输入（`AskUserQuestion`、MCP elicitation、扩展对话框、下一条 prompt） | 提问 / elicitation / UI 弹窗事件 |
| `permission` 需要授权 | agent 卡在一个**工具或操作授权**弹窗上 | 授权请求事件 |
| `done` 完成 | 一个回合正常结束，控制权交还用户 | 回合结束 / 停止事件 |
| `idle` 空闲 | `done` 后静默超过阈值仍无输入；或会话已建立但从未跑过回合 | `idle_prompt` / 长时间无事件 + 无输出 |
| `error` 出错 | 回合/会话因错误终止（API 错误、模型错误、工具致命错误、崩溃恢复） | 错误事件 / 回合错误结束原因 |
| `exited` 已退出 | 会话背后的进程结束（正常退出或消失） | 进程退出 / 会话结束事件 / 传输关闭 |

**[建议] 转换规则**：`running` 是活动态；收到授权→`permission`，收到提问/等输入→`waiting`，回合结束→`done`，`done` 超时→`idle`，新一轮 prompt/活动→回到 `running`，错误→`error`，进程消失→`exited`。

---

## 三、逐 agent 映射表

### 3.1 Claude Code

**确定事实**：hooks 事件与 matcher 以官方 hooks reference 为准。handler 支持 `command` 与 `http`（HTTP hook 的输入作为 POST body，输出格式同 command），符合 §10.5 的「HTTP hook」设想。以下事件门槛来自文档：`agent_needs_input` / `agent_completed` 需 v2.1.198+；`fork` source 需 v2.1.214+；沙箱网络请求的 `permission_prompt` 需 v2.1.246+（终端）。

| 原生事件 | 统一状态 | 触发条件 / 关键说明 |
|---|---|---|
| `SessionStart`（`source` = `startup`/`resume`/`clear`/`compact`/`fork`） | `running`（新会话） / `idle`（恢复后待输入） | 会话开始或恢复。无独立「回合」概念时先置 `running`，若恢复后等 prompt 可置 `idle` |
| `UserPromptSubmit` | `running` | 用户提交 prompt、回合开始。最可靠的「开始运行」信号 |
| `PreToolUse` / `PostToolUse` / `PostToolUseFailure` / `PostToolBatch` | `running`（活动信号） | 每工具调用触发，频率高，一般只用于刷新「有活动」时间戳 |
| `PermissionRequest` | **`permission`** | **即时**授权信号；matcher 为工具名；MCP 工具也可匹配 |
| `Notification(notification_type=permission_prompt)` | `permission` | 授权提示**等待约 6 秒**后的信号；沙箱命令的**网络请求只能靠此**（不发 `PermissionRequest`） |
| `Notification(agent_needs_input)` | `waiting` | 后台会话等待输入（agent view 打开时），或 teammate 终端设置问题约 6 秒 |
| `Notification(elicitation_dialog / elicitation_url_dialog)` | `waiting` | MCP elicitation 表单/URL 请求，约 6 秒未输入 |
| `Notification(idle_prompt)` | **`idle`**（⚠️ 与需求表存在冲突） | 「完成响应约 60 秒且用户未输入」，本质是空闲而非需要输入 |
| `Notification(agent_completed)` | `done` | 后台会话**完成或失败**；仅 agent view 打开时触发。⚠️ 失败也发此事件，需结合下文失败信息 |
| `Stop` | `done` | 主 agent 完成响应。**用户中断不触发**；API 错误改发 `StopFailure`。payload 含 `last_assistant_message`、`background_tasks[]`、`session_crons[]`（可区分「真完成」与「等后台任务唤醒」） |
| `StopFailure` | `error` | 回合因 API 错误结束；`error` 可取 `rate_limit`/`overloaded`/`authentication_failed`/`billing_error`/`server_error`/`max_output_tokens`/`unknown` 等；无决策能力，仅通知/记录 |
| `SessionEnd`（`reason` = `clear`/`resume`/`logout`/`prompt_input_exit`/`other`） | `exited` | 会话终止。**注意**：`SessionEnd` 也用于 `/clear` 和 `/resume` 切换，未必等于进程退出 |
| （无事件）用户按 Esc 中断 | ⚠️ **无法由事件捕获** | `Stop` 不触发；需要进程/输出启发式或超时兜底 |

**`waiting` vs `permission` 的判定（确定事实）**：
- **`permission`**：`PermissionRequest`（即时、首选）或 `Notification(permission_prompt)`（延迟 6 秒的兜底）。
- **`waiting`**：`Notification(agent_needs_input)`、`Notification(elicitation_dialog)`、`Notification(elicitation_url_dialog)`。
- **`idle`**：`Notification(idle_prompt)`。
- `idle_prompt` 与「需要输入」是两回事：第三方先例（tmux-claude-status 的 hook contract）明确把 `idle_prompt` 排除在「blocked on user」之外，否则每个刚完成的窗口都会被标记为「等待」。⚠️ 该先例为第三方文档，仅作参考。

### 3.2 Codex

**确定事实（版本基线 v0.120.0，2026-04-11）**：存在两套机制。

**(a) 传统 `notify`（仅完成）** — 配置在 `~/.codex/config.toml`（或 `codex -c 'notify=[...]'` 的一次性 runtime 层）。Codex 在每个回合结束（`AfterAgent`）时启动该命令，并把 JSON 作为**最后一个 argv 参数**传入（不是 stdin）。形状（源码 `codex-rs/hooks/src/legacy_notify.rs` 测试断言）：

```json
{
  "type": "agent-turn-complete",
  "thread-id": "…", "turn-id": "12345", "cwd": "/path",
  "client": "codex-tui",
  "input-messages": ["…"],
  "last-assistant-message": "…"
}
```

| `notify` 字段 | 价值 |
|---|---|
| `type` | 当前恒为 `agent-turn-complete`；解析时仍应校验以防未来新增类型 |
| `thread-id` / `turn-id` | 稳定标识，用于会话关联与去重 |
| `cwd` | 定位 worktree（§10.5 的环境变量之外的第二关联方式） |
| `client` | 交互 TUI 为 `codex-tui`；可能省略，需防御性读取 |
| `last-assistant-message` | 通知正文；可能缺失或很长，需截断 |

**限制**：`notify` 在项目级 `.codex/config.toml` 被**denylist**（与 `model_provider`、`profile` 等同列），只能在 user/system/managed/runtime 层生效；它是兼容层（`legacy_notify`），且**只有这一个事件**——没有等待、授权、错误事件。

**(b) 生命周期 hooks（v0.120 起含 Windows）** — `hooks.json` 或 `config.toml` 内联 `[hooks]`，支持 `command` 与 `mcp_tool` handler。

| 原生事件 | 统一状态 | 说明 |
|---|---|---|
| `SessionStart`（source `startup`/`resume`/`clear`/`compact`） | `running` / `idle` | 会话开始或恢复 |
| `UserPromptSubmit` | `running` | 提交 prompt |
| `PermissionRequest` | **`permission`** | 即将请求授权（shell 提权、受管网络等）；matcher 为工具名（`Bash`/`apply_patch`/MCP）；不在无需授权时触发 |
| `PreToolUse` / `PostToolUse` | `running`（活动） | 可选，频率高 |
| `Stop` | `done` | 回合正常结束；payload 含 `last_assistant_message`、`stop_hook_active`；matcher 不支持 |
| `Interrupt` | `waiting`（或 `idle`） | 用户中断活动回合；1 秒超时；不阻塞、不能重启回合 |
| `SessionEnd`（`reason` 当前恒为 `other`） | `exited` | 主线程结束 |
| `SubagentStart` / `SubagentStop` | （子代理，可选跟踪） | 带 `agent_id` / `agent_type` |
| `notify` = `agent-turn-complete` | `done` | 传统机制；每回合一次 |

**降级（只有 `notify` 时）**：只能确定 `done`，其余状态退化为进程/空闲启发式（§四）。**Windows 注意**：hooks 需要交互式 `/hooks` 审查并信任（或 `--dangerously-bypass-hook-trust`），项目级 hooks 仅在项目被信任时加载。

**额外可用信号**：`tui.notifications` 开启后，Codex 在「完成 prompt」或「请求工具授权」时向终端发送通知（历史上为 OSC 9，后续支持 BEL；`tui.notification_method` = `auto`/`osc9`/`bel`）。⚠️ 该行为来自 Codex issue #3962 的维护者回复，配置参考页确认 `tui.notifications` 存在；可作为降级期「可能有授权弹窗」的线索。

### 3.3 opencode

**确定事实**：opencode 的架构是 TUI（客户端）↔ 本地 HTTP server（`opencode serve`），server 提供 SSE 事件流（`GET /event`）。插件通过 `event` 订阅同一事件总线；外部集成也可直接消费 SSE。**版本注意**：opencode v2 正处于 beta，server API 与 SDK 契约仍在变动（官方 migrate 文档明说）。

| 事件（`event.type`） | 统一状态 | 说明 |
|---|---|---|
| `session.status` → `{type:"busy"}` | `running` | 会话正在工作 |
| `session.status` → `{type:"retry", attempt, message, next}` | `running`（附「重试中/正在恢复」） | 自动重试（如临时 API 流错误）；重试最终失败可能落到 `session.error` |
| `session.status` → `{type:"idle"}` | `done` / `idle` | 回合结束；官方建议用 `session.status` 而非已废弃的 `session.idle` |
| `session.idle` | `done` / `idle`（**deprecated**，兼容保留） | 等价于 status 转 idle |
| `permission.asked` | **`permission`** | 工具/操作需要用户批准 |
| `permission.replied` | `running` 或 `waiting` | 用户已回答授权；按 reply 取值恢复 |
| `question.asked` | **`waiting`** | question 工具向用户提问（插件文档未列，但 v2 类型中存在） |
| `question.replied` / `question.rejected` | `running` | 用户已回答/拒绝 |
| `session.error` | `error` | 会话级错误 |
| `session.compacted` | `running`（保持当前） | 压缩完成，不改变状态 |
| `message.*` / `file.*` / `todo.*` | `running`（活动信号） | 频率高，用于刷新活动时间 |

**关键陷阱（确定事实，来自官方 issue）**：子代理以**子会话**运行，`session.status` 事件**不带 `parentID`**（只有 `session.created`/`session.updated` 带），根会话在子代理运行期间可能一直只报 `busy`。因此：
- 子代理的 `permission.asked` / `question.asked` / `session.error` 会被外部消费者误当成根会话状态（issue #30043、#46685 记录了这个误报，herdr 插件因此把正在正常运行的子代理渲染成红色 `blocked`）。
- **[建议]** Ageminal 必须维护 `session.created` 的父子谱系，只允许根会话的 `permission/question/error` 改变根会话状态；子代理状态单独展示或忽略。

**类型注意**：`@opencode-ai/plugin` 的 SDK 类型曾长期滞后于 runtime（issue #7147 等），插件里常需按 v2 `Event` 类型做窄化/断言。`permission.replied` 的载荷在不同版本间有差异（新系统为 `{sessionID, requestID, action, always?}`，部分文档写 `{sessionID, requestID, reply: "once"|"always"|"reject"}`）——⚠️ 需以目标版本实测为准。

### 3.4 pi（`@earendil-works/pi-coding-agent`）

**确定事实**：pi 有四种模式：interactive（TUI）、print/`-p`、`--mode json`（一次性事件流，输出后退出）、`--mode rpc`（stdin/stdout JSONL 长连协议），另有可嵌入 Node 的 SDK（`AgentSession`）。**关键区别**：`--mode json` 与 `--mode rpc` 是**交互式 TUI 的替代**，不是旁路；要在保留 pi TUI 的同时拿到原生事件，正确路径是**第三方 TypeScript 扩展**（`pi.on(...)`），扩展可调用外部程序（如 `ageminal notify`）。

pi 的扩展生命周期事件（用于映射）：

| 扩展事件 | 统一状态 | 说明 |
|---|---|---|
| `session_start` | `running` / `idle` | 会话开始/加载/重载 |
| `before_agent_start` / `agent_start` | `running` | 开始处理 prompt |
| `turn_start` / `message_*` / `tool_execution_*` / `tool_call` | `running`（活动） | 细粒度生命周期 |
| `ui_prompt_start`（`kind`: `select`/`confirm`/`input`/`editor`/`custom`） | **`waiting`**，若为授权 gate 则 `permission` | **专为状态集成设计**：围绕阻塞式用户交互触发，官方说明用于让 host/status 集成「报告 waiting for user 而不是 running」 |
| `ui_prompt_end` | `running` | 弹窗关闭 |
| `agent_end` | `running`（**尚未 settled**） | 之后可能自动重试、压缩重试或处理排队消息 |
| `agent_settled` | **`done` / `idle`** | 官方推荐：状态集成应以 `agent_settled` 判断 pi 不会再自动继续。**这是最可靠的完成信号** |
| `auto_retry_start` | `running`（重试中） | 临时错误自动重试 |
| `auto_retry_end`（`success:false`, `finalError`） | `error` | 最终重试失败 |
| `summarization_retry_*` | `running` | 摘要重试 |
| `session_compact_failed`（非 abort） | `error`（可恢复）/警示 | 压缩失败 |
| `extension_error` | `error` | 扩展抛错 |
| `session_shutdown`（`reason: quit`） | `exited` | 会话运行时拆除；`quit` 为真正退出 |
| `queue_update` | `running` / `waiting` | 有排队/引导消息 |

**RPC/JSON 事件流**：与上表同源，事件类型包括 `agent_start` / `agent_end` / `agent_settled` / `turn_start` / `turn_end` / `tool_execution_*` / `auto_retry_*` / `extension_error` 等。`--mode rpc` 额外支持阻塞式扩展 UI 子协议：`extension_ui_request`（`select`/`confirm`/`input`/`editor`）等价于上面的 `ui_prompt_start`。

**缺口（确定事实）**：
- pi **没有内建授权弹窗**（官方明确：built-in tools 以用户权限直接读写/执行，「No permission popups」）；`permission` 状态只能由第三方扩展的 `ctx.ui.confirm()` 产生。
- `ui_prompt_start` 只告诉你「有弹窗」，**不区分是不是授权**；需要扩展在事件里携带自定义标记，或 Ageminal 只把自己安装的授权扩展识别为 `permission`，其余归 `waiting`。
- 项目信任（project trust）在交互式启动时是一次 `select`：它也是一种「等待用户」，但发生在会话开始、且非交互模式不提示。

### 3.5 DeepSeek Harness（`dsh`）

**确定事实**：官方 TypeScript SDK `@deepseek-ai/dsh-sdk-client`（设计孪生为 Python SDK）通过 **stdio 换行分隔 JSON-RPC 2.0** 驱动一个运行时的 `dsh-jsonrpc-agent` 子进程。协议方法：请求 `initialize` / `session/prompt` / `shutdown`；**服务端通知**只有四种：`session.event`（每个 `SessionEvent`，全量、未过滤）、`session.status`（`idle` | `running`，整 agent 生命周期）、`subagent.started`、`subagent.finished`。协议版本 `serverInfo.version = 0.0.1`，预发布、无兼容承诺。

| 原生事件 | 统一状态 | 说明 |
|---|---|---|
| `session.status` → `running` | `running` | 整 agent 驱动区间（可跨多个连续回合） |
| `session.status` → `idle` | `done` / `idle` | 无 driver 剩余 |
| `session.event`: `turn/start` | `running` | 回合开始 |
| `session.event`: `turn/end` `reason.completed` | **`done`** | 干净结束 |
| `turn/end` `reason.max-tokens` | `done`（**截断**，[建议] 附警示） | 任一步触顶即整回合 `max-tokens`；可能伴随「工具调用被丢弃、空输出」的静默失败 |
| `turn/end` `reason.error {error: LlmFailure}` | **`error`** | 结构化失败事实 |
| `turn/end` `reason.aborted {reason}` | `waiting` / `idle` | 用户/插件取消 |
| `turn/end` `reason.blocked` | `waiting`（⚠️ 语义为「pre-step 拒绝、未进入 step」） | 由 `agent/pre-step` 拒绝产生 |
| `turn/end` `reason.interrupted` | `error` / `exited` | **仅崩溃恢复合成**，非正常运行产生 |
| `step/start`、`step/end`、`tool/call`、`tool/result`、`assistant/chunk`、`assistant/message`、`user/message` | `running`（细粒度活动） | 都在 `session.event` 流中 |
| `subagent.started` / `subagent.finished` | （子代理，可选跟踪） | 带 lineage，可构建父子树 |
| `TransportClosedError` / 子进程退出 | **`exited`** | **没有专门的退出通知**，由传输关闭（错误消息携带 exit code + stderr 尾部）或子进程 wait 推断 |

**缺口（确定事实）**：
- **没有授权事件**。协议文档明说：client→server 通知与 server→client 请求「**两端都未实现**」，传输保留它们「为未来的 approval 流程」。因此 `permission` 在 dsh 当前不可得。
- **没有取消方法**：放弃一个回合只能关闭 runtime 进程（`close()` 是终结性的）。
- 无协议版本协商；`session/prompt` 只返回入队回执（`messageId`），回合结果只能从事件流自行拼装。
- 集成形态问题：SDK 驱动的是**独立 runtime 子进程**，不是用户交互式 TUI；若走 SDK，Ageminal 展示的会话与用户在终端里跑的 dsh 不是同一个。⚠️ 需确认 dsh 是否有可注入的 Cordis 插件/事件旁路，这是与 pi 类似的「注入方式」研究项（§10.5）。

---

## 四、降级信号（无原生事件 / 手动启动）

适用场景：手动入口（§10.2）、agent 未安装集成、或某 agent 缺少所需事件。原则仍是 §10.5：「不解析具体输出内容」，只做粗粒度推断。

| 信号 | 可推断 | 可靠性 / 误报风险 |
|---|---|---|
| **进程存在性**（守护进程持有 PTY，子进程 wait/退出码） | `exited`（确定）；退出码非 0 可附 `error` 语义 | 强。「进程消失 → exited」是硬事实；但**退出码非 0 到底算 `error` 还是 `exited` 需拍板**（§七） |
| **输出活动**（最近一次 PTY 输出时间） | 活跃 → `running`；静默 → 候选 `waiting`/`done`/`idle` | 中。**静默无法区分**等输入、长工具调用、长构建、模型在等 API；反过来流式输出也不代表「正在跑工具」 |
| **终端标题 OSC 0/2** | 部分 agent 会在标题带任务名/状态 | 弱。无标准（§7.2 支持动态标题，但语义由 agent 自定）；⚠️ 不建议依赖 |
| **CPU（进程树）** | 高 → `running`；低 → 不确定 | 弱。网络等待/流式响应是低 CPU；`sleep`/IO 等待可能高 CPU 空转 |
| **组合启发式** | alive + 最近有输出 → `running`；alive + 静默 ∈ [t1,t2) → `waiting`（或 `done`）；alive + 静默 ≥ t2 → `idle`；gone → `exited` | **[建议]** 仅此而已。`permission` / `done`（精确）/ `error` 无法可靠推断 |

**误报清单（需在 UI/文档中明示）**：
- 长命令/长构建/下载/测试（分钟级无终端输出）→ 误判为 `waiting` 或 `done`。
- 模型推理、等待 API、速率限制退避 → 无输出，误判空闲。
- `waiting` 与 `permission` 无法由静默区分。
- `done` 与 `waiting` 无法区分（两者本质都是「在等用户」）。
- 输出是行缓冲/全屏 TUI 重绘，刷新节奏与 agent 状态不同步。

**建议**：降级模式只暴露 `running` / `idle` / `exited` 三态（外加可选 `error`），并在 worktree 行/页签上标注「未启用原生集成，状态不完整」，而不是用 `waiting`/`permission`/`done` 伪装精确。

---

## 五、事件的优先级与合并规则（[建议]，尚无既定规范）

以下为设计提案，供 #24 讨论；实现前需与守护进程/事件通道设计（§5、§18.3）对齐。

1. **版本化状态**：每个会话保存 `state + source + timestamp + seq`。更新遵循「**逻辑序优先、时间序兜底**」：同一 turn/step 内用 agent 的序号（dsh `seq`、Codex `turn-id`、pi `turn`）；跨来源用事件时间戳单调不回退。
2. **冲突优先级（同一时刻多事件）**：
   - 进程消失 `exited` 最高（硬事实）；若 `SessionEnd` 先到而进程后消失，保持 `exited`。
   - agent 明确 `error` > `running` / `done`。
   - `permission` > `waiting` > `running`（`permission` 是更具体的 blocked-on-user）。
   - 新一轮任何活动事件（prompt/turn_start/tool_*）覆盖旧的 `done`/`idle`/`waiting` → `running`。
   - `done`/`idle` 低于 `running`（避免迟到的回合结束事件把新一轮运行打回完成）。
3. **去重 / 幂等**：优先用 agent 提供的稳定标识——Claude `session_id` + `hook_event_name` + 时间戳；Codex `thread-id` + `turn-id`；opencode v2 事件自带 `id`/`created`；pi `sessionId` + 事件序号；dsh `seq`。重复事件不改变状态；对同一 key 做短期防抖（如 200–500ms）。
4. **乱序 / 迟到**：turn 生命周期事件按 `turn`/`step` 单调；收到 `turn/end` 后再到的旧 `turn/start`（序号更小）直接丢弃。无序号 hook 事件按到达时间处理，但用第 2 条的「不回退」规则防护。
5. **中断 / 用户取消**：Codex `Interrupt`、dsh `turn/end.aborted`、pi `abort` 有明确信号；**Claude Code 无中断事件** → 只能靠输出静默 + 进程存活做超时兜底（⚠️ 已知缺口）。
6. **会话异常退出（进程消失）**：→ `exited`；若此前收到 `StopFailure` 等错误，保留最近错误原因（`error` → `exited` 的状态沿革）。守护进程崩溃/重启：所有会话标记 `exited`（进程无法复活，§5.4），UI 重连时按持久化骨架重建。
7. **子代理归属**：根会话状态**只由根事件**决定。子代理事件需按谱系过滤（Claude `agent_id`/`agent_type`；dsh `subagent.started/finished` 构建父子树；opencode 需用 `session.created` 的 `parentID` 自建映射，因 `session.status` 不带 parent）。子代理状态默认折叠或不展示。
8. **通知去重**：进入 `waiting`/`permission`/`done`/`error` 发桌面通知（§10.3）；`done → idle` 的超时迁移**不再重复通知**；同一来源的抖动在防抖窗口内只通知一次。

---

## 六、缺口与不确定点

| 缺口 | 影响 | 现状 |
|---|---|---|
| **Claude Code 无「用户中断」事件** | 中断后可能停在 `running` | 确定事实；需超时/启发式兜底 |
| **dsh 无授权事件** | `permission` 在 dsh 不可得 | 确定事实（协议保留给未来） |
| **dsh 无退出通知 / 无取消** | 只能靠传输关闭推断 `exited`；结束回合=关进程 | 确定事实 |
| **dsh 集成形态** | SDK 驱动独立 runtime，非交互 TUI | ⚠️ 需确认是否有插件旁路 |
| **pi 无内建授权** | `permission` 依赖自建扩展 | 确定事实 |
| **pi 注入方式** | 扩展需装入 `~/.pi/agent/extensions/` 或 `-e`；如何按会话注入不污染全局？ | 待调研（同 §10.5） |
| **opencode 子代理归属 bug** | 子代理 permission/error 误报为根会话「等待/出错」 | 官方 issue #30043 / #46685 未修 |
| **opencode v2 beta** | server API/SDK 仍在变；插件类型滞后 | 官方迁移文档明确 |
| **idle_prompt 语义** | 与 REQUIREMENTS 把其列入「等待输入」冲突 | 见 §七 |
| **done vs idle** | 需求未给判定阈值 | 见 §七 |
| **Windows 原生支持验证** | opencode/pi/dsh 为 Node 生态，事件机制本身跨平台；但各 agent 在纯 Windows 上的行为需实测 | ⚠️ 未逐项实测 |
| **各 agent 版本快速漂移** | 事件名/字段/门槛变化快 | 需在设置页固定并显示集成版本 |

---

## 七、待用户拍板的点

> ✅ **已定 —— E3（#24）**：采纳本文的逐事件映射表（`idle_prompt` → `idle`；`done` 不自动转 `idle`；非零退出 → `exited` + 退出码；降级模式只暴露 运行/空闲/退出 并标注「不完整」）。

1. **`idle_prompt` 映射到 `waiting` 还是 `idle`？**
   证据倾向 `idle`（它是「完成约 60 秒且用户没输入」，不是提问）。若按需求表归 `waiting`，会在每个空闲窗口都发「等待输入」通知，噪音大。
2. **`done` 与 `idle` 的边界与阈值。**
   是否采用 `done`（回合结束）→ 超时 T → `idle`？T 取多少（30s / 60s / 5min）？还是两者合并展示、仅内部区分？
3. **进程非零退出算 `error` 还是 `exited`？**
   建议：有 agent 错误事件→`error`；无事件、仅进程非零退出→`exited` + 标记非零退出码。需确认。
4. **降级模式的状态集合。**
   是否接受降级只给 `running`/`idle`/`exited`，并对不完整状态显式标注？（避免误报 `waiting`/`permission`。）
5. **pi 的集成策略。**
   - A：随 Ageminal 附带 pi 扩展，走 `~/.pi/agent/extensions/` 全局安装（污染小、一次装）；
   - B：用 `pi -e <path>` 仅对该会话注入（不改全局，但需改启动命令）；
   - C：改用 `--mode rpc`/SDK（放弃 pi 原生 TUI）。
   需要决定并纳入「集成安装」设计（§10.5 / §11 Agent 设置）。
6. **dsh 的集成策略。**
   是走官方 SDK 起独立 runtime（可控但会话与终端 TUI 分离），还是等/找 Cordis 插件旁路（保留 TUI）？`permission` 首版是否直接声明「dsh 不支持授权状态」。
7. **是否展示子代理状态。**
   默认不展示（根会话优先）？还是提供折叠/二级视图？
8. **首版是否要求每个 agent 都装原生集成，还是允许按 agent 选择「完整 / 降级」。**
   建议设置页每个 agent 显示「集成状态：原生事件 / 降级启发式 / 未安装」，并允许一键安装/卸载。

---

## 八、来源链接

**Claude Code**
- Hooks reference（事件表、matcher、`Notification` 各类型与 6s/60s 时序、`PermissionRequest`、`Stop`/`StopFailure`、`SessionStart`/`SessionEnd`）：<https://code.claude.com/docs/en/hooks> · markdown：<https://code.claude.com/docs/en/hooks.md>
- Hooks guide（matcher 列表、`agent_needs_input` 版本门槛、Windows 示例）：<https://docs.claude.com/en/docs/claude-code/hooks-guide>

**Codex**
- Hooks 官方文档（事件、`PermissionRequest`、`Interrupt`、`SessionStart/End`、`commandWindows`）：<https://developers.openai.com/codex/hooks>
- PR #17268「remove windows gate that disables hooks」（merged 2026-04-09）：<https://github.com/openai/codex/pull/17268>
- 0.120.0 Release（2026-04-11，含 #17268）：<https://github.com/openai/codex/releases/tag/rust-v0.120.0>
- PR #15252「Disable hooks on windows for now」（2026-03-20）：<https://github.com/openai/codex/pull/15252>
- Issue #17478「Enable hooks on Windows」（closed）：<https://github.com/openai/codex/issues/17478>
- `notify` 源码与 payload 测试：<https://github.com/openai/codex/blob/main/codex-rs/hooks/src/legacy_notify.rs>
- 配置参考（notify、项目级 denylist）：<https://developers.openai.com/codex/config-reference>
- `tui.notifications` / OSC9 / BEL（issue #3962）：<https://github.com/openai/codex/issues/3962>

**opencode**
- Plugins（事件清单）：<https://opencode.ai/docs/plugins/>
- Server / 事件与 API：<https://opencode.ai/docs/server/>
- 事件类型与 `SessionStatus` 源码：<https://github.com/anomalyco/opencode/blob/dev/packages/opencode/src/session/status.ts>
- v2 生成类型（`permission.asked`/`session.status`/`question.*`）：<https://github.com/sst/opencode/blob/dev/packages/sdk/js/src/v2/gen/types.gen.ts>
- 子代理事件归属问题：#30043、#46685（`https://github.com/anomalyco/opencode/issues/30043`、`/issues/46685`）
- 插件类型滞后：#7147
- v2 迁移：<https://opencode.ai/v2/docs/migrate-v1/>

**pi（`@earendil-works/pi-coding-agent`）**
- 文档总览 / 模式：<https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/index.md>
- 扩展与生命周期事件（含 `ui_prompt_start`/`ui_prompt_end`、`agent_settled`、`session_shutdown`）：<https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md>
- RPC 模式（命令、事件、扩展 UI 子协议）：<https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/rpc.md>
- JSON 事件流模式：<https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/json.md>
- 安全 / project trust / 无内建授权：<https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/security.md>

**DeepSeek Harness（`dsh`）**
- SDK Client README（owned-run API、通知、限制）：<https://github.com/deepseek-ai/deepseek-harness/blob/master/packages/sdk/client/README.md>
- SDK Protocol README（JSON-RPC 帧、方法表、无 cancel/无请求）：<https://github.com/deepseek-ai/deepseek-harness/blob/master/packages/sdk/protocol/README.md>
- Session 子系统（`turn/start`、`turn/end` 与 `TurnEndReasonMap`）：<https://deepseek-harness.github.io/deepseek-harness/en/reference/subsystems/session>
- Core / `agent/status`：<https://deepseek-harness.github.io/deepseek-harness/en/reference/subsystems/core>
- Agent 生命周期时序：<https://github.com/deepseek-ai/deepseek-harness/blob/master/docs/agent-lifecycle.md>
- `max-tokens` 静默截断讨论 #3685：<https://github.com/deepseek-ai/deepseek-harness/discussions/3685>

**其他（第三方先例，仅参考）**
- tmux-claude-status hook contract（把 `idle_prompt` 排除出 blocked 状态的理由）：<https://github.com/dop-amine/tmux-claude-status/blob/main/docs/hook-contract.md>

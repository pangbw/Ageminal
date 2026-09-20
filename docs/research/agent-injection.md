# 各 Agent 按会话注入方式（事件接入）

> **状态回写（2026-09-20）**：**本文的注入机制已被 E6（#27）取代。**
>
> 原「**零全局污染**」方案不再采用；现方案是 **写全局配置、且必须在设置页手动启用**：Claude 用 skills 目录插件、opencode 用插件文件、pi 用扩展文件、**只有 Codex 需要改既有用户文件**（`~/.codex/config.toml` 的 `notify`，按**文本级追加/替换**、不做 TOML 往返），并配 **diff 预览 + 自动备份 + 停用即回滚 + 幂等 + 不碰 PATH**。
>
> 本文关于各 agent **配置位置与事件机制的事实部分仍然有效**；`AGEMINAL_HOOK_PORT` **已废弃**；Codex **`-c` 内联 hooks 不采用**。

> 调研日期：**2026-09-19**（纯 Windows 前提）
> 背景：Ageminal 需要在启动各 coding agent 时，用其**原生机制**把会话状态事件回调到 Ageminal 守护进程，且**不能污染用户的全局配置 / home dotfiles**。对应 issue #23、`REQUIREMENTS.md` §10.4 / §10.5 / §18.6。
> 记号：**确定事实**来自官方文档 / 官方仓库源码 / 官方 issue 原文；**⚠️推测**为无直接来源的推断，需用户拍板或进一步实测。
> 本文只解决「如何注入 + 事件有哪些」，不替用户做最终选型。

---

## 一、结论（TL;DR）

1. **除 Codex 外，四个 agent 都存在「零全局污染」的按会话注入通道；Codex 是唯一需要权衡的。**
   按会话注入的推荐机制分别是：Claude Code `--settings <file>`、opencode `OPENCODE_CONFIG_DIR` / `OPENCODE_CONFIG_CONTENT` 环境变量、pi `-e/--extension <file>`、dsh `--patch <file>`。四者都只影响被注入的那一次进程，不写用户的全局配置。

2. **Claude Code 的 `--settings <file>` 是首选，hooks 可完整随会话注入，且与用户已有 hooks 合并。**
   `--settings` 优先级高于用户 / 项目 / local 文件、低于 managed；它接受内联 JSON 或文件路径，且**不写入任何文件**。hook handler 支持 `command` 与 `http` 两种，Windows 上可直接调用 `ageminal.exe`（command hook），或走 `127.0.0.1` HTTP hook。

3. **Codex 必须区分「`notify`（旧，只有完成事件）」与「hooks（新，事件丰富但需信任）」。**
   `notify` 的键在**项目级 `.codex/config.toml` 会被忽略**，但可用 CLI `-c/--config` 对单次调用覆盖（`-c notify=[...]`）；这是 Codex 唯一不写全局文件的完成事件通道。Codex hooks 现在（v0.124+ 已 stable，Windows 已启用）事件丰富得多，但**非托管 hook 必须逐条 review/trust 才会运行**，自动化场景需加 `--dangerously-bypass-hook-trust`；且有未关闭的 Windows bug（PreToolUse 在 Windows `command_execution` 路径不触发，open #24453）。

4. **opencode / pi / dsh 都提供「进程级、就地加载」的扩展点，且能读到继承的环境变量，天然适合按会话关联。**
   opencode 用 `OPENCODE_CONFIG_DIR` 指向 Ageminal 自有目录加载插件；pi 用 `-e` 加载 TS 扩展；dsh 用 `--patch` 注入 Cordis 插件。三者都不改用户全局配置，插件/扩展进程内可直接 `process.env` 读取 `AGEMINAL_*`。

5. **统一通道 `ageminal notify --event <name> --agent <id>` 由各注入物调用；会话关联靠继承的环境变量。**
   command hook / 扩展进程都继承 agent 自身的环境（agent 继承自 Ageminal 启动的终端会话），因此 `AGEMINAL_SESSION_ID`、`AGEMINAL_PIPE`、`AGEMINAL_WORKTREE` 可透传。HTTP hook 则通过 `headers` + `allowedEnvVars` 把这些变量放进请求头。

6. **dsh 的「交互面」是独立于注入机制的产品问题。**
   dsh 官方可交互入口是 `dsh web`（浏览器 Web UI）；终端 TUI 属第三方插件，`--profile sdk/acp` 是无 TUI 的自动化入口。若 Ageminal 要把 dsh 放进**终端页签**，需用户先决定走官方 Web、第三方 TUI，还是接受「无 TUI + SDK 驱动」的降级形态。

7. **「设置页一键安装集成」建议做成两层：默认零持久化（每次启动只生成 Ageminal 自有目录下的临时注入物），可选显式「写入全局」并带备份与撤销。**
   所有注入物建议放在 `%APPDATA%\Ageminal\integrations\<agent>\`；升级 agent 后兼容性靠「启动时探测版本 + 注入物版本标记」处理。

---

## 二、通用设计：注入物放哪、如何清理、如何关联会话

### 2.1 注入物目录（建议）

| 用途 | 建议路径 |
| --- | --- |
| CLI shim | `%LOCALAPPDATA%\Ageminal\bin\ageminal.exe`（hook 配置里写**绝对路径**，避免改 PATH） |
| 各 agent 注入物 | `%APPDATA%\Ageminal\integrations\<claude\|codex\|opencode\|pi\|dsh>\` |
| 每次会话的临时文件 | `%APPDATA%\Ageminal\sessions\<session-id>\`（会话结束即删） |

**清理语义**：按会话注入的文件随会话销毁；「一键安装」若只是把模板物化到 `integrations\`，撤销 = 删除对应目录 + 关闭设置开关。**默认不写** `%USERPROFILE%\.claude`、`%USERPROFILE%\.codex`、`%USERPROFILE%\.config\opencode`、`%USERPROFILE%\.pi`、`%USERPROFILE%\.dsh`。

### 2.2 会话关联（环境变量）

Ageminal 启动 agent 时注入（与 §10.5 一致）：

- `AGEMINAL_SESSION_ID`：当前终端会话 id
- `AGEMINAL_PIPE`（或固定命名管道名 + 每会话 token）：守护进程管道
- `AGEMINAL_WORKTREE`：worktree 根路径
- `AGEMINAL_AGENT`：agent id（`claude` / `codex` / `opencode` / `pi` / `dsh`）

所有注入物都从**继承的环境**读取这些变量，不需要在注入文件里硬编码会话 id（HTTP hook 用 `allowedEnvVars` 插值到 header）。

### 2.3 CLI shim 契约（建议）

```
ageminal notify --event <name> --agent <id> [--session <id>] [--json <payload>]
```

- 守护进程按 `AGEMINAL_SESSION_ID`（或 `--session`）定位会话；
- 管道 ACL 限当前用户（§15）；
- Codex 的 `notify` 与 hooks 会把事件 JSON 作为 **stdin** 或 **最后一个 argv** 传入，shim 需同时容忍两种（见 Codex 小节）。

### 2.4 统一状态机 → 事件名（建议，跨 agent 归一）

| 统一状态 | Claude Code | Codex | opencode | pi | dsh |
| --- | --- | --- | --- | --- | --- |
| `running` | `SessionStart` / `UserPromptSubmit` | `SessionStart` / `UserPromptSubmit` | `session.status`(busy) | `agent_start` | `agent/status`(running) |
| `waiting` | `Notification(idle_prompt/agent_needs_input)` | `PermissionRequest`（近似） | `session.idle`/`session.status`(idle) | `agent_settled` / `ui_prompt_start` | `agent/status`(idle) / `turn/end` |
| `permission` | `Notification(permission_prompt)` / `PermissionRequest` | `PermissionRequest` | `permission.asked` | `ui_prompt_start` | `approval/request` |
| `done` | `Stop` | `Stop` / notify(`agent-turn-complete`) | `session.idle` | `agent_settled` | `turn/end` |
| `error` | `StopFailure` | `Stop`（失败路径待核） | `session.error` | `tool_result(isError)` | `agent/error` |
| `exited` | `SessionEnd` | `SessionEnd` | 进程退出 | `session_shutdown` | `agent/disposed` |

> ⚠️推测：上表跨 agent 的语义映射（尤其 `done` vs `waiting`）需要按各 agent 实际事件序列再校准；本文只保证「事件名与来源」是确定事实。

---

## 三、逐 agent 注入方案

### 3.1 总览表

| Agent | 按会话机制 | 落地文件（Ageminal 自有） | 是否污染全局 | Windows 可用性 | 主要副作用 |
| --- | --- | --- | --- | --- | --- |
| Claude Code | `claude --settings <file>`（hooks 写入） | `integrations\claude\settings.json` | 否（`--settings` 一次性、不落盘） | ✅ 官方支持；command hook 可指定 `"shell": "powershell"` | hooks 会与用户已有 hooks **合并**（数组 merge）；HTTP hook 受用户 `allowedHttpHookUrls` 白名单约束 |
| Codex | `codex -c notify=[...]`（完成）；hooks 需 `-c` + `--dangerously-bypass-hook-trust`（⚠️待实测） | `integrations\codex\`（仅当选 CODEX_HOME/profile 方案） | 否（`-c` 一次性）；注：项目级 `.codex/config.toml` 会忽略 `notify` | ⚠️ 仅部分：hooks 已启用但 PreToolUse 在 `command_execution` 不触发（#24453 open）；`notify` 可用 | 绕过 hook 信任 = 安全权衡；trust 按 hash，命令一变即失效 |
| opencode | `OPENCODE_CONFIG_DIR=<dir>`（内含 `plugins\ageminal.ts`） | `integrations\opencode\plugins\ageminal.ts` | 否 | ✅（需实测）；Windows 配置路径 ⚠️待确认 | `session.idle` 官方标记为 **deprecated**，应改用 `session.status`；插件在启动时同步加载 |
| pi | `pi -e <file>` / `--extension` | `integrations\pi\ageminal.ts` | 否 | ✅ | `-e` 扩展在项目信任判定**之前**加载；扩展以完整权限运行 |
| dsh | `dsh --profile <p> --patch <file>`（Cordis 插件层） | `integrations\dsh\ageminal.patch.yml` + `ageminal.ts` | 否（`--patch` 为 argv 覆盖层） | ✅（Node 生态）；dsh 为 developer preview | 官方无可交互 TUI（仅 web）；breaking changes 高频 |

---

### 3.2 Claude Code

**确定事实（优先级 / 合并）**
- 设置文件优先级（高→低）：**Managed > Command line（`--settings`）> Project local `.claude/settings.local.json` > Shared project `.claude/settings.json` > User `~/.claude/settings.json`**。
- **`--settings`** 接受「内联 JSON 或文件路径」，应用于用户 / 项目 / local 之上、managed 之下；**不能**设置 Managed 或 Global config 键；**只对该次会话生效、不写入任何文件**。`hooks` 属于 "Any file"，因此 `--settings` 可以设置 hooks。
- **hooks 走数组 merge**：user / project / local / `--settings` 的 hooks 会**叠加**，不是替换；`disableAllHooks` 也关不掉 managed hooks。
- hooks 位置表明确列出 `~/.claude/settings.json`、`.claude/settings.json`、`.claude/settings.local.json`、managed、Plugin `hooks/hooks.json`、Skill/Subagent frontmatter。
- Windows：`~/.claude` = `%USERPROFILE%\.claude`；可用 `CLAUDE_CONFIG_DIR` 整体改家目录配置位置。command hook 可加 `"shell": "powershell"`（自动探测 `pwsh.exe`，回退 `powershell.exe`）。
- hook handler 类型：`command` / `http` / `mcp_tool` / `prompt` / `agent`。HTTP hook 字段：`url`、`headers`、`allowedEnvVars`、`timeout`；请求体为事件 JSON，`Content-Type: application/json`。HTTP hook 去重按 URL，command hook 按 command 字符串。
- 相关事件：`SessionStart`、`UserPromptSubmit`、`Stop`、`StopFailure`、`Notification`（matcher 含 `permission_prompt`/`idle_prompt`/`agent_needs_input`/`agent_completed`）、`PermissionRequest`、`PermissionDenied`、`SessionEnd`、`ConfigChange` 等。
- **HTTP 白名单**：若用户任意层设置了 `allowedHttpHookUrls`，则只有匹配的 URL 才会运行；`httpHookAllowedEnvVars` 限制可插值的环境变量。

**按会话注入方案**
1. Ageminal 启动前写 `%APPDATA%\Ageminal\sessions\<sid>\claude-settings.json`，内容只含 `hooks`（可含 `$schema`）。
2. 启动命令：`claude --settings "<绝对路径>"`。
3. hook handler 二选一：
   - **command（推荐）**：`type: "command"`，`command` 直接指向 `ageminal.exe` 绝对路径；可设 `"async": true`（后台、不阻塞 agent）。Windows 若需脚本包裹，可加 `"shell": "powershell"`。
   - **http**：`type: "http"`，`url: "http://127.0.0.1:<port>/hook"`，`headers: { "X-Ageminal-Session": "$AGEMINAL_SESSION_ID", "Authorization": "Bearer $AGEMINAL_TOKEN" }`，`allowedEnvVars: ["AGEMINAL_SESSION_ID","AGEMINAL_TOKEN"]`；需守护进程暴露 loopback 端点。注意用户白名单。
4. 会话结束删除该文件（`--settings` 本身不依赖文件常驻，但保留也无害）。

**副作用**
- 用户已有 hooks 仍会运行（merge）；如果用户在同一事件上也有 hook，两者都会跑。
- HTTP hook 若守护进程响应慢会阻塞该事件（除非 `async`）；command hook 用 `async: true` 可规避。
- 无法通过 `--settings` 覆盖 managed 策略（企业环境可能禁用 hook，见 `allowManagedHooksOnly`）。

**来源**：<https://code.claude.com/docs/en/settings> · <https://code.claude.com/docs/en/hooks> · <https://docs.anthropic.com/en/docs/claude-code/hooks>

---

### 3.3 Codex

**确定事实（配置与限制）**
- 配置文件：用户级 `~/.codex/config.toml`（Windows：`%USERPROFILE%\.codex\config.toml`，由 `CODEX_HOME` 覆盖，默认 `~/.codex`）；项目级 `.codex/config.toml`（仅项目被信任时加载，从项目根向 cwd 逐层）。
- 优先级（高→低）：**CLI flags 与 `-c/--config` 覆盖 > profile（`$CODEX_HOME/<name>.config.toml`）> 项目 config > 用户 config > 系统 config(`/etc/codex/config.toml`)**。
- **`notify` 在项目级 `.codex/config.toml` 会被忽略**（明确列在忽略键清单：`openai_base_url`、`model_provider(s)`、`notify`、`profile(s)`、`otel` 等），必须放用户级。
- 顶层 `notify = ["cmd", "arg", ...]`：命令收到**单个 JSON 参数**（payload 作为最后一个 argv，不是 stdin），事件当前只有 `agent-turn-complete`；字段含 `type`/`thread-id`/`turn-id`/`cwd`/`input-messages`/`last-assistant-message`。源码 `codex-rs/hooks/src/legacy_notify.rs` 把它实现为一个内部 hook。
- `-c/--config key=value`：值按 TOML 解析，失败则当字符串；文档称其可覆盖任意键、对本次调用生效。因此 `-c 'notify=["<abs>\\ageminal.exe","notify","--event","codex.completed","--agent","codex"]'` 是**不写全局文件**的 notify 注入途径（⚠️待实测确认 TOML 数组在 argv 引号下的解析）。
- **hooks**（v0.124 stable，2026-04-23；v0.129 增 `PreCompact/PostCompact`；v0.150 增 `Interrupt`）：
  - 来源：`hooks.json` 或 `config.toml` 内联 `[hooks]`；用户级与项目级都识别。
  - 事件：`SessionStart`/`SessionEnd`/`UserPromptSubmit`/`PreToolUse`/`PermissionRequest`/`PostToolUse`/`PreCompact`/`PostCompact`/`SubagentStart`/`SubagentStop`/`Stop`/`Interrupt`。
  - handler 类型：`command`、`mcp_tool`（`prompt`/`agent` 被解析但跳过）。command hook 读 **stdin JSON**，退出码 `2` 阻止、`stderr` 为原因；`async=true` 后台；`commandWindows` / `command_windows` 为 Windows 专用命令覆盖。
  - **信任**：非托管 hook 运行前必须 review 并 trust，trust 记录在 `~/.codex/config.toml` 的 `[hooks.state."<source>:<event>:<group>:<index>"] trusted_hash`，**按 hook 当前 hash 记录**；新/改过的 hook 会被跳过。一次性自动化可加 `--dangerously-bypass-hook-trust` 绕过持久信任。
  - **Windows**：hooks 已在 PR #17268 启用（issue #17478 closed completed，2026-04-15）。但 open issue **#24453**（2026-08-27 仍 open）：Windows 的 `command_execution` 路径不触发 `PreToolUse`，即使 matcher 为 `"*"`；`SessionStart`/`Stop`/`PermissionRequest` 等是否完整触发 ⚠️需实测。
- `CODEX_HOME` 重定向：会把 config/auth/logs/sessions/skills 全部搬到新目录。若指向临时目录，会**丢失登录与历史**（auth 可能在 OS keychain）。故仅当 Ageminal 显式复制/链接 `auth.json` 时才可用；默认不建议。

**按会话注入方案（按风险从低到高）**
1. **仅完成事件（最低风险，推荐先做）**：
   `codex -c 'notify=["<abs>\\ageminal.exe","notify","--event","codex.turn-complete","--agent","codex"]'`
   —— 不写任何文件，不触发信任流程。注意 shim 要吃掉末尾 JSON 参数。
2. **丰富事件（hooks）**：
   - 方案 A：`-c 'hooks.<Event>=[{...}]'` 逐事件内联覆盖（⚠️待实测：数组 of inline table 是否能被 `-c` TOML 解析与合并）+ `--dangerously-bypass-hook-trust`。
   - 方案 B：用 `--profile` + `$CODEX_HOME/<profile>.config.toml` 放 hooks，再用 `CODEX_HOME` 指向 Ageminal 目录 —— 会丢 auth，需处理，不推荐。
   - 方案 C：**写用户级 `~/.codex/hooks.json`**（全局污染）—— 只有用户明确选择「一键安装（全局）」时才做，且要备份、可撤销。
3. 无论哪种，hook command 用 `ageminal.exe` 绝对路径；Windows 上加 `commandWindows` 指向同一可执行文件或 `.cmd`。

**副作用**
- 方案 1 只有「完成」事件，无「等待/授权」；这些阶段需靠进程检测/空闲启发式（§10.5 降级）。
- 方案 2 绕过信任 = 任何已在用户 `~/.codex/hooks.json` 的 hook 也会在 Ageminal 启动的会话中未经 review 运行（安全权衡）。
- hook 命令字符串一旦改变，trust hash 失效；升级 Ageminal 若改了命令行需重新 trust 或继续用 bypass。
- 项目级 `.codex/config.toml` 无法承载 `notify`；把注入物写进用户 worktree 还会弄脏 `git status`，不建议。

**来源**：<https://developers.openai.com/codex/hooks> · <https://developers.openai.com/codex/config-advanced> · <https://developers.openai.com/codex/config-reference> · <https://developers.openai.com/codex/cli/reference> · <https://developers.openai.com/codex/config-file/environment-variables> · openai/codex #17478（closed）· #24453（open）· 源码 `codex-rs/hooks/src/legacy_notify.rs`

---

### 3.4 opencode

**确定事实**
- 配置优先级（低→高）：Remote `.well-known/opencode` → 全局 `~/.config/opencode/opencode.json` → **`OPENCODE_CONFIG`（自定义文件路径）** → 项目 `opencode.json` → `.opencode` 目录 → **`OPENCODE_CONFIG_CONTENT`（内联 JSON）** → managed。
- **`OPENCODE_CONFIG_DIR`**：指定一个自定义目录，会像标准 `.opencode` 目录一样被搜索 `agents/`、`commands/`、`modes/`、**`plugins/`** 等；**加载在全局配置与 `.opencode` 之后**，因此可覆盖。
- 插件加载：本地文件放 `.opencode/plugins/`（项目）或 `~/.config/opencode/plugins/`（全局），启动时自动加载；npm 插件经配置 `plugin: [...]` 由 Bun 安装到 `~/.cache/opencode/node_modules/`。官方明确 `.opencode` 子目录**复数名 canonical**，单数名向后兼容。
- 插件是导出插件函数的 JS/TS 模块，返回 hooks 对象；可订阅 `event`，并收到 `{ event }`，按 `event.type` 过滤。
- 事件清单（部分）：`session.created`、`session.status`、**`session.idle`（官方源码注释标注 deprecated，建议用 `session.status` 判断完成）**、`session.error`、`session.diff`、`session.compacted`、`permission.asked`、`permission.replied`、`message.*`、`file.edited`、`tool.execute.before/after`、`todo.updated`、`shell.env`。
- 插件上下文含 `project`、`directory`、`worktree`、`client`、`$`（Bun shell）；进程内可访问 `process.env`。

**按会话注入方案**
1. Ageminal 准备 `%APPDATA%\Ageminal\integrations\opencode\plugins\ageminal.ts`（导出插件，监听上述事件并调用 `ageminal notify`，或直接向 Named Pipe 写）。
2. 启动命令前设环境变量：`OPENCODE_CONFIG_DIR="%APPDATA%\Ageminal\integrations\opencode"`（可再配 `OPENCODE_CONFIG` 指向一个只做无害覆盖的 JSON，或省略）。
3. 若插件需外部依赖，在该目录放 `package.json`（opencode 启动时 `bun install`；依赖缓存进 `~/.cache/opencode`）。
4. 不写 `~/.config/opencode/`、不写项目 `opencode.json` / `.opencode/`。

**Windows 说明**
- opencode 是 Bun 生态；全局配置路径在文档中写作 `~/.config/opencode/`，Windows 实际解析路径 ⚠️待实测（可能为 `%USERPROFILE%\.config\opencode\`）。用 `OPENCODE_CONFIG_DIR` 后不依赖该默认路径。
- `.opencode` 多目录加载在 Windows 上同样适用。

**副作用**
- `session.idle` 已 deprecated，长期应改用 `session.status`；短期两者可能并存。
- 自定义目录**最后加载**，其插件与全局同名插件会**分别加载**（不是去重），注意不要重复发通知。
- 插件在启动时同步加载，若需异步初始化要自行处理首事件竞态。

**来源**：<https://opencode.ai/docs/plugins> · <https://opencode.ai/docs/config> · <https://github.com/anomalyco/opencode>（docs/plugins.mdx）· 社区手册对事件源码的定位（session/status.ts、session/index.ts）

---

### 3.5 pi（`@earendil-works/pi-coding-agent`）

**确定事实**
- 扩展位置：全局 `~/.pi/agent/extensions/*.ts`（或 `*/index.ts`）；项目级 `.pi/extensions/*.ts`（仅在项目被信任后加载）。也可在 `settings.json` 用 `extensions: [...]` 追加路径。
- **CLI 注入**：`pi -e <file>`（`--extension`），官方定位为「快速测试」；`-e` 扩展在**项目信任判定之前**加载（因此不受项目 trust 影响）。扩展经 jiti 直接跑 TS，无需编译。
- 扩展 API：`pi.on("<event>", handler)`；事件含 `session_start`、`session_shutdown`、`agent_start`、`agent_end`、**`agent_settled`（推荐用于「完成/空闲」状态，表示不会再自动继续）**、`turn_start/turn_end`、`message_*`、`tool_execution_*`、`tool_call`（可 block）、`tool_result`、**`ui_prompt_start` / `ui_prompt_end`（可用于「等待用户」）**、`session_before_*`、`model_select` 等。
- 其它集成模式：`pi --mode json`（stdout 逐行 JSON 事件流）、`pi --mode rpc`（stdio JSONL 双向 RPC，命令 + 事件 + extension UI 子协议）；`--print/-p` 为非交互。官方建议 Node/TS 场景优先用 SDK（`AgentSession`）而非子进程。
- 进程内扩展可访问 `process.env`，也通过 `ctx.sessionManager` 读取会话信息。

**按会话注入方案**
1. Ageminal 维护 `%APPDATA%\Ageminal\integrations\pi\ageminal.ts`（默认导出 `(pi: ExtensionAPI) => {...}`），监听 `agent_settled`、`ui_prompt_start`、`session_shutdown`、`turn_end`（错误经 `tool_result` 的 `isError` 等）并调用 `ageminal notify`。
2. 启动命令：`pi -e "<绝对路径>\ageminal.ts"`。
3. 不改 `~/.pi/agent/settings.json`，也不写 `.pi/` 到用户仓库。
4. 备选：若不需要交互 TUI，可用 `--mode json` / `--mode rpc` 由 Ageminal 直接解析结构化事件流；但这会**取代**原生 TUI（无 UI），属于不同产品形态。

**副作用**
- 扩展以完整系统权限运行（官方安全提示）；注入物应可审计、签名/哈希校验。
- 项目级 `.pi/extensions` 与 `settings.json` 是用户自有；`-e` 不碰它们。
- `agent_end` 不等于完成（可能自动重试/压缩/续跑），必须用 `agent_settled`，否则状态会抖动。

**来源**：<https://pi.dev/docs/latest/extensions> · <https://pi.dev/docs/latest/rpc> · <https://github.com/earendil-works/pi>（`packages/coding-agent/docs/extensions.md`、`docs/rpc.md`、`docs/json.md`）

---

### 3.6 dsh（DeepSeek Harness，`deepseek-ai/deepseek-harness`）

**确定事实**
- 架构：Cordis「everything-is-a-plugin」；插件是导出 `name` + `apply(ctx)` 的 TS 模块；`ctx.on("event", handler)` 注册监听，插件卸载时监听自动移除。
- 启动器：`dsh` 是 **profile 启动器**。`dsh --profile <name>` 启动 `$DSH_HOME/profiles/<name>`；`dsh web` = `--profile web`；`dsh --profile sdk|sdk-minimal|acp|headless` 为自动化入口。
- 层叠顺序（低→高）：各 bundle 的 patch（按 `dsh.profile.bundles`）→ profile 自己的 `cordis.patch.yml` → **家级 `$DSH_HOME/cordis.patch.yml`** → **`--patch <path>` 覆盖层（按 argv 顺序）**。
- 因此 **`--patch <file>` 是进程级、不落盘的注入层**；patch 内容形如：
  ```yaml
  - insert:
      - id: ageminal
        name: '<绝对路径>/ageminal-dsh-plugin.ts'
  ```
  官方教程即用 `pnpm dsh web --patch ./scratch-plugin/cordis.yml` 载入本地插件，证明该路径可行。
- 事件分两类：
  - **Cordis 实时事件**（`ctx.on`）：`agent/status`（running/idle）、`agent/pre-step`、`agent/request`、`agent/request-error`、`agent/turn-stopping`、`tools/result`、`session/event`、`session/created`、`session/flush` 等。
  - **durable SessionEvent**（只记录在会话日志，不直接 emit）：`turn/start`、`turn/end`、`step/start`、`step/end`、`user/message`、`assistant/message`、`tool/call`、`tool/result`、`compaction/*`。要观察它们须监听 `session/event` 再按 `event.type` 分派。
  - **授权**：`approval/request`（waterfall，供 answerer 决定）、审计 `approval/asked` / `approval/decided`。
- SDK：`@deepseek-ai/dsh-sdk-client`（TypeScript，stdio JSON-RPC）驱动 `dsh --profile sdk`；高层 `DeepSeekHarness.run()` 返回 `RunResult { sessionId, finalResponse, events, notifications }`；低层 `HarnessClient` 有显式 `start/initialize/prompt/request/close` 与 `subscribe()` / `subscribeSessionTree()`。服务端 `@deepseek-ai/dsh-sdk-jsonrpc-server` 由 profile 决定是否加载。
- 状态：**developer preview**，官方明示会有 breaking changes。

**按会话注入方案**
1. Ageminal 生成 `%APPDATA%\Ageminal\integrations\dsh\ageminal-dsh-plugin.ts`（`apply(ctx)` 内 `ctx.on("agent/status", ...)`、`ctx.on("session/event", ...)`、`ctx.on("approval/request", ... )`，只读观测、对 waterfall 调用 `next()` 透传）与 `ageminal.patch.yml`。
2. 启动命令（按用户选择的交互面）：`dsh --profile <p> --patch "<绝对路径>\ageminal.patch.yml"`。
3. 不改 `$DSH_HOME`（默认 `~/.dsh`），不写 `~/.dsh/cordis.patch.yml`、不改用户的 profile。
4. 若走 SDK 形态：Ageminal 运行 `dsh --profile sdk`，用 `@deepseek-ai/dsh-sdk-client` 驱动，直接消费 `events` / `notifications`（此时不需要注入插件，但**没有原生 TUI**）。

**副作用 / 待决**
- **交互面冲突**：官方可交互入口是 `dsh web`（浏览器），终端 TUI 由第三方插件提供（如 `@dsh-tui/dsh-tui`、`@deepseek-ai/dsh-tui` 等，属 out-of-tree），`--profile sdk/acp` 无 UI。Ageminal 若要「dsh 终端页签」，必须选：官方 Web（嵌浏览器 / 外开）、第三方 TUI、或 SDK 驱动无 TUI。
- developer preview：插件 API、事件名、patch 语义都可能变；升级兼容性风险最高。
- waterfall 事件（如 `agent/pre-step`、`approval/request`）若忘记调用 `next()` 会**短路**整个管线，注入物必须只读透传。

**来源**：<https://github.com/deepseek-ai/deepseek-harness>（README、`apps/cli/README.md`、`docs/user/develop/basic/`、`docs/user/develop/framework/events.md`、`docs/subsystems/approval.md`、`docs/agent-lifecycle.md`）· <https://deepseek-harness.github.io/deepseek-harness/en/develop/framework/events> · <https://www.npmjs.com/package/@deepseek-ai/dsh-sdk-client> · plugin 示例 <https://github.com/dsh-tui/dsh-tui>

---

## 四、设置页「一键安装集成」与撤销

### 4.1 建议的两层模型

**默认层（零全局污染，推荐默认开启）**
- 设置页「Agent → 集成」开关打开后，Ageminal 只做：
  1. 将注入模板物化到 `%APPDATA%\Ageminal\integrations\<agent>\`；
  2. 启动该 agent 时追加对应参数/环境变量（见上表）。
- 撤销：关闭开关 + 删除 `integrations\<agent>\`。用户全局配置**自始至终未被触碰**。

**可选层（显式「写入全局集成」）**
- 仅当用户明确勾选时才写：
  - Claude Code：`%USERPROFILE%\.claude\settings.json` 追加 hooks（先备份）。
  - Codex：`%USERPROFILE%\.codex\hooks.json`（或 config 内联），并提示需 `/hooks` 信任。
  - opencode：`%USERPROFILE%\.config\opencode\plugins\ageminal.ts`。
  - pi：`%USERPROFILE%\.pi\agent\extensions\ageminal.ts`。
  - dsh：`%USERPROFILE%\.dsh\cordis.patch.yml`（用户补丁层）。
- 撤销：按安装时记录的备份/差异回滚；UI 必须显示「改动了哪些文件」。

### 4.2 UI 上要明示的信息
- 每个 agent 一行：**机制**（settings / notify / plugin / extension / patch）、**会读写的路径**、**是否写入全局**（默认否）、**Windows 可用性**、**已知副作用**（尤其 Codex 的信任绕过与 Windows 部分失效）。
- 「一键安装」按钮旁提供「查看将要写入的内容」（diff 预览）。
- 提供「恢复默认 / 卸载集成」。

> 注：CLI shim 的 PATH。若要求用户能在普通终端手敲 `ageminal`，才需要写用户 PATH（全局环境变量）。默认方案在注入物里写**绝对路径**，因此**无需改 PATH**，规避一项全局污染。

---

## 五、升级兼容性

| Agent | 版本锚点（调研所见） | 兼容性风险 | 建议的运行时防护 |
| --- | --- | --- | --- |
| Claude Code | hooks HTTP handler 为 2026-02 新增；`--settings`、hooks 事件表持续扩充 | hook 事件名/字段新增（旧名一般保留）；managed 策略可能禁用 | 注入物带 `$schema`；启动时记录 `claude --version`；JSON 解析失败则降级 |
| Codex | hooks v0.124 stable(2026-04-23)；v0.129 PreCompact(2026-05-07)；v0.150 Interrupt(2026-08-26)；稳定版 v0.153.4(2026-09-04) | `-c notify` 未实测；hooks trust hash 绑定命令；Windows PreToolUse 缺陷 | 首次运行做 `-c` 自检；记录 `codex --version`；Windows 回退 `notify` + 进程检测 |
| opencode | v2 并行推进；`session.idle` deprecated | 事件名/插件 API 变化；配置目录默认路径在 Windows 待确认 | 同时监听 `session.status` 与 `session.idle`；插件版本标记 |
| pi | 包版本约 0.74–0.80（repo 从 `badlogic/pi-mono` 迁到 `earendil-works/pi`） | 扩展 API 预 1.0，事件语义可能调整（如 `agent_settled`） | 以 `agent_settled` 为完成锚点；捕获扩展加载异常并降级 |
| dsh | developer preview（`@deepseek-ai/dsh@next`） | **官方保证 breaking changes**；事件名/patch 语义可变 | 启动时 `dsh --version`；插件用可选链 + try/catch；把 dsh 标为「实验性」 |

**通用做法**：注入物内嵌一个 `AGEMINAL_INTEGRATION_VERSION`；守护进程在会话启动时记录 agent 版本与集成版本；不匹配则只启用最小降级（进程检测 + 空闲启发式），并在设置页提示。

---

## 六、待用户拍板的点

> ✅ **本节的机制已被 E6（#27）取代**：改为**写全局配置 + 设置页手动启用**，并配 diff 预览 / 自动备份 / 停用即回滚。

1. **Codex 路线**：接受 `--dangerously-bypass-hook-trust` 换取完整事件（授权/工具事件），还是只做 `-c notify`（仅完成）+ 进程检测降级？后者安全但能力弱，且 Windows 上 hooks 本就有 #24453 缺陷。
2. **Claude Code handler 选型**：`command`+`async`（简单、无端口）还是 `http`（守护进程需 loopback + token，且受用户 `allowedHttpHookUrls` 约束）？
3. **dsh 交互面**：官方 `dsh web`（浏览器）/ 第三方终端 TUI / `--profile sdk` 无 TUI？以及是否因 developer preview 而推迟 dsh 进入首批。
4. **pi 形态**：原生 TUI + `-e` 扩展（保留交互），还是 `--mode json/rpc`（结构化但无 TUI）？
5. **一键安装的边界**：是否允许「写入全局」选项存在？若允许，撤销/备份的 UI 做到什么程度？是否需要 shim 写 PATH？
6. **注入物目录**：`%APPDATA%\Ageminal\integrations\` 是否可接受；是否需要签名/哈希校验注入物（各 agent 都提醒扩展拥有完整权限）。
7. **Codex `-c` 内联 hooks 是否可行**需实测（本文标 ⚠️）；若不可行，是否接受方案 C（写全局 hooks.json）或直接放弃 Codex 的丰富事件。

---

## 七、关键来源

- Claude Code：<https://code.claude.com/docs/en/settings> · <https://code.claude.com/docs/en/hooks> · <https://docs.anthropic.com/en/docs/claude-code/hooks> · <https://docs.anthropic.com/en/docs/claude-code/settings>
- Codex：<https://developers.openai.com/codex/hooks> · <https://developers.openai.com/codex/config-advanced> · <https://developers.openai.com/codex/config-reference> · <https://developers.openai.com/codex/cli/reference> · <https://developers.openai.com/codex/config-file/environment-variables> · openai/codex #17478（Windows hooks 启用，closed）· #24453（Windows PreToolUse 不触发，open）· 源码 `codex-rs/hooks/src/legacy_notify.rs`
- opencode：<https://opencode.ai/docs/plugins> · <https://opencode.ai/docs/config> · <https://github.com/anomalyco/opencode>
- pi：<https://pi.dev/docs/latest/extensions> · <https://pi.dev/docs/latest/rpc> · <https://github.com/earendil-works/pi>
- dsh：<https://github.com/deepseek-ai/deepseek-harness> · <https://deepseek-harness.github.io/deepseek-harness/en/develop/framework/events> · <https://www.npmjs.com/package/@deepseek-ai/dsh-sdk-client> · <https://github.com/dsh-tui/dsh-tui>（第三方终端 TUI 示例）

---

## 更新注记（2026-09-19，来自 #46）

- **opencode v2 插件 API 漂移**：本机 `@opencode/plugin@2.0.9` 的实际契约为 `define / setup / context.event.subscribe`，与官网 v1 文档不一致；事件里的会话标识是 **`data.sessionID`**。§3.4 的 opencode 方案应以 **v2 契约**为准。
- 各 agent 的**会话恢复（resume）**机制见 `docs/research/agent-resume.md`（#46）。

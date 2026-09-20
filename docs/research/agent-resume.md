# 各 Agent 的会话 id 与 resume 机制

> **状态回写（2026-09-20）**：**已定 —— #46**。四家 CLI 均支持从指定会话恢复（`claude --resume` / `codex resume` / `opencode -s` / `pi --session`）；**默认不 fork**；**恢复时必须重传首次启动的注入参数**（否则「恢复了但事件不再回调」）；**dsh 无一等 CLI resume**。
>
> 文中标「需实测」的条目（opencode `-s` 的跨项目/cwd 作用域、pi 部分 id 的搜索范围等）**仍未实测**，留作实现期验收。

> 调研日期：**2026-09-19**
> 对应 issue **#46**（wayfinder 决策地图），语义前提：Ageminal 的「**会话恢复**」指 **agent 对话会话**的恢复，靠 agent CLI **自身的 resume 能力**（从指定会话启动），**不是**复活已死进程的终端屏幕。
> 记号：**确定事实**来自官方文档 / 官方源码 / 本机 CLI 实测；**⚠️ 推测/待实测**为无直接来源或未验证的推断；**[建议]** 为本文给 Ageminal 的设计提案。
> 关联既有调研：[`agent-injection.md`](./agent-injection.md)（注入机制与事件）、[`agent-event-mapping.md`](./agent-event-mapping.md)（事件→状态机）、[`session-lifecycle-references.md`](./session-lifecycle-references.md)（页签↔PTY 生命周期与孤儿对账）。
> 本机版本实测：`opencode v2.0.8`、`dsh 0.1.0-rc.6`；Claude / Codex / pi **本机未安装**，其结论以官方文档为准（见文末来源）。

---

## 一、结论（TL;DR）

1. **Claude Code、Codex、opencode、pi 四个 agent 都支持「从指定会话恢复」，且都可编程拿到会话 id；差别在 id 的捕获方式、cwd 限制、以及恢复时哪些启动参数不会自动继承。**
   统一形态是：**agent 把会话写在用户目录下的持久文件/数据库里，启动时用「会话 id（或会话文件路径）」重新加载对话历史**；id 与 Ageminal 的终端页签是**两套身份**，页签需要把 agent 会话 id 当作一个属性记住。

2. **id 捕获全部可以走 Ageminal 已经设计好的「按会话注入事件通道」，但字段与取法各不相同：**
   - Claude Code：hook 公共字段 **`session_id`**（`SessionStart` 等每个事件都带），另有 `transcript_path`、`cwd`。
   - Codex：hook 公共字段 **`session_id`**，另有 `transcript_path`；传统 `notify` 里是 **`thread-id`**。
   - opencode：v2 插件事件的 `session.created` 载荷里是 **`data.sessionID`**（本机 `@opencode/schema` 类型实证）。
   - pi：`session_start` / `session_shutdown` 事件**不带 id**，要从 **`ctx.sessionManager.getSessionId()` / `getSessionFile()`** 取。

3. **cwd 上有两个必须处理的分歧：**
   - **Claude Code 与 Codex 支持跨目录按 id 恢复**：Claude `--resume <id>` 会先在当前项目及 git worktree 找、再全机搜索（v2.1.223+）；Codex 在当前 cwd 与「会话保存目录」不一致时会**交互式询问用哪个目录**，除非设 `tui.resume_cwd` 或显式 `--cd`。
   - **opencode 与 pi 的会话按项目/cwd 归属**：opencode 的 `session list` 只列「当前项目」；pi 的会话目录名内嵌 cwd 路径。**恢复应在记录的 worktree 目录里启动**，并显式传目录/会话路径。

4. **恢复时「注入参数」不一定自动继承，必须每次恢复重新拼装：**
   - Claude 官方明说：`--settings`、`--mcp-config`、`--plugin-dir`、`--fallback-model`、`--add-dir` **不会**随会话恢复，需要**再次传入**。Ageminal 的按会话注入本来就是每次启动生成参数，天然满足。
   - Codex 的 `codex resume` 接受与 `codex` 相同的全局 flags，因此 `-c notify=...` / hooks / `--dangerously-bypass-hook-trust` 可与 resume 共存；但 hook **trust 按 hash**，命令变更会失效。
   - opencode / pi 的注入走**环境变量 / 启动参数**，与 `-s` / `--session` 正交，可共存。

5. **「恢复」与「分叉」必须区分。** 四个 agent 都有 fork/branch 能力（Claude `--fork-session`、Codex `codex fork`、opencode `--fork`、pi `--fork`），fork 会生成**新 id**。Ageminal 默认恢复应**不加 fork**；若用户选择「从此分叉」，页签必须用事件里随后上报的新 id 覆盖旧 id。

6. **dsh 的现状是「核心/SDK 有 resume，官方 CLI 没有一等 resume」。**
   dsh 的 agent 核心有 `ctx.agents.resume({ resumeSessionId })` 与配置项 `agents[].resumeSessionId`；启动器 `dsh --profile <name>` 只解析自己的 flags，其余透传给 profile app。官方 README 的 `dsh --profile tui --resume <id>` 是**「假设 tui profile 已安装；`--resume` 属于终端 app」的示例**，内置 profile（web/headless/sdk/sdk-minimal/acp/base）中没有官方带 resume 的终端 TUI。社区 TUI `dsh-cli` 提供 `/resume <id>` 和「自动恢复当前目录最近会话」。Ageminal 若首版不支持 dsh，应显式标注「无官方可恢复终端入口」。

---

## 二、逐 agent 总览表

| Agent | resume 命令（确定事实） | 「最近一次」 | id 捕获方式 | id 作用域 / 持久化 | resume 的 cwd 限制 | 与 Ageminal 注入的兼容 | 主要已知坑 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| **Claude Code** | `claude --resume <id>\|<name>\|<transcript-path>`（`-r`）；非交互 `claude -p --resume <id>`；会话内 `/resume` | ✅ `claude -c/--continue`（仅当前目录；跳过 `-p`/SDK/`/loop` 首提示的会话） | hook 公共字段 **`session_id`**（`SessionStart` 起每个事件都有）+ `transcript_path` + `cwd`；`claude -p --output-format json` 返回 session id | 全局 UUID；默认存 `~/.claude/projects/<project>/<session-id>.jsonl`（`<project>` = cwd 转义名）；`CLAUDE_CONFIG_DIR` 可整体迁移；默认保留 30 天 | **可跨目录**（v2.1.223+ 先本目录/worktree，再全机；仅当唯一匹配才解析）；worktree 消失则回退到当前目录 | ✅ `--settings` 与 `--resume` 独立传递；但 `--settings` **不会**随恢复继承，恢复时必须重传 | 同 id 双开会让两条消息交错进同一 transcript；`-c` 跳过 `-p`/SDK 会话；复制出的重复 transcript 会让 id 搜索报 not-found |
| **Codex** | `codex resume [SESSION_ID]`；非交互 `codex exec resume [SESSION_ID] [PROMPT]`；TUI 内 `/resume` | ✅ `codex resume --last`（**限当前 cwd**）、`codex exec resume --last`；`--all` 才跨目录 | hook 公共字段 **`session_id`** + `transcript_path`；`notify` 载荷 **`thread-id`**；`/status`、`codex archive/delete <SESSION>` 也收 id/name | UUID（或会话名）；存 `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`（另有 `history.jsonl` / 索引）；`CODEX_HOME` 迁移 | **可跨目录**，但 cwd 与「会话保存目录」不同时**会交互式询问**；用 `tui.resume_cwd="current"\|"session"` 免询问，`--cd/-C` 优先级最高 | ✅ `resume` 接受 `codex` 相同的全局 flags，`-c notify=...` / hooks / `--dangerously-bypass-hook-trust` 可共存；hooks 信任按 hash | `--last` 是 cwd 作用域，多仓库易续错；归档会话需先 `unarchive`；hook trust hash 一改即失效 |
| **opencode** | `opencode --session/-s <id>`；非交互 `opencode run -s <id>`；`opencode attach -s <id>`；TUI 会话选择器；`opencode session list` | ✅ `opencode -c/--continue`（最近会话）、`opencode run -c` | v2 插件事件 `session.created` 的 **`data.sessionID`**（`@opencode/schema` 实证）；`opencode session list --format json`；`run --format json` 的事件流 | id 形如 `ses_...`，文本主键；v2 存 SQLite `~/.local/share/opencode/opencode.db`（`session`/`session_v2` 表），另有 `storage/`；会话有 `project_id` + `directory` 字段 | ⚠️ **未找到官方明示**；会话按项目归属（`session list` 只列当前项目），**[建议] 在记录目录启动**：`opencode <dir> -s <id>` / `run --dir <dir> -s <id>` | ✅ `OPENCODE_CONFIG_DIR` / 插件与 `-s` 正交，恢复时插件照常加载 | v2 插件 API 与官网 v1 文档不一致（v2 用 `define`/`setup`/`context.event.subscribe`）；`--fork` 产生新 id；跨项目 `-s` 行为待实测 |
| **pi** | `pi --session <path\|id>`（id 可部分匹配）；`pi -r/--resume` 选择器；会话内 `/resume`、`/tree`、`/fork`、`/clone` | ✅ `pi -c/--continue` | `session_start` / `session_shutdown` **不带 id**；从 **`ctx.sessionManager.getSessionId()` / `getSessionFile()`** 取；`/session` 显示 | UUID（默认，可自定义）；存 `~/.pi/agent/sessions/--<cwd 转义>--/<timestamp>_<session-id>.jsonl`；`--session-dir` 改根目录 | 会话目录按 cwd 分桶；`--session <path>` 可指向任意文件；`-c` 取「最近会话」（应为当前项目）；⚠️ 跨目录按**部分 id** 是否可解析**未找到明确说明** | ✅ `pi -e <扩展> --session <id>` 共存；`-e` 在项目信任判定前加载 | 扩展必须从 `sessionManager` 读 id（事件里没有）；worktree 路径变化会换会话目录桶 |
| **dsh**（暂不支持） | 核心：`ctx.agents.resume({ resumeSessionId })`；配置 `agents[].resumeSessionId`。**启动器无一等 resume**；官方示例 `dsh --profile tui --resume <id>` 属「假设 tui profile 已安装」的 app 参数 | 社区 TUI `dsh-cli` 自动续当前目录最近会话；核心**只能按 id** | 核心返回 `sessionId`（全局唯一）；SDK `RunResult.sessionId`；持久化 JSONL 日志 | 全局唯一 `SessionId`；`$DSH_HOME`（默认 `~/.dsh`）下的会话日志，`cwd` 是 agent 配置项 | 未知；核心 resume 依赖持久化后端 | `--patch` 是启动器层，应用在所有 profile 层之后；与 app 的 `--resume` 正交 | 官方内置无可交互终端 TUI；developer preview，breaking changes 高频 |

---

## 三、逐 agent 详述

### 3.1 Claude Code

**确定事实（resume 命令）**
- `claude --resume, -r [<session-id>|<name>|<transcript-path>]`：按 id / 名称 / `.jsonl` **绝对路径**恢复；不给参数则打开会话选择器。
- `claude --continue, -c`：恢复**当前目录**最近一次对话；会跳过 `claude -p`、Agent SDK 创建的会话，以及首个 prompt 为 `/loop` 的会话（`claude -p --continue` 才包含它们）。
- `claude -p --resume <id> --output-format json "…"`：非交互续跑并拿到结构化结果。
- `--fork-session`：与 `--resume`/`--continue` 连用，**生成新 session id**。
- 会话内 `/resume` 切换；`/branch` 分叉。

**确定事实（id 捕获）**
- hooks 的**公共输入字段**（每个事件都有）：`session_id`（当前会话标识）、`transcript_path`（对话 JSON 路径）、`cwd`、`hook_event_name`。`SessionStart` 也带这些，并在 `source` = `startup`/`resume`/`clear`/`compact`/`fork` 时区分来源；`source` 为 `resume`/`fork` 且 transcript 已有回复时，额外带 `seconds_since_last_response`、`context_tokens` 等（v2.1.251+）。
- `claude -p --output-format json` 的返回里含 session id、usage、cost。
- `claude agents --json` 可列后台会话。

**确定事实（作用域与持久化）**
- 默认路径：`~/.claude/projects/<project>/<session-id>.jsonl`，`<project>` 是工作目录路径把非字母数字替换为 `-`（过长则截断 200 字符 + 路径 hash）。
- `CLAUDE_CONFIG_DIR` 整体迁移配置目录；`CLAUDE_CODE_PROJECT_DIR_NAME` 指定 `<project>` 目录名（须与 `CLAUDE_CONFIG_DIR` 同用）。默认保留 30 天（`cleanupPeriodDays`）。

**确定事实（cwd 限制）**
- 可从**任意目录** `claude --resume <session-id>`：先在当前项目目录及其 git worktree 查找，再在本机其它项目查找；跨项目搜索只在**恰好一个**其它项目持有该 id 的 transcript 时解析（v2.1.223 之前只在当前项目目录及 worktree）。worktree 不存在时在当前目录恢复；不相关项目会复制 `cd` + resume 命令到剪贴板。

**确定事实（与 Ageminal 注入的兼容）**
- 官方明列：`--mcp-config`、`--settings`、`--plugin-dir`、`--fallback-model`、`--add-dir` **不会**从原启动恢复，恢复时**需再次传入**；标准 `settings.json` / `settings.local.json` 会重新读取。→ Ageminal 用 `--settings` 按会话注入 hooks 的机制**与 resume 天然匹配**（每次都重新传），但**恢复命令必须带上 `--settings`**，否则该次恢复不会回调事件。

**已知坑**
- 同一会话在两个终端恢复且不 fork，消息会交错写入同一 transcript。
- `-c` 会跳过 `-p`/SDK 会话；若 Ageminal 曾用 `claude -p` 起过会话，`-c` 未必选到它，应优先用显式 id。
- 复制出来的重复 transcript 会让 id 搜索报 `No conversation found with session ID`，而不是随便挑一个。

**来源**：<https://code.claude.com/docs/en/sessions> · <https://code.claude.com/docs/en/cli-reference> · <https://code.claude.com/docs/en/hooks>

---

### 3.2 Codex

**确定事实（resume 命令）**
- `codex resume [SESSION_ID]`：按 id 或会话名恢复交互会话，缺省打开选择器。
- `codex resume --last`：恢复**当前工作目录**最近一次；加 `--all` 才跨目录考虑。`resume` 接受与 `codex` 相同的全局 flags。
- `codex exec resume [SESSION_ID] [PROMPT]`：非交互续跑；支持 `--last` / `--all` / `--json` / `--output-schema`，`PROMPT` 为 `-` 时读 stdin。
- `codex fork [--last]`：分叉为新会话；TUI 内 `/resume`、`/fork`。
- `codex archive|unarchive <SESSION>`、`codex delete <SESSION>` 都接受 id 或会话名。

**确定事实（id 捕获）**
- hooks 公共输入字段：`session_id`（当前会话 id；子代理 hook 用父会话 id）、`transcript_path`（会话 transcript 路径，可为 null）、`cwd`、`hook_event_name`、`model`；turn 级事件另带 `turn_id`。`SessionStart` 的 matcher 值为 `startup|resume|clear|compact`。
- 传统 `notify` 载荷字段：`type`（恒为 `agent-turn-complete`）、`thread-id`、`turn-id`、`cwd` 等。
- `SessionEnd` 示例载荷：`{ "session_id": "thr_123", "transcript_path": "...", "cwd": "...", ... }`。

**确定事实（作用域与持久化）**
- 会话 id 是 UUID 或会话名；官方把 `transcript_path` 作为程序化入口，并提示其格式不是稳定接口。
- `tui.resume_cwd` 引用「会话的 saved directory」，说明每个会话记录了工作目录。配置文件 `~/.codex/config.toml`，`CODEX_HOME` 可整体迁移。
- ⚠️ 社区生态一致报告的落盘布局为 `~/.codex/sessions/YYYY/MM/DD/rollout-<timestamp>-<uuid>.jsonl`，另有 `history.jsonl` / SQLite 索引；**官方文档本轮未直接给出该目录格式**，建议以 hook 的 `transcript_path` 为准（权威、按会话给路径）。

**确定事实（cwd 限制）**
- `codex resume` 的 `--last` **限当前工作目录**，除非 `--all`。
- 若当前目录与会话保存目录不一致，Codex **会询问用哪个目录**；设 `tui.resume_cwd = "current"` 或 `"session"` 可免询问复用选择；显式 `--cd/-C` 优先级最高。`codex fork` 同理。

**确定事实（与注入兼容）**
- `codex resume` 接受与 `codex` 相同的全局 flags，因此 `-c notify=[...]`、`-c hooks...`、`--dangerously-bypass-hook-trust` 可与 resume 同用；`tui.resume_cwd` 也可用 `-c` 覆盖。hooks 信任按当前定义 hash 记录，命令变更需重新信任或 bypass。

**已知坑**
- `--last` 作用域是 cwd，跨多仓库容易续到别的会话。
- 归档（archived）会话不能被 resume/fork，需先 `codex unarchive`（⚠️ 归档机制来自生态文章，官方 changelog 未在本次逐条核对）。
- 批处理里若目录不一致，交互式询问会**卡住自动化** → 必须设 `tui.resume_cwd` 或 `--cd`。

**来源**：<https://developers.openai.com/codex/cli/reference>（`.md` 版）· <https://developers.openai.com/codex/hooks> · <https://developers.openai.com/codex/config-reference>（`tui.resume_cwd`、`history.persistence`）· 生态佐证：`codex-sessions`、`cxresume`、`agent-history` 等（仅用于落盘布局）

---

### 3.3 opencode（本机 v2.0.8 实测）

**确定事实（resume 命令，本机 `--help`）**
- `opencode --continue/-c`：续最近会话；`--session/-s <id>`：续指定会话；`--fork`：续的时候分叉。
- `opencode run -c | -s <id> | --fork`：非交互续跑。
- `opencode attach [url] -c | -s <id>`：把 TUI 接到已运行 server 的会话。
- `opencode session list [--max-count N] [--format table|json]`（只列**当前项目**的顶层会话）、`opencode session delete <id>`、`opencode export [id]`、`opencode import`。

**确定事实（id 捕获）**
- v2 插件事件 `session.created` 的 `data` 结构含 **`sessionID`**、`projectID`、`location.directory`（+ 可选 `workspaceID`）、`parentID`、`slug`、`title`、`agent`、`model`、`version`。本机 `@opencode/schema@…`（`@opencode/plugin` 2.0.9）类型定义实证。
- 另有 `opencode session list --format json`、`opencode run --format json` 的事件流，以及 server 的 SSE `/event`。
- ⚠️ **v2 插件 API 与官网文档不一致**：官网 `/docs/plugins/` 描述的仍是 v1 的「插件函数返回 `{ event: async ({event}) => {} }` 钩子」；本机 v2 包使用 `define({ id, setup(context) })`，事件订阅是 `context.event.subscribe(...)`（`EventDomain extends Pick<EventApi,"subscribe">`）。**id 捕获实现必须按目标版本核对**，不能照抄 v1 文档的 `event` 钩子。

**确定事实（作用域与持久化）**
- id 形如 `ses_...`，是文本主键。本机 v2 数据目录 `~/.local/share/opencode/`：`opencode.db`（SQLite）+ `storage/`、`snapshot/`、`worktree/` 等。DB 内 `session` / `session_v2` 表均有 `id`、`project_id`、`directory`、`parent_id`、`time_created/updated` 等列；`session_v2` 另有 `fork_session_id`、`resume_attempts`、`time_suspended`（内部后台恢复用，非 CLI resume 语义）。
- `opencode session list` 文案为「List top-level sessions in the current project」→ **会话按项目归属**（本机观察到 `session_v2.project_id` 为 worktree 派生的哈希，`directory` 是会话创建目录）。

**cwd 限制**
- ⚠️ **未找到官方明示**「`-s` 是否必须在原目录启动」。结合「项目作用域 + 会话带 `directory` 字段」的实证，**[建议] Ageminal 一律在记录到的 worktree/directory 中启动**：`opencode <dir> -s <id>`（TUI 位置参数）或 `opencode run --dir <dir> -s <id>`。跨项目按 id 恢复应实测。

**确定事实（与注入兼容）**
- `OPENCODE_CONFIG_DIR` 是环境变量，插件在该目录被加载；与 `-s`/`-c` 正交，恢复时插件照常生效。

**已知坑**
- v2 仍在 beta，插件 API / 事件类型可能变；本机已见到 v2 与官网 v1 文档的差异。
- `--fork` 产生新会话 id，恢复后要更新页签记录的 id。

**来源**：本机 `opencode v2.0.8 --help` / `opencode run --help` / `opencode session --help`；本机 `~/.config/opencode/node_modules/@opencode/schema`、`@opencode/plugin@2.0.9`、`@opencode/client` 类型定义；`~/.local/share/opencode/opencode.db` schema 实测 · <https://opencode.ai/docs/cli/> · <https://opencode.ai/docs/plugins/> · <https://opencode.ai/docs/server/>

---

### 3.4 pi（`@earendil-works/pi-coding-agent`）

**确定事实（resume 命令）**
- `pi -c/--continue`：续最近会话；`pi -r/--resume`：浏览选择；`pi --session <path|id>`：用指定会话文件或**部分 UUID**；`pi --fork <path|id>`：分叉为新会话；`pi --session-dir <dir>`：自定义会话存储根；`pi --name/-n <name>`。
- 会话内 `/resume`、`/tree`、`/fork`、`/clone`、`/session`。

**确定事实（id 捕获）**
- 扩展事件 `session_start` 载荷：`reason`（`startup|reload|new|resume|fork`）、`previousSessionFile`（`new`/`resume`/`fork` 时存在）；`session_shutdown` 载荷：`reason`、`targetSessionFile`。**事件本身不带 session id**。
- 取 id 的正确途径：`ctx.sessionManager.getSessionId()`（UUID）/ `getSessionFile()`（路径）；`/session` 命令展示会话文件、id、消息数、tokens、cost。

**确定事实（作用域与持久化）**
- 路径：`~/.pi/agent/sessions/--<path>--/<timestamp>_<session-id>.jsonl`；`<path>` = cwd 去掉前导分隔符并把 `/`、`\`、`:` 替换为 `-`。`<session-id>` 默认 UUID，可经 `--session-id`/SDK 自定义。
- `SessionManager.list(cwd, sessionDir?)` 按目录列，`listAll()` 跨全部；`getCwd()` / `getSessionDir()` / `getSessionId()` / `getSessionFile()` 可读。

**cwd 限制**
- 会话按 cwd 分桶；`-c` 取当前项目最近会话。`--session <path|id>` 可给**文件路径**从而跨目录打开；给**部分 id** 时会在哪个范围搜索、是否跨目录，**本轮未找到官方明确说明** → ⚠️ 待实测。**[建议]** 恢复时在记录 worktree 中启动，或直接用记录的 `session file` 绝对路径。

**确定事实（与注入兼容）**
- `pi -e <扩展>`（`--extension`）与 `--session/-c/-r` 可同用；`-e` 在项目信任判定**之前**加载，故不受项目 trust 影响。

**已知坑**
- 扩展若想上报会话 id，必须在 `session_start` 里读 `ctx.sessionManager`（事件参数没有 id）；`session_shutdown` 的 `targetSessionFile` 用于切换/恢复目标。
- 移动/重命名 worktree 会改变 cwd 转义目录，历史会话落在旧桶里。

**来源**：<https://pi.dev/docs/latest/sessions> · <https://pi.dev/docs/latest/usage> · <https://pi.dev/docs/latest/session-format> · <https://pi.dev/docs/latest/extensions>（对应仓库 `packages/coding-agent/docs/{sessions,usage,session-format,extensions}.md`）

---

### 3.5 dsh（DeepSeek Harness，仅记录现状）

**确定事实**
- **核心/SDK 有 resume**：`ctx.agents.resume({ resumeSessionId, agentOptions, setup })` 从持久化日志重建历史并继续；声明式配置里 `agents[].resumeSessionId: <existing session id>`；`sessionId` **全局唯一**；`resumeSessionId` 与 `sessionId` 互斥；resume 依赖持久化后端，无后端会明确报错。持久化形态为 JSONL 事件日志。
- **启动器无一等 resume**：`dsh` 是 profile 启动器，`dsh --profile <name>`；启动器只解析自己的 flags（`--profile`、`--patch`、`--dump-config` 等），**第一个不认识的 token 起交给 profile app**。官方 README 原文示例：`dsh --profile tui --resume <id>  # example, assuming the tui profile is installed; --resume belongs to the terminal app`。
- 官方内置 profile：`base`、`web`、`headless`、`sdk`、`sdk-minimal`、`acp`；`headless` 的定位是「Run one fresh persisted session, print the final answer, and exit」（**fresh**，未见 resume flag）。官方无可交互终端 TUI（`dsh web` 是浏览器入口）。
- 社区 TUI `dsh-cli`（`dsh.fish/a/dsh-cli`）提供「自动恢复当前目录最近会话」与 `/resume <id>`、`/resume`、`/list`、`/new`。

**结论**：issue 说的「dsh 暂不支持」应精确表述为——**dsh 核心与 SDK 支持按会话 id resume，但官方 CLI 没有稳定的、带 resume 的终端入口；要用 resume 必须选社区 TUI 或走 SDK profile。** Ageminal 首版若不做 dsh，降级文案应写「dsh 无官方可恢复终端入口」。

**来源**：<https://github.com/deepseek-ai/deepseek-harness/blob/master/apps/cli/README.md> · <https://deepseek-harness.github.io/deepseek-harness/en/reference/subsystems/session> · 本机 `dsh 0.1.0-rc.6 --help` · 社区 TUI <https://dsh.fish/a/dsh-cli>

---

## 四、给 Ageminal 的「会话恢复流程」建议

> 前提与 [`session-lifecycle-references.md`](./session-lifecycle-references.md) 一致：页签/会话身份由 Ageminal 自己持有（建议照 orca：稳定 string `sessionId` + 每次 spawn 的 `incarnationId`），**agent 会话 id 只是页签的一个属性**，不是页签身份。

### 4.1 页签如何记住 agent 会话 id（[建议]）

给每个终端页签的持久化记录增加：

| 字段 | 含义 | 来源 |
| --- | --- | --- |
| `agent` | `claude`/`codex`/`opencode`/`pi`/`dsh` | 启动时 Ageminal 已知 |
| `agentSessionId` | agent 的对话会话 id | 由注入事件通道上报（见下） |
| `agentSessionFile` | transcript/session 文件绝对路径（可选，pi/Claude 更稳） | Claude `transcript_path`；pi `getSessionFile()`；Codex `transcript_path` |
| `agentCwd` | 会话所属 worktree/cwd | Claude/Codex hook 的 `cwd`；opencode `location.directory`；pi 用启动时 cwd |
| `agentResumeKind` | `id` / `file` / `continue` / `none` | 依 agent 能力 |
| `capturedAt`、`lastAgentEvent` | 时间戳/最近事件，判断陈旧 | 事件通道 |

**上报取法（复用 [`agent-injection.md`](./agent-injection.md) 的注入通道）**：

- **Claude**：`SessionStart` hook 的 `session_id` + `transcript_path` + `cwd`（`source` 为 `resume`/`fork` 可区分「本次是恢复」）。
- **Codex**：hooks `SessionStart` 的 `session_id` + `transcript_path`；若只挂了 `notify`，从 `thread-id` 取（但只在回合结束才有）。
- **opencode**：插件订阅 `session.created`（v2：`define`/`setup` + `context.event.subscribe`），取 `event.data.sessionID` + `event.data.location.directory`。
- **pi**：扩展在 `session_start` 里取 `ctx.sessionManager.getSessionId()` / `getSessionFile()`（事件无 id）。
- **dsh**：SDK `RunResult.sessionId`；或社区 TUI 无事件时用 `dsh` 会话日志解析（本轮未验证）。

**写入时机**：每次 `SessionStart`/`session.created`/`session_start` 都**幂等覆盖**（fork 后新 id 会自动纠正）；并在首个回合开始前就落盘，避免进程一启动即崩溃导致 id 丢失。

### 4.2 进程死亡后如何 resume 重启（[建议]）

1. 页签进程消失 → 标记 `exited`（保留页签 + overlay，照 orca），显示「恢复对话」与「新建会话」。
2. 恢复命令在 **`agentCwd`（worktree）** 目录中启动，并**重新拼装注入参数**（因为 agent 不会继承会话级启动参数）：

| Agent | 恢复命令模板（示意） |
| --- | --- |
| Claude | `claude --resume <agentSessionId> --settings <ageminal 会话 settings.json> [原 --mcp-config/=--plugin-dir/--add-dir]` |
| Codex | `codex resume <agentSessionId> -c 'tui.resume_cwd="session"' -c 'notify=[…]' [--dangerously-bypass-hook-trust]`（`-c` 仅为示意，需实测；或在 Ageminal 自有注入配置层） |
| opencode | `OPENCODE_CONFIG_DIR=<…> opencode <agentCwd> --session <agentSessionId>`（非交互：`opencode run --dir <agentCwd> -s <id>`） |
| pi | `pi --session <agentSessionFile 或 agentSessionId> -e <ageminal 扩展>` |
| dsh | 官方终端无入口 → 不提供恢复；或走 SDK `agents.resume({ resumeSessionId })`（另属产品形态） |

3. **恢复时生成新的 `incarnationId`**，沿用原页签/逻辑 `sessionId`（对齐 [`session-lifecycle-references.md`](./session-lifecycle-references.md) 的世代策略）。恢复后 `SessionStart(source=resume)` 会再次上报**同一个** agent 会话 id，可用于校验恢复是否真的接上了。
4. **默认不加 fork**；提供显式「从此刻分叉」入口，若选 fork，等事件上报新 id 后**覆盖** `agentSessionId`。
5. **恢复失败要可检测**：Claude 会报 `No conversation found with session ID: …`；Codex/pi 也有对应 not-found 行为。捕获退出码/stderr，转入降级。

### 4.3 无 resume 能力 / 恢复失败时的降级（[建议]）

- **明确区分两件事**（避免用户误期待）：Ageminal 能恢复的是 **agent 对话**；**不能**恢复已死进程的终端屏幕。UI 文案建议：「无法恢复对话（agent 不支持或未记录会话 id）；可新建会话。原终端输出仅保留在回放/转录中。」
- 触发降级的条件：
  1. agent 无 resume（dsh 官方终端形态）；
  2. 该页签**从未捕获** `agentSessionId`（如启动早期崩溃、未启用集成）；
  3. agent 报告会话不存在（被清理/归档/删除；Claude 默认 30 天保留期）；
  4. 版本/路径不匹配导致恢复报错。
- 降级动作：
  1. 禁用「恢复对话」按钮，仅留「新建会话」；`agentResumeKind = none`。
  2. 若**保留有 transcript/session 文件**：提供只读「查看转录」/「导出」，并在页签标注「原对话不可续，可新建后手动引用转录」。
  3. 显示可复制的恢复命令，便于用户手工在终端尝试。
  4. **不要让「新建会话」静默复用旧 agentSessionId**；新会话必须全新 id，避免把旧历史续进新上下文。

### 4.4 与现有注入设计的衔接

- [`agent-injection.md`](./agent-injection.md) 的「按会话注入」通道**已经提供了 resume 所需的两样东西**：会话 id 的上报（事件）与恢复时的参数注入。**无需新增全局写入**。
- 唯一必须补的契约：**「恢复」也是一次带注入的启动**，因此恢复路径要和首次启动走**同一套参数拼装代码**（否则会出现「恢复了但事件不再回调」的静默失败，Claude 的 `--settings` 尤其典型）。

---

## 五、不确定点与待实测

1. **opencode `-s` 的跨项目/cwd 行为**：官方未明示；需实测「在目录 A 创建会话，从目录 B `opencode -s <id>` 是否可解析」。本机 DB 显示会话带 `project_id` + `directory`，倾向项目作用域。
2. **pi 按部分 id 的搜索范围**：`pi --session <partial-id>` 是否只在当前 cwd 的会话桶里查，未找到明确文档；需实测。直接用 `session file` 绝对路径最稳。
3. **Codex `tui.resume_cwd` 能否用 `-c` 覆盖**：官方说明 `resume` 接受全局 flags，推断可以，但未逐字确认；自动化里应同时准备「写入 Ageminal 自有配置层」的兜底。
4. **Codex 精确落盘布局**（`~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`）：来自多个独立生态实现的一致报告，非本轮官方原文；程序化应以 hook 的 `transcript_path` 为准。
5. **opencode v2 插件 API 与官网文档的漂移**：本机 `@opencode/plugin@2.0.9` 与官网 v1 文档不同；id 捕获实现需按目标版本核对（可能影响既有 `agent-injection.md` 的 opencode 方案）。
6. **Claude/Codex/pi 未在本机实测**：本机仅装了 opencode 与 dsh；Claude/Codex/pi 的结论为官方文档证据，恢复命令的组合（尤其与注入参数同用）建议在 Windows 上做一次端到端验证。
7. **dsh 的终端 resume 入口**：官方无；社区 `dsh-cli` 非官方，是否纳入首批需产品决策（与 [`agent-injection.md`](./agent-injection.md) §6 的 dsh 交互面决策合并考虑）。

---

## 六、来源

**Claude Code**
- Manage sessions（resume/continue、cwd 查找、恢复时哪些 flag 不继承、存储路径、保留期）：<https://code.claude.com/docs/en/sessions>
- CLI reference（`--resume`/`-r`、`--continue`/`-c`、`--fork-session`、`-p --resume`）：<https://code.claude.com/docs/en/cli-reference>
- Hooks reference（公共输入字段 `session_id`/`transcript_path`/`cwd`、`SessionStart` 的 `source` 与恢复字段）：<https://code.claude.com/docs/en/hooks>

**Codex**
- CLI reference（`codex resume`、`--last/--all`、`exec resume`、`fork`、`tui.resume_cwd`、cwd 询问）：<https://developers.openai.com/codex/cli/reference>（Markdown 版 `…/reference.md`）
- Hooks（公共输入 `session_id`、`SessionStart` matcher `startup|resume|clear|compact`、`transcript_path`）：<https://developers.openai.com/codex/hooks>
- Config reference（`tui.resume_cwd`、`history.persistence`）：<https://developers.openai.com/codex/config-reference>
- 落盘布局生态佐证（`~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`、`history.jsonl`）：`codex-sessions`、`cxresume`、`agent-history`、`agent-ouija` 等第三方解析器（仅作旁证）

**opencode**
- CLI（`--continue/-c`、`--session/-s`、`--fork`、`session list`）：<https://opencode.ai/docs/cli/>
- Plugins（v1 事件与插件形态）：<https://opencode.ai/docs/plugins/>
- Server（`/session` API、事件流）：<https://opencode.ai/docs/server/>
- 本机实证：`opencode v2.0.8` CLI help；`@opencode/schema` 的 `session.created.data.sessionID`；`@opencode/plugin@2.0.9` 的 `define`/`setup`/`EventDomain`；`~/.local/share/opencode/opencode.db` 的 `session`/`session_v2` schema

**pi（`@earendil-works/pi`）**
- Sessions（`pi -c/-r/--session/--fork`、存储路径、`/resume`）：<https://pi.dev/docs/latest/sessions>
- Using Pi（CLI Session Options 表、`--session-dir`、cwd 分桶）：<https://pi.dev/docs/latest/usage>
- Session Format（文件路径与 `<session-id>`、`getSessionId()`/`getSessionFile()`、列表 API）：<https://pi.dev/docs/latest/session-format>
- Extensions（`session_start`/`session_shutdown` 载荷、`ctx.sessionManager`）：<https://pi.dev/docs/latest/extensions>

**dsh（DeepSeek Harness）**
- CLI README（启动器只解析自有 flags、`--profile`、`--resume` 属 app 的示例）：<https://github.com/deepseek-ai/deepseek-harness/blob/master/apps/cli/README.md>
- Session 子系统（`agents.resume({resumeSessionId})`、`sessionId` 全局唯一、互斥）：<https://deepseek-harness.github.io/deepseek-harness/en/reference/subsystems/session>
- 社区 TUI `dsh-cli`（`/resume <id>`、自动续当前目录最近会话）：<https://dsh.fish/a/dsh-cli>
- 本机 `dsh 0.1.0-rc.6 --help`

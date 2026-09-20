# 会话标识与终端生命周期参考核实（orca / VS Code / Zed / Contour）

> **状态回写（2026-09-20）**：**已定 —— B5（#8）**。daemon 状态（`running`/`exited`，与 shell-ready 分离）与 UI 状态对齐 VS Code `ProcessState`；**启动对账四象限**——**绝不从「list 里没有」推断进程已死**；**Detach ≠ Terminate**；会话标识 = `string` 逻辑 id + 每次 spawn 的 `incarnationId`（迟到 exit 事件按世代丢弃）。

> 调研日期：**2026-09-19**
> 对应：wayfinder issue **#45**（父票 **B5 #8**「页签↔PTY 生命周期与重连协议」）。目标：核实 B5 中**属于外推**的会话标识、启动对账、退出行为、状态模型、Zed 可借鉴点、多客户端尺寸仲裁，对照真实项目源码，避免凭空拟方案。
> 记号：**确定事实**来自源码 / 官方文档，附路径+行号或 URL；**⚠️推测**为无直接来源的推断；**[建议]** 为本文给 B5 的提案（非既定事实）。
> 证据克隆（本地只读）与 HEAD：
> - orca `/tmp/opencode/refs/orca` @ `b0ec11f5b0b88ffb0771f071fb302bae51589536`（2026-09-19）
> - VS Code `/tmp/opencode/refs/vscode` @ `d3c24c3c4d7efab36b37acb1e91d9502c0c35b28`（2026-09-19，sparse checkout 终端相关目录）
> - Zed `/tmp/opencode/refs/zed` @ `916fc2b8cb3a815cbef4a3b40e13081be72036b6`（2026-09-19）
> - Contour：`https://raw.githubusercontent.com/contour-terminal/contour/master/...`（未克隆，按需抓原文）
> 关联既有调研：[`session-daemon.md`](./session-daemon.md)、[`daemon-ipc-protocol.md`](./daemon-ipc-protocol.md)、[`reference-implementations.md`](./reference-implementations.md)、[`zed-terminal.md`](./zed-terminal.md)。

---

## 一、结论（TL;DR）

1. **四个项目对「会话标识」收敛到两种范式，且都不把「连接句柄」当会话 id。**
   - **持久逻辑 id**：orca 用 `string`（`${worktreeId}@@${uuid8}`，主进程生成、daemon 沿用），VS Code 用 `number`（pty host 内 `++_lastPtyId`，每个 pty host 生命周期内自增），Contour 用 `uint64`（daemon 内 `_nextSessionId = 1` 单调自增）。三者都由**持有 PTY 的那一侧（或主进程）**生成，不是 UI 每次连接现取。
   - **世代标识（incarnation）**：orca 额外为每次 spawn 生成 `incarnationId = randomUUID()`，用来区分**同一个 sessionId 的不同进程世代**，防止迟到的 exit 事件误伤新进程。VS Code 没有显式 incarnation，而是靠 `_lastPtyId` 单调不复用 + `ProcessState` 间接表达。**这是 B5 最值得照抄的一处。**
   - **Zed 完全没有会话标识**：终端身份 = GPUI 实体 `item_id`（运行时 u64），持久化只存 `working_directory`。

2. **启动对账：orca 的四象限最完整，VS Code 是「三象限自动 + 孤儿手动」。**
   orca 有显式的孤儿采纳协议 `terminal.adoptOrphans`（claim/retain/remove 三态），并要求 daemon 显式给出 `orphaned: true` + `ptyId` + `incarnationId` 才算「强身份」；VS Code 的 `listProcesses` 只列「无前端连接的进程」，自动恢复走 layout + `attachToProcess`，**孤儿由用户用「Attach to Session」命令手动挑**。Contour 用「结构对账」而非按 session id。Zed 无。

3. **退出行为三家都不同，但都区分「正常退出」与「异常/启动失败」。**
   VS Code 默认**关页签**（仅 `waitOnExit` / `showExitAlert` 改变）；orca 分场景：异常本地进程 → **保留 pane + overlay（含 exit code + Restart/Close）**，用户主动 `exit` → 关页签；Zed interactive shell **默认关页签**（仅 spawn 失败且非零码时保留给用户看错误），task 终端保留并把 summary（含成功与否）写进缓冲 + 提供 Rerun。

4. **状态模型可拼出一套「daemon 侧 + UI 侧」的组合。** daemon 侧照 orca 的 `SessionState`（实际只用 running/exited）+ `ShellReadyState`（4 态）；UI 侧照 VS Code 的 `ProcessState`（6 态，含 `KilledByUser` / `KilledDuringLaunch`）+ `TerminalExitReason`（5 态）；task 型终端照 Zed 的 `TaskStatus`（`Unknown/Running/Completed{success}`）。注意 orca 的 `SessionState` 类型声明了 5 个值但**当前仅 running/exited 被赋值**。

5. **Zed 的可借鉴点有限但明确：** `Event::CloseTerminal` + `TerminalBackendEvent::ChildExit(ExitStatus)` 的事件模型、`TaskStatus` 三态、以及「interactive shell 用 `keyboard_input_sent` 区分用户退出 vs spawn 失败」这条判定。它**没有**会话标识、持久化、重连、恢复——不能作为 B5 这些方面的先例（与 [`zed-terminal.md`](./zed-terminal.md) §四一致）。

6. **Contour `ClientSizePolicy` 是现成可抄的多客户端尺寸规则**：`latest`（默认）/`smallest`/`largest`；后两者**按轴（行、列独立）合并**；tmux 的 `manual` **故意缺席**。注册表按「客户端流订阅」为键，detach 后该客户端尺寸不再计票。

---

## 二、逐项目实证

### 2.1 orca（Electron；daemon 持有 PTY + xterm-headless）

#### (1) 会话标识

**确定事实：**
- 生成函数 `mintPtySessionId`：`src/main/daemon/pty-session-id.ts:21-26` ——
  `worktreeId ? `${worktreeId}@@${randomUUID().slice(0,8)}` : randomUUID()`，即 **string**，格式 `${worktreeId}@@${8位十六进制}`，分隔符来自 `PTY_SESSION_ID_SEPARATOR`。注释（`:13-19`）说明该 `${worktreeId}@@` 前缀是给启动对账用的：`reconcileOnStartup` 靠 `@@` 切出会话归属的 worktree。
- 生成时机：主进程 IPC 预检 `src/main/ipc/pty/runtime/spawn-preflight.ts:110`、`src/main/ipc/pty/ipc/spawn-preflight.ts:32,172`；daemon 侧在缺少 `opts.sessionId` 时兜底 `src/main/daemon/daemon-pty-session-spawn.ts:30`。→ **主进程（orcad/Electron main）生成，daemon 复用**。
- 解析：`parsePtySessionId`（`src/shared/pty-session-id-format`，由 `pty-session-id.ts:11` 再导出）；`reconcileOnStartup` 靠它判断会话是否属于仍存在的 worktree（`daemon-pty-process-inspection.ts:134-140`）。
- **世代 id**：`incarnationId = randomUUID()`，`src/main/providers/local-pty-spawn.ts:40`。daemon 侧存于 `sessionIncarnations: Map<string,string>`（`daemon-pty-runtime-state.ts:114`）。exit 事件带 `incarnationId`，若与当前世代不符则**丢弃**（`daemon-pty-adapter.ts:88-99`；`clearExitedSessionState` 也先比对，`daemon-pty-runtime-state.ts:157-162`）。`ListSessionsResult`/`SessionInfo` 暴露 `sessionId`、`incarnationId?`、`state`、`isAlive`、`terminalHandle?` 等（`types.ts:354-382`）。
- **跨重启复用**：会话 id 不写入独立「会话表」，而是**隐式持久化在 `terminal-history/<worktreeHash>/<sessionDir>/` 目录名**（`terminal-history-paths.ts`、`history-paths.ts`；`meta.json` 记 `startedAt/endedAt/exitCode`，`terminal-history-metadata.ts:12-19`）。若 daemon 未重启，UI 重启后按**同一 sessionId** reattach；若 daemon 重启（PTY 已死），UI 侧旧 `ptyId` 与 daemon 新状态对不上，走 rebind（ptyId 变更属正常，见 `web-session-terminal-orphan-recovery-inventory.ts:203-219` 注释 #11495）。
- **避免重启后 id 冲突**：8 位随机十六进制后缀（32 bit）已大幅降低碰撞；真正的世代歧义由 `incarnationId` 兜底。⚠️推测：这并非严格的全局唯一保证，而是「随机 + 世代」的工程折中。

#### (2) 启动对账（四象限）

**确定事实：**
- daemon 侧启动对账 `reconcileOnStartup(validWorktreeIds)`（`daemon-pty-process-inspection.ts:105-160`）：调 `listSessions`，对每个 `isAlive` 会
  - `parsePtySessionId` 切出 worktreeId；`worktreeId === null` 或不在 `validWorktreeIds` → `kill`（判为孤儿）；
  - 否则收入 `alive`、登记 checkpoint、`reconcileLiveSessionHistory`。
  - 注释 `:109-116` 警告：sessionId 内嵌 spawn 时的 worktree 路径，**重命名 worktree 会保留旧 id**，必须把 `priorWorktreeIds` 一起喂进 `validWorktreeIds`，否则误杀。
- UI 侧采用协议：`RuntimeTerminalOrphanAdoptionRequest`（`runtime-terminal-contracts.ts:100-141`）含 `worktree`、`expectedTopologyRevision`、`claims[]`（每项 `terminal(handle)`、`ptyId`、`incarnationId`、`tabId`、`leafId`）、可选 `topology`。RPC 名 `terminal.adoptOrphans`（`shared/rpc-contract/terminal-orphan-params.ts:87-105`）。
- daemon 标记孤儿：`RuntimeTerminalSummary`（`runtime-terminal-contracts.ts:21-43`）含 `handle / ptyId / incarnationId? / orphaned? / connected`；`orca-runtime-build-pty-terminal-summary.ts:28-60` 里 `orphaned = !ptyHoldsRecordedSurface(...)`，孤儿行的 `tabId/leafId` 合成为 `pty:<ptyId>`。
- UI 三态裁决（`web-session-terminal-orphan-recovery-inventory.ts:121-260`）：对每个候选 surface
  - `terminal.orphaned === true` 且 handle/ptyId/incarnationId/worktree 全部强一致 → **`claim`**（采纳，两边都有）；
  - 无匹配 live PTY，但 `surface.pending` / `listed.truncated` / `hostScopeUnverifiable` → **`retain`**（不删）；只有两次权威 inventory 都缺席才 **`remove`**（`confirmSurfaceInventoryAbsence`，`:157-168`）；
  - `pending` 且 `terminal.ptyId !== surface.expectedPtyId` → **`retain` 即 rebind**（`#11495`：ptyId 跨 host relaunch 变化是常态，**handle 才是身份**）；
  - 重复 handle / 非当前 topology → **`retain`**（防误删）。
- 「仅 UI 有」：`prepareTerminalOrphanRecovery`（`...-surface.ts:102-167`）区分 `candidates`（UI 有 handle）、`unresolved`（UI 有 tab/leaf 但无 handle）、`observed`（host frame 已 ready）；`offTree`（leaf 已不在布局树）一律 `retain`。
- 「仅 daemon 有」：host frame 带 `orphaned` 行；UI 未声明 tab 的孤儿不会被自动删除（daemon 侧由 `reconcileOnStartup` 按 worktree 杀；UI 侧由 inventory `retain`/`remove`）。

#### (3) 会话退出后的 UI 行为

**确定事实：** 分场景（`terminal-pane/pty-connection/pty-exit-hibernate.ts:242-344`）：
1. `isUnverifiedExit`（host-loss sentinel）→ 保留 tab、`markUnverifiedPtyLoss`、不关（`:242-248`）；
2. `isSuppressedExit`（用户主动 Restart / hibernation 唤醒）→ 保留 pane、重连（`:249-294`）；
3. `failedLocalProcess = !connectionId && runtimeEnvironmentId===null && exitCode!==0` → 调 `onPaneProcessDied`，弹出 **`TerminalProcessExitOverlay`**（`:296-307`）；
4. 单 pane：若「新 spawn 且用户从未输入」→ 保留死 pane 让错误可见；否则 `onPtyExit`（`:308-326`）；
5. 多 pane → `manager.closePane(paneId)`（只关该 pane，`:343`）；
- **overlay 明确显示 exit code + Restart + Close**：`TerminalProcessExitOverlay.tsx:19-63`（文案 `The shell process ended with exit code {{code}}. Its output is preserved.`）。
- tab 级 `handlePtyExit`（`use-terminal-close-actions.ts:22-40`）：非「已证实进程退出」（`isProvenProcessExit`，`shared/terminal-exit-cause.ts:139-141`）→ 不关；否则 `closeTerminalTab(tabId, {reason:'pty-exit'})`。
- Restart 实现：`use-terminal-pane-process-exit-actions.ts:60-192`（销毁旧 transport，`connectPanePty` 原地重连）。
- **exit code 落盘**：`SessionMeta.exitCode`（`terminal-history-metadata.ts:12-19`）。

#### (4) 生命周期状态模型

**确定事实：**
- `SessionState = 'created' | 'spawning' | 'running' | 'exiting' | 'exited'`（`daemon/types.ts:42`）。
- `ShellReadyState = 'pending' | 'ready' | 'timed_out' | 'unsupported'`（`daemon/types.ts:44`）。
- ⚠️注意：当前 `Session` 类只在 `:32` 初始化为 `'running'`，在 `:313/:330/:373` 置 `'exited'`；**`created`/`spawning`/`exiting` 在全仓生产代码中未被赋值**（grep 只命中类型定义）。所以实际可达状态集是 {running, exited}，其余是预留/历史。
- daemon 自身另有一层 `'running' | 'idle-shutdown-pending' | 'shutting-down'`（`daemon-server-lifecycle.ts:28`）。

#### (5) 持久化配对键：checkpoint `generation`

**确定事实：**
- `checkpoint.json` 字段含 `generation?`、`pendingOutputSeq?`（`daemon-checkpoint-file.ts:9-28`）。
- `output.log` 头部 = `magic 'OCKL' + u8 formatVersion + u32le generation`（`terminal-history-log.ts:3-16`）。
- `TerminalHistorySessionWriter.checkpoint()`：`generation = (logGeneration ?? 0) + 1`，写 checkpoint 与 log 头（`terminal-history-session-writer.ts:98-106`）；`generation` 是**快照↔增量日志的配对键**，不是会话 id，也不是全局 epoch。
- 崩溃残尾检测 / seq 断档拒绝重放见 [`reference-implementations.md`](./reference-implementations.md) §2.4。

---

### 2.2 VS Code（pty host 是独立进程）

#### (1) 会话标识

**确定事实：**
- 类型是 **number**：`createProcess(...): Promise<number>`（`src/vs/platform/terminal/common/terminal.ts:320-332`）；`IPtyHostAttachTarget.id: number`（`:191-211`）；`detachFromProcess(id: number)`、`attachToProcess(id: number)`、`listProcesses()`、`orphanQuestionReply(id: number)`、`reviveTerminalProcesses(...)`（`:333-383`）。
- **生成方 = pty host 进程**：`private _lastPtyId: number = 0`（`ptyService.ts:121`），`createProcess` 内 `const id = ++this._lastPtyId`（`:336`）。pty host 由 `nodePtyHostStarter.ts:25-45` 以 `--type=ptyHost` fork 出独立进程。
- **跨重启是否复用**：**不复用数字 id**。分两种重启：
  - **窗口 reload / 重新加载**：pty host 进程仍在 → `_lastPtyId` 继续、同一 id 直接 `attachToProcess(id)` 重连（`terminalService.ts:475-495`，`terminalProcessManager.ts:290-299`）。
  - **整个应用退出再启动**（`enablePersistentSessions` + `persistentSessionReviveProcess`）：pty host 随之死亡。退出前 `serializeTerminalState` 把每个 pty 快照写入 **workspace 级 storage**（`localTerminalBackend.ts:183-187`；storage key `terminal.integrated.bufferState`，`terminalStorageKeys.ts:6-16`）；启动时 `reviveTerminalProcesses` **创建全新 pty（新 id）**，并用 `_revivedPtyIdMap: oldId → {newId, state}` 建立旧→新映射（`ptyService.ts:255-310`、`:562-568`、`:602-632`）。
- **避免重启后 id 冲突**：数字 id 在每个 pty host 实例内单调自增、从不回退复用；旧 id 只活在「已序列化状态」与内存中的 `_revivedPtyIdMap`，因此新 pty host 从 1 重新计数也不冲突。serialized state 带版本号（`version: 1`）做格式门禁（`baseTerminalBackend.ts:105-119`）。

#### (2) 启动对账（四象限）

**确定事实：** 两层存储：layout（tabs/background 的 id 骨架，内存于 pty host + workspace storage `terminal.integrated.layoutInfo`），buffer state（可恢复的终端快照）。
- 启动时 `_initializePrimaryBackend` 读 `enablePersistentSessions`，决定 `_reconnectToLocalTerminals()`（`terminalService.ts:277-318`）。
- `getTerminalLayoutInfo`（`localTerminalBackend.ts:325-369`）：若有 buffer state，则先 `reviveTerminalProcesses` 造新 pty，再把 layout 回灌 pty host，最后返回 `_proxy.getTerminalLayoutInfo`。
- `_recreateTerminalGroups`（`terminalService.ts:497-521`）：**只对 `t.terminal.isOrphan === true` 的条目重建页签**，再 `createTerminal({config:{attachPersistentProcess}})` → `attachToProcess(id)`（`terminalProcessManager.ts:290-299`）。
- `isOrphan` 计算：`_buildProcessDetails` 调 `persistentProcess.isOrphaned()`（`ptyService.ts:638-667`）；`_isOrphaned()`（`:987-1007`）——若 disconnect 宽限计时器已排程 → 直接 true；否则向 renderer 发 `onProcessOrphanQuestion`（`AutoOpenBarrier(4000)`），renderer 在 `localPty.handleOrphanQuestion` 里立即 `orphanQuestionReply(id)`（`localPty.ts:94-96`，经 `localTerminalBackend.ts:163`）；500 ms 内无回复即判孤儿。
- detach/attach 宽限：`PersistentTerminalProcess.detach` 排程 `_disconnectRunner1`（`reconnectConstants.graceTime`），`attach()` 取消计时器（`ptyService.ts:826-842`）；`LocalReconnectConstants = GraceTime 60000 / ShortGraceTime 6000`（`terminal.ts:865-874`），退出即 `shutdown(true)`。
- **四象限结论**：
  | 情况 | VS Code 处理 |
  |---|---|
  | 两边都有（layout + 存活孤儿） | 自动 `attachToProcess`，并把 `isOrphan` 判定为 false |
  | 仅 UI 有（layout 指向已死 pty） | `_expandTerminalInstance` catch → `terminal: null` → `filtered` 丢弃（`ptyService.ts:602-631`） |
  | 仅 daemon 有（孤儿，layout 未引用） | **不自动恢复**；`listProcesses()` 返回它（`ptyService.ts:415-422`，仅 `shouldPersistTerminal` 且 `isOrphan`），由命令 **`Attach to Session`** 手动列出并 attach（`terminalActions.ts:826-869`） |
  | 都无 | 不动作 |

#### (3) 会话退出后的 UI 行为

**确定事实：** `terminalInstance.ts:_onProcessExit`（`:1718-1804`）：
- `_exitCode = parsedExitResult?.code`；`exitMessage` 仅在非零码时生成，且包含 exit code 文案（`parseExitResult`，`:2858-2927`）。
- 若设置了 `waitOnExit` 且不是 `KilledByUser` → **保留终端**，打印 exit message，禁用输入，等「按任意键关闭」（`:1757-1781`）。
- 否则 → 若 `showExitAlert`（设置 `terminal.integrated.showExitAlert`，`terminal.ts:97`）且最后一次输入不是 Ctrl+D → 弹 `Severity.Error` 通知（含 exit code 消息与「疑难解答」动作），随后 **`dispose(TerminalExitReason.Process)` 关闭页签**（`:1782-1798`）。
- **默认行为：关闭页签**；无内置「重启」按钮（重新打开就是新终端）。
- `TerminalExitReason = Unknown/Shutdown/Process/User/Extension`（`terminal.ts:1081-1087`）。

#### (4) 生命周期状态模型

**确定事实：** `ProcessState`（`src/vs/workbench/contrib/terminal/common/terminal.ts:322-339`）：
`Uninitialized=1 → Launching=2 → Running=3`；异常分支 `KilledDuringLaunch=4`（bad shell/args）、`KilledByUser=5`（事件来自 VS Code）、`KilledByProcess=6`（shell 自己崩 / `exit`）。
赋值点：`terminalProcessManager.ts:365`（Launching）、`:414-415`（Running）、`:214`（KilledByUser）、`:705-712`（KilledDuringLaunch / KilledByProcess）。

---

### 2.3 Contour（C++23 daemon，native cells+deltas 协议）

#### (1) 会话标识

**确定事实：**
- 协议字段 `uint64_t session = 0; ///< Leaf only: the SessionId.`（`src/vthost/proto/Pdu.hpp:639`）；类型 `vtworkspace::SessionId`（`src/vthost/SessionHost.hpp:130,155,220` 等）。
- daemon 侧单调分配：`uint64_t _nextSessionId = 1;`（`src/vthost/SessionHost.hpp:509-510`），由 model 的 allocator 消费。
- **不跨 daemon**：官方文档原话「Sessions do not survive the daemon」（[persistent-sessions](https://contour-terminal.org/persistent-sessions/)；[vthost internals](https://contour-terminal.org/internals/vthost/)）。daemon 停止/重启 → 会话全部结束，`_nextSessionId` 从 1 重新开始。
- **避免 id 冲突**：不依赖 id 跨重启唯一，因为**客户端不按 session id 对账，而按结构对账**——文档：「the client reconciles structurally rather than per session id (`applyRemoteLayout`'s subtractive pass)」。会话移除通过 re-push 的 `LayoutState`（空 tab 列表也是信号），而非「沉默」。
- 没有把 sessionId 落盘持久化；会话生命周期终止于 `SessionHost::handleSessionExit`（`SessionHost.hpp:386`），由 shell 退出 / native `ClosePane` / tmux `kill-pane` 三个入口汇聚。

#### (2) 启动/attach 对账

**确定事实**（文档 + `SessionHost.hpp`）：
- attach 时服务端推「attach snapshot（`SessionState` + 每会话一个 snapshot Delta），随后 20 ms 防抖 Delta」。客户端 `NativeClient` 把会话镜像进 `RemoteScreen`，`applyRemoteLayout` 做「加、减，再对已稳定树做第三遍重排」的**结构化**对账。
- 「仅 daemon 有」在 tmux 路径靠 `capture-pane -peqJ -S -` 回放历史；native 路径靠 snapshot + history 行滚动。
- 没有像 orca/VS Code 那样的「UI 持久页签骨架 + 会话 id 对账」——daemon 自身持有 layout（`vthost::SessionHost` 拥有 workspace 模型），客户端是纯 projection。**这与 Ageminal「UI 持久化页签 + daemon 无布局」的前提不同**，不能整体照搬。

#### (3) 会话退出后的 UI 行为

**确定事实：** 文档明确：会话移除由 daemon → 客户端的 `LayoutState` re-push 表达；`ClosePane` 是客户端 → daemon 的意图。没有 per-tab exit-code overlay 或 Restart 概念（Contour 是终端复用器语义，pane 关闭即关闭）。**未找到** exit code 展示或重启入口的证据。

#### (4) 生命周期状态模型

**确定事实：** 没有对外暴露的会话状态枚举；状态体现在 `SessionHost` 的布局模型（tab/pane/window）与 `SessionStreamEvents` 事件（`sessionClosed/sessionResized/sessionOutput/...`，`SessionHost.hpp:78-114`）。**未找到**可抄的有限状态机枚举。

#### (5) 多客户端尺寸仲裁 `ClientSizePolicy`（本次重点）

**确定事实**（`src/vthost/ClientSizePolicy.hpp` 原文）：
- `enum class ClientSizePolicy : uint8_t { Latest, Smallest, Largest }`；拼写表 `ClientSizePolicyNames = { {"latest",Latest}, {"smallest",Smallest}, {"largest",Largest} }`。
- 命令行 `--size-policy`，**默认 `latest`**；命名与默认都对齐 tmux 的 `window-size`。
- `resolveClientArea(policy, areas)`：
  - `Latest`：取 `sequence` 最大的 `ClientArea.size`（`sequence` 是单调递增戳，**不是时间戳**，保证可复现）；
  - `Smallest` / `Largest`：**按轴独立合并**——`lines = min/max(lines)`，`columns = min/max(columns)`。注释原话：two clients can each be the wider and the shorter one, and picking a single client's area would then hand the other a grid it cannot show；tmux 同理独立合并轴。
  - `areas` 为空 → 返回 `nullopt`，调用方保留原尺寸（daemon 无客户端也必须有尺寸）。
- `manual` **故意缺席**：docs 原话「it needs a runtime verb to set the size with, and nothing here would drive one」。
- 注册表按「客户端 stream subscription」为键，所以 **detach 的客户端尺寸不再计票**，否则「一个已离开的客户端会把所有应用钉死在它的尺寸上」。尺寸变化通过 `SessionStreamEvents::sessionResized` 通知**所有**客户端，不再由发起 resize 的连接独自处理。
- 行为：比网格大的客户端 letterbox；比网格小的客户端显示一个 viewport（`contour::geometry::viewportOrigin`，tmux `tty_window_offset1` 规则：游标过视口后水平居中、垂直保持在最后可见行、两轴独立 clamp）。⚠️文档注明 wiring 到 render offset 与鼠标命中测试**尚未完成**，当前过小客户端从左上裁剪。

---

### 2.4 Zed（in-process，无 daemon）

#### (1) 会话标识 / 持久化 / 重连 / 恢复

**确定事实：**
- **没有会话标识**：全仓 `crates/terminal` 无 `TerminalId`；终端身份是 GPUI 实体 id。持久化只存 `item_id`（`persistence.rs:70,78`）与 `working_directory`（`persistence.rs:410-424`），task 终端不序列化（`:64-75`），恢复时**新开** shell（`:284-289`）。→ **无 reattach、无回放、无跨进程存活**（与 [`zed-terminal.md`](./zed-terminal.md) §4.3 一致）。
- remote 终端也只是**本地跑 ssh**（`crates/project/src/terminals.rs:608-642`），远端 PTY 由 sshd 管理，Zed 不持有远端会话状态。
- `crates/remote_server` 只服务远程编辑（`Cargo.toml:3` 自述 “Daemon used for remote editing”），无终端/PTY 代码。

#### (2) 生命周期状态模型（可借鉴）

**确定事实：**
- 对外 `Event`（`crates/terminal/src/terminal.rs:669-679`）：`TitleChanged / BreadcrumbsChanged / CloseTerminal / Bell / Wakeup / BlinkChanged / SelectionsChanged / NewNavigationTarget / Open`。**生命周期信号只有 `CloseTerminal`**。
- 内部 `TerminalBackendEvent`（`:723-738`）含 `Exit` 与 `ChildExit(ExitStatus)`；`ChildExit` → `register_task_finished(Some(exit_status))`（`:1696-1698`）。
- `Terminal` 结构用 `child_exited: Option<ExitStatus>` 记忆子进程退出（`:1536`）；`TerminalType::Pty{...} | DisplayOnly`（`:1493-1499`）区分真 PTY 与 headless。
- **`TaskStatus`（task/命令型终端的三态，可直接借鉴）**（`:1581-1605`）：
  `Unknown`（启动后未报 exit 就被关） / `Running` / `Completed { success: bool }`。
- **退出裁决逻辑**（`:3117-3189`）：
  - interactive shell（无 task）：`keyboard_input_sent ? true : child_exited.is_none_or(|e| e.code()==Some(0))`，为 true → `Event::CloseTerminal`（→ `terminal_view.rs:1231` `ItemEvent::CloseItem` 关页签）。注释区分「用户主动 exit（总是关，即便非零码）」与「shell spawn 失败（不关，让用户看错误）」。
  - task 终端：把 `task_summary`（成功与否、命令、退出码）`append_text_to_term` 写进缓冲（`:3160-3176`），并按 `HideStrategy::{Never,Always,OnSuccess}` 决定是否关（`:3178-3188`）；`rerun_button` 提供 Rerun（`terminal_view.rs:1088-1107`）。
- → **Zed 有可借鉴点的回答：有，但只在「事件模型 + TaskStatus 三态 + interactive-vs-spawn-failure 判定」；会话标识/持久化/重连/恢复一律没有。**

---

## 三、横向对照表

| 维度 | orca | VS Code | Contour | Zed |
|---|---|---|---|---|
| 会话 id 类型 | **string** `${worktreeId}@@${uuid8}` | **number** `++_lastPtyId` | **uint64** `_nextSessionId=1` | 无（GPUI `item_id`） |
| 谁生成 | 主进程（daemon 兜底） | pty host 进程 | daemon | — |
| 落盘持久化 id | 隐式（history 目录名） | 否（序列化状态里带旧 id，仅用于映射） | 否 | 仅 `item_id`+cwd |
| 跨 daemon/进程重启复用 | daemon 未重启→复用同 id reattach；daemon 重启→冷恢复 | reload 复用同一 number；全套重启→新 id + old→new 映射 | 不复用（会话不跨 daemon） | — |
| 世代/防冲突 | **`incarnationId=randomUUID()`** | 单调不回退 + `_revivedPtyIdMap` | 无（结构对账） | — |
| 启动四象限 | **完整 claim/retain/remove** | 三象限自动 + 孤儿手动 `Attach to Session` | daemon 持有 layout，客户端结构对账 | — |
| 仅 UI 有 | `remove`（需权威 inventory）/ 否则 retain | layout 项丢弃 | — | — |
| 仅 daemon 有（孤儿） | `orphaned:true` 显式标记，UI claim 或保留 | `listProcesses` 返回，用户手选 | LayoutState 空 tab 表达 | — |
| 退出后页签 | 异常崩溃→保留+overlay(exit code+Restart)；正常 exit→关 | 默认关（`waitOnExit`/`showExitAlert` 改变） | pane 关 | interactive 默认关；task 保留+summary+rerun |
| 显示 exit code | ✅ overlay + meta.json | ✅ 非零码通知/waitOnExit 文案 | 未找到 | ✅ task summary（interactive 不显示） |
| 重启入口 | ✅ Restart 按钮 | ❌（重开即新终端） | 未找到 | ✅ task Rerun |
| 状态模型 | `SessionState`(5 声明/2 实用)+`ShellReadyState`(4) | `ProcessState`(6)+`TerminalExitReason`(5) | 无对外枚举 | `TaskStatus`(3)+`Exit/ChildExit` |
| 多客户端尺寸 | 未查到 | 无 | **`ClientSizePolicy` latest/smallest/largest（按轴）** | 无 |

---

## 四、对 B5 各外推点的裁决（照谁 / 还是都不用）

> 说明：B5（#8）的正文只有范围描述、未见成稿的外推点清单，故本节按 issue **#45 正文列出的 6 项**逐项裁决；标 **[建议]** 的为提案。

1. **会话标识方案** → **[建议] 照 orca，而非 VS Code / Contour。**
   - 用**逻辑稳定 id（string，建议含 worktree 归属前缀）+ 每次 spawn 的 `incarnationId`（uuid）**。理由：B5 需要「UI 关闭后会话仍活、重开按 session id 重 attach」，这要求 id 跨 UI 生命周期稳定；Contour 的计数器方案明确不跨 daemon，不可照；VS Code 的 number 方案要靠 `_revivedPtyIdMap` 绕，是「id 不复用」的被动结果而非主动设计。
   - **id 冲突**：不要依赖自增计数器；用「随机/uuid + 显式 incarnation 比对」；daemon 启动对账时**从落盘的 session 目录重建**有效集（orca `reconcileOnStartup`），而不是从计数器恢复。
   - ⚠️注意与已定的 [`daemon-ipc-protocol.md`](./daemon-ipc-protocol.md) 帧头 `sessionId: u32` 的关系：若沿用 u32 帧内 id，建议**另设一个 string 逻辑 id**（落盘目录/UI 持久化用），帧内 u32 只在单连接内做句柄。**不要**把 u32 当持久身份。

2. **UI 启动对账（四象限）** → **[建议] 照 orca 的显式孤儿协议，MVP 可先照 VS Code 的「自动 attach + 孤儿手动」。**
   - 四象限必须都定义；孤儿必须由 **daemon 显式标记**（照 orca `orphaned`），**不要**用「本次 list 没看到」推断死亡——orca 为此专门有 `truncated` / `hostScopeUnverifiable` / 两次权威 inventory 的保护（`...-inventory.ts:157-168`）。这条对 B5 尤其重要：一次性 `ListSessions` 不完整时误删会话是不可逆的。
   - 「仅 UI 有」：保留页签骨架但标记为不可 attach（区别于「直接删」），给用户重开/关闭选择。
   - VS Code 的 `onProcessOrphanQuestion`/`orphanQuestionReply`/`attachToProcess` **不建议**照抄其 4 s 轮询语义做 MVP：它解决的是「不信任本进程记账、向 renderer 求证」的分布式问题；Ageminal 单机单 daemon 用「daemon 记账 + 显式 orphan 标记」更简单。可保留「心跳/宽限期」思想（VS Code `GraceTime 60s / ShortGraceTime 6s`）。

3. **会话退出后的 UI 行为** → **[建议] 照 orca 的分场景，而不是 VS Code 的一刀切关闭。**
   - 正常用户退出（有键盘输入 / exit code 0）→ 关页签；
   - 异常/崩溃/启动失败（本地、非零码、无连接）→ **保留页签 + overlay**，显示 exit code，提供 **Restart** 与 Close（照 orca `TerminalProcessExitOverlay`）；
   - 至少保留 exit code 与 restart 入口；VS Code 默认关闭且无重启入口，不满足「agent 崩溃后原地重试」的产品诉求。

4. **生命周期状态模型** → **[建议] daemon 侧照 orca（`running`/`exited` + `ShellReadyState` 独立 4 态），UI 侧照 VS Code `ProcessState`。**
   - 不要照 orca `SessionState` 里声明却未使用的 `created/spawning/exiting`；
   - UI 的 `KilledByUser` / `KilledDuringLaunch` 区分很重要（决定是否弹错误、是否保留页签）；
   - task/命令型终端（如未来的 agent 一次性任务）照 Zed `TaskStatus`。

5. **Zed 的相关方案** → **只借鉴 `TaskStatus` 与事件模型，其余（会话标识/持久化/重连/恢复）都不照。** Zed 是 in-process、只存 cwd，是「反例」而非先例（已在 [`zed-terminal.md`](./zed-terminal.md) §四/§7.2 记录，本次源码复核一致）。

6. **多客户端尺寸仲裁** → **[建议] 当前单客户端可不实现；将来实现时照 Contour。**
   - 若实现：`latest`（默认）/`smallest`/`largest`，**`smallest`/`largest` 按轴独立合并**，`manual` 不提供；注册表按客户端订阅为键，detach 即退出计票；尺寸变化广播给所有客户端。

---

## 五、尚存不确定点 / 未找到证据

- **orca `SessionState` 的 `created/spawning/exiting`**：类型声明存在，但当前生产代码未发现赋值（仅 `running`/`exited`）。⚠️可能是历史/预留，无法据此说 orca「实际建模了 5 态」。
- **Contour 的会话退出 UI 行为与 exit code 展示**：官方文档与本次抓取的头部源码中**未找到**；未克隆整仓逐文件核实。
- **VS Code `TerminalExitReason.Process` 之外的 UI 表现**（例如 task 终端 auto-close）未逐条追；本文只覆盖普通终端主路径。
- **orca 的 `handle` 生成细节**（`issuePtyHandle` 的格式/是否跨 reload 稳定）未逐行读；本文只确认 `handle` 与 `ptyId` 是**两个不同**的身份（`orca-runtime-build-pty-terminal-summary.ts:32,42`），孤儿场景 `tabId/leafId` 合成为 `pty:<ptyId>`。
- **Contour `viewportOrigin` 的渲染接线**官方文档自述「尚未完成」，故「过小客户端跟随游标」目前只是规则存在、未落地。
- **VS Code 「仅 daemon 有」是否在窗口 reload 时自动恢复**：本文确认自动恢复只走 layout 里的 `isOrphan` 项；layout 之外的孤儿只能手动 `Attach to Session`。未发现自动「收养未在 layout 中的孤儿」的代码。

---

## 六、来源

### orca（`github.com/stablyai/orca`，MIT；本地 `/tmp/opencode/refs/orca` @ `b0ec11f`）
- 会话 id：`src/main/daemon/pty-session-id.ts:11-26`、`src/main/ipc/pty/runtime/spawn-preflight.ts:110`、`src/main/ipc/pty/ipc/spawn-preflight.ts:32,172`、`src/main/daemon/daemon-pty-session-spawn.ts:30`
- 世代 id：`src/main/providers/local-pty-spawn.ts:40`、`src/main/daemon/daemon-pty-runtime-state.ts:114,157-162`、`src/main/daemon/daemon-pty-adapter.ts:67-99`、`src/shared/pty-incarnation.ts`
- 会话列表/状态：`src/main/daemon/types.ts:42,44,354-382`
- 状态机：`src/main/daemon/session.ts:32,313,330,373`
- 启动对账：`src/main/daemon/daemon-pty-process-inspection.ts:105-160`
- 孤儿协议：`src/shared/runtime-terminal-contracts.ts:21-43,100-141`、`src/shared/rpc-contract/terminal-orphan-params.ts:80-105`、`src/main/runtime/orca-runtime-build-pty-terminal-summary.ts:26-60`
- UI 对账：`src/renderer/src/runtime/web-session-terminal-orphan-recovery.ts`、`...-inventory.ts:121-260`、`...-surface.ts:102-167,273-287`
- 退出 UI：`src/renderer/src/components/terminal-pane/pty-connection/pty-exit-hibernate.ts:242-344`、`.../TerminalProcessExitOverlay.tsx:19-63`、`src/renderer/src/components/use-terminal-close-actions.ts:22-40`、`src/shared/terminal-exit-cause.ts:139-141`
- 持久化：`src/main/daemon/daemon-checkpoint-file.ts:9-28`、`src/main/daemon/terminal-history-session-writer.ts:98-106`、`src/main/daemon/terminal-history-log.ts:3-16`、`src/main/daemon/terminal-history-metadata.ts:12-19`

### VS Code（`github.com/microsoft/vscode`；本地 `/tmp/opencode/refs/vscode` @ `d3c24c3`）
- `src/vs/platform/terminal/common/terminal.ts:97,104-105,191-211,320-383,859-874,1081-1087`
- `src/vs/platform/terminal/node/ptyService.ts:121,255-310,336,367-422,562-568,602-667,826-842,987-1007`
- `src/vs/platform/terminal/node/nodePtyHostStarter.ts:25-45`
- `src/vs/workbench/contrib/terminal/browser/terminalService.ts:277-318,475-556,678-735`
- `src/vs/workbench/contrib/terminal/browser/terminalInstance.ts:1718-1804,2858-2927`
- `src/vs/workbench/contrib/terminal/common/terminal.ts:322-339`
- `src/vs/workbench/contrib/terminal/electron-browser/localTerminalBackend.ts:163,183-187,313-369`
- `src/vs/workbench/contrib/terminal/electron-browser/localPty.ts:94-96`
- `src/vs/workbench/contrib/terminal/browser/terminalActions.ts:820-869`
- `src/vs/workbench/contrib/terminal/browser/baseTerminalBackend.ts:105-132`
- `src/vs/workbench/contrib/terminal/common/terminalStorageKeys.ts:6-16`

### Contour（`github.com/contour-terminal/contour`，Apache-2.0）
- `ClientSizePolicy.hpp`（枚举、`resolveClientArea`、按轴合并、manual 缺席）— <https://raw.githubusercontent.com/contour-terminal/contour/master/src/vthost/ClientSizePolicy.hpp>
- `proto/Pdu.hpp:639`（`uint64_t session`）— <https://raw.githubusercontent.com/contour-terminal/contour/master/src/vthost/proto/Pdu.hpp>
- `SessionHost.hpp:78-114,130-155,386,509-510` — <https://raw.githubusercontent.com/contour-terminal/contour/master/src/vthost/SessionHost.hpp>
- Daemon mode 文档（结构对账、会话不跨 daemon、多客户端尺寸、manual 缺失、viewport 规则）— <https://contour-terminal.org/internals/vthost/>
- Persistent sessions（用户向）— <https://contour-terminal.org/persistent-sessions/>

### Zed（`github.com/zed-industries/zed`；本地 `/tmp/opencode/refs/zed` @ `916fc2b8`）
- `crates/terminal/src/terminal.rs:669-679,723-738,1493-1499,1501-1551,1571-1605,1696-1698,3117-3189`
- `crates/terminal_view/src/terminal_view.rs:1088-1107,1231`
- `crates/terminal_view/src/persistence.rs:64-75,70,78,284-289,410-424`
- `crates/project/src/terminals.rs:608-642`、`crates/remote_server/Cargo.toml:3`

### 本项目既有调研
- [`session-daemon.md`](./session-daemon.md)、[`daemon-ipc-protocol.md`](./daemon-ipc-protocol.md)、[`reference-implementations.md`](./reference-implementations.md)、[`zed-terminal.md`](./zed-terminal.md)

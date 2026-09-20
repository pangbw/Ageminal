# Zed 终端方案调研（限于 B1 / B2 视角）

> **状态回写（2026-09-20）**：本文依赖的两个待定决策**均已定**：**B1 #4**（自研 Rust daemon + `alacritty_terminal` 作 VT 内核；路线 C 已由 **#42 实测达标**）与 **B2 #5**（单连接 + 控制优先队列 + 有界帧 + 流控；见 `daemon-ipc-protocol.md`）。

> 调研日期：**2026-09-19**
> 范围：只回答 Ageminal 的两个待定决策 —— **B1**（daemon 的 VT 内核选型 / 是否内置完整 VT 状态机）与 **B2**（daemon ↔ UI 协议、控制/数据分离、流控/背压、版本协商）。Zed 其余部分（GPUI、编辑器、Agent Panel）不在范围内。
> 证据：
> - Zed 主仓：`https://github.com/zed-industries/zed`，本地浅克隆（稀疏检出）`/tmp/opencode/refs/zed`，HEAD = `916fc2b8cb3a815cbef4a3b40e13081be72036b6`（tag `nightly`，2026-09-19）。
> - Zed 的 `alacritty_terminal` 依赖 fork：`https://github.com/zed-industries/alacritty`，本地浅克隆 `/tmp/opencode/refs/zed-alacritty`，rev = `4c129667ce56611becdc82de6e28218c80e2e88f`（2026-06-16，版本 `0.26.1-dev`）。
> - upstream alacritty：`https://github.com/alacritty/alacritty`（用于判定 fork 是否改写核心）；crates.io `alacritty_terminal 0.26.0`（发布 2026-04-06）。
> 记号：**确定事实** = 源码 / 官方 API 直接可查；**⚠️推断** = 无直接来源的解读。
> 关联既有调研：`docs/research/session-daemon.md`（B1 的 daemon/PTY 结论）、`docs/research/daemon-ipc-protocol.md`（B2 的协议/流控结论）。

---

## 一、结论（TL;DR）

1. **Zed 的 VT 内核就是 `alacritty_terminal`，而且不是自维护 fork 的定制内核。**
   Zed 依赖 `https://github.com/zed-industries/alacritty` 的一个 **固定 rev**，`alacritty_terminal` 版本 `0.26.1-dev`。抽样逐字节比对显示：`event_loop.rs`、`sync.rs`、`tty/windows/mod.rs`、`tty/windows/conpty.rs` 与 **upstream alacritty `master` 完全相同**——Zed 的 fork 在 VT 内核层面就是 upstream，没有 Ageminal 需要单独跟的分叉。**对 B1：`alacritty_terminal` 是被一个主流生产终端验证过的选择，且我们可直接用 upstream / crates.io 版本。**

2. **Zed 的终端包装层是自研的，但许可证是 GPL-3.0-or-later。** `crates/terminal`（`Term` 包装、事件循环、渲染内容抽取）是 Zed 自有代码，声明 GPL-3.0-or-later；而 `alacritty_terminal` 本身是 **Apache-2.0**。→ 引擎可放心复用，**Zed 的包装层代码不要照抄**（许可证不兼容“完全开源 + 可自由再分发”的预期，除非本项目也接受 GPL）。

3. **PTY 用 `alacritty_terminal` 自带的 `tty` 模块，不用 `portable-pty`。** Windows 直接绑 `windows-sys` 调 ConPTY，并**优先加载同目录 / PATH 里的 `conpty.dll`（OpenConsole），否则回落系统 API**；匿名管道 + 一个辅助线程（piper 有界环，容量 = 1 MiB）把阻塞管道桥接进 `polling` 轮询器。这与 `session-daemon.md` 对 `portable-pty` 的描述是同一套思路，但 Zed 是自持实现、控制了 drop 顺序与死锁。

4. **Zed 没有终端 daemon，也不做会话持久化。** `Term` + PTY 全部活在 Zed 进程内；关掉 Zed，PTY/HPCON 随之销毁。remote 项目的终端也只是**在本地跑 `ssh`**（ssh 命令作为 shell 程序），并非远端持有终端状态；Zed 的 `remote_server` 自述是 “Daemon used for remote editing”，代码里没有终端/PTY。持久化只把 **`working_directory`** 存进 SQLite，重启后**新开**一个 shell。→ **对 B1：Zed 不能作为“会话跨 UI/重启存活”“daemon 侧快照”的先例**；它只能证明“终端状态机应该在持有 PTY 的那一侧”。

5. **PTY → 渲染的数据面靠“共享内存 + Wakeup”，完全没有帧协议、没有 ack。** PTY 读线程解析进 `FairMutex<Term>`，经 `Event::Wakeup` 通知 UI；UI 在需要时直接 `renderable_content()` 读同一块内存。**背压来自“有界 1 MiB 读缓冲 + 公平锁租约”**：渲染端持锁时读线程继续读，读满 1 MiB 才强制阻塞（从而把背压传导回 ConPTY）。→ **对 B2：这套机制无法移植到 daemon + WebView 的不可阻塞一跳**，反而印证 `daemon-ipc-protocol.md` 的结论——端到端 ack 必须止于 xterm.js。

6. **控制/数据没有“两条管道”的问题，因为根本不传字节。** 控制面（`Input`/`Resize`/`Shutdown`）是一条很小的 `mpsc` 通道；输出面只发**类型化事件**（`Wakeup`/`Title`/`Bell`/`Clipboard`/`ColorRequest`/`ChildExit`/`Exit`），**PTY 字节从不作为事件传输**。→ Zed 对 B2 的“单管道+优先队列 vs 两条管道”**没有直接先例**；但它提供了一个可借鉴的形态：**daemon 解析后只向 UI 发“变更通知 + 结构化增量”，而不是原始字节中继**（代价是要接管 xterm.js 的解析，未必值得）。

7. **可直接抄的三个小设计**：
   - **首事件立即处理、其余批量**：Zed 的事件任务是“先处理 1 个，再在 4 ms 窗口内最多攒 100 个”（`select_biased!`）。这是“控制面低延迟 + 数据面批处理”的现成策略。
   - **resize 去重合并**：只按行列/尺寸变化触发，且把待处理 resize 合并成最后一个。
   - **synchronized update（DECSET 2026）**：解析器在同步窗口内不发 Wakeup，窗口结束一次性刷新——洪峰合并的通用范式。

---

## 二、引擎与封装（B1）

### 2.1 依赖与版本（确定事实）

- `crates/terminal/Cargo.toml:24` → `alacritty_terminal.workspace = true`；`:6` → `license = "GPL-3.0-or-later"`；`:19` → `[lib] path = "src/terminal.rs"`；`:50-51` → `[target.'cfg(windows)'.dependencies] windows.workspace = true`。
- 根 `Cargo.toml:523` → `alacritty_terminal = { git = "https://github.com/zed-industries/alacritty", rev = "4c129667ce56611becdc82de6e28218c80e2e88f" }`。
- `Cargo.lock:612-634` → `name = "alacritty_terminal", version = "0.26.1-dev"`，`source = git+https://github.com/zed-industries/alacritty?rev=4c129667...#4c129667...`。

### 2.2 fork 与 upstream 的关系（确定事实 + ⚠️推断）

- fork 的 `alacritty_terminal/Cargo.toml:5` → `license = "Apache-2.0"`（仓库同时带 `LICENSE-APACHE` 与 `LICENSE-MIT`）。
- 逐字节比对（2026-09-19）：
  - `alacritty_terminal/src/event_loop.rs`：fork ≡ upstream `master`。
  - `alacritty_terminal/src/sync.rs`：fork ≡ upstream `master`。
  - `alacritty_terminal/src/tty/windows/mod.rs`：fork ≡ upstream `master`（`diff` 无输出）。
  - `alacritty_terminal/src/tty/windows/conpty.rs`：fork ≡ upstream `master`。
- **⚠️推断**：Zed 的 fork 在 `alacritty_terminal` 上基本等于 upstream 的某个 master 快照；Zed 的定制主要在 `crates/terminal` 包装层，而不是 VT 内核。crates.io `alacritty_terminal 0.26.0`（2026-04-06）应包含同一套 ConPTY/event_loop/sync 代码，但若要和 Zed 完全一致可 pin 同一 rev。**待确认**：fork 相对 upstream 是否还有未抽样到的补丁（本次只抽了 4 个文件）。

### 2.3 包装形态（确定事实）

- 类型别名（`crates/terminal/src/alacritty.rs:49-55`）：
  - `AlacrittyPty = tty::Pty`
  - `AlacrittyTerm = Term<ZedListener>`
  - `AlacrittyTermLock = FairMutex<AlacrittyTerm>`
- `Term` 构造与配置：`alacritty.rs:188-201`（`new_term`，注入 `ZedListener`）。
- 事件监听：`alacritty.rs:326-330` —— `impl EventListener for ZedListener { fn send_event(&self, event) { self.0.unbounded_send(PtyEvent::Event(event.into())).ok(); } }`。
- 事件循环：`alacritty.rs:203-217` —— `EventLoop::new(term, ZedListener, pty, drain_on_exit, false)` → `event_loop.spawn()`；返回 `PtySender`（`Notifier`）用于输入/缩放/关闭。
- `Term` 内部（fork `term/mod.rs:268-330`）：`grid` / `inactive_grid` 均为**私有字段**，另有 `mode: TermMode`、`damage: TermDamageState`、`event_proxy: T`、`config`。
- 解析器：`Term` 实现 `vte::ansi::Handler`（fork `term/mod.rs:1059`）。

### 2.4 序列化 / 快照能力（B1 关键，确定事实）

- **`Grid<Cell>` 可序列化**：fork `grid/mod.rs:109` 有 `#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]`；`term/cell.rs:14,43,76,124,133`、`grid/row.rs:16`、`grid/storage.rs:32`、`index.rs:49,135,229` 同样有 serde 派生。`term/mod.rs:2735-2746` 还有一个 `grid_serde` 往返测试。
- **`Term<T>` 本身不可序列化**：`term/mod.rs:268` 的 `pub struct Term<T>` **没有** serde 派生，且 `grid` 字段私有。
- **含义**：拿 `alacritty_terminal` 做 daemon 内 VT，可以做到“**栅格快照**（把 grid dump 成字节并回灌）”，但**拿不到“引擎运行态快照”**（解析器状态、`mode`、备用屏栈、damage、`event_proxy` 都不可序列化）。若要 daemon 重启后精确恢复解析中途状态，需要自己补；若只要求“重放当前屏幕”，栅格快照够用。

### 2.5 引擎选型结论（B1）

- **`alacritty_terminal` 就是 Zed 的选择，且核心未被 Zed 改写**。对 Ageminal 而言：直接依赖 upstream（或 crates.io 0.26.0）风险低；**不需要为了跟 Zed 而 fork**。
- 但 Zed 的 daemon 不存在——它是 in-process，所以 **Zed 只证明了“引擎可用”，没有证明“引擎 + 序列化边界可用”**。这恰好是 Ageminal B1 的真正未知项。

---

## 三、PTY 与 Windows ConPTY（B1）

### 3.1 PTY 来源（确定事实）

- `alacritty.rs:180-186`：`open_pty` → `tty::new(options, window_size, window_id)`（`tty::Pty` 自持实现）。
- `tty/mod.rs:11-19`：`#[cfg(not(windows))] mod unix`；`#[cfg(windows)] pub mod windows`。
- Unix：`tty/unix.rs`，依赖 `rustix-openpty` + `rustix` + `signal-hook`（fork `alacritty_terminal/Cargo.toml:30-33`）。
- Windows 依赖：`piper`、`miow`、`windows-sys`（`Cargo.toml:35-45`）。
- **不使用 `portable-pty`**。

### 3.2 Windows ConPTY 细节（确定事实）

- **加载策略**（fork `tty/windows/conpty.rs:52-88`）：先 `LoadLibraryW("conpty.dll")` 并 `GetProcAddress` 取 `CreatePseudoConsole/ResizePseudoConsole/ClosePseudoConsole`；取不到就回落到系统 `CreatePseudoConsole` 等。注释明确：conpty.dll/OpenConsole 相对 inbox ConPTY 有“many improvements and bugfixes”，搜索路径为 PATH 与 exe 同目录。→ 与 `session-daemon.md §5.2/§六` 记的 `portable-pty` 行为一致，且 **Zed 也走“随包分发 conpty.dll”这条路**。
- **创建**（`conpty.rs:110-244`）：`miow::pipe::anonymous(0)` 各建一对输入/输出匿名管道 → `CreatePseudoConsole(COORD, conin, conout, 0, &handle)` → `STARTUPINFOEXW` + `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE` → `CreateProcessW`（`EXTENDED_STARTUPINFO_PRESENT`，可带 `CREATE_UNICODE_ENVIRONMENT`）。
- **句柄/生命周期**：`Pty` 结构把 `backend` 放**第一个字段**以保证 drop 顺序（`tty/windows/mod.rs:27-38`）；`Conpty::drop` 调 `ClosePseudoConsole`，注释警告 **会阻塞到 conout 管道排空，若 conout 已先 drop 会死锁**（`conpty.rs:97-105`）。→ 与 ConPTY 官方文档的“关闭会杀进程树 / 单线程同步使用会死锁”完全对应。
- **resize**：`OnResize for Conpty` → `ResizePseudoConsole`（`conpty.rs:303-308`），COORD 取 `num_cols/num_lines`。
- **阻塞管道桥接**：`UnblockedReader`/`UnblockedWriter`（`tty/windows/blocking.rs`）。构造时 `piper::pipe(pipe_capacity)`，并 spawn 名为 `alacritty-tty-reader-thread` / `alacritty-tty-writer-thread` 的**辅助阻塞线程**；`pipe_capacity = PIPE_CAPACITY = crate::event_loop::READ_BUFFER_SIZE = 0x10_0000 = 1 MiB`（`conpty.rs:29`）。环满时辅助线程 `thread::park()`（`blocking.rs:91-95`），即**有界缓冲 + 背压**。
- **子进程退出**：`ChildExitWatcher`（`tty/windows/child.rs`）注册进 `polling`，`next_child_event()` 读退出（`tty/windows/mod.rs:112-120`）；`pty_info.rs:50-66` 用 `GetProcessId` 兜底 PID。
- **把阻塞管道变成可 poll 的原因**：ConPTY 匿名管道不能用 OVERLAPPED，所以必须“每方向一个线程 + 有界环 + 唤醒 poller”（`blocking.rs:1`）。

### 3.3 对 Ageminal 的含义（B1）

- Zed 证明：**Windows 上直接用 `windows-sys` 绑 ConPTY 是可行的**，且必须处理三件事——(a) conpty.dll 可选增强；(b) drop/关闭顺序与排空（否则死锁）；(c) 阻塞管道要配辅助线程 + 有界环。这为 `session-daemon.md §3.1` 里“`portable-pty` vs `windows-sys`”提供了除 WezTerm 外的第二个一手先例。
- **⚠️推断**：Zed 用 1 MiB 环 + park 的背压是“单客户端、共享内存”语境下的；我们的 daemon 有“无 UI 也要排空管道”的硬约束（见 `session-daemon.md §5.2`），不能简单照抄“环满 park 辅助线程”，否则会阻塞 ConPTY 上游。

---

## 四、进程模型与会话持久化（B1）

### 4.1 没有独立 daemon（确定事实）

- 终端实体 `Terminal` 持有 `term: Arc<AlacrittyTermLock>` 与 `TerminalType::Pty { resources: PtySender, info }`（`crates/terminal/src/terminal.rs:1501-1509`, `1493-1499`）。PTY（HPCON）与 `Term` 都在 Zed 进程内。
- 终端构建在后台执行器上完成（`terminal.rs:1399 cx.background_spawn(fut)`），事件循环任务挂在 GPUI（`terminal.rs:1404-1463`），但**进程仍是 Zed 本身**。
- 全局检索 `crates/remote_server` 无终端/PTY 代码；其 `Cargo.toml:3` 自述 `description = "Daemon used for remote editing"`，`main.rs` 子命令为 `run | proxy | version`。→ **Zed 的 daemon 只服务远程编辑（文件/LSP），不服务终端。**

### 4.2 remote 终端 = 本地跑 ssh（确定事实）

- `crates/project/src/terminals.rs:608-642` `create_remote_shell`：通过 `remote_client.build_command(..., Interactive::Yes)` 生成命令，作为**本地** `Shell::WithArguments` 传入 `TerminalBuilder`。
- 判定点：`terminals.rs:69` `let is_via_remote = self.remote_client.is_some();`；`:97` `let local_path = if is_via_remote { None } else { path.clone() };`；`:250` 把 `is_via_remote` 传给构建器；`:319` 同上。
- 即：**终端字节从本地 ConPTY/pty 流过 ssh，远端 PTY 由 sshd 管理**。Zed 不把终端状态放到远端 server。

### 4.3 持久化只有 cwd（确定事实）

- SQLite 表 `terminals`：字段为 `workspace_id / item_id / working_directory`（`crates/terminal_view/src/persistence.rs:410-424`）。
- 恢复时 `deserialize_terminal_views` / `deserialize_pane_group` 重建 `TerminalView`（`persistence.rs:316-343`, `200-314`），空面板才 `project.create_terminal_shell(...)` **新开** shell（`persistence.rs:284-289`）。
- task 终端不序列化（`persistence.rs:64-75`）。
- → **没有终端内容、没有进程、没有 reattach**。关闭 Zed = `ClosePseudoConsole` = 会话结束。

### 4.4 ConPTY 查询由本地 `Term` 应答（确定事实，对 B1 重要）

- `CSI 6n`（DSR/CPR）：`term/mod.rs:1332-1346` `device_status` → `Event::PtyWrite("\x1b[{};{}R")`。
- `CSI c`（DA1/DA2）：`term/mod.rs:1257-1272` `identify_terminal` → `Event::PtyWrite("\x1b[?6c")` 等。
- 因为 `Term` 与 PTY 同进程且始终在线，**Zed 从不会遇到“无人应答 ConPTY 查询而阻塞”**的问题。→ 反证 `session-daemon.md §3.1` 的结论：**daemon 侧必须持有足以应答查询的 VT 语义**（UI 不在时也一样）。

---

## 五、数据面与背压（B2）

### 5.1 PTY → 屏幕的完整路径（确定事实）

```
ConPTY/pty ──(1)──▶ 读线程(1 MiB buf) ──(2)──▶ FairMutex<Term>.advance() ──(3)──▶ Event::Wakeup(有界? 否，unbounded)
                                                        │
                                              GPUI 事件任务(4ms/100 批) ──(4)──▶ Term 实体事件 → TerminalView cx.notify()
                                                        │
                                              (5) 渲染时 lock_unfair + make_content() 抽 cell 快照
```

逐段：
1. **读 + 解析（`event_loop.rs:104-171`）**：`READ_BUFFER_SIZE = 0x10_0000`（`event_loop.rs:24`），`MAX_LOCKED_READ = u16::MAX = 65535`（:27）。
   - 进循环先 `self.terminal.lease()` 预留锁（:117）；
   - `read()` 累积 `unprocessed`；`try_lock_unfair()` 拿不到锁时**继续读而不阻塞**（:140-145）；仅当 `unprocessed >= READ_BUFFER_SIZE` 才 `lock_unfair()` 强制阻塞；
   - 拿到锁后 `state.parser.advance(...)`（:154），`processed >= MAX_LOCKED_READ` 即 break（:160-161），保证不长时间独占锁；
   - 若 `sync_bytes_count() < processed`，发一次 `Event::Wakeup`（:166-168）——**按批次而非按字节**。
2. **synchronized update（DECSET 2026）**：`sync_timeout()` 算出超时，`events.is_empty()` 时 `stop_sync` 并只发一次 Wakeup（:228-249）。→ 应用可用同步窗口把大量更新合并成一帧。
3. **事件通道**：`ZedListener` 走 `futures::channel::mpsc::unbounded`（`alacritty.rs:57-58`, `terminal.rs:1209-1210`，注释 `TODO: Remove with a bounded sender`）。注意：**通道里只有事件，没有 PTY 字节**；真正的大对象（`Term`）是共享内存。
4. **事件任务批处理（`terminal.rs:1404-1463`）**：
   - 先立即处理 1 个事件（:1407-1409）以降低交互延迟；
   - 然后 `select_biased!` 以 **4 ms** 定时器（:1417-1420）与事件流竞速，最多攒 **100** 个（:1435-1437）；
   - 处理时若出现 `Wakeup` 先 `process_event(Wakeup)`（:1451-1453）；
   - `Wakeup` 处理器只做 `cx.emit(Event::Wakeup)`（`terminal.rs:1674-1681`），由 `TerminalView` 订阅后 `cx.notify()` 重绘（`terminal_view.rs:1136-1142`）。
5. **渲染取数**：`Terminal::sync()` 在 `lock_unfair` 下把 `events` 消化完，然后 `make_content(&terminal, &self.last_content)`（`terminal.rs:2413-2432`；`alacritty.rs:882-930` 把 grid 迭代成 `Vec<IndexedCell>`）。另有 `with_renderable_cells` 直接在锁内迭代（`terminal.rs:2434-2438`）。

### 5.2 背压机制（确定事实）

- **唯一真正的背压是“读缓冲有界 + 锁租约”**：读线程在 1 MiB 内自缓冲，渲染端（`lock_unfair`）可在轮次之间插入；读线程只在缓冲满时强制阻塞，**停读 → ConPTY 管道背压 → 子进程**。
- **输入方向无背压**：`Msg::Input` 进 `State.write_list: VecDeque<Cow<[u8]>>`（`event_loop.rs:400-405`），**无上限**；写不动时留在队列（`pty_write` :174-203）。`Notifier` 空字节直接丢弃（:335-348）。
- **Zed 侧无 ack / credit / 水位**：没有 `Ack` 消息、没有 HIGH/LOW 水位、没有 xterm.js 那套“写回调 ack”。
- **resize 防洪**：`set_size` 只在行列或 cell 尺寸真变化时才入队，且把相邻 resize 合并为最后一个（`terminal.rs:2087-2108`）。

### 5.3 为什么这套背压不能直接移植（B2）

- Zed 的“背压”本质是**共享内存 + 阻塞式生产者**；daemon → WebView 的一跳**不可阻塞**（Tauri 事件/Channel 会丢帧或缓冲无界）。因此 Zed 无法替我们回答“如何对 xterm.js 做 ack”。
- **⚠️推断**：Zed 的 1 MiB/64 KiB 两个常量可作为 daemon 侧“读—解析”分段阈值的起点，但与 daemon→UI 的流控是两回事。

---

## 六、控制 / 数据分离与协议借鉴（B2）

### 6.1 实际情况：不存在“两条管道”的问题（确定事实）

- **输入控制面**：`Msg { Input(Cow<[u8]>), Shutdown, Resize(WindowSize) }` 走一条 `std::sync::mpsc`（`event_loop.rs:29-40`, `84-101`, `174-203`）。输入字节本身也在 `Input` 里，但量级是按键，不是输出洪峰。
- **输出面**：`TerminalBackendEvent` 是一组类型化事件——`MouseCursorDirty / Title / ResetTitle / ClipboardStore / ClipboardLoad / ColorRequest / PtyWrite / TextAreaSizeRequest / CursorBlinkingChange / Wakeup / Bell / Exit / ChildExit`（`terminal.rs:723-738`）。**没有任何 “Data(bytes)” 变体**；字节只存在于共享的 `Term` 里。
- 因此 Zed 对 “单管道 + 控制优先队列 vs 两条管道” **没有给出先例**——它绕开了“传字节”这一步。

### 6.2 可借鉴的协议/调度的“零件”

| 零件 | 源码位置 | 对 B2 的映射 |
| --- | --- | --- |
| 首事件立即处理 + 4 ms/100 批 | `terminal.rs:1407-1437` | 控制帧立即 flush，数据帧批量——正是 `daemon-ipc-protocol.md §五` 想要的“控制优先”策略的现成参数化 |
| Wakeup 合并（按批非按字节） | `event_loop.rs:166-168` | daemon 侧应对“输出洪峰”做**合并通知**而非逐块推送 |
| synchronized update 延迟刷新 | `event_loop.rs:228-249` | 洪峰时把多块合并成一帧再推给 UI |
| resize 合并 | `terminal.rs:2093-2107` | 控制消息幂等化/去重 |
| 公平锁租约（lease） | fork `sync.rs:24-30` | 若 daemon 内部也跑“读线程 + 解析线程 + 多客户端”，这是防饿死的现成原语 |
| 行区间 damage 增量 | fork `term/mod.rs:176-214`, `:458`(`damage`), `:489`(`reset_damage`) | 可选的“栅格增量”而非“原始字节中继”；**但 xterm.js 需要原始字节**，见下 |
| `Msg::Shutdown` 与读线程退出 | `event_loop.rs:91-101`, `:252-254` | 关闭语义分离（对照 `Detach ≠ Terminate`） |

### 6.3 明确不适用 / 无法借鉴

1. **没有 ack / credit / 高低水位**：Zed 用共享内存代替了流控。`daemon-ipc-protocol.md §4.2` 的 VS Code 式水位仍然是我们的必选项。
2. **没有版本协商/能力位**：终端全在进程内，不存在两端版本不一致。我们的 `Hello/Welcome` 只能另找先例（WezTerm/Contour，见既有文档）。
3. **没有多客户端 attach / 重连 / 回放**：Zed 一个 `Term` 一个视图，无 `Detach/Attach`、无 `seq`、无环形缓冲对外协议。
4. **没有快照/恢复协议**：只有栅格 serde 能力，没有 wire 格式。
5. **没有控制面/数据面两条管道的取舍经验**：Zed 不做这个选择。
6. **没有跨 UI / 跨重启存活**：见第四节，Zed 不是这个需求的正例。

---

## 七、对 B1 的净启示

### 7.1 可借鉴

- **引擎选型**：`alacritty_terminal` 是 Zed 的生产选择，且**核心与 upstream 一致**（第四节 2.2）。Ageminal 直接采用 upstream/crates.io 版本即可，审计面小、无需跟随 Zed fork。许可证 **Apache-2.0**，符合完全开源诉求。
- **包装模式**：定义自己的 `EventListener` + 自有事件枚举，把 `Term` 放在 `FairMutex` 后面，`EventLoop::spawn()` 单独线程——这套结构可以原样搬到 daemon（`alacritty.rs:49-55, 188-217, 326-330`）。
- **daemon 内置完整 VT 状态机：Zed 是强支持证据。** 原因不是“为了快照”，而是：(a) ConPTY 会发 `CSI 6n`/`c` 等查询，必须有 VT 语义应答（`term/mod.rs:1257-1272, 1332-1346`）；(b) 无 UI 时若只有字节中继会阻塞上游。→ 与 `session-daemon.md §5.2` 完全一致。
- **PTY 层**：Zed 一手证明 `windows-sys` + ConPTY + 可选 `conpty.dll` + 阻塞管道辅助线程 + 有界环的可行性，可作为我们“是否从 `portable-pty` 下沉到原生”的对照实现。
- **快照可行性**：`Grid<Cell>` 可 serde（`grid/mod.rs:109` 等）→ daemon 至少能做“栅格 dump/回灌”；`Term` 不可 serde → “解析中途态快照”要自己实现。

### 7.2 不适用 / 需自行解决

- **Zed 不是会话持久化先例**：它 in-process、只存 cwd（`persistence.rs:410-424`），关进程即 `ClosePseudoConsole`。我们要的“UI 关闭后存活 / 重连回放”在 Zed 里不存在。
- **Zed 的 engine 快照能力不足**：`Term<T>` 不可序列化，且带 `event_proxy`（含通道）与 `FairMutex`，天然不适合跨进程/重启序列化。→ B1 的“daemon 重启只重建骨架”这一边界仍然成立（`session-daemon.md §5.4`）。
- **Zed 的 1 MiB/park 背压不能照抄**：它假设“读线程可以阻塞”；我们的 daemon 在无 UI 时仍要持续排空 ConPTY。
- **许可证**：`crates/terminal`（Zed 包装层）是 **GPL-3.0-or-later**，不可复制；`alacritty_terminal` 是 Apache-2.0，可放心用。

---

## 八、对 B2 的净启示

### 8.1 可借鉴

- **“解析在持有 PTY 的一侧，UI 只收变更”这一分工是正确的**，与 `daemon-ipc-protocol.md` 的“数据帧 = 原始字节”并不矛盾：daemon 侧保留 `Term` 用于应答查询与快照，UI 侧 xterm.js 仍可吃原始字节。
- **批量/优先策略参数**：4 ms 窗口 + 上限 100 + 首事件立即处理（`terminal.rs:1407-1437`）可直接作为我们“控制帧优先队列”的实现骨架——关键点：**第一个事件不 batch**，其余在短窗口内合并。
- **同步刷新语义**：`stop_sync`（`event_loop.rs:228-249`）与 `resize` 合并（`terminal.rs:2093-2107`）可作为“数据洪峰时合并、控制消息去重”的两条具体规则。
- **`Term::damage()` 行区间增量**（`term/mod.rs:176-214, 458, 489`）是“未来若要摆脱原始字节中继、改走栅格增量”时现成可用的原语；可作为 long-term 选项登记，不进 MVP。

### 8.2 不适用 / 需自行解决

- **控制面形态（单管道+优先队列 vs 两条管道）**：Zed **没有先例**，该决策仍需按 `daemon-ipc-protocol.md §五` 自行拍板。Zed 唯一的间接提示是“控制与数据本就异构，别混在一个无界队列里”。
- **流控/背压**：Zed 不传字节、无 ack；**不能**用它的“共享内存 + 阻塞读”来替代 `daemon-ipc-protocol.md §4.2` 的 xterm.js 端 ack。→ 我们仍需：`HIGH/LOW/ACK_EVERY` 水位 + ack 落在 xterm.js `write` 回调。
- **版本协商 / 重连 / seq 续传**：Zed 完全没有；继续沿用既有文档的 WezTerm/Contour 对照方案。
- **帧/消息格式**：Zed 暴露的是 Rust 枚举事件，不是 wire 帧；无可抄的线格式。
- **把“原始字节”换成“结构化 delta”**：Zed 的 GPUI 自绘决定了它不需要兼容 xterm.js；我们的 UI 栈是 xterm.js，**MVP 保留原始字节帧更稳**（否则等于自研渲染协议）。→ 与 `daemon-ipc-protocol.md §一.2` 一致。

---

## 九、来源

### 9.1 本地克隆（本次实证）

- Zed：`/tmp/opencode/refs/zed` @ `916fc2b8cb3a815cbef4a3b40e13081be72036b6`
  - `crates/terminal/Cargo.toml`
  - `crates/terminal/src/terminal.rs`
  - `crates/terminal/src/alacritty.rs`
  - `crates/terminal/src/pty_info.rs`
  - `crates/terminal_view/src/terminal_view.rs`
  - `crates/terminal_view/src/persistence.rs`
  - `crates/project/src/terminals.rs`
  - `crates/remote_server/Cargo.toml`、`crates/remote_server/src/main.rs`
  - 根 `Cargo.toml`、`Cargo.lock`
- `alacritty_terminal` fork：`/tmp/opencode/refs/zed-alacritty` @ `4c129667ce56611becdc82de6e28218c80e2e88f`
  - `alacritty_terminal/Cargo.toml`
  - `alacritty_terminal/src/event_loop.rs`
  - `alacritty_terminal/src/sync.rs`
  - `alacritty_terminal/src/thread.rs`
  - `alacritty_terminal/src/term/mod.rs`、`term/cell.rs`
  - `alacritty_terminal/src/grid/{mod,row,storage}.rs`、`index.rs`
  - `alacritty_terminal/src/tty/mod.rs`、`tty/unix.rs`
  - `alacritty_terminal/src/tty/windows/{mod,conpty,blocking,child}.rs`

### 9.2 远程比对与版本查询

- upstream alacritty `event_loop.rs` — <https://raw.githubusercontent.com/alacritty/alacritty/master/alacritty_terminal/src/event_loop.rs>
- upstream alacritty `sync.rs` — <https://raw.githubusercontent.com/alacritty/alacritty/master/alacritty_terminal/src/sync.rs>
- upstream alacritty `tty/windows/{mod,conpty}.rs` — <https://github.com/alacritty/alacritty/tree/master/alacritty_terminal/src/tty/windows>
- crates.io `alacritty_terminal`（max/stable = 0.26.0，2026-04-06）— <https://crates.io/crates/alacritty_terminal>
- Zed 源码仓库 — <https://github.com/zed-industries/zed>
- Zed 的 alacritty fork — <https://github.com/zed-industries/alacritty>

### 9.3 关联的既有调研

- `docs/research/session-daemon.md`（B1：daemon 形态、ConPTY、`portable-pty`、daemon 需持有 VT 状态）
- `docs/research/daemon-ipc-protocol.md`（B2：Named Pipe 字节模式、长度前缀帧、xterm.js ack 水位、控制优先队列 vs 双管道）

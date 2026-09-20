# spike-daemon · #40 ConPTY 守护进程 PoC

验证 B1 #4 / B2 #5 / B3 #6 的选型能不能真的跑起来：

> 守护进程用 ConPTY 起一个 shell，客户端经 Named Pipe **attach/detach**，
> 验证「**UI 关闭后进程与输出存活 + 重开回放**」；顺带测吞吐与延迟。

对应票据 **#40**（wayfinder `H3`），父地图 #1。

---

## 跑起来（Windows）

```powershell
# 1) 拷到 Windows 本地（别在 \\wsl.localhost 下编译，慢很多）
robocopy \\wsl.localhost\archlinux\home\pbw\Github\Ageminal\prototype\spike-daemon C:\dev\agm-daemon /E /XD target
cd C:\dev\agm-daemon

# 2) 一条命令跑完全部检查（会自己拉起 daemon，结束后写入报告）
cargo build --release
.\target\release\spike-daemon.exe selftest
```

报告默认写到 **`%TEMP%\agm-daemon-report.json`**（可用 `--json <路径>` 改）。

### 想手动感受「关掉再打开」

```powershell
# 终端 A：常驻 daemon（也可以让 selftest --keep 留着它）
.\target\release\spike-daemon.exe daemon

# 终端 B：交互 attach
.\target\release\spike-daemon.exe attach
#   → 随便敲点命令；然后直接**关掉终端 B**（或按 Ctrl+] detach）
#   → 等十几秒，再开一个终端 attach：应能看到这段时间里 shell 继续跑出来的输出
```

其它子命令：

| 命令 | 作用 |
| --- | --- |
| `daemon [--pipe N] [--shell P] [--ring-bytes N] [--csi6n-reply] [--log PATH]` | 常驻守护进程 |
| `attach [--pipe N]` | 交互附着（Ctrl+] detach） |
| `status [--pipe N]` | 打印会话状态 JSON |
| `selftest [--json P] [--keep] [--no-responder] [--ring-bytes N] [--shell P]` | 自动检查 + 出报告 |

`selftest` **失败要快**：先做**冒烟**（attach 后 6s 内必须看到 shell 的输出）。不过就立刻收尾出报告，并把 **daemon 的日志尾部**（`%TEMP%\agm-daemon-<pid>.log`）一起打出来——不会再有几十个超时静默卡几分钟那种事。
| `probe [--shell P] [--seconds N] [--resize] [--nul-stdio]` | **前台、聚焦**的 ConPTY 探针：把收到的每一段字节转义打出来，并报子进程存活/退出码 |
| `version` | 构建标记 |

---

## 报告怎么读

| 字段 | 含义 | 期望 |
| --- | --- | --- |
| `survival.pass` | **本票核心**：断开 5s 后重连，回放里含断开期间产生的输出，且 shell 仍存活 | `true` |
| `survival.ticks_in_replay` | 回放里数到的 `TICK-<n>` 个数 | 明显 > 1 |
| `survival.shell_alive_after_detach` | 由 daemon 回答的子进程存活状态 | `true` |
| `throughput.kib_per_sec` | 端到端（输入 → cmd → ConPTY → daemon → 客户端）吞吐 | 见下方「怎么算」 |
| `latency_echo.p50/p99` | 一次 `echo` 的端到端往返 | 毫秒级 |
| `latency_ping.p50/p99` | 纯 IPC Ping/Pong 往返 | **亚毫秒**（对照 #45 的控制面 spike） |
| `csi6n.requests_seen/replies_sent` | daemon 是否看到了 `ESC[6n`（以及开启应答器时是否回了） | 看能否复现 §5.2 的查询 |
| `status_final` | 收尾时的 daemon 会话状态快照 | — |
| `errors[]` | 任一检查失败的原因 | 空 |

**量测口径**（避免误读）：

- **吞吐**含 cmd 自身的解析与打印开销，是**整条链路**的数字，不是管道上限。
- **echo 延迟**的标记形如 `L-<i>#`，只匹配**行首**（`\r\n` 前缀）——因为 ConPTY 会把我们敲进去的命令**回显**出来，不这样区分会命中回显、量到 0ms。
- **ping 延迟**用微秒收集（毫秒粒度会全部变 0）。

---

## 设计要点（对齐已定决策）

| 决策 | 在 PoC 里的落法 |
| --- | --- |
| B1 #4：**守护进程独立持有 ConPTY** | `daemon` 是独立进程；`selftest` 用 `DETACHED_PROCESS \| CREATE_NEW_PROCESS_GROUP` 拉起，且 stdio 全部脱开 |
| B1 #4：**PTY 藏在 trait 后面** | `conpty::Pty` trait；`Session` 持 `Box<dyn Pty>`（换实现不动上层） |
| B2 #5：**长度前缀帧 + 控制优先** | `[u8 kind][u32 len]`；写侧 `select!` **biased** 先发控制帧（Pong/Status），再发数据 |
| B2 #5：**Hello/能力位** | `HELLO` / `HELLO_ACK{proto, caps, shell_pid, alive}` |
| B2 #5：**按偏移去重，避免快照与实时流重叠加倍** | 每个 chunk 带单调 `start`；衔接时按 `total` 裁掉重叠部分 |
| B3 #6：**无条件读进环形缓冲**（与有无客户端无关） | 阻塞读线程直接写 ring + 广播；`--ring-bytes` 设上限 |
| B4 #7：**单例** | `ServerOptions::first_pipe_instance(true)`：重复创建会 `PermissionDenied` |
| §5.2：**无 UI 时要有东西应答终端查询** | `--csi6n-reply` 开启最小应答器（检测 `ESC[6n` 并回 `ESC[1;1R`），并统计次数 |

**`--no-responder` 是 A/B 的另一臂**：同一台机器、同一段命令，比较「有/无最小应答器」时 `csi6n.*` 的差别。

---

## 本 PoC 不做什么（留给 #42 / 后续）

- **不做 VT 状态机**：不维护屏幕模型，也没有 alt-screen 快照。本票只验「原始流的存活与回放」；
  **快照一致性**是 #42（daemon 快照 → xterm.js）。
- **不做 DACL / SID 管道名 / 令牌**：那是 B2 #5 / B4 #7 的实现细节，PoC 用固定管道名。
- **不做落盘**：只有内存环形缓冲（落盘是 B3 #6，要评估的是磁盘预算）。
- **不含 UI**：客户端是 CLI。接进 xterm.js 属于实现阶段。

---

## H2：真机暴露的一个硬 bug（已修）

第一次真机 `selftest` 的现象：**自动弹出一个 cmd 窗口，然后就没有后续**。

根因是我把 `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE` 的值传错了：

```c
// 微软官方样例（portable-pty 同样如此）
UpdateProcThreadAttribute(list, 0, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
                          hpc,            // ← 传**句柄值本身**
                          sizeof(hpc), NULL, NULL);
```

我写成了 `&hpc`（**变量的地址**）。于是属性指向一块栈内存 → 子进程拿不到伪控制台 →
**Windows 给它另开了一个控制台窗口**（你看到的 cmd），而 ConPTY 那头**一个字节都不产出** →
后续所有标记等待全部超时。

同时把 `bInheritHandles` 改成 **FALSE**，与官方样例一致。

> **判据：跑 `selftest` 时若再出现新的 cmd 窗口，就是这里又出了问题。** 正常情况下
> ConPTY 的子进程是**无窗口**的。

## H3：第二个 ConPTY 坑（句柄释放时机）+ 退出码

H2 的真机结果：`CreatePseudoConsole ok`、`CreateProcessW ok`、**`PTY 首包 16 字节`**（那 16 字节
`ESC[?9001h ESC[?1004h` 正是 conhost 的开场序列）——**第一个坑修好了，ConPTY 真的在产出**。

但紧接着暴露第二个问题：**子进程起来后立刻死了**（`hello.alive=false`），cmd 连 banner 都没打出来，
此后一个字节都没有 → conhost 空转。

两处对齐一手实现（MS 官方样例 / `portable-pty`）：

| 项 | 原来（错） | 现在 |
| --- | --- | --- |
| `CreatePipe` 两端何时释放 | **CreateProcess 之前** | **CreateProcess 之后**（样例原话："Upon completion of the CreateProcess call … should be freed"） |
| `CreateProcess` 标志 | 仅 `EXTENDED_STARTUPINFO_PRESENT` | 再加 `CREATE_UNICODE_ENVIRONMENT`（与 portable-pty 一致） |

并补上**决定性诊断**：`GetExitCodeProcess`。现在 `probe` 与 `status`/`selftest` 都会报出
**子进程退出码**：

- `0xC0000142` = `STATUS_DLL_INIT_FAILED` —— 文档明确：**未能接到伪控制台**（给的就是这个码）
- `259` = `STILL_ACTIVE` —— 还活着
- 其它值 —— 说明进程起来了又自己退了

> 快速定位用这个（前台跑，4 秒）：
> ```powershell
> .\target\release\spike-daemon.exe probe
> ```
> 它会打印每一段收到的字节（转义后可读）、每秒的存活/退出码，并提示**是否出现了不该出现的控制台窗口**。

## H4：`probe` 通、`selftest` 不通（启动方式矩阵）

真机上出现分叉：

| | `probe`（**前台，有控制台**） | `selftest` 的 daemon（**DETACHED_PROCESS**，无控制台） |
| --- | --- | --- |
| 收到字节 | 16 + 96 + 87（banner + 提示符） | **只有 16**（conhost 开场） |
| 子进程 | `alive=true exit=259` | `alive=false`，**`exitCode=0`**（干净退出） |

**同一份 ConPTY 代码**，唯一差别是拉起 daemon 的方式。子进程「干净退 0」+ 只收到开场序列 =
它没能挂上伪控制台，于是 cmd 读不到 stdin 就正常退出了。

这直接关系到 **B1 #4「daemon 以 detached 方式拉起」**，所以不做逐个试错，用一个矩阵把变量一次拆开：

```powershell
.\target\release\spike-daemon.exe matrix
```

四个臂（每个各自拉起一个 daemon、attach、发一个唯一标记的 echo、看 6s 内是否回显）：

| 臂 | creation flags |
| --- | --- |
| A | `DETACHED_PROCESS \| CREATE_NEW_PROCESS_GROUP`（现状） |
| B | `DETACHED_PROCESS`（去掉 NEW_GROUP） |
| C | `CREATE_NO_WINDOW`（有控制台、无窗口） |
| D | 不设 flag（继承本终端控制台） |

六个臂：A–D 均为 `stdio=NUL`（只变 creation flags），E 为 **stdio 继承本终端**，F 为 A + `CREATE_BREAKAWAY_FROM_JOB`。

输出会给每个臂的**冒烟结果 + 子进程退出码 + 已产出字节数**，最后列出**可用臂**。

> 真机结果：A/B/C/D **四个臂全部以完全相同的方式失败**（都是 `exitCode=0 / bytesOut=16`）
> ⇒ **元凶不是 creation flags**。四个臂唯一共有的、且与 `probe` 不同的变量是
> **daemon 的 stdio 被重定向到 NUL**（`probe` 用的是终端 stdio）。`exitCode=0` + 零输出
> 正是「cmd 的 stdin 读到 EOF 后干净退出」的典型表现。
> H5 因此补了 `probe --nul-stdio`（**同一进程内**把自身 stdio 换成 NUL，隔离「stdio」与「是子进程」）
> 与 E/F 两臂。

## H6：两个真机结论 + 一个测试工具缺陷

### 1. 元凶确认：daemon 的 std handle

第二轮矩阵（只变 creation flags，stdio 一律 NUL）**A/B/C/D 四臂全部以完全相同方式失败**
（`exitCode=0 / bytesOut=16`），而 **E（stdio 不重定向）** 给出 `alive=true / bytesOut=262`。

⇒ 与 creation flags 无关。**`CreateProcessW` 会把父进程的 std handle 复制给 ConPTY 子进程**：
daemon 的 stdio 指向 NUL 时，子进程的 stdin 就是 NUL → cmd 读一次即 EOF → **干净退 0**（不是崩溃，
所以没有 `0xC0000142`）。

第三轮保留三臂定「怎么修」：

| 臂 | 做法 |
| --- | --- |
| A | 对照组（现状，已知失败） |
| E | stdio **完全不重定向** |
| H | stdio=NUL **+ 子进程 `STARTF_USESTDHANDLES`（三个句柄为 NULL）**，让子进程的 std handle 从 NULL 起步、由控制台子系统填成伪控制台 |

### 2. 测试工具缺陷：标记匹配不可靠

原先用 `\r\n<标记>` 匹配命令输出（依据是「回显的输入行前不会有 \r\n」）。但真机上 ConPTY 会用
**`ESC[<行>;<列>H` 绝对定位**来渲染行（probe 输出里能直接看到 `ESC[4;1HC:\dev\agm-daemon>`），
`\r\n` 前缀并不保证存在 → 冒烟会**假失败**。

现改用 `set /a A+B`（见 `src/marker.rs`）：**输出是数字，而命令行文本里不会出现这个数字**
（被 `+` 从中间切开，且逐点试探分割位置）。于是裸数字匹配**既不会命中回显，也不依赖任何渲染细节**。

## H7：定案 —— 子进程必须显式 `STARTF_USESTDHANDLES` + 三句柄 NULL

第三轮矩阵（三臂对照）的结果：

| 臂 | 做法 | 结果 |
| --- | --- | --- |
| A | `stdio=NUL`（对照） | ❌ `alive=false exitCode=0 bytesOut=16` |
| E | `stdio` 完全不重定向 | ✅ `alive=true exitCode=259 bytesOut=248` |
| **H** | **`stdio=NUL` + 子进程 `STARTF_USESTDHANDLES`（三句柄 NULL）** | ✅ `alive=true exitCode=259 bytesOut=248` |

**采用 H。** 它是三者里唯一**不需要修改 B1 #4** 的方案：daemon 的 stdio 照旧指向 NUL
（与 app 管道彻底脱钩），只要在创建 ConPTY 子进程时声明
`STARTF_USESTDHANDLES` 且 `hStdInput/hStdOutput/hStdError = NULL`。

### 机制（这条值得写进 §5）

> `CreateProcessW` 会**把父进程的 std handle 复制给子进程**。daemon 的 stdio 指向 NUL 时，
> 子进程的 stdin 也是 NUL → cmd 读一次即 EOF → **干净退 0**（不是崩溃，所以既没有
> `0xC0000142`，也没有任何错误输出，只有 conhost 的 16 字节开场序列）。
> 显式置 NULL 后，由**控制台子系统**把子进程的 std handle 填成伪控制台。

H7 起这已是 `conpty::spawn` 的**无条件行为**（不再有开关），并已删除探索期的 `matrix` 子命令。

## `vt-parity`（#42）：VT 内核快照 → xterm.js 一致性

```powershell
# 需要显式开启 feature（否则不拉 alacritty_terminal 那棵树）
cargo build --release --features vt-parity

# Windows：起 ConPTY 跑 fixture，写出四份产物到 %TEMP%\agm-vt-parity\
.\target\release\spike-daemon.exe vt-parity --tail 512
```

产物：

| 文件 | 含义 |
| --- | --- |
| `full.bin` | 完整原始字节流（= 从头重放的**参考**） |
| `tail.bin` | 末尾 N KB（**哑管道基线**：有界缓冲只能回放尾巴） |
| `snap.ansi` | **内核算出的 ANSI 快照**（C 路线的交付物） |
| `meta.json` | 尺寸 / alt-screen / 光标 / 内核逐列字符 / 逐格属性签名 |

随后在 spike 前端切到 **#42 VT parity** 视图 →「加载产物并比对」：三台 xterm.js 分别重放三份材料，
**逐行逐格**与内核视角比对，给出「达标 / 不达标」判定。

> `--tail` 要选得比流小（例如 512），否则 tail 不截断、基线那一行没有意义（视图里会告警）。
>
> 非 Windows（例如在 WSL 里先验内核链）：
> ```bash
> cargo run -q --features vt-parity -- vt-parity --emit-fixture /tmp/fixture.bin
> cargo run -q --features vt-parity -- vt-parity --replay /tmp/fixture.bin --out /tmp/agm-vt-parity
> ```

## 自审记录（交付前又查了一遍）

修掉的**运行时**隐患（编译期查不出）：

1. **`detach` 不是真的断开**：客户端读任务仍握着 `ReadHalf`，daemon 侧看不到 EOF。→ `Attached` 加 `Drop` → `abort()` 读任务。
2. **含空格的外壳路径会被拆开**：`CreateProcessW` 的 `lpApplicationName` 为 null，命令行必须自带引号。→ 给 shell 路径加引号（影响 `pwsh.exe`、`git-bash.exe` 这类路径）。
3. **子进程会多拿管道句柄副本**：`bInheritHandles=TRUE`，我们这侧的两端也标了可继承。→ `SetHandleInformation(..., HANDLE_FLAG_INHERIT, 0)`。
4. **IPC 延迟被毫秒粒度压成 0**：Ping/Pong 往返常在 1ms 以下。→ 改微秒收集。
5. **标记会命中「回显的输入」**：ConPTY 回显你说的命令，`echo X` 的标记会立刻出现。→ 标记带 `\r\n` 行首前缀，只匹配命令的**输出**行。
6. **`TICK-%i` 被误计入统计**：回显的命令行里是字面 `%i`。→ 只数 `TICK-<数字>`。
7. **广播队列上限过高**：4096 个 64KiB chunk（最坏 ~256MiB）。→ 降到 1024。

**H2 新增**（都是被上面那次失败逼出来的）：

8. **失败时要快**：加冒烟阶段 + 提前退出 + 连续失败熔断（原先最坏会静默卡 ~10 分钟）。
9. **要看得见**：每阶段打进度；daemon 侧关键节点（CreatePseudoConsole 结果、CreateProcessW 的 pid、PTY 首包、EOF）写进 `--log` 文件，出错时连日志尾部一起带回。

**仍已知的风险**（只能真机暴露，报告里会体现）：

- ConPTY 是否会把子进程写的 `ESC[6n` **透传**给 daemon（若不透传，`csi6n.requests_seen` 会是 0 —— 那本身也是有价值的结论）。
- PowerShell 启动慢，CSI 探针用了 12s 超时；若机器上无 PowerShell，该项会被记为未跑。
- `--shell` 换成 Git Bash / pwsh 时的登录与 PATH 行为（调研 §六 风险 7），报告里的 `shell` 字段会记下用的是哪个。

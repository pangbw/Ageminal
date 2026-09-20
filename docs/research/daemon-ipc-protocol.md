# 守护进程 ↔ UI 通信协议（daemon-ipc-protocol）

> **状态回写（2026-09-20）**：**已定 —— B2 #5 / B3 #6 / B4 #7 / B5 #8**，且**协议子集已在 Windows 真机跑通（#40）**：单连接 + **控制优先队列**（写侧 `select!` biased）、长度前缀帧、`Hello/HelloAck`（协议版本 + 能力位）、chunk 带**单调偏移**以便「快照 + 实时流」去重衔接、`first_pipe_instance` 单例。
>
> **实测**：纯 IPC Ping/Pong **p50 0.016 ms / p99 0.028 ms**；一次命令往返（含 shell 与 ConPTY）p50 0.88 ms；存活 + 重放 ✅（断开 5 s 后重连，回放 964 字节 / 7 个断开期间的 TICK）。
>
> 未闭合：**`ESC[6n` 是否被 ConPTY 透传**（关系「daemon 是否必须应答终端查询」这一成本项）——留待实现期用不经 shell 的探针确认。

> 调研日期：**2026-09-19**（纯 Windows 前提）
> 背景：Ageminal 拓扑为 `UI (Tauri Web) ↔ Session Daemon（常驻，持有 ConPTY）↔ Shell/Agent`，UI 不直接持有 PTY。`REQUIREMENTS.md` §3 / §5.1 / §18.5 把「守护进程 ↔ UI 通信」标为候选 `Named Pipe + 长度前缀 JSON 帧`、待专项调研；对应 issue #5。
> 记号：**确定事实**来自 Microsoft 官方文档 / 官方仓库源码 / 官方 issue 原文；**⚠️推测**为无直接来源的推断，需用户拍板或实测。
> 本文只解决「链路怎么走、帧怎么排、洪峰怎么扛、断线怎么接」，不替用户做最终选型。

---

## 一、结论（TL;DR）

1. **`Named Pipe + 长度前缀帧` 成立，但建议用「字节模式（byte mode）」，不要用消息模式（message mode）。**
   消息模式虽然能保留 `WriteFile` 的消息边界，但客户端句柄默认是字节读模式、必须再 `SetNamedPipeHandleState` 切换，且读缓冲小于消息时 `ReadFile` 只返回部分数据并置 `ERROR_MORE_DATA`，把「帧边界」和「I/O 缓冲大小」耦合在一起。我们本来就要在管道之上自己分帧（见 §三），消息模式买不到任何额外保证，只增加坑。字节模式 + 自研长度前缀是 WezTerm / Contour 的同款做法。

2. **「长度前缀 JSON 帧」应拆成两层：控制消息用 JSON（或 bincode），PTY 数据用原始字节帧。**
   ConPTY 输出是 UTF-8 + VT/OSC 的**字节流**（不是结构化二进制，但必须逐字节无损），绝不要走 JSON 字符串 / base64：base64 体积膨胀 **+33%**（RFC 4648：3 字节 → 4 字符）并额外占两次 CPU 编解码。数据帧的 payload 直接是原始字节，xterm.js 的 `write()` 本来就接受 `Uint8Array`。

3. **真正的瓶颈不在 Named Pipe，而在最后一跳 `Tauri 核心 → WebView2 → xterm.js`。**
   Named Pipe 本地吞吐足以喂满终端；而 Tauri 官方明确说事件系统「**不是为低延迟 / 高吞吐设计的**」，推荐用 Channel 做流式。xterm.js 自述处理吞吐约 **5–35 MB/s**，写缓冲超过 **50 MB** 会**直接丢弃**。因此端到端背压的 ack 必须由**渲染器**（xterm.js `write` 回调之后）产生，而不是在管道层假装有背压。

4. **流控模型抄 VS Code 的「高/低水位 + 累计 ack」，而不是简单 `pause()/resume()`。**
   VS Code：未确认字符数 > **100000** 时暂停 PTY，按 **5000** 字符一批 ack，< **5000** 恢复；ack 在 xterm.js 解析完该块的回调里发出。简单「每块 pause/resume」会把吞吐打崩（xterm.js 官方指南原话）。我们按**字节**而非 JS 字符计数即可。

5. **控制类消息必须能插队，数据洪峰不能饿死控制面。**
   现实教训：WezTerm mux-server 在持续 `PaneOutput` 下，因为「每连接无界队列 + 单线程调度」，把 `GetCodecVersion` / `ListPanes` 这类控制请求饿死到超时（issue #7692）。方案：控制帧走独立小队列并优先刷新；或干脆**控制与数据各一条 Named Pipe 实例**。

6. **需要版本协商 + 能力位；握手是 `Hello/Welcome`，重连带 `lastSeq` 做精确续传。**
   WezTerm 用 `CODEC_VERSION`（当前 45）+ `GetCodecVersion` + 「未知类型解成 `Invalid` 当作数据而非错误」来向前兼容；Contour 用 `ClientHello/ServerHello` 精确匹配。Ageminal 是本地单用户，建议「版本区间 + 能力位 + 未知帧忽略」，比 Contour 的「精确匹配、拒绝时不解释」更适合自助排障。

7. **断线检测靠三层：读返 0 / `ERROR_BROKEN_PIPE`、心跳 `Ping/Pong` 超时、以及每会话序号。**
   重连时客户端上报 `[{sessionId, lastSeq}]`，守护进程从环形缓冲补齐缺口后继续，避免重复 / 丢字节。

8. **共享内存（file mapping）不值得做进 MVP。**
   它省掉一次拷贝，但消费端 WebView 仍要再拷一次，且引入 section + 命名事件 + 环指针同步的生命周期复杂度。先在 Named Pipe 上把协议跑通，用压测决定是否需要它。

9. **安全上必须显式收口：默认管道 DACL 过宽、且命名管道默认可被远程访问。**
   `CreateNamedPipe` 未传 `lpSecurityAttributes` 时，默认描述符给 `Everyone` 和匿名账户**读权限**；且只要 Server 服务在跑，命名管道默认可远程访问。必须 `PIPE_REJECT_REMOTE_CLIENTS` + 显式 DACL 限当前用户 + 握手令牌（`REQUIREMENTS.md` §15 已要求）。

10. **一句话选型：本地 Named Pipe（字节模式，双工）+ 20 字节定长头 + 类型字段区分「原始数据帧 / JSON 控制帧」+ 会话级累计 ack 背压 + 心跳与 seq 续传。** 控制面可先同管道、预留拆成第二条管道的能力。

---

## 二、传输层选择：Named Pipe vs 其他

### 2.1 命名管道的两种类型模式（确定事实）

Microsoft「Named Pipe Type, Read, and Wait Modes」与 `CreateNamedPipe` 文档明确：

| 维度 | 字节模式（`PIPE_TYPE_BYTE`，默认） | 消息模式（`PIPE_TYPE_MESSAGE`） |
| --- | --- | --- |
| 写语义 | 不区分每次 `WriteFile`，当成连续字节流 | 每次 `WriteFile` 的字节算一个「消息单元」 |
| 读语义 | 只能字节读；读到「当前可用全部」或「请求数量」即成功 | 可字节读或消息读；消息读只在**整个消息读完**时成功 |
| 缓冲不足 | 字节流可分段，无边界概念 | 读缓冲小于消息时，只读到部分数据、返回 `ERROR_MORE_DATA`，余下需再次 `ReadFile` |
| 模式约束 | `PIPE_TYPE_BYTE` **不能**配 `PIPE_READMODE_MESSAGE` | `PIPE_TYPE_MESSAGE` 可配字节读或消息读 |
| 客户端初始状态 | 客户端句柄**初始总是字节读模式**，需 `SetNamedPipeHandleState` 才能切消息读 | 同左 |
| 写穿透 | `FILE_FLAG_WRITE_THROUGH` 只影响字节型管道（且跨机时） | 消息型管道**总是**按写穿透语义写 |

**对我们的含义**：消息模式的唯一好处是「一次 `WriteFile` = 一个消息」，但：
- 我们仍要自己分帧（要在一个连接上多路复用多个 session、控制与数据、心跳、ack），消息边界帮不上忙；
- 消息模式把「最大帧长」绑到「管道的 in/out buffer 大小」，而 buffer 是**建议值**、由内核按 nonpaged pool 分配、必要时扩容（扩容时写会阻塞）；
- 任何一帧超过读缓冲，就要处理 `ERROR_MORE_DATA` 的部分读，反而比字节流更容易写错。
→ **确定结论：用字节模式。** WezTerm 的 mux codec 与 Contour 的 native protocol 都是在字节流上自研 `长度前缀` 分帧（见 §3.2 来源）。

### 2.2 Rust 生态现状（确定事实 + ⚠️）

- **`tokio`**：`ServerOptions` 默认 `PipeMode::Byte`，默认 `out_buffer_size`/`in_buffer_size` = **65536**，默认 `reject_remote_clients = true`；`PipeMode::Message` 会同时设 `PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE`，客户端再 `SetNamedPipeHandleState` 切消息读。选 `Byte` 即可直接 `AsyncRead/AsyncWrite`。
  ⚠️推测：`tokio` 的读路径未见对 `ERROR_MORE_DATA` 的特判，若用消息模式且读缓冲 < 消息，可能被当成 I/O 错误上抛。这也是**不要用消息模式**的另一个理由。
- **`interprocess`**：`os::windows::named_pipe` 与 `pipe_mode` 模块同时提供 `Bytes` / `Messages` 标签，消息模式可用；但 `tokio` 是更主流、文档更全的选择。
- 两者都基于 overlapped I/O，符合 ConPTY 官方「每条通道单独线程、不要同步阻塞死锁」的要求。

### 2.3 命名管道的 I/O 模型（确定事实）

- `CreateNamedPipe` 建议开 `FILE_FLAG_OVERLAPPED`，用 `ReadFile`/`WriteFile` + `OVERLAPPED` 做异步；`PIPE_NOWAIT` 是为兼容 LAN Manager 2.0 保留的，**不要**拿它当异步。
- ConPTY 文档警告：输入 / 输出通道**各自用独立线程**并各自维护缓冲与消息队列；所有 pseudoconsole 活动挤在一个线程上会死锁（一个缓冲填满、你却在另一条通道上阻塞等待）。
- Windows Terminal 的真实实现可作范本：输出管道用 **overlapped `ReadFile`**，单线程、**128 KiB** 缓冲，并「先投递下一次读、再处理上一块」（双缓冲），`read == 0` 即优雅关闭；写入遇 `ERROR_BROKEN_PIPE` 视为断连。
- 优雅收尾：`FlushFileBuffers` 会**阻塞到客户端读完所有数据**，可用于「结束会话前把尾帧发完」；之后 `DisconnectNamedPipe` 会丢弃未读数据。

### 2.4 命名管道的实例 / 命名 / 单例 / 权限（确定事实）

- 名称形如 `\\.\pipe\pipename`，整串 ≤ **256** 字符，**大小写不敏感**，`pipename` 内不能含反斜杠。
- `nMaxInstances` ∈ [1, 255]（`PIPE_UNLIMITED_INSTANCES` = 255）；**所有实例的类型模式 / 访问模式 / 实例数 / 超时必须一致**。
- 单例：首实例用 `FILE_FLAG_FIRST_PIPE_INSTANCE`，若同名实例已存在则创建失败（`ERROR_ACCESS_DENIED`）——可作为「守护进程已存在」的探测原语。（⚠️ `ERROR_ACCESS_DENIED` 也可由权限不足触发，探测时需结合其它手段区分。）
- 多客户端：一个名字可有多个实例，天然支持「一条管道实例 = 一个 UI 连接」。
- 安全：默认 DACL 给 LocalSystem / Administrators / 创建者完全控制，给 `Everyone` 与匿名**读**；默认**允许远程**。→ 显式 `PIPE_REJECT_REMOTE_CLIENTS` + 显式 DACL（限当前用户 SID、拒 `NT AUTHORITY\NETWORK`）。

### 2.5 备选传输对比

| 方案 | 优点 | 缺点 | 结论 |
| --- | --- | --- | --- |
| **Named Pipe（字节模式）** | Windows 原生；ACL 精确；tokio/interprocess 一等支持；WezTerm/Contour/VS Code(ConPTY) 均有先例 | 无 Unix 的 `shutdown()` 双向关闭语义；需自研分帧 | **首选** |
| **AF_UNIX（Winsock）** | 与 Unix 后端同构，`shutdown` 语义齐全 | Win10 17063+（GA 1803+）；仅 `SOCK_STREAM`，**无** `SCM_RIGHTS`/credentials/datagram/`socketpair`；安全靠 NTFS ACL 而非 POSIX 位 | 备选；跨平台诉求出现再上 |
| **Loopback TCP** | 库最通用 | 端口/防火墙/占用；ACL 弱；被本机任意进程连接的风险 | 不推荐 |
| **Windows RPC** | 自带 IDL/端点映射 | 重量级、调试复杂、与异步生态不搭 | 不推荐 |
| **共享内存 + 命名事件** | 省一次拷贝、延迟最低 | 需 section + 事件 + 环指针 + 生命周期同步；WebView 仍要再拷 | 留作后期优化，不进 MVP |

> 注：`REQUIREMENTS.md` §5.3 已把 `ConPTY + 常驻守护进程 + Named Pipe` 定为主方向；本节的结论是「成立，且选字节模式」。

---

## 三、帧格式草案（含二进制数据）

### 3.1 设计原则

1. **字节流上自研分帧**：`长度前缀` 是唯一可靠的边界来源。
2. **两类负载**：
   - **数据帧**：payload = 原始 PTY 字节，**不转义、不 base64、不 JSON**；
   - **控制帧**：payload = UTF-8 JSON（人类可读、便于排障；若日后性能吃紧可换 bincode，但控制消息量小，JSON 足够）。
   由 `type` 决定怎么解 payload，而不是靠「猜」。
3. **定长头**：v1 用固定宽度、小端，解析无歧义、实现最简单；WezTerm/Contour 用 LEB128 varint 省字节，但我们帧普遍不大，省下的几个字节不值得引入 varint 解析分支。（大帧用 max frame size 兜底，见下。）
4. **长度是「本帧剩余字节数」**，且必须校验上界，防止损坏的长度字段导致超大分配（Contour 的 `MaxFrameSize` / `MaxGridExtent` 就是干这个的）。

### 3.2 帧布局（v1 草案）

```
offset  size  field           说明
0       4     length          u32 LE，本帧除 length 字段外的剩余字节数（含 type..payload）
4       2     type            u16 LE，见 §3.4
6       2     flags           u16 LE，位定义见下
8       4     sessionId       u32 LE，0 = 连接级（握手/心跳/枚举）；否则为会话 id
12      8     seq             u64 LE，会话内单调递增（数据帧/ack 用）；不适用时为 0
20      N     payload         数据帧 = 原始字节；控制帧 = UTF-8 JSON
```

`flags` 建议位：

| 位 | 含义 |
| --- | --- |
| bit0 | `payload_is_raw`：1 = 原始字节（数据帧），0 = JSON 控制帧 |
| bit1 | `is_replay`：1 = 回放环形缓冲的历史数据（UI 可据此插入「已重新连接」分隔） |
| bit2 | `compressed`：payload 是否压缩（v1 恒 0，预留） |
| bit3 | `more`：分片帧，后面还有同 `type` 的续帧（用于超大帧切分） |

约束：`MAX_FRAME = 1 MiB`（可配）；超大块在守护进程侧按此切分成多帧（`more` 置位），接收端凭 `sessionId + seq` 重组或直接顺序拼接。

**⚠️推测**：20 字节头对终端数据有约 0.02%–0.2% 开销（相对 32–128 KiB 的块），可忽略。

### 3.3 PTY 高吞吐输出的承载方式对比

（前提更正：ConPTY 输出按官方文档是 **UTF-8 编码的文本 / VT 序列**；只是其中可能夹带大体积的图片协议负载（sixel / iTerm2 / Kitty）与二进制控制序列，所以**必须当不透明字节流无损搬运**，而不是当「已结构化的二进制帧」。）

| 承载方式 | 体积开销 | CPU | 端到端拷贝 | 备注 |
| --- | --- | --- | --- | --- |
| JSON 字符串（文本直接进 JSON） | 转义可能膨胀，且强制 UTF-8 解码 | 编解码 + 转义 | 最多 | ❌ 会把跨块的多字节 UTF-8 序列切断，危险 |
| **JSON + base64** | **+33%**（RFC 4648） | 两端 base64 + JSON | 多 | ❌ 仅在「传输层只吃字符串」时被迫使用 |
| **长度前缀二进制帧（本方案）** | ~0 | 一次装箱 | 1 次（管道拷贝） | ✅ 首选 |
| 独立数据流（第二条 pipe / socket） | ~0 | 无 | 1 次 | ✅ 可作控制面隔离手段，见 §五 |
| 共享内存（file mapping + 事件） | ~0 | 无 | 理论 0（但 WebView 仍要拷） | ⚠️ 复杂度高，非 MVP |

**关键点**：Tauri 侧若走 JSON 事件，payload 会先被 `serde_json` 序列化成字符串；若此时把原始字节塞进字符串，就会反复 UTF-8 解码并可能破坏被切分的多字节序列。**务必**在 Rust 侧保持 `Vec<u8>` / `Uint8Array` 直到 xterm.js。

### 3.4 消息类型清单（草案）

连接级（`sessionId = 0`）：

| type | 名称 | 方向 | payload |
| --- | --- | --- | --- |
| 0x0001 | `Hello` | UI→D | `{protoMin, protoMax, caps, token, clientInstanceId, resume:[{sessionId,lastSeq}]}` |
| 0x0002 | `Welcome` | D→UI | `{proto, caps, daemonVersion, sessions:[...]}` |
| 0x0003 | `Error` | D→UI | `{code, message}` |
| 0x0004 | `Ping` / 0x0005 `Pong` | 双向 | `{}` |
| 0x0010 | `ListSessions` / 0x0011 `Sessions` | UI↔D | 会话枚举（重连后重建页签） |
| 0x0020 | `CreateSession` / 0x0021 `SessionCreated` | UI↔D | `{cwd,shell,command,cols,rows,env}` / `{sessionId,pid}` |
| 0x0022 | `Attach` / 0x0023 `Attached` | UI↔D | `{sessionId,lastSeq}` / `{sessionId,fromSeq,replayBytes}` |
| 0x0024 | `Detach` | UI→D | `{sessionId}` |
| 0x0025 | `Terminate` / 0x0026 `SessionExited` | UI↔D | `{sessionId,force}` / `{sessionId,exitCode}` |
| 0x0027 | `Resize` | UI→D | `{sessionId,cols,rows}` → `ResizePseudoConsole` |
| 0x0030 | `AgentEvent` | D→UI | agent 状态机事件（配合 §10.4 的统一事件） |

会话级：

| type | 名称 | 方向 | payload |
| --- | --- | --- | --- |
| 0x0100 | `Data` | D→UI | 原始 PTY 字节（`flags.payload_is_raw=1`），带 `sessionId` + `seq` |
| 0x0101 | `Input` | UI→D | 原始按键字节（同样 raw），写入 ConPTY 输入通道 |
| 0x0102 | `Ack` | UI→D | `{sessionId, ackedBytes}`（累计已消费字节数） |

---

## 四、流控、背压与批处理

### 4.1 链路被切成三段，三段都要管

```
ConPTY ──(A)──▶ Daemon ──(B) Named Pipe ──▶ Tauri 核心 ──(C) WebView2 ──▶ xterm.js
```

- **(A) ConPTY → Daemon**：守护进程**必须持续排空输出管道**，否则 ConPTY 侧会阻塞子进程。控制手段是「不读」——ConPTY 的输出管道是普通管道，停止 `ReadFile` 会把背压传导到 ConPTY / 子进程。VS Code 在 Windows 上正是用 node-pty 的 `pause()/resume()` 停读 ConPTY。⚠️ 代价：停读期间 `Ctrl-C` 等输入响应变差，需用高低水位、不要在每块上频繁切换。
- **(B) Daemon → Tauri 核心（Named Pipe）**：管道缓冲满时 `WriteFile` 会阻塞（字节型管道会写满已缓冲部分）。这是「管道级背压」，但**不够**——见 (C)。
- **(C) Tauri 核心 → WebView2 → xterm.js**：这一段**不可阻塞**，缓冲无界，且 Tauri 事件系统明确不适合高吞吐。**真正的端到端背压必须在 xterm.js 解析完数据后产生 ack，再逆流回守护进程。** 这是本设计里最重要的一条。

### 4.2 ack / credit 协议（参照 VS Code + xterm.js）

- 守护进程为**每个会话**维护 `unacked = 已发送字节 − 已确认字节`。
- 客户端（渲染器）每消费约 **16–64 KiB** 数据发一次累计 `Ack`（合批，避免 ack 风暴）。
- 守护进程：`unacked > HIGH` → 暂停该会话的对 UI 发送；必要时同时暂停读 ConPTY（A 段）；`unacked < LOW` → 恢复。
- 起点参数：VS Code 用 **100000 / 5000 / ack 5000**（单位是 JS 字符）；xterm.js 官方指南建议 `HIGH ≤ 500K` 才能保证 `yes` 时 Ctrl-C 可响应。建议我们初值 `HIGH = 256 KiB, LOW = 64 KiB, ACK_EVERY = 32 KiB`，再按 `yes` / `cat 大文件` 实测调。
- **每个会话独立计账**：一个后台会话的洪峰不应拖垮前台交互会话（与 §五 的优先级配合）。

### 4.3 批处理 / 合并（确定事实，均可借鉴）

| 实现 | 合并策略 | 数值 |
| --- | --- | --- |
| WezTerm | PTY 读线程 → 解析线程，按换行切分；无换行则等待再攒 | 等待窗口 ~**50 ms**；`mux_output_parser_buffer_size` 可配 |
| Contour | 服务端解析后做「20 ms 防抖」再推 delta | **20 ms** |
| Windows Terminal | overlapped 读 + 双缓冲，先投下一次读再处理当前块 | 缓冲 **128 KiB** |
| xterm.js | 写缓冲按时间片解析，避免阻塞 UI 线程 | 单次 **12 ms**，硬上限 **50 MB（超出丢弃）** |

**对本项目的建议**：守护进程收到 ConPTY 字节后，在一个短窗口（建议 **5–15 ms**）内合并成尽量大的帧（但不超过 `MAX_FRAME`）；同时保留「空闲后首字节立即冲刷」的快速路径，保证交互敲键不被 batching 拖延迟。Contour 的教训值得记：它最初对快照「一次性塞进无界队列」，导致 scrollback 多的会话**永远 attach 不上**；修正办法是把大快照**分片**并让生产者按水位自我节流。

---

## 五、控制通道如何与数据流分离

三种做法：

| 做法 | 说明 | 评价 |
| --- | --- | --- |
| **A. 单连接、类型多路复用** | 同一字节流里 `Data` 与 `Resize/Ack/Ping` 混排 | 实现最省；但必须保证控制帧能插队，否则洪峰饿死控制面 |
| **B. 单连接 + 控制优先级队列** | 数据帧可被合并/丢弃（仅回放场景），控制帧走独立小队列并永远先 flush；限制每连接在途数据字节 | ✅ 推荐起步 |
| **C. 两条 Named Pipe** | 控制一条、数据一条（或「每条用户连接一个数据实例」） | ✅ 隔离最干净；代价是两条连接的重连/生命周期要一起管 |

**必须避免的坑（真实先例）**：WezTerm mux-server 在持续 `PaneOutput` 下，因「每连接无界队列 + 单一调度线程」，把 `GetCodecVersion` / `ListPanes` / 新 attach 全部饿死到超时（issue #7692；报告者给出的方向正是「限界/合并通知队列、让请求-响应优先于 `PaneOutput` 推送」）。**确定结论**：无论选 A/B/C，都要有「控制帧优先 + 数据在途字节有上限 + 洪峰时合并而非堆叠」三条。

其它控制/数据分离的具体处理：
- `Resize`、`Terminate`、心跳：小且少，天然适合控制面；
- `Ack`：频率较高但极小，建议与 `Ping/Pong` 同级（可牺牲、可在恢复后重算），绝不能排在 `Data` 后面挨饿；
- 交接语义：`Detach` ≠ `Terminate`（Contour 专门强调过这点）——「关 UI」只 `Detach`，会话必须继续活在守护进程里，符合 `REQUIREMENTS.md` §5.2 / §12 的三层退出语义。

---

## 六、版本协商、能力位、握手与重连

### 6.1 版本与能力位（建议）

- `Hello` 携带 `protoMin/protoMax`（客户端支持区间）+ `caps` 位图 + 客户端构建信息；`Welcome` 返回守护进程选定的 `proto` + 自己的 `caps`。
- 兼容策略：取双方交集最高版本；**未知帧类型一律忽略**（WezTerm 把未知 ident 解成 `Invalid{ident}` 当数据处理，而不是报错断连）。
- 能力位示例：`CAP_BINARY_PAYLOAD`（数据帧原始字节）、`CAP_COMPRESSION`、`CAP_AGENT_EVENTS`、`CAP_MULTI_ATTACH`。
- 对比：Contour 现在是**精确匹配**握手（差一个版本就拒绝，且拒绝时故意用和「令牌不匹配」一样的外形，不泄露版本信息），代价是「用户升级 app 后忘了重启旧守护进程」时得不到任何诊断。Ageminal 是本地单用户，**建议采用「版本区间 + 能力位 + 明确错误信息」**，牺牲一点安全隐晦度换可排障性。

### 6.2 握手时序（建议）

```
UI                                   Daemon
│  Hello{protoMin,protoMax,caps,       │
│        token, clientInstanceId,      │
│        resume:[{sid,lastSeq}]}       │
│─────────────────────────────────────▶│  校验 token / 版本 / ACL
│◀─────────────────────────────────────│  Welcome{proto,caps,daemonVersion}
│  （对每个 resume 的会话）Attach{sid} │
│─────────────────────────────────────▶│
│◀─────────────────────────────────────│  Attached{sid, fromSeq, replayBytes}
│◀─────────────────────────────────────│  Data{sid, seq=fromSeq..}（回放，is_replay）
│◀─────────────────────────────────────│  Data{sid, seq..}（实时，续流）
│  Ack{sid, ackedBytes}                │
│─────────────────────────────────────▶│  恢复/继续
```

### 6.3 断线检测（确定事实 + 建议）

- **读返 0** 或 **`ERROR_BROKEN_PIPE`**：Windows Terminal 用它判断 ConPTY 连接断开；对 Named Pipe 同样适用。这是最快的「对端没了」信号，不要只靠心跳。
  补充：客户端用 `CreateFile` 连不上时可能是 `ERROR_PIPE_BUSY`（所有实例忙），需 `WaitNamedPipe` 等待。
- **心跳**：`Ping/Pong`（WezTerm 有 1/2 号 PDU）。建议 5 s 发一次、15 s 无 `Pong` 视为死连接；注意洪峰时若控制帧被数据饿死，心跳会误判——这正是 §五 必须做优先级的原因。
- **断线语义要分清**：UI 崩了 → 守护进程 `read` 返 0 → `Detach` 该客户端，会话保留；守护进程崩了 → UI 侧 `read` 报错 → 提示「会话已随守护进程退出」（`REQUIREMENTS.md` §5.4：跨守护进程崩溃只能尽力恢复元数据，进程无法复活）。

### 6.4 重连与回放（确定事实参照 + 建议）

- 用**每会话、每方向单调递增的 `seq`** 给数据帧编号。
- 重连时 `Hello.resume` 带 `lastSeq`；守护进程从环形缓冲里**从 `lastSeq` 之后**补齐，再续实时流。Contour 的 `Attached{fromSeq}` + 快照 + 20 ms delta 是同一思路。
- 回放帧用 `flags.is_replay` 标记，UI 据此在缓冲回放与实时流之间插入「—— 已重新连接 ——」分隔（对齐 `REQUIREMENTS.md` §7.5 的呈现要求）。
- 环形缓冲**有界**：溢出时旧字节丢弃，但要让 UI 知道「你的 `lastSeq` 已过期」，退化为「从头回放当前缓冲」并插入分隔提示（WezTerm 的做法是维护独立 serial 并在快照时校验上限）。

---

## 七、与 Tauri / WebView 边界的关系（易被忽视，但决定成败）

- **谁在说协议**：Named Pipe 的客户端是 **Tauri 应用的 Rust 核心（`src-tauri/`）**，不是 WebView 里的 JS。WebView 无法直接开命名管道。
- **最后一跳**：Tauri → 前端有两条路：
  - **事件系统 `emit`/`listen`**：Tauri 官方明确「适合小数据、多播；**不适合低延迟 / 高吞吐**」，且 payload 一律 JSON 字符串。**不要**用它推 PTY 数据。
  - **Channel（`tauri::ipc::Channel`）**：官方定位就是「快、有序，用于下载进度 / 子进程输出 / WebSocket」等流式场景，正是我们该用的。
- ⚠️推测：即便用 Channel，WebView2 的 `postMessage` 仍要跨进程拷贝，前端 `onmessage` 又受 JS 主线程调度影响，这一段很可能就是全链路吞吐上限。因此 §4 的 ack 必须止于 xterm.js，而不是止于 Tauri 核心。
- 建议：Rust 侧一条连接对应一个 Tauri `Channel`；每条 `Data` 帧转成一次 `Channel.send`（或按 §4.3 合并后发）；xterm.js `write(bytes, cb)` 在 `cb` 里累计字节、批量回 `Ack` 给守护进程。

---

## 八、风险清单

| # | 风险 | 依据 / 说明 | 缓解 |
| --- | --- | --- | --- |
| R1 | **WebView 最后一跳是吞吐瓶颈** | Tauri 官方说事件不适合高吞吐；xterm.js 上限 5–35 MB/s、50 MB 后丢弃 | 用 Channel + `Uint8Array`；ack 落在渲染器；必要时降帧率 / 合并 |
| R2 | **控制面被数据洪峰饿死** | WezTerm #7692 | 控制帧优先 + 数据在途有界 + 合并 |
| R3 | **停读 ConPTY 拖慢 Ctrl-C** | VS Code 用 pause/resume 的既有代价 | 高低水位 + 只在必要时停读 |
| R4 | **UTF-8 跨块切断** | ConPTY 输出是 UTF-8 字节流 | 全链路保持字节，不在中间转 JS 字符串 |
| R5 | **管道默认权限过宽 / 可远程** | CreateNamedPipe 默认 DACL + 远程可访问 | `PIPE_REJECT_REMOTE_CLIENTS` + 限当前用户 DACL + 令牌 |
| R6 | **消息模式 + `ERROR_MORE_DATA` 组合易错** | MS 文档 + tokio 未见特判 | 直接用字节模式 + 自研分帧 |
| R7 | **守护进程崩溃丢失运行中进程** | 进程无法跨守护进程崩溃复活 | 元数据落盘、UI 明确提示（§5.4 已知边界） |
| R8 | **管道名冲突 / 旧守护进程残留** | 单例探测歧义 | `FILE_FLAG_FIRST_PIPE_INSTANCE` + 版本握手暴露旧实例 |

---

## 九、待用户拍板的点

> ✅ **已定 —— B2 #5 / B4 #7 / B5 #8**，且协议子集**已在真机跑通（#40）**。

1. **控制面形态**：单管道 + 控制优先队列（省事）还是**两条 Named Pipe**（最干净）？（§五 A/B/C）
2. **Tauri 前端投递**：走 `Channel` + `Uint8Array`（推荐），还是先用事件 + base64 快速出原型？后者会在数据量上来后必须重写。
3. **流控阈值**：是否接受 VS Code 式 `HIGH/LOW/ACK`（建议初值 256 KiB / 64 KiB / 32 KiB）？是否允许守护进程在洪峰时**停读 ConPTY**（会牺牲 Ctrl-C 响应）？
4. **版本策略**：区间 + 能力位（推荐）还是 Contour 式精确匹配？旧守护进程残留时给不给明确诊断？
5. **是否要压缩**：WezTerm 对 >32 B 的 PDU 用 zstd（重复性高的 VT 输出可能受益，但 `cat` 图片类负载收益低、CPU 成本高）。MVP 建议不做，能力位预留。
6. **共享内存**：是否列入后期优化方向（需要真实压测数据支撑）。
7. **ring buffer 大小与是否落盘**（输出缓冲可选落盘，见 §5.4）。
8. **安全模型细节**：令牌放哪（`%LOCALAPPDATA%\Ageminal\daemon.token` + ACL？），是否额外校验客户端 PID/SID。

---

## 十、来源链接

**Windows Named Pipe / 进程间通信（一手）**
- Named Pipe Type, Read, and Wait Modes — <https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-type-read-and-wait-modes>
- CreateNamedPipeW（类型/读/等待模式、缓冲、DACL、`PIPE_REJECT_REMOTE_CLIENTS`、`FILE_FLAG_FIRST_PIPE_INSTANCE`）— <https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-createnamedpipew>
- Named Pipes（远程可访问性、实例）— <https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipes>
- Named Pipe Operations（`TransactNamedPipe`、`FlushFileBuffers`、`DisconnectNamedPipe`）— <https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-operations>
- Named Pipe Client（客户端初始字节读模式、消息读 + `ERROR_MORE_DATA` 示例）— <https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-client>

**ConPTY / 终端侧（一手）**
- Creating a Pseudoconsole session（独立线程警告、resize/close、死锁）— <https://learn.microsoft.com/en-us/windows/console/creating-a-pseudoconsole-session>
- Introducing the Windows Pseudo Console (ConPTY)（输入输出为 UTF-8 文本 / VT）— <https://devblogs.microsoft.com/commandline/windows-command-line-introducing-the-windows-pseudo-console-conpty/>
- Windows Terminal `ConptyConnection.cpp`（overlapped 128 KiB 读、双缓冲、`ERROR_BROKEN_PIPE`、`read==0` 关闭）— <https://github.com/microsoft/terminal/blob/main/src/cascadia/TerminalConnection/ConptyConnection.cpp>

**Rust 库（一手源码 / 文档）**
- `tokio::net::windows::named_pipe`（`PipeMode::{Byte,Message}`、默认 64 KiB、`reject_remote_clients`）— <https://docs.rs/tokio/latest/tokio/net/windows/named_pipe/> · PipeMode — <https://docs.rs/tokio/latest/tokio/net/windows/named_pipe/enum.PipeMode.html>
- `interprocess::os::windows::named_pipe::pipe_mode`（`Bytes` / `Messages`）— <https://docs.rs/interprocess/latest/x86_64-pc-windows-msvc/interprocess/os/windows/named_pipe/pipe_mode/index.html>
- `portable-pty`（ConPTY 封装，终端底层）— <https://crates.io/crates/portable-pty>

**同类守护进程协议（一手源码 / 官方文档）**
- WezTerm codec（leb128 长度 + serial + ident + payload；`CODEC_VERSION = 45`；未知 ident → `Invalid`；压缩标记位）— <https://github.com/wezterm/wezterm/blob/main/codec/src/lib.rs>
- WezTerm mux 输出合并与背压（换行切分 + 50 ms 等待；「阻塞到 mux 消费完」）— <https://github.com/wezterm/wezterm/blob/main/mux/src/lib.rs>
- WezTerm mux-server 控制面饥饿报告 — <https://github.com/wezterm/wezterm/issues/7692>
- Contour Daemon mode（native cells+deltas：`varint taggedLength/serial/ident/payload`、`ClientHello/ServerHello` 精确匹配、`MaxFrameSize`、写队列限界/快照分片/水位、20 ms 防抖、Windows AF_UNIX + ConPTY）— <https://contour-terminal.org/internals/vthost/>
- Contour Persistent sessions（用户向）— <https://contour-terminal.org/persistent-sessions/>

**流控 / 背压（一手）**
- VS Code `terminalProcess.ts`（高低水位 + ack）— <https://github.com/microsoft/vscode/blob/main/src/vs/platform/terminal/node/terminalProcess.ts>
- VS Code `terminal.ts` `FlowControlConstants`（100000 / 5000 / 5000）— <https://github.com/microsoft/vscode/blob/main/src/vs/platform/terminal/common/terminal.ts>
- VS Code `terminalInstance.ts`（在 xterm.js `write` 回调里 ack）— <https://github.com/microsoft/vscode/blob/main/src/vs/workbench/contrib/terminal/browser/terminalInstance.ts>
- xterm.js Flow Control 指南（非阻塞 write、5–35 MB/s、50 MB 丢弃、建议 HIGH ≤ 500K、websocket ACK 模式）— <https://xtermjs.org/docs/guides/flowcontrol/>
- xterm.js `WriteBuffer.ts`（`DISCARD_WATERMARK`、`WRITE_TIMEOUT_MS`）— <https://github.com/xtermjs/xterm.js/blob/master/src/common/input/WriteBuffer.ts>

**Tauri / WebView 边界（一手）**
- Calling the Frontend from Rust（事件不适合高吞吐；Channel 用于流式）— <https://v2.tauri.app/develop/calling-frontend/>
- `tauri::ipc::Channel` — <https://docs.rs/tauri/latest/tauri/ipc/struct.Channel.html>

**其它**
- RFC 4648（Base64：3 字节 → 4 字符，+33%）— <https://www.rfc-editor.org/rfc/rfc4648>
- AF_UNIX comes to Windows（Win10 17063+，仅 stream、无 `SCM_RIGHTS`）— <https://devblogs.microsoft.com/commandline/af_unix-comes-to-windows/>
- wmux（同问题现有实现；Windows 原生 IPC，非 POSIX 模拟）— <https://github.com/shreshthkapai/wmux>

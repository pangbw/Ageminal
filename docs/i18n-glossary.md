# i18n 术语表

> **领域定义以 `CONTEXT.md` 为准**（它是术语的单一事实来源，§7 已列出不译清单）；
> 本文件只补两件 i18n 特有的事：① §11 点名、而 `CONTEXT.md` 没收的**不译词**；
> ② 其余界面文案落到 **en-US** 时用哪个词。
> 维护顺序：先改 `CONTEXT.md`，再回来补这里的英文列。

## 不译：原样照抄

`CONTEXT.md` §7 的清单（`worktree` · `agent` · `daemon` · `commit` · `checkpoint` ·
`scrollback` · `pane` · `Acrylic` · `Mica` · `VT` · `ConPTY` · `SGR` · `alt-screen` ·
`pipe` · `attach` / `detach` · `token`）之外，再加：

| 词 | 说明 |
| --- | --- |
| rebase / stash / branch / HEAD / detached | git 的子命令与状态（`commit` 已在 §7） |
| Claude Code / Codex / opencode / pi | agent 产品名 |
| Ageminal / Ageminal Mono / codicon | 本产品、字体与图标集的名字 |
| `sid` / `incarnationId` / `launch spec` | 内部标识与结构名（`CONTEXT.md` §2） |
| JSON / TOML / CLI / IPC / PTY / Named Pipe | 技术名词 |

## 中英对照（界面用词）

`CONTEXT.md` 定词，这里定英文落笔；左右两列指的是同一个概念。

| zh-CN | en-US |
| --- | --- |
| 项目 | Project |
| 用户工作区 | User workspace |
| 主工作树 | Main worktree |
| 页签 | Tab |
| 面板 | Pane |
| 终端 | Terminal |
| 会话 | Session |
| 恢复（agent 对话） | Resume |
| 重建（纯 shell 页签） | Rebuild |
| 快照 / checkpoint | Snapshot / checkpoint |
| 回放 | Replay |
| 流控 | Flow control |
| 文件树 | File tree |
| 只读预览 | Read-only preview |
| 集成 | Integration |
| 集成状态 | Integration status |
| 统一状态 | Unified status |
| 降级模式 | Degraded mode |
| 外观 | Appearance |
| 快捷键 | Keybindings |
| 通用 | General |
| 关于 / 许可 | About / Licenses |
| 导出诊断 | Export diagnostics |
| 结束所有会话并退出 | End all sessions and quit |

## 加一个域或语言

1. **域**：`src/i18n/namespaces.ts` 的 `NAMESPACES` 加一项，两个 locale 目录各加同名 JSON，
   再在 `src/i18n/index.ts` 的两处映射各加一行；漏改会被 `satisfies` 与 parity 用例挡住。
2. **语言**：`src/i18n/detect.ts` 的 `LOCALES` 加一项（有内容的语言），
   内容补齐后再进 `SELECTABLE_LOCALES`（MVP 只列 `zh-CN`）。
3. **两道门禁**（`src/i18n/__tests__/`，由 `pnpm test:unit` 跑）：
   - `no-chinese-literals.test.ts`：`src/**` 里除注释与 `src/i18n/locales/**` 外不得出现中文；
   - `locale-parity.test.ts`：**zh-CN 是类型源**，其余语言的 key 集合必须与它完全一致（值可空）。

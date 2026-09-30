# i18n 术语表

> 依据 `REQUIREMENTS.md` §11（i18n）与 `CONTEXT.md`（领域词汇）。
> 界面文案一律落在 `src/i18n/locales/<locale>/<域>.json`；本文件负责**不译词**与**中英对照**，
> 免得每个域各拿各的主意。

## 不译：原样照抄

| 词 | 说明 |
| --- | --- |
| worktree | git 语义的工作树；也不译作「工作树」 |
| agent | 泛指 coding agent，不译作「智能体」 |
| commit / rebase / stash / branch / HEAD / detached | git 的子命令与状态 |
| Claude Code / Codex / opencode / pi | 产品名 |
| Ageminal / Ageminal Mono / codicon | 产品名与字体名 |
| session / tab / pane | 与 `CONTEXT.md` 的 `sid` 强绑定，避免中英互译时串味 |
| `sid` / `incarnationId` / `launch spec` | 内部标识与结构名 |
| JSON / TOML / CLI / IPC / PTY / ConPTY / Named Pipe | 技术名词 |

## 中英对照（界面用词）

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
| 快照 | Snapshot |
| 回放 | Replay |
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

1. **域**：`src/i18n/namespaces.ts` 的 `NAMESPACES` 加一项，两个 locale 目录各加同名 JSON；
   `index.ts` 的 `satisfies` 与 parity 用例会一起把门。
2. **语言**：`src/i18n/detect.ts` 的 `SUPPORTED_LOCALES` 加一项，`locales/<locale>/` 补齐同名文件；
   检测链（`plugin-os.locale()` → `navigator.language` → `zh-CN`）由 `normalizeLocale` 决定谁归到谁。
3. **两道门禁**（`src/i18n/__tests__/`，由 `pnpm test:unit` 跑）：
   - `no-chinese-literals.test.ts`：`src/**` 里除注释与 `src/i18n/locales/**` 外不得出现中文；
   - `locale-parity.test.ts`：**zh-CN 是类型源**，其余语言的 key 集合必须与它完全一致（值可空）。

# Ageminal

多工作区管理的 ADE（Agent Development Environment）。

面向个人开发者：同时运行多个 coding agent（Claude Code / Codex 等），在一处管理多个仓库、多个 git worktree 的终端、文件与预览。

- **运行环境**：纯 Windows（默认 Git Bash，回退 PowerShell）；WSL 延后
- **形态**：Tauri 2 桌面应用（待开发）
- **许可**：完全开源，**Apache-2.0**
- **当前阶段**：实现起步（issue #50 骨架已就位）

## 目录

| 路径 | 说明 |
| --- | --- |
| `REQUIREMENTS.md` | 需求文档 v3（技术路线、进程与 Agent 集成、范围划分、验收标准） |
| `prototype/index.html` | 布局原型，单文件自包含，双击浏览器打开 |
| `docs/research/ui-editor-route.md` | 技术路线调研：Tauri vs GPUI、Zed 编辑器内核复用、终端选型 |

## 原型

单文件静态 HTML（内联 CSS/JS），中文，交互为模拟实现。主题为 **Zed「One Dark Pro Glass」**（近黑底 `#080909` + 毛玻璃 + 面板透明）。

左下角「原型导航」可在 **11 个屏幕**间跳转：

1. 主界面
2. 空状态
3. 设置 · 项目
4. 设置 · 通用 / 外观
5. 添加项目对话框
6. 新建页签菜单（含 Agent 入口）
7. 文件树右键菜单
8. Agent 启动入口
9. Agent 状态 / 通知
10. 会话恢复提示
11. 三栏拖拽 / 折叠

## 开发

```bash
pnpm install          # 安装前端依赖
pnpm tauri dev        # 开发运行（需在 Windows；Linux 需 GTK/WebKit 系统库）
pnpm check            # 本地全量检查：fmt / clippy / test / lint / tsc
pnpm build            # 前端构建（tauri.conf.json 的 beforeBuildCommand）
```

`pnpm check` 是**平台感知**的：Windows 上检查整个 workspace；其他平台跳过 `src-tauri`
（它需要 GTK / WebKit 系统库），只检查三个核心 crate。

## 状态

技术路线已定：**Tauri 2 + WebView2 + xterm.js + CodeMirror 6**，先做初版；**GPUI 原生方案**转入延后分支验证（只验证 Windows 中文 IME 终端）。完全开源。

仍待调研：前端框架、持久化方式、Tauri 通信协议、会话守护进程实现、Agent 集成注入方式。

需求文档分图例标注：**已确认 / 延后（含分支验证、专项调研）/ 待调研**。

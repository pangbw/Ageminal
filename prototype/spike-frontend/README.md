# spike 骨架 · Tauri 2 + Vue 3（#47 / #38 / #48 / #39）

throwaway 探针骨架，服务三张 wayfinder 票据：
**#47**（C1 前端框架选型 → 回写 #9）、**#38**（中文 IME 实测）、**#48**（通知点击实测）、**#39**（窗口毛玻璃实测）。
**冷启动与通知的数字必须来自 release build**；`tauri dev` 带 dev server 与未优化 bundle，且官方注明 Windows 通知**仅对已安装应用正常**。

界面顶部有四个视图切换：**终端池基准（#47）** / **#38 IME** / **#48 通知** / **#39 毛玻璃**，以及一个「导出报告」。

## 要验证的 4 件事（#47）

| # | 探针 | 判定 |
| --- | --- | --- |
| 1 | Tauri 2 + Vue 3 可构建可启动 | 窗口正常打开 |
| 2 | **xterm 终端池 + DOM re-parent**（20 worktree × 3 页签 = **60** 实例） | 切走再切回**不空白**、`open()` 始终为 1、内存有界、切换无可感卡顿 |
| 3 | **冷启动分解**（Rust setup / 前端） | 总时间 vs §15 目标 **< 2s** |
| 4 | **CodeMirror 6 只读** | 打开文件正确高亮、挂载/销毁无报错 |

对应已知风险：xterm.js issue **#4978**（`open()` 二次调用不渲染，仍 open）。本骨架的做法是
**每个会话只 `open()` 一次，切换页签靠 `appendChild` 搬运宿主 `<div>`（re-parent）**，从不重建实例。

## #38 · 中文 IME 探针

切到 **#38 IME** 视图。要做的只有一件事：**点终端区，用中文输入法打几个字（如「中文测试」）**，然后看右侧读数与事件日志。

判定用读数：

| 读数 | 看什么 |
| --- | --- |
| **helper 锚点** vs **期望光标** | **候选窗会跟随 helper textarea**；若两者 rect 长期不重合，即复现 **xterm.js #5454**（候选窗远离光标） |
| **网格对齐** | `8×ASCII` 与 `4×CJK` 的渲染宽度应相等（都占 8 格）→ 验证组合输入**不破坏等宽网格** |
| **已提交中文字数 / 事件日志** | 每个 `onData` 都打印**码点**与文本 → 确认中文是否被正确送达、组合期间是否被吞 |
| `compositionstart/update/end` | 与坐标快照联看，定位候选窗锚点在组合过程中的漂移 |

> 「期望光标」是用 `buffer.cursorX/Y` × **实测单元格尺寸**算出来的，**不依赖 DOM 光标元素**
> （xterm 6 的 DOM 渲染器未聚焦时不渲染它，见「命中选择器」一行）。

## #48 · 通知点击探针

切到 **#48 通知** 视图。**必须用安装后的 NSIS 版本跑**（官方注明 Windows 通知仅对已安装应用正常）：

```powershell
npm run tauri build            # 打 NSIS 安装器
# 安装后从开始菜单启动，然后：
#   ① 点「发送（Rust 路径）」→ 点通知 → 看是否出现任何回调
#   ② 点「发送（Web API）」→ 点通知 → 看 onclick 是否触发
#   ③ 记录点击是否至少激活/聚焦了应用
```

要回答的问题（探针里也写着）：Rust 路径点击后是否有 JS 回调？Web 路径 `onclick` 是否触发？`onAction` 在 Windows 是否触发？点击能否激活应用？

## #39 · 窗口毛玻璃探针

切到 **#39 毛玻璃** 视图。窗口为 **`decorations:false` + `transparent:true` + `shadow:true`**，`html`/`body`/根容器全透明。
（`transparent:true` 是 `window-vibrancy` 官方 Tauri 配方**要求**的，不是它破坏材质。）

### 要判的两件事

**① 切换效果是否需要「nudge」（1px 重绘）**

`window-vibrancy` 有已知问题类型：*效果要 resize 一下才生效*（issue #40）。所以：

1. **关掉**顶部「自动 nudge」→ 依次点 `tabbedDark` → `acrylic` → 再回 `tabbedDark`，看哪一步**视觉上没变**（清单 M2）
2. **打开**「自动 nudge」→ 同样顺序再点一遍，看是否**每次都真的变**（清单 M3）
3. 若已应用但看不见，可点 **↻ 重新应用当前效果** 补一次重绘

含义：nudge 能解决 → 产品里「设置页切换材质」可行；不能 → 只能**启动时定死**（本就符合 C7 #15 的「单一 build 检测效果」）。

**② Mica 到底有没有生效**

顶部**材质判定卡**：左「完全不透明 `#080909`」vs 右「alpha 0.10 几乎全透明」。
**把窗口移到壁纸纹理明显处**再看——左右**明显不同** = 材质真在生效；**完全一样** = 没生效（毛玻璃只是「半透明色」）。

### H10 自审修正（重要）

H9 的测量方法有缺陷，已在 H10 修掉：

- **alpha 阶梯与材质判定卡原先挂在 `.card`（不透明 `#0d0d0d`）里** → 测的是「在实色上混色」，**没测到材质**。
  现已改为 `.probe` 的**直接子元素**，直接贴透明窗口。⇒ **「alpha 0.25 合适」这个结论要重测**。
- **「材质检视」原先会把判定卡的"不透明参照块"也改透明** → 对比失效。现在检视只作用于面板（`.card`），参照块不动。
- **`.spike` 全局 55% 暗底已移除**（它会压掉一半材质），alpha 交给各面板承担。
- **探针视图改为可滚动**（原先超出 100vh 的部分被窗口裁掉、滚不到）。
- `nudge` 的尺寸单位改为**物理像素**（原先混用 `LogicalSize`，在 125%/150% 缩放下会**永久放大窗口**）。
- 界面里字面量 `**` 全部去掉。

### 官方性能告警（已写进按钮说明）

| 效果 | 官方支持 | 性能 |
| --- | --- | --- |
| `apply_mica` | Windows 11 | 无告警 → ✅ 首选 |
| `apply_acrylic` | Windows 10/11 | ⚠️ 拖动/缩放性能差（1903+ / 22000） |
| `apply_blur` | Windows 7/10/**11 仅 22H1** | ⛔ 22621+ 性能差 |

### 已确认（上几轮，已预填进报告）

- **`backdrop-filter` 采不到桌面像素**（WebView2 #4945）：Acrylic + 检视模式下 A/B 对照无差异
- 面板 **alpha 0.25** 合适
- 拖动标题栏 / 热区缩放时**背景闪烁**；切到 **Blur 或清除后切换不动**

## 运行

> `node_modules` **未**随仓库提供（避免带入 Linux 产物）。请在 **Windows** 上安装。

**A. Windows 本地目录（推荐，构建更快）**
```powershell
robocopy \\wsl.localhost\archlinux\home\pbw\Github\Ageminal\prototype\spike-frontend C:\dev\agm-h9 /E /XD node_modules target dist
cd C:\dev\agm-h9
npm install
npm run tauri -- build --no-bundle   # release exe（#47 / #38 / #39 用）
npm run tauri build                  # 带 NSIS 安装器（#48 用）
```

**B. 就地构建（经 \\wsl.localhost，cargo 会明显变慢）**
```powershell
cd \\wsl.localhost\archlinux\home\pbw\Github\Ageminal\prototype\spike-frontend
npm install
npm run tauri dev
npm run tauri -- build --no-bundle
```

**C. 纯前端（无 Tauri）**：`npm run dev` → 浏览器打开，可验证终端池 / re-parent / CodeMirror / IME 的 DOM 行为，
但**冷启动的 Rust 段与通知不可用**（界面会标注浏览器模式）。

release exe 路径：`src-tauri\target\release\spike-frontend.exe`

## 要记录的结果

**任一侧栏/顶部点「导出报告」**即把当前所有探针结果写进 **`%TEMP%\agm-spike-report.json`**（无需手抄）。报告含：

- `cold` / `marks` / `poolSize` / `heapMB` / `switch` / `reparent` / `codeMirror` —— #47 的四项
- **`probes.ime`** —— helper 与期望光标 rect、单元格尺寸、网格对齐、已提交中文字数、`onData` 码点与事件日志
- **`probes.notify`** —— 权限、两条路径的回调事件日志
- **`probes.window`** —— 系统 build、已应用效果、**`setEffects` 调用日志**、nudge 开关、清单与备注

启动后约 3 秒还会自动跑一轮 #47 基准（突发写入 + 切换压测）并落盘一次。

## 已知边界

- 这是**前端 + 外壳**探针，**不含**真实 ConPTY/守护进程（那是 M2 的 #40/#42）。
- 终端输出是模拟流（`setInterval`），不是 PTY 背压；吞吐/背压由 M2 的 #42 测。
- #38 只覆盖 **xterm + WebView2 的 IME 行为**；**ConPTY 光标跳动**（codex #35438）留给 #40。
- #48 的结论以**安装后**的运行为准；`tauri dev` 下通知显示 PowerShell 的名称/图标。

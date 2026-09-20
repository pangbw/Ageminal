# 技术调研：Tauri 2 在 Windows 的自绘标题栏与窗口毛玻璃

> **状态回写（2026-09-20）**：**#39 已真机实测，并取代本文 §3.3 / §5.2 的「版本化 `windowEffects`、以 Mica Alt 为主」方案。**
>
> **最终决议：默认实色；设置页可开启 Acrylic（需重启生效）。**
>
> - **Mica / Mica Alt 弃用** —— 可见性**取决于桌面壁纸**：近黑壁纸上与实色**无差别**（真机截图佐证），不适合做默认观感；
> - **Blur 弃用** —— 官方标注 **22621+** 拖动/缩放性能差（实测机 26200）；
> - 本文 §四的判断被实测证实：**`backdrop-filter` 采不到桌面像素**（Acrylic + 检视模式下 A/B 对照无差异）；
> - **自绘标题栏 A1–A5 全通过**（拖动 / 双击最大化还原 / 四边四角缩放热区 / 圆角阴影 / Aero Snap）；**Snap Layouts 不可用**（接受）；
> - 附带证伪：**「ConPTY 要靠 resize 才吐首屏」不成立**（#40）。

> 调研日期：2026-09-19
> 背景：需求 §11 / §18.12 要求「Tauri 2 在 Windows 上的窗口效果（Mica / Acrylic / Blur）配合 One Dark Pro Glass 主题的 `background.appearance: blurred` 实现真实背景模糊」，并已确认「隐藏系统标题栏、使用自绘标题栏，窗口最小尺寸 900×600」。本文核实纯 Windows 上的可行性与坑，产出物之一为交给 prototype 票（H2）的**待真机验证清单**。
> 记号：**确定事实**来自 Tauri / tao / wry / window-vibrancy 源码、Tauri 官方文档、Microsoft Win32/DWM 文档与 GitHub issue 原文；**⚠️推测**为无直接来源或需真机确认的推断。所有版本号以调研日为准。
> 关联：issue #15（wayfinder ticket `C7`）；主路线见 `docs/research/ui-editor-route.md`。

---

## 一、结论（TL;DR）

1. **自绘标题栏可行，是 Tauri 一等用法。** `decorations: false` + `data-tauri-drag-region`（或手动 `startDragging()`）即可；Tauri 的 `drag.js` 已内置「单击拖动 / 双击最大化」，Windows 上不存在 macOS 那种鼠标抬起语义问题。

2. **窗口毛玻璃在 Win11 22H2+ 走系统 Mica / Mica Alt（Tabbed）/ Acrylic，Win11 21H2 走未文档化的 Mica，Win10 1809+ 只能走未文档化的 Acrylic / Blur。整体能力充足，但必须自己做版本检测与降级**——Tauri 内置的 `windowEffects` 在失败时会**静默无效果**，在 Win10 上请求 Mica 甚至可能得到一个「完全透明、没有背景」的窗口（已知 issue #15478）。

3. **主题的 `background.appearance: blurred` 不能靠网页 `backdrop-filter` 实现「桌面背景模糊」。** Chromium / WebView2 的 `backdrop-filter` 只采样本页面的像素；当 WebView2 背景 alpha=0（透明窗口必需）时，它拿不到周围像素，官方明确「目前按设计如此」，问题仍 open（WebView2Feedback #4945）。因此**真实背景模糊必须由 OS 效果（Mica/Acrylic）提供**，主题面板用**半透明纯色**让 OS 材质透出来；`backdrop-filter` 只适合模糊页面内部互相叠放的内容。

4. **Snap Layouts（悬停最大化按钮弹出贴靠布局，Win11）在自绘标题栏下拿不到。** 它依赖窗口应答 `WM_NCHITTEST` 返回 `HTMAXBUTTON`，而鼠标事件被 WebView2 子窗口吃掉；Tauri 官方明确在 WebView2 支持之前不会实现（tauri #4531，仍 open）。**Aero Snap（拖到屏幕边缘贴靠）大概率仍可用**（拖拽走原生 `HTCAPTION`），但需真机确认。

5. **900×600 最小尺寸与 Snap Layouts 冲突。** Microsoft 要求应用最小宽度 ≤ 500 epx（推荐 ≤330 epx）才能贴进布局区；900 逻辑像素的最小宽会让布局菜单「弹得出来、贴不进去」。若坚持 900×600，就应放弃 Snap Layouts，或单独为最大化按钮做原生子窗口方案（社区已有可运行做法，但要自己写 Win32）。

6. **综合建议**：走「`decorations:false` + `transparent:true` + `windowEffects`（Win11 优先 `tabbedDark`，即 Mica Alt；Win10 回退 `acrylic`）+ 启动时按 Windows build 检测 + 面板半透明」的组合；`backdrop-filter` 仅用于应用内面板叠层。所有 OS 效果相关结论都进入 H2 真机清单。

---

## 二、自绘标题栏：`decorations: false` + `data-tauri-drag-region`

### 2.1 基本用法与权限（确定）

Tauri 官方《Window Customization》给出的标准做法：

- `tauri.conf.json`：`"decorations": false`；
- 在能力文件里放开 `core:window:default`、`core:window:allow-close`、`core:window:allow-minimize`、`core:window:allow-toggle-maximize`、`core:window:allow-start-dragging`；
- HTML 里给标题栏元素加 `data-tauri-drag-region`，按钮调 `minimize() / toggleMaximize() / close()`。
- 文档补充：`data-tauri-drag-region` **只作用于直接标注的元素**，要应用到子元素需逐个添加；Windows 上若想让触摸 / 手写笔也能拖，可对元素用 `*[data-tauri-drag-region] { app-region: drag; }`。
- 官方也给了手动实现：监听 `mousedown`，`e.detail === 2 ? toggleMaximize() : startDragging()`。
  来源：<https://v2.tauri.app/learn/window-customization/>

### 2.2 Tauri 实际实现（确定，来自源码）

拖拽区行为由 `crates/tauri/src/window/scripts/drag.js` 注入脚本实现：

- 在 `mousedown`（左键、`detail` 为 1 或 2）命中拖拽区时，`preventDefault()` + `stopImmediatePropagation()`；
- `detail === 2` → 调用 `plugin:window|internal_toggle_maximize`；否则 → `plugin:window|start_dragging`；
- 属性值语义：裸值 / `"true"` = **仅直接点击该元素**；`"deep"` = 子树任意位置；`"false"` = 禁用；
- 可点击元素（`A/BUTTON/INPUT/SELECT/TEXTAREA/LABEL/SUMMARY`、`contenteditable`、带 `tabindex`、交互性 `role`）默认**阻断**拖拽，除非它自己带 `data-tauri-drag-region`；
- macOS 上最大化改到 `mouseup`，Windows/Linux 用 `mousedown`。
  来源：<https://github.com/tauri-apps/tauri/blob/dev/crates/tauri/src/window/scripts/drag.js>

底层窗口（tao）关键事实：

- 无边框窗口**没有真正移除 `WS_CAPTION | WS_SYSMENU | WS_SIZEBOX | WS_MAXIMIZEBOX | WS_MINIMIZEBOX`**，只在 `AdjustWindowRectEx` 时去掉 `WS_CAPTION | WS_THICKFRAME`；无边框绘制靠 `WM_NCCALCSIZE` 返回 0 实现。
  来源：<https://github.com/tauri-apps/tao/blob/dev/src/platform_impl/windows/window_state.rs>、<https://github.com/tauri-apps/tao/blob/dev/src/platform_impl/windows/event_loop.rs>
- 拖拽 `start_dragging` → tao `drag_window()` → `handle_os_dragging(HTCAPTION)`：`ReleaseCapture()` + `PostMessage(WM_NCLBUTTONDOWN, HTCAPTION)`，即**走系统原生的 caption 拖动逻辑**。
  来源：<https://github.com/tauri-apps/tao/blob/dev/src/platform_impl/windows/window.rs>
- 窗口缩放在无边框时由 Tauri 自建的一个透明子窗口 `TAURI_DRAG_RESIZE_WINDOW` 处理：它覆盖整窗、用 `SetWindowRgn` 在中间挖空，边缘 8 个方向 `WM_NCHITTEST` 返回对应 `HT*` 并转发给父窗口；开启 undecorated shadow 时**只处理上边缘**，其余边缘交给 DWM 原生边框。
  来源：<https://github.com/tauri-apps/tauri/blob/dev/crates/tauri-runtime-wry/src/undecorated_resizing.rs>

### 2.3 已知坑（逐条核实）

| 关注点 | 结论 | 依据 |
|---|---|---|
| **双击最大化 / 还原** | 由 JS 的 `internal_toggle_maximize` 实现，不是系统 caption 双击。曾出现「双击还原后尺寸/位置不变」（#11945，已关闭、复现困难）；从最大化状态拖拽后双击偶发不灵。 | <https://github.com/tauri-apps/tauri/issues/11945>；`drag.js` |
| **系统菜单** | 窗口保留 `WS_SYSMENU`，且 tao 对 `WM_SYSCHAR` 走 `DefWindowProc`，所以 **Alt+Space 的系统菜单链路由系统处理**（源码层面）。**右键点自绘标题栏不会自动弹系统菜单**（没有 caption 命中，需自行调 `TrackPopupMenu`/`WM_NCRBUTTONUP`）。 | tao `event_loop.rs`；⚠️右键行为推测 |
| **Aero Snap（拖到边缘）** | 拖拽走原生 `HTCAPTION`，理论上保留系统贴靠；旧 issue「无边框窗口 Aero-snap 失效」（tao #103）已关闭。**⚠️ 需真机确认**（尤其透明 + 无边框组合）。 | tao `window.rs`；<https://github.com/tauri-apps/tao/issues/103> |
| **Snap Layouts（悬停最大化按钮）** | **不支持**。需要 `WM_NCHITTEST` 返回 `HTMAXBUTTON`，但点击落在 WebView2 子窗口，NC 消息到不了父窗口；WebView2 相关提案（#446 关闭、#4532 WCO 仍 open）不解决该路径。Tauri 官方明确「WebView2 支持前不会实现」。 | <https://github.com/tauri-apps/tauri/issues/4531>、<https://github.com/MicrosoftEdge/WebView2Feedback/issues/446>、<https://github.com/MicrosoftEdge/WebView2Feedback/issues/4532>、Microsoft <https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/apply-snap-layout-menu> |
| **900×600 与 Snap Layouts** | Microsoft 要求最小宽度 ≤ 500 epx（推荐 ≤ 330 epx），否则布局菜单会出现但窗口贴不进区。Ageminal 最小宽 900 逻辑像素 → **与 Snap Layouts 不兼容**（要么放弃该特性，要么做原生子窗口方案/放宽最小宽）。 | Microsoft 同上；§6 最小尺寸 |
| **窗口阴影 / 圆角** | `shadow` 默认 true。文档：Windows 上 `false` 对带装饰窗口无效；`true` 会让**无装饰窗口带 1px 白边，并在 Win11 上有圆角**。源码层面，undecorated shadow 会通过 `WM_NCCALCSIZE` 加内缩、并保留 DWM 边框，因此 Win11 可拿到系统圆角与阴影，Win10 是 1px 白边。`shadow:false` 则无阴影、方角，缩放全靠自建子窗口。 | Tauri config 文档 `<https://v2.tauri.app/reference/config/>`；tao `event_loop.rs` |
| **自定义圆角半径** | Win11 22000+ 可用 `DwmSetWindowAttribute(DWMWA_WINDOW_CORNER_PREFERENCE)` 让 DWM 做圆角（`DONOTROUND`/`ROUND`/`ROUNDSMALL`）；**没有自定义半径**，且 Microsoft 注明「只是 hint，不保证圆角」，使用逐像素 alpha 或窗口 region 的窗口不能圆角。 | Microsoft <https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/ne-dwmapi-dwmwindowattribute> |

> 社区参考（二次来源，但已真机验证）：有开发者用「在自绘最大化按钮上叠一个透明原生子窗口、其窗口过程无条件返回 `HTMAXBUTTON`」的方式恢复 Snap Layouts，并强调 `minWidth` 需 ≤500/330、`DWMWA_WINDOW_CORNER_PREFERENCE` 做圆角；验证环境 Win11 build 26200 / Tauri 2.11.5 / 100% 与 150% 缩放。该方案需要额外 Win32 代码，属于可选增强，不阻塞初版。
> <https://github.com/Zbrooklyn/tauri-snap-layouts> · <https://github.com/dunkyl/tauri-snap-layout>

---

## 三、窗口效果：能力矩阵与降级

### 3.1 Tauri 内置 API 与 window-vibrancy 的关系（确定）

- Tauri 2 的 `tauri::window::Effect` 枚举包含 Windows 侧变体：`Mica / MicaDark / MicaLight / Tabbed / TabbedDark / TabbedLight / Blur / Acrylic`；配置层为 `windowEffects: { effects, color, radius, state }`，**要求窗口 `transparent: true`**。
  来源：<https://docs.rs/tauri/latest/tauri/window/enum.Effect.html>、<https://v2.tauri.app/reference/config/>
- Tauri 的 Windows 实现直接调用 `window-vibrancy`（当前 Tauri 2.11.5 依赖 `window-vibrancy 0.6`）：`Effect::Mica*`→`apply_mica`、`Tabbed*`→`apply_tabbed`、`Acrylic`→`apply_acrylic`、`Blur`→`apply_blur`；**只取效果列表里第一个匹配的 Windows 效果，其余忽略**，且**忽略返回错误**（`let _ = ...`）。
  来源：<https://github.com/tauri-apps/tauri/blob/dev/crates/tauri/src/vibrancy/windows.rs>、<https://github.com/tauri-apps/tauri/blob/dev/crates/tauri/src/vibrancy/mod.rs>
- 运行时可用 `WebviewWindow::set_effects(...)` / `EffectsBuilder` 改效果，传 `None` 清除。
  来源：<https://docs.rs/tauri/latest/tauri/webview/struct.WebviewWindow.html#method.set_effects>

### 3.2 各效果在 Windows 版本上的可用性（确定，来自 `window-vibrancy` 源码）

`window-vibrancy` 以 Windows build 号判定（`windows-version`）：

- `is_swca_supported` = build ≥ **17763**（Win10 1809，`SetWindowCompositionAttribute`，**未文档化 API**）
- `is_undocumented_mica_supported` = build ≥ **22000**（Win11 21H2，`DWMWA_MICA_EFFECT = 1029`，未文档化）
- `is_backdroptype_supported` = build ≥ **22523**（Win11 22H2 起；Microsoft 文档标注公开 API 门槛为 **22621**）

来源：<https://github.com/tauri-apps/window-vibrancy/blob/dev/src/windows.rs>

| 效果 | 实现路径 | 最低版本 | 行为 / 备注 |
|---|---|---|---|
| **Mica**（`DWMSBT_MAINWINDOW`） | build ≥22523 → `DWMWA_SYSTEMBACKDROP_TYPE`；否则 22000–22522 → `DWMWA_MICA_EFFECT=1`；再低→报错（被 Tauri 吞掉） | Win11 | 不透明，只采一次壁纸，随主题/焦点变化；暗色需 `MicaDark`（会设 `DWMWA_USE_IMMERSIVE_DARK_MODE`） |
| **Tabbed / Mica Alt**（`DWMSBT_TABBEDWINDOW`） | 仅 build ≥22523；22000–22522 **无回退**，直接报错 | Win11 22H2+ | 桌面色着色更强，官方推荐给「带页签标题栏」的窗口——**与 Ageminal 顶部页签栏高度契合** |
| **Acrylic**（`DWMSBT_TRANSIENTWINDOW`） | build ≥22523 → DWM backdrop；否则 build ≥17763 → SWCA `ACCENT_ENABLE_ACRYLICBLURBEHIND` | Win10 1809+ | Win10 1903+ 起 `color` 参数有效；Win11 上忽略 `color`，且 `Acrylic` 在 Win10 1903+ / Win11 22000 上有**缩放/拖动性能问题** |
| **Blur**（`ACCENT_ENABLE_BLURBEHIND`） | Win7 → `DwmEnableBlurBehindWindow`；Win10 1809+ → SWCA | Win7 / Win10 1809+ | 最老的降级项；Win11 22621 上同样**缩放/拖动性能差** |
| Mica / Tabbed 的其它 | — | — | `DWMWA_SYSTEMBACKDROP_TYPE` 还有 `DWMSBT_AUTO` / `DWMSBT_NONE`；`DWMSBT_TRANSIENTWINDOW` 即桌面 Acrylic，`DWMSBT_TABBEDWINDOW` 即 Mica Alt |

来源（版本门槛 / 效果语义）：<https://github.com/tauri-apps/window-vibrancy/blob/dev/src/windows.rs>、<https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/ne-dwmapi-dwm_systembackdrop_type>、<https://learn.microsoft.com/en-us/windows/apps/design/style/mica>、<https://learn.microsoft.com/en-us/windows/apps/design/style/acrylic>

补充（确定）：

- Mica / Acrylic 会因系统设置自动降级为纯色：用户关闭「透明效果」、节电模式、低端硬件、**窗口失活**，或 Windows 版本过低时，Mica 回退到 `SolidBackgroundFillColorBase` 一类实色。因此**失活态与激活态观感不同，主题要能容忍**。
  来源：<https://learn.microsoft.com/en-us/windows/apps/design/style/mica>、<https://learn.microsoft.com/en-us/windows/apps/design/style/acrylic>
- Acrylic 渲染是 GPU 密集操作，可能增加耗电；Mica 只采一次壁纸、更省。
  来源：同上

### 3.3 降级策略与「静默失败」陷阱（确定 + 建议）

- **陷阱**：Tauri 内置 `windowEffects` 对 `window-vibrancy` 的错误不感知。已知 issue #15478 记录：Win11 用 Mica、Win10 上效果设置「成功」但窗口变成**完全透明、无任何背景**，且调用方无法判断是否真的生效（提交者因此绕过内置 API 直接依赖 `window-vibrancy`，又引发与 Tauri 固定版本的符号冲突）。**不要直接加 `window-vibrancy` 依赖**（会与 Tauri 的 `0.6` 冲突）。
  来源：<https://github.com/tauri-apps/tauri/issues/15478>
- **建议（⚠️工程方案，待验证）**：以 `windows-version`（或 `tauri-plugin-os` 的 `version()`）读 build 号，在**创建窗口时**决定传入哪一个 `windowEffects`：
  - build ≥ 22621：`["tabbedDark"]`（Mica Alt，页签场景），或 `["micaDark"]`；
  - 22000–22620：`["micaDark"]`（走未文档化 Mica）或直接 `["acrylic"]`；
  - 17763–21999（Win10 1809+）：`["acrylic"]`，并在 `windowEffects.color` 给一个贴近 `#080909` 的暗色（Win10 1903+ 才生效）；
  - < 17763：无 OS 效果，退回不透明 `#080909`。
  - 列表中**只放一个**目标效果（Tauri 只取第一个匹配项）。
- **注意**：`windowEffects` 是一次性应用（创建时）；若运行中切换主题/效果，用 `set_effects()` / `set_effects(None)`。暗色可用窗口 `theme: "dark"` 辅助。

---

## 四、透明窗口 + WebView2 的已知问题

### 4.1 透明是怎么实现的（确定）

- 配置 `transparent: true` 后，wry 把 WebView2 的 `DefaultBackgroundColor` 设为 `(0,0,0,0)`；`backgroundColor` 配置项在 Windows 上：窗口层 alpha 被忽略；WebView2 层若 alpha 非 0 会被强制成 255，所以**要透明就不要设不透明 `backgroundColor`**。
  来源：<https://github.com/tauri-apps/wry/blob/dev/src/webview2/mod.rs>、<https://v2.tauri.app/reference/config/>
- tao 在创建透明窗口时用 `DwmEnableBlurBehindWindow` + 空 region 实现整体透明；若配置 `noRedirectionBitmap: true` 则改设 `WS_EX_NOREDIRECTIONBITMAP`，并**跳过 DWM blur-behind**。
  来源：<https://github.com/tauri-apps/tao/blob/dev/src/platform_impl/windows/window.rs>、Tauri config 文档
- `noRedirectionBitmap` 的官方用途是**消除透明窗口创建时的白闪**。⚠️ 它与 Mica/Acrylic 是否兼容未验证（理论上会影响重定向表面 / DWM backdrop），列入真机清单。
  来源：Tauri config 文档

### 4.2 已知问题（确定）

| 问题 | 状态 / 结论 | 来源 |
|---|---|---|
| **透明 WebView2 下 `backdrop-filter` 不模糊桌面** | WebView2 官方：Chromium 设计使然——模糊需要周围像素，透明背景下没有像素可采；「正在研究」，截至 2025-08 仍 open（runtime 131 报告）。**这是主题 `background.appearance: blurred` 无法在页面内还原的关键。** | <https://github.com/MicrosoftEdge/WebView2Feedback/issues/4945> |
| Win10 透明窗口 `backdrop-filter` 不生效 | 早期 wry #408 结论同为 WebView2 限制（当时未实现）。 | <https://github.com/tauri-apps/wry/issues/408> |
| 无装饰 + 透明组合：原生标题栏没完全隐藏 / 拖动出现重影 | tao #1176（Win11 23H2 / tao 0.34.5）已关闭；当前版本需真机复验。 | <https://github.com/tauri-apps/tao/issues/1176> |
| 透明窗口启动瞬间闪烁标题栏 | tao #1240，已关闭；与 GPU（AMD）、渲染时序有关，非稳定复现。 | <https://github.com/tauri-apps/tao/issues/1240> |
| `decorations:false` 连续缩放**渐进式性能退化** | tao #1241：隔离测试显示纯 Win32/纯 Rust 宿主都正常，仅在 tao 无边框路径复现；已关闭，仍需在真实配置下观察。 | <https://github.com/tauri-apps/tao/issues/1241> |
| 抠图窗口里的 `backdrop-filter` 内容不更新 | wry #229，已随焦点 bug 修复关闭（macOS 场景为主）。 | <https://github.com/tauri-apps/wry/issues/229> |
| 用 `visibility` 做「防白闪」会把 Acrylic 变成 Mica | tauri #12854（open）：显示/隐藏 workaround 会改变背景材质。Ageminal 有「最小化到托盘 / 隐藏再显示」语义，需验证材质是否保持。 | <https://github.com/tauri-apps/tauri/issues/12854> |
| Mica 文字渲染瑕疵 | tauri #7984（closed）：官方回复 Mica 由 Windows 渲染、无法控制；社区发现显式用 `micaLight`/`micaDark` 可规避。 | <https://github.com/tauri-apps/tauri/issues/7984> |

### 4.3 输入 / 拖拽 / 性能相关（确定）

- 无边框窗口的拖动依赖 Tauri 注入脚本 + 权限；触摸/笔需 `app-region: drag`。
- 若要在页面内使用 HTML5 拖放（文件/元素），Windows 上需把 `dragDropEnabled` 设为 `false`（否则 Tauri 会替换 WebView2 的拖放处理器）。
  来源：Tauri config 文档
- Acrylic / Blur 在部分版本上「缩放/拖动时性能差」（Tauri `Effect` 文档明确标注）；`backdrop-filter` 与 Acrylic 叠加会进一步增加 GPU 负载。这是「毛玻璃 + 高刷拖动」的性能风险点。

---

## 五、与 One Dark Pro Glass 对齐的做法

### 5.1 语义拆解

主题 `background.appearance: blurred`（近黑底 `#080909` + 面板透明 + 毛玻璃，边框 `#212121`）包含两层模糊：

1. **窗口背后的桌面模糊（真实背景模糊）** —— 只能由 OS 效果（Mica 不透明采样壁纸 / Acrylic 实时模糊）提供；
2. **面板与其下页面内容之间的模糊（应用内毛玻璃）** —— 理论上可由 CSS `backdrop-filter` 提供，但受 WebView2 #4945 限制：面板下方若是**透明窗口基底**则无像素可模糊；只有面板下方是**不透明/半透明的页面内容**时才有效。

### 5.2 建议做法（⚠️方案，待真机验证）

- 窗口层：`transparent: true` + `decorations: false` + `shadow: true` + 版本化 `windowEffects`（见 3.3），HTML/`body`/根容器 `background: transparent`。
  注意：社区经验指出 **`html` 与 `body` 都要透明**，只设 `body` 会露白底（二次来源，需复验）。
  来源（二次）：<https://github.com/tauri-apps/tauri/issues/3481#issuecomment>
- 面板层：用**半透明纯色**（如 `rgba(8,9,9,.55)`、边框 `#212121`）让 Mica/Acrylic 透出；不要把 `backdrop-filter` 当作「看到桌面模糊」的手段。
- `backdrop-filter` 保留给**面板压在应用内容之上**的场景（如滚动内容从顶部栏下穿过），此时下方有像素可采。
- 「失活态」：Mica/Acrylic 在窗口失活时会变实色，建议主题对失活不做额外处理，避免闪烁。
- 页签栏：若用 Mica，顶部页签栏下方应透出材质，符合官方「让 Mica 可见于标题栏」的指引；若想更强的层次，选 Mica Alt（`tabbedDark`），与顶部页签布局语义一致。
  来源：<https://learn.microsoft.com/en-us/windows/apps/design/style/mica>

### 5.3 与原型的关系

- 原型 `prototype/index.html` 的毛玻璃在浏览器里靠 `backdrop-filter` 模拟；真机里**不能把它当作验收依据**——真机整体观感由 OS 材质主导，`backdrop-filter` 只影响面板叠层。H2 应在真机上分别记录「有 OS 效果 / 无 OS 效果」两套观感。

---

## 六、DPI、900×600 与多显示器

- **尺寸是逻辑像素**：Tauri 的 `width/height/minWidth/minHeight` 均按逻辑像素；900×600 在 150% 缩放下对应 1350×900 物理像素。
  来源：Tauri config 文档
- **自绘标题栏的高度、按钮热区必须用 CSS 逻辑像素**：WebView2 会自动处理 `devicePixelRatio`；但 Windows 原生子窗口（缩放热区、可选的原生最大化按钮）需要与 CSS 尺寸对齐，社区方案明确要求「`titlebar_height`/`button_width` 必须和 CSS 一致」。
  来源（二次）：<https://github.com/Zbrooklyn/tauri-snap-layouts>
- **混合 DPI 多屏有已知问题**：tao #1297（open）——逻辑 `Position` 在混合缩放的多显示器上按枚举顺序选显示器，结果不确定；tao #652（open）——显示器被强制移除时主窗口可能「缩成一团、变透明、没有标题栏」。Min 尺寸在跨屏拖动时的物理换算需实测。
  来源：<https://github.com/tauri-apps/tao/issues/1297>、<https://github.com/tauri-apps/tao/issues/652>
- **最大化与最大尺寸**：decorations:false 时 `set_maximized(true)` 会忽略 `max_inner_size`（tao #464，open）。Ageminal 未设最大尺寸，影响有限，但说明无边框路径与原生窗口语义有偏差。
  来源：<https://github.com/tauri-apps/tao/issues/464>
- **Snap Layouts × 最小宽度**：见 2.3——900 最小宽使贴靠不可用。
- **DPI 缩放不改变 OS 材质的清晰度**：Mica/Acrylic 由 DWM 合成，缩放无关；但圆角/阴影内缩量随 DPI 计算（tao 用 `hwnd_dpi`），高 DPI 下需确认无 1px 缝隙或裁切。

---

## 七、待真机验证清单（交给 prototype 票 H2）

> 建议矩阵：Win11 22621+（22H2/23H2/25H2）、Win11 22000（若可得）、Win10 19045（22H2）、Win10 17763（1809），每种覆盖 100% / 150% 缩放；至少包含一台混合 DPI 双屏与一块 AMD GPU（闪烁类问题高发）。

**A. 标题栏与窗口行为**
1. `decorations:false` 下：拖动、双击最大化/还原、从最大化拖拽还原是否正常（对照 #11945）。
2. 拖到屏幕边缘是否触发 Aero Snap；Snap Layouts 确认不可用（记录交互缺口）。
3. Alt+Space 系统菜单是否弹出、位置是否正确；右键自绘标题栏是否需要自行实现菜单。
4. 触摸/手写笔：加 `app-region: drag` 后标题栏是否可拖；`app-region: drag` 是否误伤按钮/输入（配合 `no-drag`）。
5. 缩放热区：四边/四角在 125%/150% 下的命中与光标形状。
6. `shadow:true` 在 Win11 的圆角与阴影、Win10 的 1px 白边是否可接受；`shadow:false` 的观感与缩放。
7. 托盘「隐藏→显示」后，材质是否从 Acrylic 变成 Mica（对照 #12854）。

**B. 窗口效果与降级**
8. Win11 22621+：`tabbedDark` / `micaDark` 在 `transparent:true + decorations:false + shadow:true` 下是否正常渲染；失活态是否回退实色。
9. Win11 22000：未文档化 Mica 是否生效；失败时是否退回 `acrylic`。
10. Win10 1809/19045：`acrylic` + `color` 是否符合暗色主题；`blur` 作为再降级是否可用。
11. 版本检测与降级逻辑：在 Win10 上**确认不会得到「完全透明、无背景」窗口**（对照 #15478）。
12. `noRedirectionBitmap:true` vs `false`：启动白闪、以及是否破坏 Mica/Acrylic。
13. `backgroundColor` 与 `transparent` 的组合：确认不透明 `backgroundColor` 不会破坏透明。

**C. 主题对齐**
14. `html`/`body`/根容器全透明后，面板半透明色在 Mica 与 Acrylic 下的可读性（对照 §17 文案色 `#e6e9f0`）。
15. `backdrop-filter` 面板：在「透明基底之上」与「不透明内容之上」两种情形的实际表现，记录可用的使用范围。

**D. 性能 / 稳定性 / 多屏**
16. 无边框连续缩放 30–60s 是否出现渐进式退化（对照 #1241）；开启 Acrylic + `backdrop-filter` 时的 CPU/GPU。
17. 100%→150% 跨屏拖动、以及拔掉副屏后窗口状态（对照 tao #652 / #1297）。
18. 最小尺寸 900×600：在 100%/150%/200% 下窗口实际最小物理尺寸与布局是否可接受。

**E. 与其它需求联动**
19. 多窗口 🕓 延后，但需确认 `windowEffects` 在单实例/多窗口下不会串材质（对照 tauri #13312）。
20. 与「三层退出语义」「最小化到托盘」联动时材质的保持。

---

## 八、关键来源

**Tauri 官方**
- 自绘标题栏教程：<https://v2.tauri.app/learn/window-customization/>
- 配置参考（`windowEffects` / `WindowEffect` / `transparent` / `decorations` / `shadow` / `minWidth` / `backgroundColor` / `noRedirectionBitmap` / `dragDropEnabled`）：<https://v2.tauri.app/reference/config/>
- `Effect` 枚举：<https://docs.rs/tauri/latest/tauri/window/enum.Effect.html>
- `set_effects`：<https://docs.rs/tauri/latest/tauri/webview/struct.WebviewWindow.html#method.set_effects>
- 源码：drag.js <https://github.com/tauri-apps/tauri/blob/dev/crates/tauri/src/window/scripts/drag.js> · vibrancy/windows.rs <https://github.com/tauri-apps/tauri/blob/dev/crates/tauri/src/vibrancy/windows.rs> · undecorated_resizing.rs <https://github.com/tauri-apps/tauri/blob/dev/crates/tauri-runtime-wry/src/undecorated_resizing.rs>
- tao：window_state.rs <https://github.com/tauri-apps/tao/blob/dev/src/platform_impl/windows/window_state.rs> · event_loop.rs <https://github.com/tauri-apps/tao/blob/dev/src/platform_impl/windows/event_loop.rs> · window.rs <https://github.com/tauri-apps/tao/blob/dev/src/platform_impl/windows/window.rs>
- wry：webview2/mod.rs <https://github.com/tauri-apps/wry/blob/dev/src/webview2/mod.rs>
- window-vibrancy：README <https://github.com/tauri-apps/window-vibrancy> · windows.rs <https://github.com/tauri-apps/window-vibrancy/blob/dev/src/windows.rs> · API <https://docs.rs/window-vibrancy/latest/window_vibrancy/>

**Microsoft（Win32 / DWM）**
- `DWM_SYSTEMBACKDROP_TYPE`：<https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/ne-dwmapi-dwm_systembackdrop_type>
- `DWMWINDOWATTRIBUTE`（`DWMWA_SYSTEMBACKDROP_TYPE` / `DWMWA_WINDOW_CORNER_PREFERENCE` / `DWMWA_USE_IMMERSIVE_DARK_MODE` / `DWMWA_BORDER_COLOR`）：<https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/ne-dwmapi-dwmwindowattribute>
- `DwmSetWindowAttribute`：<https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/nf-dwmapi-dwmsetwindowattribute>
- Mica：<https://learn.microsoft.com/en-us/windows/apps/design/style/mica>
- Acrylic：<https://learn.microsoft.com/en-us/windows/apps/design/style/acrylic>
- Win32 应用 Mica：<https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/apply-mica-win32>
- Snap Layouts：<https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/apply-snap-layout-menu>

**相关 issue（Tauri / tao / wry / WebView2）**
- tauri #4531（Snap Layouts，open）· #15478（window-vibrancy 冲突 / Win10 静默透明，open）· #11945（双击还原，closed）· #3481（透明+圆角，closed）· #7984（Mica 文字瑕疵，closed）· #10064（透明+backdrop blur，closed）· #12854（visibility 改材质，open）· #13312（多实例主题干扰，open）· #11088（Win10 无边框顶部黑线，closed）
- tao #103（无边框 Aero Snap，closed）· #994（无边框缩放，closed）· #1241（缩放性能退化，closed）· #1240（透明窗口闪烁标题栏，closed）· #1176（无边框+透明重影，closed）· #652（多屏窗口塌缩，open）· #1297（混合 DPI 逻辑坐标，open）· #464（无边框最大化忽略最大尺寸，open）
- wry #408（Win10 透明+backdrop-filter，closed）· #229（透明+filter 不刷新，closed）
- WebView2Feedback #446（WM_NCHITTEST，closed）· #4945（透明 + backdrop-filter，**open**）· #4532（WCO，open）
- electron #30412（透明 + backdrop-filter，open，同根因）
- 社区方案（二次来源）：<https://github.com/Zbrooklyn/tauri-snap-layouts> · <https://github.com/dunkyl/tauri-snap-layout>

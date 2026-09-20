# 前端框架选型调研（React / Solid / Svelte / Vue，附 Preact）

> **状态回写（2026-09-20）**：**已定 —— C1（#9）：Vue 3**（官方 Vite SPA 模板）。选型 spike **#47 全部通过**：冷启动 **567.7 ms**（< 2 s 预算）、60 实例 re-parent 正确（面板内恒为 1 个宿主、`open()` 仅 1 次）、CodeMirror 只读挂载/销毁无误、0 报错。
>
> 派生约束：**终端池必须惰性 + LRU**（60 个预建实例 ≈ 180 MiB，池构建 ≈ 221 ms）。

> 调研日期：2026-09-19
> 对应 issue：#9「前端框架选型」（wayfinder 决策地图 `C1`，父地图 #1）
> 背景：Ageminal 已定 **Tauri 2 + WebView2 + xterm.js + CodeMirror 6**；本报告只解决「Web 前端用哪个框架」。参考 `REQUIREMENTS.md` §15 性能目标、§18.1 待办。
> 记号：**确定事实**来自官方文档 / npm registry / GitHub API / 实测；**⚠️推测**为无直接来源的推断。

---

## 一、结论（TL;DR）

1. **框架不是「能不能跑 Tauri」的门槛。** Tauri 官方的 `create-tauri-app` 同时维护 **React / Solid / Svelte / Vue / Preact** 模板，且 Tauri 自述「前端无关（frontend agnostic）」。这些候选全部是官方一等模板。
2. **真正决定集成成本的是 xterm.js / CodeMirror 6 的命令式模型，而不是框架的响应式模型。** 两者都自己持有并操作 DOM；正确做法是「**实例池 + 宿主元素复用**」，让高频 PTY 输出（`term.write()`）完全绕过框架。这一层做到位后，**终端吞吐与框架基本无关**。
3. **因此「细粒度性能」在本项目里只影响应用外壳**（项目/worktree 树、页签、agent 状态点、设置页）。20 worktree × 3 页签、文件树懒加载，这个体量对任何候选都绰绰有余。信号式（Solid / Svelte 5 / Vue refs）默认更省心，React 也可用（需 memo / 编译器等）。
4. **产物体积差距真实存在，但在真实应用里被 xterm/CodeMirror 稀释。** 实测官方 Tauri 模板前端产物（gzip，见 §七）：Solid ≈ 4 KB、Preact ≈ 7 KB、Vue ≈ 25 KB、Svelte（SvelteKit 静态 SPA）≈ 29 KB、React ≈ 67 KB；而 xterm.js 6.0.0 单文件就 ≈ **121 KB gzip**。框架运行时的差值是二阶的。
5. **冷启动 < 2s 的主导项是 WebView2 初始化 + Rust 侧启动 + 编辑器/终端加载，不是框架运行时。** ⚠️推测：框架选择对冷启动的可测影响很小，应在 spike 阶段实测而非按此维度决策。
6. **推荐**：默认 **Vue 3**（生态/i18n/组件库/中文社区/与 Tauri 模板最省事的组合），性能取向可选 **Solid**（架构契合度最佳、产物体积最小）。**React** 在团队已熟悉时完全可用；**Svelte 5** 是优秀备选，但其官方 Tauri 模板是 **SvelteKit**（多一层），且 i18n 生态是短板；**Preact** 无决定性优势，仅作极小体积备选。
7. **最终决策强烈依赖「团队/个人最熟悉哪个框架」——这一项本报告无法替你回答**，见 §十一「待用户拍板」。

---

## 二、候选、版本与许可证

以下版本来自 npm registry 的 `dist-tags.latest`（查询日期 2026-09-19）：

| 框架 | 最新版 | 发布日期 | 许可证 | GitHub stars | 最近推送 |
|---|---|---|---|---|---|
| React / ReactDOM | `19.3.0` | 2026-09-09 | MIT | 250,581 | 2026-09-18 |
| Vue | `3.5.43` | 2026-09-17 | MIT | 54,412 | 2026-09-19 |
| Svelte | `5.57.1` | 2026-09-18 | MIT | 88,134 | 2026-09-19 |
| Solid | `1.9.15` | 2026-08-17 | MIT | 36,052 | 2026-09-19 |
| Preact | `10.29.8` | 2026-08-01 | MIT | 38,875 | 2026-09-15 |
| Tauri | `@tauri-apps/api 2.11.1` | 2026-06-17 | Apache-2.0/MIT | 111,177 | 2026-09-19 |
| xterm.js | `@xterm/xterm 6.0.0` | 2025-12-22 | MIT | 21,200 | 2026-09-13 |
| CodeMirror | `codemirror 6.0.2` / `@codemirror/view 6.43.12` | 2026-09-15 | MIT（核心） | 7,818 | 2026-04-15 |

来源：<https://registry.npmjs.org/> · <https://api.github.com/repos/...>（经 `gh api`）。
**确定事实**：五个候选与 Tauri、编辑器内核均为宽松许可证，与 Ageminal「完全开源」定位兼容。

> CodeMirror 的 `codemirror/dev` 仓库在 GitHub API 上标记 `archived=true`，但这是**元仓库**：核心包 `@codemirror/view` 仍在活跃发布（2026-09-15）。**不要**据此得出「CodeMirror 已停止维护」的结论。来源：npm registry + `gh api repos/codemirror/dev`。

---

## 三、Tauri 2 官方 / 社区支持与模板成熟度

**确定事实**

- `create-tauri-app` 官方维护的 JS 模板预设：`vanilla`、`vue`、`svelte`、`react`、`solid`、`angular`、`preact`（另有 Rust 系 `yew`/`leptos`/`sycamore`、`.NET` Blazor、`dioxus`）。模板目录实际存在：`template-react-ts`、`template-solid-ts`、`template-svelte-ts`、`template-vue-ts`、`template-preact-ts`。
  来源：<https://v2.tauri.app/start/create-project/> · <https://github.com/tauri-apps/create-tauri-app>（`templates/` 目录，2026-09-19）。
- Tauri 文档明确：「Tauri is frontend agnostic and supports most frontend frameworks out of the box」，并推荐 **Vite** 用于 React / Vue / Svelte / Solid 这类 SPA 框架；**Tauri 原生不支持 SSR**，需用 SSG / SPA / MPA。
  来源：<https://v2.tauri.app/start/frontend/>。
- **模板形态差异（实测）**：
  - React / Solid / Vue / Preact 官方模板 = **纯 Vite SPA**（`vite build` → `dist/`）。
  - **Svelte 官方模板 = SvelteKit + `@sveltejs/adapter-static`**（SPA 模式，`fallback: "index.html"`），输出到 `build/`。模板内注释原文：「Tauri doesn't have a Node.js server to do proper SSR so we use adapter-static with a fallback to index.html to put the site in SPA mode」。
    来源：`create-tauri-app` 生成的 `svelte.config.js` + <https://v2.tauri.app/start/frontend/sveltekit/>。

**判断**

- 五者的「Tauri 支持」都是官方级、成熟。**确定事实**。
- ⚠️推测：Svelte 走 SvelteKit 会引入路由/预渲染/构建管线，对「单窗口桌面 SPA、不需要 URL 路由亦或只做轻量路由」的 Ageminal 是**多余的复杂度**；虽然可用（SPA 模式下 `ssr=false`），但 React/Vue/Solid/Preact 的纯 Vite SPA 路径更直接。
- 社区模板（Awesome Tauri）四个框架都有，但社区模板的长期维护性不可控，**建议只用官方模板**。

---

## 四、与 xterm.js / CodeMirror 6 的集成成本（本项目最关键的一节）

### 4.1 两个库的本质：命令式、自持 DOM

**确定事实**

- **xterm.js**：`new Terminal()` → `term.open(parentElement)` → `term.write(data)` / `term.onData(cb)`。`write()` 是**异步**的，按块解析；`onWriteParsed` 事件「fires at most once per frame, after data parsing completes」。`open(parent)` 要求 `parent` 可见且有尺寸。
  来源：<https://xtermjs.org/docs/api/terminal/classes/terminal/>。
- **CodeMirror 6**：官方自述架构为「**Functional Core, Imperative Shell**」；`EditorView` 是命令式组件，自己管理 DOM，「The library does not expect user code to manipulate the DOM structure it manages」，销毁必须显式调用 `view.destroy()`。
  来源：<https://codemirror.net/docs/guide/>。

**推论（确定）**：这两个库**不属于**任何框架的渲染树。框架对它们的职责只有两件：①提供一个稳定的宿主 DOM 容器；②在挂载/卸载时创建/销毁实例。**框架的 diff / 信号系统不应该、也不会去渲染终端或编辑器内部**。

### 4.2 正确集成模式：实例池 + 宿主元素复用

**确定事实 / 来源**

- xterm.js 维护者（Tyriar）在 issue #4978 中：「Calling `open` multiple times was never really supported that well … but it definitely works as that's what VS Code does in order to move the terminal around the workbench.」该 issue（`open()` 二次调用不渲染）**目前仍 open**。
  来源：<https://github.com/xtermjs/xterm.js/issues/4978>。
- xterm.js 6.0.0 曾有 dispose 注册缺口导致 `Terminal` 实例泄漏（issue #5818，2026-04-17，**已关闭**）。
  来源：<https://github.com/xtermjs/xterm.js/issues/5818>。

**推荐做法（⚠️工程模式，非框架专属）**：

1. **每个 PTY 会话对应一个长期存活的 xterm 实例**，创建一次；输出订阅也只挂一次，**即使该终端当前不在可见页签**，缓冲仍持续填充（否则切换页签回来会空白——PTY 本身不留 scrollback）。
2. **切换页签 = 把已存在的宿主 `<div>` 移动进当前视图（DOM re-parent）**，而不是销毁重建；这样 xterm 的 DOM 子树整体搬迁，无需重绘。
3. **CodeMirror 同理**：`EditorView` 只在打开预览时创建，切换文件用事务（transaction）替换文档，而不是重建 view。

### 4.3 各框架的机械差异

| 框架 | 挂载一次的正确姿势 | 开发期陷阱 | 结论 |
|---|---|---|---|
| **React** | `useRef` + `useEffect`；注意 effect 依赖数组稳定；实例放 ref | **StrictMode 开发期**会「re-run Effects an extra time」，且组件函数双调用 → 天真的 `new Terminal()` 会建两个终端；需 ref 守卫或幂等清理 | 可用，但**集成代码最容易写错**，必须专门处理 |
| **Solid** | `ref` 回调 / `onMount`；信号只做数据 | 无 | 最直接：组件只运行一次，命令式安装天然匹配 |
| **Svelte 5** | `bind:this` + `onMount` / action `use:` | 无（SvelteKit 下注意 `ssr=false`） | 很直接 |
| **Vue 3** | `ref()` + `onMounted`；可用 `<Teleport>` 搬运宿主 | 无 | 很直接 |
| **Preact** | 同 React（hooks 模型一致），StrictMode 行为类似 | 同 React | 与 React 同档 |

**确定事实（React 侧来源）**：`<StrictMode>` 在开发期会「re-run Effects an extra time」并双调用组件函数；这些检查**仅开发期**，不影响生产构建。
来源：<https://react.dev/reference/react/StrictMode>。

> **对本项目的意义**：xterm/CodeMirror 的集成成本排序大致是 **Solid ≈ Svelte ≈ Vue < React ≈ Preact**，差距主要在**开发期**（React 的 StrictMode 双调用需要显式守卫），生产期各框架等价。**确定事实**下的差异，不是性能差异。

---

## 五、细粒度更新性能：信号式 vs 虚拟 DOM

### 5.1 各框架的模型（官方口径）

| 框架 | 更新模型 | 官方原文 / 来源 |
|---|---|---|
| **Solid** | **无虚拟 DOM**。模板编译为真实 DOM 节点，信号直接绑定 DOM | 「Instead of using a Virtual DOM, it compiles its templates to real DOM nodes and updates them with fine-grained reactions.」组件函数只运行一次，只有依赖变更的绑定重跑。来源：<https://github.com/solidjs/solid> · <https://docs.solidjs.com/advanced-concepts/fine-grained-reactivity> |
| **Svelte 5** | 编译期 + **runes（信号）**。Svelte 4 是编译期粗粒度（改一个属性整个对象失效），Svelte 5 明确补上信号式细粒度 | 「other frameworks have adopted fine-grained reactivity based on *signals*, leapfrogging Svelte's performance」→ Svelte 5 引入 `$state` 等 runes。来源：<https://svelte.dev/blog/svelte-5-is-alive> · <https://svelte.dev/docs/svelte/what-are-runes> |
| **Vue 3** | **虚拟 DOM**，但「Compiler-Informed」：静态提升、patch flags、tree flattening；`ref` 本身是信号式原语 | 「Vue's rendering system is based upon [Virtual DOM]」；并称正在探索非 VDOM 的 **Vapor Mode**。来源：<https://vuejs.org/guide/extras/rendering-mechanism.html> · <https://vuejs.org/guide/extras/reactivity-in-depth.html> |
| **React** | **虚拟 DOM**：state 变更 → 重新执行组件函数 → reconciliation → commit。React Compiler 自动 memo，但不改模型 | 「React calls your components to figure out what to display on screen」；「For re-renders, React will apply the minimal necessary operations」。来源：<https://react.dev/learn/render-and-commit> |
| **Preact** | 轻量虚拟 DOM（3kB），信号可选集成；把信号直接放进 JSX 文本位可**跳过 VDOM diff 就地更新** | 来源：<https://preactjs.com/> · <https://preactjs.com/guide/v10/signals/> |

### 5.2 信号 vs VDOM 的差距是真的，但要看落在哪

- **确定事实**：信号式系统只重跑依赖变更的计算，理论上 DOM 变更次数远少于 VDOM 的子树 diff；Solid 官方 README 直接把这点作为卖点，并链接到社区基准 JS Framework Benchmark。
  来源：<https://github.com/solidjs/solid> · <https://krausest.github.io/js-framework-benchmark/current.html>。
- **⚠️重要限定**：公开基准（如 JS Framework Benchmark 及各类二次文章）大多是**合成、DOM 密集**的负载；本次检索命中大量「Solid 快 15 倍 / 省 70% 内存」类文章，经查多为二手转载或 AI 生成、缺少原始数据，**不作为一手依据**。本报告只采纳官方文档口径与社区公认基准，并提示其合成性。
- **⚠️对本项目的推论**：Ageminal 的重负载（终端高频输出、文件预览高亮）**根本不经框架**（见 §四）；框架只渲染外壳。20 个 worktree、几十个页签、若干状态点的更新频率与规模，**远达不到**让 VDOM 成为瓶颈的量级。因此：
  - 若目标是「默认不用想性能」→ Solid / Svelte 5 / Vue 更省心；
  - React 也够用，但树/列表要写 `memo`、列表 key、必要时上 React Compiler，避免把整棵树拖进重渲染；
  - **不要**用「终端吞吐」为理由选框架——那取决于 xterm.js + 你的批处理/背压策略，与框架无关。

---

## 六、生态与长期维护

### 6.1 i18n（Ageminal 是 **i18n 一等公民**：§11 要求首发 zh-CN 全量 key 化）

npm 最新版与发布时间（2026-09-19）：

| 框架 | 主流 i18n 库 | 最新版 | 最近发布 | 维护状态 |
|---|---|---|---|---|
| React | `react-i18next` | 17.0.14 | 2026-09-13 | **非常活跃** |
| React | `react-intl`（FormatJS） | 12.1.1 | 2026-09-18 | **非常活跃** |
| Vue | `vue-i18n` | 11.4.12 | 2026-09-16 | **非常活跃（事实标准）** |
| Svelte | `svelte-i18n` | 4.0.1 | **2024-10-21** | ⚠️**约两年未更新** |
| Svelte | `@inlang/paraglide-js` | 2.25.4 | 2026-09-17 | 活跃（偏 SvelteKit/编译期方案） |
| Svelte | `typesafe-i18n` | 5.27.1 | 2026-02-11 | 活跃度中等 |
| Solid | `@solid-primitives/i18n` | 2.2.1 | 2025-04-27 | 社区维护，较薄 |
| Solid | `solid-i18next` | 0.0.5 | 2026-05-14 | 早期、使用面小 |
| Preact | `preact-i18n` | 1.5.0 | **2021-05-10** | ⚠️**基本停滞**（可改用 i18next） |

**判断（确定事实）**：**React 与 Vue 的 i18n 是一等公民且仍在高频维护**；Svelte 的传统方案 `svelte-i18n` 停滞，需要用 Paraglide（SvelteKit 取向）或 typesafe-i18n；Solid 的 i18n 依赖社区原语，成熟度最低；Preact 官方 i18n 停滞。
来源：<https://registry.npmjs.org/...>。

### 6.2 组件库 / 无头组件（设置页、菜单、对话框、树、右键菜单都要用）

| 框架 | 代表 | 状态 |
|---|---|---|
| React | Radix UI、MUI、shadcn/ui、React Aria | **最庞大** |
| Vue | Element Plus、Naive UI、`reka-ui`（shadcn-vue 底座） | **庞大** |
| Svelte | `bits-ui`（2.19.2，2026-09）、shadcn-svelte、Skeleton、Flowbite | 足够用 |
| Solid | `@kobalte/core`（0.13.14，2026-09）、solid-ui（shadcn 移植） | 可用但明显更小 |
| Preact | 复用 React 生态（`preact/compat`），但兼容层有边界 | 够用但需验证 |

来源：npm registry（版本/日期）+ 各库官方站点。

### 6.3 状态管理

- React：Zustand `5.0.15`、Jotai `3.0.0`（第三方）。
- Vue：**Pinia `4.0.3`（官方）**。
- Svelte：Svelte 5 runes + `svelte/store`（内置）。
- Solid：`createStore` / Context（**内置**，信号原生）。
- Preact：Signals（内置）。
**确定事实**：Solid / Svelte / Preact 的状态是内置一等能力；Vue 有官方 Pinia；React 依赖社区（生态最丰富）。

### 6.4 长期维护

- 五个框架的核心仓库都在 **2026-09 当月有推送**，无 `archived`，均 MIT。**确定事实**。
- 社区规模：React ≫ Svelte ≈ Vue > Preact ≈ Solid（star 数，见 §二）。这是「招人/找答案/找库」的现实差异。
- ⚠️推测：对「个人开发者、开源、Windows-only」的 Ageminal，维护风险主要不在框架本身，而在**周边库**（Svelte/Solid 的小生态里，某个库停更的概率更高）。

---

## 七、构建产物体积与冷启动（实测）

**方法（可复现）**：用官方 `create-tauri-app` 生成各框架 TypeScript 模板，只跑前端 `npm run build`，统计产物 JS/CSS 的 **raw 与 gzip**。即：

```bash
npx --yes create-tauri-app@latest fw-<tpl> --template <tpl> --manager npm -y --tauri-version 2
cd fw-<tpl> && npm install && npm run build
# <tpl> ∈ react-ts / solid-ts / svelte-ts / vue-ts / preact-ts
# 统计：find dist -name '*.js' -exec gzip -c {} + | wc -c   （Svelte 产物目录为 build/）
```

环境：Node 24.21、npm 11.19、Vite 8。日期 2026-09-19。生成于 `/tmp`，未改动仓库。

| 模板 | 前端方案 | JS raw | **JS gzip** | CSS gzip | 模板内框架版本 |
|---|---|---|---|---|---|
| `react-ts` | Vite SPA | 221,238 B | **68,502 B（≈66.9 KiB）** | 669 B | `react ^19.1.0` |
| `svelte-ts` | **SvelteKit + adapter-static（SPA）** | 73,449 B | **29,004 B（≈28.3 KiB）** | 692 B | `svelte ^5.56.3` |
| `vue-ts` | Vite SPA | 65,160 B | **25,423 B（≈24.8 KiB）** | 682 B | `vue ^3.5.13` |
| `solid-ts` | Vite SPA | 10,905 B | **4,390 B（≈4.3 KiB）** | 668 B | `solid-js ^1.9.3` |
| `preact-ts` | Vite SPA | 16,454 B | **6,956 B（≈6.8 KiB）** | 671 B | `preact ^10.25.1` |

**参照：xterm.js 6.0.0 官方发布文件**（jsDelivr，`lib/xterm.min.js`，未 tree-shake、含全部渲染器）：**488,936 B raw / 121,259 B gzip**。
CodeMirror 6 为多模块、按语言 tree-shake，需 bundler 才能给出可比数字；它与语言包/终端加起来会是真实应用 JS 的**主要部分**。

**结论**

- **确定事实**：框架运行时的绝对差可达一个数量级（Solid 4 KB vs React 67 KB gzip）。Svelte 官方模板因走 SvelteKit，产物明显高于「纯 Svelte + Vite」，但仍远小于 React。
- ⚠️关键限定：真实应用会引入 xterm.js（≈121 KB gzip 单文件）+ CodeMirror + 语言包 + Tauri API + 组件库，届时 **4 KB 与 67 KB 的差值是二阶的**，不会决定性地改变冷启动体验。
- ⚠️推测：**冷启动 < 2s** 的主导项是 WebView2 runtime 初始化、Rust 侧启动、守护进程/PTY 握手与首帧渲染。框架 JS 的解析/执行是其中一小块。**建议在 spike 中用真实骨架计时**，不要仅凭 bundle 大小做决定。

---

## 八、对比总表

| 维度 | React | Solid | Svelte 5 | Vue 3 | Preact |
|---|---|---|---|---|---|
| Tauri 2 官方模板 | ✅ 纯 Vite SPA | ✅ 纯 Vite SPA | ✅ 但走 **SvelteKit** | ✅ 纯 Vite SPA | ✅ 纯 Vite SPA |
| xterm/CM 集成难度 | 中（StrictMode 需守卫） | **低** | 低 | 低 | 中（同 React） |
| 响应式模型 | VDOM | **信号（无 VDOM）** | 信号（编译期） | VDOM + 编译优化 | 轻 VDOM（可选信号） |
| 高频外壳更新 | 够用（需 memo） | **默认最优** | 默认优 | 默认优 | 够用 |
| 实测 JS 产物(gzip) | 68.5 KB | **4.4 KB** | 29.0 KB（SvelteKit） | 25.4 KB | 7.0 KB |
| i18n 一等生态 | **✅（i18next/intl）** | ⚠️ 社区薄 | ⚠️ 传统方案停滞 | **✅（vue-i18n）** | ⚠️ 官方停滞 |
| 组件库 | **最庞大** | 小 | 够用 | **庞大** | 复用 React（有边界） |
| 状态管理 | 第三方（最丰富） | **内置** | **内置** | 官方 Pinia | **内置** |
| 生态规模/维护 | **最大** | 最小 | 大 | 大 | 中 |
| 学习成本（新范式） | 低（若已会） | 中（响应式心智不同，有坑） | 低 | 低 | 低 |
| 开发期命令式陷阱 | **有（StrictMode）** | 无 | 无 | 无 | 有 |

---

## 九、关键风险

1. **选错的标准不是性能，而是熟悉度。** 若团队只会 React，强行选 Solid 会把「2–4 周学习 + 小生态摩擦」变成真实的交付风险（⚠️推测）。
2. **Svelte = SvelteKit 的额外一层。** 官方 Tauri Svelte 模板并非纯 Vite SPA；虽支持 SPA 模式，但引入路由/构建复杂度。若要用 Svelte，建议**自建纯 Vite + Svelte**（Tauri 也支持「手动 setup」），或接受 SvelteKit SPA。
3. **Svelte/Solid 的 i18n 是明确短板**，与 Ageminal「i18n 一等公民」的要求相冲突；Svelte 需改用 Paraglide/typesafe-i18n，Solid 需自建或依赖薄原语。
4. **React 的 StrictMode + 命令式库**：开发期双调用会让天真写的 `new Terminal()` / `new EditorView()` 重复创建。**确定事实**，需在实现规范里固化「ref 守卫 + 幂等 dispose」。
5. **xterm.js 的 DOM 搬运/re-parent 有真实未收敛的 issue**（#4978 仍 open），终端池方案需在 spike 中实测（尤其 20×3 页签的切换与内存）。**不要**假设它开箱即用。
6. **任意框架都不能解决** xterm.js 的 IME 已知 bug（见 `docs/research/ui-editor-route.md`）与终端高吞吐背压；这些是跨框架的独立工程项。

---

## 十、推荐与理由

> 前提：这是**待用户确认的建议**，不是既成决定（issue #9 的落地门禁要求先与用户讨论）。

**默认推荐：Vue 3。**

- 官方 Tauri 模板是**纯 Vite SPA**，与 Ageminal 的桌面 SPA 形态最贴；实测产物 **25.4 KB gzip**，介于 Solid（4.4 KB）与 React（68.5 KB）之间，兼顾体积与生态。
- `ref` 本身是信号式原语，外壳细粒度更新自然；VDOM 有编译期优化，足够。
- **i18n 有官方且高频维护的 `vue-i18n`**，直接满足 §11。
- **组件库庞大**（Element Plus / Naive UI / reka-ui），设置页、树、菜单、对话框都有成熟件。
- 官方 Pinia；中文文档/社区大，与中文界面项目匹配。
- 无 StrictMode 式命令式陷阱。

**性能取向备选：Solid。**

- 架构与「命令式终端/编辑器 + 细粒度外壳」最契合：组件只运行一次，无 VDOM diff。
- 实测产物最小（4.4 KB gzip），内置 store，无 hooks 规则。
- 代价：生态最小、i18n 与组件库需自建或接受薄方案；⚠️推测：需要预留学习与造轮子成本。

**若团队已熟悉 React：选 React 也合理**，但要接受 ①运行时最大（实测 68.5 KB gzip）；②StrictMode × 命令式库的守卫；③靠 memo/Compiler 控重渲染。**除非熟悉度优势明显，否则本场景没有非 React 不可的理由。**

**Svelte 5**：语言与产物体积都很好，但官方模板走 SvelteKit、i18n 需另选方案；作为备选成立，不作为默认。

**Preact**：与 React API 兼容、体积小，但对本场景没有决定性收益，且 i18n 生态停滞；仅在「必须极小 + 想留 React 语法」时考虑。

---

## 十一、待用户拍板

> ✅ **已定 —— C1（#9）：Vue 3**（官方 Vite SPA 模板；选型 spike #47 全通过）。本节保留当时的候选对比。

1. **团队/个人最熟悉哪个框架？**（React / Vue / Svelte / Solid）这一项权重最高，本报告无法替代。
2. 是否愿意为 Solid 的架构契合度接受**更小的生态 + i18n 自建**？
3. 若选 Svelte：接受 **SvelteKit（SPA 模式）**，还是自建 **纯 Vite + Svelte**？
4. 是否要先做一个**最小 spike**（Tauri + xterm 终端池 + CodeMirror 只读 + 20×3 页签切换 + 冷启动计时）再定框架？⚠️本报告建议做——它同时能验证 §九 的风险 5、以及冷启动的实际构成。
5. 状态管理/i18n 是否需要统一约定（如 Vue 用 Pinia + vue-i18n）。

---

## 十二、来源链接

**Tauri**
- 创建项目与官方模板：<https://v2.tauri.app/start/create-project/> · <https://github.com/tauri-apps/create-tauri-app>
- 前端配置（frontend agnostic / 推荐 Vite / 不支持 SSR）：<https://v2.tauri.app/start/frontend/>
- SvelteKit + Tauri：<https://v2.tauri.app/start/frontend/sveltekit/>

**xterm.js / CodeMirror**
- Terminal API（`open` / `write` / `onData` / `onWriteParsed`）：<https://xtermjs.org/docs/api/terminal/classes/terminal/>
- issue #4978（`.open()` 二次调用不渲染，仍 open）：<https://github.com/xtermjs/xterm.js/issues/4978>
- issue #5818（6.0.0 dispose 泄漏，已关闭）：<https://github.com/xtermjs/xterm.js/issues/5818>
- CodeMirror System Guide（Functional Core, Imperative Shell / `destroy()`）：<https://codemirror.net/docs/guide/>
- CodeMirror 示例（含 Read-Only）：<https://codemirror.net/examples/>

**框架官方口径**
- Solid 无 VDOM / 细粒度：<https://github.com/solidjs/solid> · <https://docs.solidjs.com/advanced-concepts/fine-grained-reactivity>
- Svelte 5 runes：<https://svelte.dev/blog/svelte-5-is-alive> · <https://svelte.dev/docs/svelte/what-are-runes>
- Vue 渲染机制（Compiler-Informed VDOM / Vapor）：<https://vuejs.org/guide/extras/rendering-mechanism.html> · <https://vuejs.org/guide/extras/reactivity-in-depth.html>
- React Render and Commit：<https://react.dev/learn/render-and-commit>
- React StrictMode（开发期 Effect 双跑）：<https://react.dev/reference/react/StrictMode>
- Preact（3kB / Signals）：<https://preactjs.com/> · <https://preactjs.com/guide/v10/signals/>
- 社区基准（合成负载，谨慎解读）：<https://krausest.github.io/js-framework-benchmark/current.html>

**版本 / 维护**
- npm registry `dist-tags.latest` 与 `time`（2026-09-19 查询）：<https://registry.npmjs.org/>
- GitHub 仓库 stars/pushed/license（2026-09-19 查询，经 `gh api`）：react/react · vuejs/core · sveltejs/svelte · solidjs/solid · preactjs/preact · tauri-apps/tauri · xtermjs/xterm.js
- Vue Vapor 独立仓库已归档（`vuejs/vue-vapor`，pushed 2025-02-17；`vuejs/core` 当前 packages 无 `vapor`）：`gh api`

**产物实测**
- 官方 `create-tauri-app` 模板（react-ts / solid-ts / svelte-ts / vue-ts / preact-ts），本地 `npm run build` 后的 `dist/`（Svelte 为 `build/`）产物，2026-09-19。
- xterm.js 发布文件：<https://cdn.jsdelivr.net/npm/@xterm/xterm@6.0.0/lib/xterm.min.js>

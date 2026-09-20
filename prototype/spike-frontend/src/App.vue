<script setup lang="ts">
// ─────────────────────────────────────────────────────────────────────────────
// Ageminal C1 spike（#47）：Tauri 2 + Vue 3 最小骨架
//   探针 1：xterm 终端池 + DOM re-parent（20 worktree × 3 页签 = 60 实例）
//   探针 2：冷启动分解（Rust 侧 / 前端侧）
//   探针 3：CodeMirror 6 只读预览
// 注意：冷启动数字必须来自 **release build 的 exe**；`tauri dev` 不代表真实。
// ─────────────────────────────────────────────────────────────────────────────
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { TerminalPool } from "./terminalPool";
import CodeMirrorPane from "./components/CodeMirrorPane.vue";
import ImeProbe from "./components/ImeProbe.vue";
import NotifyProbe from "./components/NotifyProbe.vue";
import WindowProbe from "./components/WindowProbe.vue";
import VtParity from "./components/VtParity.vue";
import { probeResults } from "./probeStore";
import { SPIKE_BUILD } from "./build";
import { mark, marks, spikeErrors } from "./timing";

const WORKTREES = 20;
const TABS_PER_WT = 3;

const worktrees = Array.from({ length: WORKTREES }, (_, i) => `wt-${String(i + 1).padStart(2, "0")}`);
const tabNames = Array.from({ length: TABS_PER_WT }, (_, i) => `term-${i + 1}`);

const activeWt = ref(worktrees[0]);
const activeTab = ref(tabNames[0]);
const activeTermId = computed(() => `${activeWt.value}/${activeTab.value}`);

const pane = ref<HTMLDivElement | null>(null);
const offstage = ref<HTMLDivElement | null>(null);
let pool: TerminalPool | null = null;
let simTimer: number | undefined;

const showPreview = ref(false);

// H 组探针视图：#38 中文 IME / #48 通知点击（共用同一份 Tauri 骨架）
const view = ref<"bench" | "ime" | "notify" | "window" | "vtparity">("bench");

const SAMPLE_JSON = JSON.stringify(
  {
    spike: "C1 / #47",
    framework: "vue 3",
    probes: ["xterm-pool", "dom-reparent", "cold-start", "codemirror-readonly"],
    worktrees: WORKTREES,
    tabsPerWorktree: TABS_PER_WT,
    terminals: WORKTREES * TABS_PER_WT,
    note: "CodeMirror 只读预览：验证命令式编辑器在 Vue 下的挂载/销毁与事务换文档",
  },
  null,
  2
);

interface ColdStart {
  totalMs: number;
  processToNavMs: number;
  rustSetupMs: number;
  frontendMs: number;
}

const metrics = reactive({
  isTauri: false,
  poolSize: 0,
  heapMB: 0,
  totalBytes: 0,
  activeId: "",
  activeOpens: 0,
  activeReparents: 0,
  activeMounted: false,
  paneHostCount: -1,
  lastBurstMs: 0,
  switchSamples: 0,
  switchMaxFrameMs: 0,
  switchAvgFrameMs: 0,
  previewToggles: 0,
  cold: null as ColdStart | null,
  error: "",
  tModuleEval: 0,
  tMounted: 0,
  tPoolBuilt: 0,
  tFirstFrame: 0,
});

function nextFrame(): Promise<void> {
  return new Promise((resolve) => requestAnimationFrame(() => resolve()));
}

function activateActive(): void {
  if (!pool || !pane.value || showPreview.value) return;
  pool.activate(activeTermId.value, pane.value);
  updateMetrics();
}

watch(activeTermId, activateActive, { flush: "sync" });

// 从探针视图切回基准视图时，面板重新可见 → 需要重新 fit 一次
watch(view, async (v) => {
  if (v === "bench") {
    await nextTick();
    activateActive();
  }
});

function updateMetrics(): void {
  if (!pool) return;
  metrics.poolSize = pool.size;
  metrics.totalBytes = pool.totalBytes();
  const heap = pool.usedHeapBytes();
  metrics.heapMB = heap === null ? -1 : heap / (1024 * 1024);
  const active = pool.get(activeTermId.value);
  metrics.activeId = activeTermId.value;
  metrics.activeOpens = active?.opens ?? 0;
  metrics.activeReparents = active?.reparents ?? 0;
  metrics.activeMounted = pane.value ? pool.isMountedIn(activeTermId.value, pane.value) : false;
  metrics.paneHostCount = pane.value ? pool.paneHostCount(pane.value) : -1;
}

async function collectColdStart(): Promise<void> {
  try {
    const r = await invoke<{ app_to_setup_ms: number; t0_wall_ms: number; setup_wall_ms: number }>(
      "startup_report"
    );
    const readyWall = marks.timeOrigin + marks.firstFrame;
    metrics.cold = {
      totalMs: readyWall - r.t0_wall_ms,
      processToNavMs: marks.timeOrigin - r.t0_wall_ms,
      rustSetupMs: r.app_to_setup_ms,
      frontendMs: marks.firstFrame,
    };
  } catch (e) {
    metrics.error = `startup_report 失败：${String(e)}`;
  }
}

async function exportReport(): Promise<void> {
  if (!metrics.isTauri) return;
  try {
    await invoke<string>("save_report", {
      json: JSON.stringify(
        {
          generatedAt: new Date().toISOString(),
          build: SPIKE_BUILD,
          cold: metrics.cold,
          marks,
          poolSize: metrics.poolSize,
          heapMB: metrics.heapMB,
          totalBytes: metrics.totalBytes,
          lastBurstMs: metrics.lastBurstMs,
          reparent: {
            activeId: metrics.activeId,
            mountedInPane: metrics.activeMounted,
            paneHostCount: metrics.paneHostCount,
            opens: metrics.activeOpens,
            reparents: metrics.activeReparents,
          },
          codeMirror: { previewToggles: metrics.previewToggles },
          switch: {
            samples: metrics.switchSamples,
            avgFrameMs: metrics.switchAvgFrameMs,
            maxFrameMs: metrics.switchMaxFrameMs,
          },
          userAgent: navigator.userAgent,
          errors: spikeErrors,
          // H 组探针结果（#38 中文 IME / #48 通知点击）
          probes: probeResults,
        },
        null,
        2
      ),
    });
  } catch (e) {
    metrics.error = `save_report 失败：${String(e)}`;
  }
}

function burst(linesPerTerm = 300): void {
  if (!pool) return;
  const t0 = performance.now();
  for (const wt of worktrees) {
    for (const tb of tabNames) {
      const id = `${wt}/${tb}`;
      let s = "";
      for (let i = 0; i < linesPerTerm; i++) {
        s += `[${id}] burst ${String(i).padStart(4, "0")} …………………………………………\r\n`;
      }
      pool.write(id, s);
    }
  }
  metrics.lastBurstMs = performance.now() - t0;
  updateMetrics();
  void exportReport();
}

async function switchStress(switches = 200): Promise<void> {
  if (!pool) return;
  const ids: string[] = [];
  for (const wt of worktrees) for (const tb of tabNames) ids.push(`${wt}/${tb}`);

  let maxFrame = 0;
  let sum = 0;
  let last = performance.now();
  for (let i = 0; i < switches; i++) {
    const id = ids[i % ids.length];
    const [wt, tb] = id.split("/");
    // 触发 sync watcher -> activate（re-parent），不重建实例
    activeWt.value = wt;
    activeTab.value = tb;
    await nextTick();
    await nextFrame();
    const now = performance.now();
    const dt = now - last;
    last = now;
    maxFrame = Math.max(maxFrame, dt);
    sum += dt;
  }
  metrics.switchSamples = switches;
  metrics.switchMaxFrameMs = maxFrame;
  metrics.switchAvgFrameMs = sum / switches;
  updateMetrics();
  void exportReport();
}

async function togglePreview(): Promise<void> {
  showPreview.value = !showPreview.value;
  if (showPreview.value) metrics.previewToggles += 1;
  if (!showPreview.value) {
    await nextTick();
    activateActive();
  }
}

function resetMetrics(): void {
  metrics.lastBurstMs = 0;
  metrics.switchSamples = 0;
  metrics.switchMaxFrameMs = 0;
  metrics.switchAvgFrameMs = 0;
  updateMetrics();
}

function fmt(n: number | undefined, digits = 1): string {
  return n === undefined || Number.isNaN(n) ? "—" : n.toFixed(digits);
}

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MiB`;
}

onMounted(async () => {
  mark("appMounted");
  metrics.isTauri = "__TAURI_INTERNALS__" in window;

  // 1) 预建整个终端池（60 个实例，各持宿主 <div>，停在 offstage）
  pool = new TerminalPool(offstage.value as HTMLElement);
  for (const wt of worktrees) for (const tb of tabNames) pool.ensure(`${wt}/${tb}`);
  mark("poolBuilt");

  // 2) 激活首个终端（re-parent 进面板）
  activateActive();
  await nextFrame();
  await nextFrame();
  mark("firstFrame");

  // 3) 冷启动分解（仅 Tauri 环境）
  if (metrics.isTauri) await collectColdStart();

  metrics.tModuleEval = marks.moduleEval;
  metrics.tMounted = marks.mounted ?? 0;
  metrics.tPoolBuilt = marks.poolBuilt;
  metrics.tFirstFrame = marks.firstFrame;

  // 4) 后台模拟 60 个 agent 持续输出（内存观察）
  //    每行都带自己的终端 id —— 这样肉眼就能确认「屏幕上是哪个终端」
  let lineNo = 0;
  simTimer = window.setInterval(() => {
    if (!pool) return;
    lineNo++;
    for (const wt of worktrees) {
      for (const tb of tabNames) {
        const id = `${wt}/${tb}`;
        pool.write(
          id,
          `\x1b[90m[${id}]\x1b[0m \x1b[36msim ${String(lineNo).padStart(5, "0")}\x1b[0m heartbeat\r\n`
        );
      }
    }
    updateMetrics();
  }, 250);

  updateMetrics();

  // 5) 启动后自动跑完整基准并导出报告（无需手抄界面）
  window.setTimeout(async () => {
    burst(300);
    await switchStress(200);
    // 探针 4：CodeMirror 只读预览 —— 挂载、停两帧、销毁
    await togglePreview();
    await nextFrame();
    await nextFrame();
    await togglePreview();
    await nextFrame();
    await exportReport();
  }, 3000);
});

onBeforeUnmount(() => {
  if (simTimer !== undefined) window.clearInterval(simTimer);
});
</script>

<template>
  <div class="spike">
    <header class="topbar">
      <strong>Ageminal spike</strong>
      <span class="muted">#47 / #38 / #48 / #39 / #42</span>
      <span class="build-tag">{{ SPIKE_BUILD }}</span>
      <span class="spacer" />
      <button :class="{ on: view === 'bench' }" @click="view = 'bench'">终端池基准</button>
      <button :class="{ on: view === 'ime' }" @click="view = 'ime'">#38 IME</button>
      <button :class="{ on: view === 'notify' }" @click="view = 'notify'">#48 通知</button>
      <button :class="{ on: view === 'window' }" @click="view = 'window'">#39 毛玻璃</button>
      <button :class="{ on: view === 'vtparity' }" @click="view = 'vtparity'">#42 VT parity</button>
      <button @click="exportReport()">导出报告</button>
      <span class="muted">Tauri：{{ metrics.isTauri ? "是" : "否（浏览器模式，仅前端探针）" }}</span>
    </header>

    <div v-show="view === 'bench'">
    <section class="metrics">
      <div class="card">
        <h3>冷启动（release exe 才有意义）</h3>
        <template v-if="metrics.cold">
          <div class="big">{{ fmt(metrics.cold.totalMs, 0) }} ms</div>
          <table>
            <tr><td>进程入口 → 页面导航</td><td>{{ fmt(metrics.cold.processToNavMs) }} ms</td></tr>
            <tr><td>Rust setup</td><td>{{ fmt(metrics.cold.rustSetupMs) }} ms</td></tr>
            <tr><td>前端导航 → 首帧</td><td>{{ fmt(metrics.cold.frontendMs) }} ms</td></tr>
          </table>
        </template>
        <p v-else class="muted">只在 Tauri 环境可用（browser 模式无 Rust 侧时间戳）。</p>
      </div>

      <div class="card">
        <h3>终端池 / 内存</h3>
        <table>
          <tr><td>实例数</td><td>{{ metrics.poolSize }}</td></tr>
          <tr><td>JS 堆</td><td>{{ metrics.heapMB < 0 ? "n/a" : fmt(metrics.heapMB) + " MiB" }}</td></tr>
          <tr><td>累计写入</td><td>{{ fmtBytes(metrics.totalBytes) }}</td></tr>
          <tr><td>突发写入耗时</td><td>{{ fmt(metrics.lastBurstMs) }} ms</td></tr>
        </table>
      </div>

      <div class="card">
        <h3>re-parent 断言</h3>
        <table>
          <tr><td>当前终端</td><td>{{ metrics.activeId }}</td></tr>
          <tr><td>宿主已挂在面板</td><td>{{ metrics.activeMounted ? "✅" : "❌" }}</td></tr>
          <tr><td>面板内宿主数</td><td>{{ metrics.paneHostCount }}（应为 1）</td></tr>
          <tr><td>该实例 open() 次数</td><td>{{ metrics.activeOpens }}（应为 1）</td></tr>
          <tr><td>该实例 re-parent 次数</td><td>{{ metrics.activeReparents }}</td></tr>
        </table>
      </div>

      <div class="card">
        <h3>切换压测（200 次）</h3>
        <table>
          <tr><td>样本</td><td>{{ metrics.switchSamples }}</td></tr>
          <tr><td>平均帧间隔</td><td>{{ fmt(metrics.switchAvgFrameMs) }} ms</td></tr>
          <tr><td>最大帧间隔</td><td>{{ fmt(metrics.switchMaxFrameMs) }} ms</td></tr>
        </table>
      </div>
    </section>

    <section class="toolbar">
      <button @click="burst()">突发写入（60×300 行）</button>
      <button @click="switchStress()">切换压测 ×200</button>
      <button @click="togglePreview()">{{ showPreview ? "关闭预览" : "打开 CodeMirror 预览" }}</button>
      <button @click="resetMetrics()">清空指标</button>
      <span v-if="metrics.error" class="err">{{ metrics.error }}</span>
    </section>

    <section class="body">
      <aside class="side">
        <div class="side-title">Worktrees（{{ WORKTREES }}）</div>
        <button
          v-for="wt in worktrees"
          :key="wt"
          class="wt"
          :class="{ on: wt === activeWt }"
          @click="activeWt = wt"
        >
          {{ wt }}
        </button>
      </aside>

      <main class="main">
        <div class="tabs">
          <button
            v-for="tb in tabNames"
            :key="tb"
            class="tab"
            :class="{ on: tb === activeTab }"
            @click="activeTab = tb"
          >
            {{ tb }}
          </button>
        </div>

        <div class="pane-wrap">
          <!-- 终端面板：稳定容器，池往里 re-parent 宿主 <div>，Vue 不渲染其子节点 -->
          <div ref="pane" class="pane"></div>

          <!-- CodeMirror 只读预览：覆盖层，避免影响终端宿主布局 -->
          <div v-if="showPreview" class="preview">
            <CodeMirrorPane :doc="SAMPLE_JSON" />
          </div>
        </div>
      </main>
    </section>
    </div>

    <!-- H 组探针视图（#38 IME / #48 通知 / #39 毛玻璃）—— 外层可滚动壳避免内容被裁 -->
    <div v-if="view !== 'bench'" class="probe-shell">
      <ImeProbe v-if="view === 'ime'" />
      <NotifyProbe v-else-if="view === 'notify'" />
      <WindowProbe v-else-if="view === 'window'" />
      <VtParity v-else-if="view === 'vtparity'" />
    </div>

    <!-- 屏幕外但参与布局的暂存容器：xterm open() 需要可量尺寸 -->
    <div ref="offstage" class="offstage"></div>
  </div>
</template>

<style>
:root {
  color-scheme: dark;
}
/* #39：透明窗口 —— html 与 body 都必须透明，否则会露白底（只设 body 不够） */
html,
body,
#app {
  height: 100%;
  margin: 0;
  background: transparent;
}
body {
  color: #d4d4d4;
  font-family: "Segoe UI", system-ui, sans-serif;
  font-size: 13px;
}
.spike {
  /* #39 H10：全局底改为**全透明**。
     原先的 rgba(8,9,9,.55) 会把整窗压暗、材质只剩 45%，导致 #39 的透明度判定失效。
     透明度现在由各面板/控件自己承担；材质才能原样透出来。 */
  background: transparent;
}
.spike {
  display: flex;
  flex-direction: column;
  height: 100vh;
  box-sizing: border-box;
}
.topbar,
.toolbar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 10px;
  border-bottom: 1px solid #1e1e1e;
  /* 全局底透明后，窗口chrome 自己给一点底以保证可读性 */
  background: rgba(8, 9, 9, 0.6);
}
.toolbar {
  border-top: 1px solid #1e1e1e;
  border-bottom: 1px solid #1e1e1e;
}
.spacer {
  flex: 1;
}
.build-tag {
  font-family: Consolas, monospace;
  font-size: 11px;
  color: #e5c07b;
  border: 1px solid #6b4a00;
  background: #1d1600;
  border-radius: 4px;
  padding: 1px 6px;
}
.muted {
  color: #8a8a8a;
}
.err {
  color: #f48771;
}
.metrics {
  display: grid;
  grid-template-columns: repeat(4, 1fr);
  gap: 8px;
  padding: 8px 10px;
}
.card {
  border: 1px solid #1e1e1e;
  border-radius: 6px;
  padding: 8px 10px;
  background: #0d0d0d;
}
.card h3 {
  margin: 0 0 6px;
  font-size: 12px;
  color: #9cdcfe;
  font-weight: 600;
}
.card table {
  width: 100%;
  border-collapse: collapse;
}
.card td {
  padding: 1px 0;
  color: #b0b0b0;
}
.card td:last-child {
  text-align: right;
  color: #d4d4d4;
}
.big {
  font-size: 26px;
  font-weight: 700;
  color: #4ec9b0;
  margin-bottom: 4px;
}
button {
  background: #1b1b1b;
  color: #d4d4d4;
  border: 1px solid #2a2a2a;
  border-radius: 5px;
  padding: 4px 10px;
  cursor: pointer;
  font-size: 12px;
}
button:hover {
  border-color: #3a3a3a;
  background: #222;
}
.body {
  display: flex;
  flex: 1;
  min-height: 0;
}
.side {
  width: 140px;
  border-right: 1px solid #1e1e1e;
  overflow: auto;
  padding: 6px;
  background: rgba(8, 9, 9, 0.35);
}
.side-title {
  color: #6a9955;
  font-size: 11px;
  margin-bottom: 6px;
}
.wt {
  display: block;
  width: 100%;
  text-align: left;
  margin-bottom: 3px;
  background: transparent;
  border-color: transparent;
}
.wt.on {
  background: #143b52;
  border-color: #1f6f9e;
}
.main {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-width: 0;
}
.tabs {
  display: flex;
  gap: 4px;
  padding: 6px;
  border-bottom: 1px solid #1e1e1e;
}
.tab.on {
  background: #143b52;
  border-color: #1f6f9e;
}
.pane-wrap {
  position: relative;
  flex: 1;
  min-height: 0;
}
.pane {
  position: absolute;
  inset: 0;
  padding: 4px;
  box-sizing: border-box;
}
.preview {
  position: absolute;
  inset: 0;
  background: #0d0d0d;
  z-index: 5;
}
/* H 组探针外壳（#38/#39/#48）：给 flex:1 + min-height:0 + overflow:auto，
   否则内容超出 100vh 会被窗口直接裁掉且**滚不到**（H10 修复） */
.probe-shell {
  flex: 1 1 auto;
  min-height: 0;
  overflow: auto;
}
.offstage {
  position: fixed;
  left: -100000px;
  top: 0;
  width: 1000px;
  height: 600px;
  pointer-events: none;
}
.term-host {
  width: 100%;
  height: 100%;
}
</style>

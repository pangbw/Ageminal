<script setup lang="ts">
// ─────────────────────────────────────────────────────────────────────────────
// #39 探针：Tauri 2 在 Windows 的**窗口毛玻璃**与自绘标题栏（H10）
//
// 依据 docs/research/tauri-window-effects.md（§七 的 20 项真机清单）。
//
// ── H10 修掉的自身缺陷（H9 审查结论）────────────────────────────────────────
//   ⚠️ 测量有效性（另见 App.vue：`.spike` 已改透明）
//   1. 阶梯与材质判定卡原先挂在 `.card`（不透明 #0d0d0d）里 → 测的是「在实色上混色」，
//      **没测到材质**。现在它们是 `.probe` 的**直接子元素**，直接贴在透明窗口上。
//   2. 「材质检视模式」原先把判定卡的**不透明参照块也改透明**了 → 对比失效。已删除该覆盖。
//   3. `.spike` 的全局 55% 暗底已移除，alpha 交给各面板承担。
//
//   ⚠️ 硬 bug
//   5. `nudge()` 单位混用：`innerSize()` 是**物理像素**，原先却包成 `LogicalSize` →
//      在 125%/150% 缩放下复原会把窗口**永久放大**。现在统一用 `PhysicalSize`。
//   7. nudge 前先权威查询 `isMaximized()`，最大化则跳过。
//   8. 记录基准尺寸并取消未决的复原计时器，避免连点漂移。
//   9. 删除与判定卡冲突的「实色底」开关。
//
// ── 要判的两件事 ────────────────────────────────────────────────────────────
//   ① 切换效果是否需要 nudge（1px 重绘）：window-vibrancy 已知 issue #40
//      「the effect is not applied until the window is resized」。
//   ② Mica 到底有没有生效 —— 用「材质判定卡」：完全不透明 vs alpha 0.10，并排。
//
// ── 官方性能告警（window-vibrancy README，2026-09 核对）─────────────────────
//   apply_blur    Windows 7/10/11(**22H1 only**) · 22621+ 拖动/缩放性能差 → 本机 26200 ⛔
//   apply_acrylic Windows 10/11                · 1903+ / 22000 性能差    → ⚠️
//   apply_mica    Windows 11                   · 无告警                 → ✅ 首选
//
// ⚠️ 真实应用应在 **Rust setup** 里应用效果（避免启动闪白）；本探针为便于 A/B 切换走 JS API。
// ─────────────────────────────────────────────────────────────────────────────
import { computed, onMounted, reactive, ref } from "vue";
import { Effect, getCurrentWindow } from "@tauri-apps/api/window";
import { PhysicalSize } from "@tauri-apps/api/dpi";
import { arch, platform, type as osType, version } from "@tauri-apps/plugin-os";
import { setProbeResult } from "../probeStore";

const isTauri = "__TAURI_INTERNALS__" in window;
// ⚠️ 不能在模块作用域调用 getCurrentWindow()：非 Tauri 环境会直接抛错、整块组件挂掉。
let winRef: ReturnType<typeof getCurrentWindow> | null = null;
function win() {
  if (!winRef) winRef = getCurrentWindow();
  return winRef;
}

const os = reactive({ version: "—", platform: "—", arch: "—", type: "—" });
const applied = ref<string>("（未应用）");
const effectErr = ref<string>("");
const maximized = ref(false);
const inspect = ref(false); // 材质检视模式：只把「面板」变透明，**不动判定卡的参照块**
const autoNudge = ref(true);
const events = ref<string[]>([]);

function buildOf(v: string): number | null {
  const m = /^\d+\.\d+\.(\d+)/.exec(v);
  return m ? Number(m[1]) : null;
}
const build = computed(() => buildOf(os.version));

function adviceFor(b: number | null): string {
  if (b === null) return "无法解析 build";
  if (b >= 22621) return "Win11 22H2+ → 首选 tabbedDark（Mica Alt）；Blur 在你这个版本官方已标注性能差";
  if (b >= 22000) return "Win11 21H2 → mica（配暗色主题）；Acrylic 在此版本官方标注性能差";
  if (b >= 17763) return "Win10 1809+ → acrylic（别请求 Mica）";
  return "低于 Win10 1809 → 不透明实色";
}

const EFFECTS: Array<{ key: string; label: string; note: string; effect: Effect }> = [
  { key: "tabbedDark", label: "Mica Alt 暗（tabbedDark）", note: "Win11 22H2+ 首选 · 官方无性能告警", effect: Effect.TabbedDark },
  { key: "mica", label: "Mica（跟随系统暗色）", note: "Win11 21H2 · 官方无性能告警", effect: Effect.Mica },
  { key: "acrylic", label: "Acrylic", note: "官方告警：拖动/缩放性能差（1903+ / 22000）", effect: Effect.Acrylic },
  { key: "blur", label: "Blur", note: "官方告警：22621+ 性能差 —— 本机不该用", effect: Effect.Blur },
];

// 前几轮已确认的结论先预填，使报告自带事实
const checks = reactive<Record<string, boolean>>({
  a1: true, a2: true, a3: true, a4: true, a5: true,
  c2: true, // A/B 对照已确认无差异（backdrop-filter 采不到桌面像素）
});
const notes = ref("");

const CHECKLIST: Array<{ id: string; text: string }> = [
  { id: "a1", text: "A1 拖动标题栏可移动窗口" },
  { id: "a2", text: "A2 双击标题栏可最大化 / 还原" },
  { id: "a3", text: "A3 拖到屏幕边缘能 Aero Snap；悬停最大化按钮没有 Snap Layouts（预期）" },
  { id: "a4", text: "A4 四边 / 四角缩放热区正常" },
  { id: "a5", text: "A5 圆角与阴影可接受" },
  { id: "m1", text: "M1 材质判定卡：左右两块明显不同 = 材质生效；完全一样 = 没生效" },
  { id: "m2", text: "M2 关掉「自动 nudge」后切换效果，能复现「切不动」" },
  { id: "m3", text: "M3 打开「自动 nudge」后切换效果，每次都能切" },
  { id: "b2", text: "B2 窗口失活时材质回退实色" },
  { id: "d1", text: "D1 无边框连续缩放 30–60s 无渐进式退化" },
  { id: "d2", text: "D2 拖动/缩放背景闪烁：tabbedDark 下是否出现（区分 Mica 还是 Acrylic 的问题）" },
];

function logEvent(m: string): void {
  const t = Math.round(performance.now());
  events.value.push(`[t=${t}ms] ${m}`);
  if (events.value.length > 120) events.value.splice(0, events.value.length - 120);
  sync();
}

let baseSize: PhysicalSize | null = null;
let restoreTimer: number | undefined;

/**
 * 1px 重绘。window-vibrancy 已知问题：效果要 resize 一下才生效（issue #40）。
 * ⚠️ 全程用**物理像素**（`innerSize()` 的返回值就是物理像素）。
 */
async function nudge(): Promise<void> {
  const isMax = await win()
    .isMaximized()
    .catch(() => false);
  if (isMax) {
    logEvent("nudge 跳过：窗口已最大化");
    return;
  }
  const s = await win().innerSize(); // PhysicalSize
  baseSize = s;
  if (restoreTimer !== undefined) {
    window.clearTimeout(restoreTimer);
    restoreTimer = undefined;
  }
  await win().setSize(new PhysicalSize(s.width + 1, s.height));
  restoreTimer = window.setTimeout(() => {
    restoreTimer = undefined;
    const b = baseSize;
    if (b) win().setSize(new PhysicalSize(b.width, b.height)).catch(() => {});
  }, 80);
}

async function applyEffect(key: string, opts: { nudge?: boolean } = {}): Promise<void> {
  effectErr.value = "";
  const item = EFFECTS.find((e) => e.key === key);
  if (!item) return;
  try {
    await win().setEffects({ effects: [item.effect] });
    applied.value = key;
    logEvent(`setEffects([${key}]) → ok`);
  } catch (e) {
    effectErr.value = `${key} 失败：${String(e)}`;
    applied.value = `${key}（失败）`;
    logEvent(`setEffects([${key}]) → 失败：${String(e)}`);
    return;
  }
  if (opts.nudge ?? autoNudge.value) {
    try {
      await nudge();
      logEvent("nudge(+1px → 复原) 完成");
    } catch (e) {
      logEvent(`nudge 失败：${String(e)}`);
    }
  }
}

/** 只重绘、不改效果：验证 nudge 能否救活「已应用但看不见」的效果 */
async function reapply(): Promise<void> {
  const item = EFFECTS.find((e) => e.key === applied.value);
  if (!item) {
    logEvent(`重新应用跳过：当前状态是「${applied.value}」，没有可重放的效果`);
    return;
  }
  logEvent(`重新应用 ${item.key}`);
  await applyEffect(item.key);
}

async function refreshMax(): Promise<void> {
  try {
    maximized.value = await win().isMaximized();
  } catch {
    /* ignore */
  }
}

async function autoApply(): Promise<void> {
  const b = build.value;
  if (b === null) return;
  // ⚠️ 启动时**不** nudge：窗口刚由 WM 放置，此时改尺寸会造成可见跳动。
  const opts = { nudge: false };
  if (b >= 22621) return applyEffect("tabbedDark", opts);
  if (b >= 22000) return applyEffect("mica", opts);
  if (b >= 17763) return applyEffect("acrylic", opts);
  logEvent(`build ${b} 低于 1809 → 不应用任何 OS 效果（用实色兜底）`);
}

function sync(): void {
  setProbeResult("window", {
    isTauri,
    os: { ...os, build: build.value },
    adviceFromBuild: adviceFor(build.value),
    appliedEffect: applied.value,
    effectError: effectErr.value,
    windowConfig: { decorations: false, transparent: true, shadow: true },
    inspectMode: inspect.value,
    autoNudge: autoNudge.value,
    setEffectsEvents: events.value,
    chosenPanelAlpha: "0.25（H8 用户实测选定，但见 notes：H9 前的阶梯测法有缺陷，H10 已改为直接贴窗口）",
    checks: { ...checks },
    notes: notes.value,
    knownFindings: [
      "H6：拖动标题栏 / 热区缩放时背景闪烁",
      "H7/H8：切到 Blur 或清除后，再切换效果切不动（疑为 issue #40 的重绘问题）",
      "H8：Acrylic 最为明显；Acrylic + 检视模式下 A/B 对照无差异（backdrop-filter 采不到桌面）",
      "H9 审查：阶梯/判定卡原先挂在不透明 .card 内 → alpha 结论需在 H10 重测",
    ],
    note: "官方性能告警：blur 22621+ 差、acrylic 1903+/22000 差、mica 无告警。window-vibrancy Tauri 配方要求 transparent:true。已知 issue：效果需 resize 才生效。",
  });
}

onMounted(async () => {
  if (!isTauri) {
    os.version = "（非 Tauri 环境）";
    sync();
    return;
  }
  try {
    os.version = String(await version());
    os.platform = String(await platform());
    os.arch = String(await arch());
    os.type = String(await osType());
  } catch (e) {
    os.version = `读取失败：${String(e)}`;
  }
  await refreshMax();
  await autoApply();
  sync();
});
</script>

<template>
  <section class="probe" :class="{ inspect }">
    <!-- 自绘标题栏 -->
    <div class="tbar" data-tauri-drag-region @dblclick="win().toggleMaximize().then(refreshMax)">
      <span class="dot" />
      <strong>Ageminal spike</strong>
      <span class="muted">自绘标题栏 · 拖动 / 双击最大化</span>
      <span class="spacer" />
      <label class="tg"><input type="checkbox" v-model="inspect" @change="sync" /> 材质检视</label>
      <label class="tg"><input type="checkbox" v-model="autoNudge" @change="sync" /> 自动 nudge</label>
      <button class="wbtn" title="最小化" @click="win().minimize()">—</button>
      <button class="wbtn" title="最大化/还原" @click="win().toggleMaximize().then(refreshMax)">
        {{ maximized ? "❐" : "□" }}
      </button>
      <button class="wbtn danger" title="关闭" @click="win().close()">✕</button>
    </div>

    <!-- ①② 材质判定卡：直接贴窗口，中间不隔任何面板 -->
    <p class="lead">材质判定 · 直接贴在窗口上（不隔任何面板）</p>
    <div class="judge">
      <div class="jd opaque">
        <div class="jd-t">参照 · 完全不透明</div>
        <div class="jd-b">#080909</div>
        <div class="jd-n">「没有材质」就长这样</div>
      </div>
      <div class="jd clear">
        <div class="jd-t">测试 · alpha 0.10</div>
        <div class="jd-b">几乎全透明</div>
        <div class="jd-n">材质生效时，左右会明显不同</div>
      </div>
    </div>
    <p class="hint">
      把窗口移到壁纸纹理明显处再看：左右明显不同 = 材质真在生效；完全一样 = 没生效（毛玻璃就只是半透明色）。
      右侧那块可以按窗口边缘缩放热区拖动，把它压在图标/图片上一路比对。
    </p>

    <!-- ③ 面板透明度阶梯：同样直接贴窗口，才能在材质上判断 -->
    <p class="lead">面板透明度阶梯 · alpha 越小材质越透</p>
    <div class="ladder">
      <div v-for="a in [0, 0.25, 0.55, 0.85]" :key="a" class="lad" :style="{ background: `rgba(8,9,9,${a})` }">
        <div class="lad-t">alpha {{ a }}</div>
        <div class="lad-b">中英文混排 Ageminal 123</div>
      </div>
    </div>

    <div class="cols">
      <div class="card">
        <h3>系统 / 切换</h3>
        <table>
          <tr><td>version</td><td class="mono">{{ os.version }}</td></tr>
          <tr><td>build</td><td class="mono">{{ build ?? "—" }}</td></tr>
          <tr><td>当前效果</td><td class="mono">{{ applied }}</td></tr>
          <tr v-if="effectErr"><td>错误</td><td class="err mono">{{ effectErr }}</td></tr>
        </table>
        <p class="hint">{{ adviceFor(build) }}</p>
        <div class="btns">
          <button v-for="e in EFFECTS" :key="e.key" :class="{ on: applied === e.key }" @click="applyEffect(e.key)">
            <span>{{ e.label }}</span>
            <small>{{ e.note }}</small>
          </button>
          <button @click="reapply">重新应用当前效果（再 nudge 一次）</button>
        </div>
        <h3 class="gap">setEffects 调用日志</h3>
        <pre class="evlog">{{ events.join("\n") || "（尚无调用）" }}</pre>
      </div>

      <div class="card">
        <h3>① 切换是否需要 nudge（关键实验）</h3>
        <ol class="steps">
          <li>关掉「自动 nudge」，依次点 tabbedDark → acrylic → 再回 tabbedDark，记下哪一步视觉上没变（M2）</li>
          <li>打开「自动 nudge」，同样顺序再点一遍，看是否每次都真的变（M3）</li>
          <li>对已应用但看不见的效果，可点「重新应用当前效果」补一次重绘</li>
        </ol>
        <p class="hint">
          window-vibrancy 已知问题：效果有时要 resize 一下才生效。若 nudge 能解决，设置页就能放「材质」选项；
          不能的话材质只能启动时定死（本就符合 C7 #15 的「单一 build 检测效果」决定）。
        </p>

        <h3 class="gap">② 材质判定怎么读</h3>
        <p class="hint">
          判定卡与阶梯都已移到面板之外。若你打开「材质检视」，只会把下方的面板（.card）变透明，
          判定卡的参照块不受影响——这样对比才成立。
        </p>
        <p class="hint">
          提醒：Blur 在你这个 Windows 版本（22621+）上官方标注拖动/缩放性能差，建议不要点；万一卡住，
          点「重新应用当前效果」或切换锁定的效果把它顶掉。
        </p>
      </div>
    </div>

    <div class="cols">
      <div class="card">
        <h3>真机清单</h3>
        <label v-for="c in CHECKLIST" :key="c.id" class="chk">
          <input type="checkbox" v-model="checks[c.id]" @change="sync" />
          <span>{{ c.text }}</span>
        </label>
      </div>
      <div class="card">
        <h3>备注（写进报告）</h3>
        <textarea
          v-model="notes"
          rows="10"
          class="notes"
          placeholder="例如：tabbedDark 下拖动不闪 / acrylic 下闪；判定卡左右有无差异；哪一档 alpha 在材质上最好用…"
          @input="sync"
        />
      </div>
    </div>
  </section>
</template>

<style scoped>
.probe { padding: 8px 10px; }
/* 材质检视：只让「面板」变透明；⚠️ 不碰判定卡与阶梯，否则对比失效 */
.probe.inspect .card { background: transparent; border-color: rgba(33, 33, 33, 0.55); }

.tbar {
  display: flex; align-items: center; gap: 8px;
  height: 34px; padding: 0 6px 0 10px; margin: -8px -10px 8px;
  background: rgba(8, 9, 9, 0.35); border-bottom: 1px solid #212121; user-select: none;
}
.dot { width: 10px; height: 10px; border-radius: 50%; background: #61afef; }
.spacer { flex: 1; }
.tg { display: flex; align-items: center; gap: 4px; font-size: 11px; color: #b8bfcc; }
.wbtn { width: 34px; height: 26px; padding: 0; background: transparent; border: 1px solid transparent; color: #d4d4d4; }
.wbtn:hover { background: #2c313a; border-color: #2c313a; }
.wbtn.danger:hover { background: #c42b1c; border-color: #c42b1c; }

.lead { margin: 10px 0 6px; font-size: 12px; color: #9cdcfe; }
.judge { display: flex; gap: 8px; }
.jd { flex: 1; min-width: 0; height: clamp(110px, 18vh, 220px); border: 1px solid #212121; border-radius: 6px; padding: 12px; }
.jd.opaque { background: #080909; }
.jd.clear { background: rgba(8, 9, 9, 0.1); }
.jd-t { font-size: 12px; color: #9cdcfe; }
.jd-b { font-family: Consolas, monospace; font-size: 12px; margin-top: 4px; }
.jd-n { font-size: 11px; color: #949cab; margin-top: 4px; }

.ladder { display: flex; gap: 8px; }
.lad { flex: 1; min-width: 0; border: 1px solid #212121; border-radius: 6px; padding: 10px; height: clamp(56px, 8vh, 96px); }
.lad-t { font-family: Consolas, monospace; font-size: 11px; color: #9cdcfe; }
.lad-b { font-size: 12px; }

.cols { display: flex; gap: 10px; margin: 10px 0; }
.card { flex: 1; min-width: 0; }
.card h3.gap { margin-top: 10px; }
.hint { color: #949cab; font-size: 11px; margin: 6px 0 0; line-height: 1.7; }
.mono { font-family: Consolas, monospace; font-size: 11px; }
.err { color: #ff616e; }
.btns { display: flex; flex-direction: column; gap: 4px; margin-top: 8px; }
.btns button { text-align: left; display: flex; flex-direction: column; gap: 1px; }
.btns button small { color: #949cab; font-size: 10.5px; }
.btns button.on { background: #143b52; border-color: #1f6f9e; }
.evlog { max-height: 13vh; overflow: auto; margin: 4px 0 0; font-size: 10.5px; line-height: 1.5; white-space: pre-wrap; color: #b0b0b0; }
.steps { margin: 4px 0 0; padding-left: 18px; font-size: 12px; line-height: 1.9; color: #b0b0b0; }
.chk { display: flex; gap: 6px; align-items: flex-start; font-size: 12px; line-height: 1.7; color: #b0b0b0; }
.notes { width: 100%; box-sizing: border-box; background: #0d0d0d; color: #d4d4d4; border: 1px solid #212121; border-radius: 4px; padding: 6px; font-size: 12px; }
</style>

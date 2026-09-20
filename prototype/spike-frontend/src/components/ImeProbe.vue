<script setup lang="ts">
// ─────────────────────────────────────────────────────────────────────────────
// #38 探针：xterm.js 在 WebView2 下的**中文 IME**
//
// 要回答三个问题：
//   ① 中文提交是否经 `onData` 正确送达（码点/长度），组合过程中是否被吞
//   ② 候选窗锚点（xterm 的 helper textarea）是否**贴着光标** —— xterm.js #5454
//   ③ 组合输入是否**破坏等宽网格**（CJK 双宽是否与 2×ASCII 对齐）
//
// ②的判定不依赖 DOM 光标元素（xterm 6 的 DOM 渲染器在未聚焦时并不渲染它）：
//   用 `buffer.active.cursorX/Y` × **实测单元格尺寸**算出**期望光标矩形**，与 helper rect 对比。
//
// 真 PTY（ConPTY 光标跳动，codex #35438）不在本探针内 —— 归 #40 的 PoC。
// ─────────────────────────────────────────────────────────────────────────────
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { Terminal } from "@xterm/xterm";
import { setProbeResult } from "../probeStore";

const host = ref<HTMLDivElement | null>(null);
const logs = ref<string[]>([]);
const committed = ref<string[]>([]);
const helperRect = ref<string>("—");
const cursorElRect = ref<string>("—");
const cursorElInfo = ref<string>("—");
const expectedCursor = ref<string>("—");
const grid = ref<{ ascii: number; cjk: number; equal: boolean } | null>(null);
const cell = ref<{ w: number; h: number } | null>(null);

let term: Terminal | null = null;
let textarea: HTMLTextAreaElement | null = null;
let t0 = 0;
let ro: ResizeObserver | null = null;

const CAL_LEN = 40; // 校准行长度（全 ASCII），用于量单元格宽

const cjkCount = computed(
  () =>
    committed.value
      .join("")
      .split("")
      .filter((c) => c >= "\u4e00" && c <= "\u9fff").length
);

function log(msg: string): void {
  const t = Math.round(performance.now() - t0);
  logs.value.push(`[t=${t}ms] ${msg}`);
  if (logs.value.length > 400) logs.value.splice(0, logs.value.length - 400);
  sync();
}

function r(el: Element | null): string {
  if (!el) return "null";
  const b = el.getBoundingClientRect();
  return `x=${Math.round(b.x)} y=${Math.round(b.y)} w=${Math.round(b.width)} h=${Math.round(b.height)}`;
}

/** xterm 6 的 DOM 渲染器可能不渲染光标元素；命中与否都记录 */
function findCursorEl(): Element | null {
  const h = host.value;
  if (!h) return null;
  const sels = [".xterm-cursor", ".xterm-rows .xterm-cursor", ".xterm-cursor-layer", ".xterm-cursor-block"];
  for (const s of sels) {
    const el = h.querySelector(s);
    if (el) {
      cursorElInfo.value = `${s} → class="${el.className}"`;
      return el;
    }
  }
  cursorElInfo.value = "（未命中；DOM 渲染器未聚焦时不渲染光标，属正常）";
  return null;
}

function rowsEl(): Element | null {
  return host.value?.querySelector(".xterm-rows") ?? null;
}

/** 量一行**文本**的渲染宽度（行容器满宽，必须用 Range） */
function measureLine(index: number): number | null {
  const rows = host.value?.querySelectorAll(".xterm-rows > div");
  const row = rows && rows[index];
  if (!row) return null;
  const range = document.createRange();
  range.selectNodeContents(row);
  const b = range.getBoundingClientRect();
  return Math.round(b.width * 100) / 100;
}

/** 由 buffer 光标位置 + 实测单元格尺寸，算出**期望的光标矩形** */
function computeExpectedCursor(): string {
  const rows = rowsEl();
  if (!rows || !term || !cell.value) return "—";
  const b = rows.getBoundingClientRect();
  const cx = term.buffer.active.cursorX;
  const cy = term.buffer.active.cursorY;
  const x = b.x + cx * cell.value.w;
  const y = b.y + cy * cell.value.h;
  return `x=${Math.round(x)} y=${Math.round(y)} w=${Math.round(cell.value.w)} h=${Math.round(cell.value.h)} (cursorX=${cx} cursorY=${cy})`;
}

function snapshot(why: string): void {
  helperRect.value = r(textarea);
  cursorElRect.value = r(findCursorEl());
  expectedCursor.value = computeExpectedCursor();
  log(`坐标快照(${why}) helper[${helperRect.value}] 期望光标[${expectedCursor.value}] 光标元素[${cursorElRect.value}]`);
}

/** ③ 等宽网格：8×ASCII 与 4×CJK 都占 8 格，宽度应相等；顺带校准单元格尺寸 */
function writeSample(): void {
  if (!term) return;
  term.reset();
  term.write("AAAAAAAA\r\n");
  term.write("中中中中\r\n");
  term.write("中文测试 ABC 中 z\r\n");
  term.write("M".repeat(CAL_LEN) + "\r\n");
  // 让光标停在一个**非 0 列**上（否则「锚点是否跟随光标移动」验不出来）
  term.write("abc ");
  window.setTimeout(() => {
    const ascii = measureLine(0);
    const cjk = measureLine(1);
    const cal = measureLine(3);
    const rows = host.value?.querySelectorAll(".xterm-rows > div");
    const h = rows && rows[0] ? rows[0].getBoundingClientRect().height : 0;
    if (cal !== null && h > 0) cell.value = { w: cal / CAL_LEN, h };
    const ok = ascii !== null && cjk !== null && Math.abs(ascii - cjk) < 1;
    grid.value = { ascii: ascii ?? -1, cjk: cjk ?? -1, equal: ok };
    log(
      `网格检查：8×ASCII=${ascii}px · 4×CJK=${cjk}px → ${ok ? "✅ 等宽对齐" : "❌ 未对齐"}` +
        (cell.value ? `；单元格 ≈ ${cell.value.w.toFixed(2)}×${cell.value.h.toFixed(2)}px` : "")
    );
    term?.focus();
    snapshot("writeSample 后");
  }, 150);
}

function bindComposition(): void {
  const ta = host.value?.querySelector(".xterm-helper-textarea") as HTMLTextAreaElement | null;
  textarea = ta;
  if (!ta) {
    log("❌ 未找到 .xterm-helper-textarea（IME 锚点）");
    return;
  }
  log("已绑定 composition 事件到 .xterm-helper-textarea");
  ta.addEventListener("compositionstart", () => {
    log("compositionstart");
    snapshot("compositionstart");
  });
  ta.addEventListener("compositionupdate", (e) => {
    log(`compositionupdate data=${JSON.stringify((e as CompositionEvent).data)}`);
    snapshot("compositionupdate");
  });
  ta.addEventListener("compositionend", (e) => {
    log(`compositionend data=${JSON.stringify((e as CompositionEvent).data)}`);
    snapshot("compositionend");
  });
  ta.addEventListener("input", (e) => {
    log(`input value=${JSON.stringify((e.target as HTMLTextAreaElement).value)}`);
  });
  ta.addEventListener("blur", () => log("helper blur"));
  ta.addEventListener("focus", () => log("helper focus"));
}

function sync(): void {
  setProbeResult("ime", {
    cols: term?.cols ?? null,
    rows: term?.rows ?? null,
    helperFound: !!textarea,
    helperRect: helperRect.value,
    expectedCursorRect: expectedCursor.value,
    cursorElRect: cursorElRect.value,
    cursorElInfo: cursorElInfo.value,
    cellSize: cell.value,
    grid: grid.value,
    cjkCharsCommitted: cjkCount.value,
    committedSample: committed.value.slice(-25),
    events: logs.value.slice(-80),
    note: "候选窗锚点 = helper textarea；与『期望光标』rect 长期不重合即复现 xterm.js #5454",
  });
}

onMounted(() => {
  t0 = performance.now();
  if (!host.value) return;

  term = new Terminal({
    convertEol: true,
    scrollback: 500,
    fontFamily: '"Cascadia Mono", Consolas, "Microsoft YaHei UI", monospace',
    fontSize: 14,
    theme: { background: "#080909", foreground: "#d4d4d4", cursor: "#4fc1ff" },
  });
  term.open(host.value);

  term.onData((data) => {
    const cps = Array.from(data)
      .map((c) => c.codePointAt(0)!.toString(16).padStart(4, "0"))
      .join(" ");
    committed.value.push(data);
    if (committed.value.length > 200) committed.value.splice(0, 100);
    log(`onData len=${data.length} 码点=[${cps}] 文本=${JSON.stringify(data)}`);
    // ⚠️ 本探针**没有真实 PTY**：真实实现里数据送给 PTY、由 shell 回显。
    // 这里手动回显，才能①肉眼确认中文上屏 ②让光标右移，从而检验「候选窗锚点是否跟随光标」。
    if (term) term.write(data.replace(/\r/g, "\r\n"));
  });
  term.onKey(({ domEvent }) => {
    log(
      `onKey key=${domEvent.key} ctrl=${domEvent.ctrlKey} alt=${domEvent.altKey} isComposing=${
        (domEvent as KeyboardEvent).isComposing
      }`
    );
  });

  bindComposition();
  writeSample();

  ro = new ResizeObserver(() => {
    log(`容器尺寸变化 ${r(host.value)}`);
  });
  ro.observe(host.value);

  log("跑法：① **先打几个 ASCII**（如 abc）→ ② **再用中文输入法打字**（如「中文测试」）→ ③ 文本应**上屏**、光标随之右移，而右侧「helper 锚点」应始终跟住「期望光标」；若两者 x/y 分家，即复现 xterm.js #5454。");
  sync();
});

onBeforeUnmount(() => {
  ro?.disconnect();
  term?.dispose();
  term = null;
});
</script>

<template>
  <section class="probe">
    <header class="probe-head">
      <strong>#38 · 中文 IME 探针</strong>
      <span class="muted">xterm.js + WebView2 · 候选窗锚点 / onData / 等宽网格</span>
      <span class="spacer" />
      <button @click="writeSample">重写样例并量网格</button>
      <button @click="snapshot('手动')">坐标快照</button>
      <button @click="logs = []">清空日志</button>
    </header>

    <div class="probe-cols">
      <div class="probe-term">
        <div ref="host" class="term-host"></div>
      </div>

      <div class="probe-side">
        <div class="card">
          <h3>判定用读数</h3>
          <table>
            <tr><td>helper 锚点</td><td class="mono">{{ helperRect }}</td></tr>
            <tr><td><b>期望光标</b></td><td class="mono">{{ expectedCursor }}</td></tr>
            <tr><td>光标元素</td><td class="mono">{{ cursorElRect }}</td></tr>
            <tr><td>命中选择器</td><td class="mono">{{ cursorElInfo }}</td></tr>
            <tr>
              <td>网格对齐</td>
              <td>
                <template v-if="grid">
                  {{ grid.equal ? "✅" : "❌" }} (ASCII {{ grid.ascii }}px / CJK {{ grid.cjk }}px)
                </template>
                <template v-else>—</template>
              </td>
            </tr>
            <tr><td>单元格尺寸</td><td>{{ cell ? cell.w.toFixed(2) + " × " + cell.h.toFixed(2) + " px" : "—" }}</td></tr>
            <tr><td>已提交中文字数</td><td>{{ cjkCount }}</td></tr>
          </table>
        </div>
        <div class="card">
          <h3>事件日志（最近在下）</h3>
          <pre class="logbox">{{ logs.join("\n") }}</pre>
        </div>
      </div>
    </div>
  </section>
</template>

<style scoped>
.probe { padding: 8px 10px; }
.probe-head { display: flex; align-items: center; gap: 8px; margin-bottom: 8px; }
.probe-cols { display: flex; gap: 10px; min-height: 0; }
.probe-term { flex: 1; min-width: 0; border: 1px solid #1e1e1e; border-radius: 6px; padding: 6px; background: #080909; height: 62vh; }
.probe-side { width: 46%; display: flex; flex-direction: column; gap: 8px; min-width: 0; }
.term-host { width: 100%; height: 100%; }
.mono { font-family: Consolas, monospace; font-size: 11px; }
.logbox { max-height: 34vh; overflow: auto; margin: 0; font-size: 11px; line-height: 1.5; white-space: pre-wrap; color: #b0b0b0; }
</style>

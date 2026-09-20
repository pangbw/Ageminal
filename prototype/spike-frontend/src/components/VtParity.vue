<script setup lang="ts">
// ─────────────────────────────────────────────────────────────────────────────
// #42 VT parity 探针：**daemon 侧 Rust VT 内核产出的 ANSI 快照，经 xterm.js 重放后
// 是否与参考屏幕一致。**
//
// 三份材料（由 `spike-daemon vt-parity` 产出，Windows 下走 ConPTY 实时流）：
//   full.bin  —— 完整原始字节流（= 从头重放的参考）
//   tail.bin  —— 末尾 N KB（“哑管道”基线：有界缓冲只能回放尾巴）
//   snap.ansi —— 内核快照（C 路线的交付物）
//   meta.json —— 尺寸 / alt-screen / 光标 / 内核逐列字符 / 逐格属性签名
//
// 三台 xterm.js 分别重放，然后**逐行逐格**与内核视角比对。
// 属性位定义（必须与 Rust 侧 `flag_bits` 一致）：
//   1=INVERSE 2=BOLD 4=ITALIC 8=UNDERLINE 16=DIM 32=HIDDEN 64=STRIKEOUT
// ─────────────────────────────────────────────────────────────────────────────
import { onMounted, ref, shallowRef } from "vue";
import { Terminal, type IBufferCell } from "@xterm/xterm";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { invoke } from "@tauri-apps/api/core";
import { setProbeResult } from "../probeStore";

const isTauri = "__TAURI_INTERNALS__" in window;

interface Meta {
  cols: number;
  rows: number;
  alt_screen: boolean;
  cursor: [number, number];
  full_bytes: number;
  tail_bytes: number;
  snapshot_bytes: number;
  lines: string[];
  style_rows: string[];
  cell_rows: string[];
  fixture_covers: string[];
}

interface Screen {
  cellRows: string[];
  styleRows: string[];
  alt: boolean;
  cursor: [number, number];
}

const meta = ref<Meta | null>(null);
const status = ref("（未加载）");
const results = ref<Array<Record<string, unknown>>>([]);
const verdict = ref<string>("");
const busy = ref(false);

const refEl = ref<HTMLDivElement | null>(null);
const tailEl = ref<HTMLDivElement | null>(null);
const snapEl = ref<HTMLDivElement | null>(null);
const terms = shallowRef<Terminal[]>([]);

function mkTerm(cols: number, rows: number, el: HTMLElement): Terminal {
  const t = new Terminal({
    cols,
    rows,
    fontSize: 8,
    lineHeight: 1,
    allowProposedApi: true,
    convertEol: false,
    scrollback: 0,
    theme: { background: "#080909", foreground: "#d4d4d4" },
  });
  t.loadAddon(new Unicode11Addon());
  // unicode 版本由 addon 在运行期挂上，类型里没有
  (t as unknown as { unicode: { activeVersion: string } }).unicode.activeVersion = "11";
  t.open(el);
  return t;
}

function cellSignature(cell: IBufferCell): string {
  let b = 0;
  if (cell.isInverse()) b |= 1;
  if (cell.isBold()) b |= 2;
  if (cell.isItalic()) b |= 4;
  if (cell.isUnderline()) b |= 8;
  if (cell.isDim()) b |= 16;
  if (cell.isInvisible()) b |= 32;
  if (cell.isStrikethrough()) b |= 64;
  return b.toString(16).padStart(2, "0");
}

/** 读回 xterm.js 的可见屏幕：按列对齐的字符 + 逐格属性签名 */
function readScreen(t: Terminal, cols: number, rows: number): Screen {
  const b = t.buffer.active;
  const cellRows: string[] = [];
  const styleRows: string[] = [];
  for (let y = 0; y < rows; y++) {
    const line = b.getLine(b.baseY + y);
    let chars = "";
    let styles = "";
    for (let x = 0; x < cols; x++) {
      const cell = line?.getCell(x);
      if (!cell || cell.getWidth() === 0) {
        chars += " ";
        styles += "--"; // 宽字符占位格：不参与样式比对
        continue;
      }
      const ch = cell.getChars();
      chars += ch && ch.length ? ch : " ";
      styles += cellSignature(cell);
    }
    cellRows.push(chars);
    styleRows.push(styles);
  }
  return {
    cellRows,
    styleRows,
    alt: b.type === "alternate",
    cursor: [b.cursorY, b.cursorX],
  };
}

function compare(name: string, got: Screen, m: Meta): Record<string, unknown> {
  const trim = (s: string) => s.replace(/\s+$/, "");
  const textDiff: string[] = [];
  let textBad = 0;
  for (let y = 0; y < m.rows; y++) {
    const a = trim(m.cell_rows[y] ?? "");
    const b = trim(got.cellRows[y] ?? "");
    if (a !== b) {
      textBad++;
      if (textDiff.length < 5) {
        textDiff.push(
          `行${y}: 内核=${JSON.stringify(a.slice(0, 48))} / xterm=${JSON.stringify(b.slice(0, 48))}`,
        );
      }
    }
  }
  let styleBad = 0;
  let styleCmp = 0;
  for (let y = 0; y < m.rows; y++) {
    const mr = m.style_rows[y] ?? "";
    const gr = got.styleRows[y] ?? "";
    for (let x = 0; x < m.cols; x++) {
      const mt = mr.slice(x * 2, x * 2 + 2);
      const gt = gr.slice(x * 2, x * 2 + 2);
      if (!mt || mt === "--" || gt === "--") continue;
      styleCmp++;
      if (mt !== gt) styleBad++;
    }
  }
  const altOk = got.alt === m.alt_screen;
  const cursorOk = got.cursor[0] === m.cursor[0] && got.cursor[1] === m.cursor[1];
  return {
    name,
    text_bad_lines: textBad,
    style_bad_cells: styleBad,
    style_compared_cells: styleCmp,
    alt_ok: altOk,
    alt_got: got.alt,
    alt_expected: m.alt_screen,
    cursor_ok: cursorOk,
    cursor_got: got.cursor,
    cursor_expected: m.cursor,
    text_diffs: textDiff,
    pass: textBad === 0 && styleBad === 0 && altOk && cursorOk,
  };
}

async function loadFixtures(): Promise<{ meta: Meta; full: Uint8Array; tail: Uint8Array; snap: string }> {
  if (isTauri) {
    const f = await invoke<{ meta: string; snap: string; full: number[]; tail: number[] }>("read_vt_parity");
    return {
      meta: JSON.parse(f.meta) as Meta,
      full: new Uint8Array(f.full),
      tail: new Uint8Array(f.tail),
      snap: f.snap,
    };
  }
  const base = "/vt-parity";
  const m = (await (await fetch(`${base}/meta.json`)).json()) as Meta;
  const full = new Uint8Array(await (await fetch(`${base}/full.bin`)).arrayBuffer());
  const tail = new Uint8Array(await (await fetch(`${base}/tail.bin`)).arrayBuffer());
  const snap = await (await fetch(`${base}/snap.ansi`)).text();
  return { meta: m, full, tail, snap };
}

async function run(): Promise<void> {
  busy.value = true;
  results.value = [];
  verdict.value = "";
  try {
    status.value = "加载产物…";
    const f = await loadFixtures();
    meta.value = f.meta;

    // 重建三台终端（每次都新建，避免历史状态串味）
    terms.value.forEach((t) => t.dispose());
    terms.value = [];
    const m = f.meta;
    const t1 = mkTerm(m.cols, m.rows, refEl.value!);
    const t2 = mkTerm(m.cols, m.rows, tailEl.value!);
    const t3 = mkTerm(m.cols, m.rows, snapEl.value!);
    terms.value = [t1, t2, t3];

    status.value = "重放三种材料…";
    await new Promise<void>((r) => t1.write(f.full, () => r()));
    await new Promise<void>((r) => t2.write(f.tail, () => r()));
    await new Promise<void>((r) => t3.write(f.snap, () => r()));

    const got = [readScreen(t1, m.cols, m.rows), readScreen(t2, m.cols, m.rows), readScreen(t3, m.cols, m.rows)];
    const names = [
      "参考（full.bin 从头重放）",
      "哑管道基线（tail.bin 仅末尾）",
      "**内核快照（snap.ansi）**",
    ];
    const r = names.map((n, i) => compare(n, got[i], m));
    results.value = r;

    const snap = r[2];
    const refc = r[0];
    verdict.value = snap.pass
      ? `✅ 达标：内核快照与参考屏幕一致（文本 0 行差异、样式 0 格差异、alt-screen 与光标一致）；同时参考重放自身对齐 ${refc.pass ? "通过" : "未通过"}`
      : `❌ 不达标：内核快照存在差异（文本 ${snap.text_bad_lines} 行 / 样式 ${snap.style_bad_cells} 格 / alt ${snap.alt_ok ? "ok" : "mismatch"} / cursor ${snap.cursor_ok ? "ok" : "mismatch"}）→ 按 #42 判定需评估回退 Node sidecar`;
    status.value = "完成";
    sync();
  } catch (e) {
    status.value = `失败：${String(e)}`;
    sync();
  } finally {
    busy.value = false;
  }
}

function sync(): void {
  setProbeResult("vtparity", {
    loaded: meta.value !== null,
    verdict: verdict.value,
    results: results.value,
    meta_summary: meta.value
      ? {
          cols: meta.value.cols,
          rows: meta.value.rows,
          alt_screen: meta.value.alt_screen,
          cursor: meta.value.cursor,
          full_bytes: meta.value.full_bytes,
          tail_bytes: meta.value.tail_bytes,
          snapshot_bytes: meta.value.snapshot_bytes,
        }
      : null,
    note: "参考=full.bin 从头重放；哑管道基线=tail.bin 仅末尾；内核快照=snap.ansi。文本按列对齐比对，样式比对跳过宽字符占位格。",
  });
}

onMounted(() => {
  sync();
});
</script>

<template>
  <section class="probe">
    <div class="bar">
      <strong>#42 VT parity</strong>
      <button :disabled="busy" @click="run">{{ busy ? "运行中…" : "加载产物并比对" }}</button>
      <span class="hint">{{ status }}</span>
      <span class="spacer" />
      <span class="hint">{{ isTauri ? "Tauri：读 %TEMP%\\agm-vt-parity\\" : "浏览器：读 /vt-parity/" }}</span>
    </div>

    <p v-if="meta" class="hint">
      尺寸 {{ meta.cols }}×{{ meta.rows }} · alt_screen={{ meta.alt_screen }} · cursor=({{ meta.cursor[0] }},{{
        meta.cursor[1]
      }}) · full={{ meta.full_bytes }}B / tail={{ meta.tail_bytes }}B / snap={{ meta.snapshot_bytes }}B
    </p>
    <p v-if="meta && meta.tail_bytes >= meta.full_bytes" class="warn">
      ⚠️ tail 未截断（tail == full），「哑管道基线」这一行**不构成证据**——请在 daemon 侧用更小的
      <code>--tail</code>（例如 512）重跑，让有界回放真的丢掉开头。
    </p>
    <p v-if="verdict" class="verdict" :class="{ bad: !verdict.startsWith('✅') }">{{ verdict }}</p>

    <table v-if="results.length" class="tbl">
      <thead>
        <tr>
          <th>材料</th>
          <th>文本差异行</th>
          <th>样式差异格</th>
          <th>比对格数</th>
          <th>alt-screen</th>
          <th>光标</th>
          <th>判定</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="r in results" :key="String(r.name)">
          <td>{{ r.name }}</td>
          <td>{{ r.text_bad_lines }}</td>
          <td>{{ r.style_bad_cells }}</td>
          <td>{{ r.style_compared_cells }}</td>
          <td>{{ r.alt_ok ? "✅" : `❌ (${r.alt_got}/${r.alt_expected})` }}</td>
          <td>{{ r.cursor_ok ? "✅" : `❌ (${r.cursor_got}/${r.cursor_expected})` }}</td>
          <td>{{ r.pass ? "✅" : "❌" }}</td>
        </tr>
      </tbody>
    </table>

    <div v-for="r in results" :key="String(r.name) + '-d'" class="diffs">
      <template v-if="(r.text_diffs as string[]).length">
        <div class="hint">{{ r.name }} 的文本差异（最多 5 条）：</div>
        <pre v-for="(d, i) in (r.text_diffs as string[])" :key="i" class="mono">{{ d }}</pre>
      </template>
    </div>

    <div class="cols">
      <div class="col">
        <div class="cap">参考 · full.bin</div>
        <div ref="refEl" class="term" />
      </div>
      <div class="col">
        <div class="cap">哑管道基线 · tail.bin</div>
        <div ref="tailEl" class="term" />
      </div>
      <div class="col">
        <div class="cap">内核快照 · snap.ansi</div>
        <div ref="snapEl" class="term" />
      </div>
    </div>
  </section>
</template>

<style scoped>
.probe { padding: 8px 10px; }
.bar { display: flex; align-items: center; gap: 8px; margin-bottom: 6px; }
.spacer { flex: 1; }
.hint { color: #949cab; font-size: 11px; }
.warn { font-size: 11.5px; color: #e5c07b; margin: 4px 0; line-height: 1.7; }
.warn code { font-family: Consolas, monospace; color: #9cdcfe; }
.verdict { font-size: 12px; color: #4ec9b0; margin: 6px 0; line-height: 1.7; }
.verdict.bad { color: #ff616e; }
.tbl { width: 100%; border-collapse: collapse; font-size: 12px; margin-bottom: 8px; }
.tbl th, .tbl td { border-bottom: 1px solid #212121; padding: 3px 6px; text-align: left; }
.tbl th { color: #9cdcfe; font-weight: 600; }
.diffs { margin-bottom: 6px; }
.mono { font-family: Consolas, monospace; font-size: 10.5px; margin: 2px 0; white-space: pre-wrap; color: #b0b0b0; }
.cols { display: flex; gap: 8px; overflow: auto; }
.col { min-width: 0; }
.cap { font-size: 11px; color: #9cdcfe; margin-bottom: 2px; }
.term { border: 1px solid #212121; }
</style>

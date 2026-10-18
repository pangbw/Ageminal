/**
 * #71 / #42 跨引擎 parity：daemon 产出的 ANSI 快照经 xterm.js 重放后，
 * 与内核视角逐格比对。
 *
 * 三份材料来自 `crates/daemon/tests/fixtures/vt-parity/`：
 * - `full.bin` 完整原始流（参考屏幕）
 * - `snap.ansi` 内核快照
 * - `tail.bin` 末尾 512 字节（哑管道基线，必须失败）
 *
 * 属性位定义与 Rust 侧 `cell_flags` 一致：
 * 1=INVERSE 2=BOLD 4=ITALIC 8=UNDERLINE 16=DIM 32=HIDDEN 64=STRIKEOUT。
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { Unicode11Addon } from "@xterm/addon-unicode11";
import { Terminal, type IBufferCell } from "@xterm/xterm";
import { describe, expect, it } from "vitest";

type Meta = {
  cols: number;
  rows: number;
  alt_screen: boolean;
  cursor: [number, number];
  lines: string[];
  style_rows: string[];
  cell_rows: string[];
};

const FIXTURES = join(process.cwd(), "crates/daemon/tests/fixtures/vt-parity");
const meta: Meta = JSON.parse(readFileSync(join(FIXTURES, "meta.json"), "utf8"));
const snapshot = readFileSync(join(FIXTURES, "snap.ansi"), "utf8");
const full = new Uint8Array(readFileSync(join(FIXTURES, "full.bin")));
const tail = new Uint8Array(readFileSync(join(FIXTURES, "tail.bin")));

type Screen = {
  alt: boolean;
  cursor: [number, number];
  cellRows: string[];
  styleRows: string[];
};

function signature(cell: IBufferCell): string {
  let bits = 0;
  if (cell.isInverse()) bits |= 1;
  if (cell.isBold()) bits |= 2;
  if (cell.isItalic()) bits |= 4;
  if (cell.isUnderline()) bits |= 8;
  if (cell.isDim()) bits |= 16;
  if (cell.isInvisible()) bits |= 32;
  if (cell.isStrikethrough()) bits |= 64;
  return bits.toString(16).padStart(2, "0");
}

async function replay(data: string | Uint8Array): Promise<Screen> {
  const host = document.createElement("div");
  host.style.cssText = "width:800px;height:300px;";
  document.body.appendChild(host);

  const term = new Terminal({
    cols: meta.cols,
    rows: meta.rows,
    scrollback: 0,
    allowProposedApi: true,
    convertEol: false,
    fontSize: 8,
    lineHeight: 1,
  });
  term.loadAddon(new Unicode11Addon());
  term.unicode.activeVersion = "11";
  term.open(host);
  await new Promise<void>((resolve) => term.write(data, resolve));

  const buffer = term.buffer.active;
  const cellRows: string[] = [];
  const styleRows: string[] = [];
  for (let y = 0; y < meta.rows; y += 1) {
    const line = buffer.getLine(buffer.baseY + y);
    let text = "";
    let styles = "";
    for (let x = 0; x < meta.cols; x += 1) {
      const cell = line?.getCell(x);
      if (!cell || cell.getWidth() === 0) {
        text += " ";
        styles += "--";
        continue;
      }
      const chars = cell.getChars();
      text += chars.length > 0 ? chars : " ";
      styles += signature(cell);
    }
    cellRows.push(text);
    styleRows.push(styles);
  }

  const screen: Screen = {
    alt: buffer.type === "alternate",
    cursor: [buffer.cursorY, buffer.cursorX],
    cellRows,
    styleRows,
  };
  term.dispose();
  host.remove();
  return screen;
}

function compare(got: Screen): {
  textBad: number;
  styleBad: number;
  altOk: boolean;
  cursorOk: boolean;
  pass: boolean;
} {
  const trim = (value: string) => value.replace(/\s+$/, "");
  let textBad = 0;
  for (let y = 0; y < meta.rows; y += 1) {
    if (trim(meta.cell_rows[y] ?? "") !== trim(got.cellRows[y] ?? "")) textBad += 1;
  }

  let styleBad = 0;
  for (let y = 0; y < meta.rows; y += 1) {
    for (let x = 0; x < meta.cols; x += 1) {
      const expected = (meta.style_rows[y] ?? "").slice(x * 2, x * 2 + 2);
      const actual = (got.styleRows[y] ?? "").slice(x * 2, x * 2 + 2);
      if (!expected || expected === "--" || actual === "--") continue;
      if (expected !== actual) styleBad += 1;
    }
  }

  const altOk = got.alt === meta.alt_screen;
  const cursorOk = got.cursor[0] === meta.cursor[0] && got.cursor[1] === meta.cursor[1];
  return {
    textBad,
    styleBad,
    altOk,
    cursorOk,
    pass: textBad === 0 && styleBad === 0 && altOk && cursorOk,
  };
}

describe("VT snapshot parity (#71 / #42)", () => {
  it("replays the daemon snapshot cell-for-cell", async () => {
    const result = compare(await replay(snapshot));
    expect(result).toEqual({
      textBad: 0,
      styleBad: 0,
      altOk: true,
      cursorOk: true,
      pass: true,
    });
  });

  it("proves the bounded-tail baseline is insufficient", async () => {
    const result = compare(await replay(tail));
    expect(result.pass).toBe(false);
  });

  it("keeps the full replay as a sanity reference", async () => {
    expect(compare(await replay(full)).pass).toBe(true);
  });
});

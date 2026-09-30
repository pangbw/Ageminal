import { readdirSync, readFileSync } from "node:fs";
import { join, relative, sep } from "node:path";

import { describe, expect, it } from "vitest";
import ts from "typescript";

/**
 * 门禁之一（REQUIREMENTS.md §11，issue #54）：`src/**` 里不许出现**中文字面量**。
 *
 * 白名单两类：**注释**（中英夹注是仓库风格）与 `src/i18n/locales/**`（文案本体）。
 * 做法是「把注释挖空，再看还剩不剩中日韩字符」——比找字符串字面量更保守：
 * 连标识符、属性名、CSS 值一起管住。
 */

const SRC_ROOT = join(process.cwd(), "src");
const ALLOWED_PREFIX = join(SRC_ROOT, "i18n", "locales");
const SCANNED_EXTENSIONS = [".ts", ".vue", ".js", ".mjs", ".css"];

const CJK = /[\u3000-\u303f\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff\uff00-\uffef]/u;
const HTML_COMMENT = /<!--[\s\S]*?-->/g;
const CSS_COMMENT = /\/\*[\s\S]*?\*\//g;
const VUE_BLOCK = /<(script|style)\b[^>]*>([\s\S]*?)<\/\1>/g;

type Range = [start: number, end: number];

type Finding = { line: number; column: number };

/** 把注释区间挖成空格（保留换行，行号不跑偏）。 */
function blankRanges(text: string, ranges: readonly Range[]): string {
  const chars = text.split("");
  for (const [start, end] of ranges) {
    for (let index = start; index < end && index < chars.length; index += 1) {
      if (chars[index] !== "\n") chars[index] = " ";
    }
  }
  return chars.join("");
}

function matchRanges(pattern: RegExp, text: string): Range[] {
  return [...text.matchAll(pattern)].map((match) => [
    match.index ?? 0,
    (match.index ?? 0) + match[0].length,
  ]);
}

/**
 * JS / TS 的注释区间。
 *
 * 用 TypeScript 的解析器逐节点取「前导 trivia」——正则字面量、模板串里的
 * `//`、`/*` 都不会被认成注释（裸 `ts.createScanner` 会在这里翻车）。
 * 区间相对 `script` 自身；`.vue` 里再整体加上偏移。
 */
function scriptCommentRanges(script: string): Range[] {
  const file = ts.createSourceFile("scan.ts", script, ts.ScriptTarget.Latest, true);
  const ranges: Range[] = [];

  const collectTrivia = (from: number, to: number) => {
    if (to <= from) return;
    const trivia = script.slice(from, to);
    for (const comment of ts.getLeadingCommentRanges(trivia, 0) ?? []) {
      ranges.push([from + comment.pos, from + comment.end]);
    }
  };

  const visit = (node: ts.Node) => {
    collectTrivia(node.getFullStart(), node.getStart(file));
    ts.forEachChild(node, visit);
  };
  visit(file);
  // 文件末尾的注释不属于任何节点。
  collectTrivia(file.endOfFileToken.getFullStart(), script.length);

  return ranges;
}

/** `.vue`：挖掉 HTML 注释、`<script>` 里的 JS 注释、`<style>` 里的 CSS 注释。 */
function maskVue(source: string): string {
  const ranges: Range[] = matchRanges(HTML_COMMENT, source);

  for (const match of source.matchAll(VUE_BLOCK)) {
    const [tag, body] = [match[1], match[2]];
    const bodyStart = (match.index ?? 0) + match[0].length - body.length - `</${tag}>`.length;
    const inner =
      tag === "style" ? matchRanges(CSS_COMMENT, body) : scriptCommentRanges(body);

    for (const [start, end] of inner) {
      ranges.push([bodyStart + start, bodyStart + end]);
    }
  }
  return blankRanges(source, ranges);
}

function maskComments(file: string, source: string): string {
  if (file.endsWith(".vue")) return maskVue(source);
  if (file.endsWith(".css")) return blankRanges(source, matchRanges(CSS_COMMENT, source));
  return blankRanges(source, scriptCommentRanges(source));
}

function findCjk(masked: string): Finding[] {
  const findings: Finding[] = [];
  let line = 1;
  let lineStart = 0;

  for (let index = 0; index < masked.length; index += 1) {
    const char = masked[index];
    if (char === "\n") {
      line += 1;
      lineStart = index + 1;
    } else if (CJK.test(char)) {
      findings.push({ line, column: index - lineStart + 1 });
    }
  }
  return findings;
}

function scanFile(file: string, source: string): Finding[] {
  return findCjk(maskComments(file, source));
}

function collectFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) {
      return full.startsWith(ALLOWED_PREFIX) ? [] : collectFiles(full);
    }
    if (!entry.isFile()) return [];
    return SCANNED_EXTENSIONS.some((extension) => entry.name.endsWith(extension)) ? [full] : [];
  });
}

/** 夹具里的中文写成转义序列：这个文件本身也得过这道门禁。 */
const CN = "\u4e2d";

describe("masking comments", () => {
  it("ignores Chinese inside line and block comments", () => {
    const source = `// ${CN}\nconst a = 1;\n/* ${CN} */\nconst b = 2;\n`;

    expect(scanFile("a.ts", source)).toEqual([]);
  });

  it("still reports Chinese in string and template literals", () => {
    const source = `const a = "${CN}";\nconst b = \`${CN}\`;\n`;

    expect(scanFile("a.ts", source)).toEqual([
      { line: 1, column: 12 },
      { line: 2, column: 12 },
    ]);
  });

  it("does not mistake a regex literal for a comment", () => {
    const source = `const slash = /\\/\\//;\nconst a = "${CN}";\n`;

    expect(scanFile("a.ts", source)).toEqual([{ line: 2, column: 12 }]);
  });

  it("splits a Vue component into template, script and style", () => {
    const source = [
      "<script setup lang=\"ts\">",
      `// ${CN}`,
      `const a = "${CN}";`,
      "</script>",
      "<template>",
      `  <!-- ${CN} -->`,
      `  <p>${CN}</p>`,
      "</template>",
      "<style scoped>",
      `/* ${CN} */`,
      `.a { font-family: "${CN}"; }`,
      "</style>",
      "",
    ].join("\n");

    expect(scanFile("A.vue", source)).toEqual([
      { line: 3, column: 12 },
      { line: 7, column: 6 },
      { line: 11, column: 20 },
    ]);
  });

  it("ignores Chinese inside CSS comments", () => {
    expect(scanFile("a.css", `/* ${CN} */\n.a { color: red; }\n`)).toEqual([]);
    expect(scanFile("a.css", `.a { content: "${CN}"; }\n`)).toEqual([{ line: 1, column: 16 }]);
  });
});

describe("the src tree", () => {
  it("keeps Chinese literals inside src/i18n/locales", () => {
    const files = collectFiles(SRC_ROOT);
    // 别静默地扫了个空目录（路径算错时最容易这样）。
    expect(files.length).toBeGreaterThan(10);

    const offenders = files.flatMap((file) =>
      scanFile(file, readFileSync(file, "utf8")).map(
        (finding) =>
          `${relative(SRC_ROOT, file).split(sep).join("/")}:${finding.line}:${finding.column}`,
      ),
    );

    expect(offenders, "Chinese must live in src/i18n/locales/**").toEqual([]);
  });
});

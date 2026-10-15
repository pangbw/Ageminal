#!/usr/bin/env node
// 前端依赖许可审计（#63 / REQUIREMENTS.md §20）。
//
// 用 pnpm 内建的 `licenses list --json`（不引额外的 `license-checker` 依赖），按 **SPDX
// 白名单**判定：任何 GPL / AGPL / SSPL 系许可、任何不在白名单里的许可、以及「无许可 / 未知」
// 都视为违规，退出码非 0（CI 门禁）。
//
// SPDX 表达式按分量看：`MIT OR Apache-2.0` 的每个分量都必须在白名单里（保守；双许可里夹一个
// 禁用许可也拦下）。
import { spawnSync } from "node:child_process";

/** 允许的 SPDX 许可 id（宽松 / 弱著佐权；不含 GPL 系）。 */
const ALLOW = new Set([
  "0BSD",
  "Apache-2.0",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "BlueOak-1.0.0",
  "CC-BY-4.0",
  "CC0-1.0",
  "ISC",
  "MIT",
  "MIT-0",
  "MPL-2.0",
  "Python-2.0",
  "Unlicense",
  "Zlib",
]);

/** 直接拒绝的许可（含 LGPL —— 比 §20 的「GPL/AGPL/SSPL」更严，杜绝一切著佐权）。 */
const DENY = /GPL|SSPL|BUSL|Elastic-2\.0|Commons-Clause/i;

/** SPDX 表达式里不算许可 id 的连接词。 */
const OPERATORS = new Set(["OR", "AND", "WITH"]);

/** 取 `licenses list --json` 的原始输出。 */
function listJson() {
  // Windows 上 `pnpm` 是 `pnpm.cmd`，得经 shell 才能找到。
  const result = spawnSync("pnpm", ["licenses", "list", "--json"], {
    encoding: "utf8",
    shell: process.platform === "win32",
  });
  if (result.error) {
    console.error(`无法执行 pnpm licenses：${result.error.message}`);
    process.exit(1);
  }
  if (result.status !== 0) {
    console.error(result.stderr || "pnpm licenses 失败");
    process.exit(result.status ?? 1);
  }
  return JSON.parse(result.stdout || "{}");
}

/** 一个许可字符串的违规分量；空数组 = 通过。 */
function offendingTokens(license) {
  const text = String(license ?? "").trim();
  if (text === "") return ["<empty>"];
  if (DENY.test(text)) return [text];
  const tokens = text
    .split(/[\s()]+/)
    .map((token) => token.replace(/\+$/, ""))
    .filter((token) => token !== "" && !OPERATORS.has(token));
  return tokens.filter((token) => !ALLOW.has(token));
}

const report = listJson();
const violations = [];
let total = 0;
for (const [license, packages] of Object.entries(report)) {
  total += packages.length;
  const offenders = offendingTokens(license);
  if (offenders.length > 0) {
    for (const pkg of packages) {
      violations.push(`${pkg.name}@${pkg.versions?.join(",") ?? "?"} → ${license}`);
    }
  }
}

if (violations.length > 0) {
  console.error(`许可审计未通过（${violations.length} 个包）：`);
  for (const line of violations) console.error(`  - ${line}`);
  process.exit(1);
}

console.log(`许可审计通过：${total} 个包，全部落在白名单内。`);

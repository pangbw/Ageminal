#!/usr/bin/env node
// 把 daemon / notify 的 release 产物复制为 Tauri sidecar 命名：
//   src-tauri/binaries/<name>-<target-triple>[.exe]
//
// 这些产物不入版本控制（见 .gitignore），由 `pnpm sidecars` 生成。
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const OUT_DIR = join(ROOT, "src-tauri", "binaries");
const SIDECARS = ["ageminal-daemon", "ageminal-notify"];

function targetTriple() {
  // Tauri CLI 会带上目标三元组；独立运行时回退到 rustc 的 host。
  if (process.env.TAURI_ENV_TARGET_TRIPLE) {
    return process.env.TAURI_ENV_TARGET_TRIPLE;
  }
  const out = execFileSync("rustc", ["-vV"], { encoding: "utf8" });
  const host = out.split("\n").find((line) => line.startsWith("host:"));
  if (!host) {
    throw new Error("无法从 `rustc -vV` 解析 host triple");
  }
  return host.slice("host:".length).trim();
}

const triple = targetTriple();
const ext = triple.includes("windows") ? ".exe" : "";
const cargoArgs = ["build", "--release", ...SIDECARS.flatMap((name) => ["-p", name])];

console.log(`> cargo ${cargoArgs.join(" ")}`);
execFileSync("cargo", cargoArgs, { stdio: "inherit", cwd: ROOT });

mkdirSync(OUT_DIR, { recursive: true });
for (const name of SIDECARS) {
  const from = join(ROOT, "target", "release", `${name}${ext}`);
  const to = join(OUT_DIR, `${name}-${triple}${ext}`);
  copyFileSync(from, to);
  console.log(`sidecar → ${name}-${triple}${ext}`);
}

console.log(`完成：${SIDECARS.length} 个 sidecar（${triple}）。`);

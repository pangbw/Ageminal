#!/usr/bin/env node
// 平台感知的本地检查（见 issue #50）。
//
// Windows：检查整个 workspace（含 src-tauri 桌面 crate）。
// 其他平台：src-tauri 需要 GTK / WebKit 系统库，跳过它，只检查三个核心 crate。
import { spawnSync } from "node:child_process";

const isWindows = process.platform === "win32";
const coreCrates = [
  "-p",
  "ageminal-protocol",
  "-p",
  "ageminal-daemon",
  "-p",
  "ageminal-notify",
];

const rustTargets = isWindows ? ["--all-targets"] : [...coreCrates, "--all-targets"];
const rustTests = isWindows ? ["--all"] : coreCrates;

const steps = [
  ["cargo", ["fmt", "--all", "--check"]],
  ["cargo", ["clippy", ...rustTargets, "--", "-D", "warnings"]],
  ["cargo", ["test", ...rustTests]],
  ["pnpm", ["lint"]],
  ["pnpm", ["typecheck"]],
];

if (!isWindows) {
  console.log("非 Windows 平台：跳过 src-tauri 桌面 crate（缺 GTK / WebKit 系统库）。");
}

for (const [cmd, args] of steps) {
  console.log(`\n$ ${cmd} ${args.join(" ")}`);
  const result = spawnSync(cmd, args, {
    stdio: "inherit",
    shell: process.platform === "win32",
  });
  if (result.status !== 0) {
    console.error(`\n失败：${cmd} ${args.join(" ")}`);
    process.exit(result.status ?? 1);
  }
}

console.log("\n检查全部通过。");

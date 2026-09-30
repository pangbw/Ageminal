#!/usr/bin/env node
// 平台感知的 Rust 检查（见 issue #50）。
//
// Windows：检查整个 workspace（含 src-tauri 桌面 crate）。
// 其他平台：src-tauri 需要 GTK / WebKit 系统库，跳过它，只检查三个核心 crate。
//
// 只调用 cargo，不递归调用 pnpm —— lint / typecheck 由 package.json 的 check 组合。
// 不使用 shell，避免 Node 的 DEP0190（shell + args）。
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

const steps = [
  ["cargo", ["fmt", "--all", "--check"]],
  [
    "cargo",
    [
      "clippy",
      ...(isWindows ? ["--all-targets"] : [...coreCrates, "--all-targets"]),
      "--",
      "-D",
      "warnings",
    ],
  ],
  ["cargo", ["test", ...(isWindows ? ["--all"] : coreCrates)]],
];

if (!isWindows) {
  console.log("非 Windows 平台：跳过 src-tauri 桌面 crate（缺 GTK / WebKit 系统库）。");
}

for (const [cmd, args] of steps) {
  console.log(`\n$ ${cmd} ${args.join(" ")}`);
  const result = spawnSync(cmd, args, { stdio: "inherit" });
  if (result.error) {
    console.error(`\n无法执行 ${cmd}：${result.error.message}`);
    process.exit(1);
  }
  if (result.status !== 0) {
    console.error(`\n失败：${cmd} ${args.join(" ")}`);
    process.exit(result.status ?? 1);
  }
}

console.log("\nRust 检查通过。");

#!/usr/bin/env node
// 校验生成的 TypeScript 绑定与 Rust 源码一致（无 diff）。
//
// 做法：重新生成 src/bindings.ts，再用 `git diff --exit-code` 比对。
// 手改生成物（已暂存或已提交）会在这一步被抓到。
//
// 非 Windows 平台跳过：生成绑定需要编译 src-tauri（缺 GTK / WebKit 编不过）。
import { spawnSync } from "node:child_process";

const BINDINGS = "src/bindings.ts";

if (process.platform !== "win32") {
  console.log("非 Windows 平台：跳过绑定无 diff 校验（无法编译 src-tauri）。");
  process.exit(0);
}

function run(cmd, args) {
  console.log(`\n$ ${cmd} ${args.join(" ")}`);
  const result = spawnSync(cmd, args, { stdio: "inherit" });
  if (result.error) {
    console.error(`\n无法执行 ${cmd}：${result.error.message}`);
    process.exit(1);
  }
  return result.status ?? 1;
}

if (run("cargo", ["run", "-p", "ageminal-desktop", "--features", "bindgen", "--bin", "export_bindings"]) !== 0) {
  process.exit(1);
}

if (run("git", ["diff", "--exit-code", "--", BINDINGS]) !== 0) {
  console.error(
    `\n绑定与源码不一致： ${BINDINGS} 重新生成后与仓库中的版本不同。\n` +
      "请检查改动并提交（本地生成用 `pnpm bindings`）。",
  );
  process.exit(1);
}

console.log(`\n绑定与源码一致（${BINDINGS}）。`);

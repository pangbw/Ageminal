/**
 * 窗口命令的薄封装（REQUIREMENTS.md §6、§12；#15 决议：自绘标题栏）。
 *
 * 两件事：① 组件不直接摸 `@tauri-apps/api/window`，将来换实现只改这一处；
 * ② **宿主缺席时安全失败**——`pnpm dev:web`（纯浏览器，没有 Tauri）里窗口命令一定抛，
 * 不该让外壳跟着崩（同 `i18n/index.ts` 里探针的写法）。
 */
import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

/**
 * 对当前窗口做一次操作。
 *
 * 没有 Tauri 宿主（`pnpm dev:web` 的纯浏览器）时**安静跳过**——那种环境下窗口命令必然抛，
 * 属于预期；有宿主却失败（权限没放开、窗口已销毁）才值得记一条日志。
 * 日志保持 ASCII：`src/**` 不许出现中文字面量（#54 门禁）。
 */
async function withCurrentWindow(
  action: (window: ReturnType<typeof getCurrentWindow>) => Promise<unknown>,
): Promise<void> {
  if (!isTauri()) return;

  try {
    await action(getCurrentWindow());
  } catch (error) {
    console.error("[shell] window command failed", error);
  }
}

/** 改窗口标题（§6：`Ageminal — <激活 worktree 名> (<分支>)`）。 */
export function setWindowTitle(title: string): Promise<void> {
  return withCurrentWindow((window) => window.setTitle(title));
}

export function minimizeWindow(): Promise<void> {
  return withCurrentWindow((window) => window.minimize());
}

export function toggleMaximizeWindow(): Promise<void> {
  return withCurrentWindow((window) => window.toggleMaximize());
}

/**
 * 关窗。#76 之前就是真的关窗（会退出应用）；托盘与「关窗 = 隐藏到托盘」的三层
 * 退出语义（§12）由 #76 接管——那时这里改调 hide。
 */
export function closeWindow(): Promise<void> {
  return withCurrentWindow((window) => window.close());
}

/**
 * 窗口标题跟随激活 worktree（REQUIREMENTS.md §6）。
 *
 * 格式串在 locale 文件里（`shell.windowTitle.*`），品牌名复用 `common.appName`，
 * 避免同一个产品名在文案里写两遍。没有分支可显示时（如 bare 主工作树）用另一条格式串，
 * 不然会拼出 `Ageminal — bare ()`。
 */
import { watchEffect } from "vue";

import { translate as t } from "../../i18n/translate";
import { useWorkspaceStore, type ActiveWorktree } from "../../stores/workspace";
import { setWindowTitle } from "./window";

/** `Ageminal — <激活 worktree 名> (<分支>)`；没有激活项时只留应用名。 */
export function formatWindowTitle(active: ActiveWorktree | null): string {
  if (active === null) return t("common.appName");

  if (active.branch === null) {
    return t("shell.windowTitle.activeNoBranch", {
      app: t("common.appName"),
      worktree: active.name,
    });
  }

  return t("shell.windowTitle.active", {
    app: t("common.appName"),
    worktree: active.name,
    branch: active.branch,
  });
}

/** 把窗口标题绑到激活 worktree；激活项或语言一变就重算。 */
export function useWindowTitle(): void {
  const workspace = useWorkspaceStore();

  watchEffect(() => {
    void setWindowTitle(formatWindowTitle(workspace.activeWorktree));
  });
}

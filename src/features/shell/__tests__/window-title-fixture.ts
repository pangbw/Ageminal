import { localeString } from "../../../i18n/__tests__/locale-files";
import type { Locale } from "../../../i18n/detect";

/**
 * 期望的窗口标题：用 locale 文件里的格式串拼出来，别把 `Ageminal — …` 的写法在测试里
 * 再抄一遍（改格式串时测试应跟着它走，而不是跟着一份副本走）。
 */
export function expectedWindowTitle(locale: Locale, name: string, branch: string): string {
  return localeString(locale, "shell", ["windowTitle", "active"])
    .replace("{app}", localeString(locale, "common", ["appName"]))
    .replace("{worktree}", name)
    .replace("{branch}", branch);
}

/** 没有分支可显示时（如 bare 主工作树）的期望标题。 */
export function expectedWindowTitleNoBranch(locale: Locale, name: string): string {
  return localeString(locale, "shell", ["windowTitle", "activeNoBranch"])
    .replace("{app}", localeString(locale, "common", ["appName"]))
    .replace("{worktree}", name);
}

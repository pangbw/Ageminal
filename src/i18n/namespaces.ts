/**
 * i18n 的域命名空间（REQUIREMENTS.md §11，issue #54）。
 *
 * 新增一个域 = 这里加一项 + 两个 locale 目录各加一个同名 JSON（`zh-CN` 是类型源），
 * 再在 `index.ts` 的两处映射里各加一行 —— 漏改的话 `satisfies` 与 parity 用例会一起把门。
 */
export const NAMESPACES = [
  "common",
  "shell",
  "terminal",
  "tabs",
  "files",
  "agents",
  "settings",
  "errors",
] as const;

export type Namespace = (typeof NAMESPACES)[number];

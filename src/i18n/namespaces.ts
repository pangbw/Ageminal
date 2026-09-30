/**
 * i18n 的域命名空间（REQUIREMENTS.md §11，issue #54）。
 *
 * 新增一个域 = 这里加一项 + 两个 locale 目录各加一个同名 JSON；
 * `index.ts` 的 `satisfies` 与 `locale-parity` 用例会一起把门。
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

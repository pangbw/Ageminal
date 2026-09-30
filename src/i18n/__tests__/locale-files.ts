import { readFileSync } from "node:fs";
import { join } from "node:path";

import type { Locale } from "../detect";
import type { Namespace } from "../namespaces";

/**
 * 直读 locale JSON 的**原文**。
 *
 * 测试里的 `import ... from "../locales/..."` 同样会被 `@intlify/unplugin-vue-i18n`
 * 预编译成消息 AST，拿不到人可读的字符串，所以这里绕过 Vite 直接读文件。
 */
export function readLocaleFile(locale: Locale, namespace: Namespace): unknown {
  const file = join(process.cwd(), "src", "i18n", "locales", locale, `${namespace}.json`);
  return JSON.parse(readFileSync(file, "utf8")) as unknown;
}

/** 取一个叶子上的文案，路径形如 `["appInfo", "loading"]`。 */
export function localeString(locale: Locale, namespace: Namespace, path: readonly string[]): string {
  let node: unknown = readLocaleFile(locale, namespace);
  for (const key of path) {
    node = (node as Record<string, unknown>)[key];
  }
  return node as string;
}

import { readdirSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { LOCALES, type Locale } from "../detect";
import { NAMESPACES, type Namespace } from "../namespaces";
import { readLocaleFile } from "./locale-files";

const LOCALES_DIR = join(process.cwd(), "src", "i18n", "locales");

/** 类型源：zh-CN 的 key 集合就是「应有的 key 集合」。 */
const TYPE_SOURCE: Locale = "zh-CN";

/** 拍平成 `shell.appInfo.loading` 这样的路径；**空子树也算一条**（`{}` ≠ `{"a":{}}`）。 */
function keyPaths(value: unknown, prefix = ""): string[] {
  if (value === null || typeof value !== "object") return [prefix];

  const entries = Object.entries(value as Record<string, unknown>);
  if (entries.length === 0) return [prefix];

  return entries.flatMap(([key, child]) => keyPaths(child, prefix ? `${prefix}.${key}` : key));
}

describe("locale files", () => {
  it("ships exactly the namespaces declared in namespaces.ts", () => {
    for (const locale of LOCALES) {
      const found = readdirSync(join(LOCALES_DIR, locale))
        .filter((name) => name.endsWith(".json"))
        .map((name) => name.slice(0, -".json".length))
        .sort();

      expect(found, locale).toEqual([...NAMESPACES].sort());
    }
  });

  it("has content in the type source, so the parity check is not vacuous", () => {
    for (const namespace of ["common", "shell"] satisfies Namespace[]) {
      const paths = keyPaths(readLocaleFile(TYPE_SOURCE, namespace)).filter(Boolean);
      expect(paths.length, namespace).toBeGreaterThan(0);
    }
  });

  it("keeps every other locale on the zh-CN key set", () => {
    for (const locale of LOCALES) {
      if (locale === TYPE_SOURCE) continue;

      for (const namespace of NAMESPACES) {
        const expected = keyPaths(readLocaleFile(TYPE_SOURCE, namespace)).sort();
        const actual = keyPaths(readLocaleFile(locale, namespace)).sort();
        const diff = {
          missing: expected.filter((key) => !actual.includes(key)),
          extra: actual.filter((key) => !expected.includes(key)),
        };

        expect(diff, `${locale}/${namespace}.json`).toEqual({ missing: [], extra: [] });
      }
    }
  });
});

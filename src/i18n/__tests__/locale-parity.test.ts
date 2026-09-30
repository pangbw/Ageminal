import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { SUPPORTED_LOCALES } from "../detect";
import { NAMESPACES, type Namespace } from "../namespaces";

const LOCALES_DIR = join(process.cwd(), "src", "i18n", "locales");

/** 类型源：zh-CN 的 key 集合就是「应有的 key 集合」。 */
const TYPE_SOURCE = "zh-CN";

function readNamespace(locale: string, namespace: string): unknown {
  const file = join(LOCALES_DIR, locale, `${namespace}.json`);
  return JSON.parse(readFileSync(file, "utf8"));
}

/** 拍平成 `shell.appInfo.loading` 这样的叶子路径。 */
function leafPaths(value: unknown, prefix = ""): string[] {
  if (value === null || typeof value !== "object") return [prefix];
  return Object.entries(value as Record<string, unknown>).flatMap(([key, child]) =>
    leafPaths(child, prefix ? `${prefix}.${key}` : key),
  );
}

describe("locale files", () => {
  it("ships exactly the namespaces declared in namespaces.ts", () => {
    for (const locale of SUPPORTED_LOCALES) {
      const found = readdirSync(join(LOCALES_DIR, locale))
        .filter((name) => name.endsWith(".json"))
        .map((name) => name.slice(0, -".json".length))
        .sort();

      expect(found, locale).toEqual([...NAMESPACES].sort());
    }
  });

  it("has content in the type source, so the parity check is not vacuous", () => {
    for (const namespace of ["common", "shell"] satisfies Namespace[]) {
      expect(leafPaths(readNamespace(TYPE_SOURCE, namespace)).length, namespace).toBeGreaterThan(0);
    }
  });

  it("keeps every other locale on the zh-CN key set", () => {
    for (const locale of SUPPORTED_LOCALES) {
      if (locale === TYPE_SOURCE) continue;

      for (const namespace of NAMESPACES) {
        const expected = leafPaths(readNamespace(TYPE_SOURCE, namespace)).sort();
        const actual = leafPaths(readNamespace(locale, namespace)).sort();
        const diff = {
          missing: expected.filter((key) => !actual.includes(key)),
          extra: actual.filter((key) => !expected.includes(key)),
        };

        expect(diff, `${locale}/${namespace}.json`).toEqual({ missing: [], extra: [] });
      }
    }
  });
});

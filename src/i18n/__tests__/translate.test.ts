import { readFileSync } from "node:fs";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { applyLocale } from "../index";
import { translate as t } from "../translate";

/**
 * 端到端确认「预编译 + 全局 scope」真的取得到文案：
 * 消息若没被预编译成可用形态，`t()` 会原样回吐 key，这里的断言就会红。
 */
function message(locale: string, namespace: string, path: readonly string[]): string {
  // 测试文件里的 locale JSON 也走预编译，拿不到原文，所以直读文件。
  const file = join(process.cwd(), "src", "i18n", "locales", locale, `${namespace}.json`);
  let node: unknown = JSON.parse(readFileSync(file, "utf8"));
  for (const key of path) {
    node = (node as Record<string, unknown>)[key];
  }
  return node as string;
}

describe("translate", () => {
  afterEach(() => {
    applyLocale("zh-CN");
  });

  it("reads the zh-CN messages", () => {
    applyLocale("zh-CN");

    expect(t("common.appName")).toBe("Ageminal");
    expect(t("shell.appInfo.loading")).toBe(message("zh-CN", "shell", ["appInfo", "loading"]));
  });

  it("interpolates named values", () => {
    applyLocale("zh-CN");

    expect(t("shell.appInfo.error", { message: "boom" })).toContain("boom");
  });

  it("switches locale without a reload", () => {
    const zh = message("zh-CN", "shell", ["appInfo", "loading"]);
    const en = message("en-US", "shell", ["appInfo", "loading"]);

    applyLocale("en-US");
    expect(t("shell.appInfo.loading")).toBe(en);
    expect(t("shell.appInfo.loading")).not.toBe(zh);

    applyLocale("zh-CN");
    expect(t("shell.appInfo.loading")).toBe(zh);
  });
});

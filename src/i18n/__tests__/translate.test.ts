import { afterEach, describe, expect, it } from "vitest";

import { applyLocale } from "../index";
import { translate as t } from "../translate";
import { localeString } from "./locale-files";

/**
 * 端到端确认「预编译 + 全局 scope」真的取得到文案：
 * 消息若没被预编译成可用形态，`t()` 会原样回吐 key，这里的断言就会红。
 */
describe("translate", () => {
  afterEach(() => {
    applyLocale("zh-CN");
  });

  it("reads the zh-CN messages", () => {
    applyLocale("zh-CN");

    expect(t("common.appName")).toBe("Ageminal");
    expect(t("shell.appInfo.loading")).toBe(localeString("zh-CN", "shell", ["appInfo", "loading"]));
  });

  it("interpolates named values", () => {
    applyLocale("zh-CN");

    expect(t("shell.appInfo.error", { message: "boom" })).toContain("boom");
  });

  it("switches locale without a reload", () => {
    const zh = localeString("zh-CN", "shell", ["appInfo", "loading"]);
    const en = localeString("en-US", "shell", ["appInfo", "loading"]);

    applyLocale("zh-CN");
    expect(t("shell.appInfo.loading")).toBe(zh);

    applyLocale("en-US");
    expect(t("shell.appInfo.loading")).toBe(en);
    expect(t("shell.appInfo.loading")).not.toBe(zh);
  });
});

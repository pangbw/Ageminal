import { describe, expect, it } from "vitest";

import { LOCALES, SELECTABLE_LOCALES, normalizeLocale } from "../detect";

describe("locale lists", () => {
  it("only opens up locales we have content for", () => {
    // §11：MVP 语言列表只列 zh-CN；en-US 先对齐 key 结构，内容补齐后再入列。
    expect([...SELECTABLE_LOCALES]).toEqual(["zh-CN"]);
    for (const locale of SELECTABLE_LOCALES) {
      expect(LOCALES).toContain(locale);
    }
  });
});

describe("normalizeLocale", () => {
  it("folds Chinese tags onto zh-CN", () => {
    expect(normalizeLocale("zh-CN")).toBe("zh-CN");
    expect(normalizeLocale("zh")).toBe("zh-CN");
    expect(normalizeLocale("zh-Hans-CN")).toBe("zh-CN");
    expect(normalizeLocale("zh_SG")).toBe("zh-CN");
    // 繁体也只有这一份中文内容，先给 zh-CN；等有 zh-TW 文案再细分。
    expect(normalizeLocale("zh-TW")).toBe("zh-CN");
  });

  it("ignores case and surrounding blanks", () => {
    expect(normalizeLocale("  ZH-hans  ")).toBe("zh-CN");
  });

  it("rejects locales that are not open for selection yet", () => {
    expect(normalizeLocale("en-US")).toBeNull();
    expect(normalizeLocale("en")).toBeNull();
    expect(normalizeLocale("de-DE")).toBeNull();
  });

  it("rejects empty tags", () => {
    expect(normalizeLocale("")).toBeNull();
    expect(normalizeLocale("   ")).toBeNull();
    expect(normalizeLocale(null)).toBeNull();
    expect(normalizeLocale(undefined)).toBeNull();
  });
});

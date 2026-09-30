import { describe, expect, it } from "vitest";

import { FALLBACK_LOCALE, normalizeLocale, pickLocale } from "../detect";

describe("normalizeLocale", () => {
  it("accepts the locales we ship content for", () => {
    expect(normalizeLocale("zh-CN")).toBe("zh-CN");
    expect(normalizeLocale("en-US")).toBe("en-US");
  });

  it("folds a primary subtag onto the content we have", () => {
    expect(normalizeLocale("zh")).toBe("zh-CN");
    expect(normalizeLocale("zh-Hans-CN")).toBe("zh-CN");
    expect(normalizeLocale("zh_SG")).toBe("zh-CN");
    expect(normalizeLocale("en")).toBe("en-US");
    expect(normalizeLocale("en-GB")).toBe("en-US");
  });

  it("ignores case and surrounding blanks", () => {
    expect(normalizeLocale("  ZH-hans  ")).toBe("zh-CN");
  });

  it("rejects traditional Chinese, which has no content yet", () => {
    expect(normalizeLocale("zh-TW")).toBeNull();
    expect(normalizeLocale("zh-Hant")).toBeNull();
    expect(normalizeLocale("zh-HK")).toBeNull();
    expect(normalizeLocale("zh-MO")).toBeNull();
  });

  it("rejects unknown and empty tags", () => {
    expect(normalizeLocale("de-DE")).toBeNull();
    expect(normalizeLocale("")).toBeNull();
    expect(normalizeLocale("   ")).toBeNull();
    expect(normalizeLocale(null)).toBeNull();
    expect(normalizeLocale(undefined)).toBeNull();
  });
});

describe("pickLocale", () => {
  it("takes the first candidate it knows", () => {
    expect(pickLocale(["de-DE", "en-GB", "zh-CN"])).toBe("en-US");
  });

  it("lets an unknown stored value fall through instead of blocking", () => {
    expect(pickLocale(["zh-TW", "en-US"])).toBe("en-US");
  });

  it("falls back to zh-CN when nothing is known", () => {
    expect(pickLocale([])).toBe(FALLBACK_LOCALE);
    expect(pickLocale([null, undefined, "de-DE"])).toBe(FALLBACK_LOCALE);
  });
});

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  getLanguage: vi.fn<() => Promise<string | null>>(),
  systemLocale: vi.fn<() => Promise<string | null>>(),
}));

vi.mock("../../bindings", () => ({
  commands: {
    getLanguage: mocks.getLanguage,
    systemLocale: mocks.systemLocale,
  },
}));

import { FALLBACK_LOCALE } from "../detect";
import { resolveInitialLocale } from "../index";

function stubBrowserLanguage(language: string) {
  vi.stubGlobal("navigator", { language });
}

describe("resolveInitialLocale", () => {
  beforeEach(() => {
    mocks.getLanguage.mockResolvedValue(null);
    mocks.systemLocale.mockResolvedValue(null);
    stubBrowserLanguage("zh-CN");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("uses the language persisted in Rust settings", async () => {
    mocks.getLanguage.mockResolvedValue("zh-CN");
    mocks.systemLocale.mockResolvedValue("zh-CN");

    await expect(resolveInitialLocale()).resolves.toBe("zh-CN");
  });

  it("stops asking other probes once Rust settings answer", async () => {
    mocks.getLanguage.mockResolvedValue("zh-CN");

    await resolveInitialLocale();

    expect(mocks.systemLocale).not.toHaveBeenCalled();
  });

  it("keeps asking when the stored value is not usable", async () => {
    mocks.getLanguage.mockResolvedValue("de-DE");
    mocks.systemLocale.mockResolvedValue("zh-CN");

    await expect(resolveInitialLocale()).resolves.toBe("zh-CN");
    expect(mocks.systemLocale).toHaveBeenCalled();
  });

  it("falls back to navigator.language when the OS locale is unusable", async () => {
    mocks.systemLocale.mockResolvedValue("de-DE");
    stubBrowserLanguage("zh-Hans-CN");

    await expect(resolveInitialLocale()).resolves.toBe("zh-CN");
  });

  it("keeps going when there is no Tauri host at all", async () => {
    mocks.getLanguage.mockRejectedValue(new Error("no tauri"));
    mocks.systemLocale.mockRejectedValue(new Error("no tauri"));
    stubBrowserLanguage("zh-Hans-CN");

    await expect(resolveInitialLocale()).resolves.toBe("zh-CN");
  });

  it("falls back to zh-CN when every probe is useless", async () => {
    mocks.systemLocale.mockResolvedValue("de-DE");
    stubBrowserLanguage("fr-FR");

    await expect(resolveInitialLocale()).resolves.toBe(FALLBACK_LOCALE);
  });

  it("does not select a locale that is not open for selection yet", async () => {
    const probe = {
      stored: async () => "en-US",
      system: async () => "zh-CN",
      browser: () => "zh-CN",
    };

    await expect(resolveInitialLocale(probe)).resolves.toBe("zh-CN");
  });
});

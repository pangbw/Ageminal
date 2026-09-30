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
    stubBrowserLanguage("en-US");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("prefers the language persisted in Rust settings", async () => {
    mocks.getLanguage.mockResolvedValue("zh-CN");
    mocks.systemLocale.mockResolvedValue("en-US");

    await expect(resolveInitialLocale()).resolves.toBe("zh-CN");
  });

  it("falls back to the plugin OS locale when nothing is stored", async () => {
    mocks.systemLocale.mockResolvedValue("zh-CN");

    await expect(resolveInitialLocale()).resolves.toBe("zh-CN");
  });

  it("falls back to navigator.language when the OS locale is unusable", async () => {
    mocks.systemLocale.mockResolvedValue("zh-TW");
    stubBrowserLanguage("en-GB");

    await expect(resolveInitialLocale()).resolves.toBe("en-US");
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

  it("lets a test drive the probe directly", async () => {
    const probe = {
      stored: async () => "en-US",
      system: async () => "zh-CN",
      browser: () => "zh-CN",
    };

    await expect(resolveInitialLocale(probe)).resolves.toBe("en-US");
  });
});

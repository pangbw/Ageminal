import { mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../terminal/runtime", () => ({
  close: vi.fn().mockResolvedValue(undefined),
  terminalState: vi.fn().mockReturnValue(undefined),
}));

import { commands } from "../../../bindings";
import { ok } from "../../../__tests__/workspace-fixture";
import { localeString } from "../../../i18n/__tests__/locale-files";
import { useTabsStore } from "../../../stores/tabs";
import NewTabMenu from "../NewTabMenu.vue";

vi.mock(
  "../../../bindings",
  async () => (await import("../../../__tests__/workspace-fixture")).createBindingsMock(),
);

const ROOT = "/ws/a";

function mountMenu() {
  const pinia = createPinia();
  setActivePinia(pinia);
  const tabs = useTabsStore();
  return { tabs, wrapper: mount(NewTabMenu, { props: { root: ROOT }, global: { plugins: [pinia] } }) };
}

describe("NewTabMenu", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("opens a terminal tab", async () => {
    const { tabs, wrapper } = mountMenu();

    await wrapper.findAll(".newtab__item")[0].trigger("click");

    expect(tabs.tabsFor(ROOT)).toHaveLength(1);
    expect(tabs.tabsFor(ROOT)[0].kind).toBe("terminal");
    expect(wrapper.emitted("close")).toHaveLength(1);
  });

  it("opens a preview from the system file picker and does nothing on cancel", async () => {
    const { tabs, wrapper } = mountMenu();
    vi.mocked(commands.pickFile).mockReturnValue(ok(`${ROOT}/README.md`));
    const preview = () =>
      wrapper
        .findAll(".newtab__item")
        .find((item) => item.text().includes(localeString("zh-CN", "tabs", ["menu", "file"])))!;

    await preview().trigger("click");
    await vi.waitFor(() => expect(tabs.tabsFor(ROOT)).toHaveLength(1));
    expect(tabs.tabsFor(ROOT)[0]).toMatchObject({ kind: "preview", name: "README.md" });

    vi.mocked(commands.pickFile).mockReturnValue(ok(null));
    await preview().trigger("click");
    await vi.waitFor(() => expect(wrapper.emitted("close")).toHaveLength(2));
    expect(tabs.tabsFor(ROOT)).toHaveLength(1);
  });

  it("keeps browser and other disabled", () => {
    const { wrapper } = mountMenu();
    const browser = wrapper
      .findAll(".newtab__item")
      .find((item) => item.text().includes(localeString("zh-CN", "tabs", ["menu", "browser"])))!;
    const other = wrapper
      .findAll(".newtab__item")
      .find((item) => item.text().includes(localeString("zh-CN", "tabs", ["menu", "other"])))!;

    expect(browser.attributes("disabled")).toBeDefined();
    expect(other.attributes("disabled")).toBeDefined();
    expect(browser.text()).toContain(localeString("zh-CN", "settings", ["comingSoon"]));
  });

  it("offers the four agents with settings colors and excludes dsh", async () => {
    const { tabs, wrapper } = mountMenu();

    const entries = wrapper.findAll("[data-agent-id]");
    expect(entries.map((entry) => entry.attributes("data-agent-id"))).toEqual([
      "claude",
      "opencode",
      "codex",
      "pi",
    ]);
    expect(wrapper.text()).not.toContain("dsh");

    await wrapper.find("[data-agent-id='claude']").trigger("click");

    expect(tabs.tabsFor(ROOT)[0]).toMatchObject({
      kind: "terminal",
      name: "Claude Code",
      agentId: "claude",
      agentColor: "#d97757",
    });
  });
});

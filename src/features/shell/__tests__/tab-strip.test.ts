import { mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { nextTick } from "vue";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../terminal/runtime", () => ({
  close: vi.fn().mockResolvedValue(undefined),
  terminalState: vi.fn().mockReturnValue({ status: "exited", visible: true }),
}));

import { project, snapshot, worktree } from "../../../__tests__/workspace-fixture";
import { localeString } from "../../../i18n/__tests__/locale-files";
import { useTabsStore } from "../../../stores/tabs";
import { useWorkspaceStore } from "../../../stores/workspace";
import TabStrip from "../TabStrip.vue";

vi.mock(
  "../../../bindings",
  async () => (await import("../../../__tests__/workspace-fixture")).createBindingsMock(),
);

const ROOT = "/ws/a";

function mountStrip() {
  const pinia = createPinia();
  setActivePinia(pinia);
  const workspace = useWorkspaceStore();
  workspace.hydrate(snapshot([project(ROOT, [worktree(ROOT)])], ROOT));
  const tabs = useTabsStore();
  tabs.openFile(ROOT, `${ROOT}/one.ts`, "one.ts");
  tabs.openFile(ROOT, `${ROOT}/two.ts`, "two.ts");
  tabs.openFile(ROOT, `${ROOT}/three.ts`, "three.ts");
  tabs.setActive(ROOT, `${ROOT}/one.ts`);

  return { tabs, wrapper: mount(TabStrip, { global: { plugins: [pinia] } }) };
}

describe("TabStrip", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  it("renders the active worktree's tabs and activates on click", async () => {
    const { tabs, wrapper } = mountStrip();

    expect(wrapper.findAll(".tab").map((tab) => tab.text())).toEqual([
      "one.ts",
      "two.ts",
      "three.ts",
    ]);
    expect(wrapper.get(".tab--active").text()).toBe("one.ts");

    await wrapper.findAll(".tab")[1].trigger("click");

    expect(tabs.activePreview(ROOT)?.name).toBe("two.ts");
    expect(wrapper.get(".tab--active").text()).toBe("two.ts");
  });

  it("swaps the whole group when the worktree changes and restores its active tab", async () => {
    const { tabs, wrapper } = mountStrip();
    const workspace = useWorkspaceStore();
    const other = "/ws/b";

    tabs.openFile(other, `${other}/b.ts`, "b.ts");
    tabs.setActive(ROOT, `${ROOT}/two.ts`);
    workspace.hydrate(
      snapshot([project(ROOT, [worktree(ROOT)]), project(other, [worktree(other)])], other),
    );
    await nextTick();

    expect(wrapper.findAll(".tab").map((tab) => tab.text())).toEqual(["b.ts"]);
    expect(wrapper.get(".tab--active").text()).toBe("b.ts");

    workspace.hydrate(
      snapshot([project(ROOT, [worktree(ROOT)]), project(other, [worktree(other)])], ROOT),
    );
    await nextTick();

    expect(wrapper.findAll(".tab").map((tab) => tab.text())).toEqual([
      "one.ts",
      "two.ts",
      "three.ts",
    ]);
    expect(wrapper.get(".tab--active").text()).toBe("two.ts");
  });

  it("closes a preview from the close button", async () => {
    const { tabs, wrapper } = mountStrip();

    await wrapper.findAll(".tab__close")[0].trigger("click");

    expect(tabs.tabsFor(ROOT).map((tab) => tab.name)).toEqual(["two.ts", "three.ts"]);
  });

  it("offers close / close others / close right from the context menu", async () => {
    const { tabs, wrapper } = mountStrip();

    await wrapper.findAll(".tab")[1].trigger("contextmenu", { clientX: 20, clientY: 20 });
    expect(wrapper.findAll(".tabmenu__item").map((item) => item.text())).toEqual([
      localeString("zh-CN", "tabs", ["context", "close"]),
      localeString("zh-CN", "tabs", ["context", "closeOthers"]),
      localeString("zh-CN", "tabs", ["context", "closeRight"]),
    ]);

    await wrapper.findAll(".tabmenu__item")[2].trigger("click");

    expect(tabs.tabsFor(ROOT).map((tab) => tab.name)).toEqual(["one.ts", "two.ts"]);
  });

  it("disables close others / close right when there is nothing to close", async () => {
    const { wrapper } = mountStrip();

    // 只剩一个页签时，右键菜单两项都该禁用。
    await wrapper.findAll(".tab__close")[1].trigger("click");
    await wrapper.findAll(".tab__close")[1].trigger("click");
    await wrapper.findAll(".tab")[0].trigger("contextmenu", { clientX: 20, clientY: 20 });

    const items = wrapper.findAll(".tabmenu__item");
    expect(items[1].attributes("disabled")).toBeDefined();
    expect(items[2].attributes("disabled")).toBeDefined();
  });
});

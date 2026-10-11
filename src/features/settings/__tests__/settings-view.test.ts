import { mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ok, project, snapshot, worktree } from "../../../__tests__/workspace-fixture";
import { commands } from "../../../bindings";
import { localeString } from "../../../i18n/__tests__/locale-files";
import { useWorkspaceStore } from "../../../stores/workspace";
import SettingsView from "../SettingsView.vue";

vi.mock(
  "../../../bindings",
  async () => (await import("../../../__tests__/workspace-fixture")).createBindingsMock(),
);

/** 一个项目、两个 worktree（其中一个还没加入侧栏）。 */
function state() {
  return snapshot([
    project(
      "/ws/a",
      [
        worktree("/ws/a", { name: "a", isMain: true, branchLabel: "main" }),
        worktree("/ws/a/dev", { name: "dev", branchLabel: "dev", pinned: false }),
      ],
      { name: "alpha" },
    ),
  ]);
}

function mountSettings() {
  const pinia = createPinia();
  setActivePinia(pinia);
  const workspace = useWorkspaceStore();
  workspace.hydrate(state());
  return { wrapper: mount(SettingsView, { global: { plugins: [pinia] } }), workspace };
}

describe("SettingsView", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(commands.setWorktreePinned).mockReturnValue(ok(state()));
    vi.mocked(commands.removeProject).mockReturnValue(ok(snapshot([])));
    vi.mocked(commands.renameWorktree).mockReturnValue(ok(state()));
  });

  it("lists every section in the navigation and opens Projects", () => {
    const { wrapper } = mountSettings();

    expect(wrapper.findAll(".nav__item")).toHaveLength(5);
    expect(wrapper.find(".nav__item--active").text()).toBe(
      localeString("zh-CN", "settings", ["nav", "project"]),
    );
  });

  it("lists all worktrees, including the ones outside the sidebar", () => {
    const { wrapper } = mountSettings();

    const names = wrapper.findAll(".wt__name").map((node) => node.text());
    expect(names).toEqual(["a", "dev"]);
  });

  it("adds a worktree to the sidebar through the switch", async () => {
    const { wrapper } = mountSettings();

    const toggles = wrapper.findAll("[role='switch']");
    expect(toggles).toHaveLength(2);
    await toggles[1].trigger("click");

    expect(commands.setWorktreePinned).toHaveBeenCalledWith("/ws/a", "/ws/a/dev", true);
  });

  it("removes a project", async () => {
    const { wrapper } = mountSettings();

    await wrapper.find(".button--danger").trigger("click");

    expect(commands.removeProject).toHaveBeenCalledWith("/ws/a");
  });

  it("renames a worktree display name", async () => {
    const { wrapper } = mountSettings();

    await wrapper.findAll(".wt__rename")[1].trigger("click");
    const input = wrapper.find(".wt__input");
    await input.setValue("my-dev");
    await input.trigger("keyup.enter");

    expect(commands.renameWorktree).toHaveBeenCalledWith("/ws/a", "/ws/a/dev", "my-dev");
  });

  it("clears the override when the new name is blank", async () => {
    const { wrapper } = mountSettings();

    await wrapper.findAll(".wt__rename")[1].trigger("click");
    const input = wrapper.find(".wt__input");
    await input.setValue("   ");
    await input.trigger("keyup.enter");

    expect(commands.renameWorktree).toHaveBeenCalledWith("/ws/a", "/ws/a/dev", null);
  });

  it("cancels the rename on Escape instead of committing the draft", async () => {
    const { wrapper } = mountSettings();

    await wrapper.findAll(".wt__rename")[1].trigger("click");
    const input = wrapper.find(".wt__input");
    await input.setValue("nope");
    await input.trigger("keyup.esc");

    expect(commands.renameWorktree).not.toHaveBeenCalled();
    expect(wrapper.find(".wt__input").exists()).toBe(false);

    // Esc 之后即使浏览器仍派发 blur，也不该把草稿提交上去。
    await input.trigger("blur");
    expect(commands.renameWorktree).not.toHaveBeenCalled();
  });

  it("shows a placeholder for the sections still owned by #86", async () => {
    const { wrapper } = mountSettings();

    await wrapper.findAll(".nav__item")[0].trigger("click");

    expect(wrapper.find(".page__empty").text()).toBe(
      localeString("zh-CN", "settings", ["comingSoon"]),
    );
  });
});

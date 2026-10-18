import { mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { fail, ok, project, snapshot, worktree } from "../../../__tests__/workspace-fixture";
import { commands } from "../../../bindings";
import { useTabsStore } from "../../../stores/tabs";
import { useWorkspaceStore } from "../../../stores/workspace";
import ProjectTree from "../ProjectTree.vue";

vi.mock(
  "../../../bindings",
  async () => (await import("../../../__tests__/workspace-fixture")).createBindingsMock(),
);

/** 两个项目：alpha 有 3 个 worktree（1 个未加入侧栏），beta 一个都没加入。 */
function groups() {
  return [
    project(
      "/ws/a",
      [
        worktree("/ws/a", { name: "a", isMain: true, branchLabel: "main" }),
        worktree("/ws/a/feat", { name: "feat", branchLabel: "feat/x" }),
        worktree("/ws/a/dev", { name: "dev", branchLabel: "dev", pinned: false }),
      ],
      { name: "alpha" },
    ),
    project("/ws/b", [worktree("/ws/b", { name: "b", pinned: false })], { name: "beta" }),
  ];
}

function mountTree(state = snapshot(groups())) {
  const pinia = createPinia();
  setActivePinia(pinia);
  const workspace = useWorkspaceStore();
  workspace.hydrate(state);
  const tabs = useTabsStore();
  return { wrapper: mount(ProjectTree, { global: { plugins: [pinia] } }), workspace, tabs };
}

describe("ProjectTree", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("shows the empty state before any project is added", () => {
    const { wrapper } = mountTree(snapshot([]));

    expect(wrapper.find(".tree__hint").exists()).toBe(true);
    expect(wrapper.findAll(".row")).toHaveLength(0);
  });

  it("renders only the worktrees that joined the sidebar", () => {
    const { wrapper } = mountTree();

    const names = wrapper.findAll(".row__name").map((node) => node.text());
    expect(names).toEqual(["a", "feat"]);
  });

  it("drops a project whose worktrees all left the sidebar", () => {
    const { wrapper } = mountTree();

    const projectNames = wrapper.findAll(".group__name").map((node) => node.text());
    expect(projectNames).toEqual(["alpha"]);
  });

  it("marks the active worktree", () => {
    const { wrapper } = mountTree(snapshot(groups(), "/ws/a/feat"));

    const active = wrapper.findAll(".row--active");
    expect(active).toHaveLength(1);
    expect(active[0].text()).toContain("feat");
  });

  it("activates a worktree on click", async () => {
    const { wrapper } = mountTree();
    vi.mocked(commands.setActiveWorktree).mockReturnValue(ok(snapshot(groups(), "/ws/a/feat")));

    await wrapper.findAll(".row")[1].trigger("click");

    expect(commands.setActiveWorktree).toHaveBeenCalledWith("/ws/a/feat");
  });

  it("opens the worktree agent menu with the four configured agents and no dsh", async () => {
    const { wrapper, tabs } = mountTree();

    await wrapper.findAll(".row__agent")[0].trigger("click");

    const entries = wrapper.findAll("[data-agent-id]");
    expect(entries.map((entry) => entry.attributes("data-agent-id"))).toEqual([
      "claude",
      "opencode",
      "codex",
      "pi",
    ]);
    expect(wrapper.text()).not.toContain("dsh");

    await entries[0].trigger("click");

    expect(tabs.tabsFor("/ws/a")[0]).toMatchObject({
      kind: "terminal",
      name: "Claude Code",
      agentId: "claude",
      agentColor: "#d97757",
    });
  });

  it("collapses a group through the header", async () => {
    const { wrapper } = mountTree();
    vi.mocked(commands.setProjectCollapsed).mockReturnValue(ok(snapshot(groups())));

    await wrapper.find(".group__head").trigger("click");

    expect(commands.setProjectCollapsed).toHaveBeenCalledWith("/ws/a", true);
  });

  it("hides the rows of a collapsed group", () => {
    const collapsed = groups().map((group) => ({ ...group, collapsed: true }));
    const { wrapper } = mountTree(snapshot(collapsed));

    expect(wrapper.findAll(".group__rows")).toHaveLength(0);
  });

  it("marks bare, locked and stale worktrees", () => {
    const { wrapper } = mountTree(
      snapshot([
        project("/ws/bare", [
          worktree("/ws/bare", {
            name: "bare",
            bare: true,
            locked: true,
            stale: true,
            branchLabel: null,
          }),
        ]),
      ]),
    );

    expect(wrapper.find(".row__tag").exists()).toBe(true);
    expect(wrapper.find(".row__mark").exists()).toBe(true);
    expect(wrapper.find(".row__tag--stale").exists()).toBe(true);
  });

  it("surfaces a failed command instead of swallowing it", async () => {
    const { wrapper, workspace } = mountTree();
    vi.mocked(commands.removeProject).mockReturnValue(fail("lock poisoned"));

    await workspace.removeProject("/ws/a");
    await wrapper.vm.$nextTick();

    expect(wrapper.find(".tree__error").text()).toContain("lock poisoned");
  });
});

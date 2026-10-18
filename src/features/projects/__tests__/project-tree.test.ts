import { mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { fail, ok, project, snapshot, worktree } from "../../../__tests__/workspace-fixture";
import type { AgentStatusView } from "../../../bindings";
import { commands } from "../../../bindings";
import { localeString } from "../../../i18n/__tests__/locale-files";
import { useAgentStatusStore } from "../../../stores/agentStatus";
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
    expect(wrapper.findAll(".user-workspace .row")).toHaveLength(1);
    expect(wrapper.findAll(".group")).toHaveLength(0);
  });

  it("renders only the worktrees that joined the sidebar", () => {
    const { wrapper } = mountTree();

    const names = wrapper.findAll(".row__name").map((node) => node.text());
    expect(names).toEqual(["~", "a", "feat"]);
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

    await wrapper.findAll(".group .row")[1].trigger("click");

    expect(commands.setActiveWorktree).toHaveBeenCalledWith("/ws/a/feat");
  });

  it("opens the worktree agent menu with the four configured agents and no dsh", async () => {
    const { wrapper, tabs } = mountTree();

    await wrapper.findAll(".group .row__agent")[0].trigger("click");

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

  it("shows the pinned non-git user workspace row", () => {
    const { wrapper } = mountTree();

    const row = wrapper.get(".user-workspace");
    expect(row.text()).toContain("~");
    expect(row.find(".row__branch").exists()).toBe(false);
    expect(row.find(".row__home-icon").exists()).toBe(true);
  });

  it("shows the aggregate agent status and marks degraded snapshots", async () => {
    const { wrapper, tabs } = mountTree();
    const sid = "terminal:/ws/a:1";
    tabs.openTerminal("/ws/a", sid, "/ws/a", "Claude Code", {
      agentId: "claude",
      agentColor: "#d97757",
    });
    const statuses = useAgentStatusStore();
    const status: AgentStatusView = {
      sid,
      agent: "claude",
      incarnationId: "inc-1",
      state: "waiting",
      source: "heuristic",
      degraded: true,
      timestampMs: 1,
      seq: 0,
      exitCode: null,
      reason: null,
    };
    statuses.apply(status);
    await wrapper.vm.$nextTick();

    const dot = wrapper.get(".row__status");
    expect(dot.classes()).toContain("row__status--attention");
    expect(dot.classes()).toContain("row__status--degraded");
    expect(dot.attributes("aria-label")).toContain(
      localeString("zh-CN", "agents", ["status", "degraded"]),
    );
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

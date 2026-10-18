import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { fail, ok, project, snapshot, worktree } from "../../__tests__/workspace-fixture";
import { commands } from "../../bindings";
import { useWorkspaceStore } from "../workspace";

vi.mock(
  "../../bindings",
  async () => (await import("../../__tests__/workspace-fixture")).createBindingsMock(),
);

describe("useWorkspaceStore", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.clearAllMocks();
  });

  it("hydrates projects and the active worktree from a snapshot", () => {
    const store = useWorkspaceStore();
    store.hydrate(
      snapshot([project("/ws/a", [worktree("/ws/a", { isMain: true, branchLabel: "main" })])], "/ws/a"),
    );

    expect(store.projects).toHaveLength(1);
    expect(store.activeWorktree).toEqual({ name: "a", branch: "main" });
  });

  it("hides worktrees that are not in the sidebar and drops empty groups", () => {
    const store = useWorkspaceStore();
    store.hydrate(
      snapshot([
        project("/ws/a", [worktree("/ws/a/one"), worktree("/ws/a/two", { pinned: false })]),
        project("/ws/b", [worktree("/ws/b/one", { pinned: false })]),
      ]),
    );

    expect(store.sidebarProjects).toHaveLength(1);
    expect(store.sidebarProjects[0].worktrees.map((item) => item.path)).toEqual(["/ws/a/one"]);
  });

  it("reports no active worktree when the path is unknown", () => {
    const store = useWorkspaceStore();
    store.hydrate(snapshot([], "/ws/gone"));

    expect(store.activeWorktree).toBeNull();
  });

  it("keeps the branch null when the worktree has none", () => {
    const store = useWorkspaceStore();
    store.hydrate(
      snapshot([project("/ws/bare", [worktree("/ws/bare", { bare: true })])], "/ws/bare"),
    );

    expect(store.activeWorktree).toEqual({ name: "bare", branch: null });
  });

  it("follows the renamed display name of the active worktree", () => {
    const store = useWorkspaceStore();
    store.hydrate(
      snapshot([
        project("/ws/a", [worktree("/ws/a/feat", { name: "my-feature", branchLabel: "feat" })]),
      ], "/ws/a/feat"),
    );

    expect(store.activeWorktree).toEqual({ name: "my-feature", branch: "feat" });
  });

  it("treats the user workspace as a pinned branchless active node", () => {
    const store = useWorkspaceStore();
    const state = snapshot([], "/home/user");
    state.userWorkspace = { path: "/home/user", name: "scratch" };

    store.hydrate(state);

    expect(store.userWorkspace.name).toBe("scratch");
    expect(store.activeWorktree).toEqual({ name: "scratch", branch: null });
    expect(store.byPath.get("/home/user")?.pinned).toBe(true);
  });

  it("replaces the projection from a command result and clears lastError", async () => {
    const store = useWorkspaceStore();
    vi.mocked(commands.setWorktreePinned).mockReturnValue(
      ok(snapshot([project("/ws/a", [worktree("/ws/a")])])),
    );

    const done = await store.setWorktreePinned("/ws/a", "/ws/a", true);

    expect(done).toBe(true);
    expect(commands.setWorktreePinned).toHaveBeenCalledWith("/ws/a", "/ws/a", true);
    expect(store.projects).toHaveLength(1);
    expect(store.lastError).toBeNull();
  });

  it("records the Rust message when a command fails", async () => {
    const store = useWorkspaceStore();
    vi.mocked(commands.removeProject).mockReturnValue(fail("lock poisoned"));

    const done = await store.removeProject("/ws/a");

    expect(done).toBe(false);
    expect(store.lastError).toBe("lock poisoned");
  });

  it("treats a cancelled add as a no-op, leaving the projection alone", async () => {
    const store = useWorkspaceStore();
    store.hydrate(snapshot([project("/ws/a", [worktree("/ws/a")])]));
    vi.mocked(commands.addProject).mockReturnValue(ok(null));

    expect(await store.addProject()).toBe(false);
    expect(store.projects).toHaveLength(1);
    expect(store.lastError).toBeNull();
  });

  it("toggles a group through the collapse command", async () => {
    const store = useWorkspaceStore();
    const group = project("/ws/a", [worktree("/ws/a")]);
    vi.mocked(commands.setProjectCollapsed).mockReturnValue(ok(snapshot([group])));

    await store.toggleProject(group);

    expect(commands.setProjectCollapsed).toHaveBeenCalledWith("/ws/a", true);
  });

  it("compares the active worktree by path", () => {
    const store = useWorkspaceStore();
    store.hydrate(snapshot([], "/ws/a"));

    expect(store.isActive("/ws/a")).toBe(true);
    expect(store.isActive("/ws/b")).toBe(false);
  });
});

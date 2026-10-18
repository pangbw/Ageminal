import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  restoreTabs: vi.fn(),
  getAgents: vi.fn(),
}));

vi.mock("../../../bindings", () => ({
  commands: { restoreTabs: mocks.restoreTabs, getAgents: mocks.getAgents },
}));

import { localeString } from "../../../i18n/__tests__/locale-files";
import { useTabsStore } from "../../../stores/tabs";
import { restoreWorkspaceTabs } from "../restore";

const RESTORED = [
  {
    root: "/ws/a",
    active: "terminal:/ws/a:1",
    tabs: [
      {
        kind: "terminal",
        name: "Claude Code",
        sid: "terminal:/ws/a:1",
        cwd: "/ws/a",
        agentId: "claude",
        action: "attach",
      },
    ],
  },
  {
    root: "/ws/b",
    active: "/ws/b/README.md",
    tabs: [
      {
        kind: "terminal",
        name: "",
        sid: "terminal:/ws/b:2",
        cwd: "/ws/b",
        agentId: null,
        action: "adopt",
      },
      {
        kind: "preview",
        name: "README.md",
        sid: null,
        path: "/ws/b/README.md",
        cwd: null,
        agentId: null,
        action: "preview",
      },
    ],
  },
];

describe("restore workspace tabs", () => {
  beforeEach(() => {
    mocks.restoreTabs.mockReset();
    mocks.getAgents.mockReset().mockResolvedValue({
      status: "ok",
      data: [
        { id: "claude", name: "Claude Code", command: "claude", args: "", color: "#d97757" },
        { id: "opencode", name: "opencode", command: "opencode", args: "", color: "#f5a742" },
        { id: "codex", name: "Codex", command: "codex", args: "", color: "#10a37f" },
        { id: "pi", name: "pi", command: "pi", args: "", color: "#8b5cf6" },
      ],
    });
    setActivePinia(createPinia());
  });

  it("hydrates terminal, preview and active tabs from the reconciliation", async () => {
    mocks.restoreTabs.mockResolvedValue({ status: "ok", data: RESTORED });
    const tabs = useTabsStore();

    await expect(restoreWorkspaceTabs()).resolves.toBe(3);

    expect(tabs.tabsFor("/ws/a")).toMatchObject([
      {
        kind: "terminal",
        sid: "terminal:/ws/a:1",
        cwd: "/ws/a",
        name: "Claude Code",
        agentId: "claude",
        agentColor: "#d97757",
      },
    ]);
    expect(tabs.tabsFor("/ws/b")[0]).toMatchObject({
      kind: "terminal",
      name: localeString("zh-CN", "terminal", ["defaultName"]),
      agentId: undefined,
    });
    expect(tabs.tabsFor("/ws/b")[1]).toMatchObject({
      kind: "preview",
      path: "/ws/b/README.md",
      name: "README.md",
    });
    expect(tabs.activeTab("/ws/a")?.kind).toBe("terminal");
    expect(tabs.activeTab("/ws/b")?.kind).toBe("preview");
  });

  it("replaces the projection instead of duplicating tabs", async () => {
    mocks.restoreTabs.mockResolvedValue({ status: "ok", data: RESTORED });
    const tabs = useTabsStore();

    await restoreWorkspaceTabs();
    await restoreWorkspaceTabs();

    expect(tabs.tabsFor("/ws/a")).toHaveLength(1);
    expect(tabs.tabsFor("/ws/b")).toHaveLength(2);
  });

  it("propagates a daemon error so the caller can log it", async () => {
    mocks.restoreTabs.mockResolvedValue({ status: "error", error: "daemon down" });
    const tabs = useTabsStore();

    await expect(restoreWorkspaceTabs()).rejects.toThrow("daemon down");
    expect(tabs.tabsFor("/ws/a")).toHaveLength(0);
  });
});

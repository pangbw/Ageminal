import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  terminalList: vi.fn(),
}));

vi.mock("../../../bindings", () => ({
  commands: { terminalList: mocks.terminalList },
}));

import { useTabsStore } from "../../../stores/tabs";
import { restoreTerminalTabs } from "../restore";

const SESSIONS = [
  {
    sid: "terminal:/ws/a:1",
    cwd: "/ws/a",
    pid: 100,
    incarnationId: "inc-1",
  },
  {
    sid: "terminal:/ws/b:2",
    cwd: "/ws/b",
    pid: 200,
    incarnationId: "inc-2",
  },
];

describe("restore terminal tabs", () => {
  beforeEach(() => {
    mocks.terminalList.mockReset();
    setActivePinia(createPinia());
  });

  it("rebuilds a tab for every live daemon session", async () => {
    mocks.terminalList.mockResolvedValue({ status: "ok", data: SESSIONS });
    const tabs = useTabsStore();

    await expect(restoreTerminalTabs()).resolves.toBe(2);

    expect(tabs.tabsFor("/ws/a")).toMatchObject([
      { kind: "terminal", sid: SESSIONS[0]!.sid, cwd: "/ws/a" },
    ]);
    // 后恢复的会话成为该 worktree 的激活项。
    expect(tabs.activeTerminal("/ws/b")?.sid).toBe(SESSIONS[1]!.sid);
  });

  it("does not duplicate tabs when restore runs again", async () => {
    mocks.terminalList.mockResolvedValue({ status: "ok", data: SESSIONS });
    const tabs = useTabsStore();

    await restoreTerminalTabs();
    await restoreTerminalTabs();

    expect(tabs.tabsFor("/ws/a")).toHaveLength(1);
    expect(tabs.tabsFor("/ws/b")).toHaveLength(1);
  });

  it("propagates a daemon error so the caller can log it", async () => {
    mocks.terminalList.mockResolvedValue({ status: "error", error: "daemon down" });
    const tabs = useTabsStore();

    await expect(restoreTerminalTabs()).rejects.toThrow("daemon down");
    expect(tabs.tabsFor("/ws/a")).toHaveLength(0);
  });
});

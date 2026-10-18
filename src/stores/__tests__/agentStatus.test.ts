import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { AgentStatusState, AgentStatusView } from "../../bindings";
import { useAgentStatusStore } from "../agentStatus";

const mocks = vi.hoisted(() => ({
  agentStatuses: vi.fn(),
  listen: vi.fn(),
  handler: null as ((event: { payload: unknown }) => void) | null,
  unlisten: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));
vi.mock("../../bindings", () => ({ commands: { agentStatuses: mocks.agentStatuses } }));

function status(state: AgentStatusState, timestampMs: number, sid = "s") : AgentStatusView {
  return {
    sid,
    agent: "claude",
    incarnationId: "inc-1",
    state,
    source: "native",
    degraded: false,
    timestampMs,
    seq: 0,
    exitCode: null,
    reason: null,
  };
}

describe("useAgentStatusStore", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.clearAllMocks();
    mocks.listen.mockImplementation(async (_name: string, handler: (event: { payload: unknown }) => void) => {
      mocks.handler = handler;
      return mocks.unlisten;
    });
    mocks.agentStatuses.mockResolvedValue([]);
  });

  it("hydrates snapshots and binds the global event", async () => {
    mocks.agentStatuses.mockResolvedValue([status("running", 10)]);
    const store = useAgentStatusStore();

    await store.start();

    expect(store.statusFor("s")?.state).toBe("running");
    expect(mocks.listen).toHaveBeenCalledWith("agent-status", expect.any(Function));

    mocks.handler?.({
      payload: {
        previous: "running",
        current: status("done", 20),
        changed: true,
      },
    });
    expect(store.statusFor("s")?.state).toBe("done");
  });

  it("does not let an older hydrate snapshot overwrite a live event", async () => {
    const store = useAgentStatusStore();
    store.apply(status("done", 20));

    store.apply(status("running", 10));

    expect(store.statusFor("s")?.state).toBe("done");
  });

  it("removes a closed tab status and stops the listener", async () => {
    const store = useAgentStatusStore();
    await store.start();
    store.apply(status("done", 20));
    store.remove("s");
    store.stop();

    expect(store.statusFor("s")).toBeNull();
    expect(mocks.unlisten).toHaveBeenCalledOnce();
  });
});

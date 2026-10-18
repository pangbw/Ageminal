import { describe, expect, it } from "vitest";

import type { AgentStatusState, AgentStatusView } from "../../../bindings";
import { aggregateStatus, statusLabelKey, statusTone } from "../status";

function status(state: AgentStatusState, degraded = false): AgentStatusView {
  return {
    sid: `sid-${state}`,
    agent: "claude",
    incarnationId: "inc-1",
    state,
    source: degraded ? "heuristic" : "native",
    degraded,
    timestampMs: 1,
    seq: 0,
    exitCode: null,
    reason: null,
  };
}

describe("agent status projection", () => {
  it("maps every unified state to a tone", () => {
    expect(statusTone("running")).toBe("running");
    expect(statusTone("waiting")).toBe("attention");
    expect(statusTone("permission")).toBe("attention");
    expect(statusTone("done")).toBe("done");
    expect(statusTone("error")).toBe("error");
    expect(statusTone("idle")).toBe("idle");
    expect(statusTone("exited")).toBe("exited");
  });

  it("aggregates a worktree by attention priority", () => {
    const result = aggregateStatus([
      status("running"),
      status("done"),
      status("waiting"),
      status("permission"),
      status("error"),
    ]);

    expect(result?.state).toBe("permission");
    expect(aggregateStatus([status("running"), status("done")])?.state).toBe("done");
    expect(aggregateStatus([status("idle"), status("exited")])?.state).toBe("idle");
    expect(
      aggregateStatus([status("exited"), status("exited", true)])?.degraded,
    ).toBe(true);
    expect(aggregateStatus([null, undefined])).toBeNull();
  });

  it("keeps the i18n key in one place", () => {
    expect(statusLabelKey("permission")).toBe("agents.status.permission");
  });
});

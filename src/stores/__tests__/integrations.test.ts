import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { IntegrationView } from "../../bindings";
import { ok } from "../../__tests__/workspace-fixture";
import { useIntegrationsStore } from "../integrations";

const mocks = vi.hoisted(() => ({
  integrationList: vi.fn(),
  integrationPreview: vi.fn(),
  integrationApply: vi.fn(),
  integrationDisable: vi.fn(),
  integrationTestDelivery: vi.fn(),
}));

vi.mock("../../bindings", () => ({
  commands: {
    integrationList: mocks.integrationList,
    integrationPreview: mocks.integrationPreview,
    integrationApply: mocks.integrationApply,
    integrationDisable: mocks.integrationDisable,
    integrationTestDelivery: mocks.integrationTestDelivery,
  },
}));

function view(status: IntegrationView["status"]): IntegrationView {
  return {
    agentId: "claude",
    name: "Claude Code",
    status,
    applied: status === "enabled" ? 2 : 0,
    total: 2,
    shimReady: true,
    files: [],
    lastDeliveryMs: null,
    failureCount: 0,
    detail: null,
  };
}

describe("useIntegrationsStore", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.clearAllMocks();
  });

  it("hydrates the status list and refreshes after apply", async () => {
    mocks.integrationList
      .mockResolvedValueOnce(ok([view("notEnabled")]))
      .mockResolvedValueOnce(ok([view("enabled")]));
    mocks.integrationApply.mockResolvedValue(
      ok({ view: view("enabled"), backups: ["C:/b"], errors: [], restartRequired: true }),
    );
    const store = useIntegrationsStore();

    await store.refresh();
    expect(store.byId("claude")?.status).toBe("notEnabled");

    const result = await store.apply("claude");
    expect(result.restartRequired).toBe(true);
    expect(store.byId("claude")?.status).toBe("enabled");
  });

  it("exposes preview without mutating the status list", async () => {
    mocks.integrationPreview.mockResolvedValue(
      ok({
        agentId: "pi",
        name: "pi",
        supported: true,
        shimReady: true,
        files: [],
      }),
    );
    const store = useIntegrationsStore();

    const preview = await store.preview("pi");
    expect(preview.agentId).toBe("pi");
    expect(store.list).toEqual([]);
  });
});

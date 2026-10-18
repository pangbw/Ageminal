import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

const runtime = vi.hoisted(() => ({
  close: vi.fn(),
  terminalState: vi.fn(),
}));

vi.mock("../../terminal/runtime", () => ({
  close: runtime.close,
  terminalState: runtime.terminalState,
}));

import { usePreferencesStore } from "../../../stores/preferences";
import { useTabsStore } from "../../../stores/tabs";
import {
  cancelPendingClose,
  confirmPendingClose,
  pendingClose,
  requestClose,
  requestCloseOthers,
  requestCloseRight,
} from "../actions";

const ROOT = "/ws/a";
const SID = "terminal:/ws/a:1";

function setup() {
  setActivePinia(createPinia());
  const tabs = useTabsStore();
  tabs.openFile(ROOT, `${ROOT}/one.ts`, "one.ts");
  tabs.openTerminal(ROOT, SID, ROOT, "Terminal");
  tabs.openFile(ROOT, `${ROOT}/two.ts`, "two.ts");
  return tabs;
}

describe("tab actions", () => {
  beforeEach(() => {
    runtime.close.mockReset().mockResolvedValue(undefined);
    runtime.terminalState.mockReset().mockReturnValue(undefined);
    pendingClose.value = null;
  });

  it("asks before closing a running terminal and closes on confirm", () => {
    const tabs = setup();
    const terminal = tabs.activeTerminal(ROOT) ?? tabs.tabsFor(ROOT)[1];
    if (terminal.kind !== "terminal") throw new Error("fixture");

    requestClose(ROOT, [terminal]);

    expect(pendingClose.value).not.toBeNull();
    expect(tabs.tabsFor(ROOT).map((tab) => tab.name)).toContain("Terminal");

    confirmPendingClose();

    expect(pendingClose.value).toBeNull();
    expect(tabs.tabsFor(ROOT).map((tab) => tab.name)).not.toContain("Terminal");
    expect(runtime.close).toHaveBeenCalledWith(SID);
  });

  it("closes immediately when the setting is off", () => {
    const tabs = setup();
    usePreferencesStore().confirmClose = false;
    const terminal = tabs.openTerminal(ROOT, SID, ROOT, "Terminal");

    tabs.setActive(ROOT, SID);
    requestClose(ROOT, [terminal]);

    expect(pendingClose.value).toBeNull();
    expect(tabs.tabsFor(ROOT).map((tab) => tab.name)).not.toContain("Terminal");
    expect(runtime.close).toHaveBeenCalledWith(SID);
  });

  it("does not ask for previews or already-exited terminals", () => {
    const tabs = setup();
    runtime.terminalState.mockReturnValue({ status: "exited", visible: true });
    const terminal = tabs.openTerminal(ROOT, SID, ROOT, "Terminal");
    const preview = tabs.tabsFor(ROOT)[0];
    if (preview.kind !== "preview") throw new Error("fixture");

    requestClose(ROOT, [preview, terminal]);

    expect(pendingClose.value).toBeNull();
    expect(tabs.tabsFor(ROOT)).toHaveLength(1);
  });

  it("cancels without closing anything", () => {
    const tabs = setup();
    const terminal = tabs.openTerminal(ROOT, SID, ROOT, "Terminal");

    requestClose(ROOT, [terminal]);
    cancelPendingClose();

    expect(pendingClose.value).toBeNull();
    expect(tabs.tabsFor(ROOT).map((tab) => tab.name)).toContain("Terminal");
    expect(runtime.close).not.toHaveBeenCalled();
  });

  it("closes others and closes to the right", () => {
    const tabs = setup();
    usePreferencesStore().confirmClose = false;
    const first = tabs.tabsFor(ROOT)[0];

    requestCloseOthers(ROOT, first);
    expect(tabs.tabsFor(ROOT).map((tab) => tab.name)).toEqual(["one.ts"]);

    tabs.openTerminal(ROOT, SID, ROOT, "Terminal");
    tabs.openFile(ROOT, `${ROOT}/two.ts`, "two.ts");
    const terminal = tabs.tabsFor(ROOT)[1];
    requestCloseRight(ROOT, terminal);

    expect(tabs.tabsFor(ROOT).map((tab) => tab.name)).toEqual(["one.ts", "Terminal"]);
  });
});

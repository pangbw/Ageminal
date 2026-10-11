import { mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { defineComponent, nextTick } from "vue";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  setWindowTitle: vi.fn<(title: string) => Promise<void>>(),
}));

vi.mock("../window", () => ({ setWindowTitle: mocks.setWindowTitle }));

import { applyLocale } from "../../../i18n";
import { localeString } from "../../../i18n/__tests__/locale-files";
import { project, snapshot, worktree } from "../../../__tests__/workspace-fixtures";
import { useWorkspaceStore } from "../../../stores/workspace";
import { formatWindowTitle, useWindowTitle } from "../useWindowTitle";
import { expectedWindowTitle, expectedWindowTitleNoBranch } from "./window-title-fixture";

const Probe = defineComponent({
  setup() {
    useWindowTitle();
    return () => null;
  },
});

describe("formatWindowTitle", () => {
  it("falls back to the bare app name when no worktree is active", () => {
    applyLocale("zh-CN");
    expect(formatWindowTitle(null)).toBe(localeString("zh-CN", "common", ["appName"]));
  });

  it("renders app, worktree and branch", () => {
    applyLocale("zh-CN");
    expect(formatWindowTitle({ name: "feat-agent-ui", branch: "feat/agent-ui" })).toBe(
      expectedWindowTitle("zh-CN", "feat-agent-ui", "feat/agent-ui"),
    );
  });

  it("follows the active locale", () => {
    applyLocale("en-US");
    expect(formatWindowTitle({ name: "wt", branch: "main" })).toBe(
      expectedWindowTitle("en-US", "wt", "main"),
    );
  });

  it("drops the branch part when the worktree has no branch", () => {
    applyLocale("zh-CN");
    expect(formatWindowTitle({ name: "bare", branch: null })).toBe(
      expectedWindowTitleNoBranch("zh-CN", "bare"),
    );
  });
});

describe("useWindowTitle", () => {
  beforeEach(() => {
    mocks.setWindowTitle.mockReset().mockResolvedValue(undefined);
    applyLocale("en-US");
  });

  it("pushes the bare app name before any worktree is active", () => {
    const pinia = createPinia();
    setActivePinia(pinia);

    mount(Probe, { global: { plugins: [pinia] } });

    expect(mocks.setWindowTitle).toHaveBeenCalledWith(localeString("en-US", "common", ["appName"]));
  });

  it("re-pushes the title when the active worktree changes", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    mount(Probe, { global: { plugins: [pinia] } });

    useWorkspaceStore().hydrate(
      snapshot(
        [project("/ws/feat-x", [worktree("/ws/feat-x", { name: "feat-x", branchLabel: "main" })])],
        "/ws/feat-x",
      ),
    );
    await nextTick();

    expect(mocks.setWindowTitle).toHaveBeenLastCalledWith(expectedWindowTitle("en-US", "feat-x", "main"));
  });

  it("omits the branch for a worktree that has none", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    mount(Probe, { global: { plugins: [pinia] } });

    useWorkspaceStore().hydrate(
      snapshot([project("/ws/bare", [worktree("/ws/bare", { name: "bare", bare: true })])], "/ws/bare"),
    );
    await nextTick();

    expect(mocks.setWindowTitle).toHaveBeenLastCalledWith(
      expectedWindowTitleNoBranch("en-US", "bare"),
    );
  });
});

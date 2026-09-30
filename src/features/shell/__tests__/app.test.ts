import { mount, type VueWrapper } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { nextTick } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  setWindowTitle: vi.fn<(title: string) => Promise<void>>(),
  minimizeWindow: vi.fn<() => Promise<void>>(),
  toggleMaximizeWindow: vi.fn<() => Promise<void>>(),
  closeWindow: vi.fn<() => Promise<void>>(),
}));

vi.mock("../window", () => ({
  setWindowTitle: mocks.setWindowTitle,
  minimizeWindow: mocks.minimizeWindow,
  toggleMaximizeWindow: mocks.toggleMaximizeWindow,
  closeWindow: mocks.closeWindow,
}));

import App from "../../../App.vue";
import { applyLocale } from "../../../i18n";
import { localeString } from "../../../i18n/__tests__/locale-files";
import { DEFAULT_PANEL_WIDTH } from "../../../lib/panel-layout";
import { useUiStore } from "../../../stores/ui";
import { useWorkspaceStore } from "../../../stores/workspace";
import { expectedWindowTitle } from "./window-title-fixture";

function zh(namespace: "common" | "shell" | "tabs" | "files", path: string[]): string {
  return localeString("zh-CN", namespace, path);
}

let wrapper: VueWrapper | null = null;

function mountShell(): { ui: ReturnType<typeof useUiStore>; workspace: ReturnType<typeof useWorkspaceStore> } {
  const pinia = createPinia();
  setActivePinia(pinia);
  wrapper = mount(App, { global: { plugins: [pinia] } });

  return { ui: useUiStore(), workspace: useWorkspaceStore() };
}

describe("app shell", () => {
  beforeEach(() => {
    mocks.setWindowTitle.mockReset().mockResolvedValue(undefined);
    applyLocale("zh-CN");
  });

  afterEach(() => {
    wrapper?.unmount();
    wrapper = null;
    document.body.classList.remove("is-resizing");
  });

  it("lays out the project panel, tab strip and file panel around the main area", () => {
    mountShell();

    const shell = wrapper!;
    expect(shell.get(".pane--left .app__brand").text()).toBe(zh("common", ["appName"]));
    expect(shell.get(".pane--right .pane__head").text()).toBe(zh("files", ["panel", "title"]));
    expect(shell.get(".tabbar__new").attributes("title")).toBe(zh("tabs", ["new"]));
    expect(shell.get(".titlebar").attributes("data-tauri-drag-region")).toBe("true");
    // 拖拽区逐元素标注（标题栏 + 页签条），见 TitleBar 的说明。
    expect(shell.get(".tabbar").attributes("data-tauri-drag-region")).toBe("true");
    expect(shell.get(".tabbar__strip").attributes("data-tauri-drag-region")).toBe("true");
    expect(shell.get(".resizer--left").attributes("aria-label")).toBe(zh("shell", ["panelResizer"]));
  });

  it("shows the empty-tab state while no tab is open", () => {
    mountShell();

    expect(wrapper!.get(".app__main").text()).toBe(zh("tabs", ["empty"]));
  });

  it("collapses a panel into a rail and expands it back", async () => {
    const { ui } = mountShell();
    const shell = wrapper!;

    await shell.get(".pane--left .pane__collapse").trigger("click");
    expect(shell.find(".pane--left").exists()).toBe(false);
    expect(shell.get(".rail--left .rail__label").text()).toBe(zh("shell", ["projectPanel", "railLabel"]));
    expect(ui.leftCollapsed).toBe(true);

    await shell.get(".rail--left").trigger("click");
    expect(shell.find(".pane--left").exists()).toBe(true);
    expect(ui.leftCollapsed).toBe(false);
  });

  it("keeps the two panels independent when collapsing", async () => {
    const { ui } = mountShell();
    const shell = wrapper!;

    await shell.get(".pane--right .pane__collapse").trigger("click");

    expect(shell.find(".pane--right").exists()).toBe(false);
    expect(shell.find(".rail--right").exists()).toBe(true);
    expect(shell.find(".pane--left").exists()).toBe(true);
    expect([ui.leftCollapsed, ui.rightCollapsed]).toEqual([false, true]);
  });

  it("resizes the left panel by dragging the divider", async () => {
    const { ui } = mountShell();
    const shell = wrapper!;

    await shell.get(".resizer--left").trigger("mousedown", { clientX: 100 });
    expect(document.body.classList.contains("is-resizing")).toBe(true);

    window.dispatchEvent(new MouseEvent("mousemove", { clientX: 160 }));
    await nextTick();
    expect(ui.leftWidth).toBe(DEFAULT_PANEL_WIDTH.left + 60);

    window.dispatchEvent(new MouseEvent("mouseup"));
    await nextTick();
    expect(document.body.classList.contains("is-resizing")).toBe(false);
    expect(shell.get(".pane--left").attributes("style")).toContain(`${DEFAULT_PANEL_WIDTH.left + 60}px`);
  });

  it("drives the window title from the active worktree", async () => {
    const { workspace } = mountShell();

    expect(mocks.setWindowTitle).toHaveBeenLastCalledWith(zh("common", ["appName"]));

    workspace.activeWorktree = { name: "feat-x", branch: "main" };
    await nextTick();

    expect(mocks.setWindowTitle).toHaveBeenLastCalledWith(
      expectedWindowTitle("zh-CN", "feat-x", "main"),
    );
  });

  it("routes the title-bar buttons to the window commands", async () => {
    mountShell();
    const buttons = wrapper!.findAll(".window-controls__button");

    await buttons[0]!.trigger("click");
    await buttons[1]!.trigger("click");
    await buttons[2]!.trigger("click");

    expect(mocks.minimizeWindow).toHaveBeenCalledOnce();
    expect(mocks.toggleMaximizeWindow).toHaveBeenCalledOnce();
    expect(mocks.closeWindow).toHaveBeenCalledOnce();
  });

  it("opens the overlay entries kept for later tickets", async () => {
    const { ui } = mountShell();
    const shell = wrapper!;

    await shell.get(".tabbar__new").trigger("click");
    await shell.get(".pane--left .app__add-project").trigger("click");
    await shell.get(".titlebar__button").trigger("click");

    expect([ui.newTabMenuOpen, ui.addProjectOpen, ui.settingsOpen]).toEqual([true, true, true]);
  });
});

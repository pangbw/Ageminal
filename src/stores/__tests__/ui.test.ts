import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it } from "vitest";

import { DEFAULT_PANEL_WIDTH, PANEL_MAX_WIDTH, PANEL_MIN_WIDTH } from "../../lib/panel-layout";
import { useUiStore } from "../ui";

describe("useUiStore", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it("starts with the prototype widths and both panels expanded", () => {
    const ui = useUiStore();

    expect(ui.leftWidth).toBe(DEFAULT_PANEL_WIDTH.left);
    expect(ui.rightWidth).toBe(DEFAULT_PANEL_WIDTH.right);
    expect(ui.leftCollapsed).toBe(false);
    expect(ui.rightCollapsed).toBe(false);
  });

  it("clamps any width it is asked to store", () => {
    const ui = useUiStore();

    ui.setPanelWidth("left", PANEL_MAX_WIDTH + 500);
    expect(ui.leftWidth).toBe(PANEL_MAX_WIDTH);

    ui.setPanelWidth("right", PANEL_MIN_WIDTH - 500);
    expect(ui.rightWidth).toBe(PANEL_MIN_WIDTH);
  });

  it("keeps the two panels independent", () => {
    const ui = useUiStore();

    ui.setPanelWidth("left", 300);
    ui.collapsePanel("right");

    expect(ui.leftWidth).toBe(300);
    expect(ui.rightWidth).toBe(DEFAULT_PANEL_WIDTH.right);
    expect(ui.leftCollapsed).toBe(false);
    expect(ui.rightCollapsed).toBe(true);

    ui.expandPanel("right");
    expect(ui.rightCollapsed).toBe(false);
  });

  it("reports the width of each side", () => {
    const ui = useUiStore();

    ui.setPanelWidth("left", 300);

    expect(ui.panelWidth("left")).toBe(300);
    expect(ui.panelWidth("right")).toBe(ui.rightWidth);
  });

  it("sets collapse state per side", () => {
    const ui = useUiStore();

    ui.setPanelCollapsed("left", true);

    expect([ui.leftCollapsed, ui.rightCollapsed]).toEqual([true, false]);
  });

  it("toggles the overlay entries the shell exposes", () => {
    const ui = useUiStore();

    expect(ui.newTabMenuOpen).toBe(false);
    expect(ui.addProjectOpen).toBe(false);
    expect(ui.settingsOpen).toBe(false);

    ui.toggleNewTabMenu();
    ui.toggleAddProject();
    ui.toggleSettings();
    expect([ui.newTabMenuOpen, ui.addProjectOpen, ui.settingsOpen]).toEqual([true, true, true]);

    ui.toggleSettings();
    expect(ui.settingsOpen).toBe(false);
  });
});

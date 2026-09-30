import { describe, expect, it } from "vitest";

import {
  clampPanelWidth,
  PANEL_MAX_WIDTH,
  PANEL_MIN_WIDTH,
  PANEL_RESIZE_STEP,
  panelWidthAfterDrag,
  panelWidthAfterStep,
} from "../panel-layout";

describe("clampPanelWidth", () => {
  it("keeps widths inside the drag range", () => {
    expect(clampPanelWidth(252)).toBe(252);
    expect(clampPanelWidth(PANEL_MIN_WIDTH - 1)).toBe(PANEL_MIN_WIDTH);
    expect(clampPanelWidth(PANEL_MAX_WIDTH + 1)).toBe(PANEL_MAX_WIDTH);
  });

  it("rounds fractional widths so panels never land on half a pixel", () => {
    expect(clampPanelWidth(252.4)).toBe(252);
    expect(clampPanelWidth(252.6)).toBe(253);
  });
});

describe("panelWidthAfterDrag", () => {
  it("grows the left panel when the pointer moves right", () => {
    expect(panelWidthAfterDrag("left", 252, 40)).toBe(292);
    expect(panelWidthAfterDrag("left", 252, -40)).toBe(212);
  });

  it("grows the right panel when the pointer moves left", () => {
    expect(panelWidthAfterDrag("right", 260, -40)).toBe(300);
    expect(panelWidthAfterDrag("right", 260, 40)).toBe(220);
  });

  it("clamps at both ends for either side", () => {
    expect(panelWidthAfterDrag("left", 252, -1000)).toBe(PANEL_MIN_WIDTH);
    expect(panelWidthAfterDrag("left", 252, 1000)).toBe(PANEL_MAX_WIDTH);
    expect(panelWidthAfterDrag("right", 260, 1000)).toBe(PANEL_MIN_WIDTH);
    expect(panelWidthAfterDrag("right", 260, -1000)).toBe(PANEL_MAX_WIDTH);
  });
});

describe("panelWidthAfterStep", () => {
  it("moves the left divider in the arrow's direction", () => {
    expect(panelWidthAfterStep("left", 252, 1)).toBe(252 + PANEL_RESIZE_STEP);
    expect(panelWidthAfterStep("left", 252, -1)).toBe(252 - PANEL_RESIZE_STEP);
  });

  it("moves the right divider in the arrow's direction too", () => {
    // 右栏的分隔条在它左侧：方向键向右 = 分隔条右移 = 右栏变窄。
    expect(panelWidthAfterStep("right", 260, 1)).toBe(260 - PANEL_RESIZE_STEP);
    expect(panelWidthAfterStep("right", 260, -1)).toBe(260 + PANEL_RESIZE_STEP);
  });

  it("stops at the range bounds", () => {
    expect(panelWidthAfterStep("left", PANEL_MIN_WIDTH, -1)).toBe(PANEL_MIN_WIDTH);
    expect(panelWidthAfterStep("right", PANEL_MAX_WIDTH, -1)).toBe(PANEL_MAX_WIDTH);
  });
});

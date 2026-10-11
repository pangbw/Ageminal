import { describe, expect, it } from "vitest";

import type { PreviewView } from "../../../../bindings";
import { toPreviewContent } from "../content";

function view(overrides: Partial<PreviewView>): PreviewView {
  return {
    kind: "text",
    totalBytes: 0,
    text: "",
    lossy: false,
    truncated: false,
    lines: 1,
    longLine: false,
    imageDataUrl: null,
    imageTooLarge: false,
    ...overrides,
  };
}

describe("toPreviewContent", () => {
  it("narrows a text view to its guardrails", () => {
    const content = toPreviewContent(
      view({ kind: "text", truncated: true, lossy: true, lines: 42, longLine: true }),
    );

    expect(content).toEqual({
      kind: "text",
      truncated: true,
      lossy: true,
      lines: 42,
      longLine: true,
    });
  });

  it("narrows an image view to its data url", () => {
    const content = toPreviewContent(
      view({ kind: "image", totalBytes: 99, imageDataUrl: "data:image/png;base64,AA==" }),
    );

    expect(content).toEqual({
      kind: "image",
      url: "data:image/png;base64,AA==",
      tooLarge: false,
      totalBytes: 99,
    });
  });

  it("carries the too-large flag for oversized images", () => {
    const content = toPreviewContent(
      view({ kind: "image", totalBytes: 999, imageDataUrl: null, imageTooLarge: true }),
    );

    expect(content).toEqual({ kind: "image", url: null, tooLarge: true, totalBytes: 999 });
  });

  it("narrows a binary view", () => {
    expect(toPreviewContent(view({ kind: "binary", totalBytes: 7 }))).toEqual({
      kind: "binary",
      totalBytes: 7,
    });
  });
});

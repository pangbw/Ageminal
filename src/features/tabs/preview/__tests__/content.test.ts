import { describe, expect, it } from "vitest";

import { previewView } from "../../../../__tests__/workspace-fixture";
import { toPreviewContent } from "../content";

describe("toPreviewContent", () => {
  it("narrows a text view to its guardrails", () => {
    const content = toPreviewContent(
      previewView({ kind: "text", truncated: true, lossy: true, lines: 42, longLine: true }),
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
      previewView({ kind: "image", totalBytes: 99, imageDataUrl: "data:image/png;base64,AA==" }),
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
      previewView({ kind: "image", totalBytes: 999, imageDataUrl: null, imageTooLarge: true }),
    );

    expect(content).toEqual({ kind: "image", url: null, tooLarge: true, totalBytes: 999 });
  });

  it("narrows a binary view", () => {
    expect(toPreviewContent(previewView({ kind: "binary", totalBytes: 7 }))).toEqual({
      kind: "binary",
      totalBytes: 7,
    });
  });
});

import { describe, expect, it } from "vitest";

import type { DirEntryView } from "../../../bindings";
import { isVisible, type VisibilityToggles } from "../visibility";

function entry(name: string, overrides: Partial<DirEntryView> = {}): DirEntryView {
  return { name, path: `/ws/${name}`, isDir: false, hidden: false, ignored: false, ...overrides };
}

const bothOff: VisibilityToggles = { showHidden: false, showIgnored: false };

describe("file visibility filtering", () => {
  it("hides dotfiles until the hidden-files switch is on", () => {
    const dotfile = entry(".env", { hidden: true });

    expect(isVisible(dotfile, bothOff)).toBe(false);
    expect(isVisible(dotfile, { showHidden: true, showIgnored: false })).toBe(true);
  });

  it("hides ignored entries until the ignored-files switch is on", () => {
    const ignored = entry("node_modules", { isDir: true, ignored: true });

    expect(isVisible(ignored, bothOff)).toBe(false);
    expect(isVisible(ignored, { showHidden: false, showIgnored: true })).toBe(true);
  });

  it("keeps the two dimensions independent", () => {
    const dotfile = entry(".env", { hidden: true });
    const ignored = entry("node_modules", { isDir: true, ignored: true });
    const plain = entry("README.md");

    expect(isVisible(dotfile, { showHidden: false, showIgnored: true })).toBe(false);
    expect(isVisible(ignored, { showHidden: true, showIgnored: false })).toBe(false);
    expect(isVisible(plain, bothOff)).toBe(true);
  });

  it("reveals an entry that is both hidden and ignored only when both switches are on", () => {
    const dotIgnored = entry(".cache", { isDir: true, hidden: true, ignored: true });

    expect(isVisible(dotIgnored, { showHidden: true, showIgnored: false })).toBe(false);
    expect(isVisible(dotIgnored, { showHidden: false, showIgnored: true })).toBe(false);
    expect(isVisible(dotIgnored, { showHidden: true, showIgnored: true })).toBe(true);
  });
});

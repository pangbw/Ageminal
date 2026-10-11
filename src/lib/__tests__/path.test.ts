import { describe, expect, it } from "vitest";

import { ancestorsOf, basename } from "../path";

describe("basename", () => {
  it("takes the last segment for both separators", () => {
    expect(basename("/ws/a/src/main.ts")).toBe("main.ts");
    expect(basename("C:\\ws\\a\\src\\main.ts")).toBe("main.ts");
  });

  it("falls back to the whole path when there is nothing to split", () => {
    expect(basename("main.ts")).toBe("main.ts");
  });
});

describe("ancestorsOf", () => {
  it("walks up to but not including the root", () => {
    expect(ancestorsOf("/ws/a", "/ws/a/sub/deep/alpha.txt")).toEqual([
      "/ws/a/sub",
      "/ws/a/sub/deep",
    ]);
  });

  it("handles Windows separators", () => {
    expect(ancestorsOf("C:\\ws\\a", "C:\\ws\\a\\sub\\alpha.txt")).toEqual(["C:\\ws\\a\\sub"]);
  });

  it("returns nothing for a direct child", () => {
    expect(ancestorsOf("/ws/a", "/ws/a/alpha.txt")).toEqual([]);
    expect(ancestorsOf("C:\\ws\\a", "C:\\ws\\a\\alpha.txt")).toEqual([]);
  });
});

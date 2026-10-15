import { describe, expect, it } from "vitest";

import { ancestorsOf, basename, isSameOrUnder, parentOf, relativeTo } from "../path";

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

describe("parentOf", () => {
  it("returns the immediate parent for both separators", () => {
    expect(parentOf("/ws/a/src/main.ts")).toBe("/ws/a/src");
    expect(parentOf("C:\\ws\\a\\src\\main.ts")).toBe("C:\\ws\\a\\src");
  });

  it("returns null when there is no parent", () => {
    expect(parentOf("main.ts")).toBeNull();
  });
});

describe("relativeTo", () => {
  it("strips the root and normalizes to forward slashes", () => {
    expect(relativeTo("/ws/a", "/ws/a/src/main.ts")).toBe("src/main.ts");
    expect(relativeTo("C:\\ws\\a", "C:\\ws\\a\\src\\main.ts")).toBe("src/main.ts");
  });

  it("is empty for the root itself and unchanged when outside", () => {
    expect(relativeTo("/ws/a", "/ws/a")).toBe("");
    expect(relativeTo("/ws/a", "/other/x.ts")).toBe("/other/x.ts");
  });
});

describe("isSameOrUnder", () => {
  it("matches the path itself and true descendants", () => {
    expect(isSameOrUnder("/ws/a/b", "/ws/a/b")).toBe(true);
    expect(isSameOrUnder("/ws/a/b/c.ts", "/ws/a/b")).toBe(true);
  });

  it("does not match a sibling sharing a name prefix", () => {
    expect(isSameOrUnder("/ws/a/bc", "/ws/a/b")).toBe(false);
    expect(isSameOrUnder("/ws/a", "/ws/a/b")).toBe(false);
  });
});

import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it } from "vitest";

import { useTabsStore } from "../tabs";

const A = "/ws/a";
const B = "/ws/b";

describe("useTabsStore", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it("opens a preview tab and makes it active", () => {
    const tabs = useTabsStore();

    tabs.openFile(A, `${A}/src/main.ts`, "main.ts");

    expect(tabs.tabsFor(A)).toHaveLength(1);
    expect(tabs.activePreview(A)).toMatchObject({ path: `${A}/src/main.ts`, name: "main.ts" });
  });

  it("activates an already open tab instead of duplicating it", () => {
    const tabs = useTabsStore();
    tabs.openFile(A, `${A}/one.ts`, "one.ts");
    tabs.openFile(A, `${A}/two.ts`, "two.ts");

    tabs.openFile(A, `${A}/one.ts`, "one.ts");

    expect(tabs.tabsFor(A)).toHaveLength(2);
    expect(tabs.activePreview(A)?.path).toBe(`${A}/one.ts`);
  });

  it("picks a neighbour when the active tab closes", () => {
    const tabs = useTabsStore();
    tabs.openFile(A, `${A}/one.ts`, "one.ts");
    tabs.openFile(A, `${A}/two.ts`, "two.ts");
    tabs.openFile(A, `${A}/three.ts`, "three.ts");

    tabs.closeFile(A, `${A}/two.ts`);

    expect(tabs.activePreview(A)?.path).toBe(`${A}/three.ts`);
    expect(tabs.tabsFor(A).map((tab) => tab.name)).toEqual(["one.ts", "three.ts"]);
  });

  it("reports no active tab when the last one closes", () => {
    const tabs = useTabsStore();
    tabs.openFile(A, `${A}/one.ts`, "one.ts");

    tabs.closeFile(A, `${A}/one.ts`);

    expect(tabs.activePreview(A)).toBeNull();
    expect(tabs.tabsFor(A)).toHaveLength(0);
  });

  it("keeps each worktree's tabs apart", () => {
    const tabs = useTabsStore();
    tabs.openFile(A, `${A}/a.ts`, "a.ts");
    tabs.openFile(B, `${B}/b.ts`, "b.ts");

    expect(tabs.tabsFor(A).map((tab) => tab.name)).toEqual(["a.ts"]);
    expect(tabs.tabsFor(B).map((tab) => tab.name)).toEqual(["b.ts"]);
    expect(tabs.activePreview(A)?.name).toBe("a.ts");
  });

  it("tracks a background tab's staleness", () => {
    const tabs = useTabsStore();
    tabs.openFile(A, `${A}/one.ts`, "one.ts");
    tabs.openFile(A, `${A}/two.ts`, "two.ts");

    expect(tabs.isStale(A, `${A}/one.ts`)).toBe(false);
    tabs.markStale(A, `${A}/one.ts`);
    expect(tabs.isStale(A, `${A}/one.ts`)).toBe(true);
    expect(tabs.isStale(A, `${A}/two.ts`)).toBe(false); // 互不影响

    tabs.clearStale(A, `${A}/one.ts`);
    expect(tabs.isStale(A, `${A}/one.ts`)).toBe(false);
  });

  it("marks a tab stale by path across worktrees and lists every open path", () => {
    const tabs = useTabsStore();
    tabs.openFile(A, `${A}/a.ts`, "a.ts");
    tabs.openFile(B, `${B}/b.ts`, "b.ts");

    expect(tabs.allPaths()).toEqual(expect.arrayContaining([`${A}/a.ts`, `${B}/b.ts`]));

    tabs.markStaleByPath(`${B}/b.ts`);

    expect(tabs.isStale(B, `${B}/b.ts`)).toBe(true);
    expect(tabs.isStale(A, `${A}/a.ts`)).toBe(false);
  });

  it("closes the open tabs under a deleted path", () => {
    const tabs = useTabsStore();
    tabs.openFile(A, `${A}/src/one.ts`, "one.ts");
    tabs.openFile(A, `${A}/src/two.ts`, "two.ts");
    tabs.openFile(A, `${A}/readme.md`, "readme.md");

    tabs.closeUnder(A, `${A}/src`);

    expect(tabs.tabsFor(A).map((tab) => tab.name)).toEqual(["readme.md"]);
    expect(tabs.activePreview(A)?.name).toBe("readme.md");
  });

  it("rewrites the open tabs when a parent is renamed", () => {
    const tabs = useTabsStore();
    tabs.openFile(A, `${A}/src/one.ts`, "one.ts");

    tabs.movePath(A, `${A}/src`, `${A}/lib`);

    expect(tabs.tabsFor(A)[0]).toMatchObject({ path: `${A}/lib/one.ts`, name: "one.ts" });
    expect(tabs.activePreview(A)?.path).toBe(`${A}/lib/one.ts`);
  });

  it("tolerates movePath and closeUnder when nothing is open", () => {
    const tabs = useTabsStore();

    expect(() => tabs.movePath(A, `${A}/x`, `${A}/y`)).not.toThrow();
    expect(() => tabs.closeUnder(A, `${A}/x`)).not.toThrow();
  });

  it("opens a terminal tab and keeps it independent from previews", () => {
    const tabs = useTabsStore();
    const sid = "terminal:/ws/a:1";

    tabs.openFile(A, `${A}/one.ts`, "one.ts");
    tabs.openTerminal(A, sid, A, "Terminal");

    expect(tabs.activeTerminal(A)).toMatchObject({ sid, cwd: A, name: "Terminal" });
    expect(tabs.activePreview(A)).toBeNull();
    expect(tabs.allPaths()).toEqual([`${A}/one.ts`]);

    // 再次打开同一个 sid 只激活，不重复建页签。
    tabs.openTerminal(A, sid, A, "Terminal");
    expect(tabs.tabsFor(A)).toHaveLength(2);
  });

  it("falls back to a neighbour when the active terminal closes", () => {
    const tabs = useTabsStore();
    const sid = "terminal:/ws/a:1";

    tabs.openFile(A, `${A}/one.ts`, "one.ts");
    tabs.openTerminal(A, sid, A, "Terminal");
    tabs.closeTerminal(A, sid);

    expect(tabs.activePreview(A)?.path).toBe(`${A}/one.ts`);
    expect(tabs.tabsFor(A)).toHaveLength(1);
  });

  it("does not rewrite terminal tabs on file rename or delete", () => {
    const tabs = useTabsStore();
    const sid = "terminal:/ws/a:1";

    tabs.openTerminal(A, sid, A, "Terminal");
    tabs.movePath(A, A, `${A}/moved`);
    tabs.closeUnder(A, A);

    expect(tabs.activeTerminal(A)?.sid).toBe(sid);
  });
});

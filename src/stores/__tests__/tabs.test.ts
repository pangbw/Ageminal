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
    expect(tabs.activeTab(A)).toMatchObject({ path: `${A}/src/main.ts`, name: "main.ts" });
  });

  it("activates an already open tab instead of duplicating it", () => {
    const tabs = useTabsStore();
    tabs.openFile(A, `${A}/one.ts`, "one.ts");
    tabs.openFile(A, `${A}/two.ts`, "two.ts");

    tabs.openFile(A, `${A}/one.ts`, "one.ts");

    expect(tabs.tabsFor(A)).toHaveLength(2);
    expect(tabs.activeTab(A)?.path).toBe(`${A}/one.ts`);
  });

  it("picks a neighbour when the active tab closes", () => {
    const tabs = useTabsStore();
    tabs.openFile(A, `${A}/one.ts`, "one.ts");
    tabs.openFile(A, `${A}/two.ts`, "two.ts");
    tabs.openFile(A, `${A}/three.ts`, "three.ts");

    tabs.closeFile(A, `${A}/two.ts`);

    expect(tabs.activeTab(A)?.path).toBe(`${A}/three.ts`);
    expect(tabs.tabsFor(A).map((tab) => tab.name)).toEqual(["one.ts", "three.ts"]);
  });

  it("reports no active tab when the last one closes", () => {
    const tabs = useTabsStore();
    tabs.openFile(A, `${A}/one.ts`, "one.ts");

    tabs.closeFile(A, `${A}/one.ts`);

    expect(tabs.activeTab(A)).toBeNull();
    expect(tabs.tabsFor(A)).toHaveLength(0);
  });

  it("keeps each worktree's tabs apart", () => {
    const tabs = useTabsStore();
    tabs.openFile(A, `${A}/a.ts`, "a.ts");
    tabs.openFile(B, `${B}/b.ts`, "b.ts");

    expect(tabs.tabsFor(A).map((tab) => tab.name)).toEqual(["a.ts"]);
    expect(tabs.tabsFor(B).map((tab) => tab.name)).toEqual(["b.ts"]);
    expect(tabs.activeTab(A)?.name).toBe("a.ts");
  });
});

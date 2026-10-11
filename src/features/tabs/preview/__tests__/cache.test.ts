import { EditorState } from "@codemirror/state";
import { describe, expect, it } from "vitest";

import { PREVIEW_LRU, PreviewCache, type PreviewEntry } from "../cache";

function entry(text: string, scroll = 0): PreviewEntry {
  return {
    content: { kind: "text", truncated: false, lossy: false, lines: 1, longLine: false },
    state: EditorState.create({ doc: text }),
    scroll,
  };
}

describe("PreviewCache", () => {
  it("evicts the least recently used entry past the LRU cap", () => {
    expect(PREVIEW_LRU).toBe(12);
    const cache = new PreviewCache();

    for (let index = 0; index <= PREVIEW_LRU; index += 1) {
      cache.set(`/f${index}`, entry(`doc${index}`));
    }

    expect(cache.size).toBe(PREVIEW_LRU);
    expect(cache.keys()).not.toContain("/f0");
    expect(cache.keys()).toContain(`/f${PREVIEW_LRU}`);
  });

  it("touches an entry on get so it is not evicted next", () => {
    const cache = new PreviewCache(2);
    cache.set("a", entry("a"));
    cache.set("b", entry("b"));

    cache.get("a");
    cache.set("c", entry("c"));

    expect(cache.keys()).toEqual(["a", "c"]);
  });

  it("keeps the EditorState and restores the scroll position", () => {
    const cache = new PreviewCache();
    const state = EditorState.create({ doc: "a\nb\nc" });
    cache.set("/f", {
      content: { kind: "text", truncated: false, lossy: false, lines: 3, longLine: false },
      state,
      scroll: 0,
    });

    cache.setScroll("/f", 120);
    const hit = cache.get("/f");

    // 同一份状态对象 = 折叠与选区原样回来。
    expect(hit?.state).toBe(state);
    expect(hit?.scroll).toBe(120);
  });

  it("peek does not change the eviction order", () => {
    const cache = new PreviewCache(2);
    cache.set("a", entry("a"));
    cache.set("b", entry("b"));

    cache.peek("a");
    cache.set("c", entry("c"));

    expect(cache.keys()).toEqual(["b", "c"]);
  });
});

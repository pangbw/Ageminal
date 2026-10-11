import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ok } from "../../__tests__/workspace-fixture";
import {
  commands,
  type DirEntryView,
  type DirListingView,
  type SearchHitView,
  type SearchResultView,
} from "../../bindings";
import type { CommandResult } from "../../lib/ipc";
import { useFilesStore } from "../files";
import { useUiStore } from "../ui";

vi.mock(
  "../../bindings",
  async () => (await import("../../__tests__/workspace-fixture")).createBindingsMock(),
);

const ROOT = "/ws/a";

function entry(name: string, overrides: Partial<DirEntryView> = {}): DirEntryView {
  const path = overrides.path ?? `${ROOT}/${name}`;
  return {
    name: path.split("/").pop() ?? name,
    path,
    isDir: false,
    hidden: false,
    ignored: false,
    ...overrides,
  };
}

function listing(entries: DirEntryView[], truncated = false): DirListingView {
  return { entries, truncated };
}

function hit(path: string, overrides: Partial<SearchHitView> = {}): SearchHitView {
  return { path, relative: path.replace(`${ROOT}/`, ""), isDir: false, ignored: false, ...overrides };
}

function searchResult(hits: SearchHitView[], overrides: Partial<SearchResultView> = {}): SearchResultView {
  return { hits, truncated: false, timedOut: false, ...overrides };
}

describe("useFilesStore", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.clearAllMocks();
  });

  it("reads the root directory when the worktree changes", async () => {
    const files = useFilesStore();
    vi.mocked(commands.readDir).mockReturnValue(ok(listing([entry("README.md")])));

    files.setRoot(ROOT);
    await vi.waitFor(() => expect(files.visibleRows).toHaveLength(1));

    expect(commands.readDir).toHaveBeenCalledWith(ROOT, ROOT);
    expect(files.visibleRows[0]).toMatchObject({ name: "README.md", depth: 0, loaded: false });
  });

  it("only reads a directory when it is expanded", async () => {
    const files = useFilesStore();
    const src = entry("src", { isDir: true });
    vi.mocked(commands.readDir).mockImplementation((_root, dir) =>
      dir === ROOT ? ok(listing([src, entry("README.md")])) : ok(listing([entry("main.ts")])),
    );

    files.setRoot(ROOT);
    await vi.waitFor(() => expect(files.visibleRows).toHaveLength(2));
    expect(commands.readDir).toHaveBeenCalledTimes(1); // 首屏只读了根

    await files.toggleDir(src.path);
    await vi.waitFor(() => expect(files.visibleRows).toHaveLength(3));

    expect(commands.readDir).toHaveBeenCalledWith(ROOT, src.path);
    expect(files.visibleRows.map((row) => row.name)).toEqual(["src", "main.ts", "README.md"]);
    expect(files.visibleRows[1]).toMatchObject({ depth: 1, isDir: false });
  });

  it("filters dotfiles and ignored entries by the two switches", async () => {
    const files = useFilesStore();
    const ui = useUiStore();
    vi.mocked(commands.readDir).mockReturnValue(
      ok(listing([entry("index.js"), entry(".env", { hidden: true }), entry("dist", { isDir: true, ignored: true })])),
    );

    files.setRoot(ROOT);
    await vi.waitFor(() => expect(files.visibleRows).toHaveLength(1));
    expect(files.visibleRows.map((row) => row.name)).toEqual(["index.js"]);

    ui.toggleShowHidden();
    await vi.waitFor(() => expect(files.visibleRows).toHaveLength(2));

    ui.toggleShowIgnored();
    await vi.waitFor(() => expect(files.visibleRows).toHaveLength(3));
    expect(files.visibleRows[2]).toMatchObject({ name: "dist", ignored: true });
  });

  it("reveals a hit by loading its ancestors and selecting it", async () => {
    const files = useFilesStore();
    vi.mocked(commands.readDir).mockImplementation((_root, dir) => {
      if (dir === ROOT) return ok(listing([entry("sub", { isDir: true })]));
      if (dir === `${ROOT}/sub`) return ok(listing([entry("sub/deep", { isDir: true })]));
      return ok(listing([entry("sub/deep/alpha.txt")]));
    });

    files.setRoot(ROOT);
    await vi.waitFor(() => expect(files.visibleRows).toHaveLength(1));

    await files.reveal(`${ROOT}/sub/deep/alpha.txt`);
    await vi.waitFor(() => expect(files.visibleRows).toHaveLength(3));

    expect(files.visibleRows.map((row) => row.depth)).toEqual([0, 1, 2]);
    expect(files.selected).toBe(`${ROOT}/sub/deep/alpha.txt`);
  });

  it("discards a stale directory response", async () => {
    const files = useFilesStore();
    let resolveFirst!: (value: CommandResult<DirListingView>) => void;
    vi.mocked(commands.readDir).mockReturnValueOnce(
      new Promise<CommandResult<DirListingView>>((resolve) => {
        resolveFirst = resolve;
      }),
    );

    files.setRoot(ROOT); // 第一次请求挂起
    vi.mocked(commands.readDir).mockReturnValueOnce(ok(listing([entry("B.txt")])));
    await files.loadDir(ROOT); // 第二次请求先回

    resolveFirst({ status: "ok", data: listing([entry("A.txt")]) });
    await vi.waitFor(() => expect(files.visibleRows.map((row) => row.name)).toEqual(["B.txt"]));
  });

  it("searches through the Rust command after a debounce", async () => {
    const files = useFilesStore();
    vi.mocked(commands.readDir).mockReturnValue(ok(listing([])));
    vi.mocked(commands.searchFiles).mockReturnValue(ok(searchResult([hit(`${ROOT}/src/main.ts`)])));

    files.setRoot(ROOT);
    files.setQuery("main");

    await vi.waitFor(() => expect(files.results).not.toBeNull());

    expect(commands.searchFiles).toHaveBeenCalledWith(ROOT, "main", false, false);
    expect(files.results?.map((item) => item.relative)).toEqual(["src/main.ts"]);
  });

  it("re-runs an active search when a visibility switch changes", async () => {
    const files = useFilesStore();
    const ui = useUiStore();
    vi.mocked(commands.readDir).mockReturnValue(ok(listing([])));
    vi.mocked(commands.searchFiles).mockReturnValue(ok(searchResult([])));

    files.setRoot(ROOT);
    files.setQuery("main");
    await vi.waitFor(() => expect(commands.searchFiles).toHaveBeenCalledTimes(1));

    ui.toggleShowIgnored();

    await vi.waitFor(() => expect(commands.searchFiles).toHaveBeenCalledTimes(2));
    expect(commands.searchFiles).toHaveBeenLastCalledWith(ROOT, "main", false, true);
  });

  it("marks a truncated directory with a notice row", async () => {
    const files = useFilesStore();
    vi.mocked(commands.readDir).mockReturnValue(ok(listing([entry("a.txt")], true)));

    files.setRoot(ROOT);
    await vi.waitFor(() => expect(files.visibleRows).toHaveLength(2));

    expect(files.visibleRows[1]).toMatchObject({ name: "", notice: true });
  });

  it("leaves search mode when the query is cleared", async () => {
    const files = useFilesStore();
    vi.mocked(commands.readDir).mockReturnValue(ok(listing([])));
    vi.mocked(commands.searchFiles).mockReturnValue(ok(searchResult([hit(`${ROOT}/a.txt`)])));

    files.setRoot(ROOT);
    files.setQuery("a");
    await vi.waitFor(() => expect(files.results).not.toBeNull());

    files.clearSearch();

    expect(files.results).toBeNull();
    expect(files.query).toBe("");
  });
});

import { mount } from "@vue/test-utils";
import { createPinia, setActivePinia, type Pinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ok } from "../../../__tests__/workspace-fixture";
import {
  commands,
  type DirEntryView,
  type DirListingView,
  type SearchResultView,
} from "../../../bindings";
import { useFilesStore } from "../../../stores/files";
import { useTabsStore } from "../../../stores/tabs";
import { useUiStore } from "../../../stores/ui";
import FileTree from "../FileTree.vue";

vi.mock(
  "../../../bindings",
  async () => (await import("../../../__tests__/workspace-fixture")).createBindingsMock(),
);

// 虚拟滚动在 happy-dom 里量不到容器高度；这些用例只关心行的渲染与交互，故把它摊平成全量渲染。
vi.mock("@vueuse/core", async () => {
  const { computed, ref, unref } = await import("vue");
  return {
    useVirtualList: (source: unknown) => ({
      list: computed(() => (unref(source) as unknown[]).map((data, index) => ({ data, index }))),
      scrollTo: vi.fn(),
      containerProps: { ref: ref(null), onScroll: vi.fn(), style: {} },
      wrapperProps: computed(() => ({ style: {} })),
    }),
  };
});

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

function setup() {
  const pinia = createPinia();
  setActivePinia(pinia);
  return { pinia, files: useFilesStore(), ui: useUiStore(), tabs: useTabsStore() };
}

function mountTree(pinia: Pinia) {
  return mount(FileTree, { global: { plugins: [pinia] } });
}

function names(wrapper: ReturnType<typeof mountTree>): string[] {
  return wrapper.findAll(".files__name").map((node) => node.text());
}

describe("FileTree", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("shows the empty state when no worktree is active", () => {
    const { pinia } = setup();

    const wrapper = mountTree(pinia);

    expect(wrapper.find(".empty-state").exists()).toBe(true);
  });

  it("renders the root entries and hides dotfiles", async () => {
    const { pinia, files } = setup();
    vi.mocked(commands.readDir).mockReturnValue(
      ok(listing([entry("src", { isDir: true }), entry("README.md"), entry(".env", { hidden: true })])),
    );

    files.setRoot(ROOT);
    await vi.waitFor(() => expect(files.visibleRows).toHaveLength(2));
    const wrapper = mountTree(pinia);

    expect(names(wrapper)).toEqual(["src", "README.md"]);
  });

  it("reads and expands a directory on click", async () => {
    const { pinia, files } = setup();
    vi.mocked(commands.readDir).mockImplementation((_root, dir) =>
      dir === ROOT ? ok(listing([entry("src", { isDir: true })])) : ok(listing([entry("src/main.ts")])),
    );

    files.setRoot(ROOT);
    await vi.waitFor(() => expect(files.visibleRows).toHaveLength(1));
    const wrapper = mountTree(pinia);

    await wrapper.find(".files__row").trigger("click");
    await vi.waitFor(() => expect(names(wrapper)).toEqual(["src", "main.ts"]));

    expect(commands.readDir).toHaveBeenCalledWith(ROOT, `${ROOT}/src`);
  });

  it("opens a preview tab when a file is clicked", async () => {
    const { pinia, files, tabs } = setup();
    vi.mocked(commands.readDir).mockReturnValue(ok(listing([entry("README.md")])));

    files.setRoot(ROOT);
    await vi.waitFor(() => expect(files.visibleRows).toHaveLength(1));
    const wrapper = mountTree(pinia);

    await wrapper.find(".files__row").trigger("click");

    expect(tabs.activePreview(ROOT)?.path).toBe(`${ROOT}/README.md`);
  });

  it("reveals ignored entries when the switch is turned on", async () => {
    const { pinia, files, ui } = setup();
    vi.mocked(commands.readDir).mockReturnValue(
      ok(listing([entry("index.js"), entry("dist", { isDir: true, ignored: true })])),
    );

    files.setRoot(ROOT);
    await vi.waitFor(() => expect(files.visibleRows).toHaveLength(1));
    const wrapper = mountTree(pinia);
    expect(wrapper.findAll(".files__row")).toHaveLength(1);

    ui.toggleShowIgnored();
    await vi.waitFor(() => expect(wrapper.findAll(".files__row")).toHaveLength(2));

    expect(wrapper.find(".files__row--ignored").exists()).toBe(true);
  });

  it("locates a search hit and opens it", async () => {
    const { pinia, files, tabs } = setup();
    const result: SearchResultView = {
      hits: [{ path: `${ROOT}/src/main.ts`, relative: "src/main.ts", isDir: false, ignored: false }],
      truncated: false,
      timedOut: false,
    };
    vi.mocked(commands.readDir).mockImplementation((_root, dir) =>
      dir === ROOT ? ok(listing([entry("src", { isDir: true })])) : ok(listing([entry("src/main.ts")])),
    );
    vi.mocked(commands.searchFiles).mockReturnValue(ok(result));

    files.setRoot(ROOT);
    await vi.waitFor(() => expect(files.visibleRows).toHaveLength(1));
    const wrapper = mountTree(pinia);

    await wrapper.find(".files__search-input").setValue("main");
    await vi.waitFor(() => expect(wrapper.find(".files__hit").exists()).toBe(true));

    await wrapper.find(".files__hit").trigger("click");
    await vi.waitFor(() => expect(tabs.activePreview(ROOT)?.path).toBe(`${ROOT}/src/main.ts`));

    expect(commands.searchFiles).toHaveBeenCalledWith(ROOT, "main", false, false);
    expect(files.visibleRows.map((row) => row.name)).toEqual(["src", "main.ts"]);
  });

  it("shows a truncation notice for a large directory", async () => {
    const { pinia, files } = setup();
    vi.mocked(commands.readDir).mockReturnValue(ok(listing([entry("a.txt")], true)));

    files.setRoot(ROOT);
    await vi.waitFor(() => expect(files.visibleRows).toHaveLength(2));
    const wrapper = mountTree(pinia);

    expect(wrapper.find(".files__notice").text()).toContain("5000");
  });

  it("expands a directory hit instead of opening a preview tab", async () => {
    const { pinia, files, tabs } = setup();
    const result: SearchResultView = {
      hits: [{ path: `${ROOT}/src`, relative: "src", isDir: true, ignored: false }],
      truncated: false,
      timedOut: false,
    };
    vi.mocked(commands.readDir).mockImplementation((_root, dir) =>
      dir === ROOT ? ok(listing([entry("src", { isDir: true })])) : ok(listing([entry("src/main.ts")])),
    );
    vi.mocked(commands.searchFiles).mockReturnValue(ok(result));

    files.setRoot(ROOT);
    await vi.waitFor(() => expect(files.visibleRows).toHaveLength(1));
    const wrapper = mountTree(pinia);

    await wrapper.find(".files__search-input").setValue("src");
    await vi.waitFor(() => expect(wrapper.find(".files__hit").exists()).toBe(true));
    await wrapper.find(".files__hit").trigger("click");
    await vi.waitFor(() => expect(names(wrapper)).toEqual(["src", "main.ts"]));

    expect(tabs.tabsFor(ROOT)).toHaveLength(0);
  });

  it("creates a file inline from the context menu", async () => {
    const { pinia, files } = setup();
    vi.mocked(commands.readDir).mockImplementation((_root, dir) =>
      dir === ROOT ? ok(listing([entry("src", { isDir: true })])) : ok(listing([entry("src/child.ts")])),
    );

    files.setRoot(ROOT);
    await vi.waitFor(() => expect(files.visibleRows).toHaveLength(1));
    const wrapper = mountTree(pinia);

    await wrapper.find(".files__row").trigger("contextmenu");
    await wrapper.findAll(".ctx__item")[0].trigger("click"); // 新建文件
    await vi.waitFor(() => expect(wrapper.find(".files__edit").exists()).toBe(true));

    vi.mocked(commands.createEntry).mockReturnValue(ok(`${ROOT}/src/new.ts`));
    await wrapper.find(".files__edit").setValue("new.ts");
    await wrapper.find(".files__edit").trigger("keydown.enter");

    await vi.waitFor(() =>
      expect(commands.createEntry).toHaveBeenCalledWith(ROOT, `${ROOT}/src`, "new.ts", false),
    );
  });

  it("renames inline from the context menu", async () => {
    const { pinia, files } = setup();
    vi.mocked(commands.readDir).mockReturnValue(ok(listing([entry("README.md")])));

    files.setRoot(ROOT);
    await vi.waitFor(() => expect(files.visibleRows).toHaveLength(1));
    const wrapper = mountTree(pinia);

    await wrapper.find(".files__row").trigger("contextmenu");
    await wrapper.findAll(".ctx__item")[2].trigger("click"); // 重命名
    await vi.waitFor(() => expect(wrapper.find(".files__edit").exists()).toBe(true));
    expect((wrapper.find(".files__edit").element as HTMLInputElement).value).toBe("README.md");

    vi.mocked(commands.renameEntry).mockReturnValue(ok(`${ROOT}/RENAMED.md`));
    await wrapper.find(".files__edit").setValue("RENAMED.md");
    await wrapper.find(".files__edit").trigger("keydown.enter");

    await vi.waitFor(() =>
      expect(commands.renameEntry).toHaveBeenCalledWith(ROOT, `${ROOT}/README.md`, "RENAMED.md"),
    );
  });

  it("asks for confirmation before deleting", async () => {
    const { pinia, files } = setup();
    vi.mocked(commands.readDir).mockReturnValue(ok(listing([entry("README.md")])));

    files.setRoot(ROOT);
    await vi.waitFor(() => expect(files.visibleRows).toHaveLength(1));
    const wrapper = mountTree(pinia);

    await wrapper.find(".files__row").trigger("contextmenu");
    await wrapper.findAll(".ctx__item")[8].trigger("click"); // 删除
    await vi.waitFor(() => expect(wrapper.find(".confirm__box").exists()).toBe(true));

    vi.mocked(commands.deleteEntry).mockReturnValue(ok(null));
    await wrapper.find(".confirm__button--danger").trigger("click");

    await vi.waitFor(() =>
      expect(commands.deleteEntry).toHaveBeenCalledWith(ROOT, `${ROOT}/README.md`),
    );
  });
});

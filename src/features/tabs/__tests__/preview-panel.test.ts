import { mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ok, previewView } from "../../../__tests__/workspace-fixture";
import { commands, type PreviewView } from "../../../bindings";
import { translate as t } from "../../../i18n/translate";
import { useTabsStore } from "../../../stores/tabs";
import PreviewPanel from "../PreviewPanel.vue";

vi.mock(
  "../../../bindings",
  async () => (await import("../../../__tests__/workspace-fixture")).createBindingsMock(),
);

/** 可控的 Tauri 事件替身：测试里手动 `emit`。 */
const bus = vi.hoisted(() => {
  type Handler = (event: { payload: unknown }) => void;
  const handlers = new Map<string, Set<Handler>>();
  return {
    async listen(name: string, handler: Handler) {
      const set = handlers.get(name) ?? new Set<Handler>();
      set.add(handler);
      handlers.set(name, set);
      return () => set.delete(handler);
    },
    emit(name: string, payload: unknown) {
      for (const handler of handlers.get(name) ?? []) handler({ payload });
    },
    clear() {
      handlers.clear();
    },
  };
});

vi.mock("@tauri-apps/api/event", () => ({ listen: bus.listen }));

const ROOT = "/ws/a";
const TAB = { path: `${ROOT}/a.txt`, name: "a.txt", stale: false };
const OTHER = { path: `${ROOT}/b.txt`, name: "b.txt", stale: false };

async function mountPanel(
  overrides: Partial<PreviewView> = {},
  open: string[] = [TAB.path],
): Promise<ReturnType<typeof mount>> {
  vi.mocked(commands.readPreview).mockReturnValue(ok(previewView(overrides)));
  const pinia = createPinia();
  setActivePinia(pinia);
  const tabs = useTabsStore();
  for (const path of open) tabs.openFile(ROOT, path, path.split("/").pop() ?? path);
  tabs.openFile(ROOT, TAB.path, TAB.name);

  const wrapper = mount(PreviewPanel, {
    props: { root: ROOT, tab: TAB },
    global: { plugins: [pinia] },
  });
  await vi.waitFor(() => expect(wrapper.text()).not.toContain(t("tabs.preview.loading")));
  return wrapper;
}

describe("PreviewPanel", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    bus.clear();
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("renders a text file without warnings", async () => {
    const wrapper = await mountPanel();

    expect(wrapper.find(".preview__warning").exists()).toBe(false);
    expect(wrapper.find(".preview__name").text()).toBe("a.txt");
    wrapper.unmount();
  });

  it("shows the lossy-UTF-8 warning", async () => {
    const wrapper = await mountPanel({ lossy: true });

    expect(wrapper.find(".preview__warning").text()).toBe(t("tabs.preview.invalidUtf8"));
    wrapper.unmount();
  });

  it("shows the truncation and long-line warnings", async () => {
    const wrapper = await mountPanel({ truncated: true, longLine: true });

    const warnings = wrapper.findAll(".preview__warning").map((node) => node.text());
    expect(warnings).toEqual([t("tabs.preview.truncated"), t("tabs.preview.longLine")]);
    wrapper.unmount();
  });

  it("renders a binary notice", async () => {
    const wrapper = await mountPanel({ kind: "binary", text: null, totalBytes: 4 });

    expect(wrapper.find(".preview__note").text()).toBe(t("tabs.preview.binary"));
    wrapper.unmount();
  });

  it("renders an image from its data url", async () => {
    const wrapper = await mountPanel({
      kind: "image",
      text: null,
      imageDataUrl: "data:image/png;base64,AA==",
    });

    expect(wrapper.find("img.preview__image").attributes("src")).toBe("data:image/png;base64,AA==");
    wrapper.unmount();
  });

  it("warns when an image is too large", async () => {
    const wrapper = await mountPanel({
      kind: "image",
      text: null,
      imageDataUrl: null,
      imageTooLarge: true,
      totalBytes: 99,
    });

    expect(wrapper.find(".preview__note").text()).toBe(t("tabs.preview.imageTooLarge"));
    wrapper.unmount();
  });

  it("opens the file in an external editor", async () => {
    const wrapper = await mountPanel();
    vi.mocked(commands.openInEditor).mockReturnValue(
      Promise.resolve({ status: "ok", data: null }),
    );

    await wrapper.findAll(".preview__action")[0].trigger("click");

    expect(commands.openInEditor).toHaveBeenCalledWith(ROOT, TAB.path);
    wrapper.unmount();
  });

  it("emits close from the close button", async () => {
    const wrapper = await mountPanel();

    await wrapper.findAll(".preview__action")[1].trigger("click");

    expect(wrapper.emitted("close")).toHaveLength(1);
    wrapper.unmount();
  });

  it("watches the files of the open preview tabs", async () => {
    const wrapper = await mountPanel({}, [TAB.path, OTHER.path]);

    expect(commands.setWatchedFiles).toHaveBeenCalledWith(
      expect.arrayContaining([TAB.path, OTHER.path]),
    );
    wrapper.unmount();
  });

  it("refreshes the visible tab in place and shows the updated hint", async () => {
    const wrapper = await mountPanel();
    vi.mocked(commands.readPreview).mockReturnValue(ok(previewView({ text: "new" })));

    bus.emit("fs-change", { paths: [TAB.path] });

    await vi.waitFor(() => expect(commands.readPreview).toHaveBeenCalledTimes(2));
    await vi.waitFor(() => expect(wrapper.find(".preview__updated").exists()).toBe(true));
    expect(wrapper.find(".preview__updated").text()).toBe(t("tabs.preview.updated"));
    wrapper.unmount();
  });

  it("marks a background tab stale without reloading it", async () => {
    const wrapper = await mountPanel({}, [TAB.path, OTHER.path]);
    const tabs = useTabsStore();

    bus.emit("fs-change", { paths: [OTHER.path] });

    await vi.waitFor(() => expect(tabs.isStale(ROOT, OTHER.path)).toBe(true));
    expect(commands.readPreview).toHaveBeenCalledTimes(1); // 只读了激活页签那一次
    expect(wrapper.find(".preview__updated").exists()).toBe(false);
    wrapper.unmount();
  });

  it("refreshes a stale tab when it becomes active", async () => {
    const wrapper = await mountPanel({}, [TAB.path, OTHER.path]);
    const tabs = useTabsStore();
    tabs.markStale(ROOT, OTHER.path);
    vi.mocked(commands.readPreview).mockClear();
    vi.mocked(commands.readPreview).mockReturnValue(ok(previewView({ text: "b" })));

    await wrapper.setProps({ tab: OTHER });

    await vi.waitFor(() =>
      expect(commands.readPreview).toHaveBeenCalledWith(ROOT, OTHER.path),
    );
    await vi.waitFor(() => expect(tabs.isStale(ROOT, OTHER.path)).toBe(false));
    wrapper.unmount();
  });

  it("marks a stale tab that belongs to another worktree", async () => {
    const wrapper = await mountPanel();
    const tabs = useTabsStore();
    const otherRoot = "/ws/b";
    const otherPath = `${otherRoot}/c.txt`;
    tabs.openFile(otherRoot, otherPath, "c.txt");

    bus.emit("fs-change", { paths: [otherPath] });

    await vi.waitFor(() => expect(tabs.isStale(otherRoot, otherPath)).toBe(true));
    expect(commands.readPreview).toHaveBeenCalledTimes(1); // 未重读非激活 worktree
    wrapper.unmount();
  });

  it("ignores changes to files that are not open", async () => {
    const wrapper = await mountPanel();

    bus.emit("fs-change", { paths: [`${ROOT}/other.txt`] });
    await new Promise((resolve) => setTimeout(resolve, 10));

    expect(commands.readPreview).toHaveBeenCalledTimes(1);
    expect(wrapper.find(".preview__updated").exists()).toBe(false);
    wrapper.unmount();
  });
});

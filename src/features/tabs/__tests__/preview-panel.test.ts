import { mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ok, previewView } from "../../../__tests__/workspace-fixture";
import { commands, type PreviewView } from "../../../bindings";
import { translate as t } from "../../../i18n/translate";
import PreviewPanel from "../PreviewPanel.vue";

vi.mock(
  "../../../bindings",
  async () => (await import("../../../__tests__/workspace-fixture")).createBindingsMock(),
);

const ROOT = "/ws/a";
const TAB = { path: `${ROOT}/a.txt`, name: "a.txt" };

async function mountPanel(overrides: Partial<PreviewView> = {}) {
  vi.mocked(commands.readPreview).mockReturnValue(ok(previewView(overrides)));
  const wrapper = mount(PreviewPanel, { props: { root: ROOT, tab: TAB } });
  await vi.waitFor(() =>
    expect(wrapper.text()).not.toContain(t("tabs.preview.loading")),
  );
  return wrapper;
}

describe("PreviewPanel", () => {
  beforeEach(() => {
    vi.clearAllMocks();
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
});

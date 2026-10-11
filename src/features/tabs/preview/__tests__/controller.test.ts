import { EditorSelection } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ok, previewView } from "../../../../__tests__/workspace-fixture";
import { commands, type PreviewView } from "../../../../bindings";
import type { CommandResult } from "../../../../lib/ipc";
import { PreviewController } from "../controller";

vi.mock(
  "../../../../bindings",
  async () => (await import("../../../../__tests__/workspace-fixture")).createBindingsMock(),
);

const ROOT = "/ws/a";

function mountHost(): HTMLElement {
  const host = document.createElement("div");
  document.body.appendChild(host);
  return host;
}

describe("PreviewController", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("loads text, swaps the shared state, and hits the cache on re-activation", async () => {
    const controller = new PreviewController();
    controller.mount(mountHost());
    vi.mocked(commands.readPreview).mockReturnValue(
      ok(previewView({ text: "hello\nworld", lines: 2 })),
    );

    const first = await controller.activate(ROOT, `${ROOT}/a.txt`);
    expect(first.content.kind).toBe("text");
    expect(controller.state?.doc.toString()).toBe("hello\nworld");
    expect(commands.readPreview).toHaveBeenCalledTimes(1);

    await controller.activate(ROOT, `${ROOT}/a.txt`);

    expect(commands.readPreview).toHaveBeenCalledTimes(1); // 命中缓存，不再读盘
    controller.unmount();
  });

  it("keeps the editor on the last text state for non-text content", async () => {
    const controller = new PreviewController();
    controller.mount(mountHost());
    vi.mocked(commands.readPreview).mockImplementation((_root, path) =>
      path.endsWith("a.txt")
        ? ok(previewView({ text: "text" }))
        : ok(previewView({ kind: "binary", text: null, totalBytes: 4 })),
    );

    await controller.activate(ROOT, `${ROOT}/a.txt`);
    const binary = await controller.activate(ROOT, `${ROOT}/so.bin`);

    expect(binary.content).toEqual({ kind: "binary", totalBytes: 4 });
    expect(controller.state?.doc.toString()).toBe("text");
    controller.unmount();
  });

  it("restores a cached tab's live selection when switching back", async () => {
    const controller = new PreviewController();
    const host = mountHost();
    controller.mount(host);
    vi.mocked(commands.readPreview).mockImplementation((_root, path) =>
      ok(previewView({ text: path.endsWith("a.txt") ? "aaa\nbbb\nccc" : "BBB" })),
    );

    await controller.activate(ROOT, `${ROOT}/a.txt`);
    // 模拟用户在 a 上做了选区（折叠 / 选区都体现在 EditorState 里）。
    const view = EditorView.findFromDOM(host)!;
    view.dispatch({ selection: EditorSelection.cursor(5) });

    await controller.activate(ROOT, `${ROOT}/b.txt`);
    await controller.activate(ROOT, `${ROOT}/a.txt`);

    expect(commands.readPreview).toHaveBeenCalledTimes(2); // 命中缓存
    expect(controller.state?.doc.toString()).toBe("aaa\nbbb\nccc");
    expect(controller.state?.selection.main.anchor).toBe(5);
    controller.unmount();
  });

  it("re-reads images instead of caching them", async () => {
    const controller = new PreviewController();
    controller.mount(mountHost());
    vi.mocked(commands.readPreview).mockReturnValue(
      ok(previewView({ kind: "image", text: null, imageDataUrl: "data:image/png;base64,AA==" })),
    );

    await controller.activate(ROOT, `${ROOT}/x.png`);
    await controller.activate(ROOT, `${ROOT}/x.png`);

    expect(commands.readPreview).toHaveBeenCalledTimes(2);
    controller.unmount();
  });

  it("ignores a stale load that resolves after switching tabs", async () => {
    const controller = new PreviewController();
    controller.mount(mountHost());
    const resolvers = new Map<string, (result: CommandResult<PreviewView>) => void>();
    vi.mocked(commands.readPreview).mockImplementation(
      (_root, path) =>
        new Promise<CommandResult<PreviewView>>((resolve) => {
          resolvers.set(path, resolve);
        }),
    );

    const first = controller.activate(ROOT, `${ROOT}/a.txt`);
    const second = controller.activate(ROOT, `${ROOT}/b.txt`);

    resolvers.get(`${ROOT}/b.txt`)!({ status: "ok", data: previewView({ text: "B" }) });
    await second;
    resolvers.get(`${ROOT}/a.txt`)!({ status: "ok", data: previewView({ text: "A" }) });
    await first;

    expect(controller.state?.doc.toString()).toBe("B"); // 晚到的 A 不上屏
    controller.unmount();
  });

  it("re-reads a cached tab when forced (stale activation)", async () => {
    const controller = new PreviewController();
    controller.mount(mountHost());
    vi.mocked(commands.readPreview).mockReturnValue(ok(previewView({ text: "a" })));

    await controller.activate(ROOT, `${ROOT}/a.txt`);
    await controller.activate(ROOT, `${ROOT}/a.txt`, { force: true });

    expect(commands.readPreview).toHaveBeenCalledTimes(2); // 强制绕过缓存
    controller.unmount();
  });

  it("replaces the active document in place and keeps the scroll on reload", async () => {
    const controller = new PreviewController();
    const host = mountHost();
    controller.mount(host);
    vi.mocked(commands.readPreview).mockReturnValue(ok(previewView({ text: "one\ntwo" })));
    await controller.activate(ROOT, `${ROOT}/a.txt`);

    const view = EditorView.findFromDOM(host)!;
    view.scrollDOM.scrollTop = 40;

    vi.mocked(commands.readPreview).mockReturnValue(
      ok(previewView({ text: "ONE\nTWO\nTHREE", lines: 3 })),
    );
    const entry = await controller.reload(ROOT, `${ROOT}/a.txt`);

    expect(entry?.content.kind).toBe("text");
    expect(controller.state?.doc.toString()).toBe("ONE\nTWO\nTHREE");
    expect(view.scrollDOM.scrollTop).toBe(40); // 事务替换，滚动保留
    controller.unmount();
  });
});

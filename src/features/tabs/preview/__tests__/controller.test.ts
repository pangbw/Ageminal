import { beforeEach, describe, expect, it, vi } from "vitest";

import { ok } from "../../../../__tests__/workspace-fixture";
import { commands, type PreviewView } from "../../../../bindings";
import type { CommandResult } from "../../../../lib/ipc";
import { PreviewController } from "../controller";

vi.mock(
  "../../../../bindings",
  async () => (await import("../../../../__tests__/workspace-fixture")).createBindingsMock(),
);

const ROOT = "/ws/a";

function view(overrides: Partial<PreviewView> = {}): PreviewView {
  return {
    kind: "text",
    totalBytes: 0,
    text: "",
    lossy: false,
    truncated: false,
    lines: 1,
    longLine: false,
    imageDataUrl: null,
    imageTooLarge: false,
    ...overrides,
  };
}

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
      ok(view({ text: "hello\nworld", lines: 2 })),
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
        ? ok(view({ text: "text" }))
        : ok(view({ kind: "binary", text: null, totalBytes: 4 })),
    );

    await controller.activate(ROOT, `${ROOT}/a.txt`);
    const binary = await controller.activate(ROOT, `${ROOT}/so.bin`);

    expect(binary.content).toEqual({ kind: "binary", totalBytes: 4 });
    expect(controller.state?.doc.toString()).toBe("text");
    controller.unmount();
  });

  it("caches small images and re-reads oversized ones", async () => {
    const controller = new PreviewController();
    controller.mount(mountHost());
    vi.mocked(commands.readPreview).mockImplementation((_root, path) =>
      ok(
        view({
          kind: "image",
          text: null,
          imageDataUrl: "data:image/png;base64,AA==",
          totalBytes: path.endsWith("big.png") ? 5 * 1024 * 1024 : 1024,
        }),
      ),
    );

    await controller.activate(ROOT, `${ROOT}/small.png`);
    await controller.activate(ROOT, `${ROOT}/big.png`);
    await controller.activate(ROOT, `${ROOT}/small.png`);
    await controller.activate(ROOT, `${ROOT}/big.png`);

    // 小图命中缓存（1 次），大图每次重读（2 次）——共 3 次。
    expect(commands.readPreview).toHaveBeenCalledTimes(3);
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

    resolvers.get(`${ROOT}/b.txt`)!({ status: "ok", data: view({ text: "B" }) });
    await second;
    resolvers.get(`${ROOT}/a.txt`)!({ status: "ok", data: view({ text: "A" }) });
    await first;

    expect(controller.state?.doc.toString()).toBe("B"); // 晚到的 A 不上屏
    controller.unmount();
  });

  it("drops a page from the cache with forget", async () => {
    const controller = new PreviewController();
    controller.mount(mountHost());
    vi.mocked(commands.readPreview).mockReturnValue(ok(view({ text: "x" })));

    await controller.activate(ROOT, `${ROOT}/a.txt`);
    controller.forget(`${ROOT}/a.txt`);
    await controller.activate(ROOT, `${ROOT}/a.txt`);

    expect(commands.readPreview).toHaveBeenCalledTimes(2);
    controller.unmount();
  });
});

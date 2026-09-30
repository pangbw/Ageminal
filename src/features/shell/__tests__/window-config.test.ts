import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

/**
 * 主窗口的创建期契约（REQUIREMENTS.md §6 / §12；#15 决议：自绘标题栏）。
 *
 * 这些值只能在 `tauri.conf.json` 里生效，前端测不到「窗口真的长这样」；
 * 但**配置漂移**测得出来——最小尺寸与无边框是两条明确的验收项
 * （900×600 生效、自绘标题栏），误删了得当场变红。
 */
type WindowConfig = {
  label: string;
  minWidth: number;
  minHeight: number;
  decorations: boolean;
};

function mainWindow(): WindowConfig {
  const file = join(process.cwd(), "src-tauri", "tauri.conf.json");
  const config = JSON.parse(readFileSync(file, "utf8")) as {
    app: { windows: WindowConfig[] };
  };

  const window = config.app.windows[0];
  if (!window) throw new Error(`no window configured in ${file}`);
  return window;
}

describe("main window config", () => {
  it("keeps the 900x600 minimum size", () => {
    expect(mainWindow().minWidth).toBe(900);
    expect(mainWindow().minHeight).toBe(600);
  });

  it("draws its own title bar", () => {
    // 关掉系统装饰，标题栏由外壳自绘（#15；capabilities 里放开窗口命令）。
    expect(mainWindow().decorations).toBe(false);
  });

  it("is the window the capabilities target", () => {
    const capabilities = JSON.parse(
      readFileSync(join(process.cwd(), "src-tauri", "capabilities", "default.json"), "utf8"),
    ) as { windows: string[] };

    expect(capabilities.windows).toContain(mainWindow().label);
  });
});

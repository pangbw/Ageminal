/* global after, describe, it, browser, $, $$, expect, process */
import { execFileSync } from "node:child_process";

after(() => {
  // #76 的关窗会隐藏到托盘；测试结束后显式清理测试进程，避免 CI worker 挂住。
  if (process.platform !== "win32") return;
  for (const image of ["ageminal-desktop.exe", "ageminal-daemon.exe"]) {
    try {
      execFileSync("taskkill", ["/IM", image, "/F"], { stdio: "ignore" });
    } catch {
      // 进程可能已被 service 清理。
    }
  }
});

describe("Ageminal e2e smoke", () => {
  it("starts, opens a terminal tab and closes the window", async () => {
    const app = await $(".app");
    await app.waitForExist({ timeout: 30_000 });

    const info = await browser.tauri.execute(({ core }) => core.invoke("app_info"));
    expect(info.name).toBe("Ageminal");
    expect(info.os).toBe("windows");

    // 用户工作区始终存在，避免依赖本机项目状态。
    const workspace = await $(".user-workspace .row");
    await workspace.waitForClickable({ timeout: 10_000 });
    await workspace.click();

    await $(".tabbar__new").click();
    const items = await $$(".newtab__item");
    expect(items.length).toBeGreaterThan(0);
    await items[0].click();

    const terminal = await $(".terminal-host");
    await terminal.waitForExist({ timeout: 30_000 });
    expect(await $$(".tab").length).toBeGreaterThan(0);

    // 关窗按钮：应用按 #76 隐藏到托盘；冒烟只验证窗口操作链路。
    const controls = await $$(".window-controls__button");
    expect(controls.length).toBeGreaterThanOrEqual(3);
    await controls[controls.length - 1].click();
    await browser.pause(500);

    // 关窗只会隐藏到托盘，嵌入 WebDriver 也随之消失；跳过 runner 对已消失
    // session 的 DELETE，让 after 统一清理测试进程。
    browser.sessionId = null;
  });
});

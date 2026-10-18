// WebdriverIO + @wdio/tauri-service（embedded provider）冒烟配置（#92）。
// 先运行 `pnpm e2e:build` 生成带 e2e 插件与 WDIO 前端插件的 debug 应用。
import path from "node:path";

const appBinary = path.resolve("target/debug/ageminal-desktop.exe");

export const config = {
  runner: "local",
  specs: ["./test/e2e/**/*.e2e.js"],
  maxInstances: 1,
  services: [
    [
      "@wdio/tauri-service",
      {
        appBinaryPath: appBinary,
        driverProvider: "embedded",
        startTimeout: 120_000,
        commandTimeout: 60_000,
        captureBackendLogs: false,
        captureFrontendLogs: false,
        logLevel: "warn",
      },
    ],
  ],
  capabilities: [
    {
      browserName: "tauri",
      "tauri:options": {
        application: appBinary,
      },
    },
  ],
  logLevel: "warn",
  bail: 0,
  waitforTimeout: 20_000,
  connectionRetryTimeout: 120_000,
  connectionRetryCount: 1,
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: {
    ui: "bdd",
    timeout: 120_000,
  },
};

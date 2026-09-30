import { fileURLToPath } from "node:url";

import VueI18nPlugin from "@intlify/unplugin-vue-i18n/vite";
import vue from "@vitejs/plugin-vue";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [
    vue(),
    // locale JSON 在构建期预编译（REQUIREMENTS.md §11），运行时不再编译消息。
    VueI18nPlugin({
      include: fileURLToPath(new URL("./src/i18n/locales/**", import.meta.url)),
    }),
  ],
  // 让 Rust 侧的编译错误不被 Vite 清屏冲掉。
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  test: {
    environment: "happy-dom",
    include: ["src/**/*.test.ts"],
  },
});

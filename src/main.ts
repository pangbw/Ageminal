import { createPinia } from "pinia";
import { createApp } from "vue";

import App from "./App.vue";
import { applyLocale, i18n, resolveInitialLocale } from "./i18n";
import "./styles/base.css";

async function bootstrap() {
  // 语言定下来之前先别挂载：否则会闪一屏别的语言。
  applyLocale(await resolveInitialLocale());

  // E2E 构建才加载 WDIO 前端插件；生产包不引入。
  if (import.meta.env.VITE_E2E === "1") {
    await import("@wdio/tauri-plugin");
  }

  createApp(App).use(createPinia()).use(i18n).mount("#app");
}

void bootstrap();

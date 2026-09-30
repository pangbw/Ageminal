import { createApp } from "vue";

import App from "./App.vue";
import { applyLocale, i18n, resolveInitialLocale } from "./i18n";
import "./styles/base.css";

async function bootstrap() {
  // 语言定下来之前先别挂载：否则会闪一屏别的语言。
  applyLocale(await resolveInitialLocale());

  createApp(App).use(i18n).mount("#app");
}

void bootstrap();

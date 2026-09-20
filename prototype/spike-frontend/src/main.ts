import { createApp } from "vue";
import App from "./App.vue";
import { mark, recordError } from "./timing";

window.addEventListener("error", (e) => recordError(`window.error: ${e.message}`));
window.addEventListener("unhandledrejection", (e) => recordError(`unhandledrejection: ${String(e.reason)}`));

const app = createApp(App);
app.config.errorHandler = (err) => recordError(`vue.errorHandler: ${String(err)}`);

mark("beforeMount");
app.mount("#app");
mark("mounted");

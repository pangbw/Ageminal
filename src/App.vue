<script setup lang="ts">
import { onMounted, ref } from "vue";

import { commands, type AppInfo } from "./bindings";
import { translate as t } from "./i18n/translate";

const info = ref<AppInfo | null>(null);
const failure = ref<string | null>(null);

onMounted(async () => {
  try {
    info.value = await commands.appInfo();
  } catch (error) {
    failure.value = error instanceof Error ? error.message : String(error);
  }
});
</script>

<template>
  <main class="shell">
    <h1 class="shell__title">
      {{ t("common.appName") }}
    </h1>
    <p
      v-if="failure"
      class="shell__error"
    >
      {{ t("shell.appInfo.error", { message: failure }) }}
    </p>
    <p
      v-else-if="info"
      class="shell__lead"
    >
      {{ info.name }} {{ info.version }} · {{ info.os }}
    </p>
    <p
      v-else
      class="shell__lead"
    >
      {{ t("shell.appInfo.loading") }}
    </p>
    <p class="shell__hint">
      {{ t("shell.next.hint") }}
    </p>
  </main>
</template>

<style scoped>
.shell {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  height: 100%;
  gap: 8px;
}

.shell__title {
  margin: 0;
  font-size: 28px;
  font-weight: 600;
  letter-spacing: 0.02em;
}

.shell__lead {
  margin: 0;
  color: var(--fg);
}

.shell__error {
  margin: 0;
  color: #e06c75;
}

.shell__hint {
  margin: 0;
  color: var(--fg-muted);
  font-size: 13px;
}
</style>

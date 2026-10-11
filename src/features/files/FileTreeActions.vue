<script setup lang="ts">
/**
 * 右栏面板头的两个可见性开关（§9 / #18 D3）。
 *
 * Rust 已经把标记带回（#58），这里只切前端开关：不重扫目录，只换过滤。
 */
import IconEye from "~icons/codicon/eye";
import IconEyeClosed from "~icons/codicon/eye-closed";
import IconFilter from "~icons/codicon/filter";
import IconFilterFilled from "~icons/codicon/filter-filled";

import { translate as t } from "../../i18n/translate";
import { useUiStore } from "../../stores/ui";

const ui = useUiStore();
</script>

<template>
  <button
    class="icon-button visibility-toggle"
    :class="{ 'visibility-toggle--on': ui.showHidden }"
    type="button"
    :title="t('files.visibility.showHidden')"
    :aria-pressed="ui.showHidden"
    @click="ui.toggleShowHidden()"
  >
    <IconEye v-if="ui.showHidden" />
    <IconEyeClosed v-else />
  </button>
  <button
    class="icon-button visibility-toggle"
    :class="{ 'visibility-toggle--on': ui.showIgnored }"
    type="button"
    :title="t('files.visibility.showIgnored')"
    :aria-pressed="ui.showIgnored"
    @click="ui.toggleShowIgnored()"
  >
    <IconFilterFilled v-if="ui.showIgnored" />
    <IconFilter v-else />
  </button>
</template>

<style scoped>
.visibility-toggle--on {
  color: var(--color-accent);
}
</style>

<script setup lang="ts">
/**
 * 自绘标题栏（#15 决议：`decorations: false` + `data-tauri-drag-region`）。
 *
 * 左侧是页签区（内容由 TabBar 注入），右侧是设置入口与窗口按钮。
 *
 * 拖拽区**逐元素标注**（标题栏本身 + TabBar 的条与空白区），而不是用
 * `data-tauri-drag-region="deep"` 一把罩住整条：`deep` 会把「非交互元素」也算作
 * 拖拽区，等 #69 把页签做成 `div` 时就成了坑。标注在空白容器上，页签（无论什么
 * 元素）永远只是「子元素」，不会触发拖动。Tauri 的 drag 脚本也不认领按钮 / 输入框。
 */
import IconSettingsGear from "~icons/codicon/settings-gear";

import { translate as t } from "../../i18n/translate";
import WindowControls from "./WindowControls.vue";

const emit = defineEmits<{ settings: [] }>();
</script>

<template>
  <header
    class="titlebar"
    data-tauri-drag-region="true"
  >
    <div class="titlebar__tabs">
      <slot />
    </div>
    <div class="titlebar__actions">
      <button
        class="icon-button titlebar__button"
        type="button"
        :title="t('shell.settings')"
        @click="emit('settings')"
      >
        <IconSettingsGear />
      </button>
      <WindowControls />
    </div>
  </header>
</template>

<style scoped>
.titlebar {
  display: flex;
  align-items: stretch;
  flex: 0 0 var(--titlebar-height);
  height: var(--titlebar-height);
  border-bottom: 1px solid var(--color-border);
  user-select: none;
}

.titlebar__tabs {
  display: flex;
  align-items: stretch;
  flex: 1 1 auto;
  min-width: 0;
}

.titlebar__actions {
  display: flex;
  align-items: stretch;
  flex: 0 0 auto;
  margin-left: auto;
}

.titlebar__button {
  width: 36px;
  height: auto;
  border-radius: 0;
  color: var(--color-muted);
  font-size: 15px;
}
</style>

<script setup lang="ts">
/**
 * 预览面板占位（#59）。
 *
 * 这里只证明「单击文件 → 触发预览页签」这条路打通；真正的只读 CodeMirror 预览见 #60，
 * 完整页签模型（分组 / 拖拽 / 菜单）见 #69。
 */
import IconClose from "~icons/codicon/close";
import IconFile from "~icons/codicon/file";

import { translate as t } from "../../i18n/translate";
import type { PreviewTab } from "../../stores/tabs";

defineProps<{ tab: PreviewTab }>();
const emit = defineEmits<{ close: [] }>();
</script>

<template>
  <div class="preview">
    <header class="preview__head">
      <IconFile class="preview__icon" />
      <span
        class="preview__name"
        :title="tab.path"
      >{{ tab.name }}</span>
      <button
        class="icon-button preview__close"
        type="button"
        :title="t('tabs.close')"
        @click="emit('close')"
      >
        <IconClose />
      </button>
    </header>
    <div class="preview__body">
      <p class="preview__note">
        {{ t("tabs.previewComingSoon") }}
      </p>
      <p class="preview__path">
        {{ tab.path }}
      </p>
    </div>
  </div>
</template>

<style scoped>
.preview {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  min-height: 0;
}

.preview__head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex: 0 0 var(--pane-head-height);
  height: var(--pane-head-height);
  padding: 0 var(--space-4);
  border-bottom: 1px solid var(--color-border-soft);
  color: var(--color-muted);
  font-size: var(--font-size-sm);
}

.preview__icon {
  flex: 0 0 auto;
  color: var(--color-faint);
}

.preview__name {
  flex: 1 1 auto;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--color-text);
}

.preview__close {
  flex: 0 0 auto;
}

.preview__body {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--space-2);
  min-height: 0;
  padding: var(--space-6);
  color: var(--color-faint);
}

.preview__note {
  margin: 0;
  font-size: var(--font-size-sm);
}

.preview__path {
  margin: 0;
  max-width: 100%;
  overflow: hidden;
  font-family: var(--font-mono, monospace);
  font-size: var(--font-size-xs);
  text-overflow: ellipsis;
  white-space: nowrap;
}
</style>

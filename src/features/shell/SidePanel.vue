<script setup lang="ts">
/**
 * 侧栏面板（左项目栏 / 右文件树）的容器：宽度、头部、折叠按钮。
 *
 * 内容由使用方通过插槽注入——左栏内容归 #57（项目 / worktree 树），
 * 右栏内容归 #58 / #59（忽略过滤、懒加载树）。
 */
import IconChevronLeft from "~icons/codicon/chevron-left";
import IconChevronRight from "~icons/codicon/chevron-right";

import type { PanelSide } from "../../lib/panel-layout";

const props = defineProps<{
  side: PanelSide;
  /** 面板宽度（像素）；由 `useUiStore` 持有，拖拽 / 方向键改它。 */
  width: number;
  /** 折叠按钮的无障碍标题。 */
  collapseLabel: string;
}>();

const emit = defineEmits<{ collapse: [] }>();
</script>

<template>
  <aside
    class="pane"
    :class="`pane--${props.side}`"
    :style="{ width: `${props.width}px` }"
  >
    <div class="pane__head">
      <slot name="head" />
      <div class="pane__actions">
        <slot name="actions" />
        <button
          class="icon-button pane__collapse"
          type="button"
          :title="props.collapseLabel"
          @click="emit('collapse')"
        >
          <IconChevronLeft v-if="props.side === 'left'" />
          <IconChevronRight v-else />
        </button>
      </div>
    </div>
    <div class="pane__body">
      <slot />
    </div>
  </aside>
</template>

<style scoped>
.pane {
  display: flex;
  flex-direction: column;
  flex: 0 0 auto;
  min-height: 0;
}

.pane__head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex: 0 0 var(--pane-head-height);
  height: var(--pane-head-height);
  padding: 0 var(--space-4) 0 var(--space-5);
  border-bottom: 1px solid var(--color-border-soft);
  color: var(--color-muted);
  font-size: var(--font-size-xs);
  font-weight: 600;
  letter-spacing: 0.05em;
}

.pane__actions {
  display: flex;
  align-items: center;
  gap: var(--space-1);
}

.pane__body {
  flex: 1 1 auto;
  min-height: 0;
  overflow: auto;
}
</style>

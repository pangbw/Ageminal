<script setup lang="ts">
/**
 * 折叠后的侧栏：一条竖排细条，点任意位置展开（原型 `prototype/index.html` 的 `.rail`）。
 */
import IconChevronLeft from "~icons/codicon/chevron-left";
import IconChevronRight from "~icons/codicon/chevron-right";

import type { PanelSide } from "../../lib/panel-layout";

const props = defineProps<{
  side: PanelSide;
  /** 竖排显示的面板名。 */
  label: string;
  /** 展开按钮的无障碍标题。 */
  expandLabel: string;
}>();

const emit = defineEmits<{ expand: [] }>();
</script>

<template>
  <button
    class="rail"
    :class="`rail--${props.side}`"
    type="button"
    :title="props.expandLabel"
    @click="emit('expand')"
  >
    <IconChevronRight v-if="props.side === 'left'" />
    <span class="rail__label">{{ props.label }}</span>
    <IconChevronLeft v-if="props.side === 'right'" />
  </button>
</template>

<style scoped>
.rail {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--space-2);
  flex: 0 0 var(--rail-width);
  width: var(--rail-width);
  padding-top: var(--space-3);
  color: var(--color-faint);
  font-size: 14px;
}

.rail--left {
  border-right: 1px solid var(--color-border-soft);
}

.rail--right {
  border-left: 1px solid var(--color-border-soft);
}

.rail:hover {
  color: var(--color-text);
}

.rail__label {
  margin-top: var(--space-3);
  font-size: 10.5px;
  letter-spacing: 0.14em;
  text-transform: uppercase;
  writing-mode: vertical-rl;
}
</style>

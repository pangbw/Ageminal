<script setup lang="ts">
/**
 * 空态（原型 `.other-pane`）：一个弱化图标 + 一句说明，居中占满主区。
 *
 * 文案由使用方传入（`t(...)`），图标可选——后续 #57 的「还没有添加任何项目」也复用它。
 */
import type { Component } from "vue";

defineProps<{ label: string; icon?: Component }>();
</script>

<template>
  <div class="empty-state">
    <component
      :is="icon"
      v-if="icon"
      class="empty-state__icon"
    />
    <p class="empty-state__label">
      {{ label }}
    </p>
  </div>
</template>

<style scoped>
.empty-state {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--space-3);
  color: var(--color-faint);
}

.empty-state__icon {
  font-size: 26px;
}

.empty-state__label {
  margin: 0;
}
</style>

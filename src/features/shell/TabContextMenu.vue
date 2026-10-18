<script setup lang="ts">
/**
 * 页签右键菜单（#69）：关闭 / 关闭其他 / 关闭右侧。
 *
 * 与文件上下文菜单同一套呈现约定：固定定位、贴边回拉、点背景或 Esc 关闭。
 */
import { computed, onBeforeUnmount, onMounted, ref } from "vue";

import { translate as t } from "../../i18n/translate";

type TabMenuAction = "close" | "closeOthers" | "closeRight";

const props = defineProps<{
  x: number;
  y: number;
  canCloseOthers: boolean;
  canCloseRight: boolean;
}>();

const emit = defineEmits<{ action: [action: TabMenuAction]; close: [] }>();

const MARGIN = 8;
const menuEl = ref<HTMLElement | null>(null);
const left = ref(props.x);
const top = ref(props.y);
const placed = ref(false);

const items = computed<
  { id: TabMenuAction; label: string; disabled: boolean; danger?: boolean }[]
>(() => [
  { id: "close", label: t("tabs.context.close"), disabled: false, danger: true },
  { id: "closeOthers", label: t("tabs.context.closeOthers"), disabled: !props.canCloseOthers },
  { id: "closeRight", label: t("tabs.context.closeRight"), disabled: !props.canCloseRight },
]);

function choose(action: TabMenuAction): void {
  emit("action", action);
  emit("close");
}

function onKey(event: KeyboardEvent): void {
  if (event.key === "Escape") emit("close");
}

onMounted(() => {
  window.addEventListener("keydown", onKey);
  const menu = menuEl.value;
  if (menu) {
    const rect = menu.getBoundingClientRect();
    left.value = Math.max(MARGIN, Math.min(props.x, window.innerWidth - rect.width - MARGIN));
    top.value = Math.max(MARGIN, Math.min(props.y, window.innerHeight - rect.height - MARGIN));
  }
  placed.value = true;
});
onBeforeUnmount(() => window.removeEventListener("keydown", onKey));
</script>

<template>
  <div class="tabmenu">
    <div
      class="tabmenu__backdrop"
      @click="emit('close')"
      @contextmenu.prevent="emit('close')"
    />
    <div
      ref="menuEl"
      class="tabmenu__menu"
      role="menu"
      :style="{ left: `${left}px`, top: `${top}px`, visibility: placed ? 'visible' : 'hidden' }"
    >
      <button
        v-for="item in items"
        :key="item.id"
        class="tabmenu__item"
        :class="{ 'tabmenu__item--danger': item.danger }"
        type="button"
        role="menuitem"
        :disabled="item.disabled"
        @click="choose(item.id)"
      >
        {{ item.label }}
      </button>
    </div>
  </div>
</template>

<style scoped>
.tabmenu__backdrop {
  position: fixed;
  inset: 0;
  z-index: 190;
}

.tabmenu__menu {
  position: fixed;
  z-index: 191;
  min-width: 170px;
  padding: var(--space-1) 0;
  border: 1px solid var(--color-border);
  border-radius: var(--radius-md);
  background: var(--color-elevated);
}

.tabmenu__item {
  display: block;
  width: 100%;
  padding: var(--space-2) var(--space-5);
  color: var(--color-text);
  font-size: var(--font-size-sm);
  text-align: left;
  cursor: pointer;
}

.tabmenu__item:hover:not(:disabled) {
  background: var(--color-hover);
}

.tabmenu__item:disabled {
  color: var(--color-faint);
  cursor: default;
}

.tabmenu__item--danger {
  color: var(--color-red);
}
</style>

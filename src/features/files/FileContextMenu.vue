<script setup lang="ts">
/**
 * 文件树右键菜单（#62 / §9）。
 *
 * 纯呈现：按目标（文件 / 目录）算出条目列表，点击只 `emit('action', id)`，实体操作留给
 * `FileTree`。分组用**弱分隔线**（§9：`|` 表示分组）；「删除」红色强调。
 */
import { computed, onBeforeUnmount, onMounted, ref } from "vue";

import { translate as t } from "../../i18n/translate";

/** 右键菜单的动作 id。 */
export type ContextAction =
  | "newFile"
  | "newFolder"
  | "rename"
  | "reveal"
  | "terminal"
  | "copyFile"
  | "copyPath"
  | "copyRelative"
  | "delete";

const props = defineProps<{
  x: number;
  y: number;
  target: { path: string; name: string; isDir: boolean };
}>();

const emit = defineEmits<{ action: [action: ContextAction]; close: [] }>();

/** 贴近视口边缘时把菜单拉回可见范围（8px 边距）。 */
const MARGIN = 8;

const menuEl = ref<HTMLElement | null>(null);
const left = ref(props.x);
const top = ref(props.y);
/** 量过尺寸前先隐藏，避免越界位置闪一下。 */
const placed = ref(false);

type Item = {
  id: ContextAction;
  label: string;
  /** 弱分组的第一项：上方加分隔线。 */
  groupStart?: boolean;
  danger?: boolean;
};

const items = computed<Item[]>(() => [
  { id: "newFile", label: t("files.context.newFile") },
  { id: "newFolder", label: t("files.context.newFolder") },
  { id: "rename", label: t("files.context.rename"), groupStart: true },
  { id: "reveal", label: t("files.context.reveal") },
  { id: "terminal", label: t("files.context.openTerminal") },
  { id: "copyFile", label: t("files.context.copyFile"), groupStart: true },
  { id: "copyPath", label: t("files.context.copyPath") },
  { id: "copyRelative", label: t("files.context.copyRelative") },
  { id: "delete", label: t("files.context.delete"), groupStart: true, danger: true },
]);

function choose(action: ContextAction): void {
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
    left.value = Math.max(
      MARGIN,
      Math.min(props.x, window.innerWidth - rect.width - MARGIN),
    );
    top.value = Math.max(
      MARGIN,
      Math.min(props.y, window.innerHeight - rect.height - MARGIN),
    );
  }
  placed.value = true;
});
onBeforeUnmount(() => window.removeEventListener("keydown", onKey));
</script>

<template>
  <div class="ctx">
    <div
      class="ctx__backdrop"
      @click="emit('close')"
      @contextmenu.prevent="emit('close')"
    />
    <div
      ref="menuEl"
      class="ctx__menu"
      role="menu"
      :style="{ left: `${left}px`, top: `${top}px`, visibility: placed ? 'visible' : 'hidden' }"
    >
      <template
        v-for="item in items"
        :key="item.id"
      >
        <div
          v-if="item.groupStart"
          class="ctx__sep"
        />
        <button
          class="ctx__item"
          :class="{ 'ctx__item--danger': item.danger }"
          type="button"
          role="menuitem"
          @click="choose(item.id)"
        >
          {{ item.label }}
        </button>
      </template>
    </div>
  </div>
</template>

<style scoped>
.ctx__backdrop {
  position: fixed;
  inset: 0;
  z-index: 190;
}

.ctx__menu {
  position: fixed;
  z-index: 191;
  min-width: 190px;
  max-height: calc(100vh - 16px);
  overflow: auto;
  padding: var(--space-1) 0;
  border: 1px solid var(--color-border);
  border-radius: var(--radius-md);
  background: var(--color-elevated);
}

.ctx__sep {
  height: 1px;
  margin: var(--space-1) 0;
  background: var(--color-border-soft);
}

.ctx__item {
  display: block;
  width: 100%;
  padding: var(--space-2) var(--space-5);
  color: var(--color-text);
  font-size: var(--font-size-sm);
  text-align: left;
  cursor: pointer;
}

.ctx__item:hover {
  background: var(--color-hover);
}

.ctx__item--danger {
  color: var(--color-red);
}

.ctx__item--danger:hover {
  background: var(--color-close-hover);
  color: var(--color-on-danger);
}
</style>

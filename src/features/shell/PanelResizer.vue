<script setup lang="ts">
/**
 * 三栏之间的拖拽分隔条（§6：三栏可拖拽调宽）。
 *
 * 自建（C5 #13 D5：不引 splitpanes）。拖拽上下限与步长全在 `lib/panel-layout`，
 * 这里只负责把鼠标 / 键盘动作翻译成新宽度。
 */
import { onBeforeUnmount, ref } from "vue";

import { translate as t } from "../../i18n/translate";
import {
  PANEL_MAX_WIDTH,
  PANEL_MIN_WIDTH,
  panelWidthAfterDrag,
  panelWidthAfterStep,
  type PanelSide,
} from "../../lib/panel-layout";
import { useUiStore } from "../../stores/ui";

const props = defineProps<{ side: PanelSide }>();

const ui = useUiStore();

const dragging = ref(false);
let startX = 0;
let startWidth = 0;

function onDragMove(event: MouseEvent): void {
  ui.setPanelWidth(props.side, panelWidthAfterDrag(props.side, startWidth, event.clientX - startX));
}

function stopDrag(): void {
  if (!dragging.value) return;

  dragging.value = false;
  document.body.classList.remove("is-resizing");
  window.removeEventListener("mousemove", onDragMove);
  window.removeEventListener("mouseup", stopDrag);
}

function startDrag(event: MouseEvent): void {
  event.preventDefault();

  dragging.value = true;
  startX = event.clientX;
  startWidth = ui.panelWidth(props.side);
  document.body.classList.add("is-resizing");
  window.addEventListener("mousemove", onDragMove);
  window.addEventListener("mouseup", stopDrag);
}

function onKeydown(event: KeyboardEvent): void {
  if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;

  event.preventDefault();
  const direction = event.key === "ArrowRight" ? 1 : -1;
  ui.setPanelWidth(
    props.side,
    panelWidthAfterStep(props.side, ui.panelWidth(props.side), direction),
  );
}

// 拖拽中途组件被卸载（切 worktree、折叠）时别把监听器留在 window 上。
onBeforeUnmount(stopDrag);
</script>

<template>
  <div
    class="resizer"
    :class="[`resizer--${props.side}`, { 'resizer--dragging': dragging }]"
    role="separator"
    aria-orientation="vertical"
    tabindex="0"
    :aria-label="t('shell.panelResizer')"
    :aria-valuenow="ui.panelWidth(props.side)"
    :aria-valuemin="PANEL_MIN_WIDTH"
    :aria-valuemax="PANEL_MAX_WIDTH"
    @mousedown="startDrag"
    @keydown="onKeydown"
  />
</template>

<style scoped>
.resizer {
  position: relative;
  z-index: 20;
  flex: 0 0 var(--resizer-width);
  width: var(--resizer-width);
  cursor: col-resize;
}

.resizer::after {
  content: "";
  position: absolute;
  top: 0;
  bottom: 0;
  left: 1px;
  right: 1px;
  background: var(--color-border-soft);
  transition: background 0.12s;
}

.resizer:hover::after,
.resizer--dragging::after,
.resizer:focus-visible::after {
  background: var(--color-accent);
}

.resizer:focus-visible {
  outline: none;
}
</style>

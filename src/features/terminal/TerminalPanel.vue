<script setup lang="ts">
/**
 * 终端面板（#68）。
 *
 * 只负责 DOM 宿主、尺寸观察与状态呈现；xterm 实例 / Channel / ack 都在 `runtime.ts`。
 * 面板挂载 = 会话可见；卸载 = `Detach`（会话继续活在 daemon 里，切回时 `Attach` 续流）。
 */
import IconClose from "~icons/codicon/close";
import IconSparkle from "~icons/codicon/sparkle";
import IconTerminal from "~icons/codicon/terminal";
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from "vue";

import { translate as t } from "../../i18n/translate";
import type { TerminalTab } from "../../stores/tabs";
import {
  clearTerminalSearch,
  copySelection,
  findInTerminal,
  fitTerm,
  hasSelection,
  hide,
  mountTerm,
  onSearchRequest,
  pasteClipboard,
  resize,
  selectAll,
  show,
  terminalState,
} from "./runtime";

/** resize 去抖（REQUIREMENTS.md §7.2）。 */
const RESIZE_DEBOUNCE_MS = 120;

const props = defineProps<{ tab: TerminalTab }>();
const emit = defineEmits<{ close: [] }>();

const host = ref<HTMLElement | null>(null);
const state = computed(() => terminalState(props.tab.sid));

let observer: ResizeObserver | null = null;
let timer: ReturnType<typeof setTimeout> | undefined;
let lastCols = 0;
let lastRows = 0;
let disposed = false;
let searchDispose: { dispose(): void } | null = null;

const searchOpen = ref(false);
const searchQuery = ref("");
const searchInput = ref<HTMLInputElement | null>(null);
const menu = ref<{ x: number; y: number } | null>(null);

function syncSize(): void {
  if (disposed || !statesVisible()) return;
  const element = host.value;
  if (!element || element.clientWidth === 0 || element.clientHeight === 0) return;
  const { cols, rows } = fitTerm(props.tab.sid);
  if (cols === lastCols && rows === lastRows) return;
  lastCols = cols;
  lastRows = rows;
  resize(props.tab.sid, cols, rows);
}

function openSearch(): void {
  searchOpen.value = true;
  void nextTick(() => searchInput.value?.focus());
}

function closeSearch(): void {
  searchOpen.value = false;
  searchQuery.value = "";
  clearTerminalSearch(props.tab.sid);
}

function runSearch(backwards = false): void {
  if (!searchQuery.value) {
    clearTerminalSearch(props.tab.sid);
    return;
  }
  findInTerminal(props.tab.sid, searchQuery.value, backwards);
}

function onSearchKey(event: KeyboardEvent): void {
  if (event.key === "Escape") {
    event.preventDefault();
    closeSearch();
    return;
  }
  if (event.key === "Enter") {
    event.preventDefault();
    runSearch(event.shiftKey);
  }
}

function closeMenu(): void {
  menu.value = null;
}

function onContextMenu(event: MouseEvent): void {
  event.preventDefault();
  if (hasSelection(props.tab.sid)) {
    copySelection(props.tab.sid);
    return;
  }
  menu.value = {
    x: Math.min(event.clientX, window.innerWidth - 150),
    y: Math.min(event.clientY, window.innerHeight - 90),
  };
}

function onPaste(): void {
  pasteClipboard(props.tab.sid);
  closeMenu();
}

function onSelectAll(): void {
  selectAll(props.tab.sid);
  closeMenu();
}

function statesVisible(): boolean {
  return terminalState(props.tab.sid)?.visible === true;
}

function scheduleSync(): void {
  clearTimeout(timer);
  timer = setTimeout(syncSize, RESIZE_DEBOUNCE_MS);
}

onMounted(() => {
  const element = host.value;
  if (!element) return;

  searchDispose = onSearchRequest(props.tab.sid, openSearch);
  window.addEventListener("pointerdown", closeMenu);

  const { cols, rows } = mountTerm(props.tab.sid, element);
  lastCols = cols;
  lastRows = rows;

  observer = new ResizeObserver(() => scheduleSync());
  observer.observe(element);

  // 先等一帧让面板拿到最终布局，再建会话（CreateSession 的初始尺寸就是它）。
  requestAnimationFrame(() => {
    if (disposed) return;
    const fitted = fitTerm(props.tab.sid);
    lastCols = fitted.cols;
    lastRows = fitted.rows;
    // 失败已经写进 runtime 状态，面板会显示。
    void show(props.tab.sid, props.tab.cwd, props.tab.agentId)
      .then(() => {
        if (!disposed) resize(props.tab.sid, lastCols, lastRows);
      })
      .catch(() => undefined);
  });
});

onBeforeUnmount(() => {
  disposed = true;
  searchDispose?.dispose();
  searchDispose = null;
  window.removeEventListener("pointerdown", closeMenu);
  clearTimeout(timer);
  observer?.disconnect();
  observer = null;
  hide(props.tab.sid);
});
</script>

<template>
  <div class="terminal">
    <header class="terminal__head">
      <IconSparkle
        v-if="tab.agentId"
        class="terminal__icon"
        :style="{ color: tab.agentColor }"
      />
      <IconTerminal
        v-else
        class="terminal__icon"
      />
      <span
        class="terminal__name"
        :title="tab.cwd"
      >{{ tab.name }}</span>
      <span
        v-if="state?.reconnected"
        class="terminal__note terminal__note--reconnected"
      >{{ t("terminal.reconnected") }}</span>
      <span
        v-else-if="state?.status === 'starting'"
        class="terminal__note"
      >{{ t("terminal.starting") }}</span>
      <span
        v-else-if="state?.status === 'exited'"
        class="terminal__note"
      >
        {{
          state.exitCode === null
            ? t("terminal.exited")
            : t("terminal.exitedWithCode", { code: state.exitCode })
        }}
      </span>
      <button
        class="icon-button terminal__action"
        type="button"
        :title="t('terminal.close')"
        @click="emit('close')"
      >
        <IconClose />
      </button>
    </header>

    <div
      v-if="searchOpen"
      class="terminal__search"
    >
      <input
        ref="searchInput"
        v-model="searchQuery"
        class="terminal__search-input"
        type="text"
        :placeholder="t('terminal.searchPlaceholder')"
        :aria-label="t('terminal.searchPlaceholder')"
        @input="runSearch(false)"
        @keydown="onSearchKey"
      >
      <button
        class="icon-button"
        type="button"
        :title="t('terminal.searchPrevious')"
        :aria-label="t('terminal.searchPrevious')"
        @click="runSearch(true)"
      >
        ↑
      </button>
      <button
        class="icon-button"
        type="button"
        :title="t('terminal.searchNext')"
        :aria-label="t('terminal.searchNext')"
        @click="runSearch(false)"
      >
        ↓
      </button>
      <button
        class="icon-button"
        type="button"
        :title="t('terminal.searchClose')"
        :aria-label="t('terminal.searchClose')"
        @click="closeSearch"
      >
        ×
      </button>
    </div>

    <p
      v-if="state?.error"
      class="terminal__error"
      role="alert"
    >
      {{ t("terminal.error") }}: {{ state.error }}
    </p>

    <div
      ref="host"
      class="terminal__host"
      @contextmenu.prevent="onContextMenu"
    />

    <div
      v-if="menu"
      class="terminal__menu"
      role="menu"
      :style="{ left: `${menu.x}px`, top: `${menu.y}px` }"
      @pointerdown.stop
    >
      <button
        class="terminal__menu-item"
        type="button"
        role="menuitem"
        @click="onPaste"
      >
        {{ t("terminal.paste") }}
      </button>
      <button
        class="terminal__menu-item"
        type="button"
        role="menuitem"
        @click="onSelectAll"
      >
        {{ t("terminal.selectAll") }}
      </button>
    </div>
  </div>
</template>

<style scoped>
.terminal {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  min-height: 0;
  background: #080909;
}

.terminal__head {
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

.terminal__icon {
  flex: 0 0 auto;
}

.terminal__name {
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
  color: var(--color-text);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.terminal__note {
  flex: 0 0 auto;
  color: var(--color-faint);
  font-size: var(--font-size-xs);
}

.terminal__note--reconnected {
  color: var(--color-accent);
}

.terminal__action {
  flex: 0 0 auto;
  width: 24px;
  height: 24px;
}

.terminal__search {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-2) var(--space-3);
  border-bottom: 1px solid var(--color-border-soft);
  background: var(--color-bg);
}

.terminal__search-input {
  flex: 1 1 auto;
  min-width: 0;
  padding: var(--space-1) var(--space-2);
  border: 1px solid var(--color-accent-line);
  border-radius: var(--radius-sm);
  background: var(--color-bg);
  color: var(--color-text);
  font: inherit;
}

.terminal__search .icon-button {
  width: 22px;
  height: 22px;
  font-size: var(--font-size-sm);
}

.terminal__menu {
  position: fixed;
  z-index: 30;
  min-width: 140px;
  padding: var(--space-1);
  border: 1px solid var(--color-border);
  border-radius: var(--radius-sm);
  background: var(--color-bg);
  box-shadow: 0 10px 28px rgb(0 0 0 / 45%);
}

.terminal__menu-item {
  display: block;
  width: 100%;
  padding: var(--space-2) var(--space-3);
  border-radius: var(--radius-sm);
  color: var(--color-text);
  font-size: var(--font-size-sm);
  text-align: left;
}

.terminal__menu-item:hover {
  background: var(--color-hover);
}

.terminal__error {
  margin: 0;
  padding: var(--space-2) var(--space-4);
  background: var(--color-hover);
  color: var(--color-red);
  font-size: var(--font-size-xs);
}

.terminal__host {
  flex: 1 1 auto;
  min-height: 0;
  padding: var(--space-2) var(--space-3);
}

.terminal__host :deep(.terminal-host) {
  height: 100%;
}

.terminal__host :deep(.xterm) {
  height: 100%;
}
</style>

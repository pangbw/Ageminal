<script setup lang="ts">
/**
 * 终端面板（#68）。
 *
 * 只负责 DOM 宿主、尺寸观察与状态呈现；xterm 实例 / Channel / ack 都在 `runtime.ts`。
 * 面板挂载 = 会话可见；卸载 = `Detach`（会话继续活在 daemon 里，切回时 `Attach` 续流）。
 */
import IconClose from "~icons/codicon/close";
import IconTerminal from "~icons/codicon/terminal";
import { computed, onBeforeUnmount, onMounted, ref } from "vue";

import { translate as t } from "../../i18n/translate";
import type { TerminalTab } from "../../stores/tabs";
import {
  fitTerm,
  hide,
  mountTerm,
  resize,
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
    void show(props.tab.sid, props.tab.cwd)
      .then(() => {
        if (!disposed) resize(props.tab.sid, lastCols, lastRows);
      })
      .catch(() => undefined);
  });
});

onBeforeUnmount(() => {
  disposed = true;
  clearTimeout(timer);
  observer?.disconnect();
  observer = null;
  hide(props.tab.sid);
});
</script>

<template>
  <div class="terminal">
    <header class="terminal__head">
      <IconTerminal class="terminal__icon" />
      <span
        class="terminal__name"
        :title="tab.cwd"
      >{{ tab.name }}</span>
      <span
        v-if="state?.status === 'starting'"
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
    />
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

.terminal__action {
  flex: 0 0 auto;
  width: 24px;
  height: 24px;
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

<script setup lang="ts">
/**
 * 当前 worktree 的页签条（#69）。
 *
 * 分组与激活项的事实来源是 `stores/tabs`：切换 worktree 时本组件只换一组数据，
 * 切回时按 `activeByRoot` 恢复上次激活的页签。
 *
 * 支持拖拽排序、右键关闭 / 关闭其他 / 关闭右侧；关闭运行中的终端统一走
 * `tabs/actions.ts` 的确认流程。终端页签同时显示 agent 状态彩点（#82）。
 */
import IconClose from "~icons/codicon/close";
import IconFile from "~icons/codicon/file";
import IconSparkle from "~icons/codicon/sparkle";
import IconTerminal from "~icons/codicon/terminal";
import { computed, ref, watch } from "vue";

import type { AgentStatusView } from "../../bindings";
import { translate as t } from "../../i18n/translate";
import { useAgentStatusStore } from "../../stores/agentStatus";
import { useTabsStore, type Tab } from "../../stores/tabs";
import { useWorkspaceStore } from "../../stores/workspace";
import {
  requestClose,
  requestCloseOthers,
  requestCloseRight,
} from "../tabs/actions";
import { statusLabelKey, statusTone } from "../agents/status";
import TabContextMenu from "./TabContextMenu.vue";

type TabMenuAction = "close" | "closeOthers" | "closeRight";

const tabs = useTabsStore();
const workspace = useWorkspaceStore();
const agentStatus = useAgentStatusStore();

const root = computed(() => workspace.activePath);
const list = computed(() => (root.value === null ? [] : tabs.tabsFor(root.value)));
const activeId = computed(() => {
  if (root.value === null) return null;
  const active = tabs.activeTab(root.value);
  return active ? tabs.tabId(active) : null;
});

const dragging = ref<string | null>(null);
const dropIndex = ref<number | null>(null);
const menu = ref<{ x: number; y: number; tab: Tab } | null>(null);

watch(root, () => {
  menu.value = null;
  dragging.value = null;
  dropIndex.value = null;
});

const canCloseOthers = computed(() => list.value.length > 1);
const canCloseRight = computed(() => {
  if (!menu.value) return false;
  const index = list.value.findIndex((tab) => tabs.tabId(tab) === tabs.tabId(menu.value!.tab));
  return index !== -1 && index < list.value.length - 1;
});

function statusOf(tab: Tab): AgentStatusView | null {
  return tab.kind === "terminal" ? agentStatus.statusFor(tab.sid) : null;
}

function statusToneOf(tab: Tab): string {
  const status = statusOf(tab);
  return status === null ? "" : `tab__status--${statusTone(status.state)}`;
}

function statusTitleOf(tab: Tab): string {
  const status = statusOf(tab);
  if (status === null) return "";
  const label = t(statusLabelKey(status.state));
  return status.degraded ? t("agents.status.degradedTitle", { state: label }) : label;
}

function activate(tab: Tab): void {
  if (root.value === null) return;
  tabs.setActive(root.value, tabs.tabId(tab));
}

function close(tab: Tab): void {
  if (root.value === null) return;
  requestClose(root.value, [tab]);
}

function openMenu(event: MouseEvent, tab: Tab): void {
  menu.value = { x: event.clientX, y: event.clientY, tab };
}

function onMenuAction(action: TabMenuAction): void {
  const current = menu.value;
  if (!current || root.value === null) return;
  if (action === "close") requestClose(root.value, [current.tab]);
  if (action === "closeOthers") requestCloseOthers(root.value, current.tab);
  if (action === "closeRight") requestCloseRight(root.value, current.tab);
  menu.value = null;
}

function dragStart(event: DragEvent, tab: Tab): void {
  dragging.value = tabs.tabId(tab);
  dropIndex.value = null;
  if (event.dataTransfer) {
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData("text/plain", dragging.value);
  }
}

function dragOver(index: number): void {
  if (dragging.value !== null) dropIndex.value = index;
}

function drop(event: DragEvent, index: number): void {
  event.preventDefault();
  const id = dragging.value ?? event.dataTransfer?.getData("text/plain") ?? null;
  if (root.value !== null && id !== null) tabs.moveTab(root.value, id, index);
  dragging.value = null;
  dropIndex.value = null;
}

function dragEnd(): void {
  dragging.value = null;
  dropIndex.value = null;
}

/** 纵向滚轮映射到页签条的横向滚动（鼠标没有横向滚轮时也能翻）。 */
function onWheel(event: WheelEvent): void {
  if (event.deltaY === 0) return;
  const strip = (event.currentTarget as HTMLElement).closest(".tabbar__strip");
  if (!(strip instanceof HTMLElement)) return;
  strip.scrollLeft += event.deltaY;
  event.preventDefault();
}
</script>

<template>
  <div
    class="tabstrip"
    role="tablist"
    @wheel="onWheel"
  >
    <div
      v-for="(tab, index) in list"
      :key="tabs.tabId(tab)"
      class="tab"
      :class="{
        'tab--active': tabs.tabId(tab) === activeId,
        'tab--dragging': dragging === tabs.tabId(tab),
        'tab--drop': dropIndex === index,
      }"
      role="tab"
      tabindex="0"
      draggable="true"
      :aria-selected="tabs.tabId(tab) === activeId"
      :title="tab.kind === 'preview' ? tab.path : tab.cwd"
      @click="activate(tab)"
      @keydown.enter.prevent="activate(tab)"
      @keydown.space.prevent="activate(tab)"
      @auxclick.middle.prevent="close(tab)"
      @contextmenu.prevent="openMenu($event, tab)"
      @dragstart="dragStart($event, tab)"
      @dragover.prevent="dragOver(index)"
      @drop="drop($event, index)"
      @dragend="dragEnd"
    >
      <IconSparkle
        v-if="tab.kind === 'terminal' && tab.agentId"
        class="tab__icon"
        :style="{ color: tab.agentColor }"
      />
      <IconTerminal
        v-else-if="tab.kind === 'terminal'"
        class="tab__icon"
      />
      <IconFile
        v-else
        class="tab__icon"
      />
      <span
        v-if="statusOf(tab)"
        class="tab__status"
        :class="[
          statusToneOf(tab),
          { 'tab__status--degraded': statusOf(tab)?.degraded },
        ]"
        :title="statusTitleOf(tab)"
        :aria-label="statusTitleOf(tab)"
        role="img"
      />
      <span class="tab__name">{{ tab.name }}</span>
      <button
        class="tab__close"
        type="button"
        :title="t('tabs.close')"
        @click.stop="close(tab)"
      >
        <IconClose />
      </button>
    </div>

    <TabContextMenu
      v-if="menu"
      :x="menu.x"
      :y="menu.y"
      :can-close-others="canCloseOthers"
      :can-close-right="canCloseRight"
      @action="onMenuAction"
      @close="menu = null"
    />
  </div>
</template>

<style scoped>
.tabstrip {
  display: flex;
  align-items: stretch;
  flex: 0 0 auto;
  height: 100%;
  min-width: 0;
}

.tab {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex: 0 1 auto;
  min-width: 72px;
  max-width: 200px;
  padding: 0 var(--space-2) 0 var(--space-4);
  border-right: 1px solid var(--color-border-soft);
  color: var(--color-muted);
  font-size: var(--font-size-sm);
  cursor: default;
  user-select: none;
}

.tab:hover {
  background: var(--color-hover);
}

.tab--active {
  background: var(--color-bg);
  color: var(--color-text);
  box-shadow: inset 0 -2px 0 var(--color-accent);
}

.tab--dragging {
  opacity: 0.45;
}

.tab--drop {
  box-shadow: inset 2px 0 0 var(--color-accent);
}

.tab__icon {
  flex: 0 0 auto;
  color: var(--color-faint);
  font-size: 13px;
}

.tab--active .tab__icon {
  color: var(--color-accent);
}

.tab__status {
  flex: 0 0 auto;
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: currentColor;
}

.tab__status--running {
  color: var(--color-accent);
}

.tab__status--attention {
  color: var(--color-yellow);
}

.tab__status--done {
  color: var(--color-green);
}

.tab__status--error {
  color: var(--color-red);
}

.tab__status--idle {
  color: var(--color-muted);
}

.tab__status--exited {
  color: var(--color-faint);
  background: transparent;
  box-shadow: inset 0 0 0 1px currentColor;
}

.tab__status--degraded {
  background: transparent;
  box-shadow: inset 0 0 0 1px currentColor;
}

.tab__name {
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.tab__close {
  display: flex;
  align-items: center;
  justify-content: center;
  flex: 0 0 auto;
  width: 18px;
  height: 18px;
  border-radius: var(--radius-sm);
  color: var(--color-faint);
  font-size: 12px;
  opacity: 0;
}

.tab:hover .tab__close,
.tab--active .tab__close,
.tab__close:focus-visible {
  opacity: 1;
}

.tab__close:hover {
  background: var(--color-hover);
  color: var(--color-text);
}
</style>

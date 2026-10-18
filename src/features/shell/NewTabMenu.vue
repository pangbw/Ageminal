<script setup lang="ts">
/**
 * 「+」新建页签菜单（#69）。
 *
 * 终端 / 文件预览可用；浏览器与「其他」按 §8 置灰并标「即将推出」。
 * 文件预览走 Rust 侧系统文件选择器（根 = 当前 worktree），取消不建页签。
 */
import IconEllipsis from "~icons/codicon/ellipsis";
import IconFile from "~icons/codicon/file";
import IconGlobe from "~icons/codicon/globe";
import IconSparkle from "~icons/codicon/sparkle";
import IconTerminal from "~icons/codicon/terminal";
import { onMounted, ref } from "vue";

import { commands, type AgentConfig } from "../../bindings";
import { translate as t } from "../../i18n/translate";
import { errorMessage, unwrap } from "../../lib/ipc";
import { basename } from "../../lib/path";
import { useAgentsStore } from "../../stores/agents";
import { openAgentTab, openPreviewTab, openTerminalTab } from "../tabs/actions";

const props = defineProps<{ root: string | null }>();
const emit = defineEmits<{ close: [] }>();

const error = ref<string | null>(null);
const picking = ref(false);
const agents = useAgentsStore();

onMounted(() => {
  void agents.ensureLoaded();
});

function newTerminal(): void {
  if (props.root === null) return;
  openTerminalTab(props.root);
  emit("close");
}

function newAgent(agent: AgentConfig): void {
  if (props.root === null) return;
  openAgentTab(props.root, agent);
  emit("close");
}

async function newPreview(): Promise<void> {
  if (props.root === null || picking.value) return;
  picking.value = true;
  error.value = null;
  try {
    const path = unwrap(await commands.pickFile(props.root));
    if (path !== null) openPreviewTab(props.root, path, basename(path));
    emit("close");
  } catch (failure) {
    error.value = errorMessage(failure);
  } finally {
    picking.value = false;
  }
}
</script>

<template>
  <div class="newtab">
    <div
      class="newtab__backdrop"
      @click="emit('close')"
      @contextmenu.prevent="emit('close')"
    />
    <div
      class="newtab__menu"
      role="menu"
    >
      <button
        class="newtab__item"
        type="button"
        role="menuitem"
        :disabled="root === null"
        @click="newTerminal"
      >
        <IconTerminal />
        <span>{{ t("tabs.menu.terminal") }}</span>
      </button>
      <div class="newtab__divider" />
      <button
        v-for="agent in agents.list"
        :key="agent.id"
        class="newtab__item newtab__item--agent"
        type="button"
        role="menuitem"
        :data-agent-id="agent.id"
        :disabled="root === null"
        @click="newAgent(agent)"
      >
        <IconSparkle :style="{ color: agent.color }" />
        <span>{{ t("agents.launch", { name: agent.name }) }}</span>
      </button>
      <div class="newtab__divider" />
      <button
        class="newtab__item"
        type="button"
        role="menuitem"
        :disabled="root === null || picking"
        @click="newPreview"
      >
        <IconFile />
        <span>{{ t("tabs.menu.file") }}</span>
      </button>
      <button
        class="newtab__item"
        type="button"
        role="menuitem"
        disabled
      >
        <IconGlobe />
        <span>{{ t("tabs.menu.browser") }}</span>
        <span class="newtab__tag">{{ t("settings.comingSoon") }}</span>
      </button>
      <button
        class="newtab__item"
        type="button"
        role="menuitem"
        disabled
      >
        <IconEllipsis />
        <span>{{ t("tabs.menu.other") }}</span>
        <span class="newtab__tag">{{ t("settings.comingSoon") }}</span>
      </button>
      <p
        v-if="error"
        class="newtab__error"
        role="alert"
      >
        {{ error }}
      </p>
    </div>
  </div>
</template>

<style scoped>
.newtab__backdrop {
  position: fixed;
  inset: 0;
  z-index: 190;
}

.newtab__menu {
  position: absolute;
  top: calc(100% + 2px);
  right: 0;
  z-index: 191;
  min-width: 200px;
  padding: var(--space-1) 0;
  border: 1px solid var(--color-border);
  border-radius: var(--radius-md);
  background: var(--color-elevated);
}

.newtab__item {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  width: 100%;
  padding: var(--space-2) var(--space-4);
  color: var(--color-text);
  font-size: var(--font-size-sm);
  text-align: left;
  cursor: pointer;
}

.newtab__item:hover:not(:disabled) {
  background: var(--color-hover);
}

.newtab__item:disabled {
  color: var(--color-faint);
  cursor: default;
}

.newtab__divider {
  height: 1px;
  margin: var(--space-1) 0;
  background: var(--color-border-soft);
}

.newtab__tag {
  margin-left: auto;
  padding: 0 var(--space-2);
  border-radius: var(--radius-sm);
  background: var(--color-element);
  color: var(--color-faint);
  font-size: var(--font-size-xs);
}

.newtab__error {
  margin: var(--space-2) var(--space-4) 0;
  color: var(--color-red);
  font-size: var(--font-size-xs);
}
</style>

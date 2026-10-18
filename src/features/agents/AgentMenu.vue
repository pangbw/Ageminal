<script setup lang="ts">
/**
 * worktree 行的 ✦ 菜单（#77）：四家 agent 的一等启动入口。
 *
 * 菜单只负责选择 + 打开页签；命令 / 参数 / 颜色由设置投影提供，真正启动发生在
 * `TerminalPanel` 挂载后的 `terminal_open`。
 */
import IconSparkle from "~icons/codicon/sparkle";
import { onMounted } from "vue";

import type { AgentConfig } from "../../bindings";
import { translate as t } from "../../i18n/translate";
import { useAgentsStore } from "../../stores/agents";
import { openAgentTab } from "../tabs/actions";

const props = defineProps<{ root: string; x: number; y: number }>();
const emit = defineEmits<{ close: [] }>();

const agents = useAgentsStore();

onMounted(() => {
  void agents.ensureLoaded();
});

function launch(agent: AgentConfig): void {
  openAgentTab(props.root, agent);
  emit("close");
}
</script>

<template>
  <div class="agent-menu">
    <div
      class="agent-menu__backdrop"
      @click="emit('close')"
      @contextmenu.prevent="emit('close')"
    />
    <div
      class="agent-menu__panel"
      role="menu"
      :style="{ left: `${x}px`, top: `${y}px` }"
    >
      <p class="agent-menu__title">
        {{ t("agents.menuTitle") }}
      </p>
      <button
        v-for="agent in agents.list"
        :key="agent.id"
        class="agent-menu__item"
        type="button"
        role="menuitem"
        :data-agent-id="agent.id"
        @click="launch(agent)"
      >
        <IconSparkle :style="{ color: agent.color }" />
        <span class="agent-menu__name">{{ t("agents.launch", { name: agent.name }) }}</span>
        <span class="agent-menu__command">{{ agent.command }}</span>
      </button>
    </div>
  </div>
</template>

<style scoped>
.agent-menu__backdrop {
  position: fixed;
  inset: 0;
  z-index: 190;
}

.agent-menu__panel {
  position: fixed;
  z-index: 191;
  width: 240px;
  padding: var(--space-1) 0;
  border: 1px solid var(--color-border);
  border-radius: var(--radius-md);
  background: var(--color-elevated);
}

.agent-menu__title {
  margin: 0;
  padding: var(--space-2) var(--space-4) var(--space-1);
  color: var(--color-faint);
  font-size: var(--font-size-xs);
  letter-spacing: 0.04em;
}

.agent-menu__item {
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

.agent-menu__item:hover {
  background: var(--color-hover);
}

.agent-menu__name {
  flex: 1 1 auto;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.agent-menu__command {
  flex: 0 1 auto;
  overflow: hidden;
  color: var(--color-faint);
  font-size: var(--font-size-xs);
  text-overflow: ellipsis;
  white-space: nowrap;
}
</style>

<script setup lang="ts">
/**
 * 项目级设置（#86）：默认 shell / 环境变量 / Agent 命令覆盖。
 *
 * 这些覆盖只影响该项目下的**新终端会话**；已经启动的会话按首次 launch spec 保持不变。
 */
import { onMounted, reactive, ref } from "vue";

import type {
  ProjectEnvView,
  ProjectSettingsInput,
  ProjectSettingsView,
} from "../../bindings";
import { commands } from "../../bindings";
import { translate as t } from "../../i18n/translate";
import { unwrap } from "../../lib/ipc";
import { useAgentsStore } from "../../stores/agents";

const props = defineProps<{ path: string }>();
const agents = useAgentsStore();

const AGENT_IDS = ["claude", "opencode", "codex", "pi"] as const;

const loaded = ref(false);
const saving = ref(false);
const error = ref<string | null>(null);
const saved = ref(false);
const draft = reactive({
  defaultShell: "",
  env: [] as ProjectEnvView[],
  agents: Object.fromEntries(
    AGENT_IDS.map((id) => [id, { command: "", args: "" }]),
  ) as Record<string, { command: string; args: string }>,
});

function applyView(view: ProjectSettingsView): void {
  draft.defaultShell = view.defaultShell ?? "";
  draft.env = view.env.map((entry) => ({ ...entry }));
  for (const id of AGENT_IDS) {
    draft.agents[id] = { command: "", args: "" };
  }
  for (const over of view.agents) {
    if (draft.agents[over.agentId]) {
      draft.agents[over.agentId] = {
        command: over.command ?? "",
        args: over.args ?? "",
      };
    }
  }
  loaded.value = true;
}

async function load(): Promise<void> {
  error.value = null;
  try {
    applyView(await commands.projectSettingsView(props.path).then(unwrap));
  } catch (failure) {
    error.value = failure instanceof Error ? failure.message : String(failure);
  }
}

function addEnv(): void {
  draft.env.push({ name: "", value: "" });
}

function removeEnv(index: number): void {
  draft.env.splice(index, 1);
}

async function save(): Promise<void> {
  error.value = null;
  saved.value = false;
  saving.value = true;
  try {
    const input: ProjectSettingsInput = {
      path: props.path,
      defaultShell: draft.defaultShell.trim() || null,
      env: draft.env
        .map((entry) => ({ name: entry.name.trim(), value: entry.value }))
        .filter((entry) => entry.name.length > 0),
      agents: AGENT_IDS.map((agentId) => ({
        agentId,
        command: draft.agents[agentId].command.trim() || null,
        args: draft.agents[agentId].args.trim() || null,
      })),
    };
    applyView(await commands.setProjectSettings(input).then(unwrap));
    saved.value = true;
  } catch (failure) {
    error.value = failure instanceof Error ? failure.message : String(failure);
  } finally {
    saving.value = false;
  }
}

onMounted(() => {
  void agents.ensureLoaded();
  void load();
});
</script>

<template>
  <section class="project-settings">
    <header class="project-settings__head">
      <h3 class="setting__title">
        {{ t("settings.project.override.title") }}
      </h3>
      <p class="setting__hint">
        {{ t("settings.project.override.hint") }}
      </p>
    </header>

    <label class="project-settings__field">
      <span>{{ t("settings.project.override.shell") }}</span>
      <input
        v-model="draft.defaultShell"
        class="setting__input"
        type="text"
        :placeholder="t('settings.project.override.inherit')"
      >
    </label>

    <div class="project-settings__group">
      <div class="project-settings__group-head">
        <h4>{{ t("settings.project.override.env") }}</h4>
        <button
          class="button"
          type="button"
          @click="addEnv"
        >
          {{ t("settings.project.override.addEnv") }}
        </button>
      </div>
      <p
        v-if="draft.env.length === 0"
        class="setting__hint"
      >
        {{ t("settings.project.override.noEnv") }}
      </p>
      <div
        v-for="(entry, index) in draft.env"
        :key="index"
        class="project-settings__env"
      >
        <input
          v-model="entry.name"
          class="setting__input"
          type="text"
          :placeholder="t('settings.project.override.envName')"
        >
        <input
          v-model="entry.value"
          class="setting__input"
          type="text"
          :placeholder="t('settings.project.override.envValue')"
        >
        <button
          class="button"
          type="button"
          @click="removeEnv(index)"
        >
          {{ t("settings.project.override.remove") }}
        </button>
      </div>
    </div>

    <div class="project-settings__group">
      <h4>{{ t("settings.project.override.agents") }}</h4>
      <p class="setting__hint">
        {{ t("settings.project.override.agentsHint") }}
      </p>
      <div
        v-for="id in AGENT_IDS"
        :key="id"
        class="project-settings__agent"
      >
        <span class="project-settings__agent-name">{{ agents.byId(id)?.name ?? id }}</span>
        <input
          v-model="draft.agents[id].command"
          class="setting__input"
          type="text"
          :placeholder="agents.byId(id)?.command ?? t('settings.project.override.inherit')"
        >
        <input
          v-model="draft.agents[id].args"
          class="setting__input"
          type="text"
          :placeholder="agents.byId(id)?.args || t('settings.project.override.inherit')"
        >
      </div>
    </div>

    <div class="project-settings__foot">
      <span
        v-if="saved"
        class="setting__notice"
      >{{ t("settings.project.override.saved") }}</span>
      <span
        v-if="error"
        class="setting__error"
        role="alert"
      >{{ error }}</span>
      <button
        class="button button--primary"
        type="button"
        :disabled="saving || !loaded"
        @click="save"
      >
        {{ t("settings.project.override.save") }}
      </button>
    </div>
  </section>
</template>

<style scoped>
.project-settings {
  margin-top: var(--space-4);
  padding: var(--space-4);
  border: 1px solid var(--color-border-soft);
  border-radius: var(--radius-md);
}

.project-settings__head {
  margin-bottom: var(--space-3);
}

.project-settings__field,
.project-settings__group {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  margin-top: var(--space-4);
}

.project-settings__field > span {
  color: var(--color-muted);
  font-size: var(--font-size-xs);
}

.project-settings__group-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-3);
}

.project-settings__group-head h4,
.project-settings__group > h4 {
  margin: 0;
  font-size: var(--font-size-sm);
}

.project-settings__env,
.project-settings__agent {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) auto;
  gap: var(--space-2);
  align-items: center;
}

.project-settings__agent {
  grid-template-columns: 110px minmax(0, 1fr) minmax(0, 1fr);
}

.project-settings__agent-name {
  overflow: hidden;
  color: var(--color-muted);
  font-size: var(--font-size-xs);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.project-settings__foot {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: var(--space-3);
  margin-top: var(--space-4);
}
</style>

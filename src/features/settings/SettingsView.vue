<script setup lang="ts">
/**
 * 设置页（REQUIREMENTS.md §6 / §11）：全窗口视图 + 左导航。
 *
 * **#57 只落地「项目」分页**——「加入左侧栏」开关（与侧栏实时联动）、移除项目、
 * worktree 显示名重命名。其余分页由 #86 补内容；导航项先按 §6 列全，未建的分页显示
 * 「即将推出」，免得导航看起来是坏的。
 */
import IconAdd from "~icons/codicon/add";
import IconArrowLeft from "~icons/codicon/arrow-left";
import IconEdit from "~icons/codicon/edit";
import { computed, onMounted, reactive, ref, watch } from "vue";

import type {
  AgentConfig,
  IntegrationAction,
  IntegrationPreview,
  IntegrationStatus,
  IntegrationTestResult,
} from "../../bindings";
import { translate as t, type MessagePath } from "../../i18n/translate";
import { useAgentsStore } from "../../stores/agents";
import { useIntegrationsStore } from "../../stores/integrations";
import { usePreferencesStore } from "../../stores/preferences";
import { useUiStore } from "../../stores/ui";
import { useWorkspaceStore } from "../../stores/workspace";
import WindowControls from "../shell/WindowControls.vue";
import ToggleSwitch from "./ToggleSwitch.vue";

type Section = "general" | "appearance" | "shortcuts" | "agents" | "project";

const SECTIONS = [
  { id: "general", label: "settings.nav.general" },
  { id: "appearance", label: "settings.nav.appearance" },
  { id: "shortcuts", label: "settings.nav.shortcuts" },
  { id: "agents", label: "settings.nav.agents" },
  { id: "project", label: "settings.nav.project" },
] as const satisfies readonly { id: Section; label: MessagePath }[];

const ui = useUiStore();
const workspace = useWorkspaceStore();
const preferences = usePreferencesStore();
const agents = useAgentsStore();
const integrations = useIntegrationsStore();
/** 设置写入失败时显示；成功后清空。 */
const error = ref<string | null>(null);

const section = ref<Section>("project");
/** Agent 卡片的本地草稿：输入时不直接改 Pinia 投影，保存成功后再同步。 */
const agentDrafts = reactive<Record<string, AgentConfig>>({});
const savingAgent = ref<string | null>(null);
/** 正在重命名的 worktree 路径；`null` = 没有在编辑。 */
const editing = ref<string | null>(null);
const draft = ref("");

const heading = computed<MessagePath>(
  () => SECTIONS.find((item) => item.id === section.value)?.label ?? "settings.nav.project",
);

const integrationPreview = ref<IntegrationPreview | null>(null);
const integrationBusy = ref<string | null>(null);
const integrationError = ref<string | null>(null);
const integrationNotice = ref<string | null>(null);
const integrationTest = ref<IntegrationTestResult | null>(null);
const integrationTestAgent = ref<string | null>(null);

const STATUS_LABEL: Record<IntegrationStatus, MessagePath> = {
  notEnabled: "settings.integration.status.notEnabled",
  enabled: "settings.integration.status.enabled",
  updateAvailable: "settings.integration.status.updateAvailable",
  partial: "settings.integration.status.partial",
  unsupported: "settings.integration.status.unsupported",
};

const ACTION_LABEL: Record<IntegrationAction, MessagePath> = {
  create: "settings.integration.action.create",
  update: "settings.integration.action.update",
  unchanged: "settings.integration.action.unchanged",
};

onMounted(() => {
  void agents.ensureLoaded();
  void integrations.refresh().catch((failure: unknown) => {
    integrationError.value = failure instanceof Error ? failure.message : String(failure);
  });
});

watch(section, (next) => {
  if (next !== "agents") return;
  void integrations.refresh().catch((failure: unknown) => {
    integrationError.value = failure instanceof Error ? failure.message : String(failure);
  });
});

// Rust 是事实来源：首次加载 / 保存成功后把草稿整体刷新。用户正在编辑时 store 不会自行变化。
watch(
  () => agents.list,
  (list) => {
    for (const agent of list) agentDrafts[agent.id] = { ...agent };
  },
  { immediate: true },
);

function startRename(path: string, name: string): void {
  editing.value = path;
  draft.value = name;
}

/** 提交重命名；空白 = 回到默认名（Rust 侧同样这么处理）。 */
async function commitRename(projectPath: string, worktreePath: string): Promise<void> {
  // Esc 已先把编辑态清掉；若浏览器随后仍派发 blur，不该再把草稿提交上去。
  if (editing.value !== worktreePath) return;

  const name = draft.value.trim();
  await workspace.renameWorktree(projectPath, worktreePath, name === "" ? null : name);
  editing.value = null;
}

function addProject(): void {
  void workspace.addProject();
}

function removeProject(path: string): void {
  void workspace.removeProject(path);
}

function setPinned(projectPath: string, worktreePath: string, pinned: boolean): void {
  void workspace.setWorktreePinned(projectPath, worktreePath, pinned);
}

/** 保存一个 Agent 的草稿；Rust 返回完整清单后再刷新本地值。 */
async function saveAgent(id: string): Promise<void> {
  const draft = agentDrafts[id];
  if (!draft) return;
  error.value = null;
  savingAgent.value = id;
  try {
    await agents.save({ ...draft });
    const saved = agents.byId(id);
    if (saved) agentDrafts[id] = { ...saved };
  } catch (failure) {
    error.value = failure instanceof Error ? failure.message : String(failure);
  } finally {
    savingAgent.value = null;
  }
}

function integrationStatusLabel(status: IntegrationStatus): string {
  return t(STATUS_LABEL[status]);
}

function integrationActionLabel(action: IntegrationAction): string {
  return t(ACTION_LABEL[action]);
}

function formatDelivery(ms: number | null): string {
  return ms === null ? t("settings.integration.never") : new Date(ms).toLocaleString();
}

async function refreshIntegrations(): Promise<void> {
  integrationError.value = null;
  try {
    await integrations.refresh();
  } catch (failure) {
    integrationError.value = failure instanceof Error ? failure.message : String(failure);
  }
}

async function openIntegrationPreview(agentId: string): Promise<void> {
  integrationError.value = null;
  integrationNotice.value = null;
  integrationBusy.value = agentId;
  try {
    integrationPreview.value = await integrations.preview(agentId);
  } catch (failure) {
    integrationError.value = failure instanceof Error ? failure.message : String(failure);
  } finally {
    integrationBusy.value = null;
  }
}

function closeIntegrationPreview(): void {
  integrationPreview.value = null;
}

async function applyIntegration(): Promise<void> {
  const preview = integrationPreview.value;
  if (!preview) return;
  integrationBusy.value = preview.agentId;
  integrationError.value = null;
  try {
    const result = await integrations.apply(preview.agentId);
    integrationPreview.value = null;
    integrationTest.value = null;
    integrationNotice.value = result.errors.length
      ? result.errors.join("\n")
      : result.backups.length
        ? t("settings.integration.appliedWithBackups", {
            name: preview.name,
            paths: result.backups.join("\n"),
          })
        : t("settings.integration.applied", { name: preview.name });
    if (result.errors.length) integrationError.value = result.errors.join("\n");
  } catch (failure) {
    integrationError.value = failure instanceof Error ? failure.message : String(failure);
  } finally {
    integrationBusy.value = null;
  }
}

async function disableIntegration(agentId: string): Promise<void> {
  integrationBusy.value = agentId;
  integrationError.value = null;
  try {
    const result = await integrations.disable(agentId);
    integrationTest.value = null;
    integrationNotice.value = t("settings.integration.disabled");
    if (result.errors.length) integrationError.value = result.errors.join("\n");
  } catch (failure) {
    integrationError.value = failure instanceof Error ? failure.message : String(failure);
  } finally {
    integrationBusy.value = null;
  }
}

async function testIntegration(agentId: string): Promise<void> {
  integrationBusy.value = agentId;
  integrationError.value = null;
  integrationTestAgent.value = agentId;
  try {
    const result = await integrations.testDelivery(agentId);
    integrationTest.value = result;
    if (!result.delivered) integrationError.value = result.detail;
  } catch (failure) {
    integrationTest.value = null;
    integrationError.value = failure instanceof Error ? failure.message : String(failure);
  } finally {
    integrationBusy.value = null;
  }
}

/** 通用分页目前只有「关闭确认」；其余项归 #86。 */
async function setConfirmClose(enabled: boolean): Promise<void> {
  error.value = null;
  try {
    await preferences.setConfirmClose(enabled);
  } catch (failure) {
    error.value = failure instanceof Error ? failure.message : String(failure);
  }
}
</script>

<template>
  <div class="settings">
    <header
      class="settings__bar"
      data-tauri-drag-region="true"
    >
      <button
        class="button settings__back"
        type="button"
        @click="ui.toggleSettings()"
      >
        <IconArrowLeft />
        {{ t("settings.back") }}
      </button>
      <span class="settings__title">{{ t("settings.title") }}</span>
      <WindowControls />
    </header>

    <div class="settings__body">
      <nav class="settings__nav">
        <button
          v-for="item in SECTIONS"
          :key="item.id"
          class="nav__item"
          :class="{ 'nav__item--active': item.id === section }"
          type="button"
          @click="section = item.id"
        >
          {{ t(item.label) }}
        </button>
      </nav>

      <section class="settings__page">
        <div class="page__head">
          <div>
            <h2 class="page__title">
              {{ t(heading) }}
            </h2>
            <p
              v-if="section === 'project'"
              class="page__subtitle"
            >
              {{ t("settings.project.subtitle") }}
            </p>
            <p
              v-else-if="section === 'agents'"
              class="page__subtitle"
            >
              {{ t("settings.agents.subtitle") }}
            </p>
          </div>
          <button
            v-if="section === 'project'"
            class="button button--primary"
            type="button"
            @click="addProject()"
          >
            <IconAdd />
            {{ t("settings.project.addProject") }}
          </button>
        </div>

        <template v-if="section === 'agents'">
          <article
            v-for="agent in agents.list"
            :key="agent.id"
            class="agent-card"
          >
            <header class="agent-card__head">
              <span
                class="agent-card__swatch"
                :style="{ background: agentDrafts[agent.id]?.color ?? agent.color }"
              />
              <div>
                <h3 class="agent-card__name">
                  {{ agentDrafts[agent.id]?.name || agent.name }}
                </h3>
                <code class="agent-card__id">{{ agent.id }}</code>
              </div>
            </header>

            <div class="agent-card__fields">
              <label class="agent-field">
                <span class="agent-field__label">{{ t("settings.agents.name") }}</span>
                <input
                  v-model="agentDrafts[agent.id].name"
                  class="agent-field__input"
                  type="text"
                >
              </label>
              <label class="agent-field">
                <span class="agent-field__label">{{ t("settings.agents.command") }}</span>
                <input
                  v-model="agentDrafts[agent.id].command"
                  class="agent-field__input"
                  type="text"
                >
              </label>
              <label class="agent-field">
                <span class="agent-field__label">{{ t("settings.agents.args") }}</span>
                <input
                  v-model="agentDrafts[agent.id].args"
                  class="agent-field__input"
                  type="text"
                  :placeholder="t('settings.agents.argsPlaceholder')"
                >
              </label>
              <label class="agent-field agent-field--color">
                <span class="agent-field__label">{{ t("settings.agents.color") }}</span>
                <input
                  v-model="agentDrafts[agent.id].color"
                  class="agent-field__color"
                  type="color"
                  :aria-label="t('settings.agents.color')"
                >
              </label>
            </div>

            <div class="agent-card__foot">
              <span class="agent-card__hint">{{ t("settings.agents.hint") }}</span>
              <button
                class="button"
                type="button"
                :disabled="savingAgent === agent.id"
                @click="saveAgent(agent.id)"
              >
                {{ t("settings.agents.save") }}
              </button>
            </div>
          </article>
          <p
            v-if="error"
            class="setting__error"
            role="alert"
          >
            {{ error }}
          </p>

          <section class="integration-section">
            <div class="integration-section__head">
              <div>
                <h3 class="setting__title">
                  {{ t("settings.integration.title") }}
                </h3>
                <p class="setting__hint">
                  {{ t("settings.integration.subtitle") }}
                </p>
              </div>
              <button
                class="button"
                type="button"
                :disabled="integrations.loading"
                @click="refreshIntegrations"
              >
                {{ t("settings.integration.refresh") }}
              </button>
            </div>

            <p
              v-if="integrationNotice"
              class="setting__notice"
              role="status"
            >
              {{ integrationNotice }}
            </p>
            <p
              v-if="integrationError"
              class="setting__error"
              role="alert"
            >
              {{ integrationError }}
            </p>

            <article
              v-for="item in integrations.list"
              :key="item.agentId"
              class="integration-card"
            >
              <header class="integration-card__head">
                <div>
                  <h4 class="integration-card__name">
                    {{ item.name }}
                  </h4>
                  <code class="integration-card__id">{{ item.agentId }}</code>
                </div>
                <span
                  class="integration-card__status"
                  :class="`integration-card__status--${item.status}`"
                >
                  {{ integrationStatusLabel(item.status) }}
                </span>
              </header>

              <p class="integration-card__meta">
                <span>{{ t("settings.integration.count", { applied: item.applied, total: item.total }) }}</span>
                <span>{{ t("settings.integration.lastDelivery", { time: formatDelivery(item.lastDeliveryMs) }) }}</span>
                <span>{{ t("settings.integration.failures", { count: item.failureCount }) }}</span>
              </p>

              <p
                v-if="!item.shimReady"
                class="integration-card__warn"
              >
                {{ t("settings.integration.shimMissing") }}
              </p>
              <p
                v-if="item.detail"
                class="setting__hint"
              >
                {{ item.detail }}
              </p>

              <ul
                v-if="item.files.length"
                class="integration-card__files"
              >
                <li
                  v-for="file in item.files"
                  :key="file.path"
                >
                  <span>{{ integrationActionLabel(file.action) }}</span>
                  <code>{{ file.path }}</code>
                </li>
              </ul>

              <p
                v-if="integrationTestAgent === item.agentId && integrationTest"
                class="integration-card__test"
                :class="{ 'integration-card__test--error': !integrationTest.delivered }"
              >
                <template v-if="integrationTest.delivered">
                  {{ t("settings.integration.testOk", { time: formatDelivery(integrationTest.lastDeliveryMs) }) }}
                </template>
                <template v-else>
                  {{ t("settings.integration.testFailed", { detail: integrationTest.detail }) }}
                </template>
              </p>

              <div class="integration-card__actions">
                <button
                  v-if="item.status !== 'enabled' && item.status !== 'unsupported'"
                  class="button button--primary"
                  type="button"
                  :disabled="integrationBusy === item.agentId"
                  @click="openIntegrationPreview(item.agentId)"
                >
                  {{ item.status === "notEnabled" ? t("settings.integration.enable") : t("settings.integration.reapply") }}
                </button>
                <button
                  v-if="item.status === 'enabled' || item.status === 'partial' || item.status === 'updateAvailable'"
                  class="button"
                  type="button"
                  :disabled="integrationBusy === item.agentId"
                  @click="disableIntegration(item.agentId)"
                >
                  {{ t("settings.integration.disable") }}
                </button>
                <button
                  v-if="item.status !== 'unsupported'"
                  class="button"
                  type="button"
                  :disabled="integrationBusy === item.agentId"
                  @click="testIntegration(item.agentId)"
                >
                  {{ t("settings.integration.test") }}
                </button>
              </div>
            </article>
          </section>
        </template>

        <template v-else-if="section === 'general'">
          <div class="setting">
            <div class="setting__text">
              <h3 class="setting__title">
                {{ t("settings.general.confirmClose") }}
              </h3>
              <p class="setting__hint">
                {{ t("settings.general.confirmCloseHint") }}
              </p>
            </div>
            <ToggleSwitch
              :model-value="preferences.confirmClose"
              :label="t('settings.general.confirmClose')"
              @update:model-value="setConfirmClose"
            />
          </div>
          <p
            v-if="error"
            class="setting__error"
            role="alert"
          >
            {{ error }}
          </p>
        </template>

        <p
          v-else-if="section !== 'project'"
          class="page__empty"
        >
          {{ t("settings.comingSoon") }}
        </p>

        <p
          v-else-if="workspace.projects.length === 0"
          class="page__empty"
        >
          {{ t("settings.project.empty") }}
        </p>

        <article
          v-for="project in section === 'project' ? workspace.projects : []"
          :key="project.path"
          class="project"
        >
          <header class="project__head">
            <div class="project__ident">
              <span class="project__name">{{ project.name }}</span>
              <span class="project__path">{{ project.path }}</span>
            </div>
            <button
              class="button button--danger"
              type="button"
              @click="removeProject(project.path)"
            >
              {{ t("settings.project.removeProject") }}
            </button>
          </header>

          <p
            v-if="project.error"
            class="project__error"
            role="alert"
          >
            {{ project.error }}
          </p>

          <p class="project__legend">
            {{ t("settings.project.worktreeList") }}
          </p>
          <ul class="project__rows">
            <li
              v-for="worktree in project.worktrees"
              :key="worktree.path"
              class="wt"
            >
              <input
                v-if="editing === worktree.path"
                v-model="draft"
                class="wt__input"
                type="text"
                @keyup.enter="commitRename(project.path, worktree.path)"
                @keyup.esc="editing = null"
                @blur="commitRename(project.path, worktree.path)"
              >
              <span
                v-else
                class="wt__name"
              >{{ worktree.name }}</span>

              <span class="wt__branch">
                {{ worktree.branchLabel ?? t("settings.project.noBranch") }}
              </span>

              <button
                class="icon-button wt__rename"
                type="button"
                :title="t('settings.project.rename')"
                @click="startRename(worktree.path, worktree.name)"
              >
                <IconEdit />
              </button>

              <ToggleSwitch
                :model-value="worktree.pinned"
                :label="t('settings.project.pin')"
                @update:model-value="setPinned(project.path, worktree.path, $event)"
              />
            </li>
          </ul>
        </article>
      </section>
    </div>

    <div
      v-if="integrationPreview"
      class="integration-modal"
      @click.self="closeIntegrationPreview"
    >
      <div
        class="integration-modal__panel"
        role="dialog"
        aria-modal="true"
        :aria-label="t('settings.integration.previewTitle', { name: integrationPreview.name })"
      >
        <header class="integration-modal__head">
          <h3 class="page__title">
            {{ t("settings.integration.previewTitle", { name: integrationPreview.name }) }}
          </h3>
          <button
            class="icon-button"
            type="button"
            :aria-label="t('settings.integration.cancel')"
            @click="closeIntegrationPreview"
          >
            ×
          </button>
        </header>
        <p class="setting__hint">
          {{ t("settings.integration.previewHint") }}
        </p>
        <p
          v-if="!integrationPreview.shimReady"
          class="integration-card__warn"
        >
          {{ t("settings.integration.shimMissing") }}
        </p>

        <div class="integration-modal__files">
          <article
            v-for="file in integrationPreview.files"
            :key="file.path"
            class="integration-preview"
          >
            <header class="integration-preview__head">
              <code>{{ file.path }}</code>
              <span class="integration-card__status">
                {{ integrationActionLabel(file.action) }}
              </span>
            </header>
            <p
              v-if="file.backup"
              class="setting__hint"
            >
              {{ t("settings.integration.backup", { path: file.backup }) }}
            </p>
            <pre v-if="file.diff">{{ file.diff }}</pre>
            <p
              v-else
              class="setting__hint"
            >
              {{ t("settings.integration.noChanges") }}
            </p>
          </article>
        </div>

        <footer class="integration-modal__foot">
          <button
            class="button"
            type="button"
            @click="closeIntegrationPreview"
          >
            {{ t("settings.integration.cancel") }}
          </button>
          <button
            class="button button--primary"
            type="button"
            :disabled="integrationBusy === integrationPreview.agentId"
            @click="applyIntegration"
          >
            {{ t("settings.integration.confirm") }}
          </button>
        </footer>
      </div>
    </div>
  </div>
</template>

<style scoped>
.settings {
  display: flex;
  flex-direction: column;
  height: 100%;
  overflow: hidden;
  background: var(--color-bg);
}

.settings__bar {
  display: flex;
  align-items: center;
  gap: var(--space-4);
  flex: 0 0 var(--titlebar-height);
  height: var(--titlebar-height);
  padding-left: var(--space-4);
  border-bottom: 1px solid var(--color-border);
  user-select: none;
}

.settings__back {
  height: 26px;
  padding: 0 var(--space-4);
}

.settings__title {
  color: var(--color-muted);
  font-size: var(--font-size-sm);
}

.settings__bar :deep(.window-controls) {
  margin-left: auto;
}

.settings__body {
  display: flex;
  flex: 1 1 auto;
  min-height: 0;
}

.settings__nav {
  display: flex;
  flex: 0 0 180px;
  flex-direction: column;
  gap: var(--space-1);
  padding: var(--space-5) var(--space-4);
  border-right: 1px solid var(--color-border-soft);
  overflow: auto;
}

.nav__item {
  padding: var(--space-3) var(--space-4);
  border-radius: var(--radius-sm);
  color: var(--color-muted);
  font-size: var(--font-size-sm);
  text-align: left;
}

.nav__item:hover {
  background: var(--color-hover);
}

.nav__item--active {
  background: var(--color-element);
  color: var(--color-text);
}

.settings__page {
  flex: 1 1 auto;
  min-width: 0;
  padding: var(--space-6);
  overflow: auto;
}

.page__head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: var(--space-5);
  margin-bottom: var(--space-5);
}

.page__title {
  margin: 0;
  font-size: 17px;
  font-weight: 600;
}

.page__subtitle {
  margin: var(--space-2) 0 0;
  color: var(--color-faint);
  font-size: var(--font-size-sm);
}

.page__empty {
  color: var(--color-faint);
  font-size: var(--font-size-sm);
}

.project {
  border: 1px solid var(--color-border);
  border-radius: var(--radius-md);
  padding: var(--space-4);
}

.project + .project {
  margin-top: var(--space-4);
}

.project__head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-4);
}

.project__ident {
  display: flex;
  align-items: baseline;
  gap: var(--space-4);
  min-width: 0;
}

.project__name {
  font-weight: 600;
}

.project__path {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--color-faint);
  font-size: var(--font-size-xs);
}

.project__error {
  margin: var(--space-3) 0 0;
  color: var(--color-red);
  font-size: var(--font-size-xs);
}

.project__legend {
  margin: var(--space-4) 0 var(--space-2);
  color: var(--color-faint);
  font-size: var(--font-size-xs);
  letter-spacing: 0.06em;
  text-transform: uppercase;
}

.project__rows {
  margin: 0;
  padding: 0;
  list-style: none;
}

.wt {
  display: flex;
  align-items: center;
  gap: var(--space-4);
  padding: var(--space-2) var(--space-3);
}

.wt + .wt {
  border-top: 1px solid var(--color-border-soft);
}

.wt__name {
  flex: 1 1 auto;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.wt__input {
  flex: 1 1 auto;
  min-width: 0;
  padding: var(--space-1) var(--space-3);
  border: 1px solid var(--color-accent-line);
  border-radius: var(--radius-sm);
  background: var(--color-bg);
  color: var(--color-text);
  font: inherit;
}

.wt__branch {
  flex: 0 0 auto;
  color: var(--color-faint);
  font-size: var(--font-size-xs);
}

.wt__rename {
  font-size: var(--font-size-sm);
}

.agent-card {
  border: 1px solid var(--color-border);
  border-radius: var(--radius-md);
  padding: var(--space-4);
}

.agent-card + .agent-card {
  margin-top: var(--space-4);
}

.agent-card__head {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  margin-bottom: var(--space-4);
}

.agent-card__swatch {
  width: 10px;
  height: 10px;
  flex: 0 0 auto;
  border-radius: 50%;
}

.agent-card__name {
  margin: 0;
  font-size: var(--font-size-sm);
  font-weight: 600;
}

.agent-card__id {
  color: var(--color-faint);
  font-size: var(--font-size-xs);
}

.agent-card__fields {
  display: grid;
  grid-template-columns: minmax(0, 1.2fr) minmax(0, 1.2fr) minmax(0, 1.6fr) 58px;
  gap: var(--space-3);
}

.agent-field {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  min-width: 0;
}

.agent-field__label {
  color: var(--color-faint);
  font-size: var(--font-size-xs);
}

.agent-field__input {
  width: 100%;
  min-width: 0;
  padding: var(--space-1) var(--space-3);
  border: 1px solid var(--color-border);
  border-radius: var(--radius-sm);
  background: var(--color-bg);
  color: var(--color-text);
  font: inherit;
}

.agent-field__color {
  width: 42px;
  height: 28px;
  padding: 2px;
  border: 1px solid var(--color-border);
  border-radius: var(--radius-sm);
  background: var(--color-bg);
}

.agent-card__foot {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-4);
  margin-top: var(--space-4);
}

.agent-card__hint {
  color: var(--color-faint);
  font-size: var(--font-size-xs);
}

.setting {
  display: flex;
  align-items: center;
  gap: var(--space-5);
  padding: var(--space-4) var(--space-5);
  border: 1px solid var(--color-border-soft);
  border-radius: var(--radius-md);
}

.setting__text {
  flex: 1 1 auto;
  min-width: 0;
}

.setting__title {
  margin: 0;
  color: var(--color-text);
  font-size: var(--font-size-sm);
  font-weight: 600;
}

.setting__hint {
  margin: var(--space-1) 0 0;
  color: var(--color-faint);
  font-size: var(--font-size-xs);
}

.setting__error {
  margin: var(--space-3) 0 0;
  color: var(--color-red);
  font-size: var(--font-size-xs);
  white-space: pre-line;
}

.setting__notice {
  margin: var(--space-3) 0 0;
  padding: var(--space-2) var(--space-3);
  border-radius: var(--radius-sm);
  background: var(--color-accent-dim);
  color: var(--color-accent);
  font-size: var(--font-size-xs);
  white-space: pre-line;
}

.integration-section {
  margin-top: var(--space-6);
  padding-top: var(--space-5);
  border-top: 1px solid var(--color-border-soft);
}

.integration-section__head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: var(--space-4);
  margin-bottom: var(--space-4);
}

.integration-card {
  border: 1px solid var(--color-border);
  border-radius: var(--radius-md);
  padding: var(--space-4);
}

.integration-card + .integration-card {
  margin-top: var(--space-3);
}

.integration-card__head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-4);
}

.integration-card__name {
  margin: 0;
  font-size: var(--font-size-sm);
  font-weight: 600;
}

.integration-card__id {
  color: var(--color-faint);
  font-size: var(--font-size-xs);
}

.integration-card__status {
  flex: 0 0 auto;
  padding: 1px var(--space-2);
  border-radius: var(--radius-sm);
  background: var(--color-element);
  color: var(--color-muted);
  font-size: var(--font-size-xs);
}

.integration-card__status--enabled {
  color: var(--color-green);
}

.integration-card__status--notEnabled {
  color: var(--color-faint);
}

.integration-card__status--updateAvailable,
.integration-card__status--partial {
  color: var(--color-yellow);
}

.integration-card__status--unsupported {
  color: var(--color-faint);
  border: 1px dashed var(--color-border);
}

.integration-card__meta {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2) var(--space-4);
  margin: var(--space-3) 0 0;
  color: var(--color-faint);
  font-size: var(--font-size-xs);
}

.integration-card__warn {
  margin: var(--space-2) 0 0;
  color: var(--color-yellow);
  font-size: var(--font-size-xs);
}

.integration-card__files {
  margin: var(--space-3) 0 0;
  padding: 0;
  list-style: none;
}

.integration-card__files li {
  display: flex;
  align-items: baseline;
  gap: var(--space-2);
  min-width: 0;
  color: var(--color-faint);
  font-size: var(--font-size-xs);
}

.integration-card__files li + li {
  margin-top: 2px;
}

.integration-card__files code,
.integration-preview__head code {
  overflow: hidden;
  color: var(--color-muted);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.integration-card__test {
  margin: var(--space-3) 0 0;
  color: var(--color-green);
  font-size: var(--font-size-xs);
  white-space: pre-line;
}

.integration-card__test--error {
  color: var(--color-red);
}

.integration-card__actions {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
  margin-top: var(--space-3);
}

.integration-modal {
  position: fixed;
  inset: 0;
  z-index: 40;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: var(--space-6);
  background: rgb(0 0 0 / 55%);
}

.integration-modal__panel {
  display: flex;
  flex-direction: column;
  width: min(860px, 100%);
  max-height: 82vh;
  padding: var(--space-5);
  border: 1px solid var(--color-border);
  border-radius: var(--radius-md);
  background: var(--color-bg);
  box-shadow: 0 18px 48px rgb(0 0 0 / 45%);
}

.integration-modal__head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: var(--space-4);
}

.integration-modal__files {
  flex: 1 1 auto;
  min-height: 120px;
  margin-top: var(--space-4);
  overflow: auto;
}

.integration-preview + .integration-preview {
  margin-top: var(--space-4);
}

.integration-preview__head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: var(--space-3);
  min-width: 0;
}

.integration-preview pre {
  margin: var(--space-2) 0 0;
  padding: var(--space-3);
  max-height: 260px;
  overflow: auto;
  border: 1px solid var(--color-border-soft);
  border-radius: var(--radius-sm);
  background: var(--color-bg);
  color: var(--color-muted);
  font-family: var(--font-mono);
  font-size: var(--font-size-xs);
  white-space: pre;
}

.integration-modal__foot {
  display: flex;
  justify-content: flex-end;
  gap: var(--space-3);
  margin-top: var(--space-5);
}
</style>

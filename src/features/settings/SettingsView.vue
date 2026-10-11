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
import { computed, ref } from "vue";

import { translate as t, type MessagePath } from "../../i18n/translate";
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

const section = ref<Section>("project");
/** 正在重命名的 worktree 路径；`null` = 没有在编辑。 */
const editing = ref<string | null>(null);
const draft = ref("");

const heading = computed<MessagePath>(
  () => SECTIONS.find((item) => item.id === section.value)?.label ?? "settings.nav.project",
);

function startRename(path: string, name: string): void {
  editing.value = path;
  draft.value = name;
}

/** 提交重命名；空白 = 回到默认名（Rust 侧同样这么处理）。 */
async function commitRename(projectPath: string, worktreePath: string): Promise<void> {
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

        <p
          v-if="section !== 'project'"
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
</style>

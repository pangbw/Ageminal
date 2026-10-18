<script setup lang="ts">
/**
 * 左侧项目树（REQUIREMENTS.md §6 / §4.1）。
 *
 * 只渲染**已加入左侧栏**的 worktree（`sidebarProjects`）；一个都没加入的项目整组不出现。
 * 行显示名称 + 分支 + 标记（bare / locked / 已失效）。**agent 状态列**归 #80 的统一状态，
 * 那是运行期事件、不在工作区快照里。
 */
import IconAdd from "~icons/codicon/add";
import IconChevronDown from "~icons/codicon/chevron-down";
import IconGitBranch from "~icons/codicon/git-branch";
import IconLock from "~icons/codicon/lock";
import IconSparkle from "~icons/codicon/sparkle";
import IconTerminal from "~icons/codicon/terminal";
import { ref } from "vue";

import { translate as t } from "../../i18n/translate";
import { useWorkspaceStore } from "../../stores/workspace";
import AgentMenu from "../agents/AgentMenu.vue";
import EmptyState from "../shell/EmptyState.vue";
import { openTerminalTab } from "../tabs/actions";

const workspace = useWorkspaceStore();

/** 当前打开的 ✦ 菜单；固定定位，避免被侧栏滚动容器裁切。 */
const agentMenu = ref<{ root: string; x: number; y: number } | null>(null);

function addProject(): void {
  void workspace.addProject();
}

function toggleGroup(project: (typeof workspace.sidebarProjects)[number]): void {
  void workspace.toggleProject(project);
}

function activate(path: string): void {
  void workspace.setActive(path);
}

/** 打开 worktree 行的 Agent 启动菜单。 */
function openAgentMenu(event: MouseEvent, path: string): void {
  // 从非激活 worktree 打开菜单时先切过去，启动后的页签才会出现在当前主区。
  if (!workspace.isActive(path)) void workspace.setActive(path);
  const button = event.currentTarget;
  if (!(button instanceof HTMLElement)) return;
  const rect = button.getBoundingClientRect();
  const width = 240;
  agentMenu.value = {
    root: path,
    x: Math.max(8, Math.min(rect.right - width, window.innerWidth - width - 8)),
    y: Math.min(rect.bottom + 4, window.innerHeight - 240),
  };
}

/** 在这个 worktree 里开一个终端页签；会话本身由面板挂载时创建（#68）。 */
function openTerminal(path: string): void {
  // 从非激活 worktree 的快捷入口点进来时，先把该 worktree 切为激活项。
  if (!workspace.isActive(path)) void workspace.setActive(path);
  openTerminalTab(path);
}
</script>

<template>
  <div class="tree">
    <p
      v-if="workspace.lastError"
      class="tree__error"
      role="alert"
    >
      {{ workspace.lastError }}
    </p>

    <EmptyState
      v-if="workspace.sidebarProjects.length === 0"
      :label="t('shell.projectPanel.empty')"
    >
      <p class="tree__hint">
        {{ t("shell.projectPanel.emptyHint") }}
      </p>
      <button
        class="button button--primary"
        type="button"
        @click="addProject()"
      >
        <IconAdd />
        {{ t("shell.projectPanel.addProject") }}
      </button>
    </EmptyState>

    <div
      v-for="project in workspace.sidebarProjects"
      :key="project.path"
      class="group"
    >
      <button
        class="group__head"
        type="button"
        :title="
          project.collapsed
            ? t('shell.projectPanel.expandGroup')
            : t('shell.projectPanel.collapseGroup')
        "
        @click="toggleGroup(project)"
      >
        <span
          class="group__chevron"
          :class="{ 'group__chevron--collapsed': project.collapsed }"
        >
          <IconChevronDown />
        </span>
        <span class="group__name">{{ project.name }}</span>
        <span class="group__path">{{ project.path }}</span>
      </button>

      <ul
        v-if="!project.collapsed"
        class="group__rows"
      >
        <li
          v-for="worktree in project.worktrees"
          :key="worktree.path"
        >
          <div class="row-wrap">
            <button
              class="row"
              :class="{ 'row--active': workspace.isActive(worktree.path) }"
              type="button"
              @click="activate(worktree.path)"
            >
              <span class="row__main">
                <span class="row__name">{{ worktree.name }}</span>
                <span
                  v-if="worktree.branchLabel"
                  class="row__branch"
                >
                  <IconGitBranch />
                  {{ worktree.branchLabel }}
                </span>
              </span>
              <span
                v-if="worktree.bare"
                class="row__tag"
              >
                {{ t("shell.projectPanel.bare") }}
              </span>
              <span
                v-if="worktree.locked"
                class="row__mark"
                :title="t('shell.projectPanel.locked')"
              >
                <IconLock />
              </span>
              <span
                v-if="worktree.stale"
                class="row__tag row__tag--stale"
              >
                {{ t("shell.projectPanel.stale") }}
              </span>
            </button>
            <div class="row__actions">
              <button
                class="icon-button row__agent"
                type="button"
                :title="t('agents.openMenu')"
                @click.stop="openAgentMenu($event, worktree.path)"
              >
                <IconSparkle />
              </button>
              <button
                class="icon-button row__terminal"
                type="button"
                :title="t('terminal.new')"
                @click.stop="openTerminal(worktree.path)"
              >
                <IconTerminal />
              </button>
            </div>
          </div>
        </li>
      </ul>
    </div>

    <AgentMenu
      v-if="agentMenu"
      :root="agentMenu.root"
      :x="agentMenu.x"
      :y="agentMenu.y"
      @close="agentMenu = null"
    />
  </div>
</template>

<style scoped>
.tree {
  display: flex;
  flex-direction: column;
  padding: var(--space-2) 0 var(--space-4);
}

.tree__error {
  margin: 0 var(--space-4) var(--space-2);
  padding: var(--space-2) var(--space-3);
  border-radius: var(--radius-sm);
  background: var(--color-hover);
  color: var(--color-red);
  font-size: var(--font-size-xs);
}

.tree__hint {
  margin: 0;
  max-width: 190px;
  color: var(--color-faint);
  font-size: var(--font-size-xs);
  text-align: center;
}

.group + .group {
  margin-top: var(--space-3);
}

.group__head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  width: 100%;
  padding: var(--space-2) var(--space-4) var(--space-2) var(--space-3);
  color: var(--color-muted);
  font-size: var(--font-size-xs);
  font-weight: 600;
  letter-spacing: 0.04em;
  text-align: left;
}

.group__head:hover {
  background: var(--color-hover);
}

.group__chevron {
  display: flex;
  flex: 0 0 auto;
  color: var(--color-faint);
  transition: transform 0.12s;
}

.group__chevron--collapsed {
  transform: rotate(-90deg);
}

.group__name {
  flex: 0 1 auto;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--color-text);
}

.group__path {
  flex: 1 1 auto;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--color-faint);
  font-weight: 400;
}

.group__rows {
  margin: 0;
  padding: 0;
  list-style: none;
}

.row-wrap {
  position: relative;
  display: flex;
}

.row {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  width: 100%;
  padding: var(--space-2) 64px var(--space-2) var(--space-6);
  text-align: left;
}

.row__actions {
  position: absolute;
  top: 50%;
  right: var(--space-2);
  display: flex;
  gap: var(--space-1);
  opacity: 0;
  pointer-events: none;
  transform: translateY(-50%);
}

.row__agent,
.row__terminal {
  width: 22px;
  height: 22px;
}

.row-wrap:hover .row__actions,
.row-wrap:focus-within .row__actions {
  opacity: 1;
  pointer-events: auto;
}

.row:hover {
  background: var(--color-hover);
}

.row--active {
  background: var(--color-accent-dim);
}

.row__main {
  display: flex;
  flex: 1 1 auto;
  align-items: baseline;
  gap: var(--space-3);
  min-width: 0;
}

.row__name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.row__branch {
  display: flex;
  align-items: center;
  gap: var(--space-1);
  flex: 0 1 auto;
  overflow: hidden;
  color: var(--color-faint);
  font-size: var(--font-size-xs);
  white-space: nowrap;
}

.row__tag {
  flex: 0 0 auto;
  padding: 0 var(--space-2);
  border-radius: var(--radius-sm);
  background: var(--color-element);
  color: var(--color-faint);
  font-size: var(--font-size-xs);
}

.row__tag--stale {
  color: var(--color-yellow);
}

.row__mark {
  display: flex;
  flex: 0 0 auto;
  color: var(--color-faint);
  font-size: var(--font-size-sm);
}
</style>

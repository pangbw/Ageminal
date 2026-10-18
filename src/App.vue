<script setup lang="ts">
/**
 * 组合根：外壳 + 当前主区内容，以及设置页的整页切换。
 *
 * 启动时拉一次工作区快照 hydrate（§14：前端只是 Rust 快照的内存投影）。
 * 主区 = 文件只读预览（#60）或空态；完整页签模型见 #69；右栏文件树见 #58 / #59。
 */
import IconEmptyWindow from "~icons/codicon/empty-window";
import { computed, onMounted, watch } from "vue";

import FileTree from "./features/files/FileTree.vue";
import FileTreeActions from "./features/files/FileTreeActions.vue";
import ProjectTree from "./features/projects/ProjectTree.vue";
import PreviewPanel from "./features/tabs/PreviewPanel.vue";
import SettingsView from "./features/settings/SettingsView.vue";
import AppShell from "./features/shell/AppShell.vue";
import EmptyState from "./features/shell/EmptyState.vue";
import { useWindowTitle } from "./features/shell/useWindowTitle";
import TerminalPanel from "./features/terminal/TerminalPanel.vue";
import { close as closeTerminalSession } from "./features/terminal/runtime";
import { translate as t } from "./i18n/translate";
import { useFilesStore } from "./stores/files";
import { useTabsStore } from "./stores/tabs";
import { useUiStore } from "./stores/ui";
import { useWorkspaceStore } from "./stores/workspace";

const ui = useUiStore();
const workspace = useWorkspaceStore();
const files = useFilesStore();
const tabs = useTabsStore();

// 窗口标题跟着激活 worktree 走（§6）。
useWindowTitle();

// 文件树的根 = 当前激活 worktree（§9）。换树时把旧状态整个丢掉（#59）。
watch(() => workspace.activePath, (path) => files.setRoot(path), { immediate: true });

/** 当前 worktree 的激活预览页签（含根路径）；`null` = 激活项不是预览。 */
const preview = computed(() => {
  const root = files.root;
  if (root === null) return null;
  const tab = tabs.activePreview(root);
  return tab ? { root, tab } : null;
});

/** 当前 worktree 的激活终端页签（含根路径）；`null` = 激活项不是终端。 */
const terminal = computed(() => {
  const root = files.root;
  if (root === null) return null;
  const tab = tabs.activeTerminal(root);
  return tab ? { root, tab } : null;
});

function closePreview(): void {
  const current = preview.value;
  if (current !== null) tabs.closeFile(current.root, current.tab.path);
}

function closeTerminal(): void {
  const current = terminal.value;
  if (current === null) return;
  tabs.closeTerminal(current.root, current.tab.sid);
  void closeTerminalSession(current.tab.sid);
}

onMounted(() => {
  void workspace.refresh();
});

/** 「添加项目」的实体：Rust 侧弹系统目录选择器（#32 D1）。 */
function addProject(): void {
  void workspace.addProject();
}
</script>

<template>
  <SettingsView v-if="ui.settingsOpen" />
  <AppShell
    v-else
    @add-project="addProject"
  >
    <!-- 左栏内容：项目 / worktree 树（#57）。 -->
    <template #projects>
      <ProjectTree />
    </template>
    <!-- 主区：文件只读预览（#60）、终端（#68）或空态；完整页签模型见 #69。 -->
    <template #main>
      <PreviewPanel
        v-if="preview"
        :root="preview.root"
        :tab="preview.tab"
        @close="closePreview"
      />
      <TerminalPanel
        v-else-if="terminal"
        :tab="terminal.tab"
        @close="closeTerminal"
      />
      <EmptyState
        v-else
        :icon="IconEmptyWindow"
        :label="t('tabs.empty')"
      />
    </template>
    <!-- 右栏内容：文件树（#59）+ 两个可见性开关。 -->
    <template #files-actions>
      <FileTreeActions />
    </template>
    <template #files>
      <FileTree />
    </template>
  </AppShell>
</template>

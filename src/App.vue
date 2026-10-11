<script setup lang="ts">
/**
 * 组合根：外壳 + 当前主区内容，以及设置页的整页切换。
 *
 * 启动时拉一次工作区快照 hydrate（§14：前端只是 Rust 快照的内存投影）。
 * 主区仍是空态（§8）——页签模型见 #69；右栏文件树见 #58 / #59。
 */
import IconEmptyWindow from "~icons/codicon/empty-window";
import { computed, onMounted, watch } from "vue";

import FileTree from "./features/files/FileTree.vue";
import FileTreeActions from "./features/files/FileTreeActions.vue";
import ProjectTree from "./features/projects/ProjectTree.vue";
import PreviewPlaceholder from "./features/tabs/PreviewPlaceholder.vue";
import SettingsView from "./features/settings/SettingsView.vue";
import AppShell from "./features/shell/AppShell.vue";
import EmptyState from "./features/shell/EmptyState.vue";
import { useWindowTitle } from "./features/shell/useWindowTitle";
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

/** 当前 worktree 的激活预览页签；`null` = 主区显示空态。 */
const preview = computed(() => (files.root === null ? null : tabs.activeTab(files.root)));

function closePreview(): void {
  if (files.root !== null && preview.value !== null) {
    tabs.closeFile(files.root, preview.value.path);
  }
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
    <!-- 页签：暂无页签栏，预览面板见 #60 / #69。 -->
    <template #main>
      <PreviewPlaceholder
        v-if="preview"
        :tab="preview"
        @close="closePreview"
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

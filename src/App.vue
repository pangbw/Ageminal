<script setup lang="ts">
/**
 * 组合根：外壳 + 当前主区内容，以及设置页的整页切换。
 *
 * 启动时拉一次工作区快照 hydrate（§14：前端只是 Rust 快照的内存投影）。
 * 主区仍是空态（§8）——页签模型见 #69；右栏文件树见 #58 / #59。
 */
import IconEmptyWindow from "~icons/codicon/empty-window";
import { onMounted } from "vue";

import ProjectTree from "./features/projects/ProjectTree.vue";
import SettingsView from "./features/settings/SettingsView.vue";
import AppShell from "./features/shell/AppShell.vue";
import EmptyState from "./features/shell/EmptyState.vue";
import { useWindowTitle } from "./features/shell/useWindowTitle";
import { translate as t } from "./i18n/translate";
import { useUiStore } from "./stores/ui";
import { useWorkspaceStore } from "./stores/workspace";

const ui = useUiStore();
const workspace = useWorkspaceStore();

// 窗口标题跟着激活 worktree 走（§6）。
useWindowTitle();

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
    <!-- 页签：暂无，页签模型见 #69。 -->
    <template #main>
      <EmptyState
        :icon="IconEmptyWindow"
        :label="t('tabs.empty')"
      />
    </template>
    <!-- 右栏内容：文件树（#58 / #59）。 -->
  </AppShell>
</template>

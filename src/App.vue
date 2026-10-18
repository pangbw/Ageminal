<script setup lang="ts">
/**
 * 组合根：外壳 + 当前主区内容，以及设置页的整页切换。
 *
 * 启动时拉一次工作区快照 hydrate（§14：前端只是 Rust 快照的内存投影）。
 * 主区 = 文件只读预览（#60）或空态；完整页签模型见 #69；右栏文件树见 #58 / #59。
 */
import IconEmptyWindow from "~icons/codicon/empty-window";
import { computed, onBeforeUnmount, onMounted, watch } from "vue";

import { commands } from "./bindings";

import FileTree from "./features/files/FileTree.vue";
import FileTreeActions from "./features/files/FileTreeActions.vue";
import ConfirmDialog from "./features/files/ConfirmDialog.vue";
import ProjectTree from "./features/projects/ProjectTree.vue";
import PreviewPanel from "./features/tabs/PreviewPanel.vue";
import SettingsView from "./features/settings/SettingsView.vue";
import AppShell from "./features/shell/AppShell.vue";
import EmptyState from "./features/shell/EmptyState.vue";
import TabStrip from "./features/shell/TabStrip.vue";
import { useWindowTitle } from "./features/shell/useWindowTitle";
import {
  cancelPendingClose,
  confirmPendingClose,
  pendingClose,
  requestClose,
} from "./features/tabs/actions";
import { setWindowsBuildNumber } from "./features/terminal/platform";
import { restoreWorkspaceTabs } from "./features/tabs/restore";
import TerminalPanel from "./features/terminal/TerminalPanel.vue";
import { translate as t } from "./i18n/translate";
import { unwrap } from "./lib/ipc";
import { isSameOrUnder } from "./lib/path";
import { useAgentStatusStore } from "./stores/agentStatus";
import { useAgentsStore } from "./stores/agents";
import { useFilesStore } from "./stores/files";
import { usePreferencesStore } from "./stores/preferences";
import { useTabsStore } from "./stores/tabs";
import { useUiStore } from "./stores/ui";
import { useWorkspaceStore } from "./stores/workspace";

const ui = useUiStore();
const workspace = useWorkspaceStore();
const files = useFilesStore();
const tabs = useTabsStore();
const preferences = usePreferencesStore();
const agents = useAgentsStore();
const agentStatus = useAgentStatusStore();

// 窗口标题跟着激活 worktree 走（§6）。
useWindowTitle();

// #87：Acrylic 通过窗口创建期 transparent + DWM 材质实现；CSS 只切换面板 alpha。
watch(
  () => preferences.appearance.acrylic,
  (acrylic) => {
    document.documentElement.dataset.acrylic = acrylic ? "true" : "false";
  },
  { immediate: true },
);

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
  if (current !== null) requestClose(current.root, [current.tab]);
}

function closeTerminal(): void {
  const current = terminal.value;
  if (current !== null) requestClose(current.root, [current.tab]);
}

/** 启动顺序：工作区快照 → 设置 / agent 配置 → 页签对账。 */
async function bootstrap(): Promise<void> {
  await workspace.refresh();
  await Promise.all([
    preferences.load(),
    agents.ensureLoaded(),
    agentStatus.start(),
    commands.windowsBuildNumber().then(setWindowsBuildNumber).catch(() => undefined),
  ]);
  // #78：Rust 会先按 state.json + daemon ListSessions 做四象限对账，再把骨架交回这里。
  // #86：启动行为 = 空工作区时跳过恢复，只保留下一次用户主动打开的页签。
  if (preferences.startupBehavior === "empty") {
    tabsRestored = true;
    return;
  }
  try {
    await restoreWorkspaceTabs();
    // 恢复失败时宁可本次不落盘，也不要用空 store 覆盖 state.json 里的旧骨架。
    tabsRestored = true;
    // 收养的孤儿 / 对账后裁剪的骨架也写回一次，下一次启动不依赖 daemon 仍在。
    scheduleTabsPersist();
  } catch (failure) {
    console.warn("[tabs] restore failed", failure);
  }
}

onMounted(() => {
  void bootstrap();
});

/**
 * 页签骨架的持久化投影：只在恢复完成后开始监听，避免启动时用空 store 覆盖磁盘。
 * 去抖窗口短于 Rust 的 500 ms 状态落盘窗口，连续拖拽 / 开关只留最后一次。
 */
let tabsRestored = false;
let persistTimer: ReturnType<typeof setTimeout> | undefined;

function scheduleTabsPersist(): void {
  if (!tabsRestored) return;
  clearTimeout(persistTimer);
  persistTimer = setTimeout(() => {
    // 已移除项目下的旧骨架不要再写回；项目列表为空时可能是快照尚未成功，
    // 此时宁可保留完整骨架，也不要把 state.json 清空。
    const projects = workspace.projects;
    const snapshot = tabs.snapshot();
    const worktrees =
      projects.length === 0
        ? snapshot
        : snapshot.filter((group) =>
            projects.some((project) => isSameOrUnder(group.root, project.path)),
          );
    void commands
      .saveTabs(worktrees)
      .then(unwrap)
      .catch((failure: unknown) => console.warn("[tabs] persist failed", failure));
  }, 150);
}

watch(
  [() => tabs.tabsByRoot, () => tabs.activeByRoot],
  scheduleTabsPersist,
  { deep: true },
);

onBeforeUnmount(() => {
  clearTimeout(persistTimer);
  agentStatus.stop();
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
    <!-- 页签条：按当前 worktree 分组，切换时整组替换（#69）。 -->
    <template #tabs>
      <TabStrip />
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
        :key="terminal.tab.sid"
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

  <ConfirmDialog
    v-if="pendingClose"
    :title="t('tabs.closeRunningTitle')"
    :message="pendingClose.message"
    :confirm-label="t('tabs.closeAnyway')"
    danger
    @confirm="confirmPendingClose"
    @cancel="cancelPendingClose"
  />
</template>

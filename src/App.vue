<script setup lang="ts">
/**
 * 组合根：外壳 + 当前主区内容。
 *
 * #55 只有空态（§8：最后一个页签关闭后主区显示「没有打开的页签」）。
 * 左栏（#57）、页签（#69）、右栏（#58/#59）落地后各自填进对应插槽。
 */
import IconEmptyWindow from "~icons/codicon/empty-window";

import AppShell from "./features/shell/AppShell.vue";
import EmptyState from "./features/shell/EmptyState.vue";
import { useWindowTitle } from "./features/shell/useWindowTitle";
import { translate as t } from "./i18n/translate";

// 窗口标题跟着激活 worktree 走（§6）。
useWindowTitle();
</script>

<template>
  <AppShell>
    <!-- 左栏内容：项目 / worktree 树（#57）。 -->
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

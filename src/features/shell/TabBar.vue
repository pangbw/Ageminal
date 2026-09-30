<script setup lang="ts">
/**
 * 页签栏（REQUIREMENTS.md §6）：页签可横向滚动，`+` 紧跟最后一个页签之后。
 *
 * 页签本身归 #69（按 worktree 分组、整组替换）；#55 只提供条 + 新建入口。
 * 条与空白区标注为窗口拖拽区（见 TitleBar 的说明），页签只当子元素。
 */
import IconAdd from "~icons/codicon/add";

import { translate as t } from "../../i18n/translate";
import { useUiStore } from "../../stores/ui";

const ui = useUiStore();
</script>

<template>
  <div
    class="tabbar"
    data-tauri-drag-region="true"
  >
    <div
      class="tabbar__strip"
      data-tauri-drag-region="true"
    >
      <slot />
    </div>
    <button
      class="icon-button tabbar__new"
      type="button"
      :title="t('tabs.new')"
      @click="ui.toggleNewTabMenu()"
    >
      <IconAdd />
    </button>
  </div>
</template>

<style scoped>
.tabbar {
  display: flex;
  align-items: stretch;
  flex: 1 1 auto;
  min-width: 0;
}

.tabbar__strip {
  display: flex;
  align-items: stretch;
  flex: 0 1 auto;
  min-width: 0;
  overflow-x: auto;
  overflow-y: hidden;
  scrollbar-width: none;
}

.tabbar__strip::-webkit-scrollbar {
  display: none;
}

.tabbar__new {
  flex: 0 0 auto;
  width: 34px;
  height: auto;
  border-radius: 0;
  font-size: 15px;
}
</style>

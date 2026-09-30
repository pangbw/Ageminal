<script setup lang="ts">
/**
 * 应用外壳与三栏布局（REQUIREMENTS.md §6）。
 *
 * 左项目栏全高、页签栏从其右侧开始；左 / 主 / 右三栏，左右可拖拽调宽、可折叠成细条。
 * 各栏**内容**留在插槽里：#57 填项目树、#58/#59 填文件树、#69 填页签、主区由使用方决定。
 */
import IconAdd from "~icons/codicon/add";

import { translate as t } from "../../i18n/translate";
import { useUiStore } from "../../stores/ui";
import CollapsedRail from "./CollapsedRail.vue";
import PanelResizer from "./PanelResizer.vue";
import SidePanel from "./SidePanel.vue";
import TabBar from "./TabBar.vue";
import TitleBar from "./TitleBar.vue";

const ui = useUiStore();
</script>

<template>
  <div class="app">
    <SidePanel
      v-if="!ui.leftCollapsed"
      side="left"
      :width="ui.leftWidth"
      :collapse-label="t('shell.projectPanel.collapse')"
      @collapse="ui.collapsePanel('left')"
    >
      <template #head>
        <span class="app__brand">{{ t("common.appName") }}</span>
      </template>
      <template #actions>
        <button
          class="icon-button app__add-project"
          type="button"
          :title="t('shell.projectPanel.addProject')"
          @click="ui.toggleAddProject()"
        >
          <IconAdd />
        </button>
      </template>
      <slot name="projects" />
    </SidePanel>
    <CollapsedRail
      v-else
      side="left"
      :label="t('shell.projectPanel.railLabel')"
      :expand-label="t('shell.projectPanel.expand')"
      @expand="ui.expandPanel('left')"
    />
    <PanelResizer
      v-if="!ui.leftCollapsed"
      side="left"
    />

    <div class="app__region">
      <TitleBar @settings="ui.toggleSettings()">
        <TabBar>
          <slot name="tabs" />
        </TabBar>
      </TitleBar>

      <div class="app__workspace">
        <main class="app__main">
          <slot name="main" />
        </main>

        <PanelResizer
          v-if="!ui.rightCollapsed"
          side="right"
        />
        <SidePanel
          v-if="!ui.rightCollapsed"
          side="right"
          :width="ui.rightWidth"
          :collapse-label="t('files.panel.collapse')"
          @collapse="ui.collapsePanel('right')"
        >
          <template #head>
            <span>{{ t("files.panel.title") }}</span>
          </template>
          <template #actions>
            <slot name="files-actions" />
          </template>
          <slot name="files" />
        </SidePanel>
        <CollapsedRail
          v-else
          side="right"
          :label="t('files.panel.title')"
          :expand-label="t('files.panel.expand')"
          @expand="ui.expandPanel('right')"
        />
      </div>
    </div>
  </div>
</template>

<style scoped>
.app {
  display: flex;
  flex-direction: row;
  height: 100%;
  overflow: hidden;
  background: var(--color-bg);
}

.app__region {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
}

.app__workspace {
  display: flex;
  flex: 1 1 auto;
  min-height: 0;
}

.app__main {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
}

.app__brand {
  color: var(--color-text);
  font-size: var(--font-size-ui);
  font-weight: 600;
  letter-spacing: 0.01em;
}
</style>

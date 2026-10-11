<script setup lang="ts">
/**
 * 右侧文件树（§9 / #19 D5 / D7 / D8）。
 *
 * **懒加载**：根 = 当前 worktree 根，展开哪个目录才读哪个；可见行扁平化后交给
 * `useVirtualList` 虚拟滚动（#13 D5），大目录也不会把 DOM 撑爆。
 *
 * **搜索框是 Rust 全树搜**（#19 D8）：命中以扁平列表呈现，点它把树逐级展开到该处（懒加载）
 * 并打开预览页签；命中目录则展开该目录本身。
 */
import { useVirtualList } from "@vueuse/core";
import IconChevronDown from "~icons/codicon/chevron-down";
import IconChevronRight from "~icons/codicon/chevron-right";
import IconFile from "~icons/codicon/file";
import IconFolder from "~icons/codicon/folder";
import IconFolderOpened from "~icons/codicon/folder-opened";
import IconSearch from "~icons/codicon/search";
import { computed, ref, type Component } from "vue";

import { translate as t } from "../../i18n/translate";
import { basename } from "../../lib/path";
import { useFilesStore, type FileRow } from "../../stores/files";
import type { SearchHitView } from "../../bindings";
import { useTabsStore } from "../../stores/tabs";
import EmptyState from "../shell/EmptyState.vue";

/** 行高（px），固定值才能虚拟滚动。 */
const ROW_HEIGHT = 22;

const files = useFilesStore();
const tabs = useTabsStore();

const { list, containerProps, wrapperProps } = useVirtualList(
  computed(() => files.visibleRows),
  { itemHeight: ROW_HEIGHT, overscan: 8 },
);

const searchInput = ref("");

/** 一行左侧的图标：开着的目录 / 合着的目录 / 文件。 */
function rowIcon(row: FileRow): Component {
  if (!row.isDir) return IconFile;
  return row.expanded ? IconFolderOpened : IconFolder;
}

function onSearchInput(): void {
  files.setQuery(searchInput.value);
}

function onRowClick(row: FileRow): void {
  if (row.isDir) {
    void files.toggleDir(row.path);
    return;
  }
  files.selected = row.path;
  if (files.root !== null) tabs.openFile(files.root, row.path, row.name);
}

async function onHitClick(hit: SearchHitView): Promise<void> {
  await files.reveal(hit.path);
  if (hit.isDir) {
    // 命中目录：「定位并展开」它本身，不开预览页签。
    await files.expandDir(hit.path);
  } else if (files.root !== null) {
    tabs.openFile(files.root, hit.path, basename(hit.path));
  }
  searchInput.value = "";
  files.clearSearch();
}
</script>

<template>
  <div class="files">
    <EmptyState
      v-if="files.root === null"
      :label="t('files.empty')"
    />

    <template v-else>
      <div class="files__head">
        <span
          class="files__root"
          :title="files.root"
        >{{ files.root }}</span>
        <div class="files__search">
          <IconSearch class="files__search-icon" />
          <input
            v-model="searchInput"
            class="files__search-input"
            type="search"
            :placeholder="t('files.search.placeholder')"
            @input="onSearchInput"
          >
        </div>
      </div>

      <p
        v-if="files.lastError"
        class="files__error"
        role="alert"
      >
        {{ files.lastError }}
      </p>

      <div
        v-if="files.results !== null"
        class="files__results"
      >
        <p
          v-if="files.searching"
          class="files__note"
        >
          {{ t("files.search.searching") }}
        </p>
        <p
          v-else-if="files.results.length === 0"
          class="files__note"
        >
          {{ t("files.search.noResults") }}
        </p>
        <ul
          v-else
          class="files__hits"
        >
          <li
            v-for="hit in files.results"
            :key="hit.path"
          >
            <button
              class="files__hit"
              :class="{ 'files__hit--ignored': hit.ignored }"
              type="button"
              :title="hit.path"
              @click="onHitClick(hit)"
            >
              {{ hit.relative }}
            </button>
          </li>
        </ul>
        <p
          v-if="files.searchTruncated"
          class="files__note"
        >
          {{ t("files.search.truncated") }}
        </p>
        <p
          v-if="files.searchTimedOut"
          class="files__note"
        >
          {{ t("files.search.timedOut") }}
        </p>
      </div>

      <div
        v-else
        class="files__tree"
        v-bind="containerProps"
      >
        <div v-bind="wrapperProps">
          <template
            v-for="item in list"
            :key="item.data.path"
          >
            <p
              v-if="item.data.notice"
              class="files__notice"
              :style="{ height: `${ROW_HEIGHT}px` }"
            >
              {{ t("files.truncated") }}
            </p>
            <button
              v-else
              class="files__row"
              :class="{
                'files__row--selected': files.isSelected(item.data.path),
                'files__row--ignored': item.data.ignored,
              }"
              type="button"
              :style="{ height: `${ROW_HEIGHT}px`, paddingLeft: `${6 + item.data.depth * 12}px` }"
              @click="onRowClick(item.data)"
            >
              <span class="files__tw">
                <component
                  :is="item.data.expanded ? IconChevronDown : IconChevronRight"
                  v-if="item.data.isDir"
                />
              </span>
              <span class="files__icon">
                <component :is="rowIcon(item.data)" />
              </span>
              <span class="files__name">{{ item.data.name }}</span>
            </button>
          </template>
        </div>
      </div>
    </template>
  </div>
</template>

<style scoped>
.files {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
}

.files__head {
  display: flex;
  flex: 0 0 auto;
  flex-direction: column;
  gap: var(--space-2);
  padding: var(--space-2) var(--space-4);
  border-bottom: 1px solid var(--color-border-soft);
}

.files__root {
  overflow: hidden;
  color: var(--color-faint);
  font-family: var(--font-mono, monospace);
  font-size: var(--font-size-xs);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.files__search {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: 0 var(--space-2);
  border: 1px solid var(--color-border);
  border-radius: var(--radius-sm);
  background: var(--color-element);
  color: var(--color-faint);
}

.files__search-icon {
  flex: 0 0 auto;
}

.files__search-input {
  flex: 1 1 auto;
  min-width: 0;
  padding: var(--space-1) 0;
  border: 0;
  background: transparent;
  color: var(--color-text);
  font-size: var(--font-size-sm);
  outline: none;
}

.files__error {
  margin: 0;
  padding: var(--space-2) var(--space-4);
  color: var(--color-red);
  font-size: var(--font-size-xs);
}

.files__tree {
  flex: 1 1 auto;
  min-height: 0;
}

.files__row {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  width: 100%;
  padding-right: var(--space-3);
  color: var(--color-muted);
  font-size: var(--font-size-sm);
  text-align: left;
}

.files__row:hover {
  background: var(--color-hover);
  color: var(--color-text);
}

.files__row--selected {
  background: var(--color-accent-dim);
  color: var(--color-text);
}

/* 「显示忽略文件」打开时，被忽略项弱化显示（#18 D4 / §9）。 */
.files__row--ignored,
.files__hit--ignored {
  opacity: 0.55;
}

.files__tw {
  display: flex;
  flex: 0 0 auto;
  width: 12px;
  color: var(--color-faint);
}

.files__icon {
  display: flex;
  flex: 0 0 auto;
  color: var(--color-faint);
}

.files__name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.files__notice {
  display: flex;
  align-items: center;
  margin: 0;
  padding: 0 var(--space-4);
  color: var(--color-faint);
  font-size: var(--font-size-xs);
}

.files__results {
  flex: 1 1 auto;
  min-height: 0;
  overflow: auto;
  padding: var(--space-2) 0;
}

.files__hits {
  margin: 0;
  padding: 0;
  list-style: none;
}

.files__hit {
  display: block;
  width: 100%;
  padding: var(--space-1) var(--space-4);
  overflow: hidden;
  color: var(--color-muted);
  font-size: var(--font-size-sm);
  text-align: left;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.files__hit:hover {
  background: var(--color-hover);
  color: var(--color-text);
}

.files__note {
  margin: 0;
  padding: var(--space-2) var(--space-4);
  color: var(--color-faint);
  font-size: var(--font-size-xs);
}
</style>

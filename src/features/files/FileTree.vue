<script setup lang="ts">
/**
 * 右侧文件树（§9 / #19 D5 / D7 / D8；右键操作 #62）。
 *
 * **懒加载**：根 = 当前 worktree 根，展开哪个目录才读哪个；可见行扁平化后交给
 * `useVirtualList` 虚拟滚动（#13 D5），大目录也不会把 DOM 撑爆。
 *
 * **搜索框是 Rust 全树搜**（#19 D8）：命中以扁平列表呈现，点它把树逐级展开到该处（懒加载）
 * 并打开预览页签；命中目录则展开该目录本身。
 *
 * **右键操作**（#62）：行上右键出菜单（新建 / 重命名 / 资源管理器 / 终端 / 复制 / 删除）。
 * 删除走应用内二次确认 + 回收站；新建 / 重命名用树内内联输入。
 */
import { useVirtualList } from "@vueuse/core";
import IconChevronDown from "~icons/codicon/chevron-down";
import IconChevronRight from "~icons/codicon/chevron-right";
import IconFile from "~icons/codicon/file";
import IconFolder from "~icons/codicon/folder";
import IconFolderOpened from "~icons/codicon/folder-opened";
import IconSearch from "~icons/codicon/search";
import { computed, nextTick, ref, type Component } from "vue";

import type { SearchHitView } from "../../bindings";
import { commands } from "../../bindings";
import { translate as t } from "../../i18n/translate";
import { unwrap, errorMessage, type CommandResult } from "../../lib/ipc";
import { basename, parentOf, relativeTo } from "../../lib/path";
import { useFilesStore, type FileRow } from "../../stores/files";
import { useTabsStore } from "../../stores/tabs";
import EmptyState from "../shell/EmptyState.vue";
import ConfirmDialog from "./ConfirmDialog.vue";
import FileContextMenu, { type ContextAction } from "./FileContextMenu.vue";

/** 行高（px），固定值才能虚拟滚动。 */
const ROW_HEIGHT = 22;

const files = useFilesStore();
const tabs = useTabsStore();

const { list, containerProps, wrapperProps } = useVirtualList(
  computed(() => files.visibleRows),
  { itemHeight: ROW_HEIGHT, overscan: 8 },
);

const searchInput = ref("");
/** 右键菜单（含定位与目标行）。 */
const menu = ref<{ x: number; y: number; row: FileRow } | null>(null);
/** 待确认的删除。 */
const confirm = ref<{ path: string; name: string } | null>(null);
/** 内联输入（新建 / 重命名）。 */
const editing = ref<{
  mode: "newFile" | "newFolder" | "rename";
  dir: string;
  path: string;
  value: string;
} | null>(null);
/** 复制 / 终端 / 资源管理器等的失败（不遮挡树本身）。 */
const actionError = ref<string | null>(null);

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

/** 新建时的承载目录：右键目录就放进它，右键文件就放进它的父目录。 */
function targetDir(row: FileRow): string {
  return row.isDir ? row.path : (parentOf(row.path) ?? files.root ?? row.path);
}

function onContextMenu(event: MouseEvent, row: FileRow): void {
  menu.value = { x: event.clientX, y: event.clientY, row };
}

/** 跑一个「旁路」命令（复制 / 终端 / 资源管理器）：失败只提示，不打断树。 */
async function runSideEffect(result: Promise<CommandResult<unknown>>): Promise<void> {
  try {
    unwrap(await result);
    actionError.value = null;
    // 成功即清掉上一次的红字（可能是删除失败留下的）。
    files.lastError = null;
  } catch (error) {
    actionError.value = errorMessage(error);
  }
}

async function startEdit(
  mode: "newFile" | "newFolder" | "rename",
  dir: string,
  path: string,
  value: string,
): Promise<void> {
  await files.expandDir(dir);
  editing.value = { mode, dir, path, value };
  await nextTick();
  // 同一时刻只有一个内联输入，直接按类名聚焦即可（容器 ref 归虚拟滚动持有）。
  document.querySelector<HTMLInputElement>(".files__edit")?.focus();
}

async function onMenuAction(action: ContextAction): Promise<void> {
  const row = menu.value?.row;
  const root = files.root;
  if (!row || root === null) return;
  actionError.value = null; // 新一次操作：先清掉上一次的旁路错误
  switch (action) {
    case "newFile":
      await startEdit("newFile", targetDir(row), "", "");
      break;
    case "newFolder":
      await startEdit("newFolder", targetDir(row), "", "");
      break;
    case "rename":
      await startEdit("rename", parentOf(row.path) ?? root, row.path, row.name);
      break;
    case "reveal":
      await runSideEffect(commands.revealInExplorer(root, row.path));
      break;
    case "terminal":
      await runSideEffect(commands.openTerminal(root, targetDir(row)));
      break;
    case "copyFile":
      await runSideEffect(commands.copyFiles(root, [row.path]));
      break;
    case "copyPath":
      await runSideEffect(commands.writeClipboard(row.path));
      break;
    case "copyRelative":
      await runSideEffect(commands.writeClipboard(relativeTo(root, row.path)));
      break;
    case "delete":
      confirm.value = { path: row.path, name: row.name };
      break;
  }
}

async function submitEdit(): Promise<void> {
  const current = editing.value;
  const root = files.root;
  if (!current || root === null) return;
  const name = current.value.trim();
  editing.value = null;
  if (name === "") return;

  if (current.mode === "rename") {
    const target = await files.renameEntry(current.path, name);
    if (target !== null) tabs.movePath(root, current.path, target);
    return;
  }
  await files.createEntry(current.dir, name, current.mode === "newFolder");
}

function cancelEdit(): void {
  editing.value = null;
}

async function doDelete(): Promise<void> {
  const target = confirm.value;
  const root = files.root;
  confirm.value = null;
  if (!target || root === null) return;
  const deleted = await files.deleteEntry(target.path);
  if (deleted) tabs.closeUnder(root, target.path);
}

/** 新建输入是否锚在某个目录行下（该行可见时）；否则落到树顶。 */
const newEditorRowDir = computed(() => {
  const current = editing.value;
  if (!current || current.mode === "rename") return null;
  const anchor = files.visibleRows.find((row) => row.isDir && row.path === current.dir);
  return anchor ? current.dir : null;
});
const newEditorAtTop = computed(
  () => editing.value !== null && editing.value.mode !== "rename" && newEditorRowDir.value === null,
);
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
        v-if="files.lastError || actionError"
        class="files__error"
        role="alert"
      >
        {{ files.lastError ?? actionError }}
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
        <input
          v-if="newEditorAtTop && editing"
          v-model="editing.value"
          class="files__edit files__edit--top"
          type="text"
          @keydown.enter.stop="submitEdit"
          @keydown.esc.stop="cancelEdit"
          @blur="cancelEdit"
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
            <div
              v-else
              class="files__row"
              :class="{
                'files__row--selected': files.isSelected(item.data.path),
                'files__row--ignored': item.data.ignored,
              }"
              role="button"
              tabindex="0"
              :style="{ height: `${ROW_HEIGHT}px`, paddingLeft: `${6 + item.data.depth * 12}px` }"
              @click="onRowClick(item.data)"
              @keydown.enter="onRowClick(item.data)"
              @contextmenu.prevent="onContextMenu($event, item.data)"
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
              <input
                v-if="editing && editing.mode === 'rename' && editing.path === item.data.path"
                v-model="editing.value"
                class="files__edit"
                type="text"
                @click.stop
                @keydown.enter.stop="submitEdit"
                @keydown.esc.stop="cancelEdit"
                @blur="cancelEdit"
              >
              <span
                v-else
                class="files__name"
              >{{ item.data.name }}</span>
              <input
                v-if="editing && editing.mode !== 'rename' && newEditorRowDir === item.data.path"
                v-model="editing.value"
                class="files__edit files__edit--below"
                type="text"
                @keydown.enter.stop="submitEdit"
                @keydown.esc.stop="cancelEdit"
                @blur="cancelEdit"
              >
            </div>
          </template>
        </div>
      </div>
    </template>

    <FileContextMenu
      v-if="menu"
      :x="menu.x"
      :y="menu.y"
      :target="menu.row"
      @action="onMenuAction"
      @close="menu = null"
    />

    <ConfirmDialog
      v-if="confirm"
      :title="t('files.context.deleteTitle')"
      :message="t('files.context.deleteMessage', { name: confirm.name })"
      :confirm-label="t('files.context.delete')"
      danger
      @confirm="doDelete"
      @cancel="confirm = null"
    />
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
  position: relative;
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

/* 内联输入（#62）：重命名就替换名字；新建锚在目标目录行下方。 */
.files__edit {
  flex: 1 1 auto;
  min-width: 0;
  padding: 0 var(--space-2);
  border: 1px solid var(--color-accent-line);
  border-radius: var(--radius-sm);
  background: var(--color-bg);
  color: var(--color-text);
  font-family: inherit;
  font-size: var(--font-size-sm);
  outline: none;
}

.files__edit--below {
  position: absolute;
  top: 100%;
  left: 0;
  right: 0;
  z-index: 5;
  height: 22px;
}

.files__edit--top {
  width: calc(100% - var(--space-4) * 2);
  height: 22px;
  margin: var(--space-1) var(--space-4);
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

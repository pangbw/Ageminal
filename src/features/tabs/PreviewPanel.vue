<script setup lang="ts">
/**
 * 文件只读预览面板（#60 / #61 / §9 / `CONTEXT.md` §5）。
 *
 * 单个共享 `EditorView`（[`PreviewController`]）+ 每页签 `EditorState`（含折叠 / 选区）+ 显式
 * 存滚动；切换页签不重建、不 re-parent DOM。内容按判定顺序呈现：文本走 CodeMirror，图片走
 * `<img>`（`data:` URL），二进制只给提示；护栏命中给顶部警示条。
 *
 * **文件变更实时刷新**（#61）：把当前打开着的预览文件交给 Rust 监听（`set_watched_files`），
 * 命中 `fs-change` 时——**正可见**页签就地事务替换文档并出角落轻提示「文件已更新」；**后台**
 * 页签只标 stale、激活时才刷新（不抢焦点）。
 */
import IconClose from "~icons/codicon/close";
import IconFile from "~icons/codicon/file";
import IconLinkExternal from "~icons/codicon/link-external";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";

import type { PreviewTab } from "../../stores/tabs";
import { commands } from "../../bindings";
import { translate as t } from "../../i18n/translate";
import { unwrap } from "../../lib/ipc";
import { useTabsStore } from "../../stores/tabs";
import type { PreviewContent } from "./preview/content";
import { PreviewController } from "./preview/controller";

/** 「文件已更新」轻提示的停留时长。 */
const UPDATED_HINT_MS = 2500;

/** Rust 回推的变更批次。 */
type FsChange = { paths: string[] };

const props = defineProps<{ root: string; tab: PreviewTab }>();
const emit = defineEmits<{ close: [] }>();

const tabs = useTabsStore();
const controller = new PreviewController();
const editorHost = ref<HTMLElement | null>(null);
const content = ref<PreviewContent | null>(null);
const error = ref<string | null>(null);
/** 「用外部编辑器打开」的失败（不遮挡预览本身）。 */
const actionError = ref<string | null>(null);
const loading = ref(false);
/** 「文件已更新」轻提示是否可见（#61）。 */
const updatedHint = ref(false);
/** 丢弃过期加载（快速切页签）。 */
let loadToken = 0;
let updatedTimer: ReturnType<typeof setTimeout> | undefined;
let unlisten: UnlistenFn | null = null;
let disposed = false;

const isText = computed(() => content.value?.kind === "text");
const hasWarnings = computed(
  () =>
    actionError.value !== null ||
    (content.value?.kind === "text" &&
      (content.value.lossy || content.value.truncated || content.value.longLine)),
);

async function load(root: string, path: string): Promise<void> {
  const token = (loadToken += 1);
  loading.value = true;
  error.value = null;
  actionError.value = null;
  try {
    // 后台期间变过（stale）的页签激活时强制重读（#61）。
    const stale = tabs.isStale(root, path);
    const entry = await controller.activate(root, path, { force: stale });
    if (token !== loadToken) return;
    tabs.clearStale(root, path);
    await present(entry.content);
  } catch (failure) {
    if (token !== loadToken) return;
    content.value = null;
    error.value = failure instanceof Error ? failure.message : String(failure);
  } finally {
    if (token === loadToken) loading.value = false;
  }
}

/** 把一份内容上屏并重新测量 / 恢复滚动。 */
async function present(next: PreviewContent): Promise<void> {
  content.value = next;
  await nextTick();
  controller.refresh();
}

/** 正可见页签的文件变了：就地重读，保留滚动，出轻提示（#61）。 */
async function refreshActive(): Promise<void> {
  const path = props.tab.path;
  const token = (loadToken += 1);
  try {
    const entry = await controller.reload(props.root, path);
    if (token !== loadToken || entry === null) return;
    tabs.clearStale(props.root, path);
    await present(entry.content);
    flashUpdated();
  } catch (failure) {
    if (token !== loadToken) return;
    error.value = failure instanceof Error ? failure.message : String(failure);
  }
}

/** 命中变更：正可见的立即刷新，其余的标 stale 等激活（#61）。 */
function onFsChange(payload: FsChange): void {
  const changed = new Set(payload.paths);
  if (changed.has(props.tab.path)) void refreshActive();
  for (const path of changed) {
    if (path !== props.tab.path) tabs.markStaleByPath(path);
  }
}

/** 角落轻提示：出现后自行消失，不打断阅读。 */
function flashUpdated(): void {
  updatedHint.value = true;
  clearTimeout(updatedTimer);
  updatedTimer = setTimeout(() => {
    updatedHint.value = false;
  }, UPDATED_HINT_MS);
}

function openExternal(): void {
  void commands
    .openInEditor(props.root, props.tab.path)
    .then(unwrap)
    .catch((failure: unknown) => {
      actionError.value = failure instanceof Error ? failure.message : String(failure);
    });
}

// 打开的预览文件集合一变，就把新的集合交给 Rust 监听（#61 只监听已打开的文件）。
watch(
  () => tabs.allPaths(),
  (paths) => {
    // 监听是尽力而为的旁路：失配不该打断预览。
    void commands
      .setWatchedFiles(paths)
      .then(unwrap)
      .catch(() => {});
  },
  { immediate: true },
);

onMounted(() => {
  if (editorHost.value) controller.mount(editorHost.value);
  void listen<FsChange>("fs-change", (event) => onFsChange(event.payload)).then((stop) => {
    if (disposed) stop();
    else unlisten = stop;
  });
});

onBeforeUnmount(() => {
  disposed = true;
  unlisten?.();
  clearTimeout(updatedTimer);
  controller.unmount();
});

watch(
  () => [props.root, props.tab.path] as const,
  ([root, path]) => {
    void load(root, path);
  },
  { immediate: true },
);
</script>

<template>
  <div class="preview">
    <header class="preview__head">
      <IconFile class="preview__icon" />
      <span
        class="preview__name"
        :title="tab.path"
      >{{ tab.name }}</span>
      <button
        class="icon-button preview__action"
        type="button"
        :title="t('tabs.openExternal')"
        @click="openExternal"
      >
        <IconLinkExternal />
      </button>
      <button
        class="icon-button preview__action"
        type="button"
        :title="t('tabs.close')"
        @click="emit('close')"
      >
        <IconClose />
      </button>
    </header>

    <div
      v-if="hasWarnings"
      class="preview__warnings"
    >
      <p
        v-if="actionError"
        class="preview__warning"
      >
        {{ actionError }}
      </p>
      <p
        v-if="content?.kind === 'text' && content.lossy"
        class="preview__warning"
      >
        {{ t("tabs.preview.invalidUtf8") }}
      </p>
      <p
        v-if="content?.kind === 'text' && content.truncated"
        class="preview__warning"
      >
        {{ t("tabs.preview.truncated") }}
      </p>
      <p
        v-if="content?.kind === 'text' && content.longLine"
        class="preview__warning"
      >
        {{ t("tabs.preview.longLine") }}
      </p>
    </div>

    <div class="preview__body">
      <div
        v-show="isText"
        ref="editorHost"
        class="preview__editor"
      />

      <p
        v-if="error"
        class="preview__note preview__note--error"
        role="alert"
      >
        {{ t("tabs.preview.error") }}: {{ error }}
      </p>
      <p
        v-else-if="loading"
        class="preview__note"
      >
        {{ t("tabs.preview.loading") }}
      </p>
      <p
        v-else-if="content?.kind === 'binary'"
        class="preview__note"
      >
        {{ t("tabs.preview.binary") }}
      </p>
      <template v-else-if="content?.kind === 'image'">
        <img
          v-if="content.url"
          class="preview__image"
          :src="content.url"
          :alt="tab.name"
        >
        <p
          v-else
          class="preview__note"
        >
          {{ t("tabs.preview.imageTooLarge") }}
        </p>
      </template>
    </div>

    <Transition name="preview__updated-fade">
      <div
        v-if="updatedHint"
        class="preview__updated"
        role="status"
      >
        {{ t("tabs.preview.updated") }}
      </div>
    </Transition>
  </div>
</template>

<style scoped>
.preview {
  position: relative;
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  min-height: 0;
}

.preview__head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex: 0 0 var(--pane-head-height);
  height: var(--pane-head-height);
  padding: 0 var(--space-4);
  border-bottom: 1px solid var(--color-border-soft);
  color: var(--color-muted);
  font-size: var(--font-size-sm);
}

.preview__icon {
  flex: 0 0 auto;
  color: var(--color-faint);
}

.preview__name {
  flex: 1 1 auto;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--color-text);
}

.preview__action {
  flex: 0 0 auto;
}

.preview__warnings {
  flex: 0 0 auto;
  padding: var(--space-2) var(--space-4);
  border-bottom: 1px solid var(--color-border-soft);
}

.preview__warning {
  margin: 0;
  color: var(--color-yellow);
  font-size: var(--font-size-xs);
}

.preview__body {
  position: relative;
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  min-height: 0;
  overflow: hidden;
}

.preview__editor {
  flex: 1 1 auto;
  width: 100%;
  min-height: 0;
}

/* CodeMirror 生成的 DOM 在这层组件内，尺寸交给宿主（§9：共享单个 EditorView）。 */
.preview__editor :deep(.cm-editor) {
  height: 100%;
}

.preview__image {
  max-width: 100%;
  max-height: 100%;
  object-fit: contain;
}

.preview__note {
  margin: 0;
  padding: var(--space-6);
  color: var(--color-faint);
  font-size: var(--font-size-sm);
  text-align: center;
}

.preview__note--error {
  color: var(--color-red);
}

/* 「文件已更新」轻提示（#61）：角落浮层，不接收指针事件，不打断阅读。 */
.preview__updated {
  position: absolute;
  right: var(--space-4);
  bottom: var(--space-4);
  padding: var(--space-1) var(--space-4);
  border: 1px solid var(--color-border);
  border-radius: var(--radius-sm);
  background: var(--color-element);
  color: var(--color-muted);
  font-size: var(--font-size-xs);
  pointer-events: none;
}

.preview__updated-fade-enter-active,
.preview__updated-fade-leave-active {
  transition: opacity 160ms ease;
}

.preview__updated-fade-enter-from,
.preview__updated-fade-leave-to {
  opacity: 0;
}
</style>

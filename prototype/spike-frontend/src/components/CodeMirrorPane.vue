<script setup lang="ts">
// CodeMirror 6 只读预览探针：验证命令式编辑器与 Vue 的集成（挂载一次、destroy 一次、切换文档用事务）
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import { basicSetup } from "codemirror";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { json } from "@codemirror/lang-json";

const props = defineProps<{ doc: string }>();

const host = ref<HTMLDivElement | null>(null);
let view: EditorView | null = null;

function makeState(doc: string): EditorState {
  return EditorState.create({
    doc,
    extensions: [
      basicSetup,
      json(),
      EditorView.editable.of(false), // 只读：不可编辑
      EditorState.readOnly.of(true), // 只读：拒绝事务
      EditorView.theme({
        "&": { height: "100%", backgroundColor: "#0d0d0d" },
        ".cm-scroller": { overflow: "auto", fontFamily: "Consolas, monospace" },
        ".cm-gutters": { backgroundColor: "#111", border: "none" },
      }),
    ],
  });
}

onMounted(() => {
  if (!host.value) return;
  view = new EditorView({ state: makeState(props.doc), parent: host.value });
});

// 切换文件用事务替换文档，而不是重建 view
watch(
  () => props.doc,
  (next) => {
    if (!view) return;
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: next },
    });
  }
);

onBeforeUnmount(() => {
  view?.destroy();
  view = null;
});
</script>

<template>
  <div ref="host" class="cm-host"></div>
</template>

<style scoped>
.cm-host {
  width: 100%;
  height: 100%;
  overflow: hidden;
}
</style>

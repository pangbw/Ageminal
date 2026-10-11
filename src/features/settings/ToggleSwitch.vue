<script setup lang="ts">
/**
 * 开关（原型 `.toggle`）：`modelValue` + `update:modelValue`。
 *
 * 用 `role="switch"` 而不是复选框：这是「展开 / 收起一栏」这类即时生效的开关，
 * 没有「提交」一说（§14：改动即刻走命令落盘）。
 */
const props = defineProps<{ modelValue: boolean; label: string }>();
const emit = defineEmits<{ "update:modelValue": [value: boolean] }>();

function toggle(): void {
  emit("update:modelValue", !props.modelValue);
}
</script>

<template>
  <button
    class="toggle"
    :class="{ 'toggle--on': props.modelValue }"
    type="button"
    role="switch"
    :aria-checked="props.modelValue"
    :aria-label="props.label"
    :title="props.label"
    @click="toggle()"
  >
    <span class="toggle__knob" />
  </button>
</template>

<style scoped>
.toggle {
  position: relative;
  flex: 0 0 auto;
  width: 30px;
  height: 17px;
  border-radius: 9px;
  background: var(--color-element);
  transition: background 0.12s;
}

.toggle--on {
  background: var(--color-accent-line);
}

.toggle__knob {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 13px;
  height: 13px;
  border-radius: 50%;
  background: var(--color-muted);
  transition:
    left 0.12s,
    background 0.12s;
}

.toggle--on .toggle__knob {
  left: 15px;
  background: var(--color-text);
}
</style>

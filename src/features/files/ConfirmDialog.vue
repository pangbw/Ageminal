<script setup lang="ts">
/**
 * 应用内确认弹层（#62）：删除等危险操作用。
 *
 * 刻意不用原生对话框：风格与外壳一致，且「红色强调」由 `danger` 控制（§9）。
 */
import { translate as t } from "../../i18n/translate";

defineProps<{
  title: string;
  message: string;
  confirmLabel: string;
  /** 危险操作：确认按钮用红色强调。 */
  danger?: boolean;
}>();

const emit = defineEmits<{ confirm: []; cancel: [] }>();
</script>

<template>
  <div class="confirm">
    <div
      class="confirm__backdrop"
      @click="emit('cancel')"
    />
    <div
      class="confirm__box"
      role="alertdialog"
      aria-modal="true"
    >
      <h2 class="confirm__title">
        {{ title }}
      </h2>
      <p class="confirm__message">
        {{ message }}
      </p>
      <div class="confirm__actions">
        <button
          class="confirm__button"
          type="button"
          @click="emit('cancel')"
        >
          {{ t("files.context.cancel") }}
        </button>
        <button
          class="confirm__button"
          :class="{ 'confirm__button--danger': danger }"
          type="button"
          @click="emit('confirm')"
        >
          {{ confirmLabel }}
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.confirm {
  position: fixed;
  inset: 0;
  z-index: 200;
  display: flex;
  align-items: center;
  justify-content: center;
}

.confirm__backdrop {
  position: absolute;
  inset: 0;
  background: rgba(3, 4, 4, 0.62);
}

.confirm__box {
  position: relative;
  width: 380px;
  max-width: calc(100vw - 32px);
  padding: var(--space-6);
  border: 1px solid var(--color-border);
  border-radius: var(--radius-lg);
  background: var(--color-elevated);
}

.confirm__title {
  margin: 0 0 var(--space-4);
  color: var(--color-text);
  font-size: var(--font-size-ui);
  font-weight: 600;
}

.confirm__message {
  margin: 0 0 var(--space-6);
  color: var(--color-muted);
  font-size: var(--font-size-sm);
}

.confirm__actions {
  display: flex;
  justify-content: flex-end;
  gap: var(--space-3);
}

.confirm__button {
  padding: var(--space-2) var(--space-5);
  border: 1px solid var(--color-border);
  border-radius: var(--radius-sm);
  background: var(--color-element);
  color: var(--color-text);
  font-size: var(--font-size-sm);
  cursor: pointer;
}

.confirm__button:hover {
  background: var(--color-hover);
}

.confirm__button--danger {
  border-color: var(--color-close-hover);
  background: var(--color-close-hover);
  color: var(--color-on-danger);
}
</style>

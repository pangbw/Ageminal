<script setup lang="ts">
/**
 * 窗口按钮（最小化 / 最大化还原 / 关闭）。
 *
 * 关窗语义在 #76 会变成「隐藏到托盘」（§12）；#55 阶段托盘还不存在，就是关窗。
 */
import IconChromeClose from "~icons/codicon/chrome-close";
import IconChromeMaximize from "~icons/codicon/chrome-maximize";
import IconChromeMinimize from "~icons/codicon/chrome-minimize";

import { translate as t } from "../../i18n/translate";
import { closeWindow, minimizeWindow, toggleMaximizeWindow } from "./window";
</script>

<template>
  <div class="window-controls">
    <button
      class="icon-button window-controls__button"
      type="button"
      :title="t('shell.windowControls.minimize')"
      @click="minimizeWindow()"
    >
      <IconChromeMinimize />
    </button>
    <button
      class="icon-button window-controls__button"
      type="button"
      :title="t('shell.windowControls.maximize')"
      @click="toggleMaximizeWindow()"
    >
      <IconChromeMaximize />
    </button>
    <button
      class="icon-button window-controls__button window-controls__button--close"
      type="button"
      :title="t('shell.windowControls.close')"
      @click="closeWindow()"
    >
      <IconChromeClose />
    </button>
  </div>
</template>

<style scoped>
.window-controls {
  display: flex;
  align-items: stretch;
  border-left: 1px solid var(--color-border-soft);
}

.window-controls__button {
  width: 44px;
  height: auto;
  border-radius: 0;
  color: var(--color-muted);
  font-size: 10px;
}

.window-controls__button--close:hover {
  background: var(--color-close-hover);
  color: var(--color-on-danger);
}
</style>

/**
 * 应用设置的前端投影（#69 起；完整设置页归 #86）。
 *
 * 单一事实来源仍是 Rust `settings.json`：这里只缓存 UI 需要的值，写入走命令并等成功再更新。
 * 目前只有「关闭运行中的终端页签是否二次确认」。
 */
import { defineStore } from "pinia";
import { ref } from "vue";

import { commands } from "../bindings";
import { unwrap } from "../lib/ipc";

export const usePreferencesStore = defineStore("preferences", () => {
  /** 默认 `true`：设置还没读回来时也先保护用户。 */
  const confirmClose = ref(true);
  const loaded = ref(false);

  /** 启动时拉一次；失败保持默认值，不影响使用。 */
  async function load(): Promise<void> {
    try {
      confirmClose.value = await commands.getConfirmClose();
    } catch (failure) {
      console.warn("[preferences] failed to load settings", failure);
    } finally {
      loaded.value = true;
    }
  }

  /** 写入并立即落盘；失败回滚内存值并抛出，让设置页能提示。 */
  async function setConfirmClose(enabled: boolean): Promise<void> {
    const previous = confirmClose.value;
    confirmClose.value = enabled;
    try {
      await commands.setConfirmClose(enabled).then(unwrap);
    } catch (failure) {
      confirmClose.value = previous;
      throw failure;
    }
  }

  return { confirmClose, loaded, load, setConfirmClose };
});

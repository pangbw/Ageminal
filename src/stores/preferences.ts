/**
 * 应用设置的前端投影（#69 起；完整设置页归 #86）。
 *
 * 单一事实来源仍是 Rust `settings.json`：这里只缓存 UI 需要的值，写入走命令并等成功再更新。
 * 目前只有「关闭运行中的终端页签是否二次确认」。
 */
import { defineStore } from "pinia";
import { computed, ref } from "vue";

import type {
  AppearanceInput,
  AppearanceView,
  GeneralInput,
  GeneralView,
} from "../bindings";
import { commands } from "../bindings";
import { unwrap } from "../lib/ipc";

export const usePreferencesStore = defineStore("preferences", () => {
  /** 默认 `true`：设置还没读回来时也先保护用户。 */
  const confirmClose = ref(true);
  const loaded = ref(false);
  const general = ref<GeneralView | null>(null);
  const appearance = ref<AppearanceView>({
    theme: "one-dark-pro-glass",
    fontSize: 13,
    lineHeight: 1.2,
    cursorStyle: "block",
    acrylic: false,
  });
  const startupBehavior = computed(() => general.value?.startupBehavior ?? "restore");

  /** 启动时拉一次；失败保持默认值，不影响使用。 */
  async function load(): Promise<void> {
    try {
      const [loadedGeneral, loadedAppearance] = await Promise.all([
        commands.generalSettings().then(unwrap),
        commands.appearanceSettings().then(unwrap),
      ]);
      general.value = loadedGeneral;
      appearance.value = loadedAppearance;
      confirmClose.value = loadedGeneral.confirmClose;
    } catch (failure) {
      // 旧命令作为兜底：设置命令暂不可用时至少保留关闭确认默认值。
      try {
        confirmClose.value = await commands.getConfirmClose();
      } catch {
        // keep defaults
      }
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
      if (general.value) general.value = { ...general.value, confirmClose: enabled };
    } catch (failure) {
      confirmClose.value = previous;
      throw failure;
    }
  }

  async function setGeneral(input: GeneralInput): Promise<GeneralView> {
    const view = await commands.setGeneralSettings(input).then(unwrap);
    general.value = view;
    confirmClose.value = view.confirmClose;
    return view;
  }

  async function setAppearance(input: AppearanceInput): Promise<AppearanceView> {
    const view = await commands.setAppearanceSettings(input).then(unwrap);
    appearance.value = view;
    return view;
  }

  return {
    confirmClose,
    general,
    appearance,
    startupBehavior,
    loaded,
    load,
    setConfirmClose,
    setGeneral,
    setAppearance,
  };
});

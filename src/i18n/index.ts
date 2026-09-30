/**
 * i18n 装配（REQUIREMENTS.md §11，issue #54）。
 *
 * - locale 文件按**域命名空间**分 JSON，由 `@intlify/unplugin-vue-i18n` **预编译**（见 `vite.config.ts`）；
 * - **zh-CN 是类型源**：key 拼错在 `vue-tsc` 阶段报错（走 `./translate` 的 `t()`）；
 * - 语言的持久化走 **Rust 设置**（`getLanguage` / `setLanguage`），**不用 localStorage**。
 */
import { createI18n } from "vue-i18n";

import { commands } from "../bindings";
import { FALLBACK_LOCALE, normalizeLocale, type AppLocale, type Locale } from "./detect";
import type { Namespace } from "./namespaces";

import zhCommon from "./locales/zh-CN/common.json";
import zhShell from "./locales/zh-CN/shell.json";
import zhTerminal from "./locales/zh-CN/terminal.json";
import zhTabs from "./locales/zh-CN/tabs.json";
import zhFiles from "./locales/zh-CN/files.json";
import zhAgents from "./locales/zh-CN/agents.json";
import zhSettings from "./locales/zh-CN/settings.json";
import zhErrors from "./locales/zh-CN/errors.json";

import enCommon from "./locales/en-US/common.json";
import enShell from "./locales/en-US/shell.json";
import enTerminal from "./locales/en-US/terminal.json";
import enTabs from "./locales/en-US/tabs.json";
import enFiles from "./locales/en-US/files.json";
import enAgents from "./locales/en-US/agents.json";
import enSettings from "./locales/en-US/settings.json";
import enErrors from "./locales/en-US/errors.json";

/** 一个域的文案树：叶子是字符串，中间是嵌套对象。 */
type MessageTree = { readonly [key: string]: string | MessageTree };

// `satisfies` 保证八个命名空间一个不缺、一个不多（NAMESPACES 是唯一事实来源）。
const zhCN = {
  common: zhCommon,
  shell: zhShell,
  terminal: zhTerminal,
  tabs: zhTabs,
  files: zhFiles,
  agents: zhAgents,
  settings: zhSettings,
  errors: zhErrors,
} satisfies Record<Namespace, MessageTree>;

const enUS = {
  common: enCommon,
  shell: enShell,
  terminal: enTerminal,
  tabs: enTabs,
  files: enFiles,
  agents: enAgents,
  settings: enSettings,
  errors: enErrors,
} satisfies Record<Namespace, MessageTree>;

/** 类型源：zh-CN 的 key 结构（value 的字面量类型不参与约束）。 */
export type MessageSchema = typeof zhCN;

const messages = {
  "zh-CN": zhCN,
  "en-US": enUS,
} satisfies Record<Locale, MessageTree>;

export const i18n = createI18n({
  legacy: false,
  locale: FALLBACK_LOCALE,
  fallbackLocale: FALLBACK_LOCALE,
  messages,
});

/** 定启动语言要问的三个地方；测试里可以整体替换。 */
export type LocaleProbe = {
  /** Rust 设置里已保存的语言；`null` = 未设置（跟随系统）。 */
  stored: () => Promise<string | null>;
  /** 系统 locale：Rust 的 `system_locale` 命令转出来的 `os.locale()`（前端不拿 `os:*` 权限）。 */
  system: () => Promise<string | null>;
  /** `navigator.language`。 */
  browser: () => string | null;
};

/** 生产用的探针；Tauri 宿主缺席（`pnpm dev:web`）时安全失败为 `null`。 */
const defaultProbe: LocaleProbe = {
  stored: async () => {
    try {
      return await commands.getLanguage();
    } catch {
      return null;
    }
  },
  system: async () => {
    try {
      return await commands.systemLocale();
    } catch {
      return null;
    }
  },
  browser: () => (typeof navigator === "undefined" ? null : navigator.language),
};

/**
 * 定出启动语言：**用户选过的 → 插件 `os.locale()` → `navigator.language` → 回退 `zh-CN`**
 * （`REQUIREMENTS.md` §11 的检测链，前面加上「用户选过就用它」——否则持久化没意义）。
 *
 * 逐跳短路的（不是先全部问完）：已经问到答案就不再付下一次 IPC；
 * 归不了的值（`de-DE`、已删除的旧语言）不算答案，继续往下一跳。
 */
export async function resolveInitialLocale(probe: LocaleProbe = defaultProbe): Promise<AppLocale> {
  const stored = normalizeLocale(await probe.stored());
  if (stored) return stored;

  const system = normalizeLocale(await probe.system());
  if (system) return system;

  return normalizeLocale(probe.browser()) ?? FALLBACK_LOCALE;
}

/** 切到某个语言：同步 i18n 与 `<html lang>`。能渲染的语言比能入列的宽（见 `detect.ts`）。 */
export function applyLocale(locale: Locale): void {
  i18n.global.locale.value = locale;
  if (typeof document !== "undefined") {
    document.documentElement.lang = locale;
  }
}

/** 切换并**持久化**语言；`null` = 恢复「跟随系统」（设置页用，见 #86）。 */
export async function setLocale(locale: AppLocale | null): Promise<void> {
  const result = await commands.setLanguage(locale);
  if (result.status === "error") {
    throw new Error(result.error);
  }
  applyLocale(locale ?? (await resolveInitialLocale()));
}

// 这里**故意不**增强 vue-i18n 的 `DefineLocaleMessage`：它只给 `t()` 加提示，
// 拦不住拼错的 key（`t` 的签名是 `<Key extends string>(key: Key | ...)`）。
// 真要编译期报错，用 `translate`（见 `./translate.ts`）。

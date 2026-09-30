/**
 * 界面语言的**归一化与择优**（REQUIREMENTS.md §11，issue #54）。
 *
 * 启动语言 = 用户选过的 → `插件 os.locale()` → `navigator.language` → 回退 `zh-CN`；
 * 本模块只管其中不需要副作用的纯函数，取系统 locale 的活儿在 `index.ts`。
 */

/**
 * **有内容**的语言：`locales/<locale>/` 下有 JSON，key 结构必须与 zh-CN 对齐。
 * 它比「可被选中」宽——en-US 先按 §11 对齐结构、内容补齐后再入列。
 */
export const LOCALES = ["zh-CN", "en-US"] as const;

export type Locale = (typeof LOCALES)[number];

/** MVP 允许**被检测到 / 被选中**的语言（REQUIREMENTS.md §11：语言列表只列 zh-CN）。 */
export const SELECTABLE_LOCALES = ["zh-CN"] as const satisfies readonly Locale[];

export type AppLocale = (typeof SELECTABLE_LOCALES)[number];

/** 所有候选都归不了时的回退。 */
export const FALLBACK_LOCALE: AppLocale = "zh-CN";

/**
 * 把一个 BCP-47 标签归一到**可入列**的语言；归不了则返回 `null`，交给下一个候选。
 *
 * - `zh` / `zh-CN` / `zh-Hans*` / `zh-SG` → `zh-CN`
 * - `en-*` / `de-DE` 之类 → `null`（en-US 还没入列，见 `SELECTABLE_LOCALES`）
 */
export function normalizeLocale(tag: string | null | undefined): AppLocale | null {
  const normalized = tag?.trim().replace(/_/g, "-").toLowerCase();
  if (!normalized) return null;

  if (normalized === "zh" || normalized.startsWith("zh-")) return "zh-CN";
  return null;
}

/**
 * 界面语言的**归一化与择优**（REQUIREMENTS.md §11，issue #54）。
 *
 * 检测链是 `插件 os.locale()` → `navigator.language` → 回退 `zh-CN`；
 * 本模块只管其中不需要副作用的纯函数，取系统 locale 的活儿在 `index.ts`。
 */

/** 有内容的语言（`locales/<locale>/` 下有对应 JSON）。 */
export const SUPPORTED_LOCALES = ["zh-CN", "en-US"] as const;

export type AppLocale = (typeof SUPPORTED_LOCALES)[number];

/** 所有候选都不认识时的回退。 */
export const FALLBACK_LOCALE: AppLocale = "zh-CN";

/**
 * 把一个 BCP-47 标签归一到有内容的语言；不认识则返回 `null`，交给下一个候选。
 *
 * - `zh` / `zh-CN` / `zh-Hans*` / `zh-SG` → `zh-CN`（简体）
 * - `zh-TW` / `zh-HK` / `zh-MO` / `zh-Hant*` → `null`（繁体还没有内容，往下落）
 * - `en` / `en-US` / `en-GB` / `en-*` → `en-US`（英文只有这一份内容）
 * - `de-DE` 之类 → `null`
 */
export function normalizeLocale(tag: string | null | undefined): AppLocale | null {
  const normalized = tag?.trim().replace(/_/g, "-").toLowerCase();
  if (!normalized) return null;

  if (normalized === "en" || normalized.startsWith("en-")) return "en-US";
  if (normalized === "zh" || normalized.startsWith("zh-")) {
    return isTraditionalChinese(normalized) ? null : "zh-CN";
  }
  return null;
}

/**
 * 按优先级取第一个能归一的候选；全都归不了才回退。
 *
 * `pickLocale([stored, osLocale, navigatorLanguage])` —— 未知标签（如 `de-DE`）
 * 不会拦路，而是继续看下一跳。
 */
export function pickLocale(candidates: readonly (string | null | undefined)[]): AppLocale {
  for (const candidate of candidates) {
    const locale = normalizeLocale(candidate);
    if (locale) return locale;
  }
  return FALLBACK_LOCALE;
}

function isTraditionalChinese(tag: string): boolean {
  return (
    tag.startsWith("zh-hant") || tag === "zh-tw" || tag === "zh-hk" || tag === "zh-mo"
  );
}

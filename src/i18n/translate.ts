/**
 * 强类型 `t`（REQUIREMENTS.md §11：**以 zh-CN 为类型源，key 拼错编译期报错**）。
 *
 * vue-i18n 自己的 `t()` 是 `<Key extends string>(key: Key | ResourceKeys | number)`：
 * `ResourceKeys` 只是提示，随便一个字符串都能过。所以这里把 key 收窄成
 * **zh-CN 文案树的叶子路径**，`t("common.appNamX")` 会在 `vue-tsc` 阶段报错。
 */
import { i18n, type MessageSchema } from "./index";

/** 把一个文案树的叶子拍成 `shell.appInfo.loading` 这样的路径联合。 */
export type MessagePath<T = MessageSchema> = {
  [K in keyof T & string]: T[K] extends string ? K : `${K}.${MessagePath<T[K]>}`;
}[keyof T & string];

/** 翻译一个 key；命名参数用 `{ name }` 占位符。 */
export function translate(key: MessagePath, named?: Record<string, unknown>): string {
  return named ? i18n.global.t(key, named) : i18n.global.t(key);
}

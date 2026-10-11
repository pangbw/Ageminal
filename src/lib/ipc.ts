/**
 * 命令返回值解包（`src/bindings.ts` 的 `typedError` 包裹）。
 *
 * `Result<T, E>` 在绑定里是 `{ status: "ok", data } | { status: "error", error }`；
 * 这里把错误抛出来，让调用方用常规 try / catch 处理（错误文案由 Rust 侧给出）。
 */

/** 绑定里 `typedError` 的形状。 */
export type CommandResult<T> = { status: "ok"; data: T } | { status: "error"; error: string };

/** 解包命令结果；出错就抛。 */
export function unwrap<T>(result: CommandResult<T>): T {
  if (result.status === "error") throw new Error(result.error);
  return result.data;
}

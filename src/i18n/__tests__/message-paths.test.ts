import { describe, expectTypeOf, it } from "vitest";

import type { MessagePath } from "../translate";

/**
 * 类型源守门：zh-CN 的叶子路径就是 `t()` 允许的全部 key。
 * 这些断言由 `pnpm typecheck`（vue-tsc）执行，运行时不做任何事。
 */
describe("MessagePath", () => {
  it("accepts the paths that exist in the zh-CN tree", () => {
    expectTypeOf<"common.appName">().toExtend<MessagePath>();
    expectTypeOf<"shell.appInfo.loading">().toExtend<MessagePath>();
    expectTypeOf<"shell.appInfo.error">().toExtend<MessagePath>();
    expectTypeOf<"shell.next.hint">().toExtend<MessagePath>();
  });

  it("rejects typos, unknown namespaces and intermediate nodes", () => {
    expectTypeOf<"common.appNamX">().not.toExtend<MessagePath>();
    expectTypeOf<"nope.key">().not.toExtend<MessagePath>();
    expectTypeOf<"shell.appInfo">().not.toExtend<MessagePath>();
    expectTypeOf<"">().not.toExtend<MessagePath>();
  });
});

/**
 * 工作区投影（C8 #16 D2；CONTEXT.md §0：前端 store 只是 Rust 快照的**内存投影**）。
 *
 * #55 只用到「激活 worktree」——窗口标题要跟着它走（REQUIREMENTS.md §6）。真正的项目 /
 * worktree 列表由 #56 的 git 发现填进来，届时这里换成 pins 的规范化形态
 * （`Record<id, T>` + 顺序数组，C8 #16 D4），类型也改用 specta 生成物（D8）。
 */
import { defineStore } from "pinia";
import { ref } from "vue";

/** 窗口标题需要的两个字段；#56 的 worktree 类型落地后由其取代。 */
export type ActiveWorktree = {
  /** worktree 显示名（主工作树与普通工作树的取名规则见 §6）。 */
  name: string;
  /** 分支名；detached HEAD 由 #56 归一成可显示的短 SHA。 */
  branch: string;
};

export const useWorkspaceStore = defineStore("workspace", () => {
  const activeWorktree = ref<ActiveWorktree | null>(null);

  // 只暴露状态：填充发生在 #56（Rust 快照 hydrate / 侧栏点击）。
  return { activeWorktree };
});

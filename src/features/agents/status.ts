/**
 * Agent 统一状态的展示派生（#82）。
 *
 * 纯函数只负责「多个终端页签 → worktree 行一个状态」的聚合与色调；具体文案走 i18n。
 * 词表见 `crates/agent`（#80），降级状态由 `degraded` 显式标注（#81）。
 */
import type { AgentStatusState, AgentStatusView } from "../../bindings";

/** 状态点在 UI 上的色调。 */
export type StatusTone = "running" | "attention" | "done" | "error" | "idle" | "exited";

/**
 * worktree 行聚合优先级：先显示需要用户处理的状态。
 *
 * 多个 agent 页签并存时，permission / waiting 最需要被看到；error 次之；done 保留完成信号；
 * running 作为普通活动；idle / exited 最低。
 */
const STATUS_PRIORITY: Record<AgentStatusState, number> = {
  permission: 7,
  waiting: 6,
  error: 5,
  done: 4,
  running: 3,
  idle: 2,
  exited: 1,
};

/** 状态 → 彩点色调。 */
export function statusTone(state: AgentStatusState): StatusTone {
  switch (state) {
    case "running":
      return "running";
    case "waiting":
    case "permission":
      return "attention";
    case "done":
      return "done";
    case "error":
      return "error";
    case "idle":
      return "idle";
    case "exited":
      return "exited";
  }
}

/** 从一个 worktree 的所有终端页签状态里选最需要展示的一条。 */
export function aggregateStatus(
  statuses: readonly (AgentStatusView | null | undefined)[],
): AgentStatusView | null {
  let best: AgentStatusView | null = null;
  for (const status of statuses) {
    if (!status) continue;
    if (best === null || STATUS_PRIORITY[status.state] > STATUS_PRIORITY[best.state]) {
      best = status;
      continue;
    }
    // 同一优先级下优先暴露「不完整」的降级状态，避免 worktree 行把不确定性藏掉。
    if (
      STATUS_PRIORITY[status.state] === STATUS_PRIORITY[best.state] &&
      status.degraded &&
      !best.degraded
    ) {
      best = status;
    }
  }
  return best;
}

/** 状态点的可访问文案 key；`degraded` 由调用方追加标注。 */
export function statusLabelKey(state: AgentStatusState):
  | "agents.status.running"
  | "agents.status.waiting"
  | "agents.status.permission"
  | "agents.status.done"
  | "agents.status.idle"
  | "agents.status.error"
  | "agents.status.exited" {
  return `agents.status.${state}`;
}

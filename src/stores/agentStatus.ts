/**
 * Agent 统一状态投影（#80 / #81 / #82）。
 *
 * Rust 的 `agent-status` 全局事件是实时事实；`agent_statuses` 命令负责 WebView 重载 /
 * 挂载时补水。store 只是内存投影，不落盘（`state.json` 明确不保存运行期状态）。
 */
import { isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { defineStore } from "pinia";
import { ref } from "vue";

import {
  commands,
  type AgentStatusState,
  type AgentStatusView,
} from "../bindings";

/** `agent-status` 全局事件负载（Rust `AgentStatusEvent`）。 */
export type AgentStatusEvent = {
  previous: AgentStatusState | null;
  current: AgentStatusView;
  changed: boolean;
};

export const useAgentStatusStore = defineStore("agentStatus", () => {
  const bySid = ref<Record<string, AgentStatusView>>({});
  let unlisten: UnlistenFn | null = null;
  let starting: Promise<void> | null = null;

  /** 事件与补水快照可能乱序；时间戳相等时保留先到的一份（通常是实时事件）。 */
  function apply(status: AgentStatusView): void {
    const existing = bySid.value[status.sid];
    if (existing) {
      const incomingAt = status.timestampMs ?? 0;
      const existingAt = existing.timestampMs ?? 0;
      if (existingAt >= incomingAt) return;
    }
    bySid.value = { ...bySid.value, [status.sid]: status };
  }

  function applyEvent(event: AgentStatusEvent): void {
    apply(event.current);
  }

  async function hydrate(): Promise<void> {
    if (!isTauri()) return;
    try {
      const statuses = await commands.agentStatuses();
      for (const status of statuses) apply(status);
    } catch (failure) {
      console.warn("[agent-status] hydrate failed", failure);
    }
  }

  async function ensureListening(): Promise<void> {
    if (!isTauri() || unlisten !== null) return;
    try {
      unlisten = await listen<AgentStatusEvent>("agent-status", (event) =>
        applyEvent(event.payload),
      );
    } catch (failure) {
      console.warn("[agent-status] listen failed", failure);
    }
  }

  /** 启动监听 + 补水；并发调用共享同一次。 */
  function start(): Promise<void> {
    if (starting !== null) return starting;
    starting = (async () => {
      await ensureListening();
      await hydrate();
    })().finally(() => {
      starting = null;
    });
    return starting;
  }

  function stop(): void {
    unlisten?.();
    unlisten = null;
  }

  function statusFor(sid: string): AgentStatusView | null {
    return bySid.value[sid] ?? null;
  }

  function remove(sid: string): void {
    if (!(sid in bySid.value)) return;
    const next = { ...bySid.value };
    delete next[sid];
    bySid.value = next;
  }

  return { bySid, apply, applyEvent, start, stop, hydrate, statusFor, remove };
});

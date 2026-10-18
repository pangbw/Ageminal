/**
 * Agent 配置投影（#77）。
 *
 * Rust `settings.json` 是单一事实来源：启动 / 打开菜单时拉一次；保存一个 agent 后拿回完整清单。
 * 这里额外保留一份与 Rust 缺省值一致的兜底，保证 `pnpm dev:web` 或 IPC 暂时失败时入口不消失。
 */
import { defineStore } from "pinia";
import { ref } from "vue";

import { commands, type AgentConfig } from "../bindings";
import { FALLBACK_AGENTS } from "../features/agents/defaults";
import { unwrap } from "../lib/ipc";

export const useAgentsStore = defineStore("agents", () => {
  const list = ref<AgentConfig[]>(FALLBACK_AGENTS.map((agent) => ({ ...agent })));
  const loaded = ref(false);
  let pending: Promise<void> | null = null;

  function apply(next: AgentConfig[]): void {
    // 只认四家 canonical id；dsh / 未知项即使从旧设置里冒出来也不进菜单。
    const byId = new Map(next.map((agent) => [agent.id, agent]));
    list.value = FALLBACK_AGENTS.map((fallback) => {
      const agent = byId.get(fallback.id);
      return agent ? { ...agent } : { ...fallback };
    });
  }

  /** 拉取一次配置；失败保留兜底，不阻塞应用启动。 */
  async function load(): Promise<void> {
    try {
      apply(await commands.getAgents().then(unwrap));
    } catch (failure) {
      console.warn("[agents] failed to load settings; using defaults", failure);
    } finally {
      loaded.value = true;
    }
  }

  /** 启动、菜单、恢复页签都可以调；并发调用共享同一次请求。 */
  function ensureLoaded(): Promise<void> {
    if (loaded.value) return Promise.resolve();
    if (pending === null) {
      pending = load().finally(() => {
        pending = null;
      });
    }
    return pending;
  }

  /** 保存一个 agent；成功后用 Rust 返回的完整清单替换投影。 */
  async function save(config: AgentConfig): Promise<void> {
    const next = await commands.setAgent(config).then(unwrap);
    apply(next);
    loaded.value = true;
  }

  function byId(id: string): AgentConfig | undefined {
    return list.value.find((agent) => agent.id === id);
  }

  return { list, loaded, ensureLoaded, save, byId };
});

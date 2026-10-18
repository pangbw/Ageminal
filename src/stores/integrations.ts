/**
 * Agent 集成安装状态投影（#83）。
 *
 * Rust 是文件系统与状态的事实来源；这里只缓存最近一次 `integration_list` 快照，
 * 并在 apply / disable / test 后整体刷新。
 */
import { defineStore } from "pinia";
import { ref } from "vue";

import {
  commands,
  type IntegrationApplyResult,
  type IntegrationPreview,
  type IntegrationTestResult,
  type IntegrationView,
} from "../bindings";
import { unwrap } from "../lib/ipc";

export const useIntegrationsStore = defineStore("integrations", () => {
  const list = ref<IntegrationView[]>([]);
  const loading = ref(false);

  async function refresh(): Promise<void> {
    loading.value = true;
    try {
      list.value = await commands.integrationList().then(unwrap);
    } finally {
      loading.value = false;
    }
  }

  function byId(agentId: string): IntegrationView | undefined {
    return list.value.find((item) => item.agentId === agentId);
  }

  async function preview(agentId: string): Promise<IntegrationPreview> {
    return commands.integrationPreview(agentId).then(unwrap);
  }

  async function apply(agentId: string): Promise<IntegrationApplyResult> {
    const result = await commands.integrationApply(agentId).then(unwrap);
    await refresh();
    return result;
  }

  async function disable(agentId: string): Promise<IntegrationApplyResult> {
    const result = await commands.integrationDisable(agentId).then(unwrap);
    await refresh();
    return result;
  }

  async function testDelivery(agentId: string): Promise<IntegrationTestResult> {
    const result = await commands.integrationTestDelivery(agentId).then(unwrap);
    await refresh();
    return result;
  }

  return { list, loading, refresh, byId, preview, apply, disable, testDelivery };
});

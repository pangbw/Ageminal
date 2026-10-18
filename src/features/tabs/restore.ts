/**
 * 启动恢复与四象限对账的前端接线（#78）。
 *
 * 对账本身在 Rust（`restore::reconcile`，纯函数）：它把 `state.json` 的页签骨架 / launch
 * spec 与 daemon 的 `ListSessions` 求交，给每条页签标出 `attach / resume / rebuild / adopt /
 * preview`。这里只把结果投影进 Pinia：
 * - 终端页签保留 `sid`，面板首次挂载时 `terminal_open` 会走 attach 或按 launch spec 重建；
 * - agent 页签从设置投影拿名称 / 颜色；孤儿没有持久标题时用 agent 名或「终端」。
 *
 * preview 页签没有会话，直接恢复骨架；文件不存在时由预览面板显示自己的错误。
 */
import { commands } from "../../bindings";
import { translate as t } from "../../i18n/translate";
import { unwrap } from "../../lib/ipc";
import { useAgentsStore } from "../../stores/agents";
import { useTabsStore, type HydratedWorktree, type Tab } from "../../stores/tabs";

/** 拉取对账结果并重建页签；返回恢复的数量。 */
export async function restoreWorkspaceTabs(): Promise<number> {
  const [groups] = await Promise.all([
    commands.restoreTabs().then(unwrap),
    useAgentsStore().ensureLoaded(),
  ]);
  const tabs = useTabsStore();
  const agents = useAgentsStore();
  const hydrated: HydratedWorktree[] = [];

  for (const group of groups) {
    const restored: Tab[] = [];
    for (const tab of group.tabs) {
      if (tab.kind === "preview") {
        if (!tab.path) continue;
        restored.push({
          kind: "preview",
          path: tab.path,
          name: tab.name,
          stale: false,
        });
        continue;
      }
      if (!tab.sid) continue;
      const agent = tab.agentId ? agents.byId(tab.agentId) : undefined;
      restored.push({
        kind: "terminal",
        sid: tab.sid,
        cwd: tab.cwd ?? group.root,
        name: tab.name || agent?.name || t("terminal.defaultName"),
        agentId: agent?.id ?? tab.agentId ?? undefined,
        agentColor: agent?.color,
      });
    }
    if (restored.length > 0) {
      hydrated.push({
        root: group.root,
        active: group.active ?? null,
        tabs: restored,
      });
    }
  }

  tabs.hydrate(hydrated);
  return hydrated.reduce((count, group) => count + group.tabs.length, 0);
}

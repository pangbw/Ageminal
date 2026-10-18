/**
 * 启动恢复终端页签（issue #75）。
 *
 * UI 进程关闭 / WebView reload 后，daemon 里的会话仍然活着。新前端先向 app-core 要
 * `terminal_list`，再按 daemon 报告的 `sid` / `cwd` 重建终端页签；等面板真正挂载时，
 * `terminal_open` 会发现会话已存在并走 `Attach` 快照，而不是新建。
 *
 * 完整的 `state.json` 页签骨架、死会话重建 / agent resume 与四象限对账归 #78；这里只恢复
 * **daemon 里确实存活**的会话。
 */
import { commands } from "../../bindings";
import { translate as t } from "../../i18n/translate";
import { unwrap } from "../../lib/ipc";
import { useAgentsStore } from "../../stores/agents";
import { useTabsStore } from "../../stores/tabs";

/** 拉取存活会话并重建页签；返回恢复的数量。 */
export async function restoreTerminalTabs(): Promise<number> {
  const [sessions] = await Promise.all([
    commands.terminalList().then(unwrap),
    useAgentsStore().ensureLoaded(),
  ]);
  const tabs = useTabsStore();
  const agents = useAgentsStore();
  for (const session of sessions) {
    const agent = session.agentId ? agents.byId(session.agentId) : undefined;
    tabs.openTerminal(
      session.cwd,
      session.sid,
      session.cwd,
      agent?.name ?? t("terminal.defaultName"),
      agent ? { agentId: agent.id, agentColor: agent.color } : undefined,
    );
  }
  return sessions.length;
}

/**
 * 页签动作（#69）：打开终端 / 预览，以及带确认的关闭。
 *
 * 关闭运行中的终端页签要先确认（REQUIREMENTS.md §7.3），包括「关闭其他 / 关闭右侧」这种批量
 * 操作。确认弹层由 `App.vue` 渲染，状态放这里，任何组件都能 `requestClose`。
 */
import { ref } from "vue";

import { type AgentConfig } from "../../bindings";
import { translate as t } from "../../i18n/translate";
import { usePreferencesStore } from "../../stores/preferences";
import { useTabsStore, type Tab, type TerminalTab } from "../../stores/tabs";
import { close as closeTerminalSession, terminalState } from "../terminal/runtime";

/** 等待用户确认的一次关闭（可能包含多个页签）。 */
export type PendingClose = {
  root: string;
  tabs: Tab[];
  message: string;
};

/** 非空时 App 渲染确认弹层。 */
export const pendingClose = ref<PendingClose | null>(null);

/** 终端会话的逻辑 id；与保存的 launch spec（#78）共用这个形状。 */
function newTerminalSid(root: string): string {
  const unique =
    typeof crypto !== "undefined" && "randomUUID" in crypto
      ? crypto.randomUUID()
      : `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
  return `terminal:${root}:${unique}`;
}

/** 新建一个终端页签；会话在面板挂载时才真正创建。 */
export function openTerminalTab(root: string): TerminalTab {
  return useTabsStore().openTerminal(
    root,
    newTerminalSid(root),
    root,
    t("terminal.defaultName"),
  );
}

/** 新建一个 agent 终端页签（#77）；命令由 app-core 按设置解析，面板挂载时启动。 */
export function openAgentTab(root: string, agent: AgentConfig): TerminalTab {
  return useTabsStore().openTerminal(
    root,
    newTerminalSid(root),
    root,
    agent.name,
    { agentId: agent.id, agentColor: agent.color },
  );
}

/** 打开文件预览页签（+ 菜单 / 文件选择器之后使用）。 */
export function openPreviewTab(root: string, path: string, name: string): void {
  useTabsStore().openFile(root, path, name);
}

/** 关闭一组页签；运行中的终端先弹确认。 */
export function requestClose(root: string, tabs: readonly Tab[]): void {
  if (tabs.length === 0) return;

  const preferences = usePreferencesStore();
  const running = tabs.filter(isRunning);
  if (running.length === 0 || !preferences.confirmClose) {
    performClose(root, tabs);
    return;
  }

  pendingClose.value = {
    root,
    tabs: [...tabs],
    message:
      running.length === 1
        ? t("tabs.closeRunningMessage", { name: running[0].name })
        : t("tabs.closeRunningManyMessage", { count: running.length }),
  };
}

/** 确认弹层的「仍然关闭」。 */
export function confirmPendingClose(): void {
  const pending = pendingClose.value;
  pendingClose.value = null;
  if (pending) performClose(pending.root, pending.tabs);
}

/** 确认弹层的「取消」。 */
export function cancelPendingClose(): void {
  pendingClose.value = null;
}

/** 关闭其他页签（保留 `keep`），批量关闭同样走确认。 */
export function requestCloseOthers(root: string, keep: Tab): void {
  const tabs = useTabsStore();
  const id = tabs.tabId(keep);
  requestClose(
    root,
    tabs.tabsFor(root).filter((tab) => tabs.tabId(tab) !== id),
  );
}

/** 关闭 `target` 右侧的全部页签。 */
export function requestCloseRight(root: string, target: Tab): void {
  const tabs = useTabsStore();
  const id = tabs.tabId(target);
  const list = tabs.tabsFor(root);
  const index = list.findIndex((tab) => tabs.tabId(tab) === id);
  if (index === -1) return;
  requestClose(root, list.slice(index + 1));
}

function performClose(root: string, tabs: readonly Tab[]): void {
  const store = useTabsStore();
  store.closeTabs(
    root,
    tabs.map((tab) => store.tabId(tab)),
  );
  for (const tab of tabs) {
    if (tab.kind === "terminal") void closeTerminalSession(tab.sid);
  }
}

/** 终端页签是否还可能持有活会话；状态还没建出来时按「运行中」保守处理。 */
function isRunning(tab: Tab): boolean {
  if (tab.kind !== "terminal") return false;
  const state = terminalState(tab.sid);
  return state === undefined || state.status === "starting" || state.status === "ready";
}

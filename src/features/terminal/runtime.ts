/**
 * 终端运行时（issue #68）：xterm 实例池 + 每会话 Channel + 可见性 / ack 记账。
 *
 * 三条边界：
 * - xterm 实例与宿主 `<div>` **不进 Pinia**（CONTEXT.md §0.2）；本模块用普通 `Map` 持有。
 * - 只有**当前可见**的页签在收流；不可见时向 app-core 发 `set_visible(false)`，由它 `Detach`。
 * - 合批与背压在 Rust 侧（≥16 KiB / ≥8 ms）；这里按 xterm `write` 回调做累计 ack，
 *   每 32 KiB 或空闲 25 ms 发一次（#73 的 256 / 64 KiB 高低水位输入）。
 * - 切回可见时 app-core 发 ANSI 快照，前端 `reset()` 后重放（#73）；UI 重开走
 *   `terminal_list` + `Attach(0)` 快照，真正的重连在回放边界后写「已重新连接」（#75）。
 */
import { Channel } from "@tauri-apps/api/core";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { Terminal, type IDisposable } from "@xterm/xterm";
import { reactive } from "vue";

import type { AppearanceView } from "../../bindings";
import { commands } from "../../bindings";
import { translate as t } from "../../i18n/translate";
import { errorMessage, unwrap } from "../../lib/ipc";
import { usePreferencesStore } from "../../stores/preferences";
import { useTabsStore } from "../../stores/tabs";
import { windowsBuildNumber } from "./platform";

import "@xterm/xterm/css/xterm.css";

/** app-core 通过同一条 Channel 的 JSON 帧发来的控制消息。 */
export type TerminalControl =
  | { type: "exit"; code: number | null }
  | {
      type: "attached";
      fromSeq: number;
      replayEnd: number;
      snapshot: string | null;
      altScreen: boolean;
      reconnected: boolean;
      cols: number;
      rows: number;
    }
  | { type: "replayEnd"; altScreen: boolean; reconnected: boolean }
  | {
      type: "snapshot";
      ansi: string;
      fromSeq: number;
      altScreen: boolean;
      reconnected: boolean;
    }
  | { type: "error"; message: string }
  | { type: "closed"; reason: string };

/** 一个终端的 UI 侧状态（可响应式读取）。 */
export type TerminalState = {
  sid: string;
  cwd: string;
  status: "starting" | "ready" | "exited" | "error";
  error: string | null;
  exitCode: number | null;
  visible: boolean;
  /** 刚经历过 attach 回放 / 快照（#72 的可见提示；分隔线本身归 #75）。 */
  reconnected: boolean;
};

/** xterm 实例 + 宿主容器；与 UI 状态分开，避免被 Vue 深度代理。 */
type Pooled = {
  term: Terminal;
  fit: FitAddon;
  search: SearchAddon;
  host: HTMLDivElement;
  input: IDisposable;
  /** WebGL 只在可见终端上挂载；失败 / context lost 后回到 DOM 渲染。 */
  webgl: WebglAddon | null;
  webglFailed: boolean;
  /** xterm 已解析的字节数（本次 attach 内）。 */
  parsed: number;
  /** 已报给 app-core 的字节数。 */
  acked: number;
  /** attach 世代：旧写入的 xterm 回调不能污染新 ack 基点。 */
  epoch: number;
  /** 空闲补 ack 的定时器（高水位暂停后，最后不足 32 KiB 的尾巴也必须确认）。 */
  ackTimer: number | undefined;
  /** daemon 当前 VT 尺寸；与面板一致时跳过 resize，避免 ConPTY 重绘清掉分隔线。 */
  serverCols: number;
  serverRows: number;
};

/** 一次 ack 的字节阈值（VS Code 档位 32 KiB）。 */
const ACK_BYTES = 32 * 1024;

/** 空闲补 ack 的等待时间：保证背压暂停时最后一块也能被确认。 */
const ACK_IDLE_MS = 25;

const states = reactive(new Map<string, TerminalState>());
const pooledTerms = new Map<string, Pooled>();
const opened = new Set<string>();
const opening = new Map<string, Promise<void>>();
const searchRequests = new Map<string, Set<() => void>>();

/** 未挂载时的暂存区：必须可见 / 有尺寸，xterm 首次 `open()` 才量得到格子大小。 */
let offstage: HTMLDivElement | null = null;

function stage(): HTMLDivElement {
  if (offstage === null) {
    const element = document.createElement("div");
    element.setAttribute("aria-hidden", "true");
    element.style.cssText =
      "position:fixed;left:-10000px;top:0;width:800px;height:600px;overflow:hidden;pointer-events:none;";
    document.body.appendChild(element);
    offstage = element;
  }
  return offstage;
}

/** 取会话 UI 状态（未创建时返回 undefined）。 */
export function terminalState(sid: string): TerminalState | undefined {
  return states.get(sid);
}

/** 取（必要时创建）xterm 实例。整个生命周期只 `open()` 一次。 */
export function ensureTerm(sid: string): Pooled {
  const existing = pooledTerms.get(sid);
  if (existing) return existing;

  const host = document.createElement("div");
  host.className = "terminal-host";
  host.dataset.renderer = "dom";
  host.style.cssText = "width:100%;height:100%;";
  stage().appendChild(host);

  const appearance = usePreferencesStore().appearance;
  host.dataset.fontSize = String(appearance.fontSize);
  host.dataset.lineHeight = String(appearance.lineHeight ?? "");
  const term = new Terminal({
    allowProposedApi: true,
    convertEol: false,
    scrollback: 10_000,
    allowTransparency: false,
    cursorBlink: true,
    cursorStyle: appearance.cursorStyle,
    fontFamily: '"Ageminal Mono", "Cascadia Mono", Consolas, "Courier New", monospace',
    fontSize: appearance.fontSize,
    lineHeight: appearance.lineHeight ?? undefined,
    rightClickSelectsWord: false,
    windowsPty: { backend: "conpty", buildNumber: windowsBuildNumber() },
    linkHandler: {
      activate: (_event, text) => openExternal(text),
      allowNonHttpProtocols: false,
    },
    theme: {
      background: "#080909",
      foreground: "#d4d4d4",
      cursor: "#4fc1ff",
    },
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.loadAddon(new Unicode11Addon());
  const search = new SearchAddon();
  term.loadAddon(search);
  term.loadAddon(
    new WebLinksAddon((event, uri) => {
      event.preventDefault();
      openExternal(uri);
    }),
  );
  term.onTitleChange((title) => {
    const name = title.trim();
    if (name) useTabsStore().setTerminalTitle(sid, name);
  });
  term.unicode.activeVersion = "11";
  term.open(host);
  term.attachCustomKeyEventHandler((event) => {
    if (event.type !== "keydown") return true;
    const key = event.key.toLowerCase();
    if (event.ctrlKey && event.shiftKey) {
      if (key === "f") {
        event.preventDefault();
        requestSearch(sid);
        return false;
      }
      if (key === "c") {
        event.preventDefault();
        copySelection(sid);
        return false;
      }
      if (key === "v") {
        event.preventDefault();
        pasteClipboard(sid);
        return false;
      }
      if (key === "a") {
        event.preventDefault();
        term.selectAll();
        return false;
      }
    }
    // Ctrl+C 有选区时复制，否则照常把 SIGINT 交给 shell。
    if (event.ctrlKey && !event.shiftKey && key === "c" && term.hasSelection()) {
      event.preventDefault();
      copySelection(sid);
      return false;
    }
    return true;
  });
  // 暂存区尺寸异常时忽略；真正挂载后会再 fit。
  try {
    fit.fit();
  } catch {
    /* ignored */
  }

  const input = term.onData((data) => {
    void commands
      .terminalWrite(sid, data)
      .then(unwrap)
      .catch((failure: unknown) => console.warn("[terminal] write failed", failure));
  });

  const entry: Pooled = {
    term,
    fit,
    search,
    host,
    input,
    webgl: null,
    webglFailed: false,
    parsed: 0,
    acked: 0,
    epoch: 0,
    ackTimer: undefined,
    serverCols: 0,
    serverRows: 0,
  };
  pooledTerms.set(sid, entry);
  return entry;
}

/** 把终端宿主 re-parent 进面板；返回当前的 cols / rows。 */
export function mountTerm(sid: string, pane: HTMLElement): { cols: number; rows: number } {
  const entry = ensureTerm(sid);
  if (entry.host.parentElement !== pane) pane.appendChild(entry.host);
  applyTerminalAppearance(sid, usePreferencesStore().appearance);
  const size = fitTerm(entry);
  enableWebgl(entry);
  entry.term.focus();
  return size;
}

/** 重新测量；返回尺寸。面板隐藏 / 0 尺寸时不要调用。 */
export function fitTerm(sid: string): { cols: number; rows: number };
export function fitTerm(entry: Pooled): { cols: number; rows: number };
export function fitTerm(target: string | Pooled): { cols: number; rows: number } {
  const entry = typeof target === "string" ? ensureTerm(target) : target;
  // 面板还没有布局时 fit 会抛；保持当前尺寸。
  try {
    entry.fit.fit();
  } catch {
    /* ignored */
  }
  return { cols: entry.term.cols, rows: entry.term.rows };
}

/** 显示：首次创建会话，之后 `Attach` 取快照刷新（#73）。 */
export function show(sid: string, cwd: string, agentId?: string): Promise<void> {
  const created: TerminalState = states.get(sid) ?? {
    sid,
    cwd,
    status: "starting",
    error: null,
    exitCode: null,
    visible: false,
    reconnected: false,
  };
  states.set(sid, created);
  // 取回 reactive 代理再改字段；直接改刚 set 进去的裸对象不会触发依赖。
  const state = states.get(sid) as TerminalState;
  const wasVisible = state.visible;
  state.visible = true;
  ensureTerm(sid);

  if (!opened.has(sid)) {
    const pending = opening.get(sid);
    if (pending) return pending;
    const starting = open(sid, cwd, agentId).finally(() => opening.delete(sid));
    opening.set(sid, starting);
    return starting;
  }

  if (!wasVisible && state.status === "ready") {
    resetAck(sid);
    void commands
      .terminalSetVisible(sid, true)
      .then(unwrap)
      .catch((failure: unknown) => setError(sid, errorMessage(failure)));
  }
  return Promise.resolve();
}

/** 隐藏：通知 app-core `Detach`，停止收流（会话继续活在 daemon 里）。 */
export function hide(sid: string): void {
  const state = states.get(sid);
  if (!state || !state.visible) return;
  state.visible = false;
  const entry = pooledTerms.get(sid);
  if (entry) disableWebgl(entry);
  if (!opened.has(sid)) return;
  void commands
    .terminalSetVisible(sid, false)
    .then(unwrap)
    .catch((failure: unknown) => console.warn("[terminal] hide failed", failure));
}

/** 关闭页签：显式结束会话（运行中页签的二次确认在 tabs/actions.ts）。 */
export async function close(sid: string): Promise<void> {
  const pending = opening.get(sid);
  if (pending) await pending.catch(() => undefined);

  // 即使本地还没 attach（例如 UI 重开后尚未点开的恢复页签），也要让 daemon 真正终止：
  // `terminal_close` 按 sid 找会话，不依赖本地 SessionState。
  await commands
    .terminalClose(sid)
    .then(unwrap)
    .catch((failure: unknown) => console.warn("[terminal] close failed", failure));

  const entry = pooledTerms.get(sid);
  if (entry) {
    disableWebgl(entry);
    if (entry.ackTimer !== undefined) window.clearTimeout(entry.ackTimer);
    entry.input.dispose();
    entry.term.dispose();
    entry.host.remove();
    pooledTerms.delete(sid);
  }
  opened.delete(sid);
  states.delete(sid);
}

/** 同步尺寸（由面板的 ResizeObserver 去抖后调用）。 */
export function resize(sid: string, cols: number, rows: number): void {
  if (!opened.has(sid)) return;
  const entry = pooledTerms.get(sid);
  if (entry && entry.serverCols === cols && entry.serverRows === rows) return;
  if (entry) {
    entry.serverCols = cols;
    entry.serverRows = rows;
  }
  void commands
    .terminalResize(sid, cols, rows)
    .then(unwrap)
    .catch((failure: unknown) => console.warn("[terminal] resize failed", failure));
}

/** 首次建会话：用当前 fit 出来的尺寸作为 CreateSession 的初始 cols / rows。 */
function open(sid: string, cwd: string, agentId?: string): Promise<void> {
  const entry = ensureTerm(sid);
  const { cols, rows } = fitTerm(entry);
  // CreateSession 与 attach 前的 resize 都使用这个尺寸；首次 resize 可安全跳过。
  entry.serverCols = cols;
  entry.serverRows = rows;
  const state = states.get(sid);
  if (state) {
    state.status = "starting";
    state.error = null;
    state.exitCode = null;
  }

  const channel = new Channel<ArrayBuffer | TerminalControl>();
  channel.onmessage = (message) => onMessage(sid, message);

  return commands
    .terminalOpen(sid, cwd, cols, rows, agentId ?? null, channel as unknown as Channel<number>)
    .then(unwrap)
    .then(() => {
      opened.add(sid);
      const current = states.get(sid);
      if (current) current.status = "ready";
      // 打开过程中页签被切走：立刻 detach，别让后台会话继续推流。
      if (current && !current.visible) {
        void commands
          .terminalSetVisible(sid, false)
          .then(unwrap)
          .catch((failure: unknown) => console.warn("[terminal] detach failed", failure));
      }
    })
    .catch((failure: unknown) => {
      setError(sid, errorMessage(failure));
      throw failure;
    });
}

/** 订阅当前终端的搜索快捷键（Ctrl+Shift+F）。 */
export function onSearchRequest(sid: string, listener: () => void): IDisposable {
  const listeners = searchRequests.get(sid) ?? new Set<() => void>();
  listeners.add(listener);
  searchRequests.set(sid, listeners);
  return {
    dispose: () => {
      listeners.delete(listener);
      if (listeners.size === 0) searchRequests.delete(sid);
    },
  };
}

function requestSearch(sid: string): void {
  for (const listener of searchRequests.get(sid) ?? []) listener();
}

/** 复制当前选区；返回是否真的复制了内容。 */
export function copySelection(sid: string): boolean {
  const entry = pooledTerms.get(sid);
  const text = entry?.term.getSelection() ?? "";
  if (!text) return false;
  void commands
    .writeClipboard(text)
    .then(unwrap)
    .catch((failure: unknown) => console.warn("[terminal] copy failed", failure));
  return true;
}

/** 粘贴系统剪贴板文本；xterm 会处理 bracketed paste。 */
export function pasteClipboard(sid: string): void {
  void commands
    .readClipboard()
    .then(unwrap)
    .then((text) => {
      const entry = pooledTerms.get(sid);
      if (entry && text) entry.term.paste(text);
    })
    .catch((failure: unknown) => console.warn("[terminal] paste failed", failure));
}

export function selectAll(sid: string): void {
  pooledTerms.get(sid)?.term.selectAll();
}

export function hasSelection(sid: string): boolean {
  return pooledTerms.get(sid)?.term.hasSelection() ?? false;
}

/** 实时应用外观设置（字号 / 行高 / 光标）；不重建会话。 */
export function applyTerminalAppearance(sid: string, appearance: AppearanceView): void {
  const entry = pooledTerms.get(sid);
  if (!entry) return;
  entry.term.options.fontSize = appearance.fontSize;
  entry.term.options.lineHeight = appearance.lineHeight ?? undefined;
  entry.term.options.cursorStyle = appearance.cursorStyle;
  entry.host.dataset.fontSize = String(appearance.fontSize);
  entry.host.dataset.lineHeight = String(appearance.lineHeight ?? "");
  fitTerm(entry);
}

/** 搜索当前渲染端缓冲（scrollback 10k 行）；返回是否找到。 */
export function findInTerminal(
  sid: string,
  query: string,
  backwards = false,
): boolean {
  const entry = pooledTerms.get(sid);
  if (!entry || !query) return false;
  const options = {
    incremental: !backwards,
    decorations: {
      matchBackground: "#3d6b96",
      matchBorder: "#61afef",
      matchOverviewRuler: "#61afef",
      activeMatchBackground: "#d19a66",
      activeMatchBorder: "#e5c07b",
      activeMatchColorOverviewRuler: "#e5c07b",
    },
  };
  return backwards
    ? entry.search.findPrevious(query, options)
    : entry.search.findNext(query, options);
}

export function clearTerminalSearch(sid: string): void {
  pooledTerms.get(sid)?.search.clearDecorations();
}

function openExternal(uri: string): void {
  const lower = uri.trim().toLowerCase();
  if (!lower.startsWith("http://") && !lower.startsWith("https://")) return;
  void commands
    .openUrl(uri.trim())
    .then(unwrap)
    .catch((failure: unknown) => console.warn("[terminal] open url failed", failure));
}

function enableWebgl(entry: Pooled): void {
  if (entry.webgl || entry.webglFailed) return;
  try {
    const addon = new WebglAddon();
    addon.onContextLoss(() => {
      try {
        addon.dispose();
      } finally {
        entry.webgl = null;
        entry.webglFailed = true;
        entry.host.dataset.renderer = "dom";
      }
    });
    entry.term.loadAddon(addon);
    entry.webgl = addon;
    entry.host.dataset.renderer = "webgl";
  } catch (failure) {
    // 没有 WebGL / 驱动拒绝时保持 DOM 基线，不影响终端可用性。
    entry.webglFailed = true;
    console.warn("[terminal] WebGL unavailable, using DOM", failure);
  }
}

function disableWebgl(entry: Pooled): void {
  if (!entry.webgl) return;
  try {
    entry.webgl.dispose();
  } catch {
    /* context already lost */
  }
  entry.webgl = null;
}

function onMessage(sid: string, message: ArrayBuffer | TerminalControl): void {
  if (message instanceof ArrayBuffer) {
    write(sid, new Uint8Array(message));
    return;
  }

  const state = states.get(sid);
  if (!state) return;

  switch (message.type) {
    case "attached":
      resetAck(sid);
      {
        const entry = pooledTerms.get(sid);
        if (entry) {
          entry.serverCols = message.cols;
          entry.serverRows = message.rows;
        }
      }
      // 无法精确回放时，daemon 附带 ANSI 快照：先彻底重置 xterm，再用快照重建屏幕。
      if (message.snapshot !== null) {
        const entry = ensureTerm(sid);
        entry.term.reset();
        entry.term.write(message.snapshot);
        // 快照路径没有回放数据，重连标记随 Attached 一起来；写快照之后再写分隔线。
        if (message.reconnected) announceReconnect(sid, message.altScreen);
      }
      break;
    case "replayEnd":
      // 有回放时，边界之后才写分隔线；普通隐藏页签切回不污染历史。
      if (message.reconnected) announceReconnect(sid, message.altScreen);
      break;
    case "snapshot":
      resetAck(sid);
      {
        const entry = ensureTerm(sid);
        entry.term.reset();
        entry.term.write(message.ansi);
        if (message.reconnected) announceReconnect(sid, message.altScreen);
      }
      break;
    case "exit":
      state.status = "exited";
      state.exitCode = message.code;
      break;
    case "error":
      // 操作失败（resize / ack 等）只挂提示，终端本身还能继续用。
      state.error = message.message;
      break;
    case "closed":
      state.status = "error";
      state.error = message.reason;
      break;
  }
}

function write(sid: string, data: Uint8Array): void {
  const entry = pooledTerms.get(sid);
  if (!entry) return;
  const epoch = entry.epoch;
  entry.term.write(data, () => {
    // attach 之前的写入可能在 reset 之后才解析完；它们不属于新 ack 基点。
    if (entry.epoch !== epoch) return;
    entry.parsed += data.byteLength;
    if (entry.parsed - entry.acked >= ACK_BYTES) {
      flushAck(sid, entry);
    } else {
      scheduleAck(sid, entry);
    }
  });
}

/** 立即把已解析的字节报给 daemon（不超过已解析量）。 */
function flushAck(sid: string, entry: Pooled): void {
  if (entry.parsed <= entry.acked) return;
  entry.acked = entry.parsed;
  void commands
    .terminalAck(sid, entry.acked)
    .then(unwrap)
    .catch((failure: unknown) => console.warn("[terminal] ack failed", failure));
}

/**
 * 空闲补 ack：daemon 在高水位暂停后，最后一块可能不足 32 KiB；
 * 没有这个定时器，xterm 不再产生写回调，双方会互相等死。
 */
function scheduleAck(sid: string, entry: Pooled): void {
  if (entry.ackTimer !== undefined) return;
  const epoch = entry.epoch;
  entry.ackTimer = window.setTimeout(() => {
    entry.ackTimer = undefined;
    if (entry.epoch !== epoch) return;
    flushAck(sid, entry);
  }, ACK_IDLE_MS);
}

function resetAck(sid: string): void {
  const entry = pooledTerms.get(sid);
  if (!entry) return;
  entry.epoch += 1;
  if (entry.ackTimer !== undefined) {
    window.clearTimeout(entry.ackTimer);
    entry.ackTimer = undefined;
  }
  entry.parsed = 0;
  entry.acked = 0;
}

/** 真正的重连：面板提示 + 流内分隔线（alt-screen 只留面板提示）。 */
function announceReconnect(sid: string, altScreen: boolean): void {
  const state = states.get(sid);
  if (!state) return;
  state.reconnected = true;
  window.setTimeout(() => {
    const current = states.get(sid);
    if (current) current.reconnected = false;
  }, 2500);
  // 分隔线直接写进 xterm，不注入 PTY 字节流：回放边界可能落在转义序列中间，
  // 注入字节流会把它截断；xterm 的写队列保证它排在快照 / 回放之后、实时流之前。
  if (!altScreen) {
    pooledTerms
      .get(sid)
      ?.term.write(`\r\n\x1b[2m${t("terminal.reconnected")}\x1b[0m\r\n`);
  }
}

function setError(sid: string, message: string): void {
  const state = states.get(sid);
  if (!state) return;
  state.status = "error";
  state.error = message;
}

/**
 * 终端运行时（issue #68）：xterm 实例池 + 每会话 Channel + 可见性 / ack 记账。
 *
 * 三条边界：
 * - xterm 实例与宿主 `<div>` **不进 Pinia**（CONTEXT.md §0.2）；本模块用普通 `Map` 持有。
 * - 只有**当前可见**的页签在收流；不可见时向 app-core 发 `set_visible(false)`，由它 `Detach`。
 * - 合批与背压在 Rust 侧（≥16 KiB / ≥8 ms）；这里只按 xterm `write` 回调做累计 ack。
 * - 切回时 app-core 从 `lastSeq` 回放；窗口外则发 ANSI 快照，前端 `reset()` 后重放（#72）。
 */
import { Channel } from "@tauri-apps/api/core";
import { FitAddon } from "@xterm/addon-fit";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { Terminal, type IDisposable } from "@xterm/xterm";
import { reactive } from "vue";

import { commands } from "../../bindings";
import { errorMessage, unwrap } from "../../lib/ipc";

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
    }
  | { type: "replayEnd"; altScreen: boolean }
  | { type: "snapshot"; ansi: string; fromSeq: number; altScreen: boolean }
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
  host: HTMLDivElement;
  input: IDisposable;
  /** xterm 已解析的字节数（本次 attach 内）。 */
  parsed: number;
  /** 已报给 app-core 的字节数。 */
  acked: number;
};

/** 一次 ack 的最小间隔，避免 ack 风暴（阈值取 VS Code 量级）。 */
const ACK_BYTES = 32 * 1024;

const states = reactive(new Map<string, TerminalState>());
const pooledTerms = new Map<string, Pooled>();
const opened = new Set<string>();
const opening = new Map<string, Promise<void>>();

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
  host.style.cssText = "width:100%;height:100%;";
  stage().appendChild(host);

  const term = new Terminal({
    allowProposedApi: true,
    convertEol: false,
    scrollback: 10_000,
    cursorBlink: true,
    cursorStyle: "block",
    fontFamily: '"Ageminal Mono", "Cascadia Mono", Consolas, "Courier New", monospace',
    fontSize: 13,
    lineHeight: 1.2,
    rightClickSelectsWord: false,
    windowsPty: { backend: "conpty" },
    theme: {
      background: "#080909",
      foreground: "#d4d4d4",
      cursor: "#4fc1ff",
    },
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.loadAddon(new Unicode11Addon());
  term.unicode.activeVersion = "11";
  term.open(host);
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

  const entry: Pooled = { term, fit, host, input, parsed: 0, acked: 0 };
  pooledTerms.set(sid, entry);
  return entry;
}

/** 把终端宿主 re-parent 进面板；返回当前的 cols / rows。 */
export function mountTerm(sid: string, pane: HTMLElement): { cols: number; rows: number } {
  const entry = ensureTerm(sid);
  if (entry.host.parentElement !== pane) pane.appendChild(entry.host);
  const size = fitTerm(entry);
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

/** 显示：首次创建会话，之后 `Attach` 续流。 */
export function show(sid: string, cwd: string): Promise<void> {
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
    const starting = open(sid, cwd).finally(() => opening.delete(sid));
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
  if (!opened.has(sid)) return;
  void commands
    .terminalSetVisible(sid, false)
    .then(unwrap)
    .catch((failure: unknown) => console.warn("[terminal] hide failed", failure));
}

/** 关闭：临时语义是「结束会话」（detach 语义归 #75）。 */
export async function close(sid: string): Promise<void> {
  const pending = opening.get(sid);
  if (pending) await pending.catch(() => undefined);

  if (opened.has(sid)) {
    await commands
      .terminalClose(sid)
      .then(unwrap)
      .catch((failure: unknown) => console.warn("[terminal] close failed", failure));
  }

  const entry = pooledTerms.get(sid);
  if (entry) {
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
  void commands
    .terminalResize(sid, cols, rows)
    .then(unwrap)
    .catch((failure: unknown) => console.warn("[terminal] resize failed", failure));
}

/** 首次建会话：用当前 fit 出来的尺寸作为 CreateSession 的初始 cols / rows。 */
function open(sid: string, cwd: string): Promise<void> {
  const entry = ensureTerm(sid);
  const { cols, rows } = fitTerm(entry);
  const state = states.get(sid);
  if (state) {
    state.status = "starting";
    state.error = null;
    state.exitCode = null;
  }

  const channel = new Channel<ArrayBuffer | TerminalControl>();
  channel.onmessage = (message) => onMessage(sid, message);

  return commands
    .terminalOpen(sid, cwd, cols, rows, channel as unknown as Channel<number>)
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
      // 无法精确回放时，daemon 附带 ANSI 快照：先彻底重置 xterm，再用快照重建屏幕。
      if (message.snapshot !== null) {
        const entry = ensureTerm(sid);
        entry.term.reset();
        entry.term.write(message.snapshot);
      }
      break;
    case "replayEnd":
      // 不在终端字节流里注入分隔文本：回放边界可能落在转义序列中间，
      // 注入会把序列截断。这里只给面板一个可见提示；真正的流内分隔归 #75。
      state.reconnected = true;
      window.setTimeout(() => {
        const current = states.get(sid);
        if (current) current.reconnected = false;
      }, 2500);
      break;
    case "snapshot":
      resetAck(sid);
      {
        const entry = ensureTerm(sid);
        entry.term.reset();
        entry.term.write(message.ansi);
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
  entry.term.write(data, () => {
    entry.parsed += data.byteLength;
    if (entry.parsed - entry.acked >= ACK_BYTES) {
      entry.acked = entry.parsed;
      void commands
        .terminalAck(sid, entry.acked)
        .then(unwrap)
        .catch((failure: unknown) => console.warn("[terminal] ack failed", failure));
    }
  });
}

function resetAck(sid: string): void {
  const entry = pooledTerms.get(sid);
  if (!entry) return;
  entry.parsed = 0;
  entry.acked = 0;
}


function setError(sid: string, message: string): void {
  const state = states.get(sid);
  if (!state) return;
  state.status = "error";
  state.error = message;
}

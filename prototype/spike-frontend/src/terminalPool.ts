// xterm 终端池 + DOM re-parent 探针
//
// 要验证的事（对应 xterm.js issue #4978，仍 open）：
//   1. 每个会话只 `open()` 一次，宿主 <div> 长期存活；
//   2. 切换页签 = 把宿主 <div> appendChild 到当前面板（re-parent），不销毁/重建 Terminal；
//   3. re-parent 之后终端仍正常渲染、不空白、尺寸正确。
//
// 关键设计：xterm 实例与宿主 <div> 都在 Vue 渲染树之外。Vue 只提供一个稳定的
// 面板容器，池负责往里搬运宿主。
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";

export interface PooledTerm {
  id: string;
  term: Terminal;
  fit: FitAddon;
  /** 可被搬运的宿主容器 */
  host: HTMLDivElement;
  /** 累计写入字节数（用于吞吐统计） */
  bytes: number;
  /** 被 open() 的次数，用于验证「只 open 一次」 */
  opens: number;
  /** 被 re-parent 的次数 */
  reparents: number;
}

export class TerminalPool {
  private readonly terms = new Map<string, PooledTerm>();
  private readonly offstage: HTMLElement;

  /** offstage 必须可见且有尺寸（position:fixed; 屏幕外），否则 xterm 首次 open 量不到尺寸 */
  constructor(offstage: HTMLElement) {
    this.offstage = offstage;
  }

  ensure(id: string): PooledTerm {
    const existing = this.terms.get(id);
    if (existing) return existing;

    const host = document.createElement("div");
    host.className = "term-host";
    host.dataset.termId = id;
    this.offstage.appendChild(host);

    const term = new Terminal({
      convertEol: true,
      scrollback: 2000,
      fontFamily: '"Cascadia Mono", Consolas, "Courier New", monospace',
      fontSize: 13,
      theme: {
        background: "#080909",
        foreground: "#d4d4d4",
        cursor: "#4fc1ff",
      },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host); // 整个生命周期只调用这一次
    try {
      fit.fit();
    } catch {
      /* offstage 尺寸异常时忽略，激活时会再 fit */
    }

    const pooled: PooledTerm = { id, term, fit, host, bytes: 0, opens: 1, reparents: 0 };
    this.terms.set(id, pooled);
    return pooled;
  }

  /** 把一个终端搬进面板（re-parent），并 fit + refresh。
   *  关键：面板里**只保留当前宿主**，其余宿主移回暂存区——这才是 re-parent，
   *  而不是不断追加（那样 60 个宿主会堆叠，断言也会恒真）。 */
  activate(id: string, pane: HTMLElement): PooledTerm {
    const pooled = this.ensure(id);

    for (const child of Array.from(pane.children)) {
      if (child !== pooled.host && child.classList.contains("term-host")) {
        this.offstage.appendChild(child);
      }
    }

    if (pooled.host.parentElement !== pane) {
      pane.appendChild(pooled.host); // <-- 核心：DOM re-parent，不是重建
      pooled.reparents += 1;
    }
    requestAnimationFrame(() => {
      try {
        pooled.fit.fit();
      } catch {
        /* 面板尚未布局，忽略 */
      }
      // re-parent 后强制重绘，观察是否 #4978 式的空白
      pooled.term.refresh(0, pooled.term.rows - 1);
      pooled.term.focus();
    });
    return pooled;
  }

  /** 面板内当前的宿主数量（应为 1，否则说明宿主在堆叠） */
  paneHostCount(pane: HTMLElement): number {
    return pane.querySelectorAll(":scope > .term-host").length;
  }

  write(id: string, data: string): void {
    const pooled = this.ensure(id);
    pooled.term.write(data);
    pooled.bytes += data.length;
  }

  get(id: string): PooledTerm | undefined {
    return this.terms.get(id);
  }

  get size(): number {
    return this.terms.size;
  }

  totalBytes(): number {
    let n = 0;
    for (const t of this.terms.values()) n += t.bytes;
    return n;
  }

  /** WebView2(Chromium) 才有 performance.memory */
  usedHeapBytes(): number | null {
    const mem = (performance as unknown as { memory?: { usedJSHeapSize: number } }).memory;
    return mem ? mem.usedJSHeapSize : null;
  }

  /** 供 UI 断言 re-parent 是否成功 */
  isMountedIn(id: string, pane: HTMLElement): boolean {
    return this.terms.get(id)?.host.parentElement === pane;
  }
}

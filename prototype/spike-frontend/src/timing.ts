// 前端侧时间戳（相对 performance.timeOrigin = 页面导航开始）
// 用于把「进程入口 → 首帧就绪」的冷启动拆成 Rust 侧与前端侧两段。
export const marks: Record<string, number> = {
  timeOrigin: performance.timeOrigin,
  moduleEval: performance.now(),
};

/** 运行期捕获的错误（Vue errorHandler / window.onerror / unhandledrejection） */
export const spikeErrors: string[] = [];

export function mark(name: string): void {
  marks[name] = performance.now();
}

export function recordError(message: string): void {
  spikeErrors.push(`${new Date().toISOString()} ${message}`);
}

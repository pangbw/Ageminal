// 各探针的结果汇聚点：探针写入，App.vue 的 exportReport 读取并落盘。
// 用途：#38（中文 IME）与 #48（通知点击）的实测结果都并进 %TEMP%\agm-spike-report.json。
export const probeResults: Record<string, unknown> = {
  ime: null,
  notify: null,
};

export function setProbeResult(key: string, value: unknown): void {
  probeResults[key] = value;
}

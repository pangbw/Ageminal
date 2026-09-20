// 构建标记：用于**一眼确认跑的是哪个构建**。
// 每次改动探针后手动 +1；它同时显示在界面顶部，并写进 %TEMP%\agm-spike-report.json 的 build 字段。
// 若报告里的 build 与界面上的不一致，说明读到的是**旧报告文件**。
export const SPIKE_BUILD = "H11 · 2026-09-20";

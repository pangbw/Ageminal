/** 终端平台信息（#85）：xterm 的 ConPTY buildNumber 在终端实例创建前就绪。 */
let windowsBuild: number | null = null;

export function setWindowsBuildNumber(build: number | null): void {
  windowsBuild = build;
}

export function windowsBuildNumber(): number | undefined {
  return windowsBuild ?? undefined;
}

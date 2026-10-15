/**
 * 路径小工具（纯函数），文件树与预览页签共用。
 *
 * 路径都是 Rust 侧给的**绝对路径字符串**，分隔符随平台（Windows `\`、Unix `/`），所以这里
 * 两种都认、不假设平台。
 */

/** 路径的最后一段。 */
export function basename(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/** 最后一个路径分隔符的下标；没有则 `-1`。 */
function lastSeparator(path: string): number {
  return Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
}

/** `path` 用的是哪种分隔符（有没有 `\` 来定）。 */
function separatorOf(path: string): string {
  return path.includes("\\") ? "\\" : "/";
}

/** `path` 到 `root` 之间的祖先目录，由外到内，**不含** `root` 自己。 */
export function ancestorsOf(root: string, path: string): string[] {
  const chain: string[] = [];
  let current = path;
  while (current.length > root.length) {
    const cut = lastSeparator(current);
    if (cut <= 0) break;
    current = current.slice(0, cut);
    if (current === root) break;
    chain.unshift(current);
  }
  return chain;
}

/** 路径的父目录；没有父目录（如 `C:\`）时返回 `null`。 */
export function parentOf(path: string): string | null {
  const cut = lastSeparator(path);
  if (cut <= 0) return null;
  return path.slice(0, cut);
}

/** `path` 相对 `root` 的展示路径，统一用 `/` 分隔；不在 `root` 之下时原样返回。 */
export function relativeTo(root: string, path: string): string {
  if (path === root) return "";
  const sep = separatorOf(root);
  const prefix = root.endsWith(sep) ? root : root + sep;
  const rest = path.startsWith(prefix) ? path.slice(prefix.length) : path;
  return rest.split(/[\\/]/).join("/");
}

/** `path` 是否就是 `prefix`、或落在它之下（按路径分量，`/a/b` 不属于 `/a/bc`）。 */
export function isSameOrUnder(path: string, prefix: string): boolean {
  if (path === prefix) return true;
  const sep = separatorOf(prefix);
  const base = prefix.endsWith(sep) ? prefix : prefix + sep;
  return path.startsWith(base);
}

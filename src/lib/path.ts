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

/** `path` 到 `root` 之间的祖先目录，由外到内，**不含** `root` 自己。 */
export function ancestorsOf(root: string, path: string): string[] {
  const chain: string[] = [];
  let current = path;
  while (current.length > root.length) {
    const cut = Math.max(current.lastIndexOf("/"), current.lastIndexOf("\\"));
    if (cut <= 0) break;
    current = current.slice(0, cut);
    if (current === root) break;
    chain.unshift(current);
  }
  return chain;
}

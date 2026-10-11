/**
 * 预览内容的类型收窄（#60）。
 *
 * Rust 回来的 [`PreviewView`] 是扁平的（三个 `kind` 各填一部分字段）；这里收窄成判别联合，
 * 面板据此决定渲染 `<img>` / 二进制提示 / 文本编辑器。
 *
 * 文本的正文不进这里——它直接进 CodeMirror 的 `EditorState`（`CONTEXT.md` §5：`EditorView`
 * 不进 Pinia，正文也不该在别处再存一份）。
 */
import type { PreviewView } from "../../../bindings";

/** 一个预览页签的内容（文本不含正文）。 */
export type PreviewContent =
  | {
      kind: "text";
      /** 命中字节 / 行数上限，已截断。 */
      truncated: boolean;
      /** 无效 UTF-8，已 lossy 解码。 */
      lossy: boolean;
      /** 回传文本的行数。 */
      lines: number;
      /** 含超长行（> 100 KiB）。 */
      longLine: boolean;
    }
  | {
      kind: "image";
      /** `data:` URL；超过上限时为 `null`。 */
      url: string | null;
      /** 图片超过 20 MiB，未加载数据。 */
      tooLarge: boolean;
      totalBytes: number;
    }
  | { kind: "binary"; totalBytes: number };

/** 把 Rust 的扁平视图收窄成判别联合。 */
export function toPreviewContent(view: PreviewView): PreviewContent {
  switch (view.kind) {
    case "image":
      return {
        kind: "image",
        url: view.imageDataUrl,
        tooLarge: view.imageTooLarge,
        totalBytes: view.totalBytes,
      };
    case "binary":
      return { kind: "binary", totalBytes: view.totalBytes };
    case "text":
      return {
        kind: "text",
        truncated: view.truncated,
        lossy: view.lossy,
        lines: view.lines,
        longLine: view.longLine,
      };
  }
}

/**
 * 只读预览的 CodeMirror 装配（#60 / §9）。
 *
 * 最小扩展集（§9）：行号 / 折叠 / 括号匹配 / 语法高亮 / 搜索；**不启用** history /
 * activeLine / autocompletion。只读 = `readOnly` + `editable(false)`：隐藏光标、保留选择与
 * 复制。语言走 `@codemirror/language-data` **懒加载**（`LanguageDescription.load()` 才动态
 * import 语言包），未命中纯文本。主题以 `@codemirror/theme-one-dark` 覆写为 One Dark Pro Glass。
 */
import {
  LanguageDescription,
  bracketMatching,
  codeFolding,
  defaultHighlightStyle,
  foldGutter,
  foldKeymap,
  syntaxHighlighting,
} from "@codemirror/language";
import { languages } from "@codemirror/language-data";
import { highlightSelectionMatches, search, searchKeymap } from "@codemirror/search";
import { EditorState, type Extension } from "@codemirror/state";
import { oneDark } from "@codemirror/theme-one-dark";
import { EditorView, keymap, lineNumbers } from "@codemirror/view";

import { basename } from "../../../lib/path";

/** One Dark Pro Glass 覆写：表面色对齐外壳令牌（§11），其余交给 `oneDark`。 */
const glassTheme = EditorView.theme(
  {
    "&": { backgroundColor: "var(--color-bg)", color: "var(--color-text)", height: "100%" },
    ".cm-scroller": {
      fontFamily: "var(--font-mono)",
      fontSize: "var(--font-size-sm)",
      lineHeight: "1.5",
    },
    ".cm-content": { caretColor: "transparent" },
    "&.cm-focused": { outline: "none" },
    // 只读下隐藏光标（§9），选择与复制照常。
    ".cm-cursor, .cm-dropCursor": { display: "none" },
    ".cm-gutters": {
      backgroundColor: "var(--color-bg)",
      border: "none",
      color: "var(--color-faint)",
    },
  },
  { dark: true },
);

/** 与文档无关、所有文本页签共享的扩展。 */
const baseExtensions: Extension[] = [
  lineNumbers(),
  foldGutter(),
  bracketMatching(),
  codeFolding(),
  highlightSelectionMatches(),
  search({ top: true }),
  keymap.of([...searchKeymap, ...foldKeymap]),
  // 我们这张表没有自定义高亮，用官方默认风格兜底；`oneDark` 自带的风格优先。
  syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
  EditorState.readOnly.of(true),
  EditorView.editable.of(false),
  oneDark,
  glassTheme,
];

/** 空状态的初始状态（挂载共享 `EditorView` 用）。 */
export function emptyState(): EditorState {
  return EditorState.create({ extensions: baseExtensions });
}

/** 用文本与语言建一个页签状态。 */
export function createTextState(text: string, language: Extension | null): EditorState {
  return EditorState.create({
    doc: text,
    extensions: language ? [...baseExtensions, language] : baseExtensions,
  });
}

/** 按文件名懒加载语言；未命中或加载失败 → `null`（纯文本）。 */
export async function languageFor(path: string): Promise<Extension | null> {
  const description = LanguageDescription.matchFilename(languages, basename(path));
  if (!description) return null;
  try {
    return await description.load();
  } catch {
    return null;
  }
}

/**
 * 只读预览的 CodeMirror 装配（#60 / §9）。
 *
 * 最小扩展集（§9）：行号 / 折叠 / 括号匹配 / 语法高亮 / 搜索；**不启用** history /
 * activeLine / autocompletion。只读 = `readOnly` + `editable(false)`：隐藏光标、保留选择与
 * 复制。语言走 `@codemirror/language-data` **懒加载**（`LanguageDescription.load()` 才动态
 * import 语言包），未命中纯文本。主题以 `@codemirror/theme-one-dark` 覆写为 One Dark Pro Glass。
 *
 * CodeMirror 的**库内建文案**（搜索面板、折叠提示等，REQUIREMENTS.md §11）通过
 * `EditorState.phrases` facet 换成 zh-CN；清单见 `codemirror.*` 文案域。
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
import { search, searchKeymap } from "@codemirror/search";
import { EditorState, type Extension } from "@codemirror/state";
import { oneDark } from "@codemirror/theme-one-dark";
import { EditorView, keymap, lineNumbers } from "@codemirror/view";

import { translate as t } from "../../../i18n/translate";
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

/** 与文档、语言无关、所有文本页签共享的扩展。 */
const baseExtensions: Extension[] = [
  lineNumbers(),
  foldGutter(),
  bracketMatching(),
  codeFolding(),
  search({ top: true }),
  keymap.of([...searchKeymap, ...foldKeymap]),
  // 我们这张表没有自定义高亮，用官方默认风格兜底；`oneDark` 自带的风格优先。
  syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
  EditorState.readOnly.of(true),
  EditorView.editable.of(false),
  oneDark,
  glassTheme,
];

/**
 * CodeMirror 库内建文案（§11）的 key → 译文。
 *
 * key 是**上游英文原文**（`EditorState.phrase` 用它查表），值是本地化文案。带参的几条保留
 * `$1` 占位（CodeMirror 用 `$1` / `$2` 插值，单个 `$` 等价 `$1`）。
 */
export function codemirrorPhrases(): Record<string, string> {
  return {
    Find: t("codemirror.search.find"),
    Replace: t("codemirror.search.replace"),
    replace: t("codemirror.search.replace"),
    "replace all": t("codemirror.search.replaceAll"),
    next: t("codemirror.search.next"),
    previous: t("codemirror.search.previous"),
    all: t("codemirror.search.selectAll"),
    "match case": t("codemirror.search.matchCase"),
    regexp: t("codemirror.search.regexp"),
    "by word": t("codemirror.search.byWord"),
    close: t("codemirror.common.close"),
    "Go to line": t("codemirror.search.goToLine"),
    go: t("codemirror.search.go"),
    "current match": t("codemirror.search.currentMatch"),
    "on line": t("codemirror.search.onLine"),
    "replaced $ matches": t("codemirror.search.replacedMatches"),
    "replaced match on line $": t("codemirror.search.replacedOnLine"),
    "Control character": t("codemirror.common.controlCharacter"),
    "Folded lines": t("codemirror.fold.foldedLines"),
    "Unfolded lines": t("codemirror.fold.unfoldedLines"),
    to: t("codemirror.fold.to"),
    "folded code": t("codemirror.fold.foldedCode"),
    unfold: t("codemirror.fold.unfold"),
    "Fold line": t("codemirror.fold.foldLine"),
    "Unfold line": t("codemirror.fold.unfoldLine"),
  };
}

/** 每次建状态时读一次当前语言（换语言后新开的页签用新文案）。 */
function phraseExtension(): Extension {
  return EditorState.phrases.of(codemirrorPhrases());
}

/** 空状态的初始状态（挂载共享 `EditorView` 用）。 */
export function emptyState(): EditorState {
  return EditorState.create({ extensions: [...baseExtensions, phraseExtension()] });
}

/** 用文本与语言建一个页签状态。 */
export function createTextState(text: string, language: Extension | null): EditorState {
  const extensions = [...baseExtensions, phraseExtension()];
  if (language) extensions.push(language);
  return EditorState.create({ doc: text, extensions });
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

import { describe, expect, it } from "vitest";

import { translate as t } from "../../../../i18n/translate";
import { codemirrorPhrases, createTextState } from "../editor";

describe("codemirrorPhrases", () => {
  it("translates the search panel strings", () => {
    const phrases = codemirrorPhrases();

    expect(phrases.Find).toBe(t("codemirror.search.find"));
    expect(phrases["replace all"]).toBe(t("codemirror.search.replaceAll"));
    expect(phrases["Go to line"]).toBe(t("codemirror.search.goToLine"));
    expect(phrases["Control character"]).toBe(t("codemirror.common.controlCharacter"));
  });

  it("keeps the $1 interpolation markers CodeMirror fills in", () => {
    const phrases = codemirrorPhrases();

    expect(phrases["replaced $ matches"]).toContain("$1");
    expect(phrases["replaced match on line $"]).toContain("$1");
  });
});

describe("createTextState", () => {
  it("creates a read-only state carrying the localized phrases", () => {
    const state = createTextState("a\nb", null);

    expect(state.doc.toString()).toBe("a\nb");
    expect(state.readOnly).toBe(true);
    expect(state.phrase("Find")).toBe(t("codemirror.search.find"));
  });
});

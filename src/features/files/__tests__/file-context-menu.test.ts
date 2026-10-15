import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import { translate as t } from "../../../i18n/translate";
import FileContextMenu from "../FileContextMenu.vue";

const TARGET = { path: "/ws/a/src/main.ts", name: "main.ts", isDir: false };

describe("FileContextMenu", () => {
  it("renders the grouped items and emits the chosen action", async () => {
    const wrapper = mount(FileContextMenu, { props: { x: 10, y: 20, target: TARGET } });

    expect(wrapper.findAll(".ctx__item").map((node) => node.text())).toEqual([
      t("files.context.newFile"),
      t("files.context.newFolder"),
      t("files.context.rename"),
      t("files.context.reveal"),
      t("files.context.openTerminal"),
      t("files.context.copyFile"),
      t("files.context.copyPath"),
      t("files.context.copyRelative"),
      t("files.context.delete"),
    ]);
    expect(wrapper.findAll(".ctx__sep")).toHaveLength(3); // 弱分组：rename / copyFile / delete 各起一组

    await wrapper.findAll(".ctx__item")[8].trigger("click");

    expect(wrapper.emitted("action")?.[0]).toEqual(["delete"]);
    expect(wrapper.emitted("close")).toHaveLength(1);
    wrapper.unmount();
  });

  it("closes on Escape", async () => {
    const wrapper = mount(FileContextMenu, { props: { x: 0, y: 0, target: TARGET } });

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));

    expect(wrapper.emitted("close")).toHaveLength(1);
    wrapper.unmount();
  });
});

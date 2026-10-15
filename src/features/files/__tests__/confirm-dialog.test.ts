import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import ConfirmDialog from "../ConfirmDialog.vue";

describe("ConfirmDialog", () => {
  it("emits confirm and cancel from its two buttons", async () => {
    const wrapper = mount(ConfirmDialog, {
      props: { title: "Delete", message: "Sure?", confirmLabel: "Delete", danger: true },
    });

    expect(wrapper.find(".confirm__title").text()).toBe("Delete");
    expect(wrapper.find(".confirm__button--danger").exists()).toBe(true);

    await wrapper.find(".confirm__button--danger").trigger("click");
    expect(wrapper.emitted("confirm")).toHaveLength(1);

    await wrapper.findAll(".confirm__button")[0].trigger("click");
    expect(wrapper.emitted("cancel")).toHaveLength(1);
  });
});

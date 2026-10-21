// @vitest-environment jsdom
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";
import ErrorDialog from "./ErrorDialog.vue";
import SoftDialog from "./SoftDialog.vue";

const wrappers: VueWrapper[] = [];
function show(errors = ["invalid uri authority: ?"]) {
  const wrapper = mount(ErrorDialog, {
    props: { errors },
    attachTo: document.body,
    global: { stubs: { Teleport: true } },
  });
  wrappers.push(wrapper);
  return wrapper;
}
afterEach(() => {
  wrappers.splice(0).forEach((wrapper) => wrapper.unmount());
});

describe("错误弹窗", () => {
  it("脱离页面层叠上下文覆盖主导航和原图查看器", async () => {
    const host = document.createElement("div");
    host.className = "main-workspace";
    document.body.append(host);
    const wrapper = mount(ErrorDialog, {
      props: { errors: ["database is locked"] },
      attachTo: host,
    });
    wrappers.push(wrapper);
    await flushPromises();
    const backdrop = document.querySelector(".error-dialog");
    expect(backdrop?.parentElement).toBe(document.body);
    expect(host.contains(backdrop)).toBe(false);
    expect(backdrop?.querySelector('[role="dialog"]')).not.toBeNull();
    wrapper.unmount();
    wrappers.pop();
    host.remove();
  });
  it("显示清晰说明，技术原文默认折叠，重复消息只显示一次", async () => {
    const wrapper = show([
      "invalid uri authority: ?",
      " invalid uri authority: ? ",
    ]);
    expect(wrapper.get('[role="dialog"]').attributes("aria-label")).toBe(
      "操作未完成",
    );
    expect(wrapper.text()).toContain("操作遇到问题");
    expect(wrapper.find("pre").exists()).toBe(false);
    const toggle = wrapper.get(".error-dialog-details-toggle");
    expect(toggle.attributes("aria-expanded")).toBe("false");
    await toggle.trigger("click");
    expect(toggle.attributes("aria-expanded")).toBe("true");
    expect(wrapper.findAll("pre")).toHaveLength(1);
    expect(wrapper.get("pre").text()).toBe("invalid uri authority: ?");
    await toggle.trigger("click");
    expect(wrapper.find("pre").exists()).toBe(false);
    expect(
      wrapper.findAll("footer button").map((button) => button.text()),
    ).toEqual(["知道了"]);
  });

  it("确认关闭后持续相同错误不重弹，新错误仍会弹出", async () => {
    const wrapper = show();
    await wrapper.get("footer .primary").trigger("click");
    expect(wrapper.emitted("close")).toHaveLength(1);
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
    for (let progress = 0; progress < 6; progress++)
      await wrapper.setProps({ errors: ["invalid uri authority: ?"] });
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
    await wrapper.setProps({
      errors: ["invalid uri authority: ?", "读取结果失败"],
    });
    expect(wrapper.find('[role="dialog"]').exists()).toBe(true);
    await wrapper.get(".error-dialog-details-toggle").trigger("click");
    expect(wrapper.findAll("pre").map((node) => node.text())).toEqual([
      "读取结果失败",
    ]);
  });

  it("错误确实消失后，后来一次相同失败可以再次提示", async () => {
    const wrapper = show();
    await wrapper.get('button[aria-label="关闭"]').trigger("click");
    await wrapper.setProps({ errors: [] });
    await wrapper.setProps({ errors: ["invalid uri authority: ?"] });
    expect(wrapper.find('[role="dialog"]').exists()).toBe(true);
  });

  it("刷新中的短暂清空不会使已关闭的相同错误重弹", async () => {
    const wrapper = show();
    await wrapper.get("footer .primary").trigger("click");
    await wrapper.setProps({ retrying: true, errors: [] });
    await wrapper.setProps({
      retrying: false,
      errors: ["invalid uri authority: ?"],
    });
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
  });

  it("Esc 只关闭最上层弹窗，并恢复先前焦点", async () => {
    const underlyingClosed = vi.fn();
    const underlying = mount(SoftDialog, {
      props: { title: "原操作", onClose: underlyingClosed },
      attachTo: document.body,
    });
    wrappers.push(underlying);
    await flushPromises();
    const previous = underlying.get("button").element;
    previous.focus();
    const wrapper = show();
    await flushPromises();
    const event = new KeyboardEvent("keydown", {
      key: "Escape",
      bubbles: true,
      cancelable: true,
    });
    window.dispatchEvent(event);
    await flushPromises();
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
    expect(underlyingClosed).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(previous);
  });

  it("仅在显式提供刷新入口时发出请求，读取中禁用重复点击，成功后关闭", async () => {
    const wrapper = show();
    await wrapper.setProps({ retryLabel: "刷新结果" });
    const retry = wrapper.get("footer .text-button");
    await retry.trigger("click");
    expect(wrapper.emitted("retry")).toHaveLength(1);
    await wrapper.setProps({ retrying: true, errors: [] });
    expect(wrapper.find('[role="dialog"]').exists()).toBe(true);
    expect(
      wrapper.get("footer .text-button").attributes("disabled"),
    ).toBeDefined();
    await wrapper.get("footer .text-button").trigger("click");
    expect(wrapper.emitted("retry")).toHaveLength(1);
    await wrapper.setProps({ retrying: false });
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
  });
});

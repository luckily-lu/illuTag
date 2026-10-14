// @vitest-environment jsdom
import { mount } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";
import DuplicateComparison from "./DuplicateComparison.vue";
import type { DuplicateMember } from "../../shared/contracts/DuplicateMember";
vi.mock("../../shared/ipc", () => ({ imageUrl: (path: string) => path }));
const member = (id: string): DuplicateMember => ({
  image: {
    id,
    path: "data:image/png;base64,",
    thumbnailPath: null,
    fileName: id,
    ext: "png",
    width: 800,
    height: 600,
    fileSize: 1024,
    modifiedAt: 0,
    importedAt: 0,
    folderId: 1,
    missing: false,
    trashed: false,
    isFavorite: false,
    source: "gallery",
  },
  metric: { phashDistance: 2, dhashDistance: 4, colorDistance: 0.01 },
  stale: false,
});
afterEach(() => document.body.replaceChildren());
describe("原图比对", () => {
  it("参考图与候选图独立缩放，重置仅影响对应图片", async () => {
    const wrapper = mount(DuplicateComparison, {
      props: {
        reference: member("reference"),
        candidate: member("candidate"),
        kind: "near",
      },
      attachTo: document.body,
    });
    const stages = wrapper.findAll(".duplicate-pan-stage");
    await stages[0].trigger("wheel", { deltaY: -100 });
    expect(wrapper.findAll("img")[0].attributes("style")).toContain(
      "scale(1.2)",
    );
    expect(wrapper.findAll("img")[1].attributes("style")).toContain("scale(1)");
    await stages[1].trigger("wheel", { deltaY: -100 });
    await wrapper.find('[aria-label="参考图恢复适应"]').trigger("click");
    expect(wrapper.findAll("img")[0].attributes("style")).toContain("scale(1)");
    expect(wrapper.findAll("img")[1].attributes("style")).toContain(
      "scale(1.2)",
    );
    wrapper.unmount();
  });
  it("Esc 只关闭比对，焦点返回列表打开按钮", async () => {
    const launch = document.createElement("button");
    document.body.append(launch);
    launch.focus();
    const outer = vi.fn();
    window.addEventListener("keydown", outer);
    const wrapper = mount(DuplicateComparison, {
      props: {
        reference: member("reference"),
        candidate: member("candidate"),
        kind: "exact",
      },
      attachTo: document.body,
    });
    expect(document.activeElement?.getAttribute("aria-label")).toBe("关闭比对");
    await wrapper
      .find('[aria-label="关闭比对"]')
      .trigger("keydown", { key: "Escape" });
    expect(wrapper.emitted("close")).toHaveLength(1);
    expect(outer).not.toHaveBeenCalled();
    wrapper.unmount();
    expect(document.activeElement).toBe(launch);
    window.removeEventListener("keydown", outer);
  });
});

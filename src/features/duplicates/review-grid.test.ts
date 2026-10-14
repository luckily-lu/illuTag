// @vitest-environment jsdom
import { mount, flushPromises } from "@vue/test-utils";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import DuplicateReviewGrid from "./DuplicateReviewGrid.vue";
import type { DuplicateMember } from "../../shared/contracts/DuplicateMember";
vi.mock("../../shared/ipc", () => ({ imageUrl: (path: string) => path }));
const member = (id: string): DuplicateMember => ({
  image: {
    id,
    path: id,
    fileName: id,
    thumbnailPath: null,
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
const props = (count: number) => ({
  members: Array.from({ length: count }, (_, i) => member(`image-${i}`)),
  total: count,
  kept: new Set<string>(),
  selected: new Set<string>(),
  disabled: false,
  canMore: false,
  loadingMore: false,
});
beforeEach(() => {
  HTMLElement.prototype.scrollIntoView = vi.fn();
  Object.defineProperty(window, "innerWidth", {
    configurable: true,
    value: 1280,
  });
});
afterEach(() => document.body.replaceChildren());
it("同组多图同时出现，方向键定位，保留项不能用Delete回收", async () => {
  const wrapper = mount(DuplicateReviewGrid, {
    props: props(4),
    attachTo: document.body,
  });
  expect(wrapper.findAll(".duplicate-multi-card")).toHaveLength(4);
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight" }));
  await flushPromises();
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "k" }));
  expect(wrapper.emitted("keep")?.[0]?.[0]).toMatchObject({
    image: { id: "image-1" },
  });
  await wrapper.setProps({ kept: new Set(["image-1"]) });
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Delete" }));
  expect(wrapper.emitted("manage")).toBeUndefined();
  await wrapper.setProps({ kept: new Set() });
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Delete" }));
  expect(wrapper.emitted("manage")).toHaveLength(1);
  expect(wrapper.emitted("select")?.[0]?.[0]).toMatchObject({
    image: { id: "image-1" },
  });
  wrapper.unmount();
});
it("不同图片数量与窄窗调整列数，输入与组合输入不会触发删除", async () => {
  const wrapper = mount(DuplicateReviewGrid, {
    props: props(9),
    attachTo: document.body,
  });
  expect(wrapper.find(".duplicate-multi-grid").attributes("style")).toContain(
    "--review-columns: 3",
  );
  Object.defineProperty(window, "innerWidth", { value: 500 });
  window.dispatchEvent(new Event("resize"));
  await flushPromises();
  expect(wrapper.find(".duplicate-multi-grid").attributes("style")).toContain(
    "--review-columns: 2",
  );
  window.dispatchEvent(
    new KeyboardEvent("keydown", { key: "Delete", isComposing: true }),
  );
  wrapper
    .find('input[type="checkbox"]')
    .element.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Delete", bubbles: true }),
    );
  expect(wrapper.emitted("manage")).toBeUndefined();
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
  expect(wrapper.emitted("close")).toHaveLength(1);
  wrapper.unmount();
});

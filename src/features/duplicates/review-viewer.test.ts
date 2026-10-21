// @vitest-environment jsdom
import { mount, type VueWrapper } from "@vue/test-utils";
import { nextTick } from "vue";
import { afterEach, describe, expect, it, vi } from "vitest";
import DuplicateReviewViewer from "./DuplicateReviewViewer.vue";
import type { DuplicateReviewMember } from "../../shared/contracts/DuplicateReviewMember";

vi.mock("../../shared/ipc", () => ({ imageUrl: (path: string) => path }));

const members: DuplicateReviewMember[] = ["first", "second"].map((id) => ({
  member: {
    image: {
      id,
      path: "data:image/png;base64,",
      thumbnailPath: null,
      fileName: `${id}.png`,
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
    metric: { phashDistance: null, dhashDistance: null, colorDistance: null },
    stale: false,
  },
  disposition: id === "first" ? "keep" : "clean",
  isKeeper: id === "first",
  reason: "",
}));
let wrapper: VueWrapper | undefined;
function open(host: HTMLElement = document.body) {
  wrapper = mount(DuplicateReviewViewer, {
    props: { members, imageId: "first" },
    attachTo: host,
  });
  return wrapper;
}
function element<T extends HTMLElement>(selector: string) {
  return document.querySelector<T>(selector)!;
}
function key(target: EventTarget, name: string, extra: KeyboardEventInit = {}) {
  const event = new KeyboardEvent("keydown", {
    key: name,
    bubbles: true,
    cancelable: true,
    ...extra,
  });
  target.dispatchEvent(event);
  return event;
}
afterEach(() => {
  wrapper?.unmount();
  wrapper = undefined;
  vi.restoreAllMocks();
  document.body.replaceChildren();
});

describe("审查原图查看器", () => {
  it("提升到 body，脱离工作区层叠上下文与侧栏", () => {
    const workspace = document.createElement("main");
    workspace.style.isolation = "isolate";
    document.body.append(workspace);
    open(workspace);
    const dialog = element('[aria-label="查看分组原图"]');
    expect(dialog.parentElement).toBe(document.body);
    expect(workspace.contains(dialog)).toBe(false);
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    expect(dialog.querySelector("img")?.alt).toBe("first.png");
  });

  it("Esc 仅关闭原图，卸载后焦点回到审查图片", () => {
    const launch = document.createElement("button");
    document.body.append(launch);
    launch.focus();
    const outer = vi.fn();
    window.addEventListener("keydown", outer);
    try {
      const viewer = open();
      expect(document.activeElement?.getAttribute("aria-label")).toBe(
        "关闭原图",
      );
      expect(key(document.activeElement!, "Escape").defaultPrevented).toBe(
        true,
      );
      expect(viewer.emitted("close")).toHaveLength(1);
      expect(outer).not.toHaveBeenCalled();
      viewer.unmount();
      wrapper = undefined;
      expect(document.activeElement).toBe(launch);
    } finally {
      window.removeEventListener("keydown", outer);
    }
  });

  it("方向键只切原图，审查确认快捷键不穿透，图片缩放仍可用", async () => {
    const outer = vi.fn();
    window.addEventListener("keydown", outer);
    try {
      const viewer = open();
      key(document.activeElement!, "ArrowRight");
      expect(viewer.emitted("change")).toEqual([["second"]]);
      expect(
        key(document.activeElement!, "Enter", { ctrlKey: true })
          .defaultPrevented,
      ).toBe(true);
      key(element(".duplicate-pan-stage"), "+");
      await nextTick();
      expect(
        element<HTMLImageElement>(".duplicate-pan-stage img").style.transform,
      ).toContain("scale(1.2)");
      expect(outer).not.toHaveBeenCalled();
    } finally {
      window.removeEventListener("keydown", outer);
    }
  });

  it("Tab 只在查看器内循环，焦点意外移出时可重新进入", () => {
    vi.spyOn(HTMLElement.prototype, "getClientRects").mockReturnValue([
      {} as DOMRect,
    ] as unknown as DOMRectList);
    const launch = document.createElement("button");
    document.body.append(launch);
    open();
    const first = element<HTMLButtonElement>('[aria-label="关闭原图"]');
    const last = element<HTMLButtonElement>('[aria-label="原图放大"]');
    first.focus();
    expect(key(first, "Tab", { shiftKey: true }).defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(last);
    expect(key(last, "Tab").defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(first);
    launch.focus();
    expect(key(launch, "Tab").defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(first);
  });
});

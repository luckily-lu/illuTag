// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import SettingsPage from "./SettingsPage.vue";
import { invoke } from "../../shared/ipc";

vi.mock("../../shared/ipc", () => ({ invoke: vi.fn(), errorMessage: String }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));

let wrapper: VueWrapper;
beforeEach(async () => {
  vi.clearAllMocks();
  vi.mocked(invoke).mockResolvedValue({ dataDir: "X:\\isolated\\data" });
  wrapper = mount(SettingsPage, {
    props: {
      sources: [
        {
          id: 17,
          path: "X:\\isolated\\originals",
          addedAt: 0,
          lastScannedAt: null,
        },
      ],
      thumbnails: true,
      hideTitle: false,
    },
    attachTo: document.body,
  });
  await flushPromises();
  await wrapper
    .findAll("button")
    .find((b) => b.text() === "图库与存储")!
    .trigger("click");
});
afterEach(() => wrapper.unmount());

function clickButton(text: string) {
  return wrapper
    .findAll("button")
    .find((b) => b.text() === text)!
    .trigger("click");
}

it("移除来源须确认具体路径，取消保留索引，确认才移除对应来源", async () => {
  await clickButton("移除来源");
  const dialog = wrapper.get('[role="dialog"]');
  expect(dialog.text()).toContain("X:\\isolated\\originals");
  expect(dialog.text()).toContain("图库索引");
  expect(dialog.text()).toContain("原始图片保留");
  await clickButton("取消");
  expect(invoke).not.toHaveBeenCalledWith(
    "remove_gallery_folder_command",
    expect.anything(),
  );

  await clickButton("移除来源");
  await wrapper.get('[role="dialog"] .danger').trigger("click");
  await flushPromises();
  expect(invoke).toHaveBeenCalledWith("remove_gallery_folder_command", {
    folderPath: "X:\\isolated\\originals",
  });
  expect(wrapper.emitted("refresh")).toHaveLength(1);
});

it("清理缺失索引取消时不启动任务，确认后只启动一次", async () => {
  await clickButton("手动清理缺失索引");
  expect(wrapper.get('[role="dialog"]').text()).toContain("离线来源目录会跳过");
  expect(wrapper.emitted("task")).toBeUndefined();
  await clickButton("取消");
  expect(wrapper.emitted("task")).toBeUndefined();

  await clickButton("手动清理缺失索引");
  await clickButton("开始清理");
  expect(wrapper.emitted("task")).toEqual([["cleanup"]]);
});

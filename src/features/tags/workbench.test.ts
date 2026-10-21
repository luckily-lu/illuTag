import { effectScope, nextTick } from "vue";
import { afterEach, expect, it, vi } from "vitest";
import { api, invoke } from "../../shared/ipc";
import { useTagWorkbench } from "./useTagWorkbench";
import type { GalleryImage } from "../../shared/contracts/GalleryImage";
import type { QueryPage } from "../../shared/contracts/QueryPage";
vi.mock("../../shared/ipc", () => ({
  errorMessage: String,
  invoke: vi.fn(),
  api: {
    tagCovers: vi.fn(),
    tagPreview: vi.fn(),
    favoriteTags: vi.fn(async () => [] as string[]),
    setFavoriteTag: vi.fn(),
  },
}));
const scopes: ReturnType<typeof effectScope>[] = [];
afterEach(() => {
  scopes.forEach((s) => s.stop());
  scopes.length = 0;
  vi.useRealTimers();
  vi.clearAllMocks();
});
async function setup() {
  vi.mocked(invoke).mockResolvedValue({
    groups: [
      { id: "appearance", zh: "外观" },
      { id: "scene", zh: "场景" },
    ],
    tags: [
      {
        tagEn: "long_hair",
        tagZh: "长发",
        group: "appearance",
        imageCount: 12,
        sortIndex: 1,
      },
      {
        tagEn: "blue_sky",
        tagZh: "蓝天",
        group: "scene",
        imageCount: 9,
        sortIndex: 2,
      },
    ],
  });
  vi.mocked(api.tagCovers).mockImplementation(async (tags) =>
    tags.map((tag) => ({
      tag,
      image: null,
      pinned: false,
      x: 50,
      y: 50,
      zoom: 1,
    })),
  );
  const scope = effectScope();
  scopes.push(scope);
  const w = scope.run(() => useTagWorkbench())!;
  await w.init();
  await nextTick();
  return w;
}
it("收藏标签分组可检索，取消收藏不会清空已经组合的条件", async () => {
  const w = await setup();
  w.selected.value = ["long_hair"];
  w.catalog.dictFavoriteTags.value = new Set(["long_hair"]);
  w.catalog.dictGroupId.value = "favorites";
  await nextTick();
  expect(w.visible.value.map((t) => t.tagEn)).toEqual(["long_hair"]);
  expect(
    w.catalog.dictGroupItems.value.find((g) => g.id === "favorites")?.count,
  ).toBe(1);
  vi.mocked(api.setFavoriteTag).mockResolvedValue([]);
  await w.favoriteTag("long_hair");
  expect(w.visible.value).toHaveLength(0);
  expect(w.selected.value).toEqual(["long_hair"]);
  expect(api.setFavoriteTag).toHaveBeenCalledWith("long_hair", false);
});
it("不同标签收藏的迟到响应不能覆盖较新的收藏", async () => {
  const w = await setup();
  let first!: (v: string[]) => void;
  vi.mocked(api.setFavoriteTag)
    .mockImplementationOnce(
      () =>
        new Promise((r) => {
          first = r;
        }),
    )
    .mockResolvedValueOnce(["long_hair", "blue_sky"]);
  const pending = w.favoriteTag("long_hair");
  await w.favoriteTag("blue_sky");
  first(["long_hair"]);
  await pending;
  expect([...w.catalog.dictFavoriteTags.value].sort()).toEqual([
    "blue_sky",
    "long_hair",
  ]);
});
it("分组浏览不清空组合条件或中文名称，封面不随条件重载", async () => {
  vi.useFakeTimers();
  const w = await setup();
  w.toggle("long_hair");
  await nextTick();
  w.catalog.dictGroupId.value = "scene";
  await nextTick();
  expect(w.visible.value.map((t) => t.tagEn)).toEqual(["blue_sky"]);
  expect(w.selected.value).toEqual(["long_hair"]);
  expect(w.labels.value.get("long_hair")).toBe("长发");
  expect(api.tagCovers).toHaveBeenCalledTimes(1);
  w.suspend();
});
it("快速改变组合条件时旧预览响应不能覆盖新结果", async () => {
  vi.useFakeTimers();
  const w = await setup();
  let first!: (value: any) => void;
  vi.mocked(api.tagPreview)
    .mockReturnValueOnce(new Promise((resolve) => (first = resolve)))
    .mockResolvedValueOnce({
      images: [],
      totalImageCount: 2,
      offset: 0,
      limit: 18,
      revision: 1,
    });
  w.toggle("long_hair");
  await nextTick();
  const pending = w.preview();
  w.toggle("blue_sky");
  await nextTick();
  await w.preview();
  first({ images: [], totalImageCount: 99, offset: 0, limit: 18, revision: 1 });
  await pending;
  expect(w.total.value).toBe(2);
  expect(w.selected.value).toEqual(["long_hair", "blue_sky"]);
  w.suspend();
});
it("预览分页阻止并发与隐藏请求，忽略过期结果，失败可重试且空页停止追加", async () => {
  vi.useFakeTimers();
  const w = await setup();
  const result = (offset: number, count = 18): QueryPage => ({
    images: Array.from(
      { length: count },
      (_, index) =>
        ({
          id: String(offset + index),
        }) as GalleryImage,
    ),
    totalImageCount: 100,
    offset,
    limit: 18,
    revision: 1,
  });
  const request = vi.mocked(api.tagPreview).mockReset();
  request.mockResolvedValueOnce(result(0));
  w.toggle("long_hair");
  await w.preview();
  let resolveNext!: (value: QueryPage) => void;
  request.mockReturnValueOnce(
    new Promise((resolve) => (resolveNext = resolve)),
  );
  const pending = w.preview(true);
  await w.preview(true);
  expect(request).toHaveBeenCalledTimes(2);
  expect(request).toHaveBeenLastCalledWith(["long_hair"], 18, 18);
  w.previewOpen.value = false;
  await w.preview(true);
  expect(request).toHaveBeenCalledTimes(2);
  expect(w.pageLoading.value).toBe(false);
  resolveNext(result(18));
  await pending;
  expect(w.images.value).toHaveLength(18);
  w.previewOpen.value = true;
  request.mockRejectedValueOnce(new Error("暂时不可用"));
  await w.preview(true);
  expect(w.previewError.value).toContain("暂时不可用");
  expect(w.images.value).toHaveLength(18);
  request.mockResolvedValueOnce(result(18));
  await w.preview(true);
  expect(w.images.value).toHaveLength(36);
  expect(w.previewError.value).toBe("");
  request.mockResolvedValueOnce(result(36, 0));
  await w.preview(true);
  expect(w.hasMore.value).toBe(false);
  await w.preview(true);
  expect(request).toHaveBeenCalledTimes(5);
  w.suspend();
});

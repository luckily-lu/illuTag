import { describe, it, expect, vi, beforeEach } from "vitest";
import { api } from "../../shared/ipc";
import { useGallery } from "./useGallery";
import { useSelection } from "./useSelection";
import { MasonryLayout } from "./masonry";
import type { GalleryImage } from "../../shared/contracts/GalleryImage";
vi.mock("../../shared/ipc", () => ({ api: { query: vi.fn() } }));
const image = (id: string, width = 800, height = 600): GalleryImage => ({
  id,
  path: id,
  thumbnailPath: null,
  fileName: id,
  ext: "png",
  width,
  height,
  fileSize: 1,
  modifiedAt: 0,
  importedAt: 0,
  folderId: 1,
  missing: false,
  trashed: false,
  isFavorite: false,
  source: "library",
});
const page = (id: string, revision = 0) => ({
  images: [image(id)],
  totalImageCount: 2,
  offset: 0,
  limit: 5000,
  revision,
});
describe("查询代次", () => {
  beforeEach(() => vi.mocked(api.query).mockReset());
  it("快速切换和清空不会被旧响应覆盖", async () => {
    let resolve!: (v: ReturnType<typeof page>) => void;
    vi.mocked(api.query)
      .mockImplementationOnce(() => new Promise((r) => (resolve = r)))
      .mockResolvedValueOnce(page("new"));
    const g = useGallery();
    const old = g.search();
    g.query.value.fileName = "new";
    await g.search();
    resolve(page("old"));
    await old;
    expect(g.images.value[0].id).toBe("new");
    expect(g.loading.value).toBe(false);
  });
  it("分页响应在查询切换后失效", async () => {
    let resolve!: (v: ReturnType<typeof page>) => void;
    vi.mocked(api.query)
      .mockResolvedValueOnce(page("first"))
      .mockImplementationOnce(() => new Promise((r) => (resolve = r)))
      .mockResolvedValueOnce(page("scope"));
    const g = useGallery();
    await g.search();
    const pending = g.more();
    g.query.value.scope = "favorites";
    await g.search();
    resolve(page("stale"));
    await pending;
    expect(g.images.value.map((i) => i.id)).toEqual(["scope"]);
    expect(g.pageLoading.value).toBe(false);
  });
});
describe("选择与瀑布流", () => {
  it("部分失败仅清除成功项", () => {
    const s = useSelection();
    s.all([image("ok"), image("failed")]);
    s.done(["ok"]);
    expect([...s.ids.value]).toEqual(["failed"]);
    expect(s.active.value).toBe(true);
  });
  it("全选查询后取消与执行成功只更新排除集", () => {
    const s = useSelection();
    s.selectAllResults(12000);
    expect(s.count.value).toBe(12000);
    expect(s.isSelected("unloaded-id")).toBe(true);
    s.toggle("one-id");
    expect(s.isSelected("one-id")).toBe(false);
    expect(s.count.value).toBe(11999);
    s.done(["unloaded-id"]);
    expect(s.isSelected("unloaded-id")).toBe(false);
    expect(s.count.value).toBe(11998);
  });
  it.each([400, 500, 768, 1024, 1280, 1920])(
    "宽度 %s 保持比例和增量布局",
    (width) => {
      const layout = new MasonryLayout();
      const inputs = [
        image("wide", 1600, 800),
        image("long", 300, 9000),
        image("square", 800, 800),
      ];
      layout.update(inputs, width, (p) => p);
      const first = layout.items[0];
      expect(layout.items[1].height / layout.items[1].width).toBe(30);
      expect(layout.items.every((i) => i.x + i.width <= width + 0.01)).toBe(
        true,
      );
      layout.update([...inputs, image("append")], width, (p) => p);
      expect(layout.items[0]).toBe(first);
      expect(layout.visible(0, 200).length).toBeGreaterThan(0);
    },
  );
});

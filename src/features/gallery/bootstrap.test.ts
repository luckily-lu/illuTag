import { beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../../shared/ipc";
import { useGallery } from "./useGallery";
import type { GalleryImage } from "../../shared/contracts/GalleryImage";

vi.mock("../../shared/ipc", () => ({ api: { query: vi.fn() } }));

const image = (id: string): GalleryImage => ({
  id,
  path: id,
  thumbnailPath: null,
  fileName: id,
  ext: "png",
  width: 100,
  height: 100,
  fileSize: 1,
  modifiedAt: 0,
  importedAt: 0,
  folderId: 1,
  missing: false,
  trashed: false,
  isFavorite: false,
  source: "library",
});
const page = (id: string, revision = 1) => ({
  images: [image(id)],
  totalImageCount: 1,
  offset: 0,
  limit: 5000,
  revision,
});

describe("启动 bootstrap 代次", () => {
  beforeEach(() => vi.mocked(api.query).mockReset());

  it("用户已经完成新搜索时，迟到的 bootstrap 页不能覆盖结果", async () => {
    let finish!: (value: ReturnType<typeof page>) => void;
    const pending = new Promise<ReturnType<typeof page>>(
      (resolve) => (finish = resolve),
    );
    vi.mocked(api.query).mockReturnValueOnce(pending);
    const gallery = useGallery();
    const snapshot = gallery.bootstrapSnapshot();
    gallery.query.value.scope = "favorites";
    const search = gallery.search();
    finish(page("favorite"));
    await search;

    expect(gallery.adoptBootstrap(page("all"), snapshot)).toBe(false);
    expect(gallery.images.value.map((i) => i.id)).toEqual(["favorite"]);
    expect(gallery.query.value.scope).toBe("favorites");
  });

  it("没有新的导航时采纳 bootstrap 页", () => {
    const gallery = useGallery();
    const snapshot = gallery.bootstrapSnapshot();
    expect(gallery.adoptBootstrap(page("all"), snapshot)).toBe(true);
    expect(gallery.images.value.map((i) => i.id)).toEqual(["all"]);
    expect(gallery.total.value).toBe(1);
  });

  it("只改查询条件但尚未搜索时也不采纳默认页", () => {
    const gallery = useGallery();
    const snapshot = gallery.bootstrapSnapshot();
    gallery.query.value.fileName = "typed.png";
    expect(gallery.adoptBootstrap(page("all"), snapshot)).toBe(false);
    expect(gallery.images.value).toEqual([]);
  });
});

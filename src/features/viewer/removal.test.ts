import { ref } from "vue";
import { beforeEach, expect, it, vi } from "vitest";
import { api } from "../../shared/ipc";
import { useViewerRemoval } from "./useViewerRemoval";
import type { GalleryImage } from "../../shared/contracts/GalleryImage";
vi.mock("../../shared/ipc", () => ({
  api: { mutate: vi.fn(), undoViewerTrash: vi.fn() },
  errorMessage: String,
}));
const image = (id: string) =>
  ({
    id,
    path: id,
    fileName: id,
    ext: "png",
    thumbnailPath: null,
    width: 100,
    height: 100,
    fileSize: 10,
    modifiedAt: 0,
    importedAt: 0,
    folderId: 1,
    missing: false,
    trashed: false,
    isFavorite: false,
    source: "library",
  }) as GalleryImage;
const result = (id: string, trashed: boolean, revision = 1) => ({
  images: [{ ...image(id), trashed }],
  successfulIds: [id],
  failures: [],
  revision,
});
function setup() {
  const rows = ref([image("a"), image("b"), image("c")]),
    total = ref(3),
    current = ref<GalleryImage | null>(rows.value[1]),
    key = ref("tags:long_hair");
  const more = vi.fn(async () => {}),
    changed = vi.fn();
  const removal = useViewerRemoval(
    current,
    () => ({
      key: key.value,
      images: rows.value,
      total: total.value,
      remove: (id) => {
        rows.value = rows.value.filter((i) => i.id !== id);
        total.value--;
      },
      restore: (img, index) => {
        const next = [...rows.value];
        next.splice(index, 0, img);
        rows.value = next;
        total.value++;
      },
      more,
    }),
    changed,
  );
  return { rows, total, current, key, more, changed, removal };
}
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(api.mutate).mockResolvedValue(result("b", true));
  vi.mocked(api.undoViewerTrash).mockResolvedValue(result("b", false, 2));
});
it("成功后继续下一张，撤销恢复原位置但不抢回当前图片", async () => {
  const s = setup();
  await s.removal.trash();
  expect(s.current.value?.id).toBe("c");
  expect(s.rows.value.map((i) => i.id)).toEqual(["a", "c"]);
  expect(s.key.value).toBe("tags:long_hair");
  await s.removal.restore();
  expect(api.undoViewerTrash).toHaveBeenCalledWith("b", 1);
  expect(s.rows.value.map((i) => i.id)).toEqual(["a", "b", "c"]);
  expect(s.current.value?.id).toBe("c");
});
it("撤销返回前关闭查看器，不会重新打开", async () => {
  const s = setup();
  await s.removal.trash();
  let finish!: (v: ReturnType<typeof result>) => void;
  vi.mocked(api.undoViewerTrash).mockImplementation(
    () =>
      new Promise((r) => {
        finish = r;
      }),
  );
  const pending = s.removal.restore();
  s.current.value = null;
  finish(result("b", false, 2));
  await pending;
  expect(s.current.value).toBeNull();
  expect(s.rows.value.map((i) => i.id)).toEqual(["a", "b", "c"]);
  expect(s.removal.undo.value).toBeNull();
});
it("撤销返回前切换到另一张，不会替换当前图片", async () => {
  const s = setup();
  await s.removal.trash();
  let finish!: (v: ReturnType<typeof result>) => void;
  vi.mocked(api.undoViewerTrash).mockImplementation(
    () =>
      new Promise((r) => {
        finish = r;
      }),
  );
  const pending = s.removal.restore();
  s.current.value = image("other");
  finish(result("b", false, 2));
  await pending;
  expect(s.current.value?.id).toBe("other");
  expect(s.rows.value.map((i) => i.id)).toEqual(["a", "b", "c"]);
});
it("重复提交受抑制，失败不移除当前图片", async () => {
  const s = setup();
  let reject!: (e: Error) => void;
  vi.mocked(api.mutate).mockImplementation(
    () =>
      new Promise((_, r) => {
        reject = r;
      }),
  );
  const pending = s.removal.trash();
  await s.removal.trash();
  expect(api.mutate).toHaveBeenCalledTimes(1);
  reject(new Error("locked"));
  await pending;
  expect(s.current.value?.id).toBe("b");
  expect(s.rows.value).toHaveLength(3);
  expect(s.removal.error.value).toContain("locked");
});
it("迟到结果不会改变已经切换的页面，仍保留可撤销记录", async () => {
  const s = setup();
  let finish!: (v: ReturnType<typeof result>) => void;
  vi.mocked(api.mutate).mockImplementation(
    () =>
      new Promise((r) => {
        finish = r;
      }),
  );
  const pending = s.removal.trash();
  s.key.value = "gallery:other";
  s.current.value = image("other");
  finish(result("b", true));
  await pending;
  expect(s.current.value?.id).toBe("other");
  expect(s.rows.value).toHaveLength(3);
  expect(s.removal.undo.value?.image.id).toBe("b");
});
it("最后一张先续载下一页，全部为空则关闭查看器", async () => {
  const s = setup();
  s.current.value = s.rows.value[2];
  s.total.value = 4;
  vi.mocked(api.mutate).mockResolvedValue(result("c", true));
  s.more.mockImplementation(async () => {
    s.rows.value.push(image("d"));
  });
  await s.removal.trash();
  expect(s.more).toHaveBeenCalledOnce();
  expect(s.current.value?.id).toBe("d");
  s.rows.value = [image("d")];
  s.total.value = 1;
  vi.mocked(api.mutate).mockResolvedValue(result("d", true));
  await s.removal.trash();
  expect(s.current.value).toBeNull();
});

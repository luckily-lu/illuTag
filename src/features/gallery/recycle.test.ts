import { beforeEach, expect, it, vi } from "vitest";
import { useRecycleBin } from "./useRecycleBin";
import { api } from "../../shared/ipc";
import type { GalleryQuery } from "../../shared/contracts/GalleryQuery";

vi.mock("../../shared/ipc", () => ({
  api: { prepareRecycleSelection: vi.fn(), applyRecycleAction: vi.fn() },
  errorMessage: String,
}));
beforeEach(() => vi.clearAllMocks());

it("取消操作确认后保留全选快照，下一次确认仅执行新准备的范围", async () => {
  const bin = useRecycleBin();
  const query = {
    scope: "trash", tags: [], fileName: "filtered", englishQuery: "", naturalLanguage: "",
    folderId: null, unclassifiedOnlyParentFolderId: null, randomSeed: null, offset: 0, limit: 100,
  } as GalleryQuery;
  vi.mocked(api.prepareRecycleSelection)
    .mockResolvedValueOnce({ selectionId: "frozen", count: 3, totalBytes: 300 })
    .mockResolvedValueOnce({ selectionId: "cancelled", count: 2, totalBytes: 200 })
    .mockResolvedValueOnce({ selectionId: "confirmed", count: 3, totalBytes: 300 });
  await bin.selectAll(query);
  await bin.apply("systemTrash", query, [], bin.selectionId.value, ["excluded"]);
  bin.confirm.value = null;
  expect(api.applyRecycleAction).not.toHaveBeenCalled();
  expect([bin.selectionId.value, bin.count.value, bin.totalBytes.value]).toEqual(["frozen", 3, 300]);
  await bin.apply("restore", query, [], bin.selectionId.value);
  expect(api.prepareRecycleSelection).toHaveBeenLastCalledWith({
    query: null, imageIds: [], selectionId: "frozen", excludedIds: [],
  });
  await bin.confirmApply();
  expect(api.applyRecycleAction).toHaveBeenCalledWith({
    selectionId: "confirmed", action: "restore", confirmed: false,
  });
});

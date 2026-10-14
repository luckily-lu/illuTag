import { beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../../shared/ipc";
import { useDuplicates } from "./useDuplicates";
import type { DuplicateStatus } from "../../shared/contracts/DuplicateStatus";
import type { DuplicateGroup } from "../../shared/contracts/DuplicateGroup";
import type { DuplicateGroupPage } from "../../shared/contracts/DuplicateGroupPage";
import type { DuplicateMemberPage } from "../../shared/contracts/DuplicateMemberPage";
vi.mock("../../shared/ipc", () => ({
  api: {
    duplicateStatus: vi.fn(),
    duplicateGroups: vi.fn(),
    duplicateMembers: vi.fn(),
    clearDuplicateCache: vi.fn(),
  },
  errorMessage: (error: unknown) => String(error),
}));
const member = (id: string) => ({
  image: {
    id,
    path: id,
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
  metric: { phashDistance: null, dhashDistance: null, colorDistance: null },
  stale: false,
});
const group = (id: string): DuplicateGroup => ({
  id,
  kind: "exact",
  reference: member(id),
  totalMembers: 100,
  members: [member(id)],
  nextCursor: "6",
});
const status = (
  generation: string | null = "one",
  needsUpdate = false,
): DuplicateStatus => ({
  generation,
  sensitivity: "standard",
  checkedAt: null,
  durationMs: null,
  state: generation ? "ready" : "empty",
  needsUpdate,
  coverage: {
    total: 100,
    processed: 100,
    failed: 0,
    skippedNear: 0,
    lowInformation: 0,
    tooDense: 0,
  },
  cacheBytes: 1024,
  exactGroups: 1,
  nearGroups: 0,
  uniqueImages: 100,
  errors: [],
  lastAttempt: null,
});
const page = (
  generation: string,
  id: string,
  nextCursor: string | null = "20",
): DuplicateGroupPage => ({
  generation,
  groups: [group(id)],
  nextCursor,
  totalGroups: 30,
});
describe("查重查询状态", () => {
  beforeEach(() => vi.clearAllMocks());
  it("初次状态读取只请求 20 组，不启动后台任务", async () => {
    vi.mocked(api.duplicateStatus).mockResolvedValue(status());
    vi.mocked(api.duplicateGroups).mockResolvedValue(page("one", "first"));
    const d = useDuplicates();
    await d.refresh();
    expect(api.duplicateGroups).toHaveBeenCalledWith({
      generation: "one",
      kind: "all",
      cursor: null,
      limit: 20,
    });
    expect(d.groups.value[0].id).toBe("first");
  });
  it("切换筛选不会被过期分组响应覆盖", async () => {
    let resolve!: (v: DuplicateGroupPage) => void;
    vi.mocked(api.duplicateGroups)
      .mockImplementationOnce(() => new Promise((r) => (resolve = r)))
      .mockResolvedValueOnce(page("one", "new"));
    const d = useDuplicates();
    d.status.value = status();
    const old = d.loadGroups();
    await d.setKind("near");
    resolve(page("one", "old"));
    await old;
    expect(d.groups.value.map((g) => g.id)).toEqual(["new"]);
    expect(d.loading.value).toBe(false);
  });
  it("分页响应不能混入新一代结果", async () => {
    let resolve!: (v: DuplicateGroupPage) => void;
    vi.mocked(api.duplicateGroups)
      .mockResolvedValueOnce(page("one", "first"))
      .mockImplementationOnce(() => new Promise((r) => (resolve = r)))
      .mockResolvedValueOnce(page("two", "current"));
    const d = useDuplicates();
    d.status.value = status();
    await d.loadGroups();
    const old = d.loadGroups(true);
    vi.mocked(api.duplicateStatus).mockResolvedValue(status("two"));
    await d.refresh();
    resolve(page("one", "stale"));
    await old;
    expect(d.groups.value.map((g) => g.id)).toEqual(["current"]);
    expect(d.pageLoading.value).toBe(false);
  });
  it("成员分页切换筛选后失效，单次成员请求不超过 50", async () => {
    let resolve!: (v: DuplicateMemberPage) => void;
    vi.mocked(api.duplicateMembers).mockImplementationOnce(
      () => new Promise((r) => (resolve = r)),
    );
    vi.mocked(api.duplicateGroups).mockResolvedValue(page("one", "same"));
    const d = useDuplicates();
    d.status.value = status();
    d.groups.value = [group("same")];
    const old = d.moreMembers("same");
    expect(api.duplicateMembers).toHaveBeenCalledWith({
      generation: "one",
      groupId: "same",
      cursor: "6",
      limit: 50,
    });
    await d.setKind("near");
    resolve({
      generation: "one",
      groupId: "same",
      members: [member("stale")],
      totalMembers: 100,
      nextCursor: "56",
    });
    await old;
    expect(d.groups.value[0].members.map((m) => m.image.id)).toEqual(["same"]);
  });
  it("任务进度同一代状态不会周期性重载分组", async () => {
    const d = useDuplicates();
    d.status.value = status();
    d.groups.value = [group("same")];
    vi.mocked(api.duplicateStatus).mockResolvedValue({
      ...status(),
      state: "running",
    });
    await d.refresh();
    expect(api.duplicateGroups).not.toHaveBeenCalled();
    expect(d.groups.value[0].id).toBe("same");
  });
  it("待应用档位不会修改已有结果；过期与强制重算不能只重分组", () => {
    const d = useDuplicates();
    d.status.value = status();
    d.sensitivity.value = "loose";
    expect(d.pendingSensitivity.value).toBe(true);
    expect(d.status.value.sensitivity).toBe("standard");
    expect(d.options(true)).toEqual({
      sensitivity: "loose",
      forceRecompute: false,
      regroupOnly: true,
    });
    d.status.value = status("one", true);
    expect(d.options(true).regroupOnly).toBe(false);
    d.status.value = status();
    d.forceRecompute.value = true;
    expect(d.options(true).regroupOnly).toBe(false);
  });
  it("取消或部分完成记录保留上一代完整结果", async () => {
    const d = useDuplicates();
    d.status.value = status();
    d.groups.value = [group("retained")];
    vi.mocked(api.duplicateStatus).mockResolvedValue({
      ...status(),
      lastAttempt: {
        state: "cancelled",
        coverage: status().coverage,
        updatedAt: 10,
        errors: [],
      },
    });
    await d.refresh();
    expect(d.groups.value[0].id).toBe("retained");
    expect(d.status.value?.lastAttempt?.state).toBe("cancelled");
  });
  it("清理只调用专用缓存接口并清空状态", async () => {
    const d = useDuplicates();
    d.status.value = status();
    d.groups.value = [group("cached")];
    vi.mocked(api.clearDuplicateCache).mockResolvedValue();
    vi.mocked(api.duplicateStatus).mockResolvedValue(status(null));
    await d.clear();
    expect(api.clearDuplicateCache).toHaveBeenCalledTimes(1);
    expect(d.groups.value).toEqual([]);
    expect(d.status.value?.generation).toBeNull();
  });
});

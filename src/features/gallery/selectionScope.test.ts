import { describe, expect, it, vi } from "vitest";
import { nextTick, ref } from "vue";
import type { GalleryQuery } from "../../shared/contracts/GalleryQuery";
import { semanticQueryKey } from "./queryState";
import { useSelectionScope } from "./useSelectionScope";

export const query = (over: Partial<GalleryQuery> = {}): GalleryQuery => ({
  scope: "all",
  folderId: null,
  unclassifiedOnlyParentFolderId: null,
  tags: [],
  englishQuery: "",
  fileName: "",
  naturalLanguage: "",
  offset: 0,
  limit: 5000,
  randomSeed: null,
  ...over,
});

describe("语义查询 key", () => {
  it("标签按后端的 trim、去空、排序、去重归一化", () => {
    expect(semanticQueryKey(query({ tags: [" b", "", "a", "a "] }))).toBe(
      semanticQueryKey(query({ tags: ["a", "b"] })),
    );
  });

  it("标签不做大小写折叠，与后端区分大小写的行为一致", () => {
    expect(semanticQueryKey(query({ tags: ["a", "A"] }))).not.toBe(
      semanticQueryKey(query({ tags: ["a"] })),
    );
  });

  it("英文查询按空白、大小写、排序、去重归一化", () => {
    expect(
      semanticQueryKey(query({ englishQuery: " Cat  cat dog " })),
    ).toBe(semanticQueryKey(query({ englishQuery: "dog cat" })));
  });

  it("文件名忽略首尾空白和大小写，描述只去首尾空白", () => {
    expect(semanticQueryKey(query({ fileName: " Scene.PNG " }))).toBe(
      semanticQueryKey(query({ fileName: "scene.png" })),
    );
    expect(semanticQueryKey(query({ naturalLanguage: "  Cats " }))).not.toBe(
      semanticQueryKey(query({ naturalLanguage: "cats" })),
    );
  });

  it("分页和随机种子不改变语义范围", () => {
    expect(
      semanticQueryKey(query({ offset: 5000, limit: 200, randomSeed: 7 })),
    ).toBe(semanticQueryKey(query()));
  });

  it("范围、分类、标签和文本变化都会改变 key", () => {
    const base = semanticQueryKey(query());
    for (const next of [
      query({ scope: "favorites" }),
      query({ folderId: 3 }),
      query({ unclassifiedOnlyParentFolderId: 9 }),
      query({ tags: ["tag"] }),
      query({ englishQuery: "cat" }),
      query({ fileName: "a.png" }),
      query({ naturalLanguage: "cat" }),
    ]) {
      expect(semanticQueryKey(next)).not.toBe(base);
    }
  });
});

describe("选择范围联动", () => {
  it("语义筛选变化时清空普通选择", async () => {
    const q = ref(query({ fileName: "old.png" }));
    const reset = vi.fn();
    useSelectionScope(q, reset);
    q.value = query({ fileName: "new.png" });
    await nextTick();
    expect(reset).toHaveBeenCalledTimes(1);
  });

  it("只改分页参数不清理选择", async () => {
    const q = ref(query());
    const reset = vi.fn();
    useSelectionScope(q, reset);
    q.value = query({ offset: 5000 });
    await nextTick();
    expect(reset).not.toHaveBeenCalled();
  });

  it("筛选内容等价时不清理选择", async () => {
    const q = ref(query({ tags: ["b", "a"] }));
    const reset = vi.fn();
    useSelectionScope(q, reset);
    q.value = query({ tags: [" a", "b", "a "] });
    await nextTick();
    expect(reset).not.toHaveBeenCalled();
  });
});

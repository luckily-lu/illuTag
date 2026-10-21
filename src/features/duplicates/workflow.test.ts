// @vitest-environment jsdom
import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../../shared/ipc";
import { useDuplicateWorkflow } from "./useDuplicateWorkflow";
import DuplicateWorkflowReview from "./DuplicateWorkflowReview.vue";
import type { DuplicateWorkflowStatus } from "../../shared/contracts/DuplicateWorkflowStatus";
import type { DuplicateWorkflowGroup } from "../../shared/contracts/DuplicateWorkflowGroup";
import type { DuplicateWorkflowPage } from "../../shared/contracts/DuplicateWorkflowPage";
vi.mock("../../shared/ipc", () => ({
  api: {
    duplicateWorkflow: vi.fn(),
    duplicateWorkflowGroups: vi.fn(),
    editDuplicateReview: vi.fn(),
  },
  imageUrl: (s: string) => s,
  errorMessage: (e: unknown) => String(e),
}));
const member = (id: string, keeper = false) => ({
  member: {
    image: {
      id,
      path: "data:image/png;base64,",
      thumbnailPath: null,
      fileName: id,
      ext: "png",
      width: 800,
      height: 600,
      fileSize: 1000,
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
  disposition: keeper ? "keep" : "clean",
  isKeeper: keeper,
  reason: "",
});
const group = (id = "near:1", kind = "near"): DuplicateWorkflowGroup => ({
  id,
  kind,
  referenceId: "a",
  confirmed: false,
  totalMembers: 2,
  members: [member("a", true), member("b")],
  nextCursor: null,
});
const status = (
  revision = "r1",
  generation: string | null = "one",
): DuplicateWorkflowStatus => ({
  generation,
  revision,
  stage: generation ? "refined" : "empty",
  scan: {
    generation,
    sensitivity: "standard",
    verificationMode: "candidate",
    checkedAt: 1,
    durationMs: 1,
    state: generation ? "ready" : "empty",
    needsUpdate: false,
    coverage: {
      total: 10,
      processed: 10,
      failed: 0,
      skippedNear: 0,
      lowInformation: 0,
      tooDense: 0,
      verifiedPairs: 2,
      contentVariants: 1,
      uncertainPairs: 0,
    },
    cacheBytes: 0,
    exactGroups: 1,
    nearGroups: 1,
    uniqueImages: 10,
    errors: [],
    lastAttempt: null,
  },
  categories: [],
  cleanupCount: 1,
  protectedCount: 0,
  lastOperationId: null,
  errors: [],
});
const page = (
  rows: DuplicateWorkflowGroup[] = [group()],
  revision = "r1",
  generation = "one",
): DuplicateWorkflowPage => ({
  generation,
  revision,
  groups: rows,
  nextCursor: null,
  totalGroups: rows.length,
});
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(api.duplicateWorkflow).mockResolvedValue(status());
  vi.mocked(api.duplicateWorkflowGroups).mockResolvedValue(page());
  HTMLElement.prototype.scrollTo = vi.fn();
});

describe("一次副本检索与可选审查状态", () => {
  it("摘要同步中自动轮询，失败停止轮询并允许显式重试，退出清理定时器", async () => {
    vi.useFakeTimers();
    const loading = { ...status(), stage: "loading", cleanupCount: 0 };
    vi.mocked(api.duplicateWorkflow)
      .mockResolvedValueOnce(loading)
      .mockResolvedValueOnce({
        ...status(),
        stage: "error",
        errors: ["读取失败"],
      })
      .mockResolvedValueOnce(status());
    const w = useDuplicateWorkflow();
    await w.refresh();
    expect(w.status.value?.stage).toBe("loading");
    await vi.advanceTimersByTimeAsync(1000);
    expect(w.status.value?.stage).toBe("error");
    await vi.advanceTimersByTimeAsync(3000);
    expect(api.duplicateWorkflow).toHaveBeenCalledTimes(2);
    await w.refresh(false, true);
    expect(api.duplicateWorkflow).toHaveBeenLastCalledWith(true);
    expect(w.status.value?.stage).toBe("refined");
    vi.mocked(api.duplicateWorkflow).mockResolvedValue(loading);
    await w.refresh();
    w.dispose();
    await vi.advanceTimersByTimeAsync(3000);
    expect(api.duplicateWorkflow).toHaveBeenCalledTimes(4);
    vi.useRealTimers();
  });
  it("三轮任务使用cascade，旧参数只用于兼容调用", async () => {
    const w = useDuplicateWorkflow();
    await w.refresh();
    expect(api.duplicateWorkflowGroups).not.toHaveBeenCalled();
    expect(w.options("cascade", "standard", false)).toEqual({
      sensitivity: "standard",
      forceRecompute: false,
      verificationMode: "cascade",
      regroupOnly: false,
    });
    expect(w.options("organize", "loose", true)).toEqual({
      sensitivity: "loose",
      forceRecompute: true,
      verificationMode: "organize",
      regroupOnly: false,
    });
    expect(w.options("scan", "strict", true)).toEqual({
      sensitivity: "strict",
      forceRecompute: true,
      verificationMode: "candidate",
      regroupOnly: false,
    });
    expect(w.options("refine", "standard", false)).toEqual({
      sensitivity: "standard",
      forceRecompute: false,
      verificationMode: "refine",
      regroupOnly: true,
    });
  });
  it("三轮完成后仅完全相同和近似可标记清理，类似与无法确认保持保留", () => {
    const w = useDuplicateWorkflow();
    w.status.value = { ...status(), policy: "illuTag-three-pass-v1" };
    for (const kind of ["exact", "near"]) {
      w.kind.value = kind;
      expect(w.canClean.value).toBe(true);
    }
    for (const kind of ["similar", "uncertain", "candidate"]) {
      w.kind.value = kind;
      expect(w.canClean.value).toBe(false);
    }
    w.kind.value = "exact";
    w.status.value = { ...w.status.value, stage: "scanned" };
    expect(w.canClean.value).toBe(false);
  });
  it("新三轮近似页允许人工调整；类似页保持浏览而不提供待清理", async () => {
    const w = useDuplicateWorkflow();
    w.status.value = { ...status(), policy: "illuTag-three-pass-v1" };
    w.kind.value = "near";
    w.groups.value = [group()];
    const edit = vi.spyOn(w, "edit").mockResolvedValue(true);
    const wrapper = mount(DuplicateWorkflowReview, {
      props: { workflow: w, busy: false, undoable: false, undoBusy: false },
    });
    expect(wrapper.text()).toContain("近似版本不代表全部像素相同");
    const clean = wrapper.findAll(".review-image-actions .danger")[1];
    expect(clean.attributes("disabled")).toBeUndefined();
    await clean.trigger("click");
    expect(edit).toHaveBeenCalledWith("clean", "b");
    w.kind.value = "similar";
    await flushPromises();
    expect(wrapper.text()).toContain("存在内容或细节差异，默认保留");
    expect(wrapper.find(".review-image-actions .danger").exists()).toBe(false);
    expect(wrapper.findAll(".review-image-open")).toHaveLength(2);
    wrapper.unmount();
  });
  it("快速换分类后旧响应不能盖住当前组", async () => {
    const w = useDuplicateWorkflow();
    await w.refresh();
    let release!: (p: DuplicateWorkflowPage) => void;
    vi.mocked(api.duplicateWorkflowGroups).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          release = resolve;
        }),
    );
    const old = w.open("exact");
    vi.mocked(api.duplicateWorkflowGroups).mockResolvedValueOnce(
      page([group("near:2")]),
    );
    await w.open("near");
    release(page([group("exact:1", "exact")]));
    await old;
    expect(w.kind.value).toBe("near");
    expect(w.current.value?.id).toBe("near:2");
  });
  it("重新读取成功清除旧错误，旧请求成功不会清除较新请求的失败", async () => {
    const w = useDuplicateWorkflow();
    vi.mocked(api.duplicateWorkflow).mockRejectedValueOnce("读取失败");
    await w.refresh();
    expect(w.error.value).toBe("读取失败");
    await w.refresh();
    expect(w.error.value).toBe("");
    let release!: (value: DuplicateWorkflowStatus) => void;
    vi.mocked(api.duplicateWorkflow).mockImplementationOnce(
      () => new Promise((resolve) => (release = resolve)),
    );
    const old = w.refresh();
    vi.mocked(api.duplicateWorkflow).mockRejectedValueOnce("新的读取失败");
    await w.refresh();
    release(status());
    await old;
    expect(w.error.value).toBe("新的读取失败");
  });
  it("编辑使用同一计划版本，立即更新数量，不直接删除文件", async () => {
    const w = useDuplicateWorkflow();
    await w.refresh();
    await w.open("near");
    const next = { ...status("r2"), cleanupCount: 0, protectedCount: 1 };
    vi.mocked(api.editDuplicateReview).mockResolvedValue(next);
    vi.mocked(api.duplicateWorkflowGroups).mockResolvedValue(
      page(
        [
          {
            ...group(),
            members: [
              member("a", true),
              { ...member("b"), disposition: "protected" },
            ],
          },
        ],
        "r2",
      ),
    );
    expect(await w.edit("keep", "b")).toBe(true);
    expect(api.editDuplicateReview).toHaveBeenCalledExactlyOnceWith({
      generation: "one",
      revision: "r1",
      groupId: "near:1",
      imageId: "b",
      action: "keep",
    });
    expect(w.status.value?.cleanupCount).toBe(0);
    expect(w.current.value?.members[1].disposition).toBe("protected");
  });
  it("过期编辑不自动重试，并保留新版本结果", async () => {
    const w = useDuplicateWorkflow();
    await w.refresh();
    await w.open("near");
    vi.mocked(api.editDuplicateReview).mockRejectedValue(Error("版本已变化"));
    vi.mocked(api.duplicateWorkflow).mockResolvedValue(status("r2"));
    vi.mocked(api.duplicateWorkflowGroups).mockResolvedValue(
      page([group()], "r2"),
    );
    expect(await w.edit("clean", "b")).toBe(false);
    expect(api.editDuplicateReview).toHaveBeenCalledTimes(1);
    expect(w.status.value?.revision).toBe("r2");
    expect(w.error.value).toContain("版本已变化");
  });
  it("人工操作替代未完成的状态读取后不会留下永久加载锁", async () => {
    const w = useDuplicateWorkflow();
    await w.refresh();
    await w.open("near");
    let finish!: (s: DuplicateWorkflowStatus) => void;
    vi.mocked(api.duplicateWorkflow).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const pending = w.refresh();
    expect(w.statusLoading.value).toBe(true);
    vi.mocked(api.editDuplicateReview).mockResolvedValue(status("r2"));
    vi.mocked(api.duplicateWorkflowGroups).mockResolvedValue(
      page([group()], "r2"),
    );
    expect(await w.edit("keep", "b")).toBe(true);
    finish(status());
    await pending;
    expect(w.statusLoading.value).toBe(false);
    expect(w.status.value?.revision).toBe("r2");
  });
  it("分组分页只保留一页，前后切换仍可达；组内展开按50项读取", async () => {
    const w = useDuplicateWorkflow();
    await w.refresh();
    const first = Array.from({ length: 20 }, (_, i) => group(`near:${i}`));
    vi.mocked(api.duplicateWorkflowGroups).mockImplementation(async (q) => {
      if (q.groupId)
        return page([
          { ...group(q.groupId), nextCursor: q.cursor ? null : "m50" },
        ]);
      return q.cursor
        ? { ...page([group("near:20")]), totalGroups: 21 }
        : { ...page(first), nextCursor: "g20", totalGroups: 21 };
    });
    await w.open("near");
    w.index.value = 19;
    await w.step(1);
    expect(w.groups.value).toHaveLength(1);
    expect(w.position.value).toBe(21);
    await w.step(-1);
    expect(w.groups.value).toHaveLength(20);
    expect(w.position.value).toBe(20);
    await w.open("near");
    await w.moreMembers();
    expect(api.duplicateWorkflowGroups).toHaveBeenLastCalledWith({
      generation: "one",
      kind: "near",
      groupId: "near:19",
      cursor: "m50",
      limit: 50,
    });
  });
  it("复检发布后按当前组重新锚定分页，保留展开成员并使用新的前后游标", async () => {
    const w = useDuplicateWorkflow();
    let published = false;
    vi.mocked(api.duplicateWorkflow).mockImplementation(async () => ({
      ...status(published ? "r2" : "r1"),
      stage: published ? "refined" : "scanned",
    }));
    const rows = (start: number, count: number) =>
      Array.from({ length: count }, (_, i) =>
        group(`candidate:${start + i}`, "candidate"),
      );
    vi.mocked(api.duplicateWorkflowGroups).mockImplementation(async (q) => {
      if (!published)
        return {
          ...page(rows(1, 20)),
          nextCursor: "old-cursor",
          totalGroups: 40,
        };
      if (q.groupId && !q.reanchor)
        return page(
          [
            {
              ...group(q.groupId, "candidate"),
              members: [member("c"), member("d")],
            },
          ],
          "r2",
        );
      if (q.reanchor)
        return {
          ...page(rows(20, 20), "r2"),
          groups: rows(20, 20).map((g, i) =>
            i === 0 ? { ...g, nextCursor: "members-r2" } : g,
          ),
          nextCursor: "fresh-39",
          totalGroups: 40,
          paging: { offset: 19, previousCursor: null, previousLimit: 19 },
        };
      if (q.cursor === "fresh-39")
        return {
          ...page(rows(40, 1), "r2"),
          totalGroups: 40,
          paging: { offset: 39, previousCursor: "fresh-19", previousLimit: 20 },
        };
      if (q.cursor === "fresh-19")
        return {
          ...page(rows(20, 20), "r2"),
          nextCursor: "fresh-39",
          totalGroups: 40,
          paging: { offset: 19, previousCursor: null, previousLimit: 19 },
        };
      throw Error("stale cursor should never be sent");
    });
    await w.refresh();
    await w.open("candidate");
    w.index.value = 19;
    w.groups.value = w.groups.value.map((g, i) =>
      i === 19
        ? {
            ...g,
            members: [member("a"), member("b"), member("c"), member("d")],
          }
        : g,
    );
    published = true;
    await w.refresh();
    expect(w.current.value?.id).toBe("candidate:20");
    expect(w.current.value?.members.map((m) => m.member.image.id)).toEqual([
      "a",
      "b",
      "c",
      "d",
    ]);
    expect(w.position.value).toBe(20);
    expect(w.totalGroups.value).toBe(40);
    expect(w.canPrevious.value).toBe(true);
    w.index.value = 19;
    await w.step(1);
    expect(w.position.value).toBe(40);
    expect(w.current.value?.id).toBe("candidate:40");
    await w.step(-1);
    expect(w.current.value?.id).toBe("candidate:39");
    expect(w.position.value).toBe(39);
    expect(w.error.value).toBe("");
    expect(api.duplicateWorkflowGroups).not.toHaveBeenCalledWith(
      expect.objectContaining({ cursor: "old-cursor" }),
    );
  });
  it("编辑期间的后台刷新延后执行，旧状态不能覆盖人工选择后的新版本", async () => {
    const w = useDuplicateWorkflow();
    await w.refresh();
    await w.open("near");
    let finish!: (value: DuplicateWorkflowStatus) => void;
    vi.mocked(api.editDuplicateReview).mockImplementationOnce(
      () => new Promise((resolve) => (finish = resolve)),
    );
    vi.mocked(api.duplicateWorkflow).mockResolvedValue(status("r3"));
    vi.mocked(api.duplicateWorkflowGroups).mockImplementation(async (q) =>
      page([group()], q.reanchor ? "r3" : "r2"),
    );
    const pending = w.edit("keep", "b");
    await w.refresh();
    expect(api.duplicateWorkflow).toHaveBeenCalledTimes(1);
    finish(status("r2"));
    expect(await pending).toBe(true);
    expect(api.duplicateWorkflow).toHaveBeenCalledTimes(2);
    expect(w.status.value?.revision).toBe("r3");
    expect(w.editing.value).toBe(false);
    expect(w.statusLoading.value).toBe(false);
  });
  it("关闭审查期间结果更新，再打开原分类也重新建立游标", async () => {
    const w = useDuplicateWorkflow();
    await w.refresh();
    await w.open("near");
    w.close();
    vi.mocked(api.duplicateWorkflow).mockResolvedValue(status("r2"));
    await w.refresh();
    vi.mocked(api.duplicateWorkflowGroups).mockResolvedValue(
      page([group()], "r2"),
    );
    await w.open("near");
    expect(api.duplicateWorkflowGroups).toHaveBeenLastCalledWith({
      generation: "one",
      kind: "near",
      groupId: "near:1",
      cursor: null,
      limit: 50,
      reanchor: true,
    });
    expect(w.current.value?.id).toBe("near:1");
  });
  it("确认过程中退出审查不会由迟到响应重新读组或推进下一组", async () => {
    const w = useDuplicateWorkflow();
    await w.refresh();
    await w.open("near");
    let finish!: (value: DuplicateWorkflowStatus) => void;
    vi.mocked(api.editDuplicateReview).mockImplementationOnce(
      () => new Promise((resolve) => (finish = resolve)),
    );
    const pending = w.confirmNext();
    w.close();
    finish(status("r2"));
    await pending;
    expect(w.reviewOpen.value).toBe(false);
    expect(w.status.value?.revision).toBe("r2");
    expect(api.duplicateWorkflowGroups).toHaveBeenCalledTimes(1);
    expect(w.index.value).toBe(0);
  });
  it("离开页面后迟到的人工响应不再覆盖状态或发起新查询", async () => {
    const w = useDuplicateWorkflow();
    await w.refresh();
    await w.open("near");
    let finish!: (value: DuplicateWorkflowStatus) => void;
    vi.mocked(api.editDuplicateReview).mockImplementationOnce(
      () => new Promise((resolve) => (finish = resolve)),
    );
    const pending = w.edit("keep", "b");
    w.dispose();
    finish(status("r2"));
    expect(await pending).toBe(false);
    expect(w.status.value?.revision).toBe("r1");
    expect(api.duplicateWorkflowGroups).toHaveBeenCalledTimes(1);
  });
  it("类似图具有人工待清理操作，确认下一组及键盘可用；未复检候选不允许清理", async () => {
    const w = useDuplicateWorkflow();
    w.status.value = status();
    w.kind.value = "similar";
    w.groups.value = [
      {
        ...group("similar:1", "similar"),
        members: [member("a", true), { ...member("b"), disposition: "keep" }],
      },
      group("similar:2", "similar"),
    ];
    w.totalGroups.value = 2;
    w.reviewOpen.value = true;
    const edit = vi.spyOn(w, "edit").mockResolvedValue(true),
      step = vi.spyOn(w, "step").mockResolvedValue(),
      confirm = vi.spyOn(w, "confirmNext").mockResolvedValue();
    const wrapper = mount(DuplicateWorkflowReview, {
      props: { workflow: w, busy: false, undoable: true, undoBusy: false },
      attachTo: document.body,
    });
    expect(wrapper.findAll(".review-image")).toHaveLength(2);
    expect(wrapper.findAll(".review-image-open img").length).toBe(2);
    const clean = wrapper.findAll(".review-image-actions .danger");
    expect(clean[0].attributes("disabled")).toBeDefined();
    expect(clean[1].attributes("disabled")).toBeUndefined();
    await clean[1].trigger("click");
    expect(edit).toHaveBeenCalledWith("clean", "b");
    await wrapper.find(".review-footer button").trigger("click");
    expect(confirm).toHaveBeenCalledTimes(1);
    window.dispatchEvent(
      new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }),
    );
    expect(step).toHaveBeenCalledWith(1);
    expect(wrapper.text()).toContain("撤销本次整理并继续审查");
    w.kind.value = "candidate";
    w.status.value = { ...status(), stage: "scanned" };
    await flushPromises();
    expect(
      wrapper
        .findAll(".review-image-actions .danger")
        .every((n) => n.attributes("disabled") !== undefined),
    ).toBe(true);
    wrapper.unmount();
  });
  it("整理期间限制修改但仍可用按钮和键盘切组、展开成员", async () => {
    const w = useDuplicateWorkflow();
    w.status.value = status();
    w.groups.value = [{ ...group(), nextCursor: "members" }, group("near:2")];
    w.totalGroups.value = 2;
    const step = vi.spyOn(w, "step").mockResolvedValue();
    const more = vi.spyOn(w, "moreMembers").mockResolvedValue();
    const confirm = vi.spyOn(w, "confirmNext").mockResolvedValue();
    const wrapper = mount(DuplicateWorkflowReview, {
      props: {
        workflow: w,
        busy: true,
        undoable: false,
        undoBusy: true,
        blockedReason: "正在整理图片，仍可浏览",
      },
      attachTo: document.body,
    });
    expect(wrapper.text()).toContain("正在整理图片，仍可浏览");
    const next = wrapper.findAll(".review-group-heading button")[1];
    expect(next.attributes("disabled")).toBeUndefined();
    await next.trigger("click");
    window.dispatchEvent(
      new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }),
    );
    expect(step).toHaveBeenCalledTimes(2);
    await wrapper.get(".review-more").trigger("click");
    expect(more).toHaveBeenCalledTimes(1);
    expect(
      wrapper.get(".review-footer button").attributes("disabled"),
    ).toBeDefined();
    window.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Enter",
        ctrlKey: true,
        bubbles: true,
      }),
    );
    expect(confirm).not.toHaveBeenCalled();
    wrapper.unmount();
  });
});

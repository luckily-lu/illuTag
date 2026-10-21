// @vitest-environment jsdom
import { mount, flushPromises, config } from "@vue/test-utils";
import { ref } from "vue";
import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import { api } from "../../shared/ipc";
import type { DuplicateActionPreview } from "../../shared/contracts/DuplicateActionPreview";
import type { DuplicateOperationStatus } from "../../shared/contracts/DuplicateOperationStatus";
import type { DuplicateStatus } from "../../shared/contracts/DuplicateStatus";
import type { DuplicateOptions } from "../../shared/contracts/DuplicateOptions";
import type { JobRecord } from "../../shared/contracts/JobRecord";
import { useDuplicateBatches } from "./useDuplicateBatches";
import { useDuplicateOperations } from "./useDuplicateOperations";
import { useDuplicates } from "./useDuplicates";
import DuplicatePage from "./DuplicatePage.vue";
import DuplicateOperationPanel from "./DuplicateOperationPanel.vue";
const { listeners } = vi.hoisted(() => ({
  listeners: new Map<
    string,
    (event: { payload: DuplicateOperationStatus }) => void
  >(),
}));

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: async (
    name: string,
    callback: (event: { payload: DuplicateOperationStatus }) => void,
  ) => {
    listeners.set(name, callback);
    return () => listeners.delete(name);
  },
}));
vi.mock("../../shared/ipc", () => ({
  api: {
    duplicateWorkflow: vi.fn(),
    duplicateWorkflowGroups: vi.fn(),
    editDuplicateReview: vi.fn(),
    prepareDuplicateWorkflow: vi.fn(),
    prepareDuplicateAction: vi.fn(),
    applyDuplicateAction: vi.fn(),
    duplicateOperationStatus: vi.fn(),
    duplicateOperationResults: vi.fn(),
    undoDuplicateAction: vi.fn(),
    duplicateGroups: vi.fn(),
    duplicateMembers: vi.fn(),
    duplicateStatus: vi.fn(),
    clearDuplicateManagementRecords: vi.fn(),
  },
  imageUrl: (value: string) => value,
  errorMessage: (e: unknown) => String(e),
}));
const scanStatus = (): DuplicateStatus => ({
  generation: "published",
  sensitivity: "standard",
  verificationMode: "candidate",
  checkedAt: 1,
  durationMs: 100,
  state: "ready",
  needsUpdate: false,
  coverage: {
    total: 1_000_000,
    processed: 1_000_000,
    failed: 0,
    skippedNear: 0,
    lowInformation: 0,
    tooDense: 0,
    verifiedPairs: 0,
    contentVariants: 0,
    uncertainPairs: 0,
  },
  cacheBytes: 100,
  exactGroups: 25_414,
  nearGroups: 90_981,
  uniqueImages: 100_000,
  errors: [],
  lastAttempt: null,
});
const preview = (
  groupId = "all-exact",
  operationId = "batch",
): DuplicateActionPreview => ({
  operationId,
  generation: "published",
  groupId,
  action: "trash",
  targetCount: 0,
  totalBytes: 0,
  protectedCount: 0,
  skippedCount: 0,
  metadata: {
    favoriteCount: 0,
    classifiedCount: 0,
    customTaggedCount: 0,
    supplementTaggedCount: 0,
  },
  sample: [],
  warnings: [],
  expiresAt: 0,
  requiresSystemConfirmation: false,
});
const operationStatus = (
  state = "running",
  operationId = "batch",
): DuplicateOperationStatus => ({
  operationId,
  generation: "published",
  groupId: "all-exact",
  action: "trash",
  state,
  stage: state === "completed" ? "applying" : "snapshot",
  stageProcessed: 0,
  stageTotal: 0,
  total: 0,
  processed: 0,
  succeeded: 0,
  failed: 0,
  skipped: 0,
  undoable: false,
  affectedGroupIds: [],
  affectedGroupsTruncated: false,
  errors: [],
  updatedAt: 1,
});
const job = (
  state = "running",
  id = "job",
  operationId = "batch",
): JobRecord => ({
  id,
  request: {
    kind: "duplicateManagement",
    thumbnails: false,
    sourceFolderId: null,
    duplicateOptions: null,
    duplicateOperationId: operationId,
  },
  state,
  stage: "汇总全部扫描结果",
  processed: 0,
  total: 0,
  failures: 0,
  errors: [],
  updatedAt: 1,
});
function setup() {
  const d = useDuplicates(),
    batches = useDuplicateBatches();
  d.status.value = scanStatus();
  d.kind.value = "near";
  vi.spyOn(d, "refresh").mockResolvedValue();
  vi.spyOn(d, "loadGroups").mockResolvedValue();
  const jobs = ref<JobRecord[]>([]);
  const created = vi.fn((value: JobRecord) => {
    jobs.value = [value, ...jobs.value];
  });
  const management = useDuplicateOperations(d, batches, {
    jobs: () => jobs.value,
    created,
  });
  return { d, batches, jobs, created, management };
}
beforeEach(() => {
  config.global.stubs.Teleport = true;
  vi.resetAllMocks();
  listeners.clear();
  vi.mocked(api.duplicateWorkflow).mockResolvedValue({
    generation: "published",
    revision: "plan-1",
    stage: "refined",
    policy: "illuTag-three-pass-v1",
    scan: scanStatus(),
    categories: [
      { kind: "exact", images: 200, groups: 20, cleanupCount: 180 },
      { kind: "near", images: 800, groups: 80, cleanupCount: 720 },
      { kind: "similar", images: 200, groups: 20, cleanupCount: 0 },
      { kind: "uncertain", images: 20, groups: 2, cleanupCount: 0 },
    ],
    rounds: [
      {
        round: 1,
        name: "宽筛候选",
        state: "completed",
        inputPairs: 1500,
        passedPairs: 1000,
        filteredPairs: 500,
        uncertainPairs: 0,
      },
      {
        round: 2,
        name: "内容复检",
        state: "completed",
        inputPairs: 1000,
        passedPairs: 800,
        filteredPairs: 180,
        uncertainPairs: 20,
      },
      {
        round: 3,
        name: "严格确认",
        state: "completed",
        inputPairs: 800,
        passedPairs: 200,
        filteredPairs: 600,
        uncertainPairs: 0,
      },
    ],
    cleanupCount: 900,
    protectedCount: 0,
    lastOperationId: null,
    errors: [],
  });
  vi.mocked(api.duplicateWorkflowGroups).mockResolvedValue({
    generation: "published",
    revision: "plan-1",
    groups: [],
    totalGroups: 0,
    nextCursor: null,
  });
  vi.mocked(api.prepareDuplicateWorkflow).mockResolvedValue({
    ...preview("workflow:plan-1"),
    targetCount: 900,
  });
  HTMLElement.prototype.scrollTo = vi.fn();
  vi.mocked(api.prepareDuplicateAction).mockResolvedValue(preview());
  vi.mocked(api.applyDuplicateAction).mockResolvedValue(job());
  vi.mocked(api.duplicateOperationStatus).mockResolvedValue(operationStatus());
  vi.mocked(api.duplicateOperationResults).mockResolvedValue({
    operationId: "batch",
    items: [],
    nextCursor: null,
  });
});
afterEach(() => {
  delete config.global.stubs.Teleport;
});

describe("全库自动整理批次", () => {
  it.each([
    ["exact", "整理完全相同图片"],
    ["near", "整理近似图片"],
  ] as const)(
    "三轮%s分类批次显示对应名称，并仅继续有版本校验的原批次",
    async (kind, label) => {
      const { batches } = setup();
      batches.register(preview(`workflow:plan-1:${kind}`), job("partial"));
      const wrapper = mount(DuplicateOperationPanel, {
        props: { operation: batches.latest.value!, busy: false },
      });
      expect(wrapper.text()).toContain(label);
      const retry = wrapper
        .findAll("button")
        .find((b) => /继续|重试/.test(b.text()));
      expect(retry).toBeDefined();
      await retry!.trigger("click");
      expect(wrapper.emitted("retry")).toHaveLength(1);
      wrapper.unmount();
    },
  );
  it.each([
    ["exact", "all-exact", "autoExactAll"],
    ["near", "all-near", "autoNearAll"],
  ] as const)(
    "%s 直接提交全库策略，不枚举分页结果或等待人工确认",
    async (kind, groupId, mode) => {
      const { d, batches, management } = setup();
      d.kind.value = kind === "exact" ? "near" : "exact";
      vi.mocked(api.prepareDuplicateAction).mockResolvedValue(preview(groupId));
      await management.startAll(kind);
      expect(api.prepareDuplicateAction).toHaveBeenCalledExactlyOnceWith({
        generation: "published",
        groupId,
        mode,
        keepIds: [],
        protectedIds: [],
        imageIds: [],
        action: "trash",
        sourceOperationId: null,
      });
      expect(api.applyDuplicateAction).toHaveBeenCalledExactlyOnceWith({
        operationId: "batch",
        confirmed: true,
      });
      expect(batches.latest.value?.jobId).toBe("job");
      expect(api.duplicateGroups).not.toHaveBeenCalled();
      expect(api.duplicateMembers).not.toHaveBeenCalled();
      expect(api.duplicateOperationResults).not.toHaveBeenCalled();
    },
  );
  it("准备期间重复点击只建立一个批次", async () => {
    const { management } = setup();
    let resolve!: (value: DuplicateActionPreview) => void;
    vi.mocked(api.prepareDuplicateAction).mockImplementationOnce(
      () =>
        new Promise((r) => {
          resolve = r;
        }),
    );
    const first = management.startAll("exact");
    await management.startAll("near");
    expect(api.prepareDuplicateAction).toHaveBeenCalledTimes(1);
    resolve(preview());
    await first;
    expect(api.applyDuplicateAction).toHaveBeenCalledTimes(1);
  });
  it("完成后只刷新一页，并按50条读取失败记录，不遍历成功项", async () => {
    const { d, batches, management } = setup();
    batches.register(preview(), job("completed"));
    vi.mocked(api.duplicateOperationStatus).mockResolvedValue({
      ...operationStatus("completed"),
      total: 200_000,
      processed: 200_000,
      succeeded: 199_900,
      failed: 100,
      affectedGroupIds: Array.from({ length: 1_000 }, (_, i) => `g${i}`),
      affectedGroupsTruncated: true,
    });
    const operation = batches.latest.value!;
    await management.sync(operation);
    expect(d.loadGroups).toHaveBeenCalledTimes(1);
    expect(api.duplicateOperationResults).toHaveBeenCalledExactlyOnceWith({
      operationId: "batch",
      cursor: null,
      limit: 50,
      state: "failed",
    });
    expect(api.duplicateMembers).not.toHaveBeenCalled();
    await management.sync(operation);
    expect(d.loadGroups).toHaveBeenCalledTimes(1);
    expect(api.duplicateOperationResults).toHaveBeenCalledTimes(1);
  });
  it("中断后继续使用原批次ID，避免重新随机保留", async () => {
    const { batches, management } = setup();
    batches.register(preview(), job("interrupted"));
    vi.mocked(api.applyDuplicateAction).mockResolvedValue(
      job("running", "resumed-job"),
    );
    await management.retry(batches.latest.value!);
    expect(api.prepareDuplicateAction).not.toHaveBeenCalled();
    expect(api.applyDuplicateAction).toHaveBeenCalledExactlyOnceWith({
      operationId: "batch",
      confirmed: true,
    });
    expect(batches.operations.value.size).toBe(1);
    expect(batches.latest.value?.jobId).toBe("resumed-job");
  });
  it("撤销作用于整个批次，切换页面不丢失批次记录", async () => {
    const { batches, management } = setup();
    batches.register(preview(), job("completed"));
    vi.mocked(api.undoDuplicateAction).mockResolvedValue({
      ...job("queued", "undo-job", "undo-batch"),
      request: {
        ...job().request,
        kind: "duplicateUndo",
        duplicateOperationId: "undo-batch",
      },
    });
    vi.mocked(api.duplicateOperationStatus).mockImplementation(async (id) => ({
      ...operationStatus("running", id),
      action: id === "undo-batch" ? "restore" : "trash",
    }));
    await management.undo(batches.latest.value!);
    expect(api.undoDuplicateAction).toHaveBeenCalledExactlyOnceWith("batch");
    expect(batches.operations.value.size).toBe(2);
    expect(batches.latest.value?.preview.action).toBe("restore");
    expect(batches.latest.value?.preview.operationId).toBe("undo-batch");
  });
});

describe("简化的整理页面", () => {
  it("后台复检时可人工保留和切组，全库扫描只锁修改并说明原因", async () => {
    const { d, batches } = setup();
    const healthy = await api.duplicateWorkflow();
    const reviewGroup = (id: string) => ({
      id,
      kind: "exact",
      referenceId: "a",
      confirmed: false,
      totalMembers: 2,
      nextCursor: null,
      members: ["a", "b"].map((name, index) => ({
        member: {
          image: {
            id: name,
            path: name,
            thumbnailPath: null,
            fileName: name,
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
          metric: {
            phashDistance: null,
            dhashDistance: null,
            colorDistance: null,
          },
          stale: false,
        },
        disposition: index ? "clean" : "keep",
        isKeeper: index === 0,
        reason: "",
      })),
    });
    vi.mocked(api.duplicateWorkflowGroups).mockImplementation(async (q) => ({
      generation: "published",
      revision: "plan-1",
      nextCursor: null,
      totalGroups: 2,
      groups: q.groupId
        ? [reviewGroup(q.groupId)]
        : [reviewGroup("exact:1"), reviewGroup("exact:2")],
    }));
    vi.mocked(api.editDuplicateReview).mockResolvedValue(healthy);
    const refine = {
      ...job(),
      stage: "第二轮：筛选近似与细节差异",
      request: {
        ...job().request,
        kind: "duplicates",
        duplicateOperationId: null,
        duplicateOptions: {
          sensitivity: "standard",
          verificationMode: "cascade",
          forceRecompute: false,
          regroupOnly: true,
        },
      },
    };
    const wrapper = mount(DuplicatePage, {
      props: {
        workspace: d,
        batches,
        jobs: [refine],
        folders: [],
        refreshVersion: 0,
        taskError: "",
        startJob: vi.fn(),
        controlJob: vi.fn(),
      },
      global: { stubs: { SoftSelect: true } },
    });
    await flushPromises();
    await wrapper.get('[data-review-kind="exact"]').trigger("click");
    await flushPromises();
    expect(wrapper.text()).toContain("检索在后台进行");
    const keep = wrapper
      .findAll(".review-image-actions button")
      .find((b) => b.text() === "保留")!;
    expect(keep.attributes("disabled")).toBeUndefined();
    await keep.trigger("click");
    await flushPromises();
    expect(api.editDuplicateReview).toHaveBeenCalledWith(
      expect.objectContaining({ action: "keep" }),
    );
    expect(
      wrapper.get(".review-footer button").attributes("disabled"),
    ).toBeUndefined();
    expect(
      wrapper.findAll(".review-group-heading button")[1].attributes("disabled"),
    ).toBeUndefined();
    await wrapper.setProps({ jobs: [{ ...refine, stage: "载入指纹索引" }] });
    expect(wrapper.text()).toContain("全库检索正在更新图片指纹");
    expect(wrapper.findAll(".review-image-actions button").every(
      (button) => button.attributes("disabled") !== undefined,
    )).toBe(true);
    expect(wrapper.findAll(".review-group-heading button")[1].attributes("disabled")).toBeUndefined();
    await wrapper.setProps({ jobs: [{ ...refine, stage: "第一轮：宽筛全库候选" }] });
    expect(wrapper.findAll(".review-image-actions button").find(
      (button) => button.text() === "保留",
    )!.attributes("disabled")).toBeUndefined();
    await wrapper.setProps({
      jobs: [
        {
          ...refine,
          request: {
            ...refine.request,
            duplicateOptions: {
              ...refine.request.duplicateOptions,
              verificationMode: "candidate",
            },
          },
        },
      ],
    });
    expect(wrapper.text()).toContain("全库检索正在更新图片指纹");
    expect(
      wrapper
        .findAll(".review-image-actions button")
        .every((b) => b.attributes("disabled") !== undefined),
    ).toBe(true);
    expect(
      wrapper.findAll(".review-group-heading button")[1].attributes("disabled"),
    ).toBeUndefined();
    wrapper.unmount();
  });
  it("首次读取未完成时明确显示加载，不显示空库启动入口", async () => {
    const { d, batches } = setup();
    const healthy = await api.duplicateWorkflow();
    let resolve!: (value: typeof healthy) => void;
    vi.mocked(api.duplicateWorkflow).mockImplementationOnce(
      () =>
        new Promise((done) => {
          resolve = done;
        }),
    );
    const wrapper = mount(DuplicatePage, {
      props: {
        workspace: d,
        batches,
        jobs: [],
        folders: [],
        refreshVersion: 0,
        taskError: "",
        startJob: vi.fn(),
        controlJob: vi.fn(),
      },
      global: { stubs: { SoftSelect: true } },
    });
    await flushPromises();
    expect(wrapper.text()).toContain("正在读取检索结果");
    expect(wrapper.text()).not.toContain("从全库检索开始");
    await wrapper.get(".workflow-heading > button").trigger("click");
    expect(
      wrapper.get(".workflow-options > button").attributes("disabled"),
    ).toBeDefined();
    expect(wrapper.text()).toContain("缓存信息待读取");
    resolve(healthy);
    await flushPromises();
    expect(wrapper.text()).toContain("三轮检索结果");
    expect(wrapper.text()).not.toContain("正在读取检索结果");
    wrapper.unmount();
  });
  it("历史待提交预览不会锁住新的全库检索", async () => {
    const { d, batches } = setup();
    batches.register(preview(), job("prepared"));
    vi.mocked(api.duplicateOperationStatus).mockResolvedValue(
      operationStatus("prepared"),
    );
    const healthy = await api.duplicateWorkflow();
    vi.mocked(api.duplicateWorkflow).mockResolvedValue({
      ...healthy,
      generation: null,
      stage: "empty",
      scan: { ...scanStatus(), generation: null, state: "empty" },
      categories: [],
      cleanupCount: 0,
    });
    const startJob = vi.fn(async (_options: DuplicateOptions) => true);
    const wrapper = mount(DuplicatePage, {
      props: {
        workspace: d,
        batches,
        jobs: [],
        folders: [],
        refreshVersion: 0,
        taskError: "",
        startJob,
        controlJob: vi.fn(),
      },
      global: { stubs: { SoftSelect: true } },
    });
    await flushPromises();
    const start = wrapper.get(".workflow-empty .primary");
    expect(start.attributes("disabled")).toBeUndefined();
    await start.trigger("click");
    await flushPromises();
    expect(startJob).toHaveBeenCalledTimes(1);
    expect(startJob.mock.calls[0][0].verificationMode).toBe("cascade");
    wrapper.unmount();
  });
  it("数据库读取错误只在弹窗详情中显示，刷新结果可恢复且不重放任务", async () => {
    const { d, batches } = setup();
    const healthy = await api.duplicateWorkflow();
    vi.mocked(api.duplicateWorkflow)
      .mockReset()
      .mockRejectedValueOnce("invalid uri authority: ?")
      .mockResolvedValue(healthy);
    const startJob = vi.fn();
    const wrapper = mount(DuplicatePage, {
      props: {
        workspace: d,
        batches,
        jobs: [],
        folders: [],
        refreshVersion: 0,
        taskError: "",
        startJob,
        controlJob: vi.fn(),
      },
      attachTo: document.body,
      global: { stubs: { SoftSelect: true } },
    });
    await flushPromises();
    expect(wrapper.get('[role="dialog"]').text()).toContain(
      "暂时无法读取图库数据",
    );
    expect(wrapper.find(".error-message").exists()).toBe(false);
    expect(wrapper.find(".error-dialog-details").exists()).toBe(false);
    await wrapper.get(".error-dialog-details-toggle").trigger("click");
    expect(wrapper.get(".error-dialog-details").text()).toBe(
      "invalid uri authority: ?",
    );
    await wrapper.get(".error-dialog footer .text-button").trigger("click");
    await flushPromises();
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
    expect(wrapper.findAll(".workflow-result-row")).toHaveLength(4);
    expect(api.duplicateWorkflow).toHaveBeenCalledTimes(2);
    expect(startJob).not.toHaveBeenCalled();
    expect(api.applyDuplicateAction).not.toHaveBeenCalled();
    expect(api.prepareDuplicateWorkflow).not.toHaveBeenCalled();
    expect(api.editDuplicateReview).not.toHaveBeenCalled();
    expect(api.undoDuplicateAction).not.toHaveBeenCalled();
    wrapper.unmount();
  });
  it("任务错误关闭后进度更新不重弹；新错误会提示，取消记录只保留摘要", async () => {
    const { d, batches } = setup();
    const scan = {
      ...job(),
      request: {
        ...job().request,
        kind: "duplicates",
        duplicateOperationId: null,
      },
    };
    const wrapper = mount(DuplicatePage, {
      props: {
        workspace: d,
        batches,
        jobs: [scan],
        folders: [],
        refreshVersion: 0,
        taskError: "invalid uri authority: ?",
        startJob: vi.fn(),
        controlJob: vi.fn(),
      },
      attachTo: document.body,
      global: { stubs: { SoftSelect: true } },
    });
    await flushPromises();
    await wrapper.get(".error-dialog footer .primary").trigger("click");
    for (let progress = 1; progress <= 6; progress++) {
      await wrapper.setProps({
        jobs: [{ ...scan, processed: progress }],
        refreshVersion: progress,
      });
      await flushPromises();
    }
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
    await wrapper.setProps({ taskError: "另一个读取错误" });
    expect(wrapper.find('[role="dialog"]').exists()).toBe(true);
    await wrapper.get(".error-dialog footer .primary").trigger("click");
    await wrapper.setProps({
      taskError: "",
      jobs: [{ ...scan, state: "cancelled", errors: ["人工停止复检"] }],
    });
    await flushPromises();
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
    expect(wrapper.get(".workflow-attempt").text()).toContain("已取消");
    expect(
      wrapper.get(".workflow-attempt details").attributes("open"),
    ).toBeUndefined();
    wrapper.unmount();
  });
  it("分类查看中的读取失败使用弹窗，刷新重读当前分类，Esc 不关闭分类查看", async () => {
    const { d, batches } = setup();
    const wrapper = mount(DuplicatePage, {
      props: {
        workspace: d,
        batches,
        jobs: [],
        folders: [],
        refreshVersion: 0,
        taskError: "",
        startJob: vi.fn(),
        controlJob: vi.fn(),
      },
      attachTo: document.body,
      global: { stubs: { SoftSelect: true } },
    });
    await flushPromises();
    vi.mocked(api.duplicateWorkflowGroups).mockRejectedValueOnce(
      "分类读取失败",
    );
    await wrapper.get('[data-review-kind="near"]').trigger("click");
    await flushPromises();
    expect(wrapper.find(".workflow-review").exists()).toBe(true);
    expect(wrapper.find(".workflow-review .error-message").exists()).toBe(
      false,
    );
    expect(wrapper.find('[role="dialog"]').exists()).toBe(true);
    await wrapper.get(".error-dialog footer .text-button").trigger("click");
    await flushPromises();
    expect(api.duplicateWorkflowGroups).toHaveBeenCalledTimes(2);
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
    await wrapper.setProps({ taskError: "新的任务错误" });
    await flushPromises();
    window.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
    );
    await flushPromises();
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
    expect(wrapper.find(".workflow-review").exists()).toBe(true);
    wrapper.unmount();
  });
  it("失败任务终态弹出原因，同时保留未完成摘要及折叠日志", async () => {
    const { d, batches } = setup();
    const failedScan = {
      ...job("failed"),
      request: {
        ...job().request,
        kind: "duplicates",
        duplicateOperationId: null,
      },
      errors: ["invalid uri authority: ?"],
    };
    const wrapper = mount(DuplicatePage, {
      props: {
        workspace: d,
        batches,
        jobs: [failedScan],
        folders: [],
        refreshVersion: 0,
        taskError: "",
        startJob: vi.fn(),
        controlJob: vi.fn(),
      },
      global: { stubs: { SoftSelect: true } },
    });
    await flushPromises();
    expect(wrapper.find('[role="dialog"]').exists()).toBe(true);
    expect(wrapper.get(".workflow-attempt").text()).toContain(
      "保留上一次完整结果",
    );
    expect(
      wrapper.get(".workflow-attempt details").attributes("open"),
    ).toBeUndefined();
    wrapper.unmount();
  });
  it.each(["exact", "near"] as const)(
    "%s 按全类别单独整理，不枚举分页图片或要求人工审查",
    async (kind) => {
      const { d, batches } = setup();
      const wrapper = mount(DuplicatePage, {
        props: {
          workspace: d,
          batches,
          jobs: [],
          folders: [],
          refreshVersion: 0,
          taskError: "",
          startJob: vi.fn(),
          controlJob: vi.fn(),
        },
        global: { stubs: { SoftSelect: true } },
      });
      await flushPromises();
      expect(wrapper.findAll(".workflow-result-row")).toHaveLength(4);
      expect(
        wrapper
          .get('[data-category="exact"] .workflow-result-count > strong')
          .text(),
      ).toBe("200");
      expect(
        wrapper
          .get('[data-category="near"] .workflow-result-count > strong')
          .text(),
      ).toBe("800");
      expect(wrapper.findAll("[data-review-kind]")).toHaveLength(4);
      expect(wrapper.findAll(".workflow-rounds > li")).toHaveLength(3);
      expect(wrapper.get('[data-round="1"]').text()).toContain(
        "1,000 / 1,500 对",
      );
      expect(
        wrapper.find('[data-category="similar"] [data-cleanup-kind]').exists(),
      ).toBe(false);
      expect(
        wrapper
          .find('[data-category="uncertain"] [data-cleanup-kind]')
          .exists(),
      ).toBe(false);
      expect(wrapper.text()).not.toMatch(/检索模式|同图副本（细节复核）/);
      await wrapper.get(`[data-cleanup-kind="${kind}"]`).trigger("click");
      await flushPromises();
      expect(api.prepareDuplicateWorkflow).toHaveBeenCalledExactlyOnceWith(
        {
          generation: "published",
          revision: "plan-1",
        },
        kind,
      );
      expect(api.prepareDuplicateAction).not.toHaveBeenCalled();
      expect(api.applyDuplicateAction).toHaveBeenCalledExactlyOnceWith({
        operationId: "batch",
        confirmed: true,
      });
      expect(api.duplicateWorkflowGroups).not.toHaveBeenCalled();
      expect(wrapper.find(".soft-dialog").exists()).toBe(false);
      wrapper.unmount();
    },
  );
  it("旧有损结果只能启动新版验证，不能直接沿用旧清理计划", async () => {
    const { d, batches } = setup();
    const healthy = await api.duplicateWorkflow();
    vi.mocked(api.duplicateWorkflow).mockResolvedValue({
      ...healthy,
      policy: undefined,
    });
    const startJob = vi.fn(async (_options: DuplicateOptions) => true);
    const wrapper = mount(DuplicatePage, {
      props: {
        workspace: d,
        batches,
        jobs: [],
        folders: [],
        refreshVersion: 0,
        taskError: "",
        startJob,
        controlJob: vi.fn(),
      },
      global: { stubs: { SoftSelect: true } },
    });
    await flushPromises();
    expect(wrapper.text()).toContain("已保存旧版检索结果");
    expect(wrapper.find(".workflow-cleanup-button").exists()).toBe(false);
    expect(api.prepareDuplicateWorkflow).not.toHaveBeenCalled();
    await wrapper.get(".workflow-legacy .primary").trigger("click");
    await flushPromises();
    expect(startJob).toHaveBeenCalledExactlyOnceWith({
      sensitivity: "standard",
      verificationMode: "cascade",
      forceRecompute: false,
      regroupOnly: false,
    });
    wrapper.unmount();
  });
  it("运行中的轮次随任务阶段更新，暂停停止保留，旧统计不伪装成本轮进度", async () => {
    const { d, batches } = setup();
    const current = {
      ...job(),
      request: {
        ...job().request,
        kind: "duplicates",
        duplicateOperationId: null,
        duplicateOptions: {
          sensitivity: "standard",
          verificationMode: "cascade",
          forceRecompute: false,
          regroupOnly: false,
        },
      },
      stage: "第二轮：收紧内容差异",
      processed: 25,
      total: 100,
    };
    const controlJob = vi.fn();
    const wrapper = mount(DuplicatePage, {
      props: {
        workspace: d,
        batches,
        jobs: [current],
        folders: [],
        refreshVersion: 0,
        taskError: "",
        startJob: vi.fn(),
        controlJob,
      },
      global: { stubs: { SoftSelect: true } },
    });
    await flushPromises();
    expect(wrapper.get('[data-round="1"]').classes()).toContain("is-complete");
    expect(wrapper.get('[data-round="2"]').classes()).toContain("is-active");
    expect(wrapper.get('[data-round="3"]').text()).toContain("待开始");
    expect(wrapper.get(".workflow-rounds").text()).not.toContain("1,500");
    const pause = wrapper
      .findAll(".workflow-progress-actions button")
      .find((b) => b.text() === "暂停")!;
    await pause.trigger("click");
    expect(controlJob).toHaveBeenCalledWith("job", "pause");
    await wrapper.setProps({
      jobs: [{ ...current, stage: "第三轮：严格确认相同", state: "paused" }],
    });
    expect(wrapper.get('[data-round="2"]').classes()).toContain("is-complete");
    expect(wrapper.get('[data-round="3"]').text()).toContain("已暂停");
    await wrapper
      .findAll(".workflow-progress-actions button")
      .find((b) => b.text() === "停止检索")!
      .trigger("click");
    expect(controlJob).toHaveBeenCalledWith("job", "cancel");
    expect(api.duplicateWorkflow).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });
  it("无可整理完全相同项时只禁用该类别，不影响近似分类或保留类查看", async () => {
    const { d, batches } = setup();
    const healthy = await api.duplicateWorkflow();
    vi.mocked(api.duplicateWorkflow).mockResolvedValue({
      ...healthy,
      categories: healthy.categories.map((row) =>
        row.kind === "exact" ? { ...row, cleanupCount: 0 } : row,
      ),
    });
    const wrapper = mount(DuplicatePage, {
      props: {
        workspace: d,
        batches,
        jobs: [],
        folders: [],
        refreshVersion: 0,
        taskError: "",
        startJob: vi.fn(),
        controlJob: vi.fn(),
      },
      global: { stubs: { SoftSelect: true } },
    });
    await flushPromises();
    expect(
      wrapper.get('[data-cleanup-kind="exact"]').attributes("disabled"),
    ).toBeDefined();
    expect(
      wrapper.get('[data-cleanup-kind="near"]').attributes("disabled"),
    ).toBeUndefined();
    expect(
      wrapper.get('[data-review-kind="similar"]').attributes("disabled"),
    ).toBeUndefined();
    expect(
      wrapper.get('[data-review-kind="uncertain"]').attributes("disabled"),
    ).toBeUndefined();
    wrapper.unmount();
  });
  it("扫描进度只刷新任务行，终态才读取全库分类；复检停止提示保留结果", async () => {
    const { d, batches } = setup();
    const scan = {
      ...job(),
      request: {
        ...job().request,
        kind: "duplicates",
        duplicateOperationId: null,
        duplicateOptions: {
          sensitivity: "standard",
          verificationMode: "refine",
          forceRecompute: false,
          regroupOnly: true,
        },
      },
    };
    const wrapper = mount(DuplicatePage, {
      props: {
        workspace: d,
        batches,
        jobs: [scan],
        folders: [],
        refreshVersion: 0,
        taskError: "",
        startJob: vi.fn(),
        controlJob: vi.fn(),
      },
      global: { stubs: { SoftSelect: true } },
    });
    await flushPromises();
    const reads = vi.mocked(api.duplicateWorkflow).mock.calls.length;
    expect(reads).toBe(1);
    for (let progress = 1; progress <= 6; progress++) {
      batches.version.value++;
      await wrapper.setProps({
        refreshVersion: progress,
        jobs: [{ ...scan, processed: progress, total: 100 }],
      });
      await flushPromises();
    }
    expect(api.duplicateWorkflow).toHaveBeenCalledTimes(reads);
    await wrapper.setProps({
      refreshVersion: 7,
      jobs: [{ ...scan, state: "cancelled", errors: ["人工停止复检"] }],
    });
    await flushPromises();
    expect(api.duplicateWorkflow).toHaveBeenCalledTimes(reads + 1);
    expect(wrapper.text()).toContain("上次近似图片复检已取消");
    expect(wrapper.text()).toContain("人工停止复检");
    wrapper.unmount();
  });
  it("准备阶段显示已有结果和真实阶段进度，回收数量独立", async () => {
    const batches = useDuplicateBatches();
    batches.register(preview(), job());
    batches.observeStatus({
      ...operationStatus(),
      stageProcessed: 1200,
      stageTotal: 1_000_000,
    });
    const wrapper = mount(DuplicateOperationPanel, {
      props: { operation: batches.latest.value!, job: job(), busy: true },
    });
    expect(wrapper.text()).toContain("读取已有结果");
    expect(wrapper.text()).toContain("1,200 / 1,000,000");
    expect(wrapper.text()).toContain("沿用已有检索结果");
    expect(wrapper.text()).not.toContain("0 / 0");
    batches.observeStatus({
      ...operationStatus(),
      stage: "planning",
      stageProcessed: 7296,
      stageTotal: 55276,
    });
    await flushPromises();
    expect(wrapper.text()).toContain("选择保留图片");
    expect(wrapper.text()).toContain("7,296 / 55,276");
    expect(wrapper.text()).not.toContain("已移入应用回收站");
    batches.observeStatus({
      ...operationStatus(),
      stage: "applying",
      stageProcessed: 800,
      stageTotal: 1200,
      processed: 800,
      total: 1200,
      succeeded: 800,
    });
    await wrapper.setProps({
      job: { ...job(), processed: 800, total: 1200, stage: "移入应用回收站" },
    });
    expect(wrapper.text()).toContain("800 / 1,200");
    expect(wrapper.text()).toContain("已移入应用回收站");
    expect(wrapper.find("progress").attributes("value")).toBe("800");
    const controls = wrapper.findAll("button").map((node) => node.text());
    expect(controls).toContain("暂停");
    expect(controls).toContain("停止本次整理");
    expect(controls).not.toContain("撤销整个批次");
    wrapper.unmount();
  });
  it("状态事件直接更新阶段进度，不为每次进度发查询", async () => {
    const batches = useDuplicateBatches();
    batches.register(preview(), job());
    await batches.connect();
    listeners.get("duplicate-management:changed")?.({
      payload: {
        ...operationStatus(),
        stage: "planning",
        stageProcessed: 7296,
        stageTotal: 55276,
      },
    });
    expect(batches.latest.value?.status?.stageProcessed).toBe(7296);
    expect(api.duplicateOperationStatus).not.toHaveBeenCalled();
    batches.dispose();
    expect(listeners.size).toBe(0);
  });
  it("历史prepared状态使用中文，未知目标数量不会显示0项", async () => {
    const batches = useDuplicateBatches();
    batches.register(preview(), job("prepared"));
    batches.observeStatus(operationStatus("prepared"));
    const wrapper = mount(DuplicateOperationPanel, {
      props: { operation: batches.latest.value!, busy: false },
    });
    expect(wrapper.text()).toContain("准备中");
    expect(wrapper.text()).not.toMatch(/prepared|0 项|0 \/ 0/);
    wrapper.unmount();
  });
  it("查看历史批次不会替换主区域的当前任务", async () => {
    const { d, batches } = setup();
    batches.register(
      preview("all-near", "current"),
      job("running", "current-job", "current"),
    );
    batches.register(
      preview("all-exact", "old"),
      job("completed", "old-job", "old"),
      false,
    );
    batches.activeOperationId.value = "old";
    vi.mocked(api.duplicateOperationStatus).mockImplementation(async (id) => ({
      ...operationStatus(id === "old" ? "completed" : "running", id),
      groupId: id === "old" ? "all-exact" : "all-near",
    }));
    const wrapper = mount(DuplicatePage, {
      props: {
        workspace: d,
        batches,
        jobs: [
          job("running", "current-job", "current"),
          job("completed", "old-job", "old"),
        ],
        folders: [],
        refreshVersion: 0,
        taskError: "",
        startJob: vi.fn(),
        controlJob: vi.fn(),
      },
      global: { stubs: { SoftSelect: true } },
    });
    await flushPromises();
    await wrapper.find(".workflow-heading > button").trigger("click");
    await wrapper.find(".workflow-history > summary").trigger("click");
    const main = wrapper.findAll(":scope > .duplicate-operation-panel");
    expect(main).toHaveLength(1);
    expect(main[0].text()).toContain("整理近似图片");
    expect(
      wrapper.find(".workflow-history .duplicate-operation-panel").text(),
    ).toContain("整理完全相同图片");
    wrapper.unmount();
  });
});

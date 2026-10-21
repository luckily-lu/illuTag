// @vitest-environment jsdom
import { defineComponent } from "vue";
import { mount, flushPromises, config } from "@vue/test-utils";
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import type { JobRecord } from "../../shared/contracts/JobRecord";
import { api } from "../../shared/ipc";
import { useJobs } from "./useJobs";
import TaskCenter from "./TaskCenter.vue";
import { durationLabel } from "./jobPresentation";
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => false }));
vi.mock("../../shared/ipc", () => ({
  api: {
    jobs: vi.fn(async () => []),
    jobHistory: vi.fn(),
    retryJob: vi.fn(),
    clearJobHistory: vi.fn(),
    controlJob: vi.fn(),
  },
  errorMessage: String,
}));
function job(state = "running", revision = 1): JobRecord {
  return {
    id: "job",
    request: {
      kind: "scan",
      thumbnails: false,
      sourceFolderId: 7,
      duplicateOptions: null,
      duplicateOperationId: null,
    },
    state,
    stage: "scan",
    processed: 10,
    total: 100,
    failures: 0,
    errors: [],
    updatedAt: revision,
    meta: {
      revision,
      createdAt: 1,
      startedAt: 2,
      stageCode: "scan",
      unit: "images",
      canPause: state === "running",
      canResume: state === "paused",
      canCancel: ["running", "paused", "pausing", "queued"].includes(state),
      canRetry: ["failed", "interrupted"].includes(state),
    },
  };
}
it("旧终态没有起止时间时不累计到现在，新任务按记录的结束时间计时", () => {
  const legacy = job("completed");
  delete legacy.meta;
  expect(durationLabel(legacy, 90_000_000)).toBe("未记录");
  const finished = job("completed");
  finished.meta!.startedAt = 1000;
  finished.meta!.finishedAt = 62000;
  expect(durationLabel(finished, 90_000_000)).toBe("1分1秒");
  delete finished.meta!.finishedAt;
  expect(durationLabel(finished, 90_000_000)).toBe("未记录");
});
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(api.jobs).mockResolvedValue([]);
  vi.mocked(api.jobHistory).mockResolvedValue({ rows: [] });
  config.global.stubs.Teleport = true;
});
afterEach(() => {
  delete config.global.stubs.Teleport;
});
it("旧进度与迟到列表不能盖住新状态，重试只传任务ID", async () => {
  let state!: ReturnType<typeof useJobs>;
  const wrapper = mount(
    defineComponent({
      setup() {
        state = useJobs(() => {});
        return () => null;
      },
    }),
  );
  await flushPromises();
  let finish!: (v: JobRecord[]) => void;
  vi.mocked(api.jobs).mockImplementationOnce(
    () =>
      new Promise((r) => {
        finish = r;
      }),
  );
  const refresh = state.refresh();
  state.merge([job("paused", 8)]);
  finish([job("running", 3)]);
  await refresh;
  expect(state.rows.value[0].state).toBe("paused");
  state.merge([job("completed", 9)]);
  state.merge([job("running", 10)]);
  expect(state.rows.value[0].state).toBe("completed");
  vi.mocked(api.retryJob).mockResolvedValue({
    ...job("queued", 1),
    id: "retry",
  });
  await state.retry("old-source-task");
  expect(api.retryJob).toHaveBeenCalledWith("old-source-task");
  expect(
    state.rows.value.find((j) => j.id === "retry")?.request.sourceFolderId,
  ).toBe(7);
  let oldSnapshot!: (v: JobRecord[]) => void;
  vi.mocked(api.jobs).mockImplementationOnce(
    () =>
      new Promise((r) => {
        oldSnapshot = r;
      }),
  );
  const stale = state.refresh();
  state.forgetHistory();
  oldSnapshot([job("completed", 9)]);
  await stale;
  expect(state.rows.value.some((j) => j.id === "job")).toBe(false);
  wrapper.unmount();
});
it("任务中心显示暂停确认与等待原因，按钮采用后端能力", async () => {
  const row = job("pausing");
  row.meta!.canPause = false;
  const wrapper = mount(TaskCenter, {
    props: {
      rows: [row, { ...job("queued"), id: "queued" }],
      error: "",
      thumbnails: true,
    },
  });
  await flushPromises();
  expect(wrapper.get(".task-current").text()).toContain("正在暂停");
  expect(wrapper.findAll("button").some((b) => b.text() === "暂停")).toBe(
    false,
  );
  await wrapper.setProps({
    rows: [job("paused"), { ...job("queued"), id: "queued" }],
  });
  expect(wrapper.get(".task-current").text()).toContain("等待队列也会等待");
  expect(wrapper.get(".task-queue").text()).toContain("等待当前任务继续");
  const unsaved = job("running", 12);
  unsaved.meta!.persistenceError = "database is locked";
  unsaved.meta!.canPause = false;
  unsaved.meta!.canCancel = false;
  unsaved.meta!.canRetry = true;
  await wrapper.setProps({ rows: [unsaved] });
  expect(wrapper.get(".task-current").text()).toContain("结果待保存");
  await wrapper
    .findAll("button")
    .find((b) => b.text() === "重试保存")!
    .trigger("click");
  expect(wrapper.emitted("control")).toEqual([["job", "retrySave"]]);
  wrapper.unmount();
});
it("历史读取期间到来的终态会补取一次，避免遗漏刚完成的任务", async () => {
  let finish!: (v: { rows: JobRecord[] }) => void;
  vi.mocked(api.jobHistory).mockImplementationOnce(
    () =>
      new Promise((r) => {
        finish = r;
      }),
  );
  const wrapper = mount(TaskCenter, {
    props: { rows: [job("running")], error: "", thumbnails: true },
  });
  await wrapper.setProps({ rows: [job("completed", 2)] });
  vi.mocked(api.jobHistory).mockResolvedValueOnce({
    rows: [job("completed", 2)],
  });
  finish({ rows: [] });
  await flushPromises();
  expect(api.jobHistory).toHaveBeenCalledTimes(2);
  expect(wrapper.get(".task-history-row").text()).toContain("已完成");
  wrapper.unmount();
});
it("历史只按后端游标续页，改变筛选会重新从首屏读取", async () => {
  vi.mocked(api.jobHistory)
    .mockResolvedValueOnce({
      rows: [job("failed")],
      nextCursor: "opaque-first",
    })
    .mockResolvedValueOnce({ rows: [{ ...job("completed"), id: "old" }] })
    .mockResolvedValueOnce({ rows: [] });
  const wrapper = mount(TaskCenter, {
    props: { rows: [], error: "", thumbnails: true },
  });
  await flushPromises();
  await wrapper.get(".task-history-more").trigger("click");
  await flushPromises();
  expect(api.jobHistory).toHaveBeenNthCalledWith(2, {
    state: "all",
    limit: 20,
    cursor: "opaque-first",
  });
  wrapper
    .findComponent({ name: "SoftSelect" })
    .vm.$emit("update:modelValue", "completed");
  await flushPromises();
  expect(api.jobHistory).toHaveBeenLastCalledWith({
    state: "completed",
    limit: 20,
  });
  wrapper.unmount();
});
it("清历史必须确认，普通重试保留后端请求上下文", async () => {
  vi.mocked(api.jobHistory).mockResolvedValue({ rows: [job("failed")] });
  const wrapper = mount(TaskCenter, {
    props: { rows: [], error: "", thumbnails: true },
  });
  await flushPromises();
  await wrapper
    .findAll("button")
    .find((b) => b.text() === "重新运行")!
    .trigger("click");
  expect(wrapper.emitted("retry")).toEqual([["job"]]);
  await wrapper
    .findAll("button")
    .find((b) => b.text() === "清除记录…")!
    .trigger("click");
  await flushPromises();
  expect(api.clearJobHistory).not.toHaveBeenCalled();
  expect(wrapper.get('[role="dialog"]').text()).toContain(
    "整理撤销记录均会保留",
  );
  await wrapper.get('[role="dialog"] .danger').trigger("click");
  await flushPromises();
  expect(api.clearJobHistory).toHaveBeenCalledOnce();
  wrapper.unmount();
});

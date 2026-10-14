// @vitest-environment jsdom
import { defineComponent } from "vue";
import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useJobs } from "../jobs/useJobs";
import { api } from "../../shared/ipc";
import type { JobRecord } from "../../shared/contracts/JobRecord";
const { listeners } = vi.hoisted(() => ({
  listeners: new Map<string, (event: { payload: JobRecord }) => void>(),
}));
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(
    async (name: string, callback: (event: { payload: JobRecord }) => void) => {
      listeners.set(name, callback);
      return () => listeners.delete(name);
    },
  ),
}));
vi.mock("../../shared/ipc", () => ({
  api: {
    jobs: vi.fn(async () => []),
    startJob: vi.fn(async () => ({})),
    controlJob: vi.fn(),
  },
}));
function record(state: string): JobRecord {
  return {
    id: "check",
    request: {
      kind: "duplicates",
      thumbnails: false,
      sourceFolderId: null,
      duplicateOperationId: null,
      duplicateOptions: {
        sensitivity: "standard",
        forceRecompute: false,
        regroupOnly: false,
      },
    },
    state,
    stage: "指纹检查",
    processed: 1,
    total: 100,
    failures: 0,
    errors: [],
    updatedAt: 0,
  };
}
describe("查重任务事件", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    listeners.clear();
  });
  it("进度只更新任务行，终态只刷新一次结果", async () => {
    const changed = vi.fn(),
      terminal = vi.fn();
    const wrapper = mount(
      defineComponent({
        setup() {
          useJobs(changed, terminal);
          return () => null;
        },
      }),
    );
    await flushPromises();
    listeners.get("jobs:changed")?.({ payload: record("running") });
    listeners.get("jobs:changed")?.({
      payload: { ...record("running"), processed: 80 },
    });
    expect(changed).not.toHaveBeenCalled();
    expect(terminal).not.toHaveBeenCalled();
    listeners.get("jobs:changed")?.({ payload: record("partial") });
    listeners.get("jobs:changed")?.({ payload: record("partial") });
    expect(terminal).toHaveBeenCalledTimes(1);
    expect(changed).not.toHaveBeenCalled();
    wrapper.unmount();
    expect(listeners.size).toBe(0);
  });
  it("重新运行携带原查重选项，普通任务保持空选项兼容", async () => {
    let jobs!: ReturnType<typeof useJobs>;
    const wrapper = mount(
      defineComponent({
        setup() {
          jobs = useJobs(() => undefined);
          return () => null;
        },
      }),
    );
    await flushPromises();
    const options = {
      sensitivity: "loose",
      forceRecompute: true,
      regroupOnly: false,
    };
    await jobs.start("duplicates", false, null, options);
    expect(api.startJob).toHaveBeenLastCalledWith({
      kind: "duplicates",
      thumbnails: false,
      sourceFolderId: null,
      duplicateOptions: options,
      duplicateOperationId: null,
    });
    await jobs.start("scan", true, 2);
    expect(api.startJob).toHaveBeenLastCalledWith({
      kind: "scan",
      thumbnails: true,
      sourceFolderId: 2,
      duplicateOptions: null,
      duplicateOperationId: null,
    });
    wrapper.unmount();
  });
});

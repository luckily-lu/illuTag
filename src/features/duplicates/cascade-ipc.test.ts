import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { api } from "../../shared/ipc";

vi.mock("@tauri-apps/api/core", () => ({
  isTauri: () => true,
  invoke: vi.fn(),
  convertFileSrc: (value: string) => value,
}));
beforeEach(() => vi.clearAllMocks());

it("分类整理kind作为IPC顶层参数，旧request和不传kind的调用保持兼容", async () => {
  const request = { generation: "one", revision: "plan-1" };
  await api.prepareDuplicateWorkflow(request, "exact");
  expect(invoke).toHaveBeenLastCalledWith("prepare_duplicate_workflow", {
    request,
    kind: "exact",
  });
  await api.prepareDuplicateWorkflow(request, "near");
  expect(invoke).toHaveBeenLastCalledWith("prepare_duplicate_workflow", {
    request,
    kind: "near",
  });
  await api.prepareDuplicateWorkflow(request);
  expect(invoke).toHaveBeenLastCalledWith("prepare_duplicate_workflow", {
    request,
  });
  expect(request).toEqual({ generation: "one", revision: "plan-1" });
});

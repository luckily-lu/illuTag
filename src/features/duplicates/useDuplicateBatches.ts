import { computed, ref } from "vue";
import { isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { DuplicateActionPreview } from "../../shared/contracts/DuplicateActionPreview";
import type { DuplicateOperationStatus } from "../../shared/contracts/DuplicateOperationStatus";
import type { DuplicateOperationResult } from "../../shared/contracts/DuplicateOperationResult";
import type { JobRecord } from "../../shared/contracts/JobRecord";

export type BatchKind = "exact" | "near";
export type DuplicateBatch = {
  preview: DuplicateActionPreview;
  jobId: string;
  jobState: string;
  status: DuplicateOperationStatus | null;
  items: DuplicateOperationResult[];
  nextCursor: string | null;
  resultsLoaded: boolean;
  loading: boolean;
  error: string;
  handled: boolean;
};

// App owns batch history so progress and undo survive navigation.
export function useDuplicateBatches() {
  const operations = ref(new Map<string, DuplicateBatch>());
  const activeOperationId = ref<string | null>(null);
  const currentOperationId = ref<string | null>(null);
  const scrollTop = ref(0),
    version = ref(0);
  const latest = computed(() =>
    activeOperationId.value
      ? (operations.value.get(activeOperationId.value) ?? null)
      : ([...operations.value.values()].at(-1) ?? null),
  );
  let unlisten: UnlistenFn | undefined;
  let disposed = false;
  function register(
    preview: DuplicateActionPreview,
    job: JobRecord,
    activate = true,
  ) {
    operations.value = new Map(operations.value).set(preview.operationId, {
      preview,
      jobId: job.id,
      jobState: job.state,
      status: null,
      items: [],
      nextCursor: null,
      resultsLoaded: false,
      loading: false,
      error: "",
      handled: false,
    });
    if (activate) {
      activeOperationId.value = preview.operationId;
      currentOperationId.value = preview.operationId;
    }
  }
  function observe(job: JobRecord) {
    const id = job.request.duplicateOperationId;
    if (!id) return;
    const operation = operations.value.get(id);
    if (operation) operation.jobState = job.state;
    version.value++;
  }
  function observeStatus(status: DuplicateOperationStatus) {
    const operation = operations.value.get(status.operationId);
    if (operation) operation.status = status;
  }
  async function connect() {
    if (!isTauri() || unlisten || disposed) return;
    const off = await listen<DuplicateOperationStatus>(
      "duplicate-management:changed",
      (event) => observeStatus(event.payload),
    );
    if (disposed) off();
    else unlisten = off;
  }
  function dispose() {
    disposed = true;
    unlisten?.();
    unlisten = undefined;
  }
  return {
    operations,
    activeOperationId,
    currentOperationId,
    scrollTop,
    version,
    latest,
    register,
    observe,
    observeStatus,
    connect,
    dispose,
  };
}
export type DuplicateBatches = ReturnType<typeof useDuplicateBatches>;

import { computed, ref } from "vue";
import { api, errorMessage } from "../../shared/ipc";
import type { JobRecord } from "../../shared/contracts/JobRecord";
import type {
  BatchKind,
  DuplicateBatch,
  DuplicateBatches,
} from "./useDuplicateBatches";
import type { useDuplicates } from "./useDuplicates";

const terminal = (state: string) =>
  ["completed", "partial", "cancelled", "interrupted", "failed"].includes(
    state,
  );
export function useDuplicateOperations(
  d: ReturnType<typeof useDuplicates>,
  batches: DuplicateBatches,
  callbacks: { jobs: () => JobRecord[]; created: (job: JobRecord) => void },
) {
  const preparing = ref<BatchKind | null>(null),
    submitting = ref(false);
  const error = ref(""),
    notice = ref("");
  const pendingSync = new Set<string>();
  const busy = computed(
    () =>
      !!preparing.value ||
      submitting.value ||
      callbacks
        .jobs()
        .some(
          (job) =>
            ["duplicates", "duplicateManagement", "duplicateUndo"].includes(
              job.request.kind,
            ) &&
            ["queued", "running", "pausing", "paused", "cancelling"].includes(job.state),
        ) ||
      [...batches.operations.value.values()].some((operation) =>
        ["queued", "running", "pausing", "paused", "cancelling"].includes(
          callbacks.jobs().find((job) => job.id === operation.jobId)?.state ??
            operation.status?.state ??
            operation.jobState,
        ),
      ),
  );

  function flushSync(operation: DuplicateBatch) {
    if (pendingSync.delete(operation.preview.operationId))
      queueMicrotask(() => void sync(operation));
  }
  async function startAll(kind: BatchKind) {
    const generation = d.status.value?.generation;
    if (!generation || busy.value) return;
    preparing.value = kind;
    error.value = "";
    notice.value = "";
    try {
      // Scope is the published generation, independent of filters or loaded members.
      const preview = await api.prepareDuplicateAction({
        generation,
        groupId: kind === "exact" ? "all-exact" : "all-near",
        mode: kind === "exact" ? "autoExactAll" : "autoNearAll",
        keepIds: [],
        protectedIds: [],
        imageIds: [],
        action: "trash",
        sourceOperationId: null,
      });
      const job = await api.applyDuplicateAction({
        operationId: preview.operationId,
        confirmed: true,
      });
      batches.register(preview, job);
      callbacks.created(job);
      preparing.value = null;
      await sync(batches.operations.value.get(preview.operationId)!);
    } catch (e) {
      error.value = errorMessage(e);
    } finally {
      preparing.value = null;
    }
  }
  async function results(operation: DuplicateBatch, append = false) {
    if (operation.loading || (append && !operation.nextCursor)) return;
    operation.loading = true;
    operation.error = "";
    try {
      const page = await api.duplicateOperationResults({
        operationId: operation.preview.operationId,
        cursor: append ? operation.nextCursor : null,
        limit: 50,
        state: "failed",
      });
      if (page.operationId !== operation.preview.operationId) return;
      operation.items = append
        ? [...operation.items, ...page.items]
        : page.items;
      operation.nextCursor = page.nextCursor;
      operation.resultsLoaded = true;
    } catch (e) {
      operation.error = errorMessage(e);
    } finally {
      operation.loading = false;
      flushSync(operation);
    }
  }
  async function sync(operation: DuplicateBatch, force = false) {
    if (operation.loading) {
      pendingSync.add(operation.preview.operationId);
      return;
    }
    operation.loading = true;
    operation.error = "";
    try {
      const status = await api.duplicateOperationStatus(
        operation.preview.operationId,
      );
      operation.status = status;
      operation.jobState =
        callbacks.jobs().find((job) => job.id === operation.jobId)?.state ??
        status.state;
      if (terminal(status.state) && !operation.handled) {
        // Reload only the first inspection page. Never enumerate batch members in the UI.
        await d.refresh();
        if (d.status.value?.generation) await d.loadGroups();
        operation.handled = true;
      }
    } catch (e) {
      operation.error = errorMessage(e);
    } finally {
      operation.loading = false;
      flushSync(operation);
    }
    if (
      operation.status &&
      terminal(operation.status.state) &&
      (!operation.resultsLoaded || force)
    )
      await results(operation);
  }
  async function hydrate(job: JobRecord) {
    const id = job.request.duplicateOperationId;
    if (!id || batches.operations.value.has(id)) return;
    try {
      const status = await api.duplicateOperationStatus(id);
      batches.register(
        {
          operationId: id,
          generation: status.generation,
          groupId: status.groupId,
          action: status.action,
          targetCount: status.total,
          totalBytes: 0,
          protectedCount: 0,
          skippedCount: status.skipped,
          metadata: {
            favoriteCount: 0,
            classifiedCount: 0,
            customTaggedCount: 0,
            supplementTaggedCount: 0,
          },
          sample: [],
          warnings: [],
          expiresAt: 0,
          requiresSystemConfirmation: status.action === "systemTrash",
        },
        job,
        false,
      );
      const operation = batches.operations.value.get(id)!;
      operation.status = status;
      // Historical batches do not each need another inspection-page refresh.
      operation.handled = terminal(status.state);
      await sync(operation);
    } catch (e) {
      const message = errorMessage(e);
      if (
        batches.activeOperationId.value ||
        !/批次不存在|没有查重处理记录/.test(message)
      )
        error.value = message;
    }
  }
  async function syncAll() {
    const selectedId = batches.activeOperationId.value;
    const operationJobs = callbacks
      .jobs()
      .filter((job) => job.request.duplicateOperationId);
    const currentJob =
      operationJobs.find((job) =>
        ["queued", "running", "pausing", "paused", "cancelling"].includes(job.state),
      ) ?? operationJobs[0];
    const selectedJob = operationJobs.find(
      (job) => job.request.duplicateOperationId === selectedId,
    );
    for (const job of new Set([currentJob, selectedJob])) {
      if (
        job &&
        !batches.operations.value.has(job.request.duplicateOperationId!)
      )
        await hydrate(job);
    }
    if (!batches.currentOperationId.value && currentJob)
      batches.currentOperationId.value =
        currentJob.request.duplicateOperationId;
    for (const operation of batches.operations.value.values())
      if (!operation.handled || operation === batches.latest.value)
        await sync(operation);
  }
  async function undo(operation: DuplicateBatch) {
    if (busy.value) return;
    submitting.value = true;
    error.value = "";
    try {
      const job = await api.undoDuplicateAction(operation.preview.operationId);
      const id = job.request.duplicateOperationId;
      if (!id) throw Error("撤销任务没有返回操作标识");
      batches.register(
        { ...operation.preview, operationId: id, action: "restore" },
        job,
      );
      callbacks.created(job);
      await sync(batches.operations.value.get(id)!);
      operation.status = await api.duplicateOperationStatus(
        operation.preview.operationId,
      );
    } catch (e) {
      error.value = errorMessage(e);
    } finally {
      submitting.value = false;
    }
  }
  async function retry(operation: DuplicateBatch) {
    if (busy.value) return;
    submitting.value = true;
    error.value = "";
    try {
      // Resume the saved batch so random keep choices and undo ownership are preserved.
      const job = await api.applyDuplicateAction({
        operationId: operation.preview.operationId,
        confirmed: true,
      });
      batches.register(operation.preview, job);
      callbacks.created(job);
      await sync(batches.operations.value.get(operation.preview.operationId)!);
    } catch (e) {
      error.value = errorMessage(e);
    } finally {
      submitting.value = false;
    }
  }
  return {
    preparing,
    submitting,
    error,
    notice,
    busy,
    startAll,
    results,
    sync,
    syncAll,
    undo,
    retry,
  };
}

import { computed, ref } from "vue";
import { api, errorMessage } from "../../shared/ipc";
import type { DuplicateGroup } from "../../shared/contracts/DuplicateGroup";
import type { DuplicateManagementRequest } from "../../shared/contracts/DuplicateManagementRequest";
import type { DuplicateActionPreview } from "../../shared/contracts/DuplicateActionPreview";
import type { JobRecord } from "../../shared/contracts/JobRecord";
import type {
  DuplicateReview,
  ReviewMode,
  ReviewOperation,
} from "./useDuplicateReview";
import type { useDuplicates } from "./useDuplicates";

type Workspace = ReturnType<typeof useDuplicates>;
const terminal = (state: string) =>
  ["completed", "partial", "cancelled", "interrupted", "failed"].includes(
    state,
  );
export function useDuplicateOperations(
  d: Workspace,
  review: DuplicateReview,
  callbacks: {
    jobs: () => JobRecord[];
    created: (job: JobRecord) => void;
    advance: (groupId: string, followingIds: string[]) => Promise<void>;
  },
) {
  const confirmation = ref<{
    preview: DuplicateActionPreview;
    request: DuplicateManagementRequest;
    advance: boolean;
  } | null>(null);
  const preparing = ref<string | null>(null),
    submitting = ref(false),
    error = ref(""),
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
            ["duplicateManagement", "duplicateUndo"].includes(
              job.request.kind,
            ) &&
            ["queued", "running", "paused", "cancelling"].includes(job.state),
        ) ||
      [...review.operations.value.values()].some(
        (operation) => operation.loading && !operation.handled,
      ),
  );
  function flushSync(operation: ReviewOperation) {
    if (!pendingSync.delete(operation.preview.operationId)) return;
    queueMicrotask(() => void sync(operation));
  }
  async function prepareRequest(
    request: DuplicateManagementRequest,
    advance: boolean,
  ) {
    if (busy.value) return;
    preparing.value = request.groupId;
    error.value = "";
    try {
      const preview = await api.prepareDuplicateAction(request);
      confirmation.value = { preview, request, advance };
    } catch (e) {
      error.value = errorMessage(e);
    } finally {
      preparing.value = null;
    }
  }
  async function prepare(
    group: DuplicateGroup,
    mode: ReviewMode,
    advance = true,
  ) {
    const generation = d.status.value?.generation;
    if (!generation || busy.value) return;
    review.activeGroupId.value = group.id;
    const members = [group.reference, ...group.members];
    const keepIds = [
      ...new Set(
        members
          .filter((member) => review.kept.value.has(member.image.id))
          .map((member) => member.image.id),
      ),
    ];
    if (mode === "exactOthers" && !keepIds.length) {
      error.value = "先明确保留至少一张图片，再处理本组其他成员。";
      return;
    }
    const imageIds = [...review.selection(group.id)].filter(
      (id) => !review.kept.value.has(id),
    );
    if (mode === "selected" && !imageIds.length) {
      error.value = "请先勾选已经确认需要处理的候选。";
      return;
    }
    await prepareRequest(
      {
        generation,
        groupId: group.id,
        mode,
        keepIds,
        protectedIds: [...review.kept.value],
        imageIds,
        action: "trash",
        sourceOperationId: null,
      },
      advance,
    );
  }
  async function system(operation: ReviewOperation) {
    await prepareRequest(
      {
        generation: operation.preview.generation,
        groupId: operation.preview.groupId,
        mode: "selected",
        keepIds: [],
        protectedIds: [...review.kept.value],
        imageIds: [],
        action: "systemTrash",
        sourceOperationId: operation.preview.operationId,
      },
      false,
    );
  }
  async function autoExact(groupId = "all-exact") {
    const generation = d.status.value?.generation;
    if (!generation || busy.value) return;
    await prepareRequest(
      {
        generation,
        groupId,
        mode: "autoExact",
        keepIds: [],
        protectedIds: [...review.kept.value],
        imageIds: [],
        action: "trash",
        sourceOperationId: null,
      },
      false,
    );
  }
  async function retryConfirmation() {
    const previous = confirmation.value;
    if (!previous) return;
    confirmation.value = null;
    await prepareRequest(previous.request, previous.advance);
  }
  async function apply(advance: boolean) {
    const pending = confirmation.value;
    if (!pending || busy.value) return;
    submitting.value = true;
    error.value = "";
    try {
      const job = await api.applyDuplicateAction({
        operationId: pending.preview.operationId,
        confirmed: true,
      });
      const group = d.groups.value.find(
        (value) => value.id === pending.preview.groupId,
      );
      const index = d.groups.value.findIndex(
        (value) => value.id === pending.preview.groupId,
      );
      const trackedIds = [
        ...new Set([
          ...[...review.selections.value.values()].flatMap((set) => [...set]),
          ...(group
            ? [group.reference, ...group.members].map(
                (member) => member.image.id,
              )
            : []),
          ...pending.preview.sample.map((member) => member.image.id),
        ]),
      ];
      review.register(
        pending.preview,
        pending.request.mode as ReviewMode,
        advance,
        job,
        pending.request.sourceOperationId,
        d.groups.value.slice(index + 1).map((value) => value.id),
        trackedIds,
      );
      confirmation.value = null;
      callbacks.created(job);
      notice.value = "处理任务已提交，切换页面后仍会继续运行。";
      await sync(review.operations.value.get(pending.preview.operationId)!);
    } catch (e) {
      error.value = errorMessage(e);
    } finally {
      submitting.value = false;
    }
  }
  async function results(operation: ReviewOperation, append = false) {
    if (operation.loading || (append && !operation.nextCursor)) return;
    operation.loading = true;
    operation.error = "";
    try {
      const query = {
        operationId: operation.preview.operationId,
        cursor: append ? operation.nextCursor : null,
        limit: 50,
        state: "failed",
        imageIds: null,
      };
      const page = await api.duplicateOperationResults(query);
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
  async function knownResults(operation: ReviewOperation) {
    // Bound each response and retain only already-known image IDs, never a full result snapshot.
    for (let at = 0; at < operation.trackedIds.length; at += 256) {
      const wanted = operation.trackedIds.slice(at, at + 256);
      let cursor: string | null = null;
      do {
        const query = {
          operationId: operation.preview.operationId,
          cursor,
          limit: 50,
          state: "all",
          imageIds: wanted,
        };
        const page = await api.duplicateOperationResults(query);
        const successes = page.items
          .filter(
            (item) =>
              item.state === "succeeded" &&
              item.imageId &&
              wanted.includes(item.imageId),
          )
          .map((item) => item.imageId!);
        review.done(successes, operation.preview.action === "restore");
        cursor = page.nextCursor;
      } while (cursor);
    }
  }
  async function sync(operation: ReviewOperation, force = false) {
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
      if (!terminal(status.state) || (operation.handled && !force)) return;
      if (!operation.handled) {
        await knownResults(operation);
        await d.reaudit(
          status.affectedGroupIds,
          status.affectedGroupsTruncated,
        );
        review.reconcileActive(
          d.groups.value.flatMap((group) => [
            group.reference,
            ...group.members,
          ]),
        );
        if (status.action === "restore") review.restoreGroup(operation);
        else if (
          status.state === "completed" &&
          status.failed === 0 &&
          status.skipped === 0
        )
          review.decide(status.groupId, "handled");
        for (const group of d.groups.value.filter((value) => value.needsReview))
          review.clearSelection(group.id);
        operation.handled = true;
        if (
          operation.advance &&
          status.action !== "restore" &&
          status.state === "completed" &&
          status.failed === 0 &&
          status.skipped === 0
        ) {
          await callbacks.advance(status.groupId, operation.followingGroupIds);
        }
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
    if (!id || review.operations.value.has(id)) return;
    try {
      const status = await api.duplicateOperationStatus(id);
      review.register(
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
        "selected",
        false,
        job,
      );
      const operation = review.operations.value.get(id)!;
      operation.status = status;
      await sync(operation);
    } catch (e) {
      const message = errorMessage(e);
      if (
        review.activeOperationId.value ||
        !/批次不存在|没有查重处理记录/.test(message)
      )
        error.value = message;
    }
  }
  async function syncAll() {
    const selectedId = review.activeOperationId.value;
    const knownJob = callbacks
      .jobs()
      .find(
        (job) =>
          job.request.duplicateOperationId &&
          (!selectedId || job.request.duplicateOperationId === selectedId),
      );
    if (
      knownJob &&
      !review.operations.value.has(knownJob.request.duplicateOperationId!)
    )
      await hydrate(knownJob);
    for (const operation of review.operations.value.values()) {
      if (!operation.handled || operation === review.latest.value)
        await sync(operation);
    }
  }
  async function undo(operation: ReviewOperation) {
    if (busy.value) return;
    submitting.value = true;
    error.value = "";
    try {
      const job = await api.undoDuplicateAction(operation.preview.operationId);
      const id = job.request.duplicateOperationId;
      if (!id) throw Error("撤销任务没有返回操作标识");
      review.register(
        { ...operation.preview, operationId: id, action: "restore" },
        "selected",
        false,
        job,
        operation.preview.operationId,
        [],
        operation.trackedIds,
      );
      callbacks.created(job);
      await sync(review.operations.value.get(id)!);
      operation.status = await api.duplicateOperationStatus(
        operation.preview.operationId,
      );
    } catch (e) {
      error.value = errorMessage(e);
    } finally {
      submitting.value = false;
    }
  }
  async function retry(operation: ReviewOperation) {
    if (operation.mode === "autoExact") {
      await autoExact(operation.preview.groupId);
      return;
    }
    if (operation.preview.action === "systemTrash") {
      const source = operation.sourceOperationId;
      if (!source) {
        error.value = "请回到本次软回收记录，重新准备原文件清理。";
        return;
      }
      await prepareRequest(
        {
          generation: operation.preview.generation,
          groupId: operation.preview.groupId,
          mode: "selected",
          keepIds: [],
          protectedIds: [...review.kept.value],
          imageIds: [],
          action: "systemTrash",
          sourceOperationId: source,
        },
        false,
      );
      return;
    }
    const group =
      d.groups.value.find((value) => value.id === operation.preview.groupId) ??
      (await api.duplicateGroup({
        generation: operation.preview.generation,
        groupId: operation.preview.groupId,
      }));
    if (!group) {
      error.value = "本组已无可处理的有效成员，请查看失败记录或更新检索结果。";
      return;
    }
    await prepare(group, operation.mode, false);
  }
  return {
    confirmation,
    preparing,
    submitting,
    error,
    notice,
    busy,
    prepare,
    prepareRequest,
    autoExact,
    system,
    retryConfirmation,
    apply,
    results,
    sync,
    syncAll,
    undo,
    retry,
  };
}

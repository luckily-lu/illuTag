import { ref } from "vue";
import { api, errorMessage } from "../../shared/ipc";
import type { GalleryQuery } from "../../shared/contracts/GalleryQuery";

export function useRecycleBin() {
  const selectionId = ref<string | null>(null);
  const pendingId = ref<string | null>(null);
  const count = ref(0),
    totalBytes = ref(0),
    busy = ref(false),
    error = ref("");
  const confirm = ref<null | {
    action: "restore" | "systemTrash";
    title: string;
    scope: string;
  }>(null);
  async function snapshot(
    query: GalleryQuery | null,
    imageIds: string[] = [],
    baseId: string | null = null,
    excludedIds: string[] = [],
  ) {
    busy.value = true;
    try {
      const result = await api.prepareRecycleSelection({
        query,
        imageIds,
        selectionId: baseId,
        excludedIds,
      });
      pendingId.value = result.selectionId;
      return result;
    } catch (e) {
      error.value = errorMessage(e);
      throw e;
    } finally {
      busy.value = false;
    }
  }
  async function selectAll(query: GalleryQuery) {
    const result = await snapshot({ ...query, offset: 0 });
    selectionId.value = result.selectionId;
    count.value = result.count;
    totalBytes.value = result.totalBytes;
    return result;
  }
  async function apply(
    action: "restore" | "systemTrash",
    query: GalleryQuery,
    ids: string[],
    allId: string | null,
    excludedIds: string[] = [],
    currentResults = false,
  ) {
    const result = allId
      ? await snapshot(null, [], allId, excludedIds)
      : currentResults
        ? await snapshot({ ...query, offset: 0 })
        : await snapshot(null, ids);
    if (!result.count) return;
    confirm.value = {
      action,
      title: action === "restore" ? "还原图片" : "移入 Windows 回收站",
      scope: `共 ${result.count.toLocaleString()} 张 · ${(result.totalBytes / 1048576).toFixed(1)} MB`,
    };
  }
  async function empty(query: GalleryQuery) {
    const result = await snapshot({
      ...query,
      scope: "trash",
      tags: [],
      englishQuery: "",
      fileName: "",
      naturalLanguage: "",
      folderId: null,
      unclassifiedOnlyParentFolderId: null,
      offset: 0,
    });
    confirm.value = {
      action: "systemTrash",
      title: "清空应用回收站",
      scope: `全部回收项：${result.count.toLocaleString()} 张 · ${(result.totalBytes / 1048576).toFixed(1)} MB`,
    };
  }
  async function resume(
    operationId: string,
    action: "restore" | "systemTrash",
  ) {
    busy.value = true;
    error.value = "";
    try {
      const status = await api.recycleOperationStatus(operationId);
      if (status.action !== action) throw new Error("回收批次操作类型不匹配");
      const remaining = Math.max(
        0,
        status.total - status.succeeded - status.skipped,
      );
      if (!remaining) throw new Error("该回收批次没有待继续项目");
      pendingId.value = operationId;
      count.value = remaining;
      totalBytes.value = status.totalBytes;
      confirm.value = {
        action,
        title:
          action === "restore" ? "继续还原回收批次" : "继续移入 Windows 回收站",
        scope: `原批次 ${status.total.toLocaleString()} 张 · ${(status.totalBytes / 1048576).toFixed(1)} MB；已完成 ${status.succeeded.toLocaleString()}，已跳过 ${status.skipped.toLocaleString()}，待继续 ${remaining.toLocaleString()} 张。`,
      };
    } catch (e) {
      error.value = errorMessage(e);
      throw e;
    } finally {
      busy.value = false;
    }
  }
  async function confirmApply() {
    const target = confirm.value;
    if (!target || !pendingId.value) return;
    busy.value = true;
    try {
      await api.applyRecycleAction({
        selectionId: pendingId.value,
        action: target.action,
        confirmed: target.action === "systemTrash",
      });
      confirm.value = null;
      pendingId.value = null;
    } catch (e) {
      error.value = errorMessage(e);
    } finally {
      busy.value = false;
    }
  }
  return {
    selectionId,
    count,
    totalBytes,
    busy,
    error,
    confirm,
    snapshot,
    selectAll,
    apply,
    empty,
    resume,
    confirmApply,
  };
}

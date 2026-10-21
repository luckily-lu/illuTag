import { computed, onMounted, onUnmounted, ref } from "vue";
import { isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { api, errorMessage } from "../../shared/ipc";
import type { JobRecord } from "../../shared/contracts/JobRecord";
import type { DuplicateOptions } from "../../shared/contracts/DuplicateOptions";
import { acceptsJob, isActive } from "./jobPresentation";

export function useJobs(
  changed: () => void,
  terminal?: (job: JobRecord) => void,
) {
  const rows = ref<JobRecord[]>([]),
    error = ref("");
  const controlling = ref(new Set<string>());
  let off: UnlistenFn | undefined,
    offLibrary: UnlistenFn | undefined,
    disposed = false,
    refreshEpoch = 0;
  const active = computed(() => rows.value.filter(isActive));
  function merge(incoming: JobRecord[], notify = false) {
    if (disposed) return;
    const map = new Map(rows.value.map((j) => [j.id, j]));
    for (const next of incoming) {
      const previous = map.get(next.id);
      if (!acceptsJob(previous, next)) continue;
      map.set(next.id, next);
      if (notify && previous?.state !== next.state && !isActive(next))
        terminal?.(next);
    }
    const all = [...map.values()].sort(
      (a, b) => b.updatedAt - a.updatedAt || b.id.localeCompare(a.id),
    );
    rows.value = [
      ...all.filter(isActive),
      ...all.filter((j) => !isActive(j)).slice(0, 100),
    ];
  }
  async function refresh() {
    const token = ++refreshEpoch;
    const next = await api.jobs();
    if (token === refreshEpoch) merge(next);
  }
  async function start(
    kind: string,
    thumbnails = true,
    sourceFolderId: number | null = null,
    duplicateOptions: DuplicateOptions | null = null,
    duplicateOperationId: string | null = null,
  ) {
    try {
      error.value = "";
      const record = await api.startJob({
        kind,
        thumbnails,
        sourceFolderId,
        duplicateOptions,
        duplicateOperationId,
      });
      merge([record]);
      await refresh().catch((e) => (error.value = errorMessage(e)));
      return true;
    } catch (e) {
      error.value = errorMessage(e);
      return false;
    }
  }
  async function control(id: string, action: string) {
    if (controlling.value.has(id)) return;
    controlling.value = new Set(controlling.value).add(id);
    error.value = "";
    try {
      await api.controlJob(id, action);
      await refresh();
    } catch (e) {
      error.value = errorMessage(e);
    } finally {
      const next = new Set(controlling.value);
      next.delete(id);
      controlling.value = next;
    }
  }
  async function retry(id: string) {
    if (controlling.value.has(id)) return;
    controlling.value = new Set(controlling.value).add(id);
    error.value = "";
    try {
      merge([await api.retryJob(id)]);
      await refresh();
    } catch (e) {
      error.value = errorMessage(e);
    } finally {
      const next = new Set(controlling.value);
      next.delete(id);
      controlling.value = next;
    }
  }
  onMounted(async () => {
    if (isTauri()) {
      off = await listen<JobRecord>("jobs:changed", (e) =>
        merge([e.payload], true),
      );
      offLibrary = await listen("library:changed", changed);
    }
    if (disposed) {
      off?.();
      offLibrary?.();
      return;
    }
    await refresh().catch((e) => (error.value = errorMessage(e)));
  });
  onUnmounted(() => {
    disposed = true;
    ++refreshEpoch;
    off?.();
    offLibrary?.();
  });
  function forgetHistory() {
    ++refreshEpoch;
    rows.value = rows.value.filter(isActive);
  }
  return {
    rows,
    error,
    active,
    controlling,
    start,
    control,
    retry,
    refresh,
    merge,
    forgetHistory,
  };
}

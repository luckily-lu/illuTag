import { computed, onMounted, onUnmounted, ref } from "vue";
import { isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { api } from "../../shared/ipc";
import type { JobRecord } from "../../shared/contracts/JobRecord";
import type { DuplicateOptions } from "../../shared/contracts/DuplicateOptions";
export function useJobs(
  changed: () => void,
  terminal?: (job: JobRecord) => void,
) {
  const rows = ref<JobRecord[]>([]),
    error = ref("");
  let off: UnlistenFn | undefined;
  let offLibrary: UnlistenFn | undefined;
  let disposed = false;
  const active = computed(() =>
    rows.value.filter((r) =>
      ["queued", "running", "paused", "cancelling"].includes(r.state),
    ),
  );
  async function refresh() {
    rows.value = await api.jobs();
  }
  async function start(
    kind: string,
    thumbnails = true,
    sourceFolderId: number | null = null,
    duplicateOptions: DuplicateOptions | null = null,
  ) {
    try {
      error.value = "";
      await api.startJob({
        kind,
        thumbnails,
        sourceFolderId,
        duplicateOptions,
      });
      await refresh();
      return true;
    } catch (e) {
      error.value = String(e);
      return false;
    }
  }
  async function control(id: string, action: string) {
    try {
      await api.controlJob(id, action);
      await refresh();
    } catch (e) {
      error.value = String(e);
    }
  }
  onMounted(async () => {
    if (isTauri()) {
      off = await listen<JobRecord>("jobs:changed", (e) => {
        const index = rows.value.findIndex((r) => r.id === e.payload.id);
        const previous = index < 0 ? null : rows.value[index].state;
        if (index < 0) rows.value.unshift(e.payload);
        else rows.value[index] = e.payload;
        if (
          previous !== e.payload.state &&
          [
            "completed",
            "partial",
            "cancelled",
            "interrupted",
            "failed",
          ].includes(e.payload.state)
        )
          terminal?.(e.payload);
      });
      offLibrary = await listen("library:changed", () => changed());
    }
    if (disposed) {
      off?.();
      offLibrary?.();
      return;
    }
    await refresh().catch((e) => (error.value = String(e)));
  });
  onUnmounted(() => {
    disposed = true;
    off?.();
    offLibrary?.();
  });
  return { rows, error, active, start, control, refresh };
}

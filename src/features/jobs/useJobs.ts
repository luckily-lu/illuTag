import { computed, onMounted, onUnmounted, ref } from "vue";
import { isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { api } from "../../shared/ipc";
import type { JobRecord } from "../../shared/contracts/JobRecord";
export function useJobs(changed: () => void) {
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
  ) {
    try {
      await api.startJob({ kind, thumbnails, sourceFolderId });
      await refresh();
    } catch (e) {
      error.value = String(e);
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
        if (index < 0) rows.value.unshift(e.payload);
        else rows.value[index] = e.payload;
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

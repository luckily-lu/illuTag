import { computed, ref, shallowRef } from "vue";
import { api, errorMessage } from "../../shared/ipc";
import type { DuplicateWorkflowStatus } from "../../shared/contracts/DuplicateWorkflowStatus";
import type { DuplicateWorkflowGroup } from "../../shared/contracts/DuplicateWorkflowGroup";
import type { DuplicateOptions } from "../../shared/contracts/DuplicateOptions";

export function useDuplicateWorkflow() {
  const status = shallowRef<DuplicateWorkflowStatus | null>(null);
  const groups = shallowRef<DuplicateWorkflowGroup[]>([]);
  const kind = ref("exact"),
    reviewOpen = ref(false),
    index = ref(0);
  const loading = ref(false),
    statusLoading = ref(false),
    editing = ref(false),
    error = ref("");
  const nextCursor = ref<string | null>(null),
    totalGroups = ref(0),
    pageOffset = ref(0);
  const pageCursors = new Map<number, string | null>([[0, null]]);
  let previousCursor: string | null = null,
    previousLimit = 20,
    epoch = 0,
    statusEpoch = 0,
    reviewEpoch = 0,
    disposed = false,
    deferredRefresh = false,
    deferredReload = false;
  let statusTimer: ReturnType<typeof setTimeout> | undefined;
  const current = computed(() => groups.value[index.value] ?? null);
  const position = computed(() => pageOffset.value + index.value + 1);
  const canPrevious = computed(() => index.value > 0 || pageOffset.value > 0);
  const canNext = computed(
    () => index.value + 1 < groups.value.length || !!nextCursor.value,
  );
  const canClean = computed(() =>
    ["loading", "error"].includes(status.value?.stage ?? "")
      ? false
      : status.value?.policy === "illuTag-three-pass-v1"
        ? status.value.stage === "refined" &&
          ["exact", "near"].includes(kind.value)
        : kind.value === "exact"
          ? status.value?.stage !== "empty"
          : status.value?.stage === "refined" && kind.value !== "candidate",
  );

  function options(
    stage: "scan" | "refine" | "organize" | "cascade",
    sensitivity: string,
    forceRecompute: boolean,
  ): DuplicateOptions {
    return {
      sensitivity,
      forceRecompute,
      verificationMode:
        stage === "cascade" || stage === "organize"
          ? stage
          : stage === "scan"
            ? "candidate"
            : "refine",
      regroupOnly: stage === "refine",
    };
  }
  async function refresh(reload = false, retry = false) {
    if (disposed) return;
    clearTimeout(statusTimer);
    // A background terminal event may arrive while a manual change is awaiting
    // its response. Read again after the edit instead of racing an older snapshot
    // against its returned revision.
    if (editing.value) {
      deferredRefresh = true;
      deferredReload ||= reload;
      return;
    }
    const token = ++statusEpoch;
    statusLoading.value = true;
    try {
      const next = await api.duplicateWorkflow(retry);
      if (disposed || token !== statusEpoch) return;
      error.value = "";
      const previous = status.value;
      status.value = next;
      if (next.stage === "loading") {
        statusTimer = setTimeout(() => void refresh(reload), 1000);
        return;
      }
      if (next.stage === "empty") reviewOpen.value = false;
      const previousKind = kind.value;
      if (
        next.stage !== "refined" &&
        ["near", "similar", "uncertain"].includes(kind.value)
      )
        kind.value = "candidate";
      if (
        previous?.generation !== next.generation ||
        previousKind !== kind.value
      ) {
        ++epoch;
        groups.value = [];
        pageOffset.value = 0;
        pageCursors.clear();
        pageCursors.set(0, null);
        nextCursor.value = null;
        index.value = 0;
        if (reviewOpen.value && next.generation) await loadPage(null, 0);
      } else if (
        reviewOpen.value &&
        (reload || previous?.revision !== next.revision)
      ) {
        if (current.value) await reloadCurrent(true);
        else await loadPage(null, 0);
      }
    } catch (e) {
      if (token === statusEpoch) error.value = errorMessage(e);
    } finally {
      if (token === statusEpoch) statusLoading.value = false;
    }
  }
  async function loadPage(
    cursor: string | null,
    offset: number,
    last = false,
    limit = 20,
  ) {
    const generation = status.value?.generation;
    if (!generation) return;
    const token = ++epoch;
    loading.value = true;
    error.value = "";
    try {
      const page = await api.duplicateWorkflowGroups({
        generation,
        kind: kind.value,
        cursor,
        groupId: null,
        limit,
      });
      if (
        token !== epoch ||
        generation !== status.value?.generation ||
        page.revision !== status.value?.revision
      )
        return;
      groups.value = page.groups;
      nextCursor.value = page.nextCursor;
      totalGroups.value = page.totalGroups;
      pageOffset.value = page.paging?.offset ?? offset;
      previousLimit = page.paging?.previousLimit ?? Math.min(offset, 20);
      previousCursor =
        page.paging?.previousCursor ??
        pageCursors.get(Math.max(0, offset - previousLimit)) ??
        null;
      pageCursors.set(pageOffset.value, cursor);
      index.value = last ? Math.max(0, page.groups.length - 1) : 0;
    } catch (e) {
      if (token === epoch) error.value = errorMessage(e);
    } finally {
      if (token === epoch) loading.value = false;
    }
  }
  async function open(value: string) {
    if (disposed || ["loading", "error"].includes(status.value?.stage ?? ""))
      return;
    ++reviewEpoch;
    const changed = kind.value !== value;
    kind.value = value;
    reviewOpen.value = true;
    if (changed || !groups.value.length) {
      pageCursors.clear();
      pageCursors.set(0, null);
      await loadPage(null, 0);
    } else await reloadCurrent(true);
  }
  function close() {
    ++reviewEpoch;
    reviewOpen.value = false;
    ++epoch;
    loading.value = false;
  }
  async function step(delta: number) {
    if (loading.value || editing.value) return;
    const target = index.value + delta;
    if (target >= 0 && target < groups.value.length) {
      index.value = target;
      await reloadCurrent();
      return;
    }
    if (delta > 0 && nextCursor.value) {
      await loadPage(nextCursor.value, pageOffset.value + groups.value.length);
    } else if (delta < 0 && pageOffset.value > 0)
      await loadPage(
        previousCursor,
        Math.max(0, pageOffset.value - previousLimit),
        true,
        previousLimit,
      );
  }
  async function reloadCurrent(reanchor = false) {
    const old = current.value,
      generation = status.value?.generation;
    if (!old || !generation) return;
    const token = ++epoch;
    loading.value = true;
    try {
      const page = await api.duplicateWorkflowGroups({
        generation,
        kind: kind.value,
        cursor: null,
        groupId: old.id,
        limit: 50,
        ...(reanchor ? { reanchor: true } : {}),
      });
      if (
        token !== epoch ||
        generation !== status.value?.generation ||
        page.revision !== status.value?.revision
      )
        return;
      let next = page.groups.find((g) => g.id === old.id);
      // Editing an expanded group must not collapse the member being reviewed.
      while (next?.nextCursor && next.members.length < old.members.length) {
        const tail = await api.duplicateWorkflowGroups({
          generation,
          kind: kind.value,
          cursor: next.nextCursor,
          groupId: old.id,
          limit: 50,
        });
        if (
          token !== epoch ||
          generation !== status.value?.generation ||
          tail.revision !== status.value?.revision
        )
          return;
        const part = tail.groups.find((g) => g.id === old.id);
        if (!part || !part.members.length) break;
        next = { ...part, members: [...next.members, ...part.members] };
      }
      if (reanchor) {
        groups.value = page.groups.map((g) =>
          g.id === old.id && next ? next : g,
        );
        index.value = Math.max(
          0,
          groups.value.findIndex((g) => g.id === old.id),
        );
        nextCursor.value = page.nextCursor;
        totalGroups.value = page.totalGroups;
        pageOffset.value = page.paging?.offset ?? 0;
        previousCursor = page.paging?.previousCursor ?? null;
        previousLimit = page.paging?.previousLimit ?? 20;
        pageCursors.clear();
        pageCursors.set(0, null);
      } else if (next)
        groups.value = groups.value.map((g) => (g.id === old.id ? next : g));
      else await reloadCurrent(true);
    } catch (e) {
      if (token === epoch) error.value = errorMessage(e);
    } finally {
      if (token === epoch) loading.value = false;
    }
  }
  async function moreMembers() {
    const group = current.value,
      generation = status.value?.generation;
    if (!group?.nextCursor || !generation || loading.value) return;
    const token = ++epoch;
    loading.value = true;
    try {
      const page = await api.duplicateWorkflowGroups({
        generation,
        kind: kind.value,
        groupId: group.id,
        cursor: group.nextCursor,
        limit: 50,
      });
      if (
        token !== epoch ||
        generation !== status.value?.generation ||
        page.revision !== status.value?.revision
      )
        return;
      const next = page.groups.find((g) => g.id === group.id);
      if (next) {
        const seen = new Set(group.members.map((m) => m.member.image.id));
        groups.value = groups.value.map((g) =>
          g.id === group.id
            ? {
                ...next,
                members: [
                  ...g.members,
                  ...next.members.filter((m) => !seen.has(m.member.image.id)),
                ],
              }
            : g,
        );
      }
    } catch (e) {
      if (token === epoch) error.value = errorMessage(e);
    } finally {
      if (token === epoch) loading.value = false;
    }
  }
  async function edit(action: string, imageId: string | null = null) {
    const snapshot = status.value,
      group = current.value,
      viewToken = reviewEpoch;
    if (
      disposed ||
      !snapshot?.generation ||
      !group ||
      editing.value ||
      loading.value
    )
      return false;
    ++statusEpoch;
    // A superseded read cannot clear its flag in finally. Do not leave every
    // later action locked after a manual edit overtakes a passive refresh.
    statusLoading.value = false;
    editing.value = true;
    error.value = "";
    let succeeded = false,
      failure = "";
    try {
      const next = await api.editDuplicateReview({
        generation: snapshot.generation,
        revision: snapshot.revision,
        groupId: group.id,
        imageId,
        action,
      });
      if (disposed || snapshot.generation !== status.value?.generation)
        return false;
      status.value = next;
      if (reviewOpen.value && viewToken === reviewEpoch) await reloadCurrent();
      else if (reviewOpen.value) {
        deferredRefresh = true;
        deferredReload = true;
      }
      succeeded = true;
    } catch (e) {
      // Refresh stale data but never replay a user's edit against another revision.
      failure = errorMessage(e);
      deferredRefresh = true;
      deferredReload = true;
    } finally {
      editing.value = false;
      if (!disposed && deferredRefresh) {
        const reload = deferredReload;
        deferredRefresh = deferredReload = false;
        await refresh(reload);
      }
      if (!disposed && failure) error.value = failure;
    }
    return (
      succeeded && !disposed && reviewOpen.value && viewToken === reviewEpoch
    );
  }
  async function confirmNext() {
    if (await edit("confirm")) await step(1);
  }
  function dispose() {
    disposed = true;
    clearTimeout(statusTimer);
    ++reviewEpoch;
    ++epoch;
    ++statusEpoch;
  }
  return {
    status,
    groups,
    kind,
    reviewOpen,
    index,
    current,
    position,
    canPrevious,
    canNext,
    canClean,
    loading,
    statusLoading,
    editing,
    error,
    nextCursor,
    totalGroups,
    options,
    refresh,
    open,
    close,
    step,
    moreMembers,
    edit,
    confirmNext,
    dispose,
  };
}

import { computed, ref } from "vue";
import type { DuplicateMember } from "../../shared/contracts/DuplicateMember";
import type { DuplicateActionPreview } from "../../shared/contracts/DuplicateActionPreview";
import type { DuplicateOperationStatus } from "../../shared/contracts/DuplicateOperationStatus";
import type { DuplicateOperationResult } from "../../shared/contracts/DuplicateOperationResult";
import type { JobRecord } from "../../shared/contracts/JobRecord";

export type ReviewMode = "exactOthers" | "selected" | "autoExact";
export type ReviewOperation = {
  preview: DuplicateActionPreview;
  mode: ReviewMode;
  sourceOperationId: string | null;
  advance: boolean;
  jobId: string;
  jobState: string;
  status: DuplicateOperationStatus | null;
  items: DuplicateOperationResult[];
  nextCursor: string | null;
  resultsLoaded: boolean;
  loading: boolean;
  error: string;
  handled: boolean;
  followingGroupIds: string[];
  trackedIds: string[];
};

// Owned by App, not the page instance. Gallery selection and review selection never mix.
export function useDuplicateReview() {
  const generation = ref<string | null>(null);
  const kept = ref(new Set<string>());
  const selections = ref(new Map<string, Set<string>>());
  const processed = ref(new Set<string>());
  const decisions = ref(new Map<string, "handled" | "skipped">());
  const operations = ref(new Map<string, ReviewOperation>());
  const activeGroupId = ref<string | null>(null),
    activeOperationId = ref<string | null>(null),
    scrollTop = ref(0),
    version = ref(0);
  const key = (groupId: string) => `${generation.value ?? ""}:${groupId}`;
  const reviewedCount = computed(
    () =>
      [...decisions.value.values()].filter((value) => value === "handled")
        .length,
  );
  const skippedCount = computed(
    () =>
      [...decisions.value.values()].filter((value) => value === "skipped")
        .length,
  );
  const latest = computed(
    () =>
      operations.value.get(activeOperationId.value ?? "") ??
      [...operations.value.values()].at(-1) ??
      null,
  );
  function adopt(next: string | null) {
    if (generation.value === next) return;
    generation.value = next;
    selections.value = new Map();
    processed.value = new Set();
    activeGroupId.value = null;
    scrollTop.value = 0;
    // Explicit keep choices remain protected; the backend validates their current identity.
  }
  function selection(groupId: string) {
    return selections.value.get(key(groupId)) ?? new Set<string>();
  }
  function protect(member: DuplicateMember) {
    const id = member.image.id;
    const next = new Set(kept.value);
    next.has(id) ? next.delete(id) : next.add(id);
    kept.value = next;
    if (next.has(id)) {
      const selected = new Map(selections.value);
      for (const [group, ids] of selected)
        selected.set(group, new Set([...ids].filter((value) => value !== id)));
      selections.value = selected;
    }
  }
  function toggle(groupId: string, member: DuplicateMember) {
    const id = member.image.id;
    if (kept.value.has(id) || processed.value.has(id) || member.stale) return;
    const next = new Set(selection(groupId));
    next.has(id) ? next.delete(id) : next.add(id);
    selections.value = new Map(selections.value).set(key(groupId), next);
  }
  function clearSelection(groupId: string) {
    const next = new Map(selections.value);
    next.delete(key(groupId));
    selections.value = next;
  }
  function done(ids: string[], restore = false) {
    if (restore) {
      processed.value = new Set(
        [...processed.value].filter((id) => !ids.includes(id)),
      );
      return;
    }
    processed.value = new Set([...processed.value, ...ids]);
    const selected = new Map(selections.value);
    for (const [group, values] of selected)
      selected.set(
        group,
        new Set([...values].filter((id) => !ids.includes(id))),
      );
    selections.value = selected;
  }
  function decide(groupId: string, state: "handled" | "skipped") {
    decisions.value = new Map(decisions.value).set(key(groupId), state);
  }
  function decision(groupId: string) {
    return decisions.value.get(key(groupId));
  }
  function register(
    preview: DuplicateActionPreview,
    mode: ReviewMode,
    advance: boolean,
    job: JobRecord,
    sourceOperationId: string | null = null,
    followingGroupIds: string[] = [],
    trackedIds: string[] = [],
  ) {
    operations.value = new Map(operations.value).set(preview.operationId, {
      preview,
      mode,
      sourceOperationId,
      advance,
      jobId: job.id,
      jobState: job.state,
      status: null,
      items: [],
      nextCursor: null,
      resultsLoaded: false,
      loading: false,
      error: "",
      handled: false,
      followingGroupIds,
      trackedIds,
    });
    activeOperationId.value = preview.operationId;
    version.value++;
  }
  function observe(job: JobRecord) {
    const operationId = job.request.duplicateOperationId;
    if (!operationId) return;
    const operation = operations.value.get(operationId);
    if (operation) {
      operation.jobState = job.state;
      version.value++;
    }
  }
  function reconcileActive(members: DuplicateMember[]) {
    const active = new Set(
      members
        .filter((member) => !member.image.trashed)
        .map((member) => member.image.id),
    );
    processed.value = new Set(
      [...processed.value].filter((id) => !active.has(id)),
    );
  }
  function restoreGroup(
    operation: ReviewOperation,
    restoredIds: string[] = [],
  ) {
    done(restoredIds, true);
    const next = new Map(decisions.value);
    next.delete(`${operation.preview.generation}:${operation.preview.groupId}`);
    decisions.value = next;
  }
  return {
    generation,
    kept,
    selections,
    processed,
    decisions,
    operations,
    activeGroupId,
    activeOperationId,
    scrollTop,
    version,
    reviewedCount,
    skippedCount,
    latest,
    adopt,
    selection,
    protect,
    toggle,
    clearSelection,
    done,
    decide,
    decision,
    register,
    observe,
    restoreGroup,
    reconcileActive,
  };
}
export type DuplicateReview = ReturnType<typeof useDuplicateReview>;

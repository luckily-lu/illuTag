import { computed, ref, shallowRef } from "vue";
import { api, errorMessage } from "../../shared/ipc";
import type { DuplicateStatus } from "../../shared/contracts/DuplicateStatus";
import type { DuplicateGroup } from "../../shared/contracts/DuplicateGroup";
import type { DuplicateOptions } from "../../shared/contracts/DuplicateOptions";

export function useDuplicates() {
  const status = shallowRef<DuplicateStatus | null>(null);
  const groups = shallowRef<DuplicateGroup[]>([]);
  const kind = ref("all"),
    sensitivity = ref("standard"),
    verificationMode = ref("verified"),
    forceRecompute = ref(false);
  const loading = ref(false),
    pageLoading = ref(false),
    error = ref("");
  const nextCursor = ref<string | null>(null),
    totalGroups = ref(0);
  const memberLoading = ref(new Set<string>());
  let epoch = 0,
    statusEpoch = 0;
  const memberEpochs = new Map<string, number>();
  const pendingSensitivity = computed(
    () =>
      !!status.value?.generation &&
      (sensitivity.value !== status.value.sensitivity ||
        verificationMode.value !==
          (status.value.verificationMode || "candidate")),
  );

  function options(regroupOnly = false): DuplicateOptions {
    return {
      sensitivity: sensitivity.value,
      verificationMode: verificationMode.value,
      forceRecompute: forceRecompute.value,
      regroupOnly:
        regroupOnly &&
        !!status.value?.generation &&
        !status.value.needsUpdate &&
        !forceRecompute.value,
    };
  }
  async function loadGroups(append = false) {
    const generation = status.value?.generation;
    if (
      !generation ||
      (append && (!nextCursor.value || loading.value || pageLoading.value))
    )
      return;
    const token = append ? epoch : ++epoch;
    if (append) pageLoading.value = true;
    else {
      loading.value = true;
      pageLoading.value = false;
      memberEpochs.clear();
      memberLoading.value = new Set();
      groups.value = [];
    }
    error.value = "";
    try {
      const page = await api.duplicateGroups({
        generation,
        kind: kind.value,
        cursor: append ? nextCursor.value : null,
        limit: 20,
      });
      if (token !== epoch || page.generation !== status.value?.generation)
        return;
      groups.value = append ? [...groups.value, ...page.groups] : page.groups;
      nextCursor.value = page.nextCursor;
      totalGroups.value = page.totalGroups;
    } catch (e) {
      if (token === epoch) error.value = errorMessage(e);
    } finally {
      if (token === epoch) {
        loading.value = false;
        pageLoading.value = false;
      }
    }
  }
  async function refresh() {
    const token = ++statusEpoch;
    try {
      const next = await api.duplicateStatus();
      if (token !== statusEpoch) return;
      const previous = status.value;
      status.value = next;
      if (!previous) sensitivity.value = next.sensitivity || "standard";
      if (previous?.generation !== next.generation) {
        epoch++;
        loading.value = false;
        pageLoading.value = false;
        groups.value = [];
        nextCursor.value = null;
        memberEpochs.clear();
        memberLoading.value = new Set();
        if (next.generation) await loadGroups();
      } else if (next.generation && !groups.value.length && !loading.value)
        await loadGroups();
    } catch (e) {
      if (token === statusEpoch) error.value = errorMessage(e);
    }
  }
  async function setKind(value: string) {
    if (kind.value === value) return;
    kind.value = value;
    await loadGroups();
  }
  async function moreMembers(groupId: string) {
    const group = groups.value.find((g) => g.id === groupId);
    const generation = status.value?.generation;
    if (!group?.nextCursor || !generation || memberLoading.value.has(groupId))
      return;
    const token = epoch,
      memberToken = (memberEpochs.get(groupId) ?? 0) + 1;
    memberEpochs.set(groupId, memberToken);
    memberLoading.value = new Set([...memberLoading.value, groupId]);
    try {
      const page = await api.duplicateMembers({
        generation,
        groupId,
        cursor: group.nextCursor,
        limit: 50,
      });
      if (
        token !== epoch ||
        memberEpochs.get(groupId) !== memberToken ||
        page.generation !== status.value?.generation ||
        page.groupId !== groupId
      )
        return;
      groups.value = groups.value.map((g) =>
        g.id === groupId
          ? {
              ...g,
              members: [...g.members, ...page.members],
              nextCursor: page.nextCursor,
              totalMembers: page.totalMembers,
            }
          : g,
      );
    } catch (e) {
      if (token === epoch) error.value = errorMessage(e);
    } finally {
      if (token === epoch)
        memberLoading.value = new Set(
          [...memberLoading.value].filter((id) => id !== groupId),
        );
    }
  }
  async function clear() {
    try {
      await api.clearDuplicateCache();
      statusEpoch++;
      epoch++;
      status.value = null;
      groups.value = [];
      nextCursor.value = null;
      totalGroups.value = 0;
      error.value = "";
      await refresh();
    } catch (e) {
      error.value = errorMessage(e);
    }
  }
  async function reaudit(groupIds: string[], truncated = false) {
    const generation = status.value?.generation;
    if (!generation) return;
    const token = epoch;
    const wanted = groups.value
      .filter((group) => truncated || groupIds.includes(group.id))
      .map((group) => group.id);
    for (const groupId of wanted) {
      try {
        const group = await api.duplicateGroup({ generation, groupId });
        if (token !== epoch || generation !== status.value?.generation) return;
        groups.value = group
          ? groups.value.map((old) => (old.id === groupId ? group : old))
          : groups.value.filter((old) => old.id !== groupId);
      } catch (e) {
        if (token === epoch) error.value = errorMessage(e);
      }
    }
  }
  function dispose() {
    statusEpoch++;
    epoch++;
  }
  return {
    status,
    groups,
    kind,
    sensitivity,
    forceRecompute,
    loading,
    pageLoading,
    error,
    nextCursor,
    totalGroups,
    memberLoading,
    pendingSensitivity,
    verificationMode,
    options,
    refresh,
    loadGroups,
    setKind,
    moreMembers,
    clear,
    reaudit,
    dispose,
  };
}

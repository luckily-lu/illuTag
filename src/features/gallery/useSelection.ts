import { computed, ref } from "vue";
import type { GalleryImage } from "../../shared/contracts/GalleryImage";
export function useSelection() {
  const active = ref(false),
    ids = ref(new Set<string>()),
    allSelection = ref(false),
    excludedIds = ref(new Set<string>()),
    selectionCount = ref(0);
  const count = computed(() =>
    allSelection.value
      ? selectionCount.value - excludedIds.value.size
      : ids.value.size,
  );
  function isSelected(id: string) {
    return allSelection.value ? !excludedIds.value.has(id) : ids.value.has(id);
  }
  function selectAllResults(count: number) {
    ids.value = new Set();
    excludedIds.value = new Set();
    selectionCount.value = count;
    allSelection.value = true;
    active.value = true;
  }
  function toggle(id: string) {
    active.value = true;
    if (allSelection.value) {
      const next = new Set(excludedIds.value);
      next.has(id) ? next.delete(id) : next.add(id);
      excludedIds.value = next;
      return;
    }
    const next = new Set(ids.value);
    next.has(id) ? next.delete(id) : next.add(id);
    ids.value = next;
  }
  function all(images: GalleryImage[]) {
    allSelection.value = false;
    excludedIds.value = new Set();
    ids.value = new Set(images.map((i) => i.id));
    active.value = true;
  }
  function done(successes: string[]) {
    if (allSelection.value) {
      const next = new Set(excludedIds.value);
      successes.forEach((id) => next.add(id));
      excludedIds.value = next;
      return;
    }
    const next = new Set(ids.value);
    successes.forEach((id) => next.delete(id));
    ids.value = next;
    if (!next.size) active.value = false;
  }
  function clear() {
    ids.value = new Set();
    excludedIds.value = new Set();
    allSelection.value = false;
    selectionCount.value = 0;
    active.value = false;
  }
  return {
    active,
    ids,
    count,
    toggle,
    all,
    done,
    clear,
    allSelection,
    excludedIds,
    isSelected,
    selectAllResults,
  };
}

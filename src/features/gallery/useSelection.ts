import { computed, ref } from "vue";
import type { GalleryImage } from "../../shared/contracts/GalleryImage";
export function useSelection() {
  const active = ref(false),
    ids = ref(new Set<string>());
  const count = computed(() => ids.value.size);
  function toggle(id: string) {
    active.value = true;
    const next = new Set(ids.value);
    next.has(id) ? next.delete(id) : next.add(id);
    ids.value = next;
  }
  function all(images: GalleryImage[]) {
    ids.value = new Set(images.map((i) => i.id));
    active.value = true;
  }
  function done(successes: string[]) {
    const next = new Set(ids.value);
    successes.forEach((id) => next.delete(id));
    ids.value = next;
    if (!next.size) active.value = false;
  }
  function clear() {
    ids.value = new Set();
    active.value = false;
  }
  return { active, ids, count, toggle, all, done, clear };
}

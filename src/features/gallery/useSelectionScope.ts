import { computed, watch, type Ref } from "vue";
import type { GalleryQuery } from "../../shared/contracts/GalleryQuery";
import { semanticQueryKey } from "./queryState";

/**
 * Runs `reset` whenever the semantic result range changes. Ordinary pagination
 * and re-rendering keep the selection; a new filter, scope or search text does
 * not, because the previously selected rows may no longer be visible.
 */
export function useSelectionScope(
  query: Ref<GalleryQuery>,
  reset: () => void,
) {
  const key = computed(() => semanticQueryKey(query.value));
  watch(key, () => reset());
  return key;
}

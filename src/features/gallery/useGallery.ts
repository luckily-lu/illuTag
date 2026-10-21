import { computed, ref, shallowRef } from "vue";
import { api } from "../../shared/ipc";
import type { GalleryImage } from "../../shared/contracts/GalleryImage";
import type { QueryPage } from "../../shared/contracts/QueryPage";
import type { GalleryQuery } from "../../shared/contracts/GalleryQuery";
import { QueryGeneration } from "./queryState";

export function useGallery() {
  const images = shallowRef<GalleryImage[]>([]),
    total = ref(0),
    loading = ref(false),
    pageLoading = ref(false);
  const query = ref<GalleryQuery>({
    scope: "all",
    folderId: null,
    unclassifiedOnlyParentFolderId: null,
    tags: [],
    englishQuery: "",
    fileName: "",
    naturalLanguage: "",
    offset: 0,
    limit: 5000,
    randomSeed: 0,
  });
  const generation = new QueryGeneration();
  const error = ref(""),
    revision = ref(0);
  async function search() {
    const token = generation.next();
    loading.value = true;
    pageLoading.value = false;
    error.value = "";
    try {
      const page = await api.query({ ...query.value, offset: 0 });
      if (!generation.current(token)) return;
      images.value = page.images;
      total.value = page.totalImageCount;
      revision.value = page.revision;
    } catch (e) {
      if (generation.current(token))
        error.value = String(e instanceof Error ? e.message : e);
    } finally {
      if (generation.current(token)) loading.value = false;
    }
  }
  async function more() {
    if (
      loading.value ||
      pageLoading.value ||
      images.value.length >= total.value
    )
      return;
    const snapshot = JSON.stringify(query.value),
      token = generation.next();
    pageLoading.value = true;
    try {
      const page = await api.query({
        ...query.value,
        offset: images.value.length,
      });
      if (
        !generation.current(token) ||
        snapshot !== JSON.stringify(query.value)
      )
        return;
      if (page.revision !== revision.value) {
        await search();
        return;
      }
      const known = new Set(images.value.map((i) => i.id));
      images.value = [
        ...images.value,
        ...page.images.filter((i) => !known.has(i.id)),
      ];
      total.value = page.totalImageCount;
    } catch (e) {
      if (generation.current(token)) error.value = String(e);
    } finally {
      if (generation.current(token)) pageLoading.value = false;
    }
  }
  function patch(rows: GalleryImage[]) {
    const byId = new Map(rows.map((i) => [i.id, i]));
    images.value = images.value.map((i) => byId.get(i.id) ?? i);
  }
  function adopt(page: QueryPage) {
    generation.invalidate();
    images.value = page.images;
    total.value = page.totalImageCount;
    revision.value = page.revision;
    loading.value = false;
  }
  function bootstrapSnapshot() {
    return {
      generation: generation.peek(),
      query: JSON.stringify(query.value),
    };
  }
  /**
   * Startup bootstrap always queries the default page. If the user already
   * navigated or searched while it was pending, adopting it would show default
   * rows behind a different filter. Navigation metadata is adopted separately.
   */
  function adoptBootstrap(
    page: QueryPage,
    snapshot: { generation: number; query: string },
  ) {
    if (snapshot.generation !== generation.peek()) return false;
    if (snapshot.query !== JSON.stringify(query.value)) return false;
    adopt(page);
    return true;
  }
  function removeImage(id: string, nextRevision: number) {
    generation.invalidate();
    const before = images.value.length;
    images.value = images.value.filter((i) => i.id !== id);
    total.value = Math.max(0, total.value - (before - images.value.length));
    revision.value = nextRevision;
    loading.value = false;
    pageLoading.value = false;
  }
  function restoreImage(
    image: GalleryImage,
    index: number,
    nextRevision: number,
  ) {
    generation.invalidate();
    if (!images.value.some((i) => i.id === image.id)) {
      const next = [...images.value];
      next.splice(Math.max(0, index), 0, image);
      images.value = next;
      total.value++;
    }
    revision.value = nextRevision;
    loading.value = false;
    pageLoading.value = false;
  }
  function resetFilters() {
    generation.invalidate();
    query.value.scope = "all";
    query.value.folderId = null;
    query.value.unclassifiedOnlyParentFolderId = null;
    query.value.tags = [];
    query.value.fileName = "";
    query.value.naturalLanguage = "";
    query.value.englishQuery = "";
    return search();
  }
  return {
    images,
    total,
    loading,
    pageLoading,
    query,
    error,
    revision,
    search,
    more,
    patch,
    resetFilters,
    adopt,
    bootstrapSnapshot,
    adoptBootstrap,
    removeImage,
    restoreImage,
    hasMore: computed(() => images.value.length < total.value),
  };
}

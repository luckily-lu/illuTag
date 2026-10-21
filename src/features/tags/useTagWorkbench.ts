import { computed, ref, shallowRef, watch } from "vue";
import { useTagManagement } from "../../composables/useTagManagement";
import { api, errorMessage } from "../../shared/ipc";
import type { GalleryImage } from "../../shared/contracts/GalleryImage";
import type { TagCover } from "../../shared/contracts/TagCover";

export function useTagWorkbench() {
  const error = ref("");
  const catalog = useTagManagement({
    formatError: errorMessage,
    setErrorText: (v) => (error.value = v),
  });
  const selected = ref<string[]>([]),
    page = ref(0),
    previewOpen = ref(true),
    managing = ref(false);
  const covers = shallowRef(new Map<string, TagCover>());
  const images = shallowRef<GalleryImage[]>([]),
    total = ref(0),
    loading = ref(false),
    pageLoading = ref(false),
    previewError = ref(""),
    previewExhausted = ref(false);
  const hasMore = computed(
    () =>
      images.value.length > 0 &&
      images.value.length < total.value &&
      !previewExhausted.value,
  );
  const coverLoading = ref(false);
  const favoriteLoading = ref(true);
  const favoriteBusy = ref(new Set<string>());
  async function favoriteTag(tag: string) {
    if (favoriteLoading.value || favoriteBusy.value.has(tag)) return;
    favoriteBusy.value = new Set(favoriteBusy.value).add(tag);
    try {
      const tags = await api.setFavoriteTag(
        tag,
        !catalog.dictFavoriteTags.value.has(tag),
      );
      const next = new Set(catalog.dictFavoriteTags.value);
      if (tags.includes(tag)) next.add(tag);
      else next.delete(tag);
      catalog.dictFavoriteTags.value = next;
      page.value = Math.min(page.value, pages.value - 1);
    } catch (e) {
      error.value = errorMessage(e);
    } finally {
      const next = new Set(favoriteBusy.value);
      next.delete(tag);
      favoriteBusy.value = next;
    }
  }
  const pageSize = 18;
  const visible = computed(() =>
    catalog.filteredDictTags.value.slice(
      page.value * pageSize,
      (page.value + 1) * pageSize,
    ),
  );
  const pages = computed(() =>
    Math.max(1, Math.ceil(catalog.filteredDictTags.value.length / pageSize)),
  );
  const labels = computed(
    () =>
      new Map(
        catalog.allDictTags.value.map((t) => [t.tagEn, t.tagZh || t.tagEn]),
      ),
  );
  let coverEpoch = 0,
    initEpoch = 0,
    previewEpoch = 0,
    timer: ReturnType<typeof setTimeout> | undefined,
    initialized = false,
    suspended = false,
    previewSelection = "";
  const previewActive = () =>
    !suspended && previewOpen.value && !managing.value;
  async function loadCovers(force = false) {
    const token = ++coverEpoch;
    const wanted = visible.value
      .map((t) => t.tagEn)
      .filter((tag) => force || !covers.value.has(tag));
    if (!wanted.length) {
      coverLoading.value = false;
      return;
    }
    coverLoading.value = true;
    try {
      const rows = await api.tagCovers(wanted);
      if (token !== coverEpoch) return;
      covers.value = new Map([
        ...covers.value,
        ...rows.map((c) => [c.tag, c] as const),
      ]);
    } catch (e) {
      if (token === coverEpoch) error.value = errorMessage(e);
    } finally {
      if (token === coverEpoch) coverLoading.value = false;
    }
  }
  async function preview(append = false) {
    if (!previewActive()) return;
    const tags = [...selected.value],
      selection = JSON.stringify(tags);
    if (
      append &&
      (loading.value ||
        pageLoading.value ||
        !hasMore.value ||
        previewSelection !== selection)
    )
      return;
    clearTimeout(timer);
    const token = ++previewEpoch,
      offset = append ? images.value.length : 0;
    if (append) pageLoading.value = true;
    else {
      loading.value = true;
      pageLoading.value = false;
      images.value = [];
      total.value = 0;
      previewExhausted.value = false;
      previewSelection = selection;
    }
    previewError.value = "";
    if (!tags.length) {
      loading.value = false;
      pageLoading.value = false;
      return;
    }
    try {
      const result = await api.tagPreview(tags, offset, 18);
      if (token !== previewEpoch) return;
      images.value = append
        ? [...images.value, ...result.images]
        : result.images;
      total.value = result.totalImageCount;
      previewExhausted.value = result.images.length === 0;
    } catch (e) {
      if (token === previewEpoch) previewError.value = errorMessage(e);
    } finally {
      if (token === previewEpoch) {
        loading.value = false;
        pageLoading.value = false;
      }
    }
  }
  function toggle(tag: string) {
    selected.value = selected.value.includes(tag)
      ? selected.value.filter((t) => t !== tag)
      : [...selected.value, tag];
  }
  async function init() {
    const token = ++initEpoch;
    suspended = false;
    initialized = false;
    favoriteLoading.value = true;
    catalog.tagManagerTab.value = "dict";
    await catalog.loadDictionaryBrowser();
    if (suspended || token !== initEpoch) return;
    try {
      const favorites = await api.favoriteTags();
      if (suspended || token !== initEpoch) return;
      catalog.dictFavoriteTags.value = new Set(favorites);
    } catch (e) {
      if (token === initEpoch) error.value = errorMessage(e);
    } finally {
      if (token === initEpoch) favoriteLoading.value = false;
    }
    if (suspended || token !== initEpoch) return;
    initialized = true;
    await loadCovers(true);
    if (!suspended && selected.value.length) await preview();
  }
  function setCover(cover: TagCover) {
    ++coverEpoch;
    covers.value = new Map(covers.value).set(cover.tag, cover);
    void loadCovers();
  }
  watch(
    [
      catalog.dictGroupId,
      catalog.dictQuery,
      catalog.dictOnlyUsed,
      catalog.dictSort,
    ],
    () => {
      page.value = 0;
    },
  );
  watch(visible, () => {
    if (initialized) void loadCovers();
  });
  watch(
    selected,
    () => {
      ++previewEpoch;
      images.value = [];
      total.value = 0;
      previewError.value = "";
      previewExhausted.value = false;
      loading.value = previewActive() && selected.value.length > 0;
      pageLoading.value = false;
      clearTimeout(timer);
      if (loading.value) timer = setTimeout(() => void preview(), 140);
    },
    { flush: "sync" },
  );
  watch(
    [previewOpen, managing],
    () => {
      ++previewEpoch;
      clearTimeout(timer);
      loading.value = false;
      pageLoading.value = false;
      if (
        previewActive() &&
        selected.value.length &&
        !images.value.length &&
        !previewError.value
      ) {
        loading.value = true;
        timer = setTimeout(() => void preview(), 140);
      }
    },
    { flush: "sync" },
  );
  function suspend() {
    suspended = true;
    ++initEpoch;
    ++coverEpoch;
    ++previewEpoch;
    clearTimeout(timer);
    loading.value = false;
    pageLoading.value = false;
    coverLoading.value = false;
  }
  function removeImage(id: string) {
    ++previewEpoch;
    const before = images.value.length;
    images.value = images.value.filter((i) => i.id !== id);
    total.value = Math.max(0, total.value - (before - images.value.length));
    loading.value = false;
    pageLoading.value = false;
  }
  function restoreImage(image: GalleryImage, index: number) {
    ++previewEpoch;
    if (!images.value.some((i) => i.id === image.id)) {
      const next = [...images.value];
      next.splice(Math.max(0, index), 0, image);
      images.value = next;
      total.value++;
    }
    loading.value = false;
    pageLoading.value = false;
  }
  return {
    catalog,
    error,
    selected,
    page,
    previewOpen,
    managing,
    covers,
    images,
    total,
    loading,
    pageLoading,
    previewError,
    hasMore,
    coverLoading,
    visible,
    pages,
    labels,
    init,
    preview,
    toggle,
    setCover,
    suspend,
    favoriteTag,
    favoriteLoading,
    favoriteBusy,
    removeImage,
    restoreImage,
  };
}
export type TagWorkbenchState = ReturnType<typeof useTagWorkbench>;

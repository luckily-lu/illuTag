import { ref, shallowRef, type Ref } from "vue";
import { api, errorMessage } from "../../shared/ipc";
import type { GalleryImage } from "../../shared/contracts/GalleryImage";

export type ViewerSurface = {
  key: string;
  images: GalleryImage[];
  total: number;
  remove: (id: string, revision: number) => void;
  restore: (image: GalleryImage, index: number, revision: number) => void;
  more: () => Promise<unknown>;
};
export function useViewerRemoval(
  current: Ref<GalleryImage | null>,
  surface: () => ViewerSurface,
  changed: () => void,
) {
  const busy = ref(false),
    error = ref("");
  const undo = shallowRef<{
    image: GalleryImage;
    index: number;
    key: string;
    revision: number;
  } | null>(null);
  async function trash() {
    const image = current.value;
    if (!image || image.trashed || busy.value) return;
    const origin = surface(),
      index = origin.images.findIndex((i) => i.id === image.id);
    busy.value = true;
    error.value = "";
    try {
      const result = await api.mutate({
        action: "trash",
        imageIds: [image.id],
        folderId: null,
        favorite: null,
        customTags: [],
        supplementTags: [],
      });
      if (!result.successfulIds.includes(image.id))
        throw new Error(
          result.failures[0]?.message || "图片未能移入应用回收站",
        );
      undo.value = { image, index, key: origin.key, revision: result.revision };
      if (surface().key === origin.key) {
        origin.remove(image.id, result.revision);
        if (current.value?.id === image.id) {
          let now = surface();
          if (index >= now.images.length && now.images.length < now.total) {
            try {
              await now.more();
            } catch (e) {
              error.value = errorMessage(e);
            }
            now = surface();
          }
          if (surface().key === origin.key && current.value?.id === image.id)
            current.value =
              now.images[Math.min(Math.max(0, index), now.images.length - 1)] ??
              null;
        }
      }
      changed();
    } catch (e) {
      error.value = errorMessage(e);
    } finally {
      busy.value = false;
    }
  }
  async function restore() {
    const entry = undo.value;
    if (!entry || busy.value) return;
    busy.value = true;
    error.value = "";
    try {
      const result = await api.undoViewerTrash(entry.image.id, entry.revision);
      const image = result.images.find((i) => i.id === entry.image.id);
      if (!image || !result.successfulIds.includes(entry.image.id))
        throw new Error(result.failures[0]?.message || "还原失败");
      if (surface().key === entry.key) {
        surface().restore(image, entry.index, result.revision);
        current.value = image;
      }
      undo.value = null;
      changed();
    } catch (e) {
      error.value = errorMessage(e);
    } finally {
      busy.value = false;
    }
  }
  return { busy, error, undo, trash, restore };
}

import { computed, ref } from "vue";
import { invoke } from "../../shared/ipc";
import type { UserFolder } from "../../shared/contracts/UserFolder";

export function useFolders() {
  const folders = ref<UserFolder[]>([]),
    expanded = ref(new Set<number>());
  const rows = computed(() => {
    const result: Array<UserFolder & { depth: number; hasChildren: boolean }> =
      [];
    const walk = (parent: number | null, depth: number, seen: Set<number>) => {
      for (const f of folders.value
        .filter((f) => (f.parentId ?? null) === parent)
        .sort((a, b) => a.sortOrder - b.sortOrder || a.id - b.id)) {
        if (seen.has(f.id)) continue;
        seen.add(f.id);
        result.push({
          ...f,
          depth,
          hasChildren: folders.value.some((c) => c.parentId === f.id),
        });
        if (expanded.value.has(f.id)) walk(f.id, depth + 1, seen);
      }
    };
    walk(null, 0, new Set());
    return result;
  });
  function toggle(id: number) {
    const s = new Set(expanded.value);
    s.has(id) ? s.delete(id) : s.add(id);
    expanded.value = s;
  }
  async function create(name: string, parentId: number | null) {
    await invoke("create_user_folder_command", { name, parentId });
    if (parentId !== null)
      expanded.value = new Set([...expanded.value, parentId]);
  }
  async function rename(folderId: number, name: string) {
    await invoke("rename_user_folder_command", { folderId, name });
  }
  async function remove(folderId: number) {
    await invoke("delete_user_folder_command", { folderId });
  }
  async function reorder(
    folderId: number,
    targetFolderId: number,
    position: string,
  ) {
    await invoke("reorder_user_folder_command", {
      folderId,
      targetFolderId,
      position,
    });
  }
  return { folders, expanded, rows, toggle, create, rename, remove, reorder };
}

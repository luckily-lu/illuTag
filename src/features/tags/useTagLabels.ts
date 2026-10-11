import { shallowRef } from "vue";
import { invoke } from "../../shared/ipc";
const labels = shallowRef(new Map<string, string>());
export function useTagLabels() {
  function remember(rows: { tagEn: string; tagZh?: string | null }[]) {
    const next = new Map(labels.value);
    for (const row of rows)
      if (row.tagZh?.trim()) next.set(row.tagEn, row.tagZh);
    labels.value = next;
  }
  async function resolve(tags: string[]) {
    const missing = tags.filter((tag) => !labels.value.has(tag));
    if (missing.length)
      remember(
        await invoke<{ tagEn: string; tagZh: string | null }[]>(
          "resolve_tag_labels_command",
          { tags: missing },
        ),
      );
  }
  return {
    remember,
    resolve,
    label: (tag: string) => labels.value.get(tag) ?? tag.replaceAll("_", " "),
  };
}

import { inject, provide, type InjectionKey } from "vue";
import type { useGallery } from "../features/gallery/useGallery";
import type { useSelection } from "../features/gallery/useSelection";
export type WorkspaceContext = {
  gallery: ReturnType<typeof useGallery>;
  selection: ReturnType<typeof useSelection>;
};
const key: InjectionKey<WorkspaceContext> = Symbol("illuTag workspace");
export const provideWorkspace = (context: WorkspaceContext) =>
  provide(key, context);
export function useWorkspace() {
  const value = inject(key);
  if (!value) throw new Error("Workspace is unavailable");
  return value;
}

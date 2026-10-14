import {
  invoke as tauriInvoke,
  isTauri,
  convertFileSrc,
} from "@tauri-apps/api/core";
import type { Bootstrap } from "./contracts/Bootstrap";
import type { NavigationMetadata } from "./contracts/NavigationMetadata";
import type { GalleryQuery } from "./contracts/GalleryQuery";
import type { QueryPage } from "./contracts/QueryPage";
import type { ImageDetail } from "./contracts/ImageDetail";
import type { ImageMutation } from "./contracts/ImageMutation";
import type { MutationResult } from "./contracts/MutationResult";
import type { JobRecord } from "./contracts/JobRecord";
import type { JobRequest } from "./contracts/JobRequest";
import type { DuplicateStatus } from "./contracts/DuplicateStatus";
import type { DuplicateQuery } from "./contracts/DuplicateQuery";
import type { DuplicateGroupPage } from "./contracts/DuplicateGroupPage";
import type { DuplicateMemberQuery } from "./contracts/DuplicateMemberQuery";
import type { DuplicateMemberPage } from "./contracts/DuplicateMemberPage";
import type { DuplicateGroup } from "./contracts/DuplicateGroup";
import type { DuplicateManagementRequest } from "./contracts/DuplicateManagementRequest";
import type { DuplicateActionPreview } from "./contracts/DuplicateActionPreview";
import type { DuplicateApplyRequest } from "./contracts/DuplicateApplyRequest";
import type { DuplicateOperationStatus } from "./contracts/DuplicateOperationStatus";
import type { DuplicateOperationQuery } from "./contracts/DuplicateOperationQuery";
import type { DuplicateOperationPage } from "./contracts/DuplicateOperationPage";

export function errorMessage(error: unknown): string {
  if (error && typeof error === "object" && "message" in error)
    return String(error.message);
  return String(error);
}
export async function invoke<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  if (!isTauri()) {
    // Preview is an explicit fixture mode; ordinary browsers never silently pretend to be a library.
    if (new URLSearchParams(location.search).get("preview") !== "1")
      throw new Error("请在桌面应用中打开图库");
    const { previewInvoke } = await import("./preview");
    return previewInvoke<T>(command, args);
  }
  try {
    return await tauriInvoke<T>(command, args);
  } catch (error) {
    throw new Error(errorMessage(error));
  }
}
export const imageUrl = (path: string) =>
  /^(data:|https?:|\/)/.test(path) ? path : convertFileSrc(path);
export const api = {
  bootstrap: () => invoke<Bootstrap>("get_bootstrap"),
  navigation: () => invoke<NavigationMetadata>("get_navigation"),
  query: (query: GalleryQuery) => invoke<QueryPage>("query_gallery", { query }),
  detail: (imageId: string) =>
    invoke<ImageDetail>("get_image_detail", { imageId }),
  mutate: (request: ImageMutation) =>
    invoke<MutationResult>("mutate_images", { request }),
  jobs: () => invoke<JobRecord[]>("list_jobs"),
  startJob: (request: JobRequest) =>
    invoke<JobRecord>("start_job", { request }),
  controlJob: (id: string, action: string) =>
    invoke<void>("control_job", { id, action }),
  duplicateStatus: () => invoke<DuplicateStatus>("duplicate_status"),
  duplicateGroups: (query: DuplicateQuery) =>
    invoke<DuplicateGroupPage>("query_duplicate_groups", { query }),
  duplicateMembers: (query: DuplicateMemberQuery) =>
    invoke<DuplicateMemberPage>("query_duplicate_members", { query }),
  clearDuplicateCache: () => invoke<void>("clear_duplicate_cache"),
  duplicateGroup: (
    query: Pick<DuplicateMemberQuery, "generation" | "groupId">,
  ) => invoke<DuplicateGroup | null>("query_duplicate_group", { query }),
  prepareDuplicateAction: (request: DuplicateManagementRequest) =>
    invoke<DuplicateActionPreview>("prepare_duplicate_action", { request }),
  applyDuplicateAction: (request: DuplicateApplyRequest) =>
    invoke<JobRecord>("apply_duplicate_action", { request }),
  duplicateOperationStatus: (operationId: string) =>
    invoke<DuplicateOperationStatus>("duplicate_operation_status", {
      operationId,
    }),
  duplicateOperationResults: (query: DuplicateOperationQuery) =>
    invoke<DuplicateOperationPage>("duplicate_operation_results", { query }),
  undoDuplicateAction: (operationId: string) =>
    invoke<JobRecord>("undo_duplicate_action", { operationId }),
  clearDuplicateManagementRecords: () =>
    invoke<void>("clear_duplicate_management_records", { confirmed: true }),
};

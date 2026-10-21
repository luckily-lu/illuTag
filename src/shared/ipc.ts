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
import type { JobHistoryQuery } from "./contracts/JobHistoryQuery";
import type { JobHistoryPage } from "./contracts/JobHistoryPage";
import type { DuplicateStatus } from "./contracts/DuplicateStatus";
import type { DuplicateQuery } from "./contracts/DuplicateQuery";
import type { DuplicateGroupPage } from "./contracts/DuplicateGroupPage";
import type { DuplicateMemberQuery } from "./contracts/DuplicateMemberQuery";
import type { DuplicateMemberPage } from "./contracts/DuplicateMemberPage";
import type { DuplicateGroup } from "./contracts/DuplicateGroup";
import type { DuplicateWorkflowStatus } from "./contracts/DuplicateWorkflowStatus";
import type { DuplicateWorkflowQuery } from "./contracts/DuplicateWorkflowQuery";
import type { DuplicateWorkflowPage } from "./contracts/DuplicateWorkflowPage";
import type { DuplicateReviewEdit } from "./contracts/DuplicateReviewEdit";
import type { DuplicateWorkflowRequest } from "./contracts/DuplicateWorkflowRequest";
import type { DuplicateManagementRequest } from "./contracts/DuplicateManagementRequest";
import type { DuplicateActionPreview } from "./contracts/DuplicateActionPreview";
import type { DuplicateApplyRequest } from "./contracts/DuplicateApplyRequest";
import type { DuplicateOperationStatus } from "./contracts/DuplicateOperationStatus";
import type { DuplicateOperationQuery } from "./contracts/DuplicateOperationQuery";
import type { DuplicateOperationPage } from "./contracts/DuplicateOperationPage";
import type { RecycleSelection } from "./contracts/RecycleSelection";
import type { RecycleSelectionRequest } from "./contracts/RecycleSelectionRequest";
import type { RecycleApplyRequest } from "./contracts/RecycleApplyRequest";
import type { RecycleOperationStatus } from "./contracts/RecycleOperationStatus";
import type { RecycleOperationQuery } from "./contracts/RecycleOperationQuery";
import type { RecycleOperationPage } from "./contracts/RecycleOperationPage";
import type { TagCover } from "./contracts/TagCover";
import type { TagCoverRequest } from "./contracts/TagCoverRequest";

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
  tagCovers: (tags: string[]) =>
    invoke<TagCover[]>("list_tag_covers_command", { tags }),
  saveTagCover: (request: TagCoverRequest) =>
    invoke<TagCover>("save_tag_cover_command", { request }),
  tagPreview: (tags: string[], offset = 0, limit = 18) =>
    invoke<QueryPage>("query_tag_preview_command", { tags, offset, limit }),
  tagCoverCandidates: (tag: string, offset = 0, limit = 24) =>
    invoke<QueryPage>("query_tag_cover_candidates_command", {
      tag,
      offset,
      limit,
    }),
  bootstrap: () => invoke<Bootstrap>("get_bootstrap"),
  navigation: () => invoke<NavigationMetadata>("get_navigation"),
  query: (query: GalleryQuery) => invoke<QueryPage>("query_gallery", { query }),
  detail: (imageId: string) =>
    invoke<ImageDetail>("get_image_detail", { imageId }),
  mutate: (request: ImageMutation) =>
    invoke<MutationResult>("mutate_images", { request }),
  jobs: () => invoke<JobRecord[]>("list_jobs"),
  jobHistory: (query: JobHistoryQuery) =>
    invoke<JobHistoryPage>("job_history", { query }),
  retryJob: (id: string) => invoke<JobRecord>("retry_job", { id }),
  clearJobHistory: () =>
    invoke<number>("clear_job_history", { confirmed: true }),
  favoriteTags: () => invoke<string[]>("list_favorite_tags"),
  setFavoriteTag: (tag: string, favorite: boolean) =>
    invoke<string[]>("set_favorite_tag", { tag, favorite }),
  undoViewerTrash: (imageId: string, revision: number) =>
    invoke<MutationResult>("undo_viewer_trash", { imageId, revision }),
  startJob: (request: JobRequest) =>
    invoke<JobRecord>("start_job", { request }),
  controlJob: (id: string, action: string) =>
    invoke<void>("control_job", { id, action }),
  duplicateStatus: () => invoke<DuplicateStatus>("duplicate_status"),
  duplicateWorkflow: (retry = false) =>
    invoke<DuplicateWorkflowStatus>(
      "duplicate_workflow_status",
      retry ? { retry: true } : {},
    ),
  duplicateWorkflowGroups: (query: DuplicateWorkflowQuery) =>
    invoke<DuplicateWorkflowPage>("query_duplicate_workflow", { query }),
  editDuplicateReview: (request: DuplicateReviewEdit) =>
    invoke<DuplicateWorkflowStatus>("edit_duplicate_review", { request }),
  prepareDuplicateWorkflow: (
    request: DuplicateWorkflowRequest,
    kind?: "exact" | "near",
  ) =>
    invoke<DuplicateActionPreview>(
      "prepare_duplicate_workflow",
      kind ? { request, kind } : { request },
    ),
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
  prepareRecycleSelection: (request: RecycleSelectionRequest) =>
    invoke<RecycleSelection>("prepare_recycle_selection", { request }),
  applyRecycleAction: (request: RecycleApplyRequest) =>
    invoke<JobRecord>("apply_recycle_action", { request }),
  recycleOperationStatus: (operationId: string) =>
    invoke<RecycleOperationStatus>("recycle_operation_status", { operationId }),
  recycleOperationResults: (query: RecycleOperationQuery) =>
    invoke<RecycleOperationPage>("recycle_operation_results", { query }),
};

//! Explicit, derived duplicate-search cache. The library database is never written here.
pub mod cache;
mod cache_capacity;
mod cascade_gc;
pub mod fingerprint;
pub mod index;
pub mod management;
pub mod verification;
mod evidence_store;
mod evidence;
mod cascade_pixels;
mod cascade_store;
#[cfg(test)]
mod cascade_tests;
#[cfg(test)]
mod cascade_scope_tests;
mod verification_cache;
pub mod workflow;

use crate::library::GalleryImage;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateOptions {
    pub sensitivity: String,
    #[serde(default)]
    pub force_recompute: bool,
    #[serde(default)]
    pub regroup_only: bool,
    /// candidate keeps the original visual search; verified adds local-content checks.
    #[serde(default)]
    pub verification_mode: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateCoverage {
    pub total: i64,
    pub processed: i64,
    pub failed: i64,
    pub skipped_near: i64,
    pub low_information: i64,
    pub too_dense: i64,
    #[serde(default)]
    pub verified_pairs: i64,
    #[serde(default)]
    pub content_variants: i64,
    #[serde(default)]
    pub uncertain_pairs: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateAttempt {
    pub state: String,
    pub coverage: DuplicateCoverage,
    pub updated_at: i64,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateStatus {
    pub generation: Option<String>,
    pub sensitivity: String,
    #[serde(default)]
    pub verification_mode: String,
    pub checked_at: Option<i64>,
    pub duration_ms: Option<i64>,
    pub state: String,
    pub needs_update: bool,
    pub coverage: DuplicateCoverage,
    pub cache_bytes: i64,
    pub exact_groups: i64,
    pub near_groups: i64,
    pub unique_images: i64,
    pub errors: Vec<String>,
    pub last_attempt: Option<DuplicateAttempt>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateMetric {
    pub phash_distance: Option<u32>,
    pub dhash_distance: Option<u32>,
    pub color_distance: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateMember {
    pub image: GalleryImage,
    pub metric: DuplicateMetric,
    pub stale: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroup {
    pub id: String,
    pub kind: String,
    pub reference: DuplicateMember,
    pub total_members: i64,
    pub members: Vec<DuplicateMember>,
    pub next_cursor: Option<String>,
    #[serde(default)]
    pub needs_review: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateQuery {
    pub generation: String,
    pub kind: String,
    pub cursor: Option<String>,
    pub limit: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroupPage {
    pub generation: String,
    pub groups: Vec<DuplicateGroup>,
    pub next_cursor: Option<String>,
    pub total_groups: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateMemberQuery {
    pub generation: String,
    pub group_id: String,
    pub cursor: Option<String>,
    pub limit: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateMemberPage {
    pub generation: String,
    pub group_id: String,
    pub members: Vec<DuplicateMember>,
    pub next_cursor: Option<String>,
    pub total_members: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateManagementRequest {
    pub generation: String,
    pub group_id: String,
    pub mode: String,
    pub keep_ids: Vec<String>,
    #[serde(default)]
    pub protected_ids: Vec<String>,
    #[serde(default)]
    pub image_ids: Vec<String>,
    pub action: String,
    #[serde(default)]
    pub source_operation_id: Option<String>,
}
#[derive(Debug, Default, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateMetadataSummary {
    pub favorite_count: i64,
    pub classified_count: i64,
    pub custom_tagged_count: i64,
    pub supplement_tagged_count: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateActionPreview {
    pub operation_id: String,
    pub generation: String,
    pub group_id: String,
    pub action: String,
    pub target_count: i64,
    pub total_bytes: i64,
    pub protected_count: i64,
    pub skipped_count: i64,
    pub metadata: DuplicateMetadataSummary,
    pub sample: Vec<DuplicateMember>,
    pub warnings: Vec<String>,
    pub expires_at: i64,
    pub requires_system_confirmation: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateApplyRequest {
    pub operation_id: String,
    pub confirmed: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateOperationStatus {
    pub operation_id: String,
    pub generation: String,
    pub group_id: String,
    pub action: String,
    pub state: String,
    #[serde(default)]
    pub stage: String,
    #[serde(default)]
    pub stage_processed: i64,
    #[serde(default)]
    pub stage_total: i64,
    pub processed: i64,
    pub total: i64,
    pub succeeded: i64,
    pub failed: i64,
    pub skipped: i64,
    pub undoable: bool,
    pub affected_group_ids: Vec<String>,
    pub affected_groups_truncated: bool,
    pub errors: Vec<String>,
    pub updated_at: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateOperationResult {
    pub ordinal: i64,
    pub image_id: Option<String>,
    pub file_name: String,
    pub path: Option<String>,
    pub state: String,
    pub code: String,
    pub message: String,
    pub file_moved: bool,
    pub db_updated: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateOperationQuery {
    pub operation_id: String,
    pub cursor: Option<String>,
    pub limit: i64,
    #[serde(default)]
    pub state: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateOperationPage {
    pub operation_id: String,
    pub items: Vec<DuplicateOperationResult>,
    pub next_cursor: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroupQuery {
    pub generation: String,
    pub group_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateCategory {
    pub kind: String,
    pub images: i64,
    pub groups: i64,
    pub cleanup_count: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateWorkflowStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub rounds: Option<Vec<DuplicateRoundStatus>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub policy: Option<String>,
    pub generation: Option<String>,
    pub revision: String,
    pub stage: String,
    pub scan: DuplicateStatus,
    pub categories: Vec<DuplicateCategory>,
    pub cleanup_count: i64,
    pub protected_count: i64,
    pub last_operation_id: Option<String>,
    pub errors: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateRoundStatus {
    pub round: i64,
    pub name: String,
    pub state: String,
    pub input_pairs: i64,
    pub passed_pairs: i64,
    pub filtered_pairs: i64,
    pub uncertain_pairs: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateReviewMember {
    pub member: DuplicateMember,
    pub disposition: String,
    pub is_keeper: bool,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateWorkflowGroup {
    pub id: String,
    pub kind: String,
    pub reference_id: String,
    pub confirmed: bool,
    pub total_members: i64,
    pub members: Vec<DuplicateReviewMember>,
    pub next_cursor: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateWorkflowQuery {
    pub generation: String,
    pub kind: String,
    pub cursor: Option<String>,
    pub limit: i64,
    pub group_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reanchor: Option<bool>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateWorkflowPaging {
    pub offset: i64,
    pub previous_cursor: Option<String>,
    pub previous_limit: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateWorkflowPage {
    pub generation: String,
    pub revision: String,
    pub groups: Vec<DuplicateWorkflowGroup>,
    pub next_cursor: Option<String>,
    pub total_groups: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub paging: Option<DuplicateWorkflowPaging>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateReviewEdit {
    pub generation: String,
    pub revision: String,
    pub group_id: String,
    pub image_id: Option<String>,
    pub action: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateWorkflowRequest {
    pub generation: String,
    pub revision: String,
}

pub use cache::{
    clear_cache, invalidate, invalidate_database, membership_invalidated, query_groups,
    query_members, status,
};
pub use workflow::run_job;

#[cfg(test)]
mod workflow_tests;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod verification_tests;
#[cfg(test)]
mod evidence_tests;

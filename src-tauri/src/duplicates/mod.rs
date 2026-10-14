//! Explicit, derived duplicate-search cache. The library database is never written here.
pub mod cache;
pub mod fingerprint;
pub mod index;

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

pub use cache::{
    clear_cache, invalidate, invalidate_database, query_groups, query_members, run_job, status,
};

#[cfg(test)]
mod tests;

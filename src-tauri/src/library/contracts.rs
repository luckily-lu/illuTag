use super::*;

#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct LibraryStore {
    pub folders: Vec<LibraryFolder>,
    pub images: Vec<GalleryImage>,
    pub total_image_count: i64,
    pub large_library_mode: bool,
    pub image_offset: i64,
    pub image_limit: i64,
    pub user_folders: Vec<UserFolder>,
    pub image_folders: Vec<ImageFolderAssignment>,
    pub reference_board_folders: Vec<ReferenceBoardFolder>,
    pub reference_boards: Vec<ReferenceBoard>,
    pub reference_board_items: Vec<ReferenceBoardItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct LibraryFolder {
    pub id: i64,
    pub path: String,
    pub added_at: i64,
    pub last_scanned_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct GalleryImage {
    pub id: String,
    pub path: String,
    pub thumbnail_path: Option<String>,
    pub file_name: String,
    pub ext: String,
    pub width: u32,
    pub height: u32,
    pub file_size: i64,
    pub modified_at: i64,
    pub imported_at: i64,
    pub folder_id: i64,
    pub missing: bool,
    pub trashed: bool,
    pub is_favorite: bool,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct GalleryImagePage {
    pub images: Vec<GalleryImage>,
    pub total_image_count: i64,
    pub offset: i64,
    pub limit: i64,
}

#[derive(Debug, Clone, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct DevFakeGalleryOptions {
    pub count: i64,
    pub source_folder: Option<String>,
    pub sample_image_ids: Option<Vec<String>>,
    pub randomize_dimensions: Option<bool>,
    pub randomize_file_names: Option<bool>,
    pub randomize_imported_at: Option<bool>,
    pub randomize_folders: Option<bool>,
    pub randomize_favorites: Option<bool>,
    pub randomize_tags: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct DevSmallFileSetOptions {
    pub count: i64,
    pub root_dir: Option<String>,
    pub format: Option<String>,
    pub subfolder_count: Option<i64>,
}

#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct DevStressToolResult {
    pub created_images: i64,
    pub created_files: i64,
    pub deleted_images: i64,
    pub deleted_files: i64,
    pub root_path: Option<String>,
    pub database_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct UserFolder {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub sort_order: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct ImageFolderAssignment {
    pub image_id: String,
    pub folder_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceBoardFolder {
    pub id: i64,
    pub name: String,
    pub sort_order: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceBoard {
    pub id: i64,
    pub folder_id: Option<i64>,
    pub name: String,
    pub sort_order: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceBoardItem {
    pub id: i64,
    pub board_id: i64,
    pub image_id: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub rotation: f32,
    pub flip_x: bool,
    pub flip_y: bool,
    pub z_index: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct ImageBytes {
    pub bytes: Vec<u8>,
    pub mime_type: String,
    pub file_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceBoardPasteImageInput {
    pub image_bytes: Vec<u8>,
    pub mime_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct BatchSupplementTagInput {
    pub tag_en: String,
    pub tag_zh: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct BatchSystemTrashResult {
    pub store: LibraryStore,
    pub moved_count: usize,
    pub failed_image_ids: Vec<String>,
    pub first_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct WdTaggerTagScore {
    pub tag: String,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct WdTaggerTestResult {
    pub image_id: String,
    pub ratings: Vec<WdTaggerTagScore>,
    pub general_tags: Vec<WdTaggerTagScore>,
    pub character_tags: Vec<WdTaggerTagScore>,
    pub general_threshold: f32,
    pub character_threshold: f32,
    pub elapsed_ms: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct ImageAutoTag {
    pub category: String,
    pub tag_en: String,
    pub tag_zh: Option<String>,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct ImageAutoTagSummary {
    pub image_id: String,
    pub character_tags: Vec<ImageAutoTag>,
    pub general_tags: Vec<ImageAutoTag>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct KnownAutoTagSuggestion {
    pub tag_en: String,
    pub tag_zh: Option<String>,
    pub image_count: i64,
    pub is_user_custom: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct ImageUserCustomTag {
    pub tag_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct ImageUserSupplementTag {
    pub tag_en: String,
    pub tag_zh: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct ImageUserTagSummary {
    pub image_id: String,
    pub custom_tags: Vec<ImageUserCustomTag>,
    pub supplement_tags: Vec<ImageUserSupplementTag>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct UserTagFolder {
    pub id: i64,
    pub name: String,
    pub sort_order: i64,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct TagManagementState {
    pub folders: Vec<UserTagFolder>,
    pub unclassified_tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct TagDictionaryGroup {
    pub id: String,
    pub zh: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct TagDictionaryBrowserItem {
    pub tag_en: String,
    pub tag_zh: Option<String>,
    pub group: String,
    pub image_count: i64,
    pub sort_index: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct TagDictionaryBrowserState {
    pub groups: Vec<TagDictionaryGroup>,
    pub tags: Vec<TagDictionaryBrowserItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct UserFolderRuleCondition {
    pub logic: String,
    pub source: String,
    pub keyword: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct UserFolderRule {
    pub folder_id: i64,
    pub conditions: Vec<UserFolderRuleCondition>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct DataMigrationBackup {
    pub format: String,
    pub schema_version: i64,
    pub exported_at: i64,
    pub app_version: String,
    pub library: LibraryStore,
    pub image_user_custom_tags: Vec<BackupImageUserCustomTag>,
    pub image_user_supplement_tags: Vec<BackupImageUserSupplementTag>,
    pub user_custom_tags: Vec<BackupUserCustomTag>,
    pub user_tag_folders: Vec<BackupUserTagFolder>,
    pub user_tag_folder_members: Vec<BackupUserTagFolderMember>,
    pub user_folder_rules: Vec<UserFolderRule>,
    pub app_meta: Vec<BackupAppMeta>,
    pub frontend_settings: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct BackupImageUserCustomTag {
    pub image_id: String,
    pub tag_text: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct BackupImageUserSupplementTag {
    pub image_id: String,
    pub tag_en: String,
    pub tag_zh: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct BackupUserCustomTag {
    pub tag_text: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct BackupUserTagFolder {
    pub id: i64,
    pub name: String,
    pub sort_order: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct BackupUserTagFolderMember {
    pub folder_id: i64,
    pub tag_text: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct BackupAppMeta {
    pub key: String,
    pub value: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct BackupPathStatus {
    pub path: String,
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct DataMigrationBackupInspection {
    pub schema_version: i64,
    pub exported_at: i64,
    pub app_version: String,
    pub folder_count: usize,
    pub image_count: usize,
    pub user_folder_count: usize,
    pub reference_board_count: usize,
    pub path_statuses: Vec<BackupPathStatus>,
    pub frontend_settings: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct BackupPathMapping {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct OrganizedExportManifest {
    pub format: String,
    pub schema_version: i64,
    pub exported_at: i64,
    pub app_version: String,
    pub entries: Vec<OrganizedExportManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct OrganizedExportManifestEntry {
    pub image_id: String,
    pub original_path: String,
    pub folder_id: i64,
    pub folder_path: String,
    pub exported_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct OrganizedExportReport {
    pub manifest_path: String,
    pub exported_count: i64,
    pub skipped_count: i64,
    pub failed_count: i64,
    pub failures: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct GallerySearchFilters {
    pub chinese_tag_ens: Vec<String>,
    pub english_query: String,
    pub file_name_query: String,
    pub confidence_min: f32,
    pub confidence_max: f32,
    pub scope: Option<String>,
    pub folder_id: Option<i64>,
    pub unclassified_only_parent_folder_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundScanStatus {
    pub running: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct StartupCleanupStatus {
    pub running: bool,
    pub generation: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundScanProgress {
    pub running: bool,
    pub paused: bool,
    pub phase: String,
    pub scanned_folders: i64,
    pub total_folders: i64,
    pub new_images: i64,
    pub updated_images: i64,
    pub skipped_images: i64,
    pub removed_missing_images: i64,
    pub queued_images: i64,
    pub tagged_images: i64,
    pub failed_images: i64,
    pub scanned_files: i64,
    pub current_folder: String,
    pub last_error: Option<String>,
    pub recent_errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct ThumbnailGenerationProgress {
    pub running: bool,
    pub paused: bool,
    pub phase: String,
    pub total_candidates: i64,
    pub processed_images: i64,
    pub generated_images: i64,
    pub skipped_images: i64,
    pub failed_images: i64,
    pub last_error: Option<String>,
    pub recent_errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct NaturalLanguageScanStatus {
    pub running: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct NaturalLanguageScanProgress {
    pub running: bool,
    pub paused: bool,
    pub phase: String,
    pub total_images: i64,
    pub processed_images: i64,
    pub generated_images: i64,
    pub skipped_images: i64,
    pub failed_images: i64,
    pub last_error: Option<String>,
    pub recent_errors: Vec<String>,
}

impl Default for BackgroundScanProgress {
    fn default() -> Self {
        Self {
            running: false,
            paused: false,
            phase: "idle".to_string(),
            scanned_folders: 0,
            total_folders: 0,
            new_images: 0,
            updated_images: 0,
            skipped_images: 0,
            removed_missing_images: 0,
            queued_images: 0,
            tagged_images: 0,
            failed_images: 0,
            scanned_files: 0,
            current_folder: String::new(),
            last_error: None,
            recent_errors: Vec::new(),
        }
    }
}

impl Default for ThumbnailGenerationProgress {
    fn default() -> Self {
        Self {
            running: false,
            paused: false,
            phase: "idle".to_string(),
            total_candidates: 0,
            processed_images: 0,
            generated_images: 0,
            skipped_images: 0,
            failed_images: 0,
            last_error: None,
            recent_errors: Vec::new(),
        }
    }
}

impl Default for NaturalLanguageScanProgress {
    fn default() -> Self {
        Self {
            running: false,
            paused: false,
            phase: "idle".to_string(),
            total_images: 0,
            processed_images: 0,
            generated_images: 0,
            skipped_images: 0,
            failed_images: 0,
            last_error: None,
            recent_errors: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub(super) struct ScannedImage {
    pub(super) path: String,
    pub(super) file_name: String,
    pub(super) ext: String,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) file_size: i64,
    pub(super) modified_at: i64,
    pub(super) imported_at: i64,
    /// false = 与数据库记录一致且未缺失，扫描时可跳过写库
    pub(super) unchanged: bool,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ExistingImageMeta {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) file_size: i64,
    pub(super) modified_at: i64,
    pub(super) missing: i64,
}

pub(super) struct ScanCollectResult {
    pub(super) tag_queue_image_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub(super) struct ThumbnailCandidate {
    pub(super) image_id: String,
    pub(super) image_path: String,
    pub(super) modified_at: i64,
    pub(super) file_size: i64,
    pub(super) current_thumb_path: Option<String>,
    pub(super) current_source_modified_at: Option<i64>,
    pub(super) current_source_file_size: Option<i64>,
}

pub(super) struct ThumbnailWorkerResult {
    pub(super) candidate: ThumbnailCandidate,
    pub(super) output: Result<String, String>,
}

pub(super) struct NaturalLanguageEmbeddingCandidate {
    pub(super) image_id: String,
    pub(super) image_path: String,
    pub(super) modified_at: i64,
    pub(super) current_source_modified_at: Option<i64>,
}

#[derive(Debug)]
pub(super) struct NaturalLanguageSearchHeapEntry {
    pub(super) image_id: String,
    pub(super) score: f32,
}

impl PartialEq for NaturalLanguageSearchHeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.image_id == other.image_id && self.score.to_bits() == other.score.to_bits()
    }
}

impl Eq for NaturalLanguageSearchHeapEntry {}

impl PartialOrd for NaturalLanguageSearchHeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for NaturalLanguageSearchHeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        match self
            .score
            .partial_cmp(&other.score)
            .unwrap_or(Ordering::Less)
        {
            Ordering::Equal => self.image_id.cmp(&other.image_id),
            ordering => ordering.reverse(),
        }
    }
}

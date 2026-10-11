use illutag_core::{jobs::*, library::*};
use ts_rs::{Config, TS};
fn main() {
    let config = Config::from_env();
    RestoreManifest::export_all(&config).unwrap();
    OrganizedExportReport::export_all(&config).unwrap();
    BackupPathMapping::export_all(&config).unwrap();
    Bootstrap::export_all(&config).unwrap();
    NavigationMetadata::export_all(&config).unwrap();
    TagLabel::export_all(&config).unwrap();
    QueryPage::export_all(&config).unwrap();
    GalleryQuery::export_all(&config).unwrap();
    ImageDetail::export_all(&config).unwrap();
    ImageMutation::export_all(&config).unwrap();
    MutationResult::export_all(&config).unwrap();
    AppError::export_all(&config).unwrap();
    JobRecord::export_all(&config).unwrap();
    TagManagementState::export_all(&config).unwrap();
    TagDictionaryBrowserState::export_all(&config).unwrap();
    UserFolderRule::export_all(&config).unwrap();
    KnownAutoTagSuggestion::export_all(&config).unwrap();
}

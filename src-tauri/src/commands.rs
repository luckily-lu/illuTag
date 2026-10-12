use super::*;
use illutag_core::library::*;

#[tauri::command]
pub(super) async fn resolve_tag_labels_command(
    tags: Vec<String>,
    app: tauri::AppHandle,
) -> Result<Vec<TagLabel>, AppError> {
    off_main(app, move |state| resolve_tag_labels(tags, &state))
        .await
        .map_err(Into::into)
}

// Synchronous Tauri commands run on the main thread and freeze the window ("未响应")
// while a query runs. Heavy DB work must go through this helper instead.
pub(super) async fn off_main<T, F>(app: tauri::AppHandle, work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(AppState) -> Result<T, String> + Send + 'static,
{
    let state = app.state::<AppState>().inner().clone();
    tauri::async_runtime::spawn_blocking(move || work(state))
        .await
        .map_err(|error| format!("后台任务失败：{error}"))?
}

#[tauri::command]
pub(super) async fn list_library(app: tauri::AppHandle) -> Result<LibraryStore, String> {
    off_main(app, |state| {
        let started = Instant::now();
        eprintln!("[startup-prof] list_library_command begin");
        let result = list_library_from_state(&state);
        eprintln!(
            "[startup-prof] list_library_command total_ms={}",
            started.elapsed().as_millis()
        );
        result
    })
    .await
}

#[tauri::command]
pub(super) async fn data_directory_info_command(
    app: tauri::AppHandle,
) -> Result<DataDirectoryInfo, String> {
    off_main(app, move |state| {
        let legacy_data_dir = state
            .legacy_data_dir
            .as_ref()
            .map(|path| path.to_string_lossy().to_string());
        let legacy_database_detected = state
            .legacy_data_dir
            .as_ref()
            .map(|path| path.join("illutag.sqlite").is_file())
            .unwrap_or(false);
        Ok(DataDirectoryInfo {
            data_dir: state.data_dir.to_string_lossy().to_string(),
            using_fallback: state.data_dir_fallback_message.is_some(),
            fallback_message: state.data_dir_fallback_message.clone(),
            legacy_data_dir,
            legacy_database_detected,
        })
    })
    .await
}

#[tauri::command]
pub(super) async fn list_gallery_images_page_command(
    app: tauri::AppHandle,
    scope: String,
    folder_id: Option<i64>,
    unclassified_only_parent_folder_id: Option<i64>,
    offset: i64,
    limit: i64,
) -> Result<GalleryImagePage, String> {
    off_main(app, move |state| {
        list_gallery_images_page(
            scope,
            folder_id,
            unclassified_only_parent_folder_id,
            offset,
            limit,
            &state,
        )
    })
    .await
}

#[tauri::command]
pub(super) async fn list_gallery_image_ids_for_scope_command(
    app: tauri::AppHandle,
    scope: String,
    folder_id: Option<i64>,
    unclassified_only_parent_folder_id: Option<i64>,
) -> Result<Vec<String>, String> {
    off_main(app, move |state| {
        list_gallery_image_ids_for_scope(
            scope,
            folder_id,
            unclassified_only_parent_folder_id,
            &state,
        )
    })
    .await
}

#[tauri::command]
pub(super) async fn list_gallery_images_by_ids_page_command(
    app: tauri::AppHandle,
    image_ids: Vec<String>,
    offset: i64,
    limit: i64,
) -> Result<GalleryImagePage, String> {
    off_main(app, move |state| {
        list_gallery_images_by_ids_page(image_ids, offset, limit, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn dev_create_fake_gallery_data_command(
    options: DevFakeGalleryOptions,
    app: tauri::AppHandle,
) -> Result<DevStressToolResult, String> {
    off_main(app, move |state| {
        dev_create_fake_gallery_data(options, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn dev_create_small_file_test_set_command(
    options: DevSmallFileSetOptions,
    app: tauri::AppHandle,
) -> Result<DevStressToolResult, String> {
    off_main(app, move |state| {
        dev_create_small_file_test_set(options, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn dev_cleanup_stress_test_data_command(
    app: tauri::AppHandle,
) -> Result<DevStressToolResult, String> {
    off_main(app, move |state| dev_cleanup_stress_test_data(&state)).await
}

#[tauri::command]
pub(super) async fn add_gallery_folder_command(
    app: tauri::AppHandle,
    folder_path: String,
) -> Result<(), String> {
    off_main_write(app, move |state| add_gallery_folder(folder_path, &state)).await
}

#[tauri::command]
pub(super) async fn remove_gallery_folder_command(
    folder_path: String,
    app: tauri::AppHandle,
) -> Result<(), String> {
    off_main_write(app, move |state| remove_gallery_folder(folder_path, &state)).await
}

#[tauri::command]
pub(super) async fn remove_image_from_index_command(
    image_id: String,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main_write(app, move |state| remove_image_from_index(image_id, &state)).await
}

#[tauri::command]
pub(super) async fn restore_image_from_trash_command(
    image_id: String,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main_write(app, move |state| restore_image_from_trash(image_id, &state)).await
}

#[tauri::command]
pub(super) async fn move_image_to_system_trash_command(
    image_id: String,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main_write(app, move |state| {
        move_image_to_system_trash(image_id, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn set_images_favorite_command(
    image_ids: Vec<String>,
    favorite: bool,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main_write(app, move |state| {
        set_images_favorite(image_ids, favorite, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn remove_images_from_index_command(
    image_ids: Vec<String>,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main_write(app, move |state| {
        remove_images_from_index(image_ids, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn restore_images_from_trash_command(
    image_ids: Vec<String>,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main_write(app, move |state| {
        restore_images_from_trash(image_ids, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn move_images_to_system_trash_command(
    image_ids: Vec<String>,
    app: tauri::AppHandle,
) -> Result<BatchSystemTrashResult, String> {
    off_main_write(app, move |state| {
        move_images_to_system_trash(image_ids, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn toggle_image_favorite_command(
    image_id: String,
    favorite: bool,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main_write(app, move |state| {
        toggle_image_favorite(image_id, favorite, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn read_image_bytes_command(
    image_id: String,
    app: tauri::AppHandle,
) -> Result<ImageBytes, String> {
    off_main(app, move |state| read_image_bytes(image_id, &state)).await
}

#[tauri::command]
pub(super) async fn copy_image_to_system_clipboard_command(
    image_id: String,
    app: tauri::AppHandle,
) -> Result<(), String> {
    off_main(app, move |state| {
        copy_image_to_system_clipboard(image_id, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn test_wd_swinv2_tagger_command(
    image_id: String,
    general_threshold: Option<f32>,
    character_threshold: Option<f32>,
    model_dir: Option<String>,
    app: tauri::AppHandle,
) -> Result<WdTaggerTestResult, String> {
    off_main(app, move |state| {
        test_wd_swinv2_tagger(
            image_id,
            general_threshold,
            character_threshold,
            model_dir,
            &state,
        )
    })
    .await
}

#[tauri::command]
pub(super) fn start_scan_all_folders_with_tagging_command(
    state: State<AppState>,
) -> Result<bool, String> {
    start_scan_all_folders_with_tagging(&state)
}

#[tauri::command]
pub(super) fn start_scan_all_folders_collect_only_command(
    state: State<AppState>,
) -> Result<bool, String> {
    start_scan_all_folders_collect_only(&state)
}

#[tauri::command]
pub(super) fn start_tag_pending_images_only_command(
    state: State<AppState>,
) -> Result<bool, String> {
    start_tag_pending_images_only(&state)
}

#[tauri::command]
pub(super) fn background_scan_status_command(
    state: State<AppState>,
) -> Result<BackgroundScanStatus, String> {
    background_scan_status(&state)
}

#[tauri::command]
pub(super) fn background_scan_progress_command(
    state: State<AppState>,
) -> Result<BackgroundScanProgress, String> {
    background_scan_progress(&state)
}

#[tauri::command]
pub(super) fn pause_background_scan_command(state: State<AppState>) -> Result<bool, String> {
    pause_background_scan(&state)
}

#[tauri::command]
pub(super) fn resume_background_scan_command(state: State<AppState>) -> Result<bool, String> {
    resume_background_scan(&state)
}

#[tauri::command]
pub(super) fn stop_background_scan_command(state: State<AppState>) -> Result<bool, String> {
    stop_background_scan(&state)
}

#[tauri::command]
pub(super) async fn start_natural_language_scan_command(
    app: tauri::AppHandle,
) -> Result<bool, String> {
    // 缓存改为惰性加载后，首次启动可能触发 1.66GB 全量读；放到后台线程避免主线程卡顿。
    off_main(app, |state| start_natural_language_scan(&state)).await
}

#[tauri::command]
pub(super) fn natural_language_scan_status_command(
    state: State<AppState>,
) -> Result<NaturalLanguageScanStatus, String> {
    natural_language_scan_status(&state)
}

#[tauri::command]
pub(super) fn natural_language_scan_progress_command(
    state: State<AppState>,
) -> Result<NaturalLanguageScanProgress, String> {
    natural_language_scan_progress(&state)
}

#[tauri::command]
pub(super) fn pause_natural_language_scan_command(state: State<AppState>) -> Result<bool, String> {
    pause_natural_language_scan(&state)
}

#[tauri::command]
pub(super) fn resume_natural_language_scan_command(state: State<AppState>) -> Result<bool, String> {
    resume_natural_language_scan(&state)
}

#[tauri::command]
pub(super) fn stop_natural_language_scan_command(state: State<AppState>) -> Result<bool, String> {
    stop_natural_language_scan(&state)
}

#[tauri::command]
pub(super) fn start_startup_cleanup_command(state: State<AppState>) -> Result<bool, String> {
    start_startup_cleanup(&state)
}

#[tauri::command]
pub(super) fn startup_cleanup_status_command(
    state: State<AppState>,
) -> Result<StartupCleanupStatus, String> {
    startup_cleanup_status(&state)
}

#[tauri::command]
pub(super) fn start_thumbnail_generation_command(state: State<AppState>) -> Result<bool, String> {
    start_thumbnail_generation(&state)
}

#[tauri::command]
pub(super) fn thumbnail_generation_status_command(
    state: State<AppState>,
) -> Result<ThumbnailGenerationProgress, String> {
    thumbnail_generation_status(&state)
}

#[tauri::command]
pub(super) fn pause_thumbnail_generation_command(state: State<AppState>) -> Result<bool, String> {
    pause_thumbnail_generation(&state)
}

#[tauri::command]
pub(super) fn resume_thumbnail_generation_command(state: State<AppState>) -> Result<bool, String> {
    resume_thumbnail_generation(&state)
}

#[tauri::command]
pub(super) fn stop_thumbnail_generation_command(state: State<AppState>) -> Result<bool, String> {
    stop_thumbnail_generation(&state)
}

#[tauri::command]
pub(super) async fn clear_thumbnail_cache_command(app: tauri::AppHandle) -> Result<(), String> {
    off_main_write(app, move |state| clear_thumbnail_cache(&state)).await
}

#[tauri::command]
pub(super) async fn rebuild_thumbnail_cache_command(app: tauri::AppHandle) -> Result<bool, String> {
    off_main(app, move |state| rebuild_thumbnail_cache(&state)).await
}

#[tauri::command]
pub(super) async fn list_image_auto_tags_command(
    image_id: String,
    app: tauri::AppHandle,
) -> Result<ImageAutoTagSummary, String> {
    off_main(app, move |state| list_image_auto_tags(image_id, &state)).await
}

#[tauri::command]
pub(super) async fn list_image_user_tags_command(
    image_id: String,
    app: tauri::AppHandle,
) -> Result<ImageUserTagSummary, String> {
    off_main(app, move |state| list_image_user_tags(image_id, &state)).await
}

#[tauri::command]
pub(super) async fn add_image_user_custom_tag_command(
    image_id: String,
    tag_text: String,
    app: tauri::AppHandle,
) -> Result<ImageUserTagSummary, String> {
    off_main_write(app, move |state| {
        add_image_user_custom_tag(image_id, tag_text, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn remove_image_user_custom_tag_command(
    image_id: String,
    tag_text: String,
    app: tauri::AppHandle,
) -> Result<ImageUserTagSummary, String> {
    off_main_write(app, move |state| {
        remove_image_user_custom_tag(image_id, tag_text, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn add_image_user_supplement_tag_command(
    image_id: String,
    tag_en: String,
    tag_zh: Option<String>,
    app: tauri::AppHandle,
) -> Result<ImageUserTagSummary, String> {
    off_main_write(app, move |state| {
        add_image_user_supplement_tag(image_id, tag_en, tag_zh, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn remove_image_user_supplement_tag_command(
    image_id: String,
    tag_en: String,
    app: tauri::AppHandle,
) -> Result<ImageUserTagSummary, String> {
    off_main_write(app, move |state| {
        remove_image_user_supplement_tag(image_id, tag_en, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn list_tag_management_state_command(
    app: tauri::AppHandle,
) -> Result<TagManagementState, String> {
    off_main(app, move |state| list_tag_management_state(&state)).await
}

#[tauri::command]
pub(super) async fn list_tag_dictionary_browser_command(
    app: tauri::AppHandle,
) -> Result<TagDictionaryBrowserState, String> {
    off_main(app, move |state| list_tag_dictionary_browser(&state)).await
}

#[tauri::command]
pub(super) async fn export_data_migration_backup_command(
    destination_path: String,
    frontend_settings: Option<serde_json::Value>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    off_main(app, move |state| {
        export_data_migration_backup(destination_path, frontend_settings, &state)
    })
    .await
}

#[tauri::command]
pub(super) fn inspect_data_migration_backup_command(
    backup_path: String,
) -> Result<DataMigrationBackupInspection, String> {
    inspect_data_migration_backup(backup_path)
}

#[tauri::command]
pub(super) async fn import_data_migration_backup_command(
    backup_path: String,
    path_mappings: Vec<BackupPathMapping>,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main_write(app, move |state| {
        import_data_migration_backup(backup_path, path_mappings, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn export_organized_folder_result_command(
    destination_dir: String,
    app: tauri::AppHandle,
) -> Result<OrganizedExportReport, String> {
    off_main(app, move |state| {
        export_organized_folder_result(destination_dir, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn create_user_tag_folder_command(
    name: String,
    app: tauri::AppHandle,
) -> Result<TagManagementState, String> {
    off_main_write(app, move |state| create_user_tag_folder(name, &state)).await
}

#[tauri::command]
pub(super) async fn create_user_custom_tag_command(
    tag_text: String,
    app: tauri::AppHandle,
) -> Result<TagManagementState, String> {
    off_main_write(app, move |state| create_user_custom_tag(tag_text, &state)).await
}

#[tauri::command]
pub(super) async fn delete_user_custom_tag_command(
    tag_text: String,
    app: tauri::AppHandle,
) -> Result<TagManagementState, String> {
    off_main_write(app, move |state| delete_user_custom_tag(tag_text, &state)).await
}

#[tauri::command]
pub(super) async fn rename_user_tag_folder_command(
    folder_id: i64,
    name: String,
    app: tauri::AppHandle,
) -> Result<TagManagementState, String> {
    off_main_write(app, move |state| {
        rename_user_tag_folder(folder_id, name, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn delete_user_tag_folder_command(
    folder_id: i64,
    app: tauri::AppHandle,
) -> Result<TagManagementState, String> {
    off_main_write(app, move |state| delete_user_tag_folder(folder_id, &state)).await
}

#[tauri::command]
pub(super) async fn assign_user_tag_to_folder_command(
    folder_id: i64,
    tag_text: String,
    app: tauri::AppHandle,
) -> Result<TagManagementState, String> {
    off_main_write(app, move |state| {
        assign_user_tag_to_folder(folder_id, tag_text, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn unassign_user_tag_from_folder_command(
    tag_text: String,
    app: tauri::AppHandle,
) -> Result<TagManagementState, String> {
    off_main_write(app, move |state| {
        unassign_user_tag_from_folder(tag_text, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn suggest_known_auto_tags_command(
    query: String,
    limit: Option<i64>,
    include_user_custom: Option<bool>,
    include_dictionary: Option<bool>,
    app: tauri::AppHandle,
) -> Result<Vec<KnownAutoTagSuggestion>, String> {
    off_main(app, move |state| {
        suggest_known_auto_tags(
            query,
            limit,
            include_user_custom,
            include_dictionary,
            &state,
        )
    })
    .await
}

#[tauri::command]
pub(super) async fn search_gallery_image_ids_command(
    app: tauri::AppHandle,
    filters: GallerySearchFilters,
) -> Result<Vec<String>, String> {
    off_main(app, move |state| search_gallery_image_ids(filters, &state)).await
}

#[tauri::command]
pub(super) async fn search_gallery_image_ids_by_natural_language_command(
    app: tauri::AppHandle,
    query: String,
    candidate_image_ids: Option<Vec<String>>,
) -> Result<Vec<String>, String> {
    off_main(app, move |state| {
        search_gallery_image_ids_by_natural_language(query, candidate_image_ids, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn create_user_folder_command(
    parent_id: Option<i64>,
    name: String,
    app: tauri::AppHandle,
) -> Result<(), String> {
    off_main_write(app, move |state| {
        create_user_folder(parent_id, name, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn rename_user_folder_command(
    folder_id: i64,
    name: String,
    app: tauri::AppHandle,
) -> Result<(), String> {
    off_main_write(app, move |state| {
        rename_user_folder(folder_id, name, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn delete_user_folder_command(
    folder_id: i64,
    app: tauri::AppHandle,
) -> Result<(), String> {
    off_main_write(app, move |state| delete_user_folder(folder_id, &state)).await
}

#[tauri::command]
pub(super) async fn reorder_user_folder_command(
    folder_id: i64,
    target_folder_id: i64,
    app: tauri::AppHandle,
) -> Result<(), String> {
    off_main_write(app, move |state| {
        reorder_user_folder(folder_id, target_folder_id, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn get_user_folder_rule_command(
    folder_id: i64,
    app: tauri::AppHandle,
) -> Result<Option<UserFolderRule>, String> {
    off_main(app, move |state| get_user_folder_rule(folder_id, &state)).await
}

#[tauri::command]
pub(super) async fn save_user_folder_rule_command(
    folder_id: i64,
    conditions: Vec<UserFolderRuleCondition>,
    apply_now: bool,
    app: tauri::AppHandle,
) -> Result<(), String> {
    off_main_write(app, move |state| {
        save_user_folder_rule(folder_id, conditions, apply_now, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn assign_image_to_user_folder_command(
    image_id: String,
    folder_id: i64,
    app: tauri::AppHandle,
) -> Result<(), String> {
    off_main_write(app, move |state| {
        assign_image_to_user_folder(image_id, folder_id, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn assign_images_to_user_folder_command(
    image_ids: Vec<String>,
    folder_id: i64,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main_write(app, move |state| {
        assign_images_to_user_folder(image_ids, folder_id, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn remove_image_from_user_folder_command(
    image_id: String,
    folder_id: i64,
    app: tauri::AppHandle,
) -> Result<(), String> {
    off_main_write(app, move |state| {
        remove_image_from_user_folder(image_id, folder_id, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn move_images_to_user_folder_command(
    image_ids: Vec<String>,
    from_folder_id: i64,
    target_folder_id: i64,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main_write(app, move |state| {
        move_images_to_user_folder(image_ids, from_folder_id, target_folder_id, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn remove_images_from_user_folder_command(
    image_ids: Vec<String>,
    folder_id: i64,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main_write(app, move |state| {
        remove_images_from_user_folder(image_ids, folder_id, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn add_images_user_tags_command(
    image_ids: Vec<String>,
    custom_tags: Vec<String>,
    supplement_tags: Vec<BatchSupplementTagInput>,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main_write(app, move |state| {
        add_images_user_tags(image_ids, custom_tags, supplement_tags, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn create_reference_board_folder_command(
    name: String,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main_write(app, move |state| {
        create_reference_board_folder(name, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn create_reference_board_command(
    folder_id: Option<i64>,
    name: String,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main_write(app, move |state| {
        create_reference_board(folder_id, name, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn add_image_to_reference_board_command(
    image_id: String,
    board_id: i64,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main_write(app, move |state| {
        add_image_to_reference_board(image_id, board_id, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn paste_image_to_reference_board_command(
    board_id: i64,
    image_bytes: Vec<u8>,
    mime_type: String,
    x: f32,
    y: f32,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main(app, move |state| {
        paste_image_to_reference_board(board_id, image_bytes, mime_type, x, y, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn paste_images_to_reference_board_command(
    board_id: i64,
    images: Vec<ReferenceBoardPasteImageInput>,
    x: f32,
    y: f32,
    stack_offset: f32,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main(app, move |state| {
        paste_images_to_reference_board(board_id, images, x, y, stack_offset, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn duplicate_reference_board_item_command(
    item_id: i64,
    x: Option<f32>,
    y: Option<f32>,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main(app, move |state| {
        duplicate_reference_board_item(item_id, x, y, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn restore_reference_board_item_command(
    board_id: i64,
    image_id: String,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    rotation: f32,
    flip_x: bool,
    flip_y: bool,
    z_index: i64,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main_write(app, move |state| {
        restore_reference_board_item(
            board_id, image_id, x, y, width, height, rotation, flip_x, flip_y, z_index, &state,
        )
    })
    .await
}

#[tauri::command]
pub(super) async fn import_reference_board_item_to_library_command(
    item_id: i64,
    folder_id: i64,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main_write(app, move |state| {
        import_reference_board_item_to_library(item_id, folder_id, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn export_reference_board_item_command(
    item_id: i64,
    destination: String,
    app: tauri::AppHandle,
) -> Result<(), String> {
    off_main(app, move |state| {
        export_reference_board_item_from_state(item_id, destination, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn export_gallery_image_command(
    image_id: String,
    destination: String,
    app: tauri::AppHandle,
) -> Result<(), String> {
    off_main(app, move |state| {
        export_gallery_image_from_state(image_id, destination, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn rename_reference_board_command(
    board_id: i64,
    name: String,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main_write(app, move |state| {
        rename_reference_board(board_id, name, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn rename_reference_board_folder_command(
    folder_id: i64,
    name: String,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main_write(app, move |state| {
        rename_reference_board_folder(folder_id, name, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn reorder_reference_board_folder_command(
    folder_id: i64,
    target_folder_id: i64,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main_write(app, move |state| {
        reorder_reference_board_folder(folder_id, target_folder_id, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn move_reference_board_to_folder_command(
    board_id: i64,
    folder_id: Option<i64>,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main_write(app, move |state| {
        move_reference_board_to_folder(board_id, folder_id, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn reorder_reference_board_command(
    board_id: i64,
    target_board_id: i64,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main_write(app, move |state| {
        reorder_reference_board(board_id, target_board_id, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn delete_reference_board_command(
    board_id: i64,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main_write(app, move |state| delete_reference_board(board_id, &state)).await
}

#[tauri::command]
pub(super) async fn delete_reference_board_folder_command(
    folder_id: i64,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main_write(app, move |state| {
        delete_reference_board_folder(folder_id, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn remove_reference_board_item_command(
    item_id: i64,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main_write(app, move |state| {
        remove_reference_board_item(item_id, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn update_reference_board_item_layout_command(
    item_id: i64,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    rotation: f32,
    flip_x: bool,
    flip_y: bool,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main(app, move |state| {
        update_reference_board_item_layout(
            item_id, x, y, width, height, rotation, flip_x, flip_y, &state,
        )
    })
    .await
}

#[tauri::command]
pub(super) async fn bring_reference_board_item_to_front_command(
    item_id: i64,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main(app, move |state| {
        bring_reference_board_item_to_front(item_id, &state)
    })
    .await
}

#[tauri::command]
pub(super) async fn auto_arrange_reference_board_command(
    board_id: i64,
    app: tauri::AppHandle,
) -> Result<LibraryStore, String> {
    off_main(app, move |state| {
        auto_arrange_reference_board(board_id, &state)
    })
    .await
}

#[tauri::command]
pub(super) fn window_minimize_command(window: tauri::Window) -> Result<(), String> {
    window
        .minimize()
        .map_err(|error| format!("最小化窗口失败：{error}"))
}

#[tauri::command]
pub(super) fn window_toggle_maximize_command(window: tauri::Window) -> Result<bool, String> {
    let maximized = window
        .is_maximized()
        .map_err(|error| format!("读取窗口最大化状态失败：{error}"))?;
    if maximized {
        window
            .unmaximize()
            .map_err(|error| format!("还原窗口失败：{error}"))?;
    } else {
        window
            .maximize()
            .map_err(|error| format!("最大化窗口失败：{error}"))?;
    }
    window
        .is_maximized()
        .map_err(|error| format!("更新窗口最大化状态失败：{error}"))
}

#[tauri::command]
pub(super) fn window_is_maximized_command(window: tauri::Window) -> Result<bool, String> {
    window
        .is_maximized()
        .map_err(|error| format!("读取窗口最大化状态失败：{error}"))
}

#[tauri::command]
pub(super) fn window_toggle_always_on_top_command(window: tauri::Window) -> Result<bool, String> {
    let always_on_top = window
        .is_always_on_top()
        .map_err(|error| format!("读取窗口置顶状态失败：{error}"))?;
    let next = !always_on_top;
    window
        .set_always_on_top(next)
        .map_err(|error| format!("更新窗口置顶状态失败：{error}"))?;
    window
        .is_always_on_top()
        .map_err(|error| format!("验证窗口置顶状态失败：{error}"))
}

#[tauri::command]
pub(super) fn window_is_always_on_top_command(window: tauri::Window) -> Result<bool, String> {
    window
        .is_always_on_top()
        .map_err(|error| format!("读取窗口置顶状态失败：{error}"))
}

#[tauri::command]
pub(super) async fn get_navigation(
    app: tauri::AppHandle,
) -> Result<illutag_core::library::NavigationMetadata, illutag_core::library::AppError> {
    off_main(app, |state| illutag_core::library::get_navigation(&state))
        .await
        .map_err(Into::into)
}
#[tauri::command]
pub(super) async fn get_bootstrap(
    app: tauri::AppHandle,
) -> Result<illutag_core::library::Bootstrap, illutag_core::library::AppError> {
    off_main(app, |state| illutag_core::library::get_bootstrap(&state))
        .await
        .map_err(Into::into)
}
#[tauri::command]
pub(super) async fn query_gallery(
    app: tauri::AppHandle,
    query: illutag_core::library::GalleryQuery,
) -> Result<illutag_core::library::QueryPage, illutag_core::library::AppError> {
    off_main(app, |state| {
        illutag_core::library::query_gallery(query, &state)
    })
    .await
    .map_err(Into::into)
}
#[tauri::command]
pub(super) async fn get_image_detail(
    app: tauri::AppHandle,
    image_id: String,
) -> Result<illutag_core::library::ImageDetail, illutag_core::library::AppError> {
    off_main(app, |state| {
        illutag_core::library::get_image_detail(image_id, &state)
    })
    .await
    .map_err(Into::into)
}
#[tauri::command]
pub(super) async fn mutate_images(
    app: tauri::AppHandle,
    request: illutag_core::library::ImageMutation,
) -> Result<illutag_core::library::MutationResult, illutag_core::library::AppError> {
    off_main(app, |state| {
        illutag_core::library::mutate_images(request, &state)
    })
    .await
    .map_err(Into::into)
}
#[tauri::command]
pub(super) async fn start_job(
    app: tauri::AppHandle,
    request: illutag_core::jobs::JobRequest,
) -> Result<illutag_core::jobs::JobRecord, illutag_core::library::AppError> {
    let handle = app.clone();
    off_main(app, move |state| {
        illutag_core::jobs::start_job(request, &state, handle)
    })
    .await
    .map_err(Into::into)
}
#[tauri::command]
pub(super) async fn control_job(
    app: tauri::AppHandle,
    id: String,
    action: String,
) -> Result<(), illutag_core::library::AppError> {
    let handle = app.clone();
    off_main(app, move |state| {
        illutag_core::jobs::control_job(id, action, &state, handle)
    })
    .await
    .map_err(Into::into)
}
#[tauri::command]
pub(super) fn list_jobs(
    state: State<AppState>,
) -> Result<Vec<illutag_core::jobs::JobRecord>, illutag_core::library::AppError> {
    state.jobs.list().map_err(Into::into)
}

pub(super) async fn off_main_write<T, F>(app: tauri::AppHandle, work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(AppState) -> Result<T, String> + Send + 'static,
{
    off_main(app, move |state| {
        let _guard = state.write_lock.lock().map_err(|e| e.to_string())?;
        if state.read_only || state.maintenance.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("此图库只读或维护中，请重启后继续".into());
        }
        work(state.clone())
    })
    .await
}
#[tauri::command]
pub(super) async fn create_restore_package(
    app: tauri::AppHandle,
    destination: String,
    frontend_settings: serde_json::Value,
) -> Result<String, illutag_core::library::AppError> {
    off_main(app, move |state| {
        illutag_core::library::create_restore_package(destination, frontend_settings, &state)
    })
    .await
    .map_err(Into::into)
}
#[tauri::command]
pub(super) async fn inspect_restore_package(
    app: tauri::AppHandle,
    package_dir: String,
) -> Result<illutag_core::library::RestoreManifest, illutag_core::library::AppError> {
    off_main(app, move |_| {
        illutag_core::library::inspect_restore_package(package_dir)
    })
    .await
    .map_err(Into::into)
}
#[tauri::command]
pub(super) async fn restore_package(
    app: tauri::AppHandle,
    package_dir: String,
    path_mappings: Vec<BackupPathMapping>,
) -> Result<illutag_core::library::RestoreManifest, illutag_core::library::AppError> {
    off_main(app, move |state| {
        illutag_core::library::restore_package(package_dir, path_mappings, &state)
    })
    .await
    .map_err(Into::into)
}

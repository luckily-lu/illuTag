//! Bounded query and mutation interfaces for the workbench.
use super::*;
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}
impl From<String> for AppError {
    fn from(detail: String) -> Self {
        eprintln!("[operation-error] {detail}");
        let (code, message, retryable) = if detail.contains("disk is full") {
            ("diskFull", "磁盘空间不足，请释放空间后重试".into(), true)
        } else if detail.contains("database is locked") {
            (
                "databaseBusy",
                "数据库正在处理其他操作，请稍后重试".into(),
                true,
            )
        } else if detail.contains("超时") {
            (
                "modelTimeout",
                "模型响应超时，可以重新运行任务".into(),
                true,
            )
        } else if detail.contains("Cannot find") || detail.contains("ONNX not found") {
            (
                "missingModel",
                "模型资源缺失，请检查便携程序包".into(),
                false,
            )
        } else if detail.starts_with("Failed") {
            (
                "operationFailed",
                "操作未完成，请查看日志中的原因".into(),
                false,
            )
        } else {
            ("operationFailed", detail, false)
        };
        Self {
            code: code.into(),
            message,
            retryable,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct GalleryQuery {
    pub scope: String,
    pub folder_id: Option<i64>,
    pub unclassified_only_parent_folder_id: Option<i64>,
    pub tags: Vec<String>,
    pub english_query: String,
    pub file_name: String,
    pub natural_language: String,
    pub offset: i64,
    pub limit: i64,
    pub random_seed: Option<i64>,
}
impl Default for GalleryQuery {
    fn default() -> Self {
        Self {
            scope: "all".into(),
            folder_id: None,
            unclassified_only_parent_folder_id: None,
            tags: vec![],
            english_query: String::new(),
            file_name: String::new(),
            natural_language: String::new(),
            offset: 0,
            limit: 5000,
            random_seed: None,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct QueryPage {
    pub images: Vec<GalleryImage>,
    pub total_image_count: i64,
    pub offset: i64,
    pub limit: i64,
    pub revision: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Bootstrap {
    pub folders: Vec<LibraryFolder>,
    pub user_folders: Vec<UserFolder>,
    pub page: QueryPage,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NavigationMetadata {
    pub folders: Vec<LibraryFolder>,
    pub user_folders: Vec<UserFolder>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ImageDetail {
    pub image: GalleryImage,
    pub folder_ids: Vec<i64>,
    pub auto_tags: ImageAutoTagSummary,
    pub user_tags: ImageUserTagSummary,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ItemFailure {
    pub image_id: String,
    pub message: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MutationResult {
    pub images: Vec<GalleryImage>,
    pub successful_ids: Vec<String>,
    pub failures: Vec<ItemFailure>,
    pub revision: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ImageMutation {
    pub action: String,
    pub image_ids: Vec<String>,
    pub folder_id: Option<i64>,
    pub favorite: Option<bool>,
    pub custom_tags: Vec<String>,
    pub supplement_tags: Vec<BatchSupplementTagInput>,
}

pub fn get_bootstrap(state: &AppState) -> Result<Bootstrap, String> {
    let conn = open_database(&state.database_path)?;
    Ok(Bootstrap {
        folders: load_folders(&conn)?,
        user_folders: load_user_folders(&conn)?,
        page: query_gallery(GalleryQuery::default(), state)?,
    })
}
pub fn get_navigation(state: &AppState) -> Result<NavigationMetadata, String> {
    let conn = open_database(&state.database_path)?;
    Ok(NavigationMetadata {
        folders: load_folders(&conn)?,
        user_folders: load_user_folders(&conn)?,
    })
}

/// Mutations invalidate the legacy snapshot instead of rebuilding all image
/// memberships. A bounded navigation/detail response must never become that cache.
pub(super) fn invalidate_library_metadata(state: &AppState) -> Result<i64, String> {
    *state.library.lock().map_err(|e| e.to_string())? = None;
    let mut revision = state
        .startup_cleanup_generation
        .lock()
        .map_err(|e| e.to_string())?;
    *revision += 1;
    Ok(*revision)
}

struct ForegroundGuard<'a>(&'a std::sync::atomic::AtomicUsize);
impl Drop for ForegroundGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}
pub fn query_gallery(query: GalleryQuery, state: &AppState) -> Result<QueryPage, String> {
    state
        .foreground_queries
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let _priority = ForegroundGuard(&state.foreground_queries);

    let filtered = !query.tags.is_empty()
        || !query.english_query.trim().is_empty()
        || !query.file_name.trim().is_empty();
    let semantic = !query.natural_language.trim().is_empty();
    let unrestricted_semantic = semantic
        && !filtered
        && query.scope == "all"
        && query.folder_id.is_none()
        && query.unclassified_only_parent_folder_id.is_none();
    let page = if unrestricted_semantic {
        let ids =
            search_gallery_image_ids_by_natural_language(query.natural_language, None, state)?;
        list_gallery_images_by_ids_page(ids, query.offset, query.limit, state)?
    } else if filtered || semantic || query.scope == "random" {
        // Candidate IDs stay in Rust; they never cross IPC or become reactive frontend arrays.
        let mut ids = if filtered {
            search_gallery_image_ids(
                GallerySearchFilters {
                    chinese_tag_ens: query.tags,
                    english_query: query.english_query,
                    file_name_query: query.file_name,
                    confidence_min: 0.0,
                    confidence_max: 1.0,
                    scope: Some(query.scope.clone()),
                    folder_id: query.folder_id,
                    unclassified_only_parent_folder_id: query.unclassified_only_parent_folder_id,
                },
                state,
            )?
        } else {
            list_gallery_image_ids_for_scope(
                query.scope.clone(),
                query.folder_id,
                query.unclassified_only_parent_folder_id,
                state,
            )?
        };
        if semantic {
            ids = search_gallery_image_ids_by_natural_language(
                query.natural_language,
                Some(ids),
                state,
            )?;
        } else if query.scope == "random" {
            let seed = query.random_seed.unwrap_or(0);
            ids.sort_by_cached_key(|id| {
                let mut value = seed as u64;
                for byte in id.as_bytes() {
                    value = (value ^ *byte as u64).wrapping_mul(1099511628211);
                }
                value
            });
        }
        list_gallery_images_by_ids_page(ids, query.offset, query.limit, state)?
    } else {
        list_gallery_images_page(
            query.scope,
            query.folder_id,
            query.unclassified_only_parent_folder_id,
            query.offset,
            query.limit,
            state,
        )?
    };
    Ok(QueryPage {
        images: page.images,
        total_image_count: page.total_image_count,
        offset: page.offset,
        limit: page.limit,
        revision: *state
            .startup_cleanup_generation
            .lock()
            .map_err(|e| e.to_string())?,
    })
}

pub fn get_image_detail(image_id: String, state: &AppState) -> Result<ImageDetail, String> {
    let conn = open_database(&state.database_path)?;
    let image = load_image_record(&conn, &image_id)?;
    let folder_ids = conn
        .prepare("SELECT folder_id FROM image_user_folders WHERE image_id=?1 ORDER BY folder_id")
        .map_err(|e| e.to_string())?
        .query_map([&image_id], |row| row.get(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<i64>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(ImageDetail {
        image,
        folder_ids,
        auto_tags: list_image_auto_tags(image_id.clone(), state)?,
        user_tags: list_image_user_tags(image_id, state)?,
    })
}

pub fn mutate_images(request: ImageMutation, state: &AppState) -> Result<MutationResult, String> {
    if state.read_only {
        return Err("此测试图库只允许浏览".into());
    }
    if state.maintenance.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("维护中或已切换数据，请重启应用后继续".into());
    }
    let _write = state.write_lock.lock().map_err(|e| e.to_string())?;
    let mut conn = open_database(&state.database_path)?;
    let mut successful_ids = Vec::new();
    let mut failures = Vec::new();
    for image_id in normalize_batch_image_ids(request.image_ids) {
        let result = (|| -> Result<(), String> {
            let image = load_image_record(&conn, &image_id)?;
            if request.action == "systemTrash" {
                crate::runtime::check_asset_write(Path::new(&image.path))?;
                move_file_to_system_recycle_bin(Path::new(&image.path))?;
            }
            let tx = conn.transaction().map_err(|e| e.to_string())?;
            match request.action.as_str() {
                "favorite" => {
                    tx.execute(
                        "UPDATE images SET is_favorite=?1 WHERE id=?2",
                        params![request.favorite.unwrap_or(!image.is_favorite), image_id],
                    )
                    .map_err(|e| e.to_string())?;
                }
                "trash" | "restore" => {
                    tx.execute(
                        "UPDATE images SET trashed=?1 WHERE id=?2",
                        params![request.action == "trash", image_id],
                    )
                    .map_err(|e| e.to_string())?;
                }
                "systemTrash" => {
                    tx.execute("DELETE FROM images WHERE id=?1", [&image_id])
                        .map_err(|e| e.to_string())?;
                }
                "assign" | "move" | "remove" | "assignFolder" | "moveFolder" | "removeFolder" => {
                    let folder_id = request.folder_id.ok_or("请选择虚拟文件夹")?;
                    if !user_folder_exists(&tx, folder_id)? {
                        return Err("文件夹不存在".into());
                    }
                    if ["move", "moveFolder"].contains(&request.action.as_str()) {
                        tx.execute(
                            "DELETE FROM image_user_folders WHERE image_id=?1",
                            [&image_id],
                        )
                        .map_err(|e| e.to_string())?;
                    }
                    if ["remove", "removeFolder"].contains(&request.action.as_str()) {
                        tx.execute(
                            "DELETE FROM image_user_folders WHERE image_id=?1 AND folder_id=?2",
                            params![image_id, folder_id],
                        )
                        .map_err(|e| e.to_string())?;
                    } else {
                        tx.execute("INSERT OR IGNORE INTO image_user_folders(image_id,folder_id,assigned_at) VALUES(?1,?2,?3)", params![image_id,folder_id,now_ms()]).map_err(|e| e.to_string())?;
                    }
                }
                "addTags" => {
                    for tag in &request.custom_tags {
                        let tag = tag.trim();
                        if tag.is_empty() {
                            continue;
                        }
                        upsert_user_custom_tag(&tx, tag)?;
                        tx.execute("INSERT OR IGNORE INTO image_user_custom_tags(image_id,tag_text,created_at,updated_at) VALUES(?1,?2,?3,?3)", params![image_id,tag,now_ms()]).map_err(|e| e.to_string())?;
                    }
                    for tag in &request.supplement_tags {
                        tx.execute("INSERT OR IGNORE INTO image_user_supplement_tags(image_id,tag_en,tag_zh,created_at,updated_at) VALUES(?1,?2,?3,?4,?4)", params![image_id,tag.tag_en,tag.tag_zh,now_ms()]).map_err(|e| e.to_string())?;
                    }
                    apply_matching_user_folder_rules_for_image(&tx, &image_id)?;
                }
                _ => return Err("不支持的图片操作".into()),
            }
            tx.commit().map_err(|e| e.to_string())?;
            Ok(())
        })();
        match result {
            Ok(()) => successful_ids.push(image_id),
            Err(message) => failures.push(ItemFailure { image_id, message }),
        }
    }
    let images = successful_ids
        .iter()
        .filter_map(|id| load_image_record(&conn, id).ok())
        .collect();
    let revision = invalidate_library_metadata(state)?;
    if ["trash", "restore", "systemTrash"].contains(&request.action.as_str()) {
        invalidate_all_similarity_caches(state);
    }
    Ok(MutationResult {
        images,
        successful_ids,
        failures,
        revision,
    })
}

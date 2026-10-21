use super::*;

pub fn list_library_from_state(state: &AppState) -> Result<LibraryStore, String> {
    let started = Instant::now();
    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;

    if let Some(cached) = library.as_ref() {
        eprintln!(
            "[startup-prof] list_library_from_state cache_hit=true images={} total_ms={}",
            cached.images.len(),
            started.elapsed().as_millis()
        );
        return Ok(cached.clone());
    }

    let open_started = Instant::now();
    let conn = open_database(&state.database_path)?;
    let open_ms = open_started.elapsed().as_millis();

    let load_started = Instant::now();
    let store = load_store(&conn)?;
    let load_ms = load_started.elapsed().as_millis();
    let image_count = store.images.len();
    *library = Some(store.clone());

    eprintln!(
        "[startup-prof] list_library_from_state cache_hit=false open_db_ms={} load_store_ms={} total_ms={} images={}",
        open_ms,
        load_ms,
        started.elapsed().as_millis(),
        image_count
    );

    Ok(store)
}

pub(super) fn gallery_scope_where_sql(
    alias: &'static str,
    scope: &str,
    folder_id: Option<i64>,
    unclassified_only_parent_folder_id: Option<i64>,
) -> (Cow<'static, str>, Vec<Value>) {
    let active_where =
        format!("COALESCE({alias}.source, '') <> 'reference' AND COALESCE({alias}.trashed, 0) = 0");
    let trashed_where = format!(
        "COALESCE({alias}.source, '') <> 'reference' AND COALESCE({alias}.trashed, 0) != 0"
    );
    if let Some(parent_folder_id) = unclassified_only_parent_folder_id {
        return (
            Cow::Owned(format!(
                "
                {active_where}
                AND {alias}.image_pk IN (
                  SELECT direct.image_id
                  FROM storage_image_user_folders direct
                  WHERE direct.folder_id = ?1
                )
                AND {alias}.image_pk NOT IN (
                  WITH RECURSIVE descendants(id) AS (
                    SELECT uf.id FROM user_folders uf WHERE uf.parent_id = ?1
                    UNION ALL
                    SELECT uf.id FROM user_folders uf
                    JOIN descendants d ON uf.parent_id = d.id
                  )
                  SELECT child_assignment.image_id
                  FROM storage_image_user_folders child_assignment
                  JOIN descendants d ON d.id = child_assignment.folder_id
                )
                "
            )),
            vec![Value::Integer(parent_folder_id)],
        );
    }
    if let Some(folder_id) = folder_id {
        return (
            Cow::Owned(format!(
                "
                {active_where}
                AND {alias}.image_pk IN (
                  WITH RECURSIVE folder_scope(id) AS (
                    SELECT ?1
                    UNION ALL
                    SELECT uf.id FROM user_folders uf
                    JOIN folder_scope fs ON uf.parent_id = fs.id
                  )
                  SELECT assignment.image_id
                  FROM storage_image_user_folders assignment
                  JOIN folder_scope fs ON fs.id = assignment.folder_id
                )
                "
            )),
            vec![Value::Integer(folder_id)],
        );
    }

    match scope {
        "favorites" => (
            Cow::Owned(format!("{active_where} AND COALESCE({alias}.is_favorite, 0) != 0")),
            Vec::new(),
        ),
        "trash" => (Cow::Owned(trashed_where), Vec::new()),
        "unclassified" => (
            Cow::Owned(format!(
                "{active_where} AND NOT EXISTS (SELECT 1 FROM storage_image_user_folders a WHERE a.image_id = {alias}.image_pk)"
            )),
            Vec::new(),
        ),
        _ => (Cow::Owned(active_where), Vec::new()),
    }
}

pub fn list_gallery_images_page(
    scope: String,
    folder_id: Option<i64>,
    unclassified_only_parent_folder_id: Option<i64>,
    offset: i64,
    limit: i64,
    state: &AppState,
) -> Result<GalleryImagePage, String> {
    let conn = open_database(&state.database_path)?;
    let (where_sql, values) = gallery_scope_where_sql(
        "i",
        scope.trim(),
        folder_id,
        unclassified_only_parent_folder_id,
    );

    let total_image_count =
        count_gallery_images_with_values(&conn, Some(where_sql.as_ref()), values.clone())?;
    let images = load_images_page_with_values(
        &conn,
        Some(where_sql.as_ref()),
        values,
        offset.max(0),
        limit.clamp(1, LARGE_LIBRARY_INITIAL_IMAGE_LIMIT),
    )?;
    Ok(GalleryImagePage {
        images,
        total_image_count,
        offset: offset.max(0),
        limit: limit.clamp(1, LARGE_LIBRARY_INITIAL_IMAGE_LIMIT),
    })
}

pub fn list_gallery_image_ids_for_scope(
    scope: String,
    folder_id: Option<i64>,
    unclassified_only_parent_folder_id: Option<i64>,
    state: &AppState,
) -> Result<Vec<String>, String> {
    let conn = open_database(&state.database_path)?;
    let (where_sql, values) = gallery_scope_where_sql(
        "i",
        scope.trim(),
        folder_id,
        unclassified_only_parent_folder_id,
    );
    let sql = format!(
        "
        SELECT i.id
        FROM images i
        WHERE {}
        ORDER BY i.modified_at DESC, i.path ASC
        ",
        where_sql.as_ref()
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|error| format!("Failed to prepare gallery scope id query: {error}"))?;
    let rows = stmt
        .query_map(params_from_iter(values), |row| row.get::<_, String>(0))
        .map_err(|error| format!("Failed to query gallery scope ids: {error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to collect gallery scope ids: {error}"))
}

pub fn list_gallery_images_by_ids_page(
    image_ids: Vec<String>,
    offset: i64,
    limit: i64,
    state: &AppState,
) -> Result<GalleryImagePage, String> {
    let total_image_count = image_ids.len() as i64;
    let offset = offset.max(0) as usize;
    let limit = limit.clamp(1, LARGE_LIBRARY_INITIAL_IMAGE_LIMIT) as usize;
    let page_ids = image_ids
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect::<Vec<_>>();
    if page_ids.is_empty() {
        return Ok(GalleryImagePage {
            images: Vec::new(),
            total_image_count,
            offset: offset as i64,
            limit: limit as i64,
        });
    }

    let conn = open_database(&state.database_path)?;
    let mut by_id = HashMap::<String, GalleryImage>::new();
    for chunk in page_ids.chunks(BATCH_SQL_VARIABLE_LIMIT_SAFE) {
        let placeholders = std::iter::repeat("?")
            .take(chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "
            SELECT
              i.id, i.path, t.thumb_path, i.file_name, i.ext, i.width, i.height, i.file_size, i.modified_at,
              i.imported_at, i.folder_id, i.missing, i.trashed, i.is_favorite, i.source
            FROM images i
            LEFT JOIN image_thumbnails t ON t.image_id = i.id
            WHERE i.id IN ({placeholders})
            "
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|error| format!("Failed to prepare gallery images by ids query: {error}"))?;
        let rows = stmt
            .query_map(
                params_from_iter(chunk.iter().map(|id| Value::Text(id.clone()))),
                map_gallery_image_page_row,
            )
            .map_err(|error| format!("Failed to query gallery images by ids: {error}"))?;
        for row in rows {
            let image =
                row.map_err(|error| format!("Failed to collect gallery images by ids: {error}"))?;
            by_id.insert(image.id.clone(), image);
        }
    }
    let images = page_ids
        .into_iter()
        .filter_map(|id| by_id.remove(&id))
        .collect::<Vec<_>>();
    Ok(GalleryImagePage {
        images,
        total_image_count,
        offset: offset as i64,
        limit: limit as i64,
    })
}

pub fn add_gallery_folder(folder_path: String, state: &AppState) -> Result<(), String> {
    let folder_path = normalize_folder_path(&folder_path)?;
    let scanned_at = now_ms();
    let mut conn = open_database(&state.database_path)?;

    let tx = conn
        .transaction()
        .map_err(|error| format!("打开图库事务失败：{error}"))?;
    tx.execute("INSERT INTO folders(path,added_at,last_scanned_at,hidden) VALUES(?1,?2,NULL,0) ON CONFLICT(path) DO UPDATE SET hidden=0",params![folder_path,scanned_at]).map_err(|e|e.to_string())?;
    ensure_library_root_user_folder(&tx, &folder_path, scanned_at)?;
    tx.commit()
        .map_err(|error| format!("保存图库索引失败：{error}"))?;

    invalidate_library_metadata(state)?;
    Ok(())
}

pub fn remove_gallery_folder(folder_path: String, state: &AppState) -> Result<(), String> {
    let folder_path = normalize_existing_or_stored_folder_path(&folder_path);
    let mut conn = open_database(&state.database_path)?;

    let tx = conn
        .transaction()
        .map_err(|error| format!("打开图库事务失败：{error}"))?;
    let folder_id = tx
        .query_row(
            "SELECT id FROM folders WHERE path = ?1",
            params![&folder_path],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|error| format!("查询图库文件夹失败：{error}"))?;

    if let Some(folder_id) = folder_id {
        tx.execute(
            "DELETE FROM images WHERE folder_id = ?1",
            params![folder_id],
        )
        .map_err(|error| format!("删除图片索引失败：{error}"))?;
        remove_synced_user_folder_tree_for_root(&tx, &folder_path)?;
        tx.execute("DELETE FROM folders WHERE id = ?1", params![folder_id])
            .map_err(|error| format!("删除图库文件夹失败：{error}"))?;
    }

    tx.commit()
        .map_err(|error| format!("保存图库索引失败：{error}"))?;

    invalidate_library_metadata(state)?;
    invalidate_all_similarity_caches(state);
    crate::duplicates::invalidate(state);
    Ok(())
}

pub(super) fn sync_user_folder_tree_for_library_directory(
    conn: &Connection,
    root_folder_path: &str,
    scanned_images: &[ScannedImage],
    now: i64,
) -> Result<(), String> {
    if scanned_images.is_empty() && !Path::new(root_folder_path).is_dir() {
        return Ok(());
    }

    let root_path = normalize_existing_or_stored_folder_path(root_folder_path);
    let root_prefix = if root_path.ends_with(std::path::MAIN_SEPARATOR) {
        root_path.clone()
    } else {
        format!("{root_path}{}", std::path::MAIN_SEPARATOR)
    };
    let mut directory_paths = collect_directory_tree_paths(&root_path)?;

    for image in scanned_images {
        let image_parent = Path::new(&image.path)
            .parent()
            .map(|path| normalize_existing_or_stored_folder_path(&path.to_string_lossy()))
            .unwrap_or_else(|| root_path.clone());
        if image_parent == root_path || image_parent.starts_with(&root_prefix) {
            directory_paths.insert(image_parent);
        }
    }

    if directory_paths.is_empty() {
        return Ok(());
    }

    let mut sorted_paths = directory_paths.into_iter().collect::<Vec<_>>();
    sorted_paths.sort_by(|a, b| {
        path_depth_for_sort(a)
            .cmp(&path_depth_for_sort(b))
            .then_with(|| a.cmp(b))
    });

    // 单事务包裹全部 upsert/归属写入，避免每语句自动提交带来的 fsync 风暴
    let tx = conn
        .unchecked_transaction()
        .map_err(|error| format!("Failed to open folder sync transaction: {error}"))?;

    let mut folder_id_by_path = HashMap::<String, i64>::new();
    for dir_path in sorted_paths {
        let parent_id = if dir_path == root_path {
            None
        } else {
            let parent_path = Path::new(&dir_path)
                .parent()
                .map(|path| normalize_existing_or_stored_folder_path(&path.to_string_lossy()));
            parent_path.and_then(|path| folder_id_by_path.get(&path).copied())
        };
        let name = Path::new(&dir_path)
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| dir_path.clone());
        let folder_id = upsert_synced_user_folder_for_path(&tx, &dir_path, parent_id, &name, now)?;
        folder_id_by_path.insert(dir_path, folder_id);
    }

    if !scanned_images.is_empty() {
        for image in scanned_images {
            remove_synced_folder_assignments_for_image(&tx, &image.path)?;
            let parent_path = Path::new(&image.path)
                .parent()
                .map(|path| normalize_existing_or_stored_folder_path(&path.to_string_lossy()))
                .unwrap_or_else(|| root_path.clone());
            let Some(target_folder_id) = folder_id_by_path.get(&parent_path).copied() else {
                continue;
            };
            tx.execute(
                "
                INSERT OR IGNORE INTO storage_image_user_folders (image_id, folder_id, assigned_at)
                VALUES ((SELECT image_pk FROM images WHERE id=?1), ?2, ?3)
                ",
                params![image.path, target_folder_id, now],
            )
            .map_err(|error| format!("Failed to sync image folder assignment: {error}"))?;
        }
    }

    tx.commit()
        .map_err(|error| format!("Failed to commit folder sync transaction: {error}"))?;

    Ok(())
}

pub(super) fn assign_scanned_images_to_nearest_synced_parent_folder(
    conn: &Connection,
    root_folder_path: &str,
    scanned_images: &[ScannedImage],
    now: i64,
) -> Result<(), String> {
    if scanned_images.is_empty() {
        return Ok(());
    }

    let root_path = normalize_existing_or_stored_folder_path(root_folder_path);
    let root_prefix = if root_path.ends_with(std::path::MAIN_SEPARATOR) {
        root_path.clone()
    } else {
        format!("{root_path}{}", std::path::MAIN_SEPARATOR)
    };

    let mut stmt = conn
        .prepare(
            "
            SELECT id, source_path
            FROM user_folders
            WHERE source_kind = ?1
              AND source_path IS NOT NULL
            ",
        )
        .map_err(|error| format!("Failed to load synced folders for scan assignment: {error}"))?;
    let synced_rows = stmt
        .query_map(params![USER_FOLDER_SOURCE_KIND_LIBRARY_DIR], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| format!("Failed to load synced folders for scan assignment: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to load synced folders for scan assignment: {error}"))?;
    drop(stmt);

    let mut folder_id_by_path = HashMap::<String, i64>::new();
    for (id, path) in synced_rows {
        folder_id_by_path.insert(normalize_existing_or_stored_folder_path(&path), id);
    }

    // 单事务包裹全部归属写入，避免每语句自动提交带来的 fsync 风暴
    let tx = conn
        .unchecked_transaction()
        .map_err(|error| format!("Failed to open scan assignment transaction: {error}"))?;

    for image in scanned_images {
        remove_synced_folder_assignments_for_image(&tx, &image.path)?;

        let mut current = Path::new(&image.path)
            .parent()
            .map(|path| normalize_existing_or_stored_folder_path(&path.to_string_lossy()))
            .unwrap_or_else(|| root_path.clone());

        if current != root_path && !current.starts_with(&root_prefix) {
            continue;
        }

        let mut target_folder_id = folder_id_by_path.get(&current).copied();
        while target_folder_id.is_none() && current != root_path {
            let Some(parent) = Path::new(&current)
                .parent()
                .map(|path| normalize_existing_or_stored_folder_path(&path.to_string_lossy()))
            else {
                break;
            };
            current = parent;
            target_folder_id = folder_id_by_path.get(&current).copied();
        }

        let Some(folder_id) = target_folder_id else {
            continue;
        };

        tx.execute(
            "
            INSERT OR IGNORE INTO storage_image_user_folders (image_id, folder_id, assigned_at)
            VALUES ((SELECT image_pk FROM images WHERE id=?1), ?2, ?3)
            ",
            params![image.path, folder_id, now],
        )
        .map_err(|error| format!("Failed to sync scanned image folder assignment: {error}"))?;
    }

    tx.commit()
        .map_err(|error| format!("Failed to commit scan assignment transaction: {error}"))?;

    Ok(())
}

pub(super) fn collect_directory_tree_paths(
    root_folder_path: &str,
) -> Result<HashSet<String>, String> {
    let mut paths = HashSet::<String>::new();
    let root = Path::new(root_folder_path);
    let root_prefix = if root_folder_path.ends_with(std::path::MAIN_SEPARATOR) {
        root_folder_path.to_string()
    } else {
        format!("{root_folder_path}{}", std::path::MAIN_SEPARATOR)
    };
    if !root.is_dir() {
        return Ok(paths);
    }

    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_dir() {
            continue;
        }
        let path = normalize_existing_or_stored_folder_path(&entry.path().to_string_lossy());
        if path == root_folder_path || path.starts_with(&root_prefix) {
            paths.insert(path);
        }
    }

    if paths.is_empty() {
        paths.insert(root_folder_path.to_string());
    }
    Ok(paths)
}

pub(super) fn ensure_library_root_user_folder(
    conn: &Connection,
    folder_path: &str,
    now: i64,
) -> Result<i64, String> {
    let name = Path::new(folder_path)
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| folder_path.to_string());
    upsert_synced_user_folder_for_path(conn, folder_path, None, &name, now)
}

pub(super) fn path_depth_for_sort(path_text: &str) -> usize {
    Path::new(path_text).components().count()
}

pub(super) fn upsert_synced_user_folder_for_path(
    conn: &Connection,
    source_path: &str,
    parent_id: Option<i64>,
    name: &str,
    now: i64,
) -> Result<i64, String> {
    if let Some(existing_id) = conn
        .query_row(
            "
            SELECT id
            FROM user_folders
            WHERE source_kind = ?1 AND source_path = ?2
            ",
            params![USER_FOLDER_SOURCE_KIND_LIBRARY_DIR, source_path],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|error| format!("Failed to query synced user folder: {error}"))?
    {
        conn.execute(
            "
            UPDATE user_folders
            SET parent_id = ?1, name = ?2, updated_at = ?3
            WHERE id = ?4
            ",
            params![parent_id, name, now, existing_id],
        )
        .map_err(|error| format!("Failed to update synced user folder: {error}"))?;
        return Ok(existing_id);
    }

    let sort_order = next_user_folder_sort_order(conn, parent_id)?;
    conn.execute(
        "
        INSERT INTO user_folders (
          parent_id, name, sort_order, source_kind, source_path, created_at, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
        ",
        params![
            parent_id,
            name,
            sort_order,
            USER_FOLDER_SOURCE_KIND_LIBRARY_DIR,
            source_path,
            now,
        ],
    )
    .map_err(|error| format!("Failed to create synced user folder: {error}"))?;

    conn.query_row(
        "SELECT id FROM user_folders WHERE source_path = ?1",
        params![source_path],
        |row| row.get::<_, i64>(0),
    )
    .map_err(|error| format!("Failed to load synced user folder id: {error}"))
}

pub(super) fn remove_synced_folder_assignments_for_image(
    conn: &Connection,
    image_id: &str,
) -> Result<(), String> {
    conn.execute(
        "
        DELETE FROM storage_image_user_folders
        WHERE image_id = (SELECT image_pk FROM images WHERE id=?1)
          AND folder_id IN (
            SELECT id FROM user_folders WHERE source_kind = ?2
          )
        ",
        params![image_id, USER_FOLDER_SOURCE_KIND_LIBRARY_DIR],
    )
    .map_err(|error| format!("Failed to clear synced folder assignment: {error}"))?;
    Ok(())
}

pub(super) fn remove_synced_user_folder_tree_for_root(
    conn: &Connection,
    root_path: &str,
) -> Result<(), String> {
    let root = normalize_existing_or_stored_folder_path(root_path);
    let mut ids = Vec::<i64>::new();

    let mut stmt = conn
        .prepare(
            "
            SELECT id, source_path
            FROM user_folders
            WHERE source_kind = ?1
              AND source_path IS NOT NULL
            ",
        )
        .map_err(|error| format!("Failed to load synced folders for delete: {error}"))?;
    let rows = stmt
        .query_map(params![USER_FOLDER_SOURCE_KIND_LIBRARY_DIR], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| format!("Failed to load synced folders for delete: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to load synced folders for delete: {error}"))?;
    drop(stmt);

    let prefix = if root.ends_with(std::path::MAIN_SEPARATOR) {
        root.clone()
    } else {
        format!("{root}{}", std::path::MAIN_SEPARATOR)
    };
    for (id, path) in rows {
        if path == root || path.starts_with(&prefix) {
            ids.push(id);
        }
    }

    for id in ids {
        conn.execute("DELETE FROM user_folders WHERE id = ?1", params![id])
            .map_err(|error| format!("Failed to delete synced user folder: {error}"))?;
    }
    Ok(())
}

pub fn remove_image_from_index(image_id: String, state: &AppState) -> Result<LibraryStore, String> {
    crate::duplicates::management::revoke_for_ids(std::slice::from_ref(&image_id),state)?;
    super::recycle::revoke_for_ids(std::slice::from_ref(&image_id),state)?;
    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let conn = open_database(&state.database_path)?;
    conn.execute("PRAGMA foreign_keys = ON", [])
        .map_err(|error| format!("启用数据库外键失败：{error}"))?;
    conn.execute(
        "UPDATE images SET trashed = 1 WHERE id = ?1 AND source = 'library'",
        params![image_id],
    )
    .map_err(|error| format!("Failed to move image to trash: {error}"))?;
    crate::duplicates::invalidate(state);

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    remove_clip_vector_cache_entry(&state.clip_vector_cache, &image_id)?;
    Ok(store)
}

pub fn restore_image_from_trash(
    image_id: String,
    state: &AppState,
) -> Result<LibraryStore, String> {
    crate::duplicates::management::revoke_for_ids(std::slice::from_ref(&image_id),state)?;
    super::recycle::revoke_for_ids(std::slice::from_ref(&image_id),state)?;
    let mut library = state
        .library
        .lock()
        .map_err(|_| "鍥惧簱鐘舵€佽鍗犵敤锛岃绋嶅悗鍐嶈瘯".to_string())?;
    let conn = open_database(&state.database_path)?;
    conn.execute("PRAGMA foreign_keys = ON", [])
        .map_err(|error| format!("鍚敤鏁版嵁搴撳閿け璐ワ細{error}"))?;
    conn.execute(
        "UPDATE images SET trashed = 0 WHERE id = ?1 AND source = 'library'",
        params![image_id],
    )
    .map_err(|error| format!("Failed to restore image from trash: {error}"))?;
    crate::duplicates::invalidate(state);

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    invalidate_all_similarity_caches(state);
    Ok(store)
}

pub(super) fn move_file_to_system_recycle_bin(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let normalized_path = normalize_windows_path_for_recycle_bin(path);
        let move_result = trash::delete(&normalized_path);
        if let Err(error) = move_result {
            let error_text = error.to_string();
            eprintln!(
                "[system-trash] move failed path={} status={} stdout={} stderr={} error={}",
                normalized_path.display(),
                "N/A",
                "",
                "",
                error_text
            );
            if is_permission_denied_error_text(&error_text) {
                return Err("无权限，无法移入系统回收站。".to_string());
            }
            return Err(format!("移动到系统回收站失败：{error_text}"));
        }
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        Err("Moving file to system recycle bin is only supported on Windows".to_string())
    }
}

// Share one Windows shell operation across a small, already validated batch.
pub(super) fn move_files_to_system_recycle_bin(paths: &[PathBuf]) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let paths: Vec<_> = paths.iter().map(|p| normalize_windows_path_for_recycle_bin(p)).collect();
        trash::delete_all(&paths).map_err(|error| format!("移动到系统回收站失败：{error}"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = paths;
        Err("Moving files to system recycle bin is only supported on Windows".into())
    }
}

pub(super) fn normalize_windows_path_for_recycle_bin(path: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let raw = path.to_string_lossy();
        if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = raw.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
        return path.to_path_buf();
    }
    #[cfg(not(target_os = "windows"))]
    {
        path.to_path_buf()
    }
}

pub(super) fn is_permission_denied_error_text(error_text: &str) -> bool {
    let lower = error_text.to_lowercase();
    lower.contains("permission denied")
        || lower.contains("access is denied")
        || lower.contains("operation not permitted")
        || error_text.contains("拒绝访问")
        || error_text.contains("无权限")
}

pub fn move_image_to_system_trash(
    image_id: String,
    state: &AppState,
) -> Result<LibraryStore, String> {
    crate::duplicates::management::revoke_for_ids(std::slice::from_ref(&image_id),state)?;
    super::recycle::revoke_for_ids(std::slice::from_ref(&image_id),state)?;
    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let conn = open_database(&state.database_path)?;
    conn.execute("PRAGMA foreign_keys = ON", [])
        .map_err(|error| format!("启用数据库外键失败：{error}"))?;

    let image_row = conn
        .query_row(
            "
            SELECT path, source, COALESCE(trashed, 0)
            FROM images
            WHERE id = ?1
            ",
            params![image_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|error| format!("Failed to load image before recycle-bin move: {error}"))?;
    let Some((path, source, trashed)) = image_row else {
        return Err("Image not found".to_string());
    };
    if source != "library" {
        return Err("Only library images can be moved to system recycle bin".to_string());
    }
    if trashed == 0 {
        return Err(
            "Image must be in app recycle bin before moving to system recycle bin".to_string(),
        );
    }

    let image_path = PathBuf::from(&path);
    if !image_path.exists() {
        return Err("源文件不存在，无法移入系统回收站。".to_string());
    }
    move_file_to_system_recycle_bin(&image_path)?;

    conn.execute("DELETE FROM images WHERE id = ?1", params![image_id])
        .map_err(|error| format!("Failed to remove image index after recycle-bin move: {error}"))?;
    crate::duplicates::invalidate(state);

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    remove_clip_vector_cache_entry(&state.clip_vector_cache, &image_id)?;
    Ok(store)
}

pub fn move_images_to_system_trash(
    image_ids: Vec<String>,
    state: &AppState,
) -> Result<BatchSystemTrashResult, String> {
    crate::duplicates::management::revoke_for_ids(&image_ids,state)?;
    super::recycle::revoke_for_ids(&image_ids,state)?;
    let image_ids = normalize_batch_image_ids(image_ids);
    if image_ids.is_empty() {
        return Ok(BatchSystemTrashResult {
            store: list_library_from_state(state)?,
            moved_count: 0,
            failed_image_ids: Vec::new(),
            first_error: None,
        });
    }

    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let conn = open_database(&state.database_path)?;
    conn.execute("PRAGMA foreign_keys = ON", [])
        .map_err(|error| format!("启用数据库外键失败：{error}"))?;

    let mut moved_count = 0usize;
    let mut failed_image_ids = Vec::<String>::new();
    let mut first_error = None::<String>;
    let mut removed_image_ids = Vec::<String>::new();

    for image_id in &image_ids {
        let image_row = conn
            .query_row(
                "
                SELECT path, source, COALESCE(trashed, 0)
                FROM images
                WHERE id = ?1
                ",
                params![image_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| format!("Failed to load image before recycle-bin move: {error}"))?;

        let Some((path, source, trashed)) = image_row else {
            failed_image_ids.push(image_id.clone());
            if first_error.is_none() {
                first_error = Some("Image not found".to_string());
            }
            continue;
        };
        if source != "library" {
            failed_image_ids.push(image_id.clone());
            if first_error.is_none() {
                first_error =
                    Some("Only library images can be moved to system recycle bin".to_string());
            }
            continue;
        }
        if trashed == 0 {
            failed_image_ids.push(image_id.clone());
            if first_error.is_none() {
                first_error = Some(
                    "Image must be in app recycle bin before moving to system recycle bin"
                        .to_string(),
                );
            }
            continue;
        }

        let image_path = PathBuf::from(&path);
        if !image_path.exists() {
            failed_image_ids.push(image_id.clone());
            if first_error.is_none() {
                first_error = Some("源文件不存在，无法移入系统回收站。".to_string());
            }
            continue;
        }

        if let Err(error) = move_file_to_system_recycle_bin(&image_path) {
            failed_image_ids.push(image_id.clone());
            if first_error.is_none() {
                first_error = Some(error);
            }
            continue;
        }

        conn.execute("DELETE FROM images WHERE id = ?1", params![image_id])
            .map_err(|error| {
                format!("Failed to remove image index after recycle-bin move: {error}")
            })?;
        moved_count += 1;
        removed_image_ids.push(image_id.clone());
    }

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    for image_id in &removed_image_ids {
        remove_clip_vector_cache_entry(&state.clip_vector_cache, image_id)?;
    }
    if moved_count > 0 { crate::duplicates::invalidate(state); }

    Ok(BatchSystemTrashResult {
        store,
        moved_count,
        failed_image_ids,
        first_error,
    })
}

pub fn toggle_image_favorite(
    image_id: String,
    favorite: bool,
    state: &AppState,
) -> Result<LibraryStore, String> {
    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let conn = open_database(&state.database_path)?;
    conn.execute("PRAGMA foreign_keys = ON", [])
        .map_err(|error| format!("启用数据库外键失败：{error}"))?;
    conn.execute(
        "UPDATE images SET is_favorite = ?1 WHERE id = ?2 AND source = 'library'",
        params![if favorite { 1 } else { 0 }, image_id],
    )
    .map_err(|error| format!("Failed to update image favorite state: {error}"))?;

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    Ok(store)
}

pub(super) fn normalize_batch_image_ids(image_ids: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::<String>::new();
    let mut normalized = Vec::<String>::new();
    for image_id in image_ids {
        let trimmed = image_id.trim();
        if trimmed.is_empty() {
            continue;
        }
        let owned = trimmed.to_string();
        if seen.insert(owned.clone()) {
            normalized.push(owned);
        }
    }
    normalized
}

pub(super) fn for_each_image_id_chunk<F>(image_ids: &[String], mut handler: F) -> Result<(), String>
where
    F: FnMut(&[String]) -> Result<(), String>,
{
    for chunk in image_ids.chunks(BATCH_SQL_VARIABLE_LIMIT_SAFE) {
        handler(chunk)?;
    }
    Ok(())
}

pub(super) fn build_in_placeholders(count: usize) -> String {
    std::iter::repeat("?")
        .take(count)
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn set_images_favorite(
    image_ids: Vec<String>,
    favorite: bool,
    state: &AppState,
) -> Result<LibraryStore, String> {
    let image_ids = normalize_batch_image_ids(image_ids);
    if image_ids.is_empty() {
        return list_library_from_state(state);
    }

    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let mut conn = open_database(&state.database_path)?;
    conn.execute("PRAGMA foreign_keys = ON", [])
        .map_err(|error| format!("启用数据库外键失败：{error}"))?;
    let tx = conn
        .transaction()
        .map_err(|error| format!("Failed to start batch favorite transaction: {error}"))?;

    for_each_image_id_chunk(&image_ids, |chunk| {
        let placeholders = build_in_placeholders(chunk.len());
        let sql = format!(
            "
            UPDATE images
            SET is_favorite = ?1
            WHERE source = 'library'
              AND id IN ({placeholders})
            "
        );
        let mut params_values = Vec::<Value>::with_capacity(1 + chunk.len());
        params_values.push(Value::Integer(if favorite { 1 } else { 0 }));
        for image_id in chunk {
            params_values.push(Value::Text(image_id.clone()));
        }
        tx.execute(&sql, params_from_iter(params_values.iter()))
            .map_err(|error| format!("Failed to update image favorite state in batch: {error}"))?;
        Ok(())
    })?;
    tx.commit()
        .map_err(|error| format!("Failed to commit batch favorite transaction: {error}"))?;

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    Ok(store)
}

pub fn remove_images_from_index(
    image_ids: Vec<String>,
    state: &AppState,
) -> Result<LibraryStore, String> {
    crate::duplicates::management::revoke_for_ids(&image_ids,state)?;
    super::recycle::revoke_for_ids(&image_ids,state)?;
    let image_ids = normalize_batch_image_ids(image_ids);
    if image_ids.is_empty() {
        return list_library_from_state(state);
    }

    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let mut conn = open_database(&state.database_path)?;
    conn.execute("PRAGMA foreign_keys = ON", [])
        .map_err(|error| format!("启用数据库外键失败：{error}"))?;
    let tx = conn
        .transaction()
        .map_err(|error| format!("Failed to start batch trash transaction: {error}"))?;

    for_each_image_id_chunk(&image_ids, |chunk| {
        let placeholders = build_in_placeholders(chunk.len());
        let sql = format!(
            "
            UPDATE images
            SET trashed = 1
            WHERE source = 'library'
              AND id IN ({placeholders})
            "
        );
        let params_values: Vec<Value> = chunk
            .iter()
            .map(|image_id| Value::Text(image_id.clone()))
            .collect();
        tx.execute(&sql, params_from_iter(params_values.iter()))
            .map_err(|error| format!("Failed to move images to trash in batch: {error}"))?;
        Ok(())
    })?;
    tx.commit()
        .map_err(|error| format!("Failed to commit batch trash transaction: {error}"))?;
    crate::duplicates::invalidate(state);

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    for image_id in &image_ids {
        remove_clip_vector_cache_entry(&state.clip_vector_cache, image_id)?;
    }
    Ok(store)
}

pub fn restore_images_from_trash(
    image_ids: Vec<String>,
    state: &AppState,
) -> Result<LibraryStore, String> {
    crate::duplicates::management::revoke_for_ids(&image_ids,state)?;
    super::recycle::revoke_for_ids(&image_ids,state)?;
    let image_ids = normalize_batch_image_ids(image_ids);
    if image_ids.is_empty() {
        return list_library_from_state(state);
    }

    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let mut conn = open_database(&state.database_path)?;
    conn.execute("PRAGMA foreign_keys = ON", [])
        .map_err(|error| format!("启用数据库外键失败：{error}"))?;
    let tx = conn
        .transaction()
        .map_err(|error| format!("Failed to start batch restore transaction: {error}"))?;

    for_each_image_id_chunk(&image_ids, |chunk| {
        let placeholders = build_in_placeholders(chunk.len());
        let sql = format!(
            "
            UPDATE images
            SET trashed = 0
            WHERE source = 'library'
              AND id IN ({placeholders})
            "
        );
        let params_values: Vec<Value> = chunk
            .iter()
            .map(|image_id| Value::Text(image_id.clone()))
            .collect();
        tx.execute(&sql, params_from_iter(params_values.iter()))
            .map_err(|error| format!("Failed to restore images from trash in batch: {error}"))?;
        Ok(())
    })?;
    tx.commit()
        .map_err(|error| format!("Failed to commit batch restore transaction: {error}"))?;
    crate::duplicates::invalidate(state);

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    invalidate_all_similarity_caches(state);
    Ok(store)
}

pub fn assign_images_to_user_folder(
    image_ids: Vec<String>,
    folder_id: i64,
    state: &AppState,
) -> Result<LibraryStore, String> {
    let image_ids = normalize_batch_image_ids(image_ids);
    if image_ids.is_empty() {
        return list_library_from_state(state);
    }

    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let mut conn = open_database(&state.database_path)?;
    conn.execute("PRAGMA foreign_keys = ON", [])
        .map_err(|error| format!("启用数据库外键失败：{error}"))?;

    if !user_folder_is_leaf(&conn, folder_id)? {
        return Err("只能将图片放入最小层级文件夹".to_string());
    }

    let tx = conn
        .transaction()
        .map_err(|error| format!("Failed to start batch assign transaction: {error}"))?;
    let now = now_ms();
    for image_id in &image_ids {
        tx.execute(
            "
            INSERT OR IGNORE INTO storage_image_user_folders (image_id, folder_id, assigned_at)
            VALUES ((SELECT image_pk FROM images WHERE id=?1), ?2, ?3)
            ",
            params![image_id, folder_id, now],
        )
        .map_err(|error| format!("Failed to assign image to folder in batch: {error}"))?;
    }
    tx.commit()
        .map_err(|error| format!("Failed to commit batch assign transaction: {error}"))?;

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    Ok(store)
}

pub fn move_images_to_user_folder(
    image_ids: Vec<String>,
    from_folder_id: i64,
    target_folder_id: i64,
    state: &AppState,
) -> Result<LibraryStore, String> {
    let image_ids = normalize_batch_image_ids(image_ids);
    if image_ids.is_empty() {
        return list_library_from_state(state);
    }

    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let mut conn = open_database(&state.database_path)?;
    conn.execute("PRAGMA foreign_keys = ON", [])
        .map_err(|error| format!("启用数据库外键失败：{error}"))?;

    if !user_folder_is_leaf(&conn, target_folder_id)? {
        return Err("只能将图片放入最小层级文件夹".to_string());
    }

    let tx = conn
        .transaction()
        .map_err(|error| format!("Failed to start batch move transaction: {error}"))?;
    let now = now_ms();
    for image_id in &image_ids {
        tx.execute(
            "
            INSERT OR IGNORE INTO storage_image_user_folders (image_id, folder_id, assigned_at)
            VALUES ((SELECT image_pk FROM images WHERE id=?1), ?2, ?3)
            ",
            params![image_id, target_folder_id, now],
        )
        .map_err(|error| format!("Failed to assign image during batch move: {error}"))?;
    }

    if from_folder_id != target_folder_id {
        for image_id in &image_ids {
            tx.execute(
                "
                DELETE FROM storage_image_user_folders
                WHERE image_id = (SELECT image_pk FROM images WHERE id=?1) AND folder_id = ?2
                ",
                params![image_id, from_folder_id],
            )
            .map_err(|error| {
                format!("Failed to remove image from source folder in batch move: {error}")
            })?;
        }
    }

    tx.commit()
        .map_err(|error| format!("Failed to commit batch move transaction: {error}"))?;

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    Ok(store)
}

pub fn remove_images_from_user_folder(
    image_ids: Vec<String>,
    folder_id: i64,
    state: &AppState,
) -> Result<LibraryStore, String> {
    let image_ids = normalize_batch_image_ids(image_ids);
    if image_ids.is_empty() {
        return list_library_from_state(state);
    }

    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let mut conn = open_database(&state.database_path)?;
    conn.execute("PRAGMA foreign_keys = ON", [])
        .map_err(|error| format!("启用数据库外键失败：{error}"))?;
    let tx = conn
        .transaction()
        .map_err(|error| format!("Failed to start batch remove-folder transaction: {error}"))?;

    for_each_image_id_chunk(&image_ids, |chunk| {
        let placeholders = build_in_placeholders(chunk.len());
        let sql = format!(
            "
            DELETE FROM storage_image_user_folders
            WHERE folder_id = ?1
              AND image_id IN (SELECT image_pk FROM images WHERE id IN ({placeholders}))
            "
        );
        let mut params_values = Vec::<Value>::with_capacity(1 + chunk.len());
        params_values.push(Value::Integer(folder_id));
        for image_id in chunk {
            params_values.push(Value::Text(image_id.clone()));
        }
        tx.execute(&sql, params_from_iter(params_values.iter()))
            .map_err(|error| format!("Failed to remove images from folder in batch: {error}"))?;
        Ok(())
    })?;
    tx.commit()
        .map_err(|error| format!("Failed to commit batch remove-folder transaction: {error}"))?;

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    Ok(store)
}

pub fn add_images_user_tags(
    image_ids: Vec<String>,
    custom_tags: Vec<String>,
    supplement_tags: Vec<BatchSupplementTagInput>,
    state: &AppState,
) -> Result<LibraryStore, String> {
    let image_ids = normalize_batch_image_ids(image_ids);
    if image_ids.is_empty() {
        return list_library_from_state(state);
    }

    let mut normalized_custom_tags = Vec::<String>::new();
    let mut custom_seen = HashSet::<String>::new();
    for tag in custom_tags {
        let normalized = normalize_tag_text(&tag);
        if normalized.is_empty() {
            continue;
        }
        if custom_seen.insert(normalized.clone()) {
            normalized_custom_tags.push(normalized);
        }
    }

    let mut normalized_supplement_tags = Vec::<(String, Option<String>)>::new();
    let mut supplement_seen = HashSet::<String>::new();
    for tag in supplement_tags {
        let normalized_tag_en = normalize_tag_text(&tag.tag_en);
        if normalized_tag_en.is_empty() {
            continue;
        }
        let dedupe_key = normalized_tag_en.to_lowercase();
        if !supplement_seen.insert(dedupe_key) {
            continue;
        }
        let normalized_tag_zh = normalize_optional_tag_text(tag.tag_zh);
        normalized_supplement_tags.push((normalized_tag_en, normalized_tag_zh));
    }

    if normalized_custom_tags.is_empty() && normalized_supplement_tags.is_empty() {
        return list_library_from_state(state);
    }

    let mut library = state
        .library
        .lock()
        .map_err(|_| "图库状态被占用，请稍后再试".to_string())?;
    let mut conn = open_database(&state.database_path)?;
    conn.execute("PRAGMA foreign_keys = ON", [])
        .map_err(|error| format!("启用数据库外键失败：{error}"))?;

    let tx = conn
        .transaction()
        .map_err(|error| format!("Failed to start batch add tags transaction: {error}"))?;
    let now = now_ms();

    for tag in &normalized_custom_tags {
        upsert_user_custom_tag(&tx, tag)?;
    }

    for image_id in &image_ids {
        for tag_text in &normalized_custom_tags {
            tx.execute(
                "
                INSERT INTO storage_image_user_custom_tags (
                  image_id, tag_text, created_at, updated_at
                )
                VALUES ((SELECT image_pk FROM images WHERE id=?1), ?2, ?3, ?3)
                ON CONFLICT(image_id, tag_text) DO UPDATE SET
                  updated_at = excluded.updated_at
                ",
                params![image_id, tag_text, now],
            )
            .map_err(|error| format!("Failed to add custom user tag in batch: {error}"))?;
        }

        for (tag_en, tag_zh) in &normalized_supplement_tags {
            tx.execute(
                "
                INSERT INTO storage_image_user_supplement_tags (
                  image_id, tag_en, tag_zh, created_at, updated_at
                )
                VALUES ((SELECT image_pk FROM images WHERE id=?1), ?2, ?3, ?4, ?4)
                ON CONFLICT(image_id, tag_en) DO UPDATE SET
                  tag_zh = COALESCE(excluded.tag_zh, storage_image_user_supplement_tags.tag_zh),
                  updated_at = excluded.updated_at
                ",
                params![image_id, tag_en, tag_zh, now],
            )
            .map_err(|error| format!("Failed to add supplement user tag in batch: {error}"))?;
        }

        apply_matching_user_folder_rules_for_image(&tx, image_id)?;
    }

    tx.commit()
        .map_err(|error| format!("Failed to commit batch add tags transaction: {error}"))?;

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    Ok(store)
}

pub fn read_image_bytes(image_id: String, state: &AppState) -> Result<ImageBytes, String> {
    let conn = open_database(&state.database_path)?;
    let image = load_image_record(&conn, &image_id)?;
    let bytes = fs::read(&image.path).map_err(|error| format!("读取图片文件失败：{error}"))?;
    Ok(ImageBytes {
        bytes,
        mime_type: mime_type_for_extension(&image.ext).to_string(),
        file_name: image.file_name,
    })
}

pub fn copy_image_to_system_clipboard(image_id: String, state: &AppState) -> Result<(), String> {
    let conn = open_database(&state.database_path)?;
    let image = load_image_record(&conn, &image_id)?;
    let image_path = PathBuf::from(&image.path);
    let mut clipboard =
        arboard::Clipboard::new().map_err(|error| format!("访问系统剪贴板失败：{error}"))?;

    // 优先写文件列表（Windows: CF_HDROP），速度更接近资源管理器复制文件行为。
    let file_copy_result = clipboard.set().file_list(&[image_path.clone()]);
    if file_copy_result.is_ok() {
        return Ok(());
    }
    let file_copy_error = file_copy_result
        .err()
        .map(|error| error.to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // 文件列表写入失败时，回退到像素写入，保证尽可能兼容图片粘贴目标。
    let bytes = fs::read(&image_path).map_err(|error| {
        format!(
            "文件路径复制失败：{file_copy_error}\n图像像素 fallback 失败：读取图片文件失败：{error}"
        )
    })?;
    let decoded = image::load_from_memory(&bytes).map_err(|error| {
        format!(
            "文件路径复制失败：{file_copy_error}\n图像像素 fallback 失败：解码图片失败：{error}"
        )
    })?;
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();

    clipboard
        .set_image(arboard::ImageData {
            width: width as usize,
            height: height as usize,
            bytes: Cow::Owned(rgba.into_raw()),
        })
        .map_err(|error| {
            format!(
                "文件路径复制失败：{file_copy_error}\n图像像素 fallback 失败：写入系统剪贴板失败：{error}"
            )
        })?;

    Ok(())
}

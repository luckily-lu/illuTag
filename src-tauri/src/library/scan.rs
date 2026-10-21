use super::*;

pub(super) fn cleanup_orphan_reference_images(conn: &Connection) -> Result<(), String> {
    let mut stmt = conn
        .prepare(
            "
            SELECT id, path
            FROM images
            WHERE source = 'reference'
              AND NOT EXISTS (
                SELECT 1 FROM reference_board_items WHERE image_id = images.id
              )
            ",
        )
        .map_err(|error| format!("读取临时参考图失败：{error}"))?;
    let images = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| format!("读取临时参考图失败：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取临时参考图失败：{error}"))?;
    drop(stmt);
    for (image_id, path) in images {
        conn.execute("DELETE FROM images WHERE id = ?1", params![image_id])
            .map_err(|error| format!("删除临时参考图索引失败：{error}"))?;
        let _ = fs::remove_file(path);
    }
    Ok(())
}

pub(super) fn cleanup_missing_library_images_batched(
    database_path: &Path,
    batch_size: usize,
) -> Result<i64, String> {
    let conn = open_database(database_path)?;
    let mut removed_total = 0i64;
    let mut last_rowid = 0i64;
    let limit = (batch_size.max(32)) as i64;
    let mut root_online_cache = HashMap::<String, bool>::new();

    loop {
        let mut stmt = conn
            .prepare(
                "
                SELECT i.rowid, i.id, i.path, f.path
                FROM images i
                LEFT JOIN folders f ON f.id = i.folder_id
                WHERE i.source = 'library' AND i.rowid > ?1
                ORDER BY i.rowid ASC
                LIMIT ?2
                ",
            )
            .map_err(|error| {
                format!("Failed to load library image batch for startup cleanup: {error}")
            })?;

        let rows = stmt
            .query_map(params![last_rowid, limit], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(|error| {
                format!("Failed to load library image batch for startup cleanup: {error}")
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                format!("Failed to load library image batch for startup cleanup: {error}")
            })?;
        drop(stmt);

        if rows.is_empty() {
            break;
        }

        let mut missing_ids = Vec::<(String, String)>::new();
        let mut next_rowid = last_rowid;
        for (rowid, image_id, path, folder_path) in rows {
            next_rowid = rowid;
            let Some(folder_path) = folder_path else {
                continue;
            };
            let root_online = *root_online_cache
                .entry(folder_path.clone())
                .or_insert_with(|| {
                    let normalized = normalize_existing_or_stored_folder_path(&folder_path);
                    Path::new(&normalized).is_dir()
                });
            if !root_online {
                continue;
            }
            match fs::metadata(&path) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    // A source may disconnect after the cached availability check.
                    // Only remove a missing file while its registered root remains online.
                    if Path::new(&folder_path).is_dir() {
                        missing_ids.push((image_id, folder_path));
                    } else {
                        root_online_cache.insert(folder_path, false);
                    }
                }
                Err(error) => {
                    return Err(format!(
                        "无法确认图片是否缺失，已停止清理并保留索引：{path}：{error}"
                    ));
                }
            }
        }
        last_rowid = next_rowid;

        for (image_id, folder_path) in missing_ids {
            if !Path::new(&folder_path).is_dir() {
                continue;
            }
            conn.execute("DELETE FROM images WHERE id = ?1", params![image_id])
                .map_err(|error| {
                    format!("Failed to delete missing image during startup cleanup: {error}")
                })?;
            crate::duplicates::cache::invalidate_database(database_path);
            removed_total += 1;
        }

        thread::sleep(std::time::Duration::from_millis(2));
    }

    Ok(removed_total)
}

pub(super) fn scan_all_folders_and_collect_new_images(
    database_path: &Path,
    foreground: &Arc<std::sync::atomic::AtomicUsize>,
    progress: &Arc<Mutex<BackgroundScanProgress>>,
    pause_requested: &Arc<Mutex<bool>>,
    stop_requested: &Arc<Mutex<bool>>,
    collect_tag_queue: bool,
    source_folder_id: Option<i64>,
) -> Result<ScanCollectResult, String> {
    let mut conn = open_database(database_path)?;
    let scanned_at = now_ms();
    let mut seen_paths = HashSet::new();
    let mut folders = conn
        .prepare(
            "
            SELECT id, path
            FROM folders
            WHERE COALESCE(hidden, 0) = 0
            ORDER BY added_at ASC, id ASC
            ",
        )
        .and_then(|mut stmt| {
            stmt.query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|error| format!("Failed to load folders for scan: {error}"))?;
    if let Some(id) = source_folder_id {
        folders.retain(|f| f.0 == id);
    }
    set_scan_progress_total_folders(progress, folders.len() as i64);
    write_app_meta_value(&conn, "library_scan_in_progress", "1")?;
    // Finish initial directory ownership for images that were committed before
    // an interrupted scan. The pending table is per image, so a user edit made
    // in the meantime is not overwritten.
    let replay_warnings = replay_pending_scan_assignments(&conn, scanned_at, source_folder_id)?;
    for warning in &replay_warnings {
        push_scan_progress_recent_error(progress, warning);
        eprintln!("[wd-scan] {warning}");
    }
    for (_, folder_path) in &folders {
        yield_to_foreground(foreground, stop_requested);
        let folder_path = normalize_existing_or_stored_folder_path(folder_path);
        ensure_library_root_user_folder(&conn, &folder_path, scanned_at)?;
    }
    let mut known_paths = load_known_paths(&conn)?;
    let existing_meta = load_existing_library_image_meta(&conn)?;

    let mut new_image_ids = Vec::<String>::new();
    let mut updated_images = 0i64;
    let mut skipped_images = 0i64;
    let mut scanned_folders = 0i64;
    let mut scanned_files = 0i64;

    for (_, folder_path) in folders {
        if background_scan_stop_requested(stop_requested) {
            break;
        }
        wait_for_background_scan_resume(progress, pause_requested, stop_requested, "collecting");
        if background_scan_stop_requested(stop_requested) {
            break;
        }
        let folder_path = normalize_existing_or_stored_folder_path(&folder_path);
        if !Path::new(&folder_path).is_dir() {
            return Err(format!(
                "来源目录不可访问，请连接磁盘并确认访问权限：{folder_path}"
            ));
        }
        let folder_id = upsert_folder(&conn, &folder_path, scanned_at)?;
        set_scan_progress_current_folder(progress, &folder_path);
        let mut found = Vec::<ScannedImage>::new();
        let mut pending_batch = Vec::<ScannedImage>::new();
        scan_images(
            Path::new(&folder_path),
            scanned_at,
            &mut seen_paths,
            &existing_meta,
            &mut |image| {
                scanned_files += 1;
                found.push(image.clone());
                pending_batch.push(image);
                let should_flush = pending_batch.len() >= 2000;
                if should_flush {
                    flush_scanned_image_batch(
                        &mut conn,
                        folder_id,
                        &pending_batch,
                        &mut known_paths,
                        &mut new_image_ids,
                        &mut updated_images,
                        &mut skipped_images,
                    )?;
                    pending_batch.clear();
                    set_scan_progress_new_images(progress, new_image_ids.len() as i64);
                    set_scan_progress_updated_images(progress, updated_images);
                    set_scan_progress_skipped_images(progress, skipped_images);
                }
                if scanned_files % 20 == 0 {
                    set_scan_progress_scanned_files(progress, scanned_files);
                    wait_for_background_scan_resume(
                        progress,
                        pause_requested,
                        stop_requested,
                        "collecting",
                    );
                }
                Ok(!background_scan_stop_requested(stop_requested))
            },
        )?;
        if !pending_batch.is_empty() {
            flush_scanned_image_batch(
                &mut conn,
                folder_id,
                &pending_batch,
                &mut known_paths,
                &mut new_image_ids,
                &mut updated_images,
                &mut skipped_images,
            )?;
        }
        set_scan_progress_scanned_files(progress, scanned_files);
        let found_count = found.len() as i64;
        // Initial ownership is tracked per image in scan_pending_assignments.
        // Only still-pending images are synced; user edits remove the row.
        let pending_images = load_pending_assignment_images(&conn, folder_id)?;
        if !pending_images.is_empty() {
            complete_pending_assignments(&conn, &folder_path, &pending_images, scanned_at)?;
        }
        scanned_folders += 1;
        set_scan_progress_new_images(progress, new_image_ids.len() as i64);
        set_scan_progress_updated_images(progress, updated_images);
        set_scan_progress_skipped_images(progress, skipped_images);
        set_scan_progress_scanned_folders(progress, scanned_folders);
        if found_count > 0 {
            eprintln!("[wd-scan] folder scanned, new images: {found_count}");
        }
    }

    if background_scan_stop_requested(stop_requested) {
        let _ = write_app_meta_value(&conn, "library_scan_in_progress", "0");
        return Ok(ScanCollectResult {
            tag_queue_images: Vec::new(),
        });
    }

    if !collect_tag_queue {
        set_scan_progress_queued_images(progress, 0);
        let _ = write_app_meta_value(&conn, "library_scan_in_progress", "0");
        return Ok(ScanCollectResult {
            tag_queue_images: Vec::new(),
        });
    }

    let mut tag_queue_images = collect_pending_tag_images(&conn)?;
    tag_queue_images.sort_by(|left, right| left.id.cmp(&right.id));
    tag_queue_images.dedup_by(|left, right| left.id == right.id);
    set_scan_progress_phase(progress, "tagging");
    set_scan_progress_queued_images(progress, tag_queue_images.len() as i64);

    Ok(ScanCollectResult { tag_queue_images })
}

pub(super) fn load_pending_assignment_images(
    conn: &Connection,
    folder_id: i64,
) -> Result<Vec<(i64, String)>, String> {
    conn.prepare(
        "SELECT p.image_pk, i.path
         FROM scan_pending_assignments p
         JOIN images i ON i.image_pk = p.image_pk
         WHERE i.folder_id = ?1 AND i.source = 'library'
         ORDER BY p.image_pk",
    )
    .and_then(|mut stmt| {
        stmt.query_map([folder_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()
    })
    .map_err(|error| format!("Failed to load pending folder assignments: {error}"))
}

fn pending_assignment_exists(conn: &Connection, image_pk: i64) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM scan_pending_assignments WHERE image_pk=?1)",
        [image_pk],
        |row| row.get(0),
    )
    .map_err(|error| format!("Failed to check pending folder assignment: {error}"))
}

/// Consume only the pending rows that still exist inside the same immediate
/// transaction as the folder writes. A manual edit that commits before this
/// transaction removes the marker, so the stale candidate list cannot re-add
/// the directory folder. A manual edit that arrives later blocks on the write
/// lock and wins afterwards.
pub(super) fn complete_pending_assignments(
    conn: &Connection,
    root_folder_path: &str,
    candidates: &[(i64, String)],
    now: i64,
) -> Result<(), String> {
    complete_pending_assignments_with_hook(conn, root_folder_path, candidates, now, &mut || {})
}

/// `before_write` is a test seam for the exact race window: after the stale
/// candidate list was inspected, before the immediate write transaction.
pub(super) fn complete_pending_assignments_with_hook(
    conn: &Connection,
    root_folder_path: &str,
    candidates: &[(i64, String)],
    now: i64,
    before_write: &mut dyn FnMut(),
) -> Result<(), String> {
    if candidates.is_empty() {
        return Ok(());
    }
    let mut live = Vec::<(i64, String)>::new();
    for (image_pk, path) in candidates {
        if pending_assignment_exists(conn, *image_pk)? {
            live.push((*image_pk, path.clone()));
        }
    }
    if live.is_empty() {
        return Ok(());
    }
    // Directory walking is read-only and happens before the write transaction.
    let plan = plan_folder_sync(root_folder_path, &live)?;
    before_write();
    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(|error| format!("Failed to open pending assignment transaction: {error}"))?;
    let result = (|| -> Result<(), String> {
        let mut images = Vec::<(i64, String)>::new();
        for (image_pk, path) in live {
            if pending_assignment_exists(conn, image_pk)? {
                images.push((image_pk, path));
            }
        }
        if images.is_empty() {
            return Ok(());
        }
        sync_user_folder_tree_in_tx(conn, &plan, &images, now)?;
        assign_scanned_images_to_nearest_synced_parent_folder_in_tx(conn, &plan, &images, now)?;
        clear_pending_assignments(conn, &images)?;
        Ok(())
    })();
    match result {
        Ok(()) => conn
            .execute_batch("COMMIT")
            .map_err(|error| format!("Failed to commit pending assignment transaction: {error}")),
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

fn clear_pending_assignments(
    conn: &Connection,
    images: &[(i64, String)],
) -> Result<(), String> {
    for (image_pk, _) in images {
        // Keep the marker when no initial ownership row exists: a missing
        // mapping must be retried, never silently certified as synced.
        conn.execute(
            "DELETE FROM scan_pending_assignments
             WHERE image_pk=?1
               AND EXISTS (
                 SELECT 1 FROM storage_image_user_folders f
                 JOIN user_folders u ON u.id=f.folder_id
                 WHERE f.image_id=?1 AND u.source_kind=?2
               )",
            params![image_pk, USER_FOLDER_SOURCE_KIND_LIBRARY_DIR],
        )
        .map_err(|error| format!("Failed to clear pending folder assignment: {error}"))?;
    }
    Ok(())
}

/// Replays only images that still have an unused initial-ownership record.
/// A user folder edit removes that record, so this cannot overwrite a manual
/// classification even when the edit happened after the scan chunk committed.
///
/// `scope` limits compensation to the folders this scan was asked to visit.
/// Pending rows from an offline/unrelated source are retained and reported,
/// but must not block an online source from scanning.
pub(super) fn replay_pending_scan_assignments(
    conn: &Connection,
    now: i64,
    scope: Option<i64>,
) -> Result<Vec<String>, String> {
    let rows = conn
        .prepare(
            "SELECT p.image_pk, i.path, f.path, i.folder_id
             FROM scan_pending_assignments p
             JOIN images i ON i.image_pk = p.image_pk
             JOIN folders f ON f.id = i.folder_id
             WHERE i.source = 'library'
               AND (?1 IS NULL OR i.folder_id = ?1)
             ORDER BY f.path, p.image_pk",
        )
        .map_err(|error| format!("Failed to load pending folder assignments: {error}"))?
        .query_map(params![scope], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|error| format!("Failed to load pending folder assignments: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to load pending folder assignments: {error}"))?;

    let mut warnings = Vec::<String>::new();
    let mut index = 0usize;
    while index < rows.len() {
        let folder_path = rows[index].2.clone();
        let mut images = Vec::<(i64, String)>::new();
        while index < rows.len() && rows[index].2 == folder_path {
            images.push((rows[index].0, rows[index].1.clone()));
            index += 1;
        }
        if !Path::new(&folder_path).is_dir() {
            warnings.push(format!(
                "来源暂时不可访问，保留了待补齐的目录归属：{folder_path}"
            ));
            continue;
        }
        complete_pending_assignments(conn, &folder_path, &images, now)?;
    }
    Ok(warnings)
}

pub(super) fn tag_images_with_wd_model(
    database_path: &Path,
    foreground: &Arc<std::sync::atomic::AtomicUsize>,
    images: &[TagQueueImage],
    progress: &Arc<Mutex<BackgroundScanProgress>>,
    pause_requested: &Arc<Mutex<bool>>,
    stop_requested: &Arc<Mutex<bool>>,
    wd_tagger_service: &Arc<Mutex<Option<WdTaggerService>>>,
) -> Result<(), String> {
    if images.is_empty() {
        return Ok(());
    }

    let mut conn = open_database(database_path)?;
    let dictionary = load_cn_tag_dictionary_map()?;
    let model_root = resolve_wd_tagger_model_dir(None)?;
    let model_path = model_root.join("model.onnx");
    let tags_path = model_root.join("selected_tags.csv");
    let script_path = resolve_wd_tagger_service_script_path()?;

    if !model_path.is_file() || !tags_path.is_file() || !script_path.is_file() {
        let err = "Model files or wd_tagger_service.py not found; skip tagging".to_string();
        set_scan_progress_error(progress, &err);
        push_scan_progress_recent_error(progress, &err);
        eprintln!("[wd-tag] {err}");
        return Err(err);
    }

    let mut service_guard = wd_tagger_service
        .lock()
        .map_err(|_| "WD tagger service state is locked".to_string())?;
    ensure_wd_tagger_service_started(&mut service_guard, &model_path, &tags_path, &script_path)?;

    eprintln!("[wd-tag] queue size: {}", images.len());

    let mut session = WdTagSession {
        service: &mut service_guard,
        model_path: &model_path,
        tags_path: &tags_path,
        script_path: &script_path,
    };
    let mut checkpoint = | | -> Result<bool, String> {
        yield_to_foreground(foreground, stop_requested);
        if background_scan_stop_requested(stop_requested) {
            return Ok(false);
        }
        wait_for_background_scan_resume(progress, pause_requested, stop_requested, "tagging");
        Ok(!background_scan_stop_requested(stop_requested))
    };
    let mut save_batch = |batch: &[(String, WdTaggerTestResult)]| {
        flush_wd_tag_saves(&mut conn, batch, &dictionary)
    };
    let mut failed = |error: &str| {
        increment_scan_progress_failed(progress);
        set_scan_progress_error(progress, error);
        push_scan_progress_recent_error(progress, error);
        eprintln!("[wd-tag] {error}");
    };
    let mut saved = |count: i64| add_scan_progress_tagged(progress, count);
    run_tag_pipeline(
        &mut session,
        images,
        &mut checkpoint,
        &mut save_batch,
        &mut failed,
        &mut saved,
    )
}

pub(super) trait TagSession {
    fn send(&mut self, image: &TagQueueImage) -> Result<(), String>;
    fn read(&mut self) -> Result<WdTaggerTestResult, String>;
    fn restart(&mut self) -> Result<(), String>;
}

struct WdTagSession<'a> {
    service: &'a mut Option<WdTaggerService>,
    model_path: &'a Path,
    tags_path: &'a Path,
    script_path: &'a Path,
}

impl TagSession for WdTagSession<'_> {
    fn send(&mut self, image: &TagQueueImage) -> Result<(), String> {
        ensure_wd_tagger_service_started(
            self.service,
            self.model_path,
            self.tags_path,
            self.script_path,
        )?;
        let running = self
            .service
            .as_mut()
            .ok_or_else(|| "WD tagger service unavailable".to_string())?;
        wd_tagger_send_request(running, &image.id, &image.path, 0.35, 0.85)
    }

    fn read(&mut self) -> Result<WdTaggerTestResult, String> {
        let running = self
            .service
            .as_mut()
            .ok_or_else(|| "WD tagger service unavailable".to_string())?;
        wd_tagger_read_response(running)
    }

    fn restart(&mut self) -> Result<(), String> {
        stop_python_child_service(self.service, |running| &mut running.child);
        ensure_wd_tagger_service_started(
            self.service,
            self.model_path,
            self.tags_path,
            self.script_path,
        )
    }
}

/// Prefetch depth 2: send the next request before waiting for the current
/// response. The pipeline owns service generations: after a send/read failure
/// it restarts the service and re-sends every unread request in order. A save is
/// attempted only after an ID-validated response, so a restart cannot duplicate
/// or misattribute tags.
pub(super) fn run_tag_pipeline<S: TagSession>(
    session: &mut S,
    images: &[TagQueueImage],
    checkpoint: &mut dyn FnMut() -> Result<bool, String>,
    save_batch: &mut dyn FnMut(&[(String, WdTaggerTestResult)]) -> Result<(), String>,
    failed: &mut dyn FnMut(&str),
    saved: &mut dyn FnMut(i64),
) -> Result<(), String> {
    let total = images.len();
    let mut pending = std::collections::VecDeque::<(usize, TagQueueImage)>::new();
    let mut skipped_missing = vec![false; total];
    let mut read_idx = 0usize;
    let mut send_idx = 0usize;
    let mut retried_current = false;
    let mut retried_send = false;
    let mut pending_saves: Vec<(String, WdTaggerTestResult)> = Vec::new();

    while read_idx < total {
        if !checkpoint()? {
            break;
        }
        while send_idx < total && pending.len() < 2 {
            let image = images[send_idx].clone();
            if !Path::new(&image.path).is_file() {
                // A send/read restart rewinds send_idx to read_idx and walks
                // over already-rejected missing files again. Count each missing
                // image once, not once per service generation.
                if !skipped_missing[send_idx] {
                    failed(&format!("图片文件不存在，已跳过：{}", image.path));
                    skipped_missing[send_idx] = true;
                }
                send_idx += 1;
                continue;
            }
            match session.send(&image) {
                Ok(()) => {
                    pending.push_back((send_idx, image));
                    send_idx += 1;
                }
                Err(error) => {
                    eprintln!("[wd-tag] send failed, restarting service: {error}");
                    session.restart()?;
                    pending.clear();
                    // Re-submit the failed request and every prefetched request
                    // the old service may already have accepted.
                    send_idx = read_idx;
                    if retried_send {
                        return Err(format!(
                            "WD tagger service failed twice while sending: {error}"
                        ));
                    }
                    retried_send = true;
                }
            }
        }
        if pending.is_empty() {
            break;
        }

        let (current_index, current) = pending.front().cloned().ok_or("模型请求队列为空")?;
        read_idx = current_index;
        match session.read() {
            Ok(result) => {
                if result.image_id != current.id {
                    session.restart()?;
                    return Err("模型响应与图片请求不一致，已停止任务".into());
                }
                pending.pop_front();
                pending_saves.push((current.id, result));
                read_idx += 1;
                retried_current = false;
                retried_send = false;
                if pending_saves.len() >= TAG_SAVE_BATCH_SIZE {
                    let count = pending_saves.len() as i64;
                    save_batch(&pending_saves)?;
                    pending_saves.clear();
                    saved(count);
                }
            }
            Err(first_error) => {
                eprintln!("[wd-tag] read failed, restarting service: {first_error}");
                session.restart()?;
                send_idx = read_idx;
                pending.clear();
                if retried_current {
                    let error = format!("WD tagger service failed twice: {first_error}");
                    failed(&error);
                    read_idx += 1;
                    // The failed item consumed its retry. Restart the remaining
                    // pipeline at the next item, including the lost prefetch.
                    send_idx = read_idx;
                    retried_current = false;
                    retried_send = false;
                } else {
                    retried_current = true;
                }
            }
        }
    }

    if !pending_saves.is_empty() {
        let count = pending_saves.len() as i64;
        save_batch(&pending_saves)?;
        saved(count);
    }

    // Stopping with unread requests would leave stale responses in the pipe.
    if read_idx < total && send_idx > read_idx {
        let _ = session.restart();
    }
    Ok(())
}
pub(super) fn ensure_wd_tagger_service_started(
    service: &mut Option<WdTaggerService>,
    model_path: &Path,
    tags_path: &Path,
    script_path: &Path,
) -> Result<(), String> {
    let need_restart = match service.as_ref() {
        None => true,
        Some(existing) => existing.model_path != model_path || existing.tags_path != tags_path,
    };
    if !need_restart {
        return Ok(());
    }
    stop_python_child_service(service, |running| &mut running.child);
    *service = Some(spawn_wd_tagger_service(model_path, tags_path, script_path)?);
    Ok(())
}

pub(super) fn spawn_wd_tagger_service(
    model_path: &Path,
    tags_path: &Path,
    script_path: &Path,
) -> Result<WdTaggerService, String> {
    let mut command = python_command();
    let mut child = command
        .arg("-X")
        .arg("utf8")
        .arg(script_path)
        .arg("--model")
        .arg(model_path)
        .arg("--tags")
        .arg(tags_path)
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("Failed to start WD tagger service: {error}"))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "WD tagger service stdin unavailable".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "WD tagger service stdout unavailable".to_string())?;

    Ok(WdTaggerService {
        child,
        stdin,
        stdout: crate::protocol::ResponseReader::new(stdout),
        model_path: model_path.to_path_buf(),
        tags_path: tags_path.to_path_buf(),
    })
}

pub(super) fn wd_tagger_send_request(
    service: &mut WdTaggerService,
    image_id: &str,
    image_path: &str,
    general_threshold: f32,
    character_threshold: f32,
) -> Result<(), String> {
    let request = serde_json::json!({
        "image_id": image_id,
        "image_path": image_path,
        "general_threshold": general_threshold,
        "character_threshold": character_threshold,
    });
    service
        .stdin
        .write_all(request.to_string().as_bytes())
        .and_then(|_| service.stdin.write_all(b"\n"))
        .and_then(|_| service.stdin.flush())
        .map_err(|error| format!("Failed to write WD tagger service request: {error}"))?;
    Ok(())
}

pub(super) fn wd_tagger_read_response(
    service: &mut WdTaggerService,
) -> Result<WdTaggerTestResult, String> {
    let mut response_line = String::new();
    service
        .stdout
        .read_line(&mut response_line)
        .map_err(|error| format!("Failed to read WD tagger service response: {error}"))?;
    if response_line.trim().is_empty() {
        return Err("WD tagger service returned empty response".to_string());
    }
    let value: serde_json::Value = serde_json::from_str(response_line.trim())
        .map_err(|error| format!("Failed to parse WD tagger service response: {error}"))?;
    if let Some(error_text) = value.get("error").and_then(|item| item.as_str()) {
        return Err(format!("WD tagger service error: {error_text}"));
    }
    serde_json::from_value(value)
        .map_err(|error| format!("Failed to parse WD tagger service output: {error}"))
}

pub(super) fn flush_wd_tag_saves(
    conn: &mut Connection,
    batch: &[(String, WdTaggerTestResult)],
    dictionary: &HashMap<String, String>,
) -> Result<(), String> {
    if batch.is_empty() {
        return Ok(());
    }
    let now = now_ms();
    let model_name = WD_TAGGER_MODEL_NAME;
    // 整批一个事务：避免每张图片一次 fsync
    let tx = conn
        .transaction()
        .map_err(|error| format!("Failed to open tag save transaction: {error}"))?;

    for (image_id, result) in batch {
        tx.execute(
            "DELETE FROM storage_auto_tags WHERE image_pk=(SELECT image_pk FROM images WHERE id=?1) AND model_pk=(SELECT string_pk FROM storage_strings WHERE value=?2)",
            params![image_id, model_name],
        )
        .map_err(|error| format!("Failed to clear old image tags: {error}"))?;

        for tag in &result.ratings {
            upsert_image_auto_tag(
                &tx, image_id, "rating", &tag.tag, tag.score, dictionary, model_name, now,
            )?;
        }
        for tag in &result.character_tags {
            upsert_image_auto_tag(
                &tx,
                image_id,
                "character",
                &tag.tag,
                tag.score,
                dictionary,
                model_name,
                now,
            )?;
        }
        for tag in &result.general_tags {
            upsert_image_auto_tag(
                &tx, image_id, "general", &tag.tag, tag.score, dictionary, model_name, now,
            )?;
        }
    }

    tx.commit()
        .map_err(|error| format!("Failed to commit image tags: {error}"))?;
    for (image_id, _) in batch {
        apply_matching_user_folder_rules_for_image(conn, image_id)?;
    }
    Ok(())
}

pub(super) fn upsert_image_auto_tag(
    conn: &Connection,
    image_id: &str,
    category: &str,
    tag_en: &str,
    confidence: f32,
    dictionary: &HashMap<String, String>,
    model_name: &str,
    updated_at: i64,
) -> Result<(), String> {
    let tag_zh = lookup_cn_tag(dictionary, tag_en);
    if let Some(zh) = &tag_zh {
        conn.execute(
            "
            INSERT INTO tag_dictionary (tag_en, tag_zh, updated_at)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(tag_en) DO UPDATE SET
              tag_zh = excluded.tag_zh,
              updated_at = excluded.updated_at
            ",
            params![tag_en, zh, updated_at],
        )
        .map_err(|error| format!("Failed to upsert tag dictionary: {error}"))?;
    }

    compact::upsert_auto_tag(
        conn,
        image_id,
        category,
        tag_en,
        tag_zh.as_deref(),
        f64::from(confidence),
        model_name,
        updated_at,
    )
}

pub(super) fn collect_pending_tag_images(conn: &Connection) -> Result<Vec<TagQueueImage>, String> {
    let model_name = WD_TAGGER_MODEL_NAME;
    conn.prepare(
        "
        SELECT images.id, images.path
        FROM images
        WHERE images.source = 'library'
          AND COALESCE(images.trashed, 0) = 0
          AND NOT EXISTS (
            SELECT 1 FROM storage_auto_tags t
            WHERE t.image_pk=images.image_pk
              AND t.model_pk=(SELECT string_pk FROM storage_strings WHERE value=?1)
          )
        ORDER BY images.imported_at DESC, images.id ASC
        ",
    )
    .and_then(|mut stmt| {
        stmt.query_map(params![model_name], |row| {
            Ok(TagQueueImage {
                id: row.get(0)?,
                path: row.get(1)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()
    })
    .map_err(|error| format!("Failed to collect pending tag images: {error}"))
}

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
        let mut newly_found_images = Vec::<ScannedImage>::new();
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
                        &mut newly_found_images,
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
                &mut newly_found_images,
                &mut updated_images,
                &mut skipped_images,
            )?;
        }
        set_scan_progress_scanned_files(progress, scanned_files);
        let found_count = found.len() as i64;
        // 文件夹归属只需处理新增图片：路径未变的图片其所属目录与归属关系不可能变化，
        // 跳过可避免全量 DELETE+INSERT 重写（原实现对每次扫描的全部图片逐条自动提交，是主要瓶颈）
        sync_user_folder_tree_for_library_directory(
            &conn,
            &folder_path,
            &newly_found_images,
            scanned_at,
        )?;
        assign_scanned_images_to_nearest_synced_parent_folder(
            &conn,
            &folder_path,
            &newly_found_images,
            scanned_at,
        )?;
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
            tag_queue_image_ids: Vec::new(),
        });
    }

    if !collect_tag_queue {
        set_scan_progress_queued_images(progress, 0);
        let _ = write_app_meta_value(&conn, "library_scan_in_progress", "0");
        return Ok(ScanCollectResult {
            tag_queue_image_ids: Vec::new(),
        });
    }

    let mut tag_queue_image_ids = collect_pending_tag_image_ids(&conn)?;
    tag_queue_image_ids.sort();
    tag_queue_image_ids.dedup();
    set_scan_progress_phase(progress, "tagging");
    set_scan_progress_queued_images(progress, tag_queue_image_ids.len() as i64);

    Ok(ScanCollectResult {
        tag_queue_image_ids,
    })
}

pub(super) fn tag_images_with_wd_model(
    database_path: &Path,
    foreground: &Arc<std::sync::atomic::AtomicUsize>,
    image_ids: &[String],
    progress: &Arc<Mutex<BackgroundScanProgress>>,
    pause_requested: &Arc<Mutex<bool>>,
    stop_requested: &Arc<Mutex<bool>>,
    wd_tagger_service: &Arc<Mutex<Option<WdTaggerService>>>,
) -> Result<(), String> {
    if image_ids.is_empty() {
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

    eprintln!("[wd-tag] queue size: {}", image_ids.len());

    // 流水线：发送下一张图片的请求后再等待当前响应，
    // 让 Python 侧的预处理与 GPU 推理重叠；写库按批次单事务提交。
    let total = image_ids.len();
    let mut pending = std::collections::VecDeque::<(usize, String)>::new();
    let mut read_idx = 0usize; // 下一张等待响应的图片
    let mut send_idx = 0usize; // 下一张待发送的图片
    let mut retried_current = false;
    let mut pending_saves: Vec<(String, WdTaggerTestResult)> = Vec::new();

    while read_idx < total {
        yield_to_foreground(foreground, stop_requested);
        if background_scan_stop_requested(stop_requested) {
            break;
        }
        wait_for_background_scan_resume(progress, pause_requested, stop_requested, "tagging");
        if background_scan_stop_requested(stop_requested) {
            break;
        }

        // 保持管道深度为 2（一张推理中、一张预处理中）
        while send_idx < total && pending.len() < 2 {
            let image_path = image_ids[send_idx].clone();
            if !Path::new(&image_path).is_file() {
                increment_scan_progress_failed(progress);
                send_idx += 1;
                continue;
            }
            match wd_tagger_send_with_recovery(
                &mut service_guard,
                &model_path,
                &tags_path,
                &script_path,
                &image_path,
                &image_path,
                0.35,
                0.85,
            ) {
                Ok(()) => {
                    pending.push_back((send_idx, image_path));
                    send_idx += 1;
                }
                Err(error) => {
                    eprintln!("[wd-tag] {error}");
                    increment_scan_progress_failed(progress);
                    set_scan_progress_error(progress, &error);
                    push_scan_progress_recent_error(progress, &error);
                    send_idx += 1;
                }
            }
        }
        if pending.is_empty() {
            break;
        }

        let (current_index, current_id) = pending.front().cloned().ok_or("模型请求队列为空")?;
        read_idx = current_index;
        let read_result = {
            let service = service_guard
                .as_mut()
                .ok_or_else(|| "WD tagger service unavailable".to_string())?;
            wd_tagger_read_response(service)
        };
        match read_result {
            Ok(result) => {
                if result.image_id != current_id {
                    stop_python_child_service(&mut service_guard, |running| &mut running.child);
                    return Err("模型响应与图片请求不一致，已停止任务".into());
                }
                pending.pop_front();
                pending_saves.push((current_id, result));
                read_idx += 1;
                retried_current = false;
                if pending_saves.len() >= TAG_SAVE_BATCH_SIZE {
                    let saved = pending_saves.len() as i64;
                    flush_wd_tag_saves(&mut conn, &pending_saves, &dictionary)?;
                    pending_saves.clear();
                    add_scan_progress_tagged(progress, saved);
                }
            }
            Err(first_error) => {
                // 服务可能已挂掉：重启后当前图片及已发送但未读的请求全部重发
                eprintln!("[wd-tag] read failed, restarting service: {first_error}");
                stop_python_child_service(&mut service_guard, |running| &mut running.child);
                send_idx = read_idx;
                pending.clear();
                if retried_current {
                    let error = format!("WD tagger service failed twice: {first_error}");
                    eprintln!("[wd-tag] {error}");
                    increment_scan_progress_failed(progress);
                    set_scan_progress_error(progress, &error);
                    push_scan_progress_recent_error(progress, &error);
                    read_idx += 1;
                    // The failed item has consumed its one retry. Restart the remaining
                    // pipeline at the next item, including any lost prefetch request.
                    send_idx = read_idx;
                    retried_current = false;
                } else {
                    retried_current = true;
                }
            }
        }
    }

    if !pending_saves.is_empty() {
        let saved = pending_saves.len() as i64;
        flush_wd_tag_saves(&mut conn, &pending_saves, &dictionary)?;
        pending_saves.clear();
        add_scan_progress_tagged(progress, saved);
    }

    // 若中途停止且管道中尚有未读响应，直接重启服务，避免下次读到错位的旧响应
    if read_idx < total && send_idx > read_idx {
        stop_python_child_service(&mut service_guard, |running| &mut running.child);
    }

    Ok(())
}

pub(super) fn wd_tagger_send_with_recovery(
    service: &mut Option<WdTaggerService>,
    model_path: &Path,
    tags_path: &Path,
    script_path: &Path,
    image_id: &str,
    image_path: &str,
    general_threshold: f32,
    character_threshold: f32,
) -> Result<(), String> {
    ensure_wd_tagger_service_started(service, model_path, tags_path, script_path)?;
    let primary = {
        let running = service
            .as_mut()
            .ok_or_else(|| "WD tagger service unavailable".to_string())?;
        wd_tagger_send_request(
            running,
            image_id,
            image_path,
            general_threshold,
            character_threshold,
        )
    };
    match primary {
        Ok(()) => Ok(()),
        Err(first_error) => {
            stop_python_child_service(service, |running| &mut running.child);
            ensure_wd_tagger_service_started(service, model_path, tags_path, script_path)?;
            let running = service
                .as_mut()
                .ok_or_else(|| "WD tagger service unavailable after restart".to_string())?;
            wd_tagger_send_request(
                running,
                image_id,
                image_path,
                general_threshold,
                character_threshold,
            )
            .map_err(|second_error| {
                format!(
                    "WD tagger service failed and restart retry also failed. first: {first_error}; second: {second_error}"
                )
            })
        }
    }
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
            "DELETE FROM image_auto_tags WHERE image_id = ?1 AND model_name = ?2",
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

    conn.execute(
        "
        INSERT INTO image_auto_tags (
          image_id, category, tag_en, tag_zh, confidence, model_name, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        ON CONFLICT(image_id, category, tag_en, model_name) DO UPDATE SET
          tag_zh = excluded.tag_zh,
          confidence = excluded.confidence,
          updated_at = excluded.updated_at
        ",
        params![image_id, category, tag_en, tag_zh, confidence, model_name, updated_at],
    )
    .map_err(|error| format!("Failed to save image auto tag: {error}"))?;
    Ok(())
}

pub(super) fn collect_pending_tag_image_ids(conn: &Connection) -> Result<Vec<String>, String> {
    let model_name = WD_TAGGER_MODEL_NAME;
    conn.prepare(
        "
        SELECT images.id
        FROM images
        WHERE images.source = 'library'
          AND COALESCE(images.trashed, 0) = 0
          AND images.id NOT IN (
            SELECT image_id
            FROM image_auto_tags
            WHERE model_name = ?1
          )
        ORDER BY images.imported_at DESC, images.id ASC
        ",
    )
    .and_then(|mut stmt| {
        stmt.query_map(params![model_name], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()
    })
    .map_err(|error| format!("Failed to collect pending tag images: {error}"))
}

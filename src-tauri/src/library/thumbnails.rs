use super::*;

pub(super) fn generate_thumbnails_once(
    database_path: &Path,
    foreground: &Arc<std::sync::atomic::AtomicUsize>,
    progress: &Arc<Mutex<ThumbnailGenerationProgress>>,
    pause_requested: &Arc<Mutex<bool>>,
    stop_requested: &Arc<Mutex<bool>>,
) -> Result<i64, String> {
    set_thumbnail_progress_phase(progress, "collecting");
    let conn = open_database(database_path)?;
    let mut stmt = conn
        .prepare(
            "
            SELECT
              i.id,
              i.path,
              i.modified_at,
              i.file_size,
              t.thumb_path,
              t.source_modified_at,
              t.source_file_size
            FROM images i
            LEFT JOIN storage_image_thumbnails t ON t.image_id = i.image_pk
            WHERE i.source = 'library'
              AND COALESCE(i.trashed, 0) = 0
            ORDER BY i.modified_at DESC, i.id ASC
            ",
        )
        .map_err(|error| format!("Failed to load thumbnail candidates: {error}"))?;
    let mut candidates = stmt
        .query_map([], |row| {
            Ok(ThumbnailCandidate {
                image_id: row.get(0)?,
                image_path: row.get(1)?,
                modified_at: row.get(2)?,
                file_size: row.get(3)?,
                current_thumb_path: row.get(4)?,
                current_source_modified_at: row.get(5)?,
                current_source_file_size: row.get(6)?,
            })
        })
        .map_err(|error| format!("Failed to load thumbnail candidates: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to load thumbnail candidates: {error}"))?;
    drop(stmt);

    let thumb_root = ensure_thumbnail_root_dir(database_path)?;
    repair_thumbnail_paths_for_current_data_dir(&conn, &thumb_root, &mut candidates)?;
    set_thumbnail_progress_total(progress, candidates.len() as i64);
    set_thumbnail_progress_phase(progress, "generating");

    let worker_count = THUMBNAIL_WORKER_COUNT.max(1);
    let queue_capacity = THUMBNAIL_WORKER_QUEUE_CAPACITY.max(1);
    let (result_tx, result_rx) = mpsc::channel::<ThumbnailWorkerResult>();
    let mut worker_senders = Vec::with_capacity(worker_count);
    let mut worker_handles = Vec::with_capacity(worker_count);
    for _ in 0..worker_count {
        let (job_tx, job_rx) = mpsc::sync_channel::<Option<ThumbnailCandidate>>(queue_capacity);
        let worker_result_tx = result_tx.clone();
        let worker_thumb_root = thumb_root.clone();
        let handle = thread::spawn(move || {
            thumbnail_worker_loop(job_rx, worker_result_tx, worker_thumb_root);
        });
        worker_senders.push(job_tx);
        worker_handles.push(handle);
    }
    drop(result_tx);

    let mut generated = 0i64;
    let mut skipped = 0i64;
    let mut failed = 0i64;
    let mut processed = 0i64;
    let mut in_flight = 0usize;
    let mut next_worker_index = 0usize;
    let mut stop_now = false;

    for candidate in candidates {
        yield_to_foreground(foreground, stop_requested);
        if thumbnail_stop_requested(stop_requested) {
            stop_now = true;
            break;
        }
        wait_for_thumbnail_resume(progress, pause_requested, stop_requested);
        if thumbnail_stop_requested(stop_requested) {
            stop_now = true;
            break;
        }

        if !Path::new(&candidate.image_path).is_file() {
            if let Err(error) = clear_thumbnail_for_missing_image(&conn, &candidate.image_id) {
                set_thumbnail_progress_error(progress, &error);
                push_thumbnail_progress_recent_error(progress, &error);
                failed += 1;
            } else {
                skipped += 1;
            }
            processed += 1;
            set_thumbnail_progress_counts(progress, processed, generated, skipped, failed);
            continue;
        }

        if thumbnail_up_to_date(&candidate) {
            skipped += 1;
            processed += 1;
            set_thumbnail_progress_counts(progress, processed, generated, skipped, failed);
            continue;
        }

        let sender_index = next_worker_index % worker_senders.len();
        next_worker_index += 1;
        let sender = &worker_senders[sender_index];
        let mut pending_candidate = Some(candidate);
        while let Some(job_candidate) = pending_candidate.take() {
            if thumbnail_stop_requested(stop_requested) {
                stop_now = true;
                break;
            }
            wait_for_thumbnail_resume(progress, pause_requested, stop_requested);
            if thumbnail_stop_requested(stop_requested) {
                stop_now = true;
                break;
            }
            match sender.try_send(Some(job_candidate)) {
                Ok(()) => {
                    in_flight += 1;
                }
                Err(TrySendError::Full(returned_candidate)) => {
                    pending_candidate = returned_candidate;
                    match result_rx.recv_timeout(std::time::Duration::from_millis(40)) {
                        Ok(result) => {
                            in_flight = in_flight.saturating_sub(1);
                            apply_thumbnail_worker_result(
                                &conn,
                                progress,
                                result,
                                &mut processed,
                                &mut generated,
                                skipped,
                                &mut failed,
                            );
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            let error = "Thumbnail workers disconnected unexpectedly".to_string();
                            set_thumbnail_progress_error(progress, &error);
                            push_thumbnail_progress_recent_error(progress, &error);
                            failed += 1;
                            processed += 1;
                            set_thumbnail_progress_counts(
                                progress, processed, generated, skipped, failed,
                            );
                            stop_now = true;
                            break;
                        }
                    }
                }
                Err(TrySendError::Disconnected(returned_candidate)) => {
                    let image_id = returned_candidate
                        .as_ref()
                        .map(|candidate| candidate.image_id.as_str())
                        .unwrap_or("unknown");
                    let error = format!(
                        "Thumbnail worker disconnected before processing {}",
                        image_id
                    );
                    set_thumbnail_progress_error(progress, &error);
                    push_thumbnail_progress_recent_error(progress, &error);
                    failed += 1;
                    processed += 1;
                    set_thumbnail_progress_counts(progress, processed, generated, skipped, failed);
                }
            }
        }
        if stop_now {
            break;
        }
    }

    drop(worker_senders);

    while in_flight > 0 {
        match result_rx.recv_timeout(std::time::Duration::from_millis(120)) {
            Ok(result) => {
                in_flight -= 1;
                apply_thumbnail_worker_result(
                    &conn,
                    progress,
                    result,
                    &mut processed,
                    &mut generated,
                    skipped,
                    &mut failed,
                );
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let error = "Thumbnail worker result channel disconnected".to_string();
                set_thumbnail_progress_error(progress, &error);
                push_thumbnail_progress_recent_error(progress, &error);
                break;
            }
        }
    }

    for handle in worker_handles {
        if handle.join().is_err() {
            let error = "Thumbnail worker thread panicked".to_string();
            set_thumbnail_progress_error(progress, &error);
            push_thumbnail_progress_recent_error(progress, &error);
        }
    }

    cleanup_orphan_thumbnail_files(&conn, &thumb_root)?;
    if stop_now {
        set_thumbnail_progress_phase(progress, "idle");
    }
    Ok(generated)
}

pub(super) fn thumbnail_worker_loop(
    receiver: mpsc::Receiver<Option<ThumbnailCandidate>>,
    sender: mpsc::Sender<ThumbnailWorkerResult>,
    thumb_root: PathBuf,
) {
    while let Ok(job) = receiver.recv() {
        let Some(candidate) = job else {
            break;
        };
        let output = generate_single_thumbnail(
            &candidate,
            &thumb_root,
            THUMBNAIL_LONG_EDGE,
            THUMBNAIL_WEBP_QUALITY,
        );
        let _ = sender.send(ThumbnailWorkerResult { candidate, output });
    }
}

pub(super) fn apply_thumbnail_worker_result(
    conn: &Connection,
    progress: &Arc<Mutex<ThumbnailGenerationProgress>>,
    result: ThumbnailWorkerResult,
    processed: &mut i64,
    generated: &mut i64,
    skipped: i64,
    failed: &mut i64,
) {
    match result.output {
        Ok(next_thumb_path) => {
            if let Err(error) = upsert_thumbnail_record(
                conn,
                &result.candidate.image_id,
                &next_thumb_path,
                result.candidate.modified_at,
                result.candidate.file_size,
            ) {
                set_thumbnail_progress_error(progress, &error);
                push_thumbnail_progress_recent_error(progress, &error);
                *failed += 1;
            } else {
                *generated += 1;
                if let Some(previous_thumb) = result.candidate.current_thumb_path {
                    if previous_thumb != next_thumb_path {
                        let _ = fs::remove_file(previous_thumb);
                    }
                }
            }
        }
        Err(error) => {
            set_thumbnail_progress_error(progress, &error);
            push_thumbnail_progress_recent_error(progress, &error);
            *failed += 1;
        }
    }

    *processed += 1;
    set_thumbnail_progress_counts(progress, *processed, *generated, skipped, *failed);
}

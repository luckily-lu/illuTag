use super::*;

pub(super) fn generate_natural_language_embeddings_once(
    database_path: &Path,
    foreground: &Arc<std::sync::atomic::AtomicUsize>,
    progress: &Arc<Mutex<NaturalLanguageScanProgress>>,
    clip_vector_cache: &Arc<Mutex<Option<ClipImageVectorCache>>>,
    clip_image_encoder_service: &Arc<Mutex<Option<ClipImageEncoderService>>>,
    clip_image_encoder_last_used_at: &Arc<Mutex<i64>>,
    clip_image_encoder_release_worker_running: &Arc<Mutex<bool>>,
    pause_requested: &Arc<Mutex<bool>>,
    stop_requested: &Arc<Mutex<bool>>,
) -> Result<i64, String> {
    set_natural_language_scan_progress_phase(progress, "collecting");
    let conn = open_database(database_path)?;
    let mut stmt = conn
        .prepare(
            "
            SELECT
              i.id,
              i.path,
              i.modified_at,
              e.source_modified_at
            FROM images i
            LEFT JOIN image_clip_embeddings e
              ON e.image_id = i.id
             AND e.model_id = ?1
             AND e.model_version = ?2
            WHERE i.source = 'library'
              AND COALESCE(i.trashed, 0) = 0
            ORDER BY i.modified_at DESC, i.id ASC
            ",
        )
        .map_err(|error| format!("Failed to load natural language scan candidates: {error}"))?;
    let candidates = stmt
        .query_map(
            params![CHINESE_CLIP_MODEL_ID, CHINESE_CLIP_MODEL_VERSION],
            |row| {
                Ok(NaturalLanguageEmbeddingCandidate {
                    image_id: row.get(0)?,
                    image_path: row.get(1)?,
                    modified_at: row.get(2)?,
                    current_source_modified_at: row.get(3)?,
                })
            },
        )
        .map_err(|error| format!("Failed to load natural language scan candidates: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to load natural language scan candidates: {error}"))?;
    drop(stmt);

    set_natural_language_scan_progress_total(progress, candidates.len() as i64);
    set_natural_language_scan_progress_phase(progress, "generating");

    if candidates.is_empty() {
        set_natural_language_scan_progress_phase(progress, "idle");
        return Ok(0);
    }

    let model_root = resolve_chinese_clip_model_dir(None)?;
    let script_path = resolve_chinese_clip_image_service_script_path()?;
    let mut generated = 0i64;
    let mut skipped = 0i64;
    let mut failed = 0i64;
    let mut processed = 0i64;

    for candidate in candidates {
        yield_to_foreground(foreground, stop_requested);
        if natural_language_scan_stop_requested(stop_requested) {
            break;
        }
        wait_for_natural_language_scan_resume(progress, pause_requested, stop_requested);
        if natural_language_scan_stop_requested(stop_requested) {
            break;
        }

        if natural_language_embedding_up_to_date(&candidate) {
            skipped += 1;
            processed += 1;
            set_natural_language_scan_progress_counts(
                progress, processed, generated, skipped, failed,
            );
            continue;
        }

        if !Path::new(&candidate.image_path).is_file() {
            skipped += 1;
            processed += 1;
            set_natural_language_scan_progress_counts(
                progress, processed, generated, skipped, failed,
            );
            continue;
        }

        touch_clip_image_service_last_used(clip_image_encoder_last_used_at);
        let embedding_result = {
            let mut image_service_guard = clip_image_encoder_service
                .lock()
                .map_err(|_| "Clip image encoder service is locked".to_string())?;
            run_chinese_clip_image_embedding_via_service_with_recovery(
                &mut image_service_guard,
                &model_root,
                &script_path,
                &candidate.image_path,
            )
        };
        touch_clip_image_service_last_used(clip_image_encoder_last_used_at);
        ensure_clip_image_service_idle_reaper_started(
            clip_image_encoder_service,
            clip_image_encoder_last_used_at,
            clip_image_encoder_release_worker_running,
        );

        match embedding_result {
            Ok(vector) => {
                if let Err(error) = upsert_image_clip_embedding(
                    &conn,
                    &candidate.image_id,
                    &vector,
                    candidate.modified_at,
                ) {
                    failed += 1;
                    set_natural_language_scan_progress_error(progress, &error);
                    push_natural_language_scan_recent_error(progress, &error);
                } else {
                    upsert_clip_vector_cache_entry(
                        clip_vector_cache,
                        &candidate.image_id,
                        vector.clone(),
                    );
                    generated += 1;
                }
            }
            Err(error) => {
                failed += 1;
                set_natural_language_scan_progress_error(progress, &error);
                push_natural_language_scan_recent_error(progress, &error);
                eprintln!("[clip-scan] {}", error);
            }
        }
        processed += 1;
        set_natural_language_scan_progress_counts(progress, processed, generated, skipped, failed);
    }

    set_natural_language_scan_progress_phase(progress, "idle");
    Ok(generated)
}

pub(super) fn natural_language_embedding_up_to_date(
    candidate: &NaturalLanguageEmbeddingCandidate,
) -> bool {
    matches!(candidate.current_source_modified_at, Some(source_modified_at) if source_modified_at == candidate.modified_at)
}

pub(super) fn upsert_image_clip_embedding(
    conn: &Connection,
    image_id: &str,
    vector: &[f32],
    source_modified_at: i64,
) -> Result<(), String> {
    if vector.is_empty() {
        return Err("CLIP embedding is empty".to_string());
    }
    let normalized_vector = normalize_vector(vector);
    let dim = i64::try_from(normalized_vector.len())
        .map_err(|_| "CLIP embedding dimension overflow".to_string())?;
    let vector_blob = encode_f32_blob(&normalized_vector);
    let now = now_ms();
    let model_pk = compact::clip_model(conn, CHINESE_CLIP_MODEL_ID, CHINESE_CLIP_MODEL_VERSION)?;
    conn.execute(
        "
        INSERT INTO storage_clip_embeddings (
          image_id, model_pk, dimension, normalized, vector_blob, source_modified_at, updated_at
        )
        VALUES ((SELECT image_pk FROM images WHERE id=?1), ?2, ?3, 1, ?4, ?5, ?6)
        ON CONFLICT(image_id, model_pk) DO UPDATE SET
          dimension = excluded.dimension,
          normalized = excluded.normalized,
          vector_blob = excluded.vector_blob,
          source_modified_at = excluded.source_modified_at,
          updated_at = excluded.updated_at
        ",
        params![
            image_id,
            model_pk,
            dim,
            vector_blob,
            source_modified_at,
            now
        ],
    )
    .map_err(|error| format!("Failed to save CLIP embedding: {error}"))?;
    Ok(())
}

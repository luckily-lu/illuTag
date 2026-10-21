use super::*;

pub fn warmup_clip_vector_cache(state: &AppState) -> Result<(), String> {
    ensure_clip_vector_cache_loaded(state)
}

pub(super) fn ensure_clip_vector_cache_loaded(state: &AppState) -> Result<(), String> {
    let _load_guard = state
        .clip_load_lock
        .lock()
        .map_err(|_| "向量加载状态不可用".to_string())?;
    {
        let cache = state
            .clip_vector_cache
            .lock()
            .map_err(|_| "Clip vector cache state is locked".to_string())?;
        if cache.is_some() {
            return Ok(());
        }
    }

    let conn = open_database(&state.database_path)?;
    for _ in 0..3 {
        let before: i64 = conn
            .query_row("PRAGMA data_version", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let loaded = load_clip_vector_cache_from_database(&conn)?;
        let mut cache = state.clip_vector_cache.lock().map_err(|e| e.to_string())?;
        let after: i64 = conn
            .query_row("PRAGMA data_version", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if before == after {
            *cache = Some(loaded);
            return Ok(());
        }
    }
    Err("图库索引正在更新，请稍后重试自然语言搜索".into())
}

pub(super) fn clear_optional_cache<T>(cache: &Arc<Mutex<Option<T>>>) {
    if let Ok(mut value) = cache.lock() {
        *value = None;
    }
}

pub(super) fn invalidate_all_similarity_caches(state: &AppState) {
    clear_optional_cache(&state.clip_vector_cache);
}

pub(super) fn load_clip_vector_cache_from_database(
    conn: &Connection,
) -> Result<ClipImageVectorCache, String> {
    let mut stmt = conn
        .prepare(
            "
            SELECT e.image_id, e.dimension, e.normalized, e.vector_blob
            FROM image_clip_embeddings e
            JOIN images i ON i.id = e.image_id
            WHERE e.model_id = ?1
              AND e.model_version = ?2
              AND i.source = 'library'
              AND COALESCE(i.trashed, 0) = 0
            ",
        )
        .map_err(|error| format!("Failed to prepare clip vector cache query: {error}"))?;
    let rows = stmt
        .query_map(
            params![CHINESE_CLIP_MODEL_ID, CHINESE_CLIP_MODEL_VERSION],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            },
        )
        .map_err(|error| format!("Failed to query clip vector cache: {error}"))?;
    let mut vectors = HashMap::<String, Vec<f32>>::new();
    let mut dimension = 0usize;
    for row in rows {
        let (image_id, stored_dimension, normalized_flag, blob) = row.map_err(|e| e.to_string())?;
        let mut vector = decode_f32_blob(&blob)?;
        if vector.is_empty() {
            continue;
        }
        if normalized_flag == 0 {
            vector = normalize_vector(&vector);
        }
        let expected_dimension = usize::try_from(stored_dimension).unwrap_or(0);
        if expected_dimension > 0 && vector.len() != expected_dimension {
            continue;
        }
        if dimension == 0 {
            dimension = vector.len();
        }
        if vector.len() != dimension {
            continue;
        }
        vectors.insert(image_id, vector);
    }

    Ok(ClipImageVectorCache {
        model_id: CHINESE_CLIP_MODEL_ID.to_string(),
        model_version: CHINESE_CLIP_MODEL_VERSION.to_string(),
        dimension,
        vectors,
    })
}

pub(super) fn remove_clip_vector_cache_entry(
    clip_vector_cache: &Arc<Mutex<Option<ClipImageVectorCache>>>,
    image_id: &str,
) -> Result<(), String> {
    let mut cache = clip_vector_cache
        .lock()
        .map_err(|_| "Clip vector cache state is locked".to_string())?;
    if let Some(cache) = cache.as_mut() {
        cache.vectors.remove(image_id);
    }
    Ok(())
}

pub(super) fn upsert_clip_vector_cache_entry(
    clip_vector_cache: &Arc<Mutex<Option<ClipImageVectorCache>>>,
    image_id: &str,
    vector: Vec<f32>,
) {
    if vector.is_empty() {
        return;
    }
    if let Ok(mut cache) = clip_vector_cache.lock() {
        if let Some(cache) = cache.as_mut() {
            if cache.dimension == 0 {
                cache.dimension = vector.len();
            }
            if cache.dimension != vector.len() {
                return;
            }
            cache.vectors.insert(image_id.to_string(), vector);
        }
    }
}

pub(super) fn stop_python_child_service<T, F>(service: &mut Option<T>, mut child_accessor: F)
where
    F: FnMut(&mut T) -> &mut Child,
{
    if let Some(mut running) = service.take() {
        let _ = child_accessor(&mut running).kill();
    }
}

pub(super) fn release_wd_tagger_service(service: &Arc<Mutex<Option<WdTaggerService>>>) {
    if let Ok(mut guard) = service.lock() {
        stop_python_child_service(&mut *guard, |running| &mut running.child);
    }
}

pub(super) fn release_clip_image_service(service: &Arc<Mutex<Option<ClipImageEncoderService>>>) {
    if let Ok(mut guard) = service.lock() {
        stop_python_child_service(&mut *guard, |running| &mut running.child);
    }
}

pub(super) fn touch_clip_image_service_last_used(last_used_at: &Arc<Mutex<i64>>) {
    if let Ok(mut last_used) = last_used_at.lock() {
        *last_used = now_ms();
    }
}

pub(super) fn ensure_clip_image_service_idle_reaper_started(
    service: &Arc<Mutex<Option<ClipImageEncoderService>>>,
    last_used_at: &Arc<Mutex<i64>>,
    worker_running: &Arc<Mutex<bool>>,
) {
    let should_spawn = if let Ok(mut running) = worker_running.lock() {
        if *running {
            false
        } else {
            *running = true;
            true
        }
    } else {
        false
    };
    if !should_spawn {
        return;
    }

    let service = Arc::clone(service);
    let last_used_at = Arc::clone(last_used_at);
    let worker_running = Arc::clone(worker_running);
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_millis(
                CLIP_IMAGE_SERVICE_IDLE_CHECK_INTERVAL_MS,
            ));

            let last_used = match last_used_at.lock() {
                Ok(value) => *value,
                Err(_) => 0,
            };
            let now = now_ms();
            if now.saturating_sub(last_used) < CLIP_IMAGE_SERVICE_IDLE_RELEASE_MS {
                let has_service = match service.lock() {
                    Ok(guard) => guard.is_some(),
                    Err(_) => false,
                };
                if has_service {
                    continue;
                }
                break;
            }
            let latest_last_used = match last_used_at.lock() {
                Ok(value) => *value,
                Err(_) => 0,
            };
            if now_ms().saturating_sub(latest_last_used) >= CLIP_IMAGE_SERVICE_IDLE_RELEASE_MS {
                release_clip_image_service(&service);
                break;
            }
        }

        if let Ok(mut running) = worker_running.lock() {
            *running = false;
        }
    });
}

pub(super) fn run_chinese_clip_text_embedding_via_service(
    text: &str,
    state: &AppState,
) -> Result<Vec<f32>, String> {
    let model_root = resolve_chinese_clip_model_dir(None)?;
    let script_path = resolve_chinese_clip_text_service_script_path()?;
    let mut service_guard = state
        .clip_text_encoder_service
        .lock()
        .map_err(|_| "Clip text encoder service is locked".to_string())?;
    ensure_clip_text_service_started(&mut service_guard, &model_root, &script_path)?;
    let request = serde_json::json!({ "text": text });
    let primary = {
        let service = service_guard
            .as_mut()
            .ok_or_else(|| "Clip text encoder service unavailable".to_string())?;
        run_clip_service_request(
            &mut service.stdin,
            &mut service.stdout,
            request.clone(),
            "text",
        )
    };
    let vector = match primary {
        Ok(vector) => vector,
        Err(first_error) => {
            stop_python_child_service(&mut service_guard, |running| &mut running.child);
            ensure_clip_text_service_started(&mut service_guard, &model_root, &script_path)?;
            let service = service_guard
                .as_mut()
                .ok_or_else(|| "Clip text encoder service unavailable after restart".to_string())?;
            let retry =
                run_clip_service_request(&mut service.stdin, &mut service.stdout, request, "text");
            if retry.is_err() {
                stop_python_child_service(&mut service_guard, |running| &mut running.child);
            }
            retry.map_err(|second_error| {
                    format!(
                        "Clip text encoder failed and restart retry also failed. first: {first_error}; second: {second_error}"
                    )
                })?
        }
    };
    if vector.is_empty() {
        return Err("Clip text response embedding is empty".to_string());
    }
    Ok(normalize_vector(&vector))
}

pub(super) fn ensure_clip_text_service_started(
    service: &mut Option<ClipTextEncoderService>,
    model_root: &Path,
    script_path: &Path,
) -> Result<(), String> {
    let need_restart = match service.as_ref() {
        None => true,
        Some(existing) => existing.model_root != model_root,
    };
    if !need_restart {
        return Ok(());
    }

    stop_python_child_service(service, |running| &mut running.child);
    *service = Some(spawn_clip_text_service(model_root, script_path)?);
    Ok(())
}

pub(super) fn spawn_clip_text_service(
    model_root: &Path,
    script_path: &Path,
) -> Result<ClipTextEncoderService, String> {
    let mut command = python_command();
    let mut child = command
        .arg("-X")
        .arg("utf8")
        .arg(script_path)
        .arg("--model-dir")
        .arg(model_root)
        .arg("--provider")
        .arg("cpu")
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("Failed to start clip text encoder service: {error}"))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Clip text encoder stdin unavailable".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Clip text encoder stdout unavailable".to_string())?;

    Ok(ClipTextEncoderService {
        child,
        stdin,
        stdout: crate::protocol::ResponseReader::new(stdout),
        model_root: model_root.to_path_buf(),
    })
}

pub(super) fn run_chinese_clip_image_embedding_via_service(
    image_path: &str,
    service: &mut ClipImageEncoderService,
) -> Result<Vec<f32>, String> {
    let request = serde_json::json!({ "image_path": image_path });
    let vector =
        run_clip_service_request(&mut service.stdin, &mut service.stdout, request, "image")?;
    if vector.is_empty() {
        return Err("Clip image response embedding is empty".to_string());
    }
    Ok(normalize_vector(&vector))
}

pub(super) fn run_chinese_clip_image_embedding_via_service_with_recovery(
    service: &mut Option<ClipImageEncoderService>,
    model_root: &Path,
    script_path: &Path,
    image_path: &str,
) -> Result<Vec<f32>, String> {
    ensure_clip_image_service_started(service, model_root, script_path)?;
    let primary = {
        let running = service
            .as_mut()
            .ok_or_else(|| "Clip image encoder service unavailable".to_string())?;
        run_chinese_clip_image_embedding_via_service(image_path, running)
    };
    match primary {
        Ok(vector) => Ok(vector),
        Err(first_error) => {
            stop_python_child_service(service, |running| &mut running.child);
            ensure_clip_image_service_started(service, model_root, script_path)?;
            let running = service.as_mut().ok_or_else(|| {
                "Clip image encoder service unavailable after restart".to_string()
            })?;
            let retry = run_chinese_clip_image_embedding_via_service(image_path, running);
            if retry.is_err() {
                stop_python_child_service(service, |running| &mut running.child);
            }
            retry.map_err(|second_error| {
                format!(
                    "Clip image encoder failed and restart retry also failed. first: {first_error}; second: {second_error}"
                )
            })
        }
    }
}

pub(super) fn ensure_clip_image_service_started(
    service: &mut Option<ClipImageEncoderService>,
    model_root: &Path,
    script_path: &Path,
) -> Result<(), String> {
    let need_restart = match service.as_ref() {
        None => true,
        Some(existing) => existing.model_root != model_root,
    };
    if !need_restart {
        return Ok(());
    }

    stop_python_child_service(service, |running| &mut running.child);
    *service = Some(spawn_clip_image_service(model_root, script_path)?);
    Ok(())
}

pub(super) fn spawn_clip_image_service(
    model_root: &Path,
    script_path: &Path,
) -> Result<ClipImageEncoderService, String> {
    let mut command = python_command();
    let mut child = command
        .arg("-X")
        .arg("utf8")
        .arg(script_path)
        .arg("--model-dir")
        .arg(model_root)
        .arg("--provider")
        .arg("cpu")
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("Failed to start clip image encoder service: {error}"))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Clip image encoder stdin unavailable".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Clip image encoder stdout unavailable".to_string())?;

    Ok(ClipImageEncoderService {
        child,
        stdin,
        stdout: crate::protocol::ResponseReader::new(stdout),
        model_root: model_root.to_path_buf(),
    })
}

pub(super) fn run_clip_service_request(
    stdin: &mut ChildStdin,
    stdout: &mut crate::protocol::ResponseReader,
    request: serde_json::Value,
    mode: &str,
) -> Result<Vec<f32>, String> {
    stdin
        .write_all(request.to_string().as_bytes())
        .and_then(|_| stdin.write_all(b"\n"))
        .and_then(|_| stdin.flush())
        .map_err(|error| format!("Failed to write clip {mode} request: {error}"))?;

    let mut response_line = String::new();
    stdout
        .read_line(&mut response_line)
        .map_err(|error| format!("Failed to read clip {mode} response: {error}"))?;
    if response_line.trim().is_empty() {
        return Err(format!("Clip {mode} encoder returned empty response"));
    }
    let value: serde_json::Value = serde_json::from_str(response_line.trim())
        .map_err(|error| format!("Invalid clip {mode} response: {error}"))?;
    if let Some(error_text) = value.get("error").and_then(|item| item.as_str()) {
        return Err(format!("Clip {mode} encoder error: {error_text}"));
    }
    let array = value
        .get("embedding")
        .and_then(|item| item.as_array())
        .ok_or_else(|| format!("Clip {mode} response missing embedding"))?;
    let mut vector = Vec::<f32>::with_capacity(array.len());
    for item in array {
        let number = item
            .as_f64()
            .ok_or_else(|| format!("Clip {mode} response contains invalid value"))?;
        vector.push(number as f32);
    }
    Ok(vector)
}

pub(super) fn encode_f32_blob(vector: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::<u8>::with_capacity(vector.len() * 4);
    for value in vector {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

pub(super) fn decode_f32_blob(blob: &[u8]) -> Result<Vec<f32>, String> {
    if blob.len() % 4 != 0 {
        return Err("Invalid clip embedding blob length".to_string());
    }
    let mut values = Vec::<f32>::with_capacity(blob.len() / 4);
    for chunk in blob.chunks_exact(4) {
        values.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    Ok(values)
}

pub(super) fn dot_product(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .zip(right.iter())
        .fold(0f32, |acc, (lv, rv)| acc + lv * rv)
}

#[cfg(test)]
pub(super) fn legacy_model_name_to_model_id(legacy: &str) -> &str {
    let normalized = legacy.trim().to_lowercase();
    if normalized.contains("chinese-clip") || normalized.contains("cn_clip") {
        CHINESE_CLIP_MODEL_ID
    } else {
        CHINESE_CLIP_MODEL_ID
    }
}

pub(super) fn normalize_vector(input: &[f32]) -> Vec<f32> {
    let mut sum = 0f64;
    for value in input {
        let v = *value as f64;
        sum += v * v;
    }
    if sum <= 1e-18 {
        return input.to_vec();
    }
    let inv = 1.0f64 / sum.sqrt();
    input
        .iter()
        .map(|value| (*value as f64 * inv) as f32)
        .collect()
}

pub(super) fn ensure_thumbnail_root_dir(database_path: &Path) -> Result<PathBuf, String> {
    let root = database_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("thumbs")
        .join("library");
    fs::create_dir_all(&root)
        .map_err(|error| format!("Failed to create thumbnail cache directory: {error}"))?;
    Ok(root)
}

pub(super) fn clear_thumbnail_for_missing_image(
    conn: &Connection,
    image_id: &str,
) -> Result<(), String> {
    let previous_thumb: Option<String> = conn
        .query_row(
            "SELECT thumb_path FROM image_thumbnails WHERE image_id = ?1",
            params![image_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("Failed to query stale thumbnail: {error}"))?
        .flatten();

    conn.execute(
        "DELETE FROM storage_image_thumbnails WHERE image_id = (SELECT image_pk FROM images WHERE id=?1)",
        params![image_id],
    )
    .map_err(|error| format!("Failed to clear stale thumbnail: {error}"))?;

    if let Some(path) = previous_thumb {
        let _ = fs::remove_file(path);
    }
    Ok(())
}

pub(super) fn clear_thumbnail_cache_storage(
    conn: &Connection,
    database_path: &Path,
) -> Result<(), String> {
    conn.execute("DELETE FROM storage_image_thumbnails", [])
        .map_err(|error| format!("Failed to clear thumbnail table: {error}"))?;
    let thumb_root = ensure_thumbnail_root_dir(database_path)?;
    if thumb_root.exists() {
        fs::remove_dir_all(&thumb_root)
            .map_err(|error| format!("Failed to remove thumbnail cache directory: {error}"))?;
    }
    fs::create_dir_all(&thumb_root)
        .map_err(|error| format!("Failed to recreate thumbnail cache directory: {error}"))?;
    Ok(())
}

pub(super) fn cleanup_orphan_thumbnail_files(
    conn: &Connection,
    thumb_root: &Path,
) -> Result<(), String> {
    let mut stmt = conn
        .prepare("SELECT image_id, thumb_path FROM image_thumbnails")
        .map_err(|error| format!("Failed to query thumbnail paths: {error}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| format!("Failed to query thumbnail paths: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to query thumbnail paths: {error}"))?;
    drop(stmt);

    let _ = rows;
    let _ = thumb_root;
    Ok(())
}

pub(super) fn expected_thumbnail_path_for_image_id(thumb_root: &Path, image_id: &str) -> String {
    thumb_root
        .join(format!("{}.webp", stable_hash_hex(image_id)))
        .to_string_lossy()
        .to_string()
}

pub(super) fn repair_thumbnail_paths_for_current_data_dir(
    conn: &Connection,
    thumb_root: &Path,
    candidates: &mut [ThumbnailCandidate],
) -> Result<(), String> {
    for candidate in candidates {
        // Path repair may only relocate an already valid thumbnail. If the
        // record is missing or its source signature is stale, an existing file
        // at the expected path proves nothing about its pixels; leave it for the
        // real generation path instead of certifying stale content as current.
        if !thumbnail_up_to_date(candidate) {
            continue;
        }
        let expected_thumb_path =
            expected_thumbnail_path_for_image_id(thumb_root, &candidate.image_id);
        if candidate.current_thumb_path.as_deref() == Some(expected_thumb_path.as_str()) {
            continue;
        }
        if !Path::new(&expected_thumb_path).is_file() {
            continue;
        }
        upsert_thumbnail_record(
            conn,
            &candidate.image_id,
            &expected_thumb_path,
            candidate.modified_at,
            candidate.file_size,
        )?;
        candidate.current_thumb_path = Some(expected_thumb_path);
        candidate.current_source_modified_at = Some(candidate.modified_at);
        candidate.current_source_file_size = Some(candidate.file_size);
    }
    Ok(())
}

pub(super) fn thumbnail_stop_requested(stop_requested: &Arc<Mutex<bool>>) -> bool {
    stop_requested.lock().map(|value| *value).unwrap_or(false)
}

pub(super) fn thumbnail_pause_requested(pause_requested: &Arc<Mutex<bool>>) -> bool {
    pause_requested.lock().map(|value| *value).unwrap_or(false)
}

pub(super) fn wait_for_thumbnail_resume(
    progress: &Arc<Mutex<ThumbnailGenerationProgress>>,
    pause_requested: &Arc<Mutex<bool>>,
    stop_requested: &Arc<Mutex<bool>>,
) {
    loop {
        if thumbnail_stop_requested(stop_requested) {
            return;
        }
        if thumbnail_pause_requested(pause_requested) {
            set_thumbnail_progress_phase(progress, "paused");
            thread::sleep(std::time::Duration::from_millis(120));
            continue;
        }
        break;
    }
}

pub(super) fn thumbnail_up_to_date(candidate: &ThumbnailCandidate) -> bool {
    let Some(thumb_path) = candidate.current_thumb_path.as_ref() else {
        return false;
    };
    if !Path::new(thumb_path).is_file() {
        return false;
    }
    match (
        candidate.current_source_modified_at,
        candidate.current_source_file_size,
    ) {
        (Some(modified_at), Some(file_size)) => {
            modified_at == candidate.modified_at && file_size == candidate.file_size
        }
        _ => false,
    }
}

pub(super) fn generate_single_thumbnail(
    candidate: &ThumbnailCandidate,
    thumb_root: &Path,
    long_edge: u32,
    quality: f32,
) -> Result<String, String> {
    let source = ImageReader::open(&candidate.image_path)
        .map_err(|error| format!("Failed to open image for thumbnail: {error}"))?
        .decode()
        .map_err(|error| format!("Failed to decode image for thumbnail: {error}"))?;

    let (src_w, src_h) = source.dimensions();
    if src_w == 0 || src_h == 0 {
        return Err("Invalid image dimensions for thumbnail".to_string());
    }
    let (dst_w, dst_h) = fit_long_edge(src_w, src_h, long_edge);

    let resized = if dst_w == src_w && dst_h == src_h {
        source
    } else {
        source.resize(dst_w, dst_h, FilterType::Triangle)
    };
    let rgba = resized.to_rgba8();
    let (encoded_w, encoded_h) = rgba.dimensions();
    let mut encoded = Vec::<u8>::new();
    {
        let mut cursor = Cursor::new(&mut encoded);
        let encoder = webp::Encoder::from_rgba(rgba.as_raw(), encoded_w, encoded_h);
        let webp = encoder.encode(quality);
        cursor
            .write_all(webp.as_ref())
            .map_err(|error| format!("Failed to encode thumbnail webp: {error}"))?;
    }

    let thumb_path = PathBuf::from(expected_thumbnail_path_for_image_id(
        thumb_root,
        &candidate.image_id,
    ));
    fs::write(&thumb_path, encoded)
        .map_err(|error| format!("Failed to write thumbnail file: {error}"))?;
    Ok(thumb_path.to_string_lossy().to_string())
}

pub(super) fn fit_long_edge(width: u32, height: u32, max_long_edge: u32) -> (u32, u32) {
    let max_side = width.max(height);
    if max_side <= max_long_edge {
        return (width.max(1), height.max(1));
    }
    let scale = max_long_edge as f64 / max_side as f64;
    let dst_w = ((width as f64 * scale).round() as u32).max(1);
    let dst_h = ((height as f64 * scale).round() as u32).max(1);
    (dst_w, dst_h)
}

pub(super) fn stable_hash_hex(value: &str) -> String {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;
    let mut hash = FNV_OFFSET;
    for &byte in value.as_bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{hash:016x}")
}

pub(super) fn upsert_thumbnail_record(
    conn: &Connection,
    image_id: &str,
    thumb_path: &str,
    source_modified_at: i64,
    source_file_size: i64,
) -> Result<(), String> {
    conn.execute(
        "
        INSERT INTO storage_image_thumbnails (
          image_id, thumb_path, source_modified_at, source_file_size, updated_at
        )
        VALUES ((SELECT image_pk FROM images WHERE id=?1), ?2, ?3, ?4, ?5)
        ON CONFLICT(image_id) DO UPDATE SET
          thumb_path = excluded.thumb_path,
          source_modified_at = excluded.source_modified_at,
          source_file_size = excluded.source_file_size,
          updated_at = excluded.updated_at
        ",
        params![
            image_id,
            thumb_path,
            source_modified_at,
            source_file_size,
            now_ms()
        ],
    )
    .map_err(|error| format!("Failed to upsert thumbnail record: {error}"))?;
    Ok(())
}

pub(super) fn load_cn_tag_dictionary_map() -> Result<HashMap<String, String>, String> {
    let dictionary_path = resolve_dictionary_source_path()?;
    if !dictionary_path.is_file() {
        return Ok(HashMap::new());
    }
    let exact_pairs = load_cn_tag_dictionary_pairs_from_source(&dictionary_path)?;
    Ok(build_lookup_dictionary_map(&exact_pairs))
}

pub(super) fn load_cn_tag_dictionary_pairs_from_source(
    path: &Path,
) -> Result<HashMap<String, String>, String> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if extension == "csv" {
        return load_cn_tag_dictionary_pairs_from_csv(path);
    }
    load_cn_tag_dictionary_pairs_from_xlsx(path)
}

pub(super) fn load_cn_tag_dictionary_pairs_from_csv(
    path: &Path,
) -> Result<HashMap<String, String>, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("Failed to read dictionary CSV {}: {error}", path.display()))?;
    let mut pairs = HashMap::<String, String>::new();
    for (line_index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let fields = parse_csv_line(trimmed);
        let tag_en = fields.get(1).map(|value| value.trim()).unwrap_or("");
        let tag_zh = fields.get(4).map(|value| value.trim()).unwrap_or("");
        if tag_en.is_empty() || tag_zh.is_empty() {
            continue;
        }
        if line_index == 0
            && (tag_en.eq_ignore_ascii_case("tag")
                || tag_en.eq_ignore_ascii_case("url")
                || tag_en.eq_ignore_ascii_case("english")
                || tag_en.eq_ignore_ascii_case("en")
                || tag_zh.contains("翻译")
                || tag_zh.contains("中文"))
        {
            continue;
        }
        pairs.insert(tag_en.to_string(), tag_zh.to_string());
    }
    Ok(pairs)
}

pub(super) fn load_cn_tag_dictionary_pairs_from_xlsx(
    path: &Path,
) -> Result<HashMap<String, String>, String> {
    let mut workbook = open_workbook_auto(path).map_err(|error| {
        format!(
            "Failed to open dictionary workbook {}: {error}",
            path.display()
        )
    })?;
    let sheet_name = workbook
        .sheet_names()
        .first()
        .cloned()
        .ok_or_else(|| "Dictionary workbook has no sheets".to_string())?;
    let range = workbook
        .worksheet_range(&sheet_name)
        .map_err(|error| format!("Failed to read dictionary sheet: {error}"))?;

    let mut rows = range.rows();
    let header = rows
        .next()
        .ok_or_else(|| "Dictionary sheet is empty".to_string())?;
    let header_names: Vec<String> = header
        .iter()
        .map(|cell| {
            excel_cell_to_string(Some(cell))
                .unwrap_or_default()
                .to_ascii_lowercase()
        })
        .collect();
    let tag_idx = header_names
        .iter()
        .position(|name| name == "tag")
        .or_else(|| header_names.iter().position(|name| name == "url"))
        .unwrap_or(2);
    let cn_idx = header_names
        .iter()
        .position(|name| name.contains("right_tag_cn") || name.ends_with("_cn") || name == "cn")
        .unwrap_or(3);

    let mut pairs = HashMap::<String, String>::new();
    for row in rows {
        let tag_en = excel_cell_to_string(row.get(tag_idx)).unwrap_or_default();
        let tag_zh = excel_cell_to_string(row.get(cn_idx)).unwrap_or_default();
        let tag_en = tag_en.trim();
        let tag_zh = tag_zh.trim();
        if tag_en.is_empty() || tag_zh.is_empty() {
            continue;
        }
        if tag_en.eq_ignore_ascii_case("tag")
            || tag_en.eq_ignore_ascii_case("url")
            || tag_en.eq_ignore_ascii_case("english")
            || tag_en.eq_ignore_ascii_case("en")
        {
            continue;
        }
        pairs.insert(tag_en.to_string(), tag_zh.to_string());
    }
    Ok(pairs)
}

pub(super) fn build_lookup_dictionary_map(
    pairs: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut map = HashMap::<String, String>::with_capacity(pairs.len() * 2);
    for (tag_en, tag_zh) in pairs {
        map.insert(tag_en.clone(), tag_zh.clone());
        map.insert(normalize_tag_key(tag_en), tag_zh.clone());
    }
    map
}

pub(super) fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::<String>::new();
    let mut current = String::new();
    let mut chars = line.chars().peekable();
    let mut in_quotes = false;
    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                if in_quotes {
                    if matches!(chars.peek(), Some('"')) {
                        current.push('"');
                        let _ = chars.next();
                    } else {
                        in_quotes = false;
                    }
                } else {
                    in_quotes = true;
                }
            }
            ',' if !in_quotes => {
                fields.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    fields.push(current.trim().to_string());
    fields
}

pub(super) fn excel_cell_to_string(cell: Option<&ExcelCell>) -> Option<String> {
    let cell = cell?;
    let text = match cell {
        ExcelCell::String(value) => value.clone(),
        ExcelCell::Float(value) => value.to_string(),
        ExcelCell::Int(value) => value.to_string(),
        ExcelCell::Bool(value) => value.to_string(),
        ExcelCell::DateTime(value) => value.to_string(),
        ExcelCell::DateTimeIso(value) => value.clone(),
        ExcelCell::DurationIso(value) => value.clone(),
        ExcelCell::Error(_) | ExcelCell::Empty => String::new(),
    };
    let normalized = text.trim().to_string();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

pub(super) fn normalize_tag_key(tag: &str) -> String {
    tag.trim()
        .to_ascii_lowercase()
        .replace('-', "_")
        .replace(' ', "_")
}

pub(super) fn escape_like_pattern(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '%' => escaped.push_str("\\%"),
            '_' => escaped.push_str("\\_"),
            '\\' => escaped.push_str("\\\\"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

pub(super) fn split_search_tokens(input: &str) -> Vec<String> {
    let mut tokens = input
        .split_whitespace()
        .map(|token| token.trim().to_ascii_lowercase())
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    tokens.sort();
    tokens.dedup();
    tokens
}

pub(super) fn lookup_cn_tag(dictionary: &HashMap<String, String>, tag_en: &str) -> Option<String> {
    if let Some(value) = dictionary.get(tag_en) {
        return Some(value.clone());
    }
    let normalized = normalize_tag_key(tag_en);
    dictionary.get(&normalized).cloned()
}

pub(super) fn prioritized_resource_candidates(
    executable_dir: Option<&Path>,
    working_dir: Option<&Path>,
    bundled_paths: &[&str],
    development_paths: &[&str],
    executable_fallback_paths: &[&str],
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(dir) = executable_dir {
        candidates.extend(bundled_paths.iter().map(|path| dir.join(path)));
    }
    if let Some(dir) = working_dir {
        candidates.extend(development_paths.iter().map(|path| dir.join(path)));
    }
    if let Some(dir) = executable_dir {
        candidates.extend(executable_fallback_paths.iter().map(|path| dir.join(path)));
    }
    candidates
}

pub(super) fn resource_candidates(
    bundled_paths: &[&str],
    development_paths: &[&str],
    executable_fallback_paths: &[&str],
) -> Vec<PathBuf> {
    let executable = env::current_exe().ok();
    let working_dir = env::current_dir().ok();
    prioritized_resource_candidates(
        executable.as_deref().and_then(Path::parent),
        working_dir.as_deref(),
        bundled_paths,
        development_paths,
        executable_fallback_paths,
    )
}

pub(super) fn resource_path_from_root(
    root: Option<&Path>,
    relative_path: &str,
) -> Result<Option<PathBuf>, String> {
    match root {
        Some(root) if !root.is_absolute() => Err("隔离资源根目录必须是绝对路径".into()),
        Some(root) => Ok(Some(root.join(relative_path))),
        None => Ok(None),
    }
}

pub(super) fn explicit_resource_path(relative_path: &str) -> Result<Option<PathBuf>, String> {
    let root = env::var_os("ILLUTAG_RESOURCE_ROOT").map(PathBuf::from);
    resource_path_from_root(root.as_deref(), relative_path)
}

pub(super) fn resolve_resource_file_with_roots(
    resource_root: Option<&Path>,
    executable_dir: Option<&Path>,
    working_dir: Option<&Path>,
    bundled_path: &str,
    development_paths: &[&str],
    executable_fallback_paths: &[&str],
) -> Result<PathBuf, String> {
    if let Some(path) = resource_path_from_root(resource_root, bundled_path)? {
        return path
            .is_file()
            .then_some(path.clone())
            .ok_or_else(|| format!("隔离资源缺失：{}", path.display()));
    }
    prioritized_resource_candidates(
        executable_dir,
        working_dir,
        &[bundled_path],
        development_paths,
        executable_fallback_paths,
    )
    .into_iter()
    .find(|path| path.is_file())
    .ok_or_else(|| format!("找不到程序资源：{bundled_path}"))
}

fn resolve_bundled_service_script(name: &str) -> Result<PathBuf, String> {
    let root = env::var_os("ILLUTAG_RESOURCE_ROOT").map(PathBuf::from);
    let executable = env::current_exe().ok();
    let working_dir = env::current_dir().ok();
    resolve_resource_file_with_roots(
        root.as_deref(),
        executable.as_deref().and_then(Path::parent),
        working_dir.as_deref(),
        &format!("scripts/{name}"),
        &[
            &format!("src-tauri/scripts/{name}"),
            &format!("scripts/{name}"),
        ],
        &[
            &format!("../scripts/{name}"),
            &format!("../../src-tauri/scripts/{name}"),
        ],
    )
}

pub(super) fn resolve_dictionary_source_path() -> Result<PathBuf, String> {
    let root = env::var_os("ILLUTAG_RESOURCE_ROOT").map(PathBuf::from);
    let relative = [
        "wd-swinv2-tagger-v3/selected_tags_full_translation.csv",
        "wd-swinv2-tagger-v3/dictionary01.xlsx",
    ];
    if let Some(root) = root.as_deref() {
        for name in relative {
            let path = resource_path_from_root(Some(root), name)?.unwrap();
            if path.is_file() {
                return Ok(path);
            }
        }
        return Err(format!("隔离标签字典缺失：{}", root.display()));
    }
    let candidates = resource_candidates(
        &relative,
        &[
            "wd-swinv2-tagger-v3/selected_tags_full_translation.csv",
            "../wd-swinv2-tagger-v3/selected_tags_full_translation.csv",
            "wd-swinv2-tagger-v3/dictionary01.xlsx",
            "../wd-swinv2-tagger-v3/dictionary01.xlsx",
        ],
        &[
            "../wd-swinv2-tagger-v3/selected_tags_full_translation.csv",
            "../../wd-swinv2-tagger-v3/selected_tags_full_translation.csv",
            "../wd-swinv2-tagger-v3/dictionary01.xlsx",
            "../../wd-swinv2-tagger-v3/dictionary01.xlsx",
        ],
    );
    for candidate in &candidates {
        if candidate.is_file() {
            return Ok(candidate.clone());
        }
    }
    Ok(candidates
        .into_iter()
        .next()
        .unwrap_or_else(|| PathBuf::from(relative[0])))
}

pub(super) fn ensure_app_meta_table(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "
        CREATE TABLE IF NOT EXISTS app_meta (
          key TEXT PRIMARY KEY,
          value TEXT NOT NULL,
          updated_at INTEGER NOT NULL
        )
        ",
        [],
    )
    .map_err(|error| format!("Failed to ensure app_meta table: {error}"))?;
    Ok(())
}

pub(super) fn read_app_meta_value(conn: &Connection, key: &str) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT value FROM app_meta WHERE key = ?1",
        params![key],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(|error| format!("Failed to read app_meta ({key}): {error}"))
}

pub(super) fn write_app_meta_value(
    conn: &Connection,
    key: &str,
    value: &str,
) -> Result<(), String> {
    let now = now_ms();
    conn.execute(
        "
        INSERT INTO app_meta (key, value, updated_at)
        VALUES (?1, ?2, ?3)
        ON CONFLICT(key) DO UPDATE SET
          value = excluded.value,
          updated_at = excluded.updated_at
        ",
        params![key, value, now],
    )
    .map_err(|error| format!("Failed to write app_meta ({key}): {error}"))?;
    Ok(())
}

pub(super) fn sync_tag_dictionary_from_source_if_changed(state: &AppState) -> Result<(), String> {
    let source_path = resolve_dictionary_source_path()?;
    if !source_path.is_file() {
        return Ok(());
    }
    let metadata = fs::metadata(&source_path)
        .map_err(|error| format!("Failed to read dictionary source metadata: {error}"))?;
    let modified_at = metadata.modified().ok().map(system_time_ms).unwrap_or(0);
    let signature = format!(
        "{}|{}|{}|{}",
        TAG_DICTIONARY_SOURCE_SCHEMA_VERSION,
        source_path.to_string_lossy(),
        modified_at,
        metadata.len()
    );

    let mut conn = open_database(&state.database_path)?;
    ensure_app_meta_table(&conn)?;
    let last_signature = read_app_meta_value(&conn, "tag_dictionary_source_signature")?;
    if last_signature.as_deref() == Some(signature.as_str()) {
        return Ok(());
    }

    let exact_pairs = load_cn_tag_dictionary_pairs_from_source(&source_path)?;
    let tx = conn
        .transaction()
        .map_err(|error| format!("Failed to open dictionary sync transaction: {error}"))?;
    for (tag_en, tag_zh) in &exact_pairs {
        tx.execute(
            "
            INSERT INTO tag_dictionary (tag_en, tag_zh, updated_at)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(tag_en) DO UPDATE SET
              tag_zh = excluded.tag_zh,
              updated_at = excluded.updated_at
            ",
            params![tag_en, tag_zh, now_ms()],
        )
        .map_err(|error| format!("Failed to upsert tag dictionary from source: {error}"))?;
    }
    write_app_meta_value(&tx, "tag_dictionary_source_signature", &signature)?;
    tx.commit()
        .map_err(|error| format!("Failed to commit dictionary sync transaction: {error}"))?;
    Ok(())
}

pub(super) fn set_thumbnail_progress(
    progress: &Arc<Mutex<ThumbnailGenerationProgress>>,
    next: ThumbnailGenerationProgress,
) {
    if let Ok(mut state) = progress.lock() {
        *state = next;
    }
}

pub(super) fn update_thumbnail_progress<F>(
    progress: &Arc<Mutex<ThumbnailGenerationProgress>>,
    update: F,
) where
    F: FnOnce(&mut ThumbnailGenerationProgress),
{
    if let Ok(mut state) = progress.lock() {
        update(&mut state);
    }
}

pub(super) fn set_thumbnail_progress_phase(
    progress: &Arc<Mutex<ThumbnailGenerationProgress>>,
    phase: &str,
) {
    update_thumbnail_progress(progress, |state| {
        state.phase = phase.to_string();
        state.running = phase != "idle";
        state.paused = phase == "paused";
    });
}

pub(super) fn set_thumbnail_progress_total(
    progress: &Arc<Mutex<ThumbnailGenerationProgress>>,
    total: i64,
) {
    update_thumbnail_progress(progress, |state| {
        state.total_candidates = total.max(0);
    });
}

pub(super) fn set_thumbnail_progress_counts(
    progress: &Arc<Mutex<ThumbnailGenerationProgress>>,
    processed: i64,
    generated: i64,
    skipped: i64,
    failed: i64,
) {
    update_thumbnail_progress(progress, |state| {
        state.processed_images = processed.max(0);
        state.generated_images = generated.max(0);
        state.skipped_images = skipped.max(0);
        state.failed_images = failed.max(0);
    });
}

pub(super) fn set_thumbnail_progress_error(
    progress: &Arc<Mutex<ThumbnailGenerationProgress>>,
    error: &str,
) {
    update_thumbnail_progress(progress, |state| {
        state.last_error = Some(error.to_string());
    });
}

pub(super) fn push_thumbnail_progress_recent_error(
    progress: &Arc<Mutex<ThumbnailGenerationProgress>>,
    error: &str,
) {
    update_thumbnail_progress(progress, |state| {
        let text = error.trim();
        if text.is_empty() {
            return;
        }
        state.recent_errors.push(text.to_string());
        if state.recent_errors.len() > 12 {
            let overflow = state.recent_errors.len() - 12;
            state.recent_errors.drain(0..overflow);
        }
    });
}

pub(super) fn set_thumbnail_progress_done(progress: &Arc<Mutex<ThumbnailGenerationProgress>>) {
    update_thumbnail_progress(progress, |state| {
        state.running = false;
        state.paused = false;
        state.phase = "idle".to_string();
    });
}

pub(super) fn set_natural_language_scan_progress(
    progress: &Arc<Mutex<NaturalLanguageScanProgress>>,
    next: NaturalLanguageScanProgress,
) {
    if let Ok(mut state) = progress.lock() {
        *state = next;
    }
}

pub(super) fn update_natural_language_scan_progress<F>(
    progress: &Arc<Mutex<NaturalLanguageScanProgress>>,
    update: F,
) where
    F: FnOnce(&mut NaturalLanguageScanProgress),
{
    if let Ok(mut state) = progress.lock() {
        update(&mut state);
    }
}

pub(super) fn set_natural_language_scan_progress_phase(
    progress: &Arc<Mutex<NaturalLanguageScanProgress>>,
    phase: &str,
) {
    update_natural_language_scan_progress(progress, |state| {
        state.phase = phase.to_string();
        state.running = phase != "idle";
        state.paused = phase == "paused";
    });
}

pub(super) fn set_natural_language_scan_progress_total(
    progress: &Arc<Mutex<NaturalLanguageScanProgress>>,
    total: i64,
) {
    update_natural_language_scan_progress(progress, |state| {
        state.total_images = total.max(0);
    });
}

pub(super) fn set_natural_language_scan_progress_counts(
    progress: &Arc<Mutex<NaturalLanguageScanProgress>>,
    processed: i64,
    generated: i64,
    skipped: i64,
    failed: i64,
) {
    update_natural_language_scan_progress(progress, |state| {
        state.processed_images = processed.max(0);
        state.generated_images = generated.max(0);
        state.skipped_images = skipped.max(0);
        state.failed_images = failed.max(0);
    });
}

pub(super) fn set_natural_language_scan_progress_error(
    progress: &Arc<Mutex<NaturalLanguageScanProgress>>,
    error: &str,
) {
    update_natural_language_scan_progress(progress, |state| {
        state.last_error = Some(error.to_string());
    });
}

pub(super) fn push_natural_language_scan_recent_error(
    progress: &Arc<Mutex<NaturalLanguageScanProgress>>,
    error: &str,
) {
    update_natural_language_scan_progress(progress, |state| {
        let text = error.trim();
        if text.is_empty() {
            return;
        }
        state.recent_errors.push(text.to_string());
        if state.recent_errors.len() > 12 {
            let overflow = state.recent_errors.len() - 12;
            state.recent_errors.drain(0..overflow);
        }
    });
}

pub(super) fn set_natural_language_scan_progress_done(
    progress: &Arc<Mutex<NaturalLanguageScanProgress>>,
) {
    update_natural_language_scan_progress(progress, |state| {
        state.running = false;
        state.paused = false;
        state.phase = "idle".to_string();
    });
}

pub(super) fn natural_language_scan_stop_requested(stop_requested: &Arc<Mutex<bool>>) -> bool {
    stop_requested.lock().map(|value| *value).unwrap_or(false)
}

pub(super) fn natural_language_scan_pause_requested(pause_requested: &Arc<Mutex<bool>>) -> bool {
    pause_requested.lock().map(|value| *value).unwrap_or(false)
}

pub(super) fn wait_for_natural_language_scan_resume(
    progress: &Arc<Mutex<NaturalLanguageScanProgress>>,
    pause_requested: &Arc<Mutex<bool>>,
    stop_requested: &Arc<Mutex<bool>>,
) {
    loop {
        if natural_language_scan_stop_requested(stop_requested) {
            return;
        }
        if natural_language_scan_pause_requested(pause_requested) {
            set_natural_language_scan_progress_phase(progress, "paused");
            thread::sleep(Duration::from_millis(120));
            continue;
        }
        set_natural_language_scan_progress_phase(progress, "generating");
        return;
    }
}

pub(super) fn set_scan_progress(
    progress: &Arc<Mutex<BackgroundScanProgress>>,
    next: BackgroundScanProgress,
) {
    if let Ok(mut state) = progress.lock() {
        *state = next;
    }
}

pub(super) fn update_scan_progress<F>(progress: &Arc<Mutex<BackgroundScanProgress>>, update: F)
where
    F: FnOnce(&mut BackgroundScanProgress),
{
    if let Ok(mut state) = progress.lock() {
        update(&mut state);
    }
}

pub(super) fn set_scan_progress_phase(progress: &Arc<Mutex<BackgroundScanProgress>>, phase: &str) {
    update_scan_progress(progress, |state| {
        state.phase = phase.to_string();
        state.running = phase != "idle";
        state.paused = phase == "paused";
    });
}

pub(super) fn set_scan_progress_total_folders(
    progress: &Arc<Mutex<BackgroundScanProgress>>,
    total_folders: i64,
) {
    update_scan_progress(progress, |state| {
        state.total_folders = total_folders.max(0);
    });
}

pub(super) fn set_scan_progress_scanned_folders(
    progress: &Arc<Mutex<BackgroundScanProgress>>,
    scanned_folders: i64,
) {
    update_scan_progress(progress, |state| {
        state.scanned_folders = scanned_folders.max(0);
    });
}

pub(super) fn set_scan_progress_scanned_files(
    progress: &Arc<Mutex<BackgroundScanProgress>>,
    scanned_files: i64,
) {
    update_scan_progress(progress, |state| {
        state.scanned_files = scanned_files.max(0);
    });
}

pub(super) fn set_scan_progress_current_folder(
    progress: &Arc<Mutex<BackgroundScanProgress>>,
    folder_path: &str,
) {
    update_scan_progress(progress, |state| {
        state.current_folder = folder_path.to_string();
    });
}

pub(super) fn set_scan_progress_new_images(
    progress: &Arc<Mutex<BackgroundScanProgress>>,
    new_images: i64,
) {
    update_scan_progress(progress, |state| {
        state.new_images = new_images.max(0);
    });
}

pub(super) fn set_scan_progress_updated_images(
    progress: &Arc<Mutex<BackgroundScanProgress>>,
    updated_images: i64,
) {
    update_scan_progress(progress, |state| {
        state.updated_images = updated_images.max(0);
    });
}

pub(super) fn set_scan_progress_skipped_images(
    progress: &Arc<Mutex<BackgroundScanProgress>>,
    skipped_images: i64,
) {
    update_scan_progress(progress, |state| {
        state.skipped_images = skipped_images.max(0);
    });
}

pub(super) fn set_scan_progress_queued_images(
    progress: &Arc<Mutex<BackgroundScanProgress>>,
    queued_images: i64,
) {
    update_scan_progress(progress, |state| {
        state.queued_images = queued_images.max(0);
    });
}

pub(super) fn add_scan_progress_tagged(progress: &Arc<Mutex<BackgroundScanProgress>>, count: i64) {
    update_scan_progress(progress, |state| {
        state.tagged_images += count.max(0);
    });
}

pub(super) fn increment_scan_progress_failed(progress: &Arc<Mutex<BackgroundScanProgress>>) {
    update_scan_progress(progress, |state| {
        state.failed_images += 1;
    });
}

pub(super) fn set_scan_progress_error(progress: &Arc<Mutex<BackgroundScanProgress>>, error: &str) {
    update_scan_progress(progress, |state| {
        state.last_error = Some(error.to_string());
    });
}

pub(super) fn push_scan_progress_recent_error(
    progress: &Arc<Mutex<BackgroundScanProgress>>,
    error: &str,
) {
    update_scan_progress(progress, |state| {
        let text = error.trim();
        if text.is_empty() {
            return;
        }
        state.recent_errors.push(text.to_string());
        if state.recent_errors.len() > 12 {
            let overflow = state.recent_errors.len() - 12;
            state.recent_errors.drain(0..overflow);
        }
    });
}

pub(super) fn set_scan_progress_done(progress: &Arc<Mutex<BackgroundScanProgress>>) {
    update_scan_progress(progress, |state| {
        state.running = false;
        state.paused = false;
        state.phase = "idle".to_string();
    });
}

pub(super) fn background_scan_stop_requested(stop_requested: &Arc<Mutex<bool>>) -> bool {
    stop_requested.lock().map(|value| *value).unwrap_or(false)
}

pub(super) fn background_scan_pause_requested(pause_requested: &Arc<Mutex<bool>>) -> bool {
    pause_requested.lock().map(|value| *value).unwrap_or(false)
}

pub(super) fn wait_for_background_scan_resume(
    progress: &Arc<Mutex<BackgroundScanProgress>>,
    pause_requested: &Arc<Mutex<bool>>,
    stop_requested: &Arc<Mutex<bool>>,
    resume_phase: &str,
) {
    loop {
        if background_scan_stop_requested(stop_requested) {
            return;
        }
        if background_scan_pause_requested(pause_requested) {
            set_scan_progress_phase(progress, "paused");
            thread::sleep(Duration::from_millis(120));
            continue;
        }
        set_scan_progress_phase(progress, resume_phase);
        return;
    }
}

pub(super) fn resolve_wd_tagger_model_dir(explicit_dir: Option<&str>) -> Result<PathBuf, String> {
    let valid =
        |path: &Path| path.join("model.onnx").is_file() && path.join("selected_tags.csv").is_file();
    if let Some(path) = explicit_resource_path("wd-swinv2-tagger-v3")? {
        return valid(&path)
            .then_some(path.clone())
            .ok_or_else(|| format!("隔离模型资源缺失：{}", path.display()));
    }
    if let Some(value) = explicit_dir {
        let path = PathBuf::from(value);
        return valid(&path).then_some(path.clone()).ok_or_else(|| {
            format!(
                "Configured model directory is missing model.onnx or selected_tags.csv: {}",
                path.display()
            )
        });
    }
    resource_candidates(
        &["wd-swinv2-tagger-v3"],
        &["wd-swinv2-tagger-v3", "../wd-swinv2-tagger-v3"],
        &["../wd-swinv2-tagger-v3", "../../wd-swinv2-tagger-v3"],
    ).into_iter().find(|path| valid(path))
        .ok_or_else(|| "Cannot find wd-swinv2-tagger-v3 (model.onnx + selected_tags.csv). Put it at project root or pass modelDir.".into())
}

pub(super) fn resolve_wd_tagger_script_path() -> Result<PathBuf, String> {
    resolve_bundled_service_script("wd_tagger_test.py")
}

pub(super) fn resolve_wd_tagger_service_script_path() -> Result<PathBuf, String> {
    resolve_bundled_service_script("wd_tagger_service.py")
}

pub(super) fn resolve_chinese_clip_model_dir(
    explicit_dir: Option<&str>,
) -> Result<PathBuf, String> {
    let valid = |path: &Path| {
        path.join("onnx/chinese_clip_text_encoder.onnx").is_file()
            && path.join("onnx/chinese_clip_image_encoder.onnx").is_file()
    };
    if let Some(path) = explicit_resource_path("model/chinese-clip-vit-base-patch16")? {
        return valid(&path)
            .then_some(path.clone())
            .ok_or_else(|| format!("隔离模型资源缺失：{}", path.display()));
    }
    if let Some(value) = explicit_dir {
        let path = PathBuf::from(value);
        return valid(&path).then_some(path.clone()).ok_or_else(|| {
            format!(
                "Configured model directory is missing ONNX files: {}",
                path.display()
            )
        });
    }
    resource_candidates(
        &["model/chinese-clip-vit-base-patch16"],
        &["model/chinese-clip-vit-base-patch16", "../model/chinese-clip-vit-base-patch16"],
        &["../model/chinese-clip-vit-base-patch16", "../../model/chinese-clip-vit-base-patch16"],
    ).into_iter().find(|path| valid(path))
        .ok_or_else(|| "Cannot find Chinese-CLIP ONNX model directory (expected model/chinese-clip-vit-base-patch16/onnx)".into())
}

pub(super) fn resolve_chinese_clip_image_service_script_path() -> Result<PathBuf, String> {
    resolve_bundled_service_script("chinese_clip_image_service.py")
}

pub(super) fn resolve_chinese_clip_text_service_script_path() -> Result<PathBuf, String> {
    resolve_bundled_service_script("chinese_clip_text_service.py")
}

pub(super) fn upsert_folder(
    conn: &Connection,
    folder_path: &str,
    scanned_at: i64,
) -> Result<i64, String> {
    conn.execute(
        "
        INSERT INTO folders (path, added_at, last_scanned_at, hidden)
        VALUES (?1, ?2, ?2, 0)
        ON CONFLICT(path) DO UPDATE SET
          last_scanned_at = excluded.last_scanned_at,
          hidden = 0
        ",
        params![folder_path, scanned_at],
    )
    .map_err(|error| format!("保存图库文件夹失败：{error}"))?;

    conn.query_row(
        "SELECT id FROM folders WHERE path = ?1",
        params![folder_path],
        |row| row.get(0),
    )
    .map_err(|error| format!("读取图库文件夹失败：{error}"))
}

pub(super) fn upsert_hidden_folder(
    conn: &Connection,
    folder_path: &str,
    scanned_at: i64,
) -> Result<i64, String> {
    conn.execute(
        "
        INSERT INTO folders (path, added_at, last_scanned_at, hidden)
        VALUES (?1, ?2, ?2, 1)
        ON CONFLICT(path) DO UPDATE SET
          last_scanned_at = excluded.last_scanned_at,
          hidden = 1
        ",
        params![folder_path, scanned_at],
    )
    .map_err(|error| format!("保存临时图片目录失败：{error}"))?;

    conn.query_row(
        "SELECT id FROM folders WHERE path = ?1",
        params![folder_path],
        |row| row.get(0),
    )
    .map_err(|error| format!("读取临时图片目录失败：{error}"))
}

pub(super) fn load_known_paths(conn: &Connection) -> Result<HashSet<String>, String> {
    let mut stmt = conn
        .prepare("SELECT path FROM images")
        .map_err(|error| format!("读取已有图片索引失败：{error}"))?;

    let paths = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| format!("读取已有图片索引失败：{error}"))?
        .collect::<Result<HashSet<_>, _>>()
        .map_err(|error| format!("读取已有图片索引失败：{error}"))?;

    Ok(paths)
}

pub(super) fn load_existing_library_image_meta(
    conn: &Connection,
) -> Result<HashMap<String, ExistingImageMeta>, String> {
    let mut stmt = conn
        .prepare(
            "
            SELECT path, width, height, file_size, modified_at, missing
            FROM images
            WHERE source = 'library'
            ",
        )
        .map_err(|error| format!("Failed to load existing library image metadata: {error}"))?;

    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                ExistingImageMeta {
                    width: row.get::<_, u32>(1)?,
                    height: row.get::<_, u32>(2)?,
                    file_size: row.get::<_, i64>(3)?,
                    modified_at: row.get::<_, i64>(4)?,
                    missing: row.get::<_, i64>(5)?,
                },
            ))
        })
        .map_err(|error| format!("Failed to load existing library image metadata: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to load existing library image metadata: {error}"))?;

    Ok(rows.into_iter().collect())
}

pub(super) fn upsert_image(
    conn: &Connection,
    folder_id: i64,
    image: &ScannedImage,
) -> Result<(), String> {
    conn.execute(
        "
        INSERT INTO images (
          id, path, file_name, ext, width, height, file_size, modified_at,
          imported_at, folder_id, missing, source
        )
        VALUES (?1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, 'library')
        ON CONFLICT(path) DO UPDATE SET
          file_name = excluded.file_name,
          ext = excluded.ext,
          width = excluded.width,
          height = excluded.height,
          file_size = excluded.file_size,
          modified_at = excluded.modified_at,
          folder_id = excluded.folder_id,
          missing = 0,
          trashed = images.trashed,
          source = 'library'
        ",
        params![
            image.path,
            image.file_name,
            image.ext,
            image.width,
            image.height,
            image.file_size,
            image.modified_at,
            image.imported_at,
            folder_id,
        ],
    )
    .map_err(|error| format!("保存图片索引失败：{error}"))?;

    Ok(())
}

pub(super) fn default_reference_board_item_size(width: u32, height: u32) -> (f32, f32) {
    if width == 0 || height == 0 {
        return (220.0, 220.0);
    }

    let max_side = 220.0;
    let aspect = width as f32 / height as f32;
    if aspect >= 1.0 {
        (max_side, (max_side / aspect).max(48.0))
    } else {
        ((max_side * aspect).max(48.0), max_side)
    }
}

pub(super) fn clipboard_image_extension(mime_type: &str) -> &'static str {
    match mime_type {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "png",
    }
}

pub(super) fn mime_type_for_extension(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        _ => "image/png",
    }
}

pub(super) fn flush_scanned_image_batch(
    conn: &mut Connection,
    folder_id: i64,
    batch: &[ScannedImage],
    known_paths: &mut HashSet<String>,
    new_image_ids: &mut Vec<String>,
    updated_images: &mut i64,
    skipped_images: &mut i64,
) -> Result<(), String> {
    if batch.is_empty() {
        return Ok(());
    }
    let tx = conn
        .transaction()
        .map_err(|error| format!("Failed to open scan upsert transaction: {error}"))?;
    let pending_created_at = now_ms();
    let mut committed_new_paths = HashSet::new();
    let mut committed_new_images = Vec::new();
    let mut committed_updates = 0;
    let mut committed_skips = 0;
    for image in batch {
        let is_new =
            !known_paths.contains(&image.path) && committed_new_paths.insert(image.path.clone());
        if is_new {
            upsert_image(&tx, folder_id, image)?;
            // Same transaction as the image insert: a crash after this commit
            // still leaves an exact per-image pending record for the next scan.
            tx.execute(
                "INSERT OR IGNORE INTO scan_pending_assignments(image_pk,created_at)
                 SELECT image_pk,?2 FROM images WHERE path=?1",
                params![image.path, pending_created_at],
            )
            .map_err(|error| format!("Failed to record pending folder assignment: {error}"))?;
            committed_new_images.push(image.clone());
        } else if image.unchanged {
            committed_skips += 1;
        } else {
            upsert_image(&tx, folder_id, image)?;
            // Auto tags describe the source bytes. Once the scan observed a
            // changed signature they are stale; manual tags stay untouched and
            // the image is picked up again by the pending tag queue below.
            compact::delete_auto_tags_for_image_path(&tx, &image.path)?;
            committed_updates += 1;
        }
    }
    tx.commit()
        .map_err(|error| format!("Failed to commit scan upsert transaction: {error}"))?;
    if !committed_new_images.is_empty() || committed_updates > 0 {
        if let Some(path) = conn.path() {
            crate::duplicates::cache::invalidate_database(Path::new(path));
        }
    }
    // Publish queue and progress changes only after the whole batch is durable.
    // A failed transaction must not make a retry believe uncommitted paths exist.
    known_paths.extend(committed_new_paths);
    new_image_ids.extend(committed_new_images.iter().map(|image| image.path.clone()));
    *updated_images += committed_updates;
    *skipped_images += committed_skips;
    Ok(())
}

pub(super) fn scan_images(
    folder_path: &Path,
    imported_at: i64,
    seen_paths: &mut HashSet<String>,
    existing_meta: &HashMap<String, ExistingImageMeta>,
    on_image: &mut dyn FnMut(ScannedImage) -> Result<bool, String>,
) -> Result<(), String> {
    for entry in WalkDir::new(folder_path).follow_links(false) {
        let entry = entry
            .map_err(|error| format!("无法扫描来源目录 {}：{error}", folder_path.display()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if !is_supported_image(path) {
            continue;
        }

        let metadata = entry
            .metadata()
            .map_err(|error| format!("无法读取图片属性 {}：{error}", path.display()))?;
        let modified_at = metadata
            .modified()
            .ok()
            .map(system_time_ms)
            .unwrap_or(imported_at);
        let path_text = path.to_string_lossy().to_string();

        if !seen_paths.insert(path_text.clone()) {
            continue;
        }

        let file_size = metadata.len() as i64;
        let cached = existing_meta.get(&path_text);
        let (width, height, unchanged) = if let Some(meta) = cached {
            if meta.modified_at == modified_at && meta.file_size == file_size {
                // 与索引一致：跳过尺寸读取；仅当之前被标记缺失时需要回写清除 missing
                (meta.width, meta.height, meta.missing == 0)
            } else {
                let reader = ImageReader::open(path)
                    .map_err(|error| format!("无法读取来源图片 {}：{error}", path.display()))?;
                let (width, height) = match reader.into_dimensions() {
                    Ok(dimensions) => dimensions,
                    Err(image::ImageError::IoError(error)) => {
                        return Err(format!("无法读取来源图片 {}：{error}", path.display()));
                    }
                    Err(_) => continue,
                };
                (width, height, false)
            }
        } else {
            let reader = ImageReader::open(path)
                .map_err(|error| format!("无法读取来源图片 {}：{error}", path.display()))?;
            let (width, height) = match reader.into_dimensions() {
                Ok(dimensions) => dimensions,
                Err(image::ImageError::IoError(error)) => {
                    return Err(format!("无法读取来源图片 {}：{error}", path.display()));
                }
                Err(_) => continue,
            };
            (width, height, false)
        };

        let image = ScannedImage {
            path: path_text,
            file_name: path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default(),
            ext: path
                .extension()
                .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default(),
            width,
            height,
            file_size,
            modified_at,
            imported_at,
            unchanged,
        };
        if !on_image(image)? {
            break;
        }
    }
    Ok(())
}

pub(super) fn normalize_folder_path(folder_path: &str) -> Result<String, String> {
    let path = PathBuf::from(folder_path);
    if !path.is_dir() {
        return Err("请选择一个有效的文件夹".to_string());
    }

    path.canonicalize()
        .map(|path| path.to_string_lossy().to_string())
        .map_err(|error| format!("读取文件夹失败：{error}"))
}

pub(super) fn normalize_existing_or_stored_folder_path(folder_path: &str) -> String {
    PathBuf::from(folder_path)
        .canonicalize()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|_| folder_path.to_string())
}

pub(super) fn is_supported_image(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp" | "tif" | "tiff" | "avif"
            )
        })
        .unwrap_or(false)
}

pub(super) fn now_ms() -> i64 {
    system_time_ms(SystemTime::now())
}

pub(super) fn system_time_ms(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

pub(super) fn yield_to_foreground(
    priority: &Arc<std::sync::atomic::AtomicUsize>,
    stop: &Arc<Mutex<bool>>,
) {
    while priority.load(std::sync::atomic::Ordering::SeqCst) > 0 {
        if stop.lock().map(|s| *s).unwrap_or(true) {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod resource_path_tests {
    use super::*;

    struct ResourceFixture(PathBuf);
    impl ResourceFixture {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = env::temp_dir().join(format!(
                "illutag-resource-paths-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
        fn file(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"resource fixture").unwrap();
            path
        }
    }
    impl Drop for ResourceFixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn bundled_resources_win_over_a_different_working_directory() {
        let fixture = ResourceFixture::new();
        let executable_dir = fixture.0.join("program");
        let working_dir = fixture.0.join("old-project");
        for relative in [
            "scripts/wd_tagger_service.py",
            "scripts/chinese_clip_text_service.py",
            "model/chinese-clip-vit-base-patch16/onnx/chinese_clip_image_encoder.onnx",
            "wd-swinv2-tagger-v3/model.onnx",
            "wd-swinv2-tagger-v3/selected_tags_full_translation.csv",
            "wd-swinv2-tagger-v3/tagger_config.json",
            "wd-swinv2-tagger-v3/tag_groups.json",
        ] {
            let expected = fixture.file(&format!("program/{relative}"));
            fixture.file(&format!("old-project/{relative}"));
            assert_eq!(
                resolve_resource_file_with_roots(
                    None,
                    Some(&executable_dir),
                    Some(&working_dir),
                    relative,
                    &[relative],
                    &[]
                )
                .unwrap(),
                expected,
            );
        }
    }

    #[test]
    fn source_working_directory_remains_a_development_fallback() {
        let fixture = ResourceFixture::new();
        let executable_dir = fixture.0.join("target/debug");
        let working_dir = fixture.0.join("project");
        let expected = fixture.file("project/src-tauri/scripts/wd_tagger_service.py");
        assert_eq!(
            resolve_resource_file_with_roots(
                None,
                Some(&executable_dir),
                Some(&working_dir),
                "scripts/wd_tagger_service.py",
                &[
                    "src-tauri/scripts/wd_tagger_service.py",
                    "scripts/wd_tagger_service.py"
                ],
                &[]
            )
            .unwrap(),
            expected,
        );
    }

    #[test]
    fn explicit_resource_root_is_exclusive_and_rejects_missing_resources() {
        let fixture = ResourceFixture::new();
        let executable_dir = fixture.0.join("program");
        let working_dir = fixture.0.join("old-project");
        let resource_root = fixture.0.join("isolated-resources");
        let relative = "scripts/wd_tagger_service.py";
        fixture.file(&format!("program/{relative}"));
        fixture.file(&format!("old-project/{relative}"));
        let resolve = |root: &Path| {
            resolve_resource_file_with_roots(
                Some(root),
                Some(&executable_dir),
                Some(&working_dir),
                relative,
                &[relative],
                &[],
            )
        };
        assert!(resolve(&resource_root)
            .unwrap_err()
            .contains("隔离资源缺失"));
        assert!(resolve(Path::new("relative-resource-root"))
            .unwrap_err()
            .contains("绝对路径"));
        let expected = fixture.file(&format!("isolated-resources/{relative}"));
        assert_eq!(resolve(&resource_root).unwrap(), expected);
    }
}

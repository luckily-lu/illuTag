use super::*;

pub fn test_wd_swinv2_tagger(
    image_id: String,
    general_threshold: Option<f32>,
    character_threshold: Option<f32>,
    model_dir: Option<String>,
    state: &AppState,
) -> Result<WdTaggerTestResult, String> {
    let conn = open_database(&state.database_path)?;
    let image = load_image_record(&conn, &image_id)?;

    if !Path::new(&image.path).exists() {
        return Err("Tagging failed: image file is missing".to_string());
    }

    let general_threshold = general_threshold.unwrap_or(0.35).clamp(0.0, 1.0);
    let character_threshold = character_threshold.unwrap_or(0.85).clamp(0.0, 1.0);

    let model_root = resolve_wd_tagger_model_dir(model_dir.as_deref())?;
    let model_path = model_root.join("model.onnx");
    let tags_path = model_root.join("selected_tags.csv");
    let script_path = resolve_wd_tagger_script_path()?;

    if !model_path.is_file() {
        return Err(format!("model.onnx not found: {}", model_path.display()));
    }
    if !tags_path.is_file() {
        return Err(format!(
            "selected_tags.csv not found: {}",
            tags_path.display()
        ));
    }

    let mut command = python_command();
    let output = command
        .arg("-X")
        .arg("utf8")
        .arg(&script_path)
        .arg("--image")
        .arg(&image.path)
        .arg("--model")
        .arg(&model_path)
        .arg("--tags")
        .arg(&tags_path)
        .arg("--general-threshold")
        .arg(general_threshold.to_string())
        .arg("--character-threshold")
        .arg(character_threshold.to_string())
        .arg("--image-id")
        .arg(&image_id)
        .output()
        .map_err(|error| {
            format!(
                "Failed to start WD tagger script: {error}. Ensure Python and onnxruntime/numpy/pillow are installed."
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if stderr.is_empty() {
            "unknown error".to_string()
        } else {
            stderr
        };
        return Err(format!(
            "WD tagger failed: {detail}. If dependency missing, run: pip install onnxruntime numpy pillow"
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        return Err("WD tagger returned empty output".to_string());
    }

    let mut result: WdTaggerTestResult = serde_json::from_str(&stdout)
        .map_err(|error| format!("Failed to parse tagger output: {error}"))?;
    result.image_id = image_id;
    result.general_threshold = general_threshold;
    result.character_threshold = character_threshold;
    Ok(result)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BackgroundScanJobMode {
    CollectOnly,
    CollectAndTag,
    TagPendingOnly,
}

pub(super) fn start_scan_all_folders_worker(
    state: &AppState,
    mode: BackgroundScanJobMode,
) -> Result<bool, String> {
    let mut running = state
        .background_scan_running
        .lock()
        .map_err(|_| "Background scan state is locked".to_string())?;
    if *running {
        if let Ok(mut pending) = state.background_scan_pending.lock() {
            *pending = true;
        }
        return Ok(false);
    }
    *running = true;
    drop(running);
    if let Ok(mut pending) = state.background_scan_pending.lock() {
        *pending = false;
    }
    if let Ok(mut pause_requested) = state.background_scan_pause_requested.lock() {
        *pause_requested = false;
    }
    if let Ok(mut stop_requested) = state.background_scan_stop_requested.lock() {
        *stop_requested = false;
    }

    let database_path = state.database_path.clone();
    let foreground = Arc::clone(&state.foreground_queries);
    let source_folder_id = state.scan_folder_id;
    let library_cache = Arc::clone(&state.library);
    let background_scan_running = Arc::clone(&state.background_scan_running);
    let background_scan_pending = Arc::clone(&state.background_scan_pending);
    let background_scan_pause_requested_flag = Arc::clone(&state.background_scan_pause_requested);
    let background_scan_stop_requested_flag = Arc::clone(&state.background_scan_stop_requested);
    let background_scan_progress = Arc::clone(&state.background_scan_progress);
    let startup_cleanup_running = Arc::clone(&state.startup_cleanup_running);
    let wd_tagger_service = Arc::clone(&state.wd_tagger_service);
    let clip_vector_cache = Arc::clone(&state.clip_vector_cache);
    thread::spawn(move || {
        let mut mode = mode;
        set_scan_progress(
            &background_scan_progress,
            BackgroundScanProgress {
                running: true,
                phase: "collecting".to_string(),
                ..BackgroundScanProgress::default()
            },
        );
        wait_until_startup_cleanup_finished(&startup_cleanup_running);
        eprintln!("[wd-scan] worker started");
        loop {
            let tag_pending_only = matches!(mode, BackgroundScanJobMode::TagPendingOnly);
            set_scan_progress(
                &background_scan_progress,
                BackgroundScanProgress {
                    running: true,
                    phase: if tag_pending_only {
                        "tagging".to_string()
                    } else {
                        "collecting".to_string()
                    },
                    ..BackgroundScanProgress::default()
                },
            );

            let tag_queue_result = if tag_pending_only {
                let conn = match open_database(&database_path) {
                    Ok(value) => value,
                    Err(error) => {
                        set_scan_progress_error(&background_scan_progress, &error);
                        push_scan_progress_recent_error(&background_scan_progress, &error);
                        eprintln!("[wd-tag] {error}");
                        break;
                    }
                };
                let mut tag_queue_image_ids = match collect_pending_tag_image_ids(&conn) {
                    Ok(value) => value,
                    Err(error) => {
                        set_scan_progress_error(&background_scan_progress, &error);
                        push_scan_progress_recent_error(&background_scan_progress, &error);
                        eprintln!("[wd-tag] {error}");
                        break;
                    }
                };
                tag_queue_image_ids.sort();
                tag_queue_image_ids.dedup();
                set_scan_progress_phase(&background_scan_progress, "tagging");
                set_scan_progress_queued_images(
                    &background_scan_progress,
                    tag_queue_image_ids.len() as i64,
                );
                Ok(ScanCollectResult {
                    tag_queue_image_ids,
                })
            } else {
                scan_all_folders_and_collect_new_images(
                    &database_path,
                    &foreground,
                    &background_scan_progress,
                    &background_scan_pause_requested_flag,
                    &background_scan_stop_requested_flag,
                    matches!(mode, BackgroundScanJobMode::CollectAndTag),
                    source_folder_id,
                )
            };

            match tag_queue_result {
                Ok(scan_result) => {
                    if !tag_pending_only {
                        if let Ok(mut cache) = library_cache.lock() {
                            *cache = None;
                        }
                        clear_optional_cache(&clip_vector_cache);
                    }
                    if matches!(
                        mode,
                        BackgroundScanJobMode::CollectAndTag
                            | BackgroundScanJobMode::TagPendingOnly
                    ) {
                        if let Err(error) = tag_images_with_wd_model(
                            &database_path,
                            &foreground,
                            &scan_result.tag_queue_image_ids,
                            &background_scan_progress,
                            &background_scan_pause_requested_flag,
                            &background_scan_stop_requested_flag,
                            &wd_tagger_service,
                        ) {
                            set_scan_progress_error(&background_scan_progress, &error);
                            push_scan_progress_recent_error(&background_scan_progress, &error);
                            eprintln!("[wd-tag] {error}");
                        }
                    }
                }
                Err(error) => {
                    set_scan_progress_error(&background_scan_progress, &error);
                    push_scan_progress_recent_error(&background_scan_progress, &error);
                    eprintln!("[wd-scan] {error}");
                }
            }

            if let Ok(mut cache) = library_cache.lock() {
                *cache = None;
            }
            clear_optional_cache(&clip_vector_cache);

            if background_scan_stop_requested(&background_scan_stop_requested_flag) {
                if let Ok(mut pending) = background_scan_pending.lock() {
                    *pending = false;
                }
                break;
            }

            let rerun = if let Ok(mut pending) = background_scan_pending.lock() {
                if *pending {
                    *pending = false;
                    true
                } else {
                    false
                }
            } else {
                false
            };
            if rerun {
                eprintln!("[wd-scan] pending rerun");
                mode = BackgroundScanJobMode::CollectOnly;
                continue;
            }
            break;
        }

        set_scan_progress_done(&background_scan_progress);
        if let Ok(mut cache) = library_cache.lock() {
            *cache = None;
        }
        clear_optional_cache(&clip_vector_cache);
        release_wd_tagger_service(&wd_tagger_service);
        if let Ok(mut running) = background_scan_running.lock() {
            *running = false;
        }
        if let Ok(mut pending) = background_scan_pending.lock() {
            *pending = false;
        }
        if let Ok(mut pause_requested) = background_scan_pause_requested_flag.lock() {
            *pause_requested = false;
        }
        if let Ok(mut stop_requested) = background_scan_stop_requested_flag.lock() {
            *stop_requested = false;
        }
        eprintln!("[wd-scan] worker finished");
    });

    Ok(true)
}

pub fn start_scan_all_folders_with_tagging(state: &AppState) -> Result<bool, String> {
    start_scan_all_folders_worker(state, BackgroundScanJobMode::CollectAndTag)
}

pub fn start_scan_all_folders_collect_only(state: &AppState) -> Result<bool, String> {
    start_scan_all_folders_worker(state, BackgroundScanJobMode::CollectOnly)
}

pub fn resume_incomplete_library_scan(state: &AppState) -> Result<bool, String> {
    let conn = open_database(&state.database_path)?;
    let interrupted =
        read_app_meta_value(&conn, "library_scan_in_progress")?.as_deref() == Some("1");
    let empty_roots = conn
        .query_row(
            "
            SELECT COUNT(*)
            FROM folders f
            WHERE COALESCE(f.hidden, 0) = 0
              AND NOT EXISTS (
                SELECT 1 FROM images i WHERE i.folder_id = f.id LIMIT 1
              )
            ",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| format!("Failed to check incomplete library folders: {error}"))?;
    if !interrupted && empty_roots <= 0 {
        return Ok(false);
    }
    start_scan_all_folders_collect_only(state)
}

pub fn start_tag_pending_images_only(state: &AppState) -> Result<bool, String> {
    start_scan_all_folders_worker(state, BackgroundScanJobMode::TagPendingOnly)
}

pub fn background_scan_status(state: &AppState) -> Result<BackgroundScanStatus, String> {
    let running = state
        .background_scan_running
        .lock()
        .map_err(|_| "Background scan state is locked".to_string())?;
    Ok(BackgroundScanStatus { running: *running })
}

pub fn background_scan_progress(state: &AppState) -> Result<BackgroundScanProgress, String> {
    state
        .background_scan_progress
        .lock()
        .map_err(|_| "Background scan progress state is locked".to_string())
        .map(|value| value.clone())
}

pub fn pause_background_scan(state: &AppState) -> Result<bool, String> {
    let running = state
        .background_scan_running
        .lock()
        .map_err(|_| "Background scan state is locked".to_string())
        .map(|value| *value)?;
    if !running {
        return Ok(false);
    }
    if let Ok(mut pause_requested) = state.background_scan_pause_requested.lock() {
        *pause_requested = true;
    }
    // The executor marks progress paused when it reaches its next safe point.
    Ok(true)
}

pub fn resume_background_scan(state: &AppState) -> Result<bool, String> {
    let running = state
        .background_scan_running
        .lock()
        .map_err(|_| "Background scan state is locked".to_string())
        .map(|value| *value)?;
    if !running {
        return Ok(false);
    }
    if let Ok(mut pause_requested) = state.background_scan_pause_requested.lock() {
        *pause_requested = false;
    }
    update_scan_progress(&state.background_scan_progress, |progress| {
        progress.paused = false;
        if progress.phase == "paused" {
            progress.phase = if progress.queued_images > 0 {
                "tagging".to_string()
            } else {
                "collecting".to_string()
            };
        }
    });
    Ok(true)
}

pub fn stop_background_scan(state: &AppState) -> Result<bool, String> {
    let running = state
        .background_scan_running
        .lock()
        .map_err(|_| "Background scan state is locked".to_string())
        .map(|value| *value)?;
    if !running {
        return Ok(false);
    }
    if let Ok(mut stop_requested) = state.background_scan_stop_requested.lock() {
        *stop_requested = true;
    }
    if let Ok(mut pause_requested) = state.background_scan_pause_requested.lock() {
        *pause_requested = false;
    }
    update_scan_progress(&state.background_scan_progress, |progress| {
        progress.paused = false;
        progress.phase = "stopping".to_string();
    });
    Ok(true)
}

pub fn start_startup_cleanup(state: &AppState) -> Result<bool, String> {
    let background_scan_running = state
        .background_scan_running
        .lock()
        .map_err(|_| "Background scan state is locked".to_string())?;
    if *background_scan_running {
        return Ok(false);
    }
    drop(background_scan_running);

    let mut running = state
        .startup_cleanup_running
        .lock()
        .map_err(|_| "Startup cleanup state is locked".to_string())?;
    if *running {
        return Ok(false);
    }
    *state
        .startup_cleanup_error
        .lock()
        .map_err(|_| "Startup cleanup error state is locked".to_string())? = None;
    *running = true;
    drop(running);

    let database_path = state.database_path.clone();
    let library_cache = Arc::clone(&state.library);
    let startup_cleanup_running = Arc::clone(&state.startup_cleanup_running);
    let startup_cleanup_generation = Arc::clone(&state.startup_cleanup_generation);
    let startup_cleanup_error = Arc::clone(&state.startup_cleanup_error);
    let clip_vector_cache = Arc::clone(&state.clip_vector_cache);

    thread::spawn(move || {
        let result = cleanup_missing_library_images_batched(&database_path, 256);
        if let Err(error) = &result {
            eprintln!("[startup-cleanup] {error}");
            if let Ok(mut last_error) = startup_cleanup_error.lock() {
                *last_error = Some(error.clone());
            }
        } else if let Ok(removed) = result {
            eprintln!("[startup-cleanup] removed {removed} missing library images");
        }

        if let Ok(mut cache) = library_cache.lock() {
            *cache = None;
        }
        if matches!(result, Ok(removed) if removed > 0) {
            clear_optional_cache(&clip_vector_cache);
        }
        if let Ok(mut generation) = startup_cleanup_generation.lock() {
            *generation += 1;
        }
        if let Ok(mut state) = startup_cleanup_running.lock() {
            *state = false;
        }
    });

    Ok(true)
}

pub(super) fn wait_until_startup_cleanup_finished(startup_cleanup_running: &Arc<Mutex<bool>>) {
    loop {
        let running = match startup_cleanup_running.lock() {
            Ok(state) => *state,
            Err(_) => false,
        };
        if !running {
            break;
        }
        thread::sleep(std::time::Duration::from_millis(120));
    }
}

pub fn startup_cleanup_status(state: &AppState) -> Result<StartupCleanupStatus, String> {
    let running = state
        .startup_cleanup_running
        .lock()
        .map_err(|_| "Startup cleanup state is locked".to_string())
        .map(|value| *value)?;
    let generation = state
        .startup_cleanup_generation
        .lock()
        .map_err(|_| "Startup cleanup state is locked".to_string())
        .map(|value| *value)?;
    Ok(StartupCleanupStatus {
        running,
        generation,
    })
}

pub fn start_thumbnail_generation(state: &AppState) -> Result<bool, String> {
    let mut running = state
        .thumbnail_generation_running
        .lock()
        .map_err(|_| "Thumbnail generation state is locked".to_string())?;
    if *running {
        if let Ok(mut pending) = state.thumbnail_generation_pending.lock() {
            *pending = true;
        }
        return Ok(false);
    }
    *running = true;
    drop(running);

    if let Ok(mut pending) = state.thumbnail_generation_pending.lock() {
        *pending = false;
    }
    if let Ok(mut pause_requested) = state.thumbnail_generation_pause_requested.lock() {
        *pause_requested = false;
    }
    if let Ok(mut stop_requested) = state.thumbnail_generation_stop_requested.lock() {
        *stop_requested = false;
    }
    set_thumbnail_progress(
        &state.thumbnail_generation_progress,
        ThumbnailGenerationProgress {
            running: true,
            paused: false,
            phase: "queueing".to_string(),
            ..ThumbnailGenerationProgress::default()
        },
    );

    let database_path = state.database_path.clone();
    let foreground = Arc::clone(&state.foreground_queries);
    let library_cache = Arc::clone(&state.library);
    let thumb_running = Arc::clone(&state.thumbnail_generation_running);
    let thumb_pending = Arc::clone(&state.thumbnail_generation_pending);
    let thumb_pause_requested = Arc::clone(&state.thumbnail_generation_pause_requested);
    let thumb_stop_requested = Arc::clone(&state.thumbnail_generation_stop_requested);
    let thumb_progress = Arc::clone(&state.thumbnail_generation_progress);

    thread::spawn(move || {
        loop {
            set_thumbnail_progress_phase(&thumb_progress, "queueing");
            match generate_thumbnails_once(
                &database_path,
                &foreground,
                &thumb_progress,
                &thumb_pause_requested,
                &thumb_stop_requested,
            ) {
                Ok(generated) => {
                    if generated > 0 {
                        if let Ok(mut cache) = library_cache.lock() {
                            *cache = None;
                        }
                    }
                }
                Err(error) => {
                    set_thumbnail_progress_error(&thumb_progress, &error);
                    push_thumbnail_progress_recent_error(&thumb_progress, &error);
                }
            }

            let rerun = if let Ok(mut pending) = thumb_pending.lock() {
                if *pending {
                    *pending = false;
                    true
                } else {
                    false
                }
            } else {
                false
            };
            if rerun {
                continue;
            }
            break;
        }

        set_thumbnail_progress_done(&thumb_progress);
        if let Ok(mut running) = thumb_running.lock() {
            *running = false;
        }
        if let Ok(mut pending) = thumb_pending.lock() {
            *pending = false;
        }
        if let Ok(mut pause_requested) = thumb_pause_requested.lock() {
            *pause_requested = false;
        }
        if let Ok(mut stop_requested) = thumb_stop_requested.lock() {
            *stop_requested = false;
        }
    });

    Ok(true)
}

pub fn thumbnail_generation_status(
    state: &AppState,
) -> Result<ThumbnailGenerationProgress, String> {
    state
        .thumbnail_generation_progress
        .lock()
        .map_err(|_| "Thumbnail generation progress state is locked".to_string())
        .map(|value| value.clone())
}

pub fn pause_thumbnail_generation(state: &AppState) -> Result<bool, String> {
    let running = state
        .thumbnail_generation_running
        .lock()
        .map_err(|_| "Thumbnail generation state is locked".to_string())
        .map(|value| *value)?;
    if !running {
        return Ok(false);
    }
    if let Ok(mut pause_requested) = state.thumbnail_generation_pause_requested.lock() {
        *pause_requested = true;
    }
    // The executor marks progress paused when it reaches its next safe point.
    Ok(true)
}

pub fn resume_thumbnail_generation(state: &AppState) -> Result<bool, String> {
    let running = state
        .thumbnail_generation_running
        .lock()
        .map_err(|_| "Thumbnail generation state is locked".to_string())
        .map(|value| *value)?;
    if !running {
        return Ok(false);
    }
    if let Ok(mut pause_requested) = state.thumbnail_generation_pause_requested.lock() {
        *pause_requested = false;
    }
    update_thumbnail_progress(&state.thumbnail_generation_progress, |progress| {
        progress.paused = false;
        if progress.phase == "paused" {
            progress.phase = "generating".to_string();
        }
    });
    Ok(true)
}

pub fn stop_thumbnail_generation(state: &AppState) -> Result<bool, String> {
    let running = state
        .thumbnail_generation_running
        .lock()
        .map_err(|_| "Thumbnail generation state is locked".to_string())
        .map(|value| *value)?;
    if !running {
        return Ok(false);
    }
    if let Ok(mut stop_requested) = state.thumbnail_generation_stop_requested.lock() {
        *stop_requested = true;
    }
    if let Ok(mut pause_requested) = state.thumbnail_generation_pause_requested.lock() {
        *pause_requested = false;
    }
    update_thumbnail_progress(&state.thumbnail_generation_progress, |progress| {
        progress.paused = false;
        progress.phase = "stopping".to_string();
    });
    Ok(true)
}

pub fn clear_thumbnail_cache(state: &AppState) -> Result<(), String> {
    let running = state
        .thumbnail_generation_running
        .lock()
        .map_err(|_| "Thumbnail generation state is locked".to_string())
        .map(|value| *value)?;
    if running {
        return Err("缩略图任务正在运行，请先停止后再清理缓存".to_string());
    }

    let conn = open_database(&state.database_path)?;
    clear_thumbnail_cache_storage(&conn, &state.database_path)?;
    if let Ok(mut cache) = state.library.lock() {
        *cache = None;
    }
    set_thumbnail_progress(
        &state.thumbnail_generation_progress,
        ThumbnailGenerationProgress::default(),
    );
    Ok(())
}

pub fn rebuild_thumbnail_cache(state: &AppState) -> Result<bool, String> {
    clear_thumbnail_cache(state)?;
    start_thumbnail_generation(state)
}

pub fn start_natural_language_scan(state: &AppState) -> Result<bool, String> {
    let mut running = state
        .natural_language_scan_running
        .lock()
        .map_err(|_| "Natural language scan state is locked".to_string())?;
    if *running {
        if let Ok(mut pending) = state.natural_language_scan_pending.lock() {
            *pending = true;
        }
        return Ok(false);
    }
    *running = true;
    drop(running);
    if let Ok(mut pending) = state.natural_language_scan_pending.lock() {
        *pending = false;
    }
    if let Ok(mut pause_requested) = state.natural_language_scan_pause_requested.lock() {
        *pause_requested = false;
    }
    if let Ok(mut stop_requested) = state.natural_language_scan_stop_requested.lock() {
        *stop_requested = false;
    }

    let database_path = state.database_path.clone();
    let foreground = Arc::clone(&state.foreground_queries);
    let library_cache = Arc::clone(&state.library);
    let scan_running = Arc::clone(&state.natural_language_scan_running);
    let scan_pending = Arc::clone(&state.natural_language_scan_pending);
    let scan_pause_requested = Arc::clone(&state.natural_language_scan_pause_requested);
    let scan_stop_requested = Arc::clone(&state.natural_language_scan_stop_requested);
    let scan_progress = Arc::clone(&state.natural_language_scan_progress);
    let clip_vector_cache = Arc::clone(&state.clip_vector_cache);
    let clip_image_encoder_service = Arc::clone(&state.clip_image_encoder_service);
    let clip_image_encoder_last_used_at = Arc::clone(&state.clip_image_encoder_last_used_at);
    let clip_image_encoder_release_worker_running =
        Arc::clone(&state.clip_image_encoder_release_worker_running);

    thread::spawn(move || {
        loop {
            set_natural_language_scan_progress(
                &scan_progress,
                NaturalLanguageScanProgress {
                    running: true,
                    phase: "collecting".to_string(),
                    ..NaturalLanguageScanProgress::default()
                },
            );

            if let Err(error) = generate_natural_language_embeddings_once(
                &database_path,
                &foreground,
                &scan_progress,
                &clip_vector_cache,
                &clip_image_encoder_service,
                &clip_image_encoder_last_used_at,
                &clip_image_encoder_release_worker_running,
                &scan_pause_requested,
                &scan_stop_requested,
            ) {
                set_natural_language_scan_progress_error(&scan_progress, &error);
                push_natural_language_scan_recent_error(&scan_progress, &error);
                eprintln!("[clip-scan] {error}");
            }

            if let Ok(mut cache) = library_cache.lock() {
                *cache = None;
            }

            if natural_language_scan_stop_requested(&scan_stop_requested) {
                if let Ok(mut pending) = scan_pending.lock() {
                    *pending = false;
                }
                break;
            }

            let rerun = if let Ok(mut pending) = scan_pending.lock() {
                if *pending {
                    *pending = false;
                    true
                } else {
                    false
                }
            } else {
                false
            };
            if rerun {
                eprintln!("[clip-scan] pending rerun");
                continue;
            }
            break;
        }

        set_natural_language_scan_progress_done(&scan_progress);
        if let Ok(mut running) = scan_running.lock() {
            *running = false;
        }
        if let Ok(mut pending) = scan_pending.lock() {
            *pending = false;
        }
        if let Ok(mut pause_requested) = scan_pause_requested.lock() {
            *pause_requested = false;
        }
        if let Ok(mut stop_requested) = scan_stop_requested.lock() {
            *stop_requested = false;
        }
        eprintln!("[clip-scan] worker finished");
    });

    Ok(true)
}

pub fn natural_language_scan_status(state: &AppState) -> Result<NaturalLanguageScanStatus, String> {
    let running = state
        .natural_language_scan_running
        .lock()
        .map_err(|_| "Natural language scan state is locked".to_string())?;
    Ok(NaturalLanguageScanStatus { running: *running })
}

pub fn natural_language_scan_progress(
    state: &AppState,
) -> Result<NaturalLanguageScanProgress, String> {
    state
        .natural_language_scan_progress
        .lock()
        .map_err(|_| "Natural language scan progress state is locked".to_string())
        .map(|value| value.clone())
}

pub fn pause_natural_language_scan(state: &AppState) -> Result<bool, String> {
    let running = state
        .natural_language_scan_running
        .lock()
        .map_err(|_| "Natural language scan state is locked".to_string())
        .map(|value| *value)?;
    if !running {
        return Ok(false);
    }
    if let Ok(mut pause_requested) = state.natural_language_scan_pause_requested.lock() {
        *pause_requested = true;
    }
    // The executor marks progress paused when it reaches its next safe point.
    Ok(true)
}

pub fn resume_natural_language_scan(state: &AppState) -> Result<bool, String> {
    let running = state
        .natural_language_scan_running
        .lock()
        .map_err(|_| "Natural language scan state is locked".to_string())
        .map(|value| *value)?;
    if !running {
        return Ok(false);
    }
    if let Ok(mut pause_requested) = state.natural_language_scan_pause_requested.lock() {
        *pause_requested = false;
    }
    update_natural_language_scan_progress(&state.natural_language_scan_progress, |progress| {
        progress.paused = false;
        if progress.phase == "paused" {
            progress.phase = "generating".to_string();
        }
    });
    Ok(true)
}

pub fn stop_natural_language_scan(state: &AppState) -> Result<bool, String> {
    let running = state
        .natural_language_scan_running
        .lock()
        .map_err(|_| "Natural language scan state is locked".to_string())
        .map(|value| *value)?;
    if !running {
        return Ok(false);
    }
    if let Ok(mut stop_requested) = state.natural_language_scan_stop_requested.lock() {
        *stop_requested = true;
    }
    if let Ok(mut pause_requested) = state.natural_language_scan_pause_requested.lock() {
        *pause_requested = false;
    }
    update_natural_language_scan_progress(&state.natural_language_scan_progress, |progress| {
        progress.paused = false;
        progress.phase = "stopping".to_string();
    });
    Ok(true)
}

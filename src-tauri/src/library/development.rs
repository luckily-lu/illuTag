use super::*;

pub fn dev_create_fake_gallery_data(
    options: DevFakeGalleryOptions,
    state: &AppState,
) -> Result<DevStressToolResult, String> {
    let count = options.count.clamp(0, 200_000);
    if count == 0 {
        return Ok(dev_result(0, 0, 0, 0, None, state));
    }
    let mut conn = open_database(&state.database_path)?;
    let now = now_ms();
    let folder_id = upsert_hidden_folder(&conn, ".illutag-dev-test/fake-gallery", now)?;
    let samples = collect_dev_fake_sample_images(
        &conn,
        options.source_folder.as_deref(),
        options.sample_image_ids.as_deref(),
    )?;
    if samples.is_empty() {
        return Err("No sample images available. Provide sourceFolder or sampleImageIds, or add real images first.".to_string());
    }
    let user_folder_ids = if options.randomize_folders.unwrap_or(false) {
        ensure_dev_fake_user_folders(&conn, now)?
    } else {
        Vec::new()
    };
    let tx = conn
        .transaction()
        .map_err(|error| format!("Failed to open dev fake data transaction: {error}"))?;
    let mut created = 0i64;
    for index in 0..count {
        let sample = &samples[index as usize % samples.len()];
        let seed = dev_hash_i64(index + now);
        let width = if options.randomize_dimensions.unwrap_or(true) {
            320 + (seed % 1800) as u32
        } else {
            sample.width.max(1)
        };
        let height = if options.randomize_dimensions.unwrap_or(true) {
            320 + ((seed / 17) % 1800) as u32
        } else {
            sample.height.max(1)
        };
        let file_name = if options.randomize_file_names.unwrap_or(true) {
            format!("dev_fake_{index:06}_{}.{}", seed % 10_000, sample.ext)
        } else {
            format!("dev_fake_{index:06}_{}", sample.file_name)
        };
        let image_id = format!("illutag-dev-fake:{index:08}");
        let fake_path = format!("{}#{}", sample.path, image_id);
        let imported_at = if options.randomize_imported_at.unwrap_or(true) {
            now - (seed % (180 * 24 * 60 * 60 * 1000))
        } else {
            now
        };
        tx.execute(
            "
            INSERT INTO images (
              id, path, file_name, ext, width, height, file_size, modified_at, imported_at,
              folder_id, missing, trashed, is_favorite, content_hash, source
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, 0, ?11, NULL, 'library')
            ON CONFLICT(id) DO UPDATE SET
              path = excluded.path,
              file_name = excluded.file_name,
              width = excluded.width,
              height = excluded.height,
              file_size = excluded.file_size,
              modified_at = excluded.modified_at,
              imported_at = excluded.imported_at,
              is_favorite = excluded.is_favorite
            ",
            params![
                image_id,
                fake_path,
                file_name,
                sample.ext,
                width,
                height,
                sample.file_size,
                sample.modified_at,
                imported_at,
                folder_id,
                if options.randomize_favorites.unwrap_or(false) && seed % 7 == 0 {
                    1
                } else {
                    0
                },
            ],
        )
        .map_err(|error| format!("Failed to insert dev fake image: {error}"))?;
        if !user_folder_ids.is_empty() {
            let assigned_folder_id = user_folder_ids[seed as usize % user_folder_ids.len()];
            tx.execute(
                "
                INSERT OR IGNORE INTO storage_image_user_folders (image_id, folder_id, assigned_at)
                VALUES ((SELECT image_pk FROM images WHERE id=?1), ?2, ?3)
                ",
                params![image_id, assigned_folder_id, now],
            )
            .map_err(|error| format!("Failed to assign dev fake folder: {error}"))?;
        }
        if options.randomize_tags.unwrap_or(false) {
            let tag = match seed % 5 {
                0 => "dev_warm_color",
                1 => "dev_cool_color",
                2 => "dev_lineart",
                3 => "dev_character",
                _ => "dev_landscape",
            };
            compact::upsert_auto_tag(
                &tx,
                &image_id,
                "general",
                tag,
                Some(tag),
                0.5 + ((seed % 45) as f64 / 100.0),
                WD_TAGGER_MODEL_NAME,
                now,
            )?;
        }
        tx.execute(
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
                sample.path,
                sample.modified_at,
                sample.file_size,
                now
            ],
        )
        .map_err(|error| format!("Failed to insert dev fake thumbnail: {error}"))?;
        created += 1;
    }
    tx.commit()
        .map_err(|error| format!("Failed to commit dev fake data: {error}"))?;
    invalidate_library_cache(state);
    Ok(dev_result(created, 0, 0, 0, None, state))
}

pub fn dev_create_small_file_test_set(
    options: DevSmallFileSetOptions,
    state: &AppState,
) -> Result<DevStressToolResult, String> {
    let count = options.count.clamp(0, 200_000);
    let root = options
        .root_dir
        .as_deref()
        .map(|path| PathBuf::from(path.trim()))
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| {
            state
                .database_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(".illutag-dev-test")
                .join("small-files")
        });
    fs::create_dir_all(&root)
        .map_err(|error| format!("Failed to create dev small file root: {error}"))?;
    let format = options
        .format
        .as_deref()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| value == "jpg" || value == "jpeg" || value == "png")
        .unwrap_or_else(|| "png".to_string());
    let subfolder_count = options.subfolder_count.unwrap_or(1).clamp(1, 10_000);
    let mut created = 0i64;
    for index in 0..count {
        let folder = root.join(format!("bucket_{:03}", index % subfolder_count));
        fs::create_dir_all(&folder)
            .map_err(|error| format!("Failed to create dev small file folder: {error}"))?;
        let extension = if format == "jpeg" {
            "jpg"
        } else {
            format.as_str()
        };
        let path = folder.join(format!("dev_small_{index:08}.{extension}"));
        let seed = dev_hash_i64(index);
        let image = ImageBuffer::from_fn(32, 32, |x, y| {
            Rgb([
                ((x as i64 * 7 + seed) % 255) as u8,
                ((y as i64 * 11 + seed / 3) % 255) as u8,
                (((x + y) as i64 * 5 + seed / 7) % 255) as u8,
            ])
        });
        image.save(&path).map_err(|error| {
            format!(
                "Failed to write dev small image {}: {error}",
                path.display()
            )
        })?;
        created += 1;
    }
    Ok(dev_result(0, created, 0, 0, Some(root), state))
}

pub fn dev_cleanup_stress_test_data(state: &AppState) -> Result<DevStressToolResult, String> {
    let mut conn = open_database(&state.database_path)?;
    let test_root = state
        .database_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".illutag-dev-test");
    let tx = conn
        .transaction()
        .map_err(|error| format!("Failed to open dev cleanup transaction: {error}"))?;
    let deleted_images =
        tx.execute(
            "
            DELETE FROM images
            WHERE id LIKE 'illutag-dev-fake:%'
               OR path LIKE '%.illutag-dev-test%'
               OR path LIKE '%#illutag-dev-fake:%'
            ",
            [],
        )
        .map_err(|error| format!("Failed to delete dev fake images: {error}"))? as i64;
    tx.execute(
        "DELETE FROM user_folders WHERE name LIKE 'Dev Fake Folder %'",
        [],
    )
    .map_err(|error| format!("Failed to delete dev fake user folders: {error}"))?;
    tx.execute(
        "DELETE FROM folders WHERE path LIKE '%.illutag-dev-test%'",
        [],
    )
    .map_err(|error| format!("Failed to delete dev fake folders: {error}"))?;
    tx.commit()
        .map_err(|error| format!("Failed to commit dev cleanup: {error}"))?;
    let deleted_files = remove_dev_test_files(&test_root)?;
    invalidate_library_cache(state);
    Ok(dev_result(
        0,
        0,
        deleted_images,
        deleted_files,
        Some(test_root),
        state,
    ))
}

#[derive(Debug, Clone)]
pub(super) struct DevSampleImage {
    path: String,
    file_name: String,
    ext: String,
    width: u32,
    height: u32,
    file_size: i64,
    modified_at: i64,
}

pub(super) fn collect_dev_fake_sample_images(
    conn: &Connection,
    source_folder: Option<&str>,
    sample_image_ids: Option<&[String]>,
) -> Result<Vec<DevSampleImage>, String> {
    let mut samples = Vec::<DevSampleImage>::new();
    if let Some(ids) = sample_image_ids {
        for id in ids
            .iter()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .take(100)
        {
            if let Some(sample) = conn
                .query_row(
                    "
                    SELECT path, file_name, ext, width, height, file_size, modified_at
                    FROM images
                    WHERE id = ?1
                    ",
                    params![id],
                    dev_sample_image_from_row,
                )
                .optional()
                .map_err(|error| format!("Failed to query dev sample image: {error}"))?
            {
                samples.push(sample);
            }
        }
    }

    if samples.is_empty() {
        if let Some(folder) = source_folder
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            for entry in WalkDir::new(folder)
                .max_depth(2)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_file())
                .take(100)
            {
                let path = entry.path();
                let ext = path
                    .extension()
                    .map(|value| value.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();
                if !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp" | "bmp") {
                    continue;
                }
                let metadata = match fs::metadata(path) {
                    Ok(metadata) => metadata,
                    Err(_) => continue,
                };
                let (width, height) = ImageReader::open(path)
                    .ok()
                    .and_then(|reader| reader.into_dimensions().ok())
                    .unwrap_or((512, 512));
                samples.push(DevSampleImage {
                    path: path.to_string_lossy().to_string(),
                    file_name: path
                        .file_name()
                        .map(|value| value.to_string_lossy().to_string())
                        .unwrap_or_else(|| format!("sample.{ext}")),
                    ext,
                    width,
                    height,
                    file_size: metadata.len() as i64,
                    modified_at: metadata
                        .modified()
                        .ok()
                        .map(system_time_ms)
                        .unwrap_or_else(now_ms),
                });
            }
        }
    }

    if samples.is_empty() {
        let mut stmt = conn
            .prepare(
                "
                SELECT path, file_name, ext, width, height, file_size, modified_at
                FROM images
                WHERE source = 'library'
                  AND id NOT LIKE 'illutag-dev-fake:%'
                ORDER BY imported_at DESC
                LIMIT 100
                ",
            )
            .map_err(|error| format!("Failed to prepare dev sample image query: {error}"))?;
        samples = stmt
            .query_map([], dev_sample_image_from_row)
            .map_err(|error| format!("Failed to query dev sample images: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Failed to collect dev sample images: {error}"))?;
    }
    Ok(samples)
}

pub(super) fn dev_sample_image_from_row(row: &Row<'_>) -> rusqlite::Result<DevSampleImage> {
    Ok(DevSampleImage {
        path: row.get(0)?,
        file_name: row.get(1)?,
        ext: row.get(2)?,
        width: row.get(3)?,
        height: row.get(4)?,
        file_size: row.get(5)?,
        modified_at: row.get(6)?,
    })
}

pub(super) fn ensure_dev_fake_user_folders(
    conn: &Connection,
    now: i64,
) -> Result<Vec<i64>, String> {
    let mut ids = Vec::new();
    for index in 0..8 {
        conn.execute(
            "
            INSERT INTO user_folders (parent_id, name, sort_order, source_kind, source_path, created_at, updated_at)
            VALUES (NULL, ?1, ?2, 'dev_fake', NULL, ?3, ?3)
            ON CONFLICT DO NOTHING
            ",
            params![format!("Dev Fake Folder {}", index + 1), index, now],
        )
        .map_err(|error| format!("Failed to create dev fake folder: {error}"))?;
        let id = conn
            .query_row(
                "SELECT id FROM user_folders WHERE name = ?1 ORDER BY id DESC LIMIT 1",
                params![format!("Dev Fake Folder {}", index + 1)],
                |row| row.get(0),
            )
            .map_err(|error| format!("Failed to query dev fake folder: {error}"))?;
        ids.push(id);
    }
    Ok(ids)
}

pub(super) fn dev_hash_i64(value: i64) -> i64 {
    let mut x = value
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    x ^= x >> 33;
    (x & i64::MAX).max(1)
}

pub(super) fn remove_dev_test_files(root: &Path) -> Result<i64, String> {
    if !root.exists() {
        return Ok(0);
    }
    if !root
        .components()
        .any(|component| component.as_os_str().to_string_lossy() == ".illutag-dev-test")
    {
        return Err(format!(
            "Refusing to remove non-dev test directory: {}",
            root.display()
        ));
    }
    let file_count = WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .count() as i64;
    fs::remove_dir_all(root).map_err(|error| {
        format!(
            "Failed to remove dev test files {}: {error}",
            root.display()
        )
    })?;
    Ok(file_count)
}

pub(super) fn invalidate_library_cache(state: &AppState) {
    if let Ok(mut cache) = state.library.lock() {
        *cache = None;
    }
    if let Ok(mut cache) = state.clip_vector_cache.lock() {
        *cache = None;
    }
}

pub(super) fn dev_result(
    created_images: i64,
    created_files: i64,
    deleted_images: i64,
    deleted_files: i64,
    root_path: Option<PathBuf>,
    state: &AppState,
) -> DevStressToolResult {
    DevStressToolResult {
        created_images,
        created_files,
        deleted_images,
        deleted_files,
        root_path: root_path.map(|path| path.to_string_lossy().to_string()),
        database_path: state.database_path.to_string_lossy().to_string(),
    }
}

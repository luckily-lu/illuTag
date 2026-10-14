use super::*;

pub fn export_data_migration_backup(
    destination_path: String,
    frontend_settings: Option<serde_json::Value>,
    state: &AppState,
) -> Result<(), String> {
    let conn = open_database(&state.database_path)?;
    let backup = build_data_migration_backup(
        &conn,
        frontend_settings.unwrap_or_else(|| serde_json::json!({})),
    )?;
    let file = fs::File::create(&destination_path).map_err(|e| e.to_string())?;
    serde_json::to_writer(std::io::BufWriter::new(file), &backup).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn inspect_data_migration_backup(
    backup_path: String,
) -> Result<DataMigrationBackupInspection, String> {
    let backup = read_data_migration_backup(&backup_path)?;
    Ok(inspect_data_migration_backup_payload(&backup))
}

pub fn import_data_migration_backup(
    backup_path: String,
    path_mappings: Vec<BackupPathMapping>,
    state: &AppState,
) -> Result<LibraryStore, String> {
    crate::duplicates::management::revoke_all(state)?;
    let backup = read_data_migration_backup(&backup_path)?;
    if backup.format != "illutag-data-migration-backup" {
        return Err("Invalid illuTag backup format".to_string());
    }
    if backup.schema_version != 1 {
        return Err(format!(
            "Unsupported illuTag backup schema version: {}",
            backup.schema_version
        ));
    }

    let mut library = state
        .library
        .lock()
        .map_err(|_| "Library state is locked".to_string())?;
    let mut conn = open_database(&state.database_path)?;
    let tx = conn
        .transaction()
        .map_err(|error| format!("Failed to open migration import transaction: {error}"))?;

    import_library_folders_from_backup(&tx, &backup, &path_mappings)?;
    import_images_from_backup(&tx, &backup, &path_mappings)?;
    import_user_folders_from_backup(&tx, &backup)?;
    import_image_user_folders_from_backup(&tx, &backup)?;
    import_user_tags_from_backup(&tx, &backup)?;
    import_user_folder_rules_from_backup(&tx, &backup)?;
    import_reference_boards_from_backup(&tx, &backup)?;
    import_app_meta_from_backup(&tx, &backup)?;

    tx.commit()
        .map_err(|error| format!("Failed to commit migration import: {error}"))?;
    crate::duplicates::invalidate(state);

    let store = load_store(&conn)?;
    *library = Some(store.clone());
    Ok(store)
}

pub fn export_organized_folder_result(
    destination_dir: String,
    state: &AppState,
) -> Result<OrganizedExportReport, String> {
    use std::io::Write;
    let destination_root = PathBuf::from(destination_dir);
    fs::create_dir_all(&destination_root).map_err(|e| format!("创建导出目录失败：{e}"))?;
    let conn = open_database(&state.database_path)?;
    conn.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    let folder_paths = build_organized_export_folder_paths(&load_user_folders(&conn)?);
    for folder_path in folder_paths.values() {
        fs::create_dir_all(destination_root.join(folder_path)).map_err(|e| e.to_string())?;
    }
    let manifest_path = destination_root.join("_illuTag_export_manifest.json");
    let mut writer =
        std::io::BufWriter::new(fs::File::create(&manifest_path).map_err(|e| e.to_string())?);
    let header = serde_json::json!({"format":"illutag-organized-folder-export", "schemaVersion":1,
        "exportedAt":now_ms(),"appVersion":env!("CARGO_PKG_VERSION")});
    let header = serde_json::to_string(&header).map_err(|e| e.to_string())?;
    write!(writer, "{},\"entries\":[", &header[..header.len() - 1]).map_err(|e| e.to_string())?;
    let mut report = OrganizedExportReport {
        manifest_path: manifest_path.to_string_lossy().into(),
        exported_count: 0,
        skipped_count: 0,
        failed_count: 0,
        failures: Vec::new(),
    };
    // One SQLite snapshot, streamed rows: UI pagination and full-library IPC are never involved.
    let mut stmt = conn.prepare("SELECT i.id,i.path,i.file_name,m.folder_id,i.missing FROM image_user_folders m JOIN images i ON i.id=m.image_id WHERE COALESCE(i.trashed,0)=0 ORDER BY m.folder_id,i.id").map_err(|e| e.to_string())?;
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let image_id: String = row.get(0).map_err(|e| e.to_string())?;
        let original_path: String = row.get(1).map_err(|e| e.to_string())?;
        let file_name: String = row.get(2).map_err(|e| e.to_string())?;
        let folder_id: i64 = row.get(3).map_err(|e| e.to_string())?;
        let missing: i64 = row.get(4).map_err(|e| e.to_string())?;
        let Some(folder_path) = folder_paths.get(&folder_id) else {
            report.skipped_count += 1;
            continue;
        };
        if missing != 0 || !Path::new(&original_path).is_file() {
            report.skipped_count += 1;
            continue;
        }
        let target_dir = destination_root.join(folder_path);
        let target_path = next_available_export_path(&target_dir, &file_name, &mut HashSet::new());
        if let Err(error) = fs::copy(&original_path, &target_path) {
            report.failed_count += 1;
            if report.failures.len() < 100 {
                report.failures.push(format!("{original_path}：{error}"));
            }
            continue;
        }
        if report.exported_count > 0 {
            writer.write_all(b",").map_err(|e| e.to_string())?;
        }
        let entry = OrganizedExportManifestEntry {
            image_id,
            original_path,
            folder_id,
            folder_path: folder_path.to_string_lossy().into(),
            exported_path: target_path.to_string_lossy().into(),
        };
        serde_json::to_writer(&mut writer, &entry).map_err(|e| e.to_string())?;
        report.exported_count += 1;
    }
    writer
        .write_all(b"],\"summary\":")
        .map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut writer, &report).map_err(|e| e.to_string())?;
    writer.write_all(b"}").map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())?;
    Ok(report)
}

pub(super) fn build_organized_export_folder_paths(
    user_folders: &[UserFolder],
) -> HashMap<i64, PathBuf> {
    let folders_by_id = user_folders
        .iter()
        .map(|folder| (folder.id, folder))
        .collect::<HashMap<_, _>>();
    let mut cache = HashMap::<i64, PathBuf>::new();
    for folder in user_folders {
        let path = resolve_organized_export_folder_path(folder.id, &folders_by_id, &mut cache);
        cache.insert(folder.id, path);
    }
    cache
}

pub(super) fn resolve_organized_export_folder_path(
    folder_id: i64,
    folders_by_id: &HashMap<i64, &UserFolder>,
    cache: &mut HashMap<i64, PathBuf>,
) -> PathBuf {
    if let Some(cached) = cache.get(&folder_id) {
        return cached.clone();
    }
    let Some(folder) = folders_by_id.get(&folder_id) else {
        return PathBuf::from(format!("folder-{folder_id}"));
    };
    let segment = sanitize_export_path_segment(&folder.name, &format!("folder-{folder_id}"));
    let path = match folder.parent_id {
        Some(parent_id) => {
            resolve_organized_export_folder_path(parent_id, folders_by_id, cache).join(segment)
        }
        None => PathBuf::from(segment),
    };
    cache.insert(folder_id, path.clone());
    path
}

pub(super) fn sanitize_export_path_segment(value: &str, fallback: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            ch if ch.is_control() => '_',
            ch => ch,
        })
        .collect::<String>()
        .trim()
        .trim_end_matches('.')
        .to_string();
    if sanitized.is_empty() {
        fallback.to_string()
    } else {
        sanitized
    }
}

pub(super) fn next_available_export_path(
    target_dir: &Path,
    file_name: &str,
    used_export_paths: &mut HashSet<PathBuf>,
) -> PathBuf {
    let sanitized_file_name = sanitize_export_path_segment(file_name, "image");
    let source_file_name = Path::new(&sanitized_file_name);
    let stem = source_file_name
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("image");
    let extension = source_file_name
        .extension()
        .and_then(|value| value.to_str());
    let mut index = 0usize;
    loop {
        let candidate_name = if index == 0 {
            sanitized_file_name.clone()
        } else if let Some(extension) = extension {
            format!("{stem} ({index}).{extension}")
        } else {
            format!("{stem} ({index})")
        };
        let candidate = target_dir.join(candidate_name);
        if !used_export_paths.contains(&candidate) && !candidate.exists() {
            used_export_paths.insert(candidate.clone());
            return candidate;
        }
        index += 1;
    }
}

pub(super) fn build_data_migration_backup(
    conn: &Connection,
    frontend_settings: serde_json::Value,
) -> Result<DataMigrationBackup, String> {
    Ok(DataMigrationBackup {
        format: "illutag-data-migration-backup".to_string(),
        schema_version: 1,
        exported_at: now_ms(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        library: load_complete_store(conn)?,
        image_user_custom_tags: load_backup_image_user_custom_tags(conn)?,
        image_user_supplement_tags: load_backup_image_user_supplement_tags(conn)?,
        user_custom_tags: load_backup_user_custom_tags(conn)?,
        user_tag_folders: load_backup_user_tag_folders(conn)?,
        user_tag_folder_members: load_backup_user_tag_folder_members(conn)?,
        user_folder_rules: load_backup_user_folder_rules(conn)?,
        app_meta: load_backup_app_meta(conn)?,
        frontend_settings,
    })
}

pub(super) fn read_data_migration_backup(path: &str) -> Result<DataMigrationBackup, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("Failed to read migration backup: {error}"))?;
    serde_json::from_str(&text)
        .map_err(|error| format!("Failed to parse migration backup: {error}"))
}

pub(super) fn inspect_data_migration_backup_payload(
    backup: &DataMigrationBackup,
) -> DataMigrationBackupInspection {
    DataMigrationBackupInspection {
        schema_version: backup.schema_version,
        exported_at: backup.exported_at,
        app_version: backup.app_version.clone(),
        folder_count: backup.library.folders.len(),
        image_count: backup.library.images.len(),
        user_folder_count: backup.library.user_folders.len(),
        reference_board_count: backup.library.reference_boards.len(),
        path_statuses: backup
            .library
            .folders
            .iter()
            .map(|folder| BackupPathStatus {
                path: folder.path.clone(),
                exists: Path::new(&folder.path).exists(),
            })
            .collect(),
        frontend_settings: backup.frontend_settings.clone(),
    }
}

pub(super) fn load_backup_image_user_custom_tags(
    conn: &Connection,
) -> Result<Vec<BackupImageUserCustomTag>, String> {
    let mut stmt = conn
        .prepare(
            "
            SELECT image_id, tag_text, created_at, updated_at
            FROM image_user_custom_tags
            ORDER BY image_id, tag_text
            ",
        )
        .map_err(|error| format!("Failed to prepare custom tag backup query: {error}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(BackupImageUserCustomTag {
                image_id: row.get(0)?,
                tag_text: row.get(1)?,
                created_at: row.get(2)?,
                updated_at: row.get(3)?,
            })
        })
        .map_err(|error| format!("Failed to query custom tag backup: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to collect custom tag backup: {error}"))?;
    Ok(rows)
}

pub(super) fn load_backup_image_user_supplement_tags(
    conn: &Connection,
) -> Result<Vec<BackupImageUserSupplementTag>, String> {
    let mut stmt = conn
        .prepare(
            "
            SELECT image_id, tag_en, tag_zh, created_at, updated_at
            FROM image_user_supplement_tags
            ORDER BY image_id, tag_en
            ",
        )
        .map_err(|error| format!("Failed to prepare supplement tag backup query: {error}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(BackupImageUserSupplementTag {
                image_id: row.get(0)?,
                tag_en: row.get(1)?,
                tag_zh: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })
        .map_err(|error| format!("Failed to query supplement tag backup: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to collect supplement tag backup: {error}"))?;
    Ok(rows)
}

pub(super) fn load_backup_user_custom_tags(
    conn: &Connection,
) -> Result<Vec<BackupUserCustomTag>, String> {
    let mut stmt = conn
        .prepare(
            "
            SELECT tag_text, created_at, updated_at
            FROM user_custom_tags
            ORDER BY tag_text COLLATE NOCASE
            ",
        )
        .map_err(|error| format!("Failed to prepare user custom tag backup query: {error}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(BackupUserCustomTag {
                tag_text: row.get(0)?,
                created_at: row.get(1)?,
                updated_at: row.get(2)?,
            })
        })
        .map_err(|error| format!("Failed to query user custom tag backup: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to collect user custom tag backup: {error}"))?;
    Ok(rows)
}

pub(super) fn load_backup_user_tag_folders(
    conn: &Connection,
) -> Result<Vec<BackupUserTagFolder>, String> {
    let mut stmt = conn
        .prepare(
            "
            SELECT id, name, sort_order, created_at, updated_at
            FROM user_tag_folders
            ORDER BY sort_order, id
            ",
        )
        .map_err(|error| format!("Failed to prepare tag folder backup query: {error}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(BackupUserTagFolder {
                id: row.get(0)?,
                name: row.get(1)?,
                sort_order: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })
        .map_err(|error| format!("Failed to query tag folder backup: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to collect tag folder backup: {error}"))?;
    Ok(rows)
}

pub(super) fn load_backup_user_tag_folder_members(
    conn: &Connection,
) -> Result<Vec<BackupUserTagFolderMember>, String> {
    let mut stmt = conn
        .prepare(
            "
            SELECT folder_id, tag_text, created_at, updated_at
            FROM user_tag_folder_members
            ORDER BY folder_id, tag_text COLLATE NOCASE
            ",
        )
        .map_err(|error| format!("Failed to prepare tag folder member backup query: {error}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(BackupUserTagFolderMember {
                folder_id: row.get(0)?,
                tag_text: row.get(1)?,
                created_at: row.get(2)?,
                updated_at: row.get(3)?,
            })
        })
        .map_err(|error| format!("Failed to query tag folder member backup: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to collect tag folder member backup: {error}"))?;
    Ok(rows)
}

pub(super) fn load_backup_user_folder_rules(
    conn: &Connection,
) -> Result<Vec<UserFolderRule>, String> {
    let mut stmt = conn
        .prepare(
            "
            SELECT folder_id, rule_json, updated_at
            FROM user_folder_rules
            ORDER BY folder_id
            ",
        )
        .map_err(|error| format!("Failed to prepare user folder rule backup query: {error}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|error| format!("Failed to query user folder rule backup: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to collect user folder rule backup: {error}"))?;

    let mut rules = Vec::with_capacity(rows.len());
    for (folder_id, rule_json, updated_at) in rows {
        let conditions = serde_json::from_str::<Vec<UserFolderRuleCondition>>(&rule_json)
            .map_err(|error| format!("Failed to parse user folder rule backup: {error}"))?;
        rules.push(UserFolderRule {
            folder_id,
            conditions,
            updated_at,
        });
    }
    Ok(rules)
}

pub(super) fn load_backup_app_meta(conn: &Connection) -> Result<Vec<BackupAppMeta>, String> {
    let mut stmt = conn
        .prepare(
            "
            SELECT key, value, updated_at
            FROM app_meta
            ORDER BY key
            ",
        )
        .map_err(|error| format!("Failed to prepare app meta backup query: {error}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(BackupAppMeta {
                key: row.get(0)?,
                value: row.get(1)?,
                updated_at: row.get(2)?,
            })
        })
        .map_err(|error| format!("Failed to query app meta backup: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to collect app meta backup: {error}"))?;
    Ok(rows)
}

pub(super) fn remap_backup_path(path: &str, mappings: &[BackupPathMapping]) -> String {
    let mut best_mapping: Option<&BackupPathMapping> = None;
    let path_lower = path.to_lowercase();
    for mapping in mappings {
        let from = mapping.from.trim();
        let to = mapping.to.trim();
        if from.is_empty() || to.is_empty() {
            continue;
        }
        let from_lower = from.to_lowercase();
        let is_match = path_lower == from_lower
            || path_lower
                .strip_prefix(&from_lower)
                .is_some_and(|suffix| suffix.starts_with('\\') || suffix.starts_with('/'));
        if is_match {
            let replace_best = match best_mapping {
                Some(best) => from.len() > best.from.trim().len(),
                None => true,
            };
            if replace_best {
                best_mapping = Some(mapping);
            }
        }
    }
    let Some(mapping) = best_mapping else {
        return path.to_string();
    };
    let from = mapping.from.trim();
    let to = mapping.to.trim().trim_end_matches(['\\', '/']);
    let Some(suffix) = path.get(from.len()..) else {
        return path.to_string();
    };
    if suffix.is_empty() {
        return to.to_string();
    }
    let suffix = suffix.trim_start_matches(['\\', '/']);
    if suffix.is_empty() {
        to.to_string()
    } else {
        format!("{to}\\{suffix}")
    }
}

pub(super) fn import_library_folders_from_backup(
    conn: &Connection,
    backup: &DataMigrationBackup,
    mappings: &[BackupPathMapping],
) -> Result<(), String> {
    for folder in &backup.library.folders {
        let path = remap_backup_path(&folder.path, mappings);
        conn.execute(
            "
            INSERT INTO folders (id, path, added_at, last_scanned_at, hidden)
            VALUES (?1, ?2, ?3, ?4, 0)
            ON CONFLICT(id) DO UPDATE SET
              path = excluded.path,
              added_at = excluded.added_at,
              last_scanned_at = excluded.last_scanned_at,
              hidden = 0
            ",
            params![folder.id, path, folder.added_at, folder.last_scanned_at],
        )
        .map_err(|error| format!("Failed to import library folder: {error}"))?;
    }
    Ok(())
}

pub(super) fn import_images_from_backup(
    conn: &Connection,
    backup: &DataMigrationBackup,
    mappings: &[BackupPathMapping],
) -> Result<(), String> {
    for image in &backup.library.images {
        let path = remap_backup_path(&image.path, mappings);
        conn.execute(
            "
            INSERT INTO images (
              id, path, file_name, ext, width, height, file_size, modified_at,
              imported_at, folder_id, missing, trashed, is_favorite, source
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
            ON CONFLICT(id) DO UPDATE SET
              path = excluded.path,
              file_name = excluded.file_name,
              ext = excluded.ext,
              width = excluded.width,
              height = excluded.height,
              file_size = excluded.file_size,
              modified_at = excluded.modified_at,
              imported_at = excluded.imported_at,
              folder_id = excluded.folder_id,
              missing = excluded.missing,
              trashed = excluded.trashed,
              is_favorite = excluded.is_favorite,
              source = excluded.source
            ",
            params![
                image.id,
                path,
                image.file_name,
                image.ext,
                image.width,
                image.height,
                image.file_size,
                image.modified_at,
                image.imported_at,
                image.folder_id,
                if image.missing { 1 } else { 0 },
                if image.trashed { 1 } else { 0 },
                if image.is_favorite { 1 } else { 0 },
                image.source,
            ],
        )
        .map_err(|error| format!("Failed to import image index: {error}"))?;
    }
    Ok(())
}

pub(super) fn import_user_folders_from_backup(
    conn: &Connection,
    backup: &DataMigrationBackup,
) -> Result<(), String> {
    for folder in &backup.library.user_folders {
        conn.execute(
            "
            INSERT INTO user_folders (id, parent_id, name, sort_order, source_kind, source_path, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, 'manual', NULL, ?5, ?6)
            ON CONFLICT(id) DO UPDATE SET
              parent_id = excluded.parent_id,
              name = excluded.name,
              sort_order = excluded.sort_order,
              created_at = excluded.created_at,
              updated_at = excluded.updated_at
            ",
            params![folder.id, folder.parent_id, folder.name, folder.sort_order, folder.created_at, folder.updated_at],
        )
        .map_err(|error| format!("Failed to import user folder: {error}"))?;
    }
    Ok(())
}

pub(super) fn import_image_user_folders_from_backup(
    conn: &Connection,
    backup: &DataMigrationBackup,
) -> Result<(), String> {
    for assignment in &backup.library.image_folders {
        conn.execute(
            "
            INSERT OR IGNORE INTO storage_image_user_folders (image_id, folder_id, assigned_at)
            VALUES ((SELECT image_pk FROM images WHERE id=?1), ?2, ?3)
            ",
            params![assignment.image_id, assignment.folder_id, now_ms()],
        )
        .map_err(|error| format!("Failed to import image folder assignment: {error}"))?;
    }
    Ok(())
}

pub(super) fn import_user_tags_from_backup(
    conn: &Connection,
    backup: &DataMigrationBackup,
) -> Result<(), String> {
    for tag in &backup.user_custom_tags {
        conn.execute(
            "
            INSERT INTO user_custom_tags (tag_text, created_at, updated_at)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(tag_text) DO UPDATE SET
              updated_at = excluded.updated_at
            ",
            params![tag.tag_text, tag.created_at, tag.updated_at],
        )
        .map_err(|error| format!("Failed to import user custom tag: {error}"))?;
    }
    for tag in &backup.image_user_custom_tags {
        conn.execute(
            "
            INSERT INTO storage_image_user_custom_tags (image_id, tag_text, created_at, updated_at)
            VALUES ((SELECT image_pk FROM images WHERE id=?1), ?2, ?3, ?4)
            ON CONFLICT(image_id, tag_text) DO UPDATE SET
              updated_at = excluded.updated_at
            ",
            params![tag.image_id, tag.tag_text, tag.created_at, tag.updated_at],
        )
        .map_err(|error| format!("Failed to import image custom tag: {error}"))?;
    }
    for tag in &backup.image_user_supplement_tags {
        conn.execute(
            "
            INSERT INTO storage_image_user_supplement_tags (image_id, tag_en, tag_zh, created_at, updated_at)
            VALUES ((SELECT image_pk FROM images WHERE id=?1), ?2, ?3, ?4, ?5)
            ON CONFLICT(image_id, tag_en) DO UPDATE SET
              tag_zh = COALESCE(excluded.tag_zh, storage_image_user_supplement_tags.tag_zh),
              updated_at = excluded.updated_at
            ",
            params![tag.image_id, tag.tag_en, tag.tag_zh, tag.created_at, tag.updated_at],
        )
        .map_err(|error| format!("Failed to import image supplement tag: {error}"))?;
    }
    for folder in &backup.user_tag_folders {
        conn.execute(
            "
            INSERT INTO user_tag_folders (id, name, sort_order, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(id) DO UPDATE SET
              name = excluded.name,
              sort_order = excluded.sort_order,
              created_at = excluded.created_at,
              updated_at = excluded.updated_at
            ",
            params![
                folder.id,
                folder.name,
                folder.sort_order,
                folder.created_at,
                folder.updated_at
            ],
        )
        .map_err(|error| format!("Failed to import tag folder: {error}"))?;
    }
    for member in &backup.user_tag_folder_members {
        conn.execute(
            "
            INSERT INTO user_tag_folder_members (folder_id, tag_text, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(tag_text) DO UPDATE SET
              folder_id = excluded.folder_id,
              updated_at = excluded.updated_at
            ",
            params![
                member.folder_id,
                member.tag_text,
                member.created_at,
                member.updated_at
            ],
        )
        .map_err(|error| format!("Failed to import tag folder member: {error}"))?;
    }
    Ok(())
}

pub(super) fn import_user_folder_rules_from_backup(
    conn: &Connection,
    backup: &DataMigrationBackup,
) -> Result<(), String> {
    for rule in &backup.user_folder_rules {
        let rule_json = serde_json::to_string(&rule.conditions)
            .map_err(|error| format!("Failed to serialize imported user folder rule: {error}"))?;
        conn.execute(
            "
            INSERT INTO user_folder_rules (folder_id, rule_json, updated_at)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(folder_id) DO UPDATE SET
              rule_json = excluded.rule_json,
              updated_at = excluded.updated_at
            ",
            params![rule.folder_id, rule_json, rule.updated_at],
        )
        .map_err(|error| format!("Failed to import user folder rule: {error}"))?;
    }
    Ok(())
}

pub(super) fn import_reference_boards_from_backup(
    conn: &Connection,
    backup: &DataMigrationBackup,
) -> Result<(), String> {
    for folder in &backup.library.reference_board_folders {
        conn.execute(
            "
            INSERT INTO reference_board_folders (id, name, sort_order, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(id) DO UPDATE SET
              name = excluded.name,
              sort_order = excluded.sort_order,
              created_at = excluded.created_at,
              updated_at = excluded.updated_at
            ",
            params![
                folder.id,
                folder.name,
                folder.sort_order,
                folder.created_at,
                folder.updated_at
            ],
        )
        .map_err(|error| format!("Failed to import reference board folder: {error}"))?;
    }
    for board in &backup.library.reference_boards {
        conn.execute(
            "
            INSERT INTO reference_boards (id, folder_id, name, sort_order, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(id) DO UPDATE SET
              folder_id = excluded.folder_id,
              name = excluded.name,
              sort_order = excluded.sort_order,
              created_at = excluded.created_at,
              updated_at = excluded.updated_at
            ",
            params![
                board.id,
                board.folder_id,
                board.name,
                board.sort_order,
                board.created_at,
                board.updated_at
            ],
        )
        .map_err(|error| format!("Failed to import reference board: {error}"))?;
    }
    for item in &backup.library.reference_board_items {
        conn.execute(
            "
            INSERT INTO storage_reference_board_items (
              id, board_id, image_id, x, y, width, height, rotation, flip_x, flip_y, z_index, created_at
            )
            VALUES (?1, ?2,(SELECT image_pk FROM images WHERE id=?3), ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            ON CONFLICT(id) DO UPDATE SET
              board_id = excluded.board_id,
              image_id = excluded.image_id,
              x = excluded.x,
              y = excluded.y,
              width = excluded.width,
              height = excluded.height,
              rotation = excluded.rotation,
              flip_x = excluded.flip_x,
              flip_y = excluded.flip_y,
              z_index = excluded.z_index,
              created_at = excluded.created_at
            ",
            params![
                item.id,
                item.board_id,
                item.image_id,
                item.x,
                item.y,
                item.width,
                item.height,
                item.rotation,
                if item.flip_x { 1 } else { 0 },
                if item.flip_y { 1 } else { 0 },
                item.z_index,
                item.created_at,
            ],
        )
        .map_err(|error| format!("Failed to import reference board item: {error}"))?;
    }
    Ok(())
}

pub(super) fn import_app_meta_from_backup(
    conn: &Connection,
    backup: &DataMigrationBackup,
) -> Result<(), String> {
    for meta in &backup.app_meta {
        conn.execute(
            "
            INSERT INTO app_meta (key, value, updated_at)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(key) DO UPDATE SET
              value = excluded.value,
              updated_at = excluded.updated_at
            ",
            params![meta.key, meta.value, meta.updated_at],
        )
        .map_err(|error| format!("Failed to import app meta: {error}"))?;
    }
    Ok(())
}

pub(super) fn load_complete_store(conn: &Connection) -> Result<LibraryStore, String> {
    Ok(LibraryStore {
        folders: load_folders(conn)?,
        images: load_images(conn)?,
        total_image_count: count_gallery_images(conn, Some(ACTIVE_GALLERY_IMAGE_WHERE))?,
        large_library_mode: false,
        image_offset: 0,
        image_limit: 0,
        user_folders: load_user_folders(conn)?,
        image_folders: load_image_folder_assignments(conn)?,
        reference_board_folders: load_reference_board_folders(conn)?,
        reference_boards: load_reference_boards(conn)?,
        reference_board_items: load_reference_board_items(conn)?,
    })
}

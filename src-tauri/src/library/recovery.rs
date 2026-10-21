//! Complete, verified SQLite snapshots. Original files in external source folders are excluded.
use super::*;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::sync::atomic::Ordering as AtomicOrdering;

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct PackageFile {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct RestoreManifest {
    pub format: String,
    pub package_version: u32,
    pub schema_version: i64,
    pub app_version: String,
    pub created_at: i64,
    pub source_data_dir: String,
    #[serde(default)]
    pub source_directories: Vec<String>,
    pub image_count: i64,
    pub counts: std::collections::BTreeMap<String, i64>,
    pub frontend_settings: serde_json::Value,
    pub files: Vec<PackageFile>,
    pub external_originals_included: bool,
    pub models: serde_json::Value,
}

pub struct MaintenanceGuard<'a> {
    state: &'a AppState,
    keep: bool,
    _write: std::sync::MutexGuard<'a, ()>,
}
impl Drop for MaintenanceGuard<'_> {
    fn drop(&mut self) {
        if !self.keep {
            self.state.maintenance.store(false, AtomicOrdering::SeqCst);
        }
    }
}
pub fn begin_maintenance(state: &AppState) -> Result<MaintenanceGuard<'_>, String> {
    if state.read_only {
        return Err("只读图库无法维护数据".into());
    }
    state
        .maintenance
        .compare_exchange(false, true, AtomicOrdering::SeqCst, AtomicOrdering::SeqCst)
        .map_err(|_| "已有维护操作运行或需要重启".to_string())?;
    let write = state.write_lock.lock().map_err(|e| e.to_string())?;
    let guard = MaintenanceGuard {
        state,
        keep: false,
        _write: write,
    };
    if state
        .jobs
        .list()?
        .iter()
        .any(|j| ["queued", "running", "pausing", "paused", "cancelling"].contains(&j.state.as_str()))
        || *state
            .background_scan_running
            .lock()
            .map_err(|e| e.to_string())?
        || *state
            .thumbnail_generation_running
            .lock()
            .map_err(|e| e.to_string())?
        || *state
            .natural_language_scan_running
            .lock()
            .map_err(|e| e.to_string())?
        || *state
            .startup_cleanup_running
            .lock()
            .map_err(|e| e.to_string())?
    {
        return Err("请先完成或取消后台任务，再维护数据".into());
    }
    Ok(guard)
}
pub fn file_hash(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut buf = [0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
pub fn sqlite_snapshot(source: &Path, destination: &Path) -> Result<(), String> {
    if destination.exists() {
        return Err("快照目标已存在，请使用新目录".into());
    }
    let source = Connection::open_with_flags(source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    let mut target = Connection::open(destination).map_err(|e| e.to_string())?;
    let backup = rusqlite::backup::Backup::new(&source, &mut target).map_err(|e| e.to_string())?;
    backup
        .run_to_completion(8192, Duration::from_millis(5), None)
        .map_err(|e| e.to_string())?;
    drop(backup);
    target
        .execute_batch("PRAGMA journal_mode=DELETE;")
        .map_err(|e| e.to_string())?;
    verify_database(&target)?;
    Ok(())
}
pub fn verify_database(conn: &Connection) -> Result<(), String> {
    // Verification uses the same bounded page cache as normal reads. The default 2 MiB
    // cache thrashes when checking tens of millions of tag-to-image references.
    conn.execute_batch(
        "PRAGMA cache_size=-262144; PRAGMA mmap_size=8589934592; PRAGMA temp_store=MEMORY;",
    )
    .map_err(|e| e.to_string())?;
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if !(1..=SCHEMA_USER_VERSION).contains(&version) {
        return Err(format!("不支持的数据版本 {version}"));
    }
    let check: String = conn
        .query_row("PRAGMA quick_check", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if check != "ok" {
        return Err(format!("数据库校验失败：{check}"));
    }
    let violations: bool = conn
        .prepare("PRAGMA foreign_key_check")
        .map_err(|e| e.to_string())?
        .exists([])
        .map_err(|e| e.to_string())?;
    if violations {
        return Err("数据库存在失效引用，拒绝切换".into());
    }
    Ok(())
}
fn table_counts(conn: &Connection) -> Result<std::collections::BTreeMap<String, i64>, String> {
    let compact = conn
        .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
        .map_err(|e| e.to_string())?
        == 3;
    let names=conn.prepare("SELECT name FROM sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' AND name NOT LIKE 'storage_%' ORDER BY name").map_err(|e|e.to_string())?
        .query_map([],|r|r.get::<_,String>(0)).map_err(|e|e.to_string())?.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())?;
    names
        .into_iter()
        .map(|name| {
            let physical = if compact {
                compact::physical_name(&name)
            } else {
                name.clone()
            };
            let count = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM \"{}\"", physical.replace('"', "\"\"")),
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            Ok((name, count))
        })
        .collect()
}
fn copy_tree(source: &Path, target: &Path) -> Result<(), String> {
    if !source.exists() {
        return Ok(());
    }
    fs::create_dir_all(target).map_err(|e| e.to_string())?;
    for item in WalkDir::new(source).follow_links(false) {
        let item = item.map_err(|e| e.to_string())?;
        if item.file_type().is_symlink() {
            return Err("恢复包不接受目录链接".into());
        }
        let dest = target.join(
            item.path()
                .strip_prefix(source)
                .map_err(|e| e.to_string())?,
        );
        if item.file_type().is_dir() {
            fs::create_dir_all(dest).map_err(|e| e.to_string())?;
        } else if item.file_type().is_file() {
            fs::copy(item.path(), dest).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
pub fn create_restore_package(
    destination: String,
    frontend_settings: serde_json::Value,
    state: &AppState,
) -> Result<String, String> {
    let _maintenance = begin_maintenance(state)?;
    let destination = PathBuf::from(destination);
    if !destination.is_absolute() {
        return Err("备份位置必须是绝对路径".into());
    }
    let package = destination.join(format!("illutag-restore-{}", now_ms()));
    fs::create_dir(&package).map_err(|e| format!("创建恢复包失败：{e}"))?;
    sqlite_snapshot(&state.database_path, &package.join("illutag.sqlite"))?;
    for name in ["clipboard", "managed"] {
        copy_tree(&state.data_dir.join(name), &package.join(name))?;
    }
    for name in [
        "window-state.json",
        "illutag.window-state.json",
        "preferences.json",
        "tagger_config.json",
        "tag_groups.json",
    ] {
        let p = state.data_dir.join(name);
        if p.is_file() {
            fs::copy(p, package.join(name)).map_err(|e| e.to_string())?;
        }
    }
    let conn = Connection::open_with_flags(
        package.join("illutag.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|e| e.to_string())?;
    let counts = table_counts(&conn)?;
    let schema_version = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let source_directories = load_folders(&conn)?.into_iter().map(|f| f.path).collect();
    drop(conn);
    let mut files = Vec::new();
    for entry in WalkDir::new(&package) {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_type().is_file() {
            files.push(PackageFile {
                path: entry
                    .path()
                    .strip_prefix(&package)
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .replace('\\', "/"),
                bytes: entry.metadata().map_err(|e| e.to_string())?.len(),
                sha256: file_hash(entry.path())?,
            });
        }
    }
    let model_manifest = env::var_os("ILLUTAG_RESOURCE_ROOT")
        .map(PathBuf::from)
        .or_else(|| {
            env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(Path::to_path_buf))
        })
        .map(|p| p.join("model-manifest.json"));
    let models = model_manifest
        .filter(|p| p.is_file())
        .and_then(|p| fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(serde_json::json!({"chineseClip":"onnx_v1","wdTagger":"wd-swinv2-tagger-v3"}));
    let manifest = RestoreManifest {
        format: "illutag-restore-package".into(),
        package_version: 1,
        schema_version,
        app_version: env!("CARGO_PKG_VERSION").into(),
        created_at: now_ms(),
        source_data_dir: state.data_dir.to_string_lossy().into(),
        source_directories,
        image_count: *counts.get("images").unwrap_or(&0),
        counts,
        frontend_settings,
        files,
        external_originals_included: false,
        models,
    };
    serde_json::to_writer_pretty(
        fs::File::create(package.join("manifest.json")).map_err(|e| e.to_string())?,
        &manifest,
    )
    .map_err(|e| e.to_string())?;
    inspect_restore_package(package.to_string_lossy().into())?;
    Ok(package.to_string_lossy().into())
}
pub fn inspect_restore_package(package_dir: String) -> Result<RestoreManifest, String> {
    let root = PathBuf::from(package_dir);
    let manifest: RestoreManifest = serde_json::from_reader(
        fs::File::open(root.join("manifest.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if manifest.format != "illutag-restore-package"
        || manifest.package_version != 1
        || manifest.schema_version > SCHEMA_USER_VERSION
    {
        return Err("恢复包格式或数据版本不受支持".into());
    }
    if !manifest.files.iter().any(|f| f.path == "illutag.sqlite") {
        return Err("恢复包缺少数据库".into());
    }
    for file in &manifest.files {
        let relative = Path::new(&file.path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err("恢复包包含无效相对路径".into());
        }
        let path = root.join(relative);
        if path.metadata().map_err(|e| e.to_string())?.len() != file.bytes
            || file_hash(&path)? != file.sha256
        {
            return Err(format!("恢复包文件校验失败：{}", file.path));
        }
    }
    let conn = Connection::open_with_flags(
        root.join("illutag.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|e| e.to_string())?;
    verify_database(&conn)?;
    if table_counts(&conn)? != manifest.counts {
        return Err("恢复包记录数量不一致".into());
    }
    Ok(manifest)
}
pub fn remap_database_paths(path: &Path, mappings: &[BackupPathMapping]) -> Result<(), String> {
    let mut conn = Connection::open(path).map_err(|e| e.to_string())?;
    let compact = conn
        .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
        .map_err(|e| e.to_string())?
        == 3;
    conn.execute_batch("PRAGMA foreign_keys=OFF;")
        .map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    for (table, column) in [
        ("folders", "path"),
        ("user_folders", "source_path"),
        ("image_thumbnails", "thumb_path"),
    ] {
        let table = if compact {
            compact::physical_name(table)
        } else {
            table.to_string()
        };
        let sql = format!("SELECT rowid,{column} FROM {table} WHERE {column} IS NOT NULL");
        let rows = tx
            .prepare(&sql)
            .map_err(|e| e.to_string())?
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        for (row, value) in rows {
            let next = remap_backup_path(&value, mappings);
            if next != value {
                tx.execute(
                    &format!("UPDATE {table} SET {column}=?1 WHERE rowid=?2"),
                    params![next, row],
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }
    let mut after = 0i64;
    loop {
        let rows = tx
            .prepare("SELECT rowid,id,path FROM images WHERE rowid>?1 ORDER BY rowid LIMIT 1000")
            .map_err(|e| e.to_string())?
            .query_map([after], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        if rows.is_empty() {
            break;
        }
        after = rows.last().unwrap().0;
        for (_, id, path) in rows {
            let next_id = remap_backup_path(&id, mappings);
            let next_path = remap_backup_path(&path, mappings);
            if next_id != id && !compact {
                for table in [
                    "reference_board_items",
                    "image_thumbnails",
                    "image_clip_embeddings",
                    "image_atmosphere_signatures",
                    "image_color_signatures",
                    "image_user_folders",
                    "image_auto_tags",
                    "image_user_custom_tags",
                    "image_user_supplement_tags",
                    "image_tags",
                    "image_ai_state",
                ] {
                    tx.execute(
                        &format!("UPDATE {table} SET image_id=?1 WHERE image_id=?2"),
                        params![next_id, id],
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
            if next_id != id || next_path != path {
                tx.execute(
                    "UPDATE images SET id=?1,path=?2 WHERE id=?3",
                    params![next_id, next_path, id],
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }
    tag_covers::remap_choices(&tx, mappings)?;
    tx.commit().map_err(|e| e.to_string())?;
    verify_database(&conn)
}
pub fn restore_package(
    package_dir: String,
    mut path_mappings: Vec<BackupPathMapping>,
    state: &AppState,
) -> Result<RestoreManifest, String> {
    let mut maintenance = begin_maintenance(state)?;
    let manifest = inspect_restore_package(package_dir.clone())?;
    let package = PathBuf::from(package_dir);
    let staged = state.data_dir.join(format!("restore-staging-{}", now_ms()));
    fs::create_dir(&staged).map_err(|e| e.to_string())?;
    for file in &manifest.files {
        if file.path == "illutag.sqlite" && manifest.schema_version < 3 {
            continue;
        }
        let target = staged.join(&file.path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::copy(package.join(&file.path), target).map_err(|e| e.to_string())?;
    }
    if manifest.schema_version < 3 {
        compact::convert_staged(
            &package.join("illutag.sqlite"),
            &staged.join("illutag.sqlite"),
        )?;
    }
    path_mappings.push(BackupPathMapping {
        from: manifest.source_data_dir.clone(),
        to: state.data_dir.to_string_lossy().into(),
    });
    remap_database_paths(&staged.join("illutag.sqlite"), &path_mappings)?;
    let rollback = state.data_dir.join(format!("before-restore-{}", now_ms()));
    fs::create_dir(&rollback).map_err(|e| e.to_string())?;
    // Close checkpoint connection before renaming; there are no persistent SQLite handles in AppState.
    {
        let conn = open_database(&state.database_path)?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(|e| e.to_string())?;
    }
    let mut replaced = Vec::new();
    let result = (|| -> Result<(), String> {
        for name in [
            "illutag.sqlite",
            "clipboard",
            "managed",
            "window-state.json",
            "illutag.window-state.json",
            "preferences.json",
            "tagger_config.json",
            "tag_groups.json",
        ] {
            let next = staged.join(name);
            if !next.exists() {
                continue;
            }
            let current = state.data_dir.join(name);
            let old = rollback.join(name);
            let existed = current.exists();
            if existed {
                fs::rename(&current, &old).map_err(|e| e.to_string())?;
            }
            replaced.push((name, existed));
            fs::rename(next, current).map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        let mut rollback_errors = Vec::new();
        for (name, existed) in replaced.into_iter().rev() {
            let current = state.data_dir.join(name);
            if current.exists() {
                let failed = staged.join(format!("failed-{name}"));
                if let Err(e) = fs::rename(&current, failed) {
                    rollback_errors.push(e.to_string());
                }
            }
            if existed {
                if let Err(e) = fs::rename(rollback.join(name), current) {
                    rollback_errors.push(e.to_string());
                }
            }
        }
        if !rollback_errors.is_empty() {
            maintenance.keep = true;
            return Err(format!(
                "恢复失败，请关闭程序后从 {} 恢复原快照。回滚未完成：{}；原因：{error}",
                rollback.display(),
                rollback_errors.join("；")
            ));
        }
        return Err(format!("恢复失败，已回滚原数据：{error}"));
    }
    maintenance.keep = true;
    *state.library.lock().map_err(|e| e.to_string())? = None;
    invalidate_all_similarity_caches(state);
    crate::duplicates::invalidate(state);
    Ok(manifest)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationLogIssue {
    pub kind: String,
    pub operation: String,
    pub image_pk: Option<i64>,
    pub image_id: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OperationLogMigration {
    pub journal: String,
    /// migrated | partial | skipped
    pub status: String,
    pub rows_total: u64,
    pub rows_migrated: u64,
    pub unresolved: Vec<OperationLogIssue>,
    pub message: String,
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hex_digest(bytes: &[u8]) -> String {
    hex_encode(&Sha256::digest(bytes))
}

fn canonical_journal_path(path: &Path) -> Result<String, String> {
    let value = fs::canonicalize(path)
        .map_err(|error| format!("文件无法访问：{error}"))?
        .to_string_lossy()
        .into_owned();
    Ok(if cfg!(windows) {
        value.to_lowercase()
    } else {
        value
    })
}

fn journal_is_supported(conn: &Connection, tables: &[&str]) -> Result<(), String> {
    let check: String = conn
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|error| format!("日志校验失败：{error}"))?;
    if check != "ok" {
        return Err(format!("日志校验失败：{check}"));
    }
    for table in tables {
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                [table],
                |row| row.get(0),
            )
            .map_err(|error| format!("读取日志结构失败：{error}"))?;
        if !exists {
            return Err(format!("日志缺少表 {table}，版本不受支持"));
        }
    }
    Ok(())
}

fn journal_binding(conn: &Connection) -> Option<String> {
    conn.query_row("SELECT value FROM meta WHERE key='binding'", [], |row| {
        row.get::<_, String>(0)
    })
    .ok()
}

fn snapshot_operation_journal(source: &Path, destination: &Path) -> Result<(), String> {
    let source = Connection::open_with_flags(
        source,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|error| format!("打开操作日志失败：{error}"))?;
    let mut target =
        Connection::open(destination).map_err(|error| format!("创建日志快照失败：{error}"))?;
    let backup = rusqlite::backup::Backup::new(&source, &mut target)
        .map_err(|error| format!("复制操作日志失败：{error}"))?;
    backup
        .run_to_completion(8192, Duration::from_millis(5), None)
        .map_err(|error| format!("复制操作日志失败：{error}"))?;
    drop(backup);
    target
        .execute_batch("PRAGMA journal_mode=DELETE;")
        .map_err(|error| format!("整理日志快照失败：{error}"))?;
    let check: String = target
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|error| format!("校验日志快照失败：{error}"))?;
    if check != "ok" {
        return Err(format!("日志快照校验失败：{check}"));
    }
    Ok(())
}

fn repaired_item_payload(
    db: &Connection,
    payload: &str,
) -> Result<String, (Option<i64>, String)> {
    let mut value: serde_json::Value =
        serde_json::from_str(payload).map_err(|error| (None, format!("记录无法解析：{error}")))?;
    let pk = value
        .get("pk")
        .and_then(|value| value.as_i64())
        .ok_or((None, "记录缺少 image_pk".to_string()))?;
    let image = crate::duplicates::management::main_image(db, pk)
        .map_err(|error| (Some(pk), error))?
        .ok_or((Some(pk), "图库记录已移除".to_string()))?;
    let signature = crate::duplicates::fingerprint::source_signature(Path::new(&image.path))
        .map_err(|error| (Some(pk), format!("源文件不可用：{error}")))?;
    let canonical = canonical_journal_path(Path::new(&image.path))
        .map_err(|error| (Some(pk), error))?;
    value["image"] = serde_json::to_value(&image)
        .map_err(|error| (Some(pk), format!("图片记录无法序列化：{error}")))?;
    value["public_hash"] = serde_json::json!(hex_digest(image.id.as_bytes()));
    value["signature"] = serde_json::json!(hex_encode(&crate::duplicates::cache::signature(
        &image.id,
        &signature
    )));
    value["canonical"] = serde_json::json!(canonical);
    value["identity"] = serde_json::json!(signature.identity);
    serde_json::to_string(&value)
        .map_err(|error| (Some(pk), format!("记录无法序列化：{error}")))
}

fn remap_duplicate_request_ids(
    request: &mut crate::duplicates::DuplicateManagementRequest,
    db: &Connection,
    mappings: &[BackupPathMapping],
    operation: i64,
    issues: &mut Vec<OperationLogIssue>,
) -> Result<(), String> {
    for ids in [
        &mut request.keep_ids,
        &mut request.protected_ids,
        &mut request.image_ids,
    ] {
        for id in ids.iter_mut() {
            let old = id.clone();
            let next = remap_backup_path(&old, mappings);
            let exists: bool = db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM images WHERE id=?1)",
                    [&next],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())?;
            if !exists {
                issues.push(OperationLogIssue {
                    kind: "operationRequest".into(),
                    operation: operation.to_string(),
                    image_pk: None,
                    image_id: Some(old),
                    reason: format!("请求中的图片不存在：{next}"),
                });
            }
            *id = next;
        }
    }
    Ok(())
}

fn migrate_duplicate_plan(
    id: i64,
    raw: &str,
    db: &Connection,
    mappings: &[BackupPathMapping],
) -> Result<(String, Vec<OperationLogIssue>), String> {
    let mut value: serde_json::Value =
        serde_json::from_str(raw).map_err(|error| format!("请求无法解析：{error}"))?;
    let mut issues = Vec::<OperationLogIssue>::new();

    let request_value = value
        .get("request")
        .cloned()
        .ok_or_else(|| "请求缺少 Plan.request".to_string())?;
    let mut request: crate::duplicates::DuplicateManagementRequest =
        serde_json::from_value(request_value)
            .map_err(|error| format!("Plan.request 结构不兼容：{error}"))?;
    remap_duplicate_request_ids(&mut request, db, mappings, id, &mut issues)?;
    value["request"] = serde_json::to_value(&request).map_err(|error| error.to_string())?;

    let keepers = value
        .get_mut("keepers")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| "请求缺少 Plan.keepers".to_string())?;
    for keeper in keepers.iter_mut() {
        let payload = serde_json::to_string(&*keeper).map_err(|error| error.to_string())?;
        match repaired_item_payload(db, &payload) {
            Ok(next) => {
                *keeper =
                    serde_json::from_str(&next).map_err(|error| format!("保留图修复失败：{error}"))?;
            }
            Err((image_pk, reason)) => issues.push(OperationLogIssue {
                kind: "operationKeeper".into(),
                operation: id.to_string(),
                image_pk,
                image_id: keeper
                    .get("image")
                    .and_then(|image| image.get("id"))
                    .and_then(|image| image.as_str())
                    .map(str::to_string),
                reason,
            }),
        }
    }

    if let Some(batches) = value
        .get_mut("batches")
        .and_then(serde_json::Value::as_array_mut)
    {
        for batch in batches.iter_mut() {
            let mut request: crate::duplicates::DuplicateManagementRequest =
                serde_json::from_value(batch.clone())
                    .map_err(|error| format!("Plan.batches 结构不兼容：{error}"))?;
            remap_duplicate_request_ids(&mut request, db, mappings, id, &mut issues)?;
            *batch = serde_json::to_value(&request).map_err(|error| error.to_string())?;
        }
    }

    let next = serde_json::to_string(&value).map_err(|error| error.to_string())?;
    Ok((next, issues))
}

fn mark_duplicate_item_unavailable(
    conn: &Connection,
    operation: i64,
    ordinal: i64,
    reason: &str,
) -> Result<(), String> {
    conn.execute(
        "UPDATE items SET state='skipped',code='migrationUnavailable',message=?3
         WHERE operation=?1 AND ordinal=?2 AND state='pending'",
        params![operation, ordinal, reason.chars().take(400).collect::<String>()],
    )
    .map_err(|error| format!("标记未迁移记录失败：{error}"))?;
    Ok(())
}

fn recycle_signature(path: &Path) -> Result<String, String> {
    let source = crate::duplicates::fingerprint::source_signature(path)?;
    serde_json::to_string(&(source.file_size, source.modified_ns, source.identity))
        .map_err(|error| error.to_string())
}

fn file_digest(path: &Path) -> Result<Vec<u8>, String> {
    let mut file = fs::File::open(path).map_err(|error| format!("打开源文件失败：{error}"))?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 256 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("读取源文件失败：{error}"))?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(hash.finalize().to_vec())
}

fn same_file_content(left: &Path, right: &Path) -> Result<bool, String> {
    Ok(file_digest(left)? == file_digest(right)?)
}

fn same_journal_path(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => {
            if cfg!(windows) {
                left.eq_ignore_ascii_case(right)
            } else {
                false
            }
        }
    }
}

fn mark_recycle_item_unavailable(
    conn: &Connection,
    selection: &str,
    pk: i64,
    reason: &str,
) -> Result<(), String> {
    conn.execute(
        "UPDATE items SET state='skipped',message=?3
         WHERE selection=?1 AND pk=?2 AND state IN ('pending','failed')",
        params![
            selection,
            pk,
            format!("迁移后无法继续：{reason}")
                .chars()
                .take(500)
                .collect::<String>()
        ],
    )
    .map_err(|error| format!("标记未迁移回收项失败：{error}"))?;
    Ok(())
}

fn migrate_recycle_journal(path: &Path, db: &Connection) -> Result<OperationLogMigration, String> {
    let mut conn = Connection::open(path).map_err(|error| format!("打开回收日志失败：{error}"))?;
    let mut report = OperationLogMigration {
        journal: "recycle-management.sqlite".into(),
        status: "migrated".into(),
        ..Default::default()
    };
    let tx = conn
        .transaction()
        .map_err(|error| format!("打开回收日志事务失败：{error}"))?;
    let rows = tx
        .prepare(
            "SELECT selection,pk,id,path,bytes,modified,state,phase,signature FROM items ORDER BY selection,pk",
        )
        .map_err(|error| error.to_string())?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
            ))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    for (
        selection,
        pk,
        old_id,
        old_path,
        _old_bytes,
        _old_modified,
        state,
        phase,
        old_signature,
    ) in rows
    {
        report.rows_total += 1;
        let image = db
            .query_row(
                "SELECT id,path,file_size,modified_at FROM images WHERE image_pk=?1",
                [pk],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let Some((new_id, new_path, size, modified)) = image else {
            report.unresolved.push(OperationLogIssue {
                kind: "recycleItem".into(),
                operation: selection.clone(),
                image_pk: Some(pk),
                image_id: Some(old_id),
                reason: "图库记录已移除".into(),
            });
            tx.execute(
                "UPDATE items SET state='skipped',message='迁移后无法使用：图库记录已移除'
                 WHERE selection=?1 AND pk=?2",
                params![selection, pk],
            )
            .map_err(|error| error.to_string())?;
            continue;
        };
        let action: String = tx
            .query_row(
                "SELECT action FROM selections WHERE id=?1",
                [&selection],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?
            .unwrap_or_default();
        let retryable = ["pending", "failed"].contains(&state.as_str()) && phase != "moved";
        let mut signature = old_signature.clone();
        let mut unresolved_reason = None;

        if action == "systemTrash" && retryable && !old_signature.is_empty() {
            // A destructive retry may only reuse the old authorization. Never
            // mint a new signature from whatever file currently occupies the
            // path: a same-size/same-mtime replacement has a different identity.
            if same_journal_path(&old_path, &new_path) {
                match recycle_signature(Path::new(&new_path)) {
                    Ok(current) if current == old_signature => {}
                    Ok(_) => {
                        unresolved_reason =
                            Some("原路径文件已被替换或身份已变化，旧回收记录不能继续".to_string())
                    }
                    Err(error) => unresolved_reason = Some(format!("源文件不可用：{error}")),
                }
            } else {
                let old_unchanged = recycle_signature(Path::new(&old_path))
                    .map(|current| current == old_signature)
                    .unwrap_or(false);
                if !old_unchanged {
                    unresolved_reason =
                        Some("迁移前的源文件已变化，无法证明仍是原清理对象".to_string());
                } else {
                    match same_file_content(Path::new(&old_path), Path::new(&new_path)) {
                        Ok(true) => match recycle_signature(Path::new(&new_path)) {
                            Ok(current) => signature = current,
                            Err(error) => {
                                unresolved_reason = Some(format!("迁移后源文件不可用：{error}"))
                            }
                        },
                        Ok(false) => {
                            unresolved_reason =
                                Some("迁移后的文件内容与原清理对象不一致".to_string())
                        }
                        Err(error) => unresolved_reason = Some(error),
                    }
                }
            }
        } else if action == "systemTrash" {
            // An unsigned item was never authorized; first deletion will sign
            // it. A moved item has no source path left. In both cases do not
            // create a replacement signature here.
            if old_signature.is_empty() && retryable {
                if let Err(error) = recycle_signature(Path::new(&new_path)) {
                    unresolved_reason = Some(format!("源文件不可用：{error}"));
                }
            }
        } else if retryable {
            // Restore records do not use the deletion signature. Preserve it
            // and only report whether the new source path is reachable.
            if let Err(error) = recycle_signature(Path::new(&new_path)) {
                unresolved_reason = Some(format!("源文件不可用：{error}"));
            }
        }

        tx.execute(
            "UPDATE items SET id=?3,path=?4,bytes=?5,modified=?6,signature=?7
             WHERE selection=?1 AND pk=?2",
            params![selection, pk, new_id, new_path, size, modified, signature],
        )
        .map_err(|error| error.to_string())?;
        if let Some(reason) = unresolved_reason {
            report.unresolved.push(OperationLogIssue {
                kind: "recycleItem".into(),
                operation: selection.clone(),
                image_pk: Some(pk),
                image_id: Some(new_id),
                reason: reason.clone(),
            });
            if ["pending", "failed"].contains(&state.as_str()) {
                mark_recycle_item_unavailable(&tx, &selection, pk, &reason)?;
            }
        } else {
            report.rows_migrated += 1;
        }
    }
    tx.commit().map_err(|error| error.to_string())?;
    if !report.unresolved.is_empty() {
        report.status = "partial".into();
        report.message = format!("{} 条回收记录无法完整迁移", report.unresolved.len());
    }
    Ok(report)
}

fn migrate_duplicate_journal(
    path: &Path,
    db: &Connection,
    mappings: &[BackupPathMapping],
) -> Result<OperationLogMigration, String> {
    let mut conn = Connection::open(path).map_err(|error| format!("打开整理日志失败：{error}"))?;
    let mut report = OperationLogMigration {
        journal: "duplicate-management.sqlite".into(),
        status: "migrated".into(),
        ..Default::default()
    };
    let tx = conn
        .transaction()
        .map_err(|error| format!("打开整理日志事务失败：{error}"))?;

    let items = tx
        .prepare("SELECT operation,ordinal,image_pk,payload,state FROM items ORDER BY operation,ordinal")
        .map_err(|error| error.to_string())?
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    for (operation, ordinal, pk, payload, state) in items {
        report.rows_total += 1;
        match repaired_item_payload(db, &payload) {
            Ok(next) => {
                tx.execute(
                    "UPDATE items SET payload=?3 WHERE operation=?1 AND ordinal=?2",
                    params![operation, ordinal, next],
                )
                .map_err(|error| error.to_string())?;
                report.rows_migrated += 1;
            }
            Err((_, reason)) => {
                report.unresolved.push(OperationLogIssue {
                    kind: "managementItem".into(),
                    operation: operation.to_string(),
                    image_pk: Some(pk),
                    image_id: None,
                    reason: reason.clone(),
                });
                if state == "pending" {
                    mark_duplicate_item_unavailable(&tx, operation, ordinal, &reason)?;
                }
            }
        }
    }

    for table in ["bulk_candidates", "bulk_keepers"] {
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                [table],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if !exists {
            continue;
        }
        let key = if table == "bulk_candidates" {
            "operation,image_pk,payload FROM bulk_candidates"
        } else {
            "operation,image_pk,payload FROM bulk_keepers"
        };
        let rows = tx
            .prepare(&format!("SELECT {key}"))
            .map_err(|error| error.to_string())?
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        for (operation, pk, payload) in rows {
            report.rows_total += 1;
            match repaired_item_payload(db, &payload) {
                Ok(next) => {
                    tx.execute(
                        &format!(
                            "UPDATE {table} SET payload=?3 WHERE operation=?1 AND image_pk=?2"
                        ),
                        params![operation, pk, next],
                    )
                    .map_err(|error| error.to_string())?;
                    report.rows_migrated += 1;
                }
                Err((_, reason)) => {
                    report.unresolved.push(OperationLogIssue {
                        kind: table.into(),
                        operation: operation.to_string(),
                        image_pk: Some(pk),
                        image_id: None,
                        reason: reason.clone(),
                    });
                    if table == "bulk_candidates" {
                        tx.execute(
                            "UPDATE bulk_candidates SET state=2 WHERE operation=?1 AND image_pk=?2",
                            params![operation, pk],
                        )
                        .map_err(|error| error.to_string())?;
                    }
                }
            }
        }
    }

    let operations = tx
        .prepare("SELECT id,request FROM operations ORDER BY id")
        .map_err(|error| error.to_string())?
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    for (id, request) in operations {
        report.rows_total += 1;
        match migrate_duplicate_plan(id, &request, db, mappings) {
            Ok((next, issues)) => {
                tx.execute(
                    "UPDATE operations SET request=?2 WHERE id=?1",
                    params![id, next],
                )
                .map_err(|error| error.to_string())?;
                if issues.is_empty() {
                    report.rows_migrated += 1;
                } else {
                    // Keep the operation row but make it explicit that this plan
                    // is not fully migrated. Continuing it will fail at the
                    // keeper/identity check instead of silently using old data.
                    let reasons: Vec<String> = issues
                        .iter()
                        .map(|issue| {
                            format!(
                                "{}: {}",
                                issue
                                    .image_id
                                    .as_deref()
                                    .unwrap_or("keeper"),
                                issue.reason
                            )
                        })
                        .collect();
                    let existing: String = tx
                        .query_row("SELECT errors FROM operations WHERE id=?1", [id], |row| {
                            row.get(0)
                        })
                        .map_err(|error| error.to_string())?;
                    let mut errors: Vec<String> =
                        serde_json::from_str(&existing).unwrap_or_default();
                    errors.extend(reasons);
                    tx.execute(
                        "UPDATE operations SET errors=?2 WHERE id=?1",
                        params![id, serde_json::to_string(&errors).unwrap_or_default()],
                    )
                    .map_err(|error| error.to_string())?;
                    report.unresolved.extend(issues);
                }
            }
            Err(reason) => report.unresolved.push(OperationLogIssue {
                kind: "operationRequest".into(),
                operation: id.to_string(),
                image_pk: None,
                image_id: None,
                reason,
            }),
        }
    }

    let owner_rows = tx
        .prepare("SELECT image_pk FROM owners")
        .map_err(|error| error.to_string())?
        .query_map([], |row| row.get::<_, i64>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    for pk in owner_rows {
        report.rows_total += 1;
        let image_id = db
            .query_row("SELECT id FROM images WHERE image_pk=?1", [pk], |row| {
                row.get::<_, String>(0)
            })
            .optional()
            .map_err(|error| error.to_string())?;
        match image_id {
            Some(image_id) => {
                tx.execute(
                    "UPDATE owners SET public_hash=?2 WHERE image_pk=?1",
                    params![pk, hex_digest(image_id.as_bytes())],
                )
                .map_err(|error| error.to_string())?;
                report.rows_migrated += 1;
            }
            None => report.unresolved.push(OperationLogIssue {
                kind: "owner".into(),
                operation: String::new(),
                image_pk: Some(pk),
                image_id: None,
                reason: "图库记录已移除".into(),
            }),
        }
    }

    tx.commit().map_err(|error| error.to_string())?;
    if !report.unresolved.is_empty() {
        report.status = "partial".into();
        report.message = format!("{} 条整理记录无法完整迁移", report.unresolved.len());
    }
    Ok(report)
}

fn rebind_operation_journals(data_dir: &Path, binding: &str) -> Result<Vec<String>, String> {
    let mut warned = Vec::new();
    for (name, tables) in [
        (
            "recycle-management.sqlite",
            vec!["meta", "items", "selections"],
        ),
        (
            "duplicate-management.sqlite",
            vec!["meta", "items", "operations", "owners"],
        ),
    ] {
        let path = data_dir.join("operations").join(name);
        if !path.is_file() {
            continue;
        }
        let conn = Connection::open(&path).map_err(|error| format!("打开迁移日志失败：{error}"))?;
        if let Err(error) = journal_is_supported(&conn, &tables) {
            warned.push(format!("{name}: {error}"));
            continue;
        }
        conn.execute("UPDATE meta SET value=?1 WHERE key='binding'", [binding])
            .map_err(|error| format!("重绑迁移日志失败：{error}"))?;
        let check: String = conn
            .query_row("PRAGMA quick_check", [], |row| row.get(0))
            .map_err(|error| format!("校验迁移日志失败：{error}"))?;
        if check != "ok" {
            warned.push(format!("{name}: 校验失败：{check}"));
        }
    }
    Ok(warned)
}

pub fn prepare_data_directory_migration(
    target: &Path,
    state: &AppState,
) -> Result<(u64, u64, Vec<OperationLogMigration>), String> {
    let mut guard = begin_maintenance(state)?;
    let reference_target = target.to_string_lossy().to_string();
    if !target.is_absolute() {
        return Err("目标目录必须是绝对路径".into());
    }
    fs::create_dir_all(target).map_err(|e| e.to_string())?;
    let source = state.data_dir.canonicalize().map_err(|e| e.to_string())?;
    let target = target.canonicalize().map_err(|e| e.to_string())?;
    if target.starts_with(&source) || source.starts_with(&target) {
        return Err("源目录和目标目录不得重合或相互包含".into());
    }
    if fs::read_dir(&target)
        .map_err(|e| e.to_string())?
        .next()
        .is_some()
    {
        return Err("请选择空目录，避免覆盖已有数据".into());
    }
    let mappings = vec![
        BackupPathMapping {
            from: state.data_dir.to_string_lossy().into(),
            to: reference_target.clone(),
        },
        BackupPathMapping {
            from: source.to_string_lossy().into(),
            to: reference_target.clone(),
        },
    ];
    let source_binding = crate::duplicates::cache::db_binding(state)?;
    let stage = target.join("migration-staging");
    fs::create_dir(&stage).map_err(|e| e.to_string())?;
    sqlite_snapshot(&state.database_path, &stage.join("illutag.sqlite"))?;
    for name in ["clipboard", "managed", "thumbs"] {
        copy_tree(&source.join(name), &stage.join(name))?;
    }
    for name in [
        "window-state.json",
        "illutag.window-state.json",
        "preferences.json",
        "tagger_config.json",
        "tag_groups.json",
    ] {
        let file = source.join(name);
        if file.is_file() {
            fs::copy(file, stage.join(name)).map_err(|e| e.to_string())?;
        }
    }

    // Operation journals are copied before the main database is switched. The
    // record-level repair happens after path remapping, because an image inside
    // the data directory changes both its public id and its file identity.
    let operations_source = source.join("operations");
    let mut copied_journals = Vec::<&'static str>::new();
    let mut reports = Vec::<OperationLogMigration>::new();
    if operations_source.is_dir() {
        fs::create_dir_all(stage.join("operations")).map_err(|e| e.to_string())?;
        for (name, tables) in [
            (
                "recycle-management.sqlite",
                vec!["meta", "items", "selections"],
            ),
            (
                "duplicate-management.sqlite",
                vec!["meta", "items", "operations", "owners"],
            ),
        ] {
            let from = operations_source.join(name);
            if !from.is_file() {
                continue;
            }
            let source_journal = match Connection::open_with_flags(
                &from,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            ) {
                Ok(conn) => conn,
                Err(error) => {
                    reports.push(OperationLogMigration {
                        journal: name.into(),
                        status: "skipped".into(),
                        message: format!("无法打开，未迁移：{error}"),
                        ..Default::default()
                    });
                    continue;
                }
            };
            if let Err(error) = journal_is_supported(&source_journal, &tables) {
                reports.push(OperationLogMigration {
                    journal: name.into(),
                    status: "skipped".into(),
                    message: format!("{error}，未迁移"),
                    ..Default::default()
                });
                continue;
            }
            if journal_binding(&source_journal).as_deref() != Some(source_binding.as_str()) {
                reports.push(OperationLogMigration {
                    journal: name.into(),
                    status: "skipped".into(),
                    message: "日志不属于当前图库（binding 不一致），未迁移".into(),
                    ..Default::default()
                });
                continue;
            }
            drop(source_journal);
            let to = stage.join("operations").join(name);
            if let Err(error) = snapshot_operation_journal(&from, &to) {
                reports.push(OperationLogMigration {
                    journal: name.into(),
                    status: "skipped".into(),
                    message: format!("{error}，未迁移"),
                    ..Default::default()
                });
                continue;
            }
            copied_journals.push(name);
        }
    }

    remap_database_paths(&stage.join("illutag.sqlite"), &mappings)?;

    let mut files = 0;
    let mut bytes = 0;
    for item in WalkDir::new(&stage) {
        let item = item.map_err(|e| e.to_string())?;
        if item.file_type().is_file() {
            files += 1;
            bytes += item.metadata().map_err(|e| e.to_string())?.len();
        }
    }
    for item in fs::read_dir(&stage).map_err(|e| e.to_string())? {
        let item = item.map_err(|e| e.to_string())?;
        fs::rename(item.path(), target.join(item.file_name())).map_err(|e| e.to_string())?;
    }
    fs::remove_dir(stage).map_err(|e| e.to_string())?;

    // Record repair runs after the switch so file identity/signature values in
    // the journals match the files at their final paths.
    let migrated_db = Connection::open_with_flags(
        &target.join("illutag.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|error| format!("打开迁移后主库失败：{error}"))?;
    for name in &copied_journals {
        let path = target.join("operations").join(name);
        let result = if *name == "recycle-management.sqlite" {
            migrate_recycle_journal(&path, &migrated_db)
        } else {
            migrate_duplicate_journal(&path, &migrated_db, &mappings)
        };
        match result {
            Ok(report) => reports.push(report),
            Err(error) => reports.push(OperationLogMigration {
                journal: (*name).into(),
                status: "skipped".into(),
                message: format!("记录修复失败，未迁移：{error}"),
                ..Default::default()
            }),
        }
    }
    drop(migrated_db);

    // Rebind the copied journals to the final database identity only after the
    // files have reached their final path.
    let target_binding = crate::duplicates::cache::db_binding_for(&target.join("illutag.sqlite"))?;
    let warnings = rebind_operation_journals(&target, &target_binding)?;
    if !warnings.is_empty() {
        reports.push(OperationLogMigration {
            journal: "operations".into(),
            status: "partial".into(),
            unresolved: warnings
                .into_iter()
                .map(|reason| OperationLogIssue {
                    kind: "binding".into(),
                    operation: String::new(),
                    image_pk: None,
                    image_id: None,
                    reason,
                })
                .collect(),
            message: "部分日志未能重绑，需要重新清理旧撤销记录".into(),
            ..Default::default()
        });
    }

    guard.keep = true;
    crate::duplicates::invalidate(state);
    Ok((files, bytes, reports))
}

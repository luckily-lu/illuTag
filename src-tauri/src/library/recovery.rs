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
        .any(|j| ["queued", "running", "paused", "cancelling"].contains(&j.state.as_str()))
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
    let names=conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").map_err(|e|e.to_string())?
        .query_map([],|r|r.get::<_,String>(0)).map_err(|e|e.to_string())?.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())?;
    names
        .into_iter()
        .map(|name| {
            let count = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM \"{}\"", name.replace('"', "\"\"")),
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
    conn.execute_batch("PRAGMA foreign_keys=OFF;")
        .map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    for (table, column) in [
        ("folders", "path"),
        ("user_folders", "source_path"),
        ("image_thumbnails", "thumb_path"),
    ] {
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
    let rows = tx
        .prepare("SELECT id,path FROM images")
        .map_err(|e| e.to_string())?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    for (id, path) in rows {
        let next_id = remap_backup_path(&id, mappings);
        let next_path = remap_backup_path(&path, mappings);
        if next_id != id {
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
        let target = staged.join(&file.path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::copy(package.join(&file.path), target).map_err(|e| e.to_string())?;
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
    Ok(manifest)
}

pub fn prepare_data_directory_migration(
    target: &Path,
    state: &AppState,
) -> Result<(u64, u64), String> {
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
    remap_database_paths(
        &stage.join("illutag.sqlite"),
        &[
            BackupPathMapping {
                from: state.data_dir.to_string_lossy().into(),
                to: reference_target.clone(),
            },
            BackupPathMapping {
                from: source.to_string_lossy().into(),
                to: reference_target,
            },
        ],
    )?;
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
    guard.keep = true;
    Ok((files, bytes))
}

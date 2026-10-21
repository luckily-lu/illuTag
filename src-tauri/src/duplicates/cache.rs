//! A bounded, disposable SQLite cache. All library connections are read-only.
use super::{fingerprint, index, *};
use crate::library::{AppState, GalleryImage};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const CACHE_VERSION: i64 = 1;
const ALGORITHM: &str = fingerprint::ALGORITHM_VERSION;
const MIN_FREE_BYTES: u64 = super::cache_capacity::MIN_FREE_BYTES;
const MAX_MEMORY_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ERRORS: usize = 24;
const ACTIVE: &str = "COALESCE(i.source,'') <> 'reference' AND COALESCE(i.trashed,0)=0";
const SCHEMA: &str = r#"
PRAGMA user_version=1;
CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE fingerprints(
 image_pk INTEGER PRIMARY KEY,public_hash BLOB NOT NULL,signature BLOB NOT NULL,
 size INTEGER NOT NULL,mtime INTEGER NOT NULL,identity BLOB NOT NULL,
 sha BLOB,phash BLOB,dhash BLOB,color BLOB,width INTEGER NOT NULL,height INTEGER NOT NULL,
 low_info INTEGER NOT NULL,skip TEXT,image_order INTEGER NOT NULL,seen_run INTEGER NOT NULL
);
CREATE INDEX fingerprints_seen ON fingerprints(seen_run,image_pk);
CREATE TABLE generations(
 generation INTEGER PRIMARY KEY,sensitivity TEXT NOT NULL,checked_at INTEGER NOT NULL,
 state TEXT NOT NULL,coverage TEXT NOT NULL,errors TEXT NOT NULL,
 exact_groups INTEGER NOT NULL DEFAULT 0,near_groups INTEGER NOT NULL DEFAULT 0,
 unique_images INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE groups(
 generation INTEGER NOT NULL,group_no INTEGER NOT NULL,kind INTEGER NOT NULL,
 reference_pk INTEGER NOT NULL,total_members INTEGER NOT NULL,
 PRIMARY KEY(generation,group_no)
) WITHOUT ROWID;
CREATE INDEX groups_kind ON groups(generation,kind,group_no);
CREATE TABLE members(
 generation INTEGER NOT NULL,group_no INTEGER NOT NULL,member_no INTEGER NOT NULL,
 image_pk INTEGER NOT NULL,public_hash BLOB NOT NULL,signature BLOB NOT NULL,phash_distance INTEGER,
 dhash_distance INTEGER,color_distance REAL,
 PRIMARY KEY(generation,group_no,member_no)
) WITHOUT ROWID;
CREATE INDEX members_image ON members(generation,image_pk);
"#;

#[derive(Default)]
struct Runtime {
    active: AtomicBool,
    invalidated: AtomicBool,
    revision: AtomicU64,
    membership_revision: AtomicU64,
    live: Mutex<Option<Arc<LiveView>>>,
    disk_allowance: AtomicU64,
}
#[derive(Clone)]
struct LiveGroup {
    id: i64,
    source_group: i64,
    kind: i64,
    reference: i64,
    total: i64,
    recalculate_metrics: bool,
    // Most groups retain their published member order without another copy.
    // Only a near star whose original content unit disappeared needs a subset.
    members: Option<Vec<(i64, i64)>>,
}
struct LiveView {
    generation: i64,
    revision: u64,
    groups: BTreeMap<i64, LiveGroup>,
    exact: i64,
    near: i64,
    unique: i64,
}
fn runtime(state: &AppState) -> Arc<Runtime> {
    runtime_for_path(cache_path(state))
}
fn runtime_for_path(path: PathBuf) -> Arc<Runtime> {
    static RUNTIMES: OnceLock<Mutex<HashMap<PathBuf, Arc<Runtime>>>> = OnceLock::new();
    let mut all = RUNTIMES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    all.entry(path).or_default().clone()
}
struct RunGuard(Arc<Runtime>);
impl Drop for RunGuard {
    fn drop(&mut self) {
        self.0.active.store(false, Ordering::SeqCst);
    }
}
pub(super) fn cache_path(state: &AppState) -> PathBuf {
    state.data_dir.join("cache").join("duplicates.sqlite")
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
fn hash(value: &[u8]) -> [u8; 32] {
    Sha256::digest(value).into()
}
pub(crate) fn signature(public_id: &str, file: &fingerprint::SourceSignature) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(hash(public_id.as_bytes()));
    digest.update(hash(file.identity.as_bytes()));
    digest.update(file.file_size.to_le_bytes());
    digest.update(file.modified_ns.to_le_bytes());
    digest.finalize().into()
}
pub(crate) fn db_binding_for(database_path: &Path) -> Result<String, String> {
    let info = fingerprint::source_signature(database_path)?;
    let canonical = fs::canonicalize(database_path).map_err(|e| e.to_string())?;
    Ok(format!("{}|{}", canonical.to_string_lossy(), info.identity))
}
pub(crate) fn db_binding(state: &AppState) -> Result<String, String> {
    db_binding_for(&state.database_path)
}
pub(crate) fn library(state: &AppState) -> Result<Connection, String> {
    let conn = Connection::open_with_flags(&state.database_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("无法读取图库：{e}"))?;
    conn.busy_timeout(Duration::from_secs(2))
        .map_err(|e| e.to_string())?;
    conn.execute_batch("PRAGMA query_only=ON; PRAGMA cache_size=-4096; PRAGMA mmap_size=0;")
        .map_err(|e| e.to_string())?;
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if version != 3 {
        return Err("重复检索需要 schema 3 图库".into());
    }
    Ok(conn)
}
fn configure(conn: &Connection, writable: bool, path: &Path) -> Result<(), String> {
    conn.busy_timeout(Duration::from_secs(2))
        .map_err(|e| e.to_string())?;
    conn.execute_batch("PRAGMA cache_size=-8192; PRAGMA mmap_size=0; PRAGMA temp_store=MEMORY;")
        .map_err(|e| e.to_string())?;
    if writable {
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=256; PRAGMA journal_size_limit=4194304;")
            .map_err(|e| format!("无法配置查重缓存：{e}"))?;
        let images: u64 = conn
            .query_row("SELECT COUNT(*) FROM fingerprints", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        configure_capacity(conn, path, images)?;
    } else {
        conn.execute_batch("PRAGMA query_only=ON;")
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn meta(conn: &Connection, key: &str) -> Result<Option<String>, String> {
    conn.query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())
}
fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<(), String> {
    conn.execute("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key,value])
        .map_err(|e| format!("写入查重缓存失败：{e}"))?;
    Ok(())
}
fn validate(conn: &Connection, state: &AppState) -> Result<(), String> {
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if version != CACHE_VERSION || meta(conn, "algorithm")?.as_deref() != Some(ALGORITHM) {
        return Err("查重缓存版本不兼容，请清理缓存后手动重新检索".into());
    }
    if meta(conn, "database")?.as_deref() != Some(db_binding(state)?.as_str()) {
        return Err("图库已恢复、迁移或替换，请清理查重缓存后手动重新检索".into());
    }
    Ok(())
}
pub(crate) fn open_existing(state: &AppState, writable: bool) -> Result<Connection, String> {
    let flags = if writable {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    } else {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    };
    let conn = Connection::open_with_flags(cache_path(state), flags)
        .map_err(|e| format!("查重缓存无法打开，请清理缓存后重新检索：{e}"))?;
    validate(&conn, state)?;
    configure(&conn, writable, &cache_path(state))?;
    Ok(conn)
}

/// Retry only an atomic statement (or BEGIN IMMEDIATE) that SQLite has not
/// committed. Never retain a write transaction while waiting or pausing.
pub(super) fn controlled_write<T>(
    conn: &Connection,
    control: &mut dyn FnMut() -> Result<(), String>,
    mut write: impl FnMut() -> rusqlite::Result<T>,
) -> Result<T, String> {
    conn.busy_timeout(Duration::from_millis(50))
        .map_err(|e| e.to_string())?;
    let result = (|| {
        let mut waited = Duration::ZERO;
        loop {
            // Pause time is not charged to the contention budget. No statement
            // or writer is held while this hook waits for resume/cancel.
            control()?;
            let started = Instant::now();
            match write() {
                Ok(value) => return Ok(value),
                Err(rusqlite::Error::SqliteFailure(error, message))
                    if matches!(
                        error.code,
                        rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                    ) =>
                {
                    waited += started.elapsed();
                    if !conn.is_autocommit() || waited >= Duration::from_secs(30) {
                        return Err(format!(
                            "查重缓存持续被其他写入占用，已保留已有结果，请稍后重试。技术信息：{}",
                            message.unwrap_or_else(|| error.to_string())
                        ));
                    }
                    thread::sleep(Duration::from_millis(20));
                    waited += Duration::from_millis(20);
                }
                Err(error) => return Err(write_error(conn, &error)),
            }
        }
    })();
    let _ = conn.busy_timeout(Duration::from_secs(2));
    result
}
fn create_or_open(state: &AppState) -> Result<Connection, String> {
    if cache_path(state).exists() {
        return open_existing(state, true);
    }
    let dir = cache_path(state).parent().unwrap().to_path_buf();
    fs::create_dir_all(&dir).map_err(|e| format!("无法创建查重缓存目录：{e}"))?;
    check_disk(&cache_path(state))?;
    let conn = Connection::open(cache_path(state)).map_err(|e| e.to_string())?;
    conn.execute_batch("PRAGMA page_size=4096;")
        .map_err(|e| e.to_string())?;
    conn.execute_batch(SCHEMA)
        .map_err(|e| format!("创建查重缓存失败：{e}"))?;
    configure(&conn, true, &cache_path(state))?;
    set_meta(&conn, "algorithm", ALGORITHM)?;
    set_meta(&conn, "database", &db_binding(state)?)?;
    set_meta(&conn, "dirty", "1")?;
    Ok(conn)
}

/// No cache, image decode or directory creation happens on application startup.
pub fn status(state: &AppState) -> Result<DuplicateStatus, String> {
    status_inner(state, true)
}

/// Read the published generation and dirty/attempt headers only. Starting work
/// or polling workflow status must not build the active-membership view.
pub fn published_status(state: &AppState) -> Result<DuplicateStatus, String> {
    status_inner(state, false)
}

fn status_inner(state: &AppState, active_members: bool) -> Result<DuplicateStatus, String> {
    let path = cache_path(state);
    let running = runtime(state).active.load(Ordering::SeqCst);
    let mut result = DuplicateStatus {
        generation: None,
        sensitivity: "standard".into(),
        verification_mode: "candidate".into(),
        checked_at: None,
        duration_ms: None,
        state: if running { "running" } else { "empty" }.into(),
        needs_update: true,
        coverage: DuplicateCoverage::default(),
        cache_bytes: cache_bytes(&path)? as i64,
        exact_groups: 0,
        near_groups: 0,
        unique_images: 0,
        errors: vec![],
        last_attempt: None,
    };
    if !path.exists() {
        return Ok(result);
    }
    let conn = match open_existing(state, false) {
        Ok(conn) => conn,
        Err(error) => {
            result.state = "invalid".into();
            result.errors.push(error);
            return Ok(result);
        }
    };
    if active_members {
        attach_live(&conn, state)?;
    }
    // A generation and its metadata must be read from one SQLite snapshot.
    conn.execute_batch("BEGIN DEFERRED;")
        .map_err(|e| e.to_string())?;
    let read = (|| -> Result<(), String> {
        if let Some(attempt) = meta(&conn, "attempt")? {
            let mut attempt: DuplicateAttempt =
                serde_json::from_str(&attempt).map_err(|e| e.to_string())?;
            if attempt.state == "running" && !running {
                attempt.state = "interrupted".into();
            }
            result.last_attempt = Some(attempt);
        }
        let generation = meta(&conn, "published")?;
        if let Some(id) = generation {
            let row = conn.query_row("SELECT sensitivity,checked_at,state,coverage,errors,exact_groups,near_groups,unique_images FROM generations WHERE generation=?1", [&id], |r| {
                Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,i64>(5)?,r.get::<_,i64>(6)?,r.get::<_,i64>(7)?))
            }).map_err(|e| e.to_string())?;
            result.verification_mode =
                meta(&conn, &format!("verification:{id}"))?.unwrap_or_else(|| "candidate".into());
            result.generation = Some(id);
            result.sensitivity = row.0;
            result.checked_at = Some(row.1);
            // Generation IDs encode the millisecond start time plus a 10-bit
            // in-process sequence, keeping quick repeated runs distinct.
            result.duration_ms = result
                .generation
                .as_ref()
                .and_then(|id| id.parse::<i64>().ok())
                .map(|id| row.1.saturating_sub(id / 1024).max(0));
            result.state = row.2;
            result.coverage = serde_json::from_str(&row.3).map_err(|e| e.to_string())?;
            result.errors = serde_json::from_str(&row.4).map_err(|e| e.to_string())?;
            result.exact_groups = row.5;
            result.near_groups = row.6;
            result.unique_images = row.7;
            if active_members {
                let view = live_view(
                    &conn,
                    result
                        .generation
                        .as_ref()
                        .unwrap()
                        .parse()
                        .map_err(|_| "重复检索代次无效")?,
                    state,
                )?;
                result.exact_groups = view.exact;
                result.near_groups = view.near;
                result.unique_images = view.unique;
            }
        }
        let verification_stale = result.verification_mode == "verified"
            && result.generation.as_ref().is_some_and(|id| {
                meta(&conn, &format!("verifier:{id}"))
                    .ok()
                    .flatten()
                    .as_deref()
                    != Some(super::verification::ALGORITHM_VERSION)
            });
        let dirty = verification_stale
            || runtime(state).invalidated.load(Ordering::SeqCst)
            || meta(&conn, "dirty")?.as_deref() != Some("0");
        result.needs_update = dirty || result.state != "ready";
        if dirty && result.state == "ready" {
            result.state = "stale".into();
        }
        if running {
            result.state = "running".into();
        }
        Ok(())
    })();
    if let Err(error) = read {
        result.state = "invalid".into();
        result
            .errors
            .push(format!("查重缓存损坏，请清理后重新检索：{error}"));
    }
    Ok(result)
}

/// Changes to source membership invalidate results without doing background work.
pub fn invalidate(state: &AppState) {
    let rt = runtime(state);
    rt.revision.fetch_add(1, Ordering::SeqCst);
    rt.membership_revision.fetch_add(1, Ordering::SeqCst);
    rt.invalidated.store(true, Ordering::SeqCst);
    if !cache_path(state).exists() {
        return;
    }
    if let Ok(conn) = open_existing(state, true) {
        let _ = set_meta(&conn, "dirty", "1");
    }
}

/// Scan/file-mutation adapters already know the DB path and can invalidate only
/// after a real membership/source change, without hydrating AppState or a cache.
pub fn invalidate_database(database_path: &Path) {
    let Some(dir) = database_path.parent() else {
        return;
    };
    let path = dir.join("cache/duplicates.sqlite");
    let rt = runtime_for_path(path.clone());
    rt.revision.fetch_add(1, Ordering::SeqCst);
    rt.membership_revision.fetch_add(1, Ordering::SeqCst);
    rt.invalidated.store(true, Ordering::SeqCst);
    if !path.exists() {
        return;
    }
    if let Ok(conn) = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE) {
        let _ = conn.busy_timeout(Duration::from_millis(250));
        let version = conn
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .ok();
        if version == Some(CACHE_VERSION)
            && meta(&conn, "algorithm").ok().flatten().as_deref() == Some(ALGORITHM)
        {
            let _ = set_meta(&conn, "dirty", "1");
        }
    }
}

pub fn clear_cache(state: &AppState) -> Result<(), String> {
    if state.read_only {
        return Err("只读图库不能清理缓存".into());
    }
    let _write = state.write_lock.lock().map_err(|e| e.to_string())?;
    let rt = runtime(state);
    if crate::duplicates::management::is_running(state) {
        return Err("整理任务运行中，不能清理指纹缓存".into());
    }
    if rt.active.load(Ordering::SeqCst)
        || state.jobs.list()?.iter().any(|job| {
            ["duplicates", "duplicateManagement", "duplicateUndo"]
                .contains(&job.request.kind.as_str())
                && ["queued", "running", "pausing", "paused", "cancelling"]
                    .contains(&job.state.as_str())
        })
    {
        return Err("查重任务正在运行或排队，请取消后再清理缓存".into());
    }
    // Only these derived-cache files can be removed. Never recurse into data_dir.
    let path = cache_path(state);
    for suffix in ["-wal", "-shm", "-journal", ""] {
        let item = PathBuf::from(format!("{}{suffix}", path.to_string_lossy()));
        if item.exists() {
            let info = fs::symlink_metadata(&item).map_err(|e| e.to_string())?;
            if !info.file_type().is_file() || info.file_type().is_symlink() {
                return Err("查重缓存路径不是普通文件，未清理".into());
            }
            fs::remove_file(&item).map_err(|e| format!("无法清理查重缓存：{e}"))?;
        }
    }
    rt.invalidated.store(false, Ordering::SeqCst);
    rt.revision.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

fn cache_bytes(path: &Path) -> Result<u64, String> {
    let mut size = 0u64;
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let item = PathBuf::from(format!("{}{suffix}", path.to_string_lossy()));
        match fs::symlink_metadata(item) {
            Ok(info) => {
                if !info.file_type().is_file() || info.file_type().is_symlink() {
                    return Err("查重缓存路径不是普通文件".into());
                }
                size = size.saturating_add(info.len());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("无法检查查重缓存大小：{error}")),
        }
    }
    Ok(size)
}
fn budget(cache_size: u64, free: u64, memory: u64, limit: u64) -> Result<(), String> {
    if free < MIN_FREE_BYTES {
        return Err("磁盘剩余空间不足 2 GiB，查重已停止，保留已计算指纹".into());
    }
    if cache_size > super::cache_capacity::soft_limit(limit) {
        return Err(format!("查重缓存接近容量预算（有效占用 {} MiB，预算 {} MiB），已保留上次完整结果和已提交检查记录。",cache_size.div_ceil(1024*1024),limit/(1024*1024)));
    }
    if memory > MAX_MEMORY_BYTES {
        return Err("查重索引预计超过 512 MiB 内存预算，结果仅部分完成".into());
    }
    Ok(())
}
pub(super) fn check_disk(path: &Path) -> Result<(), String> {
    let dir = path.parent().ok_or("查重缓存路径无效")?;
    budget(
        effective_cache_bytes(path)?,
        free_bytes(dir)?,
        0,
        disk_allowance(path),
    )
}
/// Use the active writer's own page view before publication. A separate reader
/// would still see the pre-transaction freelist and miss consumed reusable pages.
pub(super) fn check_disk_on(conn: &Connection, path: &Path) -> Result<(), String> {
    let dir = path.parent().ok_or("查重缓存路径无效")?;
    budget(
        effective_cache_bytes_on(conn, path)?,
        free_bytes(dir)?,
        0,
        disk_allowance(path),
    )
}
fn effective_cache_bytes_on(conn: &Connection, path: &Path) -> Result<u64, String> {
    let (pages,free,page_size):(u64,u64,u64)=conn.query_row("SELECT page_count,freelist_count,page_size FROM pragma_page_count(),pragma_freelist_count(),pragma_page_size()",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|e|format!("无法检查当前查重事务占用：{e}"))?;
    let db_allocated = fs::metadata(path).map_err(|e| e.to_string())?.len();
    let sidecars = cache_bytes(path)?.saturating_sub(db_allocated);
    Ok(pages
        .saturating_sub(free)
        .saturating_mul(page_size)
        .saturating_add(sidecars))
}
// DELETE releases reusable SQLite pages without shrinking the allocated file.
// Account for that reuse instead of demanding VACUUM or discarding fingerprints.
fn effective_cache_bytes(path: &Path) -> Result<u64, String> {
    let allocated = cache_bytes(path)?;
    if !path.exists() {
        return Ok(allocated);
    }
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("无法检查查重缓存占用：{e}"))?;
    c.busy_timeout(Duration::from_millis(100))
        .map_err(|e| e.to_string())?;
    c.execute_batch("PRAGMA query_only=ON; PRAGMA mmap_size=0; PRAGMA cache_size=-256;")
        .map_err(|e| e.to_string())?;
    let reusable: u64 = c
        .query_row(
            "SELECT freelist_count*page_size FROM pragma_freelist_count(),pragma_page_size()",
            [],
            |r| r.get(0),
        )
        .map_err(|e| format!("无法检查查重缓存可复用页：{e}"))?;
    Ok(allocated.saturating_sub(reusable))
}
fn disk_allowance(path: &Path) -> u64 {
    let configured = runtime_for_path(path.to_path_buf())
        .disk_allowance
        .load(Ordering::SeqCst);
    if configured == 0 {
        super::cache_capacity::BASE_BYTES
    } else {
        configured
    }
}

fn configure_capacity(conn: &Connection, path: &Path, images: u64) -> Result<(), String> {
    let dir = path.parent().ok_or("查重缓存路径无效")?;
    let total = super::cache_capacity::configure(conn, images, free_bytes(dir)?)?;
    runtime_for_path(path.to_path_buf())
        .disk_allowance
        .store(total, Ordering::SeqCst);
    Ok(())
}

fn write_error(conn: &Connection, error: &rusqlite::Error) -> String {
    write_error_with_limit(conn, error, None)
}
fn write_error_with_limit(
    conn: &Connection,
    error: &rusqlite::Error,
    limit: Option<u64>,
) -> String {
    if error.sqlite_error_code() != Some(rusqlite::ErrorCode::DiskFull) {
        return error.to_string();
    }
    if let Some(dir) = conn.path().map(Path::new).and_then(Path::parent) {
        if let Ok(free) = free_bytes(dir) {
            if free < MIN_FREE_BYTES {
                return format!("查重缓存所在磁盘可用空间不足（剩余 {} MiB），已提交进度保留。技术信息：{error}",free/(1024*1024));
            }
        }
    }
    let pages = conn
        .query_row("PRAGMA page_count", [], |r| r.get::<_, u64>(0))
        .unwrap_or(0);
    let page_size = conn
        .query_row("PRAGMA page_size", [], |r| r.get::<_, u64>(0))
        .unwrap_or(4096);
    let maximum = limit
        .map(|total| super::cache_capacity::max_pages(total, page_size, pages))
        .unwrap_or_else(|| {
            conn.query_row("PRAGMA max_page_count", [], |r| r.get::<_, u64>(0))
                .unwrap_or(u64::MAX)
        });
    if maximum.saturating_sub(pages).saturating_mul(page_size) <= 32 * 1024 * 1024 {
        return format!("查重数据库达到缓存容量上限（约 {} MiB），已保留上次完整结果和已提交检查记录。技术信息：{error}",maximum.saturating_mul(page_size).div_ceil(1024*1024));
    }
    "查重数据库写入空间不足，已保留上次完整结果和已提交检查记录；请检查数据目录及临时目录的可用空间。技术信息：database or disk is full".into()
}

pub(super) fn explain_storage_error(error: String, state: &AppState, stage: &str) -> String {
    if error.contains("查重缓存接近容量") || error.contains("磁盘剩余空间不足") {
        return format!("{stage}：{error}");
    }
    if !error.contains("database or disk is full") {
        return error;
    }
    if error.contains("查重数据库") {
        return format!("{stage}：{error}");
    }
    match open_existing(state, false) {
        Ok(c) => {
            let configured = runtime(state).disk_allowance.load(Ordering::SeqCst);
            format!(
                "{stage}：{}",
                write_error_with_limit(
                    &c,
                    &rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL),
                        Some(error)
                    ),
                    (configured > 0).then_some(configured)
                )
            )
        }
        Err(_) => format!("{stage}：查重数据库写入空间不足，已提交记录保留。技术信息：{error}"),
    }
}

#[cfg(windows)]
fn free_bytes(dir: &Path) -> Result<u64, String> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(
            path: *const u16,
            available: *mut u64,
            total: *mut u64,
            free: *mut u64,
        ) -> i32;
    }
    let mut path: Vec<u16> = dir.as_os_str().encode_wide().collect();
    path.push(0);
    let mut available = 0;
    let success = unsafe {
        GetDiskFreeSpaceExW(
            path.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if success == 0 {
        return Err(format!(
            "无法检查磁盘可用空间：{}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(available)
}
#[cfg(not(windows))]
fn free_bytes(dir: &Path) -> Result<u64, String> {
    let output = std::process::Command::new("df")
        .args(["-Pk", "--"])
        .arg(dir)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("无法检查磁盘可用空间".into());
    }
    let line = String::from_utf8_lossy(&output.stdout)
        .lines()
        .last()
        .unwrap_or("")
        .to_string();
    line.split_whitespace()
        .nth(3)
        .ok_or("磁盘空间输出无效")?
        .parse::<u64>()
        .map(|n| n * 1024)
        .map_err(|e| e.to_string())
}

#[derive(Clone)]
struct CachedFingerprint {
    image_pk: i64,
    public_hash: [u8; 32],
    signature: [u8; 32],
    size: i64,
    mtime: i64,
    identity: [u8; 32],
    sha: Option<[u8; 32]>,
    near: Option<fingerprint::NearFingerprint>,
    width: u32,
    height: u32,
    skip: Option<String>,
    image_order: u32,
}
fn fixed<const N: usize>(value: Vec<u8>) -> rusqlite::Result<[u8; N]> {
    value.try_into().map_err(|_| rusqlite::Error::InvalidQuery)
}
fn cached_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CachedFingerprint> {
    let phash: Option<Vec<u8>> = row.get(7)?;
    let dhash: Option<Vec<u8>> = row.get(8)?;
    let color: Option<Vec<u8>> = row.get(9)?;
    let width: u32 = row.get(10)?;
    let height: u32 = row.get(11)?;
    let near = match (phash, dhash, color) {
        (Some(p), Some(d), Some(c)) => Some(fingerprint::NearFingerprint {
            phash: u64::from_le_bytes(fixed(p)?),
            dhash: u64::from_le_bytes(fixed(d)?),
            color: fixed(c)?,
            width,
            height,
            low_information: row.get::<_, i64>(12)? != 0,
        }),
        (None, None, None) => None,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    Ok(CachedFingerprint {
        image_pk: row.get(0)?,
        public_hash: fixed(row.get(1)?)?,
        signature: fixed(row.get(2)?)?,
        size: row.get(3)?,
        mtime: row.get(4)?,
        identity: fixed(row.get(5)?)?,
        sha: row.get::<_, Option<Vec<u8>>>(6)?.map(fixed).transpose()?,
        near,
        width,
        height,
        skip: row.get(13)?,
        image_order: row.get(14)?,
    })
}

#[cfg(test)]
pub(super) fn evidence_records(
    state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>,
) -> Result<Vec<index::IndexRecord>, String> {
    evidence_records_with_progress(state, control, &mut |_, _| {})
}

pub(super) fn evidence_records_with_progress(
    state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>,
    progress: &mut dyn FnMut(i64, i64),
) -> Result<Vec<index::IndexRecord>, String> {
    control()?;
    let conn = open_existing(state, false)?;
    let uri = super::workflow::readonly_uri(&state.database_path)?;
    conn.execute("ATTACH DATABASE ?1 AS evidence_library", [uri])
        .map_err(|e| e.to_string())?;
    let version: i64 = conn
        .query_row("PRAGMA evidence_library.user_version", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if version != 3 {
        return Err("重复检索需要 schema 3 图库".into());
    }
    // Every callback may persist task state to the library, including pause
    // acknowledgement. A long library read would block that write in DELETE
    // journal mode. Materialize bounded keyset pages and release their readers
    // before calling either callback. Membership revisions bind the whole load.
    let membership = membership_token(state);
    let active =
        "i.trashed=0 AND i.missing=0 AND COALESCE(i.source,'')<>'reference' AND f.sha IS NOT NULL";
    let count:usize=conn.query_row(&format!("SELECT COUNT(*) FROM fingerprints f JOIN evidence_library.images i ON i.image_pk=f.image_pk WHERE {active}"),[],|r|r.get(0)).map_err(|e|e.to_string())?;
    progress(0, count as i64);
    if index::estimate_memory_bytes(count).saturating_add(24 * 1024 * 1024) > MAX_MEMORY_BYTES {
        return Err("当前索引超过512MiB预算，未替换已有结果".into());
    }
    let mut records = Vec::new();
    records
        .try_reserve_exact(count)
        .map_err(|_| "索引内存不足")?;
    let columns = FINGERPRINT_COLUMNS
        .split(',')
        .map(|c| format!("f.{c}"))
        .collect::<Vec<_>>()
        .join(",");
    let sql=format!("SELECT {columns},i.id FROM fingerprints f CROSS JOIN evidence_library.images i ON i.image_pk=f.image_pk WHERE {active} AND f.image_pk>?1 ORDER BY f.image_pk LIMIT 1000");
    let mut after = i64::MIN;
    loop {
        control()?;
        if membership != membership_token(state) {
            return Err("载入指纹期间图库成员已变化，保留原检索结果，请重新运行".into());
        }
        let page = {
            let mut statement = conn.prepare(&sql).map_err(|e| e.to_string())?;
            let page = statement
                .query_map([after], |r| Ok((cached_row(r)?, r.get::<_, String>(15)?)))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            page
        }; // No library statement/transaction survives this boundary.
        if page.is_empty() {
            break;
        }
        for (row, public_id) in page {
            after = row.image_pk;
            if hash(public_id.as_bytes()) != row.public_hash {
                mark_stale(state);
                return Err("图片身份已变化，请重新检查指纹；保留原检索结果".into());
            }
            records.push(index::IndexRecord {
                image_pk: row.image_pk,
                image_order: row.image_order,
                file_size: row.size as u64,
                width: row.width,
                height: row.height,
                sha256: row.sha.unwrap(),
                near: row.near,
            });
        }
        if index::estimate_memory_bytes(records.len()).saturating_add(24 * 1024 * 1024)
            > MAX_MEMORY_BYTES
        {
            return Err("当前索引超过512MiB预算，未替换已有结果".into());
        }
        progress(records.len() as i64, count as i64);
    }
    if index::estimate_memory_bytes(records.len()).saturating_add(24 * 1024 * 1024)
        > MAX_MEMORY_BYTES
    {
        return Err("当前索引超过512MiB预算，未替换已有结果".into());
    }
    control()?;
    if membership != membership_token(state) || records.len() != count {
        return Err("载入指纹期间图库成员已变化，保留原检索结果，请重新运行".into());
    }
    progress(records.len() as i64, count as i64);
    Ok(records)
}

pub(super) fn evidence_published(state: &AppState, membership: &str) {
    let rt = runtime(state);
    rt.invalidated.store(false, Ordering::SeqCst);
    // A source change just after COMMIT must not be cleared by publication.
    if membership != membership_token(state) {
        rt.invalidated.store(true, Ordering::SeqCst);
        if let Ok(conn) = open_existing(state, true) {
            let _ = set_meta(&conn, "dirty", "1");
        }
    }
    if let Ok(mut view) = rt.live.lock() {
        *view = None;
    };
}
const FINGERPRINT_COLUMNS:&str="image_pk,public_hash,signature,size,mtime,identity,sha,phash,dhash,color,width,height,low_info,skip,image_order";
fn get_fingerprint(conn: &Connection, image_pk: i64) -> Result<Option<CachedFingerprint>, String> {
    conn.query_row(
        &format!("SELECT {FINGERPRINT_COLUMNS} FROM fingerprints WHERE image_pk=?1"),
        [image_pk],
        cached_row,
    )
    .optional()
    .map_err(|e| format!("查重缓存指纹损坏，请清理缓存后重新检索：{e}"))
}

struct LiveSourceMember {
    number: i64,
    pk: i64,
    sha: Option<[u8; 32]>,
}

fn insert_live_group(view: &mut LiveView, group: LiveGroup) {
    if group.kind == 0 {
        view.exact += 1;
    } else {
        view.near += 1;
    }
    view.groups.insert(group.id, group);
}

fn live_source_group(
    conn: &Connection,
    view: &mut LiveView,
    source: i64,
    kind: i64,
    original_reference: i64,
    members: &[LiveSourceMember],
    threshold: index::Threshold,
    next_id: &mut i64,
    unique: &mut HashSet<i64>,
) -> Result<(), String> {
    if members.len() < 2 {
        return Ok(());
    }
    let original = members.iter().find(|m| m.pk == original_reference);
    let original_sha = match original {
        Some(member) => member.sha,
        None if kind == 1 => conn.query_row(
            "SELECT f.sha FROM members m JOIN fingerprints f ON f.image_pk=m.image_pk AND f.signature=m.signature WHERE m.generation=?1 AND m.group_no=?2 AND m.image_pk=?3 LIMIT 1",
            params![view.generation, source, original_reference],
            |r| r.get::<_, Option<Vec<u8>>>(0)?.map(fixed).transpose(),
        ).optional().map_err(|e| e.to_string())?.flatten(),
        None => None,
    };
    let replacement = original
        .or_else(|| original_sha.and_then(|sha| members.iter().find(|m| m.sha == Some(sha))));
    // A near result requires at least two content units. Copies of the last
    // surviving unit remain in exact results and must not keep a near star alive.
    if kind == 1 && members[0].sha.is_some() && members.iter().all(|m| m.sha == members[0].sha) {
        return Ok(());
    }
    let reference_changed = kind == 1
        && meta(conn, &format!("review:{}:{source}", view.generation))?.as_deref() == Some("1");
    if kind == 0 || (replacement.is_some() && !reference_changed) {
        unique.extend(members.iter().map(|m| m.pk));
        insert_live_group(
            view,
            LiveGroup {
                id: source,
                source_group: source,
                kind,
                reference: replacement.map(|m| m.pk).unwrap_or(members[0].pk),
                total: members.len() as i64,
                recalculate_metrics: replacement.is_some_and(|m| m.pk != original_reference),
                members: None,
            },
        );
        return Ok(());
    }
    // Losing a star's entire reference content unit does not make its leaves
    // mutually similar. Rebuild only this star from already cached numbers;
    // no file is opened, decoded or fingerprinted here, and no transitive
    // component is formed. Multiple direct stars may survive the removal.
    if index::estimate_memory_bytes(members.len()).saturating_add(48 * 1024 * 1024)
        > MAX_MEMORY_BYTES
    {
        return Err("活跃查重分组超过内存预算，请手动更新结果".into());
    }
    let mut records = Vec::with_capacity(members.len());
    let mut numbers = Vec::with_capacity(members.len());
    for member in members {
        let Some(cached) = get_fingerprint(conn, member.pk)? else {
            continue;
        };
        // A partial newer scan may have replaced a fingerprint. Do not apply
        // that new geometry to membership from an older published generation.
        if cached.sha != member.sha || member.sha.is_none() {
            continue;
        }
        records.push(index::IndexRecord {
            image_pk: member.pk,
            image_order: cached.image_order,
            file_size: cached.size as u64,
            sha256: cached.sha.unwrap(),
            near: cached.near,
            width: cached.width,
            height: cached.height,
        });
        numbers.push(member.number);
    }
    let mut first = true;
    let refined =
        meta(conn, &format!("verification:{}", view.generation))?.as_deref() == Some("verified");
    index::build_groups_filtered(
        &records,
        threshold,
        &mut || Ok(()),
        &mut |a, b, kind| {
            if !refined {
                return Ok(true);
            }
            super::verification_cache::accepted_cached(
                conn,
                records[a].image_pk,
                records[b].image_pk,
                kind == index::DuplicateKind::Exact,
            )
        },
        &mut |group| {
            if group.kind != index::DuplicateKind::Near {
                return Ok(());
            }
            let id = if first {
                first = false;
                source
            } else {
                let id = *next_id;
                *next_id += 1;
                id
            };
            let mut selected: Vec<_> = group
                .members
                .iter()
                .map(|m| {
                    let pk = records[m.record_index].image_pk;
                    unique.insert(pk);
                    (numbers[m.record_index], pk)
                })
                .collect();
            selected.sort_unstable();
            insert_live_group(
                view,
                LiveGroup {
                    id,
                    source_group: source,
                    kind: 1,
                    reference: records[group.reference].image_pk,
                    total: selected.len() as i64,
                    recalculate_metrics: true,
                    members: Some(selected),
                },
            );
            Ok(())
        },
    )?;
    Ok(())
}

/// A complete active-membership snapshot is computed once per published
/// generation / operation revision, then shared by status and every result page.
/// The main database and the published cache stay read-only. Normal groups only
/// retain a small summary; member lists are retained solely for rebuilt stars.
fn live_view(
    conn: &Connection,
    generation: i64,
    state: &AppState,
) -> Result<Arc<LiveView>, String> {
    let rt = runtime(state);
    let revision = rt.membership_revision.load(Ordering::SeqCst);
    let mut memo = rt.live.lock().map_err(|e| e.to_string())?;
    if let Some(view) = memo
        .as_ref()
        .filter(|v| v.generation == generation && v.revision == revision)
    {
        return Ok(view.clone());
    }
    let sensitivity: String = conn
        .query_row(
            "SELECT sensitivity FROM generations WHERE generation=?1",
            [generation],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let threshold = match sensitivity.as_str() {
        "strict" => index::Threshold::Strict,
        "loose" => index::Threshold::Loose,
        _ => index::Threshold::Standard,
    };
    let mut next_id: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(group_no),0)+1 FROM groups WHERE generation=?1",
            [generation],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let mut view = LiveView {
        generation,
        revision,
        groups: BTreeMap::new(),
        exact: 0,
        near: 0,
        unique: 0,
    };
    let mut unique = HashSet::new();
    let mut source = None;
    let mut members = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT g.group_no,g.kind,g.reference_pk,m.member_no,m.image_pk,m.public_hash,i.id,CASE WHEN f.signature=m.signature THEN f.sha END FROM groups g CROSS JOIN members m ON m.generation=g.generation AND m.group_no=g.group_no CROSS JOIN live.images i ON i.image_pk=m.image_pk LEFT JOIN fingerprints f ON f.image_pk=m.image_pk WHERE g.generation=?1 AND COALESCE(i.trashed,0)=0 AND COALESCE(i.source,'')<>'reference' ORDER BY g.group_no,m.member_no"
    ).map_err(|e| e.to_string())?;
    let mut rows = stmt.query([generation]).map_err(|e| e.to_string())?;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let current = (
            row.get::<_, i64>(0).map_err(|e| e.to_string())?,
            row.get::<_, i64>(1).map_err(|e| e.to_string())?,
            row.get::<_, i64>(2).map_err(|e| e.to_string())?,
        );
        if source != Some(current) {
            if let Some((group, kind, reference)) = source {
                live_source_group(
                    conn,
                    &mut view,
                    group,
                    kind,
                    reference,
                    &members,
                    threshold,
                    &mut next_id,
                    &mut unique,
                )?;
            }
            members.clear();
            source = Some(current);
        }
        let public: Vec<u8> = row.get(5).map_err(|e| e.to_string())?;
        let id: String = row.get(6).map_err(|e| e.to_string())?;
        if hash(id.as_bytes()).as_slice() != public.as_slice() {
            mark_stale(state);
            continue;
        }
        members.push(LiveSourceMember {
            number: row.get(3).map_err(|e| e.to_string())?,
            pk: row.get(4).map_err(|e| e.to_string())?,
            sha: row
                .get::<_, Option<Vec<u8>>>(7)
                .map_err(|e| e.to_string())?
                .map(fixed)
                .transpose()
                .map_err(|e| e.to_string())?,
        });
    }
    if let Some((group, kind, reference)) = source {
        live_source_group(
            conn,
            &mut view,
            group,
            kind,
            reference,
            &members,
            threshold,
            &mut next_id,
            &mut unique,
        )?;
    }
    view.unique = unique.len() as i64;
    let view = Arc::new(view);
    *memo = Some(view.clone());
    Ok(view)
}

/// Soft recycle, restore and confirmed deletion only change which published
/// members remain active. They do not require a new scan and must not erase
/// an existing dirty flag caused by added or externally changed source files.
pub fn membership_invalidated(state: &AppState) {
    let rt = runtime(state);
    rt.membership_revision.fetch_add(1, Ordering::SeqCst);
    rt.revision.fetch_add(1, Ordering::SeqCst);
}
pub(super) fn membership_token(state: &AppState) -> String {
    static BOOT: OnceLock<i64> = OnceLock::new();
    format!(
        "{}:{}",
        BOOT.get_or_init(now),
        runtime(state).membership_revision.load(Ordering::SeqCst)
    )
}
fn put_batch(
    conn: &mut Connection,
    rows: &mut Vec<CachedFingerprint>,
    run: i64,
) -> Result<(), String> {
    if rows.is_empty() {
        return Ok(());
    }
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    {
        let mut insert=tx.prepare("INSERT INTO fingerprints(image_pk,public_hash,signature,size,mtime,identity,sha,phash,dhash,color,width,height,low_info,skip,image_order,seen_run) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16) ON CONFLICT(image_pk) DO UPDATE SET public_hash=excluded.public_hash,signature=excluded.signature,size=excluded.size,mtime=excluded.mtime,identity=excluded.identity,sha=excluded.sha,phash=excluded.phash,dhash=excluded.dhash,color=excluded.color,width=excluded.width,height=excluded.height,low_info=excluded.low_info,skip=excluded.skip,image_order=excluded.image_order,seen_run=excluded.seen_run")
            .map_err(|e|e.to_string())?;
        for row in rows.iter() {
            let phash = row.near.as_ref().map(|n| n.phash.to_le_bytes());
            let dhash = row.near.as_ref().map(|n| n.dhash.to_le_bytes());
            insert
                .execute(params![
                    row.image_pk,
                    row.public_hash.as_slice(),
                    row.signature.as_slice(),
                    row.size,
                    row.mtime,
                    row.identity.as_slice(),
                    row.sha.as_ref().map(|h| h.as_slice()),
                    phash.as_ref().map(|h| h.as_slice()),
                    dhash.as_ref().map(|h| h.as_slice()),
                    row.near.as_ref().map(|n| n.color.as_slice()),
                    row.width,
                    row.height,
                    row.near
                        .as_ref()
                        .map(|n| n.low_information)
                        .unwrap_or(false) as i64,
                    row.skip,
                    row.image_order,
                    run
                ])
                .map_err(|e| format!("写入查重指纹失败（可能达到缓存预算或磁盘已满）：{e}"))?;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    rows.clear();
    Ok(())
}
fn add_error(errors: &mut Vec<String>, error: impl Into<String>) {
    if errors.len() < MAX_ERRORS {
        let error = error.into();
        errors.push(error.chars().take(240).collect());
    }
}
fn attempt(
    conn: &Connection,
    state: &str,
    coverage: &DuplicateCoverage,
    errors: &[String],
) -> Result<(), String> {
    let row = DuplicateAttempt {
        state: state.into(),
        coverage: coverage.clone(),
        updated_at: now(),
        errors: errors.to_vec(),
    };
    set_meta(
        conn,
        "attempt",
        &serde_json::to_string(&row).map_err(|e| e.to_string())?,
    )
}
fn checkpoint_foreground(
    state: &AppState,
    checkpoint: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    checkpoint()?;
    while state.foreground_queries.load(Ordering::SeqCst) > 0 {
        thread::sleep(Duration::from_millis(10));
        checkpoint()?;
    }
    if state.maintenance.load(Ordering::SeqCst) {
        return Err("数据维护已启动，查重中断".into());
    }
    Ok(())
}
pub(super) fn delete_unpublished(
    conn: &mut Connection,
    checkpoint: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    let published = meta(conn, "published")?
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(-1);
    // Select generations in bounded pages; each delete probes one PK prefix.
    // Never rescan the entire retained complete generation for every 5,000 rows.
    let mut last = i64::MIN;
    loop {
        checkpoint()?;
        let generations = {
            let mut stmt=conn.prepare("SELECT generation FROM generations WHERE generation>?1 AND generation<>?2 ORDER BY generation LIMIT 100").map_err(|e|e.to_string())?;
            let page = stmt
                .query_map(params![last, published], |r| r.get::<_, i64>(0))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            page
        };
        if generations.is_empty() {
            break;
        }
        for generation in generations {
            loop {
                checkpoint()?;
                let changed=conn.execute("DELETE FROM members WHERE (generation,group_no,member_no) IN (SELECT generation,group_no,member_no FROM members WHERE generation=?1 LIMIT 5000)",[generation]).map_err(|e|format!("清理旧查重缓存失败（可能达到预算或磁盘已满）：{e}"))?;
                if changed == 0 {
                    break;
                }
            }
            checkpoint()?;
            conn.execute("DELETE FROM groups WHERE generation=?1", [generation])
                .map_err(|e| e.to_string())?;
            conn.execute("DELETE FROM generations WHERE generation=?1", [generation])
                .map_err(|e| e.to_string())?;
            last = generation;
        }
    }
    conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE);")
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Runs on the task worker, never on the UI thread and never inside a main-DB write transaction.
pub fn run_job(
    options: DuplicateOptions,
    state: &AppState,
    checkpoint: &mut dyn FnMut() -> Result<(), String>,
    progress: &mut dyn FnMut(&str, &DuplicateCoverage),
) -> Result<(), String> {
    if state.read_only {
        return Err("只读图库不能计算查重缓存".into());
    }
    if options.force_recompute && options.regroup_only {
        return Err("强制重新计算不能与仅重新分组同时使用".into());
    }
    let refined = match options.verification_mode.as_str() {
        "" | "candidate" | "prepareEvidence" => false,
        "verified" => true,
        _ => return Err("近似复核模式无效".into()),
    };
    let threshold = match options.sensitivity.as_str() {
        "strict" => index::Threshold::Strict,
        "standard" => index::Threshold::Standard,
        "loose" => index::Threshold::Loose,
        _ => return Err("查重档位无效".into()),
    };
    let rt = runtime(state);
    rt.active
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .map_err(|_| "查重任务已经运行".to_string())?;
    let _guard = RunGuard(rt.clone());
    let run_started_at = now();
    let starting_revision = rt.revision.load(Ordering::SeqCst);
    let db = library(state)?;
    let mut conn = create_or_open(state)?;
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let run = run_started_at.saturating_mul(1024)
        + (SEQUENCE.fetch_add(1, Ordering::SeqCst) % 1024) as i64;
    let mut coverage = DuplicateCoverage::default();
    coverage.total = db
        .query_row(
            &format!("SELECT COUNT(*) FROM images i WHERE {ACTIVE}"),
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    configure_capacity(&conn, &cache_path(state), coverage.total as u64)?;
    let mut errors = Vec::new();
    attempt(&conn, "running", &coverage, &errors)?;
    set_meta(&conn, "dirty", "1")?;
    let mut records = Vec::<index::IndexRecord>::new();
    let mut generation_started = false;
    let mut group_no = 0i64;
    let mut exact = 0i64;
    let mut near = 0i64;
    let mut group_complete = false;
    let mut grouping_error = None;
    let mut rows_to_commit = Vec::with_capacity(100);
    let mut last_emit = Instant::now() - Duration::from_secs(1);
    let mut last_stage = String::new();
    let mut emit = |stage: &str, cov: &DuplicateCoverage| {
        if stage != last_stage || last_emit.elapsed() >= Duration::from_millis(200) {
            progress(stage, cov);
            last_emit = Instant::now();
            last_stage = stage.into();
        }
    };
    let result = (|| -> Result<(), String> {
        emit("整理旧查重缓存", &coverage);
        let mut cleanup_checkpoint = || checkpoint_foreground(state, checkpoint);
        delete_unpublished(&mut conn, &mut cleanup_checkpoint)?;
        check_disk(&cache_path(state))?;
        let mut last_id = String::new();
        let mut first = true;
        let mut image_order = 0u32;
        loop {
            checkpoint_foreground(state, checkpoint)?;
            // IDs are indexed and uniquely ordered. No all-library path vector is retained.
            let page = {
                let compare = if first { ">=" } else { ">" };
                let sql=format!("SELECT image_pk,id,path,COALESCE(width,0),COALESCE(height,0) FROM images i WHERE {ACTIVE} AND id {compare} ?1 ORDER BY id LIMIT 1000");
                let mut stmt = db.prepare(&sql).map_err(|e| e.to_string())?;
                let page = stmt
                    .query_map([&last_id], |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, u32>(3)?,
                            r.get::<_, u32>(4)?,
                        ))
                    })
                    .map_err(|e| e.to_string())?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())?;
                page
            };
            if page.is_empty() {
                break;
            }
            first = false;
            last_id = page.last().unwrap().1.clone();
            for (image_pk, id, path, width, height) in page {
                checkpoint_foreground(state, checkpoint)?;
                let source = Path::new(&path);
                let public_hash = hash(id.as_bytes());
                let cached = get_fingerprint(&conn, image_pk)?;
                let info = fingerprint::source_signature(source);
                let mut row = match info {
                    Ok(info) => {
                        let current = signature(&id, &info);
                        if !options.force_recompute
                            && cached.as_ref().is_some_and(|c| {
                                c.signature == current
                                    && c.public_hash == public_hash
                                    && c.sha.is_some()
                            })
                        {
                            cached.unwrap()
                        } else if options.regroup_only {
                            coverage.failed += 1;
                            add_error(
                                &mut errors,
                                "部分图片指纹缺失或已变化，请点击更新结果，不能仅重新分组",
                            );
                            CachedFingerprint {
                                image_pk,
                                public_hash,
                                signature: current,
                                size: info.file_size as i64,
                                mtime: info.modified_ns,
                                identity: hash(info.identity.as_bytes()),
                                sha: None,
                                near: None,
                                width,
                                height,
                                skip: Some("unindexed".into()),
                                image_order,
                            }
                        } else {
                            let mut yielding = || checkpoint_foreground(state, checkpoint);
                            match fingerprint::fingerprint(source, &mut yielding) {
                                Ok(value) => CachedFingerprint {
                                    image_pk,
                                    public_hash,
                                    signature: signature(&id, &value.signature),
                                    size: value.signature.file_size as i64,
                                    mtime: value.signature.modified_ns,
                                    identity: hash(value.signature.identity.as_bytes()),
                                    sha: Some(value.sha256),
                                    width: value.near.as_ref().map(|n| n.width).unwrap_or(width),
                                    height: value.near.as_ref().map(|n| n.height).unwrap_or(height),
                                    near: value.near,
                                    skip: value.skip_reason,
                                    image_order,
                                },
                                Err(error) => {
                                    checkpoint_foreground(state, checkpoint)?;
                                    coverage.failed += 1;
                                    add_error(
                                        &mut errors,
                                        format!(
                                            "{}：{error}",
                                            source
                                                .file_name()
                                                .unwrap_or_default()
                                                .to_string_lossy()
                                        ),
                                    );
                                    CachedFingerprint {
                                        image_pk,
                                        public_hash,
                                        signature: current,
                                        size: info.file_size as i64,
                                        mtime: info.modified_ns,
                                        identity: hash(info.identity.as_bytes()),
                                        sha: None,
                                        near: None,
                                        width,
                                        height,
                                        skip: Some("read_error".into()),
                                        image_order,
                                    }
                                }
                            }
                        }
                    }
                    Err(error) => {
                        coverage.failed += 1;
                        add_error(
                            &mut errors,
                            format!(
                                "文件无法读取（来源可能离线）：{}：{error}",
                                source.file_name().unwrap_or_default().to_string_lossy()
                            ),
                        );
                        CachedFingerprint {
                            image_pk,
                            public_hash,
                            signature: [0; 32],
                            size: 0,
                            mtime: 0,
                            identity: [0; 32],
                            sha: None,
                            near: None,
                            width,
                            height,
                            skip: Some("offline_or_missing".into()),
                            image_order,
                        }
                    }
                };
                row.image_order = image_order;
                image_order = image_order.checked_add(1).ok_or("图片数量超出索引容量")?;
                coverage.processed += 1;
                if row.sha.is_some()
                    && (row.near.is_none() || row.near.as_ref().is_some_and(|n| n.low_information))
                {
                    coverage.skipped_near += 1;
                }
                if row.near.as_ref().is_some_and(|n| n.low_information) {
                    coverage.low_information += 1;
                }
                if row.skip.as_deref() == Some("decode_error") {
                    add_error(
                        &mut errors,
                        format!(
                            "{} 解码失败，仅完成文件哈希检查",
                            source.file_name().unwrap_or_default().to_string_lossy()
                        ),
                    );
                }
                rows_to_commit.push(row);
                if rows_to_commit.len() >= 100 {
                    put_batch(&mut conn, &mut rows_to_commit, run)?;
                    check_disk(&cache_path(state))?;
                    attempt(&conn, "running", &coverage, &errors)?;
                }
                emit("计算图片指纹", &coverage);
            }
        }
        put_batch(&mut conn, &mut rows_to_commit, run)?;
        check_disk(&cache_path(state))?;
        if options.verification_mode == "prepareEvidence" {
            // The replacement job owns discovery. Do not build/store the legacy
            // coarse graph just to throw it away again in the next phase.
            conn.execute("INSERT INTO generations(generation,sensitivity,checked_at,state,coverage,errors) VALUES(?1,?2,?3,'building',?4,?5)",params![run,options.sensitivity,now(),serde_json::to_string(&coverage).unwrap(),serde_json::to_string(&errors).unwrap()]).map_err(|e|e.to_string())?;
            generation_started = true;
            return Ok(());
        }
        // Only numeric fingerprints enter the index; paths are fetched per result page.
        let count: usize = conn
            .query_row(
                "SELECT COUNT(*) FROM fingerprints WHERE seen_run=?1 AND sha IS NOT NULL",
                [run],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let estimate = index::estimate_memory_bytes(count).saturating_add(24 * 1024 * 1024);
        budget(
            effective_cache_bytes(&cache_path(state))?,
            free_bytes(cache_path(state).parent().unwrap())?,
            estimate,
            disk_allowance(&cache_path(state)),
        )?;
        records
            .try_reserve_exact(count)
            .map_err(|_| "查重索引内存不足")?;
        {
            let mut stmt=conn.prepare(&format!("SELECT {FINGERPRINT_COLUMNS} FROM fingerprints WHERE seen_run=?1 AND sha IS NOT NULL ORDER BY image_pk")).map_err(|e|e.to_string())?;
            let mut cursor = stmt.query([run]).map_err(|e| e.to_string())?;
            while let Some(value) = cursor.next().map_err(|e| e.to_string())? {
                let row = cached_row(value).map_err(|e| e.to_string())?;
                records.push(index::IndexRecord {
                    image_pk: row.image_pk,
                    image_order: row.image_order,
                    file_size: row.size as u64,
                    sha256: row.sha.unwrap(),
                    near: row.near,
                    width: row.width,
                    height: row.height,
                });
                if records.len() % 1000 == 0 {
                    checkpoint_foreground(state, checkpoint)?;
                }
            }
        }
        conn.execute("INSERT INTO generations(generation,sensitivity,checked_at,state,coverage,errors) VALUES(?1,?2,?3,'building',?4,?5)",params![run,options.sensitivity,now(),serde_json::to_string(&coverage).unwrap(),serde_json::to_string(&errors).unwrap()]).map_err(|e|e.to_string())?;
        generation_started = true;
        set_meta(
            &conn,
            &format!("verification:{run}"),
            if refined { "verified" } else { "candidate" },
        )?;
        set_meta(
            &conn,
            &format!("verifier:{run}"),
            super::verification::ALGORITHM_VERSION,
        )?;
        let verifier = std::cell::RefCell::new(super::verification_cache::Verifier::new(
            state,
            options.force_recompute,
        )?);
        emit("整理重复分组", &coverage);
        // The index and its output callback invoke checkpoints sequentially.
        // RefCell shares the same cancellation/pause hook without retaining
        // two simultaneous mutable borrows of the callback.
        let control = std::cell::RefCell::new(&mut *checkpoint);
        let reporting = std::cell::RefCell::new(&mut emit);
        let verification_progress = std::cell::RefCell::new(coverage.clone());
        let mut accept_pair =
            |a: usize, b: usize, kind: index::DuplicateKind| -> Result<bool, String> {
                if kind == index::DuplicateKind::Near && !refined {
                    return Ok(true);
                }
                checkpoint_foreground(state, &mut **control.borrow_mut())?;
                let mut verifier = verifier.borrow_mut();
                let result = verifier.check(
                    records[a].image_pk,
                    records[b].image_pk,
                    kind == index::DuplicateKind::Exact,
                    state,
                    &mut **control.borrow_mut(),
                )?;
                let mut cov = verification_progress.borrow_mut();
                cov.verified_pairs = verifier.accepted;
                cov.content_variants = verifier.variants;
                cov.uncertain_pairs = verifier.uncertain;
                reporting.borrow_mut()(
                    if kind == index::DuplicateKind::Exact {
                        "逐字节确认完全相同图片"
                    } else {
                        "复核候选图片的局部内容"
                    },
                    &cov,
                );
                Ok(result.is_equivalent())
            };
        let mut on_group = |group: index::DuplicateGroup| -> Result<(), String> {
            let kind = match group.kind {
                index::DuplicateKind::Exact => 0,
                index::DuplicateKind::Near => 1,
            };
            group_no += 1;
            for (chunk_index, chunk) in group.members.chunks(1000).enumerate() {
                checkpoint_foreground(state, &mut **control.borrow_mut())?;
                // The page/WAL reserve applies to one bounded transaction,
                // including checkpoint overlap, not an entire huge group.
                let growth = (chunk.len() as u64)
                    .saturating_mul(320)
                    .saturating_add(64 * 1024);
                let free = free_bytes(cache_path(state).parent().unwrap())?;
                budget(
                    effective_cache_bytes(&cache_path(state))?.saturating_add(growth),
                    free.saturating_sub(growth),
                    0,
                    disk_allowance(&cache_path(state)),
                )?;
                let tx = conn.transaction().map_err(|e| e.to_string())?;
                if chunk_index == 0 {
                    tx.execute("INSERT INTO groups(generation,group_no,kind,reference_pk,total_members) VALUES(?1,?2,?3,?4,0)",params![run,group_no,kind,records[group.reference].image_pk]).map_err(|e|e.to_string())?;
                }
                {
                    let mut insert=tx.prepare("INSERT INTO members(generation,group_no,member_no,image_pk,public_hash,signature,phash_distance,dhash_distance,color_distance) SELECT ?1,?2,?3,image_pk,public_hash,signature,?5,?6,?7 FROM fingerprints WHERE image_pk=?4").map_err(|e|e.to_string())?;
                    for (offset, member) in chunk.iter().enumerate() {
                        let metrics = member.metrics.as_ref();
                        insert
                            .execute(params![
                                run,
                                group_no,
                                (chunk_index * 1000 + offset) as i64,
                                records[member.record_index].image_pk,
                                metrics.map(|m| m.phash_distance),
                                metrics.map(|m| m.dhash_distance),
                                metrics.map(|m| m.color_distance as f64)
                            ])
                            .map_err(|e| format!("保存查重分组失败（可能达到缓存预算）：{e}"))?;
                    }
                }
                let committed = (chunk_index * 1000 + chunk.len()) as i64;
                tx.execute(
                    "UPDATE groups SET total_members=?3 WHERE generation=?1 AND group_no=?2",
                    params![run, group_no, committed],
                )
                .map_err(|e| e.to_string())?;
                tx.commit().map_err(|e| e.to_string())?;
                if chunk_index == 0 {
                    if kind == 0 {
                        exact += 1;
                    } else {
                        near += 1;
                    }
                }
                conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                    .map_err(|e| e.to_string())?;
                reporting.borrow_mut()("保存检索分组", &verification_progress.borrow());
            }
            Ok(())
        };
        let mut yielding = || checkpoint_foreground(state, &mut **control.borrow_mut());
        let grouping = index::build_groups_filtered(
            &records,
            threshold,
            &mut yielding,
            &mut accept_pair,
            &mut on_group,
        );
        drop(on_group);
        drop(reporting);
        coverage.verified_pairs = verifier.borrow().accepted;
        coverage.content_variants = verifier.borrow().variants;
        coverage.uncertain_pairs = verifier.borrow().uncertain;
        if coverage.uncertain_pairs > 0 {
            add_error(
                &mut errors,
                format!(
                    "{} 对图片无法完成逐字节或细节复核，已保留；本次结果不完整",
                    coverage.uncertain_pairs
                ),
            );
        }
        match grouping {
            Ok(summary) => {
                coverage.too_dense = summary.too_dense_references.len() as i64;
                if coverage.too_dense > 0 {
                    add_error(
                        &mut errors,
                        format!(
                            "{} 个参考图的候选超过 50,000 个不同指纹，近似检索不完整",
                            coverage.too_dense
                        ),
                    );
                }
                group_complete = true;
            }
            Err(error) => {
                grouping_error = Some(error.clone());
                return Err(error);
            }
        }
        Ok(())
    })();
    // Commit successful fingerprints even when a checkpoint cancels an in-flight batch.
    let flush = put_batch(&mut conn, &mut rows_to_commit, run);
    if let Err(error) = flush {
        add_error(&mut errors, error);
    }
    let cancelled = result
        .as_ref()
        .err()
        .is_some_and(|e| e.contains("取消") || e.contains("cancel"));
    if let Err(error) = result.as_ref() {
        add_error(&mut errors, error.clone());
    }
    let dirty = rt.revision.load(Ordering::SeqCst) != starting_revision;
    if dirty {
        add_error(&mut errors, "检索期间图库发生变化，请更新结果");
    }
    let complete = result.is_ok()
        && coverage.failed == 0
        && coverage.too_dense == 0
        && coverage.uncertain_pairs == 0
        && !dirty
        && coverage.processed == coverage.total;
    let final_state = if cancelled {
        "cancelled"
    } else if complete {
        "completed"
    } else {
        "partial"
    };
    let published = meta(&conn, "published")?;
    if generation_started {
        let unique: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT image_pk) FROM members WHERE generation=?1",
                [run],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        conn.execute("UPDATE generations SET checked_at=?2,state=?3,coverage=?4,errors=?5,exact_groups=?6,near_groups=?7,unique_images=?8 WHERE generation=?1",params![run,now(),if complete {"ready"} else {"partial"},serde_json::to_string(&coverage).unwrap(),serde_json::to_string(&errors).unwrap(),exact,near,unique]).map_err(|e|e.to_string())?;
        // A partial run never replaces the last complete generation. First-run partials are labelled.
        if options.verification_mode == "prepareEvidence" {
            if complete {
                set_meta(&conn, "prepared_index", &run.to_string())?;
            }
        } else if complete || published.is_none() {
            set_meta(&conn, "published", &run.to_string())?;
            set_meta(&conn, "dirty", if complete { "0" } else { "1" })?;
            if complete {
                rt.invalidated.store(false, Ordering::SeqCst);
                if rt.revision.load(Ordering::SeqCst) != starting_revision {
                    rt.invalidated.store(true, Ordering::SeqCst);
                    set_meta(&conn, "dirty", "1")?;
                }
            }
        }
    }
    attempt(&conn, final_state, &coverage, &errors)?;
    progress(
        if complete {
            "重复检索完成"
        } else {
            "重复检索部分完成"
        },
        &coverage,
    );
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .map_err(|e| e.to_string())?;
    if complete && options.verification_mode != "prepareEvidence" {
        let mut cleanup_checkpoint = || checkpoint_foreground(state, checkpoint);
        if let Err(error) = delete_unpublished(&mut conn, &mut cleanup_checkpoint) {
            // Publication is already complete and must survive a cancellation
            // during disposal of old derived generations. The next manual run
            // resumes that cleanup; the completed generation is never removed.
            add_error(
                &mut errors,
                format!("检索结果已完成，旧缓存清理尚未结束：{error}"),
            );
            attempt(&conn, "completed", &coverage, &errors)?;
            return Err(format!("检索结果已完成，旧缓存清理尚未结束：{error}"));
        }
    }
    if let Err(error) = result {
        return Err(error);
    }
    if !complete {
        return Err(if !group_complete && grouping_error.is_some() {
            grouping_error.unwrap()
        } else {
            "重复检索部分完成，请查看覆盖量与失败原因".into()
        });
    }
    Ok(())
}

fn published(conn: &Connection, generation: &str) -> Result<i64, String> {
    let number = generation.parse::<i64>().map_err(|_| "重复检索代次无效")?;
    if meta(conn, "published")?.as_deref() != Some(generation) {
        return Err("重复检索结果已更新，请刷新".into());
    }
    Ok(number)
}
fn group_number(id: &str) -> Result<i64, String> {
    id.parse::<i64>()
        .ok()
        .filter(|v| *v > 0)
        .ok_or_else(|| "重复分组编号无效".into())
}
fn page_limit(limit: i64, default: i64, max: i64) -> i64 {
    if limit <= 0 {
        default
    } else {
        limit.clamp(1, max)
    }
}
fn group_cursor(cursor: &Option<String>, generation: &str) -> Result<(i64, i64), String> {
    let Some(cursor) = cursor else {
        return Ok((-1, 0));
    };
    let items: Vec<&str> = cursor.split(':').collect();
    if items.len() != 4 || items[0] != "g" || items[1] != generation {
        return Err("重复检索分页已过期，请刷新".into());
    }
    let kind = items[2]
        .parse::<i64>()
        .ok()
        .filter(|v| *v == 0 || *v == 1)
        .ok_or("分页游标无效")?;
    let group = items[3]
        .parse::<i64>()
        .ok()
        .filter(|v| *v > 0)
        .ok_or("分页游标无效")?;
    Ok((kind, group))
}
fn member_cursor(cursor: &Option<String>, generation: &str, group: &str) -> Result<i64, String> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let items: Vec<&str> = cursor.split(':').collect();
    if items.len() != 4 || items[0] != "m" || items[1] != generation || items[2] != group {
        return Err("重复组成员分页已过期，请刷新".into());
    }
    items[3]
        .parse::<i64>()
        .ok()
        .filter(|v| *v >= 0)
        .ok_or_else(|| "成员分页游标无效".into())
}
#[derive(Clone)]
struct StoredMember {
    image_pk: i64,
    public_hash: [u8; 32],
    signature: [u8; 32],
    metric: DuplicateMetric,
}
fn member_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredMember> {
    Ok(StoredMember {
        image_pk: row.get(0)?,
        public_hash: fixed(row.get(1)?)?,
        signature: fixed(row.get(2)?)?,
        metric: DuplicateMetric {
            phash_distance: row.get(3)?,
            dhash_distance: row.get(4)?,
            color_distance: row.get(5)?,
        },
    })
}
const MEMBER_COLUMNS: &str =
    "image_pk,public_hash,signature,phash_distance,dhash_distance,color_distance";
fn image(db: &Connection, image_pk: i64) -> Result<Option<GalleryImage>, String> {
    db.query_row(&format!("SELECT i.id,i.path,CASE WHEN t.source_modified_at=i.modified_at AND t.source_file_size=i.file_size THEN t.thumb_path ELSE NULL END,i.file_name,i.ext,COALESCE(i.width,0),COALESCE(i.height,0),i.file_size,i.modified_at,i.imported_at,i.folder_id,i.missing,i.trashed,i.is_favorite,i.source FROM images i LEFT JOIN storage_image_thumbnails t ON t.image_id=i.image_pk WHERE i.image_pk=?1 AND {ACTIVE}"),[image_pk],|r| {
        Ok(GalleryImage{id:r.get(0)?,path:r.get(1)?,thumbnail_path:r.get(2)?,file_name:r.get(3)?,ext:r.get(4)?,width:r.get(5)?,height:r.get(6)?,file_size:r.get(7)?,modified_at:r.get(8)?,imported_at:r.get(9)?,folder_id:r.get(10)?,missing:r.get::<_,i64>(11)?!=0,trashed:r.get::<_,i64>(12)?!=0,is_favorite:r.get::<_,i64>(13)?!=0,source:r.get(14)?})
    }).optional().map_err(|e|format!("无法核对查重图片：{e}"))
}
pub(crate) fn mark_stale(state: &AppState) {
    let rt = runtime(state);
    rt.revision.fetch_add(1, Ordering::SeqCst);
    rt.membership_revision.fetch_add(1, Ordering::SeqCst);
    let already = rt.invalidated.swap(true, Ordering::SeqCst);
    if !already {
        if let Ok(conn) = open_existing(state, true) {
            let _ = set_meta(&conn, "dirty", "1");
        }
    }
}
fn resolve(
    stored: StoredMember,
    db: &Connection,
    state: &AppState,
) -> Result<Option<DuplicateMember>, String> {
    let Some(mut image) = image(db, stored.image_pk)? else {
        return Ok(None);
    };
    if hash(image.id.as_bytes()) != stored.public_hash {
        // A restored DB may reuse an integer key for a different public image. Never show it.
        mark_stale(state);
        return Ok(None);
    }
    let source = fingerprint::source_signature(Path::new(&image.path));
    let stale = source
        .as_ref()
        .map(|info| signature(&image.id, info) != stored.signature)
        .unwrap_or(true);
    if let Ok(info) = source {
        // An external edit can precede the next library scan. Never show its
        // old cached thumbnail as though it were the currently checked file.
        if image.file_size != info.file_size as i64
            || image.modified_at != info.modified_ns / 1_000_000
        {
            image.thumbnail_path = None;
        }
        image.file_size = info.file_size as i64;
        image.modified_at = info.modified_ns / 1_000_000;
    }
    if stale {
        mark_stale(state);
    }
    Ok(Some(DuplicateMember {
        image,
        metric: stored.metric,
        stale,
    }))
}
pub fn query_groups(query: DuplicateQuery, state: &AppState) -> Result<DuplicateGroupPage, String> {
    let conn = open_existing(state, false)?;
    attach_live(&conn, state)?;
    conn.execute_batch("BEGIN DEFERRED;")
        .map_err(|e| e.to_string())?;
    let generation = published(&conn, &query.generation)?;
    let kind = match query.kind.as_str() {
        "all" | "" => -1,
        "exact" => 0,
        "near" => 1,
        _ => return Err("重复结果类型无效".into()),
    };
    let limit = page_limit(query.limit, 20, 100);
    let (last_kind, last_group) = group_cursor(&query.cursor, &query.generation)?;
    let view = live_view(&conn, generation, state)?;
    let total = match kind {
        0 => view.exact,
        1 => view.near,
        _ => view.exact + view.near,
    };
    let mut rows = Vec::new();
    for local_kind in 0..=1 {
        if kind != -1 && kind != local_kind {
            continue;
        }
        for group in view.groups.values().filter(|g| g.kind == local_kind) {
            if (group.kind, group.id) <= (last_kind, last_group) {
                continue;
            }
            rows.push((group.id, group.kind));
            if rows.len() > limit as usize {
                break;
            }
        }
        if rows.len() > limit as usize {
            break;
        }
    }
    let has_more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next_cursor = if has_more {
        rows.last()
            .map(|r| format!("g:{}:{}:{}", query.generation, r.1, r.0))
    } else {
        None
    };
    let mut groups = Vec::with_capacity(rows.len());
    for (group, _) in rows {
        if let Some(local) = query_group(
            DuplicateGroupQuery {
                generation: query.generation.clone(),
                group_id: group.to_string(),
            },
            state,
        )? {
            groups.push(local);
        }
    }
    Ok(DuplicateGroupPage {
        generation: query.generation,
        groups,
        next_cursor,
        total_groups: total,
    })
}

fn attach_live(conn: &Connection, state: &AppState) -> Result<(), String> {
    // Only an existing, identity-validated main file is attached. query_only
    // remains ON for this connection and neither database can be mutated.
    conn.execute(
        "ATTACH DATABASE ?1 AS live",
        [state.database_path.to_string_lossy().as_ref()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
fn live_members(
    conn: &Connection,
    gen: i64,
    group: i64,
    start: i64,
    limit: i64,
) -> Result<Vec<(i64, StoredMember)>, String> {
    let sql="SELECT m.image_pk,m.public_hash,m.signature,m.phash_distance,m.dhash_distance,m.color_distance,m.member_no FROM members m JOIN live.images i ON i.image_pk=m.image_pk WHERE m.generation=?1 AND m.group_no=?2 AND m.member_no>=?3 AND COALESCE(i.trashed,0)=0 AND COALESCE(i.source,'')<>'reference' ORDER BY m.member_no LIMIT ?4";
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let result = stmt
        .query_map(params![gen, group, start, limit], |row| {
            Ok((row.get(6)?, member_row(row)?))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(result)
}
fn view_members(
    conn: &Connection,
    generation: i64,
    group: &LiveGroup,
    start: i64,
    limit: i64,
) -> Result<Vec<(i64, StoredMember)>, String> {
    let Some(selected) = &group.members else {
        return live_members(conn, generation, group.source_group, start, limit);
    };
    let mut result = Vec::new();
    let mut stmt = conn
        .prepare_cached(&format!(
        "SELECT {MEMBER_COLUMNS} FROM members WHERE generation=?1 AND group_no=?2 AND member_no=?3"
    ))
        .map_err(|e| e.to_string())?;
    for (number, _) in selected
        .iter()
        .filter(|(number, _)| *number >= start)
        .take(limit as usize)
    {
        let member = stmt
            .query_row(params![generation, group.source_group, number], member_row)
            .map_err(|e| e.to_string())?;
        result.push((*number, member));
    }
    Ok(result)
}
fn local_metric(
    conn: &Connection,
    reference: i64,
    member: i64,
    threshold: index::Threshold,
) -> Result<Option<DuplicateMetric>, String> {
    let Some(a) = get_fingerprint(conn, reference)? else {
        return Ok(None);
    };
    let Some(b) = get_fingerprint(conn, member)? else {
        return Ok(None);
    };
    let metric = a
        .near
        .zip(b.near)
        .and_then(|(a, b)| index::compare_near(&a, &b, threshold))
        .map(|m| DuplicateMetric {
            phash_distance: Some(m.phash_distance),
            dhash_distance: Some(m.dhash_distance),
            color_distance: Some(m.color_distance as f64),
        });
    Ok(metric)
}
pub fn query_group(
    query: DuplicateGroupQuery,
    state: &AppState,
) -> Result<Option<DuplicateGroup>, String> {
    let conn = open_existing(state, false)?;
    attach_live(&conn, state)?;
    conn.execute_batch("BEGIN DEFERRED;")
        .map_err(|e| e.to_string())?;
    let gen = published(&conn, &query.generation)?;
    let group = group_number(&query.group_id)?;
    let view = live_view(&conn, gen, state)?;
    let Some(active) = view.groups.get(&group) else {
        return Ok(None);
    };
    let db = library(state)?;
    let sensitivity: String = conn
        .query_row(
            "SELECT sensitivity FROM generations WHERE generation=?1",
            [gen],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let threshold = match sensitivity.as_str() {
        "strict" => index::Threshold::Strict,
        "loose" => index::Threshold::Loose,
        _ => index::Threshold::Standard,
    };
    let mut rows = view_members(&conn, gen, active, 0, 7)?;
    let reference_row=conn.query_row(&format!("SELECT {MEMBER_COLUMNS} FROM members WHERE generation=?1 AND group_no=?2 AND image_pk=?3 LIMIT 1"),params![gen,active.source_group,active.reference],member_row).optional().map_err(|e|e.to_string())?;
    let reference = match reference_row {
        Some(row) => resolve(row, &db, state)?,
        None => None,
    };
    let Some(reference) = reference else {
        return Ok(None);
    };
    let more = rows.len() > 6;
    rows.truncate(6);
    let next = if more {
        rows.last()
            .map(|(n, _)| format!("m:{}:{}:{}", query.generation, query.group_id, n + 1))
    } else {
        None
    };
    let mut members = Vec::new();
    for (_, stored) in rows {
        let pk = stored.image_pk;
        if let Some(mut member) = resolve(stored, &db, state)? {
            if active.kind == 1 && active.recalculate_metrics {
                member.metric = if pk == active.reference {
                    DuplicateMetric::default()
                } else {
                    local_metric(&conn, active.reference, pk, threshold)?.unwrap_or_default()
                };
            }
            members.push(member);
        }
    }
    Ok(Some(DuplicateGroup {
        id: query.group_id,
        kind: if active.kind == 0 { "exact" } else { "near" }.into(),
        reference,
        total_members: active.total,
        members,
        next_cursor: next,
        needs_review: false,
    }))
}
pub(crate) fn review_affected(
    generation: &str,
    pks: &[i64],
    keep_ids: &[String],
    state: &AppState,
) -> Result<(Vec<String>, bool), String> {
    if !cache_path(state).exists() {
        return Ok((vec![], true));
    }
    let conn = open_existing(state, true)?;
    let gen = match published(&conn, generation) {
        Ok(value) => value,
        Err(_) => return Ok((vec![], true)),
    };
    let db = library(state)?;
    let mut affected = std::collections::BTreeSet::new();
    let mut truncated = false;
    for pk in pks {
        let mut stmt = conn
            .prepare("SELECT group_no FROM members WHERE generation=?1 AND image_pk=?2 LIMIT 102")
            .map_err(|e| e.to_string())?;
        for group in stmt
            .query_map(params![gen, pk], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())?
        {
            affected.insert(group.map_err(|e| e.to_string())?);
            if affected.len() > 100 {
                truncated = true;
                break;
            }
        }
        if truncated {
            break;
        }
    }
    for group in affected.iter().take(100) {
        let original: i64 = conn
            .query_row(
                "SELECT reference_pk FROM groups WHERE generation=?1 AND group_no=?2",
                params![gen, group],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let mut preferred = None;
        for id in keep_ids {
            let pk: Option<i64> = db
                .query_row(
                    "SELECT image_pk FROM images WHERE id=?1 AND trashed=0",
                    [id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            if let Some(pk) = pk {
                let exists:i64=conn.query_row("SELECT COUNT(*) FROM members WHERE generation=?1 AND group_no=?2 AND image_pk=?3",params![gen,group,pk],|r|r.get(0)).map_err(|e|e.to_string())?;
                if exists > 0 {
                    preferred = Some(pk);
                    break;
                }
            }
        }
        if let Some(pk) = preferred {
            if pk != original {
                conn.execute(
                    "UPDATE groups SET reference_pk=?3 WHERE generation=?1 AND group_no=?2",
                    params![gen, group, pk],
                )
                .map_err(|e| e.to_string())?;
                set_meta(&conn, &format!("review:{gen}:{group}"), "1")?;
            }
        }
    }
    Ok((
        affected.iter().take(100).map(|g| g.to_string()).collect(),
        truncated,
    ))
}

pub fn query_members(
    query: DuplicateMemberQuery,
    state: &AppState,
) -> Result<DuplicateMemberPage, String> {
    let conn = open_existing(state, false)?;
    attach_live(&conn, state)?;
    conn.execute_batch("BEGIN DEFERRED;")
        .map_err(|e| e.to_string())?;
    let generation = published(&conn, &query.generation)?;
    let group = group_number(&query.group_id)?;
    let view = live_view(&conn, generation, state)?;
    let active = view.groups.get(&group).ok_or("重复分组已失效，请刷新")?;
    let start = member_cursor(&query.cursor, &query.generation, &query.group_id)?;
    let limit = page_limit(query.limit, 50, 200);
    let db = library(state)?;
    let sensitivity: String = conn
        .query_row(
            "SELECT sensitivity FROM generations WHERE generation=?1",
            [generation],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let threshold = match sensitivity.as_str() {
        "strict" => index::Threshold::Strict,
        "loose" => index::Threshold::Loose,
        _ => index::Threshold::Standard,
    };
    let mut rows = view_members(&conn, generation, active, start, limit + 1)?;
    let more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next = rows.last().map(|(n, _)| n + 1);
    let mut members = Vec::new();
    for (_, stored) in rows {
        let pk = stored.image_pk;
        if let Some(mut member) = resolve(stored, &db, state)? {
            if active.kind == 1 && active.recalculate_metrics {
                member.metric = if pk == active.reference {
                    DuplicateMetric::default()
                } else {
                    local_metric(&conn, active.reference, pk, threshold)?.unwrap_or_default()
                };
            }
            members.push(member);
        }
    }
    Ok(DuplicateMemberPage {
        generation: query.generation.clone(),
        group_id: query.group_id.clone(),
        members,
        next_cursor: if more {
            next.map(|next| format!("m:{}:{}:{next}", query.generation, query.group_id))
        } else {
            None
        },
        total_members: active.total,
    })
}

#[cfg(test)]
#[path = "cache_performance_tests.rs"]
mod performance_tests;
#[cfg(test)]
#[path = "cache_storage_tests.rs"]
mod storage_tests;

#[cfg(test)]
pub(super) fn check_budget_for_test(bytes: u64, free: u64, memory: u64) -> Result<(), String> {
    budget(bytes, free, memory, super::cache_capacity::BASE_BYTES)
}

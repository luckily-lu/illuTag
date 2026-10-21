//! Fixed candidate -> detail review workflow. Derived state stays outside the library.
use super::*;
use crate::library::AppState;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex, OnceLock},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

#[path = "workflow_discovery.rs"]
mod discovery;
#[path = "workflow_cascade.rs"]
mod cascade;
#[path = "cascade_scope.rs"]
mod scope;
#[path = "workflow_plan.rs"]
mod planning;
#[path = "workflow_review.rs"]
mod review;
pub use review::{edit, query};

#[cfg(test)]
pub(super) fn reset_cascade_bench_counters() {
    cascade::reset_bench_counters();
}

#[cfg(test)]
pub(super) fn cascade_bench_counters() -> (u64, u64, u64) {
    cascade::bench_counters()
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS workflow_runs(
 generation INTEGER PRIMARY KEY,refinement INTEGER NOT NULL DEFAULT 0,plan INTEGER NOT NULL DEFAULT 0,
 revision INTEGER NOT NULL DEFAULT 0,membership TEXT NOT NULL DEFAULT '',last_operation TEXT);
CREATE TABLE IF NOT EXISTS workflow_class(
 run INTEGER NOT NULL,group_no INTEGER NOT NULL,image_pk INTEGER NOT NULL,kind TEXT NOT NULL,reason TEXT NOT NULL,
 PRIMARY KEY(run,group_no,image_pk)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS workflow_class_kind ON workflow_class(run,kind,group_no,image_pk);
CREATE INDEX IF NOT EXISTS workflow_class_image ON workflow_class(run,image_pk,kind);
CREATE TABLE IF NOT EXISTS workflow_edges(
 run INTEGER NOT NULL,image_pk INTEGER NOT NULL,peer INTEGER NOT NULL,
 PRIMARY KEY(run,image_pk,peer)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS workflow_choices(
 public_hash BLOB PRIMARY KEY,signature BLOB NOT NULL,choice TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS workflow_keepers(
 generation INTEGER NOT NULL,group_no INTEGER NOT NULL,image_pk INTEGER NOT NULL,
 PRIMARY KEY(generation,group_no)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS workflow_keepers_image ON workflow_keepers(generation,image_pk);
CREATE TABLE IF NOT EXISTS workflow_confirmed(
 generation INTEGER NOT NULL,group_id TEXT NOT NULL,PRIMARY KEY(generation,group_id)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS workflow_plan(
 run INTEGER NOT NULL,image_pk INTEGER NOT NULL,keeper INTEGER,kind TEXT NOT NULL,manual INTEGER NOT NULL DEFAULT 0,
 PRIMARY KEY(run,image_pk)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS workflow_plan_keeper ON workflow_plan(run,keeper);
"#;
const ACTIVE: &str = "i.trashed=0 AND i.missing=0 AND COALESCE(i.source,'')<>'reference'";
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
fn serial() -> i64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    now() * 1024 + (N.fetch_add(1, Ordering::Relaxed) % 1024) as i64
}
fn lock(state: &AppState) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<std::path::PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    LOCKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(state.data_dir.clone())
        .or_default()
        .clone()
}
fn attach(conn: &Connection, state: &AppState) -> Result<(), String> {
    let uri = readonly_uri(&state.database_path)?;
    conn.execute("ATTACH DATABASE ?1 AS library", [uri])
        .map_err(|e| format!("无法读取图库数据库，请检查数据目录是否可访问。技术信息：{e}"))?;
    Ok(())
}
/// SQLite interprets `file://?/D:/...` as a URI with authority `?`, not a
/// Windows verbatim path. Keep the authority empty and escape filename bytes
/// before appending query parameters. A UNC share is a double-slash *path*.
pub(super) fn readonly_uri(path: &Path) -> Result<String, String> {
    if !path.is_absolute() {
        return Err("图库数据库路径必须是绝对路径".into());
    }
    let raw = path.to_str().ok_or("图库数据库路径无法编码")?;
    let normalized = if raw
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("\\\\?\\UNC\\"))
    {
        format!("//{}", raw[8..].replace('\\', "/"))
    } else {
        raw.strip_prefix("\\\\?\\")
            .unwrap_or(raw)
            .replace('\\', "/")
    };
    let mut uri = String::from("file://");
    if !normalized.starts_with('/') {
        uri.push('/');
    }
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in normalized.as_bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/:".contains(byte) {
            uri.push(*byte as char);
        } else {
            uri.push('%');
            uri.push(HEX[(byte >> 4) as usize] as char);
            uri.push(HEX[(byte & 15) as usize] as char);
        }
    }
    uri.push_str("?mode=ro");
    Ok(uri)
}
fn open(state: &AppState) -> Result<Connection, String> {
    let conn = cache::open_existing(state, true)?;
    conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    attach(&conn, state)?;
    Ok(conn)
}
fn run(conn: &Connection, generation: i64) -> Result<(i64, i64, i64, String), String> {
    conn.query_row(
        "SELECT refinement,plan,revision,membership FROM workflow_runs WHERE generation=?1",
        [generation],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
    .map_err(|e| e.to_string())
}
fn revision(generation: i64, r: &(i64, i64, i64, String)) -> String {
    format!("{generation}:{}:{}:{}", r.0, r.1, r.2)
}
fn paging_revision(generation: i64, r: &(i64, i64, i64, String)) -> String {
    format!("{generation}:{}:{}", r.0, r.3)
}
fn current(conn: &Connection, generation: &str) -> Result<i64, String> {
    let actual: String = conn
        .query_row("SELECT value FROM meta WHERE key='published'", [], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    if actual != generation {
        return Err("检索结果已更新，请刷新后重试".into());
    }
    generation.parse().map_err(|_| "检索版本无效".into())
}
fn ensure(conn: &Connection, generation: i64, state: &AppState) -> Result<(), String> {
    ensure_controlled(conn, generation, state, &mut || Ok(()))
}
fn ensure_controlled(
    conn: &Connection,
    generation: i64,
    state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    control()?;
    conn.execute(
        "INSERT OR IGNORE INTO workflow_runs(generation) VALUES(?1)",
        [generation],
    )
    .map_err(|e| e.to_string())?;
    let r = run(conn, generation)?;
    if r.1 == 0 || r.3 != cache::membership_token(state) {
        planning::build(conn, generation, r.0, state, control)?;
    }
    Ok(())
}
fn busy(state: &AppState) -> Result<(), String> {
    if state.read_only || state.maintenance.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("图库只读或正在维护".into());
    }
    if state.jobs.list()?.iter().any(|j| {
        matches!(
            j.request.kind.as_str(),
            "duplicates" | "duplicateManagement" | "duplicateUndo"
        ) && matches!(
            j.state.as_str(),
            "queued" | "running" | "pausing" | "paused" | "cancelling"
        )
    }) {
        return Err("请先结束当前检索或整理任务".into());
    }
    Ok(())
}
fn category_sql(kind: &str, refinement: i64) -> Result<String, String> {
    category_sql_history(kind, refinement, false)
}
fn category_sql_history(kind: &str, refinement: i64, history: bool) -> Result<String, String> {
    let mut sql=match kind {
        // Qualify each group once. A correlated EXISTS per displayed member
        // repeatedly scans a large all-retained group when no target exists.
        "recycle"|"retained"=>format!("WITH wanted AS MATERIALIZED (SELECT z.group_no FROM members z JOIN workflow_plan p ON p.image_pk=z.image_pk AND p.run=(SELECT plan FROM workflow_runs WHERE generation=?1) WHERE z.generation=?1 AND {} GROUP BY z.group_no) SELECT m.group_no,m.image_pk FROM wanted w CROSS JOIN members m ON m.generation=?1 AND m.group_no=w.group_no",if kind=="recycle" {"p.keeper IS NOT NULL AND EXISTS(SELECT 1 FROM members k WHERE k.generation=z.generation AND k.group_no=z.group_no AND k.image_pk=p.keeper)"} else {"p.keeper IS NULL"}),
        "exact"=>"SELECT g.group_no,m.image_pk FROM groups g JOIN members m ON m.generation=g.generation AND m.group_no=g.group_no WHERE g.generation=?1 AND g.kind=0".into(),
        "candidate"=>"SELECT g.group_no,m.image_pk FROM groups g JOIN members m ON m.generation=g.generation AND m.group_no=g.group_no WHERE g.generation=?1 AND g.kind=1".into(),
        "near"|"similar"|"uncertain"|"suggested"=>format!("SELECT c.group_no,c.image_pk FROM workflow_class c WHERE c.run={refinement} AND c.kind='{kind}' UNION SELECT c.group_no,g.reference_pk FROM workflow_class c JOIN groups g ON g.generation=?1 AND g.group_no=c.group_no WHERE c.run={refinement} AND c.kind='{kind}'"),
        _=>return Err("结果分类无效".into())
    };
    if kind == "near" {
        // Strict-equal references may have been recycled by the independent
        // exact action. Keep the actual live final keeper in review context.
        sql=format!("{sql} UNION SELECT c.group_no,p.keeper FROM workflow_class c CROSS JOIN workflow_plan p ON p.run=(SELECT plan FROM workflow_runs WHERE generation=?1) AND p.image_pk=c.image_pk WHERE c.run={refinement} AND c.kind='near' AND p.keeper IS NOT NULL AND p.kind='near' AND EXISTS(SELECT 1 FROM meta WHERE key='policy:'||?1 AND value='illuTag-three-pass-v1')");
    }
    let active = if history {
        "COALESCE(i.source,'')<>'reference'"
    } else {
        ACTIVE
    };
    // Preserve the category/group driver through the complete wrapper too.
    // Reordering the final membership check ahead of wanted scanned every
    // wanted group once per library member even though wanted was materialized.
    if kind=="near" {
        Ok(format!("SELECT x.group_no,x.image_pk FROM ({sql}) x CROSS JOIN library.images i ON i.image_pk=x.image_pk CROSS JOIN fingerprints f ON f.image_pk=x.image_pk WHERE {active} AND EXISTS(SELECT 1 FROM members m INDEXED BY members_image WHERE m.generation=?1 AND m.image_pk=x.image_pk AND m.public_hash=f.public_hash AND m.signature=f.signature)"))
    } else {
        Ok(format!("SELECT x.group_no,x.image_pk FROM ({sql}) x CROSS JOIN library.images i ON i.image_pk=x.image_pk CROSS JOIN fingerprints f ON f.image_pk=x.image_pk CROSS JOIN members m INDEXED BY members_image ON m.generation=?1 AND m.group_no=x.group_no AND m.image_pk=x.image_pk AND m.public_hash=f.public_hash AND m.signature=f.signature WHERE {active}"))
    }
}
fn summary(
    conn: &Connection,
    scan: DuplicateStatus,
    state: &AppState,
) -> Result<DuplicateWorkflowStatus, String> {
    let value = summary_controlled(conn, scan, state, &mut || Ok(()))?;
    remember_summary(conn, &value, state)?;
    Ok(value)
}
fn summary_controlled(
    conn: &Connection,
    mut scan: DuplicateStatus,
    state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>,
) -> Result<DuplicateWorkflowStatus, String> {
    let membership = cache::membership_token(state);
    let Some(generation) = scan.generation.clone() else {
        return Ok(empty(scan));
    };
    let gen = current(conn, &generation)?;
    ensure_controlled(conn, gen, state, control)?;
    let r = run(conn, gen)?;
    let policy: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key=?1",
            [format!("policy:{gen}")],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let kinds = if r.0 > 0 {
        vec![
            "exact",
            "candidate",
            "near",
            "similar",
            "uncertain",
            "suggested",
        ]
    } else {
        vec!["exact", "candidate"]
    };
    let mut categories = Vec::new();
    for kind in kinds {
        control()?;
        let category_plan = category_plan(conn, gen, kind, state, control)?;
        let sql = category_sql(kind, r.0)?;
        let predicate = match kind {
            "candidate" => "0",
            "exact" => "p.kind='exact'",
            "near" => "p.kind='near'",
            _ => "p.manual=1",
        };
        let valid=format!("WITH active AS ({sql}), valid AS (SELECT group_no FROM active GROUP BY group_no HAVING COUNT(*)>1) SELECT COUNT(DISTINCT a.image_pk),COUNT(DISTINCT a.group_no),COUNT(DISTINCT CASE WHEN p.keeper IS NOT NULL AND {predicate} THEN a.image_pk END) FROM active a JOIN valid v ON v.group_no=a.group_no LEFT JOIN workflow_plan p ON p.run=?2 AND p.image_pk=a.image_pk");
        let (images, groups, cleanup_count) = conn
            .query_row(&valid, params![gen, category_plan], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(|e| e.to_string())?;
        categories.push(DuplicateCategory {
            kind: kind.into(),
            images,
            groups,
            cleanup_count,
        });
    }
    let cleanup_count=conn.query_row(&format!("SELECT COUNT(*) FROM workflow_plan p JOIN library.images i ON i.image_pk=p.image_pk WHERE p.run=?1 AND p.keeper IS NOT NULL AND {ACTIVE}"),[r.1],|row|row.get(0)).map_err(|e|e.to_string())?;
    let protected_count=conn.query_row(&format!("SELECT COUNT(*) FROM workflow_choices c JOIN fingerprints f ON f.public_hash=c.public_hash AND f.signature=c.signature JOIN library.images i ON i.image_pk=f.image_pk WHERE c.choice='keep' AND {ACTIVE}"),[],|row|row.get(0)).map_err(|e|e.to_string())?;
    if policy.as_deref() == Some(super::evidence::POLICY) {
        let total: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM library.images i WHERE {ACTIVE}"),
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        for (kind, images) in [
            ("recycle", cleanup_count),
            ("retained", total - cleanup_count),
        ] {
            let sql = category_sql(kind, r.0)?;
            let groups=conn.query_row(&format!("SELECT COUNT(*) FROM (SELECT group_no FROM ({sql}) GROUP BY group_no HAVING COUNT(*)>1)"),[gen],|r|r.get(0)).map_err(|e|e.to_string())?;
            categories.push(DuplicateCategory {
                kind: kind.into(),
                images,
                groups,
                cleanup_count: if kind == "recycle" { images } else { 0 },
            });
        }
    }
    // Published headers keep the original scan totals. Live group counts are
    // calculated once with this summary, rather than reloading the live view
    // for every poll. A remaining singleton is no longer a duplicate group.
    scan.exact_groups = categories
        .iter()
        .find(|c| c.kind == "exact")
        .map_or(0, |c| c.groups);
    scan.near_groups = categories
        .iter()
        .find(|c| c.kind == "candidate")
        .map_or(0, |c| c.groups);
    control()?;
    scan.unique_images = conn.query_row(&format!("WITH active AS MATERIALIZED (SELECT m.group_no,m.image_pk FROM members m CROSS JOIN library.images i ON i.image_pk=m.image_pk CROSS JOIN fingerprints f ON f.image_pk=m.image_pk AND m.public_hash=f.public_hash AND m.signature=f.signature WHERE m.generation=?1 AND {ACTIVE}), valid AS (SELECT group_no FROM active GROUP BY group_no HAVING COUNT(*)>1) SELECT COUNT(DISTINCT a.image_pk) FROM active a JOIN valid v ON v.group_no=a.group_no"),[gen],|r|r.get(0)).map_err(|e|e.to_string())?;
    let last_operation_id = conn
        .query_row(
            "SELECT last_operation FROM workflow_runs WHERE generation=?1",
            [gen],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    control()?;
    if membership != cache::membership_token(state) {
        return Err(SUMMARY_DEFERRED.into());
    }
    Ok(DuplicateWorkflowStatus {
        rounds: if policy.as_deref()==Some(cascade::POLICY){Some(cascade::rounds(conn,gen)?)}else{None},
        policy,
        generation: Some(generation),
        revision: revision(gen, &r),
        stage: if r.0 > 0 { "refined" } else { "scanned" }.into(),
        scan,
        categories,
        cleanup_count,
        protected_count,
        last_operation_id,
        errors: vec![],
    })
}
fn empty(scan: DuplicateStatus) -> DuplicateWorkflowStatus {
    DuplicateWorkflowStatus {
        rounds: None,
        policy: None,
        generation: None,
        revision: String::new(),
        stage: "empty".into(),
        scan,
        categories: vec![],
        cleanup_count: 0,
        protected_count: 0,
        last_operation_id: None,
        errors: vec![],
    }
}

#[cfg(test)]
#[path = "workflow_concurrency_tests.rs"]
mod concurrency_tests;
#[cfg(test)]
#[path = "workflow_loading_tests.rs"]
mod loading_tests;
#[cfg(test)]
#[path = "status_performance_tests.rs"]
mod performance_tests;
#[cfg(test)]
#[path = "workflow_readonly_tests.rs"]
mod readonly_tests;

pub fn status(state: &AppState) -> Result<DuplicateWorkflowStatus, String> {
    let scan = cache::published_status(state)?;
    if scan.generation.is_none() {
        return Ok(empty(scan));
    }
    // Old filtered generations discarded the candidate pool. Preserve their cache,
    // but require an explicit first step to rebuild grouping from reusable fingerprints.
    if scan.verification_mode == "verified" {
        let mut value = empty(scan);
        value
            .errors
            .push("旧版结果缺少完整近似候选池。请先全库检索，将复用已有有效指纹。".into());
        return Ok(value);
    }
    let probe = cache::open_existing(state, false)?;
    let key = summary_key(&probe, &scan, state)?;
    if let Some(mut value) = summary_memo(state)
        .lock()
        .map_err(|e| e.to_string())?
        .ready
        .as_ref()
        .filter(|(cached, _)| *cached == key)
        .map(|(_, value)| value.clone())
    {
        refresh_header(&mut value, scan);
        return Ok(value);
    }
    drop(probe);
    let gate = lock(state);
    let _guard = gate.lock().map_err(|e| e.to_string())?;
    let conn = open(state)?;
    summary(&conn, scan, state)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SummaryKey {
    generation: String,
    membership: String,
    stored_membership: String,
    refinement: i64,
    plan: i64,
    revision: i64,
    choices: String,
    policy: Option<String>,
    last_operation: Option<String>,
}

#[derive(Default)]
struct SummaryMemo {
    running: bool,
    ready: Option<(SummaryKey, DuplicateWorkflowStatus)>,
    failure: Option<(SummaryKey, String)>,
}

fn summary_memo(state: &AppState) -> Arc<Mutex<SummaryMemo>> {
    static MEMOS: OnceLock<Mutex<HashMap<std::path::PathBuf, Arc<Mutex<SummaryMemo>>>>> =
        OnceLock::new();
    MEMOS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(state.data_dir.clone())
        .or_default()
        .clone()
}

fn summary_key(
    conn: &Connection,
    scan: &DuplicateStatus,
    state: &AppState,
) -> Result<SummaryKey, String> {
    let generation = scan.generation.as_ref().ok_or("尚无已发布检索结果")?;
    let gen = current(conn, generation)?;
    let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='workflow_runs')", [], |r|r.get(0)).map_err(|e|e.to_string())?;
    let row: Option<(i64, i64, i64, String, Option<String>)> = if exists {
        conn.query_row("SELECT refinement,plan,revision,membership,last_operation FROM workflow_runs WHERE generation=?1",[gen],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional().map_err(|e|e.to_string())?
    } else {
        None
    };
    let (refinement, plan, revision, stored_membership, last_operation) = row.unwrap_or_default();
    let choices = conn
        .query_row(
            "SELECT COALESCE((SELECT value FROM meta WHERE key='choice_revision'),'0')",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let policy = conn
        .query_row(
            "SELECT value FROM meta WHERE key=?1",
            [format!("policy:{gen}")],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    Ok(SummaryKey {
        generation: generation.clone(),
        membership: cache::membership_token(state),
        stored_membership,
        refinement,
        plan,
        revision,
        choices,
        policy,
        last_operation,
    })
}

fn remember_summary(
    conn: &Connection,
    value: &DuplicateWorkflowStatus,
    state: &AppState,
) -> Result<(), String> {
    if value.generation.is_none() {
        return Ok(());
    }
    let key = summary_key(conn, &value.scan, state)?;
    if key.membership != key.stored_membership {
        return Ok(());
    }
    let memo = summary_memo(state);
    let mut memo = memo.lock().map_err(|e| e.to_string())?;
    memo.ready = Some((key, value.clone()));
    memo.failure = None;
    Ok(())
}

const SUMMARY_DEFERRED: &str = "正在执行图库任务，稍后同步已发布摘要";
fn summary_deferred(state: &AppState) -> bool {
    state.maintenance.load(std::sync::atomic::Ordering::SeqCst)
        || state
            .jobs
            .try_list()
            .map(|jobs| {
                jobs.is_none_or(|jobs| {
                    jobs.iter().any(|job| {
                        matches!(job.state.as_str(), "running" | "pausing" | "cancelling")
                            && matches!(
                                job.request.kind.as_str(),
                                "duplicates"
                                    | "duplicateManagement"
                                    | "duplicateUndo"
                                    | "pipeline"
                                    | "scan"
                                    | "cleanup"
                                    | "recycleRestore"
                                    | "recycleDelete"
                            )
                    })
                })
            })
            .unwrap_or(true)
}

fn pending_summary(
    scan: DuplicateStatus,
    key: &SummaryKey,
    error: Option<String>,
) -> DuplicateWorkflowStatus {
    let mut value = empty(scan);
    value.generation = Some(key.generation.clone());
    value.policy = key.policy.clone();
    value.revision = format!(
        "{}:{}:{}:{}",
        key.generation, key.refinement, key.plan, key.revision
    );
    value.last_operation_id = key.last_operation.clone();
    value.stage = if error.is_some() { "error" } else { "loading" }.into();
    value.errors = error.into_iter().collect();
    value
}

fn refresh_header(value: &mut DuplicateWorkflowStatus, mut scan: DuplicateStatus) {
    scan.exact_groups = value.scan.exact_groups;
    scan.near_groups = value.scan.near_groups;
    scan.unique_images = value.scan.unique_images;
    value.scan = scan;
}

/// UI polling reads headers and a complete memo only. A missing/stale summary is
/// synchronized once in the background; incomplete or failed counts are never exposed.
pub fn status_snapshot(state: &AppState) -> Result<DuplicateWorkflowStatus, String> {
    let scan = cache::published_status(state)?;
    if scan.generation.is_none() {
        return Ok(empty(scan));
    }
    if scan.verification_mode == "verified" {
        let mut value = empty(scan);
        value
            .errors
            .push("旧版结果缺少完整近似候选池。请先全库检索，将复用已有有效指纹。".into());
        return Ok(value);
    }
    let conn = cache::open_existing(state, false)?;
    conn.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    let key = summary_key(&conn, &scan, state)?;
    drop(conn);
    let memo = summary_memo(state);
    let mut view = memo.lock().map_err(|e| e.to_string())?;
    if let Some((_, cached)) = view.ready.as_ref().filter(|(cached, _)| *cached == key) {
        let mut value = cached.clone();
        refresh_header(&mut value, scan);
        return Ok(value);
    }
    if let Some((_, error)) = view.failure.as_ref().filter(|(failed, _)| *failed == key) {
        return Ok(pending_summary(scan, &key, Some(error.clone())));
    }
    if view.running || summary_deferred(state) {
        return Ok(pending_summary(scan, &key, None));
    }
    view.running = true;
    let context = state.clone();
    let initial = scan.clone();
    let requested = key.clone();
    let memo_for_work = memo.clone();
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            compute_summary(&context, initial, &requested)
        }))
        .unwrap_or_else(|_| Err("读取已发布摘要时后台同步异常终止，请重试读取".into()));
        let mut memo = memo_for_work.lock().unwrap_or_else(|e| e.into_inner());
        memo.running = false;
        match result {
            Ok((key, value)) => {
                memo.ready = Some((key, value));
                memo.failure = None;
            }
            Err(error) if error == SUMMARY_DEFERRED => {}
            Err(error) => {
                memo.failure = Some((requested, error));
            }
        }
    });
    Ok(pending_summary(scan, &key, None))
}

/// Explicitly retry a failed summary without discarding a ready or in-flight one.
/// Regular polling preserves failures so an invalid cache cannot cause a retry loop.
pub fn retry_status_snapshot(state: &AppState) -> Result<DuplicateWorkflowStatus, String> {
    let memo = summary_memo(state);
    {
        let mut view = memo.lock().map_err(|e| e.to_string())?;
        if !view.running && view.failure.is_some() {
            view.failure = None;
        }
    }
    status_snapshot(state)
}

fn compute_summary(
    state: &AppState,
    scan: DuplicateStatus,
    requested: &SummaryKey,
) -> Result<(SummaryKey, DuplicateWorkflowStatus), String> {
    if summary_deferred(state) {
        return Err(SUMMARY_DEFERRED.into());
    }
    let gate = lock(state);
    let _guard = gate.lock().map_err(|e| e.to_string())?;
    if summary_deferred(state) {
        return Err(SUMMARY_DEFERRED.into());
    }
    let conn = open(state)?;
    // Interrupt a long aggregate if a real library job starts. The planner's
    // control hook handles its row loop; SQLite interruption handles one slow SQL.
    let result = std::thread::scope(|scope| {
        use std::sync::atomic::{AtomicBool, Ordering};
        struct Stop(Arc<AtomicBool>);
        impl Drop for Stop {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let stop = Arc::new(AtomicBool::new(false));
        let interrupted = Arc::new(AtomicBool::new(false));
        let watch_stop = stop.clone();
        let watch_interrupted = interrupted.clone();
        let handle = conn.get_interrupt_handle();
        let watcher = scope.spawn(move || {
            while !watch_stop.load(Ordering::SeqCst) {
                if summary_deferred(state) {
                    watch_interrupted.store(true, Ordering::SeqCst);
                    handle.interrupt();
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        });
        let guard = Stop(stop);
        let value = summary_controlled(&conn, scan, state, &mut || {
            if summary_deferred(state) {
                Err(SUMMARY_DEFERRED.into())
            } else {
                Ok(())
            }
        });
        drop(guard);
        let _ = watcher.join();
        if interrupted.load(Ordering::SeqCst) {
            Err(SUMMARY_DEFERRED.into())
        } else {
            value
        }
    })?;
    let key = summary_key(&conn, &result.scan, state)?;
    if key.membership != requested.membership {
        return Err(SUMMARY_DEFERRED.into());
    }
    Ok((key, result))
}

pub fn run_job(
    options: DuplicateOptions,
    state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>,
    progress: &mut dyn FnMut(&str, &DuplicateCoverage),
) -> Result<(), String> {
    if options.verification_mode == "cascade" {
        let mut stage="准备三轮检索".to_owned();
        let result=cascade::run(options,state,control,&mut |name,coverage|{stage=name.to_owned();progress(name,coverage)});
        return result.map_err(|error|cache::explain_storage_error(error,state,&stage));
    }
    if options.verification_mode == "organize" {
        return discovery::run(options, state, control, progress);
    }
    if options.verification_mode != "refine" {
        return cache::run_job(options, state, control, progress);
    }
    let conn = cache::open_existing(state, false)?;
    let is_current:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM meta WHERE key='policy:'||(SELECT value FROM meta WHERE key='published') AND value=?1)",[super::evidence::POLICY],|r|r.get(0)).map_err(|e|e.to_string())?;
    if is_current {
        return Err("旧版复检入口已停用，请使用“查找可整理副本”；当前结果保留".into());
    }
    refine(state, options.force_recompute, control, progress)
}
fn refine(
    state: &AppState,
    force: bool,
    control: &mut dyn FnMut() -> Result<(), String>,
    progress: &mut dyn FnMut(&str, &DuplicateCoverage),
) -> Result<(), String> {
    let initial = cache::status(state)?;
    let generation = initial.generation.clone().ok_or("请先完成全库检索")?;
    if initial.state != "ready" || initial.verification_mode == "verified" {
        return Err("请先完成全库检索，建立完整候选池".into());
    }
    let gen = generation.parse::<i64>().map_err(|e| e.to_string())?;
    let stamp = cache::membership_token(state);
    let conn = open(state)?;
    let run_id = serial();
    let retained: i64 = conn
        .query_row(
            "SELECT refinement FROM workflow_runs WHERE generation=?1",
            [gen],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .unwrap_or(0);
    cache::controlled_write(&conn, control, || {
        conn.execute("DELETE FROM workflow_class WHERE run<>?1", [retained])
    })
    .map_err(|e| e.to_string())?;
    let mut verifier = verification_cache::Verifier::new(state, force)?;
    let mut coverage = DuplicateCoverage::default();
    coverage.total=conn.query_row("SELECT COUNT(*) FROM members m JOIN groups g ON g.generation=m.generation AND g.group_no=m.group_no JOIN library.images i ON i.image_pk=m.image_pk WHERE m.generation=?1 AND g.kind=1 AND m.image_pk<>g.reference_pk AND i.trashed=0",[gen],|r|r.get(0)).map_err(|e|e.to_string())?;
    progress("复检近似候选", &coverage);
    let mut cursor = (-1_i64, -1_i64);
    let mut last = Instant::now();
    loop {
        control()?;
        current(&conn, &generation)?;
        let rows = {
            let mut stmt=conn.prepare("SELECT m.group_no,m.member_no,g.reference_pk,m.image_pk FROM members m JOIN groups g ON g.generation=m.generation AND g.group_no=m.group_no JOIN library.images i ON i.image_pk=m.image_pk WHERE m.generation=?1 AND g.kind=1 AND (m.group_no,m.member_no)>(?2,?3) AND m.image_pk<>g.reference_pk AND i.trashed=0 ORDER BY m.group_no,m.member_no LIMIT 128").map_err(|e|e.to_string())?;
            let rows = stmt
                .query_map(params![gen, cursor.0, cursor.1], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)?,
                    ))
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            rows
        };
        if rows.is_empty() {
            break;
        }
        for (group, number, reference, pk) in rows {
            control()?;
            let outcome = verifier.check(reference, pk, false, state, control)?;
            let kind = match outcome {
                verification::VerificationOutcome::Identical
                | verification::VerificationOutcome::Equivalent => {
                    coverage.verified_pairs += 1;
                    "near"
                }
                verification::VerificationOutcome::Variant(_) => {
                    coverage.content_variants += 1;
                    "similar"
                }
                verification::VerificationOutcome::Uncertain(_) => {
                    coverage.uncertain_pairs += 1;
                    "uncertain"
                }
            };
            cache::controlled_write(&conn, control, || {
                conn.execute(
                    "INSERT INTO workflow_class VALUES(?1,?2,?3,?4,?5)",
                    params![run_id, group, pk, kind, outcome.reason()],
                )
            })
            .map_err(|e| e.to_string())?;
            coverage.processed += 1;
            cursor = (group, number);
            if last.elapsed().as_millis() >= 200 {
                progress("复检近似候选", &coverage);
                last = Instant::now();
            }
        }
        cache::check_disk(&cache::cache_path(state))?;
    }
    control()?;
    if stamp != cache::membership_token(state) {
        return Err("复检期间图库成员发生变化，保留上次完整结果，请重新复检".into());
    }
    current(&conn, &generation)?;
    cache::controlled_write(&conn, control, || {
        conn.execute(
            "INSERT OR IGNORE INTO workflow_runs(generation) VALUES(?1)",
            [gen],
        )
    })
    .map_err(|e| e.to_string())?;
    progress("合并整理计划", &coverage);
    loop {
        match planning::build_for_job(&conn, gen, run_id, state, control) {
            Err(error) if error == planning::CHOICES_CHANGED => {
                progress("合并最新人工选择", &coverage);
                control()?;
            }
            result => {
                result?;
                break;
            }
        }
    }
    // Only dispose derived results after a complete classification + plan is published.
    if let Err(error) = conn.execute("DELETE FROM workflow_class WHERE run<>?1", [run_id]) {
        eprintln!("[duplicate-class-cleanup] {error}");
    }
    if let Err(error) = conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE)") {
        eprintln!("[duplicate-class-checkpoint] {error}");
    }
    progress("近似复检完成", &coverage);
    Ok(())
}

pub fn prepare(
    request: DuplicateWorkflowRequest,
    state: &AppState,
) -> Result<DuplicateActionPreview, String> {
    prepare_category(request,None,state)
}
pub fn prepare_category(
    request: DuplicateWorkflowRequest,
    kind: Option<&str>,
    state: &AppState,
) -> Result<DuplicateActionPreview,String>{
    busy(state)?;
    let gate = lock(state);
    let _guard = gate.lock().map_err(|e| e.to_string())?;
    let conn = open(state)?;
    let gen = current(&conn, &request.generation)?;
    ensure(&conn, gen, state)?;
    let r = run(&conn, gen)?;
    if r.0 == 0 {
        return Err("请先完成近似图片复检".into());
    }
    let policy: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key=?1",
            [format!("policy:{gen}")],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if policy.as_deref()==Some(cascade::POLICY) {
        if !matches!(kind,Some("exact"|"near")){return Err("请选择整理完全相同图或近似图；类似图默认保留".into());}
    } else if policy.as_deref() != Some(super::evidence::POLICY) {
        return Err(
            "旧版有损判断不能作为新版回收证据，请运行“查找可整理副本”；已有指纹会复用".into(),
        );
    }
    if revision(gen, &r) != request.revision {
        return Err("整理计划已变化，请刷新后重试".into());
    }
    let plan = category_plan(&conn, gen, kind.unwrap_or("all"), state, &mut || Ok(()))?;
    let preview = management::prepare_workflow_category(&request, &conn, plan, kind, state)?;
    conn.execute(
        "UPDATE workflow_runs SET last_operation=?2 WHERE generation=?1",
        params![gen, preview.operation_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(preview)
}

// Callers hold the workflow gate, so a scope cannot race a choice/publication.
fn category_plan(conn: &Connection, generation: i64, kind: &str, state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>) -> Result<i64, String> {
    let cascade: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM meta WHERE key=?1 AND value=?2)",
        params![format!("policy:{generation}"), cascade::POLICY], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if kind == "exact" && cascade { scope::exact_plan(conn, generation, state, control) }
    else { Ok(run(conn, generation)?.1) }
}

pub(super) fn validate_plan(
    request: &DuplicateManagementRequest,
    state: &AppState,
) -> Result<(), String> {
    if !matches!(request.mode.as_str(),"workflow"|"workflowExact"|"workflowNear") {
        return Ok(());
    }
    let conn = open(state)?;
    let gen = current(&conn, &request.generation)?;
    let r = run(&conn, gen)?;
    let policy: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key=?1",
            [format!("policy:{gen}")],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let new_mode=request.mode!="workflow";
    if policy.as_deref() != Some(if new_mode{cascade::POLICY}else{super::evidence::POLICY}) {
        return Err("旧版整理计划需要重新验证副本；该批次已完成项仍可撤销".into());
    }
    let expected=if new_mode{format!("workflow:{}:{}",revision(gen,&r),if request.mode=="workflowExact"{"exact"}else{"near"})}else{format!("workflow:{}",revision(gen,&r))};
    if request.group_id != expected
        || r.3 != cache::membership_token(state)
    {
        return Err("人工选择、图库成员或检索版本已变化，旧计划不能继续，请重新整理".into());
    }
    Ok(())
}

#[cfg(test)]
mod path_tests {
    use super::*;
    use crate::duplicates::tests::Fixture;
    #[test]
    fn complete_node_query_seeks_members_by_image_with_signature_checks() {
        let f = Fixture::new(2);
        f.run().unwrap();
        let conn = open(&f.state).unwrap();
        let sql = format!("EXPLAIN QUERY PLAN {}", planning::nodes_sql());
        let details = conn
            .prepare(&sql)
            .unwrap()
            .query_map(params![1_i64, 0_i64], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let member_lookups = details
            .iter()
            .filter(|s| s.starts_with("SEARCH m ") || s.starts_with("SCAN m"))
            .collect::<Vec<_>>();
        assert_eq!(
            member_lookups.len(),
            2,
            "both correlated checks: {details:?}"
        );
        for lookup in member_lookups {
            assert!(
                lookup.contains("members_image") && lookup.contains("image_pk=?"),
                "must seek current image, never scan the full generation: {details:?}"
            );
        }
        let generation: i64 = conn
            .query_row("SELECT value FROM meta WHERE key='published'", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap()
            .parse()
            .unwrap();
        conn.execute(
            "UPDATE fingerprints SET signature=zeroblob(32) WHERE image_pk=2",
            [],
        )
        .unwrap();
        let pks = conn
            .prepare(&planning::nodes_sql())
            .unwrap()
            .query_map(params![generation, 0_i64], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            pks,
            vec![1],
            "optimization must still reject stale member signatures"
        );
    }
    #[test]
    fn exact_membership_seeks_current_image_before_group_kind() {
        let f = Fixture::new(2);
        f.run().unwrap();
        let conn = cache::open_existing(&f.state, false).unwrap();
        let sql = format!(
            "EXPLAIN QUERY PLAN SELECT f.image_pk,EXISTS({}) FROM fingerprints f",
            planning::EXACT_MEMBERSHIP
        );
        let details = conn
            .prepare(&sql)
            .unwrap()
            .query_map([1_i64], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let image = details
            .iter()
            .position(|s| {
                s.contains("SEARCH m USING COVERING INDEX members_image")
                    && s.contains("image_pk=?")
            })
            .expect("membership must seek the current image");
        let group = details
            .iter()
            .position(|s| s.contains("SEARCH g"))
            .expect("group lookup");
        assert!(
            image < group,
            "must not scan exact groups for every image: {details:?}"
        );
    }
    #[test]
    fn workflow_reads_canonical_special_path_without_mutating_the_library() {
        let mut f = Fixture::new(2);
        let data = f.root.join("图库 # 100% 空格");
        std::fs::rename(&f.state.data_dir, &data).unwrap();
        f.state.data_dir = data.clone();
        f.state.database_path = data.join("illutag.sqlite").canonicalize().unwrap();
        let before = std::fs::read(&f.state.database_path).unwrap();
        f.run().unwrap();
        let info = status(&f.state).unwrap();
        assert_eq!(info.stage, "scanned");
        assert_eq!(info.cleanup_count, 1);
        let conn = open(&f.state).unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM library.images", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert!(
            conn.execute("UPDATE library.images SET is_favorite=1", [])
                .is_err(),
            "attached library must stay read-only"
        );
        assert_eq!(before, std::fs::read(&f.state.database_path).unwrap());
        let uri = readonly_uri(&f.state.database_path).unwrap();
        assert!(uri.contains("%23"));
        assert!(uri.contains("%25"));
        assert!(uri.contains("%20"));
        assert!(!uri.contains("//?/"));
        run_job(
            DuplicateOptions {
                sensitivity: "standard".into(),
                force_recompute: false,
                regroup_only: true,
                verification_mode: "refine".into(),
            },
            &f.state,
            &mut || Ok(()),
            &mut |_, _| {},
        )
        .unwrap();
        let refined = status(&f.state).unwrap();
        assert_eq!(refined.stage, "refined");
        let page = query(
            DuplicateWorkflowQuery {
                generation: refined.generation.unwrap(),
                kind: "exact".into(),
                cursor: None,
                limit: 20,
                group_id: None,
                reanchor: None,
            },
            &f.state,
        )
        .unwrap();
        assert_eq!(page.groups.len(), 1);
        assert_eq!(before, std::fs::read(&f.state.database_path).unwrap());
    }
    #[cfg(windows)]
    #[test]
    fn windows_uri_has_empty_authority_for_verbatim_drive_and_unc_paths() {
        assert_eq!(
            readonly_uri(Path::new(r"\\?\D:\图库 #100%\illutag.sqlite")).unwrap(),
            "file:///D:/%E5%9B%BE%E5%BA%93%20%23100%25/illutag.sqlite?mode=ro"
        );
        assert_eq!(
            readonly_uri(Path::new(r"\\?\UNC\server\share\图库 #100%\illutag.sqlite")).unwrap(),
            "file:////server/share/%E5%9B%BE%E5%BA%93%20%23100%25/illutag.sqlite?mode=ro"
        );
        assert_eq!(
            readonly_uri(Path::new(r"\\server\share\a.sqlite")).unwrap(),
            "file:////server/share/a.sqlite?mode=ro"
        );
        assert!(readonly_uri(Path::new("relative.sqlite")).is_err());
        assert_eq!(
            readonly_uri(Path::new(r"\\?\uNc\server\share\a.sqlite")).unwrap(),
            "file:////server/share/a.sqlite?mode=ro"
        );
    }
}

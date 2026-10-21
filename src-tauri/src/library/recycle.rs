//! Frozen, disk-backed recycle-bin selections; bounded background operations.
use super::*;
use crate::duplicates::{cache, fingerprint};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RecycleSelectionRequest {
    pub query: Option<GalleryQuery>,
    pub image_ids: Vec<String>,
    pub selection_id: Option<String>,
    pub excluded_ids: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RecycleSelection {
    pub selection_id: String,
    pub count: i64,
    pub total_bytes: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RecycleApplyRequest {
    pub selection_id: String,
    pub action: String,
    pub confirmed: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RecycleOperationStatus {
    pub operation_id: String,
    pub action: String,
    pub state: String,
    pub total: i64,
    pub processed: i64,
    pub succeeded: i64,
    pub failed: i64,
    pub skipped: i64,
    pub total_bytes: i64,
    pub errors: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RecycleOperationQuery {
    pub operation_id: String,
    pub cursor: Option<i64>,
    pub limit: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RecycleOperationResult {
    pub image_id: String,
    pub file_name: String,
    pub state: String,
    pub message: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RecycleOperationPage {
    pub items: Vec<RecycleOperationResult>,
    pub next_cursor: Option<i64>,
}

fn writable(state: &AppState) -> Result<(), String> {
    if state.read_only || state.maintenance.load(AtomicOrdering::SeqCst) {
        return Err("图库只读或维护中".into());
    }
    Ok(())
}
fn journal(state: &AppState) -> Result<Connection, String> {
    writable(state)?;
    let path = state.data_dir.join("operations/recycle-management.sqlite");
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let c = Connection::open(path).map_err(|e| e.to_string())?;
    c.busy_timeout(Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    c.execute_batch("PRAGMA journal_mode=WAL;PRAGMA cache_size=-2048;PRAGMA temp_store=FILE;
      CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS selections(id TEXT PRIMARY KEY,action TEXT NOT NULL DEFAULT '',state TEXT NOT NULL DEFAULT 'prepared',
      count INTEGER NOT NULL DEFAULT 0,bytes INTEGER NOT NULL DEFAULT 0,processed INTEGER NOT NULL DEFAULT 0,
      succeeded INTEGER NOT NULL DEFAULT 0,failed INTEGER NOT NULL DEFAULT 0,skipped INTEGER NOT NULL DEFAULT 0,updated INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS items(selection TEXT NOT NULL,pk INTEGER NOT NULL,id TEXT NOT NULL,path TEXT NOT NULL,name TEXT NOT NULL,
      bytes INTEGER NOT NULL,modified INTEGER NOT NULL,state TEXT NOT NULL DEFAULT 'pending',phase TEXT NOT NULL DEFAULT '',
      signature TEXT NOT NULL DEFAULT '',message TEXT NOT NULL DEFAULT '',revoked INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(selection,pk),UNIQUE(selection,id)) WITHOUT ROWID;
      CREATE INDEX IF NOT EXISTS recycle_pending ON items(selection,state,pk);
      CREATE INDEX IF NOT EXISTS recycle_revoke ON items(id,state);").map_err(|e|e.to_string())?;
    let binding = cache::db_binding(state)?;
    c.execute(
        "INSERT OR IGNORE INTO meta VALUES('binding',?1)",
        [&binding],
    )
    .map_err(|e| e.to_string())?;
    let old: String = c
        .query_row("SELECT value FROM meta WHERE key='binding'", [], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    if old != binding {
        return Err("图库已恢复或更换，旧回收批次不可继续".into());
    }
    Ok(c)
}
fn id() -> String {
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        now_ms(),
        SERIAL.fetch_add(1, AtomicOrdering::SeqCst)
    )
}
pub fn prepare(
    request: RecycleSelectionRequest,
    state: &AppState,
) -> Result<RecycleSelection, String> {
    writable(state)?;
    let modes = request.query.is_some() as u8
        + request.selection_id.is_some() as u8
        + (!request.image_ids.is_empty()) as u8;
    if modes != 1 || (!request.excluded_ids.is_empty() && request.selection_id.is_none()) {
        return Err("请选择一种回收站操作范围".into());
    }
    let mut log = journal(state)?;
    let db = cache::library(state)?;
    let selection = id();
    let tx = log.transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO selections(id,updated) VALUES(?1,?2)",
        params![selection, now_ms()],
    )
    .map_err(|e| e.to_string())?;
    let insert="INSERT OR IGNORE INTO items(selection,pk,id,path,name,bytes,modified) VALUES(?1,?2,?3,?4,?5,?6,?7)";
    let add = |pk: i64,
               id: String,
               path: String,
               name: String,
               bytes: i64,
               modified: i64|
     -> Result<(), String> {
        tx.execute(
            insert,
            params![selection, pk, id, path, name, bytes, modified],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    };
    if let Some(parent) = request.selection_id {
        let found: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM selections WHERE id=?1)",
                [&parent],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !found {
            return Err("选择范围不存在，请重新选择".into());
        }
        tx.execute("INSERT INTO items(selection,pk,id,path,name,bytes,modified,revoked) SELECT ?1,pk,id,path,name,bytes,modified,revoked FROM items WHERE selection=?2",params![selection,parent]).map_err(|e|e.to_string())?;
        for excluded in request.excluded_ids {
            tx.execute(
                "DELETE FROM items WHERE selection=?1 AND id=?2",
                params![selection, excluded],
            )
            .map_err(|e| e.to_string())?;
        }
    } else if let Some(mut q) = request.query {
        if q.scope != "trash"
            || q.folder_id.is_some()
            || q.unclassified_only_parent_folder_id.is_some()
        {
            return Err("全量操作仅限应用回收站".into());
        }
        if !q.natural_language.trim().is_empty() {
            // Existing semantic result semantics are bounded to the top 600 matches.
            q.offset = 0;
            q.limit = NATURAL_LANGUAGE_SEARCH_DEFAULT_TOP_K as i64;
            for image in query_gallery(q, state)?.images {
                let pk:Option<i64>=db.query_row("SELECT image_pk FROM images WHERE id=?1 AND source='library' AND trashed<>0",[&image.id],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
                if let Some(pk) = pk {
                    add(
                        pk,
                        image.id,
                        image.path,
                        image.file_name,
                        image.file_size,
                        image.modified_at,
                    )?;
                }
            }
        } else {
            let (sql, values) = super::tag_search::gallery_search_sql(GallerySearchFilters {
                chinese_tag_ens: q.tags,
                english_query: q.english_query,
                file_name_query: q.file_name,
                confidence_min: 0.0,
                confidence_max: 1.0,
                scope: Some("trash".into()),
                folder_id: None,
                unclassified_only_parent_folder_id: None,
            });
            let where_sql = sql
                .split_once("FROM images")
                .ok_or("范围查询无效")?
                .1
                .split(" ORDER BY")
                .next()
                .unwrap();
            let sql=format!("SELECT images.image_pk,images.id,images.path,images.file_name,images.file_size,images.modified_at FROM images {where_sql} AND images.source='library' ORDER BY images.image_pk");
            let mut stmt = db.prepare(&sql).map_err(|e| e.to_string())?;
            let mut rows = stmt
                .query(params_from_iter(values))
                .map_err(|e| e.to_string())?;
            while let Some(r) = rows.next().map_err(|e| e.to_string())? {
                add(
                    r.get(0).map_err(|e| e.to_string())?,
                    r.get(1).map_err(|e| e.to_string())?,
                    r.get(2).map_err(|e| e.to_string())?,
                    r.get(3).map_err(|e| e.to_string())?,
                    r.get(4).map_err(|e| e.to_string())?,
                    r.get(5).map_err(|e| e.to_string())?,
                )?;
            }
        }
    } else {
        for ident in request.image_ids {
            let row=db.query_row("SELECT image_pk,id,path,file_name,file_size,modified_at FROM images WHERE id=?1 AND source='library' AND trashed<>0",[ident],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,i64>(4)?,r.get::<_,i64>(5)?))).optional().map_err(|e|e.to_string())?;
            if let Some((pk, i, p, n, b, m)) = row {
                add(pk, i, p, n, b, m)?;
            }
        }
    }
    let (count, bytes): (i64, i64) = tx
        .query_row(
            "SELECT COUNT(*),COALESCE(SUM(bytes),0) FROM items WHERE selection=?1",
            [&selection],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE selections SET count=?2,bytes=?3 WHERE id=?1",
        params![selection, count, bytes],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(RecycleSelection {
        selection_id: selection,
        count,
        total_bytes: bytes,
    })
}
pub fn apply(
    request: RecycleApplyRequest,
    state: &AppState,
    app: tauri::AppHandle,
) -> Result<crate::jobs::JobRecord, String> {
    apply_request(request, None, state, app)
}
pub(crate) fn retry(
    request: crate::jobs::JobRequest,
    state: &AppState,
    app: tauri::AppHandle,
) -> Result<crate::jobs::JobRecord, String> {
    let action = match request.kind.as_str() {
        "recycleRestore" => "restore",
        "recycleDelete" => "systemTrash",
        _ => return Err("不支持的回收任务类型".into()),
    };
    let apply = RecycleApplyRequest {
        selection_id: request.recycle_operation_id.clone().ok_or("回收批次缺失")?,
        action: action.into(),
        confirmed: true,
    };
    apply_request(apply, Some(request), state, app)
}
fn apply_request(
    request: RecycleApplyRequest,
    original: Option<crate::jobs::JobRequest>,
    state: &AppState,
    app: tauri::AppHandle,
) -> Result<crate::jobs::JobRecord, String> {
    writable(state)?;
    if !["restore", "systemTrash"].contains(&request.action.as_str()) {
        return Err("不支持的回收站操作".into());
    }
    if request.action == "systemTrash" && !request.confirmed {
        return Err("请确认原文件将移入Windows回收站".into());
    }
    let _lock = state.write_lock.lock().map_err(|e| e.to_string())?;
    if state.jobs.list()?.iter().any(|j| {
        j.request.recycle_operation_id.as_deref() == Some(&request.selection_id)
            && ["queued", "running", "pausing", "paused", "cancelling"].contains(&j.state.as_str())
    }) {
        return Err("该回收批次正在运行".into());
    }
    let mut log = journal(state)?;
    let (action, phase, count): (String, String, i64) = log
        .query_row(
            "SELECT action,state,count FROM selections WHERE id=?1",
            [&request.selection_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|_| "回收选择已失效")?;
    if count == 0 {
        return Err("没有可处理的回收项".into());
    }
    if !action.is_empty() && action != request.action {
        return Err("批次操作不可更改".into());
    }
    if phase == "completed" {
        return Err("该批次已完成".into());
    }
    let tx = log.transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE items SET state='pending',message='' WHERE selection=?1 AND state='failed'",
        [&request.selection_id],
    )
    .map_err(|e| e.to_string())?;
    tx.execute("UPDATE selections SET action=?2,state='queued',processed=(SELECT COUNT(*) FROM items WHERE selection=?1 AND state IN ('succeeded','skipped')),failed=0,updated=?3 WHERE id=?1",params![request.selection_id,request.action,now_ms()]).map_err(|e|e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    let operation = request.selection_id.clone();
    let result = crate::jobs::start_job_locked(
        original.unwrap_or_else(|| crate::jobs::JobRequest {
            kind: if request.action == "restore" {
                "recycleRestore"
            } else {
                "recycleDelete"
            }
            .into(),
            thumbnails: false,
            source_folder_id: None,
            duplicate_options: None,
            duplicate_operation_id: None,
            recycle_operation_id: Some(request.selection_id),
        }),
        state,
        app,
    );
    if result.is_err() {
        log.execute(
            "UPDATE selections SET state='interrupted' WHERE id=?1",
            [operation],
        )
        .map_err(|e| e.to_string())?;
    }
    result
}
pub(crate) fn validate_queued(operation: &str, kind: &str, state: &AppState) -> Result<(), String> {
    let c = journal(state)?;
    let (action, phase): (String, String) = c
        .query_row(
            "SELECT action,state FROM selections WHERE id=?1",
            [operation],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| "回收批次不存在")?;
    let expected = if kind == "recycleRestore" {
        "restore"
    } else {
        "systemTrash"
    };
    if action != expected || phase != "queued" {
        return Err("请从回收站确认操作范围后启动或继续批次".into());
    }
    Ok(())
}
pub fn status(operation: &str, state: &AppState) -> Result<RecycleOperationStatus, String> {
    let c = journal(state)?;
    let mut s=c.query_row("SELECT action,state,count,processed,succeeded,failed,skipped,bytes FROM selections WHERE id=?1",[operation],|r|Ok(RecycleOperationStatus{
        operation_id:operation.into(),action:r.get(0)?,state:r.get(1)?,total:r.get(2)?,processed:r.get(3)?,succeeded:r.get(4)?,failed:r.get(5)?,skipped:r.get(6)?,total_bytes:r.get(7)?,errors:vec![],
    })).map_err(|_|"回收批次不存在")?;
    if ["queued", "running"].contains(&s.state.as_str())
        && !state.jobs.list()?.iter().any(|j| {
            j.request.recycle_operation_id.as_deref() == Some(operation)
                && ["queued", "running", "pausing", "paused", "cancelling"].contains(&j.state.as_str())
        })
    {
        s.state = "interrupted".into();
    }
    let mut stmt=c.prepare("SELECT message FROM items WHERE selection=?1 AND state IN ('failed','skipped') LIMIT 8").map_err(|e|e.to_string())?;
    s.errors = stmt
        .query_map([operation], |r| r.get(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(s)
}
pub fn results(q: RecycleOperationQuery, state: &AppState) -> Result<RecycleOperationPage, String> {
    let c = journal(state)?;
    let limit = q.limit.clamp(1, 100);
    let mut stmt=c.prepare("SELECT pk,id,name,state,message FROM items WHERE selection=?1 AND pk>?2 AND state IN ('failed','skipped') ORDER BY pk LIMIT ?3").map_err(|e|e.to_string())?;
    let rows = stmt
        .query_map(
            params![q.operation_id, q.cursor.unwrap_or(-1), limit + 1],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    RecycleOperationResult {
                        image_id: r.get(1)?,
                        file_name: r.get(2)?,
                        state: r.get(3)?,
                        message: r.get(4)?,
                    },
                ))
            },
        )
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let next = if rows.len() > limit as usize {
        Some(rows[limit as usize - 1].0)
    } else {
        None
    };
    Ok(RecycleOperationPage {
        items: rows
            .into_iter()
            .take(limit as usize)
            .map(|(_, r)| r)
            .collect(),
        next_cursor: next,
    })
}

#[derive(Clone)]
struct Item {
    pk: i64,
    id: String,
    path: String,
    name: String,
    bytes: i64,
    modified: i64,
    phase: String,
    signature: String,
    revoked: bool,
}
fn read_item(r: &Row) -> rusqlite::Result<Item> {
    Ok(Item {
        pk: r.get(0)?,
        id: r.get(1)?,
        path: r.get(2)?,
        name: r.get(3)?,
        bytes: r.get(4)?,
        modified: r.get(5)?,
        phase: r.get(6)?,
        signature: r.get(7)?,
        revoked: r.get::<_, i64>(8)? != 0,
    })
}
pub(crate) fn revoke_for_ids(ids: &[String], state: &AppState) -> Result<(), String> {
    if !state
        .data_dir
        .join("operations/recycle-management.sqlite")
        .exists()
    {
        return Ok(());
    }
    let mut c = match journal(state) {
        Ok(c) => c,
        Err(e) if e.contains("图库已恢复或更换") => return Ok(()),
        Err(e) => return Err(e),
    };
    let tx = c.transaction().map_err(|e| e.to_string())?;
    for ident in ids {
        tx.execute(
            "UPDATE items SET revoked=1 WHERE id=?1 AND state IN ('pending','failed')",
            [ident],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())
}
fn live(item: &Item, db: &Connection) -> Result<bool, String> {
    db.query_row("SELECT EXISTS(SELECT 1 FROM images WHERE image_pk=?1 AND id=?2 AND path=?3 AND source='library' AND trashed<>0 AND file_size=?4 AND modified_at=?5)",params![item.pk,item.id,item.path,item.bytes,item.modified],|r|r.get(0)).map_err(|e|e.to_string())
}
fn is_revoked(log: &Connection, operation: &str, pk: i64) -> Result<bool, String> {
    log.query_row(
        "SELECT revoked<>0 FROM items WHERE selection=?1 AND pk=?2",
        params![operation, pk],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}
// One bounded disk-backed lookup build per operation, rather than a library scan per file.
fn prepare_path_lookup(log: &Connection, db: &Connection, operation: &str) -> Result<(), String> {
    db.execute_batch("PRAGMA temp_store=FILE;CREATE TEMP TABLE recycle_selected_paths(path TEXT COLLATE NOCASE PRIMARY KEY) WITHOUT ROWID;CREATE TEMP TABLE recycle_paths(path TEXT COLLATE NOCASE,pk INTEGER,PRIMARY KEY(path,pk)) WITHOUT ROWID;").map_err(|e|e.to_string())?;
    db.execute_batch("BEGIN").map_err(|e| e.to_string())?;
    let work = (|| -> Result<(), String> {
        let mut stmt = log
            .prepare("SELECT path FROM items WHERE selection=?1 AND state='pending'")
            .map_err(|e| e.to_string())?;
        let mut paths = stmt.query([operation]).map_err(|e| e.to_string())?;
        let mut insert = db
            .prepare("INSERT OR IGNORE INTO recycle_selected_paths VALUES(?1)")
            .map_err(|e| e.to_string())?;
        while let Some(path) = paths.next().map_err(|e| e.to_string())? {
            insert
                .execute([path.get::<_, String>(0).map_err(|e| e.to_string())?])
                .map_err(|e| e.to_string())?;
        }
        db.execute("INSERT INTO recycle_paths SELECT i.path,i.image_pk FROM images i CROSS JOIN recycle_selected_paths p ON p.path=i.path COLLATE NOCASE",[]).map_err(|e|e.to_string())?;
        Ok(())
    })();
    if let Err(e) = work {
        let _ = db.execute_batch("ROLLBACK");
        return Err(e);
    }
    db.execute_batch("COMMIT").map_err(|e| e.to_string())
}
fn physical(item: &Item) -> Result<String, String> {
    let s = fingerprint::source_signature(Path::new(&item.path))?;
    if s.file_size != item.bytes as u64 || s.modified_ns / 1_000_000 != item.modified {
        return Err("原文件已变化，已保留此回收项".into());
    }
    serde_json::to_string(&(s.file_size, s.modified_ns, s.identity)).map_err(|e| e.to_string())
}
fn outcomes(
    c: &mut Connection,
    operation: &str,
    results: &[(i64, &str, String)],
) -> Result<(), String> {
    let tx = c.transaction().map_err(|e| e.to_string())?;
    let mut succeeded = 0;
    let mut skipped = 0;
    for (pk, phase, message) in results {
        tx.execute("UPDATE items SET state=?3,message=?4 WHERE selection=?1 AND pk=?2",
            params![operation, pk, phase, message.chars().take(500).collect::<String>()])
            .map_err(|e| e.to_string())?;
        succeeded += (*phase == "succeeded") as i64;
        skipped += (*phase == "skipped") as i64;
    }
    tx.execute("UPDATE selections SET processed=processed+?2,succeeded=succeeded+?3,skipped=skipped+?4,failed=failed+?5,updated=?6 WHERE id=?1",
        params![operation, results.len() as i64, succeeded, skipped, results.len() as i64-succeeded-skipped, now_ms()])
        .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

const SYSTEM_RECYCLE_BATCH: usize = 16;

fn delete_chunk(
    items: &[Item],
    operation: &str,
    log: &mut Connection,
    db: &Connection,
    state: &AppState,
    mover: &mut dyn FnMut(&[PathBuf]) -> Result<(), String>,
    committed: &std::cell::Cell<bool>,
) -> Result<(), String> {
    let mut results = Vec::new();
    let mut ready = Vec::new();
    let mut moved = Vec::new();
    for item in items {
        if item.revoked || is_revoked(log, operation, item.pk)? {
            results.push((item.pk, "skipped", "用户后续操作已改变此回收项".into()));
            continue;
        }
        if !live(item, db)? {
            let absent: bool = db.query_row("SELECT NOT EXISTS(SELECT 1 FROM images WHERE image_pk=?1)", [item.pk], |r| r.get(0)).map_err(|e|e.to_string())?;
            results.push((item.pk, if item.phase == "moved" && absent { "succeeded" } else { "skipped" }, "图片记录已变化或移除".into()));
            continue;
        }
        if item.phase == "moved" {
            moved.push(item);
            continue;
        }
        let validation = (|| -> Result<String, String> {
            let signature = physical(item)?;
            if !item.signature.is_empty() && item.signature != signature {
                return Err("文件身份已变化，未移动原文件".into());
            }
            crate::runtime::check_asset_write(Path::new(&item.path))?;
            let aliases:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM recycle_paths p CROSS JOIN images i ON i.image_pk=p.pk WHERE p.path=?2 COLLATE NOCASE AND p.pk<>?1 AND i.trashed=0)",params![item.pk,item.path],|r|r.get(0)).map_err(|e|e.to_string())?;
            if aliases { return Err("仍有正常图库记录使用该原文件，已保留".into()); }
            Ok(signature)
        })();
        match validation {
            Ok(signature) => ready.push((item, signature)),
            Err(error) => results.push((item.pk, "failed", format!("{}：{error}", item.name))),
        }
    }
    if !ready.is_empty() {
        crate::duplicates::management::revoke_for_ids(&ready.iter().map(|(i,_)| i.id.clone()).collect::<Vec<_>>(), state)?;
        let intent = log.transaction().map_err(|e|e.to_string())?;
        for (item, signature) in &ready {
            intent.execute("UPDATE items SET phase='intent',signature=?3 WHERE selection=?1 AND pk=?2", params![operation, item.pk, signature]).map_err(|e|e.to_string())?;
        }
        intent.commit().map_err(|e|e.to_string())?;
        let mut unique = std::collections::HashSet::new();
        let paths: Vec<_> = ready.iter().filter_map(|(i,_)| unique.insert(i.path.to_lowercase()).then(|| PathBuf::from(&i.path))).collect();
        let move_error = mover(&paths).err();
        // A shell batch can partially complete. Confirm each path instead of
        // treating the whole call as all-or-nothing or retrying moved files.
        let confirmed = log.transaction().map_err(|e|e.to_string())?;
        for (item, _) in &ready {
            match fs::metadata(&item.path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    confirmed.execute("UPDATE items SET phase='moved' WHERE selection=?1 AND pk=?2", params![operation,item.pk]).map_err(|e|e.to_string())?;
                    moved.push(*item);
                }
                result => {
                    let error = match result {
                        Err(e) => format!("无法确认原文件移动结果：{e}"),
                        Ok(_) => move_error.clone().unwrap_or_else(|| "原路径仍存在文件，未删除图库记录".into()),
                    };
                    results.push((item.pk, "failed", format!("{}：{error}", item.name)));
                }
            }
        }
        confirmed.commit().map_err(|e|e.to_string())?;
    }
    db.execute_batch("BEGIN IMMEDIATE").map_err(|e|e.to_string())?;
    let update = (|| -> Result<(), String> {
        for item in moved {
            match fs::metadata(&item.path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                result => {
                    let message = match result { Ok(_) => "原路径仍存在文件，已保留图库记录".into(), Err(e) => format!("无法确认原文件移动结果：{e}") };
                    results.push((item.pk,"failed",message));
                    continue;
                }
            }
            let changed = db.execute("DELETE FROM images WHERE image_pk=?1 AND id=?2 AND path=?3 AND trashed<>0 AND source='library'",params![item.pk,item.id,item.path]).map_err(|e|e.to_string())?;
            results.push((item.pk, if changed == 1 { "succeeded" } else { "skipped" }, "已完成".into()));
        }
        Ok(())
    })();
    if let Err(e) = update { let _ = db.execute_batch("ROLLBACK"); return Err(e); }
    db.execute_batch("COMMIT").map_err(|e|e.to_string())?;
    committed.set(true);
    outcomes(log, operation, &results)
}
fn restore_chunk(
    items: Vec<Item>,
    operation: &str,
    log: &mut Connection,
    db: &Connection,
    state: &AppState,
    binding: &str,
    committed: &std::cell::Cell<bool>,
) -> Result<(), String> {
    let _guard = state.write_lock.lock().map_err(|e| e.to_string())?;
    writable(state)?;
    if cache::db_binding(state)? != binding {
        return Err("图库已替换，停止此回收批次".into());
    }
    let mut ready = Vec::new();
    let mut recovered = Vec::new();
    let mut skipped = Vec::new();
    for item in items {
        if item.revoked || is_revoked(log,operation,item.pk)?{skipped.push(item);}
        else if live(&item,db)?{ready.push(item);}
        else if item.phase=="restoreIntent" && db.query_row("SELECT EXISTS(SELECT 1 FROM images WHERE image_pk=?1 AND id=?2 AND path=?3 AND source='library' AND trashed=0)",params![item.pk,item.id,item.path],|r|r.get::<_,bool>(0)).map_err(|e|e.to_string())?{recovered.push(item);}
        else{skipped.push(item);}
    }
    crate::duplicates::management::revoke_for_ids(
        &ready.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
        state,
    )?;
    let intent = log.transaction().map_err(|e| e.to_string())?;
    for item in &ready {
        intent
            .execute(
                "UPDATE items SET phase='restoreIntent' WHERE selection=?1 AND pk=?2",
                params![operation, item.pk],
            )
            .map_err(|e| e.to_string())?;
    }
    intent.commit().map_err(|e| e.to_string())?;
    db.execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| e.to_string())?;
    let changed = (|| -> Result<(), String> {
        for item in &ready {
            if db.execute("UPDATE images SET trashed=0 WHERE image_pk=?1 AND id=?2 AND path=?3 AND source='library' AND trashed<>0",params![item.pk,item.id,item.path]).map_err(|e|e.to_string())?!=1{return Err("图片状态已变化，分块已回滚".into());}
        }
        Ok(())
    })();
    if let Err(e) = changed {
        let _ = db.execute_batch("ROLLBACK");
        return Err(e);
    }
    db.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    committed.set(true);
    let tx = log.transaction().map_err(|e| e.to_string())?;
    let success = ready.len() + recovered.len();
    for item in ready.iter().chain(recovered.iter()) {
        tx.execute("UPDATE items SET state='succeeded',message='已还原到图库' WHERE selection=?1 AND pk=?2",params![operation,item.pk]).map_err(|e|e.to_string())?;
    }
    for item in &skipped {
        tx.execute("UPDATE items SET state='skipped',message='用户后续操作或图片状态已变化，此项保留' WHERE selection=?1 AND pk=?2",params![operation,item.pk]).map_err(|e|e.to_string())?;
    }
    tx.execute("UPDATE selections SET processed=processed+?2,succeeded=succeeded+?3,skipped=skipped+?4,updated=?5 WHERE id=?1",params![operation,(success+skipped.len()) as i64,success as i64,skipped.len() as i64,now_ms()]).map_err(|e|e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}
pub fn run(
    operation: &str,
    state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>,
    progress: &mut dyn FnMut(&RecycleOperationStatus),
) -> Result<(), String> {
    run_with_batches(operation, state, control, progress, &mut |paths| {
        move_files_to_system_recycle_bin(paths)
    })
}
#[cfg(test)]
fn run_with(
    operation: &str,
    state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>,
    progress: &mut dyn FnMut(&RecycleOperationStatus),
    mover: &mut dyn FnMut(&Path) -> Result<(), String>,
) -> Result<(), String> {
    run_with_batches(operation, state, control, progress, &mut |paths| {
        for path in paths { mover(path)?; }
        Ok(())
    })
}
struct MembershipCommitGuard<'a> {
    state: &'a AppState,
    committed: &'a std::cell::Cell<bool>,
}
impl Drop for MembershipCommitGuard<'_> {
    fn drop(&mut self) {
        if self.committed.get() {
            // The batch may have failed after a chunk was already durable
            // (journal status, progress, final state). Invalidate exactly once
            // here so stale gallery/vector/duplicate caches cannot survive.
            let _ = api::invalidate_library_metadata(self.state);
            invalidate_all_similarity_caches(self.state);
            crate::duplicates::membership_invalidated(self.state);
        }
    }
}
fn run_with_batches(
    operation: &str,
    state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>,
    progress: &mut dyn FnMut(&RecycleOperationStatus),
    mover: &mut dyn FnMut(&[PathBuf]) -> Result<(), String>,
) -> Result<(), String> {
    let mut log = journal(state)?;
    let db = open_database(&state.database_path)?;
    let action: String = log
        .query_row(
            "SELECT action FROM selections WHERE id=?1",
            [operation],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if !["restore", "systemTrash"].contains(&action.as_str()) {
        return Err("回收批次尚未确认".into());
    }
    let committed = std::cell::Cell::new(false);
    let _membership_guard = MembershipCommitGuard {
        state,
        committed: &committed,
    };
    log.execute(
        "UPDATE selections SET state='running',updated=?2 WHERE id=?1",
        params![operation, now_ms()],
    )
    .map_err(|e| e.to_string())?;
    let binding = cache::db_binding(state)?;
    let mut stamp = Instant::now() - Duration::from_secs(1);
    let work = (|| -> Result<(), String> {
        control()?;
        if action == "systemTrash" {
            prepare_path_lookup(&log, &db, operation)?;
        }
        loop {
            control()?;
            let rows = {
                let mut stmt=log.prepare("SELECT pk,id,path,name,bytes,modified,phase,signature,revoked FROM items WHERE selection=?1 AND state='pending' ORDER BY pk LIMIT 64").map_err(|e|e.to_string())?;
                let rows = stmt
                    .query_map([operation], read_item)
                    .map_err(|e| e.to_string())?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())?;
                rows
            };
            if rows.is_empty() {
                break;
            }
            if cache::db_binding(state)? != binding {
                return Err("图库已替换，停止此回收批次".into());
            }
            if action == "restore" {
                restore_chunk(rows, operation, &mut log, &db, state, &binding, &committed)?;
                if stamp.elapsed() >= Duration::from_millis(200) {
                    progress(&status(operation, state)?);
                    stamp = Instant::now();
                }
                continue;
            }
            for chunk in rows.chunks(SYSTEM_RECYCLE_BATCH) {
                control()?;
                let _guard = state.write_lock.lock().map_err(|e| e.to_string())?;
                writable(state)?;
                if cache::db_binding(state)? != binding {
                    return Err("图库已替换，停止此回收批次".into());
                }
                delete_chunk(chunk, operation, &mut log, &db, state, mover, &committed)?;
                drop(_guard);
                if stamp.elapsed() >= Duration::from_millis(200) {
                    progress(&status(operation, state)?);
                    stamp = Instant::now();
                }
            }
        }
        Ok(())
    })();
    let failed: i64 = log
        .query_row(
            "SELECT failed FROM selections WHERE id=?1",
            [operation],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let final_state = if work.is_err() {
        "interrupted"
    } else if failed > 0 {
        "partial"
    } else {
        "completed"
    };
    log.execute(
        "UPDATE selections SET state=?2,updated=?3 WHERE id=?1",
        params![operation, final_state, now_ms()],
    )
    .map_err(|e| e.to_string())?;
    progress(&status(operation, state)?);
    work
}

#[cfg(test)]
#[path = "recycle_tests.rs"]
mod tests;

//! Disk-backed global duplicate runs. Only CHUNK source rows are resident at once.
//!
//! Snapshot, greedy survivor selection, action and undo all have durable cursors.
//! Audit is intentionally not constrained by the old interactive-plan 32 MiB cap:
//! its size is proportional to the published members and affected images.
use super::*;
use std::hash::{BuildHasher, Hasher};

const CHUNK: i64 = 128;
const SNAPSHOT_PAGE_SQL: &str = "SELECT m.group_no,m.member_no,m.image_pk,m.public_hash,m.signature FROM members m JOIN groups g ON g.generation=m.generation AND g.group_no=m.group_no WHERE m.generation=?1 AND (m.group_no,m.member_no)>(?3,?4) AND g.kind=?2 ORDER BY m.group_no,m.member_no LIMIT ?5";
// CROSS JOIN deliberately fixes the loop order. An unconstrained JOIN lets
// SQLite scan every retained keeper for each candidate, which is quadratic.
const KEEPER_CANDIDATES_SQL: &str = "SELECT DISTINCT k.image_pk,k.payload FROM bulk_members m INDEXED BY bulk_members_image CROSS JOIN bulk_group_keepers g ON g.operation=m.operation AND g.group_no=m.group_no CROSS JOIN bulk_keepers k ON k.operation=g.operation AND k.image_pk=g.image_pk WHERE m.operation=?1 AND m.image_pk=?2 ORDER BY k.image_pk";

pub(super) fn schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(r#"
CREATE TABLE IF NOT EXISTS bulk_runs(
 operation INTEGER PRIMARY KEY,stage TEXT NOT NULL,seed TEXT NOT NULL,sensitivity TEXT NOT NULL,
 last_group INTEGER NOT NULL DEFAULT -1,last_member INTEGER NOT NULL DEFAULT -1,
 scanned INTEGER NOT NULL DEFAULT 0,members_total INTEGER NOT NULL DEFAULT 0,
 candidates_total INTEGER NOT NULL DEFAULT 0,planned INTEGER NOT NULL DEFAULT 0,
 total INTEGER NOT NULL DEFAULT 0,processed INTEGER NOT NULL DEFAULT 0,
 succeeded INTEGER NOT NULL DEFAULT 0,failed INTEGER NOT NULL DEFAULT 0,skipped INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS bulk_members(
 operation INTEGER NOT NULL,group_no INTEGER NOT NULL,image_pk INTEGER NOT NULL,
 PRIMARY KEY(operation,group_no,image_pk)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS bulk_members_image ON bulk_members(operation,image_pk,group_no);
CREATE TABLE IF NOT EXISTS bulk_candidates(
 operation INTEGER NOT NULL,image_pk INTEGER NOT NULL,payload TEXT NOT NULL,
 area INTEGER NOT NULL,bytes INTEGER NOT NULL,random_key TEXT NOT NULL,state INTEGER NOT NULL DEFAULT 0,
 PRIMARY KEY(operation,image_pk)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS bulk_candidates_random ON bulk_candidates(operation,state,random_key,image_pk);
CREATE INDEX IF NOT EXISTS bulk_candidates_area ON bulk_candidates(operation,state,area DESC,bytes DESC,image_pk);
CREATE TABLE IF NOT EXISTS bulk_keepers(
 operation INTEGER NOT NULL,image_pk INTEGER NOT NULL,payload TEXT NOT NULL,
 PRIMARY KEY(operation,image_pk)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS bulk_group_keepers(
 operation INTEGER NOT NULL,group_no INTEGER NOT NULL,image_pk INTEGER NOT NULL,
 PRIMARY KEY(operation,group_no,image_pk)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS owners_operation ON owners(operation,image_pk);
CREATE TABLE IF NOT EXISTS bulk_workflow_scope(operation INTEGER PRIMARY KEY,plan INTEGER NOT NULL);
"#).map_err(|e| e.to_string())
}

pub(super) fn is_request(req: &DuplicateManagementRequest) -> bool {
    ["autoExactAll", "autoNearAll"].contains(&req.mode.as_str())
}

pub(super) fn exists(conn: &Connection, id: i64) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM bulk_runs WHERE operation=?1)",
        [id],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

fn insert_descriptor(
    conn: &Connection,
    plan: &Plan,
    stage: &str,
    sensitivity: &str,
    total: i64,
) -> Result<String, String> {
    let id = next_operation_id();
    // RandomState draws fresh process entropy; persist it before selecting any keeper.
    let mut random = std::collections::hash_map::RandomState::new().build_hasher();
    random.write_i64(id);
    let seed = format!("{:016x}", random.finish());
    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| e.to_string())?;
    let result = (|| {
        conn.execute("INSERT INTO operations(id,generation,group_id,action,state,request,created,updated,expires) VALUES(?1,?2,?3,?4,'prepared',?5,?6,?6,?7)",
            params![id,plan.request.generation,plan.request.group_id,plan.request.action,serde_json::to_string(plan).map_err(|e| e.to_string())?,timestamp(),timestamp()+TTL]).map_err(|e| e.to_string())?;
        conn.execute("INSERT INTO bulk_runs(operation,stage,seed,sensitivity,members_total) VALUES(?1,?2,?3,?4,?5)",params![id,stage,seed,sensitivity,total]).map_err(|e| e.to_string())?;
        Ok(id.to_string())
    })();
    finish_transaction(conn, result)
}

pub(super) fn prepare(
    req: DuplicateManagementRequest,
    state: &AppState,
) -> Result<DuplicateActionPreview, String> {
    let exact = req.mode == "autoExactAll";
    if req.action != "trash"
        || req.group_id != if exact { "all-exact" } else { "all-near" }
        || !req.keep_ids.is_empty()
        || !req.protected_ids.is_empty()
        || !req.image_ids.is_empty()
        || req.source_operation_id.is_some()
    {
        return Err("全库自动整理参数无效；仅支持应用回收站".into());
    }
    let source = cache::open_existing(state, false)?;
    ensure_generation(&source, &req.generation)?;
    let sensitivity: String = source
        .query_row(
            "SELECT sensitivity FROM generations WHERE generation=?1",
            [&req.generation],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let total: i64 = source
        .query_row(
            "SELECT COALESCE(SUM(total_members),0) FROM groups WHERE generation=?1 AND kind=?2",
            params![req.generation, if exact { 0 } else { 1 }],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if total == 0 {
        return Err("当前扫描没有此类重复图片".into());
    }
    let plan = Plan {
        request: req.clone(),
        keepers: vec![],
        targets: 0,
        batches: vec![],
    };
    let log = journal(state, true)?;
    let operation_id = insert_descriptor(&log, &plan, "snapshot", &sensitivity, total)?;
    Ok(DuplicateActionPreview {
        operation_id,generation:req.generation,group_id:req.group_id,action:req.action,
        target_count:0,total_bytes:0,protected_count:0,skipped_count:0,
        metadata:DuplicateMetadataSummary::default(),sample:vec![],
        warnings:vec!["后台将遍历本次扫描的全部分组，自动连续分块规划和整理；数量将在任务运行时统计。".into(),
            if exact { "每个完全相同组随机保留一张；保留结果持久化，取消后继续不会重新随机。" } else { "按像素面积从高到低保留；只回收与最终保留图直接匹配的版本，保护跨组保留图。" }.into(),
            "只移入应用回收站，原文件及元数据保留；整次操作可一键撤销。审计日志随实际处理规模增长。".into()],
        expires_at:timestamp()+TTL,requires_system_confirmation:false,
    })
}

fn ensure_generation(conn: &Connection, generation: &str) -> Result<(), String> {
    let current: String = conn
        .query_row("SELECT value FROM meta WHERE key='published'", [], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    if current != generation {
        return Err("查重结果已变化，请重新启动全库整理".into());
    }
    Ok(())
}

pub(super) fn prepare_workflow_category(request:&DuplicateWorkflowRequest,source:&Connection,plan_id:i64,kind:Option<&str>,state:&AppState)->Result<DuplicateActionPreview,String>{
    let category=kind.unwrap_or("all");
    if !matches!(category,"all"|"exact"|"near"){return Err("整理分类无效".into());}
    let (total,total_bytes):(i64,i64)=source.query_row("SELECT COUNT(*),COALESCE(SUM(f.size),0) FROM workflow_plan p JOIN fingerprints f ON f.image_pk=p.image_pk WHERE p.run=?1 AND p.keeper IS NOT NULL AND (?2='all' OR p.kind=?2)",params![plan_id,category],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|e|e.to_string())?;
    if total == 0 {
        return Err("当前没有待清理图片".into());
    }
    let req = DuplicateManagementRequest {
        generation: request.generation.clone(),
        group_id: if category=="all"{format!("workflow:{}",request.revision)}else{format!("workflow:{}:{category}",request.revision)},
        mode: match category{"exact"=>"workflowExact","near"=>"workflowNear",_=>"workflow"}.into(),
        keep_ids: vec![],
        protected_ids: vec![],
        image_ids: vec![],
        action: "trash".into(),
        source_operation_id: None,
    };
    let descriptor = Plan {
        request: req.clone(),
        keepers: vec![],
        targets: total,
        batches: vec![],
    };
    let log = journal(state, true)?;
    let operation_id = insert_descriptor(&log, &descriptor, "workflowSnapshot", "standard", total)?;
    log.execute("INSERT INTO bulk_workflow_scope VALUES(?1,?2)", params![op_id(&operation_id)?, plan_id])
        .map_err(|e| e.to_string())?;
    Ok(DuplicateActionPreview {
        operation_id,
        generation: req.generation,
        group_id: req.group_id,
        action: "trash".into(),
        target_count: total,
        total_bytes,
        protected_count: 0,
        skipped_count: 0,
        metadata: DuplicateMetadataSummary::default(),
        sample: vec![],
        warnings: vec![
            match category {
                "exact" => "仅整理原生尺寸与全部像素一致的组件，每组件保留至少一张；人工保留优先。只移入应用回收站，可整批撤销。",
                "near" => "仅整理与最终保留图直接通过复检的近似版本；人工保留优先。只移入应用回收站，可整批撤销。",
                _ => "完全相同和已复检副本统一去重整理；人工保留优先。只移入应用回收站，可整批撤销。",
            }.into(),
        ],
        expires_at: timestamp() + TTL,
        requires_system_confirmation: false,
    })
}

fn snapshot_workflow(
    conn: &Connection,
    id: i64,
    plan: &Plan,
    state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>,
    progress: &mut dyn FnMut(&DuplicateOperationStatus),
) -> Result<(), String> {
    let source = cache::open_existing(state, false)?;
    let db = cache::library(state)?;
    let plan_id: i64 = if let Some(pointer) = conn.query_row(
        "SELECT plan FROM bulk_workflow_scope WHERE operation=?1", [id], |r| r.get::<_, i64>(0),
    ).optional().map_err(|e| e.to_string())? { pointer } else if plan.request.mode == "workflowExact" {
        return Err("完全相同分类计划缺少快照指针，请重新整理".into());
    } else { source
        .query_row(
            "SELECT plan FROM workflow_runs WHERE generation=?1",
            [&plan.request.generation],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())? };
    let category=match plan.request.mode.as_str(){"workflowExact"=>"exact","workflowNear"=>"near",_=>"all"};
    loop {
        control()?;
        super::super::workflow::validate_plan(&plan.request, state)?;
        let last: i64 = conn
            .query_row(
                "SELECT last_member FROM bulk_runs WHERE operation=?1",
                [id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let rows = {
            let mut stmt=source.prepare("SELECT p.image_pk,p.keeper,p.manual,f.public_hash,f.signature FROM workflow_plan p JOIN fingerprints f ON f.image_pk=p.image_pk WHERE p.run=?1 AND p.image_pk>?2 AND p.keeper IS NOT NULL AND (?3='all' OR p.kind=?3) ORDER BY p.image_pk LIMIT 128").map_err(|e|e.to_string())?;
            let rows = stmt
                .query_map(params![plan_id, last,category], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, Vec<u8>>(3)?,
                        r.get::<_, Vec<u8>>(4)?,
                    ))
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            rows
        };
        if rows.is_empty() {
            conn.execute(
                "UPDATE bulk_runs SET stage='applying' WHERE operation=?1",
                [id],
            )
            .map_err(|e| e.to_string())?;
            break;
        }
        conn.execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| e.to_string())?;
        let result = (|| -> Result<(), String> {
            for (pk, keeper, manual, public, signature) in &rows {
                let (mut item, _) =
                    snapshot_item(&source, &db, *pk, public.clone(), signature.clone())?;
                item.keeper_pks = vec![*keeper];
                item.manual_review = *manual != 0;
                let (kp, ks): (Vec<u8>, Vec<u8>) = source
                    .query_row(
                        "SELECT public_hash,signature FROM fingerprints WHERE image_pk=?1",
                        [keeper],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .map_err(|e| e.to_string())?;
                let (keep, _) = snapshot_item(&source, &db, *keeper, kp, ks)?;
                let keeper_is_target: bool = source
                    .query_row(
                        "SELECT keeper IS NOT NULL FROM workflow_plan WHERE run=?1 AND image_pk=?2",
                        params![plan_id, keeper],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                if keeper_is_target {
                    return Err("整理计划的保留关系冲突，已停止".into());
                }
                conn.execute(
                    "INSERT OR IGNORE INTO bulk_keepers VALUES(?1,?2,?3)",
                    params![
                        id,
                        keeper,
                        serde_json::to_string(&keep).map_err(|e| e.to_string())?
                    ],
                )
                .map_err(|e| e.to_string())?;
                conn.execute(
                    "INSERT INTO items(operation,ordinal,image_pk,payload) VALUES(?1,?2,?2,?3)",
                    params![
                        id,
                        pk,
                        serde_json::to_string(&item).map_err(|e| e.to_string())?
                    ],
                )
                .map_err(|e| e.to_string())?;
            }
            conn.execute("UPDATE bulk_runs SET last_member=?2,scanned=scanned+?3,total=total+?3,candidates_total=candidates_total+?3,planned=planned+?3 WHERE operation=?1",params![id,rows.last().unwrap().0,rows.len() as i64]).map_err(|e|e.to_string())?;
            Ok(())
        })();
        finish_transaction(conn, result)?;
        checkpoint(conn, id)?;
        progress(&operation_status(&id.to_string(), state)?);
    }
    Ok(())
}

pub(super) fn prepare_undo(
    operation: &str,
    mut plan: Plan,
    state: &AppState,
) -> Result<String, String> {
    let conn = journal(state, false)?;
    let source = op_id(operation)?;
    let total: i64 = conn.query_row("SELECT COUNT(*) FROM items i JOIN owners o ON o.image_pk=i.image_pk AND o.operation=i.operation AND o.public_hash=json_extract(i.payload,'$.public_hash') WHERE i.operation=?1 AND i.state='succeeded' AND i.file_moved=0",[source],|r|r.get(0)).map_err(|e|e.to_string())?;
    if total == 0 {
        return Err("该批次没有仍可撤销的成功项".into());
    }
    plan.request.action = "restore".into();
    plan.request.source_operation_id = Some(operation.into());
    insert_descriptor(&conn, &plan, "undoSnapshot", "standard", total)
}

type Counts = (i64, i64, i64, i64, i64);
pub(super) fn status_counts(conn: &Connection, id: i64) -> Result<Option<Counts>, String> {
    conn.query_row("SELECT stage,scanned,members_total,candidates_total,planned,total,processed,succeeded,failed,skipped FROM bulk_runs WHERE operation=?1",[id],|r|{
        let stage:String=r.get(0)?;
        let (total,processed)=match stage.as_str(){
            "snapshot" | "workflowSnapshot" | "undoSnapshot" | "planning" => (0,0),
            _ => (r.get(5)?,r.get(6)?),
        };
        Ok((total,processed,r.get(7)?,r.get(8)?,r.get(9)?))
    }).optional().map_err(|e|e.to_string())
}

pub(super) fn stage_progress(
    conn: &Connection,
    id: i64,
) -> Result<Option<(String, i64, i64)>, String> {
    conn.query_row("SELECT stage,scanned,members_total,candidates_total,planned,total,processed FROM bulk_runs WHERE operation=?1", [id], |r| {
        let stage: String = r.get(0)?;
        let (processed,total) = match stage.as_str() {
            "snapshot" | "workflowSnapshot" | "undoSnapshot" => (r.get(1)?,r.get(2)?),
            "planning" => (r.get(4)?,r.get(3)?),
            _ => (r.get(6)?,r.get(5)?),
        };
        Ok((stage,processed,total))
    }).optional().map_err(|e|e.to_string())
}

fn finish_transaction<T>(conn: &Connection, result: Result<T, String>) -> Result<T, String> {
    match result {
        Ok(value) => {
            conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
            Ok(value)
        }
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

fn checkpoint(conn: &Connection, id: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE operations SET updated=?2 WHERE id=?1",
        params![id, timestamp()],
    )
    .map_err(|e| e.to_string())?;
    conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE)")
        .map_err(|e| e.to_string())
}

// Cached payload creation does no file IO. Files are revalidated before survivor
// selection and again immediately before every mutation.
fn snapshot_item(
    source: &Connection,
    db: &Connection,
    pk: i64,
    public: Vec<u8>,
    signature: Vec<u8>,
) -> Result<(Item, i64), String> {
    let image = main_image(db, pk)?.unwrap_or_else(|| GalleryImage {
        id: format!("missing:{pk}"),
        path: String::new(),
        thumbnail_path: None,
        file_name: format!("已移除图片 {pk}"),
        ext: String::new(),
        width: 0,
        height: 0,
        file_size: 0,
        modified_at: 0,
        imported_at: 0,
        folder_id: 0,
        missing: true,
        trashed: false,
        is_favorite: false,
        source: String::new(),
    });
    let row=source.query_row("SELECT sha,phash,dhash,color,width,height,low_info FROM fingerprints WHERE image_pk=?1",[pk],|r|Ok((r.get::<_,Option<Vec<u8>>>(0)?,r.get::<_,Option<Vec<u8>>>(1)?,r.get::<_,Option<Vec<u8>>>(2)?,r.get::<_,Option<Vec<u8>>>(3)?,r.get::<_,u32>(4)?,r.get::<_,u32>(5)?,r.get::<_,i64>(6)?))).optional().map_err(|e|e.to_string())?;
    let (sha, near, area) = if let Some((sha, p, d, c, w, h, low)) = row {
        let near = match (p, d, c) {
            (Some(p), Some(d), Some(c)) => {
                p.try_into()
                    .ok()
                    .zip(d.try_into().ok())
                    .map(|(p, d)| NearData {
                        p: u64::from_le_bytes(p),
                        d: u64::from_le_bytes(d),
                        c,
                        w,
                        h,
                        low: low != 0,
                    })
            }
            _ => None,
        };
        (
            sha.map(|s| hex(&s)).unwrap_or_default(),
            near,
            (u64::from(w) * u64::from(h)).min(i64::MAX as u64) as i64,
        )
    } else {
        (String::new(), None, 0)
    };
    Ok((
        Item {
            pk,
            image,
            public_hash: hex(&public),
            signature: hex(&signature),
            sha,
            near,
            identity: String::new(),
            canonical: String::new(),
            keeper_pks: vec![],
            manual_review: false,
        },
        area,
    ))
}

fn validate_record(item: &Item, db: &Connection, trashed: bool) -> Result<(), String> {
    let current = main_image(db, item.pk)?.ok_or("图库记录已移除")?;
    if digest(current.id.as_bytes()) != item.public_hash
        || current.path != item.image.path
        || current.source != item.image.source
        || current.source == "reference"
        || current.trashed != trashed
        || current.missing
    {
        return Err("图片身份、路径或回收状态已变化".into());
    }
    if item.sha.is_empty() {
        return Err("扫描文件哈希缺失".into());
    }
    Ok(())
}

fn hydrate(
    mut item: Item,
    db: &Connection,
    trashed: bool,
    state: &AppState,
) -> Result<Item, String> {
    validate_record(&item, db, trashed)?;
    let source = fingerprint::source_signature(Path::new(&item.image.path)).map_err(|error| {
        cache::mark_stale(state);
        error
    })?;
    if hex(&cache::signature(&item.image.id, &source)) != item.signature {
        cache::mark_stale(state);
        return Err("源文件已变化、离线或指纹缺失".into());
    }
    item.identity = source.identity;
    // Soft trash changes only a database flag. Physical aliases/canonical paths
    // are irrelevant here; identity + current path + source signature are checked.
    Ok(item)
}
fn verify_source(
    item: &Item,
    state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    let checked = hash_file(item, control);
    if checked
        .as_ref()
        .err()
        .is_some_and(|e| !e.contains("取消") && !e.contains("cancel"))
    {
        cache::mark_stale(state);
    }
    checked
}

fn snapshot(
    conn: &Connection,
    id: i64,
    plan: &Plan,
    state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>,
    progress: &mut dyn FnMut(&DuplicateOperationStatus),
) -> Result<(), String> {
    let source = cache::open_existing(state, false)?;
    let db = cache::library(state)?;
    let kind = if plan.request.mode == "autoExactAll" {
        0
    } else {
        1
    };
    loop {
        control()?;
        ensure_generation(&source, &plan.request.generation)?;
        let (last_group, last_member, seed): (i64, i64, String) = conn
            .query_row(
                "SELECT last_group,last_member,seed FROM bulk_runs WHERE operation=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(|e| e.to_string())?;
        let rows = {
            let mut stmt = source
                .prepare_cached(SNAPSHOT_PAGE_SQL)
                .map_err(|e| e.to_string())?;
            let out = stmt
                .query_map(
                    params![
                        plan.request.generation,
                        kind,
                        last_group,
                        last_member,
                        CHUNK
                    ],
                    |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, i64>(1)?,
                            r.get::<_, i64>(2)?,
                            r.get::<_, Vec<u8>>(3)?,
                            r.get::<_, Vec<u8>>(4)?,
                        ))
                    },
                )
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            out
        };
        if rows.is_empty() {
            conn.execute(
                "UPDATE bulk_runs SET stage='planning' WHERE operation=?1",
                [id],
            )
            .map_err(|e| e.to_string())?;
            break;
        }
        conn.execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| e.to_string())?;
        let result = (|| {
            let mut added = 0;
            for (group, _, pk, public, signature) in &rows {
                let present:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM bulk_candidates WHERE operation=?1 AND image_pk=?2)",params![id,pk],|r|r.get(0)).map_err(|e|e.to_string())?;
                if !present {
                    let (item, area) =
                        snapshot_item(&source, &db, *pk, public.clone(), signature.clone())?;
                    if !item.image.trashed && item.image.source != "reference" {
                        let random = digest(format!("{seed}:{pk}").as_bytes());
                        conn.execute("INSERT INTO bulk_candidates(operation,image_pk,payload,area,bytes,random_key) VALUES(?1,?2,?3,?4,?5,?6)",params![id,pk,serde_json::to_string(&item).map_err(|e|e.to_string())?,area,item.image.file_size,random]).map_err(|e|e.to_string())?;
                        added += 1;
                    }
                }
                conn.execute(
                    "INSERT OR IGNORE INTO bulk_members VALUES(?1,?2,?3)",
                    params![id, group, pk],
                )
                .map_err(|e| e.to_string())?;
            }
            let last = rows.last().unwrap();
            conn.execute("UPDATE bulk_runs SET scanned=scanned+?2,candidates_total=candidates_total+?3,last_group=?4,last_member=?5 WHERE operation=?1",params![id,rows.len() as i64,added,last.0,last.1]).map_err(|e|e.to_string())?;
            Ok(())
        })();
        finish_transaction(conn, result)?;
        checkpoint(conn, id)?;
        progress(&operation_status(&id.to_string(), state)?);
    }
    Ok(())
}

fn note(conn: &Connection, id: i64, error: &str) -> Result<(), String> {
    let old: String = conn
        .query_row("SELECT errors FROM operations WHERE id=?1", [id], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    let mut errors: Vec<String> = serde_json::from_str(&old).unwrap_or_default();
    if errors.len() < 20 {
        errors.push(error.chars().take(240).collect());
        conn.execute(
            "UPDATE operations SET errors=?2 WHERE id=?1",
            params![id, serde_json::to_string(&errors).unwrap()],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn insert_skipped(conn: &Connection, id: i64, item: &Item, error: &str) -> Result<(), String> {
    conn.execute("INSERT INTO items(operation,ordinal,image_pk,payload,state,code,message) VALUES(?1,?2,?2,?3,'skipped','sourceChanged',?4)",params![id,item.pk,serde_json::to_string(item).map_err(|e|e.to_string())?,error.chars().take(240).collect::<String>()]).map_err(|e|e.to_string())?;
    conn.execute("UPDATE bulk_runs SET total=total+1,processed=processed+1,skipped=skipped+1 WHERE operation=?1",[id]).map_err(|e|e.to_string())?;
    note(conn, id, &format!("{}：{error}", item.image.file_name))
}

fn plan_candidates(
    conn: &Connection,
    id: i64,
    plan: &Plan,
    state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>,
    progress: &mut dyn FnMut(&DuplicateOperationStatus),
) -> Result<(), String> {
    let db = cache::library(state)?;
    let sensitivity: String = conn
        .query_row(
            "SELECT sensitivity FROM bulk_runs WHERE operation=?1",
            [id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let exact = plan.request.mode == "autoExactAll";
    let mut verifier = super::super::verification_cache::Verifier::new(state, false)?;
    let mut last_report = timestamp();
    loop {
        control()?;
        super::super::workflow::validate_plan(&plan.request, state)?;
        let rows = {
            let order = if exact {
                "random_key,image_pk"
            } else {
                "area DESC,bytes DESC,image_pk"
            };
            let mut stmt=conn.prepare(&format!("SELECT payload FROM bulk_candidates WHERE operation=?1 AND state=0 ORDER BY {order} LIMIT {CHUNK}")).map_err(|e|e.to_string())?;
            let out = stmt
                .query_map([id], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            out
        };
        if rows.is_empty() {
            conn.execute(
                "UPDATE bulk_runs SET stage='applying' WHERE operation=?1",
                [id],
            )
            .map_err(|e| e.to_string())?;
            break;
        }
        let mut prepared = Vec::new();
        for payload in rows {
            control()?;
            let original: Item = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
            let hydrated = hydrate(original.clone(), &db, false, state);
            prepared.push((original, hydrated));
        }
        for (original, hydrated) in prepared {
            control()?;
            let mut chosen = None;
            if let Ok(candidate) = &hydrated {
                {
                    let mut stmt = conn
                        .prepare_cached(KEEPER_CANDIDATES_SQL)
                        .map_err(|e| e.to_string())?;
                    let mut keepers = stmt
                        .query(params![id, candidate.pk])
                        .map_err(|e| e.to_string())?;
                    while let Some(row) = keepers.next().map_err(|e| e.to_string())? {
                        let keeper: Item = serde_json::from_str(
                            &row.get::<_, String>(1).map_err(|e| e.to_string())?,
                        )
                        .map_err(|e| e.to_string())?;
                        if matches(
                            &candidate,
                            std::slice::from_ref(&keeper),
                            if exact { "exact" } else { "near" },
                            &sensitivity,
                        ) && verifier
                            .check(candidate.pk, keeper.pk, exact, state, control)?
                            .is_equivalent()
                        {
                            chosen = Some(keeper.pk);
                            break;
                        }
                    }
                }
            }
            // Source decoding/byte comparison above must never retain a write transaction.
            conn.execute_batch("BEGIN IMMEDIATE")
                .map_err(|e| e.to_string())?;
            let result = (|| {
                let mut added_targets = 0_i64;
                match hydrated {
                    Err(error) => insert_skipped(conn, id, &original, &error)?,
                    Ok(mut candidate) => {
                        if let Some(keeper) = chosen {
                            candidate.keeper_pks = vec![keeper];
                            conn.execute("INSERT INTO items(operation,ordinal,image_pk,payload) VALUES(?1,?2,?2,?3)",params![id,candidate.pk,serde_json::to_string(&candidate).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
                            added_targets += 1;
                        } else {
                            conn.execute(
                                "INSERT INTO bulk_keepers VALUES(?1,?2,?3)",
                                params![
                                    id,
                                    candidate.pk,
                                    serde_json::to_string(&candidate).map_err(|e| e.to_string())?
                                ],
                            )
                            .map_err(|e| e.to_string())?;
                            conn.execute("INSERT INTO bulk_group_keepers SELECT operation,group_no,image_pk FROM bulk_members WHERE operation=?1 AND image_pk=?2",params![id,candidate.pk]).map_err(|e|e.to_string())?;
                        }
                    }
                }
                conn.execute(
                    "UPDATE bulk_candidates SET state=1 WHERE operation=?1 AND image_pk=?2",
                    params![id, original.pk],
                )
                .map_err(|e| e.to_string())?;
                conn.execute(
                    "UPDATE bulk_runs SET total=total+?2,planned=planned+?3 WHERE operation=?1",
                    params![id, added_targets, 1_i64],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            })();
            finish_transaction(conn, result)?;
            if timestamp() - last_report >= 200 {
                progress(&operation_status(&id.to_string(), state)?);
                last_report = timestamp();
            }
        }
        checkpoint(conn, id)?;
        progress(&operation_status(&id.to_string(), state)?);
    }
    Ok(())
}

fn undo_snapshot(
    conn: &Connection,
    id: i64,
    plan: &Plan,
    state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>,
    progress: &mut dyn FnMut(&DuplicateOperationStatus),
) -> Result<(), String> {
    let source = op_id(
        plan.request
            .source_operation_id
            .as_deref()
            .ok_or("撤销来源缺失")?,
    )?;
    loop {
        control()?;
        let cursor: i64 = conn
            .query_row(
                "SELECT last_member FROM bulk_runs WHERE operation=?1",
                [id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let rows = {
            let mut stmt=conn.prepare("SELECT i.ordinal,i.image_pk,i.payload FROM items i JOIN owners o ON o.image_pk=i.image_pk AND o.operation=i.operation AND o.public_hash=json_extract(i.payload,'$.public_hash') WHERE i.operation=?1 AND i.ordinal>?2 AND i.state='succeeded' AND i.file_moved=0 ORDER BY i.ordinal LIMIT ?3").map_err(|e|e.to_string())?;
            let out = stmt
                .query_map(params![source, cursor, CHUNK], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            out
        };
        if rows.is_empty() {
            conn.execute(
                "UPDATE bulk_runs SET stage='applying' WHERE operation=?1",
                [id],
            )
            .map_err(|e| e.to_string())?;
            break;
        }
        conn.execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| e.to_string())?;
        let result = (|| {
            for (ordinal, pk, payload) in &rows {
                conn.execute(
                    "INSERT INTO items(operation,ordinal,image_pk,payload) VALUES(?1,?2,?3,?4)",
                    params![id, ordinal, pk, payload],
                )
                .map_err(|e| e.to_string())?;
            }
            conn.execute("UPDATE bulk_runs SET scanned=scanned+?2,total=total+?2,last_member=?3 WHERE operation=?1",params![id,rows.len() as i64,rows.last().unwrap().0]).map_err(|e|e.to_string())?;
            Ok(())
        })();
        finish_transaction(conn, result)?;
        checkpoint(conn, id)?;
        progress(&operation_status(&id.to_string(), state)?);
    }
    Ok(())
}

fn skip_pending(conn: &Connection, id: i64, n: i64, error: &str) -> Result<(), String> {
    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| e.to_string())?;
    let result = (|| {
        item_outcome(conn, id, n, "skipped", "sourceChanged", error, false, false)?;
        conn.execute(
            "UPDATE bulk_runs SET processed=processed+1,skipped=skipped+1 WHERE operation=?1",
            [id],
        )
        .map_err(|e| e.to_string())?;
        note(conn, id, error)
    })();
    finish_transaction(conn, result)
}

fn complete_items(
    conn: &Connection,
    id: i64,
    items: &[(i64, Item)],
    restore: bool,
    source: Option<i64>,
) -> Result<(), String> {
    if items.is_empty() {
        return Ok(());
    }
    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| e.to_string())?;
    let result = (|| {
        for (n, item) in items {
            if restore {
                // Keep ownership until the main restore has committed. A crash before
                // this transaction leaves an unambiguous, resumable restoreIntent.
                conn.execute(
                    "DELETE FROM owners WHERE image_pk=?1 AND public_hash=?2 AND operation=?3",
                    params![item.pk, item.public_hash, source],
                )
                .map_err(|e| e.to_string())?;
            }
            item_outcome(
                conn,
                id,
                *n,
                "succeeded",
                if restore { "restored" } else { "trashed" },
                if restore {
                    "已还原到图库"
                } else {
                    "已移入应用回收站"
                },
                false,
                true,
            )?;
        }
        conn.execute(
            "UPDATE bulk_runs SET processed=processed+?2,succeeded=succeeded+?2 WHERE operation=?1",
            params![id, items.len() as i64],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })();
    finish_transaction(conn, result)
}

fn apply_items(
    conn: &Connection,
    id: i64,
    plan: &Plan,
    state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>,
    progress: &mut dyn FnMut(&DuplicateOperationStatus),
) -> Result<(), String> {
    let binding: String = conn
        .query_row("SELECT value FROM meta WHERE key='binding'", [], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    let db = main_write(state, &binding)?;
    let restore = plan.request.action == "restore";
    let mut verifier = if restore {
        None
    } else {
        Some(super::super::verification_cache::Verifier::new(
            state, false,
        )?)
    };
    let source = if restore {
        Some(op_id(
            plan.request
                .source_operation_id
                .as_deref()
                .ok_or("撤销来源缺失")?,
        )?)
    } else {
        None
    };
    loop {
        control()?;
        if !restore {
            super::super::workflow::validate_plan(&plan.request, state)?;
        }
        let rows = {
            let mut stmt=conn.prepare("SELECT ordinal,payload,code FROM items WHERE operation=?1 AND state='pending' ORDER BY ordinal LIMIT ?2").map_err(|e|e.to_string())?;
            let out = stmt
                .query_map(params![id, CHUNK], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            out
        };
        if rows.is_empty() {
            break;
        }
        let mut ready = Vec::<(i64, Item, Option<i64>)>::new();
        let mut recovered = Vec::<(i64, Item)>::new();
        let mut verified_keepers = std::collections::HashMap::<i64, Result<Item, String>>::new();
        // Hash and control callbacks stay outside the library write lock and SQL transactions.
        for (n, payload, code) in rows {
            control()?;
            let item: Item = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
            let committed = if restore {
                code == "restoreIntent"
                    && ownership(conn, &item)? == source
                    && hydrate(item.clone(), &db, false, state).is_ok()
            } else {
                code == "softIntent"
                    && ownership(conn, &item)? == Some(id)
                    && hydrate(item.clone(), &db, true, state).is_ok()
            };
            if committed {
                recovered.push((n, item));
                continue;
            }
            let checked = (|| -> Result<Option<i64>, String> {
                // hash_file already checks the source signature before and after
                // reading. Avoid another file open before that same verification.
                validate_record(&item, &db, restore)?;
                let keep = if restore {
                    if ownership(conn, &item)? != source {
                        return Err("撤销归属已被后续操作移除，此项跳过".into());
                    }
                    None
                } else {
                    let keep_pk = *item.keeper_pks.first().ok_or("保留图记录缺失")?;
                    if !verified_keepers.contains_key(&keep_pk) {
                        let checked_keeper = (|| -> Result<Item, String> {
                            let payload: String = conn
                        .query_row(
                            "SELECT payload FROM bulk_keepers WHERE operation=?1 AND image_pk=?2",
                            params![id, keep_pk],
                            |r| r.get(0),
                        )
                        .map_err(|e| e.to_string())?;
                            let keep: Item =
                                serde_json::from_str(&payload).map_err(|e| e.to_string())?;
                            validate_record(&keep, &db, false)?;
                            verify_source(&keep, state, control)?;
                            Ok(keep)
                        })();
                        verified_keepers.insert(keep_pk, checked_keeper);
                    }
                    verified_keepers
                        .get(&keep_pk)
                        .unwrap()
                        .as_ref()
                        .map_err(Clone::clone)?;
                    Some(keep_pk)
                };
                verify_source(&item, state, control)?;
                if let Some(keep_pk) =
                    keep.filter(|_| !(plan.request.mode == "workflow" && item.manual_review))
                {
                    if matches!(plan.request.mode.as_str(),"workflowExact"|"workflowNear"){
                        let outcome=verifier.as_mut().unwrap().check_cascade(keep_pk,item.pk,state,control)?;
                        if !outcome.passed()||(plan.request.mode=="workflowExact"&&!outcome.strict){return Err(format!("三轮分类证据未通过，保留此图：{}",outcome.reason));}
                    }else{
                    let outcome = verifier.as_mut().unwrap().check_evidence(
                        keep_pk,
                        item.pk,
                        plan.request.mode == "autoExactAll",
                        state,
                        control,
                    )?;
                    if !outcome.automatic {
                        return Err(format!("替代证据未通过，保留此图：{}", outcome.reason));
                    }
                    }
                }
                Ok(keep)
            })();
            match checked {
                Ok(keep) => ready.push((n, item, keep)),
                Err(error) => {
                    // A cancellation leaves the current chunk pending for the same operation ID.
                    control()?;
                    skip_pending(conn, id, n, &format!("{}：{error}", item.image.file_name))?;
                }
            }
        }
        control()?;
        {
            let _write = state.write_lock.lock().map_err(|e| e.to_string())?;
            writable(state)?;
            if cache::db_binding(state)? != binding {
                return Err("图库已更换，停止当前整理".into());
            }
            let mut valid_recovered = Vec::new();
            for (n, item) in recovered {
                let owner = if restore { source } else { Some(id) };
                if ownership(conn, &item)? == owner
                    && hydrate(item.clone(), &db, !restore, state).is_ok()
                {
                    valid_recovered.push((n, item));
                } else {
                    skip_pending(conn, id, n, "中断后的记录状态已被其他操作改变，此项跳过")?;
                }
            }
            complete_items(conn, id, &valid_recovered, restore, source)?;
            // The chunk shares one full keeper hash, but its identity/size/mtime
            // and live record are still rechecked after all target reads, under
            // the write lock immediately before committing the chunk.
            let keeper_states: std::collections::HashMap<_, _> = verified_keepers
                .into_iter()
                .map(|(pk, checked)| {
                    (
                        pk,
                        checked.and_then(|keep| hydrate(keep, &db, false, state).map(|_| ())),
                    )
                })
                .collect();
            let mut valid = Vec::new();
            for (n, item, keep) in ready {
                let checked: Result<(), String> = (|| {
                    hydrate(item.clone(), &db, restore, state)?;
                    if let Some(pk) = keep {
                        keeper_states
                            .get(&pk)
                            .ok_or("保留图复核记录缺失")?
                            .as_ref()
                            .map_err(Clone::clone)?;
                    }
                    if restore && ownership(conn, &item)? != source {
                        return Err("撤销归属已被后续操作移除，此项跳过".into());
                    }
                    Ok(())
                })();
                match checked {
                    Ok(()) => valid.push((n, item)),
                    Err(error) => skip_pending(conn, id, n, &error)?,
                }
            }
            if !valid.is_empty() {
                // One durable intent transaction, one main transaction and one audit
                // transaction per chunk, independent of total run size.
                conn.execute_batch("BEGIN IMMEDIATE")
                    .map_err(|e| e.to_string())?;
                let intent = (|| {
                    for (n, item) in &valid {
                        if !restore {
                            conn.execute("INSERT INTO owners(image_pk,public_hash,operation) VALUES(?1,?2,?3) ON CONFLICT(image_pk) DO UPDATE SET public_hash=excluded.public_hash,operation=excluded.operation",params![item.pk,item.public_hash,id]).map_err(|e|e.to_string())?;
                        }
                        conn.execute(
                            "UPDATE items SET code=?3 WHERE operation=?1 AND ordinal=?2",
                            params![
                                id,
                                n,
                                if restore {
                                    "restoreIntent"
                                } else {
                                    "softIntent"
                                }
                            ],
                        )
                        .map_err(|e| e.to_string())?;
                    }
                    Ok(())
                })();
                finish_transaction(conn, intent)?;
                db.execute_batch("BEGIN IMMEDIATE")
                    .map_err(|e| e.to_string())?;
                let changed = (|| {
                    for (_, item) in &valid {
                        let count=db.execute("UPDATE images SET trashed=?3 WHERE image_pk=?1 AND id=?2 AND trashed=?4",params![item.pk,item.image.id,(!restore) as i64,restore as i64]).map_err(|e|e.to_string())?;
                        if count != 1 {
                            return Err("图库回收状态已变化，当前分块已回滚".into());
                        }
                    }
                    Ok(())
                })();
                finish_transaction(&db, changed)?;
                complete_items(conn, id, &valid, restore, source)?;
            }
        }
        checkpoint(conn, id)?;
        progress(&operation_status(&id.to_string(), state)?);
    }
    Ok(())
}

pub(super) fn run(
    operation: &str,
    conn: &Connection,
    plan: Plan,
    phase: &str,
    expires: i64,
    state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>,
    progress: &mut dyn FnMut(&DuplicateOperationStatus),
) -> Result<(), String> {
    if !["prepared", "partial", "cancelled", "interrupted", "failed"].contains(&phase) {
        return Err("此批次已经完成或正在运行".into());
    }
    if phase == "prepared" && timestamp() > expires {
        return Err("处理预览已过期，请重新启动".into());
    }
    let id = op_id(operation)?;
    conn.execute(
        "UPDATE operations SET state='running',updated=?2 WHERE id=?1",
        params![id, timestamp()],
    )
    .map_err(|e| e.to_string())?;
    let result: Result<(), String> = (|| {
        loop {
            control()?;
            let stage: String = conn
                .query_row(
                    "SELECT stage FROM bulk_runs WHERE operation=?1",
                    [id],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            match stage.as_str() {
                "workflowSnapshot" => snapshot_workflow(conn, id, &plan, state, control, progress)?,
                "snapshot" => snapshot(conn, id, &plan, state, control, progress)?,
                "planning" => plan_candidates(conn, id, &plan, state, control, progress)?,
                "undoSnapshot" => undo_snapshot(conn, id, &plan, state, control, progress)?,
                "applying" => {
                    apply_items(conn, id, &plan, state, control, progress)?;
                    break;
                }
                _ => return Err("全库处理阶段无效".into()),
            }
        }
        Ok(())
    })();
    let final_phase = match &result {
        Ok(()) => "completed",
        Err(e) if e.contains("取消") || e.contains("cancel") => "cancelled",
        Err(_) => "partial",
    };
    if let Err(error) = &result {
        note(conn, id, error)?;
    }
    conn.execute(
        "UPDATE operations SET state=?2,updated=?3,truncated=1 WHERE id=?1",
        params![id, final_phase, timestamp()],
    )
    .map_err(|e| e.to_string())?;
    let changed: i64 = conn
        .query_row(
            "SELECT succeeded FROM bulk_runs WHERE operation=?1",
            [id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    // A snapshot-only cancellation has not changed membership. Invalidating it
    // would reject this very operation's saved scope when the user continues.
    // An intent may precede a main commit whose audit completion failed, so it
    // must still refresh even when the succeeded counter has not advanced.
    let applying: bool = conn.query_row("SELECT stage='applying' FROM bulk_runs WHERE operation=?1",
        [id], |r| r.get(0)).map_err(|e| e.to_string())?;
    let has_intent = changed == 0 && applying && conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM items WHERE operation=?1 AND code IN ('softIntent','restoreIntent'))",
        [id], |r| r.get(0),
    ).map_err(|e| e.to_string())?;
    if changed > 0 || has_intent { cache::membership_invalidated(state); }
    if changed > 0 {
        if let Ok(mut library) = state.library.lock() {
            *library = None;
        }
        if let Ok(mut revision) = state.startup_cleanup_generation.lock() {
            *revision += 1;
        }
    }
    checkpoint(conn, id)?;
    progress(&operation_status(operation, state)?);
    result
}

#[cfg(test)]
#[path = "management_bulk_tests.rs"]
mod tests;

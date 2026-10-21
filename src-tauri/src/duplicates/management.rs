//! Explicit, bounded duplicate management. Audit/undo never live in the disposable fingerprint cache.
use super::*;
use crate::library::{AppState, GalleryImage};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::AppHandle;

const MAX_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ITEMS: usize = 20_000;
const TTL: i64 = 30 * 60 * 1000;
#[path = "management_bulk.rs"]
mod bulk;
const LOG_SCHEMA: &str = r#"
CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE operations(id INTEGER PRIMARY KEY,generation TEXT NOT NULL,group_id TEXT NOT NULL,action TEXT NOT NULL,state TEXT NOT NULL,request TEXT NOT NULL,created INTEGER NOT NULL,updated INTEGER NOT NULL,expires INTEGER NOT NULL,errors TEXT NOT NULL DEFAULT '[]',affected TEXT NOT NULL DEFAULT '[]',truncated INTEGER NOT NULL DEFAULT 0);
CREATE TABLE items(operation INTEGER NOT NULL,ordinal INTEGER NOT NULL,image_pk INTEGER NOT NULL,payload TEXT NOT NULL,state TEXT NOT NULL DEFAULT 'pending',code TEXT NOT NULL DEFAULT '',message TEXT NOT NULL DEFAULT '',file_moved INTEGER NOT NULL DEFAULT 0,db_updated INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(operation,ordinal)) WITHOUT ROWID;
CREATE INDEX items_state ON items(operation,state,ordinal);
CREATE TABLE owners(image_pk INTEGER PRIMARY KEY,public_hash TEXT NOT NULL,operation INTEGER NOT NULL);
PRAGMA user_version=1;
"#;
#[derive(Debug, Clone, Serialize, Deserialize)]
struct NearData {
    p: u64,
    d: u64,
    c: Vec<u8>,
    w: u32,
    h: u32,
    low: bool,
}
impl From<fingerprint::NearFingerprint> for NearData {
    fn from(n: fingerprint::NearFingerprint) -> Self {
        Self {
            p: n.phash,
            d: n.dhash,
            c: n.color.to_vec(),
            w: n.width,
            h: n.height,
            low: n.low_information,
        }
    }
}
impl NearData {
    fn near(&self) -> Option<fingerprint::NearFingerprint> {
        Some(fingerprint::NearFingerprint {
            phash: self.p,
            dhash: self.d,
            color: self.c.clone().try_into().ok()?,
            width: self.w,
            height: self.h,
            low_information: self.low,
        })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Item {
    pub pk: i64,
    pub image: GalleryImage,
    pub public_hash: String,
    pub signature: String,
    pub sha: String,
    near: Option<NearData>,
    pub identity: String,
    pub canonical: String,
    #[serde(default)]
    keeper_pks: Vec<i64>,
    #[serde(default)]
    manual_review: bool,
}
#[derive(Clone, Serialize, Deserialize)]
struct Plan {
    request: DuplicateManagementRequest,
    keepers: Vec<Item>,
    targets: i64,
    #[serde(default)]
    batches: Vec<DuplicateManagementRequest>,
}
fn timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
fn next_operation_id() -> i64 {
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    timestamp() * 1024 + (SERIAL.fetch_add(1, Ordering::SeqCst) % 1024) as i64
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}
fn journal_path(state: &AppState) -> PathBuf {
    state
        .data_dir
        .join("operations/duplicate-management.sqlite")
}
fn size(path: &Path) -> u64 {
    ["", "-wal", "-shm", "-journal"]
        .iter()
        .filter_map(|s| fs::metadata(format!("{}{s}", path.to_string_lossy())).ok())
        .map(|m| m.len())
        .sum()
}
fn journal(state: &AppState, create: bool) -> Result<Connection, String> {
    let path = journal_path(state);
    if !path.exists() {
        if !create {
            return Err("没有查重处理记录".into());
        }
        if state.read_only {
            return Err("只读图库不能记录处理操作".into());
        }
        fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        let conn = Connection::open(&path).map_err(|e| e.to_string())?;
        conn.execute_batch(LOG_SCHEMA).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO meta VALUES('binding',?1)",
            [cache::db_binding(state)?],
        )
        .map_err(|e| e.to_string())?;
    }
    let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|e| format!("处理日志无法读取：{e}"))?;
    conn.busy_timeout(std::time::Duration::from_secs(2))
        .map_err(|e| e.to_string())?;
    let binding: String = conn
        .query_row("SELECT value FROM meta WHERE key='binding'", [], |r| {
            r.get(0)
        })
        .map_err(|_| "处理日志损坏，不能使用旧撤销记录")?;
    if binding != cache::db_binding(state)? {
        return Err("图库已恢复或更换，旧处理记录不能用于撤销或继续删除".into());
    }
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if version != 1 {
        return Err("处理日志版本不兼容".into());
    }
    // Bulk audit grows on disk with the run; RAM and individual legacy plans remain bounded.
    // Reset the old persisted 32 MiB page ceiling before adding resumable bulk records.
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA cache_size=-2048; PRAGMA mmap_size=0; PRAGMA temp_store=FILE; PRAGMA wal_autocheckpoint=128; PRAGMA journal_size_limit=1048576; PRAGMA max_page_count=2147483646;").map_err(|e|e.to_string())?;
    bulk::schema(&conn)?;
    Ok(conn)
}
fn writable(state: &AppState) -> Result<(), String> {
    if state.read_only {
        Err("此图库只允许浏览".into())
    } else if state.maintenance.load(Ordering::SeqCst) {
        Err("数据维护中，不能处理图片".into())
    } else {
        Ok(())
    }
}
fn op_id(id: &str) -> Result<i64, String> {
    id.parse::<i64>()
        .ok()
        .filter(|v| *v > 0)
        .ok_or_else(|| "处理批次编号无效".into())
}
fn canonical(path: &Path) -> Result<String, String> {
    let p = fs::canonicalize(path)
        .map_err(|e| format!("文件无法访问：{e}"))?
        .to_string_lossy()
        .into_owned();
    Ok(if cfg!(windows) { p.to_lowercase() } else { p })
}
pub(crate) fn main_image(conn: &Connection, pk: i64) -> Result<Option<GalleryImage>, String> {
    conn.prepare_cached("SELECT id,path,file_name,ext,COALESCE(width,0),COALESCE(height,0),file_size,modified_at,imported_at,folder_id,missing,trashed,is_favorite,source FROM images WHERE image_pk=?1").map_err(|e|e.to_string())?.query_row([pk],|r|Ok(GalleryImage{id:r.get(0)?,path:r.get(1)?,thumbnail_path:None,file_name:r.get(2)?,ext:r.get(3)?,width:r.get(4)?,height:r.get(5)?,file_size:r.get(6)?,modified_at:r.get(7)?,imported_at:r.get(8)?,folder_id:r.get(9)?,missing:r.get::<_,i64>(10)?!=0,trashed:r.get::<_,i64>(11)?!=0,is_favorite:r.get::<_,i64>(12)?!=0,source:r.get(13)?})).optional().map_err(|e|e.to_string())
}
fn item(
    conn: &Connection,
    db: &Connection,
    pk: i64,
    expected: Option<(Vec<u8>, Vec<u8>)>,
) -> Result<Item, String> {
    let image = main_image(db, pk)?.ok_or("图片已从图库移除，请重新预览")?;
    if image.source == "reference" {
        return Err("历史参考板图片不能通过查重批量处理".into());
    }
    let public_hash = digest(image.id.as_bytes());
    let row=conn.query_row("SELECT public_hash,signature,sha,phash,dhash,color,width,height,low_info FROM fingerprints WHERE image_pk=?1",[pk],|r|Ok((r.get::<_,Vec<u8>>(0)?,r.get::<_,Vec<u8>>(1)?,r.get::<_,Option<Vec<u8>>>(2)?,r.get::<_,Option<Vec<u8>>>(3)?,r.get::<_,Option<Vec<u8>>>(4)?,r.get::<_,Option<Vec<u8>>>(5)?,r.get::<_,u32>(6)?,r.get::<_,u32>(7)?,r.get::<_,i64>(8)?))).optional().map_err(|e|e.to_string())?.ok_or("图片没有有效查重指纹")?;
    if hex(&row.0) != public_hash
        || expected
            .as_ref()
            .is_some_and(|(p, s)| *p != row.0 || *s != row.1)
    {
        return Err("组内图片身份或指纹已变化，请重新检索".into());
    }
    let source = fingerprint::source_signature(Path::new(&image.path))?;
    let signature = hex(&cache::signature(&image.id, &source));
    if signature != hex(&row.1) {
        return Err(format!("{} 已变化或离线，请更新结果", image.file_name));
    }
    let sha = hex(&row.2.ok_or("图片文件哈希缺失")?);
    let near = match (row.3, row.4, row.5) {
        (Some(p), Some(d), Some(c)) => Some(NearData {
            p: u64::from_le_bytes(p.try_into().map_err(|_| "指纹损坏")?),
            d: u64::from_le_bytes(d.try_into().map_err(|_| "指纹损坏")?),
            c,
            w: row.6,
            h: row.7,
            low: row.8 != 0,
        }),
        _ => None,
    };
    Ok(Item {
        pk,
        canonical: canonical(Path::new(&image.path))?,
        identity: source.identity,
        image,
        public_hash,
        signature,
        sha,
        near,
        keeper_pks: vec![],
        manual_review: false,
    })
}
fn public_item(conn: &Connection, db: &Connection, id: &str) -> Result<Item, String> {
    let pk: i64 = db
        .query_row("SELECT image_pk FROM images WHERE id=?1", [id], |r| {
            r.get(0)
        })
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or("保留项不存在")?;
    item(conn, db, pk, None)
}
fn threshold(value: &str) -> index::Threshold {
    match value {
        "strict" => index::Threshold::Strict,
        "loose" => index::Threshold::Loose,
        _ => index::Threshold::Standard,
    }
}
fn matches(target: &Item, keepers: &[Item], kind: &str, sensitivity: &str) -> bool {
    keepers.iter().any(|keep| {
        if kind == "exact" {
            target.sha == keep.sha
        } else {
            target
                .near
                .as_ref()
                .and_then(NearData::near)
                .zip(keep.near.as_ref().and_then(NearData::near))
                .is_some_and(|(a, b)| index::compare_near(&a, &b, threshold(sensitivity)).is_some())
        }
    })
}
fn plan_items(
    req: &DuplicateManagementRequest,
    state: &AppState,
) -> Result<(Vec<Item>, Vec<Item>, String, String, i64), String> {
    if req.keep_ids.is_empty() && req.action != "systemTrash" {
        return Err("每组至少保留一张图片".into());
    }
    if req.keep_ids.len() > 2000
        || req.protected_ids.len() > 100_000
        || req.image_ids.len() > MAX_ITEMS
    {
        return Err("选择数量超过单次处理上限，请分批处理".into());
    }
    if !["trash", "systemTrash"].contains(&req.action.as_str()) {
        return Err("不支持的整理操作".into());
    }
    if req.action == "systemTrash" && req.source_operation_id.is_none() {
        return Err("请先移入应用回收站，再从该次处理结果执行系统回收站操作".into());
    }
    if req.action == "systemTrash" {
        return system_plan(req, state);
    }
    let conn = cache::open_existing(state, false)?;
    let published: String = conn
        .query_row("SELECT value FROM meta WHERE key='published'", [], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    if published != req.generation {
        return Err("检索结果代次已变，请重新预览".into());
    }
    let group: i64 = req.group_id.parse().map_err(|_| "分组编号无效")?;
    let (kind, total): (i64, i64) = conn
        .query_row(
            "SELECT kind,total_members FROM groups WHERE generation=?1 AND group_no=?2",
            params![req.generation, group],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or("分组不存在")?;
    let kind = if kind == 0 { "exact" } else { "near" }.to_string();
    let sensitivity: String = conn
        .query_row(
            "SELECT sensitivity FROM generations WHERE generation=?1",
            [&req.generation],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let others = ["exactOthers", "others"].contains(&req.mode.as_str());
    if kind == "near" && others {
        return Err("近似组只能处理明确选中的图片，不能自动处理整组其他项".into());
    }
    if !others && req.mode != "selected" {
        return Err("处理选择模式无效".into());
    }
    if others && total > MAX_ITEMS as i64 {
        return Err("整组超过2万项，超出32 MiB撤销日志预算，请分批明确选择处理".into());
    }
    let db = cache::library(state)?;
    let mut keepers = Vec::new();
    let protected: HashSet<_> = req
        .keep_ids
        .iter()
        .chain(req.protected_ids.iter())
        .cloned()
        .collect();
    let mut keep_seen = HashSet::new();
    for id in &req.keep_ids {
        if !keep_seen.insert(id.clone()) {
            continue;
        }
        let keep = public_item(&conn, &db, id)?;
        if keep.image.trashed || keep.image.missing {
            return Err("保留项已被移走或离线，不能处理此组".into());
        }
        if req.keep_ids.contains(id) {
            let exists:i64=conn.query_row("SELECT COUNT(*) FROM members WHERE generation=?1 AND group_no=?2 AND image_pk=?3",params![req.generation,group,keep.pk],|r|r.get(0)).map_err(|e|e.to_string())?;
            if exists == 0 {
                return Err("保留项不属于该组".into());
            }
        }
        keepers.push(keep);
    }
    let local: Vec<_> = keepers
        .iter()
        .filter(|k| req.keep_ids.contains(&k.image.id))
        .cloned()
        .collect();
    let mut source_owned = HashSet::new();
    if let Some(source) = req.source_operation_id.as_ref() {
        let j = journal(state, false)?;
        let source = op_id(source)?;
        let action: String = j
            .query_row("SELECT action FROM operations WHERE id=?1", [source], |r| {
                r.get(0)
            })
            .map_err(|_| "原软回收批次不存在")?;
        if action != "trash" {
            return Err("系统回收站必须来源于软回收批次".into());
        }
        let mut stmt=j.prepare("SELECT i.image_pk FROM items i JOIN owners o ON o.image_pk=i.image_pk AND o.operation=i.operation WHERE i.operation=?1 AND i.state='succeeded'").map_err(|e|e.to_string())?;
        source_owned = stmt
            .query_map([source], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<HashSet<_>, _>>()
            .map_err(|e| e.to_string())?;
    }
    let selected: HashSet<_> = req.image_ids.iter().cloned().collect();
    let mut candidates: Vec<(i64, Vec<u8>, Vec<u8>)> = Vec::new();
    if !others {
        for id in &selected {
            let pk: Option<i64> = db
                .query_row("SELECT image_pk FROM images WHERE id=?1", [id], |r| {
                    r.get(0)
                })
                .optional()
                .map_err(|e| e.to_string())?;
            if let Some(pk) = pk {
                let row=conn.query_row("SELECT image_pk,public_hash,signature FROM members WHERE generation=?1 AND group_no=?2 AND image_pk=?3 LIMIT 1",params![req.generation,group,pk],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|e|e.to_string())?;
                if let Some(row) = row {
                    candidates.push(row);
                }
            }
        }
    } else {
        let mut stmt=conn.prepare("SELECT image_pk,public_hash,signature FROM members WHERE generation=?1 AND group_no=?2 ORDER BY member_no").map_err(|e|e.to_string())?;
        candidates = stmt
            .query_map(params![req.generation, group], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
    }
    let mut targets = Vec::new();
    let mut protected_count = 0;
    for (pk, public, sig) in candidates {
        let Some(image) = main_image(&db, pk)? else {
            continue;
        };
        let wanted = if req.action == "systemTrash" {
            source_owned.contains(&pk) && (selected.is_empty() || selected.contains(&image.id))
        } else {
            !image.trashed && (others || selected.contains(&image.id))
        };
        if !wanted {
            continue;
        }
        if protected.contains(&image.id) {
            protected_count += 1;
            if !keep_seen.contains(&image.id) {
                let keep = public_item(&conn, &db, &image.id)?;
                if keep.image.trashed {
                    return Err("相关保护项已回收，需重新预览".into());
                }
                keep_seen.insert(image.id.clone());
                keepers.push(keep);
            }
            continue;
        }
        if req.action == "systemTrash" && !image.trashed {
            return Err("系统回收站只允许处理仍处于应用回收站的本批次图片".into());
        }
        let mut target = item(&conn, &db, pk, Some((public, sig)))?;
        target.keeper_pks = local.iter().map(|k| k.pk).collect();
        if !matches(&target, &local, &kind, &sensitivity) {
            return Err(format!(
                "{} 与当前保留图不匹配，请重新目视审核",
                target.image.file_name
            ));
        }
        if req.action == "systemTrash"
            && keepers
                .iter()
                .any(|k| k.canonical == target.canonical || k.identity == target.identity)
        {
            return Err("目标与保留项指向同一物理文件或路径别名，不能移入系统回收站".into());
        }
        targets.push(target);
        if targets.len() > MAX_ITEMS {
            return Err("处理项超过单批上限".into());
        }
    }
    if !others
        && req.action == "trash"
        && selected
            .iter()
            .any(|id| !protected.contains(id) && !targets.iter().any(|t| &t.image.id == id))
    {
        return Err("部分选中项已不在该组，请重新预览".into());
    }
    Ok((keepers, targets, kind, sensitivity, protected_count))
}

fn system_plan(
    req: &DuplicateManagementRequest,
    state: &AppState,
) -> Result<(Vec<Item>, Vec<Item>, String, String, i64), String> {
    let source = op_id(
        req.source_operation_id
            .as_deref()
            .ok_or("系统操作需要软回收来源")?,
    )?;
    let log = journal(state, false)?;
    let (source_plan, _, _) = load_plan(&log, source)?;
    if source_plan.request.action != "trash" {
        return Err("系统操作必须来自应用回收站批次".into());
    }
    let selected: HashSet<_> = req.image_ids.iter().cloned().collect();
    let protected: HashSet<_> = req
        .protected_ids
        .iter()
        .chain(req.keep_ids.iter())
        .cloned()
        .collect();
    let mut targets = Vec::new();
    let mut protected_count = 0;
    for (_, mut target, phase, moved) in item_rows(&log, source)? {
        if phase != "succeeded"
            || moved
            || ownership(&log, &target)? != Some(source)
            || (!selected.is_empty() && !selected.contains(&target.image.id))
        {
            continue;
        }
        if protected.contains(&target.image.id) {
            protected_count += 1;
            continue;
        }
        validate_item(&target, state, true)?;
        target.image.trashed = true;
        targets.push(target);
    }
    if selected
        .iter()
        .any(|id| !protected.contains(id) && !targets.iter().any(|t| &t.image.id == id))
    {
        return Err("所选图片不再属于原软回收批次，请重新预览".into());
    }
    let mut relevant = HashSet::new();
    for target in &targets {
        relevant.extend(target.keeper_pks.iter().copied());
    }
    let keepers: Vec<_> = source_plan
        .keepers
        .into_iter()
        .filter(|k| relevant.is_empty() || relevant.contains(&k.pk))
        .collect();
    if keepers.is_empty() {
        return Err("原批次没有可核实的保留图，不能执行系统回收站操作".into());
    }
    for keep in &keepers {
        validate_item(keep, state, false)?;
    }
    if targets.iter().any(|t| {
        keepers
            .iter()
            .any(|k| k.identity == t.identity || k.canonical == t.canonical)
    }) {
        return Err("目标和保留图指向同一物理文件或别名，不能移动".into());
    }
    Ok((
        keepers,
        targets,
        "exact".into(),
        "standard".into(),
        protected_count,
    ))
}
fn auto_request(
    req: &DuplicateManagementRequest,
    state: &AppState,
) -> Result<DuplicateManagementRequest, String> {
    let conn = cache::open_existing(state, false)?;
    let current: String = conn
        .query_row("SELECT value FROM meta WHERE key='published'", [], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    if current != req.generation {
        return Err("查重结果已变化，请重新预览".into());
    }
    let (kind, total): (i64, i64) = conn
        .query_row(
            "SELECT kind,total_members FROM groups WHERE generation=?1 AND group_no=?2",
            params![req.generation, req.group_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    if kind != 0 {
        return Err("自动保留仅适用于文件完全相同组，近似组不参与".into());
    }
    if total > MAX_ITEMS as i64 {
        return Err("该组超过2万项日志上限，需要分批明确选择".into());
    }
    let db = cache::library(state)?;
    let mut stmt = conn
        .prepare("SELECT image_pk FROM members WHERE generation=?1 AND group_no=?2")
        .map_err(|e| e.to_string())?;
    let pks = stmt
        .query_map(params![req.generation, req.group_id], |r| {
            r.get::<_, i64>(0)
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let mut ranked = Vec::new();
    for pk in pks {
        if let Some(image) = main_image(&db, pk)? {
            if image.trashed || image.source == "reference" {
                continue;
            }
            let informed:i64=db.query_row("SELECT EXISTS(SELECT 1 FROM storage_image_user_folders WHERE image_id=?1)+EXISTS(SELECT 1 FROM storage_image_user_custom_tags WHERE image_id=?1)+EXISTS(SELECT 1 FROM storage_image_user_supplement_tags WHERE image_id=?1)",[pk],|r|r.get(0)).map_err(|e|e.to_string())?;
            ranked.push((
                u64::from(image.width) * u64::from(image.height),
                image.is_favorite,
                informed,
                image.id,
            ));
        }
    }
    if ranked.len() < 2 {
        return Err("该组已无可整理的重复项".into());
    }
    ranked.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| b.2.cmp(&a.2))
            .then_with(|| a.3.cmp(&b.3))
    });
    let mut expanded = req.clone();
    expanded.mode = "exactOthers".into();
    expanded.keep_ids = vec![ranked[0].3.clone()];
    Ok(expanded)
}
fn build_prepare(
    req: &DuplicateManagementRequest,
    state: &AppState,
) -> Result<(Plan, Vec<Item>, i64, Vec<String>), String> {
    if req.mode != "autoExact" {
        let (k, t, _, _, p) = plan_items(req, state)?;
        return Ok((
            Plan {
                request: req.clone(),
                keepers: k,
                targets: t.len() as i64,
                batches: vec![],
            },
            t,
            p,
            vec![],
        ));
    }
    if req.action != "trash" {
        return Err("自动保留先整理到应用回收站，系统操作请在批次完成后单独执行".into());
    }
    if req.group_id != "all-exact" {
        let expanded = auto_request(req, state)?;
        let (k, t, _, _, p) = plan_items(&expanded, state)?;
        return Ok((
            Plan {
                request: expanded,
                keepers: k,
                targets: t.len() as i64,
                batches: vec![],
            },
            t,
            p,
            vec![
                "已按全组像素面积推荐一张；相同像素时优先收藏和手动信息，跨组保护项仍保留。".into(),
            ],
        ));
    }
    let conn = cache::open_existing(state, false)?;
    let mut stmt = conn
        .prepare("SELECT group_no FROM groups WHERE generation=?1 AND kind=0 ORDER BY group_no")
        .map_err(|e| e.to_string())?;
    let groups = stmt
        .query_map([&req.generation], |r| r.get::<_, i64>(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let mut keepers = Vec::new();
    let mut targets = Vec::new();
    let mut batches = Vec::new();
    let mut protected = 0;
    let mut skipped = 0;
    let log = journal(state, true)?;
    let used:i64=log.query_row("SELECT (page_count-freelist_count)*page_size FROM pragma_page_count(),pragma_freelist_count(),pragma_page_size()",[],|r|r.get(0)).map_err(|e|e.to_string())?;
    let mut estimate = used as u64 + 16384;
    for id in groups {
        let mut one = req.clone();
        one.group_id = id.to_string();
        let expanded = match auto_request(&one, state) {
            Ok(r) => r,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let (k, t, _, _, p) = match plan_items(&expanded, state) {
            Ok(r) => r,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let bytes = t
            .iter()
            .chain(k.iter())
            .map(|i| {
                serde_json::to_string(i)
                    .map(|s| s.len() as u64 + 200)
                    .unwrap_or(MAX_BYTES)
            })
            .sum::<u64>();
        if targets.len() + t.len() > MAX_ITEMS
            || estimate + bytes > 12 * 1024 * 1024
            || batches.len() >= 1000
        {
            skipped += 1;
            continue;
        }
        estimate += bytes;
        protected += p;
        keepers.extend(k);
        targets.extend(t);
        batches.push(expanded);
    }
    let mut seen = HashSet::new();
    keepers.retain(|k| seen.insert(k.pk));
    let warnings=vec![format!("本批处理 {} 个完全相同组；另有 {} 个组未纳入（可能已处理、超大、源失效或达到本批预算），请完成后再次预览。近似组完全排除。",batches.len(),skipped)];
    Ok((
        Plan {
            request: req.clone(),
            keepers,
            targets: targets.len() as i64,
            batches,
        },
        targets,
        protected,
        warnings,
    ))
}
fn append_operation(plan: Plan, targets: &[Item], state: &AppState) -> Result<String, String> {
    let mut conn = journal(state, true)?;
    let plan_json = serde_json::to_string(&plan).map_err(|e| e.to_string())?;
    let payloads: Vec<_> = targets
        .iter()
        .map(|i| serde_json::to_string(i).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let used:i64=conn.query_row("SELECT (page_count-freelist_count)*page_size FROM pragma_page_count(),pragma_freelist_count(),pragma_page_size()",[],|r|r.get(0)).map_err(|e|e.to_string())?;
    let estimate =
        payloads.iter().map(|s| s.len() as u64 + 160).sum::<u64>() + plan_json.len() as u64 + 16384;
    let reserve = if plan.request.action == "trash" {
        estimate.saturating_mul(2)
    } else {
        estimate
    };
    if used as u64 + reserve > 26 * 1024 * 1024 {
        return Err("本批次会超过32 MiB处理日志预算，请缩小选择范围".into());
    }
    let id = next_operation_id();
    conn.execute("INSERT INTO operations(id,generation,group_id,action,state,request,created,updated,expires) VALUES(?1,?2,?3,?4,'preparing',?5,?6,?6,?7)",params![id,plan.request.generation,plan.request.group_id,plan.request.action,plan_json,timestamp(),timestamp()+TTL]).map_err(|e|e.to_string())?;
    for (chunk_no, chunk) in payloads.chunks(100).enumerate() {
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        {
            let mut insert = tx
                .prepare(
                    "INSERT INTO items(operation,ordinal,image_pk,payload) VALUES(?1,?2,?3,?4)",
                )
                .map_err(|e| e.to_string())?;
            for (offset, payload) in chunk.iter().enumerate() {
                let ordinal = chunk_no * 100 + offset;
                insert
                    .execute(params![id, ordinal as i64, targets[ordinal].pk, payload])
                    .map_err(|e| format!("处理日志空间不足，未修改任何图片：{e}"))?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(|e| e.to_string())?;
        if size(&journal_path(state)) > MAX_BYTES {
            return Err("处理日志达到32 MiB预算，未修改任何图片".into());
        }
    }
    conn.execute("UPDATE operations SET state='prepared' WHERE id=?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(id.to_string())
}
pub fn prepare(
    request: DuplicateManagementRequest,
    state: &AppState,
) -> Result<DuplicateActionPreview, String> {
    writable(state)?;
    let _write = state.write_lock.lock().map_err(|e| e.to_string())?;
    if bulk::is_request(&request) {
        return bulk::prepare(request, state);
    }
    let (plan, targets, protected, mut warnings) = build_prepare(&request, state)?;
    if targets.is_empty() {
        return Err("没有可处理的非保留图片".into());
    }
    let db = cache::library(state)?;
    let mut metadata = DuplicateMetadataSummary::default();
    for target in &targets {
        metadata.favorite_count += target.image.is_favorite as i64;
        for (table, slot) in [
            ("storage_image_user_folders", &mut metadata.classified_count),
            (
                "storage_image_user_custom_tags",
                &mut metadata.custom_tagged_count,
            ),
            (
                "storage_image_user_supplement_tags",
                &mut metadata.supplement_tagged_count,
            ),
        ] {
            let yes: i64 = db
                .query_row(
                    &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE image_id=?1)"),
                    [target.pk],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            *slot += yes;
        }
    }
    let total_bytes = targets.iter().map(|i| i.image.file_size).sum();
    let sample = targets
        .iter()
        .take(6)
        .map(|i| DuplicateMember {
            image: i.image.clone(),
            metric: DuplicateMetric::default(),
            stale: false,
        })
        .collect();
    let id = append_operation(plan, &targets, state)?;
    warnings.push(
        if request.action == "trash" {
            "仅移入应用回收站；原文件、分类、标签、收藏仍保留，不自动合并到保留图。"
        } else {
            "将原文件移入Windows回收站并删除其图库记录及关联元数据；本程序不能撤销此步骤。"
        }
        .into(),
    );
    Ok(DuplicateActionPreview {
        operation_id: id,
        generation: request.generation,
        group_id: request.group_id,
        action: request.action.clone(),
        target_count: targets.len() as i64,
        total_bytes,
        protected_count: protected,
        skipped_count: 0,
        metadata,
        sample,
        warnings,
        expires_at: timestamp() + TTL,
        requires_system_confirmation: request.action == "systemTrash",
    })
}
fn load_plan(conn: &Connection, id: i64) -> Result<(Plan, String, i64), String> {
    let row = conn
        .query_row(
            "SELECT request,state,expires FROM operations WHERE id=?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or("处理批次不存在")?;
    Ok((
        serde_json::from_str(&row.0).map_err(|e| e.to_string())?,
        row.1,
        row.2,
    ))
}
fn item_rows(conn: &Connection, id: i64) -> Result<Vec<(i64, Item, String, bool)>, String> {
    let mut stmt=conn.prepare("SELECT ordinal,payload,state,file_moved FROM items WHERE operation=?1 ORDER BY ordinal").map_err(|e|e.to_string())?;
    let result = stmt
        .query_map([id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)? != 0,
            ))
        })
        .map_err(|e| e.to_string())?
        .map(|r| {
            let (n, p, s, m) = r.map_err(|e| e.to_string())?;
            Ok((
                n,
                serde_json::from_str(&p).map_err(|e| e.to_string())?,
                s,
                m,
            ))
        })
        .collect();
    result
}
fn hash_file(item: &Item, control: &mut dyn FnMut() -> Result<(), String>) -> Result<(), String> {
    let before = fingerprint::source_signature(Path::new(&item.image.path))?;
    if hex(&cache::signature(&item.image.id, &before)) != item.signature {
        return Err("源文件已变化或离线".into());
    }
    let mut file = fs::File::open(&item.image.path).map_err(|e| e.to_string())?;
    let mut sha = Sha256::new();
    let mut buffer = [0u8; 256 * 1024];
    loop {
        control()?;
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        sha.update(&buffer[..n]);
    }
    if hex(&sha.finalize()) != item.sha
        || fingerprint::source_signature(Path::new(&item.image.path))? != before
    {
        return Err("图片内容已变化，请重新预览".into());
    }
    Ok(())
}
fn validate_item(item: &Item, state: &AppState, trashed: bool) -> Result<(), String> {
    let db = cache::library(state)?;
    let current = main_image(&db, item.pk)?.ok_or("图库记录已移除")?;
    if digest(current.id.as_bytes()) != item.public_hash
        || current.path != item.image.path
        || current.source != item.image.source
        || current.trashed != trashed
    {
        return Err("图片身份、路径或回收状态已变化，请重新预览".into());
    }
    let source = fingerprint::source_signature(Path::new(&current.path))?;
    if hex(&cache::signature(&current.id, &source)) != item.signature
        || canonical(Path::new(&current.path))? != item.canonical
    {
        return Err("源文件已变化或离线，请重新预览".into());
    }
    Ok(())
}
fn operation_active(operation: &str, state: &AppState) -> Result<bool, String> {
    Ok(is_running(state)
        || state.jobs.list()?.iter().any(|j| {
            j.request.duplicate_operation_id.as_deref() == Some(operation)
                && ["queued", "running", "pausing", "paused", "cancelling"]
                    .contains(&j.state.as_str())
        }))
}
fn recover_interrupted(
    conn: &Connection,
    id: i64,
    operation: &str,
    state: &AppState,
) -> Result<(), String> {
    if !operation_active(operation, state)? {
        conn.execute(
            "UPDATE operations SET state='interrupted',updated=?2 WHERE id=?1 AND state='running'",
            params![id, timestamp()],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
pub(crate) fn validate_apply(
    request: &DuplicateApplyRequest,
    state: &AppState,
) -> Result<(), String> {
    writable(state)?;
    if !request.confirmed {
        return Err("请先确认处理预览".into());
    }
    let id = op_id(&request.operation_id)?;
    {
        let conn = journal(state, false)?;
        recover_interrupted(&conn, id, &request.operation_id, state)?;
        let (plan, phase, expires) = load_plan(&conn, id)?;
        if plan.request.action != "restore" {
            super::workflow::validate_plan(&plan.request, state)?;
        }
        let bulk = bulk::exists(&conn, id)?;
        if !["prepared", "partial", "cancelled", "interrupted"].contains(&phase.as_str())
            && !(bulk && phase == "failed")
        {
            return Err("此批次已提交或正在执行，不能重复应用".into());
        }
        if phase == "prepared" && timestamp() > expires {
            return Err("处理预览已过期，请重新预览".into());
        }
    }
    Ok(())
}
pub fn apply(
    request: DuplicateApplyRequest,
    state: &AppState,
    app: AppHandle,
) -> Result<crate::jobs::JobRecord, String> {
    validate_apply(&request, state)?;
    let record = crate::jobs::start_job(
        crate::jobs::JobRequest {
            recycle_operation_id: None,
            kind: "duplicateManagement".into(),
            thumbnails: false,
            source_folder_id: None,
            duplicate_options: None,
            duplicate_operation_id: Some(request.operation_id),
        },
        state,
        app,
    )?;
    Ok(record)
}
pub(super) fn prepare_workflow_category(request:&DuplicateWorkflowRequest,source:&Connection,plan:i64,kind:Option<&str>,state:&AppState)->Result<DuplicateActionPreview,String>{
    writable(state)?;
    bulk::prepare_workflow_category(request,source,plan,kind,state)
}
pub fn undo(
    operation: String,
    state: &AppState,
    app: AppHandle,
) -> Result<crate::jobs::JobRecord, String> {
    let id = prepare_undo(&operation, state)?;
    crate::jobs::start_job(
        crate::jobs::JobRequest {
            recycle_operation_id: None,
            kind: "duplicateUndo".into(),
            thumbnails: false,
            source_folder_id: None,
            duplicate_options: None,
            duplicate_operation_id: Some(id),
        },
        state,
        app,
    )
}
pub(crate) fn prepare_undo(operation: &str, state: &AppState) -> Result<String, String> {
    writable(state)?;
    let _write = state.write_lock.lock().map_err(|e| e.to_string())?;
    let conn = journal(state, false)?;
    let source = op_id(operation)?;
    let (plan, _, _) = load_plan(&conn, source)?;
    if plan.request.action != "trash" {
        return Err("只有应用回收站操作可以撤销".into());
    }
    if operation_active(operation, state)? {
        return Err("请先暂停并取消当前整理任务，再撤销本批次".into());
    }
    // Legacy history shares this journal with scalable runs. Stream every undo
    // through the resumable path so later bulk audit cannot exhaust its budget.
    bulk::prepare_undo(operation, plan, state)
}
pub fn revoke_for_ids(ids: &[String], state: &AppState) -> Result<(), String> {
    if !journal_path(state).exists() {
        return Ok(());
    }
    let conn = match journal(state, false) {
        Ok(c) => c,
        Err(e) if e.contains("图库已恢复或更换") => return Ok(()),
        Err(e) => return Err(e),
    };
    let db = cache::library(state)?;
    for id in ids {
        let pk: Option<i64> = db
            .query_row("SELECT image_pk FROM images WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(pk) = pk {
            conn.execute("DELETE FROM owners WHERE image_pk=?1", [pk])
                .map_err(|e| e.to_string())?;
        }
    }
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .map_err(|e| e.to_string())?;
    Ok(())
}
pub fn revoke_all(state: &AppState) -> Result<(), String> {
    if journal_path(state).exists() {
        let conn = match journal(state, false) {
            Ok(c) => c,
            Err(e) if e.contains("图库已恢复或更换") => return Ok(()),
            Err(e) => return Err(e),
        };
        conn.execute("DELETE FROM owners", [])
            .map_err(|e| e.to_string())?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn item_outcome(
    conn: &Connection,
    id: i64,
    n: i64,
    state: &str,
    code: &str,
    message: &str,
    moved: bool,
    updated: bool,
) -> Result<(), String> {
    conn.execute("UPDATE items SET state=?3,code=?4,message=?5,file_moved=?6,db_updated=?7 WHERE operation=?1 AND ordinal=?2",params![id,n,state,code,message.chars().take(240).collect::<String>(),moved as i64,updated as i64]).map_err(|e|e.to_string())?;
    Ok(())
}
pub fn operation_status(
    operation: &str,
    state: &AppState,
) -> Result<DuplicateOperationStatus, String> {
    let conn = journal(state, false)?;
    let id = op_id(operation)?;
    let (plan, mut phase, _) = load_plan(&conn, id)?;
    if phase == "running" && !operation_active(operation, state)? {
        phase = "interrupted".into();
    }
    let counts = if let Some(counts) = bulk::status_counts(&conn, id)? {
        counts
    } else {
        conn.query_row("SELECT COUNT(*),SUM(state<>'pending'),SUM(state='succeeded'),SUM(state='failed'),SUM(state='skipped') FROM items WHERE operation=?1",[id],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Option<i64>>(1)?.unwrap_or(0),r.get::<_,Option<i64>>(2)?.unwrap_or(0),r.get::<_,Option<i64>>(3)?.unwrap_or(0),r.get::<_,Option<i64>>(4)?.unwrap_or(0)))).map_err(|e|e.to_string())?
    };
    let (stage, stage_processed, stage_total) =
        bulk::stage_progress(&conn, id)?.unwrap_or_else(|| (String::new(), counts.1, counts.0));
    let owners: i64 = if bulk::exists(&conn, id)? {
        conn.query_row("SELECT EXISTS(SELECT 1 FROM owners o JOIN items i ON i.operation=o.operation AND i.ordinal=o.image_pk WHERE o.operation=?1 AND i.state='succeeded' AND i.file_moved=0)",[id],|r|r.get(0)).map_err(|e|e.to_string())?
    } else {
        conn
        .query_row(
            "SELECT COUNT(*) FROM owners o JOIN items i ON i.operation=o.operation AND i.image_pk=o.image_pk WHERE o.operation=?1 AND i.state='succeeded' AND i.file_moved=0",
            [id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?
    };
    let (extra, errors, truncated, updated): (String, String, bool, i64) = conn
        .query_row(
            "SELECT affected,errors,truncated,updated FROM operations WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0, r.get(3)?)),
        )
        .map_err(|e| e.to_string())?;
    Ok(DuplicateOperationStatus {
        operation_id: operation.into(),
        generation: plan.request.generation,
        group_id: plan.request.group_id,
        action: plan.request.action.clone(),
        state: phase,
        stage,
        stage_processed,
        stage_total,
        processed: counts.1,
        total: counts.0,
        succeeded: counts.2,
        failed: counts.3,
        skipped: counts.4,
        undoable: plan.request.action == "trash" && owners > 0,
        affected_group_ids: serde_json::from_str(&extra).unwrap_or_default(),
        affected_groups_truncated: truncated,
        errors: serde_json::from_str(&errors).unwrap_or_default(),
        updated_at: updated,
    })
}
pub fn operation_results(
    query: DuplicateOperationQuery,
    state: &AppState,
) -> Result<DuplicateOperationPage, String> {
    let conn = journal(state, false)?;
    let id = op_id(&query.operation_id)?;
    load_plan(&conn, id)?;
    let start = match query.cursor.as_ref() {
        None => -1,
        Some(c) => {
            let parts: Vec<_> = c.split(':').collect();
            if parts.len() != 3 || parts[0] != "op" || parts[1] != query.operation_id {
                return Err("处理结果分页已过期".into());
            }
            parts[2].parse().map_err(|_| "处理结果游标无效")?
        }
    };
    let limit = if query.limit <= 0 {
        50
    } else {
        query.limit.clamp(1, 200)
    };
    let only_fail = query.state.as_deref() == Some("failed");
    let mut stmt=conn.prepare("SELECT ordinal,payload,state,code,message,file_moved,db_updated FROM items WHERE operation=?1 AND ordinal>?2 AND (?3=0 OR state IN ('failed','skipped')) ORDER BY ordinal LIMIT ?4").map_err(|e|e.to_string())?;
    let mut rows = stmt
        .query(params![id, start, only_fail as i64, limit + 1])
        .map_err(|e| e.to_string())?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let item: Item = serde_json::from_str(&row.get::<_, String>(1).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        items.push(DuplicateOperationResult {
            ordinal: row.get(0).map_err(|e| e.to_string())?,
            image_id: Some(item.image.id),
            file_name: item.image.file_name,
            path: Some(item.image.path),
            state: row.get(2).map_err(|e| e.to_string())?,
            code: row.get(3).map_err(|e| e.to_string())?,
            message: row.get(4).map_err(|e| e.to_string())?,
            file_moved: row.get::<_, i64>(5).map_err(|e| e.to_string())? != 0,
            db_updated: row.get::<_, i64>(6).map_err(|e| e.to_string())? != 0,
        });
    }
    let more = items.len() > limit as usize;
    items.truncate(limit as usize);
    let next_cursor = if more {
        items
            .last()
            .map(|i| format!("op:{}:{}", query.operation_id, i.ordinal))
    } else {
        None
    };
    Ok(DuplicateOperationPage {
        operation_id: query.operation_id,
        items,
        next_cursor,
    })
}

fn active_paths() -> &'static std::sync::Mutex<HashSet<PathBuf>> {
    static ACTIVE: std::sync::OnceLock<std::sync::Mutex<HashSet<PathBuf>>> =
        std::sync::OnceLock::new();
    ACTIVE.get_or_init(Default::default)
}
pub fn is_running(state: &AppState) -> bool {
    active_paths()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&journal_path(state))
}
struct Lease(PathBuf);
impl Drop for Lease {
    fn drop(&mut self) {
        active_paths()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}
fn ownership(conn: &Connection, item: &Item) -> Result<Option<i64>, String> {
    conn.query_row(
        "SELECT operation FROM owners WHERE image_pk=?1 AND public_hash=?2",
        params![item.pk, item.public_hash],
        |r| r.get(0),
    )
    .optional()
    .map_err(|e| e.to_string())
}
fn main_write(state: &AppState, binding: &str) -> Result<Connection, String> {
    if cache::db_binding(state)? != binding {
        return Err("主库已更换，已停止该批次".into());
    }
    let conn = Connection::open_with_flags(&state.database_path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|e| e.to_string())?;
    let schema: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if schema != 3 {
        return Err("当前数据库不是schema 3，未写入".into());
    }
    conn.busy_timeout(std::time::Duration::from_secs(2))
        .map_err(|e| e.to_string())?;
    conn.execute_batch("PRAGMA foreign_keys=ON;PRAGMA cache_size=-4096;PRAGMA mmap_size=0;")
        .map_err(|e| e.to_string())?;
    Ok(conn)
}
// Bounded alias pages; cached identities avoid touching unrelated files.
// If the disposable cache was cleared, only file metadata is inspected.
fn validate_physical_aliases(
    targets: &[&Item],
    state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    if targets.is_empty() {
        return Ok(());
    }
    let identities: HashSet<_> = targets
        .iter()
        .map(|t| digest(t.identity.as_bytes()))
        .collect();
    let actual_identities: HashSet<_> = targets.iter().map(|t| t.identity.as_str()).collect();
    let paths: HashSet<_> = targets.iter().map(|t| t.canonical.as_str()).collect();
    let db = cache::library(state)?;
    let indexed = cache::open_existing(state, false).is_ok();
    if indexed {
        db.execute(
            "ATTACH DATABASE ?1 AS derived",
            [state
                .data_dir
                .join("cache/duplicates.sqlite")
                .to_string_lossy()
                .as_ref()],
        )
        .map_err(|e| e.to_string())?;
    }
    let sql = if indexed {
        "SELECT i.image_pk,i.id,i.path,f.identity,f.public_hash FROM images i LEFT JOIN derived.fingerprints f ON f.image_pk=i.image_pk WHERE COALESCE(i.trashed,0)=0 AND COALESCE(i.source,'')<>'reference' AND i.image_pk>?1 ORDER BY i.image_pk LIMIT 1000"
    } else {
        "SELECT image_pk,id,path,NULL,NULL FROM images WHERE COALESCE(trashed,0)=0 AND COALESCE(source,'')<>'reference' AND image_pk>?1 ORDER BY image_pk LIMIT 1000"
    };
    let mut after = i64::MIN;
    loop {
        // A pause acknowledgement writes task_runs. Do not retain a main-library
        // reader across this callback when the main DB uses rollback journaling.
        control()?;
        let page = {
            let mut stmt = db.prepare(sql).map_err(|e| e.to_string())?;
            let page = stmt
                .query_map([after], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<Vec<u8>>>(3)?,
                        r.get::<_, Option<Vec<u8>>>(4)?,
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
        for (pk, id, path, cached_identity, public) in page {
            after = pk;
            if cached_identity
                .as_ref()
                .is_some_and(|i| !identities.contains(&hex(i)))
                && public
                    .as_ref()
                    .is_some_and(|p| hex(p) == digest(id.as_bytes()))
            {
                continue;
            }
            let Ok(signature) = fingerprint::source_signature(Path::new(&path)) else {
                continue;
            };
            if actual_identities.contains(signature.identity.as_str())
                || canonical(Path::new(&path)).is_ok_and(|p| paths.contains(p.as_str()))
            {
                return Err(format!(
                    "samePhysicalFile:仍有未批准的图库项指向该物理文件，未移动：{path}"
                ));
            }
        }
    }
    Ok(())
}
pub fn run_operation(
    operation: &str,
    state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>,
    progress: &mut dyn FnMut(&DuplicateOperationStatus),
) -> Result<(), String> {
    writable(state)?;
    let path = journal_path(state);
    if !active_paths()
        .lock()
        .map_err(|e| e.to_string())?
        .insert(path.clone())
    {
        return Err("已有整理操作运行".into());
    }
    let _lease = Lease(path);
    let conn = journal(state, false)?;
    let binding: String = conn
        .query_row("SELECT value FROM meta WHERE key='binding'", [], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    let id = op_id(operation)?;
    let (plan, phase, expires) = load_plan(&conn, id)?;
    if bulk::exists(&conn, id)? {
        return bulk::run(
            operation, &conn, plan, &phase, expires, state, control, progress,
        );
    }
    if !["prepared", "partial", "cancelled", "interrupted"].contains(&phase.as_str()) {
        return Err("此处理批次已完成或正在运行".into());
    }
    if phase == "prepared" && timestamp() > expires {
        return Err("处理预览已过期，请重新预览".into());
    }
    let rows = item_rows(&conn, id)?;
    let retry = phase != "prepared";
    let action = plan.request.action.as_str();
    let source_revision = *state
        .startup_cleanup_generation
        .lock()
        .map_err(|e| e.to_string())?;
    let preflight = (|| -> Result<(), String> {
        // Preflight the entire bounded plan before changing the first source record.
        // A changed target/count or unavailable keeper requires a new confirmation.
        if action != "restore" {
            if !retry {
                let mut current = Vec::new();
                if plan.batches.is_empty() {
                    current = plan_items(&plan.request, state)?.1;
                } else {
                    for batch in &plan.batches {
                        current.extend(plan_items(batch, state)?.1);
                    }
                }
                let old: HashSet<_> = rows
                    .iter()
                    .map(|(_, i, _, _)| (i.pk, i.signature.clone()))
                    .collect();
                let new: HashSet<_> = current
                    .iter()
                    .map(|i| (i.pk, i.signature.clone()))
                    .collect();
                if old != new {
                    return Err("预览后的处理范围已变化，请重新预览确认".into());
                }
            }
            for keep in &plan.keepers {
                if rows
                    .iter()
                    .filter(|(_, _, s, _)| s != "succeeded")
                    .all(|(_, _, _, m)| *m)
                {
                    break;
                }
                validate_item(keep, state, false)?;
                hash_file(keep, control)?;
            }
        }
        for (_, target, phase, moved) in &rows {
            if phase == "succeeded" || *moved {
                continue;
            }
            validate_item(target, state, action != "trash")?;
            if action == "systemTrash" {
                hash_file(target, control)?;
            }
        }
        let uncertain:i64=conn.query_row("SELECT COUNT(*) FROM items WHERE operation=?1 AND code IN ('fsIntent','fsUncertain') AND file_moved=0",[id],|r|r.get(0)).map_err(|e|e.to_string())?;
        if uncertain > 0 {
            return Err("有未确认的系统移动意图，禁止再次移动，请检查Windows回收站和原路径".into());
        }
        if action == "systemTrash" {
            validate_physical_aliases(
                &rows
                    .iter()
                    .filter(|(_, _, s, m)| s != "succeeded" && !m)
                    .map(|(_, i, _, _)| i)
                    .collect::<Vec<_>>(),
                state,
                control,
            )?;
        }
        Ok(())
    })();
    if let Err(error) = preflight {
        conn.execute(
            "UPDATE operations SET state='failed',errors=?2,updated=?3 WHERE id=?1",
            params![
                id,
                serde_json::to_string(&vec![error.clone()]).unwrap(),
                timestamp()
            ],
        )
        .map_err(|e| e.to_string())?;
        progress(&operation_status(operation, state)?);
        return Err(error);
    }
    conn.execute(
        "UPDATE operations SET state='running',updated=?2 WHERE id=?1",
        params![id, timestamp()],
    )
    .map_err(|e| e.to_string())?;
    let mut errors = Vec::<String>::new();
    let mut changed = Vec::<i64>::new();
    let mut terminal_error = None;
    let keep_map: std::collections::HashMap<_, _> =
        plan.keepers.iter().map(|k| (k.pk, k)).collect();
    for (n, target, old, moved) in rows {
        if old == "succeeded" {
            continue;
        }
        if let Err(error) = control() {
            terminal_error = Some(error);
            break;
        }
        if size(&journal_path(state)) > MAX_BYTES.saturating_sub(2 * 1024 * 1024) {
            terminal_error = Some("处理日志达到32 MiB预算，已完成项保留，余下部分未处理".into());
            break;
        }
        let relevant: Vec<_> = if target.keeper_pks.is_empty() {
            plan.keepers.iter().collect()
        } else {
            target
                .keeper_pks
                .iter()
                .filter_map(|p| keep_map.get(p).copied())
                .collect()
        };
        let keeps_valid = if action == "restore" || moved {
            Ok(())
        } else {
            relevant
                .iter()
                .try_for_each(|keep| validate_item(keep, state, false))
        };
        if let Err(error) = keeps_valid {
            terminal_error = Some(format!("保留图已失效，已停止余下处理：{error}"));
            break;
        }
        if n % 100 == 0 && action != "restore" && !moved {
            if let Err(error) = relevant
                .iter()
                .try_for_each(|keep| hash_file(keep, control))
            {
                terminal_error = Some(format!("保留图内容已变化，已停止余下处理：{error}"));
                break;
            }
        }
        // Hashing/control may pause. It must happen outside the main write lock.
        let checked = if moved {
            Ok(())
        } else {
            hash_file(&target, control)
        };
        let checked = checked.and_then(|_| {
            if action == "restore" || moved {
                return Ok(());
            }
            // Historical/manual plans must not bypass the new safety boundary.
            for keep in &relevant {
                let exact = plan.request.mode == "autoExact"
                    || plan.request.mode == "autoExactAll"
                    || (!target.sha.is_empty() && target.sha == keep.sha);
                if super::verification::verify_pair(
                    Path::new(&keep.image.path),
                    Path::new(&target.image.path),
                    exact,
                    control,
                )?
                .is_equivalent()
                {
                    return Ok(());
                }
            }
            Err("逐字节或局部内容复核未通过，保留此图".into())
        });
        let result = checked.and_then(|_| -> Result<(), String> {
            let _write = state.write_lock.lock().map_err(|e| e.to_string())?;
            writable(state)?;
            if cache::db_binding(state)?!=binding{return Err("主库已更换，未继续写入".into());}
            if action!="restore" && !moved {for keep in &relevant{validate_item(keep,state,false)?;}}
            if action=="systemTrash" && !moved && *state.startup_cleanup_generation.lock().map_err(|e|e.to_string())?!=source_revision{return Err("图库在物理别名复核后变化，请重新预览".into());}
            if action == "restore" {
                let source = op_id(
                    plan.request
                        .source_operation_id
                        .as_deref()
                        .ok_or("撤销来源缺失")?,
                )?;
                if ownership(&conn, &target)? != Some(source) {
                    return Err("undoOwnershipLost:此项随后被单独处理，不再属于该次撤销".into());
                }
                validate_item(&target, state, true)?;
                // Revoke first: a crash can lose convenience, never revive a later user deletion.
                conn.execute("DELETE FROM owners WHERE image_pk=?1", [target.pk])
                    .map_err(|e| e.to_string())?;
                let db = main_write(state,&binding)?;
                let count=db.execute(
                    "UPDATE images SET trashed=0 WHERE image_pk=?1 AND id=?2 AND trashed<>0",
                    params![target.pk, target.image.id],
                )
                .map_err(|e| e.to_string())?;
                if count!=1{return Err("还原未改变任何记录".into());}
                item_outcome(
                    &conn,
                    id,
                    n,
                    "succeeded",
                    "restored",
                    "已还原到图库",
                    false,
                    true,
                )?;
            } else if action == "trash" {
                validate_item(&target, state, false)?;
                // Intent is durable before the main change; a pending intent is not undoable.
                conn.execute("INSERT INTO owners(image_pk,public_hash,operation) VALUES(?1,?2,?3) ON CONFLICT(image_pk) DO UPDATE SET public_hash=excluded.public_hash,operation=excluded.operation",params![target.pk,target.public_hash,id]).map_err(|e|e.to_string())?;
                conn.execute(
                    "UPDATE items SET code='softIntent' WHERE operation=?1 AND ordinal=?2",
                    params![id, n],
                )
                .map_err(|e| e.to_string())?;
                let db = main_write(state,&binding)?;
                let count = db
                    .execute(
                        "UPDATE images SET trashed=1 WHERE image_pk=?1 AND id=?2 AND trashed=0",
                        params![target.pk, target.image.id],
                    )
                    .map_err(|e| e.to_string())?;
                if count != 1 {
                    return Err("图片回收状态已变化".into());
                }
                item_outcome(
                    &conn,
                    id,
                    n,
                    "succeeded",
                    "trashed",
                    "已移入应用回收站",
                    false,
                    true,
                )?;
            } else {
                let source = op_id(
                    plan.request
                        .source_operation_id
                        .as_deref()
                        .ok_or("软回收来源缺失")?,
                )?;
                if !moved && ownership(&conn, &target)? != Some(source) {
                    return Err("ownershipLost:此项不再属于原软回收批次".into());
                }
                if !moved {
                    validate_item(&target, state, true)?;
                    let actual = fingerprint::source_signature(Path::new(&target.image.path))?;
                    let actual_path = canonical(Path::new(&target.image.path))?;
                    if relevant
                        .iter()
                        .any(|k| k.identity == actual.identity || k.canonical == actual_path)
                    {
                        return Err("samePhysicalFile:目标与保留项指向同一文件，未移动".into());
                    }
                    crate::runtime::check_asset_write(Path::new(&target.image.path))?;
                    conn.execute("UPDATE items SET code='fsIntent',state='pending' WHERE operation=?1 AND ordinal=?2",params![id,n]).map_err(|e|e.to_string())?;
                    conn.execute_batch("PRAGMA wal_checkpoint(FULL);")
                        .map_err(|e| e.to_string())?;
                    crate::library::recycle_duplicate_source(Path::new(&target.image.path))?;
                    item_outcome(
                        &conn,
                        id,
                        n,
                        "failed",
                        "indexPending",
                        "原文件已移入系统回收站，正在移除图库索引",
                        true,
                        false,
                    )?;
                    // Physical movement irrevocably revokes soft undo before any SQL deletion.
                    conn.execute("DELETE FROM owners WHERE image_pk=?1", [target.pk])
                        .map_err(|e| e.to_string())?;
                }
                let db = main_write(state,&binding)?;
                let current = main_image(&db, target.pk)?
                    .ok_or("indexMissing:无法确认原图库索引状态，请检查记录")?;
                if digest(current.id.as_bytes()) != target.public_hash
                    || current.path != target.image.path
                    || !current.trashed
                {
                    return Err("indexChanged:图库索引已变化，不再自动删除".into());
                }
                if Path::new(&target.image.path).exists() {
                    return Err("pathReappeared:原路径又出现文件，保留索引并要求人工检查".into());
                }
                let count=db.execute(
                    "DELETE FROM images WHERE image_pk=?1 AND id=?2 AND trashed<>0",
                    params![target.pk, target.image.id],
                )
                .map_err(|e| e.to_string())?;
                if count!=1{return Err("系统索引移除未改变任何记录".into());}
                item_outcome(
                    &conn,
                    id,
                    n,
                    "succeeded",
                    "systemTrashed",
                    "已移入系统回收站并移除图库索引",
                    true,
                    true,
                )?;
            }
            Ok(())
        });
        match result {
            Ok(()) => changed.push(target.pk),
            Err(error) => {
                let (code, message) = error
                    .split_once(':')
                    .map(|(c, m)| (c.to_string(), m.to_string()))
                    .unwrap_or(("itemFailed".into(), error.clone()));
                let file_moved: bool = conn
                    .query_row(
                        "SELECT file_moved FROM items WHERE operation=?1 AND ordinal=?2",
                        params![id, n],
                        |r| Ok(r.get::<_, i64>(0)? != 0),
                    )
                    .map_err(|e| e.to_string())?;
                item_outcome(
                    &conn,
                    id,
                    n,
                    "failed",
                    if file_moved { "indexPending" } else { &code },
                    &message,
                    file_moved,
                    false,
                )?;
                if errors.len() < 20 {
                    errors.push(format!("{}：{}", target.image.file_name, message));
                }
            }
        }
        conn.execute(
            "UPDATE operations SET updated=?2 WHERE id=?1",
            params![id, timestamp()],
        )
        .map_err(|e| e.to_string())?;
        if n % 25 == 0 {
            progress(&operation_status(operation, state)?);
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                .map_err(|e| e.to_string())?;
        }
    }
    let review = cache::review_affected(
        &plan.request.generation,
        &changed,
        &plan
            .keepers
            .iter()
            .map(|k| k.image.id.clone())
            .collect::<Vec<_>>(),
        state,
    );
    let (affected, truncated) = match review {
        Ok(v) => v,
        Err(e) => {
            errors.push(format!("已处理结果保留，局部重审待更新：{e}"));
            (vec![], true)
        }
    };
    if let Some(error) = terminal_error.as_ref() {
        errors.push(error.clone());
    }
    let status = operation_status(operation, state)?;
    let final_phase = if terminal_error
        .as_ref()
        .is_some_and(|e| e.contains("取消") || e.contains("cancel"))
    {
        "cancelled"
    } else if terminal_error.is_some() || status.failed > 0 {
        "partial"
    } else {
        "completed"
    };
    conn.execute(
        "UPDATE operations SET state=?2,errors=?3,affected=?4,truncated=?5,updated=?6 WHERE id=?1",
        params![
            id,
            final_phase,
            serde_json::to_string(&errors).unwrap(),
            serde_json::to_string(&affected).unwrap(),
            truncated as i64,
            timestamp()
        ],
    )
    .map_err(|e| e.to_string())?;
    if !changed.is_empty() {
        cache::membership_invalidated(state);
        if let Ok(mut library) = state.library.lock() {
            *library = None;
        }
        if let Ok(mut revision) = state.startup_cleanup_generation.lock() {
            *revision += 1;
        }
    }
    progress(&operation_status(operation, state)?);
    if let Some(error) = terminal_error {
        return Err(error);
    }
    if status.failed > 0 {
        return Err("部分图片未处理，请查看逐项结果".into());
    }
    Ok(())
}

pub fn clear_records(confirmed: bool, state: &AppState) -> Result<(), String> {
    writable(state)?;
    if !confirmed {
        return Err("清理记录会放弃批次撤销，请明确确认".into());
    }
    let _write = state.write_lock.lock().map_err(|e| e.to_string())?;
    if active_paths()
        .lock()
        .map_err(|e| e.to_string())?
        .contains(&journal_path(state))
        || state.jobs.list()?.iter().any(|j| {
            ["duplicateManagement", "duplicateUndo"].contains(&j.request.kind.as_str())
                && ["queued", "running", "pausing", "paused", "cancelling"]
                    .contains(&j.state.as_str())
        })
    {
        return Err("整理任务运行中，不能清理记录".into());
    }
    if !journal_path(state).exists() {
        return Ok(());
    }
    // Explicit reset must also work after a main-file replacement or a full log.
    let conn = Connection::open_with_flags(journal_path(state), OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|e| e.to_string())?;
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if version != 1 {
        return Err("处理日志版本不兼容，未清理".into());
    }
    let pending: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM items WHERE (file_moved=1 AND db_updated=0) OR code IN ('fsIntent','fsUncertain')",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if pending > 0 {
        return Err("仍有系统文件移动待确认或索引待修复项，不能清理记录".into());
    }
    bulk::schema(&conn)?;
    conn.execute_batch("DELETE FROM bulk_group_keepers;DELETE FROM bulk_keepers;DELETE FROM bulk_members;DELETE FROM bulk_candidates;DELETE FROM bulk_runs;DELETE FROM owners;DELETE FROM items;DELETE FROM operations;PRAGMA wal_checkpoint(TRUNCATE);VACUUM;").map_err(|e|e.to_string())?;
    conn.execute(
        "UPDATE meta SET value=?1 WHERE key='binding'",
        [cache::db_binding(state)?],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
#[path = "management_tests.rs"]
mod tests;

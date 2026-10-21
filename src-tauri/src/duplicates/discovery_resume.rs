//! A resumable draft, bound to the complete numeric input and fingerprint versions.
//! Only derived cache tables are written; published results never move until complete.
use super::super::index::{AnchorCheckpoint, EvidenceResume, IndexRecord};
use super::*;

const SCHEMA:&str="CREATE TABLE IF NOT EXISTS discovery_checkpoint(id INTEGER PRIMARY KEY CHECK(id=1),generation INTEGER NOT NULL,input_key BLOB NOT NULL,next_anchor INTEGER NOT NULL DEFAULT 0,layout BLOB,coverage TEXT NOT NULL,exact_done INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS discovery_covered(unit INTEGER PRIMARY KEY) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS discovery_dense(reference INTEGER PRIMARY KEY) WITHOUT ROWID;";

pub(super) fn input_key(
    conn: &Connection,
    records: &[IndexRecord],
    sensitivity: &str,
    control: &mut dyn FnMut() -> Result<(), String>,
) -> Result<Vec<u8>, String> {
    let mut digest = Sha256::new();
    digest.update(b"illuTag-discovery-resume-v1");
    digest.update(index::CANDIDATE_VERSION.as_bytes());
    digest.update(super::super::evidence::POLICY.as_bytes());
    digest.update(sensitivity.as_bytes());
    for (n, r) in records.iter().enumerate() {
        if n % 4096 == 0 {
            control()?;
        }
        digest.update(r.image_pk.to_le_bytes());
        digest.update(r.image_order.to_le_bytes());
        digest.update(r.file_size.to_le_bytes());
        digest.update(r.width.to_le_bytes());
        digest.update(r.height.to_le_bytes());
        digest.update(r.sha256);
        if let Some(f) = &r.near {
            digest.update([1]);
            digest.update(f.phash.to_le_bytes());
            digest.update(f.dhash.to_le_bytes());
            digest.update(f.color);
            digest.update(f.width.to_le_bytes());
            digest.update(f.height.to_le_bytes());
            digest.update([f.low_information as u8]);
        } else {
            digest.update([0]);
        }
    }
    let mut stmt = conn
        .prepare("SELECT image_pk,public_hash,signature FROM fingerprints ORDER BY image_pk")
        .map_err(|e| e.to_string())?;
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
    let mut n = 0;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        if n % 4096 == 0 {
            control()?;
        }
        n += 1;
        digest.update(
            row.get::<_, i64>(0)
                .map_err(|e| e.to_string())?
                .to_le_bytes(),
        );
        digest.update(row.get::<_, Vec<u8>>(1).map_err(|e| e.to_string())?);
        digest.update(row.get::<_, Vec<u8>>(2).map_err(|e| e.to_string())?);
    }
    Ok(digest.finalize().to_vec())
}
pub(super) struct Journal {
    pub generation: i64,
    pub coverage: DuplicateCoverage,
    pub exact_done: bool,
    pub index: EvidenceResume,
    next: usize,
    layout: Option<[u8; 32]>,
    covered: Vec<usize>,
    dense: Vec<usize>,
    last: Instant,
    count: usize,
}
pub(super) fn load(conn: &Connection, key: &[u8]) -> Result<Option<Journal>, String> {
    conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    let row:Option<(i64,i64,Option<Vec<u8>>,String,bool)>=conn.query_row("SELECT c.generation,c.next_anchor,c.layout,c.coverage,c.exact_done FROM discovery_checkpoint c JOIN generations g ON g.generation=c.generation WHERE c.input_key=?1 AND g.state='building'",[key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional().map_err(|e|e.to_string())?;
    let Some((generation, next, layout, coverage, exact_done)) = row else {
        return Ok(None);
    };
    let layout = layout
        .map(|v| v.try_into().map_err(|_| "查重断点损坏".to_string()))
        .transpose()?;
    let mut covered = std::collections::HashSet::new();
    let mut s = conn
        .prepare("SELECT unit FROM discovery_covered")
        .map_err(|e| e.to_string())?;
    for row in s
        .query_map([], |r| r.get::<_, i64>(0))
        .map_err(|e| e.to_string())?
    {
        covered
            .insert(usize::try_from(row.map_err(|e| e.to_string())?).map_err(|_| "查重断点损坏")?);
    }
    let mut s = conn
        .prepare("SELECT reference FROM discovery_dense")
        .map_err(|e| e.to_string())?;
    let dense = s
        .query_map([], |r| r.get::<_, usize>(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(Some(Journal {
        generation,
        coverage: serde_json::from_str(&coverage).map_err(|e| format!("查重断点损坏：{e}"))?,
        exact_done,
        index: EvidenceResume {
            next_anchor: usize::try_from(next).map_err(|_| "查重断点损坏")?,
            covered,
            dense,
            layout,
        },
        next: next as usize,
        layout,
        covered: vec![],
        dense: vec![],
        last: Instant::now(),
        count: 0,
    }))
}
pub(super) fn create(conn: &Connection, generation: i64, key: &[u8]) -> Result<Journal, String> {
    conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    conn.execute_batch("BEGIN IMMEDIATE; DELETE FROM discovery_checkpoint; DELETE FROM discovery_covered; DELETE FROM discovery_dense;").map_err(|e|e.to_string())?;
    let result = conn.execute(
        "INSERT INTO discovery_checkpoint(id,generation,input_key,coverage) VALUES(1,?1,?2,?3)",
        params![
            generation,
            key,
            serde_json::to_string(&DuplicateCoverage::default()).unwrap()
        ],
    );
    if let Err(e) = result {
        let _ = conn.execute_batch("ROLLBACK");
        return Err(e.to_string());
    }
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    load(conn, key)?.ok_or_else(|| "无法创建查重断点".into())
}
impl Journal {
    pub fn record(&mut self, c: AnchorCheckpoint<'_>, coverage: &DuplicateCoverage) -> bool {
        let initial = self.layout.is_none();
        self.next = c.next_anchor;
        self.layout = Some(c.layout);
        self.exact_done = true;
        self.covered.extend_from_slice(c.newly_covered);
        self.dense.extend(c.dense_reference);
        self.coverage = coverage.clone();
        self.count += 1;
        initial || self.count >= 512 || self.last.elapsed().as_secs() >= 1
    }
    pub fn flush(
        &mut self,
        conn: &Connection,
        control: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<(), String> {
        cache::controlled_write(conn, control, || conn.execute_batch("BEGIN IMMEDIATE"))?;
        let result = (|| -> Result<(), String> {
            conn.execute("UPDATE discovery_checkpoint SET next_anchor=?1,layout=?2,coverage=?3,exact_done=?4 WHERE id=1 AND generation=?5",params![self.next as i64,self.layout.as_ref().map(|x|x.as_slice()),serde_json::to_string(&self.coverage).unwrap(),self.exact_done,self.generation]).map_err(|e|e.to_string())?;
            let mut c = conn
                .prepare_cached("INSERT OR IGNORE INTO discovery_covered VALUES(?1)")
                .map_err(|e| e.to_string())?;
            for &unit in &self.covered {
                c.execute([unit as i64]).map_err(|e| e.to_string())?;
            }
            let mut d = conn
                .prepare_cached("INSERT OR IGNORE INTO discovery_dense VALUES(?1)")
                .map_err(|e| e.to_string())?;
            for &pk in &self.dense {
                d.execute([pk as i64]).map_err(|e| e.to_string())?;
            }
            Ok(())
        })();
        if let Err(e) = result {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(e);
        }
        conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
        self.covered.clear();
        self.dense.clear();
        self.count = 0;
        self.last = Instant::now();
        Ok(())
    }
}
pub(super) fn forget(conn: &Connection) -> Result<(), String> {
    conn.execute_batch("DELETE FROM discovery_checkpoint; DELETE FROM discovery_covered; DELETE FROM discovery_dense;").map_err(|e|e.to_string())
}

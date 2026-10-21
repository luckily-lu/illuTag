//! Compact durable outcomes are separate from bounded diagnostic JSON samples.
//! Source versions bind both fingerprint signature and SHA; policy changes clear them.
use super::{
    cache,
    evidence::{Evidence, POLICY},
};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS evidence_versions(id INTEGER PRIMARY KEY,image_pk INTEGER NOT NULL,version BLOB NOT NULL,UNIQUE(image_pk,version));
CREATE TABLE IF NOT EXISTS evidence_outcomes(av INTEGER NOT NULL,bv INTEGER NOT NULL,level INTEGER NOT NULL,reason TEXT NOT NULL,PRIMARY KEY(av,bv)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS evidence_diagnostics(a INTEGER NOT NULL,b INTEGER NOT NULL,result TEXT NOT NULL,PRIMARY KEY(a,b)) WITHOUT ROWID;";
fn version(signature: &[u8], sha: &[u8]) -> Vec<u8> {
    let mut digest = Sha256::new();
    digest.update(signature);
    digest.update(sha);
    digest.finalize().to_vec()
}
fn code(answer: &Evidence) -> Option<i64> {
    match (answer.automatic, answer.level.as_str()) {
        (true, "raster") => Some(1),
        (false, "difference") => Some(2),
        (false, "lossy") => Some(3),
        _ => None,
    }
}
pub(super) fn initialize(conn: &Connection, force: bool) -> Result<(), String> {
    conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    let old: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key='compact_evidence_policy'",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if force || old.as_deref() != Some(POLICY) {
        conn.execute_batch("BEGIN IMMEDIATE; DELETE FROM evidence_outcomes; DELETE FROM evidence_versions; DELETE FROM evidence_diagnostics;").map_err(|e|e.to_string())?;
        let result = conn.execute(
            "INSERT OR REPLACE INTO meta VALUES('compact_evidence_policy',?1)",
            [POLICY],
        );
        if let Err(e) = result {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(e.to_string());
        }
        conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    }
    Ok(())
}
pub(super) fn lookup(
    conn: &Connection,
    a: i64,
    b: i64,
    asig: &[u8],
    bsig: &[u8],
    asha: &[u8],
    bsha: &[u8],
) -> Result<Option<Evidence>, String> {
    let row:Option<(i64,String)>=conn.query_row(
        "SELECT o.level,o.reason FROM evidence_versions a CROSS JOIN evidence_versions b CROSS JOIN evidence_outcomes o ON o.av=a.id AND o.bv=b.id WHERE a.image_pk=?1 AND a.version=?3 AND b.image_pk=?2 AND b.version=?4",
        params![a,b,version(asig,asha),version(bsig,bsha)],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|e.to_string())?;
    row.map(|(level, reason)| match level {
        1 => Ok(Evidence::result("raster", &reason, true)),
        2 => Ok(Evidence::result("difference", &reason, false)),
        3 => Ok(Evidence::result("lossy", &reason, false)),
        _ => Err("验证结论缓存损坏".into()),
    })
    .transpose()
}
// Caller holds a short write transaction. No image IO or decode occurs here.
pub(super) fn insert(
    conn: &Connection,
    a: i64,
    b: i64,
    asig: &[u8],
    bsig: &[u8],
    asha: &[u8],
    bsha: &[u8],
    answer: &Evidence,
) -> Result<(), String> {
    let Some(level) = code(answer) else {
        return Ok(());
    };
    let av = version(asig, asha);
    let bv = version(bsig, bsha);
    let mut upsert = conn
        .prepare_cached("INSERT OR IGNORE INTO evidence_versions(image_pk,version) VALUES(?1,?2)")
        .map_err(|e| e.to_string())?;
    upsert.execute(params![a, av]).map_err(|e| e.to_string())?;
    upsert.execute(params![b, bv]).map_err(|e| e.to_string())?;
    conn.execute("INSERT OR REPLACE INTO evidence_outcomes SELECT a.id,b.id,?5,?6 FROM evidence_versions a CROSS JOIN evidence_versions b WHERE a.image_pk=?1 AND a.version=?3 AND b.image_pk=?2 AND b.version=?4",params![a,b,av,bv,level,answer.reason]).map_err(|e|e.to_string())?;
    Ok(())
}
pub(super) fn sample(conn: &Connection, a: i64, b: i64, answer: &Evidence) -> Result<(), String> {
    conn.execute(
        "INSERT OR REPLACE INTO evidence_diagnostics VALUES(?1,?2,?3)",
        params![
            a,
            b,
            serde_json::to_string(answer).map_err(|e| e.to_string())?
        ],
    )
    .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM evidence_diagnostics WHERE (a,b) IN (SELECT a,b FROM evidence_diagnostics ORDER BY a,b DESC LIMIT -1 OFFSET 256)",[]).map_err(|e|e.to_string())?;
    Ok(())
}
/// Incrementally reclaim verbose old rows into compact outcomes. SQLite can reuse
/// the freed pages; no full-cache copy or VACUUM is required.
pub(super) fn migrate(
    conn: &Connection,
    control: &mut dyn FnMut() -> Result<(), String>,
    progress: &mut dyn FnMut(i64, i64),
) -> Result<(), String> {
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM pair_evidence", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let mut done = 0;
    progress(done, total);
    loop {
        control()?;
        let rows = {
            let mut s=conn.prepare("SELECT a,b,algorithm,a_signature,b_signature,a_sha,b_sha,result FROM pair_evidence ORDER BY a,b LIMIT 256").map_err(|e|e.to_string())?;
            let rows = s
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Vec<u8>>(3)?,
                        r.get::<_, Vec<u8>>(4)?,
                        r.get::<_, Vec<u8>>(5)?,
                        r.get::<_, Vec<u8>>(6)?,
                        r.get::<_, String>(7)?,
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
        cache::controlled_write(conn, control, || conn.execute_batch("BEGIN IMMEDIATE"))?;
        let result = (|| -> Result<(), String> {
            for (a, b, policy, asig, bsig, asha, bsha, json) in &rows {
                if policy == POLICY {
                    if let Ok(answer) = serde_json::from_str::<Evidence>(json) {
                        insert(conn, *a, *b, asig, bsig, asha, bsha, &answer)?;
                    }
                }
                conn.execute(
                    "DELETE FROM pair_evidence WHERE a=?1 AND b=?2",
                    params![a, b],
                )
                .map_err(|e| e.to_string())?;
            }
            Ok(())
        })();
        if let Err(e) = result {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(e);
        }
        conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
        done += rows.len() as i64;
        progress(done, total);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_reuses_current_proofs_without_promoting_old_lossy_or_uncertain_results() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL); CREATE TABLE pair_evidence(a INTEGER,b INTEGER,algorithm TEXT,a_signature BLOB,b_signature BLOB,a_sha BLOB,b_sha BLOB,result TEXT,PRIMARY KEY(a,b));").unwrap();
        initialize(&c, false).unwrap();
        for (b, policy, answer) in [
            (2, POLICY, Evidence::result("raster", "full_pixels", true)),
            (
                3,
                "illuTag-replacement-v1",
                Evidence::result("raster", "old_lossy", true),
            ),
            (4, POLICY, Evidence::result("uncertain", "offline", false)),
            (5, POLICY, Evidence::result("lossy", "suggestion", false)),
        ] {
            c.execute(
                "INSERT INTO pair_evidence VALUES(1,?1,?2,?3,?3,?4,?4,?5)",
                params![
                    b,
                    policy,
                    vec![1_u8; 32],
                    vec![2_u8; 32],
                    serde_json::to_string(&answer).unwrap()
                ],
            )
            .unwrap();
        }
        migrate(&c, &mut || Ok(()), &mut |_, _| {}).unwrap();
        assert!(
            lookup(&c, 1, 2, &[1; 32], &[1; 32], &[2; 32], &[2; 32])
                .unwrap()
                .unwrap()
                .automatic
        );
        for b in [3, 4] {
            assert!(lookup(&c, 1, b, &[1; 32], &[1; 32], &[2; 32], &[2; 32])
                .unwrap()
                .is_none());
        }
        assert!(
            !lookup(&c, 1, 5, &[1; 32], &[1; 32], &[2; 32], &[2; 32])
                .unwrap()
                .unwrap()
                .automatic
        );
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM pair_evidence", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    #[test]
    fn compact_outcomes_exceed_the_old_limit_without_losing_early_results() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL)")
            .unwrap();
        initialize(&c, false).unwrap();
        c.execute_batch("BEGIN").unwrap();
        let answer = Evidence::result("difference", "changed_detail", false);
        for b in 2..=66_000 {
            insert(&c, 1, b, &[1; 32], &[2; 32], &[3; 32], &[4; 32], &answer).unwrap();
        }
        c.execute_batch("COMMIT").unwrap();
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM evidence_outcomes", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            65_999
        );
        assert_eq!(
            lookup(&c, 1, 2, &[1; 32], &[2; 32], &[3; 32], &[4; 32])
                .unwrap()
                .unwrap()
                .reason,
            "changed_detail"
        );
        assert!(lookup(&c, 1, 2, &[9; 32], &[2; 32], &[3; 32], &[4; 32])
            .unwrap()
            .is_none());
        assert!(lookup(&c, 1, 2, &[1; 32], &[2; 32], &[8; 32], &[4; 32])
            .unwrap()
            .is_none());
        c.execute("UPDATE meta SET value='old'", []).unwrap();
        initialize(&c, false).unwrap();
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM evidence_outcomes", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

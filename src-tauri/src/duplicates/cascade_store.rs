//! Compact, versioned three-round outcomes. Per-image signatures are not repeated per pair.
use super::cascade_pixels::{Analysis, POLICY};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
pub(super) fn initialize(c: &Connection, force: bool) -> Result<(), String> {
    c.execute_batch("CREATE TABLE IF NOT EXISTS cascade_versions(id INTEGER PRIMARY KEY,image_pk INTEGER NOT NULL,version BLOB NOT NULL,UNIQUE(image_pk,version));
    CREATE TABLE IF NOT EXISTS cascade_verdicts(av INTEGER NOT NULL,bv INTEGER NOT NULL,kind INTEGER NOT NULL,reason INTEGER NOT NULL,PRIMARY KEY(av,bv)) WITHOUT ROWID;
    CREATE TABLE IF NOT EXISTS cascade_native(image_pk INTEGER PRIMARY KEY,version BLOB NOT NULL,digest BLOB NOT NULL);").map_err(|e|e.to_string())?;
    if force {
        c.execute_batch(
            "DELETE FROM cascade_verdicts;DELETE FROM cascade_versions;DELETE FROM cascade_native;",
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
pub(super) fn version(sig: &[u8], sha: &[u8]) -> Vec<u8> {
    let mut h = Sha256::new();
    h.update(POLICY);
    h.update(sig);
    h.update(sha);
    h.finalize().to_vec()
}
fn reason_code(reason: &str) -> i64 {
    match reason {
        "all_native_samples_equal" => 1,
        "multiscale_content_passed" => 2,
        "alpha_detail_difference" => 3,
        "aspect_ratio_difference" => 4,
        "color_variant" => 5,
        "global_pixel_difference" => 6,
        "local_detail_difference" => 7,
        _ => 0,
    }
}
fn reason_name(code: i64) -> Result<&'static str, String> {
    Ok(match code {
        0 => "measured_detail_difference",
        1 => "all_native_samples_equal",
        2 => "multiscale_content_passed",
        3 => "alpha_detail_difference",
        4 => "aspect_ratio_difference",
        5 => "color_variant",
        6 => "global_pixel_difference",
        7 => "local_detail_difference",
        _ => return Err("三轮结论缓存损坏，请清理查重缓存后重试".into()),
    })
}
pub(super) fn lookup(
    c: &Connection,
    a: i64,
    b: i64,
    asig: &[u8],
    bsig: &[u8],
    asha: &[u8],
    bsha: &[u8],
) -> Result<Option<Analysis>, String> {
    let row:Option<(i64,i64)>=c.query_row("SELECT p.kind,p.reason FROM cascade_versions a CROSS JOIN cascade_versions b CROSS JOIN cascade_verdicts p ON p.av=a.id AND p.bv=b.id WHERE a.image_pk=?1 AND b.image_pk=?2 AND a.version=?3 AND b.version=?4",params![a,b,version(asig,asha),version(bsig,bsha)],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|e.to_string())?;
    row.map(|(kind, reason)| {
        let kind = match kind {
            1 => "exact",
            2 => "near",
            3 => "similar",
            _ => return Err("三轮结论缓存损坏，请清理查重缓存后重试".into()),
        };
        Ok(Analysis::new(kind, reason_name(reason)?, kind == "exact"))
    })
    .transpose()
}
pub(super) fn insert(
    c: &Connection,
    a: i64,
    b: i64,
    asig: &[u8],
    bsig: &[u8],
    asha: &[u8],
    bsha: &[u8],
    answer: &Analysis,
) -> Result<(), String> {
    insert_in_transaction(c, a, b, asig, bsig, asha, bsha, answer)
}

/// Caller owns the surrounding transaction. Used by the batched verifier.
pub(super) fn insert_in_transaction(
    c: &Connection,
    a: i64,
    b: i64,
    asig: &[u8],
    bsig: &[u8],
    asha: &[u8],
    bsha: &[u8],
    answer: &Analysis,
) -> Result<(), String> {
    let code = match answer.kind.as_str() {
        "exact" if answer.strict => 1,
        "near" if !answer.strict => 2,
        "similar" if !answer.strict => 3,
        _ => return Ok(()),
    };
    let av = version(asig, asha);
    let bv = version(bsig, bsha);
    c.execute(
        "INSERT OR IGNORE INTO cascade_versions(image_pk,version) VALUES(?1,?2)",
        params![a, av],
    )
    .map_err(|e| e.to_string())?;
    c.execute(
        "INSERT OR IGNORE INTO cascade_versions(image_pk,version) VALUES(?1,?2)",
        params![b, bv],
    )
    .map_err(|e| e.to_string())?;
    c.execute("INSERT OR REPLACE INTO cascade_verdicts SELECT a.id,b.id,?5,?6 FROM cascade_versions a CROSS JOIN cascade_versions b WHERE a.image_pk=?1 AND b.image_pk=?2 AND a.version=?3 AND b.version=?4",params![a,b,av,bv,code,reason_code(&answer.reason)]).map_err(|e|e.to_string())?;
    Ok(())
}

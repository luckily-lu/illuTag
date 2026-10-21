//! Retire obsolete verifier data only after a complete three-round publication.
//! Each statement commits at most 512 primary keys; no pause hook holds a writer.
use super::{cache, cascade_pixels::POLICY};
use crate::library::AppState;
use rusqlite::{params, Connection};

fn publication_is_current(c: &Connection, g: i64) -> Result<(), String> {
    let published: bool = c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key='published' AND value=?1)
         AND EXISTS(SELECT 1 FROM meta WHERE key=?2 AND value=?3)",
            params![g.to_string(), format!("policy:{g}"), POLICY],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if !published {
        return Err("仅在当前三轮结果完整发布后回收旧验证缓存；发布版本已变化，未继续清理".into());
    }
    Ok(())
}

fn retire_non_byte_checks(
    c: &Connection,
    g: i64,
    state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    // Retained kind=1 rows must not be rescanned from the beginning for every
    // kind=0 batch. RETURNING advances the actual deleted primary-key boundary.
    let mut last = (i64::MIN, i64::MIN, i64::MIN);
    loop {
        control()?;
        publication_is_current(c, g)?;
        let removed = cache::controlled_write(c, control, || {
            let mut statement = c.prepare(
                "DELETE FROM pair_checks WHERE (a,b,kind) IN
                (SELECT a,b,kind FROM pair_checks WHERE kind=0 AND (a,b,kind)>(?4,?5,?6)
                 ORDER BY a,b,kind LIMIT 512)
                AND EXISTS(SELECT 1 FROM meta WHERE key='published' AND value=?1)
                AND EXISTS(SELECT 1 FROM meta WHERE key=?2 AND value=?3)
                RETURNING a,b,kind",
            )?;
            let rows = statement.query_map(
                params![
                    g.to_string(),
                    format!("policy:{g}"),
                    POLICY,
                    last.0,
                    last.1,
                    last.2
                ],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                },
            )?;
            rows.collect::<Result<Vec<_>, _>>()
        })?;
        publication_is_current(c, g)?;
        cache::check_disk_on(c, &cache::cache_path(state))?;
        let Some(boundary) = removed.into_iter().max() else {
            return Ok(());
        };
        last = boundary;
    }
}

pub(super) fn retire_legacy(
    c: &Connection,
    g: i64,
    state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    publication_is_current(c, g)?;
    // Old outcome references must disappear before their version dictionary.
    // Byte-equality kind=1 is still used by the current three-round verifier.
    for (table, columns, predicate) in [
        ("evidence_outcomes", "av,bv", "1"),
        ("evidence_diagnostics", "a,b", "1"),
        ("pair_evidence", "a,b", "1"),
        ("pair_checks", "a,b,kind", "kind=0"),
        ("evidence_versions", "id", "1"),
    ] {
        control()?;
        publication_is_current(c, g)?;
        let exists: bool = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                [table],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !exists {
            continue;
        }
        if table == "pair_checks" {
            retire_non_byte_checks(c, g, state, control)?;
            continue;
        }
        let key = if columns.contains(',') {
            format!("({columns})")
        } else {
            columns.into()
        };
        // The publication predicates belong to the atomic DELETE too: a new
        // publication during a control/writer wait must not bypass the gate.
        let sql = format!(
            "DELETE FROM {table} WHERE {key} IN
            (SELECT {columns} FROM {table} WHERE {predicate} ORDER BY {columns} LIMIT 512)
            AND EXISTS(SELECT 1 FROM meta WHERE key='published' AND value=?1)
            AND EXISTS(SELECT 1 FROM meta WHERE key=?2 AND value=?3)"
        );
        loop {
            control()?;
            publication_is_current(c, g)?;
            let changed = cache::controlled_write(c, control, || {
                c.execute(&sql, params![g.to_string(), format!("policy:{g}"), POLICY])
            })?;
            publication_is_current(c, g)?;
            cache::check_disk_on(c, &cache::cache_path(state))?;
            if changed == 0 {
                break;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::duplicates::tests::Fixture;
    use std::fs;

    fn fixture() -> (Fixture, Connection) {
        let f = Fixture::new(0);
        let path = cache::cache_path(&f.state);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let c = Connection::open(path).unwrap();
        c.execute_batch("PRAGMA foreign_keys=ON;
            CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL) WITHOUT ROWID;
            INSERT INTO meta VALUES('published','7');
            CREATE TABLE evidence_versions(id INTEGER PRIMARY KEY,image_pk INTEGER,version BLOB);
            CREATE TABLE evidence_outcomes(av INTEGER REFERENCES evidence_versions(id),bv INTEGER REFERENCES evidence_versions(id),level INTEGER,reason TEXT,PRIMARY KEY(av,bv)) WITHOUT ROWID;
            CREATE TABLE evidence_diagnostics(a INTEGER,b INTEGER,result TEXT,PRIMARY KEY(a,b)) WITHOUT ROWID;
            CREATE TABLE pair_evidence(a INTEGER,b INTEGER,result TEXT,PRIMARY KEY(a,b)) WITHOUT ROWID;
            CREATE TABLE pair_checks(a INTEGER,b INTEGER,kind INTEGER,outcome INTEGER,PRIMARY KEY(a,b,kind)) WITHOUT ROWID;
            WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1538)
            INSERT INTO evidence_versions SELECT x,x,X'01' FROM n;
            INSERT INTO evidence_outcomes SELECT 1,id,2,'old' FROM evidence_versions WHERE id>1;
            INSERT INTO evidence_diagnostics VALUES(1,2,'legacy');
            INSERT INTO pair_evidence VALUES(1,2,'legacy');
            INSERT INTO pair_checks SELECT 1,id,0,1 FROM evidence_versions WHERE id>1;
            INSERT INTO pair_checks SELECT 1,id,1,1 FROM evidence_versions WHERE id>1;
            CREATE TABLE fingerprints(payload TEXT); INSERT INTO fingerprints VALUES('fingerprint');
            CREATE TABLE members(generation INTEGER,payload TEXT); INSERT INTO members VALUES(6,'old-complete'),(7,'current');
            CREATE TABLE cascade_verdicts(payload TEXT); INSERT INTO cascade_verdicts VALUES('current-verdict');
            CREATE TABLE cascade_versions(payload TEXT); INSERT INTO cascade_versions VALUES('current-version');
            CREATE TABLE cascade_pairs(payload TEXT); INSERT INTO cascade_pairs VALUES('current-pair');
            CREATE TABLE cascade_native(payload TEXT); INSERT INTO cascade_native VALUES('native-digest');
            CREATE TABLE cascade_round2(payload TEXT); INSERT INTO cascade_round2 VALUES('round2-checkpoint');
            CREATE TABLE discovery_checkpoint(payload TEXT); INSERT INTO discovery_checkpoint VALUES('checkpoint');
            CREATE TABLE workflow_choices(payload TEXT); INSERT INTO workflow_choices VALUES('manual-keep');
            CREATE TABLE workflow_plan(payload TEXT); INSERT INTO workflow_plan VALUES('current-plan');").unwrap();
        c.execute("INSERT INTO meta VALUES('policy:7',?1)", [POLICY])
            .unwrap();
        let operations = f.state.data_dir.join("operations");
        fs::create_dir_all(&operations).unwrap();
        Connection::open(operations.join("duplicate-management.sqlite"))
            .unwrap()
            .execute_batch(
                "CREATE TABLE undo_log(payload TEXT); INSERT INTO undo_log VALUES('undo-intact');",
            )
            .unwrap();
        (f, c)
    }
    fn count(c: &Connection, table: &str) -> i64 {
        c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn cascade_legacy_gc_requires_current_cascade_publication() {
        let (f, c) = fixture();
        assert!(retire_legacy(&c, 8, &f.state, &mut || Ok(())).is_err());
        assert_eq!(count(&c, "evidence_outcomes"), 1537);
        c.execute(
            "UPDATE meta SET value='illuTag-replacement-v2' WHERE key='policy:7'",
            [],
        )
        .unwrap();
        assert!(retire_legacy(&c, 7, &f.state, &mut || Ok(())).is_err());
        assert_eq!(count(&c, "evidence_versions"), 1538);
    }

    #[test]
    fn cascade_legacy_gc_cancels_in_short_batches_and_resumes_without_touching_current_data() {
        let (f, c) = fixture();
        let main_before = fs::read(&f.state.database_path).unwrap();
        let undo_path = f
            .state
            .data_dir
            .join("operations/duplicate-management.sqlite");
        let undo_before = fs::read(&undo_path).unwrap();
        let result = retire_legacy(&c, 7, &f.state, &mut || {
            assert!(
                c.is_autocommit(),
                "control may not pause inside a write transaction"
            );
            if count(&c, "evidence_outcomes") < 1537 {
                Err("cancelled".into())
            } else {
                Ok(())
            }
        });
        assert_eq!(result.unwrap_err(), "cancelled");
        assert_eq!(
            count(&c, "evidence_outcomes"),
            1025,
            "exactly one bounded batch committed"
        );
        assert_eq!(
            count(&c, "evidence_versions"),
            1538,
            "versions cannot precede outcome retirement"
        );
        retire_legacy(&c, 7, &f.state, &mut || Ok(())).unwrap();
        retire_legacy(&c, 7, &f.state, &mut || Ok(())).unwrap();
        for table in [
            "evidence_outcomes",
            "evidence_versions",
            "evidence_diagnostics",
            "pair_evidence",
        ] {
            assert_eq!(count(&c, table), 0, "{table}");
        }
        assert_eq!(count(&c, "pair_checks"), 1537);
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM pair_checks WHERE kind=1", [], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap(),
            1537
        );
        for table in [
            "fingerprints",
            "cascade_verdicts",
            "cascade_versions",
            "cascade_pairs",
            "cascade_native",
            "cascade_round2",
            "discovery_checkpoint",
            "workflow_choices",
            "workflow_plan",
        ] {
            assert_eq!(count(&c, table), 1, "{table} must remain intact");
        }
        assert_eq!(
            count(&c, "members"),
            2,
            "old and current published context remain intact"
        );
        assert_eq!(fs::read(&f.state.database_path).unwrap(), main_before);
        assert_eq!(fs::read(undo_path).unwrap(), undo_before);
    }

    #[test]
    fn cascade_legacy_gc_rechecks_publication_after_control_and_accepts_missing_legacy_tables() {
        let (f, c) = fixture();
        let mut changed = false;
        let result = retire_legacy(&c, 7, &f.state, &mut || {
            if !changed {
                c.execute("UPDATE meta SET value='8' WHERE key='published'", [])
                    .unwrap();
                changed = true;
            }
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(count(&c, "evidence_outcomes"), 1537);
        c.execute_batch("DROP TABLE evidence_outcomes; DROP TABLE evidence_versions; DROP TABLE evidence_diagnostics; DROP TABLE pair_evidence; DROP TABLE pair_checks; UPDATE meta SET value='7' WHERE key='published';").unwrap();
        retire_legacy(&c, 7, &f.state, &mut || Ok(())).unwrap();
        assert_eq!(count(&c, "fingerprints"), 1);
    }
}

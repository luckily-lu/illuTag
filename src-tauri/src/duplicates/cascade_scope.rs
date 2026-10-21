//! Exact cleanup is independent of the quality-ordered near plan. A larger
//! near survivor must not consume every member of a native-pixel component.
use super::*;

pub(super) fn exact_plan(
    conn: &Connection,
    generation: i64,
    state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>,
) -> Result<i64, String> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS cascade_scope_runs(
        generation INTEGER PRIMARY KEY,revision TEXT NOT NULL,membership TEXT NOT NULL,plan INTEGER NOT NULL);")
        .map_err(|e| e.to_string())?;
    let original = run(conn, generation)?;
    let revision = revision(generation, &original);
    let membership = cache::membership_token(state);
    let existing = conn.query_row(
        "SELECT plan FROM cascade_scope_runs WHERE generation=?1 AND revision=?2 AND membership=?3",
        params![generation, revision, membership], |r| r.get::<_, i64>(0),
    ).optional().map_err(|e| e.to_string())?;
    if let Some(plan) = existing {
        return Ok(plan);
    }
    control()?;
    cache::check_disk_on(conn, &cache::cache_path(state))?;
    let plan = serial();
    conn.execute_batch("DROP TABLE IF EXISTS temp.cascade_exact_nodes;
        CREATE TEMP TABLE cascade_exact_nodes(group_no INTEGER NOT NULL,image_pk INTEGER NOT NULL,protected INTEGER NOT NULL,priority INTEGER NOT NULL,random INTEGER NOT NULL,PRIMARY KEY(group_no,image_pk)) WITHOUT ROWID;
        DROP TABLE IF EXISTS temp.cascade_exact_draft;
        CREATE TEMP TABLE cascade_exact_draft(run INTEGER NOT NULL,image_pk INTEGER NOT NULL,keeper INTEGER,kind TEXT NOT NULL,manual INTEGER NOT NULL,PRIMARY KEY(run,image_pk)) WITHOUT ROWID;")
        .map_err(|e| e.to_string())?;
    // No control/progress hook runs while an attached-library reader is alive.
    // Current and carried-forward explicit protection always outrank cleanup.
    let active = category_sql("exact", original.0)?;
    conn.execute(&format!("INSERT INTO cascade_exact_nodes
        SELECT a.group_no,a.image_pk,
          CASE WHEN c.choice IN ('keep','anchor') OR EXISTS(SELECT 1 FROM workflow_keepers k WHERE k.generation=?1 AND k.image_pk=a.image_pk) THEN 1 ELSE 0 END,
          CASE WHEN c.choice IN ('clean','manual') THEN 1 ELSE 0 END,
          (a.image_pk*1103515245+?1)%2147483647
        FROM ({active}) a CROSS JOIN fingerprints f ON f.image_pk=a.image_pk
        LEFT JOIN workflow_choices c ON c.public_hash=f.public_hash AND c.signature=f.signature"),
        [generation]).map_err(|e| e.to_string())?;
    control()?;
    conn.execute("INSERT INTO cascade_exact_draft
        SELECT ?1,image_pk,CASE WHEN protected=1 OR image_pk=first_pk THEN NULL ELSE first_pk END,'exact',0
        FROM (SELECT image_pk,protected,FIRST_VALUE(image_pk) OVER
          (PARTITION BY group_no ORDER BY protected DESC,priority,random,image_pk) first_pk
          FROM cascade_exact_nodes)", [plan]).map_err(|e| e.to_string())?;
    control()?;
    if membership != cache::membership_token(state) || run(conn, generation)? != original {
        return Err("图库成员或人工选择已变化，请刷新后重新整理".into());
    }
    cache::check_disk_on(conn, &cache::cache_path(state))?;
    // The caller holds the workflow gate. Publish this independent pointer only
    // after all temporary work finishes; never pause while holding a writer.
    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| e.to_string())?;
    let result = (|| -> Result<(), String> {
        if membership != cache::membership_token(state) || run(conn, generation)? != original {
            return Err("图库成员或人工选择已变化，请刷新后重新整理".into());
        }
        conn.execute(
            "INSERT INTO workflow_plan SELECT * FROM cascade_exact_draft",
            [],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO cascade_scope_runs VALUES(?1,?2,?3,?4)",
            params![generation, revision, membership, plan],
        )
        .map_err(|e| e.to_string())?;
        cache::check_disk_on(conn, &cache::cache_path(state))?;
        Ok(())
    })();
    match result {
        Ok(()) => conn.execute_batch("COMMIT").map_err(|e| e.to_string())?,
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(error);
        }
    }
    conn.execute_batch("DROP TABLE cascade_exact_nodes; DROP TABLE cascade_exact_draft;")
        .map_err(|e| e.to_string())?;
    Ok(plan)
}

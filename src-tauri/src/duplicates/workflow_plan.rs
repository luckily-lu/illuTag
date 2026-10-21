//! One global survivor set for both categories. A survivor never becomes a target.
use super::*;

// Keep the image lookup as the outer loop. SQLite otherwise chooses the
// kind index and rescans every exact group for every image in the pool.
pub(super) const EXACT_MEMBERSHIP: &str = "SELECT 1 FROM members m INDEXED BY members_image CROSS JOIN groups g ON g.generation=m.generation AND g.group_no=m.group_no WHERE m.generation=?1 AND m.image_pk=f.image_pk AND g.kind=0";

// The signature predicate makes SQLite prefer the generation-only primary-key
// range on real caches. That rescans all members for each node. Keep this seek
// explicit too, and test the complete statement rather than only one EXISTS.
pub(super) fn nodes_sql() -> String {
    format!("SELECT f.image_pk,
        CASE WHEN c.choice IN ('keep','anchor') OR EXISTS(SELECT 1 FROM workflow_keepers k WHERE (k.generation=?1 OR (EXISTS(SELECT 1 FROM meta WHERE key='policy:'||?1 AND value IN ('illuTag-replacement-v2','illuTag-three-pass-v1')) AND k.generation=CAST((SELECT value FROM meta WHERE key='published') AS INTEGER))) AND k.image_pk=f.image_pk) THEN 0
             WHEN c.choice IN ('clean','manual') THEN 3
             WHEN EXISTS(SELECT 1 FROM meta WHERE key='policy:'||?1 AND value IN ('illuTag-replacement-v2','illuTag-three-pass-v1')) THEN 2
             WHEN EXISTS(SELECT 1 FROM workflow_class x WHERE x.run=?2 AND x.image_pk=f.image_pk AND x.kind IN ('similar','uncertain')) THEN 1 ELSE 2 END,
        CAST(f.width AS INTEGER)*f.height,f.size,COALESCE(c.choice,''),
        CASE WHEN EXISTS({EXACT_MEMBERSHIP}) THEN 'exact' ELSE 'near' END,
        (f.image_pk*1103515245+?1)%2147483647
        FROM (SELECT DISTINCT image_pk FROM members WHERE generation=?1) selected
        CROSS JOIN fingerprints f ON f.image_pk=selected.image_pk
        CROSS JOIN library.images i ON i.image_pk=f.image_pk
        LEFT JOIN workflow_choices c ON c.public_hash=f.public_hash AND c.signature=f.signature
        WHERE {ACTIVE} AND EXISTS(SELECT 1 FROM members m INDEXED BY members_image WHERE m.generation=?1 AND m.image_pk=f.image_pk AND m.public_hash=f.public_hash AND m.signature=f.signature)")
}

fn identity_keeper(conn: &Connection, pk: i64) -> Result<Option<i64>, String> {
    let component: i64 = conn
        .query_row(
            "SELECT COALESCE((SELECT component FROM workflow_exact_identity WHERE image_pk=?1),?1)",
            [pk],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if let Some(keeper) = conn
        .query_row(
            "SELECT keeper FROM workflow_identity_targets WHERE component=?1",
            [component],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
    {
        return Ok(Some(keeper));
    }
    let keeper = conn
        .query_row(
            "SELECT s.image_pk FROM workflow_identity_edges e
         CROSS JOIN workflow_identity_survivors s ON s.component=e.peer
         WHERE e.component=?1 LIMIT 1",
            [component],
            |r| r.get::<_, i64>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some(keeper) = keeper {
        // Only cache this component's resolution. Adding it to survivors would
        // incorrectly allow another near component to follow this near link.
        conn.execute(
            "INSERT INTO workflow_identity_targets VALUES(?1,?2)",
            params![component, keeper],
        )
        .map_err(|e| e.to_string())?;
    }
    // With no match, the caller retains this node in its own strict component;
    // later members then match that exact survivor without rewalking its edges.
    Ok(keeper)
}

pub(super) fn build(
    conn: &Connection,
    generation: i64,
    refinement: i64,
    state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    build_inner(conn, generation, refinement, state, control, false, false)
}

pub(super) const CHOICES_CHANGED: &str = "人工选择已更新，正在重新合并整理计划";

/// The job computes a draft without the workflow gate so a paused job does not
/// block review. Publication alone takes that gate and checks the choice revision.
pub(super) fn build_for_job(
    conn: &Connection,
    generation: i64,
    refinement: i64,
    state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    build_inner(conn, generation, refinement, state, control, true, false)
}

pub(super) fn publish_evidence(
    conn: &Connection,
    generation: i64,
    state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    build_inner(conn, generation, generation, state, control, true, true)
}

fn build_inner(
    conn: &Connection,
    generation: i64,
    refinement: i64,
    state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>,
    job: bool,
    publish_generation: bool,
) -> Result<(), String> {
    let plan = serial();
    let membership = cache::membership_token(state);
    let original = run(conn, generation)?;
    let cascade: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key=?1 AND value=?2)",
            params![
                format!("policy:{generation}"),
                super::super::cascade_pixels::POLICY
            ],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let choices: String = conn
        .query_row(
            "SELECT COALESCE((SELECT value FROM meta WHERE key='choice_revision'),'0')",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let choices_changed = || -> Result<bool, String> {
        Ok(conn
            .query_row(
                "SELECT COALESCE((SELECT value FROM meta WHERE key='choice_revision'),'0')",
                [],
                |r| r.get::<_, String>(0),
            )
            .map_err(|e| e.to_string())?
            != choices)
    };
    cache::check_disk_on(conn, &cache::cache_path(state))?;
    conn.execute_batch("DROP TABLE IF EXISTS temp.workflow_group_survivors;
        CREATE TEMP TABLE workflow_group_survivors(group_no INTEGER NOT NULL,image_pk INTEGER NOT NULL,PRIMARY KEY(group_no,image_pk)) WITHOUT ROWID;
        DROP TABLE IF EXISTS temp.workflow_nodes;
        CREATE TEMP TABLE workflow_nodes(image_pk INTEGER PRIMARY KEY,priority INTEGER NOT NULL,area INTEGER NOT NULL,bytes INTEGER NOT NULL,choice TEXT NOT NULL,kind TEXT NOT NULL,random INTEGER NOT NULL);
        DROP TABLE IF EXISTS temp.workflow_plan_draft;
        CREATE TEMP TABLE workflow_plan_draft(run INTEGER NOT NULL,image_pk INTEGER NOT NULL,keeper INTEGER,kind TEXT NOT NULL,manual INTEGER NOT NULL,PRIMARY KEY(run,image_pk)) WITHOUT ROWID;
        DROP TABLE IF EXISTS temp.workflow_edges_draft;
        CREATE TEMP TABLE workflow_edges_draft(run INTEGER NOT NULL,image_pk INTEGER NOT NULL,peer INTEGER NOT NULL,PRIMARY KEY(run,image_pk,peer)) WITHOUT ROWID;
        DROP TABLE IF EXISTS temp.workflow_exact_identity;
        CREATE TEMP TABLE workflow_exact_identity(image_pk INTEGER PRIMARY KEY,component INTEGER NOT NULL);
        DROP TABLE IF EXISTS temp.workflow_identity_edges;
        CREATE TEMP TABLE workflow_identity_edges(component INTEGER NOT NULL,peer INTEGER NOT NULL,PRIMARY KEY(component,peer)) WITHOUT ROWID;
        DROP TABLE IF EXISTS temp.workflow_identity_survivors;
        CREATE TEMP TABLE workflow_identity_survivors(component INTEGER NOT NULL,image_pk INTEGER NOT NULL,PRIMARY KEY(component,image_pk)) WITHOUT ROWID;
        DROP TABLE IF EXISTS temp.workflow_identity_targets;
        CREATE TEMP TABLE workflow_identity_targets(component INTEGER PRIMARY KEY,keeper INTEGER NOT NULL);").map_err(|e|e.to_string())?;
    conn.execute(
        "INSERT INTO workflow_edges_draft SELECT * FROM workflow_edges WHERE run=?1",
        [refinement],
    )
    .map_err(|e| e.to_string())?;
    conn.execute("INSERT OR IGNORE INTO workflow_edges_draft SELECT c.run,c.image_pk,g.reference_pk FROM workflow_class c JOIN groups g ON g.generation=?1 AND g.group_no=c.group_no WHERE c.run=?2 AND c.kind='near'",params![generation,refinement]).map_err(|e|e.to_string())?;
    conn.execute("INSERT OR IGNORE INTO workflow_edges_draft SELECT c.run,g.reference_pk,c.image_pk FROM workflow_class c JOIN groups g ON g.generation=?1 AND g.group_no=c.group_no WHERE c.run=?2 AND c.kind='near'",params![generation,refinement]).map_err(|e|e.to_string())?;
    if cascade {
        // Native-pixel equality permits substituting an endpoint, even when an
        // exact-only batch recycled the original endpoint. Keep historical rows
        // only while their published fingerprint version still matches. This is
        // not a near component: near edges themselves are never unioned.
        conn.execute("INSERT INTO workflow_exact_identity
            SELECT image_pk,MIN(image_pk) OVER (PARTITION BY group_no)
            FROM (SELECT m.group_no,m.image_pk FROM groups g
              CROSS JOIN members m ON m.generation=g.generation AND m.group_no=g.group_no
              CROSS JOIN fingerprints f ON f.image_pk=m.image_pk AND f.public_hash=m.public_hash AND f.signature=m.signature
              WHERE g.generation=?1 AND g.kind=0)", [generation]).map_err(|e| e.to_string())?;
        // At most one edge per pair of strict components. Expanding their
        // members against each other would create an unnecessary Cartesian set.
        conn.execute(
            "INSERT OR IGNORE INTO workflow_identity_edges
            SELECT COALESCE(a.component,e.image_pk),COALESCE(b.component,e.peer)
            FROM workflow_edges_draft e
            LEFT JOIN workflow_exact_identity a ON a.image_pk=e.image_pk
            LEFT JOIN workflow_exact_identity b ON b.image_pk=e.peer
            WHERE e.run=?1 AND COALESCE(a.component,e.image_pk)<>COALESCE(b.component,e.peer)",
            [refinement],
        )
        .map_err(|e| e.to_string())?;
    }
    // Indexed EXISTS checks avoid an all-pairs join even for giant exact groups.
    conn.execute(
        &format!("INSERT INTO workflow_nodes {}", nodes_sql()),
        params![generation, refinement],
    )
    .map_err(|e| e.to_string())?;
    let nodes: i64 = conn
        .query_row("SELECT COUNT(*) FROM workflow_nodes", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if nodes > 2_000_000 {
        return Err("整理计划超过内存预算，未替换已有计划".into());
    }
    conn.execute_batch("CREATE INDEX workflow_node_order ON workflow_nodes(priority,area DESC,bytes DESC,random,image_pk);").map_err(|e|e.to_string())?;
    // All O(nodes) work stays in TEMP tables. Holding a cache writer here used
    // to abort the concurrent verifier after its two-second busy timeout.
    let result = (|| -> Result<(), String> {
        let mut stmt=conn.prepare("SELECT image_pk,priority,choice,kind FROM workflow_nodes ORDER BY priority,area DESC,bytes DESC,random,image_pk").map_err(|e|e.to_string())?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut n = 0;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            if n % 128 == 0 {
                control()?;
                if run(conn, generation)?.2 != original.2 || choices_changed()? {
                    return Err(CHOICES_CHANGED.into());
                }
            }
            n += 1;
            let pk: i64 = row.get(0).map_err(|e| e.to_string())?;
            let priority: i64 = row.get(1).map_err(|e| e.to_string())?;
            let choice: String = row.get(2).map_err(|e| e.to_string())?;
            let kind: String = row.get(3).map_err(|e| e.to_string())?;
            // Manual protection and content variants are retained globally. A manual
            // clean instruction is considered last and can never remove the final survivor.
            let mut keeper = None;
            let mut selected_kind = kind.clone();
            let mut manual = false;
            // A content variant must survive near matching, but its byte-identical
            // copies are still duplicates. Explicit human protection stays priority 0.
            if priority >= 1 {
                let exact=conn.query_row("SELECT p.image_pk FROM members m INDEXED BY members_image CROSS JOIN groups g ON g.generation=m.generation AND g.group_no=m.group_no CROSS JOIN workflow_group_survivors p ON p.group_no=m.group_no WHERE m.generation=?1 AND m.image_pk=?2 AND g.kind=0 LIMIT 1",params![generation,pk],|r|r.get::<_,i64>(0)).optional().map_err(|e|e.to_string())?;
                if let Some(found) = exact {
                    keeper = Some(found);
                    selected_kind = "exact".into();
                }
                if priority >= 2 && keeper.is_none() && refinement > 0 {
                    // Only direct verified edges may lead to an automatic target.
                    let near = if cascade {
                        identity_keeper(conn, pk)?
                    } else {
                        conn.query_row("SELECT p.image_pk FROM workflow_edges_draft e CROSS JOIN workflow_plan_draft p ON p.run=?3 AND p.image_pk=e.peer AND p.keeper IS NULL WHERE e.run=?1 AND e.image_pk=?2 LIMIT 1",params![refinement,pk,plan],|r|r.get::<_,i64>(0))
                            .optional().map_err(|e|e.to_string())?
                    };
                    if let Some(found) = near {
                        keeper = Some(found);
                        selected_kind = "near".into();
                    }
                }
                if priority >= 2 && keeper.is_none() && choice == "manual" && refinement > 0 {
                    let found=conn.query_row("SELECT p.image_pk FROM members m INDEXED BY members_image CROSS JOIN workflow_group_survivors p ON p.group_no=m.group_no WHERE m.generation=?1 AND m.image_pk=?2 LIMIT 1",params![generation,pk],|r|r.get::<_,i64>(0)).optional().map_err(|e|e.to_string())?;
                    if let Some(found) = found {
                        keeper = Some(found);
                        selected_kind = "manual".into();
                        manual = true;
                    }
                }
            }
            conn.execute(
                "INSERT INTO workflow_plan_draft VALUES(?1,?2,?3,?4,?5)",
                params![plan, pk, keeper, selected_kind, manual as i64],
            )
            .map_err(|e| e.to_string())?;
            if keeper.is_none() {
                conn.execute("INSERT OR IGNORE INTO workflow_group_survivors SELECT group_no,image_pk FROM members WHERE generation=?1 AND image_pk=?2",params![generation,pk]).map_err(|e|e.to_string())?;
                if cascade {
                    conn.execute("INSERT INTO workflow_identity_survivors VALUES(COALESCE((SELECT component FROM workflow_exact_identity WHERE image_pk=?1),?1),?1)",
                        [pk]).map_err(|e| e.to_string())?;
                }
            }
        }
        control()?;
        if membership != cache::membership_token(state) {
            return Err("计算整理计划期间图库成员已变化，保留上次完整结果，请刷新后重试".into());
        }
        if run(conn, generation)?.2 != original.2 || choices_changed()? {
            return Err(CHOICES_CHANGED.into());
        }
        // Budget failure must still be rollbackable; never report an unpublished
        // attempt after switching the active classification/plan pointer.
        cache::check_disk_on(conn, &cache::cache_path(state))?;
        Ok(())
    })();
    result?;
    let gate = job.then(|| lock(state));
    // Never call a potentially pausing hook while holding the workflow gate or
    // a cache writer. A contended BEGIN releases the gate before retrying.
    let _publication_guard = if let Some(gate) = gate.as_ref() {
        conn.busy_timeout(std::time::Duration::from_millis(50))
            .map_err(|e| e.to_string())?;
        let mut waited = std::time::Duration::ZERO;
        let acquired = loop {
            control()?;
            let started = Instant::now();
            match gate.try_lock() {
                Ok(guard) => match conn.execute_batch("BEGIN IMMEDIATE") {
                    Ok(()) => break guard,
                    Err(rusqlite::Error::SqliteFailure(error, _))
                        if matches!(
                            error.code,
                            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                        ) =>
                    {
                        drop(guard);
                    }
                    Err(error) => return Err(error.to_string()),
                },
                Err(std::sync::TryLockError::WouldBlock) => {}
                Err(error) => return Err(error.to_string()),
            }
            waited += started.elapsed() + std::time::Duration::from_millis(20);
            if waited >= std::time::Duration::from_secs(30) {
                return Err("整理计划发布持续被其他写入占用，保留上次完整结果，请稍后重试".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        let _ = conn.busy_timeout(std::time::Duration::from_secs(2));
        Some(acquired)
    } else {
        cache::controlled_write(conn, control, || conn.execute_batch("BEGIN IMMEDIATE"))?;
        None
    };
    // There are no decoding, pause or cancellation hooks inside publication.
    // Failure before COMMIT leaves the prior plan and refinement pointer intact.
    let result = (|| -> Result<(), String> {
        if membership != cache::membership_token(state) {
            return Err("图库成员已变化，请刷新后重试".into());
        }
        if run(conn, generation)?.2 != original.2 || choices_changed()? {
            return Err(CHOICES_CHANGED.into());
        }
        conn.execute(
            "INSERT INTO workflow_plan SELECT * FROM workflow_plan_draft",
            [],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR IGNORE INTO workflow_edges SELECT * FROM workflow_edges_draft",
            [],
        )
        .map_err(|e| e.to_string())?;
        cache::check_disk_on(conn, &cache::cache_path(state))?;
        conn.execute("UPDATE workflow_runs SET plan=?2,refinement=?3,revision=revision+1,membership=?4 WHERE generation=?1",params![generation,plan,refinement,membership]).map_err(|e|e.to_string())?;
        if publish_generation {
            let is_cascade: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM meta WHERE key=?1 AND value=?2)",
                    params![
                        format!("policy:{generation}"),
                        super::super::cascade_pixels::POLICY
                    ],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if is_cascade {
                conn.execute(
                    "UPDATE cascade_runs SET phase=4 WHERE generation=?1",
                    [generation],
                )
                .map_err(|e| e.to_string())?;
            }
            // Carry current explicit keeper preferences across new group numbering.
            conn.execute("INSERT OR IGNORE INTO workflow_choices SELECT f.public_hash,f.signature,'anchor' FROM workflow_keepers k JOIN fingerprints f ON f.image_pk=k.image_pk WHERE k.generation=CAST((SELECT value FROM meta WHERE key='published') AS INTEGER)",[]).map_err(|e|e.to_string())?;
            conn.execute(
                "INSERT OR REPLACE INTO meta VALUES('published',?1)",
                [generation.to_string()],
            )
            .map_err(|e| e.to_string())?;
            conn.execute("INSERT OR REPLACE INTO meta VALUES('dirty','0')", [])
                .map_err(|e| e.to_string())?;
            conn.execute(
                "UPDATE generations SET state='ready' WHERE generation=?1",
                [generation],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(e);
        }
    }
    // Publication has succeeded. Disposal is best effort and may be retried by
    // the next explicit run; it cannot turn this complete result into a failure.
    if let Err(error) = cache::controlled_write(conn, &mut || Ok(()), || {
        conn.execute("DELETE FROM workflow_plan WHERE run<>?1", [plan])
    }) {
        eprintln!("[duplicate-plan-cleanup] {error}");
    }
    if let Err(error) = cache::controlled_write(conn, &mut || Ok(()), || {
        conn.execute("DELETE FROM workflow_edges WHERE run<>?1", [refinement])
    }) {
        eprintln!("[duplicate-edge-cleanup] {error}");
    }
    if let Err(error)=conn.execute_batch("DROP TABLE workflow_nodes; DROP TABLE workflow_group_survivors; DROP TABLE workflow_plan_draft; DROP TABLE workflow_edges_draft; DROP TABLE workflow_exact_identity; DROP TABLE workflow_identity_edges; DROP TABLE workflow_identity_survivors; DROP TABLE workflow_identity_targets; PRAGMA wal_checkpoint(PASSIVE)") {eprintln!("[duplicate-plan-checkpoint] {error}");}
    Ok(())
}

use super::*;
use crate::duplicates::tests::Fixture;
use std::cell::Cell;

fn request(f: &Fixture, exact: bool) -> DuplicateManagementRequest {
    DuplicateManagementRequest {
        generation: cache::status(&f.state).unwrap().generation.unwrap(),
        group_id: if exact { "all-exact" } else { "all-near" }.into(),
        mode: if exact { "autoExactAll" } else { "autoNearAll" }.into(),
        keep_ids: vec![],
        protected_ids: vec![],
        image_ids: vec![],
        action: "trash".into(),
        source_operation_id: None,
    }
}

fn execute(f: &Fixture, id: &str) -> Result<(), String> {
    run_operation(id, &f.state, &mut || Ok(()), &mut |_| {})
}

fn live(f: &Fixture) -> Vec<i64> {
    let db = cache::library(&f.state).unwrap();
    let mut stmt = db
        .prepare("SELECT image_pk FROM images WHERE trashed=0 ORDER BY image_pk")
        .unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn groups(f: &Fixture, kind: i64, sets: &[Vec<i64>]) {
    let generation = cache::status(&f.state).unwrap().generation.unwrap();
    let conn = cache::open_existing(&f.state, true).unwrap();
    conn.execute_batch("BEGIN;DELETE FROM members;DELETE FROM groups;")
        .unwrap();
    for (n, pks) in sets.iter().enumerate() {
        let group = n as i64 + 1;
        conn.execute(
            "INSERT INTO groups VALUES(?1,?2,?3,?4,?5)",
            params![generation, group, kind, pks[0], pks.len() as i64],
        )
        .unwrap();
        for (m, pk) in pks.iter().enumerate() {
            conn.execute("INSERT INTO members SELECT ?1,?2,?3,image_pk,public_hash,signature,NULL,NULL,NULL FROM fingerprints WHERE image_pk=?4",params![generation,group,m as i64,pk]).unwrap();
        }
    }
    conn.execute_batch("COMMIT").unwrap();
    cache::membership_invalidated(&f.state);
}

#[test]
fn bulk_exact_all_exceeds_group_page_and_1000_groups_then_global_undo() {
    let count = 1003;
    let f = Fixture::new(count * 2);
    f.run().unwrap();
    groups(
        &f,
        0,
        &(0..count)
            .map(|n| vec![n as i64 * 2 + 1, n as i64 * 2 + 2])
            .collect::<Vec<_>>(),
    );
    let preview = prepare(request(&f, true), &f.state).unwrap();
    assert_eq!(
        preview.target_count, 0,
        "prepare must defer full planning to its background job"
    );
    let log = journal(&f.state, false).unwrap();
    assert_eq!(
        log.query_row(
            "SELECT COUNT(*) FROM items WHERE operation=?1",
            [op_id(&preview.operation_id).unwrap()],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    drop(log);
    execute(&f, &preview.operation_id).unwrap();
    let survivors = live(&f);
    assert_eq!(survivors.len(), count);
    for pair in 0..count {
        assert_eq!(
            survivors
                .iter()
                .filter(|&&pk| pk == pair as i64 * 2 + 1 || pk == pair as i64 * 2 + 2)
                .count(),
            1
        );
    }
    let status = operation_status(&preview.operation_id, &f.state).unwrap();
    assert_eq!(status.succeeded, count as i64);
    assert_eq!(status.total, count as i64);
    assert!(status.undoable);
    let active = cache::status(&f.state).unwrap();
    assert_eq!(
        (
            active.exact_groups,
            active.near_groups,
            active.unique_images
        ),
        (0, 0, 0)
    );
    assert!(!active.needs_update);
    // A fresh all-results run needs no UI paging and does not remove last survivors.
    let repeat = prepare(request(&f, true), &f.state).unwrap();
    execute(&f, &repeat.operation_id).unwrap();
    assert_eq!(live(&f), survivors);
    let undo =
        crate::duplicates::management::prepare_undo(&preview.operation_id, &f.state).unwrap();
    execute(&f, &undo).unwrap();
    assert_eq!(live(&f).len(), count * 2);
    assert_eq!(
        operation_status(&undo, &f.state).unwrap().succeeded,
        count as i64
    );
    assert!(f.paths.iter().all(|p| p.exists()));
    let restored = cache::status(&f.state).unwrap();
    assert_eq!(
        (restored.exact_groups, restored.unique_images),
        (count as i64, count as i64 * 2)
    );
    assert!(!restored.needs_update);
}

#[test]
fn bulk_cancelled_after_a_committed_chunk_refreshes_active_counts_without_a_scan() {
    let f = Fixture::new(130);
    f.run().unwrap();
    let generation = cache::status(&f.state).unwrap().generation;
    let preview = prepare(request(&f, true), &f.state).unwrap();
    let cancelled = Cell::new(false);
    let result = run_operation(
        &preview.operation_id,
        &f.state,
        &mut || {
            if cancelled.get() {
                Err("任务已取消".into())
            } else {
                Ok(())
            }
        },
        &mut |status| {
            if status.succeeded > 0 {
                cancelled.set(true);
            }
        },
    );
    assert!(result.unwrap_err().contains("取消"));
    let active = cache::status(&f.state).unwrap();
    assert_eq!(active.generation, generation);
    assert_eq!((active.exact_groups, active.unique_images), (1, 2));
    assert!(!active.needs_update);
    execute(&f, &preview.operation_id).unwrap();
    let completed = cache::status(&f.state).unwrap();
    assert_eq!((completed.exact_groups, completed.unique_images), (0, 0));
}

#[test]
fn bulk_near_keeps_highest_area_and_direct_survivors_across_overlapping_stars() {
    let f = Fixture::new(5);
    f.run().unwrap();
    let c = cache::open_existing(&f.state, true).unwrap();
    for (pk, hash, w, h) in [
        (1_i64, 0_u64, 512, 384),
        (2, 15, 256, 192),
        (3, 255, 128, 96),
        (4, 65535, 64, 48),
        (5, 65535, 64, 48),
    ] {
        c.execute("UPDATE fingerprints SET phash=?2,dhash=?3,width=?4,height=?5,low_info=0 WHERE image_pk=?1",params![pk,hash.to_le_bytes().as_slice(),0_u64.to_le_bytes().as_slice(),w,h]).unwrap();
    }
    drop(c);
    // A~B and B~C, but A does not match C. B appears in both stars.
    groups(&f, 1, &[vec![2, 1, 3], vec![1, 2], vec![4, 5]]);
    let preview = prepare(request(&f, false), &f.state).unwrap();
    execute(&f, &preview.operation_id).unwrap();
    assert_eq!(live(&f), vec![1, 3, 4]);
    assert_eq!(
        operation_status(&preview.operation_id, &f.state)
            .unwrap()
            .succeeded,
        2
    );
    let log = journal(&f.state, false).unwrap();
    let payload: String = log
        .query_row(
            "SELECT payload FROM items WHERE operation=?1 AND image_pk=2",
            [op_id(&preview.operation_id).unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Item>(&payload).unwrap().keeper_pks,
        vec![1]
    );
}

#[test]
fn bulk_cancel_resume_does_not_rerandomize_and_undo_after_cache_removal() {
    let f = Fixture::new(17);
    f.run().unwrap();
    let preview = prepare(request(&f, true), &f.state).unwrap();
    let calls = Cell::new(0);
    let mut seed_keeps = vec![];
    let result = run_operation(
        &preview.operation_id,
        &f.state,
        &mut || {
            calls.set(calls.get() + 1);
            if calls.get() > 65 {
                Err("任务已取消".into())
            } else {
                Ok(())
            }
        },
        &mut |_| {},
    );
    assert!(result.is_err());
    let log = journal(&f.state, false).unwrap();
    {
        let mut stmt = log
            .prepare("SELECT payload FROM bulk_keepers WHERE operation=?1 ORDER BY image_pk")
            .unwrap();
        seed_keeps.extend(
            stmt.query_map([op_id(&preview.operation_id).unwrap()], |r| {
                r.get::<_, String>(0)
            })
            .unwrap()
            .map(Result::unwrap),
        );
    }
    let seed: String = log
        .query_row(
            "SELECT seed FROM bulk_runs WHERE operation=?1",
            [op_id(&preview.operation_id).unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    drop(log);
    execute(&f, &preview.operation_id).unwrap();
    let log = journal(&f.state, false).unwrap();
    assert_eq!(
        seed,
        log.query_row(
            "SELECT seed FROM bulk_runs WHERE operation=?1",
            [op_id(&preview.operation_id).unwrap()],
            |r| r.get::<_, String>(0)
        )
        .unwrap()
    );
    for payload in seed_keeps {
        let pk = serde_json::from_str::<Item>(&payload).unwrap().pk;
        assert_eq!(
            payload,
            log.query_row(
                "SELECT payload FROM bulk_keepers WHERE operation=?1 AND image_pk=?2",
                params![op_id(&preview.operation_id).unwrap(), pk],
                |r| r.get::<_, String>(0)
            )
            .unwrap()
        );
    }
    drop(log);
    assert_eq!(live(&f).len(), 1);
    cache::clear_cache(&f.state).unwrap();
    let undo =
        crate::duplicates::management::prepare_undo(&preview.operation_id, &f.state).unwrap();
    execute(&f, &undo).unwrap();
    assert_eq!(live(&f).len(), 17);
}

#[test]
fn bulk_changed_sources_skip_only_affected_items_and_keep_audit_pages() {
    let f = Fixture::new(6);
    f.run().unwrap();
    groups(&f, 0, &[vec![1, 2, 3], vec![4, 5, 6]]);
    let preview = prepare(request(&f, true), &f.state).unwrap();
    fs::write(&f.paths[0], b"changed synthetic source").unwrap();
    execute(&f, &preview.operation_id).unwrap();
    let status = operation_status(&preview.operation_id, &f.state).unwrap();
    assert_eq!(status.state, "completed");
    assert_eq!(status.skipped, 1);
    assert_eq!(status.succeeded, 3);
    assert!(live(&f).contains(&1));
    assert!(
        cache::status(&f.state).unwrap().needs_update,
        "a source change detected during planning still requires a manual scan"
    );
    let first = operation_results(
        DuplicateOperationQuery {
            operation_id: preview.operation_id.clone(),
            cursor: None,
            limit: 1,
            state: None,
        },
        &f.state,
    )
    .unwrap();
    assert_eq!(first.items.len(), 1);
    assert!(first.next_cursor.is_some());
    let failed = operation_results(
        DuplicateOperationQuery {
            operation_id: preview.operation_id,
            cursor: None,
            limit: 10,
            state: Some("failed".into()),
        },
        &f.state,
    )
    .unwrap();
    assert_eq!(failed.items.len(), 1);
    assert_eq!(failed.items[0].state, "skipped");
    assert!(f.paths.iter().all(|p| p.exists()));
}

#[test]
fn bulk_soft_intent_recovers_after_main_write_before_journal_outcome() {
    let f = Fixture::new(3);
    f.run().unwrap();
    let preview = prepare(request(&f, true), &f.state).unwrap();
    let log = journal(&f.state, false).unwrap();
    let id = op_id(&preview.operation_id).unwrap();
    let (plan, _, _) = load_plan(&log, id).unwrap();
    snapshot(&log, id, &plan, &f.state, &mut || Ok(()), &mut |_| {}).unwrap();
    plan_candidates(&log, id, &plan, &f.state, &mut || Ok(()), &mut |_| {}).unwrap();
    let (ordinal, pk, payload): (i64, i64, String) = log
        .query_row(
            "SELECT ordinal,image_pk,payload FROM items WHERE operation=?1 LIMIT 1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    let item: Item = serde_json::from_str(&payload).unwrap();
    log.execute(
        "INSERT INTO owners VALUES(?1,?2,?3)",
        params![pk, item.public_hash, id],
    )
    .unwrap();
    log.execute(
        "UPDATE items SET code='softIntent' WHERE operation=?1 AND ordinal=?2",
        params![id, ordinal],
    )
    .unwrap();
    Connection::open(&f.state.database_path)
        .unwrap()
        .execute("UPDATE images SET trashed=1 WHERE image_pk=?1", [pk])
        .unwrap();
    log.execute(
        "UPDATE operations SET state='interrupted' WHERE id=?1",
        [id],
    )
    .unwrap();
    drop(log);
    execute(&f, &preview.operation_id).unwrap();
    assert_eq!(
        operation_status(&preview.operation_id, &f.state)
            .unwrap()
            .succeeded,
        2
    );
    let undo =
        crate::duplicates::management::prepare_undo(&preview.operation_id, &f.state).unwrap();
    execute(&f, &undo).unwrap();
    assert_eq!(live(&f).len(), 3);
}

#[test]
#[ignore = "larger synthetic file fixture; run explicitly for the bulk journal stress check"]
fn bulk_more_than_20000_targets_exceeds_old_journal_budget_and_undo() {
    let f = Fixture::new(20_006);
    f.run().unwrap();
    let preview = prepare(request(&f, true), &f.state).unwrap();
    execute(&f, &preview.operation_id).unwrap();
    assert_eq!(live(&f).len(), 1);
    assert_eq!(
        operation_status(&preview.operation_id, &f.state)
            .unwrap()
            .succeeded,
        20_005
    );
    assert!(size(&journal_path(&f.state)) > MAX_BYTES);
    let undo =
        crate::duplicates::management::prepare_undo(&preview.operation_id, &f.state).unwrap();
    execute(&f, &undo).unwrap();
    assert_eq!(live(&f).len(), 20_006);
}

#[test]
fn bulk_restore_intent_resumes_before_and_after_main_commit() {
    for main_committed in [false, true] {
        let f = Fixture::new(4);
        f.run().unwrap();
        let preview = prepare(request(&f, true), &f.state).unwrap();
        execute(&f, &preview.operation_id).unwrap();
        let undo =
            crate::duplicates::management::prepare_undo(&preview.operation_id, &f.state).unwrap();
        let log = journal(&f.state, false).unwrap();
        let id = op_id(&undo).unwrap();
        let (plan, _, _) = load_plan(&log, id).unwrap();
        undo_snapshot(&log, id, &plan, &f.state, &mut || Ok(()), &mut |_| {}).unwrap();
        let (n, pk): (i64, i64) = log
            .query_row(
                "SELECT ordinal,image_pk FROM items WHERE operation=?1 LIMIT 1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        log.execute(
            "UPDATE items SET code='restoreIntent' WHERE operation=?1 AND ordinal=?2",
            params![id, n],
        )
        .unwrap();
        if main_committed {
            Connection::open(&f.state.database_path)
                .unwrap()
                .execute("UPDATE images SET trashed=0 WHERE image_pk=?1", [pk])
                .unwrap();
        }
        // Source ownership survives either crash point until restore outcome commits.
        assert_eq!(
            log.query_row(
                "SELECT operation FROM owners WHERE image_pk=?1",
                [pk],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            op_id(&preview.operation_id).unwrap()
        );
        log.execute(
            "UPDATE operations SET state='interrupted' WHERE id=?1",
            [id],
        )
        .unwrap();
        drop(log);
        execute(&f, &undo).unwrap();
        assert_eq!(live(&f).len(), 4);
        assert_eq!(operation_status(&undo, &f.state).unwrap().succeeded, 3);
        assert!(
            !operation_status(&preview.operation_id, &f.state)
                .unwrap()
                .undoable
        );
    }
}

#[test]
fn bulk_undo_does_not_revive_a_later_individual_deletion() {
    let f = Fixture::new(4);
    f.run().unwrap();
    let preview = prepare(request(&f, true), &f.state).unwrap();
    execute(&f, &preview.operation_id).unwrap();
    let log = journal(&f.state, false).unwrap();
    let payload: String = log
        .query_row(
            "SELECT payload FROM items WHERE operation=?1 AND state='succeeded' LIMIT 1",
            [op_id(&preview.operation_id).unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    let target: Item = serde_json::from_str(&payload).unwrap();
    drop(log);
    for action in ["restore", "trash"] {
        let result = crate::library::mutate_images(
            crate::library::ImageMutation {
                action: action.into(),
                image_ids: vec![target.image.id.clone()],
                favorite: None,
                folder_id: None,
                custom_tags: vec![],
                supplement_tags: vec![],
            },
            &f.state,
        )
        .unwrap();
        assert_eq!(result.successful_ids.len(), 1);
    }
    let undo =
        crate::duplicates::management::prepare_undo(&preview.operation_id, &f.state).unwrap();
    execute(&f, &undo).unwrap();
    assert_eq!(live(&f).len(), 3);
    assert!(!live(&f).contains(&target.pk));
    assert_eq!(operation_status(&undo, &f.state).unwrap().succeeded, 2);
}

#[test]
fn bulk_undo_legacy_history_after_journal_grows_beyond_32_mib() {
    let f = Fixture::new(3);
    f.run().unwrap();
    let mut req = request(&f, true);
    let cache = cache::open_existing(&f.state, false).unwrap();
    req.group_id = cache
        .query_row(
            "SELECT group_no FROM groups WHERE generation=?1 AND kind=0 LIMIT 1",
            [&req.generation],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
        .to_string();
    req.mode = "exactOthers".into();
    req.keep_ids = vec![f.paths[2].to_string_lossy().into_owned()];
    drop(cache);
    let legacy = crate::duplicates::management::prepare(req, &f.state).unwrap();
    execute(&f, &legacy.operation_id).unwrap();
    assert_eq!(live(&f), vec![3]);
    let log = journal(&f.state, false).unwrap();
    assert!(!exists(&log, op_id(&legacy.operation_id).unwrap()).unwrap());
    // Occupied audit pages model the persistent disk usage of an unrelated bulk
    // run without generating thousands of image files or changing old history.
    log.execute_batch("CREATE TABLE test_bulk_audit_growth(payload BLOB NOT NULL); INSERT INTO test_bulk_audit_growth VALUES(zeroblob(34*1024*1024)); PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
    assert!(size(&journal_path(&f.state)) > MAX_BYTES);
    drop(log);
    let undo = crate::duplicates::management::prepare_undo(&legacy.operation_id, &f.state).unwrap();
    let log = journal(&f.state, false).unwrap();
    assert!(exists(&log, op_id(&undo).unwrap()).unwrap());
    drop(log);
    execute(&f, &undo).unwrap();
    assert_eq!(live(&f), vec![1, 2, 3]);
    assert_eq!(operation_status(&undo, &f.state).unwrap().succeeded, 2);
    assert!(
        !operation_status(&legacy.operation_id, &f.state)
            .unwrap()
            .undoable
    );
    assert!(f.paths.iter().all(|p| p.exists()));
}

#[test]
fn bulk_keeper_lookup_seeks_only_the_candidate_groups() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(LOG_SCHEMA).unwrap();
    schema(&conn).unwrap();
    // Thousands of unrelated keepers must not become the outer query loop.
    for pk in 1..=3000_i64 {
        conn.execute("INSERT INTO bulk_members VALUES(1,?1,?1)", [pk])
            .unwrap();
        conn.execute("INSERT INTO bulk_group_keepers VALUES(1,?1,?1)", [pk])
            .unwrap();
        conn.execute("INSERT INTO bulk_keepers VALUES(1,?1,'{}')", [pk])
            .unwrap();
    }
    // The same direct keeper appears in two overlapping candidate groups.
    conn.execute("INSERT INTO bulk_members VALUES(1,3001,3)", [])
        .unwrap();
    conn.execute("INSERT INTO bulk_group_keepers VALUES(1,3001,3)", [])
        .unwrap();
    let mut plan = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {KEEPER_CANDIDATES_SQL}"))
        .unwrap();
    let details = plan
        .query_map(params![1, 3], |r| r.get::<_, String>(3))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(
        details[0].contains("SEARCH m") && details[0].contains("bulk_members_image"),
        "{details:?}"
    );
    assert!(details[1].contains("group_no=?"), "{details:?}");
    let mut query = conn.prepare(KEEPER_CANDIDATES_SQL).unwrap();
    let keepers = query
        .query_map(params![1, 3], |r| r.get::<_, i64>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(keepers, vec![3]);
}

#[test]
fn bulk_snapshot_tuple_cursor_pages_inside_large_groups() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE groups(generation INTEGER,group_no INTEGER,kind INTEGER,PRIMARY KEY(generation,group_no)) WITHOUT ROWID; CREATE INDEX groups_kind ON groups(generation,kind,group_no); CREATE TABLE members(generation INTEGER,group_no INTEGER,member_no INTEGER,image_pk INTEGER,public_hash BLOB,signature BLOB,PRIMARY KEY(generation,group_no,member_no)) WITHOUT ROWID;").unwrap();
    conn.execute("INSERT INTO groups VALUES(1,1,0),(1,2,1),(1,3,0)", [])
        .unwrap();
    for member in 0..1025_i64 {
        conn.execute(
            "INSERT INTO members VALUES(1,1,?1,?1,x'00',x'00')",
            [member],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO members VALUES(1,2,0,2000,x'00',x'00'),(1,3,0,3000,x'00',x'00')",
        [],
    )
    .unwrap();
    let mut explain = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {SNAPSHOT_PAGE_SQL}"))
        .unwrap();
    let detail = explain
        .query_row(params![1, 0, -1, -1, CHUNK], |r| r.get::<_, String>(3))
        .unwrap();
    assert!(detail.contains("group_no,member_no"), "{detail}");
    let mut cursor = (-1, -1);
    let mut all = Vec::new();
    loop {
        let mut stmt = conn.prepare(SNAPSHOT_PAGE_SQL).unwrap();
        let rows = stmt
            .query_map(params![1, 0, cursor.0, cursor.1, CHUNK], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        if rows.is_empty() {
            break;
        }
        cursor = (rows.last().unwrap().0, rows.last().unwrap().1);
        all.extend(rows.into_iter().map(|r| r.2));
    }
    assert_eq!(all.len(), 1026);
    assert_eq!(all[1024], 1024);
    assert_eq!(all[1025], 3000);
}

#[test]
fn bulk_partial_planning_resumes_legacy_state_and_reports_stage_progress() {
    let f = Fixture::new(300);
    f.run().unwrap();
    let preview = prepare(request(&f, true), &f.state).unwrap();
    let id = op_id(&preview.operation_id).unwrap();
    let status = operation_status(&preview.operation_id, &f.state).unwrap();
    assert_eq!(status.stage, "snapshot");
    assert_eq!(status.stage_total, 300);
    let mut legacy_json = serde_json::to_value(&status).unwrap();
    for field in ["stage", "stageProcessed", "stageTotal"] {
        legacy_json.as_object_mut().unwrap().remove(field);
    }
    let legacy_status: DuplicateOperationStatus = serde_json::from_value(legacy_json).unwrap();
    assert_eq!(
        (
            legacy_status.stage,
            legacy_status.stage_processed,
            legacy_status.stage_total
        ),
        (String::new(), 0, 0)
    );
    let cancelled = Cell::new(false);
    assert!(run_operation(
        &preview.operation_id,
        &f.state,
        &mut || {
            if cancelled.get() {
                Err("任务已取消".into())
            } else {
                Ok(())
            }
        },
        &mut |status| {
            if status.stage == "planning" && status.stage_processed >= CHUNK {
                assert_eq!(status.total, 0);
                assert_eq!(status.stage_total, 300);
                cancelled.set(true);
            }
        }
    )
    .is_err());
    let log = journal(&f.state, false).unwrap();
    let (stage, planned, seed): (String, i64, String) = log
        .query_row(
            "SELECT stage,planned,seed FROM bulk_runs WHERE operation=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!((stage.as_str(), planned), ("planning", 128));
    let (keeper_pk, payload): (i64, String) = log
        .query_row(
            "SELECT image_pk,payload FROM bulk_keepers WHERE operation=?1 LIMIT 1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    // Old 1.5.0 payloads included a canonical path. Keep that representation and
    // all existing seed/cursor/keeper rows while the optimized code continues.
    let mut old_keeper: Item = serde_json::from_str(&payload).unwrap();
    old_keeper.canonical = canonical(Path::new(&old_keeper.image.path)).unwrap();
    let old_payload = serde_json::to_string(&old_keeper).unwrap();
    log.execute(
        "UPDATE bulk_keepers SET payload=?3 WHERE operation=?1 AND image_pk=?2",
        params![id, keeper_pk, old_payload],
    )
    .unwrap();
    log.execute(
        "UPDATE operations SET state='interrupted' WHERE id=?1",
        [id],
    )
    .unwrap();
    drop(log);
    execute(&f, &preview.operation_id).unwrap();
    assert_eq!(live(&f), vec![keeper_pk]);
    let log = journal(&f.state, false).unwrap();
    assert_eq!(
        log.query_row("SELECT seed FROM bulk_runs WHERE operation=?1", [id], |r| r
            .get::<_, String>(0))
            .unwrap(),
        seed
    );
    assert_eq!(
        log.query_row(
            "SELECT payload FROM bulk_keepers WHERE operation=?1 AND image_pk=?2",
            params![id, keeper_pk],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        old_payload
    );
    let status = operation_status(&preview.operation_id, &f.state).unwrap();
    assert_eq!(
        (
            status.stage,
            status.stage_processed,
            status.stage_total,
            status.succeeded
        ),
        ("applying".into(), 299, 299, 299)
    );
}

#[test]
fn bulk_full_hash_still_rejects_same_size_same_mtime_changed_target() {
    let f = Fixture::new(3);
    f.run().unwrap();
    let preview = prepare(request(&f, true), &f.state).unwrap();
    let log = journal(&f.state, false).unwrap();
    let id = op_id(&preview.operation_id).unwrap();
    let (plan, _, _) = load_plan(&log, id).unwrap();
    snapshot(&log, id, &plan, &f.state, &mut || Ok(()), &mut |_| {}).unwrap();
    plan_candidates(&log, id, &plan, &f.state, &mut || Ok(()), &mut |_| {}).unwrap();
    let payload: String = log
        .query_row(
            "SELECT payload FROM items WHERE operation=?1 LIMIT 1",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    let target: Item = serde_json::from_str(&payload).unwrap();
    let path = Path::new(&target.image.path);
    let modified = fs::metadata(path).unwrap().modified().unwrap();
    let mut bytes = fs::read(path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    fs::write(path, bytes).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    assert!(
        hydrate(
            target.clone(),
            &cache::library(&f.state).unwrap(),
            false,
            &f.state
        )
        .is_ok(),
        "metadata alone cannot detect this edit"
    );
    drop(log);
    execute(&f, &preview.operation_id).unwrap();
    let status = operation_status(&preview.operation_id, &f.state).unwrap();
    assert_eq!((status.succeeded, status.skipped), (1, 1));
    assert!(live(&f).contains(&target.pk));
}

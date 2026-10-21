use super::*;
use crate::duplicates::tests::Fixture;
use std::{cell::Cell, sync::mpsc};

#[test]
fn published_header_does_not_wait_for_the_live_membership_mutex() {
    let f = Fixture::new(3);
    f.run().unwrap();
    let before = fs::read(&f.state.database_path).unwrap();
    let rt = runtime(&f.state);
    let guard = rt.live.lock().unwrap();
    let state = f.state.clone();
    let (tx, rx) = mpsc::channel();
    let reader = thread::spawn(move || tx.send(published_status(&state)).unwrap());
    let result = rx.recv_timeout(Duration::from_secs(1));
    drop(guard);
    reader.join().unwrap();
    let header = result
        .expect("metadata must not wait for the full live view")
        .unwrap();
    assert_eq!(header.coverage.processed, 3);
    assert_eq!(header.exact_groups, 1);
    assert_eq!(before, fs::read(&f.state.database_path).unwrap());
}

#[test]
fn evidence_join_streams_only_active_sources_without_touching_files_or_signatures() {
    let f = Fixture::new(6);
    f.run().unwrap();
    let main = Connection::open(&f.state.database_path).unwrap();
    main.execute_batch("UPDATE images SET trashed=1 WHERE image_pk=1; UPDATE images SET missing=1 WHERE image_pk=2; UPDATE images SET source='reference' WHERE image_pk=3;").unwrap();
    // Removing the synthetic source files proves this stage only reuses numbers.
    for path in &f.paths {
        fs::remove_file(path).unwrap();
    }
    let before = fs::read(&f.state.database_path).unwrap();
    let conn = open_existing(&f.state, false).unwrap();
    let signature = get_fingerprint(&conn, 4).unwrap().unwrap().signature;
    let mut progress = Vec::new();
    let records = evidence_records_with_progress(&f.state, &mut || Ok(()), &mut |done, total| {
        progress.push((done, total))
    })
    .unwrap();
    assert_eq!(
        records.iter().map(|r| r.image_pk).collect::<Vec<_>>(),
        vec![4, 5, 6]
    );
    assert_eq!(progress.first(), Some(&(0, 3)));
    assert_eq!(progress.last(), Some(&(3, 3)));
    assert_eq!(
        signature,
        get_fingerprint(&conn, 4).unwrap().unwrap().signature
    );
    assert_eq!(before, fs::read(&f.state.database_path).unwrap());
    assert_eq!(
        main.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        3
    );
}

#[test]
fn evidence_progress_and_pause_callbacks_can_persist_to_the_rollback_journal_library() {
    let f = Fixture::new(1005);
    f.run().unwrap();
    let db = Connection::open(&f.state.database_path).unwrap();
    assert_eq!(
        db.query_row("PRAGMA journal_mode=DELETE", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "delete"
    );
    db.busy_timeout(Duration::from_millis(40)).unwrap();
    let mut errors = Vec::new();
    let mut control_writes = 0;
    let mut progress_writes = 0;
    let records = evidence_records_with_progress(
        &f.state,
        &mut || {
            db.execute("INSERT OR REPLACE INTO task_runs(id,kind,state,payload,updated_at) VALUES('pause-check','duplicates','running','{}',1)",[]).map_err(|e|format!("control callback cannot save task state: {e}"))?;
            control_writes += 1;
            Ok(())
        },
        &mut |_, _| {
            progress_writes += 1;
            if let Err(e) = db.execute("INSERT OR REPLACE INTO task_runs(id,kind,state,payload,updated_at) VALUES('progress-check','duplicates','running','{}',1)",[]) { errors.push(e.to_string()); }
        },
    );
    assert!(
        errors.is_empty(),
        "progress callback was blocked by its own index read: {errors:?}"
    );
    assert!(records.is_ok(), "{records:?}");
    assert_eq!(records.unwrap().len(), 1005);
    assert!(control_writes >= 3 && progress_writes >= 3);
}

#[test]
fn evidence_paging_rejects_membership_changes_between_released_read_batches() {
    let f = Fixture::new(3);
    f.run().unwrap();
    let db = Connection::open(&f.state.database_path).unwrap();
    let result = evidence_records_with_progress(&f.state, &mut || Ok(()), &mut |done, _| {
        if done == 0 {
            db.execute("UPDATE images SET trashed=1 WHERE image_pk=3", [])
                .unwrap();
            membership_invalidated(&f.state);
        }
    });
    assert!(result.unwrap_err().contains("图库成员已变化"));
}

#[test]
fn evidence_join_preserves_identity_validation_and_cancellation() {
    let f = Fixture::new(3);
    f.run().unwrap();
    let calls = Cell::new(0);
    let stopped = evidence_records_with_progress(
        &f.state,
        &mut || {
            calls.set(calls.get() + 1);
            if calls.get() >= 2 {
                Err("synthetic stop".into())
            } else {
                Ok(())
            }
        },
        &mut |_, _| {},
    );
    assert!(stopped.unwrap_err().contains("synthetic stop"));
    Connection::open(&f.state.database_path)
        .unwrap()
        .execute(
            "UPDATE images SET id='changed-identity' WHERE image_pk=1",
            [],
        )
        .unwrap();
    assert!(evidence_records(&f.state, &mut || Ok(()))
        .unwrap_err()
        .contains("图片身份已变化"));
    assert!(published_status(&f.state).unwrap().needs_update);
}

#[test]
fn unpublished_cleanup_keeps_published_rows_and_does_not_wait_for_a_wal_reader() {
    let f = Fixture::new(2);
    f.run().unwrap();
    let header = published_status(&f.state).unwrap();
    let generation = header.generation.as_ref().unwrap().parse::<i64>().unwrap();
    let mut conn = open_existing(&f.state, true).unwrap();
    conn.execute("INSERT INTO generations SELECT ?2,sensitivity,checked_at,'building',coverage,errors,exact_groups,near_groups,unique_images FROM generations WHERE generation=?1",params![generation,generation+1]).unwrap();
    conn.execute("INSERT INTO groups SELECT ?2,group_no,kind,reference_pk,total_members FROM groups WHERE generation=?1",params![generation,generation+1]).unwrap();
    conn.execute("INSERT INTO members SELECT ?2,group_no,member_no,image_pk,public_hash,signature,phash_distance,dhash_distance,color_distance FROM members WHERE generation=?1",params![generation,generation+1]).unwrap();
    let reader = open_existing(&f.state, false).unwrap();
    reader.execute_batch("BEGIN DEFERRED").unwrap();
    assert!(
        reader
            .query_row("SELECT COUNT(*) FROM members", [], |r| r.get::<_, i64>(0))
            .unwrap()
            > 2
    );
    let started = Instant::now();
    delete_unpublished(&mut conn, &mut || Ok(())).unwrap();
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "passive checkpoint must not wait for this reader"
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM members WHERE generation=?1",
            [generation],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM generations WHERE generation=?1",
            [generation + 1],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

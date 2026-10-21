use super::*;
use crate::duplicates::tests::Fixture;
use std::{thread, time::Duration};

fn ready(state: &AppState) -> DuplicateWorkflowStatus {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let value = status_snapshot(state).unwrap();
        if value.stage != "loading" {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "metadata synchronization did not complete"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn snapshot_cold_load_is_nonblocking_and_exposes_only_complete_counts() {
    let f = Fixture::new(3);
    f.run().unwrap();
    let gate = lock(&f.state);
    let guard = gate.lock().unwrap();
    let first = status_snapshot(&f.state).unwrap();
    assert_eq!(first.stage, "loading");
    assert!(first.categories.is_empty());
    assert_eq!(first.cleanup_count, 0);
    assert_eq!(first.scan.coverage.processed, 3);
    for _ in 0..3 {
        assert_eq!(status_snapshot(&f.state).unwrap().stage, "loading");
        assert_eq!(retry_status_snapshot(&f.state).unwrap().stage, "loading");
    }
    drop(guard);
    let complete = ready(&f.state);
    assert_eq!(complete.stage, "scanned");
    assert_eq!(complete.cleanup_count, 2);
    let guard = gate.lock().unwrap();
    // Both APIs can now reuse a complete memo while the workflow gate is busy.
    assert_eq!(
        status_snapshot(&f.state).unwrap().revision,
        complete.revision
    );
    assert_eq!(status(&f.state).unwrap().revision, complete.revision);
    assert_eq!(
        retry_status_snapshot(&f.state).unwrap().revision,
        complete.revision
    );
    drop(guard);
}

#[test]
fn snapshot_invalidates_for_membership_and_manual_choice_revisions() {
    let f = Fixture::new(3);
    f.run().unwrap();
    let initial = ready(&f.state);
    assert_eq!(initial.cleanup_count, 2);
    let gen = initial.generation.as_ref().unwrap().parse::<i64>().unwrap();
    let conn = open(&f.state).unwrap();
    conn.execute("INSERT OR REPLACE INTO workflow_choices SELECT public_hash,signature,'keep' FROM fingerprints WHERE image_pk IN (1,2)",[]).unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO meta VALUES('choice_revision','new-choice')",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE workflow_runs SET plan=0,revision=revision+1 WHERE generation=?1",
        [gen],
    )
    .unwrap();
    let changed = ready(&f.state);
    assert_eq!(changed.cleanup_count, 1);
    assert_eq!(changed.protected_count, 2);
    assert_ne!(changed.revision, initial.revision);
    Connection::open(&f.state.database_path)
        .unwrap()
        .execute("UPDATE images SET trashed=1 WHERE image_pk=3", [])
        .unwrap();
    cache::membership_invalidated(&f.state);
    let refreshed = ready(&f.state);
    assert_eq!(refreshed.cleanup_count, 0);
    assert_eq!(
        refreshed
            .categories
            .iter()
            .find(|c| c.kind == "exact")
            .unwrap()
            .images,
        2
    );
}

#[test]
fn snapshot_failure_is_reported_and_not_restarted_for_the_same_key() {
    let f = Fixture::new(3);
    f.run().unwrap();
    let conn = open(&f.state).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_summary BEFORE INSERT ON workflow_plan BEGIN SELECT RAISE(ABORT,'synthetic summary failure'); END").unwrap();
    let failed = ready(&f.state);
    assert_eq!(failed.stage, "error");
    assert!(failed.categories.is_empty());
    assert!(failed
        .errors
        .iter()
        .any(|e| e.contains("synthetic summary failure")));
    for _ in 0..5 {
        assert_eq!(status_snapshot(&f.state).unwrap().stage, "error");
    }
    assert!(!summary_memo(&f.state).lock().unwrap().running);
    conn.execute_batch("DROP TRIGGER reject_summary").unwrap();
    // A regular read retains the error for this unchanged key. Only an explicit
    // retry restarts the work after the transient database failure is removed.
    assert_eq!(status_snapshot(&f.state).unwrap().stage, "error");
    assert_eq!(retry_status_snapshot(&f.state).unwrap().stage, "loading");
    assert_eq!(ready(&f.state).stage, "scanned");
}

#[test]
fn snapshot_defers_metadata_initialization_during_library_maintenance() {
    let f = Fixture::new(2);
    f.run().unwrap();
    f.state
        .maintenance
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(status_snapshot(&f.state).unwrap().stage, "loading");
    assert!(!summary_memo(&f.state).lock().unwrap().running);
    f.state
        .maintenance
        .store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(ready(&f.state).cleanup_count, 1);
}

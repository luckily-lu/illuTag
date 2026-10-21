use super::*;
use std::{collections::HashSet, path::PathBuf, sync::Arc};

struct Fixture {
    root: PathBuf,
    state: AppState,
}
impl Fixture {
    fn new() -> Self {
        let base = PathBuf::from(
            std::env::var_os("ILLUTAG_TEST_ROOT").expect("set an isolated test root"),
        );
        let root = base.join(format!("jobs-{}", next_id()));
        std::fs::create_dir_all(&root).unwrap();
        let state = AppState::new(root.join("library.sqlite"), root.clone(), None, None);
        database(&state).unwrap();
        Self { root, state }
    }
    fn add(&self, mut record: JobRecord) {
        store::advance(&mut record, None);
        persist(&record, &self.state).unwrap();
        self.state.jobs.records.lock().unwrap().push(record);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn row(id: &str, state: &str) -> JobRecord {
    JobRecord {
        id: id.into(),
        request: JobRequest {
            kind: "rules".into(),
            thumbnails: false,
            source_folder_id: None,
            duplicate_options: None,
            duplicate_operation_id: None,
            recycle_operation_id: None,
        },
        state: state.into(),
        stage: "rules".into(),
        processed: 0,
        total: 100,
        failures: 0,
        errors: vec![],
        updated_at: 0,
        meta: None,
    }
}
fn query(state: Option<&str>, cursor: Option<String>, limit: u32) -> JobHistoryQuery {
    JobHistoryQuery {
        state: state.map(str::to_string),
        cursor,
        limit: Some(limit),
    }
}

#[test]
fn cancellation_is_irreversible_and_terminal_snapshots_are_immutable() {
    let f = Fixture::new();
    f.add(row("one", "running"));
    let publish = |_: &JobRecord| {};
    let _gate = f.state.jobs.gate.lock().unwrap();
    f.state
        .jobs
        .commit_locked("one", &f.state, publish, |r| r.state = "cancelling".into())
        .unwrap();
    for state in ["running", "pausing", "paused", "completed"] {
        assert!(f
            .state
            .jobs
            .commit_locked("one", &f.state, publish, |r| r.state = state.into())
            .is_err());
        assert_eq!(store::load("one", &f.state).unwrap().state, "cancelling");
    }
    f.state
        .jobs
        .commit_locked("one", &f.state, publish, |r| r.state = "cancelled".into())
        .unwrap();
    let revision = store::load("one", &f.state).unwrap().meta.unwrap().revision;
    assert!(f
        .state
        .jobs
        .commit_locked("one", &f.state, publish, |r| r.processed = 99)
        .is_err());
    let final_row = store::load("one", &f.state).unwrap();
    assert_eq!(final_row.processed, 0);
    assert_eq!(final_row.meta.unwrap().revision, revision);
    assert!(store::control_state("cancelling", "resume").is_err());
    assert!(store::control_state("cancelling", "pause").is_err());
}

#[test]
fn pause_requires_acknowledgement_and_capabilities_follow_the_state() {
    let f = Fixture::new();
    f.add(row("one", "running"));
    let _gate = f.state.jobs.gate.lock().unwrap();
    let target = store::control_state("running", "pause").unwrap();
    assert_eq!(target, "pausing");
    f.state
        .jobs
        .commit_locked("one", &f.state, |_| {}, |r| r.state = target.into())
        .unwrap();
    let requested = f.state.jobs.find("one").unwrap();
    assert!(!requested.meta.as_ref().unwrap().can_resume);
    assert!(requested.meta.as_ref().unwrap().can_cancel);
    assert!(store::control_state("pausing", "resume").is_err());
    f.state
        .jobs
        .commit_locked("one", &f.state, |_| {}, |r| r.state = "paused".into())
        .unwrap();
    let paused = f.state.jobs.find("one").unwrap();
    assert!(paused.meta.unwrap().can_resume);
    assert_eq!(store::control_state("paused", "resume").unwrap(), "running");
}

#[test]
fn legacy_worker_pause_request_does_not_fabricate_acknowledgement() {
    let f = Fixture::new();
    *f.state.background_scan_running.lock().unwrap() = true;
    *f.state.thumbnail_generation_running.lock().unwrap() = true;
    *f.state.natural_language_scan_running.lock().unwrap() = true;
    assert!(pause_background_scan(&f.state).unwrap());
    assert!(pause_thumbnail_generation(&f.state).unwrap());
    assert!(pause_natural_language_scan(&f.state).unwrap());
    assert!(*f.state.background_scan_pause_requested.lock().unwrap());
    assert!(*f.state.thumbnail_generation_pause_requested.lock().unwrap());
    assert!(*f
        .state
        .natural_language_scan_pause_requested
        .lock()
        .unwrap());
    assert!(!background_scan_progress(&f.state).unwrap().paused);
    assert!(!thumbnail_generation_status(&f.state).unwrap().paused);
    assert!(!natural_language_scan_progress(&f.state).unwrap().paused);
}

#[test]
fn queued_cancellation_removes_the_fifo_entry_and_frees_capacity() {
    let f = Fixture::new();
    for n in 0..10 {
        let id = format!("q-{n}");
        f.add(row(&id, "queued"));
        f.state.jobs.queue.lock().unwrap().push_back(id);
    }
    let _gate = f.state.jobs.gate.lock().unwrap();
    f.state
        .jobs
        .cancel_queued_locked("q-4", &f.state, |_| {})
        .unwrap();
    let queue = f.state.jobs.queue.lock().unwrap();
    assert_eq!(queue.len(), 9);
    assert_eq!(
        queue.iter().map(String::as_str).collect::<Vec<_>>(),
        vec!["q-0", "q-1", "q-2", "q-3", "q-5", "q-6", "q-7", "q-8", "q-9"]
    );
    assert_eq!(f.state.jobs.find("q-4").unwrap().state, "cancelled");
}

#[test]
fn failed_claim_keeps_queued_work_and_success_claims_only_one_fifo_entry() {
    let f = Fixture::new();
    for id in ["first", "second"] {
        f.add(row(id, "queued"));
        f.state.jobs.queue.lock().unwrap().push_back(id.into());
    }
    let conn = database(&f.state).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_claim BEFORE INSERT ON task_runs BEGIN SELECT RAISE(ABORT,'synthetic rejection'); END").unwrap();
    let _gate = f.state.jobs.gate.lock().unwrap();
    assert!(f.state.jobs.claim_locked(&f.state, |_| {}).is_err());
    assert_eq!(f.state.jobs.queue.lock().unwrap().len(), 2);
    assert_eq!(f.state.jobs.find("first").unwrap().state, "queued");
    conn.execute_batch("DROP TRIGGER reject_claim").unwrap();
    let claimed = f
        .state
        .jobs
        .claim_locked(&f.state, |_| {})
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id, "first");
    assert_eq!(claimed.state, "running");
    assert_eq!(
        *f.state.jobs.queue.lock().unwrap(),
        VecDeque::from(["second".to_string()])
    );
    assert_eq!(f.state.jobs.find("second").unwrap().state, "queued");
}

#[test]
fn committed_events_are_monotonic_and_match_the_durable_snapshot() {
    let f = Fixture::new();
    f.add(row("one", "running"));
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut workers = Vec::new();
    for _ in 0..4 {
        let state = f.state.clone();
        let events = events.clone();
        workers.push(thread::spawn(move || {
            for _ in 0..12 {
                let _gate = state.jobs.gate.lock().unwrap();
                state
                    .jobs
                    .commit_locked(
                        "one",
                        &state,
                        |record| {
                            let saved = store::load("one", &state).unwrap();
                            assert_eq!(
                                saved.meta.as_ref().unwrap().revision,
                                record.meta.as_ref().unwrap().revision
                            );
                            events
                                .lock()
                                .unwrap()
                                .push(record.meta.as_ref().unwrap().revision);
                        },
                        |r| r.processed += 1,
                    )
                    .unwrap();
            }
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    let events = events.lock().unwrap();
    assert_eq!(*events, (2..=49).collect::<Vec<i64>>());
    assert_eq!(store::load("one", &f.state).unwrap().processed, 48);
}

#[test]
fn persistence_failure_does_not_publish_or_change_memory() {
    let f = Fixture::new();
    f.add(row("one", "running"));
    database(&f.state).unwrap().execute_batch("CREATE TRIGGER reject_job BEFORE INSERT ON task_runs BEGIN SELECT RAISE(ABORT,'synthetic rejection'); END").unwrap();
    let mut published = false;
    let _gate = f.state.jobs.gate.lock().unwrap();
    assert!(f
        .state
        .jobs
        .commit_locked(
            "one",
            &f.state,
            |_| published = true,
            |r| r.state = "cancelling".into()
        )
        .is_err());
    assert!(!published);
    assert_eq!(f.state.jobs.find("one").unwrap().state, "running");
    assert_eq!(store::load("one", &f.state).unwrap().state, "running");
}

#[test]
fn history_paginates_beyond_runtime_limit_with_ties_and_server_filtering() {
    let f = Fixture::new();
    for n in 0..237 {
        let mut record = row(
            &format!("history-{n:04}"),
            if n % 3 == 0 { "failed" } else { "completed" },
        );
        record.updated_at = 7;
        persist(&record, &f.state).unwrap();
    }
    f.state.jobs.hydrate(&f.state).unwrap();
    assert_eq!(f.state.jobs.list().unwrap().len(), 100);
    for (filter, expected) in [(None, 237), (Some("failed"), 79)] {
        let mut cursor = None;
        let mut ids = HashSet::new();
        loop {
            let page = history(query(filter, cursor, 17), &f.state).unwrap();
            assert!(page.rows.len() <= 17);
            for record in page.rows {
                assert!(ids.insert(record.id));
                if let Some(state) = filter {
                    assert_eq!(record.state, state);
                }
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(ids.len(), expected);
    }
    assert!(history(query(None, Some("bad cursor".into()), 1), &f.state).is_err());
    assert!(history(query(Some("running"), None, 1), &f.state).is_err());
}

#[test]
fn restart_recovers_all_unfinished_rows_without_resuming_work() {
    let f = Fixture::new();
    for n in 0..125 {
        let mut record = row(
            &format!("old-{n}"),
            if n % 2 == 0 { "queued" } else { "pausing" },
        );
        record.updated_at = n;
        persist(&record, &f.state).unwrap();
    }
    f.state.jobs.hydrate(&f.state).unwrap();
    assert_eq!(f.state.jobs.list().unwrap().len(), 100);
    let conn = database(&f.state).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM task_runs WHERE state='interrupted'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        125
    );
    assert!(f.state.jobs.queue.lock().unwrap().is_empty());
    assert!(!*f.state.jobs.running.lock().unwrap());
    assert!(!*f.state.background_scan_running.lock().unwrap());
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        3
    );
}

#[test]
fn clearing_history_requires_confirmation_and_preserves_active_work_and_journals() {
    let f = Fixture::new();
    f.add(row("done", "completed"));
    f.add(row("active", "pausing"));
    let operations = f.root.join("operations");
    std::fs::create_dir_all(&operations).unwrap();
    let journal = operations.join("duplicate-management.sqlite");
    std::fs::write(&journal, b"synthetic undo journal").unwrap();
    assert!(clear_history(false, &f.state).is_err());
    assert_eq!(
        history(query(None, None, 5), &f.state).unwrap().rows.len(),
        1
    );
    assert_eq!(clear_history(true, &f.state).unwrap(), 1);
    assert_eq!(f.state.jobs.list().unwrap().len(), 1);
    assert_eq!(store::load("active", &f.state).unwrap().state, "pausing");
    assert_eq!(std::fs::read(journal).unwrap(), b"synthetic undo journal");
    assert!(history(query(None, None, 5), &f.state)
        .unwrap()
        .rows
        .is_empty());
}

#[test]
fn retry_preserves_the_original_scope_options_and_batch_identity() {
    let mut original = row("original", "interrupted");
    original.request.kind = "duplicates".into();
    original.request.thumbnails = true;
    original.request.source_folder_id = Some(77);
    original.request.duplicate_options = Some(crate::duplicates::DuplicateOptions {
        sensitivity: "strict".into(),
        force_recompute: false,
        regroup_only: true,
        verification_mode: "organize".into(),
    });
    original.request.duplicate_operation_id = Some("frozen-operation".into());
    let expected = serde_json::to_value(&original.request).unwrap();
    assert_eq!(
        serde_json::to_value(retry_request(original).unwrap()).unwrap(),
        expected
    );
    assert!(retry_request(row("active", "pausing")).is_err());
}

#[test]
fn repeated_requests_share_the_active_job_but_different_scopes_can_queue() {
    let f = Fixture::new();
    let mut record = row("one", "pausing");
    record.request.source_folder_id = Some(9);
    f.add(record.clone());
    assert_eq!(
        f.state
            .jobs
            .active_request(&record.request)
            .unwrap()
            .unwrap()
            .id,
        "one"
    );
    record.request.source_folder_id = Some(10);
    assert!(f
        .state
        .jobs
        .active_request(&record.request)
        .unwrap()
        .is_none());
}

#[test]
fn terminal_write_lock_failure_keeps_the_queue_and_retry_saves_without_reexecution() {
    let f = Fixture::new();
    let mut finished = row("finished", "running");
    finished.processed = 37;
    f.add(finished);
    f.add(row("next", "queued"));
    f.state.jobs.queue.lock().unwrap().push_back("next".into());
    {
        let _gate = f.state.jobs.gate.lock().unwrap();
        f.state
            .jobs
            .defer_terminal_locked("finished", "completed")
            .unwrap();
    }
    // A real independent SQLite writer prevents the terminal UPDATE/INSERT.
    let writer = database(&f.state).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    let events = Mutex::new(Vec::<JobRecord>::new());
    assert!(retry_terminal_save_with("finished", &f.state, |r| events
        .lock()
        .unwrap()
        .push(r.clone()))
    .is_err());
    assert_eq!(
        events.lock().unwrap().len(),
        3,
        "one recovery round must be bounded"
    );
    let waiting = f.state.jobs.find("finished").unwrap();
    assert_eq!(
        waiting.state, "running",
        "a failed write must not publish a terminal state"
    );
    let meta = waiting.meta.as_ref().unwrap();
    assert!(meta.persistence_error.is_some());
    assert!(meta.can_retry);
    assert!(!meta.can_pause && !meta.can_resume && !meta.can_cancel);
    assert_eq!(
        store::load("finished", &f.state)
            .unwrap()
            .meta
            .unwrap()
            .revision,
        1
    );
    assert_eq!(
        f.state
            .jobs
            .list()
            .unwrap()
            .iter()
            .find(|r| r.id == "finished")
            .unwrap()
            .meta
            .as_ref()
            .unwrap()
            .revision,
        meta.revision
    );
    {
        let _gate = f.state.jobs.gate.lock().unwrap();
        assert!(f.state.jobs.claim_locked(&f.state, |_| {}).is_err());
    }
    assert_eq!(f.state.jobs.queue.lock().unwrap().len(), 1);
    assert_eq!(f.state.jobs.find("next").unwrap().state, "queued");
    writer.execute_batch("ROLLBACK").unwrap();
    assert_eq!(
        clear_history(true, &f.state).unwrap(),
        0,
        "pending results are still active"
    );
    let saved = retry_terminal_save_with("finished", &f.state, |r| {
        events.lock().unwrap().push(r.clone())
    })
    .unwrap();
    assert_eq!(saved.state, "completed");
    assert_eq!(
        saved.processed, 37,
        "saving must not repeat the completed work"
    );
    assert!(saved.meta.as_ref().unwrap().persistence_error.is_none());
    assert!(saved.meta.as_ref().unwrap().revision > meta.revision);
    assert!(f.state.jobs.pending_id().is_none());
    assert_eq!(
        store::load("finished", &f.state).unwrap().state,
        "completed"
    );
    let published = events.lock().unwrap();
    assert!(published
        .windows(2)
        .all(|p| p[0].meta.as_ref().unwrap().revision < p[1].meta.as_ref().unwrap().revision));
    drop(published);
    // A late second save request returns the same result and cannot rerun work.
    let repeated =
        retry_terminal_save_with("finished", &f.state, |_| panic!("already saved")).unwrap();
    assert_eq!(
        repeated.meta.unwrap().revision,
        saved.meta.unwrap().revision
    );
    let _gate = f.state.jobs.gate.lock().unwrap();
    assert_eq!(
        f.state
            .jobs
            .claim_locked(&f.state, |_| {})
            .unwrap()
            .unwrap()
            .id,
        "next"
    );
}

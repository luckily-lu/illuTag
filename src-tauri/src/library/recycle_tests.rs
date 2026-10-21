use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture {
    root: PathBuf,
    state: AppState,
    ids: Vec<String>,
}
impl Fixture {
    fn new(count: usize) -> Self {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let base =
            PathBuf::from(env::var_os("ILLUTAG_TEST_ROOT").expect("set an isolated test root"));
        assert!(
            base.is_absolute()
                && base
                    .components()
                    .any(|c| c.as_os_str() == "illuTag-refactor-work")
        );
        let root = base.join(format!(
            "recycle-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        assert!(!root.exists());
        fs::create_dir_all(&root).unwrap();
        let state = AppState::new(root.join("illutag.sqlite"), root.clone(), None, None);
        let mut c = open_database(&state.database_path).unwrap();
        c.execute(
            "INSERT INTO folders(id,path,added_at) VALUES(1,?1,0)",
            [root.to_string_lossy().as_ref()],
        )
        .unwrap();
        let tx = c.transaction().unwrap();
        let mut ids = Vec::new();
        for n in 0..count {
            let path = root.join(format!("synthetic-{n:05}.txt"));
            fs::write(&path, b"recycle-test-fixture").unwrap();
            let sig = fingerprint::source_signature(&path).unwrap();
            let ident = path.to_string_lossy().into_owned();
            tx.execute("INSERT INTO images(image_pk,id,path,file_name,ext,width,height,file_size,modified_at,imported_at,folder_id,trashed,is_favorite) VALUES(?1,?2,?2,?3,'txt',1,1,?4,?5,0,1,1,1)",
                params![n as i64+1,ident,format!("synthetic-{n:05}.txt"),sig.file_size as i64,sig.modified_ns/1_000_000]).unwrap();
            ids.push(ident);
        }
        tx.execute("INSERT INTO user_folders(id,name,sort_order,created_at,updated_at) VALUES(1,'保留分类',0,0,0)",[]).unwrap();
        tx.execute(
            "INSERT INTO storage_image_user_folders SELECT image_pk,1,0 FROM images",
            [],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO storage_image_user_custom_tags SELECT image_pk,'保留标签',0,0 FROM images",
            [],
        )
        .unwrap();
        tx.commit().unwrap();
        drop(c);
        Self { root, state, ids }
    }
    fn select(&self) -> RecycleSelection {
        prepare(
            RecycleSelectionRequest {
                query: Some(query()),
                image_ids: vec![],
                selection_id: None,
                excluded_ids: vec![],
            },
            &self.state,
        )
        .unwrap()
    }
    fn confirm(&self, id: &str, action: &str) {
        journal(&self.state)
            .unwrap()
            .execute(
                "UPDATE selections SET action=?2,state='queued' WHERE id=?1",
                params![id, action],
            )
            .unwrap();
    }
    fn run(&self, id: &str) -> RecycleOperationStatus {
        run_with(id, &self.state, &mut || Ok(()), &mut |_| {}, &mut |p| {
            let target = self.root.join(format!(
                "moved-{}",
                p.file_name().unwrap().to_string_lossy()
            ));
            fs::rename(p, target).map_err(|e| e.to_string())
        })
        .unwrap();
        status(id, &self.state).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // Only the fresh root created by this fixture; never clean prior runs.
        let expected=format!("recycle-{}-",std::process::id());
        if self.root.file_name().is_some_and(|n|n.to_string_lossy().starts_with(&expected)) {
            let _=fs::remove_dir_all(&self.root);
        }
    }
}
fn query() -> GalleryQuery {
    serde_json::from_value(serde_json::json!({"scope":"trash","folderId":null,"unclassifiedOnlyParentFolderId":null,"tags":[],"englishQuery":"","fileName":"","naturalLanguage":"","offset":0,"limit":1,"randomSeed":0})).unwrap()
}

#[test]
fn recycle_full_selection_exclusions_resume_and_metadata() {
    let f = Fixture::new(5007);
    let first = f.select();
    assert_eq!(first.count, 5007);
    let selected = prepare(
        RecycleSelectionRequest {
            query: None,
            image_ids: vec![],
            selection_id: Some(first.selection_id),
            excluded_ids: vec![f.ids[0].clone()],
        },
        &f.state,
    )
    .unwrap();
    assert_eq!(selected.count, 5006);
    assert_eq!(selected.total_bytes, 5006 * 20);
    f.confirm(&selected.selection_id, "restore");
    let mut chunks = 0;
    assert!(run_with(
        &selected.selection_id,
        &f.state,
        &mut || {
            chunks += 1;
            if chunks > 3 {
                Err("cancel".into())
            } else {
                Ok(())
            }
        },
        &mut |_| {},
        &mut |_| panic!("restore must not move files")
    )
    .is_err());
    let partial = status(&selected.selection_id, &f.state).unwrap();
    assert!(partial.processed > 0 && partial.processed < 5006);
    let done = f.run(&selected.selection_id);
    assert_eq!(
        (done.processed, done.succeeded, done.failed),
        (5006, 5006, 0)
    );
    let c = open_database(&f.state.database_path).unwrap();
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM images WHERE trashed=1", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    for sql in [
        "SELECT COUNT(*) FROM images WHERE is_favorite=1",
        "SELECT COUNT(*) FROM storage_image_user_folders",
        "SELECT COUNT(*) FROM storage_image_user_custom_tags",
    ] {
        assert_eq!(c.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap(), 5007);
    }
    assert!(f.ids.iter().all(|p| Path::new(p).exists()));
}

#[test]
fn recycle_filter_scope_frozen_snapshot_and_manual_decision() {
    let f = Fixture::new(4);
    let mut q = query();
    q.file_name = "synthetic-00001".into();
    q.tags = vec!["保留标签".into()];
    let only = prepare(
        RecycleSelectionRequest {
            query: Some(q),
            image_ids: vec![],
            selection_id: None,
            excluded_ids: vec![],
        },
        &f.state,
    )
    .unwrap();
    assert_eq!(only.count, 1);
    let all = f.select();
    // A later explicit user decision invalidates ownership, even if trashed again.
    api::mutate_images(
        ImageMutation {
            action: "restore".into(),
            image_ids: vec![f.ids[0].clone()],
            folder_id: None,
            favorite: None,
            custom_tags: vec![],
            supplement_tags: vec![],
        },
        &f.state,
    )
    .unwrap();
    api::mutate_images(
        ImageMutation {
            action: "trash".into(),
            image_ids: vec![f.ids[0].clone()],
            folder_id: None,
            favorite: None,
            custom_tags: vec![],
            supplement_tags: vec![],
        },
        &f.state,
    )
    .unwrap();
    f.confirm(&all.selection_id, "restore");
    let s = f.run(&all.selection_id);
    assert_eq!((s.succeeded, s.skipped), (3, 1));
    let mut bad = query();
    bad.scope = "all".into();
    assert!(prepare(
        RecycleSelectionRequest {
            query: Some(bad),
            image_ids: vec![],
            selection_id: None,
            excluded_ids: vec![]
        },
        &f.state
    )
    .is_err());
    let page = results(
        RecycleOperationQuery {
            operation_id: all.selection_id,
            cursor: None,
            limit: 1,
        },
        &f.state,
    )
    .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].image_id, f.ids[0]);
}

#[test]
fn recycle_system_move_retains_changed_missing_and_active_aliases() {
    let f = Fixture::new(4);
    let selected = f.select();
    fs::write(&f.ids[1], b"changed-size").unwrap();
    fs::remove_file(&f.ids[2]).unwrap();
    let c = open_database(&f.state.database_path).unwrap();
    c.execute("INSERT INTO images(id,path,file_name,ext,width,height,file_size,modified_at,imported_at,folder_id,trashed) SELECT 'active-alias',UPPER(path),'alias','txt',1,1,file_size,modified_at,0,1,0 FROM images WHERE image_pk=4",[]).unwrap();
    f.confirm(&selected.selection_id, "systemTrash");
    let s = f.run(&selected.selection_id);
    assert_eq!((s.succeeded, s.failed, s.skipped), (1, 3, 0));
    assert!(!Path::new(&f.ids[0]).exists());
    assert!(Path::new(&f.ids[3]).exists());
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM images", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        4
    );
    // Retry cannot move a newly-created replacement after a recorded file intent.
    let log = journal(&f.state).unwrap();
    log.execute("UPDATE items SET state='pending',signature='old-file-identity' WHERE selection=?1 AND pk=2",[&selected.selection_id]).unwrap();
    fs::write(&f.ids[1], b"recycle-test-fixture").unwrap();
    let sig = fingerprint::source_signature(Path::new(&f.ids[1])).unwrap();
    c.execute(
        "UPDATE images SET modified_at=?1 WHERE image_pk=2",
        [sig.modified_ns / 1_000_000],
    )
    .unwrap();
    log.execute(
        "UPDATE items SET modified=?2 WHERE selection=?1 AND pk=2",
        params![selected.selection_id, sig.modified_ns / 1_000_000],
    )
    .unwrap();
    let mut moved = false;
    run_with(
        &selected.selection_id,
        &f.state,
        &mut || Ok(()),
        &mut |_| {},
        &mut |_| {
            moved = true;
            Ok(())
        },
    )
    .unwrap();
    assert!(!moved);
    assert!(Path::new(&f.ids[1]).exists());
}

#[test]
fn recycle_crash_recovery_never_guesses_ambiguous_file_move() {
    let f = Fixture::new(3);
    let s = f.select();
    f.confirm(&s.selection_id, "systemTrash");
    let log = journal(&f.state).unwrap();
    for pk in 1..=3 {
        fs::remove_file(&f.ids[pk - 1]).unwrap();
    }
    log.execute(
        "UPDATE items SET phase=CASE WHEN pk=1 THEN 'intent' ELSE 'moved' END WHERE selection=?1",
        [&s.selection_id],
    )
    .unwrap();
    open_database(&f.state.database_path)
        .unwrap()
        .execute("DELETE FROM images WHERE image_pk=3", [])
        .unwrap();
    let done = f.run(&s.selection_id);
    assert_eq!((done.succeeded, done.failed), (2, 1));
    assert_eq!(
        open_database(&f.state.database_path)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM images", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    // A prepared selection cannot bypass action confirmation through start_job.
    let other = f.select();
    assert!(validate_queued(&other.selection_id, "recycleDelete", &f.state).is_err());
    assert!(validate_queued(&s.selection_id, "recycleRestore", &f.state).is_err());
}

#[test]
fn recycle_shell_batches_record_partial_moves_and_keep_failed_files() {
    let f = Fixture::new(37);
    let s = f.select();
    f.confirm(&s.selection_id, "systemTrash");
    let mut sizes = Vec::new();
    run_with_batches(&s.selection_id, &f.state, &mut || Ok(()), &mut |_| {}, &mut |paths| {
        sizes.push(paths.len());
        for (index, path) in paths.iter().enumerate() {
            if sizes.len() == 1 && index == 7 { return Err("synthetic partial shell failure".into()); }
            fs::rename(path, f.root.join(format!("moved-{}", path.file_name().unwrap().to_string_lossy()))).map_err(|e|e.to_string())?;
        }
        Ok(())
    }).unwrap();
    assert_eq!(sizes, vec![16, 16, 5]);
    let done = status(&s.selection_id, &f.state).unwrap();
    assert_eq!((done.processed, done.succeeded, done.failed), (37, 28, 9));
    assert_eq!(f.ids.iter().filter(|p| Path::new(p).exists()).count(), 9);
    let db = open_database(&f.state.database_path).unwrap();
    for sql in ["SELECT COUNT(*) FROM images", "SELECT COUNT(*) FROM storage_image_user_custom_tags"] {
        assert_eq!(db.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap(), 9);
    }
}

#[test]
fn recycle_stops_between_small_batches_and_resumes_without_repeating_moves() {
    let f = Fixture::new(37);
    let s = f.select();
    f.confirm(&s.selection_id, "systemTrash");
    let calls = std::cell::Cell::new(0);
    assert!(run_with_batches(&s.selection_id, &f.state, &mut || {
        assert!(f.state.write_lock.try_lock().is_ok(), "Control must run outside the main write lock");
        if calls.get() > 0 { Err("cancel".into()) } else { Ok(()) }
    }, &mut |_| {}, &mut |paths| {
        calls.set(calls.get()+1);
        for path in paths { fs::rename(path, f.root.join(format!("moved-{}", path.file_name().unwrap().to_string_lossy()))).map_err(|e|e.to_string())?; }
        Ok(())
    }).is_err());
    assert_eq!(calls.get(), 1);
    assert_eq!(status(&s.selection_id, &f.state).unwrap().succeeded, 16);
    let done = f.run(&s.selection_id);
    assert_eq!((done.succeeded, done.failed), (37, 0));
}

#[test]
fn recycle_batch_database_failure_repairs_only_recorded_moves() {
    let f = Fixture::new(4);
    let s = f.select();
    f.confirm(&s.selection_id, "systemTrash");
    let db = open_database(&f.state.database_path).unwrap();
    db.execute_batch("CREATE TRIGGER synthetic_delete_failure BEFORE DELETE ON images WHEN OLD.image_pk=2 BEGIN SELECT RAISE(ABORT,'synthetic write failure'); END;").unwrap();
    assert!(run_with_batches(&s.selection_id, &f.state, &mut || Ok(()), &mut |_| {}, &mut |paths| {
        for path in paths { fs::rename(path, f.root.join(format!("moved-{}", path.file_name().unwrap().to_string_lossy()))).map_err(|e|e.to_string())?; }
        Ok(())
    }).is_err());
    assert_eq!(db.query_row("SELECT COUNT(*) FROM images", [], |r| r.get::<_, i64>(0)).unwrap(), 4);
    assert_eq!(status(&s.selection_id, &f.state).unwrap().processed, 0);
    db.execute_batch("DROP TRIGGER synthetic_delete_failure").unwrap();
    run_with_batches(&s.selection_id, &f.state, &mut || Ok(()), &mut |_| {}, &mut |_| panic!("Recorded moved files must never be moved twice")).unwrap();
    assert_eq!(status(&s.selection_id, &f.state).unwrap().succeeded, 4);
}

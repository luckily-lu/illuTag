use super::*;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
static SERIAL: AtomicUsize = AtomicUsize::new(0);
struct Fixture {
    root: PathBuf,
    state: AppState,
}
impl Fixture {
    fn new(count: usize) -> Self {
        let root = env::temp_dir().join(format!(
            "illutag-refactor-test-{}-{}-{}",
            std::process::id(),
            now_ms(),
            SERIAL.fetch_add(1, AtomicOrdering::SeqCst)
        ));
        fs::create_dir(&root).unwrap();
        let db = root.join("illutag.sqlite");
        let state = AppState::new(db.clone(), root.clone(), None, None);
        let mut conn = open_database(&db).unwrap();
        let tx = conn.transaction().unwrap();
        tx.execute(
            "INSERT INTO folders(id,path,added_at) VALUES(1,?1,0)",
            [root.to_string_lossy().as_ref()],
        )
        .unwrap();
        {
            let mut insert=tx.prepare("INSERT INTO images(id,path,file_name,ext,width,height,file_size,modified_at,imported_at,folder_id) VALUES(?1,?1,?2,'png',640,480,100,?3,0,1)").unwrap();
            for i in 0..count {
                let id = root
                    .join(format!("image-{i:06}.png"))
                    .to_string_lossy()
                    .to_string();
                insert
                    .execute(params![id, format!("image-{i:06}.png"), i as i64])
                    .unwrap();
            }
        }
        tx.commit().unwrap();
        Self { root, state }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[test]
fn migration_only_moves_forward() {
    let f = Fixture::new(1);
    let conn = Connection::open(&f.state.database_path).unwrap();
    conn.pragma_update(None, "user_version", 2).unwrap();
    drop(conn);
    assert!(open_database(&f.state.database_path)
        .unwrap_err()
        .contains("离线存储升级"));
    let conn = Connection::open(&f.state.database_path).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    conn.pragma_update(None, "user_version", 4).unwrap();
    drop(conn);
    assert!(open_database(&f.state.database_path)
        .unwrap_err()
        .contains("高于"));
    let conn = Connection::open(&f.state.database_path).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        4
    );
}

#[test]
fn favorite_tags_survive_reopen_without_changing_images_or_schema() {
    let f = Fixture::new(2);
    let conn = open_database(&f.state.database_path).unwrap();
    let before: String = conn
        .query_row(
            "SELECT group_concat(id||':'||trashed) FROM images",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        set_favorite_tag("long_hair".into(), true, &f.state).unwrap(),
        vec!["long_hair"]
    );
    assert_eq!(
        set_favorite_tag("long_hair".into(), true, &f.state)
            .unwrap()
            .len(),
        1
    );
    let reopened = AppState::new(f.state.database_path.clone(), f.root.clone(), None, None);
    assert_eq!(list_favorite_tags(&reopened).unwrap(), vec!["long_hair"]);
    assert_eq!(
        conn.query_row(
            "SELECT group_concat(id||':'||trashed) FROM images",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        before
    );
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        3
    );
    assert!(set_favorite_tag("long_hair".into(), false, &f.state)
        .unwrap()
        .is_empty());
}

#[test]
fn viewer_undo_restores_only_its_latest_unchanged_mutation() {
    let f = Fixture::new(2);
    let conn = open_database(&f.state.database_path).unwrap();
    let ids: Vec<String> = conn
        .prepare("SELECT id FROM images ORDER BY image_pk")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let request = |action: &str, id: &str| ImageMutation {
        action: action.into(),
        image_ids: vec![id.into()],
        folder_id: None,
        favorite: Some(true),
        custom_tags: vec![],
        supplement_tags: vec![],
    };
    let removed = mutate_images(request("trash", &ids[0]), &f.state).unwrap();
    assert_eq!(removed.successful_ids.len(), 1);
    assert!(
        !undo_viewer_trash(ids[0].clone(), removed.revision, &f.state)
            .unwrap()
            .images[0]
            .trashed
    );
    let removed = mutate_images(request("trash", &ids[0]), &f.state).unwrap();
    mutate_images(request("favorite", &ids[1]), &f.state).unwrap();
    assert!(
        undo_viewer_trash(ids[0].clone(), removed.revision, &f.state)
            .unwrap_err()
            .contains("后续修改")
    );
    assert!(load_image_record(&conn, &ids[0]).unwrap().trashed);
}

#[test]
fn tag_queue_keeps_stable_id_separate_from_real_path() {
    let f = Fixture::new(2);
    let path = f
        .root
        .join("image-000000.png")
        .to_string_lossy()
        .into_owned();
    let normal = f
        .root
        .join("image-000001.png")
        .to_string_lossy()
        .into_owned();
    let conn = open_database(&f.state.database_path).unwrap();
    conn.execute("UPDATE images SET id='legacy-id' WHERE path=?1", [&path])
        .unwrap();
    drop(conn);

    let queue = collect_pending_tag_images(&open_database(&f.state.database_path).unwrap()).unwrap();
    let item = queue
        .iter()
        .find(|item| item.path == path)
        .expect("mapped image must be queued");
    assert_eq!(item.id, "legacy-id");
    assert_eq!(item.path, path);
    let plain = queue
        .iter()
        .find(|item| item.path == normal)
        .expect("normal image must be queued");
    assert_eq!(plain.id, normal);
}

#[test]
fn changed_source_invalidates_auto_tags_and_requeues() {
    let f = Fixture::new(1);
    let path = f
        .root
        .join("image-000000.png")
        .to_string_lossy()
        .into_owned();
    let conn = open_database(&f.state.database_path).unwrap();
    compact::upsert_auto_tag(
        &conn,
        &path,
        "general",
        "cat",
        None,
        0.9,
        WD_TAGGER_MODEL_NAME,
        1,
    )
    .unwrap();
    conn.execute(
        "INSERT INTO storage_image_user_custom_tags(image_id,tag_text,created_at,updated_at) SELECT image_pk,'manual',0,0 FROM images WHERE path=?1",
        [&path],
    )
    .unwrap();
    drop(conn);

    let mut conn = open_database(&f.state.database_path).unwrap();
    let mut known_paths = HashSet::from([path.clone()]);
    let mut new_ids = Vec::new();
    let mut updated = 0;
    let mut skipped = 0;
    flush_scanned_image_batch(
        &mut conn,
        1,
        &[ScannedImage {
            path: path.clone(),
            file_name: "image-000000.png".into(),
            ext: "png".into(),
            width: 640,
            height: 480,
            file_size: 200,
            modified_at: 5,
            imported_at: 0,
            unchanged: false,
        }],
        &mut known_paths,
        &mut new_ids,
        &mut updated,
        &mut skipped,
    )
    .unwrap();

    assert_eq!(updated, 1);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM storage_auto_tags", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0,
        "auto tags from the old source bytes must be invalidated"
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM storage_image_user_custom_tags", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1,
        "manual tags must survive"
    );
    let queue = collect_pending_tag_images(&conn).unwrap();
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].id, path);
    assert_eq!(queue[0].path, path);
}

#[test]
fn changed_source_invalidates_auto_tags_for_mapped_id() {
    let f = Fixture::new(1);
    let path = f
        .root
        .join("image-000000.png")
        .to_string_lossy()
        .into_owned();
    let conn = open_database(&f.state.database_path).unwrap();
    conn.execute("UPDATE images SET id='legacy-id' WHERE path=?1", [&path])
        .unwrap();
    compact::upsert_auto_tag(
        &conn,
        "legacy-id",
        "general",
        "cat",
        None,
        0.9,
        WD_TAGGER_MODEL_NAME,
        1,
    )
    .unwrap();
    drop(conn);

    let mut conn = open_database(&f.state.database_path).unwrap();
    let mut known_paths = HashSet::from([path.clone()]);
    let mut new_ids = Vec::new();
    let mut updated = 0;
    let mut skipped = 0;
    flush_scanned_image_batch(
        &mut conn,
        1,
        &[ScannedImage {
            path: path.clone(),
            file_name: "image-000000.png".into(),
            ext: "png".into(),
            width: 640,
            height: 480,
            file_size: 200,
            modified_at: 5,
            imported_at: 0,
            unchanged: false,
        }],
        &mut known_paths,
        &mut new_ids,
        &mut updated,
        &mut skipped,
    )
    .unwrap();

    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM storage_auto_tags", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0,
        "invalidation must resolve the image by path, not by a stale public id"
    );
    let queue = collect_pending_tag_images(&conn).unwrap();
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].id, "legacy-id");
    assert_eq!(queue[0].path, path);
}

#[test]
fn unchanged_scan_keeps_auto_tags() {
    let f = Fixture::new(1);
    let path = f
        .root
        .join("image-000000.png")
        .to_string_lossy()
        .into_owned();
    let conn = open_database(&f.state.database_path).unwrap();
    compact::upsert_auto_tag(
        &conn,
        &path,
        "general",
        "cat",
        None,
        0.9,
        WD_TAGGER_MODEL_NAME,
        1,
    )
    .unwrap();
    drop(conn);

    let mut conn = open_database(&f.state.database_path).unwrap();
    let mut known_paths = HashSet::from([path.clone()]);
    let mut new_ids = Vec::new();
    let mut updated = 0;
    let mut skipped = 0;
    flush_scanned_image_batch(
        &mut conn,
        1,
        &[ScannedImage {
            path: path.clone(),
            file_name: "image-000000.png".into(),
            ext: "png".into(),
            width: 640,
            height: 480,
            file_size: 100,
            modified_at: 0,
            imported_at: 0,
            unchanged: true,
        }],
        &mut known_paths,
        &mut new_ids,
        &mut updated,
        &mut skipped,
    )
    .unwrap();

    assert_eq!(skipped, 1);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM storage_auto_tags", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

fn pending_assignment_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM scan_pending_assignments", [], |r| {
        r.get(0)
    })
    .unwrap()
}

fn image_folder_rows(conn: &Connection, image_path: &str) -> Vec<(i64, String)> {
    conn.prepare(
        "SELECT u.id,u.source_kind FROM storage_image_user_folders f
         JOIN images i ON i.image_pk=f.image_id
         JOIN user_folders u ON u.id=f.folder_id
         WHERE i.path=?1
         ORDER BY u.id",
    )
    .unwrap()
    .query_map([image_path], |r| Ok((r.get(0)?, r.get(1)?)))
    .unwrap()
    .collect::<Result<Vec<_>, _>>()
    .unwrap()
}

struct FakeTagSession {
    sends: std::collections::VecDeque<Result<(), String>>,
    reads: std::collections::VecDeque<Result<WdTaggerTestResult, String>>,
    sent_ids: Vec<String>,
    restarts: usize,
}
impl TagSession for FakeTagSession {
    fn send(&mut self, image: &TagQueueImage) -> Result<(), String> {
        self.sent_ids.push(image.id.clone());
        self.sends.pop_front().unwrap_or(Ok(()))
    }
    fn read(&mut self) -> Result<WdTaggerTestResult, String> {
        self.reads.pop_front().unwrap_or_else(|| Err("no response".into()))
    }
    fn restart(&mut self) -> Result<(), String> {
        self.restarts += 1;
        Ok(())
    }
}
fn wd_result(id: &str) -> WdTaggerTestResult {
    WdTaggerTestResult {
        image_id: id.into(),
        ratings: vec![],
        general_tags: vec![],
        character_tags: vec![],
        general_threshold: 0.35,
        character_threshold: 0.85,
        elapsed_ms: 0.0,
    }
}
fn pipeline_images(f: &Fixture, count: usize) -> Vec<TagQueueImage> {
    (0..count)
        .map(|index| {
            let path = f.root.join(format!("pipeline-{index}.png"));
            fs::write(&path, b"pipeline fixture").unwrap();
            TagQueueImage {
                id: format!("image-{index}"),
                path: path.to_string_lossy().into_owned(),
            }
        })
        .collect()
}
fn run_fake_pipeline(
    session: &mut FakeTagSession,
    images: &[TagQueueImage],
) -> (
    Result<(), String>,
    Vec<String>,
    Vec<String>,
    i64,
) {
    let mut saved = Vec::<String>::new();
    let mut failures = Vec::<String>::new();
    let mut saved_count = 0;
    let result = run_tag_pipeline(
        session,
        images,
        &mut || Ok(true),
        &mut |batch| {
            saved.extend(batch.iter().map(|(id, _)| id.clone()));
            Ok(())
        },
        &mut |error| failures.push(error.into()),
        &mut |count| saved_count += count,
    );
    (result, saved, failures, saved_count)
}

#[test]
fn prefetch_send_failure_resends_unread_requests_without_duplicates() {
    let f = Fixture::new(0);
    let images = pipeline_images(&f, 2);
    let mut session = FakeTagSession {
        sends: std::collections::VecDeque::from([
            Ok(()),
            Err("child exited".into()),
            Ok(()),
            Ok(()),
        ]),
        reads: std::collections::VecDeque::from([Ok(wd_result("image-0")), Ok(wd_result("image-1"))]),
        sent_ids: vec![],
        restarts: 0,
    };
    let (result, saved, failures, count) = run_fake_pipeline(&mut session, &images);
    result.unwrap();
    assert_eq!(saved, vec!["image-0", "image-1"]);
    assert_eq!(count, 2);
    assert!(failures.is_empty());
    assert_eq!(session.restarts, 1);
    assert_eq!(
        session.sent_ids,
        vec!["image-0", "image-1", "image-0", "image-1"],
        "the failed send and its already-accepted prefetch must be re-sent in order"
    );
}

#[test]
fn prefetch_restart_does_not_recount_an_already_skipped_missing_image() {
    let f = Fixture::new(0);
    let images = pipeline_images(&f, 3);
    fs::remove_file(&images[0].path).unwrap();
    let mut session = FakeTagSession {
        sends: std::collections::VecDeque::from([
            Ok(()),
            Err("child exited".into()),
            Ok(()),
            Ok(()),
        ]),
        reads: std::collections::VecDeque::from([Ok(wd_result("image-1")), Ok(wd_result("image-2"))]),
        sent_ids: vec![],
        restarts: 0,
    };
    let (result, saved, failures, count) = run_fake_pipeline(&mut session, &images);
    result.unwrap();
    assert_eq!(saved, vec!["image-1", "image-2"]);
    assert_eq!(count, 2);
    assert_eq!(
        failures.len(),
        1,
        "the missing image must be reported once: {failures:?}"
    );
    assert!(failures[0].contains("pipeline-0"), "{failures:?}");
    assert_eq!(session.restarts, 1);
    assert_eq!(session.sent_ids, vec!["image-1", "image-2", "image-1", "image-2"]);
}

#[test]
fn read_restart_does_not_recount_an_already_skipped_missing_image() {
    let f = Fixture::new(0);
    let images = pipeline_images(&f, 3);
    fs::remove_file(&images[1].path).unwrap();
    let mut session = FakeTagSession {
        sends: std::collections::VecDeque::from([Ok(()), Ok(()), Ok(()), Ok(())]),
        reads: std::collections::VecDeque::from([
            Err("child exited".into()),
            Ok(wd_result("image-0")),
            Ok(wd_result("image-2")),
        ]),
        sent_ids: vec![],
        restarts: 0,
    };
    let (result, saved, failures, count) = run_fake_pipeline(&mut session, &images);
    result.unwrap();
    assert_eq!(saved, vec!["image-0", "image-2"]);
    assert_eq!(count, 2);
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(failures[0].contains("pipeline-1"), "{failures:?}");
    assert_eq!(session.restarts, 1);
    assert_eq!(session.sent_ids, vec!["image-0", "image-2", "image-0", "image-2"]);
}

#[test]
fn prefetch_read_failure_resends_from_the_unread_item() {
    let f = Fixture::new(0);
    let images = pipeline_images(&f, 2);
    let mut session = FakeTagSession {
        sends: std::collections::VecDeque::from([Ok(()), Ok(()), Ok(()), Ok(())]),
        reads: std::collections::VecDeque::from([
            Err("child exited".into()),
            Ok(wd_result("image-0")),
            Ok(wd_result("image-1")),
        ]),
        sent_ids: vec![],
        restarts: 0,
    };
    let (result, saved, failures, count) = run_fake_pipeline(&mut session, &images);
    result.unwrap();
    assert_eq!(saved, vec!["image-0", "image-1"]);
    assert_eq!(count, 2);
    assert!(failures.is_empty());
    assert_eq!(session.restarts, 1);
}

#[test]
fn prefetch_response_mismatch_stops_without_saving_wrong_tags() {
    let f = Fixture::new(0);
    let images = pipeline_images(&f, 1);
    let mut session = FakeTagSession {
        sends: std::collections::VecDeque::from([Ok(()), Ok(())]),
        reads: std::collections::VecDeque::from([Ok(wd_result("other-image"))]),
        sent_ids: vec![],
        restarts: 0,
    };
    let (result, saved, _, count) = run_fake_pipeline(&mut session, &images);
    assert!(result.is_err());
    assert!(saved.is_empty());
    assert_eq!(count, 0);
    assert_eq!(session.restarts, 1);
}

#[test]
fn migration_reports_an_unsupported_operation_journal() {
    let f = Fixture::new(1);
    let operations = f.root.join("operations");
    fs::create_dir_all(&operations).unwrap();
    let conn = Connection::open(operations.join("recycle-management.sqlite")).unwrap();
    conn.execute_batch(
        "CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
         INSERT INTO meta VALUES('binding','not-current');",
    )
    .unwrap();
    drop(conn);
    let destination_fixture = Fixture::new(0);
    let destination = destination_fixture.root.join("unsupported-migration");
    fs::create_dir(&destination).unwrap();
    let (_, _, reports) = prepare_data_directory_migration(&destination, &f.state).unwrap();
    let report = reports
        .iter()
        .find(|report| report.journal == "recycle-management.sqlite")
        .expect("journal must be reported");
    assert_eq!(report.status, "skipped");
    assert!(!report.message.is_empty());
    assert!(!destination
        .join("operations/recycle-management.sqlite")
        .exists());
    assert!(reports.iter().all(|report| report.unresolved.is_empty()));
}

fn recycle_signature_text(path: &Path) -> String {
    let source = crate::duplicates::fingerprint::source_signature(path).unwrap();
    serde_json::to_string(&(source.file_size, source.modified_ns, source.identity)).unwrap()
}

fn register_recycle_file(f: &Fixture, pk: i64, path: &Path) {
    let source = crate::duplicates::fingerprint::source_signature(path).unwrap();
    let id = path.to_string_lossy().into_owned();
    let conn = open_database(&f.state.database_path).unwrap();
    conn.execute(
        "UPDATE images SET id=?1,path=?1,file_size=?2,modified_at=?3,trashed=1 WHERE image_pk=?4",
        params![
            id,
            source.file_size as i64,
            source.modified_ns / 1_000_000,
            pk
        ],
    )
    .unwrap();
}

fn arm_recycle_selection(state: &AppState, selection: &str) {
    let conn = Connection::open(state.data_dir.join("operations/recycle-management.sqlite")).unwrap();
    conn.execute(
        "UPDATE selections SET action='systemTrash',state='queued' WHERE id=?1",
        [selection],
    )
    .unwrap();
}

fn fail_once_with_fake_mover(f: &Fixture, selection: &str) {
    let result = super::recycle::run_with(
        selection,
        &f.state,
        &mut || Ok(()),
        &mut |_| {},
        &mut |_| Err("synthetic mover failure".into()),
    );
    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn migration_does_not_reauth_a_replaced_external_recycle_source() {
    let f = Fixture::new(1);
    let external = f.root.parent().unwrap().join(format!(
        "recycle-external-{}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, AtomicOrdering::SeqCst)
    ));
    fs::create_dir_all(&external).unwrap();
    let external_file = external.join("pending.png");
    fs::write(&external_file, b"original-file").unwrap();
    register_recycle_file(&f, 1, &external_file);
    let id = external_file.to_string_lossy().into_owned();
    let selection = super::recycle::prepare(
        super::recycle::RecycleSelectionRequest {
            query: None,
            image_ids: vec![id],
            selection_id: None,
            excluded_ids: vec![],
        },
        &f.state,
    )
    .unwrap();
    arm_recycle_selection(&f.state, &selection.selection_id);
    fail_once_with_fake_mover(&f, &selection.selection_id);

    let old_metadata = fs::metadata(&external_file).unwrap();
    let old_modified = old_metadata.modified().unwrap();
    let before = recycle_signature_text(&external_file);
    fs::remove_file(&external_file).unwrap();
    fs::write(&external_file, b"replaced-data").unwrap();
    fs::File::options()
        .write(true)
        .open(&external_file)
        .unwrap()
        .set_modified(old_modified)
        .unwrap();
    assert_eq!(fs::metadata(&external_file).unwrap().len(), old_metadata.len());
    let after = recycle_signature_text(&external_file);
    assert_ne!(before, after, "the replacement must have a different identity");

    let destination = f.root.parent().unwrap().join(format!(
        "recycle-external-migrated-{}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, AtomicOrdering::SeqCst)
    ));
    fs::create_dir(&destination).unwrap();
    let (_, _, reports) =
        prepare_data_directory_migration(&destination, &f.state).unwrap();
    let report = reports
        .iter()
        .find(|report| report.journal == "recycle-management.sqlite")
        .expect("recycle report");
    assert_eq!(report.status, "partial", "{report:?}");
    assert!(
        report.unresolved.iter().any(|issue| issue.kind == "recycleItem"),
        "{report:?}"
    );
    let log = Connection::open(destination.join("operations/recycle-management.sqlite")).unwrap();
    let (state, signature): (String, String) = log
        .query_row(
            "SELECT state,signature FROM items WHERE selection=?1 AND pk=1",
            [&selection.selection_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, "skipped");
    assert_eq!(signature, before, "migration must preserve the old authorization");

    let mut new_state = AppState::new(
        destination.join("illutag.sqlite"),
        destination.clone(),
        None,
        None,
    );
    new_state.read_only = false;
    let mut moved = Vec::<PathBuf>::new();
    super::recycle::run_with(
        &selection.selection_id,
        &new_state,
        &mut || Ok(()),
        &mut |_| {},
        &mut |path| {
            moved.push(path.to_path_buf());
            Err("must not move a replaced file".into())
        },
    )
    .unwrap();
    assert!(moved.is_empty(), "{moved:?}");
    assert!(external_file.is_file());
    let _ = fs::remove_dir_all(&external);
    let _ = fs::remove_dir_all(&destination);
}

#[test]
fn migration_rebinds_an_unchanged_managed_recycle_source() {
    let f = Fixture::new(1);
    let clipboard = f.root.join("clipboard");
    fs::create_dir_all(&clipboard).unwrap();
    let old_file = clipboard.join("pending.png");
    image::RgbaImage::from_pixel(4, 4, image::Rgba([9, 8, 7, 255]))
        .save(&old_file)
        .unwrap();
    register_recycle_file(&f, 1, &old_file);
    let id = old_file.to_string_lossy().into_owned();
    let selection = super::recycle::prepare(
        super::recycle::RecycleSelectionRequest {
            query: None,
            image_ids: vec![id],
            selection_id: None,
            excluded_ids: vec![],
        },
        &f.state,
    )
    .unwrap();
    arm_recycle_selection(&f.state, &selection.selection_id);
    fail_once_with_fake_mover(&f, &selection.selection_id);

    let destination = f.root.parent().unwrap().join(format!(
        "recycle-managed-migrated-{}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, AtomicOrdering::SeqCst)
    ));
    fs::create_dir(&destination).unwrap();
    let (_, _, reports) =
        prepare_data_directory_migration(&destination, &f.state).unwrap();
    let report = reports
        .iter()
        .find(|report| report.journal == "recycle-management.sqlite")
        .expect("recycle report");
    assert_eq!(report.status, "migrated", "{report:?}");
    assert!(report.unresolved.is_empty(), "{report:?}");

    let mut new_state = AppState::new(
        destination.join("illutag.sqlite"),
        destination.clone(),
        None,
        None,
    );
    new_state.read_only = false;
    let expected = destination.join("clipboard/pending.png");
    let log = Connection::open(destination.join("operations/recycle-management.sqlite")).unwrap();
    // A retry resets failed items to pending before the job runs (same as apply_request).
    log.execute(
        "UPDATE items SET state='pending',message='' WHERE selection=?1",
        [&selection.selection_id],
    )
    .unwrap();
    drop(log);
    let mut moved = Vec::<PathBuf>::new();
    super::recycle::run_with(
        &selection.selection_id,
        &new_state,
        &mut || Ok(()),
        &mut |_| {},
        &mut |path| {
            moved.push(path.to_path_buf());
            Err("synthetic mover failure".into())
        },
    )
    .unwrap();
    let log = Connection::open(destination.join("operations/recycle-management.sqlite")).unwrap();
    let (state, message): (String, String) = log
        .query_row(
            "SELECT state,message FROM items WHERE selection=?1 AND pk=1",
            [&selection.selection_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        moved,
        vec![expected.clone()],
        "state={state} message={message}"
    );
    assert!(expected.is_file());
    let _ = fs::remove_dir_all(&destination);
}

#[test]
fn migration_does_not_reauth_a_replaced_managed_recycle_source() {
    let f = Fixture::new(1);
    let clipboard = f.root.join("clipboard");
    fs::create_dir_all(&clipboard).unwrap();
    let old_file = clipboard.join("pending.png");
    image::RgbaImage::from_pixel(4, 4, image::Rgba([9, 8, 7, 255]))
        .save(&old_file)
        .unwrap();
    register_recycle_file(&f, 1, &old_file);
    let id = old_file.to_string_lossy().into_owned();
    let selection = super::recycle::prepare(
        super::recycle::RecycleSelectionRequest {
            query: None,
            image_ids: vec![id],
            selection_id: None,
            excluded_ids: vec![],
        },
        &f.state,
    )
    .unwrap();
    arm_recycle_selection(&f.state, &selection.selection_id);
    fail_once_with_fake_mover(&f, &selection.selection_id);

    let old_metadata = fs::metadata(&old_file).unwrap();
    let old_modified = old_metadata.modified().unwrap();
    let before = recycle_signature_text(&old_file);
    fs::remove_file(&old_file).unwrap();
    fs::write(&old_file, vec![0u8; old_metadata.len() as usize]).unwrap();
    fs::File::options()
        .write(true)
        .open(&old_file)
        .unwrap()
        .set_modified(old_modified)
        .unwrap();
    assert_ne!(before, recycle_signature_text(&old_file));

    let destination = f.root.parent().unwrap().join(format!(
        "recycle-managed-replaced-migrated-{}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, AtomicOrdering::SeqCst)
    ));
    fs::create_dir(&destination).unwrap();
    let (_, _, reports) =
        prepare_data_directory_migration(&destination, &f.state).unwrap();
    let report = reports
        .iter()
        .find(|report| report.journal == "recycle-management.sqlite")
        .expect("recycle report");
    assert_eq!(report.status, "partial", "{report:?}");

    let mut new_state = AppState::new(
        destination.join("illutag.sqlite"),
        destination.clone(),
        None,
        None,
    );
    new_state.read_only = false;
    let mut moved = Vec::<PathBuf>::new();
    super::recycle::run_with(
        &selection.selection_id,
        &new_state,
        &mut || Ok(()),
        &mut |_| {},
        &mut |path| {
            moved.push(path.to_path_buf());
            Err("must not move a replaced file".into())
        },
    )
    .unwrap();
    assert!(moved.is_empty(), "{moved:?}");
    let _ = fs::remove_dir_all(&destination);
}

#[test]
fn migration_rebinds_recycle_journal_and_restores_inside_clipboard() {
    let f = Fixture::new(1);
    let clipboard = f.root.join("clipboard");
    fs::create_dir_all(&clipboard).unwrap();
    let old_path = clipboard.join("pending.png");
    image::RgbaImage::from_pixel(4, 4, image::Rgba([9, 8, 7, 255]))
        .save(&old_path)
        .unwrap();
    let signature = crate::duplicates::fingerprint::source_signature(&old_path).unwrap();
    let old_id = old_path.to_string_lossy().into_owned();
    let conn = open_database(&f.state.database_path).unwrap();
    conn.execute(
        "UPDATE images SET id=?1,path=?1,file_size=?2,modified_at=?3,trashed=1 WHERE image_pk=1",
        params![
            old_id,
            signature.file_size as i64,
            signature.modified_ns / 1_000_000
        ],
    )
    .unwrap();
    drop(conn);

    let selection = super::recycle::prepare(
        super::recycle::RecycleSelectionRequest {
            query: None,
            image_ids: vec![old_id.clone()],
            selection_id: None,
            excluded_ids: vec![],
        },
        &f.state,
    )
    .unwrap();
    assert_eq!(selection.count, 1);
    let journal = Connection::open(f.root.join("operations/recycle-management.sqlite")).unwrap();
    journal
        .execute(
            "UPDATE selections SET action='restore',state='queued' WHERE id=?1",
            [&selection.selection_id],
        )
        .unwrap();
    drop(journal);

    let destination_fixture = Fixture::new(0);
    let destination = destination_fixture.root.join("new-data");
    let (files, _, reports) = prepare_data_directory_migration(&destination, &f.state).unwrap();
    assert!(files >= 3);
    assert!(
        reports.iter().all(|report| report.status == "migrated"),
        "{reports:?}"
    );
    assert!(destination.join("clipboard/pending.png").is_file());

    let mut new_state = AppState::new(
        destination.join("illutag.sqlite"),
        destination.clone(),
        None,
        None,
    );
    new_state.read_only = false;
    super::recycle::run(
        &selection.selection_id,
        &new_state,
        &mut || Ok(()),
        &mut |_| {},
    )
    .unwrap();
    let new_conn = open_database(&new_state.database_path).unwrap();
    let (trashed, new_path): (i64, String) = new_conn
        .query_row("SELECT trashed,path FROM images WHERE image_pk=1", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(trashed, 0);
    assert!(Path::new(&new_path).is_file());
    assert!(
        new_path.starts_with(destination.to_string_lossy().as_ref()),
        "{new_path}"
    );
    let status = super::recycle::status(&selection.selection_id, &new_state).unwrap();
    assert_eq!(status.succeeded, 1);
}

#[test]
fn pending_scan_assignment_is_replayed_per_image() {
    let f = Fixture::new(1);
    let id = f
        .root
        .join("image-000000.png")
        .to_string_lossy()
        .into_owned();
    let conn = open_database(&f.state.database_path).unwrap();
    conn.execute(
        "INSERT INTO scan_pending_assignments(image_pk,created_at) SELECT image_pk,0 FROM images WHERE id=?1",
        [&id],
    )
    .unwrap();
    drop(conn);

    let conn = open_database(&f.state.database_path).unwrap();
    replay_pending_scan_assignments(&conn, 10, None).unwrap();
    assert_eq!(pending_assignment_count(&conn), 0);
    assert_eq!(
        image_folder_rows(&conn, &id),
        vec![(1, "library_dir".to_string())]
    );
    // Idempotent replay: nothing pending, no duplicates.
    replay_pending_scan_assignments(&conn, 11, None).unwrap();
    assert_eq!(image_folder_rows(&conn, &id).len(), 1);
}

#[test]
fn pending_scan_assignment_is_not_replayed_after_a_user_folder_edit() {
    let f = Fixture::new(1);
    let id = f
        .root
        .join("image-000000.png")
        .to_string_lossy()
        .into_owned();
    let conn = open_database(&f.state.database_path).unwrap();
    conn.execute("INSERT INTO user_folders(id,name,sort_order,created_at,updated_at) VALUES(7,'manual',0,0,0)", [])
        .unwrap();
    conn.execute(
        "INSERT INTO scan_pending_assignments(image_pk,created_at) SELECT image_pk,0 FROM images WHERE id=?1",
        [&id],
    )
    .unwrap();
    drop(conn);

    mutate_images(
        ImageMutation {
            action: "move".into(),
            image_ids: vec![id.clone()],
            folder_id: Some(7),
            favorite: None,
            custom_tags: vec![],
            supplement_tags: vec![],
        },
        &f.state,
    )
    .unwrap();
    let conn = open_database(&f.state.database_path).unwrap();
    assert_eq!(
        pending_assignment_count(&conn),
        0,
        "a user classification edit must cancel the pending initial ownership"
    );
    replay_pending_scan_assignments(&conn, 10, None).unwrap();
    assert_eq!(
        image_folder_rows(&conn, &id),
        vec![(7, "manual".to_string())],
        "replay must not re-add the directory folder"
    );
}

#[test]
fn pending_scan_assignment_uses_image_pk_for_mapped_id() {
    let f = Fixture::new(1);
    let path = f
        .root
        .join("image-000000.png")
        .to_string_lossy()
        .into_owned();
    let conn = open_database(&f.state.database_path).unwrap();
    conn.execute("UPDATE images SET id='legacy-id' WHERE path=?1", [&path])
        .unwrap();
    conn.execute(
        "INSERT INTO scan_pending_assignments(image_pk,created_at) SELECT image_pk,0 FROM images WHERE path=?1",
        [&path],
    )
    .unwrap();
    drop(conn);

    let conn = open_database(&f.state.database_path).unwrap();
    replay_pending_scan_assignments(&conn, 10, None).unwrap();
    assert_eq!(pending_assignment_count(&conn), 0);
    let pk: i64 = conn
        .query_row(
            "SELECT image_pk FROM images WHERE path=?1",
            [&path],
            |r| r.get(0),
        )
        .unwrap();
    let assigned: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM storage_image_user_folders WHERE image_id=?1",
            [pk],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(assigned, 1);
}

#[test]
fn pending_scan_assignment_survives_a_failed_replay() {
    let f = Fixture::new(1);
    let id = f
        .root
        .join("image-000000.png")
        .to_string_lossy()
        .into_owned();
    let conn = open_database(&f.state.database_path).unwrap();
    conn.execute(
        "INSERT INTO scan_pending_assignments(image_pk,created_at) SELECT image_pk,0 FROM images WHERE id=?1",
        [&id],
    )
    .unwrap();
    conn.execute_batch(
        "CREATE TRIGGER reject_assignment BEFORE INSERT ON storage_image_user_folders
         BEGIN SELECT RAISE(ABORT,'synthetic assignment failure'); END;",
    )
    .unwrap();
    assert!(replay_pending_scan_assignments(&conn, 10, None).is_err());
    assert_eq!(
        pending_assignment_count(&conn),
        1,
        "the pending marker must not be cleared when assignment failed"
    );
    conn.execute_batch("DROP TRIGGER reject_assignment").unwrap();
    replay_pending_scan_assignments(&conn, 11, None).unwrap();
    assert_eq!(pending_assignment_count(&conn), 0);
}

#[test]
fn pending_assignment_stale_candidate_cannot_re_add_a_manual_edit() {
    let f = Fixture::new(1);
    let id = f
        .root
        .join("image-000000.png")
        .to_string_lossy()
        .into_owned();
    let conn = open_database(&f.state.database_path).unwrap();
    conn.execute("INSERT INTO user_folders(id,name,sort_order,created_at,updated_at) VALUES(7,'manual',0,0,0)", [])
        .unwrap();
    conn.execute(
        "INSERT INTO scan_pending_assignments(image_pk,created_at) SELECT image_pk,0 FROM images WHERE id=?1",
        [&id],
    )
    .unwrap();
    // The scan read its replay work before the user's edit commits. Run the
    // edit after that read but before the immediate write transaction: the
    // stale Vec still contains the image, so only the in-transaction pending
    // check prevents the directory folder from being re-added.
    let stale = scan::load_pending_assignment_images(&conn, 1).unwrap();
    assert_eq!(stale.len(), 1);
    scan::complete_pending_assignments_with_hook(
        &conn,
        f.root.to_string_lossy().as_ref(),
        &stale,
        10,
        &mut || {
            mutate_images(
                ImageMutation {
                    action: "move".into(),
                    image_ids: vec![id.clone()],
                    folder_id: Some(7),
                    favorite: None,
                    custom_tags: vec![],
                    supplement_tags: vec![],
                },
                &f.state,
            )
            .unwrap();
        },
    )
    .unwrap();
    assert_eq!(
        image_folder_rows(&conn, &id),
        vec![(7, "manual".to_string())],
        "a stale pending list must not resurrect the directory folder after the edit"
    );
    assert_eq!(pending_assignment_count(&conn), 0);
}

#[test]
fn offline_source_pending_does_not_block_a_different_online_source_scan() {
    let f = Fixture::new(0);
    let a = f.root.join("source-a");
    let b = f.root.join("source-b");
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    let a_image = a.join("a.png");
    let b_image = b.join("b.png");
    fs::write(&a_image, b"a").unwrap();
    image::RgbaImage::from_pixel(2, 2, image::Rgba([4, 5, 6, 255]))
        .save(&b_image)
        .unwrap();
    let conn = open_database(&f.state.database_path).unwrap();
    conn.execute(
        "INSERT INTO folders(id,path,added_at) VALUES(10,?1,0),(11,?2,0)",
        params![a.to_string_lossy().as_ref(), b.to_string_lossy().as_ref()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO images(image_pk,id,path,file_name,ext,width,height,file_size,modified_at,imported_at,folder_id)
         VALUES(1,?1,?1,'a.png','png',1,1,1,0,0,10),(2,?2,?2,'b.png','png',1,1,1,0,0,11)",
        params![a_image.to_string_lossy().as_ref(), b_image.to_string_lossy().as_ref()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO scan_pending_assignments(image_pk,created_at) VALUES(1,0)",
        [],
    )
    .unwrap();
    drop(conn);

    fs::rename(&a, f.root.join("source-a-offline")).unwrap();
    let result = scan_all_folders_and_collect_new_images(
        &f.state.database_path,
        &f.state.foreground_queries,
        &f.state.background_scan_progress,
        &f.state.background_scan_pause_requested,
        &f.state.background_scan_stop_requested,
        false,
        Some(11),
    );
    assert!(
        result.is_ok(),
        "an offline pending source must not abort an online source scan: {:?}",
        result.err()
    );
    let conn = open_database(&f.state.database_path).unwrap();
    assert_eq!(
        pending_assignment_count(&conn),
        1,
        "the offline source marker must be retained"
    );
    assert_eq!(
        image_folder_rows(&conn, a_image.to_string_lossy().as_ref()),
        vec![]
    );

    let warnings =
        replay_pending_scan_assignments(&conn, 12, None).expect("offline groups are warnings");
    assert!(
        warnings.iter().any(|warning| warning.contains("source-a")),
        "{warnings:?}"
    );
    assert!(pending_assignment_count(&conn) == 1);

    fs::rename(f.root.join("source-a-offline"), &a).unwrap();
    replay_pending_scan_assignments(&conn, 13, Some(10)).unwrap();
    assert_eq!(pending_assignment_count(&conn), 0);
    let rows = image_folder_rows(&conn, a_image.to_string_lossy().as_ref());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1, "library_dir");
}

#[test]
fn normal_scan_clears_pending_assignment_after_folder_sync() {
    let f = Fixture::new(0);
    let image = f.root.join("new.png");
    image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]))
        .save(&image)
        .unwrap();
    let result = scan_all_folders_and_collect_new_images(
        &f.state.database_path,
        &f.state.foreground_queries,
        &f.state.background_scan_progress,
        &f.state.background_scan_pause_requested,
        &f.state.background_scan_stop_requested,
        false,
        None,
    )
    .unwrap();
    assert!(result.tag_queue_images.is_empty());
    let conn = open_database(&f.state.database_path).unwrap();
    assert_eq!(pending_assignment_count(&conn), 0);
    let id = normalize_existing_or_stored_folder_path(&image.to_string_lossy());
    assert_eq!(
        image_folder_rows(&conn, &id),
        vec![(1, "library_dir".to_string())]
    );
}

#[test]
fn thumbnail_path_repair_never_certifies_stale_content() {
    use super::inference::{
        expected_thumbnail_path_for_image_id, generate_single_thumbnail, repair_thumbnail_paths_for_current_data_dir,
    };

    let f = Fixture::new(1);
    let id = f
        .root
        .join("image-000000.png")
        .to_string_lossy()
        .into_owned();
    let conn = open_database(&f.state.database_path).unwrap();
    let thumb_root = f.root.join("thumbs");
    fs::create_dir_all(&thumb_root).unwrap();
    let expected = expected_thumbnail_path_for_image_id(&thumb_root, &id);
    let old = thumb_root.join("old.webp").to_string_lossy().into_owned();
    fs::write(&old, b"old thumbnail").unwrap();
    fs::write(&expected, b"unrelated file at expected path").unwrap();
    super::inference::upsert_thumbnail_record(&conn, &id, &old, 10, 10).unwrap();

    let candidate = |thumb: Option<String>, modified_at: i64, file_size: i64| ThumbnailCandidate {
        image_id: id.clone(),
        image_path: id.clone(),
        modified_at: 20,
        file_size: 200,
        current_thumb_path: thumb,
        current_source_modified_at: Some(modified_at),
        current_source_file_size: Some(file_size),
    };

    // The record describes old bytes at 10/10 while the source is now 20/20.
    let mut stale = vec![candidate(Some(old.clone()), 10, 10)];
    repair_thumbnail_paths_for_current_data_dir(&conn, &thumb_root, &mut stale).unwrap();
    assert_eq!(stale[0].current_thumb_path.as_deref(), Some(old.as_str()));
    assert_eq!(
        conn.query_row(
            "SELECT source_modified_at,source_file_size,thumb_path FROM storage_image_thumbnails",
            [],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?)),
        )
        .unwrap(),
        (10, 10, old.clone()),
        "a stale record must not be rewritten with the new source signature"
    );

    // A record that actually matches the source may be relocated.
    conn.execute(
        "UPDATE storage_image_thumbnails SET source_modified_at=20,source_file_size=200",
        [],
    )
    .unwrap();
    let mut current = vec![candidate(Some(old.clone()), 20, 200)];
    repair_thumbnail_paths_for_current_data_dir(&conn, &thumb_root, &mut current).unwrap();
    assert_eq!(
        current[0].current_thumb_path.as_deref(),
        Some(expected.as_str())
    );
    assert_eq!(
        conn.query_row(
            "SELECT source_modified_at,source_file_size,thumb_path FROM storage_image_thumbnails",
            [],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?)),
        )
        .unwrap(),
        (20, 200, expected.clone())
    );

    // No record at all: an existing file at the expected path is not proof.
    conn.execute("DELETE FROM storage_image_thumbnails", []).unwrap();
    let mut missing = vec![candidate(None, 0, 0)];
    repair_thumbnail_paths_for_current_data_dir(&conn, &thumb_root, &mut missing).unwrap();
    assert!(missing[0].current_thumb_path.is_none());
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM storage_image_thumbnails", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );

    // The real generation path then writes the current pixels to the expected path.
    image::RgbaImage::from_pixel(4, 4, image::Rgba([20, 40, 60, 255]))
        .save(&id)
        .unwrap();
    let generated = generate_single_thumbnail(&missing[0], &thumb_root, 64, 0.8).unwrap();
    assert_eq!(generated, expected);
    assert!(Path::new(&expected).is_file());
    assert_ne!(fs::read(&expected).unwrap(), b"unrelated file at expected path");
}

#[test]
fn compact_tags_preserve_translation_and_distinct_image_statistics() {
    let f = Fixture::new(1);
    let conn = open_database(&f.state.database_path).unwrap();
    let id = f
        .root
        .join("image-000000.png")
        .to_string_lossy()
        .into_owned();
    compact::upsert_auto_tag(
        &conn,
        &id,
        "general",
        "same",
        None,
        0.123456789123,
        "model",
        17,
    )
    .unwrap();
    compact::upsert_auto_tag(&conn, &id, "character", "same", Some(""), 0.9, "model", 18).unwrap();
    assert_eq!(
        conn.query_row("SELECT image_count FROM known_image_tags", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let values = conn
        .prepare("SELECT category,tag_zh,confidence FROM image_auto_tags ORDER BY category")
        .unwrap()
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, f64>(2)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(values[0].1, Some(String::new()));
    assert_eq!(values[1].1, None);
    assert_eq!(values[1].2.to_bits(), 0.123456789123_f64.to_bits());
    let pk = conn
        .query_row("SELECT image_pk FROM images", [], |r| r.get::<_, i64>(0))
        .unwrap();
    conn.execute(
        "UPDATE images SET path='D:/new-location.png' WHERE id=?1",
        [&id],
    )
    .unwrap();
    assert_eq!(
        conn.query_row("SELECT image_pk FROM images", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        pk
    );
    conn.execute("DELETE FROM storage_auto_tags WHERE category_pk=(SELECT string_pk FROM storage_strings WHERE value='general')",[]).unwrap();
    assert_eq!(
        conn.query_row("SELECT image_count FROM known_image_tags", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    conn.execute("DELETE FROM images WHERE id=?1", [&id])
        .unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM storage_auto_tags", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM known_image_tags", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn exact_ranking_retains_path_tie_break_and_top_600_boundaries() {
    for count in [599usize, 600, 601] {
        let mut cache = ClipImageVectorCache {
            model_id: CHINESE_CLIP_MODEL_ID.into(),
            model_version: CHINESE_CLIP_MODEL_VERSION.into(),
            dimension: 2,
            vectors: HashMap::new(),
        };
        let ids = (0..count)
            .map(|n| format!("D:/image-{n:05}.png"))
            .collect::<Vec<_>>();
        for id in ids.iter().rev() {
            cache.vectors.insert(id.clone(), vec![1.0, 0.0]);
        }
        assert_eq!(
            rank_clip_candidates(&cache, &[1.0, 0.0], None),
            ids[..count.min(600)]
        );
        let selected = HashSet::from([ids[count - 1].clone()]);
        assert_eq!(
            rank_clip_candidates(&cache, &[1.0, 0.0], Some(&selected)),
            vec![ids[count - 1].clone()]
        );
        assert!(rank_clip_candidates(&cache, &[1.0, 0.0], Some(&HashSet::new())).is_empty());
    }
}

#[test]
fn compact_search_combines_model_supplement_custom_and_filename_filters() {
    let f = Fixture::new(3);
    let ids = (0..3)
        .map(|n| {
            f.root
                .join(format!("image-{n:06}.png"))
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    let conn = open_database(&f.state.database_path).unwrap();
    compact::upsert_auto_tag(
        &conn,
        &ids[0],
        "general",
        "1girl",
        Some("女孩"),
        0.8,
        WD_TAGGER_MODEL_NAME,
        17,
    )
    .unwrap();
    compact::upsert_auto_tag(
        &conn,
        &ids[0],
        "general",
        "blue_hair",
        None,
        0.7,
        WD_TAGGER_MODEL_NAME,
        17,
    )
    .unwrap();
    drop(conn);
    add_image_user_supplement_tag(ids[1].clone(), "1girl".into(), None, &f.state).unwrap();
    add_image_user_custom_tag(ids[2].clone(), "1girl".into(), &f.state).unwrap();
    let page = query_gallery(
        GalleryQuery {
            tags: vec!["1girl".into()],
            ..Default::default()
        },
        &f.state,
    )
    .unwrap();
    assert_eq!(
        page.images.iter().map(|i| i.id.clone()).collect::<Vec<_>>(),
        ids.iter().rev().cloned().collect::<Vec<_>>()
    );
    let page = query_gallery(
        GalleryQuery {
            tags: vec!["1girl".into(), "blue_hair".into()],
            ..Default::default()
        },
        &f.state,
    )
    .unwrap();
    assert_eq!(page.images.len(), 1);
    assert_eq!(page.images[0].id, ids[0]);
    let page = query_gallery(
        GalleryQuery {
            tags: vec!["1girl".into()],
            file_name: "000001".into(),
            ..Default::default()
        },
        &f.state,
    )
    .unwrap();
    assert_eq!(page.images.len(), 1);
    assert_eq!(page.images[0].id, ids[1]);
    let page = query_gallery(
        GalleryQuery {
            english_query: "1girl".into(),
            ..Default::default()
        },
        &f.state,
    )
    .unwrap();
    assert_eq!(
        page.images.iter().map(|i| i.id.clone()).collect::<Vec<_>>(),
        vec![ids[1].clone(), ids[0].clone()]
    );
}

#[test]
fn old_schema_conversion_preserves_exact_vectors_and_historical_translation() {
    let f = Fixture::new(0);
    let legacy = f.root.join("legacy.sqlite");
    let conn = Connection::open(&legacy).unwrap();
    migrate_database(&conn).unwrap();
    conn.execute_batch("CREATE TABLE task_runs(id TEXT PRIMARY KEY,kind TEXT NOT NULL,state TEXT NOT NULL,payload TEXT NOT NULL,updated_at INTEGER NOT NULL); PRAGMA user_version=2;").unwrap();
    conn.execute(
        "INSERT INTO folders(id,path,added_at) VALUES(1,'D:/old',0)",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO images(id,path,file_name,ext,width,height,file_size,modified_at,imported_at,folder_id) VALUES('historic-id','D:/old/a.png','a.png','png',1,1,1,2,3,1)",[]).unwrap();
    conn.execute("INSERT INTO image_auto_tags VALUES('historic-id','general','term',NULL,0.123456789123,'legacy-model',19)",[]).unwrap();
    conn.execute("INSERT INTO image_auto_tags VALUES('historic-id','character','term','',0.5,'legacy-model',20)",[]).unwrap();
    conn.execute("INSERT INTO image_auto_tags VALUES('historic-id','unknown-category','other','历史译名',0.6,'other-model',21)",[]).unwrap();
    let blob = vec![1, 2, 3, 4, 5, 6, 7, 8];
    conn.execute(
        "INSERT INTO image_clip_embeddings VALUES('historic-id','clip','version',2,0,?1,27,29)",
        [&blob],
    )
    .unwrap();
    drop(conn);
    let before = fs::read(&legacy).unwrap();
    let target = f.root.join("converted.sqlite");
    compact::convert_staged(&legacy, &target).unwrap();
    assert_eq!(fs::read(&legacy).unwrap(), before);
    let old = Connection::open(&legacy).unwrap();
    let new = open_database(&target).unwrap();
    let logical = |c: &Connection| {
        c.prepare("SELECT * FROM image_auto_tags ORDER BY image_id,category,tag_en,model_name")
            .unwrap()
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, f64>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, i64>(6)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(logical(&old), logical(&new));
    assert_eq!(
        new.query_row("SELECT vector_blob FROM image_clip_embeddings", [], |r| r
            .get::<_, Vec<
            u8,
        >>(
            0
        ))
        .unwrap(),
        blob
    );
    assert_eq!(
        new.query_row("SELECT normalized FROM image_clip_embeddings", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        new.query_row("SELECT id,path FROM images", [], |r| Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?
        )))
        .unwrap(),
        ("historic-id".into(), "D:/old/a.png".into())
    );
}
#[test]
fn page_boundaries_and_complete_legacy_backup() {
    for count in [4999, 5000, 5001, 29999, 30000, 30001] {
        let f = Fixture::new(count);
        let first = query_gallery(GalleryQuery::default(), &f.state).unwrap();
        assert_eq!(first.total_image_count, count as i64);
        assert_eq!(first.images.len(), count.min(5000));
        let mut loaded = first.images.len();
        while loaded < count {
            let page = query_gallery(
                GalleryQuery {
                    offset: loaded as i64,
                    ..Default::default()
                },
                &f.state,
            )
            .unwrap();
            assert!(!page.images.is_empty());
            loaded += page.images.len();
        }
        assert_eq!(loaded, count);
        let conn = open_database(&f.state.database_path).unwrap();
        assert_eq!(
            build_data_migration_backup(&conn, serde_json::json!({}))
                .unwrap()
                .library
                .images
                .len(),
            count
        );
    }
}
#[test]
fn mutations_report_item_failure_and_keep_valid_results() {
    let f = Fixture::new(2);
    let id = f
        .root
        .join("image-000000.png")
        .to_string_lossy()
        .to_string();
    let result = mutate_images(
        ImageMutation {
            action: "favorite".into(),
            image_ids: vec![id.clone(), "missing-id".into()],
            favorite: Some(true),
            folder_id: None,
            custom_tags: vec![],
            supplement_tags: vec![],
        },
        &f.state,
    )
    .unwrap();
    assert_eq!(result.successful_ids, vec![id]);
    assert_eq!(result.failures.len(), 1);
    assert!(result.images[0].is_favorite);
    assert_eq!(result.images.len(), 1);
}

fn metadata_fixture() -> (Fixture, LibraryStore) {
    let f = Fixture::new(5001);
    let conn = open_database(&f.state.database_path).unwrap();
    conn.execute("INSERT INTO user_folders(id,name,sort_order,created_at,updated_at) VALUES(1,'已有归属',0,0,0),(2,'规则分类',1,0,0),(3,'补充分类',2,0,0)", []).unwrap();
    conn.execute(
        "INSERT INTO storage_image_user_folders SELECT image_pk,1,0 FROM images",
        [],
    )
    .unwrap();
    // Simulate an already populated legacy snapshot, including every membership.
    let cached = build_data_migration_backup(&conn, serde_json::json!({}))
        .unwrap()
        .library;
    drop(conn);
    assert_eq!(cached.images.len(), 5001);
    assert_eq!(cached.image_folders.len(), 5001);
    (f, cached)
}

fn assert_metadata_write<T: Serialize>(
    f: &Fixture,
    cached: &LibraryStore,
    work: impl FnOnce(&AppState) -> Result<T, String>,
) -> T {
    *f.state.library.lock().unwrap() = Some(cached.clone());
    let previous_revision = *f.state.startup_cleanup_generation.lock().unwrap();
    let response = work(&f.state).unwrap();
    assert!(serde_json::to_vec(&response).unwrap().len() < 1024);
    assert!(f.state.library.lock().unwrap().is_none());
    let navigation = get_navigation(&f.state).unwrap();
    let json = serde_json::to_value(navigation).unwrap();
    assert!(json.get("images").is_none());
    assert!(json.get("imageFolders").is_none());
    assert!(serde_json::to_vec(&json).unwrap().len() < 4096);
    // Reading a bounded response must not replace the legacy cache with a partial store.
    assert!(f.state.library.lock().unwrap().is_none());
    assert!(*f.state.startup_cleanup_generation.lock().unwrap() > previous_revision);
    response
}

#[test]
fn metadata_writes_do_not_reload_or_return_gallery_memberships() {
    let (f, cached) = metadata_fixture();
    let response = assert_metadata_write(&f, &cached, |state| {
        create_user_folder(None, "新分类".into(), state)
    });
    assert_eq!(serde_json::to_string(&response).unwrap(), "null");
    let folder_id = get_navigation(&f.state)
        .unwrap()
        .user_folders
        .iter()
        .find(|folder| folder.name == "新分类")
        .unwrap()
        .id;
    assert_metadata_write(&f, &cached, |state| {
        rename_user_folder(folder_id, "重命名分类".into(), state)
    });
    assert_metadata_write(&f, &cached, |state| {
        reorder_user_folder(folder_id, 2, state)
    });
    assert!(get_navigation(&f.state)
        .unwrap()
        .user_folders
        .iter()
        .any(|folder| folder.id == folder_id && folder.name == "重命名分类"));
    assert_metadata_write(&f, &cached, |state| {
        save_user_folder_rule(
            2,
            vec![UserFolderRuleCondition {
                logic: "AND".into(),
                source: "custom".into(),
                keyword: "分类标签".into(),
            }],
            false,
            state,
        )
    });
    assert_eq!(
        get_user_folder_rule(2, &f.state)
            .unwrap()
            .unwrap()
            .conditions[0]
            .keyword,
        "分类标签"
    );
    let id = f
        .root
        .join("image-005000.png")
        .to_string_lossy()
        .to_string();
    assert_metadata_write(&f, &cached, |state| {
        assign_image_to_user_folder(id.clone(), 2, state)
    });
    assert!(get_image_detail(id.clone(), &f.state)
        .unwrap()
        .folder_ids
        .contains(&2));
    assert_metadata_write(&f, &cached, |state| {
        remove_image_from_user_folder(id.clone(), 2, state)
    });
    assert_eq!(get_image_detail(id, &f.state).unwrap().folder_ids, vec![1]);
    assert_metadata_write(&f, &cached, |state| delete_user_folder(folder_id, state));
    let source = f.root.join("registered-source");
    fs::create_dir(&source).unwrap();
    let source_path = source.to_string_lossy().to_string();
    assert_metadata_write(&f, &cached, |state| {
        add_gallery_folder(source_path.clone(), state)
    });
    let registered = get_navigation(&f.state).unwrap().folders;
    assert_eq!(registered.len(), 2);
    assert!(registered
        .iter()
        .all(|source| source.last_scanned_at.is_none()));
    assert_metadata_write(&f, &cached, |state| {
        remove_gallery_folder(source_path, state)
    });
    assert_eq!(get_navigation(&f.state).unwrap().folders.len(), 1);
    let conn = open_database(&f.state.database_path).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM image_user_folders", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        5001
    );
    assert!(f.state.library.lock().unwrap().is_none());
}

#[test]
fn single_image_tags_return_local_data_and_keep_tag_manager_contract() {
    let (f, cached) = metadata_fixture();
    let id = f
        .root
        .join("image-005000.png")
        .to_string_lossy()
        .to_string();
    *f.state.clip_vector_cache.lock().unwrap() = Some(ClipImageVectorCache {
        model_id: CHINESE_CLIP_MODEL_ID.into(),
        model_version: CHINESE_CLIP_MODEL_VERSION.into(),
        dimension: 2,
        vectors: HashMap::from([(id.clone(), vec![1.0, 0.0])]),
    });
    save_user_folder_rule(
        2,
        vec![UserFolderRuleCondition {
            logic: "AND".into(),
            source: "custom".into(),
            keyword: "分类标签".into(),
        }],
        false,
        &f.state,
    )
    .unwrap();
    save_user_folder_rule(
        3,
        vec![UserFolderRuleCondition {
            logic: "AND".into(),
            source: "danbooru".into(),
            keyword: "blue_eyes".into(),
        }],
        false,
        &f.state,
    )
    .unwrap();
    let custom = assert_metadata_write(&f, &cached, |state| {
        add_image_user_custom_tag(id.clone(), "分类标签".into(), state)
    });
    assert_eq!(custom.image_id, id);
    assert_eq!(custom.custom_tags.len(), 1);
    assert_eq!(custom.custom_tags[0].tag_text, "分类标签");
    assert!(get_image_detail(id.clone(), &f.state)
        .unwrap()
        .folder_ids
        .contains(&2));
    let supplement = assert_metadata_write(&f, &cached, |state| {
        add_image_user_supplement_tag(id.clone(), "blue_eyes".into(), Some("蓝眼睛".into()), state)
    });
    assert_eq!(supplement.image_id, id);
    assert_eq!(supplement.supplement_tags.len(), 1);
    assert!(get_image_detail(id.clone(), &f.state)
        .unwrap()
        .folder_ids
        .contains(&3));
    let removed = assert_metadata_write(&f, &cached, |state| {
        remove_image_user_custom_tag(id.clone(), "分类标签".into(), state)
    });
    assert!(removed.custom_tags.is_empty());
    let removed = assert_metadata_write(&f, &cached, |state| {
        remove_image_user_supplement_tag(id.clone(), "blue_eyes".into(), state)
    });
    assert!(removed.supplement_tags.is_empty());
    let folder = create_user_tag_folder("标签分类".into(), &f.state).unwrap();
    let tag_folder_id = folder.folders[0].id;
    let created = create_user_custom_tag("共享标签".into(), &f.state).unwrap();
    assert!(created.unclassified_tags.contains(&"共享标签".into()));
    let assigned = assign_user_tag_to_folder(tag_folder_id, "共享标签".into(), &f.state).unwrap();
    assert_eq!(assigned.folders[0].tags, vec!["共享标签"]);
    let renamed = rename_user_tag_folder(tag_folder_id, "新标签分类".into(), &f.state).unwrap();
    assert_eq!(renamed.folders[0].name, "新标签分类");
    let unassigned = unassign_user_tag_from_folder("共享标签".into(), &f.state).unwrap();
    assert!(unassigned.unclassified_tags.contains(&"共享标签".into()));
    assert!(delete_user_tag_folder(tag_folder_id, &f.state)
        .unwrap()
        .folders
        .is_empty());
    let conn = open_database(&f.state.database_path).unwrap();
    conn.execute(
        "INSERT INTO storage_image_user_custom_tags SELECT image_pk,'共享标签',0,0 FROM images",
        [],
    )
    .unwrap();
    drop(conn);
    let deleted = assert_metadata_write(&f, &cached, |state| {
        delete_user_custom_tag("共享标签".into(), state)
    });
    assert!(!deleted.unclassified_tags.contains(&"共享标签".into()));
    let conn = open_database(&f.state.database_path).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM image_user_custom_tags WHERE tag_text='共享标签'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM image_user_folders", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        5003
    );
    assert_eq!(
        f.state
            .clip_vector_cache
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .vectors
            .get(&id),
        Some(&vec![1.0, 0.0])
    );
}

#[test]
fn failed_tag_rule_application_rolls_back_local_tag_write() {
    let f = Fixture::new(1);
    let conn = open_database(&f.state.database_path).unwrap();
    conn.execute("INSERT INTO user_folders(id,name,sort_order,created_at,updated_at) VALUES(1,'规则写入失败',0,0,0)", []).unwrap();
    let rule = serde_json::to_string(&vec![UserFolderRuleCondition {
        logic: "AND".into(),
        source: "custom".into(),
        keyword: "不能部分保存".into(),
    }])
    .unwrap();
    conn.execute("INSERT INTO user_folder_rules VALUES(1,?1,0)", [rule])
        .unwrap();
    conn.execute_batch("CREATE TRIGGER reject_rule_assignment BEFORE INSERT ON storage_image_user_folders BEGIN SELECT RAISE(ABORT,'fixture assignment fault'); END;").unwrap();
    drop(conn);
    let id = f
        .root
        .join("image-000000.png")
        .to_string_lossy()
        .to_string();
    assert!(add_image_user_custom_tag(id.clone(), "不能部分保存".into(), &f.state).is_err());
    assert!(list_image_user_tags(id, &f.state)
        .unwrap()
        .custom_tags
        .is_empty());
    let conn = open_database(&f.state.database_path).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM user_custom_tags", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let rule = serde_json::to_string(&vec![UserFolderRuleCondition {
        logic: "AND".into(),
        source: "danbooru".into(),
        keyword: "failed_supplement".into(),
    }])
    .unwrap();
    conn.execute("UPDATE user_folder_rules SET rule_json=?1", [rule])
        .unwrap();
    drop(conn);
    let id = f
        .root
        .join("image-000000.png")
        .to_string_lossy()
        .to_string();
    assert!(
        add_image_user_supplement_tag(id.clone(), "failed_supplement".into(), None, &f.state)
            .is_err()
    );
    assert!(list_image_user_tags(id, &f.state)
        .unwrap()
        .supplement_tags
        .is_empty());
}
#[test]
fn backup_restore_retains_vectors_historical_references_and_beyond_first_page() {
    let f = Fixture::new(5001);
    let id = f
        .root
        .join("image-005000.png")
        .to_string_lossy()
        .to_string();
    let conn = open_database(&f.state.database_path).unwrap();
    conn.execute(
        "INSERT INTO storage_clip_embeddings VALUES((SELECT image_pk FROM images WHERE id=?1),?2,2,1,?3,0,0)",
        params![
            id,
            compact::clip_model(&conn, CHINESE_CLIP_MODEL_ID, CHINESE_CLIP_MODEL_VERSION).unwrap(),
            vec![0u8; 8]
        ],
    )
    .unwrap();
    conn.execute("INSERT INTO reference_boards(id,name,created_at,updated_at,sort_order) VALUES(1,'history',0,0,0)",[]).unwrap();
    conn.execute(
        "INSERT INTO storage_reference_board_items(id,board_id,image_id,created_at) VALUES(1,1,(SELECT image_pk FROM images WHERE id=?1),0)",
        [&id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO storage_image_color_signatures VALUES((SELECT image_pk FROM images WHERE id=?1),0,100,?2,0)",
        params![id, vec![1u8; 8]],
    )
    .unwrap();
    drop(conn);
    fs::create_dir(f.root.join("clipboard")).unwrap();
    fs::write(f.root.join("clipboard/old.png"), b"historical asset").unwrap();
    let dest = f.root.join("packages");
    fs::create_dir(&dest).unwrap();
    let package = create_restore_package(
        dest.to_string_lossy().into(),
        serde_json::json!({"localStorage":{"illutag.themeMode":"light"}}),
        &f.state,
    )
    .unwrap();
    let m = inspect_restore_package(package.clone()).unwrap();
    assert_eq!(m.image_count, 5001);
    assert_eq!(m.counts["image_clip_embeddings"], 1);
    assert!(!m.external_originals_included);
    let target = Fixture::new(0);
    restore_package(package, vec![], &target.state).unwrap();
    let conn = Connection::open(&target.state.database_path).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM images", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        5001
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM image_color_signatures", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM reference_board_items", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(target.root.join("clipboard/old.png").is_file());
    verify_database(&conn).unwrap();
    assert!(target.state.maintenance.load(AtomicOrdering::SeqCst));
}
#[test]
fn rejected_snapshot_keeps_current_database() {
    let f = Fixture::new(1);
    let dest = f.root.join("packages");
    fs::create_dir(&dest).unwrap();
    let package = create_restore_package(
        dest.to_string_lossy().into(),
        serde_json::json!({}),
        &f.state,
    )
    .unwrap();
    fs::write(Path::new(&package).join("illutag.sqlite"), b"broken").unwrap();
    assert!(restore_package(package, vec![], &f.state).is_err());
    let conn = open_database(&f.state.database_path).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM images", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(!f.state.maintenance.load(AtomicOrdering::SeqCst));
}
#[test]
fn restart_marks_incomplete_jobs_interrupted() {
    let f = Fixture::new(0);
    let job = crate::jobs::JobRecord {
        id: "old".into(),
        request: crate::jobs::JobRequest {
            recycle_operation_id: None,
            duplicate_operation_id: None,
            duplicate_options: None,
            kind: "scan".into(),
            thumbnails: false,
            source_folder_id: None,
        },
        state: "running".into(),
        stage: "scan".into(),
        processed: 12,
        total: 100,
        failures: 0,
        errors: vec![],
        updated_at: 0,
        meta: None,
    };
    let conn = open_database(&f.state.database_path).unwrap();
    conn.execute(
        "INSERT INTO task_runs VALUES('old','scan','running',?1,0)",
        [serde_json::to_string(&job).unwrap()],
    )
    .unwrap();
    drop(conn);
    f.state.jobs.hydrate(&f.state).unwrap();
    assert_eq!(f.state.jobs.list().unwrap()[0].state, "interrupted");
    assert!(!*f.state.background_scan_running.lock().unwrap());
}
#[test]
fn offline_source_cleanup_preserves_images() {
    let f = Fixture::new(3);
    let conn = open_database(&f.state.database_path).unwrap();
    conn.execute("UPDATE folders SET path='Z:\\offline-illutag-source'", [])
        .unwrap();
    drop(conn);
    assert_eq!(
        cleanup_missing_library_images_batched(&f.state.database_path, 32).unwrap(),
        0
    );
}

#[test]
fn cleanup_removes_missing_files_and_keeps_accessible_files() {
    let f = Fixture::new(3);
    let existing = f.root.join("image-000000.png");
    fs::write(&existing, b"existing fixture").unwrap();
    assert_eq!(
        cleanup_missing_library_images_batched(&f.state.database_path, 32).unwrap(),
        2
    );
    let conn = open_database(&f.state.database_path).unwrap();
    let remaining: String = conn
        .query_row("SELECT path FROM images", [], |row| row.get(0))
        .unwrap();
    assert_eq!(remaining, existing.to_string_lossy());
}

#[test]
fn cleanup_does_not_treat_metadata_errors_as_missing_files() {
    let f = Fixture::new(1);
    {
        let conn = open_database(&f.state.database_path).unwrap();
        conn.execute("UPDATE images SET path=?1", ["invalid\0path.png"])
            .unwrap();
    }
    let error = cleanup_missing_library_images_batched(&f.state.database_path, 32).unwrap_err();
    assert!(error.contains("已停止清理并保留索引"), "{error}");
    let conn = open_database(&f.state.database_path).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM images", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn cleanup_worker_reports_failure_to_jobs_and_clears_it_on_retry() {
    let f = Fixture::new(1);
    {
        let conn = open_database(&f.state.database_path).unwrap();
        conn.execute("UPDATE images SET path=?1", ["invalid\0path.png"])
            .unwrap();
    }
    assert!(start_startup_cleanup(&f.state).unwrap());
    let wait_for_cleanup = || {
        let deadline = Instant::now() + Duration::from_secs(2);
        while startup_cleanup_status(&f.state).unwrap().running {
            assert!(Instant::now() < deadline, "cleanup worker did not finish");
            thread::sleep(Duration::from_millis(5));
        }
    };
    wait_for_cleanup();
    let (failures, errors) = crate::jobs::cleanup_failures(&f.state).unwrap();
    assert_eq!(failures, 1);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("已停止清理并保留索引"));
    let existing = f.root.join("image-000000.png");
    fs::write(&existing, b"existing fixture").unwrap();
    {
        let conn = open_database(&f.state.database_path).unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM images", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        conn.execute(
            "UPDATE images SET path=?1",
            [existing.to_string_lossy().as_ref()],
        )
        .unwrap();
    }
    assert!(start_startup_cleanup(&f.state).unwrap());
    wait_for_cleanup();
    let (failures, errors) = crate::jobs::cleanup_failures(&f.state).unwrap();
    assert_eq!(failures, 0);
    assert!(errors.is_empty());
}
#[test]
fn dto_serialization_matches_generated_contract() {
    let q = serde_json::to_value(GalleryQuery::default()).unwrap();
    assert!(q["folderId"].is_null());
    assert_eq!(q["limit"], 5000);
    assert!(q.get("file_name").is_none());
    let error = serde_json::to_value(AppError::from("error".to_string())).unwrap();
    assert_eq!(error["retryable"], false);
}

#[test]
fn classified_export_is_complete_beyond_first_page() {
    let f = Fixture::new(5001);
    let conn = open_database(&f.state.database_path).unwrap();
    conn.execute("INSERT INTO user_folders(id,name,sort_order,created_at,updated_at) VALUES(1,'全部分类',0,0,0)", []).unwrap();
    conn.execute(
        "INSERT INTO storage_image_user_folders SELECT image_pk,1,0 FROM images",
        [],
    )
    .unwrap();
    let mut stmt = conn.prepare("SELECT path FROM images").unwrap();
    for row in stmt.query_map([], |r| r.get::<_, String>(0)).unwrap() {
        fs::write(row.unwrap(), b"fixture").unwrap();
    }
    drop(stmt);
    drop(conn);
    let report =
        export_organized_folder_result(f.root.join("export").to_string_lossy().into(), &f.state)
            .unwrap();
    assert_eq!(report.exported_count, 5001);
    assert_eq!(report.failed_count, 0);
    let manifest: serde_json::Value =
        serde_json::from_reader(fs::File::open(&report.manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["entries"].as_array().unwrap().len(), 5001);
    assert_eq!(manifest["summary"]["exportedCount"], 5001);
    assert!(serde_json::to_vec(&report).unwrap().len() < 1024);
}

#[test]
fn directory_migration_validates_before_switch_and_preserves_source() {
    let f = Fixture::new(3);
    fs::create_dir(f.root.join("clipboard")).unwrap();
    fs::write(f.root.join("clipboard/owned.png"), b"managed fixture").unwrap();
    fs::write(f.root.join("window-state.json"), b"{}").unwrap();
    let occupied = f.root.join("occupied");
    fs::create_dir(&occupied).unwrap();
    fs::write(occupied.join("keep"), b"original").unwrap();
    assert!(prepare_data_directory_migration(&occupied, &f.state).is_err());
    assert!(f.state.database_path.is_file());
    assert_eq!(fs::read(occupied.join("keep")).unwrap(), b"original");
    let target = Fixture::new(0);
    let destination = target.root.join("new-data");
    let (files, _, _) = prepare_data_directory_migration(&destination, &f.state).unwrap();
    assert!(files >= 3);
    assert!(f.state.database_path.is_file());
    assert!(destination.join("clipboard/owned.png").is_file());
    let conn = open_database(&destination.join("illutag.sqlite")).unwrap();
    let path: String = conn
        .query_row("SELECT path FROM images LIMIT 1", [], |r| r.get(0))
        .unwrap();
    assert!(path.starts_with(destination.to_string_lossy().as_ref()));
    assert!(f.state.maintenance.load(AtomicOrdering::SeqCst));
    verify_database(&conn).unwrap();
}

#[test]
fn malformed_model_json_is_rejected() {
    let mut command = Command::new("python");
    command
        .args([
            "-u",
            "-c",
            "import sys;sys.stdin.readline();print('invalid json',flush=True)",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    apply_hidden_child_window(&mut command);
    let mut child = command.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = crate::protocol::ResponseReader::new(child.stdout.take().unwrap());
    assert!(run_clip_service_request(
        &mut stdin,
        &mut stdout,
        serde_json::json!({"text":"query"}),
        "text"
    )
    .is_err());
    child.wait().unwrap();
}

#[test]
fn sqlite_full_during_scan_batch_preserves_committed_images() {
    let f = Fixture::new(1);
    let mut conn = open_database(&f.state.database_path).unwrap();
    let page_count: i64 = conn
        .query_row("PRAGMA page_count", [], |row| row.get(0))
        .unwrap();
    conn.pragma_update(None, "max_page_count", page_count)
        .unwrap();
    let large_name = "x".repeat(128 * 1024);
    let batch = vec![ScannedImage {
        path: f.root.join("new.png").to_string_lossy().into(),
        file_name: large_name,
        ext: "png".into(),
        width: 1,
        height: 1,
        file_size: 100,
        modified_at: 0,
        imported_at: 0,
        unchanged: false,
    }];
    let mut known_paths = HashSet::new();
    let mut new_ids = Vec::new();
    let mut updated = 0;
    let mut skipped = 0;
    let error = flush_scanned_image_batch(
        &mut conn,
        1,
        &batch,
        &mut known_paths,
        &mut new_ids,
        &mut updated,
        &mut skipped,
    )
    .unwrap_err();
    assert!(error.contains("database or disk is full"), "{error}");
    assert!(conn.is_autocommit());
    assert!(known_paths.is_empty());
    assert!(new_ids.is_empty());
    assert_eq!((updated, skipped), (0, 0));
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM images", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    verify_database(&conn).unwrap();
}

fn scan_fixture(f: &Fixture) -> Result<ScanCollectResult, String> {
    scan_all_folders_and_collect_new_images(
        &f.state.database_path,
        &f.state.foreground_queries,
        &f.state.background_scan_progress,
        &f.state.background_scan_pause_requested,
        &f.state.background_scan_stop_requested,
        false,
        None,
    )
}

#[test]
fn exact_two_thousand_item_scan_write_failure_reaches_caller() {
    let f = Fixture::new(1);
    let assets = f.root.join("assets");
    fs::create_dir(&assets).unwrap();
    let first = assets.join("image-0000.png");
    image::RgbaImage::from_pixel(1, 1, image::Rgba([20, 40, 60, 255]))
        .save(&first)
        .unwrap();
    for i in 1..2000 {
        let next = assets.join(format!("image-{i:04}.png"));
        // NTFS allows only 1,024 links per file; use two tiny fixture inodes.
        if i == 1000 {
            fs::copy(&first, &next).unwrap();
        } else {
            let seed = if i < 1000 {
                first.clone()
            } else {
                assets.join("image-1000.png")
            };
            fs::hard_link(seed, next).unwrap();
        }
    }
    {
        let conn = open_database(&f.state.database_path).unwrap();
        conn.execute(
            "UPDATE folders SET path=?1",
            [assets.to_string_lossy().as_ref()],
        )
        .unwrap();
        conn.execute_batch(
            "CREATE TRIGGER reject_scan_image BEFORE INSERT ON images
             BEGIN SELECT RAISE(ABORT, 'injected scan write failure'); END;",
        )
        .unwrap();
    }
    let error = scan_fixture(&f)
        .err()
        .expect("a failed full batch must abort scanning");
    assert!(error.contains("injected scan write failure"), "{error}");
    let conn = open_database(&f.state.database_path).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM images", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let progress = f.state.background_scan_progress.lock().unwrap();
    assert_eq!(progress.new_images, 0);
    assert_eq!(progress.updated_images, 0);
}

#[test]
fn offline_source_scan_reports_error_without_deleting_index() {
    let f = Fixture::new(3);
    let missing = f.root.join("disconnected-source");
    {
        let conn = open_database(&f.state.database_path).unwrap();
        conn.execute(
            "UPDATE folders SET path=?1",
            [missing.to_string_lossy().as_ref()],
        )
        .unwrap();
    }
    let error = scan_fixture(&f)
        .err()
        .expect("offline source must be reported");
    assert!(error.contains("来源目录不可访问"), "{error}");
    let conn = open_database(&f.state.database_path).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM images", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        3
    );
}

#[test]
fn directory_enumeration_failure_is_not_an_empty_success() {
    let f = Fixture::new(0);
    let mut visits = 0;
    let error = scan_images(
        &f.root.join("missing-directory"),
        0,
        &mut HashSet::new(),
        &HashMap::new(),
        &mut |_| {
            visits += 1;
            Ok(true)
        },
    )
    .unwrap_err();
    assert!(error.contains("无法扫描来源目录"), "{error}");
    assert_eq!(visits, 0);
}

#[cfg(windows)]
#[test]
fn source_file_access_failure_is_reported_without_inserting_image() {
    use std::os::windows::fs::OpenOptionsExt;
    let f = Fixture::new(0);
    let file = f.root.join("locked.png");
    image::RgbaImage::from_pixel(1, 1, image::Rgba([20, 40, 60, 255]))
        .save(&file)
        .unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&file)
        .unwrap();
    let result = scan_fixture(&f);
    drop(lock);
    let error = result.err().expect("access failure must abort scanning");
    assert!(error.contains("无法读取来源图片"), "{error}");
    let conn = open_database(&f.state.database_path).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM images", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[cfg(windows)]
#[test]
fn inaccessible_windows_directory_is_reported_without_clearing_index() {
    use std::os::windows::fs::OpenOptionsExt;
    let f = Fixture::new(1);
    let assets = f.root.join("assets");
    fs::create_dir(&assets).unwrap();
    {
        let conn = open_database(&f.state.database_path).unwrap();
        conn.execute(
            "UPDATE folders SET path=?1",
            [assets.to_string_lossy().as_ref()],
        )
        .unwrap();
    }
    let directory_lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .custom_flags(0x02000000) // FILE_FLAG_BACKUP_SEMANTICS opens the directory itself.
        .open(&assets)
        .unwrap();
    let result = scan_fixture(&f);
    drop(directory_lock);
    let error = result
        .err()
        .expect("inaccessible directory must fail scanning");
    assert!(error.contains("无法扫描来源目录"), "{error}");
    let conn = open_database(&f.state.database_path).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM images", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn missing_clip_model_is_rejected_without_starting_python() {
    let f = Fixture::new(0);
    let missing = f.root.join("missing-model");
    let error = resolve_chinese_clip_model_dir(Some(missing.to_str().unwrap())).unwrap_err();
    assert!(error.contains("missing ONNX files"), "{error}");
    let incomplete = f.root.join("incomplete-model");
    fs::create_dir(&incomplete).unwrap();
    let error = resolve_chinese_clip_model_dir(Some(incomplete.to_str().unwrap())).unwrap_err();
    assert!(error.contains("missing ONNX files"), "{error}");
    assert!(f.state.clip_image_encoder_service.lock().unwrap().is_none());
    assert!(f.state.clip_text_encoder_service.lock().unwrap().is_none());
}

#[cfg(windows)]
#[test]
fn readonly_database_rejects_mutation_and_preserves_image() {
    let f = Fixture::new(1);
    {
        let conn = open_database(&f.state.database_path).unwrap();
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;")
            .unwrap();
    }
    let id = f
        .root
        .join("image-000000.png")
        .to_string_lossy()
        .to_string();
    let mut permissions = fs::metadata(&f.state.database_path).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&f.state.database_path, permissions).unwrap();
    let result = mutate_images(
        ImageMutation {
            action: "favorite".into(),
            image_ids: vec![id.clone()],
            favorite: Some(true),
            folder_id: None,
            custom_tags: vec![],
            supplement_tags: vec![],
        },
        &f.state,
    );
    let mut permissions = fs::metadata(&f.state.database_path).unwrap().permissions();
    permissions.set_readonly(false);
    fs::set_permissions(&f.state.database_path, permissions).unwrap();
    match result {
        Ok(result) => {
            assert!(result.successful_ids.is_empty());
            assert!(result.images.is_empty());
            assert_eq!(result.failures.len(), 1);
            assert!(result.failures[0].message.contains("readonly"));
        }
        Err(error) => assert!(error.contains("readonly"), "{error}"),
    }
    assert!(!get_image_detail(id, &f.state).unwrap().image.is_favorite);
}

#[cfg(windows)]
#[test]
fn migration_copy_failure_keeps_source_and_does_not_promote_staging() {
    use std::os::windows::fs::OpenOptionsExt;
    let f = Fixture::new(3);
    fs::create_dir(f.root.join("managed")).unwrap();
    let owned = f.root.join("managed/locked.png");
    fs::write(&owned, b"managed fixture").unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&owned)
        .unwrap();
    let destination_fixture = Fixture::new(0);
    let target = destination_fixture.root.join("new-data");
    let result = prepare_data_directory_migration(&target, &f.state);
    drop(lock);
    assert!(result.is_err());
    assert!(target.join("migration-staging/illutag.sqlite").is_file());
    assert!(!target.join("illutag.sqlite").exists());
    assert!(!f.state.maintenance.load(AtomicOrdering::SeqCst));
    assert_eq!(fs::read(&owned).unwrap(), b"managed fixture");
    let conn = open_database(&f.state.database_path).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM images", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        3
    );
    verify_database(&conn).unwrap();
}

#[cfg(windows)]
#[test]
fn restore_switch_failure_rolls_back_already_replaced_database() {
    use std::os::windows::fs::OpenOptionsExt;
    let source = Fixture::new(1);
    fs::create_dir(source.root.join("clipboard")).unwrap();
    fs::write(source.root.join("clipboard/new.png"), b"new fixture").unwrap();
    let packages = source.root.join("packages");
    fs::create_dir(&packages).unwrap();
    let package = create_restore_package(
        packages.to_string_lossy().into(),
        serde_json::json!({}),
        &source.state,
    )
    .unwrap();
    let target = Fixture::new(3);
    fs::create_dir(target.root.join("clipboard")).unwrap();
    let original = target.root.join("clipboard/locked.png");
    fs::write(&original, b"original fixture").unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&original)
        .unwrap();
    let result = restore_package(package, vec![], &target.state);
    drop(lock);
    let error = result.unwrap_err();
    assert!(error.contains("已回滚原数据"), "{error}");
    assert!(!target.state.maintenance.load(AtomicOrdering::SeqCst));
    assert_eq!(fs::read(&original).unwrap(), b"original fixture");
    assert!(!target.root.join("clipboard/new.png").exists());
    let conn = open_database(&target.state.database_path).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM images", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        3
    );
    verify_database(&conn).unwrap();
}

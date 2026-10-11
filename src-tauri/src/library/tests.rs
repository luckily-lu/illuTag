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
    conn.pragma_update(None, "user_version", 1).unwrap();
    drop(conn);
    let conn = open_database(&f.state.database_path).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    conn.pragma_update(None, "user_version", 3).unwrap();
    drop(conn);
    assert!(open_database(&f.state.database_path)
        .unwrap_err()
        .contains("高于"));
    let conn = Connection::open(&f.state.database_path).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        3
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
        "INSERT INTO image_clip_embeddings VALUES(?1,?2,?3,2,1,?4,0,0)",
        params![
            id,
            CHINESE_CLIP_MODEL_ID,
            CHINESE_CLIP_MODEL_VERSION,
            vec![0u8; 8]
        ],
    )
    .unwrap();
    conn.execute("INSERT INTO reference_boards(id,name,created_at,updated_at,sort_order) VALUES(1,'history',0,0,0)",[]).unwrap();
    conn.execute(
        "INSERT INTO reference_board_items(id,board_id,image_id,created_at) VALUES(1,1,?1,0)",
        [&id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO image_color_signatures VALUES(?1,0,100,?2,0)",
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
        "INSERT INTO image_user_folders SELECT id,1,0 FROM images",
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
    let (files, _) = prepare_data_directory_migration(&destination, &f.state).unwrap();
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

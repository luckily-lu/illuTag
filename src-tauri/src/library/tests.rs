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

fn metadata_fixture() -> (Fixture, LibraryStore) {
    let f = Fixture::new(5001);
    let conn = open_database(&f.state.database_path).unwrap();
    conn.execute("INSERT INTO user_folders(id,name,sort_order,created_at,updated_at) VALUES(1,'已有归属',0,0,0),(2,'规则分类',1,0,0),(3,'补充分类',2,0,0)", []).unwrap();
    conn.execute(
        "INSERT INTO image_user_folders SELECT id,1,0 FROM images",
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
        "INSERT INTO image_user_custom_tags SELECT id,'共享标签',0,0 FROM images",
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
    conn.execute_batch("CREATE TRIGGER reject_rule_assignment BEFORE INSERT ON image_user_folders BEGIN SELECT RAISE(ABORT,'fixture assignment fault'); END;").unwrap();
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
    let mut new_images = Vec::new();
    let mut updated = 0;
    let mut skipped = 0;
    let error = flush_scanned_image_batch(
        &mut conn,
        1,
        &batch,
        &mut known_paths,
        &mut new_ids,
        &mut new_images,
        &mut updated,
        &mut skipped,
    )
    .unwrap_err();
    assert!(error.contains("database or disk is full"), "{error}");
    assert!(conn.is_autocommit());
    assert!(known_paths.is_empty());
    assert!(new_ids.is_empty());
    assert!(new_images.is_empty());
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

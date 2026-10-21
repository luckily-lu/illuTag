use super::*;
use crate::duplicates::tests::Fixture;
use std::cell::Cell;

fn id(f: &Fixture, index: usize) -> String {
    f.paths[index].to_string_lossy().into_owned()
}
fn group(f: &Fixture) -> DuplicateGroup {
    let generation = cache::status(&f.state).unwrap().generation.unwrap();
    cache::query_groups(
        DuplicateQuery {
            generation,
            kind: "exact".into(),
            cursor: None,
            limit: 20,
        },
        &f.state,
    )
    .unwrap()
    .groups
    .remove(0)
}
fn request(
    f: &Fixture,
    g: &DuplicateGroup,
    mode: &str,
    keeps: Vec<String>,
    selected: Vec<String>,
) -> DuplicateManagementRequest {
    DuplicateManagementRequest {
        generation: cache::status(&f.state).unwrap().generation.unwrap(),
        group_id: g.id.clone(),
        mode: mode.into(),
        keep_ids: keeps,
        protected_ids: vec![],
        image_ids: selected,
        action: "trash".into(),
        source_operation_id: None,
    }
}
fn execute(f: &Fixture, operation: &str) -> Result<(), String> {
    run_operation(operation, &f.state, &mut || Ok(()), &mut |_| {})
}
fn trashed(f: &Fixture, index: usize) -> bool {
    cache::library(&f.state)
        .unwrap()
        .query_row(
            "SELECT trashed FROM images WHERE id=?1",
            [id(f, index)],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
        != 0
}

#[test]
fn migration_repairs_duplicate_payloads_and_undo_still_works() {
    let mut f = Fixture::new(3);
    let new_assets = f.state.data_dir.join("clipboard");
    fs::rename(f.root.join("assets"), &new_assets).unwrap();
    let db = Connection::open(&f.state.database_path).unwrap();
    db.execute(
        "UPDATE folders SET path=?1",
        [new_assets.to_string_lossy().as_ref()],
    )
    .unwrap();
    for (index, path) in f.paths.iter_mut().enumerate() {
        let next = new_assets.join(path.file_name().unwrap());
        db.execute(
            "UPDATE images SET id=?1,path=?1 WHERE image_pk=?2",
            params![next.to_string_lossy().as_ref(), index as i64 + 1],
        )
        .unwrap();
        *path = next;
    }
    drop(db);
    f.run().unwrap();

    let g = group(&f);
    let preview = prepare(
        request(&f, &g, "exactOthers", vec![id(&f, 0)], vec![]),
        &f.state,
    )
    .unwrap();
    execute(&f, &preview.operation_id).unwrap();
    assert!(trashed(&f, 1));
    assert!(trashed(&f, 2));

    let destination = f.root.join("migrated");
    fs::create_dir(&destination).unwrap();
    let (_, _, reports) =
        crate::library::prepare_data_directory_migration(&destination, &f.state).unwrap();
    assert!(
        reports.iter().all(|report| report.status == "migrated"),
        "{reports:?}"
    );
    assert!(destination.join("clipboard/image-000.png").is_file());

    let mut new_state = crate::library::AppState::new(
        destination.join("illutag.sqlite"),
        destination.clone(),
        None,
        None,
    );
    new_state.read_only = false;
    let undo = prepare_undo(&preview.operation_id, &new_state).unwrap();
    run_operation(&undo, &new_state, &mut || Ok(()), &mut |_| {}).unwrap();
    let new_db = cache::library(&new_state).unwrap();
    let remaining: i64 = new_db
        .query_row("SELECT COUNT(*) FROM images WHERE trashed<>0", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(remaining, 0, "undo must work after migration");
    assert_eq!(
        operation_status(&undo, &new_state).unwrap().succeeded,
        2
    );
}

#[test]
fn migration_repairs_partial_plan_keepers_and_continue_executes() {
    let mut f = Fixture::new(3);
    let new_assets = f.state.data_dir.join("clipboard");
    fs::rename(f.root.join("assets"), &new_assets).unwrap();
    let db = Connection::open(&f.state.database_path).unwrap();
    db.execute(
        "UPDATE folders SET path=?1",
        [new_assets.to_string_lossy().as_ref()],
    )
    .unwrap();
    for (index, path) in f.paths.iter_mut().enumerate() {
        let next = new_assets.join(path.file_name().unwrap());
        db.execute(
            "UPDATE images SET id=?1,path=?1 WHERE image_pk=?2",
            params![next.to_string_lossy().as_ref(), index as i64 + 1],
        )
        .unwrap();
        *path = next;
    }
    drop(db);
    f.run().unwrap();

    let g = group(&f);
    let preview = prepare(
        request(&f, &g, "exactOthers", vec![id(&f, 0)], vec![]),
        &f.state,
    )
    .unwrap();
    let operation = op_id(&preview.operation_id).unwrap();
    let log = journal(&f.state, false).unwrap();
    let first_target: i64 = log
        .query_row(
            "SELECT image_pk FROM items WHERE operation=?1 AND state='pending' ORDER BY ordinal LIMIT 1",
            [operation],
            |row| row.get(0),
        )
        .unwrap();
    log.execute(
        "UPDATE items SET state='succeeded' WHERE operation=?1 AND image_pk=?2",
        params![operation, first_target],
    )
    .unwrap();
    log.execute(
        "UPDATE operations SET state='partial' WHERE id=?1",
        [operation],
    )
    .unwrap();
    drop(log);
    let db = Connection::open(&f.state.database_path).unwrap();
    db.execute(
        "UPDATE images SET trashed=1 WHERE image_pk=?1",
        [first_target],
    )
    .unwrap();
    drop(db);

    let destination = f.root.join("migrated-partial");
    fs::create_dir(&destination).unwrap();
    let (_, _, reports) =
        crate::library::prepare_data_directory_migration(&destination, &f.state).unwrap();
    let report = reports
        .iter()
        .find(|report| report.journal == "duplicate-management.sqlite")
        .expect("duplicate journal report");
    assert_eq!(report.status, "migrated", "{report:?}");
    assert!(report.unresolved.is_empty(), "{report:?}");

    let mut new_state = crate::library::AppState::new(
        destination.join("illutag.sqlite"),
        destination.clone(),
        None,
        None,
    );
    new_state.read_only = false;
    run_operation(
        &preview.operation_id,
        &new_state,
        &mut || Ok(()),
        &mut |_| {},
    )
    .unwrap();
    let new_db = cache::library(&new_state).unwrap();
    let trashed: i64 = new_db
        .query_row("SELECT COUNT(*) FROM images WHERE trashed<>0", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(trashed, 2, "continue must finish the remaining target");

    let journal = Connection::open(destination.join("operations/duplicate-management.sqlite"))
        .unwrap();
    let raw: String = journal
        .query_row(
            "SELECT request FROM operations WHERE id=?1",
            [operation],
            |row| row.get(0),
        )
        .unwrap();
    let plan: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let keeper = &plan["keepers"][0];
    let keeper_path = keeper["image"]["path"].as_str().unwrap();
    assert!(
        keeper_path.starts_with(destination.to_string_lossy().as_ref()),
        "keeper must point at the migrated file: {keeper_path}"
    );
    let canonical = keeper["canonical"].as_str().unwrap();
    let canonical = canonical.strip_prefix(r"\\?\").unwrap_or(canonical);
    assert!(
        canonical
            .to_lowercase()
            .starts_with(&destination.to_string_lossy().to_lowercase()),
        "canonical={canonical} destination={}",
        destination.display()
    );
    assert!(Path::new(canonical).is_file(), "{canonical}");
    let new_keeper_id: String = new_db
        .query_row("SELECT id FROM images WHERE image_pk=1", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        plan["request"]["keepIds"][0].as_str(),
        Some(new_keeper_id.as_str())
    );
    assert_eq!(keeper["image"]["id"].as_str(), Some(new_keeper_id.as_str()));
}

#[test]
fn migration_report_marks_unresolved_plan_keeper_as_partial() {
    let mut f = Fixture::new(3);
    let new_assets = f.state.data_dir.join("clipboard");
    fs::rename(f.root.join("assets"), &new_assets).unwrap();
    let db = Connection::open(&f.state.database_path).unwrap();
    db.execute(
        "UPDATE folders SET path=?1",
        [new_assets.to_string_lossy().as_ref()],
    )
    .unwrap();
    for (index, path) in f.paths.iter_mut().enumerate() {
        let next = new_assets.join(path.file_name().unwrap());
        db.execute(
            "UPDATE images SET id=?1,path=?1 WHERE image_pk=?2",
            params![next.to_string_lossy().as_ref(), index as i64 + 1],
        )
        .unwrap();
        *path = next;
    }
    drop(db);
    f.run().unwrap();

    let g = group(&f);
    let preview = prepare(
        request(&f, &g, "exactOthers", vec![id(&f, 0)], vec![]),
        &f.state,
    )
    .unwrap();
    let operation = op_id(&preview.operation_id).unwrap();
    let db = Connection::open(&f.state.database_path).unwrap();
    db.execute("DELETE FROM images WHERE image_pk=1", [])
        .unwrap();
    drop(db);

    let destination = f.root.join("migrated-missing-keeper");
    fs::create_dir(&destination).unwrap();
    let (_, _, reports) =
        crate::library::prepare_data_directory_migration(&destination, &f.state).unwrap();
    let report = reports
        .iter()
        .find(|report| report.journal == "duplicate-management.sqlite")
        .expect("duplicate journal report");
    assert_eq!(report.status, "partial", "{report:?}");
    assert!(
        report.unresolved.iter().any(|issue| issue.kind == "operationKeeper"),
        "{report:?}"
    );
    assert!(
        report.message.contains("无法完整迁移"),
        "the report must not claim a complete migration: {}",
        report.message
    );
    let journal = Connection::open(destination.join("operations/duplicate-management.sqlite"))
        .unwrap();
    let errors: String = journal
        .query_row(
            "SELECT errors FROM operations WHERE id=?1",
            [operation],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        errors.contains("图库记录已移除") || errors.contains("保留图"),
        "{errors}"
    );
}

#[test]
fn physical_alias_preflight_releases_library_readers_before_pause_acknowledgement() {
    let f = Fixture::new(1005);
    f.run().unwrap();
    let db = Connection::open(&f.state.database_path).unwrap();
    db.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA busy_timeout=40;")
        .unwrap();
    let cache = cache::open_existing(&f.state, false).unwrap();
    let target = item(&cache, &db, 1, None).unwrap();
    db.execute("UPDATE images SET trashed=1 WHERE image_pk=1", [])
        .unwrap();
    let mut calls = 0;
    validate_physical_aliases(&[&target],&f.state,&mut ||{
        db.execute("INSERT OR REPLACE INTO task_runs(id,kind,state,payload,updated_at) VALUES('alias-pause','duplicateManagement','paused','{}',1)",[]).map_err(|e|format!("pause writer blocked: {e}"))?;
        calls+=1;
        Ok(())
    }).unwrap();
    assert!(calls >= 3);
    assert!(
        f.paths.iter().all(|p| p.exists()),
        "preflight must not move any file"
    );
}

#[test]
fn management_auto_exact_batch_keeps_one_per_exact_group_and_excludes_near() {
    let f = Fixture::new(13);
    let second = image::RgbImage::from_fn(64, 48, |x, y| {
        image::Rgb([
            ((x * 3 + y * 9) % 256) as u8,
            ((x * 17 + y * 2) % 256) as u8,
            ((x * 11 + y * 5) % 256) as u8,
        ])
    });
    second.save(&f.paths[8]).unwrap();
    for n in 9..13 {
        fs::copy(&f.paths[8], &f.paths[n]).unwrap();
    }
    let db = Connection::open(&f.state.database_path).unwrap();
    db.execute("UPDATE images SET is_favorite=1 WHERE id=?1", [id(&f, 12)])
        .unwrap();
    // Existing metadata with higher pixel area takes precedence over favorites.
    db.execute(
        "UPDATE images SET width=128,height=96 WHERE id=?1",
        [id(&f, 7)],
    )
    .unwrap();
    drop(db);
    f.run().unwrap();
    let g = group(&f);
    let generation = cache::status(&f.state).unwrap().generation.unwrap();
    let c = cache::open_existing(&f.state, true).unwrap();
    // An overlapping near group must never be part of the automatic action.
    c.execute("INSERT INTO groups(generation,group_no,kind,reference_pk,total_members) VALUES(?1,999,1,1,8)",[&generation]).unwrap();
    c.execute("INSERT INTO members SELECT generation,999,member_no,image_pk,public_hash,signature,phash_distance,dhash_distance,color_distance FROM members WHERE generation=?1 AND group_no=?2",params![generation,g.id]).unwrap();
    drop(c);
    let mut req = request(&f, &g, "autoExact", vec![], vec![]);
    req.group_id = "all-exact".into();
    let preview = prepare(req, &f.state).unwrap();
    assert_eq!(preview.target_count, 11);
    assert!(preview
        .warnings
        .iter()
        .any(|w| w.contains("2 个完全相同组")));
    let log = journal(&f.state, false).unwrap();
    let (plan, _, _) = load_plan(&log, op_id(&preview.operation_id).unwrap()).unwrap();
    assert_eq!(plan.keepers.len(), 2);
    assert!(plan.keepers.iter().any(|k| k.image.id == id(&f, 7)));
    assert!(plan.keepers.iter().any(|k| k.image.id == id(&f, 12)));
    drop(log);
    let fp_before = Connection::open(f.state.data_dir.join("cache/duplicates.sqlite"))
        .unwrap()
        .query_row(
            "SELECT GROUP_CONCAT(hex(sha)||':'||hex(signature)) FROM fingerprints",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap();
    execute(&f, &preview.operation_id).unwrap();
    assert!(!trashed(&f, 7));
    assert!(!trashed(&f, 12));
    assert_eq!(
        cache::library(&f.state)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM images WHERE trashed=0", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        operation_status(&preview.operation_id, &f.state)
            .unwrap()
            .succeeded,
        11
    );
    let fp_after = Connection::open(f.state.data_dir.join("cache/duplicates.sqlite"))
        .unwrap()
        .query_row(
            "SELECT GROUP_CONCAT(hex(sha)||':'||hex(signature)) FROM fingerprints",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap();
    assert_eq!(fp_before, fp_after);
}

#[test]
fn management_soft_undo_preserves_cross_group_protection_and_later_ownership() {
    let f = Fixture::new(13);
    f.run().unwrap();
    let g = group(&f);
    let mut req = request(&f, &g, "exactOthers", vec![id(&f, 12)], vec![]);
    req.protected_ids = vec![id(&f, 0), "unrelated-offline-keeper".into()];
    let preview = prepare(req, &f.state).unwrap();
    assert_eq!(preview.target_count, 11);
    execute(&f, &preview.operation_id).unwrap();
    let live = cache::query_group(
        DuplicateGroupQuery {
            generation: preview.generation.clone(),
            group_id: g.id.clone(),
        },
        &f.state,
    )
    .unwrap()
    .unwrap();
    assert_eq!(live.total_members, 2);
    let page = cache::query_members(
        DuplicateMemberQuery {
            generation: preview.generation.clone(),
            group_id: g.id,
            cursor: None,
            limit: 1,
        },
        &f.state,
    )
    .unwrap();
    assert_eq!(page.total_members, 2);
    assert_eq!(page.members.len(), 1);
    assert!(page.next_cursor.is_some());
    let next = cache::query_members(
        DuplicateMemberQuery {
            generation: preview.generation.clone(),
            group_id: live.id,
            cursor: page.next_cursor,
            limit: 50,
        },
        &f.state,
    )
    .unwrap();
    assert_eq!(next.members.len(), 1);
    for action in ["restore", "trash"] {
        let changed = crate::library::mutate_images(
            crate::library::ImageMutation {
                action: action.into(),
                image_ids: vec![id(&f, 1)],
                favorite: None,
                folder_id: None,
                custom_tags: vec![],
                supplement_tags: vec![],
            },
            &f.state,
        )
        .unwrap();
        assert_eq!(changed.successful_ids.len(), 1);
    }
    cache::clear_cache(&f.state).unwrap();
    assert!(journal_path(&f.state).exists());
    let undo = prepare_undo(&preview.operation_id, &f.state).unwrap();
    execute(&f, &undo).unwrap();
    assert!(trashed(&f, 1));
    assert!(!trashed(&f, 2));
    assert_eq!(operation_status(&undo, &f.state).unwrap().succeeded, 10);
    assert_eq!(
        operation_status(&undo, &f.state).unwrap().state,
        "completed"
    );
}

#[test]
fn management_near_requires_explicit_selection_and_system_inherits_source_keepers() {
    let f = Fixture::new(3);
    f.run().unwrap();
    let g = group(&f);
    let c = cache::open_existing(&f.state, true).unwrap();
    c.execute(
        "UPDATE groups SET kind=1 WHERE group_no=?1",
        [g.id.parse::<i64>().unwrap()],
    )
    .unwrap();
    drop(c);
    assert!(prepare(
        request(&f, &g, "exactOthers", vec![id(&f, 2)], vec![]),
        &f.state
    )
    .unwrap_err()
    .contains("近似组"));
    let preview = prepare(
        request(&f, &g, "selected", vec![id(&f, 2)], vec![id(&f, 0)]),
        &f.state,
    )
    .unwrap();
    assert_eq!(preview.target_count, 1);
    execute(&f, &preview.operation_id).unwrap();
    assert!(!trashed(&f, 1));
    let mut req = request(&f, &g, "selected", vec![], vec![id(&f, 0)]);
    cache::clear_cache(&f.state).unwrap();
    req.action = "systemTrash".into();
    req.source_operation_id = Some(preview.operation_id.clone());
    // Preparation does not move any source file, and remains valid without cache.
    let physical = prepare(req, &f.state).unwrap();
    assert_eq!(physical.target_count, 1);
    assert!(physical.requires_system_confirmation);
    assert!(f.paths[0].exists());
    let (plan, _, _) = load_plan(
        &journal(&f.state, false).unwrap(),
        op_id(&physical.operation_id).unwrap(),
    )
    .unwrap();
    assert_eq!(plan.keepers[0].image.id, id(&f, 2));
}

#[test]
fn management_control_never_holds_write_lock_and_locked_keeper_guard_prevents_change() {
    let f = Fixture::new(3);
    f.run().unwrap();
    let g = group(&f);
    let preview = prepare(
        request(&f, &g, "exactOthers", vec![id(&f, 2)], vec![]),
        &f.state,
    )
    .unwrap();
    let calls = Cell::new(0);
    let result = run_operation(
        &preview.operation_id,
        &f.state,
        &mut || {
            assert!(
                f.state.write_lock.try_lock().is_ok(),
                "pause/control must not run under database write lock"
            );
            let n = calls.get() + 1;
            calls.set(n);
            if n == 5 {
                let _guard = f.state.write_lock.lock().unwrap();
                Connection::open(&f.state.database_path)
                    .unwrap()
                    .execute("UPDATE images SET trashed=1 WHERE id=?1", [id(&f, 2)])
                    .unwrap();
            }
            Ok(())
        },
        &mut |_| {},
    );
    assert!(result.is_err());
    assert!(!trashed(&f, 0));
    assert!(!trashed(&f, 1));
    assert_eq!(
        operation_status(&preview.operation_id, &f.state)
            .unwrap()
            .succeeded,
        0
    );
}

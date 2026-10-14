use super::*;
use crate::library::AppState;
use image::{Rgb, RgbImage};
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::{
    cell::Cell,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
pub(super) struct Fixture {
    pub(super) root: PathBuf,
    pub(super) state: AppState,
    pub(super) paths: Vec<PathBuf>,
}
impl Fixture {
    pub(super) fn new(count: usize) -> Self {
        let base = std::env::var_os("ILLUTAG_TEST_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let root = base.join(format!(
            "illutag-duplicates-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::SeqCst)
        ));
        let data = root.join("data");
        let assets = root.join("assets");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&assets).unwrap();
        let db = data.join("illutag.sqlite");
        let mut conn = Connection::open(&db).unwrap();
        conn.execute_batch(&format!(
            "{}\n{}\nPRAGMA user_version=3;",
            crate::library::compact::SCHEMA,
            crate::library::compact::INDEXES
        ))
        .unwrap();
        conn.execute(
            "INSERT INTO folders(id,path,added_at) VALUES(1,?1,0)",
            [assets.to_string_lossy().as_ref()],
        )
        .unwrap();
        let mut paths = Vec::new();
        let image = RgbImage::from_fn(64, 48, |x, y| {
            Rgb([
                ((x * 7 + y * 11) % 256) as u8,
                ((x * 2 + y * 5) % 256) as u8,
                ((x * 13 + y * 3) % 256) as u8,
            ])
        });
        let master = assets.join("image-000.png");
        image.save(&master).unwrap();
        let tx = conn.transaction().unwrap();
        for index in 0..count {
            let path = assets.join(format!("image-{index:03}.png"));
            if index > 0 {
                fs::copy(&master, &path).unwrap();
            }
            tx.execute("INSERT INTO images(image_pk,id,path,file_name,ext,width,height,file_size,modified_at,imported_at,folder_id) VALUES(?1,?2,?2,?3,'png',64,48,?4,0,0,1)",params![index as i64+1,path.to_string_lossy().as_ref(),format!("image-{index:03}.png"),fs::metadata(&path).unwrap().len() as i64]).unwrap();
            paths.push(path);
        }
        tx.commit().unwrap();
        drop(conn);
        let mut state = AppState::new(db, data, None, None);
        state.read_only = false;
        Self { root, state, paths }
    }
    pub(super) fn run(&self) -> Result<(), String> {
        run_job(options(), &self.state, &mut || Ok(()), &mut |_, _| {})
    }
    fn cache(&self) -> PathBuf {
        self.state.data_dir.join("cache/duplicates.sqlite")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn options() -> DuplicateOptions {
    DuplicateOptions {
        sensitivity: "standard".into(),
        force_recompute: false,
        regroup_only: false,
    }
}
fn digest(path: &std::path::Path) -> Vec<u8> {
    Sha256::digest(fs::read(path).unwrap()).to_vec()
}
fn groups(state: &AppState, generation: &str) -> DuplicateGroupPage {
    query_groups(
        DuplicateQuery {
            generation: generation.into(),
            kind: "all".into(),
            cursor: None,
            limit: 20,
        },
        state,
    )
    .unwrap()
}

#[test]
fn ordinary_status_does_not_create_a_cache() {
    let fixture = Fixture::new(2);
    let before = digest(&fixture.state.database_path);
    let info = status(&fixture.state).unwrap();
    assert_eq!(info.state, "empty");
    assert!(info.generation.is_none());
    assert!(!fixture.cache().exists());
    assert_eq!(digest(&fixture.state.database_path), before);
}

#[test]
fn exact_groups_page_without_writing_library_metadata() {
    let fixture = Fixture::new(9);
    let before = digest(&fixture.state.database_path);
    fixture.run().unwrap();
    let info = status(&fixture.state).unwrap();
    assert_eq!(info.state, "ready");
    assert!(info.duration_ms.is_some_and(|duration| duration >= 0));
    assert_eq!(info.coverage.processed, 9);
    assert_eq!(info.exact_groups, 1);
    assert_eq!(info.unique_images, 9);
    assert_eq!(digest(&fixture.state.database_path), before);
    let generation = info.generation.unwrap();
    let page = groups(&fixture.state, &generation);
    assert_eq!(page.groups.len(), 1);
    let group = &page.groups[0];
    assert_eq!(group.kind, "exact");
    assert_eq!(group.total_members, 9);
    assert_eq!(group.members.len(), 6);
    assert!(!group.reference.stale);
    assert!(group
        .members
        .iter()
        .all(|m| m.metric.phash_distance.is_none()));
    let rest = query_members(
        DuplicateMemberQuery {
            generation: generation.clone(),
            group_id: group.id.clone(),
            cursor: group.next_cursor.clone(),
            limit: 50,
        },
        &fixture.state,
    )
    .unwrap();
    assert_eq!(rest.members.len(), 3);
    assert!(rest.next_cursor.is_none());
    let mut regroup = options();
    regroup.sensitivity = "strict".into();
    regroup.regroup_only = true;
    run_job(regroup, &fixture.state, &mut || Ok(()), &mut |_, _| {}).unwrap();
    let next = status(&fixture.state).unwrap();
    assert_eq!(next.sensitivity, "strict");
    assert_ne!(next.generation.as_deref(), Some(generation.as_str()));
    assert!(query_groups(
        DuplicateQuery {
            generation,
            kind: "all".into(),
            cursor: None,
            limit: 20
        },
        &fixture.state
    )
    .unwrap_err()
    .contains("更新"));
}

#[test]
fn partial_attempt_keeps_the_last_complete_generation() {
    let fixture = Fixture::new(3);
    fixture.run().unwrap();
    let generation = status(&fixture.state).unwrap().generation.unwrap();
    fs::rename(
        &fixture.paths[1],
        fixture.root.join("temporarily-offline.png"),
    )
    .unwrap();
    assert!(fixture.run().unwrap_err().contains("部分"));
    let info = status(&fixture.state).unwrap();
    assert_eq!(info.generation.as_deref(), Some(generation.as_str()));
    assert!(info.needs_update);
    assert_eq!(info.last_attempt.unwrap().coverage.failed, 1);
    let page = groups(&fixture.state, &generation);
    assert!(page.groups[0].members.iter().any(|m| m.stale));
    fs::rename(
        fixture.root.join("temporarily-offline.png"),
        &fixture.paths[1],
    )
    .unwrap();
    fixture.run().unwrap();
    assert_eq!(status(&fixture.state).unwrap().state, "ready");
}

#[test]
fn first_partial_results_are_labelled_not_claimed_complete() {
    let fixture = Fixture::new(4);
    fs::remove_file(&fixture.paths[3]).unwrap();
    assert!(fixture.run().is_err());
    let info = status(&fixture.state).unwrap();
    assert_eq!(info.state, "partial");
    assert_eq!(info.coverage.failed, 1);
    assert_eq!(info.coverage.processed, 4);
    assert_eq!(info.exact_groups, 1);
    assert!(info.needs_update);
    let page = groups(&fixture.state, info.generation.as_deref().unwrap());
    assert_eq!(page.groups[0].total_members, 3);
}

#[test]
fn cancellation_retains_committed_fingerprints_for_manual_rerun() {
    let fixture = Fixture::new(8);
    let cancelled = Cell::new(false);
    let error = run_job(
        options(),
        &fixture.state,
        &mut || {
            if cancelled.get() {
                Err("任务已取消".into())
            } else {
                Ok(())
            }
        },
        &mut |stage, cov| {
            if stage == "计算图片指纹" && cov.processed >= 1 {
                cancelled.set(true)
            }
        },
    )
    .unwrap_err();
    assert!(error.contains("取消"));
    let conn = Connection::open(fixture.cache()).unwrap();
    let saved: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM fingerprints WHERE sha IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(saved > 0 && saved < 8);
    drop(conn);
    assert_eq!(
        status(&fixture.state).unwrap().last_attempt.unwrap().state,
        "cancelled"
    );
    fixture.run().unwrap();
    assert_eq!(status(&fixture.state).unwrap().coverage.processed, 8);
}

#[test]
fn regroup_only_does_not_read_changed_image_payloads() {
    let fixture = Fixture::new(3);
    fixture.run().unwrap();
    let generation = status(&fixture.state).unwrap().generation;
    fs::write(
        &fixture.paths[1],
        b"changed payload which must not be fingerprinted by regroup",
    )
    .unwrap();
    let mut regroup = options();
    regroup.regroup_only = true;
    assert!(run_job(regroup, &fixture.state, &mut || Ok(()), &mut |_, _| {}).is_err());
    let conn = Connection::open(fixture.cache()).unwrap();
    let sha: Option<Vec<u8>> = conn
        .query_row("SELECT sha FROM fingerprints WHERE image_pk=2", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(sha.is_none());
    drop(conn);
    assert_eq!(status(&fixture.state).unwrap().generation, generation);
}

#[test]
fn reused_integer_key_cannot_display_a_different_public_image() {
    let fixture = Fixture::new(3);
    fixture.run().unwrap();
    let generation = status(&fixture.state).unwrap().generation.unwrap();
    let conn = Connection::open(&fixture.state.database_path).unwrap();
    conn.execute(
        "UPDATE images SET id='different-public-id' WHERE image_pk=2",
        [],
    )
    .unwrap();
    drop(conn);
    let page = groups(&fixture.state, &generation);
    assert!(page.groups[0]
        .members
        .iter()
        .all(|m| m.image.id != "different-public-id"));
    assert_eq!(page.groups[0].members.len(), 2);
    assert!(status(&fixture.state).unwrap().needs_update);
}

#[test]
fn cache_corruption_and_unknown_versions_are_explicit_and_disposable() {
    let fixture = Fixture::new(2);
    fixture.run().unwrap();
    let before = digest(&fixture.state.database_path);
    let conn = Connection::open(fixture.cache()).unwrap();
    conn.pragma_update(None, "user_version", 99).unwrap();
    drop(conn);
    assert_eq!(status(&fixture.state).unwrap().state, "invalid");
    assert!(fixture.run().unwrap_err().contains("版本"));
    clear_cache(&fixture.state).unwrap();
    assert!(!fixture.cache().exists());
    fixture.run().unwrap();
    fs::write(fixture.cache(), b"corrupt disposable cache").unwrap();
    assert_eq!(status(&fixture.state).unwrap().state, "invalid");
    clear_cache(&fixture.state).unwrap();
    assert_eq!(digest(&fixture.state.database_path), before);
    assert_eq!(status(&fixture.state).unwrap().state, "empty");
}

#[test]
fn replaced_database_invalidates_cache_even_with_identical_records() {
    let fixture = Fixture::new(2);
    fixture.run().unwrap();
    let next = fixture.root.join("replacement.sqlite");
    fs::copy(&fixture.state.database_path, &next).unwrap();
    fs::rename(
        &fixture.state.database_path,
        fixture.root.join("old.sqlite"),
    )
    .unwrap();
    fs::rename(next, &fixture.state.database_path).unwrap();
    let info = status(&fixture.state).unwrap();
    assert_eq!(info.state, "invalid");
    assert!(info.errors[0].contains("替换"));
    assert!(fixture.run().is_err());
    clear_cache(&fixture.state).unwrap();
    fixture.run().unwrap();
    assert_eq!(status(&fixture.state).unwrap().state, "ready");
}

#[test]
fn complete_backup_and_directory_migration_exclude_duplicate_cache() {
    let fixture = Fixture::new(2);
    fixture.run().unwrap();
    let destination = fixture.root.join("packages");
    fs::create_dir(&destination).unwrap();
    let package = crate::library::create_restore_package(
        destination.to_string_lossy().into(),
        serde_json::json!({}),
        &fixture.state,
    )
    .unwrap();
    assert!(!PathBuf::from(&package).join("cache").exists());
    let manifest = crate::library::inspect_restore_package(package).unwrap();
    assert!(!manifest
        .files
        .iter()
        .any(|f| f.path.contains("duplicates.sqlite")));
    assert_eq!(manifest.schema_version, 3);
    let migrated = fixture.root.join("migrated");
    crate::library::prepare_data_directory_migration(&migrated, &fixture.state).unwrap();
    assert!(!migrated.join("cache").exists());
    assert!(migrated.join("illutag.sqlite").is_file());
    let mut new_state = AppState::new(migrated.join("illutag.sqlite"), migrated, None, None);
    new_state.read_only = false;
    assert_eq!(status(&new_state).unwrap().state, "empty");
}

#[test]
fn cache_cleanup_preserves_all_other_cache_and_library_files() {
    let fixture = Fixture::new(2);
    fixture.run().unwrap();
    let before = digest(&fixture.state.database_path);
    let other = fixture.state.data_dir.join("cache/unrelated.keep");
    fs::write(&other, b"keep").unwrap();
    clear_cache(&fixture.state).unwrap();
    assert!(other.exists());
    assert_eq!(digest(&fixture.state.database_path), before);
    assert!(!fixture.cache().exists());
    assert_eq!(status(&fixture.state).unwrap().state, "empty");
}

#[test]
fn cache_cleanup_is_rejected_while_duplicate_task_is_running() {
    let fixture = Fixture::new(2);
    let checked = Cell::new(false);
    run_job(options(), &fixture.state, &mut || Ok(()), &mut |_, _| {
        if !checked.replace(true) {
            assert!(clear_cache(&fixture.state).unwrap_err().contains("运行"));
        }
    })
    .unwrap();
    assert!(checked.get());
    assert_eq!(status(&fixture.state).unwrap().state, "ready");
    clear_cache(&fixture.state).unwrap();
}

#[test]
fn large_exact_group_commits_bounded_chunks_and_cancels_between_them() {
    let fixture = Fixture::new(2100);
    // Initialize a disposable cache, then seed valid exact-file fingerprints.
    // Near-image algorithm behavior is covered separately; no 2,100 decodes are
    // needed to exercise transaction boundaries and generation publication.
    assert!(run_job(
        options(),
        &fixture.state,
        &mut || Err("任务已取消".into()),
        &mut |_, _| {}
    )
    .is_err());
    let mut conn = Connection::open(fixture.cache()).unwrap();
    let sha = Sha256::digest(fs::read(&fixture.paths[0]).unwrap()).to_vec();
    let tx = conn.transaction().unwrap();
    {
        let mut insert=tx.prepare("INSERT INTO fingerprints(image_pk,public_hash,signature,size,mtime,identity,sha,phash,dhash,color,width,height,low_info,skip,image_order,seen_run) VALUES(?1,?2,?3,?4,?5,?6,?7,NULL,NULL,NULL,64,48,0,'animated',?8,0)").unwrap();
        for (index, path) in fixture.paths.iter().enumerate() {
            let id = path.to_string_lossy();
            let source = fingerprint::source_signature(path).unwrap();
            let public = Sha256::digest(id.as_bytes()).to_vec();
            let identity = Sha256::digest(source.identity.as_bytes()).to_vec();
            let mut signature = Sha256::new();
            signature.update(&public);
            signature.update(&identity);
            signature.update(source.file_size.to_le_bytes());
            signature.update(source.modified_ns.to_le_bytes());
            insert
                .execute(params![
                    index as i64 + 1,
                    public,
                    signature.finalize().to_vec(),
                    source.file_size as i64,
                    source.modified_ns,
                    identity,
                    sha,
                    index as u32
                ])
                .unwrap();
        }
    }
    tx.commit().unwrap();
    drop(conn);
    let mut regroup = options();
    regroup.regroup_only = true;
    let error = run_job(
        regroup.clone(),
        &fixture.state,
        &mut || {
            let conn = Connection::open_with_flags(
                fixture.cache(),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            let committed: i64 = conn
                .query_row("SELECT COUNT(*) FROM members", [], |r| r.get(0))
                .unwrap();
            if committed >= 1000 {
                Err("任务已取消".into())
            } else {
                Ok(())
            }
        },
        &mut |_, _| {},
    )
    .unwrap_err();
    assert!(error.contains("取消"));
    let info = status(&fixture.state).unwrap();
    assert_eq!(info.state, "partial");
    assert_eq!(info.exact_groups, 1);
    let page = groups(&fixture.state, info.generation.as_deref().unwrap());
    assert_eq!(page.groups[0].total_members, 1000);
    assert_eq!(page.groups[0].members.len(), 6);
    assert!(info.cache_bytes < 8 * 1024 * 1024);
    run_job(regroup, &fixture.state, &mut || Ok(()), &mut |_, _| {}).unwrap();
    let done = status(&fixture.state).unwrap();
    assert_eq!(done.state, "ready");
    assert_eq!(
        groups(&fixture.state, done.generation.as_deref().unwrap()).groups[0].total_members,
        2100
    );
}

#[test]
fn cleanup_cancellation_keeps_published_generation_and_resumes_by_generation() {
    let fixture = Fixture::new(2);
    fixture.run().unwrap();
    let published = status(&fixture.state).unwrap().generation.unwrap();
    let mut conn = Connection::open(fixture.cache()).unwrap();
    let tx = conn.transaction().unwrap();
    tx.execute("INSERT INTO generations(generation,sensitivity,checked_at,state,coverage,errors) VALUES(-17,'standard',0,'building','{}','[]')",[]).unwrap();
    tx.execute("INSERT INTO groups(generation,group_no,kind,reference_pk,total_members) VALUES(-17,1,0,1,6000)",[]).unwrap();
    {
        let mut insert=tx.prepare("INSERT INTO members(generation,group_no,member_no,image_pk,public_hash,signature) VALUES(-17,1,?1,1,zeroblob(32),zeroblob(32))").unwrap();
        for member in 0..6000 {
            insert.execute([member]).unwrap();
        }
    }
    tx.commit().unwrap();
    drop(conn);
    let error = run_job(
        options(),
        &fixture.state,
        &mut || {
            let conn = Connection::open_with_flags(
                fixture.cache(),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            let remaining: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM members WHERE generation=-17",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            if remaining > 0 && remaining <= 1000 {
                Err("任务已取消".into())
            } else {
                Ok(())
            }
        },
        &mut |_, _| {},
    )
    .unwrap_err();
    assert!(error.contains("取消"));
    assert_eq!(
        status(&fixture.state).unwrap().generation.as_deref(),
        Some(published.as_str())
    );
    assert_eq!(
        groups(&fixture.state, &published).groups[0].total_members,
        2
    );
    fixture.run().unwrap();
    let conn = Connection::open(fixture.cache()).unwrap();
    let remaining: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM generations WHERE generation=-17",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(remaining, 0);
    assert_eq!(status(&fixture.state).unwrap().state, "ready");
}

#[test]
fn disk_cache_and_memory_limits_stop_before_budgets_are_exceeded() {
    assert!(cache::check_budget_for_test(1, 3 * 1024 * 1024 * 1024, 128 * 1024 * 1024).is_ok());
    assert!(
        cache::check_budget_for_test(512 * 1024 * 1024, 3 * 1024 * 1024 * 1024, 1)
            .unwrap_err()
            .contains("512")
    );
    assert!(cache::check_budget_for_test(1, 1024 * 1024 * 1024, 1)
        .unwrap_err()
        .contains("2 GiB"));
    assert!(
        cache::check_budget_for_test(1, 3 * 1024 * 1024 * 1024, 513 * 1024 * 1024)
            .unwrap_err()
            .contains("内存")
    );
}

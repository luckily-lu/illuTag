//! Full loading path at realistic cardinalities, using metadata rather than files.
use super::super::tests::Fixture;
use super::*;
use std::{fs::File, io::Read, time::Duration};

fn database_digest(path: &Path) -> Vec<u8> {
    let mut file = File::open(path).unwrap();
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).unwrap();
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    digest.finalize().to_vec()
}

fn metadata_fixture(images: i64, members: i64) -> (Fixture, i64) {
    assert!(images >= members && members >= 8 && members % 4 == 0);
    let fixture = Fixture::new(2);
    fixture.run().unwrap();
    let mut cache = cache::open_existing(&fixture.state, true).unwrap();
    let generation: i64 = cache
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM meta WHERE key='published'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let mut library = Connection::open(&fixture.state.database_path).unwrap();
    let main = library.transaction().unwrap();
    let tx = cache.transaction().unwrap();
    tx.execute_batch("DELETE FROM members; DELETE FROM groups;")
        .unwrap();
    {
        let mut image = main.prepare("INSERT INTO images(image_pk,id,path,file_name,ext,width,height,file_size,modified_at,imported_at,folder_id) VALUES(?1,?2,?2,?3,'png',64,48,4096,0,0,1)").unwrap();
        let mut fingerprint = tx.prepare("INSERT OR REPLACE INTO fingerprints SELECT ?1,?2,?3,size,mtime,identity,?4,phash,dhash,color,width,height,low_info,skip,?1,seen_run FROM fingerprints WHERE image_pk=1").unwrap();
        let mut group = tx
            .prepare("INSERT INTO groups VALUES(?1,?2,?3,?4,2)")
            .unwrap();
        let mut member = tx.prepare("INSERT INTO members SELECT ?1,?2,?3,image_pk,public_hash,signature,NULL,NULL,NULL FROM fingerprints WHERE image_pk=?4").unwrap();
        for pk in 1..=images {
            let id = if pk <= 2 {
                fixture.paths[(pk - 1) as usize]
                    .to_string_lossy()
                    .into_owned()
            } else {
                let name = format!("metadata-{pk:07}.png");
                let id = fixture
                    .root
                    .join("assets")
                    .join(&name)
                    .to_string_lossy()
                    .into_owned();
                image.execute(params![pk, id, name]).unwrap();
                id
            };
            let number = (pk + 1) / 2;
            let kind = if number % 2 == 1 { 0_i64 } else { 1_i64 };
            // Exact pairs share bytes; candidate pairs have distinct content units.
            let content = if kind == 0 {
                format!("exact-{number}")
            } else {
                format!("near-{pk}")
            };
            let public = Sha256::digest(id.as_bytes());
            let signature = Sha256::digest(format!("source-{pk}").as_bytes());
            let sha = Sha256::digest(content.as_bytes());
            fingerprint
                .execute(params![
                    pk,
                    public.as_slice(),
                    signature.as_slice(),
                    sha.as_slice()
                ])
                .unwrap();
            if pk <= members {
                if pk % 2 == 1 {
                    group
                        .execute(params![generation, number, kind, pk])
                        .unwrap();
                }
                member
                    .execute(params![generation, number, (pk - 1) % 2, pk])
                    .unwrap();
            }
        }
    }
    let coverage = DuplicateCoverage {
        total: images,
        processed: images,
        ..Default::default()
    };
    tx.execute("UPDATE generations SET coverage=?2,exact_groups=?3,near_groups=?3,unique_images=?4 WHERE generation=?1",
        params![generation, serde_json::to_string(&coverage).unwrap(), members / 4, members]).unwrap();
    tx.commit().unwrap();
    main.commit().unwrap();
    drop(cache);
    drop(library);
    cache::membership_invalidated(&fixture.state);
    // No generated image path exists: status must only read this synthetic metadata.
    assert!(!fixture
        .root
        .join("assets")
        .join(format!("metadata-{images:07}.png"))
        .exists());
    (fixture, generation)
}

fn assert_category(
    status: &DuplicateWorkflowStatus,
    kind: &str,
    groups: i64,
    images: i64,
    cleanup: i64,
) {
    let category = status
        .categories
        .iter()
        .find(|entry| entry.kind == kind)
        .unwrap();
    assert_eq!(
        (category.groups, category.images, category.cleanup_count),
        (groups, images, cleanup)
    );
}

fn exercise_loading(images: i64, members: i64) {
    let seeded = Instant::now();
    let (fixture, generation) = metadata_fixture(images, members);
    let conn = open(&fixture.state).unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO meta VALUES(?1,?2)",
        params![
            format!("policy:{generation}"),
            super::super::evidence::POLICY
        ],
    )
    .unwrap();
    drop(conn);
    eprintln!(
        "workflow metadata fixture: {images} images / {members} members in {:?}",
        seeded.elapsed()
    );
    let before = database_digest(&fixture.state.database_path);
    let conn = open(&fixture.state).unwrap();
    for kind in ["recycle", "retained"] {
        let sql = category_sql(kind, 0).unwrap();
        let action_plan: Vec<String> = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap()
            .query_map([generation], |r| r.get(3))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            action_plan
                .iter()
                .any(|line| line.contains("MATERIALIZE wanted")),
            "action-category qualification must be shared across members: {action_plan:#?}"
        );
        assert!(
            !action_plan
                .iter()
                .any(|line| line.contains("SEARCH m USING PRIMARY KEY (generation=?)")),
            "the complete action query must seek members after driving groups: {action_plan:#?}"
        );
    }
    let explain = format!("EXPLAIN QUERY PLAN {}", planning::nodes_sql());
    let plan: Vec<String> = conn
        .prepare(&explain)
        .unwrap()
        .query_map(params![generation, 0], |r| r.get(3))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let image_seeks = plan
        .iter()
        .filter(|line| {
            line.contains("SEARCH m ")
                && line.contains("members_image")
                && line.contains("image_pk=?")
        })
        .count();
    assert_eq!(
        image_seeks, 2,
        "both correlated member predicates must seek the current image: {plan:#?}"
    );
    drop(conn);

    let started = Instant::now();
    let first = status(&fixture.state).unwrap();
    let first_elapsed = started.elapsed();
    assert_eq!(first.stage, "scanned");
    assert_eq!(first.scan.coverage.processed, images);
    assert_eq!(
        (
            first.scan.exact_groups,
            first.scan.near_groups,
            first.scan.unique_images
        ),
        (members / 4, members / 4, members)
    );
    assert_category(&first, "exact", members / 4, members / 2, members / 4);
    assert_category(&first, "candidate", members / 4, members / 2, 0);
    assert_eq!(first.cleanup_count, members / 4);
    assert_category(&first, "recycle", members / 4, members / 4, members / 4);
    assert_category(&first, "retained", members / 2, images - members / 4, 0);
    let conn = open(&fixture.state).unwrap();
    let run = run(&conn, generation).unwrap();
    let planned: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM workflow_plan WHERE run=?1",
            [run.1],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        planned, members,
        "first status builds the complete plan, not a sample or UI page"
    );
    drop(conn);

    let started = Instant::now();
    let second = status(&fixture.state).unwrap();
    let second_elapsed = started.elapsed();
    assert_eq!(
        first.revision, second.revision,
        "unchanged status reuses the published plan"
    );
    assert_eq!(first.cleanup_count, second.cleanup_count);
    assert_eq!(
        database_digest(&fixture.state.database_path),
        before,
        "loading must not mutate the library"
    );

    // Only this explicit fixture mutation models recycling a member. Refresh must
    // rebuild metadata and counts without fingerprinting or touching source files.
    let db = Connection::open(&fixture.state.database_path).unwrap();
    db.execute("UPDATE images SET trashed=1 WHERE image_pk=2", [])
        .unwrap();
    drop(db);
    let recycled = database_digest(&fixture.state.database_path);
    cache::membership_invalidated(&fixture.state);
    let started = Instant::now();
    let refreshed = status(&fixture.state).unwrap();
    let refresh_elapsed = started.elapsed();
    assert_ne!(second.revision, refreshed.revision);
    assert_category(
        &refreshed,
        "exact",
        members / 4 - 1,
        members / 2 - 2,
        members / 4 - 1,
    );
    assert_category(&refreshed, "candidate", members / 4, members / 2, 0);
    assert_eq!(refreshed.cleanup_count, members / 4 - 1);
    assert_eq!(refreshed.scan.unique_images, members - 2);
    assert_eq!(
        database_digest(&fixture.state.database_path),
        recycled,
        "membership refresh must not mutate the library"
    );
    eprintln!("workflow status {images}/{members}: first={first_elapsed:?}, cached={second_elapsed:?}, membership_refresh={refresh_elapsed:?}");
}

#[test]
fn full_status_loads_twenty_thousand_metadata_nodes_and_refreshes_recycled_member() {
    exercise_loading(20_000, 20_000);
}

#[test]
#[ignore = "explicit large metadata regression; creates no image copies and never accesses production data"]
fn full_status_loads_eight_hundred_thousand_images_and_two_hundred_forty_thousand_members() {
    exercise_loading(800_000, 240_000);
}

#[test]
#[ignore = "explicit 800k index-load regression with real rollback-journal task writes; synthetic metadata only"]
fn paged_index_load_800k_allows_real_task_writes_without_read_lock_starvation() {
    let (f, _) = metadata_fixture(800_000, 240_000);
    let db = Connection::open(&f.state.database_path).unwrap();
    assert_eq!(
        db.query_row("PRAGMA journal_mode=DELETE", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "delete"
    );
    db.busy_timeout(Duration::from_millis(50)).unwrap();
    let started = Instant::now();
    let mut errors = Vec::new();
    let mut writes = 0;
    let mut last = 0;
    let records=cache::evidence_records_with_progress(&f.state,&mut ||Ok(()),&mut |done,total|{
        assert_eq!(total,800_000);
        assert!(done>=last);
        last=done;
        if let Err(e)=db.execute("INSERT OR REPLACE INTO task_runs(id,kind,state,payload,updated_at) VALUES('actual-progress','duplicates','running',?1,?2)",params![format!("{{\"processed\":{done}}}"),done]) { errors.push(e.to_string()); }
        writes+=1;
    }).unwrap();
    assert!(
        errors.is_empty(),
        "main DB progress writes blocked: {errors:?}"
    );
    assert_eq!(records.len(), 800_000);
    assert_eq!(last, 800_000);
    assert!(writes >= 801);
    eprintln!(
        "rollback-journal index load: images={} task_writes={writes} elapsed_ms={}",
        records.len(),
        started.elapsed().as_millis()
    );
}

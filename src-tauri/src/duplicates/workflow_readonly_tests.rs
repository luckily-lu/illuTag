//! Explicit metadata-only diagnostic: persistent databases are opened read-only.
//! All writes made by the real planner are shadowed by TEMP tables in memory.
use super::*;
use rusqlite::OpenFlags;

#[test]
#[ignore = "requires explicit ILLUTAG_READONLY_DATA_ROOT; never launches the app or reads image files"]
fn existing_metadata_full_loading_path_readonly() {
    let data = std::path::PathBuf::from(
        std::env::var_os("ILLUTAG_READONLY_DATA_ROOT").expect("explicit data root required"),
    );
    assert!(data.is_absolute());
    let database = data.join("illutag.sqlite");
    let cache = data.join("cache/duplicates.sqlite");
    let before = [&database, &cache].map(|path| {
        let m = std::fs::metadata(path).unwrap();
        (m.len(), m.modified().unwrap())
    });
    let mut fixture = super::super::tests::Fixture::new(0);
    fixture.state.database_path = database.clone();
    std::fs::create_dir_all(fixture.state.data_dir.join("cache")).unwrap();
    let readonly_state = AppState::new(database.clone(), data, None, None);
    let started = Instant::now();
    let scan = cache::status(&readonly_state).unwrap();
    let scan_time = started.elapsed();
    let gen: i64 = scan
        .generation
        .as_ref()
        .expect("published generation")
        .parse()
        .unwrap();
    let conn = Connection::open_with_flags(&cache, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    conn.execute_batch("PRAGMA temp_store=MEMORY; PRAGMA cache_size=-8192; PRAGMA mmap_size=0;")
        .unwrap();
    attach(&conn, &fixture.state).unwrap();
    // These are the only persistent tables the planner writes. Explicit TEMP
    // shadows preserve the real schema/constraints and prevent production writes.
    for name in ["workflow_runs", "workflow_plan", "workflow_edges"] {
        let ddl: String = conn
            .query_row(
                "SELECT sql FROM main.sqlite_master WHERE type='table' AND name=?1",
                [name],
                |r| r.get(0),
            )
            .unwrap();
        assert!(ddl.starts_with("CREATE TABLE "));
        conn.execute_batch(&ddl.replacen("CREATE TABLE ", "CREATE TEMP TABLE ", 1))
            .unwrap();
    }
    conn.execute_batch("CREATE INDEX temp.readonly_plan_keeper ON workflow_plan(run,keeper);")
        .unwrap();
    conn.execute(
        "INSERT INTO temp.workflow_runs SELECT * FROM main.workflow_runs WHERE generation=?1",
        [gen],
    )
    .unwrap();
    let refinement = run(&conn, gen).unwrap().0;
    conn.execute(
        "INSERT INTO temp.workflow_edges SELECT * FROM main.workflow_edges WHERE run=?1",
        [refinement],
    )
    .unwrap();
    let plan_started = Instant::now();
    planning::build(&conn, gen, refinement, &fixture.state, &mut || Ok(())).unwrap();
    let plan_time = plan_started.elapsed();
    let summary_started = Instant::now();
    let info = summary(&conn, scan, &fixture.state).unwrap();
    let summary_time = summary_started.elapsed();
    let node_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM temp.workflow_plan", [], |r| r.get(0))
        .unwrap();
    assert!(node_count > 0);
    assert_eq!(info.generation.as_deref(), Some(gen.to_string().as_str()));
    let after = [&database, &cache].map(|path| {
        let m = std::fs::metadata(path).unwrap();
        (m.len(), m.modified().unwrap())
    });
    assert_eq!(before, after, "persistent databases must remain untouched");
    println!(
        "{}",
        serde_json::json!({
            "readonlyProductionMetadata":true, "imageFilesRead":0, "nodes":node_count,
            "scanStatusMs":scan_time.as_millis(), "planMs":plan_time.as_millis(),
            "summaryMs":summary_time.as_millis(), "totalMs":started.elapsed().as_millis(),
            "categories":info.categories, "cleanupCount":info.cleanup_count
        })
    );
}

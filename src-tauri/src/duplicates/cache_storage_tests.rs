use super::*;
use crate::duplicates::tests::Fixture;

#[test]
fn sqlite_full_is_explained_and_new_writers_can_continue_without_clearing_data() {
    let f = Fixture::new(0);
    f.run().unwrap();
    let main = fs::read(&f.state.database_path).unwrap();
    let c = open_existing(&f.state, true).unwrap();
    c.execute_batch(
        "CREATE TABLE capacity_probe(x BLOB); INSERT INTO capacity_probe VALUES(X'010203');",
    )
    .unwrap();
    let published = meta(&c, "published").unwrap();
    let pages: u64 = c.query_row("PRAGMA page_count", [], |r| r.get(0)).unwrap();
    c.execute_batch(&format!("PRAGMA max_page_count={};", pages + 4))
        .unwrap();
    let error = controlled_write(&c, &mut || Ok(()), || {
        c.execute("INSERT INTO capacity_probe VALUES(zeroblob(524288))", [])
    })
    .unwrap_err();
    assert!(error.contains("缓存容量上限"), "{error}");
    assert_eq!(meta(&c, "published").unwrap(), published);
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM capacity_probe", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    drop(c);
    // All writers, including the verifier's own connection, get the same policy.
    let verifier = crate::duplicates::verification_cache::Verifier::new(&f.state, false).unwrap();
    let c = open_existing(&f.state, true).unwrap();
    c.execute("INSERT INTO capacity_probe VALUES(zeroblob(524288))", [])
        .unwrap();
    assert_eq!(
        c.query_row(
            "SELECT length(x) FROM capacity_probe ORDER BY rowid LIMIT 1",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        3
    );
    assert_eq!(meta(&c, "published").unwrap(), published);
    assert_eq!(fs::read(&f.state.database_path).unwrap(), main);
    drop(verifier);
}

#[test]
fn retired_pages_are_reusable_without_vacuum_or_cache_reset() {
    let f = Fixture::new(0);
    f.run().unwrap();
    let path = cache_path(&f.state);
    let c = open_existing(&f.state, true).unwrap();
    c.execute_batch("CREATE TABLE retired_probe(x BLOB); WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<96) INSERT INTO retired_probe SELECT zeroblob(32768) FROM n; PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
    let allocated = fs::metadata(&path).unwrap().len();
    runtime(&f.state)
        .disk_allowance
        .store(allocated / 2 + 64 * 1024 * 1024, Ordering::SeqCst);
    assert!(check_disk(&path).unwrap_err().contains("容量预算"));
    c.execute_batch("DELETE FROM retired_probe; PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().len(),
        allocated,
        "no VACUUM/shrink"
    );
    assert!(effective_cache_bytes(&path).unwrap() < allocated / 2);
    check_disk(&path).unwrap();
    let before = fs::metadata(&path).unwrap().len();
    c.execute("INSERT INTO retired_probe VALUES(zeroblob(1048576))", [])
        .unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().len(),
        before,
        "existing pages reused"
    );
}

#[test]
fn writer_budget_sees_freelist_consumption_before_commit_and_can_roll_back() {
    let f = Fixture::new(0);
    f.run().unwrap();
    let path = cache_path(&f.state);
    let c = open_existing(&f.state, true).unwrap();
    c.execute_batch("CREATE TABLE writer_probe(x BLOB); INSERT INTO writer_probe VALUES(zeroblob(4194304)); PRAGMA wal_checkpoint(TRUNCATE); DELETE FROM writer_probe; PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
    runtime(&f.state)
        .disk_allowance
        .store(66 * 1024 * 1024, Ordering::SeqCst); // 2MiB soft headroom in this isolated test.
    check_disk_on(&c, &path).unwrap();
    c.execute_batch("BEGIN IMMEDIATE; INSERT INTO writer_probe VALUES(zeroblob(2621440));")
        .unwrap();
    check_disk(&path).unwrap(); // A separate reader still sees the old freelist.
    assert!(check_disk_on(&c, &path).unwrap_err().contains("容量预算"));
    c.execute_batch("ROLLBACK").unwrap();
    check_disk_on(&c, &path).unwrap();
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM writer_probe", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

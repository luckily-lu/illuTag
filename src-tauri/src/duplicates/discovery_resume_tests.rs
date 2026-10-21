use super::{resume, *};
use crate::duplicates::{fingerprint::NearFingerprint, index};
use std::{
    collections::HashSet,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct JournalDb {
    root: PathBuf,
    path: PathBuf,
}

impl JournalDb {
    fn new() -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let base = std::env::var_os("ILLUTAG_TEST_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let root = base.join(format!(
            "illutag-discovery-resume-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&root).unwrap();
        let value = Self {
            path: root.join("derived.sqlite"),
            root,
        };
        value
            .open()
            .execute_batch(
                "CREATE TABLE generations(generation INTEGER PRIMARY KEY,state TEXT NOT NULL);
                 INSERT INTO generations VALUES(7,'building');
                 CREATE TABLE fingerprints(image_pk INTEGER PRIMARY KEY,public_hash BLOB NOT NULL,signature BLOB NOT NULL);
                 INSERT INTO fingerprints VALUES(1,zeroblob(32),zeroblob(32));",
            )
            .unwrap();
        value
    }

    fn open(&self) -> Connection {
        Connection::open(&self.path).unwrap()
    }
}

impl Drop for JournalDb {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn coverage(processed: i64) -> DuplicateCoverage {
    DuplicateCoverage {
        processed,
        total: 100,
        verified_pairs: processed / 2,
        ..Default::default()
    }
}

#[test]
fn discovery_resume_rejects_a_completed_dense_cursor_from_the_raw_bucket_policy() {
    let db = JournalDb::new();
    let conn = db.open();
    let mut legacy = Sha256::new();
    legacy.update(b"illuTag-discovery-resume-v1");
    legacy.update(super::super::super::evidence::POLICY.as_bytes());
    legacy.update(b"standard");
    legacy.update(1_i64.to_le_bytes());
    legacy.update([0_u8; 32]);
    legacy.update([0_u8; 32]);
    let legacy = legacy.finalize().to_vec();
    let mut old = resume::create(&conn, 7, &legacy).unwrap();
    old.record(
        index::AnchorCheckpoint {
            next_anchor: 999,
            newly_covered: &[],
            dense_reference: Some(1),
            layout: [5; 32],
        },
        &coverage(999),
    );
    old.flush(&conn, &mut || Ok(())).unwrap();
    let current = resume::input_key(&conn, &[], "standard", &mut || Ok(())).unwrap();
    assert_ne!(legacy, current);
    assert!(resume::load(&conn, &current).unwrap().is_none());
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM fingerprints", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

fn record(index: u32) -> index::IndexRecord {
    let mut sha256 = [0; 32];
    sha256[..4].copy_from_slice(&index.to_le_bytes());
    index::IndexRecord {
        image_pk: i64::from(index) + 1,
        image_order: index,
        file_size: 123,
        width: 1280,
        height: 720,
        sha256,
        near: Some(NearFingerprint {
            phash: 42,
            dhash: 0x5555_5555_5555_5555,
            color: [128; 48],
            width: 1280,
            height: 720,
            low_information: false,
        }),
    }
}

#[test]
fn discovery_resume_journal_restores_a_committed_checkpoint_across_connections() {
    let db = JournalDb::new();
    let conn = db.open();
    let key = [3; 32];
    let mut journal = resume::create(&conn, 7, &key).unwrap();
    assert!(!journal.exact_done);
    assert_eq!(journal.index.next_anchor, 0);
    assert!(journal.record(
        index::AnchorCheckpoint {
            next_anchor: 3,
            newly_covered: &[0, 2],
            dense_reference: Some(1),
            layout: [9; 32],
        },
        &coverage(42),
    ));
    journal.flush(&conn, &mut || Ok(())).unwrap();
    drop(journal);
    drop(conn);

    let restored = resume::load(&db.open(), &key).unwrap().unwrap();
    assert_eq!(restored.generation, 7);
    assert!(restored.exact_done);
    assert_eq!(restored.index.next_anchor, 3);
    assert_eq!(restored.index.layout, Some([9; 32]));
    assert_eq!(restored.index.covered, HashSet::from([0, 2]));
    assert_eq!(restored.index.dense, vec![1]);
    assert_eq!(restored.coverage.processed, 42);
    assert_eq!(restored.coverage.verified_pairs, 21);
}

#[test]
fn discovery_resume_rejects_different_inputs_and_a_published_generation() {
    let db = JournalDb::new();
    let conn = db.open();
    let mut records = vec![record(0)];
    let key = resume::input_key(&conn, &records, "standard", &mut || Ok(())).unwrap();
    resume::create(&conn, 7, &key).unwrap();
    assert!(resume::load(&conn, &key).unwrap().is_some());
    let sensitivity = resume::input_key(&conn, &records, "strict", &mut || Ok(())).unwrap();
    assert_ne!(sensitivity, key);
    assert!(resume::load(&conn, &sensitivity).unwrap().is_none());
    records[0].image_pk += 1;
    let membership = resume::input_key(&conn, &records, "standard", &mut || Ok(())).unwrap();
    assert_ne!(membership, key);
    assert!(resume::load(&conn, &membership).unwrap().is_none());
    records[0].image_pk -= 1;
    conn.execute(
        "UPDATE fingerprints SET signature=?1",
        [[8_u8; 32].as_slice()],
    )
    .unwrap();
    let signature = resume::input_key(&conn, &records, "standard", &mut || Ok(())).unwrap();
    assert_ne!(signature, key);
    assert!(resume::load(&conn, &signature).unwrap().is_none());
    conn.execute("UPDATE generations SET state='complete'", [])
        .unwrap();
    assert!(resume::load(&conn, &key).unwrap().is_none());
}

#[test]
fn discovery_resume_failed_flush_rolls_back_all_tables_and_can_retry() {
    let db = JournalDb::new();
    let conn = db.open();
    let key = [4; 32];
    let mut journal = resume::create(&conn, 7, &key).unwrap();
    journal.record(
        index::AnchorCheckpoint {
            next_anchor: 1,
            newly_covered: &[0],
            dense_reference: None,
            layout: [5; 32],
        },
        &coverage(2),
    );
    journal.flush(&conn, &mut || Ok(())).unwrap();
    journal.record(
        index::AnchorCheckpoint {
            next_anchor: 3,
            newly_covered: &[1, 2],
            dense_reference: Some(2),
            layout: [5; 32],
        },
        &coverage(8),
    );
    conn.execute_batch("CREATE TRIGGER fail_dense BEFORE INSERT ON discovery_dense BEGIN SELECT RAISE(ABORT,'synthetic checkpoint write failure'); END;").unwrap();
    let error = journal.flush(&conn, &mut || Ok(())).unwrap_err();
    assert!(error.contains("synthetic checkpoint write failure"));
    assert!(conn.is_autocommit());
    let restored = resume::load(&db.open(), &key).unwrap().unwrap();
    assert_eq!(restored.index.next_anchor, 1);
    assert_eq!(restored.index.covered, HashSet::from([0]));
    assert!(restored.index.dense.is_empty());
    assert_eq!(restored.coverage.processed, 2);

    conn.execute_batch("DROP TRIGGER fail_dense").unwrap();
    journal.flush(&conn, &mut || Ok(())).unwrap();
    let restored = resume::load(&db.open(), &key).unwrap().unwrap();
    assert_eq!(restored.index.next_anchor, 3);
    assert_eq!(restored.index.covered, HashSet::from([0, 1, 2]));
    assert_eq!(restored.index.dense, vec![2]);
    assert_eq!(restored.coverage.processed, 8);
}

#[test]
fn discovery_resume_failed_creation_preserves_the_previous_checkpoint() {
    let db = JournalDb::new();
    let conn = db.open();
    let previous = [1; 32];
    let mut journal = resume::create(&conn, 7, &previous).unwrap();
    journal.record(
        index::AnchorCheckpoint {
            next_anchor: 2,
            newly_covered: &[1],
            dense_reference: Some(0),
            layout: [5; 32],
        },
        &coverage(4),
    );
    journal.flush(&conn, &mut || Ok(())).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_create BEFORE INSERT ON discovery_checkpoint BEGIN SELECT RAISE(ABORT,'synthetic checkpoint create failure'); END;").unwrap();
    let error = resume::create(&conn, 7, &[2; 32]).err().unwrap();
    assert!(error.contains("synthetic checkpoint create failure"));
    assert!(conn.is_autocommit());
    let restored = resume::load(&db.open(), &previous).unwrap().unwrap();
    assert_eq!(restored.index.next_anchor, 2);
    assert_eq!(restored.index.covered, HashSet::from([1]));
    assert_eq!(restored.index.dense, vec![0]);
    assert_eq!(restored.coverage.processed, 4);
}

#[test]
fn discovery_resume_index_skips_completed_anchors_and_keeps_covered_bridges() {
    let records: Vec<_> = (0..3).map(record).collect();
    let mut saved = index::EvidenceResume::default();
    let mut before = Vec::new();
    let result = index::build_evidence_resumable(
        &records,
        index::Threshold::Standard,
        &index::EvidenceResume::default(),
        &mut || Ok(()),
        &mut |a, b, _| Ok((a.min(b), a.max(b)) != (0, 2)),
        &mut |group| {
            before.push(group);
            Ok(())
        },
        &mut |checkpoint| {
            saved.layout = Some(checkpoint.layout);
            saved.next_anchor = checkpoint.next_anchor;
            saved.covered.extend(checkpoint.newly_covered);
            if checkpoint.next_anchor == 1 {
                Err("synthetic stop after committed anchor".into())
            } else {
                Ok(())
            }
        },
    );
    assert!(result.unwrap_err().contains("synthetic stop"));
    assert_eq!(saved.next_anchor, 1);
    assert_eq!(saved.covered, HashSet::from([0, 1]));
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].reference, 0);

    let mut comparisons = Vec::new();
    let mut groups = Vec::new();
    let mut anchors = Vec::new();
    let summary = index::build_evidence_resumable(
        &records,
        index::Threshold::Standard,
        &saved,
        &mut || Ok(()),
        &mut |a, b, _| {
            comparisons.push((a, b));
            Ok((a.min(b), a.max(b)) != (0, 2))
        },
        &mut |group| {
            groups.push(group);
            Ok(())
        },
        &mut |checkpoint| {
            anchors.push(checkpoint.next_anchor);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(anchors, vec![1, 2, 3]);
    assert_eq!(comparisons, vec![(2, 0), (2, 1)]);
    assert_eq!(summary.group_count, 1);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].reference, 2);
    assert_eq!(
        groups[0]
            .members
            .iter()
            .map(|m| m.record_index)
            .collect::<Vec<_>>(),
        vec![2, 1],
        "the previously covered image must remain a bridge, not disappear"
    );
}

#[test]
fn discovery_resume_index_rejects_changed_layout_before_near_comparisons() {
    let mut records: Vec<_> = (0..3).map(record).collect();
    let mut saved = index::EvidenceResume::default();
    index::build_evidence_resumable(
        &records,
        index::Threshold::Standard,
        &index::EvidenceResume::default(),
        &mut || Ok(()),
        &mut |_, _, _| Ok(false),
        &mut |_| Ok(()),
        &mut |checkpoint| {
            saved.layout = Some(checkpoint.layout);
            Ok(())
        },
    )
    .unwrap();
    records[1].image_pk += 100;
    let error = index::build_evidence_resumable(
        &records,
        index::Threshold::Standard,
        &saved,
        &mut || Ok(()),
        &mut |_, _, _| panic!("changed layout must reject before comparisons"),
        &mut |_| panic!("changed layout must not publish a near group"),
        &mut |_| panic!("changed layout must not advance the cursor"),
    )
    .unwrap_err();
    assert!(error.contains("断点内容单元已变化"));
}

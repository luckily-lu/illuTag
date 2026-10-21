use super::*;
use std::{cell::Cell, sync::mpsc, thread, time::Duration};

#[test]
fn optional_paging_contract_fields_are_omitted_instead_of_null() {
    let request = DuplicateWorkflowQuery {
        generation: "1".into(),
        kind: "exact".into(),
        cursor: None,
        group_id: None,
        limit: 20,
        reanchor: None,
    };
    assert!(serde_json::to_value(&request)
        .unwrap()
        .get("reanchor")
        .is_none());
    let response = DuplicateWorkflowPage {
        generation: "1".into(),
        revision: "1".into(),
        groups: vec![],
        next_cursor: None,
        total_groups: 0,
        paging: None,
    };
    assert!(serde_json::to_value(&response)
        .unwrap()
        .get("paging")
        .is_none());
}

fn uncached_pair() -> super::super::tests::Fixture {
    let f = super::super::verification_tests::fixture();
    f.run().unwrap();
    let conn = cache::open_existing(&f.state, true).unwrap();
    conn.execute("DELETE FROM pair_checks WHERE a=1 AND b=2", [])
        .unwrap();
    f
}

#[test]
fn planning_draft_does_not_hold_a_cache_writer_during_control_hooks() {
    let f = uncached_pair();
    let info = status(&f.state).unwrap();
    let gen: i64 = info.generation.unwrap().parse().unwrap();
    let conn = open(&f.state).unwrap();
    let mut verifier = verification_cache::Verifier::new(&f.state, false).unwrap();
    let mut checked = false;
    planning::build_for_job(&conn, gen, 0, &f.state, &mut || {
        if !checked {
            assert!(
                lock(&f.state).try_lock().is_ok(),
                "a paused job must not retain the review gate"
            );
            checked = true;
            verifier.check(1, 2, false, &f.state, &mut || Ok(()))?;
        }
        Ok(())
    })
    .unwrap();
    assert!(checked);
    assert!(verifier.accepted + verifier.variants > 0);
}

#[test]
fn pair_cache_waits_through_a_writer_longer_than_old_timeout_and_stays_cancellable() {
    let f = uncached_pair();
    let mut verifier = verification_cache::Verifier::new(&f.state, false).unwrap();
    let other = cache::open_existing(&f.state, true).unwrap();
    let (ready_tx, ready_rx) = mpsc::channel();
    let writer = thread::spawn(move || {
        other.execute_batch("BEGIN IMMEDIATE").unwrap();
        ready_tx.send(()).unwrap();
        thread::sleep(Duration::from_millis(2300));
        other.execute_batch("COMMIT").unwrap();
    });
    ready_rx.recv().unwrap();
    let mut checkpoints = 0;
    assert!(verifier
        .check(1, 2, false, &f.state, &mut || {
            checkpoints += 1;
            Ok(())
        })
        .unwrap()
        .is_equivalent());
    writer.join().unwrap();
    assert!(
        checkpoints > 10,
        "waiting must check pause/cancel frequently"
    );

    let conn = cache::open_existing(&f.state, true).unwrap();
    let retry = cache::open_existing(&f.state, true).unwrap();
    conn.execute("DELETE FROM pair_checks WHERE a=1 AND b=2", [])
        .unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    let mut waits = 0;
    let result = cache::controlled_write(
        &retry,
        &mut || {
            waits += 1;
            if waits > 8 {
                Err("任务已取消".into())
            } else {
                Ok(())
            }
        },
        || retry.execute("INSERT INTO meta VALUES('cancelled_wait','1')", []),
    );
    conn.execute_batch("ROLLBACK").unwrap();
    assert!(result.unwrap_err().contains("任务已取消"));
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM pair_checks WHERE a=1 AND b=2",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM meta WHERE key='cancelled_wait'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn manual_edit_during_plan_control_is_merged_without_repeating_image_refinement() {
    let f = uncached_pair();
    let first = status(&f.state).unwrap();
    let page = query(
        DuplicateWorkflowQuery {
            generation: first.generation.clone().unwrap(),
            kind: "candidate".into(),
            cursor: None,
            group_id: None,
            limit: 20,
            reanchor: None,
        },
        &f.state,
    )
    .unwrap();
    let group = &page.groups[0];
    let image = group
        .members
        .iter()
        .find(|m| !m.is_keeper)
        .unwrap()
        .member
        .image
        .id
        .clone();
    let merging = Cell::new(false);
    let edited = Cell::new(false);
    let mut merged_again = false;
    run_job(
        DuplicateOptions {
            sensitivity: "standard".into(),
            force_recompute: false,
            regroup_only: true,
            verification_mode: "refine".into(),
        },
        &f.state,
        &mut || {
            if merging.get() && !edited.get() {
                assert!(
                    lock(&f.state).try_lock().is_ok(),
                    "plan control cannot hold the review gate"
                );
                edited.set(true);
                let current = status(&f.state)?;
                edit(
                    DuplicateReviewEdit {
                        generation: current.generation.unwrap(),
                        revision: current.revision,
                        group_id: group.id.clone(),
                        image_id: Some(image.clone()),
                        action: "keep".into(),
                    },
                    &f.state,
                )?;
            }
            Ok(())
        },
        &mut |stage, _| {
            if stage == "合并整理计划" {
                merging.set(true);
            }
            if stage == "合并最新人工选择" {
                merged_again = true;
            }
        },
    )
    .unwrap();
    assert!(edited.get() && merged_again);
    let after = status(&f.state).unwrap();
    assert_eq!(after.stage, "refined");
    assert_eq!(after.protected_count, 1);
}

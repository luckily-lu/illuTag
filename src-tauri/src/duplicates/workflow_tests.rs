use super::tests::Fixture;
use super::*;
use rusqlite::params;
use sha2::Digest;

fn query_for(status: &DuplicateWorkflowStatus, kind: &str) -> DuplicateWorkflowQuery {
    DuplicateWorkflowQuery {
        generation: status.generation.clone().unwrap(),
        kind: kind.into(),
        cursor: None,
        limit: 20,
        group_id: None,
        reanchor: None,
    }
}
fn edit_for(
    status: &DuplicateWorkflowStatus,
    group: &DuplicateWorkflowGroup,
    image: Option<String>,
    action: &str,
) -> DuplicateReviewEdit {
    DuplicateReviewEdit {
        generation: status.generation.clone().unwrap(),
        revision: status.revision.clone(),
        group_id: group.id.clone(),
        image_id: image,
        action: action.into(),
    }
}
fn refine(f: &Fixture) {
    run_job(
        DuplicateOptions {
            sensitivity: "standard".into(),
            force_recompute: false,
            regroup_only: true,
            verification_mode: "organize".into(),
        },
        &f.state,
        &mut || Ok(()),
        &mut |_, _| {},
    )
    .unwrap();
}

#[test]
fn review_reanchors_published_pages_without_reusing_stale_cursors() {
    let f = Fixture::new(2);
    f.run().unwrap();
    let initial = workflow::status(&f.state).unwrap();
    let generation: i64 = initial.generation.as_ref().unwrap().parse().unwrap();
    let conn = cache::open_existing(&f.state, true).unwrap();
    conn.execute_batch("CREATE TEMP TABLE source_members AS SELECT * FROM members;
        CREATE TEMP TABLE source_groups AS SELECT * FROM groups;
        DELETE FROM members; DELETE FROM groups;
        WITH RECURSIVE nums(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM nums WHERE n<43)
        INSERT INTO groups SELECT s.generation,n*10,s.kind,s.reference_pk,s.total_members FROM source_groups s CROSS JOIN nums;
        WITH RECURSIVE nums(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM nums WHERE n<43)
        INSERT INTO members SELECT s.generation,n*10,s.member_no,s.image_pk,s.public_hash,s.signature,s.phash_distance,s.dhash_distance,s.color_distance FROM source_members s CROSS JOIN nums;").unwrap();
    let first = workflow::query(query_for(&initial, "exact"), &f.state).unwrap();
    let old_cursor = first.next_cursor.clone().unwrap();
    cache::membership_invalidated(&f.state);
    let updated = workflow::status(&f.state).unwrap();
    assert_ne!(initial.revision, updated.revision);
    let mut stale = query_for(&updated, "exact");
    stale.cursor = Some(old_cursor);
    assert!(workflow::query(stale, &f.state)
        .unwrap_err()
        .contains("刷新分页"));

    let mut request = query_for(&updated, "exact");
    request.group_id = Some("exact:220".into());
    request.reanchor = Some(true);
    let anchored = workflow::query(request, &f.state).unwrap();
    assert_eq!(anchored.groups[0].id, "exact:220");
    assert_eq!(anchored.groups.last().unwrap().id, "exact:410");
    assert_eq!(anchored.total_groups, 43);
    let paging = anchored.paging.unwrap();
    assert_eq!(paging.offset, 21);
    assert_eq!(paging.previous_limit, 20);
    let mut previous = query_for(&updated, "exact");
    previous.cursor = paging.previous_cursor;
    previous.limit = paging.previous_limit;
    let previous = workflow::query(previous, &f.state).unwrap();
    assert_eq!(previous.groups[0].id, "exact:20");
    assert_eq!(previous.groups.last().unwrap().id, "exact:210");
    assert_eq!(previous.paging.unwrap().offset, 1);
    let mut next = query_for(&updated, "exact");
    next.cursor = anchored.next_cursor;
    let next = workflow::query(next, &f.state).unwrap();
    assert_eq!(next.groups[0].id, "exact:420");
    assert_eq!(next.groups.len(), 2);

    let mut gone = query_for(&updated, "exact");
    gone.group_id = Some("exact:9999".into());
    gone.reanchor = Some(true);
    let fallback = workflow::query(gone, &f.state).unwrap();
    assert_eq!(fallback.paging.unwrap().offset, 23);
    assert_eq!(fallback.groups.last().unwrap().id, "exact:430");
    assert_eq!(fallback.generation, generation.to_string());
}

#[test]
fn exact_copies_of_a_content_variant_still_leave_only_one_copy() {
    let f = super::verification_tests::fixture();
    // Images 3 and 4 contain the same edited detail; each differs from image 1,
    // but they are byte-for-byte copies of one another.
    std::fs::copy(&f.paths[2], &f.paths[3]).unwrap();
    f.run().unwrap();
    let conn = cache::open_existing(&f.state, true).unwrap();
    conn.execute("UPDATE groups SET reference_pk=1 WHERE kind=1 AND EXISTS(SELECT 1 FROM members m WHERE m.generation=groups.generation AND m.group_no=groups.group_no AND m.image_pk=1)", []).unwrap();
    drop(conn);
    refine(&f);
    let ready = workflow::status(&f.state).unwrap();
    let conn = cache::open_existing(&f.state, false).unwrap();
    let variants: i64 = conn.query_row("SELECT COUNT(DISTINCT image_pk) FROM workflow_class WHERE kind='similar' AND image_pk IN (3,4)", [], |r| r.get(0)).unwrap();
    assert_eq!(
        variants, 1,
        "one representative per confirmed byte-content unit is compared"
    );
    let exact = ready.categories.iter().find(|c| c.kind == "exact").unwrap();
    assert_eq!(exact.images, 2);
    assert_eq!(
        exact.cleanup_count, 1,
        "preserve the distinct content, but do not protect every identical copy of it"
    );
    let exact = workflow::query(query_for(&ready, "exact"), &f.state).unwrap();
    assert_eq!(
        exact.groups[0]
            .members
            .iter()
            .filter(|m| m.disposition == "clean")
            .count(),
        1
    );
    assert!(exact
        .groups
        .iter()
        .flat_map(|g| &g.members)
        .any(|m| m.disposition == "clean" && m.reason.contains("字节完全相同")));
}

#[test]
fn rejected_review_actions_do_not_invalidate_the_existing_plan() {
    let f = super::verification_tests::fixture();
    f.run().unwrap();
    let before = workflow::status(&f.state).unwrap();
    let groups = workflow::query(query_for(&before, "candidate"), &f.state).unwrap();
    let group = &groups.groups[0];
    let image = group.members[0].member.image.id.clone();
    for action in ["clean", "invalid-action"] {
        let result = workflow::edit(
            edit_for(&before, group, Some(image.clone()), action),
            &f.state,
        );
        assert!(result.is_err(), "{action} should be rejected");
        let after = workflow::status(&f.state).unwrap();
        assert_eq!(
            after.revision, before.revision,
            "a rejected {action} action must not expire issued cursors/plans"
        );
        assert_eq!(after.cleanup_count, before.cleanup_count);
    }
}

#[test]
fn manual_protection_during_unpublished_refinement_survives_publication() {
    let f = super::verification_tests::fixture();
    f.run().unwrap();
    let before = workflow::status(&f.state).unwrap();
    let candidates = workflow::query(query_for(&before, "candidate"), &f.state).unwrap();
    let group = &candidates.groups[0];
    let protected = group
        .members
        .iter()
        .find(|m| !m.is_keeper)
        .unwrap()
        .member
        .image
        .id
        .clone();
    let mut edited = false;
    run_job(
        DuplicateOptions {
            sensitivity: "standard".into(),
            force_recompute: false,
            regroup_only: true,
            verification_mode: "refine".into(),
        },
        &f.state,
        &mut || Ok(()),
        &mut |stage, _| {
            if stage == "复检近似候选" && !edited {
                workflow::edit(
                    edit_for(&before, group, Some(protected.clone()), "keep"),
                    &f.state,
                )
                .unwrap();
                edited = true;
            }
        },
    )
    .unwrap();
    assert!(edited);
    let after = workflow::status(&f.state).unwrap();
    assert_eq!(after.stage, "refined");
    assert_eq!(after.protected_count, 1);
    let conn = cache::open_existing(&f.state, false).unwrap();
    let public = sha2::Sha256::digest(protected.as_bytes());
    let target: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM workflow_plan p JOIN fingerprints f ON f.image_pk=p.image_pk WHERE p.run=(SELECT plan FROM workflow_runs WHERE generation=?1) AND f.public_hash=?2 AND p.keeper IS NOT NULL)", params![after.generation, public.as_slice()], |r|r.get(0)).unwrap();
    assert!(
        !target,
        "publication must use the manual choice made during refinement"
    );
}
#[test]
fn workflow_two_stages_manual_protection_and_global_undo() {
    let f = Fixture::new(4);
    f.run().unwrap();
    let first = workflow::status(&f.state).unwrap();
    assert_eq!(first.stage, "scanned");
    assert_eq!(first.cleanup_count, 3);
    assert!(workflow::prepare(
        DuplicateWorkflowRequest {
            generation: first.generation.clone().unwrap(),
            revision: first.revision.clone()
        },
        &f.state
    )
    .is_err());
    let page = workflow::query(query_for(&first, "exact"), &f.state).unwrap();
    assert_eq!(page.groups[0].members.len(), 4);
    let candidate = page.groups[0]
        .members
        .iter()
        .find(|m| m.disposition == "clean")
        .unwrap()
        .member
        .image
        .id
        .clone();
    let protected = workflow::edit(
        edit_for(&first, &page.groups[0], Some(candidate), "keep"),
        &f.state,
    )
    .unwrap();
    assert_eq!(protected.cleanup_count, 2);
    assert_eq!(protected.protected_count, 1);
    assert!(workflow::edit(edit_for(&first, &page.groups[0], None, "confirm"), &f.state).is_err());
    refine(&f);
    let ready = workflow::status(&f.state).unwrap();
    assert_eq!(ready.stage, "refined");
    assert_eq!(ready.cleanup_count, 2);
    let preview = workflow::prepare(
        DuplicateWorkflowRequest {
            generation: ready.generation.clone().unwrap(),
            revision: ready.revision.clone(),
        },
        &f.state,
    )
    .unwrap();
    assert_eq!(preview.target_count, 2);
    let progress = management::operation_status(&preview.operation_id, &f.state).unwrap();
    assert_eq!(progress.stage, "workflowSnapshot");
    assert_eq!(progress.stage_processed, 0);
    assert_eq!(progress.stage_total, 2);
    management::run_operation(&preview.operation_id, &f.state, &mut || Ok(()), &mut |_| {})
        .unwrap();
    let after = workflow::status(&f.state).unwrap();
    assert_eq!(after.cleanup_count, 0);
    let undo = management::prepare_undo(&preview.operation_id, &f.state).unwrap();
    management::run_operation(&undo, &f.state, &mut || Ok(()), &mut |_| {}).unwrap();
    assert_eq!(workflow::status(&f.state).unwrap().cleanup_count, 2);
}

#[test]
fn workflow_stale_page_and_keeper_preview_are_rejected() {
    let f = Fixture::new(3);
    f.run().unwrap();
    refine(&f);
    let info = workflow::status(&f.state).unwrap();
    let page = workflow::query(query_for(&info, "exact"), &f.state).unwrap();
    let preview = workflow::prepare(
        DuplicateWorkflowRequest {
            generation: info.generation.clone().unwrap(),
            revision: info.revision.clone(),
        },
        &f.state,
    )
    .unwrap();
    let target = page.groups[0]
        .members
        .iter()
        .find(|m| !m.is_keeper)
        .unwrap();
    let edited = workflow::edit(
        edit_for(
            &info,
            &page.groups[0],
            Some(target.member.image.id.clone()),
            "keeper",
        ),
        &f.state,
    )
    .unwrap();
    assert_ne!(info.revision, edited.revision);
    assert!(management::run_operation(
        &preview.operation_id,
        &f.state,
        &mut || Ok(()),
        &mut |_| {}
    )
    .is_err());
    let now = workflow::query(query_for(&edited, "exact"), &f.state).unwrap();
    assert!(
        now.groups[0]
            .members
            .iter()
            .find(|m| m.member.image.id == target.member.image.id)
            .unwrap()
            .is_keeper
    );
}

#[test]
fn workflow_classifies_content_edits_and_only_explicit_manual_clean_can_recycle_them() {
    let f = super::verification_tests::fixture();
    f.run().unwrap();
    let scanned = workflow::status(&f.state).unwrap();
    let initial = workflow::query(query_for(&scanned, "candidate"), &f.state).unwrap();
    assert!(!initial.groups.is_empty());
    let edit = initial.groups[0]
        .members
        .iter()
        .find(|m| m.member.image.id == f.paths[2].to_string_lossy())
        .unwrap();
    assert!(workflow::edit(
        edit_for(
            &scanned,
            &initial.groups[0],
            Some(edit.member.image.id.clone()),
            "clean"
        ),
        &f.state
    )
    .is_err());
    refine(&f);
    let info = workflow::status(&f.state).unwrap();
    let similar = workflow::query(query_for(&info, "similar"), &f.state).unwrap();
    let near = workflow::query(query_for(&info, "near"), &f.state).unwrap();
    assert!(
        near.groups.is_empty(),
        "uncalibrated JPEG re-encoding must not become automatic raster evidence"
    );
    assert!(!similar.groups.is_empty());
    let edited_id = f.paths[2].to_string_lossy().into_owned();
    assert!(similar.groups.iter().any(|g| g
        .members
        .iter()
        .any(|m| m.member.image.id == edited_id && m.disposition == "keep")));
    assert!(!near
        .groups
        .iter()
        .any(|g| g.members.iter().any(|m| m.member.image.id == edited_id)));
    let prepared = workflow::prepare(
        DuplicateWorkflowRequest {
            generation: info.generation.clone().unwrap(),
            revision: info.revision.clone(),
        },
        &f.state,
    )
    .unwrap();
    management::run_operation(
        &prepared.operation_id,
        &f.state,
        &mut || Ok(()),
        &mut |_| {},
    )
    .unwrap();
    let db = cache::library(&f.state).unwrap();
    assert_eq!(
        db.query_row("SELECT trashed FROM images WHERE image_pk=3", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        management::operation_status(&prepared.operation_id, &f.state)
            .unwrap()
            .succeeded,
        1
    );
    let undo = management::prepare_undo(&prepared.operation_id, &f.state).unwrap();
    management::run_operation(&undo, &f.state, &mut || Ok(()), &mut |_| {}).unwrap();
    let current = workflow::status(&f.state).unwrap();
    let page = workflow::query(query_for(&current, "similar"), &f.state).unwrap();
    let changed = workflow::edit(
        edit_for(&current, &page.groups[0], Some(edited_id.clone()), "clean"),
        &f.state,
    )
    .unwrap();
    let page = workflow::query(query_for(&changed, "similar"), &f.state).unwrap();
    assert_eq!(
        page.groups[0]
            .members
            .iter()
            .find(|m| m.member.image.id == edited_id)
            .unwrap()
            .disposition,
        "manual"
    );
    let p = workflow::prepare(
        DuplicateWorkflowRequest {
            generation: changed.generation.unwrap(),
            revision: changed.revision,
        },
        &f.state,
    )
    .unwrap();
    management::run_operation(&p.operation_id, &f.state, &mut || Ok(()), &mut |_| {}).unwrap();
    assert_eq!(
        db.query_row("SELECT trashed FROM images WHERE image_pk=3", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn workflow_cancelled_refinement_retains_published_classification_and_plan() {
    use std::cell::Cell;
    let f = super::verification_tests::fixture();
    f.run().unwrap();
    refine(&f);
    let before = workflow::status(&f.state).unwrap();
    let calls = Cell::new(0);
    let result = run_job(
        DuplicateOptions {
            sensitivity: "standard".into(),
            force_recompute: false,
            regroup_only: true,
            verification_mode: "refine".into(),
        },
        &f.state,
        &mut || {
            calls.set(calls.get() + 1);
            if calls.get() >= 5 {
                Err("任务已取消".into())
            } else {
                Ok(())
            }
        },
        &mut |_, _| {},
    );
    assert!(result.is_err());
    let after = workflow::status(&f.state).unwrap();
    assert_eq!(before.revision, after.revision);
    assert_eq!(before.cleanup_count, after.cleanup_count);
}

#[test]
fn workflow_final_survivor_and_recycled_review_are_accessible() {
    let f = Fixture::new(2);
    f.run().unwrap();
    refine(&f);
    let info = workflow::status(&f.state).unwrap();
    let page = workflow::query(query_for(&info, "exact"), &f.state).unwrap();
    let keep = page.groups[0].members.iter().find(|m| m.is_keeper).unwrap();
    assert!(workflow::edit(
        edit_for(
            &info,
            &page.groups[0],
            Some(keep.member.image.id.clone()),
            "clean"
        ),
        &f.state
    )
    .is_err());
    let info = workflow::status(&f.state).unwrap();
    let preview = workflow::prepare(
        DuplicateWorkflowRequest {
            generation: info.generation.clone().unwrap(),
            revision: info.revision.clone(),
        },
        &f.state,
    )
    .unwrap();
    management::run_operation(&preview.operation_id, &f.state, &mut || Ok(()), &mut |_| {})
        .unwrap();
    let done = workflow::status(&f.state).unwrap();
    assert_eq!(
        done.categories
            .iter()
            .find(|c| c.kind == "exact")
            .unwrap()
            .groups,
        0
    );
    let old = workflow::query(query_for(&done, "exact"), &f.state).unwrap();
    assert_eq!(old.groups.len(), 1);
    assert!(old.groups[0].members.iter().any(|m| m.member.image.trashed));
}

#[test]
fn workflow_overlapping_stars_do_not_recycle_a_survivor_through_a_bridge() {
    use rusqlite::params;
    let f = Fixture::new(3);
    f.run().unwrap();
    let generation = status(&f.state).unwrap().generation.unwrap();
    let conn = cache::open_existing(&f.state, true).unwrap();
    conn.execute("DELETE FROM members WHERE generation=?1", [&generation])
        .unwrap();
    conn.execute("DELETE FROM groups WHERE generation=?1", [&generation])
        .unwrap();
    for (group, reference, ids) in [(0, 1, vec![1, 2]), (1, 2, vec![2, 3])] {
        conn.execute(
            "INSERT INTO groups VALUES(?1,?2,1,?3,2)",
            params![generation, group, reference],
        )
        .unwrap();
        for (number, pk) in ids.iter().enumerate() {
            conn.execute("INSERT INTO members SELECT ?1,?2,?3,image_pk,public_hash,signature,0,0,0 FROM fingerprints WHERE image_pk=?4",params![generation,group,number as i64,pk]).unwrap();
        }
    }
    conn.execute(
        "UPDATE fingerprints SET width=(4-image_pk)*100,height=100",
        [],
    )
    .unwrap();
    drop(conn);
    cache::membership_invalidated(&f.state);
    // Exercise a deliberately sparse proof graph, rather than rediscovering the
    // byte-identical fixture (which would legitimately find all three copies).
    run_job(
        DuplicateOptions {
            sensitivity: "standard".into(),
            force_recompute: false,
            regroup_only: true,
            verification_mode: "refine".into(),
        },
        &f.state,
        &mut || Ok(()),
        &mut |_, _| {},
    )
    .unwrap();
    let conn = cache::open_existing(&f.state, true).unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO meta VALUES(?1,?2)",
        params![format!("policy:{generation}"), evidence::POLICY],
    )
    .unwrap();
    drop(conn);
    let ready = workflow::status(&f.state).unwrap();
    assert_eq!(ready.cleanup_count, 1);
    let prepared = workflow::prepare(
        DuplicateWorkflowRequest {
            generation: ready.generation.unwrap(),
            revision: ready.revision,
        },
        &f.state,
    )
    .unwrap();
    management::run_operation(
        &prepared.operation_id,
        &f.state,
        &mut || Ok(()),
        &mut |_| {},
    )
    .unwrap();
    let db = cache::library(&f.state).unwrap();
    assert_eq!(
        db.query_row("SELECT SUM(trashed) FROM images", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row("SELECT trashed FROM images WHERE image_pk=1", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        db.query_row("SELECT trashed FROM images WHERE image_pk=3", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn workflow_failed_publication_keeps_complete_classification_and_plan() {
    let f = super::verification_tests::fixture();
    f.run().unwrap();
    refine(&f);
    let before = workflow::status(&f.state).unwrap();
    let conn = cache::open_existing(&f.state, true).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_workflow_publication BEFORE UPDATE ON workflow_runs BEGIN SELECT RAISE(ABORT,'simulated disk failure before publication'); END").unwrap();
    let failed = run_job(
        DuplicateOptions {
            sensitivity: "standard".into(),
            force_recompute: false,
            regroup_only: true,
            verification_mode: "refine".into(),
        },
        &f.state,
        &mut || Ok(()),
        &mut |_, _| {},
    );
    assert!(failed.is_err());
    conn.execute_batch("DROP TRIGGER fail_workflow_publication")
        .unwrap();
    drop(conn);
    let after = workflow::status(&f.state).unwrap();
    assert_eq!(before.revision, after.revision);
    assert_eq!(before.cleanup_count, after.cleanup_count);
    assert_eq!(
        before
            .categories
            .iter()
            .map(|c| (&c.kind, c.images))
            .collect::<Vec<_>>(),
        after
            .categories
            .iter()
            .map(|c| (&c.kind, c.images))
            .collect::<Vec<_>>()
    );
}

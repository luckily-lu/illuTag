//! Category buttons must be independent even when the global near survivor is
//! outside a smaller exact component. All images and databases are synthetic.
use super::tests::Fixture;
use super::*;
use image::{
    codecs::png::{CompressionType, FilterType, PngEncoder},
    ImageEncoder, Rgb, RgbImage,
};
use rusqlite::{params, Connection};
use std::{cell::Cell, fs::File};

fn scene() -> RgbImage {
    RgbImage::from_fn(512, 384, |x, y| {
        let wave = ((x as f64 / 35.).sin() * 15. + (y as f64 / 47.).cos() * 10.) as i32;
        let c = if (x as i64 - 310).pow(2) + (y as i64 - 130).pow(2) < 4200 {
            [190, 100, 85]
        } else if x > 115 && x < 285 && y > 115 && y < 315 {
            [70, 125, 170]
        } else {
            [125 + (x / 12) as i32, 150, 155 + (y / 16) as i32]
        };
        Rgb(c.map(|v| (v + wave).clamp(0, 255) as u8))
    })
}
fn fixture() -> Fixture {
    let f = Fixture::new(3);
    let large = scene();
    let small = image::imageops::resize(&large, 256, 192, image::imageops::FilterType::Triangle);
    let db = Connection::open(&f.state.database_path).unwrap();
    for (index, pixels, compression) in [
        (0, &large, CompressionType::Default),
        (1, &small, CompressionType::Fast),
        (2, &small, CompressionType::Best),
    ] {
        PngEncoder::new_with_quality(
            File::create(&f.paths[index]).unwrap(),
            compression,
            FilterType::Adaptive,
        )
        .write_image(
            pixels.as_raw(),
            pixels.width(),
            pixels.height(),
            image::ExtendedColorType::Rgb8,
        )
        .unwrap();
        db.execute(
            "UPDATE images SET width=?2,height=?3,file_size=?4 WHERE image_pk=?1",
            params![
                index as i64 + 1,
                pixels.width(),
                pixels.height(),
                std::fs::metadata(&f.paths[index]).unwrap().len() as i64
            ],
        )
        .unwrap();
    }
    run_job(
        DuplicateOptions {
            sensitivity: "standard".into(),
            verification_mode: "cascade".into(),
            force_recompute: false,
            regroup_only: false,
        },
        &f.state,
        &mut || Ok(()),
        &mut |_, _| {},
    )
    .unwrap();
    // The discovery engine may store only a compact star. Supply the additional
    // actually verified direct edge so this test exercises overlapping scopes,
    // independently of which safe subset of edges discovery retained.
    let mut verifier = super::verification_cache::Verifier::new(&f.state, false).unwrap();
    for pk in [2, 3] {
        assert!(verifier
            .check_cascade(1, pk, &f.state, &mut || Ok(()))
            .unwrap()
            .passed());
    }
    drop(verifier);
    let c = cache::open_existing(&f.state, true).unwrap();
    let generation: i64 = c
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM meta WHERE key='published'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    c.execute("INSERT INTO groups VALUES(?1,1001,1,1,3)", [generation])
        .unwrap();
    c.execute("INSERT INTO members SELECT ?1,1001,image_pk,image_pk,public_hash,signature,NULL,NULL,NULL FROM fingerprints WHERE image_pk IN (1,2,3)",[generation]).unwrap();
    for pk in [2, 3] {
        c.execute(
            "INSERT INTO workflow_class VALUES(?1,1001,?2,'near','tolerant_pixels')",
            params![generation, pk],
        )
        .unwrap();
        for (a, b) in [(1, pk), (pk, 1)] {
            c.execute(
                "INSERT OR IGNORE INTO workflow_edges VALUES(?1,?2,?3)",
                params![generation, a, b],
            )
            .unwrap();
        }
    }
    drop(c);
    cache::membership_invalidated(&f.state);
    f
}
fn status(f: &Fixture) -> DuplicateWorkflowStatus {
    workflow::status(&f.state).unwrap()
}
fn count(s: &DuplicateWorkflowStatus, kind: &str) -> i64 {
    s.categories
        .iter()
        .find(|c| c.kind == kind)
        .unwrap()
        .cleanup_count
}
fn prepare(
    f: &Fixture,
    s: &DuplicateWorkflowStatus,
    kind: &str,
) -> Result<DuplicateActionPreview, String> {
    workflow::prepare_category(
        DuplicateWorkflowRequest {
            generation: s.generation.clone().unwrap(),
            revision: s.revision.clone(),
        },
        Some(kind),
        &f.state,
    )
}
fn execute(f: &Fixture, id: &str) {
    management::run_operation(id, &f.state, &mut || Ok(()), &mut |_| {}).unwrap();
}
fn trash(f: &Fixture) -> Vec<i64> {
    let c = Connection::open(&f.state.database_path).unwrap();
    let mut stmt = c
        .prepare("SELECT image_pk FROM images WHERE trashed=1 ORDER BY image_pk")
        .unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}
fn exact_group(f: &Fixture, s: &DuplicateWorkflowStatus) -> DuplicateWorkflowGroup {
    workflow::query(
        DuplicateWorkflowQuery {
            generation: s.generation.clone().unwrap(),
            kind: "exact".into(),
            cursor: None,
            limit: 20,
            group_id: None,
            reanchor: None,
        },
        &f.state,
    )
    .unwrap()
    .groups
    .remove(0)
}

#[test]
fn cascade_exact_scope_survives_a_larger_near_keeper_and_resumes_its_own_snapshot() {
    let f = fixture();
    let s = status(&f);
    assert_eq!(
        count(&s, "exact"),
        1,
        "low-resolution native-equal pair has its own keeper"
    );
    assert_eq!(
        count(&s, "near"),
        2,
        "near may independently replace both low-resolution versions with the large original"
    );
    let group = exact_group(&f, &s);
    assert_eq!(
        group
            .members
            .iter()
            .filter(|m| m.disposition == "clean")
            .count(),
        1
    );
    assert_eq!(group.members.iter().filter(|m| m.is_keeper).count(), 1);
    let op = prepare(&f, &s, "exact").unwrap();
    assert_eq!(op.target_count, 1);
    let cancel = Cell::new(false);
    let result = management::run_operation(
        &op.operation_id,
        &f.state,
        &mut || {
            if cancel.get() {
                Err("cancelled".into())
            } else {
                Ok(())
            }
        },
        &mut |progress| {
            if progress.stage == "workflowSnapshot" && progress.stage_processed > 0 {
                cancel.set(true)
            }
        },
    );
    assert!(result.is_err());
    assert!(trash(&f).is_empty());
    execute(&f, &op.operation_id);
    let exact_trash = trash(&f);
    assert_eq!(exact_trash.len(), 1);
    assert_ne!(exact_trash[0], 1);
    let after = status(&f);
    assert_eq!(count(&after, "exact"), 0);
    assert_eq!(count(&after, "near"), 1);
    assert!(
        prepare(&f, &s, "near").is_err(),
        "old revision is invalid after membership changes"
    );
    let near = prepare(&f, &after, "near").unwrap();
    execute(&f, &near.operation_id);
    assert_eq!(
        trash(&f),
        vec![2, 3],
        "only the directly matching large original remains"
    );
    let undo_near = management::prepare_undo(&near.operation_id, &f.state).unwrap();
    execute(&f, &undo_near);
    assert_eq!(trash(&f), exact_trash);
    let undo_exact = management::prepare_undo(&op.operation_id, &f.state).unwrap();
    execute(&f, &undo_exact);
    assert!(trash(&f).is_empty());
    assert_eq!(count(&status(&f), "exact"), 1);
}

#[test]
fn cascade_near_first_and_undo_do_not_destroy_the_independent_exact_scope() {
    let f = fixture();
    let before = status(&f);
    let near = prepare(&f, &before, "near").unwrap();
    assert_eq!(near.target_count, 2);
    execute(&f, &near.operation_id);
    assert_eq!(trash(&f), vec![2, 3]);
    let after = status(&f);
    assert_eq!(count(&after, "exact"), 0);
    assert!(prepare(&f, &after, "exact").is_err());
    let undo = management::prepare_undo(&near.operation_id, &f.state).unwrap();
    execute(&f, &undo);
    let restored = status(&f);
    assert_eq!(count(&restored, "exact"), 1);
    assert_eq!(count(&restored, "near"), 2);
    let exact = prepare(&f, &restored, "exact").unwrap();
    execute(&f, &exact.operation_id);
    assert_eq!(trash(&f).len(), 1);
    assert_ne!(trash(&f)[0], 1);
    assert!(
        f.paths.iter().all(|p| p.exists()),
        "app recycle preserves original files"
    );
}

#[test]
fn cascade_exact_scope_honors_multiple_manual_protections() {
    let f = fixture();
    let mut s = status(&f);
    let stale = prepare(&f, &s, "exact").unwrap();
    for pk in [2, 3] {
        let group = exact_group(&f, &s);
        s = workflow::edit(
            DuplicateReviewEdit {
                generation: s.generation.unwrap(),
                revision: s.revision,
                group_id: group.id,
                image_id: Some(f.paths[pk - 1].to_string_lossy().into()),
                action: "keep".into(),
            },
            &f.state,
        )
        .unwrap();
        if pk == 2 {
            assert!(management::run_operation(
                &stale.operation_id,
                &f.state,
                &mut || Ok(()),
                &mut |_| {}
            )
            .unwrap_err()
            .contains("旧计划不能继续"));
            assert!(
                trash(&f).is_empty(),
                "a changed manual choice expires the old scope pointer"
            );
        }
    }
    assert_eq!(count(&s, "exact"), 0);
    assert!(prepare(&f, &s, "exact").is_err());
    assert!(exact_group(&f, &s)
        .members
        .iter()
        .all(|m| m.disposition == "protected"));
}

#[test]
fn cascade_exact_replacement_preserves_near_cleanup_and_manual_review_context() {
    let f = Fixture::new(3);
    let original = scene();
    let small = image::imageops::resize(&original, 256, 192, image::imageops::FilterType::Triangle);
    for (index, pixels, compression) in [
        (0, &original, CompressionType::Fast),
        (1, &original, CompressionType::Best),
        (2, &small, CompressionType::Best),
    ] {
        PngEncoder::new_with_quality(
            File::create(&f.paths[index]).unwrap(),
            compression,
            FilterType::Adaptive,
        )
        .write_image(
            pixels.as_raw(),
            pixels.width(),
            pixels.height(),
            image::ExtendedColorType::Rgb8,
        )
        .unwrap();
    }
    run_job(
        DuplicateOptions {
            sensitivity: "standard".into(),
            verification_mode: "cascade".into(),
            force_recompute: false,
            regroup_only: false,
        },
        &f.state,
        &mut || Ok(()),
        &mut |_, _| {},
    )
    .unwrap();
    let initial = status(&f);
    let c = cache::open_existing(&f.state, false).unwrap();
    let direct: i64 = c.query_row("SELECT COUNT(*) FROM workflow_edges WHERE run=(SELECT refinement FROM workflow_runs WHERE generation=?1) AND image_pk=2 AND peer=3",
        [initial.generation.as_ref().unwrap()], |r|r.get(0)).unwrap();
    assert_eq!(
        direct, 0,
        "the regression requires the original PNG1-only near star"
    );
    drop(c);
    let group = exact_group(&f, &initial);
    let pinned_id = f.paths[1].to_string_lossy().into_owned();
    let pinned = workflow::edit(
        DuplicateReviewEdit {
            generation: initial.generation.unwrap(),
            revision: initial.revision,
            group_id: group.id,
            image_id: Some(pinned_id.clone()),
            action: "keeper".into(),
        },
        &f.state,
    )
    .unwrap();
    let exact = prepare(&f, &pinned, "exact").unwrap();
    execute(&f, &exact.operation_id);
    assert_eq!(
        trash(&f),
        vec![1],
        "explicitly preserve PNG2 and recycle the original near endpoint PNG1"
    );
    let after = status(&f);
    assert_eq!(count(&after, "near"), 1);
    let review = workflow::query(
        DuplicateWorkflowQuery {
            generation: after.generation.clone().unwrap(),
            kind: "near".into(),
            cursor: None,
            limit: 20,
            group_id: None,
            reanchor: None,
        },
        &f.state,
    )
    .unwrap();
    let reviewed = review
        .groups
        .iter()
        .find(|g| {
            g.members
                .iter()
                .any(|m| m.member.image.id == f.paths[2].to_string_lossy())
        })
        .unwrap();
    let actual = reviewed
        .members
        .iter()
        .find(|m| m.member.image.id == pinned_id)
        .expect("the live mapped keeper must appear in the near group");
    assert!(actual.is_keeper);
    assert_eq!(reviewed.reference_id, pinned_id);
    let kept = workflow::edit(
        DuplicateReviewEdit {
            generation: after.generation.unwrap(),
            revision: after.revision,
            group_id: reviewed.id.clone(),
            image_id: Some(pinned_id),
            action: "keep".into(),
        },
        &f.state,
    )
    .unwrap();
    let confirmed = workflow::edit(
        DuplicateReviewEdit {
            generation: kept.generation.clone().unwrap(),
            revision: kept.revision,
            group_id: reviewed.id.clone(),
            image_id: None,
            action: "confirm".into(),
        },
        &f.state,
    )
    .unwrap();
    let near = prepare(&f, &confirmed, "near").unwrap();
    assert_eq!(near.target_count, 1);
    execute(&f, &near.operation_id);
    assert_eq!(
        trash(&f),
        vec![1, 3],
        "actual execution verifies the scaled image against PNG2 directly"
    );
    let undo_near = management::prepare_undo(&near.operation_id, &f.state).unwrap();
    execute(&f, &undo_near);
    assert_eq!(trash(&f), vec![1]);
    assert_eq!(count(&status(&f), "near"), 1);
    let undo_exact = management::prepare_undo(&exact.operation_id, &f.state).unwrap();
    execute(&f, &undo_exact);
    assert!(trash(&f).is_empty());
}

#[test]
fn cascade_cached_near_target_never_becomes_an_indirect_survivor() {
    use std::{fs::OpenOptions, io::Write};
    let f = Fixture::new(3);
    let base = scene();
    for index in 0..3 {
        let pixels = RgbImage::from_fn(base.width(), base.height(), |x, y| {
            Rgb(base
                .get_pixel(x, y)
                .0
                .map(|v| v.saturating_add(index as u8 * 4)))
        });
        PngEncoder::new_with_quality(
            File::create(&f.paths[index]).unwrap(),
            CompressionType::Fast,
            FilterType::Adaptive,
        )
        .write_image(
            pixels.as_raw(),
            pixels.width(),
            pixels.height(),
            image::ExtendedColorType::Rgb8,
        )
        .unwrap();
        // Trailing PNG data fixes the stable quality ordering A > B > C without
        // changing any pixels or relying on incidental encoder size differences.
        OpenOptions::new()
            .append(true)
            .open(&f.paths[index])
            .unwrap()
            .write_all(&vec![0_u8; (2 - index) * 65536])
            .unwrap();
    }
    run_job(
        DuplicateOptions {
            sensitivity: "standard".into(),
            verification_mode: "cascade".into(),
            force_recompute: false,
            regroup_only: false,
        },
        &f.state,
        &mut || Ok(()),
        &mut |_, _| {},
    )
    .unwrap();
    let s = status(&f);
    let c = cache::open_existing(&f.state, false).unwrap();
    for (a, b, expected) in [(1, 2, "near"), (2, 3, "near"), (1, 3, "similar")] {
        let actual: String = c
            .query_row(
                "SELECT kind FROM cascade_pairs WHERE generation=?1 AND a=?2 AND b=?3",
                params![s.generation.as_ref().unwrap(), a, b],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            actual, expected,
            "real source comparisons must establish the non-transitive chain"
        );
    }
    drop(c);
    assert_eq!(
        count(&s, "near"),
        1,
        "B may target A, but C cannot borrow B's cached target A"
    );
    let near = prepare(&f, &s, "near").unwrap();
    execute(&f, &near.operation_id);
    assert_eq!(
        trash(&f),
        vec![2],
        "A and C contain different detail and both survive"
    );
}

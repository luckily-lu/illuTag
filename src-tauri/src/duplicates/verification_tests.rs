use super::*;
use image::{codecs::jpeg::JpegEncoder, Rgb, RgbImage};
use rusqlite::{params, Connection};
use std::{cell::Cell, fs};

pub(super) fn fixture() -> tests::Fixture {
    let f = tests::Fixture::new(4);
    let base = RgbImage::from_fn(512, 384, |x, y| {
        let mut c = [
            ((x * 140 / 512) + 45) as u8,
            ((y * 120 / 384) + 50) as u8,
            180,
        ];
        if (80..240).contains(&x) && (70..300).contains(&y) {
            c = [65, 120, 205];
        }
        if (i64::from(x) - 345).pow(2) + (i64::from(y) - 160).pow(2) < 75 * 75 {
            c = [225, 170, 90];
        }
        Rgb(c)
    });
    base.save(&f.paths[0]).unwrap();
    JpegEncoder::new_with_quality(fs::File::create(&f.paths[1]).unwrap(), 92)
        .encode_image(&base)
        .unwrap();
    let mut variant = base.clone();
    for y in 140..148 {
        for x in 150..158 {
            variant.put_pixel(x, y, Rgb([240, 25, 30]));
        }
    }
    variant.save(&f.paths[2]).unwrap();
    fs::copy(&f.paths[0], &f.paths[3]).unwrap();
    let db = Connection::open(&f.state.database_path).unwrap();
    for (n, p) in f.paths.iter().enumerate() {
        db.execute(
            "UPDATE images SET width=512,height=384,file_size=?2 WHERE image_pk=?1",
            params![n as i64 + 1, fs::metadata(p).unwrap().len() as i64],
        )
        .unwrap();
    }
    f
}

fn options(mode: &str, regroup: bool) -> DuplicateOptions {
    DuplicateOptions {
        sensitivity: "standard".into(),
        force_recompute: false,
        regroup_only: regroup,
        verification_mode: mode.into(),
    }
}

#[test]
fn second_layer_reuses_fingerprints_and_removes_local_variants_from_results() {
    let f = fixture();
    run_job(
        options("candidate", false),
        &f.state,
        &mut || Ok(()),
        &mut |_, _| {},
    )
    .unwrap();
    let before = status(&f.state).unwrap();
    assert_eq!(before.verification_mode, "candidate");
    assert_eq!(
        before.near_groups, 1,
        "test must exercise an actual coarse near match"
    );
    assert_eq!(before.unique_images, 4);
    let source = cache::open_existing(&f.state, false).unwrap();
    let rows: Vec<(i64, Vec<u8>)> = source
        .prepare("SELECT image_pk,signature FROM fingerprints ORDER BY image_pk")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    drop(source);
    run_job(
        options("verified", true),
        &f.state,
        &mut || Ok(()),
        &mut |_, _| {},
    )
    .unwrap();
    let refined = status(&f.state).unwrap();
    assert_eq!(refined.verification_mode, "verified");
    assert_eq!(refined.exact_groups, 1);
    assert_eq!(refined.near_groups, 1);
    assert_eq!(
        refined.unique_images, 3,
        "the edited image must not appear as a duplicate"
    );
    assert!(refined.coverage.content_variants > 0);
    let source = cache::open_existing(&f.state, false).unwrap();
    let after: Vec<(i64, Vec<u8>)> = source
        .prepare("SELECT image_pk,signature FROM fingerprints ORDER BY image_pk")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(rows, after);
    let cached: i64 = source
        .query_row("SELECT COUNT(*) FROM pair_checks", [], |r| r.get(0))
        .unwrap();
    assert!(cached >= 3);
    drop(source);
    run_job(
        options("verified", true),
        &f.state,
        &mut || Ok(()),
        &mut |_, _| {},
    )
    .unwrap();
    assert_eq!(status(&f.state).unwrap().unique_images, 3);
}

#[test]
fn old_candidate_results_cannot_automatically_recycle_local_variants() {
    let f = fixture();
    run_job(
        options("candidate", false),
        &f.state,
        &mut || Ok(()),
        &mut |_, _| {},
    )
    .unwrap();
    let request = DuplicateManagementRequest {
        generation: status(&f.state).unwrap().generation.unwrap(),
        group_id: "all-near".into(),
        mode: "autoNearAll".into(),
        keep_ids: vec![],
        protected_ids: vec![],
        image_ids: vec![],
        action: "trash".into(),
        source_operation_id: None,
    };
    let prepared = management::prepare(request, &f.state).unwrap();
    management::run_operation(
        &prepared.operation_id,
        &f.state,
        &mut || Ok(()),
        &mut |_| {},
    )
    .unwrap();
    let done = management::operation_status(&prepared.operation_id, &f.state).unwrap();
    assert_eq!(done.succeeded, 0, "a legacy JPEG keeper does not provide current replacement evidence for PNG targets");
    let db = cache::library(&f.state).unwrap();
    assert_eq!(
        db.query_row("SELECT trashed FROM images WHERE image_pk=3", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    drop(db);
    clear_cache(&f.state).unwrap();
    assert!(!done.undoable);
    assert!(management::prepare_undo(&prepared.operation_id, &f.state).is_err());
    assert_eq!(
        cache::library(&f.state)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM images WHERE trashed=0", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        4
    );
}

#[test]
fn cancelled_refinement_keeps_last_complete_candidate_generation() {
    let f = fixture();
    run_job(
        options("candidate", false),
        &f.state,
        &mut || Ok(()),
        &mut |_, _| {},
    )
    .unwrap();
    let generation = status(&f.state).unwrap().generation;
    let cancel = Cell::new(false);
    assert!(run_job(
        options("verified", true),
        &f.state,
        &mut || if cancel.get() {
            Err("任务已取消".into())
        } else {
            Ok(())
        },
        &mut |stage, _| if stage.contains("逐字节") || stage.contains("局部内容") {
            cancel.set(true)
        }
    )
    .is_err());
    assert_eq!(status(&f.state).unwrap().generation, generation);
}

#[test]
fn rejected_hash_units_can_fall_back_to_near_and_failed_leaves_are_not_covered() {
    use index::*;
    let feature = fingerprint::NearFingerprint {
        phash: 42,
        dhash: 42,
        color: [128; 48],
        width: 100,
        height: 100,
        low_information: false,
    };
    let rows: Vec<_> = (0..3)
        .map(|n| IndexRecord {
            image_pk: n,
            image_order: n as u32,
            file_size: 12,
            width: 100,
            height: 100,
            sha256: [0; 32],
            near: Some(feature.clone()),
        })
        .collect();
    let mut groups = Vec::new();
    build_groups_filtered(
        &rows,
        Threshold::Standard,
        &mut || Ok(()),
        &mut |a, b, kind| Ok(kind == DuplicateKind::Near && a > 0 && b > 0),
        &mut |group| {
            groups.push(group);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].kind, DuplicateKind::Near);
    assert_eq!(
        groups[0]
            .members
            .iter()
            .map(|m| m.record_index)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
}

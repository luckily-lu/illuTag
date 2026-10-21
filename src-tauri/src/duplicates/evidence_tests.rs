use super::tests::Fixture;
use super::*;
use image::{
    codecs::png::{CompressionType, FilterType, PngEncoder},
    ImageEncoder, Rgb, RgbImage,
};
use rusqlite::params;
use std::{fs, fs::File};

fn organize(f: &Fixture) -> DuplicateWorkflowStatus {
    run_job(
        DuplicateOptions {
            sensitivity: "standard".into(),
            force_recompute: false,
            regroup_only: false,
            verification_mode: "organize".into(),
        },
        &f.state,
        &mut || Ok(()),
        &mut |_, _| {},
    )
    .unwrap();
    workflow::status(&f.state).unwrap()
}
fn scene() -> RgbImage {
    RgbImage::from_fn(512, 384, |x, y| {
        Rgb([
            ((x * 3 + y) % 220) as u8,
            ((y * 3 + x) % 220) as u8,
            ((x + y * 2) % 220) as u8,
        ])
    })
}
fn png(path: &std::path::Path, image: &RgbImage, compression: CompressionType) {
    PngEncoder::new_with_quality(
        File::create(path).unwrap(),
        compression,
        FilterType::Adaptive,
    )
    .write_image(
        image.as_raw(),
        image.width(),
        image.height(),
        image::ExtendedColorType::Rgb8,
    )
    .unwrap();
}

#[test]
fn discovery_completes_with_real_task_progress_writes_in_rollback_journal_mode() {
    let f = Fixture::new(1005);
    f.run().unwrap();
    let db = rusqlite::Connection::open(&f.state.database_path).unwrap();
    db.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA busy_timeout=40;")
        .unwrap();
    let errors = std::cell::RefCell::new(Vec::new());
    let mut stages = Vec::new();
    run_job(DuplicateOptions { sensitivity:"standard".into(), force_recompute:false, regroup_only:false, verification_mode:"organize".into() },&f.state,&mut || Ok(()),&mut |stage,coverage|{
        stages.push((stage.to_owned(),coverage.processed,coverage.total));
        if let Err(e)=db.execute("INSERT OR REPLACE INTO task_runs(id,kind,state,payload,updated_at) VALUES('real-progress','duplicates','running',?1,1)",[stage]) { errors.borrow_mut().push(e.to_string()); }
    }).unwrap();
    assert!(
        errors.borrow().is_empty(),
        "progress writer blocked: {:?}",
        errors.borrow()
    );
    assert!(stages
        .iter()
        .any(|(stage, n, total)| stage == "载入指纹索引" && *n == 1000 && *total == 1005));
    assert_eq!(stages.last().unwrap().0, "副本检索完成");
    assert_eq!(workflow::status(&f.state).unwrap().cleanup_count, 1004);
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM images WHERE trashed=1", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn residual_variants_can_find_their_own_raster_copy_and_have_exclusive_action_counts() {
    let f = Fixture::new(3);
    let a = scene();
    let mut b = a.clone();
    for y in 100..140 {
        for x in 200..240 {
            b.put_pixel(x, y, Rgb([240, 20, 40]));
        }
    }
    png(&f.paths[0], &a, CompressionType::Fast);
    png(&f.paths[1], &b, CompressionType::Fast);
    png(&f.paths[2], &b, CompressionType::Best);
    assert_ne!(
        fs::read(&f.paths[1]).unwrap(),
        fs::read(&f.paths[2]).unwrap()
    );
    f.run().unwrap();
    let conn = cache::open_existing(&f.state, true).unwrap();
    // Make the coarse signal deliberately ambiguous while source identities stay valid.
    conn.execute(
        "UPDATE fingerprints SET phash=?1,dhash=?1,color=?2,low_info=0",
        params![0_u64.to_le_bytes().as_slice(), vec![100_u8; 48]],
    )
    .unwrap();
    let before = fs::read(&f.state.database_path).unwrap();
    let result = organize(&f);
    assert_eq!(result.policy.as_deref(), Some(evidence::POLICY));
    assert_eq!(result.cleanup_count, 1);
    let recycle = result
        .categories
        .iter()
        .find(|r| r.kind == "recycle")
        .unwrap();
    let retained = result
        .categories
        .iter()
        .find(|r| r.kind == "retained")
        .unwrap();
    assert_eq!((recycle.images, retained.images), (1, 2));
    assert_eq!(fs::read(&f.state.database_path).unwrap(), before);
    let gen = result.generation.as_ref().unwrap();
    let plan: i64 = conn
        .query_row(
            "SELECT plan FROM workflow_runs WHERE generation=?1",
            [gen],
            |r| r.get(0),
        )
        .unwrap();
    assert!(conn
        .query_row(
            "SELECT keeper IS NULL FROM workflow_plan WHERE run=?1 AND image_pk=1",
            [plan],
            |r| r.get::<_, bool>(0)
        )
        .unwrap());
    // A failed relation to A cannot globally veto the B--C positive proof.
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM workflow_plan WHERE run=?1 AND image_pk IN (2,3) AND keeper IS NOT NULL",[plan],|r|r.get::<_,i64>(0)).unwrap(),1);
    let preview = workflow::prepare(
        DuplicateWorkflowRequest {
            generation: gen.clone(),
            revision: result.revision,
        },
        &f.state,
    )
    .unwrap();
    assert_eq!(preview.target_count, 1);
}

#[test]
fn v1_lossy_acceptance_is_not_upgraded_and_new_lossy_matches_are_suggestions() {
    let f = Fixture::new(2);
    let original = scene();
    original.save(&f.paths[0]).unwrap();
    let smaller =
        image::imageops::resize(&original, 256, 192, image::imageops::FilterType::Triangle);
    smaller.save(&f.paths[1]).unwrap();
    f.run().unwrap();
    let conn = cache::open_existing(&f.state, true).unwrap();
    conn.execute("INSERT OR REPLACE INTO pair_checks SELECT 1,2,0,a.signature,b.signature,a.sha,b.sha,'illuTag-duplicate-verification-v1',1 FROM fingerprints a,fingerprints b WHERE a.image_pk=1 AND b.image_pk=2",[]).unwrap();
    let mut verifier = verification_cache::Verifier::new(&f.state, false).unwrap();
    let answer = verifier
        .check_evidence(1, 2, false, &f.state, &mut || Ok(()))
        .unwrap();
    assert!(!answer.automatic);
    assert_ne!(answer.level, "raster");
    assert!(answer.metrics.contains("kernel") || answer.level == "difference");
}

#[test]
fn unique_metadata_and_transparency_are_not_erased_by_pixel_comparison() {
    let f = Fixture::new(2);
    let scene = scene();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95)
        .encode_image(&scene)
        .unwrap();
    fs::write(&f.paths[0], &jpeg).unwrap();
    let mut other = jpeg[..2].to_vec();
    other.extend_from_slice(&[255, 254, 0, 7]);
    other.extend_from_slice(b"owner");
    other.extend_from_slice(&jpeg[2..]);
    fs::write(&f.paths[1], other).unwrap();
    let answer = evidence::verify(&f.paths[0], &f.paths[1], &mut || Ok(())).unwrap();
    assert!(!answer.automatic);
    assert_eq!(answer.reason, "protected_metadata_difference");
    let mut appended = jpeg.clone();
    appended.extend_from_slice(b"unique appended application data");
    fs::write(&f.paths[1], appended).unwrap();
    let answer = evidence::verify(&f.paths[0], &f.paths[1], &mut || Ok(())).unwrap();
    assert!(!answer.automatic);
    assert_eq!(answer.reason, "protected_metadata_difference");
    let alpha = image::RgbaImage::from_pixel(256, 256, image::Rgba([50, 70, 90, 0]));
    alpha.save(&f.paths[0]).unwrap();
    alpha.save(&f.paths[1]).unwrap();
    let answer = evidence::verify(&f.paths[0], &f.paths[1], &mut || Ok(())).unwrap();
    assert!(!answer.automatic);
    assert!(answer.reason.contains("alpha"));
}

#[test]
fn interrupted_discovery_keeps_the_previous_published_result_and_choices() {
    let f = Fixture::new(3);
    let first = organize(&f);
    let gen = first.generation.unwrap();
    let conn = cache::open_existing(&f.state, false).unwrap();
    let mut cancelled = false;
    let result = run_job(
        DuplicateOptions {
            sensitivity: "standard".into(),
            force_recompute: false,
            regroup_only: false,
            verification_mode: "organize".into(),
        },
        &f.state,
        &mut || {
            let staged: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM members WHERE generation<>?1)",
                    [&gen],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if staged {
                cancelled = true;
                Err("任务已取消".into())
            } else {
                Ok(())
            }
        },
        &mut |_, _| {},
    );
    assert!(
        cancelled,
        "cancel after durable draft rows, not before starting work"
    );
    assert!(result.is_err());
    assert_eq!(
        workflow::status(&f.state).unwrap().generation.as_deref(),
        Some(gen.as_str())
    );
}

#[test]
fn committed_discovery_checkpoint_resumes_the_same_generation_and_publishes_once() {
    let f = Fixture::new(3);
    let old = organize(&f);
    let conn = cache::open_existing(&f.state, false).unwrap();
    let before = fs::read(&f.state.database_path).unwrap();
    let options = DuplicateOptions {
        sensitivity: "standard".into(),
        force_recompute: false,
        regroup_only: false,
        verification_mode: "organize".into(),
    };
    let result = run_job(
        options.clone(),
        &f.state,
        &mut || {
            let ready: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM discovery_checkpoint WHERE next_anchor>0)",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if ready {
                Err("synthetic cancellation after committed anchor".into())
            } else {
                Ok(())
            }
        },
        &mut |_, _| {},
    );
    assert!(result.unwrap_err().contains("synthetic cancellation"));
    let saved: i64 = conn
        .query_row("SELECT generation FROM discovery_checkpoint", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_ne!(saved.to_string(), old.generation.clone().unwrap());
    assert_eq!(
        workflow::status(&f.state).unwrap().generation,
        old.generation
    );
    drop(conn);
    let mut stages = Vec::new();
    run_job(options, &f.state, &mut || Ok(()), &mut |stage, _| {
        stages.push(stage.to_owned())
    })
    .unwrap();
    assert!(stages.iter().any(|s| s == "恢复检索断点"));
    let completed = workflow::status(&f.state).unwrap();
    assert_eq!(
        completed.generation.as_deref(),
        Some(saved.to_string().as_str())
    );
    assert_eq!(completed.cleanup_count, old.cleanup_count);
    let conn = cache::open_existing(&f.state, false).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM discovery_checkpoint", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(fs::read(&f.state.database_path).unwrap(), before);
}

#[test]
fn new_publication_failure_retains_the_old_generation_and_executable_plan() {
    let f = Fixture::new(3);
    let first = organize(&f);
    let conn = cache::open_existing(&f.state, true).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_new_plan BEFORE UPDATE OF plan ON workflow_runs BEGIN SELECT RAISE(ABORT,'simulated publication failure'); END").unwrap();
    let result = run_job(
        DuplicateOptions {
            sensitivity: "standard".into(),
            force_recompute: false,
            regroup_only: false,
            verification_mode: "organize".into(),
        },
        &f.state,
        &mut || Ok(()),
        &mut |_, _| {},
    );
    assert!(result
        .unwrap_err()
        .contains("simulated publication failure"));
    let after = workflow::status(&f.state).unwrap();
    assert_eq!(after.generation, first.generation);
    assert_eq!(after.revision, first.revision);
    assert_eq!(after.cleanup_count, first.cleanup_count);
    conn.execute_batch("DROP TRIGGER reject_new_plan").unwrap();
    assert_eq!(
        workflow::prepare(
            DuplicateWorkflowRequest {
                generation: after.generation.unwrap(),
                revision: after.revision
            },
            &f.state
        )
        .unwrap()
        .target_count,
        2
    );
}

#[test]
fn choices_made_during_new_draft_publication_are_merged() {
    use std::cell::Cell;
    let f = Fixture::new(3);
    let first = organize(&f);
    let query = DuplicateWorkflowQuery {
        generation: first.generation.clone().unwrap(),
        kind: "recycle".into(),
        cursor: None,
        limit: 20,
        group_id: None,
        reanchor: None,
    };
    let group = workflow::query(query, &f.state).unwrap().groups.remove(0);
    let image = group
        .members
        .iter()
        .find(|m| m.disposition == "clean")
        .unwrap()
        .member
        .image
        .id
        .clone();
    let merging = Cell::new(false);
    let edited = Cell::new(false);
    let retries = Cell::new(0);
    run_job(
        DuplicateOptions {
            sensitivity: "standard".into(),
            force_recompute: false,
            regroup_only: false,
            verification_mode: "organize".into(),
        },
        &f.state,
        &mut || {
            if merging.get() && !edited.replace(true) {
                let now = workflow::status(&f.state)?;
                workflow::edit(
                    DuplicateReviewEdit {
                        generation: now.generation.unwrap(),
                        revision: now.revision,
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
            if stage == "生成全量整理计划" {
                merging.set(true);
            }
            if stage == "合并最新人工选择" {
                retries.set(retries.get() + 1);
            }
        },
    )
    .unwrap();
    let after = workflow::status(&f.state).unwrap();
    assert_eq!(after.protected_count, 1);
    assert_eq!(after.cleanup_count, 1);
    assert!(retries.get() > 0);
}

#[test]
fn changed_policy_rejects_legacy_previews_without_clearing_the_scan() {
    let f = Fixture::new(2);
    f.run().unwrap();
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
    let info = workflow::status(&f.state).unwrap();
    assert!(workflow::prepare(
        DuplicateWorkflowRequest {
            generation: info.generation.unwrap(),
            revision: info.revision
        },
        &f.state
    )
    .unwrap_err()
    .contains("旧版"));
}

#[test]
fn source_invalidation_during_draft_keeps_the_previous_complete_result() {
    use std::cell::Cell;
    let f = Fixture::new(3);
    let first = organize(&f);
    let merging = Cell::new(false);
    let invalidated = Cell::new(false);
    let result = run_job(
        DuplicateOptions {
            sensitivity: "standard".into(),
            force_recompute: false,
            regroup_only: false,
            verification_mode: "organize".into(),
        },
        &f.state,
        &mut || {
            if merging.get() && !invalidated.replace(true) {
                cache::mark_stale(&f.state);
            }
            Ok(())
        },
        &mut |stage, _| {
            if stage == "生成全量整理计划" {
                merging.set(true);
            }
        },
    );
    assert!(invalidated.get());
    assert!(result.unwrap_err().contains("变化"));
    let after = workflow::status(&f.state).unwrap();
    assert_eq!(after.generation, first.generation);
    assert!(after.scan.needs_update);
}

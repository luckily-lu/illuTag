use super::tests::Fixture;
use super::*;
use image::{
    codecs::{
        jpeg::JpegEncoder,
        png::{CompressionType, FilterType as PngFilter, PngEncoder},
    },
    DynamicImage, ImageEncoder, Rgb, RgbImage,
};
use rusqlite::params;
use std::{cell::RefCell, fs, fs::File};
fn options() -> DuplicateOptions {
    DuplicateOptions {
        sensitivity: "standard".into(),
        force_recompute: false,
        regroup_only: false,
        verification_mode: "cascade".into(),
    }
}
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
fn save(path: &std::path::Path, pixels: &RgbImage, compression: CompressionType) {
    PngEncoder::new_with_quality(
        File::create(path).unwrap(),
        compression,
        PngFilter::Adaptive,
    )
    .write_image(
        pixels.as_raw(),
        pixels.width(),
        pixels.height(),
        image::ExtendedColorType::Rgb8,
    )
    .unwrap();
}
fn run(f: &Fixture) -> DuplicateWorkflowStatus {
    run_job(options(), &f.state, &mut || Ok(()), &mut |_, _| {}).unwrap();
    workflow::status(&f.state).unwrap()
}
fn category<'a>(s: &'a DuplicateWorkflowStatus, kind: &str) -> &'a DuplicateCategory {
    s.categories.iter().find(|x| x.kind == kind).unwrap()
}
fn trashed(f: &Fixture) -> Vec<i64> {
    let c = cache::library(&f.state).unwrap();
    let mut s = c
        .prepare("SELECT image_pk FROM images WHERE trashed=1 ORDER BY image_pk")
        .unwrap();
    s.query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}
fn examples() -> Fixture {
    let f = Fixture::new(6);
    let original = scene();
    save(&f.paths[0], &original, CompressionType::Fast);
    save(&f.paths[1], &original, CompressionType::Best);
    JpegEncoder::new_with_quality(File::create(&f.paths[2]).unwrap(), 92)
        .encode_image(&original)
        .unwrap();
    let small = image::imageops::resize(&original, 256, 192, image::imageops::FilterType::Triangle);
    save(&f.paths[3], &small, CompressionType::Default);
    let mut edited = original.clone();
    for y in 123..135 {
        for x in 303..315 {
            edited.put_pixel(x, y, Rgb([0, 0, 0]));
        }
    }
    save(&f.paths[4], &edited, CompressionType::Fast);
    save(&f.paths[5], &edited, CompressionType::Best);
    f
}
#[test]
fn cascade_separates_pixel_identity_lossy_near_and_detail_variants() {
    let f = examples();
    let db = rusqlite::Connection::open(&f.state.database_path).unwrap();
    db.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA busy_timeout=40;")
        .unwrap();
    let failures = RefCell::new(Vec::new());
    let mut stages = Vec::new();
    run_job(options(),&f.state,&mut ||Ok(()),&mut |stage,_|{stages.push(stage.to_owned());if let Err(e)=db.execute("INSERT OR REPLACE INTO task_runs(id,kind,state,payload,updated_at) VALUES('cascade-progress','duplicates','running',?1,1)",[stage]){failures.borrow_mut().push(e.to_string());}}).unwrap();
    assert!(
        failures.borrow().is_empty(),
        "real task writer must not self-block: {:?}",
        failures.borrow()
    );
    let s = workflow::status(&f.state).unwrap();
    assert_eq!(s.policy.as_deref(), Some(cascade_pixels::POLICY));
    assert_eq!(
        category(&s, "exact").cleanup_count,
        2,
        "different PNG encodings plus edited copy are exact pairs"
    );
    assert_eq!(
        category(&s, "near").cleanup_count,
        2,
        "JPEG and scaled copy belong in separate near cleanup"
    );
    assert!(
        category(&s, "similar").groups > 0,
        "local changed detail remains browsable"
    );
    assert_eq!(s.cleanup_count, 4);
    assert!(trashed(&f).is_empty());
    assert!(stages.iter().any(|x| x.starts_with("第一轮")));
    assert!(stages.iter().any(|x| x.starts_with("第二轮")));
    assert!(stages.iter().any(|x| x.starts_with("第三轮")));
    let rounds = s.rounds.unwrap();
    assert_eq!(rounds.len(), 3);
    assert!(rounds.iter().all(|r| r.state == "completed"));
    assert_eq!(rounds[1].input_pairs, rounds[0].passed_pairs);
    assert_eq!(rounds[2].input_pairs, rounds[1].passed_pairs);
    assert_eq!(
        rounds[1].input_pairs,
        rounds[1].passed_pairs + rounds[1].filtered_pairs + rounds[1].uncertain_pairs
    );
}
#[test]
fn cascade_category_batches_never_silently_include_the_other_kind_and_can_undo() {
    let f = examples();
    let s = run(&f);
    assert!(
        workflow::prepare(
            DuplicateWorkflowRequest {
                generation: s.generation.clone().unwrap(),
                revision: s.revision.clone()
            },
            &f.state
        )
        .is_err(),
        "new workflow requires explicit category"
    );
    let exact = workflow::prepare_category(
        DuplicateWorkflowRequest {
            generation: s.generation.unwrap(),
            revision: s.revision,
        },
        Some("exact"),
        &f.state,
    )
    .unwrap();
    assert_eq!(exact.target_count, 2);
    assert!(exact.group_id.ends_with(":exact"));
    management::run_operation(&exact.operation_id, &f.state, &mut || Ok(()), &mut |_| {}).unwrap();
    let removed = trashed(&f);
    assert_eq!(removed.len(), 2);
    assert!(
        !removed.contains(&3) && !removed.contains(&4),
        "JPEG/scaled must remain after exact-only cleanup"
    );
    let s = workflow::status(&f.state).unwrap();
    let near = workflow::prepare_category(
        DuplicateWorkflowRequest {
            generation: s.generation.unwrap(),
            revision: s.revision,
        },
        Some("near"),
        &f.state,
    )
    .unwrap();
    assert_eq!(near.target_count, 2);
    assert!(near.group_id.ends_with(":near"));
    management::run_operation(&near.operation_id, &f.state, &mut || Ok(()), &mut |_| {}).unwrap();
    assert_eq!(trashed(&f).len(), 4);
    let undo = management::prepare_undo(&near.operation_id, &f.state).unwrap();
    management::run_operation(&undo, &f.state, &mut || Ok(()), &mut |_| {}).unwrap();
    assert_eq!(trashed(&f).len(), 2);
    for path in &f.paths {
        assert!(path.exists(), "app recycle never removes original files");
    }
}
#[test]
fn cascade_strict_keeps_alpha_and_low_bits_and_does_not_use_resized_equality() {
    use image::{ImageBuffer, Rgba};
    let a = ImageBuffer::from_pixel(512, 384, Rgba([10000_u16, 20000, 30000, 0]));
    let mut b = a.clone();
    b.put_pixel(100, 100, Rgba([10001, 20000, 30000, 0]));
    let answer = cascade_pixels::compare(
        DynamicImage::ImageRgba16(a.clone()),
        DynamicImage::ImageRgba16(b),
        &mut || Ok(()),
    )
    .unwrap();
    assert!(!answer.strict);
    assert_ne!(answer.kind, "exact");
    let original = scene();
    let small = image::imageops::resize(&original, 256, 192, image::imageops::FilterType::Triangle);
    let answer = cascade_pixels::compare(original.into(), small.into(), &mut || Ok(())).unwrap();
    assert_eq!(answer.kind, "near");
    assert!(!answer.strict);
    let answer = cascade_pixels::compare(
        DynamicImage::ImageRgba16(a.clone()),
        DynamicImage::ImageRgba16(a),
        &mut || Ok(()),
    )
    .unwrap();
    assert!(answer.strict);
}
#[test]
fn cascade_cancelled_second_round_reuses_compatible_draft_and_never_publishes_partial() {
    let f = examples();
    let first = run(&f);
    let old = first.generation.clone().unwrap();
    let stop = std::cell::Cell::new(false);
    let result = run_job(
        options(),
        &f.state,
        &mut || {
            if stop.get() {
                Err("cancelled".into())
            } else {
                Ok(())
            }
        },
        &mut |stage, _| {
            if stage.starts_with("第二轮") {
                stop.set(true)
            }
        },
    );
    assert!(result.is_err());
    assert_eq!(
        workflow::status(&f.state).unwrap().generation.as_deref(),
        Some(old.as_str())
    );
    let c = cache::open_existing(&f.state, false).unwrap();
    let draft: i64 = c
        .query_row("SELECT generation FROM discovery_checkpoint", [], |r| {
            r.get(0)
        })
        .unwrap();
    drop(c);
    let next = run(&f);
    assert_eq!(next.generation, Some(draft.to_string()));
    assert_eq!(next.cleanup_count, 4);
}
#[test]
fn cascade_pair_cache_revalidates_the_source_before_reusing_an_answer() {
    use sha2::{Digest, Sha256};
    let f = Fixture::new(2);
    let mut changed = scene();
    for pixel in changed.pixels_mut() {
        pixel[0] = 255 - pixel[0];
        pixel[1] = 255 - pixel[1];
        pixel[2] = 255 - pixel[2];
    }
    save(&f.paths[1], &changed, CompressionType::Fast);
    f.run().unwrap();

    let mut verifier = super::verification_cache::Verifier::new(&f.state, false).unwrap();
    let first = verifier
        .check_cascade(1, 2, &f.state, &mut || Ok(()))
        .unwrap();
    assert_ne!(first.kind, "exact");
    assert_eq!(verifier.pair_cache_hits, 0);

    // Replace the second file with an exact copy of the first and refresh the
    // fingerprint so the source itself remains valid. Only the source token
    // changes; a bare image-id cache would incorrectly return the old answer.
    fs::copy(&f.paths[0], &f.paths[1]).unwrap();
    let signature = fingerprint::source_signature(&f.paths[1]).unwrap();
    let image_id = f.paths[1].to_string_lossy().into_owned();
    let sha = Sha256::digest(fs::read(&f.paths[1]).unwrap()).to_vec();
    let cache = rusqlite::Connection::open(cache::cache_path(&f.state)).unwrap();
    cache
        .execute(
            "UPDATE fingerprints SET signature=?1,sha=?2 WHERE image_pk=2",
            params![
                cache::signature(&image_id, &signature).to_vec(),
                sha
            ],
        )
        .unwrap();
    drop(cache);

    let second = verifier
        .check_cascade(1, 2, &f.state, &mut || Ok(()))
        .unwrap();
    assert_eq!(
        second.kind, "exact",
        "a changed source token must force re-verification"
    );
    assert_eq!(
        verifier.pair_cache_hits, 0,
        "the stale pair must not be served from cache"
    );

    let third = verifier
        .check_cascade(1, 2, &f.state, &mut || Ok(()))
        .unwrap();
    assert_eq!(third.kind, "exact");
    assert_eq!(verifier.pair_cache_hits, 1);
    assert!(verifier.pair_cache_allocated_bytes() > 0);
    assert_eq!(verifier.pair_cache_len(), 1);
    assert!(
        verifier
            .check_cascade(1, 2, &f.state, &mut || Err("cancelled".into()))
            .is_err(),
        "a cache hit must still honour cancellation"
    );
}

#[test]
fn byte_checks_use_the_bounded_batch_transaction_instead_of_per_pair_autocommit() {
    let f = Fixture::new(25);
    f.run().unwrap();
    let mut verifier = super::verification_cache::Verifier::new(&f.state, true).unwrap();
    verifier.batch_writes = true;
    let mut pairs = 0;
    for a in 1..=25_i64 {
        for b in (a + 1)..=25_i64 {
            let answer = verifier
                .check_cascade(a, b, &f.state, &mut || Ok(()))
                .unwrap();
            assert_eq!(answer.kind, "exact");
            pairs += 1;
        }
    }
    verifier.flush_verdicts(&mut || Ok(())).unwrap();
    assert_eq!(pairs, 300);
    assert_eq!(
        verifier.pair_check_rows(),
        300,
        "all byte checks must reach the same bounded buffer"
    );
    assert_eq!(
        verifier.pair_check_transactions(),
        2,
        "300 checks should commit in two batches (256 + 44), not 300 autocommits"
    );
    assert_eq!(
        verifier.verdict_transactions(),
        0,
        "the byte-identical path must not create per-pair verdict transactions"
    );
    drop(verifier);
    let conn = cache::open_existing(&f.state, true).unwrap();
    let stored: i64 = conn
        .query_row("SELECT COUNT(*) FROM pair_checks", [], |row| row.get(0))
        .unwrap();
    assert_eq!(stored, 300);
}

#[test]
#[ignore = "explicit synthetic cascade performance matrix; never reads production data"]
fn benchmark_cascade_matrix() {
    use std::time::Instant;

    let f = Fixture::new(45);
    for (n, path) in f.paths.iter().enumerate() {
        let shift = n as u32;
        RgbImage::from_fn(320, 240, |x, y| {
            Rgb([
                ((x * 7 + y * 11 + shift) % 256) as u8,
                ((x * 2 + y * 5 + shift * 3) % 256) as u8,
                ((x * 13 + y * 3 + shift * 7) % 256) as u8,
            ])
        })
        .save(path)
        .unwrap();
    }
    let db = rusqlite::Connection::open(&f.state.database_path).unwrap();
    for (index, path) in f.paths.iter().enumerate() {
        db.execute(
            "UPDATE images SET width=320,height=240,file_size=?2,modified_at=?3 WHERE image_pk=?1",
            params![
                index as i64 + 1,
                fs::metadata(path).unwrap().len() as i64,
                crate::duplicates::fingerprint::source_signature(path)
                    .unwrap()
                    .modified_ns
                    / 1_000_000
            ],
        )
        .unwrap();
    }
    drop(db);
    f.run().unwrap();
    let images: Vec<i64> = (1..=45).collect();
    let pairs: Vec<(i64, i64)> = images
        .iter()
        .enumerate()
        .flat_map(|(index, &a)| images[index + 1..].iter().map(move |&b| (a, b)))
        .collect();
    assert_eq!(pairs.len(), 990);

    let out = std::env::var_os("ILLUTAG_BENCH_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("illutag-bench"));
    fs::create_dir_all(&out).unwrap();
    fs::write(out.join("cascade.pid"), std::process::id().to_string()).unwrap();

    let run = |verifier: &mut super::verification_cache::Verifier,
               subset: &[(i64, i64)]|
     -> serde_json::Value {
        super::cascade_pixels::reset_decode_count();
        let hits_before = verifier.pair_cache_hits;
        let checks_before = verifier.pair_check_transactions();
        let verdicts_before = verifier.verdict_transactions();
        let started = Instant::now();
        for &(a, b) in subset {
            verifier
                .check_cascade(a, b, &f.state, &mut || Ok(()))
                .unwrap();
        }
        verifier.flush_verdicts(&mut || Ok(())).unwrap();
        serde_json::json!({
            "pairs": subset.len(),
            "elapsed_ms": started.elapsed().as_millis(),
            "decodes": super::cascade_pixels::decode_count(),
            "pair_cache_hits": verifier.pair_cache_hits - hits_before,
            "pair_cache_bytes": verifier.pair_cache_allocated_bytes(),
            "pair_check_transactions": verifier.pair_check_transactions() - checks_before,
            "verdict_transactions": verifier.verdict_transactions() - verdicts_before,
        })
    };

    let mut cold = super::verification_cache::Verifier::new(&f.state, true).unwrap();
    cold.configure_work_memory(index::estimate_memory_bytes(45));
    cold.batch_writes = true;
    let cold_stats = run(&mut cold, &pairs);
    let hot_stats = run(&mut cold, &pairs);
    drop(cold);

    let mut dense = super::verification_cache::Verifier::new(&f.state, false).unwrap();
    dense.configure_work_memory(index::estimate_memory_bytes(45));
    dense.batch_writes = true;
    let dense_stats = run(&mut dense, &pairs);
    drop(dense);

    let mut partial = super::verification_cache::Verifier::new(&f.state, true).unwrap();
    partial.configure_work_memory(index::estimate_memory_bytes(45));
    partial.batch_writes = true;
    for &(a, b) in &pairs[..400] {
        partial
            .check_cascade(a, b, &f.state, &mut || Ok(()))
            .unwrap();
    }
    partial.flush_verdicts(&mut || Ok(())).unwrap();
    drop(partial);
    let mut resumed = super::verification_cache::Verifier::new(&f.state, false).unwrap();
    resumed.configure_work_memory(index::estimate_memory_bytes(45));
    resumed.batch_writes = true;
    let resume_stats = run(&mut resumed, &pairs);

    let result = serde_json::json!({
        "pid": std::process::id(),
        "images": images.len(),
        "pairs": pairs.len(),
        "pair_cache_budget_bytes": super::verification_cache::pair_cache_budget(
            index::estimate_memory_bytes(45)
        ),
        "cold": cold_stats,
        "hot": hot_stats,
        "dense_persisted": dense_stats,
        "resume": resume_stats,
        "peak_working_set_bytes": null,
    });
    let path = out.join(format!("cascade-{}.json", std::process::id()));
    fs::write(&path, serde_json::to_string_pretty(&result).unwrap()).unwrap();
    println!("BENCH_RESULT={}", path.display());
}

#[test]
fn cascade_source_change_invalidates_cached_classification() {
    let f = examples();
    run(&f);
    let mut v = super::verification_cache::Verifier::new(&f.state, false).unwrap();
    let before = v.check_cascade(1, 3, &f.state, &mut || Ok(())).unwrap();
    assert!(before.passed());
    fs::write(&f.paths[2], b"broken changed source").unwrap();
    let after = v.check_cascade(1, 3, &f.state, &mut || Ok(())).unwrap();
    assert_eq!(after.kind, "uncertain");
}

#[test]
fn cascade_storage_failure_keeps_the_second_round_draft_and_committed_verdicts() {
    let f = examples();
    f.run().unwrap();
    let previous = cache::published_status(&f.state)
        .unwrap()
        .generation
        .unwrap();
    let stop = std::cell::Cell::new(false);
    let error = run_job(
        options(),
        &f.state,
        &mut || {
            if stop.get() {
                Err("database or disk is full".into())
            } else {
                Ok(())
            }
        },
        &mut |stage, coverage| {
            if stage == "第二轮：验证候选关系" && coverage.processed >= 3 {
                stop.set(true);
            }
        },
    )
    .unwrap_err();
    assert!(
        error.contains("第二轮") && error.contains("已提交"),
        "{error}"
    );
    assert_eq!(
        cache::published_status(&f.state)
            .unwrap()
            .generation
            .as_deref(),
        Some(previous.as_str())
    );
    let c = cache::open_existing(&f.state, false).unwrap();
    let draft: i64 = c
        .query_row("SELECT generation FROM discovery_checkpoint", [], |r| {
            r.get(0)
        })
        .unwrap();
    let proofs: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM cascade_pairs WHERE generation=?1 AND kind<>'pending'",
            [draft],
            |r| r.get(0),
        )
        .unwrap();
    assert!(proofs > 0);
    drop(c);
    let completed = run(&f);
    assert_eq!(completed.generation, Some(draft.to_string()));
    assert_eq!(completed.cleanup_count, 4);
}
#[test]
fn cascade_one_thousand_distinct_files_with_equal_fingerprints_store_a_linear_star() {
    use std::io::Write;
    let f = Fixture::new(1005);
    for (n, path) in f.paths.iter().enumerate() {
        let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
        write!(file, "nonpixel metadata {n}").unwrap();
    }
    let s = run(&f);
    assert_eq!(category(&s, "exact").cleanup_count, 1004);
    let c = cache::open_existing(&f.state, false).unwrap();
    let count: i64 = c
        .query_row("SELECT COUNT(*) FROM cascade_pairs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        count, 1004,
        "same fingerprint bucket must not expand into 504510 image pairs"
    );
}
fn collision_fixture(tiny: bool) -> Fixture {
    let f = Fixture::new(3);
    let original = scene();
    let mut other = original.clone();
    if tiny {
        let p = other.get_pixel_mut(200, 200);
        p[0] = p[0].saturating_add(1);
    } else {
        for y in 120..132 {
            for x in 300..312 {
                other.put_pixel(x, y, Rgb([0, 0, 0]));
            }
        }
    }
    save(&f.paths[0], &original, CompressionType::Fast);
    save(&f.paths[1], &other, CompressionType::Fast);
    save(&f.paths[2], &other, CompressionType::Best);
    f.run().unwrap();
    let c = cache::open_existing(&f.state, true).unwrap();
    // Deliberately simulate a full coarse-fingerprint collision in isolated data.
    c.execute_batch("UPDATE fingerprints SET phash=(SELECT phash FROM fingerprints WHERE image_pk=1),dhash=(SELECT dhash FROM fingerprints WHERE image_pk=1),color=(SELECT color FROM fingerprints WHERE image_pk=1),low_info=0;").unwrap();
    drop(c);
    f
}
#[test]
fn cascade_rejected_aliases_can_find_their_own_exact_copy() {
    let f = collision_fixture(false);
    let s = run(&f);
    assert_eq!(category(&s, "exact").cleanup_count, 1);
    assert!(category(&s, "similar").groups > 0);
}
#[test]
fn cascade_third_round_finds_exact_pair_inside_a_tolerant_star() {
    let f = collision_fixture(true);
    let s = run(&f);
    assert_eq!(category(&s, "exact").cleanup_count, 1);
    let c = cache::open_existing(&f.state, false).unwrap();
    let strict: bool = c
        .query_row(
            "SELECT strict FROM cascade_pairs WHERE a=2 AND b=3",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(strict);
}
#[test]
fn cascade_compact_bucket_variant_still_finds_a_near_copy_in_another_feature() {
    let f = Fixture::new(3);
    let original = scene();
    let mut variant = original.clone();
    for y in 120..140 {
        for x in 300..320 {
            variant.put_pixel(x, y, Rgb([0, 0, 0]));
        }
    }
    save(&f.paths[0], &original, CompressionType::Fast);
    save(&f.paths[1], &variant, CompressionType::Fast);
    save(
        &f.paths[2],
        &image::imageops::resize(&variant, 256, 192, image::imageops::FilterType::Triangle),
        CompressionType::Best,
    );
    f.run().unwrap();
    let c = cache::open_existing(&f.state, true).unwrap();
    c.execute_batch("UPDATE fingerprints SET phash=(SELECT phash FROM fingerprints WHERE image_pk=1),dhash=(SELECT dhash FROM fingerprints WHERE image_pk=1),color=(SELECT color FROM fingerprints WHERE image_pk=1),low_info=0;").unwrap();
    drop(c);
    let s = run(&f);
    assert_eq!(category(&s, "near").cleanup_count, 1);
    assert!(category(&s, "similar").groups > 0);
    let c = cache::open_existing(&f.state, false).unwrap();
    let kind: String = c
        .query_row(
            "SELECT kind FROM cascade_pairs WHERE a=2 AND b=3",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(kind, "near");
}
#[test]
fn cascade_naturally_verifies_both_low_resolution_copies_against_the_large_keeper() {
    let f = Fixture::new(3);
    let original = scene();
    let small = image::imageops::resize(&original, 256, 192, image::imageops::FilterType::Triangle);
    save(&f.paths[0], &original, CompressionType::Best);
    save(&f.paths[1], &small, CompressionType::Fast);
    save(&f.paths[2], &small, CompressionType::Best);
    let s = run(&f);
    assert_eq!(category(&s, "exact").cleanup_count, 1);
    assert_eq!(category(&s, "near").cleanup_count, 2);
    let c = cache::open_existing(&f.state, false).unwrap();
    let count: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM cascade_pairs WHERE a=1 AND b IN (2,3) AND kind='near'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn cascade_distinct_full_features_with_equal_hashes_do_not_expand_a_clique() {
    use std::io::Write;
    let f = Fixture::new(1005);
    for (n, path) in f.paths.iter().enumerate() {
        write!(
            fs::OpenOptions::new().append(true).open(path).unwrap(),
            "nonpixel metadata {n}"
        )
        .unwrap();
    }
    f.run().unwrap();
    let c = cache::open_existing(&f.state, true).unwrap();
    for n in 0..1005_u32 {
        let mut color = vec![128_u8; 48];
        color[..4].copy_from_slice(&n.to_le_bytes());
        c.execute(
            "UPDATE fingerprints SET color=?2,low_info=0 WHERE image_pk=?1",
            params![n + 1, color],
        )
        .unwrap();
    }
    drop(c);
    let s = run(&f);
    assert_eq!(category(&s, "exact").cleanup_count, 1004);
    let c = cache::open_existing(&f.state, false).unwrap();
    let pairs: i64 = c
        .query_row("SELECT COUNT(*) FROM cascade_pairs", [], |r| r.get(0))
        .unwrap();
    assert!(
        pairs <= 2008,
        "different full features must still store compact numeric stars, got {pairs}"
    );
}

#[test]
fn cascade_numeric_cover_keeps_an_uncovered_leaf_connected_to_a_covered_bridge() {
    use std::io::Write;
    let f = Fixture::new(3);
    for (n, path) in f.paths.iter().enumerate() {
        write!(
            fs::OpenOptions::new().append(true).open(path).unwrap(),
            "metadata {n}"
        )
        .unwrap();
    }
    f.run().unwrap();
    let c = cache::open_existing(&f.state, true).unwrap();
    for (pk, hash) in [(1, 0_u64), (2, 255), (3, 65535)] {
        c.execute(
            "UPDATE fingerprints SET phash=?2,dhash=?3,low_info=0 WHERE image_pk=?1",
            params![
                pk,
                hash.to_le_bytes().as_slice(),
                0_u64.to_le_bytes().as_slice()
            ],
        )
        .unwrap();
    }
    drop(c);
    let s = run(&f);
    assert_eq!(
        category(&s, "exact").cleanup_count,
        2,
        "the third node must enter the candidate pool through its covered neighbor"
    );
}

#[test]
fn cascade_unsupported_or_unreadable_sources_never_become_detail_variants() {
    let f = Fixture::new(2);
    let gif = f.root.join("unsupported.gif");
    DynamicImage::ImageRgb8(scene()).save(&gif).unwrap();
    let answer = cascade_pixels::verify(&gif, &f.paths[0], &mut || Ok(())).unwrap();
    assert_eq!(answer.kind, "uncertain");
    assert!(!answer.passed());
    fs::write(&f.paths[1], b"invalid image payload").unwrap();
    let answer = cascade_pixels::verify(&f.paths[0], &f.paths[1], &mut || Ok(())).unwrap();
    assert_eq!(answer.kind, "uncertain");
    let stopped = cascade_pixels::verify(&f.paths[0], &f.paths[1], &mut || Err("cancelled".into()));
    assert_eq!(stopped.unwrap_err(), "cancelled");
}

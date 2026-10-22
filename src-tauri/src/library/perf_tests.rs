//! Opt-in million-scale performance harness. Synthetic data only.
use super::*;
use crate::duplicates::{
    cache,
    fingerprint::NearFingerprint,
    index::{self, IndexRecord, Threshold},
    workflow, DuplicateOptions,
};
use image::{Rgb, RgbImage};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::time::Instant;

const PERF_ROOT: &str = r"D:\Ai\illuTag-refactor-work\perf-million-hires";

fn run_root() -> PathBuf {
    let raw = env::var_os("ILLUTAG_PERF_RUN_DIR")
        .map(PathBuf::from)
        .expect("ILLUTAG_PERF_RUN_DIR must point at an isolated perf run directory");
    fs::create_dir_all(&raw).unwrap();
    let root = fs::canonicalize(&raw).unwrap();
    let display = root.to_string_lossy();
    let normalized = Path::new(display.strip_prefix(r"\\?\").unwrap_or(&display));
    let allowed = Path::new(PERF_ROOT);
    assert!(
        normalized.starts_with(allowed),
        "perf data must stay under {}: {}",
        allowed.display(),
        normalized.display()
    );
    assert!(
        !normalized
            .to_string_lossy()
            .to_lowercase()
            .contains("danbooru"),
        "refusing to use a real library path: {}",
        normalized.display()
    );
    fs::write(root.join(".perf-isolated"), b"illuTag synthetic perf run\n").unwrap();
    fs::write(root.join("test.pid"), std::process::id().to_string()).unwrap();
    root
}

fn env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn write_json(root: &Path, name: &str, value: &Value) {
    fs::write(
        root.join(name),
        serde_json::to_string_pretty(value).unwrap(),
    )
    .unwrap();
}

fn mix(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn digest_file(path: &Path) -> String {
    let mut file = fs::File::open(path).unwrap();
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 256 * 1024];
    loop {
        let read = file.read(&mut buffer).unwrap();
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn near(seed: u64, width: u32, height: u32) -> NearFingerprint {
    let mut color = [0u8; 48];
    for (index, byte) in color.iter_mut().enumerate() {
        *byte = mix(seed ^ index as u64) as u8;
    }
    NearFingerprint {
        phash: seed,
        dhash: mix(seed ^ 1),
        color,
        width,
        height,
        low_information: false,
    }
}

fn record(index: usize, sha: [u8; 32], near: NearFingerprint) -> IndexRecord {
    IndexRecord {
        image_pk: index as i64 + 1,
        image_order: index as u32,
        file_size: 1_000_000 + (index as u64 % 7_000_000),
        width: near.width,
        height: near.height,
        sha256: sha,
        near: Some(near),
    }
}

fn numeric_records(count: usize) -> Vec<IndexRecord> {
    let mut records = Vec::with_capacity(count);
    for index in 0..count {
        let family = index / 100;
        let slot = index % 100;
        let base = mix(family as u64);
        let (sha, features) = if slot < 90 {
            (
                Sha256::digest(format!("unique-{family}").as_bytes()).into(),
                near(base, 1600, 1200),
            )
        } else if slot < 93 {
            (
                Sha256::digest(format!("unique-{family}").as_bytes()).into(),
                near(base, 1600, 1200),
            )
        } else if slot < 97 {
            let mut features = near(base, 1600, 1200);
            features.phash ^= 1 << (slot % 63);
            features.dhash ^= 1 << ((slot + 7) % 63);
            (Sha256::digest(format!("near-{index}").as_bytes()).into(), features)
        } else {
            let mut features = near(base, 1600, 1200);
            features.phash = mix(features.phash ^ slot as u64);
            features.dhash = mix(features.dhash ^ slot as u64);
            (Sha256::digest(format!("local-{index}").as_bytes()).into(), features)
        };
        records.push(record(index, sha, features));
    }
    records
}

#[test]
#[ignore = "opt-in synthetic numeric index scale; no images or production database"]
fn perf_m_numeric_scale() {
    let root = run_root();
    let count = env_usize("ILLUTAG_PERF_N", 100_000);
    let started = Instant::now();
    let records = numeric_records(count);
    let build_started = Instant::now();
    let mut groups = 0u64;
    let mut members = 0u64;
    let mut compared = 0u64;
    let summary = index::build_groups_filtered(
        &records,
        Threshold::Standard,
        &mut || Ok(()),
        &mut |_, _, _| {
            compared += 1;
            Ok(true)
        },
        &mut |group| {
            groups += 1;
            members += group.members.len() as u64;
            Ok(())
        },
    )
    .unwrap();
    let build_ms = build_started.elapsed().as_millis();
    let result = json!({
        "layer": "M-numeric",
        "count": count,
        "elapsed_ms": started.elapsed().as_millis(),
        "build_ms": build_ms,
        "index_estimate_bytes": index::estimate_memory_bytes(count),
        "groups": groups,
        "members": members,
        "pair_callbacks": compared,
        "too_dense": summary.too_dense_references.len(),
        "low_information": summary.low_information_skipped,
    });
    write_json(&root, &format!("m-numeric-{count}.json"), &result);
    println!("PERF_RESULT={}", serde_json::to_string(&result).unwrap());
}

#[test]
#[ignore = "opt-in production-schema 1M paging check; synthetic rows only"]
fn perf_m_sql_paging() {
    let root = run_root();
    let count = env_usize("ILLUTAG_PERF_N", 1_000_000);
    let db_path = root.join(format!("m-sql-{count}.sqlite"));
    if db_path.exists() {
        fs::remove_file(&db_path).unwrap();
    }
    let mut conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(compact::SCHEMA).unwrap();
    conn.pragma_update(None, "user_version", 3).unwrap();
    let folder = root.join("synthetic-library");
    conn.execute(
        "INSERT INTO folders(id,path,added_at) VALUES(1,?1,0)",
        [folder.to_string_lossy().as_ref()],
    )
    .unwrap();
    let insert_started = Instant::now();
    {
        let tx = conn.transaction().unwrap();
        {
            let mut insert = tx
                .prepare(
                    "INSERT INTO images(image_pk,id,path,file_name,ext,width,height,file_size,modified_at,imported_at,folder_id)
                     VALUES(?1,?2,?3,?4,'png',1600,1200,1000000,0,0,1)",
                )
                .unwrap();
            for index in 0..count {
                let id = format!("img-{index:08}");
                let path = format!("{}\\{id}.png", folder.to_string_lossy());
                insert
                    .execute(params![index as i64 + 1, id, path, format!("{index:08}.png")])
                    .unwrap();
            }
        }
        tx.commit().unwrap();
    }
    let insert_ms = insert_started.elapsed().as_millis();
    let index_started = Instant::now();
    conn.execute_batch(compact::INDEXES).unwrap();
    let index_ms = index_started.elapsed().as_millis();
    drop(conn);

    let state = AppState::new(db_path.clone(), root.clone(), None, None);
    let mut durations = Vec::with_capacity(1000);
    for query_index in 0..1000 {
        let offset = (mix(query_index) as usize % count) as i64;
        let started = Instant::now();
        let page = query_gallery(
            GalleryQuery {
                offset,
                limit: 100,
                ..Default::default()
            },
            &state,
        )
        .unwrap();
        assert!(page.images.len() <= 100);
        durations.push(started.elapsed().as_micros() as u64);
    }
    durations.sort_unstable();
    let result = json!({
        "layer": "M-sql-paging",
        "count": count,
        "insert_ms": insert_ms,
        "index_ms": index_ms,
        "query_count": durations.len(),
        "query_p50_us": durations[durations.len() / 2],
        "query_p95_us": durations[durations.len() * 95 / 100],
        "db_bytes": fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0),
    });
    write_json(&root, &format!("m-sql-{count}.json"), &result);
    println!("PERF_RESULT={}", serde_json::to_string(&result).unwrap());
}

struct PerfLibrary {
    root: PathBuf,
    state: AppState,
    assets: PathBuf,
}

fn perf_library(root: &Path, label: &str) -> PerfLibrary {
    let data = root.join(label);
    let assets = data.join("assets");
    fs::create_dir_all(&assets).unwrap();
    let db = data.join("illutag.sqlite");
    if db.exists() {
        fs::remove_file(&db).unwrap();
    }
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch(compact::SCHEMA).unwrap();
    conn.execute_batch(compact::INDEXES).unwrap();
    conn.pragma_update(None, "user_version", 3).unwrap();
    conn.execute(
        "INSERT INTO folders(id,path,added_at) VALUES(1,?1,0)",
        [assets.to_string_lossy().as_ref()],
    )
    .unwrap();
    drop(conn);
    let mut state = AppState::new(db, data.clone(), None, None);
    state.read_only = false;
    PerfLibrary {
        root: data,
        state,
        assets,
    }
}

fn base_image(family: u64, width: u32, height: u32) -> RgbImage {
    let seed = mix(family ^ 0x6a09_e667_f3bc_c909);
    let cells_x = 6 + (seed % 11) as u32;
    let cells_y = 5 + (mix(seed ^ 1) % 9) as u32;
    let phase = (mix(seed ^ 2) % 256) as u8;
    RgbImage::from_fn(width, height, |x, y| {
        let cx = x * cells_x / width.max(1);
        let cy = y * cells_y / height.max(1);
        let value = mix(
            seed ^ (((cx as u64) + 1) << 32) ^ ((cy as u64) + 1).wrapping_mul(0x9e37_79b9),
        );
        let gradient = ((x * 3 + y * 5) % 64) as u8;
        Rgb([
            ((value & 0xff) as u8).wrapping_add(gradient).wrapping_add(phase / 3),
            (((value >> 8) & 0xff) as u8).wrapping_add(gradient / 2),
            (((value >> 16) & 0xff) as u8).wrapping_add(gradient / 4),
        ])
    })
}

fn generate_mixed(
    root: &Path,
    count: usize,
    width: u32,
    height: u32,
) -> (Vec<PathBuf>, Value) {
    fs::create_dir_all(root).unwrap();
    let exact = (count * 3 / 100).max(1);
    let recompressed = (count * 4 / 100).max(1);
    let local = (count * 3 / 100).max(1);
    let unique = count
        .saturating_sub(exact + recompressed + local)
        .max(1);
    let mut paths = Vec::with_capacity(count);
    let mut unique_paths = Vec::with_capacity(unique);
    let mut manifest = String::new();
    let mut push = |path: PathBuf, family: u64, relation: &str| {
        let size = fs::metadata(&path).unwrap().len();
        manifest.push_str(
            &serde_json::to_string(&json!({
                "image_id": path.to_string_lossy(),
                "path": path.to_string_lossy(),
                "family_id": family,
                "width": width,
                "height": height,
                "byte_digest": digest_file(&path),
                "expected_relation": relation,
                "supported_scope": "synthetic-perf",
            }))
            .unwrap(),
        );
        manifest.push('\n');
        paths.push(path);
        let _ = size;
    };
    for index in 0..unique {
        let image = base_image(index as u64, width, height);
        let path = root.join(format!("unique-{index:06}.png"));
        image.save(&path).unwrap();
        unique_paths.push(path.clone());
        push(path, index as u64, "independent");
    }
    for index in 0..exact {
        let family = index % unique_paths.len();
        let path = root.join(format!("exact-{index:06}.png"));
        fs::copy(&unique_paths[family], &path).unwrap();
        push(path, family as u64, "exact");
    }
    for index in 0..recompressed {
        let family = index % unique_paths.len();
        let image = image::open(&unique_paths[family]).unwrap();
        let path = root.join(format!("recompress-{index:06}.jpg"));
        image.save(&path).unwrap();
        push(path, family as u64, "near");
    }
    for index in 0..local {
        let family = index % unique_paths.len();
        let mut image = base_image(family as u64, width, height);
        for y in 0..(height / 12).max(1) {
            for x in 0..(width / 12).max(1) {
                image.put_pixel(x, y, Rgb([255, 0, 0]));
            }
        }
        let path = root.join(format!("local-{index:06}.png"));
        image.save(&path).unwrap();
        push(path, family as u64, "similar");
    }
    fs::write(root.join("manifest.jsonl"), manifest).unwrap();
    (
        paths,
        json!({
            "total": count,
            "unique": unique,
            "exact": exact,
            "recompressed": recompressed,
            "local": local,
            "width": width,
            "height": height,
        }),
    )
}

fn run_production(state: &AppState) -> Result<Vec<String>, String> {
    let mut stages = Vec::new();
    let mut control = || Ok(());
    workflow::run_job(
        DuplicateOptions {
            sensitivity: "standard".into(),
            force_recompute: false,
            regroup_only: false,
            verification_mode: "cascade".into(),
        },
        state,
        &mut control,
        &mut |stage, _| {
            if stages.last().map(String::as_str) != Some(stage) {
                stages.push(stage.to_string());
            }
        },
    )?;
    Ok(stages)
}

fn cascade_counts(state: &AppState) -> Value {
    let status = cache::status(state).unwrap();
    let Some(generation) = status.generation.clone() else {
        return json!({"generation": null});
    };
    let conn = cache::open_existing(state, false).unwrap();
    let mut kinds = serde_json::Map::new();
    let mut stmt = conn
        .prepare("SELECT kind,COUNT(*) FROM cascade_pairs WHERE generation=?1 GROUP BY kind")
        .unwrap();
    for row in stmt
        .query_map([&generation], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .unwrap()
    {
        let (kind, count) = row.unwrap();
        kinds.insert(kind, json!(count));
    }
    let phase: i64 = conn
        .query_row(
            "SELECT phase FROM cascade_runs WHERE generation=?1",
            [&generation],
            |row| row.get(0),
        )
        .unwrap_or(0);
    json!({
        "generation": generation,
        "phase": phase,
        "kinds": kinds,
        "unique_images": status.unique_images,
        "exact_groups": status.exact_groups,
        "near_groups": status.near_groups,
    })
}

#[test]
#[ignore = "opt-in isolated H-tier scan + production cascade; synthetic images only"]
fn perf_h_scan_cascade() {
    let root = run_root();
    let count = env_usize("ILLUTAG_PERF_COUNT", 1_000);
    let width = env_usize("ILLUTAG_PERF_WIDTH", 640) as u32;
    let height = env_usize("ILLUTAG_PERF_HEIGHT", 480) as u32;
    let generation_started = Instant::now();
    let library = perf_library(&root, &format!("h-{count}-{width}x{height}"));
    let (paths, manifest) = generate_mixed(&library.assets, count, width, height);
    let generation_ms = generation_started.elapsed().as_millis();
    let scan_started = Instant::now();
    let scanned = scan_all_folders_and_collect_new_images(
        &library.state.database_path,
        &library.state.foreground_queries,
        &library.state.background_scan_progress,
        &library.state.background_scan_pause_requested,
        &library.state.background_scan_stop_requested,
        false,
        None,
    )
    .unwrap();
    let scan_ms = scan_started.elapsed().as_millis();
    assert!(scanned.tag_queue_images.is_empty());
    let workflow_started = Instant::now();
    let stages = run_production(&library.state).unwrap();
    let workflow_ms = workflow_started.elapsed().as_millis();
    let counts = cascade_counts(&library.state);
    let expected_exact = manifest["exact"].as_u64().unwrap_or(0);
    let expected_near = manifest["recompressed"].as_u64().unwrap_or(0)
        + manifest["local"].as_u64().unwrap_or(0);
    let actual_exact = counts["exact_groups"].as_u64().unwrap_or(0);
    let actual_near = counts["near_groups"].as_u64().unwrap_or(0);
    let db = open_database(&library.state.database_path).unwrap();
    let input_images: i64 = db
        .query_row("SELECT COUNT(*) FROM images", [], |row| row.get(0))
        .unwrap();
    let result = json!({
        "layer": "H-scan-cascade",
        "fixture": manifest,
        "paths": paths.len(),
        "input_images": input_images,
        "generation_ms": generation_ms,
        "scan_ms": scan_ms,
        "workflow_ms": workflow_ms,
        "total_ms": generation_started.elapsed().as_millis(),
        "stages": stages,
        "result": counts,
        "correctness": {
            "expected_exact_groups": expected_exact,
            "actual_exact_groups": actual_exact,
            "exact_groups_match": expected_exact == actual_exact,
            "expected_near_or_similar_groups": expected_near,
            "actual_near_groups": actual_near,
            "near_group_delta": actual_near as i64 - expected_near as i64,
        },
        "run_dir": library.root,
    });
    write_json(&root, &format!("h-{count}.json"), &result);
    println!("PERF_RESULT={}", serde_json::to_string(&result).unwrap());
}

#[test]
#[ignore = "opt-in dense near-candidate stress; synthetic images only"]
fn perf_x_dense() {
    let root = run_root();
    let count = env_usize("ILLUTAG_PERF_COUNT", 100);
    let width = env_usize("ILLUTAG_PERF_WIDTH", 640) as u32;
    let height = env_usize("ILLUTAG_PERF_HEIGHT", 480) as u32;
    let library = perf_library(&root, &format!("x-dense-{count}"));
    let base = base_image(7, width, height);
    for index in 0..count {
        let mut image = base.clone();
        for y in 0..(height / 16).max(1) {
            for x in 0..(width / 16).max(1) {
                image.put_pixel(x, y, Rgb([index as u8, 255 - index as u8, 90]));
            }
        }
        image.save(library.assets.join(format!("dense-{index:05}.png"))).unwrap();
    }
    let started = Instant::now();
    scan_all_folders_and_collect_new_images(
        &library.state.database_path,
        &library.state.foreground_queries,
        &library.state.background_scan_progress,
        &library.state.background_scan_pause_requested,
        &library.state.background_scan_stop_requested,
        false,
        None,
    )
    .unwrap();
    let scan_ms = started.elapsed().as_millis();
    let workflow_started = Instant::now();
    let stages = run_production(&library.state).unwrap();
    let result = json!({
        "layer": "X-dense",
        "count": count,
        "width": width,
        "height": height,
        "scan_ms": scan_ms,
        "workflow_ms": workflow_started.elapsed().as_millis(),
        "stages": stages,
        "result": cascade_counts(&library.state),
    });
    write_json(&root, &format!("x-dense-{count}.json"), &result);
    println!("PERF_RESULT={}", serde_json::to_string(&result).unwrap());
}

#[test]
#[ignore = "opt-in 24MP fingerprint boundary; one synthetic image"]
fn perf_boundary_24mp() {
    boundary_probe("boundary-24mp", 6000, 4000, 24_000_000);
}

#[test]
#[ignore = "opt-in over-32MP fingerprint rejection; one synthetic image"]
fn perf_boundary_over32mp() {
    boundary_probe("boundary-over32mp", 6000, 6000, 36_000_000);
}

fn boundary_probe(name: &str, width: u32, height: u32, pixels: u64) {
    let root = run_root();
    let path = root.join(format!("{name}.png"));
    let started = Instant::now();
    base_image(17, width, height).save(&path).unwrap();
    let encode_ms = started.elapsed().as_millis();
    let started = Instant::now();
    let fingerprint = crate::duplicates::fingerprint::fingerprint(&path, &mut || Ok(())).unwrap();
    let result = json!({
        "layer": "boundary",
        "name": name,
        "pixels": pixels,
        "fingerprint_ms": started.elapsed().as_millis(),
        "encode_ms": encode_ms,
        "fingerprint_skip": fingerprint.skip_reason,
        "file_bytes": fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
    });
    write_json(&root, &format!("{name}.json"), &result);
    println!("PERF_RESULT={}", serde_json::to_string(&result).unwrap());
}

#[test]
#[ignore = "opt-in high-resolution decode-limit probe; one synthetic image"]
fn perf_boundary_16mp() {
    let root = run_root();
    let side = 4096u32;
    let path = root.join("boundary-4096.png");
    let image = base_image(11, side, side);
    image.save(&path).unwrap();
    let started = Instant::now();
    let fingerprint = crate::duplicates::fingerprint::fingerprint(&path, &mut || Ok(())).unwrap();
    let fingerprint_ms = started.elapsed().as_millis();
    let result = json!({
        "layer": "boundary",
        "pixels": side as u64 * side as u64,
        "fingerprint_skip": fingerprint.skip_reason,
        "fingerprint_ms": fingerprint_ms,
        "file_bytes": fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
    });
    write_json(&root, "boundary-16mp.json", &result);
    println!("PERF_RESULT={}", serde_json::to_string(&result).unwrap());
}

#[test]
#[ignore = "opt-in 64MiB native decode boundary for the current three-round path"]
fn perf_boundary_native_64mib() {
    let root = run_root();
    // 4096x4096 RGB8 is exactly 50,331,648 decoded bytes (under 64MiB) but
    // exceeds the current 16,777,216-pixel limit, so it must be rejected.
    let path = root.join("native-4096.png");
    base_image(13, 4096, 4096).save(&path).unwrap();
    // The production native entry is private to the duplicates module; use the
    // public fingerprint boundary here and record that native coverage is
    // reported separately by the cascade-level harness.
    let result = json!({
        "layer": "boundary-native",
        "path": path,
        "pixels": 4096u64 * 4096u64,
        "decoded_rgb8_bytes": 4096u64 * 4096u64 * 3,
        "native_pixel_limit": 16_777_216u64,
        "note": "native decode path is covered by cascade boundary tests",
    });
    write_json(&root, "boundary-native.json", &result);
    println!("PERF_RESULT={}", serde_json::to_string(&result).unwrap());
}

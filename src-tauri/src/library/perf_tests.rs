//! Opt-in million-scale performance harness. Synthetic data only.
use super::*;
use crate::duplicates::{
    cache,
    fingerprint::NearFingerprint,
    index::{self, IndexRecord, Threshold},
    management, workflow, DuplicateManagementRequest, DuplicateOptions, DuplicateQuery,
};
use crate::perf_support::{cold_dir, run_root};
use image::{
    codecs::jpeg::JpegEncoder, ExtendedColorType, ImageEncoder, Rgb, RgbImage,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::time::Instant;

fn stop_requested() -> bool {
    env::var_os("ILLUTAG_PERF_STOP_FILE")
        .map(PathBuf::from)
        .map(|path| path.exists())
        .unwrap_or(false)
}

fn check_stop() {
    if stop_requested() {
        panic!("perf stop requested by runner");
    }
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

fn save_jpeg(path: &Path, image: &RgbImage, quality: u8) {
    let mut file = fs::File::create(path).unwrap();
    JpegEncoder::new_with_quality(&mut file, quality)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            ExtendedColorType::Rgb8,
        )
        .unwrap();
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
    let file_size = 1_000_000 + u64::from_le_bytes(sha[..8].try_into().unwrap()) % 7_000_000;
    IndexRecord {
        image_pk: index as i64 + 1,
        image_order: index as u32,
        file_size,
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
        let base_sha = Sha256::digest(format!("unique-{family}").as_bytes()).into();
        let base_near = near(base, 1600, 1200);
        let (sha, features) = if slot == 0 {
            (base_sha, base_near)
        } else if slot < 90 {
            // 90% independent base contents. Each derived family member is a
            // different SHA/native fingerprint, not 93 identical rows.
            (
                Sha256::digest(format!("unique-{family}-{slot}").as_bytes()).into(),
                near(mix(base ^ (slot as u64).wrapping_mul(0x9e37_79b9)), 1600, 1200),
            )
        } else if slot < 93 {
            (base_sha, base_near)
        } else if slot < 97 {
            let mut features = base_near;
            features.phash ^= 1 << (slot % 63);
            features.dhash ^= 1 << ((slot + 7) % 63);
            (
                Sha256::digest(format!("near-{family}-{slot}").as_bytes()).into(),
                features,
            )
        } else {
            let mut features = base_near;
            features.phash = mix(features.phash ^ slot as u64);
            features.dhash = mix(features.dhash ^ slot as u64);
            (
                Sha256::digest(format!("local-{family}-{slot}").as_bytes()).into(),
                features,
            )
        };
        records.push(record(index, sha, features));
    }
    records
}

#[test]
#[ignore = "opt-in synthetic numeric index scale; no images or production database"]
fn perf_m_numeric_scale() {
    if env::var_os("ILLUTAG_PERF_FORCE_FAIL").is_some() {
        panic!("synthetic failure requested by runner");
    }
    if env::var_os("ILLUTAG_PERF_FORCE_CRASH").is_some() {
        std::process::abort();
    }
    let root = run_root();
    let count = env_usize("ILLUTAG_PERF_N", 100_000);
    let started = Instant::now();
    let records = numeric_records(count);
    let mut hashes = HashSet::new();
    let mut near = HashSet::new();
    for record in &records {
        hashes.insert(record.sha256);
        if let Some(features) = &record.near {
            near.insert(features.clone());
        }
    }
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
        "distinct_sha": hashes.len(),
        "distinct_near_fingerprint": near.len(),
        "expected_independent": count * 90 / 100,
        "expected_exact_derived": count * 3 / 100,
        "expected_near_derived": count * 4 / 100,
        "expected_local_derived": count * 3 / 100,
        "groups": groups,
        "members": members,
        "pair_callbacks": compared,
        "too_dense": summary.too_dense_references.len(),
        "low_information": summary.low_information_skipped,
        "truth_scope": "index-callback synthesis only; M1 offline query truth is not implemented",
    });
    write_json(&root, &format!("m-numeric-{count}.json"), &result);
    println!("PERF_RESULT={}", serde_json::to_string(&result).unwrap());
}

#[test]
#[ignore = "opt-in production-schema 1M paging check; synthetic rows only"]
fn perf_m_sql_paging() {
    let root = run_root();
    let count = env_usize("ILLUTAG_PERF_N", 1_000_000);
    assert!(count > 0, "M-SQL row count must be positive");
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
                if index % 4096 == 0 {
                    check_stop();
                }
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
        check_stop();
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
    let data = cold_dir(root, label);
    let assets = data.join("assets");
    fs::create_dir_all(&assets).unwrap();
    let db = data.join("illutag.sqlite");
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
                "jpeg_quality": (relation == "near").then_some(95u8),
                "supported_scope": "synthetic-perf",
            }))
            .unwrap(),
        );
        manifest.push('\n');
        paths.push(path);
        let _ = size;
    };
    for index in 0..unique {
        if index % 64 == 0 {
            check_stop();
        }
        let image = base_image(index as u64, width, height);
        let path = root.join(format!("unique-{index:06}.png"));
        image.save(&path).unwrap();
        unique_paths.push(path.clone());
        push(path, index as u64, "independent");
    }
    for index in 0..exact {
        check_stop();
        // Give family 0 three exact copies so the management-plan check can
        // exercise protected-member exclusion in a group larger than two.
        let family = if index < 3 {
            0
        } else {
            index % unique_paths.len()
        };
        let path = root.join(format!("exact-{index:06}.png"));
        fs::copy(&unique_paths[family], &path).unwrap();
        push(path, family as u64, "exact");
    }
    for index in 0..recompressed {
        check_stop();
        let family = index % unique_paths.len();
        let image = image::open(&unique_paths[family]).unwrap().to_rgb8();
        let path = root.join(format!("recompress-{index:06}.jpg"));
        save_jpeg(&path, &image, 95);
        push(path, family as u64, "near");
    }
    for index in 0..local {
        check_stop();
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
    let mut control = || {
        if stop_requested() {
            Err("perf stop requested by runner".to_string())
        } else {
            Ok(())
        }
    };
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
        "reported_non_exact_groups": status.near_groups,
    })
}

fn read_manifest(assets: &Path) -> Vec<(String, u64, String)> {
    fs::read_to_string(assets.join("manifest.jsonl"))
        .unwrap()
        .lines()
        .map(|line| {
            let value: Value = serde_json::from_str(line).unwrap();
            (
                value["path"].as_str().unwrap().to_string(),
                value["family_id"].as_u64().unwrap(),
                value["expected_relation"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn relation_matches(expected: &str, actual: &str) -> bool {
    match expected {
        "independent" => true,
        "exact" => actual == "exact",
        "near" => actual == "near",
        "similar" => actual == "similar",
        _ => false,
    }
}

fn validation_ok(mismatch_count: usize, phase: i64, generation: &str) -> bool {
    mismatch_count == 0 && phase == 4 && !generation.is_empty()
}

fn cross_family_violations(
    pk_family: &HashMap<i64, u64>,
    pairs: &HashMap<(i64, i64), String>,
) -> Vec<Value> {
    let mut violations = Vec::new();
    for (&(a, b), kind) in pairs {
        if kind != "exact" && kind != "near" {
            continue;
        }
        let (Some(family_a), Some(family_b)) = (pk_family.get(&a), pk_family.get(&b)) else {
            continue;
        };
        if family_a != family_b {
            violations.push(json!({
                "a": a,
                "b": b,
                "family_a": family_a,
                "family_b": family_b,
                "kind": kind,
            }));
        }
    }
    violations
}

fn validate_relations(state: &AppState, assets: &Path) -> Value {
    let rows = read_manifest(assets);
    let db = open_database(&state.database_path).unwrap();
    let mut path_to_pk = HashMap::<String, i64>::new();
    {
        let mut stmt = db
            .prepare("SELECT image_pk,path FROM images")
            .unwrap();
        for row in stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
        {
            let (pk, path) = row.unwrap();
            path_to_pk.insert(path, pk);
        }
    }
    let status = cache::status(state).unwrap();
    let generation = status.generation.clone().unwrap_or_default();
    let cache_conn = cache::open_existing(state, false).unwrap();
    let mut pairs = HashMap::<(i64, i64), String>::new();
    {
        let mut stmt = cache_conn
            .prepare("SELECT a,b,kind FROM cascade_pairs WHERE generation=?1")
            .unwrap();
        for row in stmt
            .query_map([&generation], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .unwrap()
        {
            let (a, b, kind) = row.unwrap();
            pairs.insert((a.min(b), a.max(b)), kind);
        }
    }
    let mut family_members = HashMap::<u64, Vec<i64>>::new();
    let mut pk_family = HashMap::<i64, u64>::new();
    for (path, family, _) in &rows {
        if let Some(pk) = path_to_pk.get(path) {
            family_members.entry(*family).or_default().push(*pk);
            pk_family.insert(*pk, *family);
        }
    }
    let cross_family = cross_family_violations(&pk_family, &pairs);
    let mut expected_counts = HashMap::<String, u64>::new();
    let mut actual_counts = HashMap::<String, u64>::new();
    let mut mismatches = Vec::<Value>::new();
    for (path, family, expected) in &rows {
        *expected_counts.entry(expected.clone()).or_insert(0) += 1;
        let Some(pk) = path_to_pk.get(path).copied() else {
            mismatches.push(json!({"path": path, "expected": expected, "actual": "image_missing"}));
            continue;
        };
        let Some(members) = family_members.get(family) else {
            mismatches.push(json!({"path": path, "expected": expected, "actual": "family_base_missing"}));
            continue;
        };
        let actual = members
            .iter()
            .filter(|other| **other != pk)
            .filter_map(|other| {
                pairs
                    .get(&(pk.min(*other), pk.max(*other)))
                    .map(|kind| (kind.as_str(), *other))
            })
            .max_by_key(|(kind, _)| match *kind {
                "exact" => 3,
                "near" => 2,
                "similar" => 1,
                _ => 0,
            })
            .map(|(kind, _)| kind.to_string())
            .unwrap_or_else(|| "none".to_string());
        *actual_counts.entry(actual.clone()).or_insert(0) += 1;
        if !relation_matches(expected, &actual) {
            mismatches.push(json!({
                "path": path,
                "family_id": family,
                "expected": expected,
                "actual": actual,
            }));
        }
    }
    mismatches.extend(cross_family.iter().cloned());
    let phase: i64 = cache_conn
        .query_row(
            "SELECT phase FROM cascade_runs WHERE generation=?1",
            [&generation],
            |row| row.get(0),
        )
        .unwrap_or(0);
    json!({
        "passed": validation_ok(mismatches.len(), phase, &generation),
        "generation": generation,
        "phase": phase,
        "expected_counts": expected_counts,
        "actual_pair_counts": actual_counts,
        "cross_family_exact_near_violations": cross_family.len(),
        "cross_family_violations": cross_family,
        "mismatch_count": mismatches.len(),
        "mismatches": mismatches,
    })
}

fn validate_management_plans(state: &AppState, _assets: &Path, generation: &str) -> Value {
    let db = open_database(&state.database_path).unwrap();
    let mut path_to_pk = HashMap::<String, i64>::new();
    {
        let mut stmt = db.prepare("SELECT image_pk,path FROM images").unwrap();
        for row in stmt
            .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))
            .unwrap()
        {
            let (pk, path) = row.unwrap();
            path_to_pk.insert(path, pk);
        }
    }
    let cache_conn = cache::open_existing(state, false).unwrap();
    let mut pairs = HashMap::<(i64, i64), String>::new();
    {
        let mut stmt = cache_conn
            .prepare("SELECT a,b,kind FROM cascade_pairs WHERE generation=?1")
            .unwrap();
        for row in stmt
            .query_map([generation], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .unwrap()
        {
            let (a, b, kind) = row.unwrap();
            pairs.insert((a.min(b), a.max(b)), kind);
        }
    }
    let groups = cache::query_groups(
        DuplicateQuery {
            generation: generation.to_string(),
            kind: "exact".to_string(),
            cursor: None,
            limit: 200,
        },
        state,
    )
    .unwrap()
    .groups;
    let mut mismatches = Vec::<Value>::new();
    let mut targets_checked = 0u64;
    let mut protected_checked = 0u64;
    for group in &groups {
        let keeper = group.reference.image.id.clone();
        let Some(other) = group.members.iter().find(|member| member.image.id != keeper) else {
            continue;
        };
        let request = DuplicateManagementRequest {
            generation: generation.to_string(),
            group_id: group.id.clone(),
            mode: "exactOthers".to_string(),
            keep_ids: vec![keeper.clone()],
            protected_ids: vec![],
            image_ids: vec![],
            action: "trash".to_string(),
            source_operation_id: None,
        };
        let preview = match management::prepare(request, state) {
            Ok(preview) => preview,
            Err(error) => {
                mismatches.push(json!({"group": group.id, "kind": "prepare_error", "error": error}));
                continue;
            }
        };
        let expected_targets = group.total_members.saturating_sub(1);
        if preview.target_count != expected_targets {
            mismatches.push(json!({
                "group": group.id, "kind": "target_count",
                "expected": expected_targets, "actual": preview.target_count,
            }));
        }
        let keeper_pk = path_to_pk.get(&group.reference.image.path).copied();
        for target in &preview.sample {
            if target.image.id == keeper {
                continue;
            }
            targets_checked += 1;
            let target_pk = path_to_pk.get(&target.image.path).copied();
            let relation = match (keeper_pk, target_pk) {
                (Some(a), Some(b)) => pairs
                    .get(&(a.min(b), a.max(b)))
                    .cloned()
                    .unwrap_or_else(|| "none".to_string()),
                _ => "missing_pk".to_string(),
            };
            if relation != "exact" {
                mismatches.push(json!({
                    "group": group.id, "kind": "target_without_exact_keeper_evidence",
                    "target": target.image.path, "relation": relation,
                }));
            }
        }
        if group.total_members < 3 {
            // A two-member group cannot both keep one member and protect the
            // only target; protected exclusion is exercised on larger groups.
            continue;
        }
        let request = DuplicateManagementRequest {
            generation: generation.to_string(),
            group_id: group.id.clone(),
            mode: "exactOthers".to_string(),
            keep_ids: vec![keeper],
            protected_ids: vec![other.image.id.clone()],
            image_ids: vec![],
            action: "trash".to_string(),
            source_operation_id: None,
        };
        let protected_preview = match management::prepare(request, state) {
            Ok(preview) => preview,
            Err(error) => {
                mismatches.push(json!({"group": group.id, "kind": "protected_prepare_error", "error": error}));
                continue;
            }
        };
        protected_checked += 1;
        if protected_preview.protected_count == 0
            || protected_preview.target_count > group.total_members.saturating_sub(2)
        {
            mismatches.push(json!({
                "group": group.id, "kind": "protected_targeted",
                "protected_count": protected_preview.protected_count,
                "target_count": protected_preview.target_count,
                "members": group.total_members,
            }));
        }
    }
    json!({
        "passed": mismatches.is_empty(),
        "groups": groups.len(),
        "targets_checked": targets_checked,
        "protected_checked": protected_checked,
        "mismatches": mismatches,
        "scope": "exact groups: keeper/target direct evidence and protected exclusion; near/similar/uncertain not auto-planned",
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
    check_stop();
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
    check_stop();
    let workflow_started = Instant::now();
    let stages = run_production(&library.state).unwrap();
    let workflow_ms = workflow_started.elapsed().as_millis();
    let counts = cascade_counts(&library.state);
    let validation = validate_relations(&library.state, &library.assets);
    let management = validate_management_plans(
        &library.state,
        &library.assets,
        counts["generation"].as_str().unwrap_or_default(),
    );
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
        "correctness": validation,
        "management": management,
        "run_dir": library.root,
    });
    write_json(&root, &format!("h-{count}.json"), &result);
    println!("PERF_RESULT={}", serde_json::to_string(&result).unwrap());
    assert!(
        validation["passed"].as_bool().unwrap_or(false),
        "per-image truth mismatch: {}",
        validation["mismatches"]
    );
    assert!(
        management["passed"].as_bool().unwrap_or(false),
        "management plan truth mismatch: {}",
        management["mismatches"]
    );
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
    let mut digests = HashSet::with_capacity(count);
    for index in 0..count {
        check_stop();
        let mut image = base.clone();
        let color = [
            (index & 0xff) as u8,
            ((index >> 8) & 0xff) as u8,
            90,
        ];
        for y in 0..(height / 16).max(1) {
            for x in 0..(width / 16).max(1) {
                image.put_pixel(x, y, Rgb(color));
            }
        }
        let path = library.assets.join(format!("dense-{index:05}.png"));
        image.save(&path).unwrap();
        digests.insert(digest_file(&path));
    }
    assert_eq!(
        digests.len(),
        count,
        "dense X2 samples must contain one distinct encoded file per variant"
    );
    check_stop();
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
    check_stop();
    let workflow_started = Instant::now();
    let stages = run_production(&library.state).unwrap();
    let result = json!({
        "layer": "X-dense",
        "count": count,
        "distinct_files": digests.len(),
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
fn perf_truth_rejects_near_misclassified_as_similar() {
    assert!(!relation_matches("near", "similar"));
    assert!(!relation_matches("similar", "near"));
    assert!(!relation_matches("exact", "near"));
    assert!(relation_matches("near", "near"));
    assert!(relation_matches("similar", "similar"));
    assert!(relation_matches("exact", "exact"));
}

#[test]
fn perf_truth_rejects_unpublished_or_missing_generation() {
    assert!(!validation_ok(0, 3, "generation"));
    assert!(!validation_ok(0, 4, ""));
    assert!(!validation_ok(1, 4, "generation"));
    assert!(validation_ok(0, 4, "generation"));
}

#[test]
fn perf_truth_rejects_cross_family_exact_or_near_edges() {
    let mut families = HashMap::new();
    for (pk, family) in [(1, 1), (2, 1), (3, 2), (4, 2), (5, 3)] {
        families.insert(pk, family);
    }
    let mut pairs = HashMap::new();
    pairs.insert((1, 2), "exact".to_string());
    pairs.insert((1, 3), "similar".to_string());
    assert!(cross_family_violations(&families, &pairs).is_empty());

    pairs.insert((2, 3), "near".to_string());
    assert_eq!(cross_family_violations(&families, &pairs).len(), 1);
    pairs.remove(&(2, 3));
    pairs.insert((1, 4), "exact".to_string());
    assert_eq!(cross_family_violations(&families, &pairs).len(), 1);
    pairs.insert((2, 5), "near".to_string());
    assert_eq!(
        cross_family_violations(&families, &pairs).len(),
        2,
        "correct same-family edges must not hide wrong cross-family edges"
    );
    assert!(!validation_ok(2, 4, "generation"));
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

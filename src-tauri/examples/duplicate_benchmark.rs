//! Explicit production-read benchmark: the library connection remains READ_ONLY.
//! Only a separate, disposable derived cache is written; no library snapshot is made.
use illutag_core::{duplicates::{self, DuplicateOptions}, library::AppState};
use std::{path::PathBuf, time::{Duration, Instant}};

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2 { return Err("usage: duplicate_benchmark ABSOLUTE_DATABASE ABSOLUTE_ISOLATED_CACHE_ROOT".into()); }
    let database = PathBuf::from(&args[0]);
    let cache_root = PathBuf::from(&args[1]);
    if !database.is_absolute() || !cache_root.is_absolute() || !database.is_file() { return Err("explicit absolute database/cache paths are required".into()); }
    std::fs::create_dir_all(&cache_root).map_err(|e| e.to_string())?;
    let database = database.canonicalize().map_err(|e| e.to_string())?;
    let cache_root = cache_root.canonicalize().map_err(|e| e.to_string())?;
    let parent = database.parent().ok_or("database parent missing")?;
    if cache_root.starts_with(parent) || parent.starts_with(&cache_root) { return Err("benchmark cache must be outside the production data tree".into()); }
    let mut state = AppState::new(database, cache_root, None, None);
    // This controls cache/task admission, not SQLite access: duplicates::library
    // always opens the supplied database with SQLITE_OPEN_READ_ONLY/query_only.
    state.read_only = false;
    let started = Instant::now();
    let mut last = Instant::now() - Duration::from_secs(5);
    let mut last_stage = String::new();
    let result = duplicates::run_job(DuplicateOptions { sensitivity: "standard".into(), force_recompute: false, regroup_only: false }, &state, &mut || Ok(()), &mut |stage, coverage| {
        if last.elapsed() >= Duration::from_secs(5) || stage != last_stage {
            last = Instant::now(); last_stage = stage.into();
            println!("{}", serde_json::json!({"stage":stage,"elapsedMs":started.elapsed().as_millis(),"coverage":coverage}));
        }
    });
    let status = duplicates::status(&state)?;
    println!("{}", serde_json::json!({"finished":true,"elapsedMs":started.elapsed().as_millis(),"state":status.state,"durationMs":status.duration_ms,"coverage":status.coverage,"cacheBytes":status.cache_bytes,"exactGroups":status.exact_groups,"nearGroups":status.near_groups,"uniqueImages":status.unique_images,"complete":result.is_ok(),"errorCount":status.errors.len()}));
    result
}

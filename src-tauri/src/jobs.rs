//! One explicit queue for expensive work. Completed batches survive cancellation.
use crate::library::*;
use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, sync::Mutex, thread, time::Duration};
use tauri::{AppHandle, Emitter};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct JobRequest {
    pub kind: String,
    pub thumbnails: bool,
    #[serde(default)]
    pub source_folder_id: Option<i64>,
    #[serde(default)]
    pub duplicate_options: Option<crate::duplicates::DuplicateOptions>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct JobRecord {
    pub id: String,
    pub request: JobRequest,
    pub state: String,
    pub stage: String,
    pub processed: i64,
    pub total: i64,
    pub failures: i64,
    pub errors: Vec<String>,
    pub updated_at: i64,
}
#[derive(Default)]
pub struct JobRuntime {
    records: Mutex<Vec<JobRecord>>,
    queue: Mutex<VecDeque<String>>,
    running: Mutex<bool>,
}

impl JobRuntime {
    pub fn hydrate(&self, state: &AppState) -> Result<(), String> {
        if state.read_only {
            return Ok(());
        }
        let conn = database(state)?;
        let mut stmt = conn
            .prepare("SELECT payload FROM task_runs ORDER BY updated_at DESC LIMIT 100")
            .map_err(|e| e.to_string())?;
        let mut records = Vec::new();
        for row in stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?
        {
            let mut record: JobRecord = serde_json::from_str(&row.map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            if ["queued", "running", "paused", "cancelling"].contains(&record.state.as_str()) {
                record.state = "interrupted".into();
                record.stage = "已中断，可重新运行".into();
            }
            records.push(record);
        }
        *self.records.lock().map_err(|e| e.to_string())? = records;
        for record in self.list()? {
            persist(&record, state)?;
        }
        Ok(())
    }
    pub fn list(&self) -> Result<Vec<JobRecord>, String> {
        Ok(self.records.lock().map_err(|e| e.to_string())?.clone())
    }
    fn update(
        &self,
        id: &str,
        state: &AppState,
        app: &AppHandle,
        change: impl FnOnce(&mut JobRecord),
    ) -> Result<(), String> {
        let record = {
            let mut records = self.records.lock().map_err(|e| e.to_string())?;
            let record = records
                .iter_mut()
                .find(|r| r.id == id)
                .ok_or("任务不存在")?;
            change(record);
            record.updated_at = timestamp();
            record.clone()
        };
        persist(&record, state)?;
        let _ = app.emit("jobs:changed", &record);
        Ok(())
    }
}
fn timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
fn database(state: &AppState) -> Result<rusqlite::Connection, String> {
    crate::library::job_database(state)
}
fn persist(record: &JobRecord, state: &AppState) -> Result<(), String> {
    let conn = database(state)?;
    conn.execute("INSERT INTO task_runs(id,kind,state,payload,updated_at) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET state=excluded.state,payload=excluded.payload,updated_at=excluded.updated_at",
        rusqlite::params![record.id,record.request.kind,record.state,serde_json::to_string(record).map_err(|e|e.to_string())?,record.updated_at]).map_err(|e|e.to_string())?;
    Ok(())
}

pub fn start_job(
    request: JobRequest,
    state: &AppState,
    app: AppHandle,
) -> Result<JobRecord, String> {
    let _write = state.write_lock.lock().map_err(|e| e.to_string())?;
    if state.read_only {
        return Err("此图库只允许浏览".into());
    }
    if state.maintenance.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("数据维护中，请稍后或重启后启动任务".into());
    }
    if ![
        "pipeline",
        "scan",
        "thumbnails",
        "semantic",
        "tagging",
        "cleanup",
        "rules",
        "duplicates",
    ]
    .contains(&request.kind.as_str())
    {
        return Err("不支持的任务类型".into());
    }
    if request.kind == "duplicates" {
        let options = request.duplicate_options.clone().unwrap_or(crate::duplicates::DuplicateOptions {
            sensitivity: "standard".into(), force_recompute: false, regroup_only: false,
        });
        if !["strict", "standard", "loose"].contains(&options.sensitivity.as_str()) {
            return Err("不支持的近似检索档位".into());
        }
        if state.jobs.list()?.iter().any(|r| r.request.kind == "duplicates" && ["queued", "running", "paused", "cancelling"].contains(&r.state.as_str())) {
            return Err("已有重复检索任务，请等待或取消当前任务".into());
        }
    }
    let mut queue = state.jobs.queue.lock().map_err(|e| e.to_string())?;
    if queue.len() >= 10 {
        return Err("任务队列已满".into());
    }
    let record = JobRecord {
        id: format!("{}-{}", timestamp(), state.jobs.list()?.len()),
        request,
        state: "queued".into(),
        stage: "等待处理".into(),
        processed: 0,
        total: 0,
        failures: 0,
        errors: vec![],
        updated_at: timestamp(),
    };
    persist(&record, state)?;
    state
        .jobs
        .records
        .lock()
        .map_err(|e| e.to_string())?
        .insert(0, record.clone());
    queue.push_back(record.id.clone());
    drop(queue);
    let mut running = state.jobs.running.lock().map_err(|e| e.to_string())?;
    if !*running {
        *running = true;
        let state = state.clone();
        thread::spawn(move || dispatch(state, app));
    }
    Ok(record)
}

fn dispatch(state: AppState, app: AppHandle) {
    loop {
        // Lock ordering matches start_job; avoid a lost enqueue at the idle boundary.
        let mut queue = state.jobs.queue.lock().unwrap();
        let Some(id) = queue.pop_front() else {
            *state.jobs.running.lock().unwrap() = false;
            return;
        };
        drop(queue);
        let Some(record) = state
            .jobs
            .list()
            .ok()
            .and_then(|rows| rows.into_iter().find(|r| r.id == id))
        else {
            continue;
        };
        if record.state == "cancelled" {
            continue;
        }
        let stages: Vec<&str> = if record.request.kind == "pipeline" {
            if record.request.thumbnails {
                vec!["scan", "thumbnails", "semantic", "tagging"]
            } else {
                vec!["scan", "semantic", "tagging"]
            }
        } else {
            vec![record.request.kind.as_str()]
        };
        let mut failed = false;
        let mut cancelled = false;
        for stage in stages {
            if is_cancelled(&state, &id) {
                cancelled = true;
                break;
            }
            let _ = state.jobs.update(&id, &state, &app, |r| {
                r.state = "running".into();
                r.stage = stage.into();
                r.processed = 0;
                r.total = 0;
            });
            match run_stage(stage, &id, &state, &app) {
                Ok(()) => {}
                Err(error) => {
                    failed = true;
                    let _ = state
                        .jobs
                        .update(&id, &state, &app, |r| r.errors.push(error));
                    if stage == "scan" {
                        break;
                    }
                }
            }
        }
        cancelled |= is_cancelled(&state, &id);
        let _ = state.jobs.update(&id, &state, &app, |r| {
            r.state = if cancelled {
                "cancelled"
            } else if failed || r.failures > 0 {
                "partial"
            } else {
                "completed"
            }
            .into()
        });
        if record.request.kind == "duplicates" {
            let _ = app.emit("duplicates:changed", ());
        } else {
            if let Ok(mut revision) = state.startup_cleanup_generation.lock() {
                *revision += 1;
            }
            let _ = app.emit("library:changed", ());
        }
    }
}
fn is_cancelled(state: &AppState, id: &str) -> bool {
    state
        .jobs
        .list()
        .unwrap_or_default()
        .iter()
        .any(|r| r.id == id && ["cancelled", "cancelling"].contains(&r.state.as_str()))
}
fn run_stage(stage: &str, id: &str, state: &AppState, app: &AppHandle) -> Result<(), String> {
    if stage == "duplicates" {
        return run_duplicates(id, state, app);
    }
    if stage == "rules" {
        return run_rules(id, state, app);
    }
    let mut scoped_state = state.clone();
    scoped_state.scan_folder_id = state
        .jobs
        .list()?
        .into_iter()
        .find(|r| r.id == id)
        .and_then(|r| r.request.source_folder_id);
    let started = match stage {
        "scan" => start_scan_all_folders_collect_only(&scoped_state)?,
        "tagging" => start_tag_pending_images_only(state)?,
        "thumbnails" => start_thumbnail_generation(state)?,
        "semantic" => start_natural_language_scan(state)?,
        "cleanup" => start_startup_cleanup(state)?,
        _ => false,
    };
    if !started {
        return Err("此任务已有处理程序运行".into());
    }
    let previous = state
        .jobs
        .list()?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or("任务不存在")?;
    loop {
        thread::sleep(Duration::from_millis(200));
        let (running, processed, total, failures, errors) = match stage {
            "scan" | "tagging" => {
                let p = background_scan_progress(state)?;
                (
                    background_scan_status(state)?.running,
                    if stage == "scan" {
                        p.scanned_files
                    } else {
                        p.tagged_images + p.failed_images
                    },
                    if stage == "scan" { 0 } else { p.queued_images },
                    p.failed_images,
                    p.recent_errors,
                )
            }
            "thumbnails" => {
                let p = thumbnail_generation_status(state)?;
                (
                    p.running,
                    p.processed_images,
                    p.total_candidates,
                    p.failed_images,
                    p.recent_errors,
                )
            }
            "semantic" => {
                let p = natural_language_scan_progress(state)?;
                (
                    natural_language_scan_status(state)?.running,
                    p.processed_images,
                    p.total_images,
                    p.failed_images,
                    p.recent_errors,
                )
            }
            "cleanup" => {
                let p = startup_cleanup_status(state)?;
                let (failures, errors) = cleanup_failures(state)?;
                (p.running, 0, 0, failures, errors)
            }
            _ => unreachable!(),
        };
        // IPC progress is 200ms; persist only once per second or at the terminal boundary.
        {
            let mut records = state.jobs.records.lock().map_err(|e| e.to_string())?;
            let r = records
                .iter_mut()
                .find(|r| r.id == id)
                .ok_or("任务不存在")?;
            r.processed = processed;
            r.total = total;
            r.failures = previous.failures + failures;
            r.errors = previous
                .errors
                .iter()
                .cloned()
                .chain(errors.iter().cloned())
                .collect();
            let now = timestamp();
            if !running || now - r.updated_at >= 1000 {
                r.updated_at = now;
                persist(r, state)?;
            }
            let _ = app.emit("jobs:changed", r.clone());
        }
        if !running {
            if failures > 0 || !errors.is_empty() {
                return Err(format!("{stage} 阶段部分失败"));
            }
            break;
        }
    }
    Ok(())
}
pub(crate) fn cleanup_failures(state: &AppState) -> Result<(i64, Vec<String>), String> {
    let error = state
        .startup_cleanup_error
        .lock()
        .map_err(|error| error.to_string())?
        .clone();
    let errors: Vec<String> = error.into_iter().collect();
    Ok((errors.len() as i64, errors))
}
pub fn control_job(
    id: String,
    action: String,
    state: &AppState,
    app: AppHandle,
) -> Result<(), String> {
    let record = state
        .jobs
        .list()?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or("任务不存在")?;
    if record.state == "queued" && action == "cancel" {
        return state
            .jobs
            .update(&id, state, &app, |r| r.state = "cancelled".into());
    }
    if !["running", "paused", "cancelling"].contains(&record.state.as_str()) {
        return Err("此任务已结束".into());
    }
    let pause = action == "pause";
    let resume = action == "resume";
    if !pause && !resume && action != "cancel" {
        return Err("不支持的控制操作".into());
    }
    match if record.request.kind == "duplicates" { "duplicates" } else { record.stage.as_str() } {
        "rules" | "duplicates" => {}
        "scan" | "tagging" => {
            if pause {
                pause_background_scan(state)?;
            } else if resume {
                resume_background_scan(state)?;
            } else {
                stop_background_scan(state)?;
            }
        }
        "thumbnails" => {
            if pause {
                pause_thumbnail_generation(state)?;
            } else if resume {
                resume_thumbnail_generation(state)?;
            } else {
                stop_thumbnail_generation(state)?;
            }
        }
        "semantic" => {
            if pause {
                pause_natural_language_scan(state)?;
            } else if resume {
                resume_natural_language_scan(state)?;
            } else {
                stop_natural_language_scan(state)?;
            }
        }
        _ => return Err("此维护阶段不支持暂停或取消".into()),
    }
    state.jobs.update(&id, state, &app, |r| {
        r.state = if pause {
            "paused"
        } else if resume {
            "running"
        } else {
            "cancelling"
        }
        .into()
    })
}

fn run_rules(id: &str, state: &AppState, app: &AppHandle) -> Result<(), String> {
    let mut row = 0;
    let mut processed = 0;
    loop {
        if is_cancelled(state, id) {
            return Ok(());
        }
        if state
            .jobs
            .list()?
            .iter()
            .any(|r| r.id == id && r.state == "paused")
        {
            thread::sleep(Duration::from_millis(100));
            continue;
        }
        let (last, count, total) = crate::library::reapply_rules_batch(row, state)?;
        if count == 0 {
            break;
        }
        row = last;
        processed += count;
        state.jobs.update(id, state, app, |r| {
            r.processed = processed;
            r.total = total;
        })?;
        thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}

fn run_duplicates(id: &str, state: &AppState, app: &AppHandle) -> Result<(), String> {
    let request = state.jobs.list()?.into_iter().find(|r| r.id == id).ok_or("任务不存在")?.request;
    let options = request.duplicate_options.unwrap_or(crate::duplicates::DuplicateOptions {
        sensitivity: "standard".into(), force_recompute: false, regroup_only: false,
    });
    let mut checkpoint = || -> Result<(), String> {
        loop {
            let current = state.jobs.list()?.into_iter().find(|r| r.id == id).ok_or("任务不存在")?;
            if ["cancelled", "cancelling"].contains(&current.state.as_str()) {
                return Err("任务已取消".into());
            }
            if current.state != "paused" && state.foreground_queries.load(std::sync::atomic::Ordering::Relaxed) == 0 {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }
    };
    let mut last_emit = std::time::Instant::now() - Duration::from_millis(200);
    let mut last_stage = String::new();
    let mut progress = |stage: &str, coverage: &crate::duplicates::DuplicateCoverage| {
        if last_emit.elapsed() < Duration::from_millis(200) && stage == last_stage { return; }
        last_emit = std::time::Instant::now();
        last_stage = stage.into();
        let update = (|| -> Result<(), String> {
            let (record, save) = {
                let mut records = state.jobs.records.lock().map_err(|e| e.to_string())?;
                let record = records.iter_mut().find(|r| r.id == id).ok_or("任务不存在")?;
                record.stage = stage.into();
                record.processed = coverage.processed;
                record.total = coverage.total;
                record.failures = coverage.failed + coverage.too_dense;
                let now = timestamp();
                let save = now - record.updated_at >= 1000;
                if save { record.updated_at = now; }
                (record.clone(), save)
            };
            if save { persist(&record, state)?; }
            let _ = app.emit("jobs:changed", record);
            Ok(())
        })();
        if let Err(error) = update { eprintln!("[duplicates-progress] {error}"); }
    };
    let result = crate::duplicates::run_job(options, state, &mut checkpoint, &mut progress);
    if is_cancelled(state, id) { Ok(()) } else { result }
}

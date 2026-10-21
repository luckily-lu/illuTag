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
    #[serde(default)]
    pub duplicate_operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub recycle_operation_id: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub meta: Option<JobMeta>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct JobMeta {
    pub revision: i64,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub started_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub finished_at: Option<i64>,
    pub stage_code: String,
    pub unit: String,
    pub can_pause: bool,
    pub can_resume: bool,
    pub can_cancel: bool,
    pub can_retry: bool,
    /// Memory-only: the executor has finished, but its terminal result has not
    /// been committed. Cleared after a successful save; never a durable terminal state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub persistence_error: Option<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct JobHistoryQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct JobHistoryPage {
    pub rows: Vec<JobRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub next_cursor: Option<String>,
}
#[derive(Default)]
pub struct JobRuntime {
    // Serialize state, durable snapshots and event publication, including controls.
    gate: Mutex<()>,
    records: Mutex<Vec<JobRecord>>,
    queue: Mutex<VecDeque<String>>,
    running: Mutex<bool>,
    pending_terminal: Mutex<Option<store::PendingTerminal>>,
}
#[path = "jobs_store.rs"]
mod store;
pub use store::{clear_history, history};
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
    persist_with_timeout(record, state, None)
}
fn persist_with_timeout(
    record: &JobRecord,
    state: &AppState,
    timeout: Option<Duration>,
) -> Result<(), String> {
    let conn = database(state)?;
    if let Some(timeout) = timeout {
        conn.busy_timeout(timeout).map_err(|e| e.to_string())?;
    }
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
    start_job_locked(request, state, app)
}

// Caller holds write_lock, including the journal confirmation/enqueue transaction.
pub(crate) fn start_job_locked(
    mut request: JobRequest,
    state: &AppState,
    app: AppHandle,
) -> Result<JobRecord, String> {
    if state.read_only {
        return Err("此图库只允许浏览".into());
    }
    if state.maintenance.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("数据维护中，请稍后或重启后启动任务".into());
    }
    // Public duplicate jobs, including retries of retired workflows, always use
    // the current three-round controller. Internal fingerprint preparation does
    // not enter the public job queue.
    if request.kind=="duplicates"{
        let options=request.duplicate_options.get_or_insert(crate::duplicates::DuplicateOptions{sensitivity:"standard".into(),force_recompute:false,regroup_only:false,verification_mode:"cascade".into()});
        options.verification_mode="cascade".into();options.regroup_only=false;
    }
    let _gate = state.jobs.gate.lock().map_err(|e| e.to_string())?;
    if ![
        "pipeline",
        "scan",
        "thumbnails",
        "semantic",
        "tagging",
        "cleanup",
        "rules",
        "duplicates",
        "duplicateManagement",
        "duplicateUndo",
        "recycleRestore",
        "recycleDelete",
    ]
    .contains(&request.kind.as_str())
    {
        return Err("不支持的任务类型".into());
    }
    if let Some(existing) = state.jobs.active_request(&request)? {
        ensure_dispatch(state, app);
        return Ok(existing);
    }
    if request.kind == "duplicates" {
        let options =
            request
                .duplicate_options
                .clone()
                .unwrap_or(crate::duplicates::DuplicateOptions {
                    sensitivity: "standard".into(),
                    force_recompute: false,
                    regroup_only: false,
                    verification_mode: "candidate".into(),
                });
        if !["strict", "standard", "loose"].contains(&options.sensitivity.as_str()) {
            return Err("不支持的近似检索档位".into());
        }
        if state
            .jobs
            .list()?
            .iter()
            .any(|r| r.request.kind == "duplicates" && store::ACTIVE.contains(&r.state.as_str()))
        {
            return Err("已有重复检索任务，请等待或取消当前任务".into());
        }
    }
    if ["duplicateManagement", "duplicateUndo"].contains(&request.kind.as_str()) {
        let operation = request
            .duplicate_operation_id
            .as_ref()
            .ok_or("整理批次编号缺失")?;
        if state.jobs.list()?.iter().any(|r| {
            r.request.duplicate_operation_id.as_ref() == Some(operation)
                && store::ACTIVE.contains(&r.state.as_str())
        }) {
            return Err("该整理批次已经提交，不能重复应用".into());
        }
    }
    if ["recycleRestore", "recycleDelete"].contains(&request.kind.as_str()) {
        let operation = request
            .recycle_operation_id
            .as_deref()
            .ok_or("回收批次编号缺失")?;
        if state.jobs.list()?.iter().any(|j| {
            j.request.recycle_operation_id.as_deref() == Some(operation)
                && store::ACTIVE.contains(&j.state.as_str())
        }) {
            return Err("该回收批次正在运行".into());
        }
        crate::library::recycle::validate_queued(operation, &request.kind, state)?;
    }
    let mut queue = state.jobs.queue.lock().map_err(|e| e.to_string())?;
    if queue.len() >= 10 {
        return Err("任务队列已满".into());
    }
    let record = JobRecord {
        id: next_id(),
        request,
        state: "queued".into(),
        stage: "等待处理".into(),
        processed: 0,
        total: 0,
        failures: 0,
        errors: vec![],
        updated_at: timestamp(),
        meta: None,
    };
    let record = state.jobs.insert_locked(record, state, &app)?;
    queue.push_back(record.id.clone());
    drop(queue);
    ensure_dispatch(state, app);
    Ok(record)
}

// Caller holds gate, including the idle/enqueue boundary.
fn ensure_dispatch(state: &AppState, app: AppHandle) {
    let mut running = state.jobs.running.lock().unwrap();
    if !*running {
        *running = true;
        let state = state.clone();
        thread::spawn(move || dispatch(state, app));
    }
}

fn next_id() -> String {
    static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "{}-{}-{:020}",
        timestamp(),
        std::process::id(),
        SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

fn dispatch(state: AppState, app: AppHandle) {
    loop {
        if let Some(id) = state.jobs.pending_id() {
            if retry_terminal_save(&id, &state, &app).is_err() && stop_for_pending(&state) {
                return;
            }
        }
        // A single claim/control boundary prevents cancelled work from starting.
        let gate = state.jobs.gate.lock().unwrap();
        let record = match state.jobs.claim_locked(&state, |r| {
            let _ = app.emit("jobs:changed", r);
        }) {
            Ok(Some(record)) => record,
            result => {
                if let Err(error) = result {
                    eprintln!("[jobs-dispatch] {error}");
                }
                *state.jobs.running.lock().unwrap() = false;
                return;
            }
        };
        let id = record.id.clone();
        drop(gate);
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
        let mut completed_stage = false;
        for stage in stages {
            if let Err(error) = checkpoint(&state, &id, &app) {
                cancelled = is_cancelled(&state, &id);
                failed = !cancelled;
                if failed {
                    let _ = state
                        .jobs
                        .update(&id, &state, &app, |r| r.errors.push(error));
                }
                break;
            }
            if state
                .jobs
                .update(&id, &state, &app, |r| {
                    r.stage = stage.into();
                    r.processed = 0;
                    r.total = 0;
                    if let Some(meta) = r.meta.as_mut() {
                        meta.stage_code = stage.into();
                        meta.unit = store::unit(stage).into();
                    }
                })
                .is_err()
            {
                failed = true;
                break;
            }
            match run_stage(stage, &id, &state, &app) {
                Ok(()) => {
                    completed_stage = true;
                }
                Err(error) => {
                    if is_cancelled(&state, &id) {
                        cancelled = true;
                        break;
                    }
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
        {
            let _gate = state.jobs.gate.lock().unwrap();
            let r = state.jobs.find(&id).unwrap();
            let outcome = if cancelled || r.state == "cancelling" {
                "cancelled"
            } else if failed && !completed_stage && r.processed == 0 && r.failures == 0 {
                "failed"
            } else if failed || r.failures > 0 {
                "partial"
            } else {
                "completed"
            };
            state.jobs.defer_terminal_locked(&id, outcome).unwrap();
        }
        let saved = retry_terminal_save(&id, &state, &app).is_ok();
        if ["duplicates", "duplicateManagement", "duplicateUndo"]
            .contains(&record.request.kind.as_str())
        {
            let _ = app.emit("duplicates:changed", ());
            if record.request.kind != "duplicates" {
                let _ = app.emit("library:changed", ());
            }
        } else {
            if let Ok(mut revision) = state.startup_cleanup_generation.lock() {
                *revision += 1;
            }
            let _ = app.emit("library:changed", ());
        }
        if !saved && stop_for_pending(&state) {
            return;
        }
    }
}

// A bounded retry round never re-enters an executor or consumes a queued job.
fn retry_terminal_save(id: &str, state: &AppState, app: &AppHandle) -> Result<JobRecord, String> {
    retry_terminal_save_with(id, state, |r| {
        let _ = app.emit("jobs:changed", r);
    })
}
fn retry_terminal_save_with(
    id: &str,
    state: &AppState,
    publish: impl Fn(&JobRecord),
) -> Result<JobRecord, String> {
    let mut last_error = String::new();
    for attempt in 0..3 {
        let result = {
            let _gate = state.jobs.gate.lock().map_err(|e| e.to_string())?;
            state.jobs.save_terminal_locked(id, state, &publish)
        };
        match result {
            Ok(record) => return Ok(record),
            Err(error) => last_error = error,
        }
        if attempt < 2 {
            thread::sleep(Duration::from_millis(200 * (attempt + 1)));
        }
    }
    Err(last_error)
}

fn stop_for_pending(state: &AppState) -> bool {
    let _gate = state.jobs.gate.lock().unwrap();
    if state.jobs.pending_id().is_none() {
        return false;
    }
    *state.jobs.running.lock().unwrap() = false;
    true
}
fn is_cancelled(state: &AppState, id: &str) -> bool {
    state
        .jobs
        .find(id)
        .map(|r| matches!(r.state.as_str(), "cancelled" | "cancelling"))
        .unwrap_or(true)
}

// Cooperative executors acknowledge a pause only after reaching this boundary.
fn checkpoint(state: &AppState, id: &str, app: &AppHandle) -> Result<(), String> {
    loop {
        let gate = state.jobs.gate.lock().map_err(|e| e.to_string())?;
        let record = state.jobs.find(id)?;
        match record.state.as_str() {
            "cancelling" | "cancelled" => return Err("任务已取消".into()),
            "pausing" => state.jobs.commit_locked(
                id,
                state,
                |r| {
                    let _ = app.emit("jobs:changed", r);
                },
                |r| r.state = "paused".into(),
            )?,
            "paused" => {}
            "running" => return Ok(()),
            _ => return Err("任务已结束".into()),
        }
        drop(gate);
        thread::sleep(Duration::from_millis(50));
    }
}
fn run_stage(stage: &str, id: &str, state: &AppState, app: &AppHandle) -> Result<(), String> {
    if ["recycleRestore", "recycleDelete"].contains(&stage) {
        return run_recycle(id, state, app);
    }
    if stage == "duplicates" {
        return run_duplicates(id, state, app);
    }
    if ["duplicateManagement", "duplicateUndo"].contains(&stage) {
        return run_duplicate_management(id, state, app);
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
    let started = loop {
        checkpoint(state, id, app)?;
        let _gate = state.jobs.gate.lock().map_err(|e| e.to_string())?;
        // Pause/cancel and worker launch share the same boundary. A stop request
        // cannot be lost when the legacy worker clears its flags on startup.
        if state.jobs.find(id)?.state != "running" {
            continue;
        }
        break match stage {
            "scan" => start_scan_all_folders_collect_only(&scoped_state)?,
            "tagging" => start_tag_pending_images_only(state)?,
            "thumbnails" => start_thumbnail_generation(state)?,
            "semantic" => start_natural_language_scan(state)?,
            "cleanup" => start_startup_cleanup(state)?,
            _ => false,
        };
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
        let (running, paused, processed, total, failures, errors) = match stage {
            "scan" | "tagging" => {
                let p = background_scan_progress(state)?;
                (
                    background_scan_status(state)?.running,
                    p.paused,
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
                    p.paused,
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
                    p.paused,
                    p.processed_images,
                    p.total_images,
                    p.failed_images,
                    p.recent_errors,
                )
            }
            "cleanup" => {
                let p = startup_cleanup_status(state)?;
                let (failures, errors) = cleanup_failures(state)?;
                (p.running, false, 0, 0, failures, errors)
            }
            _ => unreachable!(),
        };
        if let Err(error) = state.jobs.update(id, state, app, |r| {
            if paused && r.state == "pausing" {
                r.state = "paused".into();
            }
            r.processed = processed;
            r.total = total;
            r.failures = previous.failures + failures;
            r.errors = previous
                .errors
                .iter()
                .cloned()
                .chain(errors.iter().cloned())
                .collect();
        }) {
            eprintln!("[jobs-progress] {error}");
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

#[derive(Debug, PartialEq, Eq)]
enum JobControlTarget {
    Record,
    Scan,
    Thumbnails,
    Semantic,
}

fn job_control_target(kind: &str, stage: &str) -> Result<JobControlTarget, String> {
    let stage = if matches!(
        kind,
        "duplicates" | "duplicateManagement" | "duplicateUndo" | "recycleRestore" | "recycleDelete"
    ) {
        kind
    } else {
        stage
    };
    match stage {
        "rules"
        | "duplicates"
        | "duplicateManagement"
        | "duplicateUndo"
        | "recycleRestore"
        | "recycleDelete" => Ok(JobControlTarget::Record),
        "scan" | "tagging" => Ok(JobControlTarget::Scan),
        "thumbnails" => Ok(JobControlTarget::Thumbnails),
        "semantic" => Ok(JobControlTarget::Semantic),
        _ => Err("此维护阶段不支持暂停或取消".into()),
    }
}

pub fn control_job(
    id: String,
    action: String,
    state: &AppState,
    app: AppHandle,
) -> Result<(), String> {
    if action == "retrySave" {
        retry_terminal_save(&id, state, &app)?;
        let _gate = state.jobs.gate.lock().map_err(|e| e.to_string())?;
        ensure_dispatch(state, app);
        return Ok(());
    }
    let _gate = state.jobs.gate.lock().map_err(|e| e.to_string())?;
    if state.jobs.pending_id().as_deref() == Some(&id) {
        return Err("任务结果尚未保存，请重试保存".into());
    }
    let record = state.jobs.find(&id)?;
    let desired = store::control_state(&record.state, &action)?;
    if record.state == "queued" && action == "cancel" {
        return state.jobs.cancel_queued_locked(&id, state, |r| {
            let _ = app.emit("jobs:changed", r);
        });
    }
    let pause = action == "pause";
    let resume = action == "resume";
    let stage = record
        .meta
        .as_ref()
        .map_or(record.stage.as_str(), |m| m.stage_code.as_str());
    let target = job_control_target(&record.request.kind, stage)?;
    // Persist the intent before touching a worker flag. Failed persistence must
    // not stop work while leaving its task snapshot in the running state.
    state.jobs.commit_locked(
        &id,
        state,
        |r| {
            let _ = app.emit("jobs:changed", r);
        },
        |r| r.state = desired.into(),
    )?;
    match target {
        JobControlTarget::Record => true,
        JobControlTarget::Scan => {
            if pause {
                pause_background_scan(state)?
            } else if resume {
                resume_background_scan(state)?
            } else {
                stop_background_scan(state)?
            }
        }
        JobControlTarget::Thumbnails => {
            if pause {
                pause_thumbnail_generation(state)?
            } else if resume {
                resume_thumbnail_generation(state)?
            } else {
                stop_thumbnail_generation(state)?
            }
        }
        JobControlTarget::Semantic => {
            if pause {
                pause_natural_language_scan(state)?
            } else if resume {
                resume_natural_language_scan(state)?
            } else {
                stop_natural_language_scan(state)?
            }
        }
    };
    // A false legacy result means the stage has just ended or has not launched
    // yet; dispatch and its safe-point check still observe the committed intent.
    Ok(())
}

/// Retry the persisted request, preserving its scope and options. Batch retries
/// pass through their original journal validation and keep the same operation.
pub fn retry_job(id: String, state: &AppState, app: AppHandle) -> Result<JobRecord, String> {
    if state.jobs.pending_id().as_deref() == Some(&id) {
        let record = retry_terminal_save(&id, state, &app)?;
        let _gate = state.jobs.gate.lock().map_err(|e| e.to_string())?;
        ensure_dispatch(state, app);
        return Ok(record);
    }
    let request = retry_request(store::load(&id, state)?)?;
    match request.kind.as_str() {
        "duplicateManagement" | "duplicateUndo" => {
            crate::duplicates::management::validate_apply(
                &crate::duplicates::DuplicateApplyRequest {
                    operation_id: request
                        .duplicate_operation_id
                        .clone()
                        .ok_or("整理批次缺失")?,
                    confirmed: true,
                },
                state,
            )?;
            start_job(request, state, app)
        }
        "recycleRestore" | "recycleDelete" => crate::library::recycle::retry(request, state, app),
        _ => start_job(request, state, app),
    }
}

fn retry_request(record: JobRecord) -> Result<JobRequest, String> {
    if !store::terminal(&record.state) {
        return Err("请先等待当前任务结束".into());
    }
    Ok(record.request)
}

fn run_rules(id: &str, state: &AppState, app: &AppHandle) -> Result<(), String> {
    let mut row = 0;
    let mut processed = 0;
    loop {
        checkpoint(state, id, app)?;
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
    let request = state
        .jobs
        .list()?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or("任务不存在")?
        .request;
    let options = request
        .duplicate_options
        .unwrap_or(crate::duplicates::DuplicateOptions {
            sensitivity: "standard".into(),
            force_recompute: false,
            regroup_only: false,
            verification_mode: "candidate".into(),
        });
    let progress_error = std::cell::RefCell::new(None::<String>);
    let mut check = || -> Result<(), String> {
        loop {
            if let Some(error) = progress_error.borrow_mut().take() {
                return Err(error);
            }
            checkpoint(state, id, app)?;
            if state
                .foreground_queries
                .load(std::sync::atomic::Ordering::Relaxed)
                == 0
            {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }
    };
    let mut last_emit = std::time::Instant::now() - Duration::from_millis(200);
    let mut last_stage = String::new();
    let mut progress = |stage: &str, coverage: &crate::duplicates::DuplicateCoverage| {
        if last_emit.elapsed() < Duration::from_millis(200) && stage == last_stage {
            return;
        }
        last_emit = std::time::Instant::now();
        last_stage = stage.into();
        let update = state.jobs.update(id, state, app, |record| {
            record.stage = if stage.contains("复核候选") || stage.contains("逐字节确认") {
                format!(
                    "{stage} · 通过 {} 对 / 差异 {} 对 / 无法确认 {} 对",
                    coverage.verified_pairs, coverage.content_variants, coverage.uncertain_pairs
                )
            } else {
                stage.into()
            };
            record.processed = coverage.processed;
            record.total = coverage.total;
            record.failures = coverage.failed + coverage.too_dense;
            if let Some(meta) = record.meta.as_mut() {
                meta.stage_code = if stage.starts_with("第一轮") {"cascadeRecall"}
                else if stage.starts_with("第二轮") {"cascadeContent"}
                else if stage.starts_with("第三轮") {"cascadePixels"}
                else if stage.contains("计划") || stage.contains("合并") {
                    "planning"
                } else if stage.contains("验证")
                    || stage.contains("复核")
                    || stage.contains("逐字节")
                {
                    "verification"
                } else if stage.contains("指纹") {
                    "fingerprints"
                } else {
                    "discovery"
                }
                .into();
                meta.unit = if stage=="第二轮：载入候选指纹" || stage=="第三轮：核对原生像素摘要" { "images" }
                else if stage.starts_with("第二轮")||stage.starts_with("第三轮")||stage=="三轮检索完成"||stage=="生成分类整理计划"||[
                    "检索直接替代关系",
                    "验证候选原图（比较关系）",
                    "整理验证缓存",
                    "恢复检索断点",
                    "构建候选索引",
                    "生成全量整理计划",
                    "合并最新人工选择",
                    "副本检索完成",
                    "复检近似候选",
                    "合并整理计划",
                    "近似复检完成",
                ]
                .contains(&stage)
                {
                    "pairs"
                } else {
                    "images"
                }
                .into();
            }
        });
        if let Err(error) = update {
            eprintln!("[duplicates-progress] {error}");
            *progress_error.borrow_mut() = Some(format!(
                "保存查重任务进度失败，检索已停止并保留已提交结果：{error}"
            ));
        }
    };
    let result = crate::duplicates::run_job(options, state, &mut check, &mut progress);
    let result = result.and_then(|_| progress_error.into_inner().map_or(Ok(()), Err));
    if is_cancelled(state, id) {
        Ok(())
    } else {
        result
    }
}

fn run_duplicate_management(id: &str, state: &AppState, app: &AppHandle) -> Result<(), String> {
    let request = state
        .jobs
        .list()?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or("任务不存在")?
        .request;
    let operation = request.duplicate_operation_id.ok_or("整理批次缺失")?;
    let mut control = || checkpoint(state, id, app);
    let mut last = std::time::Instant::now() - Duration::from_secs(1);
    let mut progress = |status: &crate::duplicates::DuplicateOperationStatus| {
        if last.elapsed() < Duration::from_millis(200) && status.state == "running" {
            return;
        }
        last = std::time::Instant::now();
        let _ = state.jobs.update(id, state, app, |r| {
            r.processed = status.processed;
            r.total = status.total;
            r.failures = status.failed;
            r.errors = status.errors.clone();
            if let Some(meta) = r.meta.as_mut() {
                meta.stage_code = status.stage.clone();
            }
            r.stage = match status.stage.as_str() {
                "snapshot" | "workflowSnapshot" => format!(
                    "复用已扫描结果 {}/{}",
                    status.stage_processed, status.stage_total
                ),
                "planning" => format!(
                    "选择保留图片 {}/{}",
                    status.stage_processed, status.stage_total
                ),
                "undoSnapshot" => format!(
                    "准备整批还原 {}/{}",
                    status.stage_processed, status.stage_total
                ),
                _ if status.action == "restore" => "还原本次软回收".into(),
                _ if status.action == "systemTrash" => "移入系统回收站".into(),
                _ => "核实源文件并整理重复图片".into(),
            };
        });
        let _ = app.emit("duplicate-management:changed", status);
    };
    crate::duplicates::management::run_operation(&operation, state, &mut control, &mut progress)
}

fn run_recycle(id: &str, state: &AppState, app: &AppHandle) -> Result<(), String> {
    let operation = state
        .jobs
        .list()?
        .into_iter()
        .find(|r| r.id == id)
        .and_then(|r| r.request.recycle_operation_id)
        .ok_or("回收批次缺失")?;
    let mut control = || checkpoint(state, id, app);
    let mut progress = |status: &crate::library::recycle::RecycleOperationStatus| {
        let _ = state.jobs.update(id, state, app, |r| {
            r.processed = status.processed;
            r.total = status.total;
            r.failures = status.failed;
            r.errors = status.errors.clone();
            r.stage = if status.action == "restore" {
                "还原回收站图片"
            } else {
                "移入Windows回收站"
            }
            .into();
        });
    };
    crate::library::recycle::run(&operation, state, &mut control, &mut progress)
}

#[cfg(test)]
#[path = "jobs_tests.rs"]
mod runtime_tests;

#[cfg(test)]
mod tests {
    use super::{job_control_target, JobControlTarget};

    #[test]
    fn recycle_control_uses_job_record_after_progress_changes_stage() {
        for (kind, progress_stage) in [
            ("recycleRestore", "还原回收站图片"),
            ("recycleDelete", "移入Windows回收站"),
        ] {
            for stage in [kind, progress_stage] {
                assert_eq!(
                    job_control_target(kind, stage),
                    Ok(JobControlTarget::Record)
                );
            }
        }
    }

    #[test]
    fn control_preserves_pipeline_stage_routing_and_maintenance_rejection() {
        for (stage, target) in [
            ("rules", JobControlTarget::Record),
            ("scan", JobControlTarget::Scan),
            ("tagging", JobControlTarget::Scan),
            ("thumbnails", JobControlTarget::Thumbnails),
            ("semantic", JobControlTarget::Semantic),
        ] {
            assert_eq!(job_control_target("pipeline", stage), Ok(target));
        }
        assert!(job_control_target("pipeline", "backup").is_err());
    }
}

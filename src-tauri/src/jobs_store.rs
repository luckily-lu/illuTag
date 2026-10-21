//! Bounded runtime snapshots; task history stays in the existing schema-3 table.
use super::*;

pub(super) const RECENT_LIMIT: usize = 100;
pub(super) const ACTIVE: [&str; 5] = ["queued", "running", "pausing", "paused", "cancelling"];

#[derive(Clone)]
pub(super) struct PendingTerminal {
    id: String,
    outcome: String,
    stage: String,
    /// Time the work actually ended. Deferred persistence must not include the
    /// waiting time between completion and a successful terminal save.
    finished_at: i64,
}

pub(super) fn terminal(state: &str) -> bool {
    !ACTIVE.contains(&state)
}

pub(super) fn legal_transition(from: &str, to: &str) -> bool {
    if from == to {
        return !terminal(from);
    }
    match from {
        "queued" => matches!(to, "running" | "cancelled" | "interrupted"),
        "running" => matches!(
            to,
            "pausing" | "cancelling" | "completed" | "partial" | "failed" | "interrupted"
        ),
        "pausing" => matches!(
            to,
            "paused" | "cancelling" | "completed" | "partial" | "failed" | "interrupted"
        ),
        "paused" => matches!(
            to,
            "running" | "cancelling" | "completed" | "partial" | "failed" | "interrupted"
        ),
        "cancelling" => matches!(to, "cancelled" | "interrupted"),
        _ => false,
    }
}

pub(super) fn control_state(state: &str, action: &str) -> Result<&'static str, String> {
    match (state, action) {
        ("queued", "cancel") => Ok("cancelled"),
        ("running", "pause") => Ok("pausing"),
        ("paused", "resume") => Ok("running"),
        ("running" | "pausing" | "paused", "cancel") => Ok("cancelling"),
        _ => Err("当前任务状态不允许此操作".into()),
    }
}

pub(super) fn unit(stage: &str) -> &'static str {
    match stage {
        "scan" => "files",
        "cleanup" => "unknown",
        _ => "images",
    }
}

pub(super) fn decorate(record: &mut JobRecord) {
    let meta = record.meta.get_or_insert_with(|| JobMeta {
        revision: 0,
        created_at: record.updated_at,
        started_at: None,
        finished_at: None,
        stage_code: if record.stage.is_ascii() {
            record.stage.clone()
        } else {
            record.request.kind.clone()
        },
        unit: unit(&record.request.kind).into(),
        can_pause: false,
        can_resume: false,
        can_cancel: false,
        can_retry: false,
        persistence_error: None,
    });
    let controllable = job_control_target(&record.request.kind, &meta.stage_code).is_ok();
    meta.can_pause = record.state == "running" && controllable;
    meta.can_resume = record.state == "paused" && controllable;
    meta.can_cancel = record.state == "queued"
        || (matches!(record.state.as_str(), "running" | "pausing" | "paused") && controllable);
    let batch = record.request.duplicate_operation_id.is_some()
        || record.request.recycle_operation_id.is_some();
    meta.can_retry = terminal(&record.state) && (!batch || record.state != "completed");
    if meta.persistence_error.is_some() {
        meta.can_pause = false;
        meta.can_resume = false;
        meta.can_cancel = false;
        meta.can_retry = true;
    }
}

pub(super) fn advance(record: &mut JobRecord, previous: Option<&JobRecord>) {
    record.errors.truncate(100);
    decorate(record);
    let now = timestamp();
    record.updated_at = previous.map_or(now, |p| now.max(p.updated_at.saturating_add(1)));
    let meta = record.meta.as_mut().unwrap();
    meta.revision = previous
        .and_then(|p| p.meta.as_ref())
        .map_or(1, |m| m.revision.saturating_add(1));
    if previous.is_none() {
        meta.created_at = now;
    }
    if record.state == "running" && meta.started_at.is_none() {
        meta.started_at = Some(now);
    }
    if terminal(&record.state) && meta.finished_at.is_none() {
        meta.finished_at = Some(now);
    }
}

fn trim(records: &mut Vec<JobRecord>) {
    records.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    let mut recent = 0;
    records.retain(|record| {
        if !terminal(&record.state) {
            return true;
        }
        recent += 1;
        recent <= RECENT_LIMIT
    });
}

impl JobRuntime {
    pub(super) fn active_request(&self, request: &JobRequest) -> Result<Option<JobRecord>, String> {
        let value = serde_json::to_value(request).map_err(|e| e.to_string())?;
        Ok(self
            .records
            .lock()
            .map_err(|e| e.to_string())?
            .iter()
            .find(|record| {
                ACTIVE.contains(&record.state.as_str())
                    && serde_json::to_value(&record.request).ok().as_ref() == Some(&value)
            })
            .cloned())
    }
    pub fn hydrate(&self, state: &AppState) -> Result<(), String> {
        if state.read_only {
            return Ok(());
        }
        let _gate = self.gate.lock().map_err(|e| e.to_string())?;
        let conn = database(state)?;
        conn.execute_batch("CREATE INDEX IF NOT EXISTS task_runs_history ON task_runs(updated_at DESC,id DESC); CREATE INDEX IF NOT EXISTS task_runs_state_history ON task_runs(state,updated_at DESC,id DESC)")
            .map_err(|e| e.to_string())?;
        // Recover every unfinished record, including ones outside the recent-history window.
        loop {
            let mut stmt = conn.prepare("SELECT payload FROM task_runs WHERE state IN ('queued','running','pausing','paused','cancelling') LIMIT 100")
                .map_err(|e| e.to_string())?;
            let pending = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            if pending.is_empty() {
                break;
            }
            for payload in pending {
                let previous: JobRecord =
                    serde_json::from_str(&payload).map_err(|e| e.to_string())?;
                let mut record = previous.clone();
                record.state = "interrupted".into();
                record.stage = "已中断，可重新运行".into();
                // Restart time includes time with the app closed; it is not execution time.
                if let Some(meta) = record.meta.as_mut() {
                    meta.finished_at = Some(previous.updated_at);
                }
                advance(&mut record, Some(&previous));
                persist(&record, state)?;
            }
        }
        let page = history(
            JobHistoryQuery {
                state: None,
                cursor: None,
                limit: Some(RECENT_LIMIT as u32),
            },
            state,
        )?;
        *self.records.lock().map_err(|e| e.to_string())? = page.rows;
        self.queue.lock().map_err(|e| e.to_string())?.clear();
        *self.running.lock().map_err(|e| e.to_string())? = false;
        *self.pending_terminal.lock().map_err(|e| e.to_string())? = None;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<JobRecord>, String> {
        Ok(self.records.lock().map_err(|e| e.to_string())?.clone())
    }
    pub fn try_list(&self) -> Result<Option<Vec<JobRecord>>, String> {
        match self.records.try_lock() {
            Ok(rows) => Ok(Some(rows.clone())),
            Err(std::sync::TryLockError::WouldBlock) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    pub(super) fn find(&self, id: &str) -> Result<JobRecord, String> {
        self.records
            .lock()
            .map_err(|e| e.to_string())?
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or_else(|| "任务不存在".into())
    }

    // Caller holds gate. Persistence succeeds before the visible snapshot and event change.
    pub(super) fn commit_locked(
        &self,
        id: &str,
        state: &AppState,
        publish: impl FnOnce(&JobRecord),
        change: impl FnOnce(&mut JobRecord),
    ) -> Result<(), String> {
        self.commit_with_timeout_locked(id, state, publish, change, None)
    }

    fn commit_with_timeout_locked(
        &self,
        id: &str,
        state: &AppState,
        publish: impl FnOnce(&JobRecord),
        change: impl FnOnce(&mut JobRecord),
        timeout: Option<Duration>,
    ) -> Result<(), String> {
        let mut records = self.records.lock().map_err(|e| e.to_string())?;
        let index = records
            .iter()
            .position(|r| r.id == id)
            .ok_or("任务不存在")?;
        let previous = &records[index];
        let mut record = previous.clone();
        change(&mut record);
        if !legal_transition(&previous.state, &record.state) {
            return Err("任务状态已变化，请刷新后重试".into());
        }
        advance(&mut record, Some(previous));
        persist_with_timeout(&record, state, timeout)?;
        records[index] = record.clone();
        trim(&mut records);
        drop(records);
        publish(&record);
        Ok(())
    }

    pub(super) fn update(
        &self,
        id: &str,
        state: &AppState,
        app: &AppHandle,
        change: impl FnOnce(&mut JobRecord),
    ) -> Result<(), String> {
        let _gate = self.gate.lock().map_err(|e| e.to_string())?;
        self.commit_locked(
            id,
            state,
            |r| {
                let _ = app.emit("jobs:changed", r);
            },
            change,
        )
    }

    pub(super) fn insert_locked(
        &self,
        mut record: JobRecord,
        state: &AppState,
        app: &AppHandle,
    ) -> Result<JobRecord, String> {
        advance(&mut record, None);
        persist(&record, state)?;
        let mut records = self.records.lock().map_err(|e| e.to_string())?;
        records.insert(0, record.clone());
        trim(&mut records);
        drop(records);
        let _ = app.emit("jobs:changed", &record);
        Ok(record)
    }

    pub(super) fn cancel_queued_locked(
        &self,
        id: &str,
        state: &AppState,
        publish: impl FnOnce(&JobRecord),
    ) -> Result<(), String> {
        if self.find(id)?.state != "queued" {
            return Err("任务已经开始".into());
        }
        self.commit_locked(id, state, publish, |r| r.state = "cancelled".into())?;
        self.queue
            .lock()
            .map_err(|e| e.to_string())?
            .retain(|queued| queued != id);
        Ok(())
    }

    pub(super) fn note_claim_failure(
        &self,
        id: &str,
        error: &str,
        attempt: u32,
        delay: std::time::Duration,
    ) -> Option<JobRecord> {
        let mut records = self.records.lock().ok()?;
        let record = records.iter_mut().find(|record| record.id == id)?;
        if record.state != "queued" {
            return None;
        }
        let previous = record.clone();
        let reason: String = error.chars().take(160).collect();
        record.stage = format!(
            "队列领取失败，正在重试（第 {attempt} 次，约 {} 秒后）：{reason}",
            delay.as_secs().max(1)
        );
        advance(record, Some(&previous));
        let snapshot = record.clone();
        drop(records);
        Some(snapshot)
    }

    pub(super) fn claim_locked(
        &self,
        state: &AppState,
        publish: impl FnOnce(&JobRecord),
    ) -> Result<Option<JobRecord>, String> {
        if self.pending_id().is_some() {
            return Err("任务结果尚未保存，队列已暂停".into());
        }
        let mut queue = self.queue.lock().map_err(|e| e.to_string())?;
        while let Some(id) = queue.front().cloned() {
            let record = self.find(&id)?;
            if record.state != "queued" {
                queue.pop_front();
                continue;
            }
            // A failed write keeps the pending entry available for cancellation
            // or a later enqueue to restart dispatch. Bound the writer wait so a
            // queued cancellation can obtain the gate promptly and retry.
            self.commit_with_timeout_locked(
                &id,
                state,
                publish,
                |r| r.state = "running".into(),
                Some(std::time::Duration::from_millis(250)),
            )?;
            queue.pop_front();
            return self.find(&id).map(Some);
        }
        Ok(None)
    }

    pub(super) fn pending_id(&self) -> Option<String> {
        self.pending_terminal
            .lock()
            .unwrap()
            .as_ref()
            .map(|pending| pending.id.clone())
    }

    pub(super) fn defer_terminal_locked(&self, id: &str, outcome: &str) -> Result<(), String> {
        let record = self.find(id)?;
        let finished_at = timestamp();
        {
            let mut pending = self.pending_terminal.lock().map_err(|e| e.to_string())?;
            if pending.is_some() {
                return Err("已有待保存的任务结果".into());
            }
            *pending = Some(PendingTerminal {
                id: id.into(),
                outcome: outcome.into(),
                stage: record.stage,
                finished_at,
            });
        }
        // Freeze the visible duration immediately; persistence may be deferred.
        let mut records = self.records.lock().map_err(|e| e.to_string())?;
        if let Some(record) = records.iter_mut().find(|record| record.id == id) {
            let previous = record.clone();
            if let Some(meta) = record.meta.as_mut() {
                meta.finished_at = Some(finished_at);
            }
            advance(record, Some(&previous));
        }
        Ok(())
    }

    pub(super) fn save_terminal_locked(
        &self,
        id: &str,
        state: &AppState,
        publish: impl Fn(&JobRecord),
    ) -> Result<JobRecord, String> {
        let pending = self
            .pending_terminal
            .lock()
            .map_err(|e| e.to_string())?
            .clone();
        let Some(pending) = pending else {
            let record = self.find(id)?;
            return if terminal(&record.state) {
                Ok(record)
            } else {
                Err("此任务没有待保存的结果".into())
            };
        };
        if pending.id != id {
            return Err("此任务没有待保存的结果".into());
        }
        let result = self.commit_with_timeout_locked(
            id,
            state,
            &publish,
            |record| {
                record.state = pending.outcome;
                record.stage = pending.stage;
                if let Some(meta) = record.meta.as_mut() {
                    meta.persistence_error = None;
                    meta.finished_at = Some(
                        pending
                            .finished_at
                            .max(meta.started_at.unwrap_or(pending.finished_at)),
                    );
                }
            },
            Some(Duration::from_millis(250)),
        );
        if let Err(error) = result {
            let message = format!("任务结果尚未保存，队列已暂停。请重试保存：{error}");
            // This is an explicitly transient diagnostic, not a persisted finish.
            // Its revision also advances, so list/response snapshots cannot undo it.
            let mut records = self.records.lock().map_err(|e| e.to_string())?;
            let record = records
                .iter_mut()
                .find(|r| r.id == id)
                .ok_or("任务不存在")?;
            let previous = record.clone();
            record.stage = "结果未保存，队列已暂停".into();
            if let Some(meta) = record.meta.as_mut() {
                meta.persistence_error = Some(message.clone());
            }
            advance(record, Some(&previous));
            let snapshot = record.clone();
            drop(records);
            publish(&snapshot);
            return Err(message);
        }
        *self.pending_terminal.lock().map_err(|e| e.to_string())? = None;
        self.find(id)
    }
}

pub fn history(query: JobHistoryQuery, state: &AppState) -> Result<JobHistoryPage, String> {
    let limit = query.limit.unwrap_or(30).clamp(1, RECENT_LIMIT as u32) as usize;
    let cursor: Option<(i64, String)> = query
        .cursor
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|_| "任务历史游标无效".to_string())?;
    let filter = query.state.as_deref().filter(|s| *s != "all");
    if filter.is_some_and(|s| {
        !["completed", "partial", "failed", "cancelled", "interrupted"].contains(&s)
    }) {
        return Err("不支持的任务历史状态".into());
    }
    let conn = database(state)?;
    let state_clause = if filter.is_some() {
        "state=?4"
    } else {
        "?4 IS NULL AND state NOT IN ('queued','running','pausing','paused','cancelling')"
    };
    let boundary = if cursor.is_some() {
        " AND (updated_at,id)<(?1,?2)"
    } else {
        ""
    };
    let sql = format!("SELECT payload FROM task_runs WHERE {state_clause}{boundary} ORDER BY updated_at DESC,id DESC LIMIT ?3");
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let payloads = stmt
        .query_map(
            rusqlite::params![
                cursor.as_ref().map(|c| c.0),
                cursor.as_ref().map(|c| &c.1),
                limit + 1,
                filter
            ],
            |r| r.get::<_, String>(0),
        )
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let more = payloads.len() > limit;
    let mut rows = payloads
        .into_iter()
        .take(limit)
        .map(|payload| {
            let mut record: JobRecord =
                serde_json::from_str(&payload).map_err(|e| e.to_string())?;
            decorate(&mut record);
            Ok(record)
        })
        .collect::<Result<Vec<_>, String>>()?;
    let next_cursor = if more {
        rows.last()
            .map(|r| serde_json::to_string(&(r.updated_at, &r.id)))
            .transpose()
            .map_err(|e| e.to_string())?
    } else {
        None
    };
    Ok(JobHistoryPage {
        rows: std::mem::take(&mut rows),
        next_cursor,
    })
}

pub fn clear_history(confirmed: bool, state: &AppState) -> Result<i64, String> {
    if !confirmed {
        return Err("请先确认清除任务历史".into());
    }
    if state.read_only {
        return Err("此图库只允许浏览".into());
    }
    if state.maintenance.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("数据维护中，暂不能清除任务历史".into());
    }
    let _gate = state.jobs.gate.lock().map_err(|e| e.to_string())?;
    let conn = database(state)?;
    let deleted = conn.execute("DELETE FROM task_runs WHERE state IN ('completed','partial','failed','cancelled','interrupted')", [])
        .map_err(|e| e.to_string())?;
    state
        .jobs
        .records
        .lock()
        .map_err(|e| e.to_string())?
        .retain(|r| !terminal(&r.state));
    Ok(deleted as i64)
}

pub(super) fn load(id: &str, state: &AppState) -> Result<JobRecord, String> {
    let conn = database(state)?;
    let payload: String = conn
        .query_row("SELECT payload FROM task_runs WHERE id=?1", [id], |r| {
            r.get(0)
        })
        .map_err(|_| "任务不存在".to_string())?;
    serde_json::from_str(&payload).map_err(|e| e.to_string())
}

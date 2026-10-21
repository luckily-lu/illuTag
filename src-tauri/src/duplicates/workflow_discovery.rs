//! One explicit job: reuse/update fingerprints, discover direct evidence, publish once.
use super::*;
use std::cell::RefCell;
#[path = "discovery_resume.rs"]
mod resume;

fn member(conn: &Connection, gen: i64, group: i64, pk: i64) -> Result<usize, String> {
    conn.execute("INSERT OR IGNORE INTO members SELECT ?1,?2,image_pk,image_pk,public_hash,signature,NULL,NULL,NULL FROM fingerprints WHERE image_pk=?3",params![gen,group,pk]).map_err(|e|e.to_string())
}
fn pair(
    conn: &Connection,
    gen: i64,
    reference: i64,
    pk: i64,
    answer: &super::super::evidence::Evidence,
    control: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    let mut upgrade = false;
    if !answer.automatic {
        // A failed relationship is diagnostic context, not a global image label.
        // One retained example per candidate bounds stored graph size; discovery
        // still compares other references and always records later positive proof.
        let recorded:Option<String>=conn.query_row("SELECT kind FROM workflow_class INDEXED BY workflow_class_image WHERE run=?1 AND image_pk=?2 LIMIT 1",params![gen,pk],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
        upgrade = answer.level == "lossy"
            && recorded
                .as_ref()
                .is_some_and(|k| matches!(k.as_str(), "similar" | "uncertain"));
        if recorded.is_some() && !upgrade {
            return Ok(());
        }
    }
    let group = reference
        .checked_mul(2)
        .and_then(|v| v.checked_add(1))
        .ok_or("图片键超出范围")?;
    cache::controlled_write(conn, control, || conn.execute_batch("BEGIN IMMEDIATE"))?;
    let result = (|| -> Result<(), String> {
        if upgrade {
            conn.execute("DELETE FROM workflow_class WHERE run=?1 AND image_pk=?2 AND kind IN ('similar','uncertain')",params![gen,pk]).map_err(|e|e.to_string())?;
        }
        conn.execute(
            "INSERT OR IGNORE INTO groups VALUES(?1,?2,1,?3,0)",
            params![gen, group, reference],
        )
        .map_err(|e| e.to_string())?;
        let count = member(conn, gen, group, reference)? + member(conn, gen, group, pk)?;
        conn.execute(
            "UPDATE groups SET total_members=total_members+?3 WHERE generation=?1 AND group_no=?2",
            params![gen, group, count],
        )
        .map_err(|e| e.to_string())?;
        let kind = if answer.automatic {
            "near"
        } else {
            match answer.level.as_str() {
                "lossy" => "suggested",
                "uncertain" => "uncertain",
                _ => "similar",
            }
        };
        conn.execute(
            "INSERT OR REPLACE INTO workflow_class VALUES(?1,?2,?3,?4,?5)",
            params![gen, group, pk, kind, answer.reason],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = conn.execute_batch("ROLLBACK");
        return Err(error);
    }
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())
}

pub(super) fn run(
    options: DuplicateOptions,
    state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>,
    progress: &mut dyn FnMut(&str, &DuplicateCoverage),
) -> Result<(), String> {
    progress("读取检索元数据", &DuplicateCoverage::default());
    let initial = cache::published_status(state)?;
    let source = if initial.generation.is_none() || initial.needs_update || options.force_recompute
    {
        let pending = cache::open_existing(state, true).ok();
        if let Some(conn) = pending {
            conn.execute("DELETE FROM meta WHERE key='prepared_index'", [])
                .map_err(|e| e.to_string())?;
        }
        cache::run_job(
            DuplicateOptions {
                verification_mode: "prepareEvidence".into(),
                regroup_only: false,
                ..options.clone()
            },
            state,
            control,
            progress,
        )?;
        let conn = cache::open_existing(state, true)?;
        conn.query_row(
            "SELECT value FROM meta WHERE key='prepared_index'",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or("指纹检查未完整完成，保留已有结果")?
    } else {
        initial.generation.unwrap()
    };
    control()?;
    let mut conn = open(state)?;
    let indexed_coverage: String = conn
        .query_row(
            "SELECT coverage FROM generations WHERE generation=?1",
            [&source],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let mut coverage = DuplicateCoverage::default();
    let records = cache::evidence_records_with_progress(state, control, &mut |done, total| {
        coverage.processed = done;
        coverage.total = total;
        progress("载入指纹索引", &coverage);
    })?;
    progress("核对检索断点", &DuplicateCoverage::default());
    let input_key = resume::input_key(&conn, &records, &options.sensitivity, control)?;
    let saved = if options.force_recompute {
        None
    } else {
        resume::load(&conn, &input_key)?
    };
    let mut journal = if let Some(saved) = saved {
        saved
    } else {
        progress("清理未发布草稿", &DuplicateCoverage::default());
        cache::delete_unpublished(&mut conn, control)?;
        // Dispose only unpublished work. Keep the active generation, user choices,
        // old full results and valid pair evidence while a replacement is computing.
        for table in ["workflow_class", "workflow_edges"] {
            conn.execute(&format!("DELETE FROM {table} WHERE run NOT IN (SELECT refinement FROM workflow_runs WHERE generation=CAST((SELECT value FROM meta WHERE key='published') AS INTEGER))"),[]).map_err(|e|e.to_string())?;
        }
        conn.execute("DELETE FROM workflow_plan WHERE run NOT IN (SELECT plan FROM workflow_runs WHERE generation=CAST((SELECT value FROM meta WHERE key='published') AS INTEGER))",[]).map_err(|e|e.to_string())?;
        let gen = serial();
        coverage = DuplicateCoverage::default();
        conn.execute("INSERT INTO generations(generation,sensitivity,checked_at,state,coverage,errors) VALUES(?1,?2,?3,'building',?4,'[]')",params![gen,options.sensitivity,now(),serde_json::to_string(&coverage).unwrap()]).map_err(|e|e.to_string())?;
        conn.execute("INSERT INTO workflow_runs(generation) VALUES(?1)", [gen])
            .map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO meta VALUES(?1,?2)",
            params![format!("policy:{gen}"), super::super::evidence::POLICY],
        )
        .map_err(|e| e.to_string())?;
        resume::create(&conn, gen, &input_key)?
    };
    let gen = journal.generation;
    let stamp = cache::membership_token(state);
    coverage = journal.coverage.clone();
    let exact_reused = journal.exact_done;
    let index_resume = std::mem::take(&mut journal.index);
    conn.execute(
        "INSERT OR REPLACE INTO meta VALUES(?1,'candidate')",
        [format!("verification:{gen}")],
    )
    .map_err(|e| e.to_string())?;
    let threshold = match options.sensitivity.as_str() {
        "strict" => index::Threshold::Strict,
        "loose" => index::Threshold::Loose,
        _ => index::Threshold::Standard,
    };
    let mut verifier = verification_cache::Verifier::new(state, false)?;
    verifier.prepare_evidence(control, &mut |done, total| {
        let mut count = DuplicateCoverage::default();
        count.processed = done;
        count.total = total;
        progress("整理验证缓存", &count);
    })?;
    progress(
        if exact_reused {
            "恢复检索断点"
        } else {
            "构建候选索引"
        },
        &coverage,
    );
    let verifier = RefCell::new(verifier);
    let journal = RefCell::new(journal);
    let controls = RefCell::new(control);
    let stats = RefCell::new(coverage.clone());
    let mut last = Instant::now();
    let mut emit = |group: index::DuplicateGroup| -> Result<(), String> {
        if group.kind != index::DuplicateKind::Exact {
            let reference = records[group.reference].image_pk;
            for item in &group.members {
                let pk = records[item.record_index].image_pk;
                if pk != reference {
                    pair(
                        &conn,
                        gen,
                        reference,
                        pk,
                        &super::super::evidence::Evidence::result(
                            "raster",
                            "full_raster_via_byte_unit",
                            true,
                        ),
                        &mut **controls.borrow_mut(),
                    )?;
                }
            }
            return Ok(());
        }
        let reference = records[group.reference].image_pk;
        let number = reference.checked_mul(2).ok_or("图片键超出范围")?;
        for chunk in group.members.chunks(256) {
            cache::controlled_write(&conn, &mut **controls.borrow_mut(), || {
                conn.execute_batch("BEGIN IMMEDIATE")
            })?;
            let result = (|| -> Result<(), String> {
                conn.execute(
                    "INSERT OR IGNORE INTO groups VALUES(?1,?2,0,?3,0)",
                    params![gen, number, reference],
                )
                .map_err(|e| e.to_string())?;
                let mut added = 0;
                for item in chunk {
                    added += member(&conn, gen, number, records[item.record_index].image_pk)?;
                }
                conn.execute("UPDATE groups SET total_members=total_members+?3 WHERE generation=?1 AND group_no=?2",params![gen,number,added]).map_err(|e|e.to_string())?;
                Ok(())
            })();
            if let Err(error) = result {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(error);
            }
            conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
        }
        Ok(())
    };
    let result = index::build_evidence_resumable(
        &records,
        threshold,
        &index_resume,
        &mut || (**controls.borrow_mut())(),
        &mut |a, b, kind| {
            let answer = verifier.borrow_mut().check_evidence(
                records[a].image_pk,
                records[b].image_pk,
                kind == index::DuplicateKind::Exact,
                state,
                &mut **controls.borrow_mut(),
            )?;
            if kind == index::DuplicateKind::Near {
                pair(
                    &conn,
                    gen,
                    records[a].image_pk,
                    records[b].image_pk,
                    &answer,
                    &mut **controls.borrow_mut(),
                )?;
            }
            if exact_reused && kind == index::DuplicateKind::Exact {
                return Ok(answer.automatic);
            }
            let mut stat = stats.borrow_mut();
            stat.processed += 1;
            if answer.automatic {
                stat.verified_pairs += 1;
            } else if answer.level == "uncertain" {
                stat.uncertain_pairs += 1;
            } else {
                stat.content_variants += 1;
            }
            if last.elapsed().as_millis() >= 200 {
                progress("验证候选原图（比较关系）", &stat);
                last = Instant::now();
                cache::check_disk(&cache::cache_path(state))?;
            }
            Ok(answer.automatic)
        },
        &mut emit,
        &mut |checkpoint| {
            let save = journal.borrow_mut().record(checkpoint, &stats.borrow());
            if save {
                verifier
                    .borrow_mut()
                    .flush_evidence(&mut **controls.borrow_mut())?;
                journal
                    .borrow_mut()
                    .flush(&conn, &mut **controls.borrow_mut())?;
            }
            Ok(())
        },
    );
    if result.is_ok() {
        verifier
            .borrow_mut()
            .flush_evidence(&mut **controls.borrow_mut())?;
        journal
            .borrow_mut()
            .flush(&conn, &mut **controls.borrow_mut())?;
    } else if result
        .as_ref()
        .is_err_and(|e| e.contains("断点内容单元已变化"))
    {
        resume::forget(&conn)?;
    }
    drop(verifier);
    drop(records);
    coverage = stats.into_inner();
    coverage.total = coverage.processed;
    let control = controls.into_inner();
    let summary = result?;
    if !summary.too_dense_references.is_empty() {
        return Err(format!(
            "{} 个候选范围超过预算，尚未查尽；保留上次完整结果及已保存证据",
            summary.too_dense_references.len()
        ));
    }
    control()?;
    if stamp != cache::membership_token(state) {
        return Err("检索期间图库成员变化，未替换上次完整结果".into());
    }
    // Recover the original indexing coverage; comparison counts are a different unit.
    conn.execute("UPDATE generations SET coverage=?2,exact_groups=(SELECT COUNT(*) FROM groups WHERE generation=?1 AND kind=0),near_groups=(SELECT COUNT(*) FROM groups WHERE generation=?1 AND kind=1),unique_images=(SELECT COUNT(DISTINCT image_pk) FROM members WHERE generation=?1) WHERE generation=?1",params![gen,indexed_coverage]).map_err(|e|e.to_string())?;
    progress("生成全量整理计划", &coverage);
    loop {
        match planning::publish_evidence(&conn, gen, state, control) {
            Err(e) if e == planning::CHOICES_CHANGED => {
                control()?;
                progress("合并最新人工选择", &coverage);
            }
            value => {
                value?;
                break;
            }
        }
    }
    cache::evidence_published(state, &stamp);
    resume::forget(&conn)?;
    // Old user choices and undo journal stay; only unreferenced derived graphs go.
    let _ = conn.execute("DELETE FROM workflow_class WHERE run<>?1", [gen]);
    let _ = conn.execute("DELETE FROM members WHERE generation<>?1", [gen]);
    let _ = conn.execute("DELETE FROM groups WHERE generation<>?1", [gen]);
    let _ = conn.execute("DELETE FROM generations WHERE generation<>?1", [gen]);
    let _ = conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE)");
    progress("副本检索完成", &coverage);
    Ok(())
}

#[cfg(test)]
#[path = "discovery_resume_tests.rs"]
mod discovery_resume_tests;

//! Paged real groups and optional durable manual choices; never mutates source files.
use super::*;

fn blocks_review(job: &crate::jobs::JobRecord) -> bool {
    matches!(
        job.state.as_str(),
        "queued" | "running" | "pausing" | "paused" | "cancelling"
    ) && match job.request.kind.as_str() {
        "duplicates" => job
            .request
            .duplicate_options
            .as_ref()
            .is_none_or(|options| {
                if options.verification_mode == "cascade" {
                    return !["第一轮", "第二轮", "第三轮", "生成分类整理计划"]
                        .iter()
                        .any(|prefix| job.stage.starts_with(prefix));
                }
                !matches!(options.verification_mode.as_str(), "refine" | "organize")
            }),
        "duplicateManagement" | "duplicateUndo" => true,
        _ => false,
    }
}

fn review_busy(state: &AppState) -> Result<(), String> {
    if state.read_only || state.maintenance.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("图库只读或正在维护".into());
    }
    if state.jobs.list()?.iter().any(blocks_review) {
        return Err("全库扫描、整理或撤销正在执行，暂不可修改保留项；仍可浏览已有分组".into());
    }
    Ok(())
}

#[cfg(test)]
mod activity_tests {
    use super::*;
    #[test]
    fn refinement_allows_manual_choices_but_scans_and_execution_do_not() {
        let mut job: crate::jobs::JobRecord = serde_json::from_value(serde_json::json!({
            "id":"test", "request":{"kind":"duplicates", "thumbnails":false,
            "sourceFolderId":null, "duplicateOperationId":null,
            "duplicateOptions":{"sensitivity":"standard", "forceRecompute":false,
            "regroupOnly":true, "verificationMode":"refine"}},
            "state":"running", "stage":"复检近似候选", "processed":1,"total":4,
            "failures":0,"errors":[],"updatedAt":1
        }))
        .unwrap();
        for state in ["queued", "running", "pausing", "paused", "cancelling"] {
            job.state = state.into();
            assert!(
                !blocks_review(&job),
                "review should remain available during {state} refinement"
            );
            job.request
                .duplicate_options
                .as_mut()
                .unwrap()
                .verification_mode = "candidate".into();
            assert!(
                blocks_review(&job),
                "source fingerprints are being replaced"
            );
            job.request
                .duplicate_options
                .as_mut()
                .unwrap()
                .verification_mode = "refine".into();
            for kind in ["duplicateManagement", "duplicateUndo"] {
                job.request.kind = kind.into();
                assert!(blocks_review(&job), "execution must retain its snapshot");
            }
            job.request.kind = "duplicates".into();
        }
        job.request.duplicate_options = None;
        assert!(
            blocks_review(&job),
            "legacy scan without options remains exclusive"
        );
        job.state = "completed".into();
        assert!(!blocks_review(&job));
    }
}

fn group_number(id: &str, kind: &str) -> Result<i64, String> {
    let (prefix, number) = id.split_once(':').ok_or("分组标识无效")?;
    if prefix != kind {
        return Err("分组分类已变化，请刷新".into());
    }
    number.parse().map_err(|_| "分组标识无效".into())
}
fn cursor(value: &Option<String>, revision: &str, scope: &str) -> Result<i64, String> {
    let Some(value) = value else {
        return Ok(-1);
    };
    let parts: Vec<String> = serde_json::from_str(value).map_err(|_| "分页游标无效")?;
    if parts.len() != 3 || parts[0] != revision || parts[1] != scope {
        return Err("结果或人工选择已变化，请刷新分页".into());
    }
    parts[2].parse().map_err(|_| "分页游标无效".into())
}
fn next(revision: &str, scope: &str, last: i64) -> String {
    serde_json::to_string(&[revision, scope, &last.to_string()]).unwrap()
}

fn group(
    conn: &Connection,
    db: &Connection,
    gen: i64,
    r: &(i64, i64, i64, String),
    kind: &str,
    number: i64,
    start: i64,
    limit: i64,
    state: &AppState,
) -> Result<DuplicateWorkflowGroup, String> {
    let sql = category_sql(kind, r.0)?;
    let cascade: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key=?1 AND value=?2)",
            params![
                format!("policy:{gen}"),
                super::super::cascade_pixels::POLICY
            ],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    // Explicit group lookup includes already recycled records so the user can undo
    // in place. The normal category list contains only live groups of at least two.
    let all=match kind {
        "near" if cascade=>format!("SELECT image_pk FROM ({}) WHERE group_no=?2",category_sql_history(kind,r.0,true)?),
        "recycle"|"retained"=>format!("SELECT image_pk FROM ({}) WHERE group_no=?2",category_sql_history(kind,r.0,true)?),
        "exact"|"candidate"=>format!("SELECT m.image_pk FROM members m JOIN groups g ON g.generation=m.generation AND g.group_no=m.group_no WHERE m.generation=?1 AND m.group_no=?2 AND g.kind={}",if kind=="exact"{0}else{1}),
        _=>format!("SELECT c.image_pk FROM workflow_class c WHERE c.run={} AND c.group_no=?2 AND c.kind='{}' UNION SELECT g.reference_pk FROM groups g WHERE g.generation=?1 AND g.group_no=?2 AND EXISTS(SELECT 1 FROM workflow_class c WHERE c.run={} AND c.group_no=g.group_no AND c.kind='{}')",r.0,kind,r.0,kind)
    };
    let id = format!("{kind}:{number}");
    let total_members = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM ({all})"),
            params![gen, number],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    let original = conn
        .query_row(
            "SELECT reference_pk FROM groups WHERE generation=?1 AND group_no=?2",
            params![gen, number],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or("分组已失效，请刷新")?;
    let reference=conn.query_row(&format!("SELECT a.image_pk FROM ({all}) a JOIN workflow_plan p ON p.run=?3 AND p.image_pk=a.image_pk AND p.keeper IS NULL JOIN library.images i ON i.image_pk=a.image_pk WHERE i.trashed=0 ORDER BY CASE WHEN a.image_pk=(SELECT image_pk FROM workflow_keepers WHERE generation=?1 AND group_no=?2) THEN 0 WHEN a.image_pk=?4 THEN 1 ELSE 2 END,a.image_pk LIMIT 1"),params![gen,number,r.1,original],|row|row.get::<_,i64>(0)).optional().map_err(|e|e.to_string())?.unwrap_or(original);
    let reference_id = management::main_image(db, reference)?
        .map(|i| i.id)
        .unwrap_or_default();
    let pks = {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT image_pk FROM ({all}) WHERE image_pk>?3 ORDER BY image_pk LIMIT ?4"
            ))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![gen, number, start, limit + 1], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        rows
    };
    let more = pks.len() as i64 > limit;
    let mut members = Vec::new();
    for pk in pks.iter().take(limit as usize) {
        let Some(mut image) = management::main_image(db, *pk)? else {
            continue;
        };
        let (public,signature):(Vec<u8>,Vec<u8>)=if cascade && kind=="near" {
            conn.query_row("SELECT public_hash,signature FROM fingerprints WHERE image_pk=?1",[pk],|row|Ok((row.get(0)?,row.get(1)?)))
        } else {
            conn.query_row("SELECT public_hash,signature FROM members WHERE generation=?1 AND group_no=?2 AND image_pk=?3 LIMIT 1",params![gen,number,pk],|row|Ok((row.get(0)?,row.get(1)?)))
        }.map_err(|e|e.to_string())?;
        if public != Sha256::digest(image.id.as_bytes()).to_vec() {
            continue;
        }
        let actual = fingerprint::source_signature(Path::new(&image.path));
        let stale = actual
            .as_ref()
            .map(|info| cache::signature(&image.id, info).to_vec() != signature)
            .unwrap_or(true);
        if let Ok(info) = actual {
            image.file_size = info.file_size as i64;
        }
        if stale {
            cache::mark_stale(state);
        }
        let planned: Option<(Option<i64>, String, i64)> = conn
            .query_row(
                "SELECT keeper,kind,manual FROM workflow_plan WHERE run=?1 AND image_pk=?2",
                params![r.1, pk],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let protected:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM workflow_choices WHERE public_hash=?1 AND signature=?2 AND choice='keep')",params![public,signature],|row|row.get(0)).map_err(|e|e.to_string())?;
        let is_keeper = *pk == reference;
        let disposition = if protected {
            "protected"
        } else if image.trashed {
            "keep"
        } else if planned.as_ref().is_some_and(|p| p.0.is_some()) {
            if planned.as_ref().unwrap().2 != 0 {
                "manual"
            } else {
                "clean"
            }
        } else if kind == "candidate" {
            "pending"
        } else {
            "keep"
        };
        let comparison: Option<String> = conn
            .query_row(
                "SELECT reason FROM workflow_class WHERE run=?1 AND group_no=?2 AND image_pk=?3",
                params![r.0, number, pk],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let comparison_reason = match comparison.as_deref() {
            Some("all_native_samples_equal") => "原生尺寸与全部像素一致",
            Some("multiscale_content_passed") => "全图及局部内容通过近似容差检查",
            Some("alpha_detail_difference") => "透明度细节不同，保留",
            Some("aspect_ratio_difference") => "宽高比存在差异，保留",
            Some("animation_not_verified") | Some("multi_image_container_not_verified") => {
                "动态或多图容器未完成像素验证，保留"
            }
            Some("pixel_or_memory_limit") => "超过当前原图解码范围，保留",
            Some("lossy_uncalibrated") => {
                "重编码或缩放验证符合候选规则；尚未通过真实样本校准，保留供人工判断"
            }
            Some("protected_metadata_difference") => "存在不同的受保护元数据或色彩信息，保留",
            Some("alpha_or_precision_outside_scope") => {
                "透明度或位深超出当前完整像素验证范围，保留"
            }
            Some("local_detail_difference") => "局部像素或边缘存在未被当前规则解释的差异，保留",
            Some("global_pixel_difference") => {
                "整体像素差异超过当前规则，保留；不代表已确认内容不同"
            }
            Some("color_variant") => "整体色彩存在差异，保留",
            Some("aspect_ratio_changed") => "宽高比存在变化，保留",
            Some("insufficient_resolution") => "分辨率不足以判断细节是否相同，保留",
            Some("resolution_gap") => "分辨率差距超出当前验证范围，保留",
            Some("format_outside_raster_scope") => "格式或动态图超出当前完整像素验证范围，保留",
            Some("source_unavailable") | Some("source_changed") => "源文件已变化或无法读取，保留",
            Some("full_raster_equal") | Some("full_raster_via_byte_unit") => {
                "完整像素及受保护信息一致"
            }
            Some("byte_identical") => "文件逐字节相同",
            Some(_) => "当前证据不足以确认可以替代，保留",
            None => "保留基准或未找到可靠替代关系",
        };
        let reason = if image.trashed {
            "已移入应用回收站，可撤销本次整理".into()
        } else if stale {
            "源文件已变化或无法读取，执行时保留".into()
        } else if disposition == "manual" {
            "人工标记；不代表算法已确认同图".into()
        } else if planned
            .as_ref()
            .is_some_and(|p| p.0.is_some() && p.1 == "exact")
        {
            if cascade {
                "原生尺寸与全部像素一致；本组件保留至少一张，人工保留优先".into()
            } else {
                "此文件是字节完全相同的副本；不同内容会保留，相同内容只保留一张".into()
            }
        } else if planned.as_ref().is_some_and(|p| p.0.is_some()) {
            if cascade {
                "已有内容近似证据；整理前直接复核当前保留图".into()
            } else {
                "与最终保留图的完整像素及受保护信息一致".into()
            }
        } else {
            comparison_reason.into()
        };
        members.push(DuplicateReviewMember {
            member: DuplicateMember {
                image,
                metric: DuplicateMetric::default(),
                stale,
            },
            disposition: disposition.into(),
            is_keeper,
            reason,
        });
    }
    let confirmed = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM workflow_confirmed WHERE generation=?1 AND group_id=?2)",
            params![gen, id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    let rev = paging_revision(gen, r);
    let next_cursor = if more {
        pks.get(limit as usize - 1).map(|pk| next(&rev, &id, *pk))
    } else {
        None
    };
    let _ = sql;
    Ok(DuplicateWorkflowGroup {
        id,
        kind: kind.into(),
        reference_id,
        confirmed,
        total_members,
        members,
        next_cursor,
    })
}

pub fn query(
    request: DuplicateWorkflowQuery,
    state: &AppState,
) -> Result<DuplicateWorkflowPage, String> {
    let gate = lock(state);
    let _guard = gate.lock().map_err(|e| e.to_string())?;
    let conn = open(state)?;
    let gen = current(&conn, &request.generation)?;
    ensure(&conn, gen, state)?;
    let r = run(&conn, gen)?;
    let mut group_run = r.clone();
    group_run.1 = category_plan(&conn, gen, &request.kind, state, &mut || Ok(()))?;
    let rev = revision(gen, &r);
    let paging = paging_revision(gen, &r);
    if r.0 == 0 && matches!(request.kind.as_str(), "near" | "similar" | "uncertain") {
        return Err("近似候选尚未完成复检".into());
    }
    let db = cache::library(state)?;
    let (groups, next_cursor, total_groups, paging) = if let Some(id) = request
        .group_id
        .as_ref()
        .filter(|_| request.reanchor != Some(true))
    {
        let number = group_number(&id, &request.kind)?;
        let start = cursor(&request.cursor, &paging, &id)?;
        let item = group(
            &conn,
            &db,
            gen,
            &group_run,
            &request.kind,
            number,
            start,
            request.limit.clamp(1, 50),
            state,
        )?;
        (vec![item], None, 1, None)
    } else {
        let history: bool = conn
            .query_row(
                "SELECT last_operation IS NOT NULL FROM workflow_runs WHERE generation=?1",
                [gen],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        let sql = category_sql_history(&request.kind, r.0, history)?;
        let valid = format!("SELECT group_no FROM ({sql}) GROUP BY group_no HAVING COUNT(*)>1");
        // A newly published refinement or changed membership invalidates old
        // cursors. Seek from a stable group ID without parsing an opaque cursor
        // in the client or walking every intervening page.
        let mut start = if request.reanchor == Some(true) {
            group_number(
                request.group_id.as_deref().ok_or("未指定审查分组")?,
                &request.kind,
            )?
            .saturating_sub(1)
        } else {
            cursor(&request.cursor, &paging, &request.kind)?
        };
        let (total, mut offset): (i64, i64) = conn
            .query_row(
                &format!("SELECT COUNT(*),COALESCE(SUM(group_no<=?2),0) FROM ({valid})"),
                params![gen, start],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| e.to_string())?;
        // If the anchor was the final group and disappeared, keep browsing from
        // the last surviving page rather than presenting a misleading empty list.
        if request.reanchor == Some(true) && total > 0 && offset == total {
            offset = (total - 20).max(0);
            start = if offset == 0 {
                -1
            } else {
                conn.query_row(
                    &format!("SELECT group_no FROM ({valid}) ORDER BY group_no LIMIT 1 OFFSET ?2"),
                    params![gen, offset - 1],
                    |row| row.get(0),
                )
                .map_err(|e| e.to_string())?
            };
        }
        let previous_limit = offset.min(20);
        let previous = if offset > 0 {
            let before = conn
                .query_row(
                    &format!("SELECT group_no FROM ({valid}) WHERE group_no<=?2 ORDER BY group_no DESC LIMIT 1 OFFSET ?3"),
                    params![gen, start, previous_limit],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            before.map(|number| next(&paging, &request.kind, number))
        } else {
            None
        };
        let limit = request.limit.clamp(1, 20);
        let nums = {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT group_no FROM ({valid}) WHERE group_no>?2 ORDER BY group_no LIMIT ?3"
                ))
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params![gen, start, limit + 1], |row| row.get::<_, i64>(0))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            rows
        };
        let more = nums.len() as i64 > limit;
        let mut groups = Vec::new();
        for number in nums.iter().take(limit as usize) {
            groups.push(group(
                &conn,
                &db,
                gen,
                &group_run,
                &request.kind,
                *number,
                -1,
                50,
                state,
            )?);
        }
        let next_cursor = if more {
            nums.get(limit as usize - 1)
                .map(|number| next(&paging, &request.kind, *number))
        } else {
            None
        };
        (
            groups,
            next_cursor,
            total,
            Some(DuplicateWorkflowPaging {
                offset,
                previous_cursor: previous,
                previous_limit,
            }),
        )
    };
    Ok(DuplicateWorkflowPage {
        generation: request.generation,
        revision: rev,
        groups,
        next_cursor,
        total_groups,
        paging,
    })
}

pub fn edit(
    request: DuplicateReviewEdit,
    state: &AppState,
) -> Result<DuplicateWorkflowStatus, String> {
    // Refinement writes a separate, unpublished run. The workflow gate below
    // serializes choice edits with publication, whose plan reads latest choices.
    review_busy(state)?;
    let scan = cache::published_status(state)?;
    let gate = lock(state);
    let _guard = gate.lock().map_err(|e| e.to_string())?;
    let conn = open(state)?;
    let gen = current(&conn, &request.generation)?;
    ensure(&conn, gen, state)?;
    let r = run(&conn, gen)?;
    if revision(gen, &r) != request.revision {
        return Err("人工选择或检索结果已变化，请刷新后重试".into());
    }
    let (kind, _) = request.group_id.split_once(':').ok_or("分组无效")?;
    let cascade: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key=?1 AND value=?2)",
            params![
                format!("policy:{gen}"),
                super::super::cascade_pixels::POLICY
            ],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if cascade && request.action == "clean" && !matches!(kind, "exact" | "near") {
        return Err("类似图和无法确认项默认保留，不加入分类整理".into());
    }
    let number = group_number(&request.group_id, kind)?;
    category_sql(kind, r.0)?;
    if !matches!(
        request.action.as_str(),
        "confirm" | "keep" | "clean" | "keeper"
    ) {
        return Err("审查操作无效".into());
    }
    if request.action == "clean" && (kind == "candidate" || (r.0 == 0 && kind != "exact")) {
        return Err("候选尚未复检，只能设置保留".into());
    }
    if request.action == "confirm" {
        conn.execute(
            "INSERT OR REPLACE INTO workflow_confirmed VALUES(?1,?2)",
            params![gen, request.group_id],
        )
        .map_err(|e| e.to_string())?;
        return summary(&conn, scan, state);
    }
    let image_id = request.image_id.ok_or("未指定图片")?;
    let scope = category_sql(kind, r.0)?;
    let pk:i64=conn.query_row(&format!("SELECT i.image_pk FROM ({scope}) m CROSS JOIN library.images i ON i.image_pk=m.image_pk WHERE m.group_no=?2 AND i.id=?3 AND {ACTIVE}"),params![gen,number,image_id],|row|row.get(0)).optional().map_err(|e|e.to_string())?.ok_or("图片已失效或已回收，请撤销整理后继续审查")?;
    let (public, signature): (Vec<u8>, Vec<u8>) = conn
        .query_row(
            "SELECT public_hash,signature FROM fingerprints WHERE image_pk=?1",
            [pk],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    let info = fingerprint::source_signature(Path::new(&image_id))?;
    if signature != cache::signature(&image_id, &info) {
        return Err("图片已变化，请更新检索结果".into());
    }
    // Invalidate issued previews before changing any durable preference. Even a
    // disk/cancellation failure during replanning cannot leave an old plan executable.
    let invalidate = || {
        conn.execute("INSERT INTO meta VALUES('choice_revision','1') ON CONFLICT(key) DO UPDATE SET value=CAST(CAST(value AS INTEGER)+1 AS TEXT)",[]).map_err(|e|e.to_string())?;
        conn.execute(
            "UPDATE workflow_runs SET revision=revision+1,membership='' WHERE generation=?1",
            [gen],
        )
        .map_err(|e| e.to_string())
    };
    match request.action.as_str() {
        "keep" => {
            invalidate()?;
            // “保留” protects an extra image; it does not silently swap out the
            // survivor. Swapping is a separate explicit “设为保留” action.
            conn.execute("INSERT OR IGNORE INTO workflow_choices SELECT f.public_hash,f.signature,'anchor' FROM workflow_plan p JOIN fingerprints f ON f.image_pk=p.keeper WHERE p.run=?1 AND p.image_pk=?2 AND p.keeper IS NOT NULL",params![r.1,pk]).map_err(|e|e.to_string())?;
            conn.execute(
                "INSERT OR REPLACE INTO workflow_choices VALUES(?1,?2,'keep')",
                params![public, signature],
            )
            .map_err(|e| e.to_string())?;
        }
        "clean" => {
            let survivor:i64=conn.query_row(&format!("SELECT COUNT(*) FROM workflow_plan p JOIN ({scope}) m ON m.image_pk=p.image_pk WHERE m.group_no=?2 AND p.run=?3 AND p.keeper IS NULL"),params![gen,number,r.1],|row|row.get(0)).map_err(|e|e.to_string())?;
            let current_keeper: bool = conn
                .query_row(
                    "SELECT keeper IS NULL FROM workflow_plan WHERE run=?1 AND image_pk=?2",
                    params![r.1, pk],
                    |row| row.get(0),
                )
                .map_err(|e| e.to_string())?;
            if current_keeper && survivor <= 1 {
                return Err("本组至少保留一张，请先指定另一张保留图".into());
            }
            let pinned:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM workflow_keepers WHERE generation=?1 AND image_pk=?2)",params![gen,pk],|row|row.get(0)).map_err(|e|e.to_string())?;
            if pinned {
                return Err("此图是已指定的保留图，请先更换保留图".into());
            }
            invalidate()?;
            conn.execute(
                "INSERT OR REPLACE INTO workflow_choices VALUES(?1,?2,?3)",
                params![
                    public,
                    signature,
                    if matches!(
                        kind,
                        "similar" | "uncertain" | "suggested" | "retained" | "recycle"
                    ) {
                        "manual"
                    } else {
                        "clean"
                    }
                ],
            )
            .map_err(|e| e.to_string())?;
        }
        "keeper" => {
            // Changing the keeper creates only new, directly checked edges. Existing
            // proofs about the old reference can never justify the replacement.
            if r.0 > 0 && kind != "exact" {
                let pks = {
                    let mut stmt=conn.prepare(&format!("SELECT m.image_pk FROM ({scope}) m WHERE m.group_no=?2 AND m.image_pk<>?3")).map_err(|e|e.to_string())?;
                    let rows = stmt
                        .query_map(params![gen, number, pk], |row| row.get::<_, i64>(0))
                        .map_err(|e| e.to_string())?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|e| e.to_string())?;
                    rows
                };
                let mut verifier = verification_cache::Verifier::new(state, false)?;
                for peer in pks {
                    let accepted = if cascade {
                        verifier
                            .check_cascade(pk, peer, state, &mut || Ok(()))?
                            .passed()
                    } else {
                        verifier
                            .check_evidence(pk, peer, false, state, &mut || Ok(()))?
                            .automatic
                    };
                    if accepted {
                        conn.execute(
                            "INSERT OR IGNORE INTO workflow_edges VALUES(?1,?2,?3)",
                            params![r.0, pk, peer],
                        )
                        .map_err(|e| e.to_string())?;
                        conn.execute(
                            "INSERT OR IGNORE INTO workflow_edges VALUES(?1,?2,?3)",
                            params![r.0, peer, pk],
                        )
                        .map_err(|e| e.to_string())?;
                    }
                }
            }
            invalidate()?;
            conn.execute("DELETE FROM workflow_choices WHERE choice='anchor' AND public_hash IN (SELECT f.public_hash FROM members m JOIN fingerprints f ON f.image_pk=m.image_pk WHERE m.generation=?1 AND m.group_no=?2)",params![gen,number]).map_err(|e|e.to_string())?;
            conn.execute(
                "INSERT OR REPLACE INTO workflow_keepers VALUES(?1,?2,?3)",
                params![gen, number, pk],
            )
            .map_err(|e| e.to_string())?;
            // A new keeper overrides a previous explicit clean instruction.
            conn.execute("DELETE FROM workflow_choices WHERE public_hash=?1 AND choice IN ('clean','manual')",[public]).map_err(|e|e.to_string())?;
        }
        _ => return Err("审查操作无效".into()),
    }
    planning::build(&conn, gen, r.0, state, &mut || Ok(()))?;
    conn.execute("DELETE FROM workflow_confirmed WHERE generation=?1", [gen])
        .map_err(|e| e.to_string())?;
    summary(&conn, scan, state)
}

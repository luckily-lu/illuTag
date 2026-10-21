//! A new three-round controller. The old replacement discovery is not its engine.
use super::*;
use std::cell::RefCell;
#[path = "discovery_resume.rs"]
mod resume;
pub(super) const POLICY: &str = super::super::cascade_pixels::POLICY;
const SCHEMA:&str="CREATE TABLE IF NOT EXISTS cascade_runs(generation INTEGER PRIMARY KEY,phase INTEGER NOT NULL DEFAULT 1,pairs INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS cascade_pairs(generation INTEGER NOT NULL,a INTEGER NOT NULL,b INTEGER NOT NULL,reference INTEGER NOT NULL,kind TEXT NOT NULL DEFAULT 'pending',strict INTEGER NOT NULL DEFAULT 0,reason TEXT NOT NULL DEFAULT '',PRIMARY KEY(generation,a,b)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS cascade_pending ON cascade_pairs(generation,kind,a,b);";
const ALIASES:&str="CREATE TABLE IF NOT EXISTS cascade_aliases(generation INTEGER NOT NULL,feature INTEGER NOT NULL,image_pk INTEGER NOT NULL,PRIMARY KEY(generation,feature,image_pk)) WITHOUT ROWID; CREATE INDEX IF NOT EXISTS cascade_alias_image ON cascade_aliases(generation,image_pk,feature);";

#[cfg(test)]
thread_local! {
    static BENCH_OUTER_FLUSHES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static BENCH_ANCHORS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static BENCH_ROUND2_CHECKPOINTS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(super) fn reset_bench_counters() {
    BENCH_OUTER_FLUSHES.with(|value| value.set(0));
    BENCH_ANCHORS.with(|value| value.set(0));
    BENCH_ROUND2_CHECKPOINTS.with(|value| value.set(0));
}

#[cfg(test)]
pub(super) fn bench_counters() -> (u64, u64, u64) {
    (
        BENCH_OUTER_FLUSHES.with(std::cell::Cell::get),
        BENCH_ANCHORS.with(std::cell::Cell::get),
        BENCH_ROUND2_CHECKPOINTS.with(std::cell::Cell::get),
    )
}

fn condense(
    records: Vec<index::IndexRecord>,
    control: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(Vec<index::IndexRecord>, Vec<Vec<i64>>), String> {
    #[derive(Hash, PartialEq, Eq)]
    enum Key {
        Feature(fingerprint::NearFingerprint),
        Bytes(u64, [u8; 32]),
    }
    let mut map = HashMap::<Key, usize>::with_capacity(records.len());
    let mut chosen = Vec::<index::IndexRecord>::new();
    let mut aliases = Vec::<Vec<i64>>::new();
    for (n, record) in records.into_iter().enumerate() {
        if n % 8192 == 0 {
            control()?;
        }
        let key = match &record.near {
            Some(n) => Key::Feature(n.clone()),
            None => Key::Bytes(record.file_size, record.sha256),
        };
        if let Some(&n) = map.get(&key) {
            if aliases[n].is_empty() {
                aliases[n].push(chosen[n].image_pk);
            }
            aliases[n].push(record.image_pk);
            if record.file_size > chosen[n].file_size {
                chosen[n] = record;
            }
        } else {
            map.insert(key, chosen.len());
            chosen.push(record);
            aliases.push(Vec::new());
        }
    }
    Ok((chosen, aliases))
}

fn alias_groups(c: &Connection, g: i64) -> Result<Vec<i64>, String> {
    let mut s = c
        .prepare(
            "SELECT DISTINCT feature FROM cascade_aliases WHERE generation=?1 ORDER BY feature",
        )
        .map_err(|e| e.to_string())?;
    let rows = s
        .query_map([g], |r| r.get(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

fn classify_pool(
    c: &Connection,
    g: i64,
    key: &[u8],
    state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>,
    progress: &mut dyn FnMut(&str, &DuplicateCoverage),
) -> Result<(), String> {
    c.execute_batch("CREATE TABLE IF NOT EXISTS cascade_round2(generation INTEGER PRIMARY KEY,input_key BLOB NOT NULL,next_anchor INTEGER NOT NULL,layout BLOB,covered BLOB NOT NULL,dense TEXT NOT NULL);").map_err(|e|e.to_string())?;
    let pool = {
        let mut s=c.prepare("SELECT a FROM cascade_pairs WHERE generation=?1 UNION SELECT b FROM cascade_pairs WHERE generation=?1").map_err(|e|e.to_string())?;
        let rows = s
            .query_map([g], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<std::collections::HashSet<_>, _>>()
            .map_err(|e| e.to_string())?;
        rows
    };
    let mut records = cache::evidence_records_with_progress(state, control, &mut |n, total| {
        progress(
            "第二轮：载入候选指纹",
            &DuplicateCoverage {
                processed: n,
                total,
                ..Default::default()
            },
        )
    })?;
    records.retain(|r| pool.contains(&r.image_pk));
    drop(pool);
    records.shrink_to_fit();
    let saved:Option<(usize,Option<Vec<u8>>,Vec<u8>,String)>=c.query_row("SELECT next_anchor,layout,covered,dense FROM cascade_round2 WHERE generation=?1 AND input_key=?2",params![g,key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(|e|e.to_string())?;
    let mut resume = index::EvidenceResume::default();
    let mut covered = vec![0_u8; records.len().div_ceil(8)];
    if let Some((next, layout, bits, dense)) = saved {
        if bits.len() != covered.len() {
            return Err("第二轮断点候选池已变化，请重新检索".into());
        }
        resume.next_anchor = next;
        resume.layout = layout
            .map(|x| x.try_into().map_err(|_| "第二轮断点损坏".to_owned()))
            .transpose()?;
        for (n, &byte) in bits.iter().enumerate() {
            for bit in 0..8 {
                if byte & (1 << bit) != 0 {
                    resume.covered.insert(n * 8 + bit);
                }
            }
        }
        resume.dense = serde_json::from_str(&dense).map_err(|_| "第二轮断点损坏")?;
        covered = bits;
    }
    let controls = RefCell::new(control);
    let verifier = RefCell::new(verification_cache::Verifier::new(state, false)?);
    verifier
        .borrow_mut()
        .configure_work_memory(index::estimate_memory_bytes(records.len()));
    verifier.borrow_mut().batch_writes = true;
    let analyses = RefCell::new(Vec::<(i64, i64, super::super::cascade_pixels::Analysis)>::new());
    let bits = RefCell::new(covered);
    let dense = RefCell::new(resume.dense.clone());
    let next = std::cell::Cell::new(resume.next_anchor);
    let layout = RefCell::new(resume.layout);
    let mut last = Instant::now();
    let checked = std::cell::Cell::new(0_i64);
    let flush = || -> Result<(), String> {
        cache::controlled_write(c, &mut **controls.borrow_mut(), || {
            c.execute(
                "INSERT OR REPLACE INTO cascade_round2 VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    g,
                    key,
                    next.get(),
                    layout.borrow().as_ref().map(|x| x.as_slice()),
                    &*bits.borrow(),
                    serde_json::to_string(&*dense.borrow()).unwrap()
                ],
            )
        })?;
        #[cfg(test)]
        BENCH_ROUND2_CHECKPOINTS.with(|value| value.set(value.get() + 1));
        Ok(())
    };
    let flush_analyses = |control: &mut dyn FnMut() -> Result<(), String>| -> Result<(), String> {
        if analyses.borrow().is_empty() {
            return Ok(());
        }
        cache::controlled_write(c, control, || {
            c.execute_batch("BEGIN IMMEDIATE")
        })?;
        let result = (|| -> Result<(), String> {
            let mut insert = c
                .prepare_cached(
                    "INSERT INTO cascade_pairs(generation,a,b,reference,kind,strict,reason)
                     VALUES(?1,?2,?3,?2,?4,?5,?6)
                     ON CONFLICT(generation,a,b) DO UPDATE SET
                       kind=excluded.kind,strict=excluded.strict,reason=excluded.reason",
                )
                .map_err(|error| error.to_string())?;
            for (a, b, answer) in analyses.borrow().iter() {
                insert
                    .execute(params![
                        g,
                        a.min(b),
                        a.max(b),
                        answer.kind,
                        answer.strict as i64,
                        answer.reason
                    ])
                    .map_err(|error| error.to_string())?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            let _ = c.execute_batch("ROLLBACK");
            return Err(error);
        }
        c.execute_batch("COMMIT").map_err(|error| error.to_string())?;
        analyses.borrow_mut().clear();
        #[cfg(test)]
        BENCH_OUTER_FLUSHES.with(|value| value.set(value.get() + 1));
        Ok(())
    };
    let mut pair = |a: usize, b: usize| -> Result<super::super::cascade_pixels::Analysis, String> {
        let answer = verifier.borrow_mut().check_cascade(
            records[a].image_pk,
            records[b].image_pk,
            state,
            &mut **controls.borrow_mut(),
        )?;
        analyses.borrow_mut().push((
            records[a].image_pk,
            records[b].image_pk,
            answer.clone(),
        ));
        if analyses.borrow().len() >= 512 {
            flush_analyses(&mut **controls.borrow_mut())?;
        }
        if verifier.borrow().pending_write_count() >= 256 {
            verifier
                .borrow_mut()
                .flush_verdicts(&mut **controls.borrow_mut())?;
        }
        checked.set(checked.get() + 1);
        progress(
            "第二轮：验证候选关系",
            &DuplicateCoverage {
                processed: checked.get(),
                ..Default::default()
            },
        );
        Ok(answer)
    };
    let pair = RefCell::new(&mut pair);
    let result = index::build_evidence_resumable(
        &records,
        index::Threshold::Cascade,
        &resume,
        &mut || (**controls.borrow_mut())(),
        &mut |a, b, kind| {
            let answer = (pair.borrow_mut())(a, b)?;
            Ok(if kind == index::DuplicateKind::Exact {
                answer.strict
            } else {
                answer.passed()
            })
        },
        &mut |group| {
            // Byte content units are a compact numeric optimization, not permission
            // to leave any actual target without its own directly verified keeper.
            for member in group.members {
                if member.record_index != group.reference {
                    (pair.borrow_mut())(group.reference, member.record_index)?;
                }
            }
            Ok(())
        },
        &mut |anchor| {
            #[cfg(test)]
            BENCH_ANCHORS.with(|value| value.set(value.get() + 1));
            next.set(anchor.next_anchor);
            *layout.borrow_mut() = Some(anchor.layout);
            for &unit in anchor.newly_covered {
                if unit / 8 >= bits.borrow().len() {
                    return Err("第二轮断点单元超出范围".into());
                }
                bits.borrow_mut()[unit / 8] |= 1 << (unit % 8);
            }
            if let Some(reference) = anchor.dense_reference {
                dense.borrow_mut().push(reference);
            }
            if anchor.next_anchor % 512 == 0 || last.elapsed().as_secs() >= 1 {
                flush_analyses(&mut **controls.borrow_mut())?;
                verifier
                    .borrow_mut()
                    .flush_verdicts(&mut **controls.borrow_mut())?;
                flush()?;
                cache::check_disk(&cache::cache_path(state))?;
                last = Instant::now();
            }
            Ok(())
        },
    );
    if result.is_ok() {
        flush_analyses(&mut **controls.borrow_mut())?;
        verifier
            .borrow_mut()
            .flush_verdicts(&mut **controls.borrow_mut())?;
        flush()?;
    } else {
        // Persist already-started relationships and verdicts for the draft even
        // when the cancellation/stop hook would reject further writes. This is a
        // bounded best-effort flush of work that was already completed.
        let mut ignore_cancel = || Ok::<(), String>(());
        let _ = flush_analyses(&mut ignore_cancel);
        let _ = verifier
            .borrow_mut()
            .flush_verdicts(&mut ignore_cancel);
    }
    let result = result?;
    if !result.too_dense_references.is_empty() {
        return Err(format!(
            "第二轮{}个有效候选范围尚未查尽，保留已完成验证与断点",
            result.too_dense_references.len()
        ));
    }
    Ok(())
}
fn alias_members(c: &Connection, g: i64, feature: i64) -> Result<Vec<i64>, String> {
    let mut s=c.prepare("SELECT image_pk FROM cascade_aliases WHERE generation=?1 AND feature=?2 ORDER BY image_pk").map_err(|e|e.to_string())?;
    let rows = s
        .query_map(params![g, feature], |r| r.get(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}
fn save_analysis(
    c: &Connection,
    g: i64,
    a: i64,
    b: i64,
    answer: &super::super::cascade_pixels::Analysis,
    control: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    cache::controlled_write(c, control, || {
        c.execute("INSERT INTO cascade_pairs(generation,a,b,reference,kind,strict,reason) VALUES(?1,?2,?3,?2,?4,?5,?6) ON CONFLICT(generation,a,b) DO UPDATE SET kind=excluded.kind,strict=excluded.strict,reason=excluded.reason",params![g,a.min(b),a.max(b),answer.kind,answer.strict,answer.reason])
    })?;
    Ok(())
}
fn strict_aliases(
    c: &Connection,
    g: i64,
    state: &AppState,
    verifier: &mut verification_cache::Verifier,
    control: &mut dyn FnMut() -> Result<(), String>,
    progress: &mut dyn FnMut(&str, &DuplicateCoverage),
) -> Result<(), String> {
    let mut processed = 0;
    for feature in alias_groups(c, g)? {
        let members = alias_members(c, g, feature)?;
        let has_near:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM cascade_aliases x CROSS JOIN cascade_pairs p ON p.generation=x.generation AND p.a=x.image_pk WHERE x.generation=?1 AND x.feature=?2 AND p.kind='near') OR EXISTS(SELECT 1 FROM cascade_aliases x CROSS JOIN cascade_pairs p INDEXED BY cascade_pair_b ON p.generation=x.generation AND p.b=x.image_pk WHERE x.generation=?1 AND x.feature=?2 AND p.kind='near')",params![g,feature],|r|r.get(0)).map_err(|e|e.to_string())?;
        if !has_near {
            continue;
        }
        let mut by_pixels = HashMap::<Vec<u8>, i64>::new();
        for pk in members {
            control()?;
            let passed:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM cascade_pairs WHERE generation=?1 AND a=?2 AND kind IN ('exact','near')) OR EXISTS(SELECT 1 FROM cascade_pairs INDEXED BY cascade_pair_b WHERE generation=?1 AND b=?2 AND kind IN ('exact','near'))",params![g,pk],|r|r.get(0)).map_err(|e|e.to_string())?;
            if !passed {
                continue;
            }
            if let Some(digest) = verifier.native_digest(pk, state, control)? {
                if let Some(&peer) = by_pixels.get(&digest) {
                    let answer = verifier.check_cascade(peer, pk, state, control)?;
                    if answer.strict {
                        save_analysis(c, g, peer, pk, &answer, control)?;
                    }
                } else {
                    by_pixels.insert(digest, pk);
                }
            }
            processed += 1;
            progress(
                "第三轮：核对原生像素摘要",
                &DuplicateCoverage {
                    processed,
                    ..Default::default()
                },
            );
        }
    }
    Ok(())
}

pub(super) fn rounds(c: &Connection, g: i64) -> Result<Vec<DuplicateRoundStatus>, String> {
    c.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    let phase = c
        .query_row(
            "SELECT phase FROM cascade_runs WHERE generation=?1",
            [g],
            |r| r.get::<_, i64>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .unwrap_or(0);
    let (all,passed,exact,similar,uncertain):(i64,i64,i64,i64,i64)=c.query_row("SELECT COUNT(*),COALESCE(SUM(kind IN ('exact','near')),0),COALESCE(SUM(kind='exact'),0),COALESCE(SUM(kind='similar'),0),COALESCE(SUM(kind='uncertain'),0) FROM cascade_pairs WHERE generation=?1",[g],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).map_err(|e|e.to_string())?;
    Ok([
        (1, "宽筛候选", all, all, 0, 0),
        (2, "内容筛选", all, passed, similar, uncertain),
        (3, "像素确认", passed, exact, passed - exact, 0),
    ]
    .into_iter()
    .map(|(n, name, input, yes, no, unknown)| DuplicateRoundStatus {
        round: n,
        name: name.into(),
        state: if phase > n {
            "completed"
        } else if phase == n {
            "pending"
        } else {
            "waiting"
        }
        .into(),
        input_pairs: input,
        passed_pairs: yes,
        filtered_pairs: no,
        uncertain_pairs: unknown,
    })
    .collect())
}

fn flush_pairs(
    c: &Connection,
    g: i64,
    pending: &mut Vec<(i64, i64, i64)>,
    control: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    if pending.is_empty() {
        return Ok(());
    }
    cache::controlled_write(c, control, || c.execute_batch("BEGIN IMMEDIATE"))?;
    let result = (|| {
        let mut insert = c
            .prepare_cached(
                "INSERT OR IGNORE INTO cascade_pairs(generation,a,b,reference) VALUES(?1,?2,?3,?4)",
            )
            .map_err(|e| e.to_string())?;
        for &(a, b, reference) in pending.iter() {
            insert
                .execute(params![g, a, b, reference])
                .map_err(|e| e.to_string())?;
        }
        Ok::<_, String>(())
    })();
    if let Err(e) = result {
        let _ = c.execute_batch("ROLLBACK");
        return Err(e);
    }
    c.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    pending.clear();
    Ok(())
}
fn add(p: &mut Vec<(i64, i64, i64)>, a: i64, b: i64) {
    if a != b {
        p.push((a.min(b), a.max(b), a));
    }
}
fn insert_member(c: &Connection, g: i64, group: i64, pk: i64) -> Result<usize, String> {
    c.execute("INSERT OR IGNORE INTO members SELECT ?1,?2,image_pk,image_pk,public_hash,signature,NULL,NULL,NULL FROM fingerprints WHERE image_pk=?3",params![g,group,pk]).map_err(|e|e.to_string())
}
fn materialize(
    c: &Connection,
    g: i64,
    state: &AppState,
    control: &mut dyn FnMut() -> Result<(), String>,
    progress: &mut dyn FnMut(&str, &DuplicateCoverage),
) -> Result<(), String> {
    // Rebuilding an unpublished third-round graph is idempotent; comparison
    // outcomes remain durable and no images are decoded here.
    for (table, key) in [
        ("members", "generation"),
        ("groups", "generation"),
        ("workflow_class", "run"),
        ("workflow_edges", "run"),
    ] {
        cache::controlled_write(c, control, || {
            c.execute(&format!("DELETE FROM {table} WHERE {key}=?1"), [g])
        })?;
    }
    let mut parent = HashMap::<i64, i64>::new();
    fn root(parent: &mut HashMap<i64, i64>, x: i64) -> i64 {
        let mut n = x;
        while let Some(&p) = parent.get(&n) {
            if n == p {
                break;
            }
            n = p;
        }
        let mut v = x;
        while let Some(&p) = parent.get(&v) {
            if p == v {
                break;
            }
            parent.insert(v, n);
            v = p;
        }
        n
    }
    let total: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM cascade_pairs WHERE generation=?1 AND kind IN ('exact','near')",
            [g],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let mut cursor = (-1_i64, -1_i64);
    let mut count = 0;
    loop {
        control()?;
        let rows = {
            let mut s=c.prepare("SELECT a,b,reference,kind,reason FROM cascade_pairs WHERE generation=?1 AND (a,b)>(?2,?3) ORDER BY a,b LIMIT 128").map_err(|e|e.to_string())?;
            let rows = s
                .query_map(params![g, cursor.0, cursor.1], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            rows
        };
        if rows.is_empty() {
            break;
        }
        cache::check_disk(&cache::cache_path(state))?;
        cache::controlled_write(c, control, || c.execute_batch("BEGIN IMMEDIATE"))?;
        let result = (|| {
            for (a, b, reference, kind, reason) in &rows {
                if kind == "exact" {
                    let x = root(&mut parent, *a);
                    let y = root(&mut parent, *b);
                    parent.insert(x, x.min(y));
                    parent.insert(y, x.min(y));
                    count += 1;
                    continue;
                }
                let peer = if reference == a { *b } else { *a };
                let group = reference
                    .checked_mul(2)
                    .and_then(|n| n.checked_add(1))
                    .ok_or("图片键超出范围")?;
                c.execute(
                    "INSERT OR IGNORE INTO groups VALUES(?1,?2,1,?3,0)",
                    params![g, group, reference],
                )
                .map_err(|e| e.to_string())?;
                let n = insert_member(c, g, group, *reference)? + insert_member(c, g, group, peer)?;
                c.execute("UPDATE groups SET total_members=total_members+?3 WHERE generation=?1 AND group_no=?2",params![g,group,n]).map_err(|e|e.to_string())?;
                c.execute(
                    "INSERT OR REPLACE INTO workflow_class VALUES(?1,?2,?3,?4,?5)",
                    params![g, group, peer, kind, reason],
                )
                .map_err(|e| e.to_string())?;
                if kind == "near" {
                    for (x, y) in [(*a, *b), (*b, *a)] {
                        c.execute(
                            "INSERT OR IGNORE INTO workflow_edges VALUES(?1,?2,?3)",
                            params![g, x, y],
                        )
                        .map_err(|e| e.to_string())?;
                    }
                    count += 1;
                }
            }
            Ok::<_, String>(())
        })();
        if let Err(e) = result {
            let _ = c.execute_batch("ROLLBACK");
            return Err(e);
        }
        c.execute_batch("COMMIT").map_err(|e| e.to_string())?;
        let last = rows.last().unwrap();
        cursor = (last.0, last.1);
        progress(
            "第三轮：确认完整像素",
            &DuplicateCoverage {
                processed: count,
                total,
                ..Default::default()
            },
        );
    }
    // Exact equality is transitive; tolerant near edges are never unioned.
    let keys: Vec<_> = parent.keys().copied().collect();
    let mut components = HashMap::<i64, Vec<i64>>::new();
    for pk in keys {
        let r = root(&mut parent, pk);
        components.entry(r).or_default().push(pk);
    }
    for (first, mut members) in components {
        control()?;
        members.sort_unstable();
        let group = first.checked_mul(2).ok_or("图片键超出范围")?;
        cache::controlled_write(c, control, || {
            c.execute(
                "INSERT INTO groups VALUES(?1,?2,0,?3,?4)",
                params![g, group, members[0], members.len() as i64],
            )
        })?;
        for chunk in members.chunks(128) {
            cache::controlled_write(c, control, || c.execute_batch("BEGIN IMMEDIATE"))?;
            let result = (|| {
                for &pk in chunk {
                    insert_member(c, g, group, pk)?;
                }
                Ok::<_, String>(())
            })();
            if let Err(e) = result {
                let _ = c.execute_batch("ROLLBACK");
                return Err(e);
            }
            c.execute_batch("COMMIT").map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub(super) fn run(
    options: DuplicateOptions,
    state: &AppState,
    mut control: &mut dyn FnMut() -> Result<(), String>,
    progress: &mut dyn FnMut(&str, &DuplicateCoverage),
) -> Result<(), String> {
    progress("准备三轮检索", &DuplicateCoverage::default());
    let initial = cache::published_status(state)?;
    let source = if initial.generation.is_none() || initial.needs_update || options.force_recompute
    {
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
        let c = cache::open_existing(state, true)?;
        c.query_row(
            "SELECT value FROM meta WHERE key='prepared_index'",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or("指纹检查未完成，保留已有结果")?
    } else {
        initial.generation.unwrap()
    };
    let mut c = open(state)?;
    c.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    c.execute_batch(ALIASES).map_err(|e| e.to_string())?;
    c.execute_batch(
        "CREATE INDEX IF NOT EXISTS cascade_pair_b ON cascade_pairs(generation,b,a,kind);",
    )
    .map_err(|e| e.to_string())?;
    let indexed: String = c
        .query_row(
            "SELECT coverage FROM generations WHERE generation=?1",
            [&source],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let records = cache::evidence_records_with_progress(state, control, &mut |done, total| {
        progress(
            "载入指纹索引",
            &DuplicateCoverage {
                processed: done,
                total,
                ..Default::default()
            },
        )
    })?;
    let stamp = cache::membership_token(state);
    let key = resume::input_key(&c, &records, POLICY, control)?;
    let saved = if options.force_recompute {
        None
    } else {
        resume::load(&c, &key)?
    };
    let mut journal = if let Some(s) = saved {
        s
    } else {
        cache::delete_unpublished(&mut c, control)?;
        for table in [
            "cascade_aliases",
            "cascade_pairs",
            "cascade_runs",
            "cascade_round2",
        ] {
            let exists: bool = c
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                    [table],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if !exists {
                continue;
            }
            cache::controlled_write(&c, control, || {
                c.execute(&format!("DELETE FROM {table} WHERE generation<>COALESCE(CAST((SELECT value FROM meta WHERE key='published') AS INTEGER),-1)"),[])
            })?;
        }
        let g = serial();
        c.execute("INSERT INTO generations(generation,sensitivity,checked_at,state,coverage,errors) VALUES(?1,'cascade',?2,'building',?3,'[]')",params![g,now(),indexed]).map_err(|e|e.to_string())?;
        c.execute("INSERT INTO workflow_runs(generation) VALUES(?1)", [g])
            .map_err(|e| e.to_string())?;
        c.execute("INSERT INTO cascade_runs(generation) VALUES(?1)", [g])
            .map_err(|e| e.to_string())?;
        c.execute(
            "INSERT OR REPLACE INTO meta VALUES(?1,?2)",
            params![format!("policy:{g}"), POLICY],
        )
        .map_err(|e| e.to_string())?;
        c.execute(
            "INSERT OR REPLACE INTO meta VALUES(?1,'candidate')",
            [format!("verification:{g}")],
        )
        .map_err(|e| e.to_string())?;
        resume::create(&c, g, &key)?
    };
    let g = journal.generation;
    let mut records = Some(records);
    let phase: i64 = c
        .query_row(
            "SELECT phase FROM cascade_runs WHERE generation=?1",
            [g],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if phase == 1 {
        let (records, aliases) = condense(records.take().unwrap(), control)?;
        let mut initial = Vec::new();
        for (n, members) in aliases.iter().enumerate() {
            if members.is_empty() {
                continue;
            }
            control()?;
            let feature = *members.iter().min().unwrap();
            for chunk in members.chunks(128) {
                cache::controlled_write(&c, control, || c.execute_batch("BEGIN IMMEDIATE"))?;
                let result = (|| {
                    for &pk in chunk {
                        c.execute(
                            "INSERT OR IGNORE INTO cascade_aliases VALUES(?1,?2,?3)",
                            params![g, feature, pk],
                        )
                        .map_err(|e| e.to_string())?;
                        add(&mut initial, records[n].image_pk, pk);
                    }
                    Ok::<_, String>(())
                })();
                if let Err(e) = result {
                    let _ = c.execute_batch("ROLLBACK");
                    return Err(e);
                }
                c.execute_batch("COMMIT").map_err(|e| e.to_string())?;
                flush_pairs(&c, g, &mut initial, control)?;
                cache::check_disk(&cache::cache_path(state))?;
                progress("第一轮：压缩同指纹候选", &DuplicateCoverage::default());
            }
        }
        drop(aliases);
        let index_resume = std::mem::take(&mut journal.index);
        let pending = RefCell::new(Vec::new());
        let journal = RefCell::new(journal);
        let check = RefCell::new(control);
        let verifier = RefCell::new(verification_cache::Verifier::new(state, false)?);
        let mut last = Instant::now();
        progress(
            "第一轮：宽筛全库候选",
            &DuplicateCoverage {
                total: records.len() as i64,
                ..Default::default()
            },
        );
        let result = index::build_evidence_resumable(
            &records,
            index::Threshold::Cascade,
            &index_resume,
            &mut || (**check.borrow_mut())(),
            &mut |a, b, kind| {
                if kind == index::DuplicateKind::Exact {
                    let yes = verifier.borrow_mut().check(
                        records[a].image_pk,
                        records[b].image_pk,
                        true,
                        state,
                        &mut **check.borrow_mut(),
                    )? == verification::VerificationOutcome::Identical;
                    if yes {
                        add(
                            &mut pending.borrow_mut(),
                            records[a].image_pk,
                            records[b].image_pk,
                        );
                    }
                    return Ok(yes);
                }
                // This cover means candidate-pool inclusion only. Persist the
                // emitted numeric stars below; do not expand every broad edge.
                // Round two rebuilds the pool index and requires pixel proof.
                Ok(true)
            },
            &mut |group| {
                for member in group.members {
                    add(
                        &mut pending.borrow_mut(),
                        records[group.reference].image_pk,
                        records[member.record_index].image_pk,
                    );
                }
                flush_pairs(&c, g, &mut pending.borrow_mut(), &mut **check.borrow_mut())
            },
            &mut |anchor| {
                let coverage = DuplicateCoverage {
                    processed: anchor.next_anchor as i64,
                    total: records.len() as i64,
                    ..Default::default()
                };
                let flush = journal.borrow_mut().record(anchor, &coverage);
                if flush {
                    flush_pairs(&c, g, &mut pending.borrow_mut(), &mut **check.borrow_mut())?;
                    journal.borrow_mut().flush(&c, &mut **check.borrow_mut())?;
                    cache::check_disk(&cache::cache_path(state))?;
                }
                if last.elapsed().as_millis() >= 200 {
                    progress("第一轮：宽筛全库候选", &coverage);
                    last = Instant::now();
                }
                Ok(())
            },
        );
        if result.is_ok() {
            flush_pairs(&c, g, &mut pending.borrow_mut(), &mut **check.borrow_mut())?;
            journal.borrow_mut().flush(&c, &mut **check.borrow_mut())?;
        }
        let summary = result?;
        if !summary.too_dense_references.is_empty() {
            return Err(format!(
                "第一轮{}个有效候选范围仍超出预算，尚未查尽；保留旧完整结果和本轮断点",
                summary.too_dense_references.len()
            ));
        }
        control = check.into_inner();
        c.execute("UPDATE cascade_runs SET phase=2,pairs=(SELECT COUNT(*) FROM cascade_pairs WHERE generation=?1) WHERE generation=?1",[g]).map_err(|e|e.to_string())?;
    }
    // Decode phase never retains the full numeric index.
    drop(records);
    classify_pool(&c, g, &key, state, control, progress)?;
    c.execute(
        "DELETE FROM cascade_pairs WHERE generation=?1 AND kind='pending'",
        [g],
    )
    .map_err(|e| e.to_string())?;
    let mut verifier = verification_cache::Verifier::new(state, false)?;
    c.execute("UPDATE cascade_runs SET phase=3 WHERE generation=?1", [g])
        .map_err(|e| e.to_string())?;
    strict_aliases(&c, g, state, &mut verifier, control, progress)?;
    c.execute("UPDATE cascade_runs SET pairs=(SELECT COUNT(*) FROM cascade_pairs WHERE generation=?1) WHERE generation=?1",[g]).map_err(|e|e.to_string())?;
    let total: i64 = c
        .query_row(
            "SELECT pairs FROM cascade_runs WHERE generation=?1",
            [g],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    materialize(&c, g, state, control, progress)?;
    if stamp != cache::membership_token(state) {
        return Err("三轮检索期间图库成员变化，未替换上次完整结果".into());
    }
    c.execute("UPDATE generations SET coverage=?2,exact_groups=(SELECT COUNT(*) FROM groups WHERE generation=?1 AND kind=0),near_groups=(SELECT COUNT(*) FROM groups WHERE generation=?1 AND kind=1),unique_images=(SELECT COUNT(DISTINCT image_pk) FROM members WHERE generation=?1) WHERE generation=?1",params![g,indexed]).map_err(|e|e.to_string())?;
    progress(
        "生成分类整理计划",
        &DuplicateCoverage {
            processed: total,
            total,
            ..Default::default()
        },
    );
    loop {
        match planning::publish_evidence(&c, g, state, control) {
            Err(e) if e == planning::CHOICES_CHANGED => control()?,
            result => {
                result?;
                break;
            }
        }
    }
    cache::evidence_published(state, &stamp);
    let _ = resume::forget(&c);
    progress("第三轮：回收旧版验证缓存", &DuplicateCoverage::default());
    // The complete pointer is already committed. Optional retirement must not
    // turn that successful publication into a failed or discarded result.
    if let Err(error)=super::super::cascade_gc::retire_legacy(&c,g,state,control){
        eprintln!("[duplicate-legacy-cache-retirement] {error}");
    }
    if let Err(error) = super::super::cascade_gc::retire_stale_cascade(&c, g, state, control) {
        eprintln!("[duplicate-stale-cascade-retirement] {error}");
    }
    for (table, key) in [
        ("cascade_aliases", "generation"),
        ("cascade_pairs", "generation"),
        ("cascade_runs", "generation"),
        ("cascade_round2", "generation"),
        ("workflow_class", "run"),
        ("members", "generation"),
        ("groups", "generation"),
        ("generations", "generation"),
    ] {
        let _ = c.execute(&format!("DELETE FROM {table} WHERE {key}<>?1"), [g]);
    }
    let _ = c.execute_batch("PRAGMA wal_checkpoint(PASSIVE)");
    progress(
        "三轮检索完成",
        &DuplicateCoverage {
            processed: total,
            total,
            ..Default::default()
        },
    );
    Ok(())
}

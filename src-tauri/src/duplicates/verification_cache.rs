//! Pair decisions are disposable, bounded cache entries, never main-library writes.
use super::{
    cache, fingerprint, management,
    verification::{self, VerificationOutcome},
};
use crate::library::AppState;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::path::Path;

#[cfg(test)]
use std::cell::Cell;

#[cfg(test)]
thread_local! {
    static BENCH_PAIR_CHECK_TX: Cell<u64> = const { Cell::new(0) };
    static BENCH_VERDICT_TX: Cell<u64> = const { Cell::new(0) };
}

#[cfg(test)]
pub(super) fn reset_batch_counters() {
    BENCH_PAIR_CHECK_TX.with(|value| value.set(0));
    BENCH_VERDICT_TX.with(|value| value.set(0));
}

#[cfg(test)]
pub(super) fn batch_counters() -> (u64, u64) {
    (
        BENCH_PAIR_CHECK_TX.with(Cell::get),
        BENCH_VERDICT_TX.with(Cell::get),
    )
}

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS pair_checks(
 a INTEGER NOT NULL,b INTEGER NOT NULL,kind INTEGER NOT NULL,
 a_signature BLOB NOT NULL,b_signature BLOB NOT NULL,a_sha BLOB NOT NULL,b_sha BLOB NOT NULL,
 algorithm TEXT NOT NULL,outcome INTEGER NOT NULL,
 PRIMARY KEY(a,b,kind)) WITHOUT ROWID;
 CREATE TABLE IF NOT EXISTS pair_evidence(a INTEGER NOT NULL,b INTEGER NOT NULL,algorithm TEXT NOT NULL,a_signature BLOB NOT NULL,b_signature BLOB NOT NULL,a_sha BLOB NOT NULL,b_sha BLOB NOT NULL,result TEXT NOT NULL,PRIMARY KEY(a,b)) WITHOUT ROWID;";

pub(super) struct Verifier {
    conn: Connection,
    db: Connection,
    checks: usize,
    pub accepted: i64,
    pub variants: i64,
    pub uncertain: i64,
    pending: Vec<PendingEvidence>,
    evidence_ready: bool,
    /// Source-version keyed answers for repeated/reverse pairs in one run.
    pair_cache: PairCache,
    pub(super) pair_cache_hits: u64,
    /// Cascade workflow batches verdict writes through `flush_verdicts`.
    pub(super) batch_writes: bool,
    pending_verdicts: Vec<PendingVerdict>,
    /// Byte-equality checks share the same bounded write transaction.
    pending_checks: Vec<PendingCheck>,
    pair_check_transactions: u64,
    pair_check_rows: u64,
    verdict_transactions: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct PairToken {
    signature: [u8; 32],
    sha: [u8; 32],
}

#[derive(Clone, Copy)]
struct CachedAnswer {
    kind: u8,
    reason: u8,
    strict: bool,
}

const CACHED_EXACT: u8 = 1;
const CACHED_NEAR: u8 = 2;
const CACHED_SIMILAR: u8 = 3;

fn cached_kind(kind: &str) -> Option<u8> {
    match kind {
        "exact" => Some(CACHED_EXACT),
        "near" => Some(CACHED_NEAR),
        "similar" => Some(CACHED_SIMILAR),
        _ => None,
    }
}

fn cached_kind_name(code: u8) -> &'static str {
    match code {
        CACHED_EXACT => "exact",
        CACHED_NEAR => "near",
        _ => "similar",
    }
}

fn cached_reason(reason: &str) -> Option<u8> {
    Some(match reason {
        "all_native_samples_equal" => 1,
        "multiscale_content_passed" => 2,
        "alpha_detail_difference" => 3,
        "aspect_ratio_difference" => 4,
        "color_variant" => 5,
        "global_pixel_difference" => 6,
        "local_detail_difference" => 7,
        "measured_detail_difference" => 8,
        "byte_identical" => 9,
        _ => return None,
    })
}

fn cached_reason_name(code: u8) -> &'static str {
    match code {
        1 => "all_native_samples_equal",
        2 => "multiscale_content_passed",
        3 => "alpha_detail_difference",
        4 => "aspect_ratio_difference",
        5 => "color_variant",
        6 => "global_pixel_difference",
        7 => "local_detail_difference",
        8 => "measured_detail_difference",
        _ => "byte_identical",
    }
}

impl CachedAnswer {
    fn from_analysis(answer: &super::cascade_pixels::Analysis) -> Option<Self> {
        Some(Self {
            kind: cached_kind(&answer.kind)?,
            reason: cached_reason(&answer.reason)?,
            strict: answer.strict,
        })
    }

    fn into_analysis(self) -> super::cascade_pixels::Analysis {
        super::cascade_pixels::Analysis::new(
            cached_kind_name(self.kind),
            cached_reason_name(self.reason),
            self.strict,
        )
    }
}

const SLOT_EMPTY: u8 = 0;
const SLOT_OCCUPIED: u8 = 1;
const SLOT_TOMBSTONE: u8 = 2;

#[derive(Clone, Copy)]
struct PairSlot {
    key: (i64, i64),
    left: PairToken,
    right: PairToken,
    answer: CachedAnswer,
    state: u8,
}

const EMPTY_SLOT: PairSlot = PairSlot {
    key: (0, 0),
    left: PairToken {
        signature: [0; 32],
        sha: [0; 32],
    },
    right: PairToken {
        signature: [0; 32],
        sha: [0; 32],
    },
    answer: CachedAnswer {
        kind: 0,
        reason: 0,
        strict: false,
    },
    state: SLOT_EMPTY,
};

/// Fixed-capacity open-addressed table. The backing allocation is the hard
/// limit: entries never grow a heap map beyond the configured bytes, and
/// eviction keeps the same accounted allocation instead of pretending the
/// memory was released.
struct PairCache {
    slots: Vec<PairSlot>,
    len: usize,
    used: usize,
    max_bytes: usize,
}

const PAIR_CACHE_DEFAULT_BYTES: usize = 8 * 1024 * 1024;
const PAIR_CACHE_MAX_BYTES: usize = 96 * 1024 * 1024;
const PAIR_CACHE_LOAD_PERCENT: usize = 76;
const WORK_MEMORY_BUDGET_BYTES: usize = super::index::MEMORY_BUDGET_BYTES as usize;
const WORK_MEMORY_HEADROOM_BYTES: usize = 24 * 1024 * 1024;

pub(super) fn pair_cache_budget(index_bytes: u64) -> usize {
    let index_bytes = usize::try_from(index_bytes).unwrap_or(usize::MAX);
    let remaining = WORK_MEMORY_BUDGET_BYTES
        .saturating_sub(index_bytes)
        .saturating_sub(WORK_MEMORY_HEADROOM_BYTES);
    (remaining / 3).min(PAIR_CACHE_MAX_BYTES)
}

impl PairCache {
    fn new(max_bytes: usize) -> Self {
        Self {
            slots: Vec::new(),
            len: 0,
            used: 0,
            max_bytes,
        }
    }

    fn set_max_bytes(&mut self, max_bytes: usize) {
        self.max_bytes = max_bytes;
        if self.allocated_bytes() > max_bytes {
            self.clear_and_release();
        }
        if self.max_slots() == 0 {
            self.clear_and_release();
        }
    }

    fn max_slots(&self) -> usize {
        self.max_bytes / std::mem::size_of::<PairSlot>()
    }

    fn allocated_bytes(&self) -> usize {
        self.slots.capacity().saturating_mul(std::mem::size_of::<PairSlot>())
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.len
    }

    fn clear_and_release(&mut self) {
        self.slots = Vec::new();
        self.len = 0;
        self.used = 0;
    }

    fn clear_entries(&mut self) {
        self.slots.fill(EMPTY_SLOT);
        self.len = 0;
        self.used = 0;
    }

    fn hash(key: (i64, i64)) -> usize {
        let mut value = (key.0 as u64) ^ (key.1 as u64).rotate_left(32);
        value ^= value >> 30;
        value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value ^= value >> 27;
        value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^= value >> 31;
        value as usize
    }

    fn probe(&self, key: (i64, i64)) -> (usize, bool) {
        if self.slots.is_empty() {
            return (0, false);
        }
        let start = Self::hash(key) % self.slots.len();
        for step in 0..self.slots.len() {
            let index = (start + step) % self.slots.len();
            let slot = &self.slots[index];
            if slot.state == SLOT_EMPTY {
                return (index, false);
            }
            if slot.state == SLOT_OCCUPIED && slot.key == key {
                return (index, true);
            }
        }
        (start, false)
    }

    fn get(&mut self, key: (i64, i64), left: PairToken, right: PairToken) -> Option<CachedAnswer> {
        let (index, found) = self.probe(key);
        if !found {
            return None;
        }
        let slot = self.slots[index];
        if slot.left == left && slot.right == right {
            return Some(slot.answer);
        }
        self.slots[index].state = SLOT_TOMBSTONE;
        self.len = self.len.saturating_sub(1);
        None
    }

    fn insert(&mut self, key: (i64, i64), left: PairToken, right: PairToken, answer: CachedAnswer) {
        if self.slots.is_empty() {
            let slots = self.max_slots();
            if slots == 0 {
                return;
            }
            let mut allocated = Vec::new();
            if allocated.try_reserve_exact(slots).is_err() {
                return;
            }
            allocated.resize(slots, EMPTY_SLOT);
            self.slots = allocated;
        }
        if (self.used + 1) * 100 > self.slots.len() * PAIR_CACHE_LOAD_PERCENT {
            self.clear_entries();
        }
        let (index, found) = self.probe(key);
        if found {
            self.slots[index] = PairSlot {
                key,
                left,
                right,
                answer,
                state: SLOT_OCCUPIED,
            };
            return;
        }
        if self.slots[index].state == SLOT_EMPTY {
            self.used += 1;
        }
        self.slots[index] = PairSlot {
            key,
            left,
            right,
            answer,
            state: SLOT_OCCUPIED,
        };
        self.len += 1;
    }
}

#[derive(Clone)]
struct PendingVerdict {
    a: i64,
    b: i64,
    left_signature: Vec<u8>,
    right_signature: Vec<u8>,
    left_sha: Vec<u8>,
    right_sha: Vec<u8>,
    answer: super::cascade_pixels::Analysis,
}

struct PendingCheck {
    a: i64,
    b: i64,
    kind: i64,
    left_signature: Vec<u8>,
    right_signature: Vec<u8>,
    left_sha: Vec<u8>,
    right_sha: Vec<u8>,
    outcome: i64,
}

const CHECK_BUFFER_MAX: usize = 256;

struct PendingEvidence {
    a: i64,
    b: i64,
    left: Source,
    right: Source,
    answer: super::evidence::Evidence,
}

struct Source {
    path: String,
    signature: Vec<u8>,
    sha: Vec<u8>,
}

fn version(exact: bool) -> &'static str {
    if exact {
        "illuTag-byte-equality-v1"
    } else {
        verification::ALGORITHM_VERSION
    }
}

impl Verifier {
    pub fn new(state: &AppState, force: bool) -> Result<Self, String> {
        let conn = cache::open_existing(state, true)?;
        super::cascade_store::initialize(&conn, force)?;
        conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
        if force {
            conn.execute("DELETE FROM pair_evidence", [])
                .map_err(|e| e.to_string())?;
            conn.execute("DELETE FROM pair_checks", [])
                .map_err(|e| e.to_string())?;
        }
        super::evidence_store::initialize(&conn, force)?;
        Ok(Self {
            conn,
            db: cache::library(state)?,
            checks: 0,
            accepted: 0,
            variants: 0,
            uncertain: 0,
            pending: Vec::new(),
            evidence_ready: false,
            pair_cache: PairCache::new(PAIR_CACHE_DEFAULT_BYTES),
            pair_cache_hits: 0,
            batch_writes: false,
            pending_verdicts: Vec::new(),
            pending_checks: Vec::new(),
            pair_check_transactions: 0,
            pair_check_rows: 0,
            verdict_transactions: 0,
        })
    }

    fn source(&self, pk: i64) -> Result<Source, String> {
        let image = management::main_image(&self.db, pk)?.ok_or("图库记录已移除")?;
        if image.trashed || image.missing || image.source == "reference" {
            return Err("图片不再参与检索".into());
        }
        let (public, signature, sha): (Vec<u8>, Vec<u8>, Option<Vec<u8>>) = self
            .conn
            .query_row(
                "SELECT public_hash,signature,sha FROM fingerprints WHERE image_pk=?1",
                [pk],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(|e| e.to_string())?;
        let actual = fingerprint::source_signature(Path::new(&image.path))?;
        if public != Sha256::digest(image.id.as_bytes()).to_vec()
            || signature != cache::signature(&image.id, &actual)
        {
            return Err("源文件身份或内容状态已变化".into());
        }
        Ok(Source {
            path: image.path,
            signature,
            sha: sha.ok_or("文件哈希缺失")?,
        })
    }

    fn pending_check_outcome(
        &self,
        a: i64,
        b: i64,
        exact: bool,
        left: &Source,
        right: &Source,
    ) -> Option<VerificationOutcome> {
        let pending = self.pending_checks.iter().find(|check| {
            check.a == a
                && check.b == b
                && check.kind == exact as i64
                && check.left_signature == left.signature
                && check.right_signature == right.signature
                && check.left_sha == left.sha
                && check.right_sha == right.sha
        })?;
        Some(match (pending.outcome, exact) {
            (1, true) => VerificationOutcome::Identical,
            (1, false) => VerificationOutcome::Equivalent,
            _ => VerificationOutcome::Variant("cached_local_difference"),
        })
    }

    pub fn check(
        &mut self,
        a: i64,
        b: i64,
        exact: bool,
        state: &AppState,
        control: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<VerificationOutcome, String> {
        control()?;
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let sources = self
            .source(a)
            .and_then(|left| self.source(b).map(|right| (left, right)));
        let (left, right) = match sources {
            Ok(value) => value,
            Err(_) => {
                self.uncertain += 1;
                return Ok(VerificationOutcome::Uncertain("source_unavailable"));
            }
        };
        // Byte-identical candidates need no image decoding, including tiny/animated files.
        let exact = exact || left.sha == right.sha;
        let outcome = if let Some(outcome) =
            self.pending_check_outcome(a, b, exact, &left, &right)
        {
            outcome
        } else {
            let cached: Option<i64> = self.conn.query_row(
            "SELECT outcome FROM pair_checks WHERE a=?1 AND b=?2 AND kind=?3 AND a_signature=?4 AND b_signature=?5 AND a_sha=?6 AND b_sha=?7 AND algorithm=?8",
            params![a,b,exact as i64,left.signature,right.signature,left.sha,right.sha,version(exact)],
            |r| r.get(0),
        ).optional().map_err(|e| e.to_string())?;
            match cached {
                Some(1) if exact => VerificationOutcome::Identical,
                Some(1) => VerificationOutcome::Equivalent,
                Some(0) => VerificationOutcome::Variant("cached_local_difference"),
                _ => {
                    let outcome = verification::verify_pair(
                        Path::new(&left.path),
                        Path::new(&right.path),
                        exact,
                        control,
                    )?;
                    // Decoder errors/offline files are retryable, not permanent negative decisions.
                    if !matches!(outcome, VerificationOutcome::Uncertain(_)) {
                        if self.checks % 64 == 0 {
                            cache::check_disk(&cache::cache_path(state))?;
                        }
                        if self.batch_writes {
                            if self.pending_checks.len() >= CHECK_BUFFER_MAX {
                                self.flush_verdicts(control)?;
                            }
                            self.pending_checks.push(PendingCheck {
                                a,
                                b,
                                kind: exact as i64,
                                left_signature: left.signature.clone(),
                                right_signature: right.signature.clone(),
                                left_sha: left.sha.clone(),
                                right_sha: right.sha.clone(),
                                outcome: outcome.is_equivalent() as i64,
                            });
                        } else {
                            cache::controlled_write(&self.conn, control, || {
                                self.conn.execute(
                                    "INSERT OR REPLACE INTO pair_checks VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                                    params![
                                        a,
                                        b,
                                        exact as i64,
                                        left.signature,
                                        right.signature,
                                        left.sha,
                                        right.sha,
                                        version(exact),
                                        outcome.is_equivalent() as i64
                                    ],
                                )
                            })
                            .map_err(|e| format!("保存细节复核缓存失败：{e}"))?;
                        }
                    }
                    outcome
                }
            }
        };
        self.checks += 1;
        match &outcome {
            VerificationOutcome::Identical | VerificationOutcome::Equivalent => self.accepted += 1,
            VerificationOutcome::Variant(_) => self.variants += 1,
            VerificationOutcome::Uncertain(_) => self.uncertain += 1,
        }
        Ok(outcome)
    }

    pub(super) fn native_digest(
        &mut self,
        pk: i64,
        state: &AppState,
        control: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<Option<Vec<u8>>, String> {
        control()?;
        let source = match self.source(pk) {
            Ok(s) => s,
            Err(_) => return Ok(None),
        };
        let mut hash = Sha256::new();
        hash.update(super::cascade_pixels::POLICY);
        hash.update(&source.signature);
        hash.update(&source.sha);
        let version = hash.finalize().to_vec();
        if let Some(digest) = self
            .conn
            .query_row(
                "SELECT digest FROM cascade_native WHERE image_pk=?1 AND version=?2",
                params![pk, version],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
        {
            if digest.len() == 32 { return Ok(Some(digest)); }
        }
        let digest = super::cascade_pixels::native_digest(Path::new(&source.path), control)?;
        if let Some(value) = &digest {
            cache::check_disk(&cache::cache_path(state))?;
            cache::controlled_write(&self.conn, control, || {
                self.conn.execute(
                    "INSERT OR REPLACE INTO cascade_native VALUES(?1,?2,?3)",
                    params![pk, version, value],
                )
            })?;
        }
        Ok(digest)
    }
    fn source_token(source: &Source) -> Option<PairToken> {
        let signature: [u8; 32] = source.signature.as_slice().try_into().ok()?;
        let sha: [u8; 32] = source.sha.as_slice().try_into().ok()?;
        Some(PairToken { signature, sha })
    }

    fn pair_key(a: i64, b: i64) -> (i64, i64) {
        if a <= b {
            (a, b)
        } else {
            (b, a)
        }
    }

    fn cached_pair(
        &mut self,
        a: i64,
        b: i64,
        left: &Source,
        right: &Source,
    ) -> Option<super::cascade_pixels::Analysis> {
        let key = Self::pair_key(a, b);
        let tokens = (Self::source_token(left)?, Self::source_token(right)?);
        self.pair_cache
            .get(key, tokens.0, tokens.1)
            .map(CachedAnswer::into_analysis)
    }

    fn remember_pair(
        &mut self,
        a: i64,
        b: i64,
        left: &Source,
        right: &Source,
        answer: &super::cascade_pixels::Analysis,
    ) {
        let (Some(left_token), Some(right_token)) =
            (Self::source_token(left), Self::source_token(right))
        else {
            return;
        };
        let Some(answer) = CachedAnswer::from_analysis(answer) else {
            return;
        };
        self.pair_cache
            .insert(Self::pair_key(a, b), left_token, right_token, answer);
    }

    /// Fit the pair cache inside the remaining 512 MiB duplicate-work budget.
    /// The low-level table charges its actual Vec capacity, not an entry guess.
    pub(super) fn configure_work_memory(&mut self, index_bytes: u64) {
        self.pair_cache
            .set_max_bytes(pair_cache_budget(index_bytes));
    }

    #[cfg(test)]
    pub(super) fn pair_cache_allocated_bytes(&self) -> usize {
        self.pair_cache.allocated_bytes()
    }

    #[cfg(test)]
    pub(super) fn pair_cache_len(&self) -> usize {
        self.pair_cache.len()
    }

    pub(super) fn flush_verdicts(
        &mut self,
        control: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<(), String> {
        if self.pending_verdicts.is_empty() && self.pending_checks.is_empty() {
            return Ok(());
        }
        let check_rows = self.pending_checks.len() as u64;
        let verdict_rows = self.pending_verdicts.len();
        cache::controlled_write(&self.conn, control, || {
            self.conn.execute_batch("BEGIN IMMEDIATE")
        })?;
        let mut failure = None;
        for check in &self.pending_checks {
            if let Err(error) = self.conn.execute(
                "INSERT OR REPLACE INTO pair_checks VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    check.a,
                    check.b,
                    check.kind,
                    check.left_signature,
                    check.right_signature,
                    check.left_sha,
                    check.right_sha,
                    version(check.kind != 0),
                    check.outcome
                ],
            ) {
                failure = Some(format!("保存细节复核缓存失败：{error}"));
                break;
            }
        }
        if failure.is_none() {
            for verdict in &self.pending_verdicts {
                if let Err(error) = super::cascade_store::insert_in_transaction(
                    &self.conn,
                    verdict.a,
                    verdict.b,
                    &verdict.left_signature,
                    &verdict.right_signature,
                    &verdict.left_sha,
                    &verdict.right_sha,
                    &verdict.answer,
                ) {
                    failure = Some(error);
                    break;
                }
            }
        }
        if let Some(error) = failure {
            let _ = self.conn.execute_batch("ROLLBACK");
            return Err(error);
        }
        self.conn
            .execute_batch("COMMIT")
            .map_err(|error| error.to_string())?;
        self.pending_checks.clear();
        self.pending_verdicts.clear();
        if check_rows > 0 {
            self.pair_check_transactions += 1;
            self.pair_check_rows += check_rows;
            #[cfg(test)]
            BENCH_PAIR_CHECK_TX.with(|value| value.set(value.get() + 1));
        }
        if verdict_rows > 0 {
            self.verdict_transactions += 1;
            #[cfg(test)]
            BENCH_VERDICT_TX.with(|value| value.set(value.get() + 1));
        }
        Ok(())
    }

    pub(super) fn pending_write_count(&self) -> usize {
        self.pending_checks.len() + self.pending_verdicts.len()
    }

    #[cfg(test)]
    pub(super) fn pair_check_transactions(&self) -> u64 {
        self.pair_check_transactions
    }

    #[cfg(test)]
    pub(super) fn pair_check_rows(&self) -> u64 {
        self.pair_check_rows
    }

    #[cfg(test)]
    pub(super) fn verdict_transactions(&self) -> u64 {
        self.verdict_transactions
    }

    fn queue_verdict(
        &mut self,
        a: i64,
        b: i64,
        left: &Source,
        right: &Source,
        answer: &super::cascade_pixels::Analysis,
    ) -> Result<(), String> {
        self.pending_verdicts.push(PendingVerdict {
            a,
            b,
            left_signature: left.signature.clone(),
            right_signature: right.signature.clone(),
            left_sha: left.sha.clone(),
            right_sha: right.sha.clone(),
            answer: answer.clone(),
        });
        Ok(())
    }

    pub(super) fn check_cascade(
        &mut self,
        a: i64,
        b: i64,
        state: &AppState,
        control: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<super::cascade_pixels::Analysis, String> {
        use super::cascade_pixels::Analysis;
        control()?;
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let (left, right) = match self.source(a).and_then(|x| self.source(b).map(|y| (x, y))) {
            Ok(x) => x,
            Err(_) => return Ok(Analysis::new("uncertain", "source_unavailable", false)),
        };
        if let Some(answer) = self.cached_pair(a, b, &left, &right) {
            // Source tokens already matched above; still honour cancellation and
            // never turn an uncertain/no-source result into a permanent answer.
            control()?;
            self.pair_cache_hits += 1;
            return Ok(answer);
        }
        if left.sha == right.sha {
            let answer = if self.check(a, b, true, state, control)? == VerificationOutcome::Identical
            {
                Analysis::new("exact", "byte_identical", true)
            } else {
                Analysis::new("uncertain", "byte_proof_failed", false)
            };
            if answer.kind != "uncertain" {
                self.remember_pair(a, b, &left, &right, &answer);
            }
            return Ok(answer);
        }
        if let Some(answer) = super::cascade_store::lookup(
            &self.conn,
            a,
            b,
            &left.signature,
            &right.signature,
            &left.sha,
            &right.sha,
        )? {
            self.remember_pair(a, b, &left, &right, &answer);
            return Ok(answer);
        }
        let answer =
            super::cascade_pixels::verify(Path::new(&left.path), Path::new(&right.path), control)?;
        if answer.kind != "uncertain" {
            if self.checks % 32 == 0 {
                cache::check_disk(&cache::cache_path(state))?;
            }
            if self.batch_writes {
                self.queue_verdict(a, b, &left, &right, &answer)?;
            } else {
                cache::controlled_write(&self.conn, control, || {
                    self.conn.execute_batch("BEGIN IMMEDIATE")
                })?;
                if let Err(e) = super::cascade_store::insert(
                    &self.conn,
                    a,
                    b,
                    &left.signature,
                    &right.signature,
                    &left.sha,
                    &right.sha,
                    &answer,
                ) {
                    let _ = self.conn.execute_batch("ROLLBACK");
                    return Err(e);
                }
                self.conn
                    .execute_batch("COMMIT")
                    .map_err(|e| e.to_string())?;
            }
            self.remember_pair(a, b, &left, &right, &answer);
            self.checks += 1;
        }
        Ok(answer)
    }

    /// v1 lossy pass/fail entries never become current replacement evidence.
    pub(super) fn check_evidence(
        &mut self,
        a: i64,
        b: i64,
        exact: bool,
        state: &AppState,
        control: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<super::evidence::Evidence, String> {
        use super::evidence::Evidence;
        if !self.evidence_ready {
            self.prepare_evidence(control, &mut |_, _| {})?;
        }
        control()?;
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let (left, right) = match self.source(a).and_then(|x| self.source(b).map(|y| (x, y))) {
            Ok(x) => x,
            Err(_) => return Ok(Evidence::uncertain("source_unavailable")),
        };
        let exact = exact || left.sha == right.sha;
        if exact {
            let answer = self.check(a, b, true, state, control)?;
            return Ok(if answer == VerificationOutcome::Identical {
                Evidence::result("bytes", "byte_identical", true)
            } else {
                Evidence::uncertain(answer.reason())
            });
        }
        if let Some(p) = self.pending.iter().find(|p| {
            p.a == a
                && p.b == b
                && p.left.signature == left.signature
                && p.right.signature == right.signature
                && p.left.sha == left.sha
                && p.right.sha == right.sha
        }) {
            return Ok(p.answer.clone());
        }
        if let Some(answer) = super::evidence_store::lookup(
            &self.conn,
            a,
            b,
            &left.signature,
            &right.signature,
            &left.sha,
            &right.sha,
        )? {
            return Ok(answer);
        }
        let answer =
            super::evidence::verify(Path::new(&left.path), Path::new(&right.path), control)?;
        if answer.level != "uncertain" {
            if self.checks % 32 == 0 {
                cache::check_disk(&cache::cache_path(state))?;
            }
            self.checks += 1;
            self.pending.push(PendingEvidence {
                a,
                b,
                left,
                right,
                answer: answer.clone(),
            });
            if self.pending.len() >= 64 {
                self.flush_evidence(control)?;
            }
        }
        Ok(answer)
    }

    pub(super) fn prepare_evidence(
        &mut self,
        control: &mut dyn FnMut() -> Result<(), String>,
        progress: &mut dyn FnMut(i64, i64),
    ) -> Result<(), String> {
        super::evidence_store::migrate(&self.conn, control, progress)?;
        self.evidence_ready = true;
        Ok(())
    }
    pub(super) fn flush_evidence(
        &mut self,
        control: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<(), String> {
        if self.pending.is_empty() {
            return Ok(());
        }
        cache::controlled_write(&self.conn, control, || {
            self.conn.execute_batch("BEGIN IMMEDIATE")
        })?;
        let result = (|| -> Result<(), String> {
            for p in &self.pending {
                super::evidence_store::insert(
                    &self.conn,
                    p.a,
                    p.b,
                    &p.left.signature,
                    &p.right.signature,
                    &p.left.sha,
                    &p.right.sha,
                    &p.answer,
                )?;
            }
            if let Some(p) = self.pending.last() {
                super::evidence_store::sample(&self.conn, p.a, p.b, &p.answer)?;
            }
            Ok(())
        })();
        if let Err(e) = result {
            let _ = self.conn.execute_batch("ROLLBACK");
            return Err(e);
        }
        self.conn
            .execute_batch("COMMIT")
            .map_err(|e| e.to_string())?;
        self.pending.clear();
        Ok(())
    }
}

/// UI regrouping never opens images. A changed reference may use only an existing proof.
pub(super) fn accepted_cached(
    conn: &Connection,
    a: i64,
    b: i64,
    exact: bool,
) -> Result<bool, String> {
    if a == b {
        return Ok(true);
    }
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='pair_checks')",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if !exists {
        return Ok(false);
    }
    conn.query_row("SELECT EXISTS(SELECT 1 FROM pair_checks p JOIN fingerprints a ON a.image_pk=p.a JOIN fingerprints b ON b.image_pk=p.b WHERE p.a=?1 AND p.b=?2 AND p.kind=?3 AND p.algorithm=?4 AND p.outcome=1 AND a.signature=p.a_signature AND b.signature=p.b_signature AND a.sha=p.a_sha AND b.sha=p.b_sha)",
        params![a,b,exact as i64,version(exact)], |r|r.get(0)).map_err(|e|e.to_string())
}

#[cfg(test)]
mod pair_cache_tests {
    use super::*;

    fn token(byte: u8) -> PairToken {
        PairToken {
            signature: [byte; 32],
            sha: [byte.wrapping_add(1); 32],
        }
    }

    fn answer() -> CachedAnswer {
        CachedAnswer {
            kind: CACHED_SIMILAR,
            reason: 7,
            strict: false,
        }
    }

    #[test]
    fn allocation_is_the_limit_and_eviction_keeps_it_counted() {
        let mut cache = PairCache::new(1024 * 1024);
        for n in 0..50_000_i64 {
            cache.insert((n, n + 1), token(1), token(2), answer());
        }
        assert!(
            cache.allocated_bytes() <= 1024 * 1024,
            "allocated {}",
            cache.allocated_bytes()
        );
        assert_eq!(
            cache.allocated_bytes(),
            cache.slots.capacity() * std::mem::size_of::<PairSlot>()
        );
        assert!(cache.len() <= cache.slots.len() * PAIR_CACHE_LOAD_PERCENT / 100);
        let allocated = cache.allocated_bytes();
        cache.clear_entries();
        assert_eq!(cache.len(), 0);
        assert_eq!(
            cache.allocated_bytes(),
            allocated,
            "retained capacity must stay in the byte accounting, not become zero"
        );
    }

    #[test]
    fn changed_tokens_are_not_served_from_the_pair_cache() {
        let mut cache = PairCache::new(1024 * 1024);
        cache.insert((1, 2), token(1), token(2), answer());
        assert!(cache.get((1, 2), token(1), token(2)).is_some());
        assert!(cache.get((1, 2), token(1), token(9)).is_none());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn dense_thousand_image_pair_set_fits_the_real_capacity() {
        let mut cache = PairCache::new(96 * 1024 * 1024);
        for a in 0..1000_i64 {
            for b in (a + 1)..1000_i64 {
                cache.insert((a, b), token(1), token(2), answer());
            }
        }
        assert!(
            cache.allocated_bytes() <= 96 * 1024 * 1024,
            "allocated {}",
            cache.allocated_bytes()
        );
        assert_eq!(cache.len(), 499_500);
    }

    #[test]
    fn pair_cache_budget_stays_inside_the_work_memory_budget() {
        assert_eq!(pair_cache_budget(0), 96 * 1024 * 1024);
        let index = super::super::index::estimate_memory_bytes(800_000);
        let cache = pair_cache_budget(index);
        assert!(cache > 0);
        assert!(
            cache + index as usize + WORK_MEMORY_HEADROOM_BYTES
                <= WORK_MEMORY_BUDGET_BYTES
        );
        assert_eq!(pair_cache_budget(u64::MAX), 0);
    }
}

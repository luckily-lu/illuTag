//! Pair decisions are disposable, bounded cache entries, never main-library writes.
use super::{
    cache, fingerprint, management,
    verification::{self, VerificationOutcome},
};
use crate::library::AppState;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::path::Path;

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
}

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
        let cached: Option<i64> = self.conn.query_row(
            "SELECT outcome FROM pair_checks WHERE a=?1 AND b=?2 AND kind=?3 AND a_signature=?4 AND b_signature=?5 AND a_sha=?6 AND b_sha=?7 AND algorithm=?8",
            params![a,b,exact as i64,left.signature,right.signature,left.sha,right.sha,version(exact)],
            |r| r.get(0),
        ).optional().map_err(|e| e.to_string())?;
        let outcome = match cached {
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
                outcome
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
        if left.sha == right.sha {
            if self.check(a, b, true, state, control)? == VerificationOutcome::Identical {
                return Ok(Analysis::new("exact", "byte_identical", true));
            }
            return Ok(Analysis::new("uncertain", "byte_proof_failed", false));
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
            return Ok(answer);
        }
        let answer =
            super::cascade_pixels::verify(Path::new(&left.path), Path::new(&right.path), control)?;
        if answer.kind != "uncertain" {
            if self.checks % 32 == 0 {
                cache::check_disk(&cache::cache_path(state))?;
            }
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

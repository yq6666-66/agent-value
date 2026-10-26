//! Official fingerprint baselines in the local database.
//!
//! The row key is a SHA-256 of the identity fields below, including a
//! SHA-256 of the full sampling config. It is not an API key. This module
//! does not send requests and does not score probes.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use rusqlite::{params, Connection, OptionalExtension};

use crate::digest::sha256_hex;
use crate::{StoreError, StoreResult};
use agent_value_core::privacy::has_secret_prefix;
use agent_value_core::CoreError;

/// Storage lifetime of one baseline. Usable through this age, missing one
/// millisecond later. Not a probe verdict threshold.
pub const FINGERPRINT_BASELINE_TTL_MS: i64 = 30 * 86_400_000;

/// Distribution payload this store reads and writes.
pub const DISTRIBUTION_SCHEMA_VERSION: u32 = 1;

const QUESTIONS: usize = 10;
const MAX_ANSWERS: usize = 64;
const MAX_ID_CHARS: usize = 200;
const MAX_SAMPLING_CHARS: usize = 4_000;
const MAX_ANSWER_CHARS: usize = 200;
const MAX_DISTRIBUTION_BYTES: usize = 256 * 1024;

/// UTC epoch milliseconds.
pub trait Clock: Send {
    fn now_ms(&self) -> i64;
}

/// Wall clock as UTC epoch milliseconds.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_millis()).ok())
            .unwrap_or(i64::MAX)
    }
}

/// A clock whose time the caller sets. Tests share one value with the store.
#[derive(Debug, Clone)]
pub struct FixedClock {
    now_ms: Arc<AtomicI64>,
}

impl FixedClock {
    pub fn new(now_ms: i64) -> Self {
        Self {
            now_ms: Arc::new(AtomicI64::new(now_ms)),
        }
    }

    pub fn set(&self, now_ms: i64) {
        self.now_ms.store(now_ms, Ordering::Relaxed);
    }
}

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.now_ms.load(Ordering::Relaxed)
    }
}

/// Fields that identify one official baseline. `model_revision` may be empty.
/// `sampling_config` is the full canonical sampling config; the store hashes
/// it and does not reduce it to a summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FingerprintBaselineIdentity {
    pub provider: String,
    pub model_id: String,
    pub model_revision: String,
    pub protocol: String,
    pub connection_kind: String,
    pub task_set_version: String,
    pub system_prompt_hash: String,
    pub sampling_config: String,
    pub distribution_schema_version: u32,
}

/// One observed answer and how many times it occurred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnswerCount {
    pub answer: String,
    pub count: u32,
}

/// Answers for one fingerprint question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionAnswers {
    pub answers: Vec<AnswerCount>,
}

/// Schema version 1: two runs of ten questions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FingerprintDistribution {
    pub run1: Vec<QuestionAnswers>,
    pub run2: Vec<QuestionAnswers>,
}

/// A baseline to store. Only these fields are accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FingerprintBaselineWrite {
    pub identity: FingerprintBaselineIdentity,
    pub collected_at_ms: i64,
    pub distribution: FingerprintDistribution,
}

/// A stored baseline, including the distribution payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FingerprintBaselineRecord {
    pub baseline_key: String,
    pub identity: FingerprintBaselineIdentity,
    pub sampling_hash: String,
    pub collected_at_ms: i64,
    pub expires_at_ms: i64,
    pub distribution: FingerprintDistribution,
}

/// A list row. The distribution payload and the sampling config are omitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FingerprintBaselineSummary {
    pub baseline_key: String,
    pub provider: String,
    pub model_id: String,
    pub model_revision: String,
    pub protocol: String,
    pub connection_kind: String,
    pub task_set_version: String,
    pub system_prompt_hash: String,
    pub sampling_hash: String,
    pub distribution_schema_version: u32,
    pub collected_at_ms: i64,
    pub expires_at_ms: i64,
}

/// Result of an upsert. An older `collected_at_ms` does not replace the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum FingerprintBaselineUpsert {
    Inserted,
    Updated,
    StaleIgnored,
}

struct Prepared {
    identity: FingerprintBaselineIdentity,
    baseline_key: String,
    sampling_hash: String,
    collected_at_ms: i64,
    expires_at_ms: i64,
    distribution_json: String,
}

pub(crate) fn upsert(
    conn: &Connection,
    write: &FingerprintBaselineWrite,
) -> StoreResult<FingerprintBaselineUpsert> {
    let prepared = prepare(write)?;
    let tx = conn.unchecked_transaction()?;
    let existing: Option<i64> = tx
        .query_row(
            "SELECT collected_at_ms FROM fingerprint_baselines WHERE baseline_key = ?1",
            [&prepared.baseline_key],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(old) = existing {
        if prepared.collected_at_ms < old {
            return Ok(FingerprintBaselineUpsert::StaleIgnored);
        }
        tx.execute(
            "UPDATE fingerprint_baselines
             SET provider = ?2, model_id = ?3, model_revision = ?4, protocol = ?5,
                 connection_kind = ?6, task_set_version = ?7, system_prompt_hash = ?8,
                 sampling_hash = ?9, sampling_config = ?10, distribution_schema_version = ?11,
                 collected_at_ms = ?12, expires_at_ms = ?13, distribution_json = ?14
             WHERE baseline_key = ?1",
            params![
                prepared.baseline_key,
                prepared.identity.provider,
                prepared.identity.model_id,
                prepared.identity.model_revision,
                prepared.identity.protocol,
                prepared.identity.connection_kind,
                prepared.identity.task_set_version,
                prepared.identity.system_prompt_hash,
                prepared.sampling_hash,
                prepared.identity.sampling_config,
                prepared.identity.distribution_schema_version,
                prepared.collected_at_ms,
                prepared.expires_at_ms,
                prepared.distribution_json,
            ],
        )?;
        tx.commit()?;
        return Ok(FingerprintBaselineUpsert::Updated);
    }
    tx.execute(
        "INSERT INTO fingerprint_baselines (
            baseline_key, provider, model_id, model_revision, protocol, connection_kind,
            task_set_version, system_prompt_hash, sampling_hash, sampling_config,
            distribution_schema_version, collected_at_ms, expires_at_ms, distribution_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            prepared.baseline_key,
            prepared.identity.provider,
            prepared.identity.model_id,
            prepared.identity.model_revision,
            prepared.identity.protocol,
            prepared.identity.connection_kind,
            prepared.identity.task_set_version,
            prepared.identity.system_prompt_hash,
            prepared.sampling_hash,
            prepared.identity.sampling_config,
            prepared.identity.distribution_schema_version,
            prepared.collected_at_ms,
            prepared.expires_at_ms,
            prepared.distribution_json,
        ],
    )?;
    tx.commit()?;
    Ok(FingerprintBaselineUpsert::Inserted)
}

pub(crate) fn get(
    conn: &Connection,
    now_ms: i64,
    identity: &FingerprintBaselineIdentity,
) -> StoreResult<Option<FingerprintBaselineRecord>> {
    let key = content_key(identity)?;
    let Some(record) = load_record(conn, &key)? else {
        return Ok(None);
    };
    if now_ms > record.expires_at_ms {
        return Ok(None);
    }
    Ok(Some(record))
}

pub(crate) fn list(
    conn: &Connection,
    now_ms: i64,
    include_expired: bool,
) -> StoreResult<Vec<FingerprintBaselineSummary>> {
    let sql = if include_expired {
        "SELECT baseline_key, provider, model_id, model_revision, protocol, connection_kind,
                task_set_version, system_prompt_hash, sampling_hash, sampling_config,
                distribution_schema_version, collected_at_ms, expires_at_ms
         FROM fingerprint_baselines ORDER BY baseline_key"
    } else {
        "SELECT baseline_key, provider, model_id, model_revision, protocol, connection_kind,
                task_set_version, system_prompt_hash, sampling_hash, sampling_config,
                distribution_schema_version, collected_at_ms, expires_at_ms
         FROM fingerprint_baselines
         WHERE expires_at_ms >= ?1
         ORDER BY baseline_key"
    };
    let mut st = conn.prepare(sql)?;
    let map = |r: &rusqlite::Row<'_>| {
        Ok(ListedBaseline {
            baseline_key: r.get(0)?,
            provider: r.get(1)?,
            model_id: r.get(2)?,
            model_revision: r.get(3)?,
            protocol: r.get(4)?,
            connection_kind: r.get(5)?,
            task_set_version: r.get(6)?,
            system_prompt_hash: r.get(7)?,
            sampling_hash: r.get(8)?,
            sampling_config: r.get(9)?,
            distribution_schema_version: r.get(10)?,
            collected_at_ms: r.get(11)?,
            expires_at_ms: r.get(12)?,
        })
    };
    let rows = if include_expired {
        st.query_map([], map)?.collect::<Result<Vec<_>, _>>()?
    } else {
        st.query_map(params![now_ms], map)?
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let version = schema_version(row.distribution_schema_version)?;
        let identity = FingerprintBaselineIdentity {
            provider: row.provider,
            model_id: row.model_id,
            model_revision: row.model_revision,
            protocol: row.protocol,
            connection_kind: row.connection_kind,
            task_set_version: row.task_set_version,
            system_prompt_hash: row.system_prompt_hash,
            sampling_config: row.sampling_config,
            distribution_schema_version: version,
        };
        let expect = content_key(&identity)?;
        if expect != row.baseline_key
            || row.sampling_hash != sha256_hex(identity.sampling_config.as_bytes())
        {
            return Err(StoreError::Corrupt(
                "fingerprint baseline key does not match its fields".into(),
            ));
        }
        out.push(FingerprintBaselineSummary {
            baseline_key: row.baseline_key,
            provider: identity.provider,
            model_id: identity.model_id,
            model_revision: identity.model_revision,
            protocol: identity.protocol,
            connection_kind: identity.connection_kind,
            task_set_version: identity.task_set_version,
            system_prompt_hash: identity.system_prompt_hash,
            sampling_hash: row.sampling_hash,
            distribution_schema_version: version,
            collected_at_ms: row.collected_at_ms,
            expires_at_ms: row.expires_at_ms,
        });
    }
    Ok(out)
}

pub(crate) fn prune(conn: &Connection, now_ms: i64) -> StoreResult<usize> {
    Ok(conn.execute(
        "DELETE FROM fingerprint_baselines WHERE expires_at_ms < ?1",
        [now_ms],
    )?)
}

fn prepare(write: &FingerprintBaselineWrite) -> StoreResult<Prepared> {
    let identity = normalize_identity(&write.identity)?;
    reject_secret(&identity.provider)?;
    reject_secret(&identity.model_id)?;
    reject_secret(&identity.model_revision)?;
    reject_secret(&identity.protocol)?;
    reject_secret(&identity.connection_kind)?;
    reject_secret(&identity.task_set_version)?;
    reject_secret(&identity.system_prompt_hash)?;
    reject_secret(&identity.sampling_config)?;
    let distribution = normalize_distribution(&write.distribution)?;
    if write.collected_at_ms < 0 {
        return Err(CoreError::InvalidField {
            field: "collected_at_ms",
            reason: "must be a UTC epoch millisecond count >= 0",
        }
        .into());
    }
    let expires_at_ms = write
        .collected_at_ms
        .checked_add(FINGERPRINT_BASELINE_TTL_MS)
        .ok_or(CoreError::Overflow {
            what: "fingerprint baseline expiry",
        })?;
    let sampling_hash = sha256_hex(identity.sampling_config.as_bytes());
    let baseline_key = hash_identity(&identity, &sampling_hash);
    Ok(Prepared {
        identity,
        baseline_key,
        sampling_hash,
        collected_at_ms: write.collected_at_ms,
        expires_at_ms,
        distribution_json: encode_distribution(&distribution),
    })
}

fn content_key(identity: &FingerprintBaselineIdentity) -> StoreResult<String> {
    let identity = normalize_identity(identity)?;
    reject_secret(&identity.provider)?;
    reject_secret(&identity.model_id)?;
    reject_secret(&identity.model_revision)?;
    reject_secret(&identity.protocol)?;
    reject_secret(&identity.connection_kind)?;
    reject_secret(&identity.task_set_version)?;
    reject_secret(&identity.system_prompt_hash)?;
    reject_secret(&identity.sampling_config)?;
    let sampling_hash = sha256_hex(identity.sampling_config.as_bytes());
    Ok(hash_identity(&identity, &sampling_hash))
}

fn hash_identity(identity: &FingerprintBaselineIdentity, sampling_hash: &str) -> String {
    let version = identity.distribution_schema_version.to_string();
    let mut buf = Vec::new();
    for part in [
        identity.provider.as_str(),
        identity.model_id.as_str(),
        identity.model_revision.as_str(),
        identity.protocol.as_str(),
        identity.connection_kind.as_str(),
        identity.task_set_version.as_str(),
        identity.system_prompt_hash.as_str(),
        sampling_hash,
        version.as_str(),
    ] {
        let bytes = part.as_bytes();
        buf.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
        buf.extend_from_slice(bytes);
    }
    sha256_hex(&buf)
}

fn normalize_identity(
    identity: &FingerprintBaselineIdentity,
) -> StoreResult<FingerprintBaselineIdentity> {
    let provider = require_id("provider", &identity.provider)?;
    let model_id = require_id("model_id", &identity.model_id)?;
    let model_revision = optional_id("model_revision", &identity.model_revision)?;
    let protocol = require_id("protocol", &identity.protocol)?;
    let connection_kind = require_id("connection_kind", &identity.connection_kind)?;
    let task_set_version = require_id("task_set_version", &identity.task_set_version)?;
    let system_prompt_hash = identity.system_prompt_hash.trim();
    if !is_sha256_hex(system_prompt_hash) {
        return Err(CoreError::InvalidField {
            field: "system_prompt_hash",
            reason: "must be 64 lower-case hex digits",
        }
        .into());
    }
    if identity.sampling_config.trim().is_empty() {
        return Err(CoreError::EmptyField {
            field: "sampling_config",
        }
        .into());
    }
    if identity.sampling_config.chars().count() > MAX_SAMPLING_CHARS
        || has_disallowed_control(&identity.sampling_config, true)
    {
        return Err(CoreError::InvalidField {
            field: "sampling_config",
            reason: "must be the full canonical sampling config",
        }
        .into());
    }
    if identity.distribution_schema_version != DISTRIBUTION_SCHEMA_VERSION {
        return Err(CoreError::InvalidField {
            field: "distribution_schema_version",
            reason: "is not supported",
        }
        .into());
    }
    Ok(FingerprintBaselineIdentity {
        provider,
        model_id,
        model_revision,
        protocol,
        connection_kind,
        task_set_version,
        system_prompt_hash: system_prompt_hash.to_string(),
        sampling_config: identity.sampling_config.clone(),
        distribution_schema_version: identity.distribution_schema_version,
    })
}

fn normalize_distribution(dist: &FingerprintDistribution) -> StoreResult<FingerprintDistribution> {
    Ok(FingerprintDistribution {
        run1: normalize_run(&dist.run1)?,
        run2: normalize_run(&dist.run2)?,
    })
}

fn normalize_run(run: &[QuestionAnswers]) -> StoreResult<Vec<QuestionAnswers>> {
    if run.len() != QUESTIONS {
        return Err(CoreError::LengthMismatch {
            left: run.len(),
            right: QUESTIONS,
        }
        .into());
    }
    run.iter()
        .map(|q| {
            if q.answers.is_empty() || q.answers.len() > MAX_ANSWERS {
                return Err(CoreError::InvalidField {
                    field: "fingerprint answers",
                    reason: "each question needs between 1 and 64 answers",
                }
                .into());
            }
            let mut seen = Vec::new();
            let mut answers = Vec::with_capacity(q.answers.len());
            for a in &q.answers {
                let answer = a.answer.trim();
                if answer.is_empty() || answer.chars().count() > MAX_ANSWER_CHARS {
                    return Err(CoreError::InvalidField {
                        field: "fingerprint answer",
                        reason: "must be a non-empty token",
                    }
                    .into());
                }
                if has_disallowed_control(answer, false) {
                    return Err(CoreError::InvalidField {
                        field: "fingerprint answer",
                        reason: "must be a non-empty token",
                    }
                    .into());
                }
                reject_secret(answer)?;
                if a.count == 0 {
                    return Err(CoreError::InvalidField {
                        field: "fingerprint count",
                        reason: "must be at least 1",
                    }
                    .into());
                }
                if seen.iter().any(|s: &String| s == answer) {
                    return Err(CoreError::InvalidField {
                        field: "fingerprint answer",
                        reason: "is repeated in one question",
                    }
                    .into());
                }
                seen.push(answer.to_string());
                answers.push(AnswerCount {
                    answer: answer.to_string(),
                    count: a.count,
                });
            }
            Ok(QuestionAnswers { answers })
        })
        .collect()
}

fn require_id(field: &'static str, value: &str) -> StoreResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(CoreError::EmptyField { field }.into());
    }
    check_id(field, value)?;
    Ok(value.to_string())
}

fn optional_id(field: &'static str, value: &str) -> StoreResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(String::new());
    }
    check_id(field, value)?;
    Ok(value.to_string())
}

fn check_id(field: &'static str, value: &str) -> StoreResult<()> {
    if value.chars().count() > MAX_ID_CHARS || has_disallowed_control(value, false) {
        return Err(CoreError::InvalidField {
            field,
            reason: "has an empty or disallowed form",
        }
        .into());
    }
    Ok(())
}

fn has_disallowed_control(value: &str, allow_multiline: bool) -> bool {
    value.chars().any(|c| {
        let u = c as u32;
        if allow_multiline && (c == '\n' || c == '\r' || c == '\t') {
            return false;
        }
        u < 0x20 || u == 0x7f
    })
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn reject_secret(value: &str) -> StoreResult<()> {
    if has_secret_prefix(value) {
        Err(CoreError::LooksLikeSecret {
            field: "fingerprint baseline",
        }
        .into())
    } else {
        Ok(())
    }
}

fn schema_version(value: i64) -> StoreResult<u32> {
    u32::try_from(value)
        .ok()
        .filter(|v| *v == DISTRIBUTION_SCHEMA_VERSION)
        .ok_or_else(|| {
            StoreError::Corrupt("fingerprint distribution schema version is not supported".into())
        })
}

struct ListedBaseline {
    baseline_key: String,
    provider: String,
    model_id: String,
    model_revision: String,
    protocol: String,
    connection_kind: String,
    task_set_version: String,
    system_prompt_hash: String,
    sampling_hash: String,
    sampling_config: String,
    distribution_schema_version: i64,
    collected_at_ms: i64,
    expires_at_ms: i64,
}

struct LoadedBaseline {
    provider: String,
    model_id: String,
    model_revision: String,
    protocol: String,
    connection_kind: String,
    task_set_version: String,
    system_prompt_hash: String,
    sampling_hash: String,
    sampling_config: String,
    distribution_schema_version: i64,
    collected_at_ms: i64,
    expires_at_ms: i64,
    distribution_json: String,
}

fn load_record(
    conn: &Connection,
    baseline_key: &str,
) -> StoreResult<Option<FingerprintBaselineRecord>> {
    let row = conn
        .query_row(
            "SELECT provider, model_id, model_revision, protocol, connection_kind,
                    task_set_version, system_prompt_hash, sampling_hash, sampling_config,
                    distribution_schema_version, collected_at_ms, expires_at_ms, distribution_json
             FROM fingerprint_baselines WHERE baseline_key = ?1",
            [baseline_key],
            |r| {
                Ok(LoadedBaseline {
                    provider: r.get(0)?,
                    model_id: r.get(1)?,
                    model_revision: r.get(2)?,
                    protocol: r.get(3)?,
                    connection_kind: r.get(4)?,
                    task_set_version: r.get(5)?,
                    system_prompt_hash: r.get(6)?,
                    sampling_hash: r.get(7)?,
                    sampling_config: r.get(8)?,
                    distribution_schema_version: r.get(9)?,
                    collected_at_ms: r.get(10)?,
                    expires_at_ms: r.get(11)?,
                    distribution_json: r.get(12)?,
                })
            },
        )
        .optional()?;
    let Some(row) = row else {
        return Ok(None);
    };
    let version = schema_version(row.distribution_schema_version)?;
    let identity = FingerprintBaselineIdentity {
        provider: row.provider,
        model_id: row.model_id,
        model_revision: row.model_revision,
        protocol: row.protocol,
        connection_kind: row.connection_kind,
        task_set_version: row.task_set_version,
        system_prompt_hash: row.system_prompt_hash,
        sampling_config: row.sampling_config,
        distribution_schema_version: version,
    };
    let expect = hash_identity(&identity, &sha256_hex(identity.sampling_config.as_bytes()));
    if expect != baseline_key
        || row.sampling_hash != sha256_hex(identity.sampling_config.as_bytes())
    {
        return Err(StoreError::Corrupt(
            "fingerprint baseline key does not match its fields".into(),
        ));
    }
    if row.distribution_json.len() > MAX_DISTRIBUTION_BYTES {
        return Err(StoreError::Corrupt(
            "fingerprint distribution could not be read".into(),
        ));
    }
    let distribution = decode_distribution(&row.distribution_json)?;
    Ok(Some(FingerprintBaselineRecord {
        baseline_key: baseline_key.to_string(),
        identity,
        sampling_hash: row.sampling_hash,
        collected_at_ms: row.collected_at_ms,
        expires_at_ms: row.expires_at_ms,
        distribution,
    }))
}

fn encode_distribution(dist: &FingerprintDistribution) -> String {
    let mut out = String::from("{\"runs\":[");
    for (i, run) in [&dist.run1, &dist.run2].into_iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"questions\":[");
        for (q, question) in run.iter().enumerate() {
            if q > 0 {
                out.push(',');
            }
            out.push_str("{\"answers\":[");
            for (a, ans) in question.answers.iter().enumerate() {
                if a > 0 {
                    out.push(',');
                }
                out.push_str("{\"answer\":");
                push_json_string(&mut out, &ans.answer);
                out.push_str(",\"count\":");
                out.push_str(&ans.count.to_string());
                out.push('}');
            }
            out.push_str("]}");
        }
        out.push_str("]}");
    }
    out.push_str("]}");
    out
}

fn push_json_string(out: &mut String, value: &str) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn decode_distribution(text: &str) -> StoreResult<FingerprintDistribution> {
    let mut p = Parser {
        s: text.as_bytes(),
        i: 0,
    };
    let dist = p
        .parse_distribution()
        .map_err(|_| StoreError::Corrupt("fingerprint distribution could not be read".into()))?;
    normalize_distribution(&dist)
        .map_err(|_| StoreError::Corrupt("fingerprint distribution could not be read".into()))
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> Parser<'a> {
    fn skip(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        self.skip();
        if self.i < self.s.len() && self.s[self.i] == c {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, c: u8) -> Result<(), ()> {
        if self.eat(c) {
            Ok(())
        } else {
            Err(())
        }
    }

    fn parse_distribution(&mut self) -> Result<FingerprintDistribution, ()> {
        self.expect(b'{')?;
        let key = self.parse_string()?;
        if key != "runs" {
            return Err(());
        }
        self.expect(b':')?;
        self.expect(b'[')?;
        let run1 = self.parse_run()?;
        self.expect(b',')?;
        let run2 = self.parse_run()?;
        self.skip();
        if self.peek() == Some(b',') {
            return Err(());
        }
        self.expect(b']')?;
        self.skip();
        if self.peek() == Some(b',') || self.peek() == Some(b'"') {
            return Err(());
        }
        self.expect(b'}')?;
        self.skip();
        if self.i != self.s.len() {
            return Err(());
        }
        Ok(FingerprintDistribution { run1, run2 })
    }

    fn parse_run(&mut self) -> Result<Vec<QuestionAnswers>, ()> {
        self.expect(b'{')?;
        let key = self.parse_string()?;
        if key != "questions" {
            return Err(());
        }
        self.expect(b':')?;
        self.expect(b'[')?;
        let mut questions = Vec::new();
        self.skip();
        if !self.eat(b']') {
            loop {
                questions.push(self.parse_question()?);
                self.skip();
                if self.eat(b',') {
                    continue;
                }
                self.expect(b']')?;
                break;
            }
        }
        self.skip();
        if self.peek() == Some(b',') || self.peek() == Some(b'"') {
            return Err(());
        }
        self.expect(b'}')?;
        Ok(questions)
    }

    fn parse_question(&mut self) -> Result<QuestionAnswers, ()> {
        self.expect(b'{')?;
        let key = self.parse_string()?;
        if key != "answers" {
            return Err(());
        }
        self.expect(b':')?;
        self.expect(b'[')?;
        let mut answers = Vec::new();
        self.skip();
        if !self.eat(b']') {
            loop {
                answers.push(self.parse_answer()?);
                self.skip();
                if self.eat(b',') {
                    continue;
                }
                self.expect(b']')?;
                break;
            }
        }
        self.skip();
        if self.peek() == Some(b',') || self.peek() == Some(b'"') {
            return Err(());
        }
        self.expect(b'}')?;
        Ok(QuestionAnswers { answers })
    }

    fn parse_answer(&mut self) -> Result<AnswerCount, ()> {
        self.expect(b'{')?;
        let mut answer = None;
        let mut count = None;
        loop {
            self.skip();
            if answer.is_none() && count.is_none() && self.eat(b'}') {
                return Err(());
            }
            let key = self.parse_string()?;
            self.expect(b':')?;
            match key.as_str() {
                "answer" if answer.is_none() => answer = Some(self.parse_string()?),
                "count" if count.is_none() => count = Some(self.parse_u32()?),
                _ => return Err(()),
            }
            self.skip();
            if self.eat(b',') {
                self.skip();
                if self.peek() == Some(b'}') {
                    return Err(());
                }
                continue;
            }
            self.expect(b'}')?;
            break;
        }
        match (answer, count) {
            (Some(answer), Some(count)) => Ok(AnswerCount { answer, count }),
            _ => Err(()),
        }
    }

    fn parse_u32(&mut self) -> Result<u32, ()> {
        self.skip();
        let start = self.i;
        if self.i >= self.s.len() || !self.s[self.i].is_ascii_digit() {
            return Err(());
        }
        if self.s[self.i] == b'0'
            && self.i + 1 < self.s.len()
            && self.s[self.i + 1].is_ascii_digit()
        {
            return Err(());
        }
        while self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
            self.i += 1;
        }
        let text = std::str::from_utf8(&self.s[start..self.i]).map_err(|_| ())?;
        text.parse::<u32>().map_err(|_| ())
    }

    fn parse_string(&mut self) -> Result<String, ()> {
        self.skip();
        self.expect(b'"')?;
        let mut out = String::new();
        while self.i < self.s.len() {
            let b = self.s[self.i];
            self.i += 1;
            match b {
                b'"' => return Ok(out),
                b'\\' => {
                    let esc = *self.s.get(self.i).ok_or(())?;
                    self.i += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            if self.i + 4 > self.s.len() {
                                return Err(());
                            }
                            let hex =
                                std::str::from_utf8(&self.s[self.i..self.i + 4]).map_err(|_| ())?;
                            let code = u32::from_str_radix(hex, 16).map_err(|_| ())?;
                            out.push(char::from_u32(code).ok_or(())?);
                            self.i += 4;
                        }
                        _ => return Err(()),
                    }
                }
                b if b < 0x20 => return Err(()),
                b => {
                    if b < 0x80 {
                        out.push(b as char);
                    } else {
                        self.i -= 1;
                        let rest = std::str::from_utf8(&self.s[self.i..]).map_err(|_| ())?;
                        let ch = rest.chars().next().ok_or(())?;
                        out.push(ch);
                        self.i += ch.len_utf8();
                    }
                }
            }
        }
        Err(())
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip();
        self.s.get(self.i).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;

    fn question(answer: &str) -> QuestionAnswers {
        QuestionAnswers {
            answers: vec![AnswerCount {
                answer: answer.into(),
                count: 20,
            }],
        }
    }

    fn distribution() -> FingerprintDistribution {
        FingerprintDistribution {
            run1: vec![question("red"); QUESTIONS],
            run2: vec![question("blue"); QUESTIONS],
        }
    }

    #[test]
    fn distribution_json_round_trips_and_rejects_unknown_fields() {
        let dist = distribution();
        let json = encode_distribution(&dist);
        assert_eq!(decode_distribution(&json).unwrap(), dist);
        let extra = json.replace("{\"runs\":", "{\"note\":\"sk-not-echoed\",\"runs\":");
        let err = decode_distribution(&extra).unwrap_err();
        let rendered = format!("{err} {err:?}");
        assert!(!rendered.contains("sk-"));
        assert!(!rendered.contains("not-echoed"));
    }

    #[test]
    fn ttl_in_the_migration_matches_the_constant() {
        assert_eq!(FINGERPRINT_BASELINE_TTL_MS, 2_592_000_000);
        assert!(schema::MIGRATIONS[3].contains(&FINGERPRINT_BASELINE_TTL_MS.to_string()));
        assert!(schema::MIGRATIONS[3].contains("CREATE TABLE IF NOT EXISTS fingerprint_baselines"));
        assert!(!schema::MIGRATIONS[3].contains("ALTER TABLE"));
    }

    #[test]
    fn migration_can_run_twice_and_does_not_rewrite_earlier_tables() {
        let conn = Connection::open_in_memory().unwrap();
        for sql in &schema::MIGRATIONS[..3] {
            conn.execute_batch(sql).unwrap();
        }
        let before = table_sql(&conn);
        conn.execute_batch(schema::MIGRATIONS[3]).unwrap();
        conn.execute_batch(schema::MIGRATIONS[3]).unwrap();
        let after = table_sql(&conn);
        for (name, sql) in &before {
            assert_eq!(
                after.get(name).map(String::as_str),
                Some(sql.as_str()),
                "{name}"
            );
        }
        assert!(after.contains_key("fingerprint_baselines"));
        assert!(after
            .keys()
            .any(|name| name == "fingerprint_baselines_expires_at"));
    }

    fn table_sql(conn: &Connection) -> std::collections::BTreeMap<String, String> {
        let mut st = conn
            .prepare("SELECT name, sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name")
            .unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    /// Regenerates `tests/fixtures/schema-v3.sqlite` from migrations 1–3.
    /// Not part of the suite; the committed file is the old-database fixture.
    #[test]
    #[ignore]
    fn write_schema_v3_fixture() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/schema-v3.sqlite");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        let conn = Connection::open(&path).unwrap();
        conn.pragma_update(None, "journal_mode", "DELETE").unwrap();
        for sql in &schema::MIGRATIONS[..3] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 3).unwrap();
        conn.execute(
            "INSERT INTO arms (model, provider_kind, host, protocol, client)
             VALUES ('fixture-model', 'official', 'api.example.test', 'anthropic-messages', 'fixture-cli')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO probe_consent (id, accepted, at_ms) VALUES (1, 1, 1000)",
            [],
        )
        .unwrap();
        conn.execute_batch("VACUUM").unwrap();
    }
}

//! agent-value-store: local SQLite storage for agent-value (v22 §8).
//!
//! Stores route metadata per request, tasks, behaviour events with their
//! available-arm snapshots, probe results, frozen cell weights, weight
//! versions with frozen windows, and other versioned configuration.
//!
//! Privacy (§8): everything stays in one local file. No prompt or code text
//! is stored, only a category, a length and a keyed digest; API key values
//! have no column at all (only the env var name); task-level data is purged
//! after 180 days; export and delete-all are provided.
//!
//! A [`Store`] owns one SQLite connection. It is `Send` but not `Sync`; a
//! multi-threaded caller (the desktop shell) wraps it in a `Mutex`.

#![forbid(unsafe_code)]

mod digest;
mod error;
mod schema;

use std::path::Path;

use rusqlite::types::ValueRef;
use rusqlite::{params, Connection, OptionalExtension, Transaction};

use agent_value_core::arm::{Arm, ModelId, Protocol, ProviderKind, Route};
use agent_value_core::cells::{CellId, CellWeights};
use agent_value_core::outcome::{Judgement, Rule};
use agent_value_core::privacy::{has_secret_prefix, TextFacts, RETENTION_DAYS};
use agent_value_core::records::{
    BehaviorEvent, MicroYuan, Outcome, ProbeClass, ProbeResult, ProbeVerdict, RequestRecord,
    TaskRecord,
};
use agent_value_core::scope::ScopeId;
use agent_value_core::weights::{
    Evidence, FiveLevel, FrozenWindowParts, SelfReportAnswer, TrialResolution, VersionId,
    VersionStatus, WeightLedger, WeightSource, WeightVersion, Weights,
};
use agent_value_core::window::{FrozenInputs, WindowIndex};
use agent_value_core::CoreError;

pub use digest::{sha256_hex, DigestKey, MIN_DIGEST_KEY_BYTES};
pub use error::{StoreError, StoreResult};

/// One stored outcome judgement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredJudgement {
    pub judged_at_ms: i64,
    pub judgement: Judgement,
    pub rules_version: String,
}

/// Current schema version (`PRAGMA user_version`).
pub const SCHEMA_VERSION: i64 = schema::MIGRATIONS.len() as i64;

const MS_PER_DAY: i64 = 86_400_000;

/// Row id of an arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArmId(pub i64);

/// The first probe-consent decision stored for this database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeConsent {
    pub accepted: bool,
    pub at_ms: i64,
}

/// Kinds of versioned configuration frozen at window start (§8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigKind {
    ScopeConfig,
    ClassificationRules,
    PriceTable,
    Reference,
    Fallback,
}

impl ConfigKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ScopeConfig => "scope-config",
            Self::ClassificationRules => "classification-rules",
            Self::PriceTable => "price-table",
            Self::Reference => "reference",
            Self::Fallback => "fallback",
        }
    }
}

/// A stored configuration version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigVersion {
    pub kind: &'static str,
    /// Scope id, or `*` for all scopes.
    pub scope: String,
    pub label: String,
    pub payload: String,
    pub content_sha256: String,
    pub created_ms: i64,
}

/// One user task in a window, with the arm route and the summed charge.
///
/// Probe tasks are never listed. `charged_micros` is `None` when no
/// non-auxiliary request recorded a charge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserTaskRow {
    pub id: String,
    pub model: String,
    pub provider_kind: String,
    pub host: String,
    pub protocol: String,
    pub client: String,
    pub cell: CellId,
    pub submitted_ms: i64,
    pub ended_ms: Option<i64>,
    pub outcome: Outcome,
    pub active_ms: Option<i64>,
    pub wait_ms: Option<i64>,
    pub rating: Option<u8>,
    pub charged_micros: Option<i64>,
}

/// Rows removed by [`Store::purge_older_than`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PurgeCounts {
    pub requests: usize,
    pub tasks: usize,
    pub behavior_events: usize,
    pub probe_results: usize,
}

/// One table in an export.
#[derive(Debug, Clone, PartialEq)]
pub struct TableDump {
    pub name: &'static str,
    pub columns: Vec<String>,
    /// Values as text; `None` is SQL NULL. Reals use Rust's round-trip format.
    pub rows: Vec<Vec<Option<String>>>,
}

/// A deterministic dump of every table (rows in rowid order).
#[derive(Debug, Clone, PartialEq)]
pub struct ExportDump {
    pub schema_version: i64,
    pub tables: Vec<TableDump>,
}

/// The local agent-value database.
pub struct Store {
    conn: Connection,
    digest_key: DigestKey,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").finish_non_exhaustive()
    }
}

fn to_i64(what: &'static str, v: u64) -> StoreResult<i64> {
    i64::try_from(v).map_err(|_| StoreError::Core(CoreError::Overflow { what }))
}

fn corrupt<E: std::fmt::Display>(e: E) -> StoreError {
    StoreError::Corrupt(e.to_string())
}

fn latest_label(conn: &Connection, kind: ConfigKind, scope: &str) -> StoreResult<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT label FROM config_versions WHERE kind = ?1 AND scope = ?2
             ORDER BY created_ms DESC, label ASC LIMIT 1",
            params![kind.as_str(), scope],
            |r| r.get(0),
        )
        .optional()?)
}

impl Store {
    /// Opens (creating if needed) a database file and migrates it.
    pub fn open(path: &Path, digest_key: DigestKey) -> StoreResult<Self> {
        let conn = Connection::open(path)?;
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        Self::init(conn, digest_key)
    }

    /// An in-memory database (for tests and previews).
    pub fn open_in_memory(digest_key: DigestKey) -> StoreResult<Self> {
        Self::init(Connection::open_in_memory()?, digest_key)
    }

    fn init(mut conn: Connection, digest_key: DigestKey) -> StoreResult<Self> {
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.pragma_update(None, "secure_delete", true)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(StoreError::SchemaTooNew {
                found: version,
                supported: SCHEMA_VERSION,
            });
        }
        for (i, sql) in schema::MIGRATIONS.iter().enumerate().skip(version as usize) {
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", (i + 1) as i64)?;
            tx.commit()?;
        }
        Ok(Self { conn, digest_key })
    }

    /// The schema version of the open database.
    pub fn schema_version(&self) -> StoreResult<i64> {
        Ok(self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?)
    }

    /// Facts about a piece of user text: category, length and keyed digest.
    /// The text itself is dropped here and never stored.
    pub fn text_facts(&self, category: &str, text: &str) -> StoreResult<TextFacts> {
        let len = u32::try_from(text.chars().count()).unwrap_or(u32::MAX);
        Ok(TextFacts::new(
            category,
            len,
            &self.digest_key.digest_hex(text),
        )?)
    }

    // ---- arms ----

    /// Returns the id of `arm`, inserting it if new. Two arms are the same
    /// row exactly when model and every route field match.
    pub fn upsert_arm(&self, arm: &Arm) -> StoreResult<ArmId> {
        upsert_arm(&self.conn, arm)
    }

    pub fn arm(&self, id: ArmId) -> StoreResult<Arm> {
        load_arm(&self.conn, id)
    }

    pub fn arm_count(&self) -> StoreResult<u64> {
        count(&self.conn, "SELECT COUNT(*) FROM arms")
    }

    /// Every arm, in id order. Rows contain only the model and route fields
    /// stored in `arms` (no key material; the table has no such column).
    pub fn list_arms(&self) -> StoreResult<Vec<(ArmId, Arm)>> {
        let mut statement = self.conn.prepare("SELECT id FROM arms ORDER BY id")?;
        let ids = statement
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|id| {
                let arm_id = ArmId(id);
                Ok((arm_id, load_arm(&self.conn, arm_id)?))
            })
            .collect()
    }

    // ---- tasks and requests ----

    pub fn insert_task(&self, task: &TaskRecord) -> StoreResult<()> {
        task.validate()?;
        let arm_id = upsert_arm(&self.conn, &task.arm)?;
        let fm = task.first_message.as_ref();
        self.conn.execute(
            "INSERT INTO tasks (id, scope, cell, is_probe, arm_id, submitted_ms, ended_ms, outcome,
                first_msg_category, first_msg_chars, first_msg_digest, active_ms, wait_ms,
                interruptions, rating)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                task.id,
                task.scope.to_string(),
                task.cell.as_str(),
                task.is_probe(),
                arm_id.0,
                task.submitted_ms,
                task.ended_ms,
                task.outcome.as_str(),
                fm.map(|f| f.category()),
                fm.map(|f| f.char_len()),
                fm.map(|f| f.digest_hex()),
                task.active_ms,
                task.wait_ms,
                task.interruptions,
                task.rating,
            ],
        )?;
        Ok(())
    }

    /// Records a (re-)judgement of a task's outcome (§3.1): appends it to the
    /// judgement history and sets the task's current outcome, atomically.
    /// `rules_version` names the frozen classification rules used.
    pub fn record_judgement(
        &mut self,
        task_id: &str,
        judgement: &Judgement,
        judged_at_ms: i64,
        rules_version: &str,
    ) -> StoreResult<()> {
        if rules_version.trim().is_empty() {
            return Err(StoreError::Core(CoreError::EmptyField {
                field: "rules_version",
            }));
        }
        if has_secret_prefix(rules_version) {
            return Err(StoreError::Core(CoreError::LooksLikeSecret {
                field: "rules_version",
            }));
        }
        let tx = self.conn.transaction()?;
        let updated = tx.execute(
            "UPDATE tasks SET outcome = ?2 WHERE id = ?1",
            params![task_id, judgement.outcome.as_str()],
        )?;
        if updated == 0 {
            return Err(StoreError::Conflict("judgement for an unknown task"));
        }
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM outcome_judgements WHERE task_id = ?1",
            [task_id],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO outcome_judgements (task_id, seq, judged_at_ms, outcome, rule,
                provisional, final_after_ms, rules_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                task_id,
                seq,
                judged_at_ms,
                judgement.outcome.as_str(),
                judgement.rule.as_str(),
                judgement.provisional,
                judgement.final_after_ms,
                rules_version.trim(),
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The judgement history of a task, oldest first.
    pub fn judgement_history(&self, task_id: &str) -> StoreResult<Vec<StoredJudgement>> {
        let mut st = self.conn.prepare(
            "SELECT judged_at_ms, outcome, rule, provisional, final_after_ms, rules_version
             FROM outcome_judgements WHERE task_id = ?1 ORDER BY seq",
        )?;
        let rows = st
            .query_map([task_id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, bool>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, String>(5)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(
                |(at, outcome, rule, provisional, final_after_ms, rules_version)| {
                    Ok(StoredJudgement {
                        judged_at_ms: at,
                        judgement: Judgement {
                            outcome: Outcome::parse(&outcome)?,
                            rule: Rule::parse(&rule)?,
                            provisional,
                            final_after_ms,
                        },
                        rules_version,
                    })
                },
            )
            .collect()
    }

    /// The task's current outcome.
    pub fn task_outcome(&self, task_id: &str) -> StoreResult<Option<Outcome>> {
        let s: Option<String> = self
            .conn
            .query_row("SELECT outcome FROM tasks WHERE id = ?1", [task_id], |r| {
                r.get(0)
            })
            .optional()?;
        s.map(|s| Outcome::parse(&s).map_err(StoreError::Core))
            .transpose()
    }

    /// Sets (or replaces) the user's 1-5 star rating of a task; one rating
    /// per task, the latest wins (§3.1). Probe tasks cannot be rated.
    pub fn set_task_rating(&self, task_id: &str, stars: u8) -> StoreResult<()> {
        agent_value_core::quality::stars_to_quality(stars)?;
        let n = self.conn.execute(
            "UPDATE tasks SET rating = ?2 WHERE id = ?1",
            params![task_id, stars],
        )?;
        if n == 0 {
            return Err(StoreError::Conflict("rating for an unknown task"));
        }
        Ok(())
    }

    pub fn insert_request(&self, req: &RequestRecord) -> StoreResult<()> {
        req.validate()?;
        let arm_id = upsert_arm(&self.conn, &req.arm)?;
        self.conn.execute(
            "INSERT INTO requests (id, task_id, arm_id, auxiliary, returned_model, response_id,
                key_env_var, start_ms, first_token_ms, end_ms, input_tokens, output_tokens,
                cache_read_tokens, cache_write_tokens, charged_micro_yuan)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                req.id,
                req.task_id,
                arm_id.0,
                req.auxiliary,
                req.returned_model,
                req.response_id,
                req.key_env_var.as_ref().map(|k| k.as_str()),
                req.start_ms,
                req.first_token_ms,
                req.end_ms,
                to_i64("input tokens", req.usage.input)?,
                to_i64("output tokens", req.usage.output)?,
                to_i64("cache read tokens", req.usage.cache_read)?,
                to_i64("cache write tokens", req.usage.cache_write)?,
                req.charged.map(|c| c.micros()),
            ],
        )?;
        Ok(())
    }

    /// Number of requests stored for `arm`.
    pub fn request_count_for_arm(&self, arm: ArmId) -> StoreResult<u64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM requests WHERE arm_id = ?1",
            [arm.0],
            |r| r.get::<_, i64>(0),
        )? as u64)
    }

    /// User (non-probe) task counts per cell for `scope` with
    /// `from_ms <= submitted_ms < to_ms`. Probe tasks are never counted.
    pub fn user_task_counts(
        &self,
        scope: &ScopeId,
        from_ms: i64,
        to_ms: i64,
    ) -> StoreResult<Vec<(CellId, u64)>> {
        let mut st = self.conn.prepare(
            "SELECT cell, COUNT(*) FROM tasks
             WHERE scope = ?1 AND is_probe = 0 AND submitted_ms >= ?2 AND submitted_ms < ?3
             GROUP BY cell ORDER BY cell",
        )?;
        let rows = st.query_map(params![scope.to_string(), from_ms, to_ms], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (cell, n) = row?;
            out.push((CellId::parse(&cell).map_err(corrupt)?, n as u64));
        }
        Ok(out)
    }

    /// User (non-probe) tasks of `scope` submitted before `before_ms`; zero
    /// means the next window is a first window (§A).
    pub fn user_task_count_before(&self, scope: &ScopeId, before_ms: i64) -> StoreResult<u64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM tasks WHERE scope = ?1 AND is_probe = 0 AND submitted_ms < ?2",
            params![scope.to_string(), before_ms],
            |r| r.get::<_, i64>(0),
        )? as u64)
    }

    /// User (non-probe) tasks of `scope` with `from_ms <= submitted_ms < to_ms`,
    /// oldest first. `charged_micros` is the sum of non-auxiliary request
    /// charges, or `None` when every charge is missing.
    pub fn list_user_tasks(
        &self,
        scope: &ScopeId,
        from_ms: i64,
        to_ms: i64,
    ) -> StoreResult<Vec<UserTaskRow>> {
        let mut st = self.conn.prepare(
            "SELECT t.id, a.model, a.provider_kind, a.host, a.protocol, a.client,
                    t.cell, t.submitted_ms, t.ended_ms, t.outcome, t.active_ms, t.wait_ms, t.rating,
                    (SELECT SUM(r.charged_micro_yuan) FROM requests r
                      WHERE r.task_id = t.id AND r.auxiliary = 0)
             FROM tasks t
             JOIN arms a ON a.id = t.arm_id
             WHERE t.scope = ?1 AND t.is_probe = 0 AND t.submitted_ms >= ?2 AND t.submitted_ms < ?3
             ORDER BY t.submitted_ms, t.id",
        )?;
        let rows = st.query_map(params![scope.to_string(), from_ms, to_ms], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, i64>(7)?,
                r.get::<_, Option<i64>>(8)?,
                r.get::<_, String>(9)?,
                r.get::<_, Option<i64>>(10)?,
                r.get::<_, Option<i64>>(11)?,
                r.get::<_, Option<i64>>(12)?,
                r.get::<_, Option<i64>>(13)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (
                id,
                model,
                provider_kind,
                host,
                protocol,
                client,
                cell,
                submitted_ms,
                ended_ms,
                outcome,
                active_ms,
                wait_ms,
                rating,
                charged_micros,
            ) = row?;
            let rating = match rating {
                None => None,
                Some(stars) => Some(u8::try_from(stars).map_err(|_| {
                    StoreError::Corrupt(format!("rating {stars} does not fit in u8"))
                })?),
            };
            out.push(UserTaskRow {
                id,
                model,
                provider_kind,
                host,
                protocol,
                client,
                cell: CellId::parse(&cell).map_err(corrupt)?,
                submitted_ms,
                ended_ms,
                outcome: Outcome::parse(&outcome).map_err(corrupt)?,
                active_ms,
                wait_ms,
                rating,
                charged_micros,
            });
        }
        Ok(out)
    }

    /// Newest configuration label for `scope`, else the all-scope (`*`) row.
    pub fn latest_config_label(
        &self,
        kind: ConfigKind,
        scope: &ScopeId,
    ) -> StoreResult<Option<String>> {
        if let Some(label) = latest_label(&self.conn, kind, &scope.to_string())? {
            return Ok(Some(label));
        }
        latest_label(&self.conn, kind, "*")
    }

    // ---- behaviour events ----

    /// Stores an event and its available-arm snapshot atomically.
    pub fn insert_behavior_event(&mut self, ev: &BehaviorEvent) -> StoreResult<()> {
        ev.validate()?;
        let tx = self.conn.transaction()?;
        let chosen = upsert_arm(&tx, &ev.chosen)?;
        tx.execute(
            "INSERT INTO behavior_events (id, at_ms, kind, scope, cell, chosen_arm_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                ev.id,
                ev.at_ms,
                ev.kind.as_str(),
                ev.scope.to_string(),
                ev.cell.as_str(),
                chosen.0
            ],
        )?;
        for a in &ev.available {
            let id = upsert_arm(&tx, a)?;
            tx.execute(
                "INSERT INTO event_available_arms (event_id, arm_id) VALUES (?1, ?2)",
                params![ev.id, id.0],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The available-arm snapshot of an event, sorted.
    pub fn event_available_arms(&self, event_id: &str) -> StoreResult<Vec<Arm>> {
        let mut st = self
            .conn
            .prepare("SELECT arm_id FROM event_available_arms WHERE event_id = ?1")?;
        let ids: Vec<i64> = st
            .query_map([event_id], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        let mut arms = ids
            .into_iter()
            .map(|id| load_arm(&self.conn, ArmId(id)))
            .collect::<StoreResult<Vec<_>>>()?;
        arms.sort();
        Ok(arms)
    }

    // ---- probes ----

    /// Stores a probe result. One result per (probe, version, arm, run day).
    pub fn insert_probe_result(&self, p: &ProbeResult) -> StoreResult<()> {
        insert_probe_result_on(&self.conn, p)
    }

    /// Stores every class result of one probe run and the charge for that
    /// run. One transaction: a failure leaves neither the results nor the
    /// charge. This does not send a request. The charge is the amount the
    /// caller passes.
    pub fn record_probe_run(
        &self,
        results: &[ProbeResult],
        at_ms: i64,
        charged: MicroYuan,
    ) -> StoreResult<()> {
        if results.is_empty() {
            return Err(StoreError::Conflict("probe run has no results"));
        }
        let tx = self.conn.unchecked_transaction()?;
        for result in results {
            insert_probe_result_on(&tx, result)?;
        }
        record_probe_charge_on(&tx, at_ms, charged)?;
        tx.commit()?;
        Ok(())
    }

    /// Every stored probe result, ordered by arm, time, then id.
    pub fn list_probe_results(&self) -> StoreResult<Vec<ProbeResult>> {
        let mut st = self.conn.prepare(
            "SELECT probe_id, probe_version, arm_id, at_ms, class, verdict, hard_failure,
                    evidence_summary
             FROM probe_results ORDER BY arm_id, at_ms, id",
        )?;
        let rows = st
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, String>(7)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut out = Vec::with_capacity(rows.len());
        for (probe_id, probe_version, arm_id, at_ms, class, verdict, hard, evidence) in rows {
            let result = ProbeResult {
                probe_id,
                probe_version,
                arm: load_arm(&self.conn, ArmId(arm_id))?,
                at_ms,
                class: ProbeClass::parse(&class)?,
                verdict: ProbeVerdict::parse(&verdict)?,
                hard_failure: hard != 0,
                evidence_summary: evidence,
            };
            result.validate()?;
            out.push(result);
        }
        Ok(out)
    }

    /// The first probe-consent decision, if one was stored.
    pub fn probe_consent(&self) -> StoreResult<Option<ProbeConsent>> {
        Ok(self
            .conn
            .query_row(
                "SELECT accepted, at_ms FROM probe_consent WHERE id = 1",
                [],
                |r| {
                    let accepted: i64 = r.get(0)?;
                    Ok(ProbeConsent {
                        accepted: accepted != 0,
                        at_ms: r.get(1)?,
                    })
                },
            )
            .optional()?)
    }

    /// Records the first consent decision. A later call returns the stored
    /// row and does not change it.
    pub fn set_probe_consent(&self, accepted: bool, at_ms: i64) -> StoreResult<ProbeConsent> {
        if let Some(existing) = self.probe_consent()? {
            return Ok(existing);
        }
        self.conn.execute(
            "INSERT INTO probe_consent (id, accepted, at_ms) VALUES (1, ?1, ?2)",
            params![i64::from(accepted), at_ms],
        )?;
        Ok(ProbeConsent { accepted, at_ms })
    }

    /// Records money already spent on probes. This does not send a request.
    pub fn record_probe_charge(&self, at_ms: i64, charged: MicroYuan) -> StoreResult<()> {
        record_probe_charge_on(&self.conn, at_ms, charged)
    }

    /// Probe charges as `(at_ms, micro-yuan)`, ordered by time.
    pub fn probe_charges(&self) -> StoreResult<Vec<(i64, i64)>> {
        let mut st = self
            .conn
            .prepare("SELECT at_ms, charged_micro_yuan FROM probe_charges ORDER BY at_ms, id")?;
        let rows = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Stored run days of probe results for `arm`, sorted.
    pub fn probe_run_days(&self, arm: ArmId) -> StoreResult<Vec<String>> {
        let mut st = self
            .conn
            .prepare("SELECT run_day FROM probe_results WHERE arm_id = ?1 ORDER BY run_day")?;
        let v = st
            .query_map([arm.0], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(v)
    }

    // ---- cell weights ----

    /// Freezes the cell weights of `scope` for `window`. Immutable once set.
    /// The probe cell is stored explicitly with omega = 0.
    pub fn set_cell_weights(
        &mut self,
        scope: &ScopeId,
        window: WindowIndex,
        weights: &CellWeights,
    ) -> StoreResult<()> {
        let tx = self.conn.transaction()?;
        let exists: i64 = tx.query_row(
            "SELECT COUNT(*) FROM cell_weights WHERE scope = ?1 AND window_index = ?2",
            params![scope.to_string(), window.0],
            |r| r.get(0),
        )?;
        if exists > 0 {
            return Err(StoreError::Conflict(
                "cell weights for this window are already frozen",
            ));
        }
        let mut rows: Vec<(&str, f64)> = weights
            .entries()
            .iter()
            .map(|(c, w)| (c.as_str(), *w))
            .collect();
        rows.push((CellId::Probe.as_str(), weights.omega(&CellId::Probe)));
        for (cell, w) in rows {
            tx.execute(
                "INSERT INTO cell_weights (scope, window_index, cell, omega) VALUES (?1, ?2, ?3, ?4)",
                params![scope.to_string(), window.0, cell, w],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn cell_weights(
        &self,
        scope: &ScopeId,
        window: WindowIndex,
    ) -> StoreResult<Option<CellWeights>> {
        let mut st = self.conn.prepare(
            "SELECT cell, omega FROM cell_weights WHERE scope = ?1 AND window_index = ?2 ORDER BY cell",
        )?;
        let rows: Vec<(String, f64)> = st
            .query_map(params![scope.to_string(), window.0], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?
            .collect::<Result<_, _>>()?;
        if rows.is_empty() {
            return Ok(None);
        }
        let entries = rows
            .into_iter()
            .map(|(c, w)| Ok((CellId::parse(&c).map_err(corrupt)?, w)))
            .collect::<StoreResult<Vec<_>>>()?;
        Ok(Some(CellWeights::new(entries).map_err(corrupt)?))
    }

    // ---- weight ledger ----

    /// Saves a scope's weight ledger. Rows already stored must be unchanged
    /// (versions and frozen windows are immutable); only new versions, new
    /// trial resolutions and new frozen windows are added.
    pub fn save_weight_ledger(&mut self, ledger: &WeightLedger) -> StoreResult<()> {
        let scope = ledger.scope().to_string();
        let old = self.load_weight_ledger(ledger.scope())?;
        let (old_versions, old_frozen) = match &old {
            Some(o) => (o.versions(), o.frozen_windows()),
            None => (&[][..], &[][..]),
        };
        if old_versions.len() > ledger.versions().len()
            || old_frozen.len() > ledger.frozen_windows().len()
        {
            return Err(StoreError::Conflict("ledger is older than the stored one"));
        }
        for (o, n) in old_versions.iter().zip(ledger.versions()) {
            if strip_resolution(o) != strip_resolution(n) {
                return Err(StoreError::Conflict("a stored weight version would change"));
            }
            let (ro, rn) = (resolution(o), resolution(n));
            if ro.is_some_and(|r| r != TrialResolution::Pending) && ro != rn {
                return Err(StoreError::Conflict("a trial resolution would change"));
            }
        }
        for (o, n) in old_frozen.iter().zip(ledger.frozen_windows()) {
            if o != n {
                return Err(StoreError::Conflict("a frozen window would change"));
            }
        }
        let tx = self.conn.transaction()?;
        for (i, v) in ledger.versions().iter().enumerate() {
            if i >= old_versions.len() {
                insert_version(&tx, &scope, v)?;
            }
            let was_pending = old_versions
                .get(i)
                .is_none_or(|o| resolution(o) == Some(TrialResolution::Pending));
            if was_pending {
                insert_resolution(&tx, &scope, v)?;
            }
        }
        for f in ledger.frozen_windows().iter().skip(old_frozen.len()) {
            let inp = f.inputs();
            tx.execute(
                "INSERT INTO frozen_windows (scope, window_index, weight_version_id, is_first_window,
                    cell_weights_version, reference_version, fallback_version, price_table_version,
                    classification_rules_version)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    scope,
                    f.window().0,
                    to_i64("version id", f.weight_version_id().0)?,
                    f.is_first_window(),
                    inp.cell_weights_version,
                    inp.reference_version,
                    inp.fallback_version,
                    inp.price_table_version,
                    inp.classification_rules_version,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Loads and re-validates a scope's weight ledger, if one was saved.
    pub fn load_weight_ledger(&self, scope: &ScopeId) -> StoreResult<Option<WeightLedger>> {
        let s = scope.to_string();
        let mut st = self.conn.prepare(
            "SELECT id, a, b, g, source, status, effective_from, created_in, previous
             FROM weight_versions WHERE scope = ?1 ORDER BY id",
        )?;
        type Row = (
            i64,
            f64,
            f64,
            f64,
            String,
            String,
            Option<i64>,
            Option<i64>,
            Option<i64>,
        );
        let rows: Vec<Row> = st
            .query_map([&s], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                ))
            })?
            .collect::<Result<_, _>>()?;
        if rows.is_empty() {
            return Ok(None);
        }
        let mut versions = Vec::with_capacity(rows.len());
        for (id, a, b, g, source, status, eff, created, prev) in rows {
            let weights = Weights::new(a, b, g).map_err(corrupt)?;
            let source = WeightSource::parse(&source).map_err(corrupt)?;
            let evidence = load_evidence(&self.conn, &s, id)?;
            let window = |w: Option<i64>| -> StoreResult<WindowIndex> {
                w.and_then(|w| u32::try_from(w).ok())
                    .map(WindowIndex)
                    .ok_or_else(|| StoreError::Corrupt("bad window index".into()))
            };
            let status = match status.as_str() {
                "official" => VersionStatus::Official {
                    effective_from: window(eff)?,
                },
                "trial" => VersionStatus::Trial {
                    created_in: window(created)?,
                    resolution: load_resolution(&self.conn, &s, id)?,
                },
                other => return Err(StoreError::Corrupt(format!("bad status {other}"))),
            };
            versions.push(
                WeightVersion::from_parts(
                    VersionId(id as u64),
                    scope.clone(),
                    weights,
                    source,
                    evidence,
                    status,
                    prev.map(|p| VersionId(p as u64)),
                )
                .map_err(corrupt)?,
            );
        }
        let mut st = self.conn.prepare(
            "SELECT window_index, weight_version_id, is_first_window, cell_weights_version,
                reference_version, fallback_version, price_table_version, classification_rules_version
             FROM frozen_windows WHERE scope = ?1 ORDER BY window_index",
        )?;
        let frozen = st
            .query_map([&s], |r| {
                Ok(FrozenWindowParts {
                    window: WindowIndex(r.get(0)?),
                    weight_version: VersionId(r.get::<_, i64>(1)? as u64),
                    is_first_window: r.get(2)?,
                    inputs: FrozenInputs {
                        cell_weights_version: r.get(3)?,
                        reference_version: r.get(4)?,
                        fallback_version: r.get(5)?,
                        price_table_version: r.get(6)?,
                        classification_rules_version: r.get(7)?,
                    },
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(
            WeightLedger::from_parts(scope.clone(), versions, frozen).map_err(corrupt)?,
        ))
    }

    // ---- other configuration versions ----

    /// Stores an immutable configuration version. The payload is opaque here
    /// (its format is defined by later steps) but must not carry secrets.
    pub fn put_config_version(
        &self,
        kind: ConfigKind,
        scope: Option<&ScopeId>,
        label: &str,
        payload: &str,
        created_ms: i64,
    ) -> StoreResult<()> {
        if label.trim().is_empty() {
            return Err(CoreError::EmptyField {
                field: "config label",
            }
            .into());
        }
        if has_secret_prefix(payload) || has_secret_prefix(label) {
            return Err(CoreError::LooksLikeSecret {
                field: "config payload",
            }
            .into());
        }
        let scope = scope.map_or_else(|| "*".to_string(), ToString::to_string);
        self.conn.execute(
            "INSERT INTO config_versions (kind, scope, label, payload, content_sha256, created_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                kind.as_str(),
                scope,
                label,
                payload,
                sha256_hex(payload.as_bytes()),
                created_ms
            ],
        )?;
        Ok(())
    }

    pub fn config_version(
        &self,
        kind: ConfigKind,
        scope: Option<&ScopeId>,
        label: &str,
    ) -> StoreResult<Option<ConfigVersion>> {
        let scope = scope.map_or_else(|| "*".to_string(), ToString::to_string);
        Ok(self
            .conn
            .query_row(
                "SELECT scope, label, payload, content_sha256, created_ms FROM config_versions
                 WHERE kind = ?1 AND scope = ?2 AND label = ?3",
                params![kind.as_str(), scope, label],
                |r| {
                    Ok(ConfigVersion {
                        kind: kind.as_str(),
                        scope: r.get(0)?,
                        label: r.get(1)?,
                        payload: r.get(2)?,
                        content_sha256: r.get(3)?,
                        created_ms: r.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    // ---- retention, export, delete ----

    /// Deletes task-level data older than the 180-day retention period
    /// (§8): requests, tasks (with their requests), behaviour events (with
    /// their snapshots) and probe results. Versions, frozen windows and
    /// arms are kept as the audit trail. Idempotent.
    pub fn purge_older_than(&mut self, now_ms: i64) -> StoreResult<PurgeCounts> {
        let cutoff = now_ms
            .checked_sub(i64::from(RETENTION_DAYS) * MS_PER_DAY)
            .ok_or(StoreError::Core(CoreError::Overflow {
                what: "retention cutoff",
            }))?;
        let tx = self.conn.transaction()?;
        let requests = tx.execute("DELETE FROM requests WHERE start_ms < ?1", [cutoff])?;
        let tasks = tx.execute("DELETE FROM tasks WHERE submitted_ms < ?1", [cutoff])?;
        let behavior_events =
            tx.execute("DELETE FROM behavior_events WHERE at_ms < ?1", [cutoff])?;
        let probe_results = tx.execute("DELETE FROM probe_results WHERE at_ms < ?1", [cutoff])?;
        tx.commit()?;
        Ok(PurgeCounts {
            requests,
            tasks,
            behavior_events,
            probe_results,
        })
    }

    /// Every table, every row, in a fixed order.
    pub fn export(&self) -> StoreResult<ExportDump> {
        let mut tables = Vec::with_capacity(schema::TABLES.len());
        for &name in schema::TABLES {
            let mut st = self
                .conn
                .prepare(&format!("SELECT * FROM {name} ORDER BY rowid"))?;
            let columns: Vec<String> = st.column_names().iter().map(|c| c.to_string()).collect();
            let n = columns.len();
            let rows = st
                .query_map([], |r| {
                    (0..n)
                        .map(|i| {
                            Ok(match r.get_ref(i)? {
                                ValueRef::Null => None,
                                ValueRef::Integer(v) => Some(v.to_string()),
                                ValueRef::Real(v) => Some(format!("{v:?}")),
                                ValueRef::Text(t) => Some(String::from_utf8_lossy(t).into_owned()),
                                ValueRef::Blob(b) => Some(sha256_hex(b)),
                            })
                        })
                        .collect::<Result<Vec<_>, rusqlite::Error>>()
                })?
                .collect::<Result<Vec<_>, _>>()?;
            tables.push(TableDump {
                name,
                columns,
                rows,
            });
        }
        Ok(ExportDump {
            schema_version: self.schema_version()?,
            tables,
        })
    }

    /// Deletes every row of every table, then checkpoints the WAL and
    /// vacuums so deleted bytes do not linger in the file (secure_delete is
    /// also on). The schema stays in place.
    pub fn delete_all(&mut self) -> StoreResult<()> {
        let tx = self.conn.transaction()?;
        for name in schema::TABLES.iter().rev() {
            tx.execute(&format!("DELETE FROM {name}"), [])?;
        }
        tx.commit()?;
        self.checkpoint()?;
        self.conn.execute_batch("VACUUM")?;
        self.checkpoint()
    }

    /// Writes the WAL back into the main file and truncates it.
    pub fn checkpoint(&self) -> StoreResult<()> {
        self.conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        Ok(())
    }

    /// Direct access for tests of SQL-level constraints.
    #[doc(hidden)]
    pub fn raw_connection(&self) -> &Connection {
        &self.conn
    }
}

fn count(conn: &Connection, sql: &str) -> StoreResult<u64> {
    Ok(conn.query_row(sql, [], |r| r.get::<_, i64>(0))? as u64)
}

fn insert_probe_result_on(conn: &Connection, p: &ProbeResult) -> StoreResult<()> {
    p.validate()?;
    let arm_id = upsert_arm(conn, &p.arm)?;
    conn.execute(
        "INSERT INTO probe_results (probe_id, probe_version, arm_id, run_day, at_ms, class,
            verdict, hard_failure, evidence_summary)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            p.probe_id,
            p.probe_version,
            arm_id.0,
            p.run_day()?.to_string(),
            p.at_ms,
            p.class.as_str(),
            p.verdict.as_str(),
            p.hard_failure,
            p.evidence_summary,
        ],
    )?;
    Ok(())
}

fn record_probe_charge_on(conn: &Connection, at_ms: i64, charged: MicroYuan) -> StoreResult<()> {
    conn.execute(
        "INSERT INTO probe_charges (at_ms, charged_micro_yuan) VALUES (?1, ?2)",
        params![at_ms, charged.micros()],
    )?;
    Ok(())
}

fn upsert_arm(conn: &Connection, arm: &Arm) -> StoreResult<ArmId> {
    let r = arm.route();
    let p = params![
        arm.model().as_str(),
        r.kind().as_str(),
        r.host(),
        r.protocol().as_str(),
        r.client()
    ];
    conn.execute(
        "INSERT INTO arms (model, provider_kind, host, protocol, client)
         VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT DO NOTHING",
        p,
    )?;
    Ok(ArmId(conn.query_row(
        "SELECT id FROM arms WHERE model = ?1 AND provider_kind = ?2 AND host = ?3
           AND protocol = ?4 AND client = ?5",
        p,
        |row| row.get(0),
    )?))
}

fn load_arm(conn: &Connection, id: ArmId) -> StoreResult<Arm> {
    let (m, k, h, p, c): (String, String, String, String, String) = conn.query_row(
        "SELECT model, provider_kind, host, protocol, client FROM arms WHERE id = ?1",
        [id.0],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )?;
    let route = Route::new(
        ProviderKind::parse(&k).map_err(corrupt)?,
        &h,
        Protocol::parse(&p).map_err(corrupt)?,
        &c,
    )
    .map_err(corrupt)?;
    Ok(Arm::new(ModelId::new(&m).map_err(corrupt)?, route))
}

fn resolution(v: &WeightVersion) -> Option<TrialResolution> {
    match v.status() {
        VersionStatus::Trial { resolution, .. } => Some(resolution),
        VersionStatus::Official { .. } => None,
    }
}

/// The version with a trial's resolution reset to `Pending`, for comparing
/// the immutable part.
fn strip_resolution(v: &WeightVersion) -> WeightVersion {
    match v.status() {
        VersionStatus::Trial { created_in, .. } => WeightVersion::from_parts(
            v.id(),
            v.scope().clone(),
            v.weights(),
            v.source(),
            v.evidence().clone(),
            VersionStatus::Trial {
                created_in,
                resolution: TrialResolution::Pending,
            },
            v.previous(),
        )
        .unwrap_or_else(|_| v.clone()),
        VersionStatus::Official { .. } => v.clone(),
    }
}

fn insert_version(tx: &Transaction<'_>, scope: &str, v: &WeightVersion) -> StoreResult<()> {
    let id = to_i64("version id", v.id().0)?;
    let (status, eff, created) = match v.status() {
        VersionStatus::Official { effective_from } => ("official", Some(effective_from.0), None),
        VersionStatus::Trial { created_in, .. } => ("trial", None, Some(created_in.0)),
    };
    let w = v.weights();
    tx.execute(
        "INSERT INTO weight_versions (scope, id, a, b, g, source, status, effective_from,
            created_in, previous)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            scope,
            id,
            w.a(),
            w.b(),
            w.g(),
            v.source().as_str(),
            status,
            eff,
            created,
            v.previous().map(|p| p.0 as i64)
        ],
    )?;
    match v.evidence() {
        Evidence::None => {}
        Evidence::SelfReportAnswers(answers) => {
            for (seq, a) in answers.iter().enumerate() {
                tx.execute(
                    "INSERT INTO weight_evidence (scope, version_id, seq, kind, item, level)
                     VALUES (?1, ?2, ?3, 'answer', ?4, ?5)",
                    params![scope, id, seq as i64, a.question_id, a.answer.level()],
                )?;
            }
        }
        Evidence::BehaviorEvents(ids) => {
            for (seq, e) in ids.iter().enumerate() {
                tx.execute(
                    "INSERT INTO weight_evidence (scope, version_id, seq, kind, item, level)
                     VALUES (?1, ?2, ?3, 'event', ?4, NULL)",
                    params![scope, id, seq as i64, e],
                )?;
            }
        }
    }
    Ok(())
}

fn insert_resolution(tx: &Transaction<'_>, scope: &str, v: &WeightVersion) -> StoreResult<()> {
    let id = to_i64("version id", v.id().0)?;
    let (res, confirmed_as) = match resolution(v) {
        Some(TrialResolution::Confirmed(o)) => ("confirmed", Some(o.0 as i64)),
        Some(TrialResolution::Discarded) => ("discarded", None),
        _ => return Ok(()),
    };
    tx.execute(
        "INSERT INTO trial_resolutions (scope, version_id, resolution, confirmed_as)
         VALUES (?1, ?2, ?3, ?4)",
        params![scope, id, res, confirmed_as],
    )?;
    Ok(())
}

fn load_evidence(conn: &Connection, scope: &str, id: i64) -> StoreResult<Evidence> {
    let mut st = conn.prepare(
        "SELECT kind, item, level FROM weight_evidence WHERE scope = ?1 AND version_id = ?2
         ORDER BY seq",
    )?;
    let rows: Vec<(String, String, Option<i8>)> = st
        .query_map(params![scope, id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<Result<_, _>>()?;
    if rows.is_empty() {
        return Ok(Evidence::None);
    }
    if rows.iter().all(|r| r.0 == "answer") {
        let answers = rows
            .into_iter()
            .map(|(_, q, l)| {
                let level = l.ok_or_else(|| StoreError::Corrupt("answer without level".into()))?;
                Ok(SelfReportAnswer {
                    question_id: q,
                    answer: FiveLevel::from_level(level).map_err(corrupt)?,
                })
            })
            .collect::<StoreResult<Vec<_>>>()?;
        Ok(Evidence::SelfReportAnswers(answers))
    } else if rows.iter().all(|r| r.0 == "event") {
        Ok(Evidence::BehaviorEvents(
            rows.into_iter().map(|(_, e, _)| e).collect(),
        ))
    } else {
        Err(StoreError::Corrupt("mixed evidence kinds".into()))
    }
}

fn load_resolution(conn: &Connection, scope: &str, id: i64) -> StoreResult<TrialResolution> {
    let row: Option<(String, Option<i64>)> = conn
        .query_row(
            "SELECT resolution, confirmed_as FROM trial_resolutions
             WHERE scope = ?1 AND version_id = ?2",
            params![scope, id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(match row {
        None => TrialResolution::Pending,
        Some((r, Some(o))) if r == "confirmed" => TrialResolution::Confirmed(VersionId(o as u64)),
        Some((r, None)) if r == "discarded" => TrialResolution::Discarded,
        Some((r, _)) => return Err(StoreError::Corrupt(format!("bad trial resolution {r}"))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_cell_id_matches_core() {
        assert_eq!(
            schema::PROBE_CELL_SQL,
            agent_value_core::cells::PROBE_CELL_ID
        );
    }

    #[test]
    fn export_lists_every_table() {
        let s = Store::open_in_memory(DigestKey::from_bytes(&[1u8; 32]).unwrap()).unwrap();
        let mut names: Vec<String> = s
            .raw_connection()
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        names.sort();
        let mut want: Vec<String> = schema::TABLES.iter().map(|s| s.to_string()).collect();
        want.sort();
        assert_eq!(names, want);
    }
}

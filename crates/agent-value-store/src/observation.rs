//! Observation-period controls: preregistered two-day windows and final audits.
//!
//! Writes run in one IMMEDIATE transaction whose clock is `temp.obs_txn_now`.
//! Callers cannot pass `now`, `created_at`, or `voided_at`. The legacy
//! [`agent_value_core::authenticity::observation`] helper is not consulted
//! when deciding whether a window is ready.

use rusqlite::{params, Connection, OptionalExtension};

use crate::fingerprint_baseline::freeze_baseline;
use crate::{load_arm, upsert_arm, ArmId, Store, StoreError, StoreResult};
use agent_value_core::arm::Arm;
use agent_value_core::authenticity::{
    observation_window_verdict, FrozenObservationBaseline, ObservationDayRow, ObservationProbeRef,
    ObservationProbeSet, ObservationWindowInput, ObservationWindowVerdict,
};
use agent_value_core::privacy::has_secret_prefix;
use agent_value_core::records::{ProbeClass, ProbeVerdict};
use agent_value_core::runday::RunDay;
use agent_value_core::{constants, probe, CoreError};

/// Positive or negative observation arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlRole {
    Positive,
    Negative,
}

impl ControlRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Positive => "positive",
            Self::Negative => "negative",
        }
    }

    fn parse(value: &str) -> StoreResult<Self> {
        match value {
            "positive" => Ok(Self::Positive),
            "negative" => Ok(Self::Negative),
            _ => Err(StoreError::Corrupt(format!("control role {value}"))),
        }
    }
}

/// Append-only control action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlAction {
    Set,
    Clear,
}

impl ControlAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Set => "set",
            Self::Clear => "clear",
        }
    }

    fn parse(value: &str) -> StoreResult<Self> {
        match value {
            "set" => Ok(Self::Set),
            "clear" => Ok(Self::Clear),
            _ => Err(StoreError::Corrupt(format!("control action {value}"))),
        }
    }
}

/// One stored control row and the event written in the same transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationControl {
    pub id: i64,
    pub role: ControlRole,
    pub action: ControlAction,
    pub arm: Option<Arm>,
    pub created_at_ms: i64,
}

/// Open: not void and `now < end_exclusive`. Ended: `now >= end_exclusive`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationWindowStatus {
    Open,
    Ended,
    Void,
}

/// A preregistered window. `window_ok` is `Some` only after a final snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationWindowRow {
    pub id: i64,
    pub positive: Arm,
    pub negative: Arm,
    pub start: String,
    pub end_date: String,
    pub end_exclusive_ms: i64,
    pub baseline_key: String,
    pub baseline_content_sha256: String,
    pub tau_js: String,
    pub observation_run_days: u32,
    pub det_pass_min: u32,
    pub det_fail_max: u32,
    pub fp_pass_min: u32,
    pub fp_fail_max: u32,
    pub status: ObservationWindowStatus,
    pub window_ok: Option<bool>,
}

/// What [`Store::register_window`] freezes. Timestamps come from the store clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterObservationWindow {
    pub positive: Arm,
    pub negative: Arm,
    pub start: RunDay,
    pub baseline_key: String,
    pub deterministic_probe_id: String,
    pub deterministic_probe_version: String,
    pub fingerprint_probe_id: String,
    pub fingerprint_probe_version: String,
    pub capability_probe_id: String,
    pub capability_probe_version: String,
}

/// Final snapshot for one ended window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationAuditReport {
    pub window_id: i64,
    pub created_at_ms: i64,
    pub window_ok: bool,
    pub report: String,
}

const KNOWN_CONFLICTS: &[&str] = &[
    "obs_txn_now is missing",
    "intent created_at must equal obs_txn_now",
    "intent must be inserted unconsumed",
    "intent update may only consume 0 to 1",
    "intent consume must change exactly one row",
    "window created_at must equal obs_txn_now",
    "window intent match must be exactly one",
    "observation window must be registered before its Beijing start",
    "observation arm day set already has probe results",
    "arm day overlaps an open observation window",
    "replace is forbidden",
    "voided_at must equal obs_txn_now",
    "void must be before window start",
    "void requires an empty arm day set",
    "unknown observation window",
    "snapshot created_at must equal obs_txn_now",
    "snapshot requires a window that is not void",
    "snapshot requires an ended window",
    "control created_at must equal obs_txn_now",
    "event created_at must equal obs_txn_now",
    "event does not match control",
    "probe write matches an ended observation window",
    "probe write matches more than one open observation window",
    "baseline key and content must both be set or both be null",
    "observation_controls rows are immutable",
    "observation_control_events rows are immutable",
    "observation_window_intents rows are immutable",
    "observation_windows rows are immutable",
    "observation_window_voids rows are immutable",
    "observation_window_snapshots rows are immutable",
    "probe_results rows are immutable",
];

pub(crate) fn map_sql(err: rusqlite::Error) -> StoreError {
    let text = err.to_string();
    for message in KNOWN_CONFLICTS {
        if text.contains(message) {
            return StoreError::Conflict(message);
        }
    }
    if text.contains("UNIQUE constraint failed: observation_windows") {
        return StoreError::Conflict("observation window already registered for this start");
    }
    if text.contains("UNIQUE constraint failed: observation_window_voids") {
        return StoreError::Conflict("observation window is already void");
    }
    if text.contains("UNIQUE constraint failed: observation_window_snapshots") {
        return StoreError::Conflict("observation window already has a final snapshot");
    }
    if text.contains("UNIQUE constraint failed: observation_window_intents") {
        return StoreError::Conflict("an unconsumed observation intent already exists");
    }
    StoreError::Sqlite(err)
}

/// One IMMEDIATE transaction, one clock row. Nested calls keep the clock
/// already written for this transaction.
pub(crate) fn with_obs_immediate<T>(
    conn: &Connection,
    now_ms: i64,
    body: impl FnOnce(&Connection) -> StoreResult<T>,
) -> StoreResult<T> {
    let own = conn.is_autocommit();
    if own {
        conn.execute_batch("BEGIN IMMEDIATE")?;
    }
    let result = (|| {
        write_clock(conn, now_ms, own)?;
        body(conn)
    })();
    if own {
        if result.is_ok() {
            conn.execute_batch("COMMIT")?;
        } else {
            conn.execute_batch("ROLLBACK")?;
        }
    }
    result
}

fn write_clock(conn: &Connection, now_ms: i64, reset: bool) -> StoreResult<()> {
    if reset {
        conn.execute("DELETE FROM temp.obs_txn_now", [])?;
        conn.execute(
            "INSERT INTO temp.obs_txn_now (id, ms) VALUES (1, ?1)",
            [now_ms],
        )?;
        return Ok(());
    }
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM temp.obs_txn_now", [], |r| r.get(0))?;
    if n == 0 {
        conn.execute(
            "INSERT INTO temp.obs_txn_now (id, ms) VALUES (1, ?1)",
            [now_ms],
        )?;
    } else if n != 1 {
        return Err(StoreError::Conflict("obs_txn_now is missing"));
    }
    Ok(())
}

fn txn_now(conn: &Connection) -> StoreResult<i64> {
    conn.query_row("SELECT ms FROM temp.obs_txn_now", [], |r| r.get(0))
        .map_err(map_sql)
}

fn check_probe_token(field: &'static str, value: &str) -> StoreResult<()> {
    if value.trim().is_empty() {
        return Err(CoreError::EmptyField { field }.into());
    }
    if value.len() > 128 || value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(CoreError::InvalidField {
            field,
            reason: "must be at most 128 characters without whitespace",
        }
        .into());
    }
    if has_secret_prefix(value) {
        return Err(CoreError::LooksLikeSecret { field }.into());
    }
    Ok(())
}

/// Direct consume used when a caller updates one intent outside window insert.
/// A conditional update that matches nothing does not fire the intent trigger,
/// so the row count is checked here. Window insert repeats this contract in
/// SQL (`changes() <> 1`).
#[cfg(test)]
fn consume_unconsumed_intent(conn: &Connection, id: i64) -> StoreResult<()> {
    let changed = conn.execute(
        "UPDATE observation_window_intents SET consumed = 1 WHERE id = ?1 AND consumed = 0",
        [id],
    )?;
    if changed != 1 {
        return Err(StoreError::Conflict(
            "intent consume must change exactly one row",
        ));
    }
    Ok(())
}

impl Store {
    /// Records the arm as the current control for `role`.
    ///
    /// A role that already has an arm must be cleared first. The control row
    /// and its event are inserted in this transaction; neither can be updated.
    pub fn set_control(&self, role: ControlRole, arm: &Arm) -> StoreResult<ObservationControl> {
        with_obs_immediate(&self.conn, self.clock.now_ms(), |conn| {
            let now = txn_now(conn)?;
            let arm_id = upsert_arm(conn, arm)?;
            match latest_control(conn, role)? {
                Some((ControlAction::Set, Some(existing))) if existing == arm_id.0 => {
                    return Err(StoreError::Conflict("control is already set"));
                }
                Some((ControlAction::Set, _)) => {
                    return Err(StoreError::Conflict(
                        "clear the control before setting another arm",
                    ));
                }
                _ => {}
            }
            conn.execute(
                "INSERT INTO observation_controls (role, arm_id, action, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![role.as_str(), arm_id.0, ControlAction::Set.as_str(), now],
            )
            .map_err(map_sql)?;
            load_latest_control(conn, role)
        })
    }

    /// Clears `role`. Fails when that role is not currently set.
    pub fn clear_control(&self, role: ControlRole) -> StoreResult<ObservationControl> {
        with_obs_immediate(&self.conn, self.clock.now_ms(), |conn| {
            let now = txn_now(conn)?;
            match latest_control(conn, role)? {
                Some((ControlAction::Set, Some(_))) => {}
                _ => return Err(StoreError::Conflict("control is not set")),
            }
            conn.execute(
                "INSERT INTO observation_controls (role, arm_id, action, created_at)
                 VALUES (?1, NULL, ?2, ?3)",
                params![role.as_str(), ControlAction::Clear.as_str(), now],
            )
            .map_err(map_sql)?;
            load_latest_control(conn, role)
        })
    }

    /// Control history, oldest first.
    pub fn list_observation_controls(&self) -> StoreResult<Vec<ObservationControl>> {
        let mut statement = self.conn.prepare(
            "SELECT id, role, arm_id, action, created_at
             FROM observation_controls ORDER BY id",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(id, role, arm_id, action, created_at)| {
                control_from_parts(&self.conn, id, &role, arm_id, &action, created_at)
            })
            .collect()
    }

    /// Inserts an unconsumed intent and the window that consumes it.
    ///
    /// `start` must still be in the future (strictly before Beijing midnight).
    /// Both arms' two days must have no probe rows, and those arm-days must
    /// not overlap an open window. There is no API that exempts a failed window.
    pub fn register_window(
        &self,
        request: &RegisterObservationWindow,
    ) -> StoreResult<ObservationWindowRow> {
        check_probe_token("deterministic probe id", &request.deterministic_probe_id)?;
        check_probe_token(
            "deterministic probe version",
            &request.deterministic_probe_version,
        )?;
        check_probe_token("fingerprint probe id", &request.fingerprint_probe_id)?;
        check_probe_token(
            "fingerprint probe version",
            &request.fingerprint_probe_version,
        )?;
        check_probe_token("capability probe id", &request.capability_probe_id)?;
        check_probe_token(
            "capability probe version",
            &request.capability_probe_version,
        )?;
        if request.positive == request.negative {
            return Err(StoreError::Conflict(
                "positive and negative arms must differ",
            ));
        }
        with_obs_immediate(&self.conn, self.clock.now_ms(), |conn| {
            let now = txn_now(conn)?;
            let pos = upsert_arm(conn, &request.positive)?;
            let neg = upsert_arm(conn, &request.negative)?;
            if pos == neg {
                return Err(StoreError::Conflict(
                    "positive and negative arms must differ",
                ));
            }
            let frozen = freeze_baseline(conn, &request.baseline_key, now)?;
            let end = request.start.checked_add_days(1)?;
            let end_exclusive = request
                .start
                .checked_add_days(2)?
                .beijing_midnight_unix_ms()?;
            let start = request.start.to_string();
            conn.execute(
                "INSERT INTO observation_window_intents
                    (pos_arm, neg_arm, start, consumed, created_at)
                 VALUES (?1, ?2, ?3, 0, ?4)",
                params![pos.0, neg.0, start, now],
            )
            .map_err(map_sql)?;
            let intent_id = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO observation_windows (
                    intent_id, pos_arm, neg_arm, start, end_date, end_exclusive,
                    baseline_key, baseline_content_sha256,
                    det_pass_min, det_fail_max, fp_pass_min, fp_fail_max, tau_js,
                    det_probe_id, det_probe_version, fp_probe_id, fp_probe_version,
                    cap_probe_id, cap_probe_version, observation_run_days, created_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6,
                    ?7, ?8,
                    ?9, ?10, ?11, ?12, ?13,
                    ?14, ?15, ?16, ?17,
                    ?18, ?19, ?20, ?21
                 )",
                params![
                    intent_id,
                    pos.0,
                    neg.0,
                    start,
                    end.to_string(),
                    end_exclusive,
                    frozen.baseline_key,
                    frozen.content_sha256,
                    i64::from(constants::DETERMINISTIC_PASS_MIN),
                    i64::from(probe::DETERMINISTIC_FAIL_MAX),
                    i64::from(constants::FINGERPRINT_PASS_MIN),
                    i64::from(probe::FINGERPRINT_FAIL_MAX),
                    frozen.tau_js,
                    request.deterministic_probe_id,
                    request.deterministic_probe_version,
                    request.fingerprint_probe_id,
                    request.fingerprint_probe_version,
                    request.capability_probe_id,
                    request.capability_probe_version,
                    i64::try_from(agent_value_core::authenticity::OBSERVATION_RUN_DAYS).map_err(
                        |_| StoreError::Core(CoreError::Overflow {
                            what: "observation_run_days",
                        }),
                    )?,
                    now,
                ],
            )
            .map_err(map_sql)?;
            let window_id = conn.last_insert_rowid();
            let consumed: i64 = conn.query_row(
                "SELECT consumed FROM observation_window_intents WHERE id = ?1",
                [intent_id],
                |row| row.get(0),
            )?;
            if consumed != 1 {
                return Err(StoreError::Conflict(
                    "intent consume must change exactly one row",
                ));
            }
            load_window(conn, window_id, now)
        })
    }

    /// Voids a window before its Beijing start, when neither arm has a probe
    /// on the window's two days. The same start cannot be registered again.
    pub fn void_window(&self, window_id: i64) -> StoreResult<()> {
        with_obs_immediate(&self.conn, self.clock.now_ms(), |conn| {
            let now = txn_now(conn)?;
            let exists: Option<i64> = conn
                .query_row(
                    "SELECT id FROM observation_windows WHERE id = ?1",
                    [window_id],
                    |row| row.get(0),
                )
                .optional()?;
            if exists.is_none() {
                return Err(StoreError::Conflict("unknown observation window"));
            }
            conn.execute(
                "INSERT INTO observation_window_voids (window_id, voided_at) VALUES (?1, ?2)",
                params![window_id, now],
            )
            .map_err(map_sql)?;
            Ok(())
        })
    }

    /// Every window, oldest first. Status uses the store clock.
    pub fn list_observation_windows(&self) -> StoreResult<Vec<ObservationWindowRow>> {
        let now = self.clock.now_ms();
        let mut statement = self
            .conn
            .prepare("SELECT id FROM observation_windows ORDER BY id")?;
        let ids: Vec<i64> = statement
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        ids.into_iter()
            .map(|id| load_window(&self.conn, id, now))
            .collect()
    }

    /// Writes the final snapshot once `now >= end_exclusive`.
    ///
    /// An open window returns [`StoreError::Conflict`] and writes nothing.
    /// A later call returns the same snapshot.
    pub fn audit_window(&self, window_id: i64) -> StoreResult<ObservationAuditReport> {
        with_obs_immediate(&self.conn, self.clock.now_ms(), |conn| {
            let now = txn_now(conn)?;
            audit_on(conn, window_id, now)
        })
    }

    /// True when a non-void final snapshot has `window_ok`.
    ///
    /// Reads snapshots only. It does not call the legacy observation helper
    /// and it does not treat a failed window as exempt.
    pub fn audit_ready(&self) -> StoreResult<bool> {
        let ready: i64 = self.conn.query_row(
            "SELECT EXISTS (
                SELECT 1 FROM observation_window_snapshots s
                JOIN observation_windows w ON w.id = s.window_id
                WHERE s.window_ok = 1
                  AND NOT EXISTS (
                      SELECT 1 FROM observation_window_voids v WHERE v.window_id = w.id
                  )
             )",
            [],
            |row| row.get(0),
        )?;
        Ok(ready != 0)
    }

    /// The stored final snapshot. Does not recompute from the legacy helper.
    pub fn audit_report(&self, window_id: i64) -> StoreResult<ObservationAuditReport> {
        load_snapshot(&self.conn, window_id)?.ok_or(StoreError::Conflict(
            "observation window has no final snapshot",
        ))
    }
}

fn audit_on(conn: &Connection, window_id: i64, now: i64) -> StoreResult<ObservationAuditReport> {
    if load_snapshot(conn, window_id)?.is_some() {
        return load_snapshot(conn, window_id)?.ok_or(StoreError::Conflict(
            "observation window has no final snapshot",
        ));
    }
    let meta = window_meta(conn, window_id)?;
    if meta.voided {
        return Err(StoreError::Conflict("observation window is void"));
    }
    if now < meta.end_exclusive {
        return Err(StoreError::Conflict("observation window is still open"));
    }
    let verdict = evaluate_window(conn, &meta)?;
    conn.execute(
        "INSERT INTO observation_window_snapshots (window_id, created_at, window_ok, report)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            window_id,
            now,
            i64::from(verdict.window_ok),
            verdict.report()
        ],
    )
    .map_err(map_sql)?;
    Ok(ObservationAuditReport {
        window_id,
        created_at_ms: now,
        window_ok: verdict.window_ok,
        report: verdict.report(),
    })
}

struct WindowMeta {
    pos_arm: i64,
    neg_arm: i64,
    start: String,
    end_date: String,
    end_exclusive: i64,
    start_ms: i64,
    end_ms: i64,
    voided: bool,
    baseline_key: String,
    content_sha256: String,
    probes: ObservationProbeSet,
}

struct WindowColumns {
    pos_arm: i64,
    neg_arm: i64,
    start: String,
    end_date: String,
    end_exclusive: i64,
    baseline_key: String,
    content_sha256: String,
    det_id: String,
    det_ver: String,
    fp_id: String,
    fp_ver: String,
    cap_id: String,
    cap_ver: String,
}

fn window_meta(conn: &Connection, window_id: i64) -> StoreResult<WindowMeta> {
    let row = conn
        .query_row(
            "SELECT pos_arm, neg_arm, start, end_date, end_exclusive,
                    baseline_key, baseline_content_sha256,
                    det_probe_id, det_probe_version, fp_probe_id, fp_probe_version,
                    cap_probe_id, cap_probe_version
             FROM observation_windows WHERE id = ?1",
            [window_id],
            |row| {
                Ok(WindowColumns {
                    pos_arm: row.get(0)?,
                    neg_arm: row.get(1)?,
                    start: row.get(2)?,
                    end_date: row.get(3)?,
                    end_exclusive: row.get(4)?,
                    baseline_key: row.get(5)?,
                    content_sha256: row.get(6)?,
                    det_id: row.get(7)?,
                    det_ver: row.get(8)?,
                    fp_id: row.get(9)?,
                    fp_ver: row.get(10)?,
                    cap_id: row.get(11)?,
                    cap_ver: row.get(12)?,
                })
            },
        )
        .optional()?;
    let Some(WindowColumns {
        pos_arm,
        neg_arm,
        start,
        end_date,
        end_exclusive,
        baseline_key,
        content_sha256,
        det_id,
        det_ver,
        fp_id,
        fp_ver,
        cap_id,
        cap_ver,
    }) = row
    else {
        return Err(StoreError::Conflict("unknown observation window"));
    };
    let start_day = RunDay::parse(&start)?;
    let end_day = RunDay::parse(&end_date)?;
    let voided: i64 = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM observation_window_voids WHERE window_id = ?1)",
        [window_id],
        |row| row.get(0),
    )?;
    Ok(WindowMeta {
        pos_arm,
        neg_arm,
        start,
        end_date,
        end_exclusive,
        start_ms: start_day.beijing_midnight_unix_ms()?,
        end_ms: end_day.beijing_midnight_unix_ms()?,
        voided: voided != 0,
        baseline_key,
        content_sha256,
        probes: ObservationProbeSet {
            deterministic: ObservationProbeRef {
                probe_id: det_id,
                probe_version: det_ver,
            },
            fingerprint: ObservationProbeRef {
                probe_id: fp_id,
                probe_version: fp_ver,
            },
            capability: ObservationProbeRef {
                probe_id: cap_id,
                probe_version: cap_ver,
            },
        },
    })
}

fn evaluate_window(conn: &Connection, meta: &WindowMeta) -> StoreResult<ObservationWindowVerdict> {
    let rows = day_rows(conn, meta)?;
    let pick = |arm: i64, day: &str| -> Vec<ObservationDayRow> {
        rows.iter()
            .filter(|row| row.0 == arm && row.1 == day)
            .map(|row| row.2.clone())
            .collect()
    };
    let start_positive = pick(meta.pos_arm, &meta.start);
    let start_negative = pick(meta.neg_arm, &meta.start);
    let end_positive = pick(meta.pos_arm, &meta.end_date);
    let end_negative = pick(meta.neg_arm, &meta.end_date);
    let frozen = FrozenObservationBaseline {
        baseline_key: meta.baseline_key.clone(),
        content_sha256: meta.content_sha256.clone(),
    };
    observation_window_verdict(&ObservationWindowInput {
        start_positive: &start_positive,
        start_negative: &start_negative,
        end_positive: &end_positive,
        end_negative: &end_negative,
        probes: &meta.probes,
        frozen: &frozen,
        start_ms: meta.start_ms,
        end_ms: meta.end_ms,
    })
    .map_err(StoreError::from)
}

fn day_rows(
    conn: &Connection,
    meta: &WindowMeta,
) -> StoreResult<Vec<(i64, String, ObservationDayRow)>> {
    let mut statement = conn.prepare(
        "SELECT arm_id, run_day, probe_id, probe_version, class, verdict, hard_failure,
                baseline_key, baseline_content_sha256
         FROM probe_results
         WHERE arm_id IN (?1, ?2) AND run_day IN (?3, ?4)
         ORDER BY id",
    )?;
    let rows = statement
        .query_map(
            params![meta.pos_arm, meta.neg_arm, meta.start, meta.end_date],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(
            |(arm, day, probe_id, probe_version, class, verdict, hard, key, content)| {
                Ok((
                    arm,
                    day,
                    ObservationDayRow {
                        probe_id,
                        probe_version,
                        class: ProbeClass::parse(&class)?,
                        verdict: ProbeVerdict::parse(&verdict)?,
                        hard_failure: hard != 0,
                        baseline_key: key,
                        baseline_content_sha256: content,
                    },
                ))
            },
        )
        .collect()
}

fn load_window(conn: &Connection, id: i64, now: i64) -> StoreResult<ObservationWindowRow> {
    let meta = window_meta(conn, id)?;
    let (det_pass, det_fail, fp_pass, fp_fail, days, tau): (i64, i64, i64, i64, i64, String) = conn
        .query_row(
            "SELECT det_pass_min, det_fail_max, fp_pass_min, fp_fail_max,
                    observation_run_days, tau_js
             FROM observation_windows WHERE id = ?1",
            [id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )?;
    let snapshot = load_snapshot(conn, id)?;
    let status = if meta.voided {
        ObservationWindowStatus::Void
    } else if now < meta.end_exclusive {
        ObservationWindowStatus::Open
    } else {
        ObservationWindowStatus::Ended
    };
    Ok(ObservationWindowRow {
        id,
        positive: load_arm(conn, ArmId(meta.pos_arm))?,
        negative: load_arm(conn, ArmId(meta.neg_arm))?,
        start: meta.start,
        end_date: meta.end_date,
        end_exclusive_ms: meta.end_exclusive,
        baseline_key: meta.baseline_key,
        baseline_content_sha256: meta.content_sha256,
        tau_js: tau,
        observation_run_days: u32::try_from(days)
            .map_err(|_| StoreError::Corrupt("observation_run_days is out of range".into()))?,
        det_pass_min: u32::try_from(det_pass)
            .map_err(|_| StoreError::Corrupt("det_pass_min is out of range".into()))?,
        det_fail_max: u32::try_from(det_fail)
            .map_err(|_| StoreError::Corrupt("det_fail_max is out of range".into()))?,
        fp_pass_min: u32::try_from(fp_pass)
            .map_err(|_| StoreError::Corrupt("fp_pass_min is out of range".into()))?,
        fp_fail_max: u32::try_from(fp_fail)
            .map_err(|_| StoreError::Corrupt("fp_fail_max is out of range".into()))?,
        status,
        window_ok: snapshot.map(|report| report.window_ok),
    })
}

fn load_snapshot(conn: &Connection, window_id: i64) -> StoreResult<Option<ObservationAuditReport>> {
    let row: Option<(i64, i64, String)> = conn
        .query_row(
            "SELECT created_at, window_ok, report FROM observation_window_snapshots
             WHERE window_id = ?1",
            [window_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    Ok(row.map(
        |(created_at_ms, window_ok, report)| ObservationAuditReport {
            window_id,
            created_at_ms,
            window_ok: window_ok != 0,
            report,
        },
    ))
}

fn latest_control(
    conn: &Connection,
    role: ControlRole,
) -> StoreResult<Option<(ControlAction, Option<i64>)>> {
    let row: Option<(String, Option<i64>)> = conn
        .query_row(
            "SELECT action, arm_id FROM observation_controls
             WHERE role = ?1 ORDER BY id DESC LIMIT 1",
            [role.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    match row {
        None => Ok(None),
        Some((action, arm_id)) => Ok(Some((ControlAction::parse(&action)?, arm_id))),
    }
}

fn load_latest_control(conn: &Connection, role: ControlRole) -> StoreResult<ObservationControl> {
    let row: (i64, String, Option<i64>, String, i64) = conn.query_row(
        "SELECT id, role, arm_id, action, created_at FROM observation_controls
         WHERE role = ?1 ORDER BY id DESC LIMIT 1",
        [role.as_str()],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?;
    control_from_parts(conn, row.0, &row.1, row.2, &row.3, row.4)
}

fn control_from_parts(
    conn: &Connection,
    id: i64,
    role: &str,
    arm_id: Option<i64>,
    action: &str,
    created_at_ms: i64,
) -> StoreResult<ObservationControl> {
    let arm = arm_id
        .map(|arm_id| load_arm(conn, ArmId(arm_id)))
        .transpose()?;
    Ok(ObservationControl {
        id,
        role: ControlRole::parse(role)?,
        action: ControlAction::parse(action)?,
        arm,
        created_at_ms,
    })
}

/// Drops the audit DELETE triggers inside the caller's transaction.
///
/// Returns each trigger's `CREATE` statement so the wipe can restore them.
pub(crate) fn drop_audit_delete_triggers(conn: &Connection) -> StoreResult<Vec<String>> {
    let mut statement = conn.prepare(
        "SELECT name, sql FROM sqlite_master
         WHERE type = 'trigger' AND name LIKE 'audit_nodelete_%'
         ORDER BY name",
    )?;
    let rows: Vec<(String, String)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let mut sql = Vec::with_capacity(rows.len());
    for (name, create) in rows {
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(StoreError::Corrupt(format!(
                "unexpected trigger name {name}"
            )));
        }
        conn.execute(&format!("DROP TRIGGER {name}"), [])?;
        sql.push(create);
    }
    Ok(sql)
}

pub(crate) fn restore_audit_delete_triggers(conn: &Connection, sql: &[String]) -> StoreResult<()> {
    for statement in sql {
        conn.execute_batch(statement)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DigestKey, Store};

    #[test]
    fn conditional_consume_checks_changes_when_the_trigger_does_not_fire() {
        let store = Store::open_in_memory(DigestKey::from_bytes(&[7u8; 32]).unwrap()).unwrap();
        let pos = store
            .upsert_arm(&agent_value_core::arm::Arm::new(
                agent_value_core::arm::ModelId::new("example-model-1").unwrap(),
                agent_value_core::arm::Route::new(
                    agent_value_core::arm::ProviderKind::Official,
                    "api.vendor.example",
                    agent_value_core::arm::Protocol::AnthropicMessages,
                    "example-cli",
                )
                .unwrap(),
            ))
            .unwrap();
        let neg = store
            .upsert_arm(&agent_value_core::arm::Arm::new(
                agent_value_core::arm::ModelId::new("example-model-2").unwrap(),
                agent_value_core::arm::Route::new(
                    agent_value_core::arm::ProviderKind::Relay,
                    "relay.example",
                    agent_value_core::arm::Protocol::AnthropicMessages,
                    "example-cli",
                )
                .unwrap(),
            ))
            .unwrap();
        with_obs_immediate(store.raw_connection(), 10, |conn| {
            conn.execute(
                "INSERT INTO observation_window_intents
                    (pos_arm, neg_arm, start, consumed, created_at)
                 VALUES (?1, ?2, '2026-10-28', 0, 10)",
                params![pos.0, neg.0],
            )
            .map_err(map_sql)?;
            let id = conn.last_insert_rowid();
            consume_unconsumed_intent(conn, id)?;
            let again = consume_unconsumed_intent(conn, id);
            assert!(matches!(
                again,
                Err(StoreError::Conflict(
                    "intent consume must change exactly one row"
                ))
            ));
            let forced = conn.execute(
                "UPDATE observation_window_intents SET consumed = 1 WHERE id = ?1",
                [id],
            );
            assert!(
                forced.is_err(),
                "unconditional update of a consumed intent aborts"
            );
            Ok(())
        })
        .unwrap();
    }
}

//! Observation-window audit: preregistration, consume, predicates, ready.

use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use agent_value_core::arm::{Arm, ModelId, Protocol, ProviderKind, Route};
use agent_value_core::authenticity::{observation, DaySummary, Observation, OBSERVATION_RUN_DAYS};
use agent_value_core::constants::{DETERMINISTIC_PASS_MIN, FINGERPRINT_PASS_MIN};
use agent_value_core::probe::{DETERMINISTIC_FAIL_MAX, FINGERPRINT_FAIL_MAX};
use agent_value_core::records::{ProbeClass, ProbeResult, ProbeVerdict};
use agent_value_core::runday::RunDay;
use agent_value_store::{
    sha256_hex, AnswerCount, ControlAction, ControlRole, DigestKey, FingerprintBaselineIdentity,
    FingerprintBaselineUpsert, FingerprintBaselineWrite, FingerprintDistribution, FixedClock,
    ObservationWindowStatus, QuestionAnswers, RegisterObservationWindow, Store, StoreError,
    DISTRIBUTION_SCHEMA_VERSION, FINGERPRINT_BASELINE_TTL_MS,
};

const QUESTIONS: usize = 10;

fn key() -> DigestKey {
    DigestKey::from_bytes(b"test-only-digest-key-aaaaaaaaaaaa").unwrap()
}

fn arm(model: &str, host: &str, kind: ProviderKind) -> Arm {
    Arm::new(
        ModelId::new(model).unwrap(),
        Route::new(kind, host, Protocol::AnthropicMessages, "example-cli").unwrap(),
    )
}

fn positive() -> Arm {
    arm(
        "example-model-1",
        "api.vendor.example",
        ProviderKind::Official,
    )
}

fn negative() -> Arm {
    arm("example-model-2", "relay.example", ProviderKind::Relay)
}

fn other() -> Arm {
    arm(
        "example-model-3",
        "other.example",
        ProviderKind::Subscription,
    )
}

fn question(answer: &str) -> QuestionAnswers {
    QuestionAnswers {
        answers: vec![AnswerCount {
            answer: answer.into(),
            count: 20,
        }],
    }
}

fn distribution(answer: &str) -> FingerprintDistribution {
    FingerprintDistribution {
        run1: vec![question(answer); QUESTIONS],
        run2: vec![question(answer); QUESTIONS],
    }
}

fn baseline_write(collected_at_ms: i64, answer: &str) -> FingerprintBaselineWrite {
    FingerprintBaselineWrite {
        identity: FingerprintBaselineIdentity {
            provider: "example-provider".into(),
            model_id: "example-model".into(),
            model_revision: String::new(),
            protocol: "anthropic-messages".into(),
            connection_kind: "official-direct".into(),
            task_set_version: "probe-set-1".into(),
            system_prompt_hash: sha256_hex(b"observation-prompt"),
            sampling_config: "temperature=1\ntop_p=1\nmax_tokens=64".into(),
            distribution_schema_version: DISTRIBUTION_SCHEMA_VERSION,
        },
        collected_at_ms,
        distribution: distribution(answer),
    }
}

fn open_at(now_ms: i64) -> (Store, FixedClock) {
    let clock = FixedClock::new(now_ms);
    let store = Store::open_in_memory_with_clock(key(), clock.clone()).unwrap();
    (store, clock)
}

fn start_day() -> RunDay {
    RunDay::parse("2026-11-02").unwrap()
}

fn noon(day: RunDay) -> i64 {
    day.beijing_midnight_unix_ms().unwrap() + 12 * 3_600_000
}

struct World {
    store: Store,
    clock: FixedClock,
    baseline_key: String,
}

fn world() -> World {
    let start = start_day();
    let before = start.beijing_midnight_unix_ms().unwrap() - 60_000;
    let (store, clock) = open_at(before);
    let write = baseline_write(before, "red");
    assert_eq!(
        store.upsert_fingerprint_baseline(&write).unwrap(),
        FingerprintBaselineUpsert::Inserted
    );
    let baseline_key = store
        .get_fingerprint_baseline(&write.identity)
        .unwrap()
        .unwrap()
        .baseline_key;
    assert!(before + FINGERPRINT_BASELINE_TTL_MS > noon(start));
    World {
        store,
        clock,
        baseline_key,
    }
}

fn registration(baseline_key: &str, start: RunDay) -> RegisterObservationWindow {
    RegisterObservationWindow {
        positive: positive(),
        negative: negative(),
        start,
        baseline_key: baseline_key.to_string(),
        deterministic_probe_id: "det".into(),
        deterministic_probe_version: "1".into(),
        fingerprint_probe_id: "fp".into(),
        fingerprint_probe_version: "1".into(),
        capability_probe_id: "cap".into(),
        capability_probe_version: "1".into(),
    }
}

fn conflict(result: Result<(), StoreError>) -> &'static str {
    match result {
        Err(StoreError::Conflict(message)) => message,
        Ok(()) => panic!("expected a conflict"),
        Err(err) => panic!("expected a conflict, got {err}"),
    }
}

fn probe(
    store: &Store,
    arm: Arm,
    day: RunDay,
    class: ProbeClass,
    verdict: ProbeVerdict,
    hard: bool,
) {
    let id = match class {
        ProbeClass::Deterministic => "det",
        ProbeClass::Fingerprint => "fp",
        ProbeClass::Capability => "cap",
    };
    store
        .insert_probe_result(&ProbeResult {
            probe_id: id.into(),
            probe_version: "1".into(),
            arm,
            at_ms: noon(day),
            class,
            verdict,
            hard_failure: hard,
            evidence_summary: String::new(),
        })
        .unwrap();
}

fn stamp(store: &Store, now: i64) {
    let conn = store.raw_connection();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    conn.execute("DELETE FROM temp.obs_txn_now", []).unwrap();
    conn.execute(
        "INSERT INTO temp.obs_txn_now (id, ms) VALUES (1, ?1)",
        [now],
    )
    .unwrap();
}

#[test]
fn constants_frozen_on_the_window_match_the_unmodified_thresholds() {
    assert_eq!(OBSERVATION_RUN_DAYS, 2);
    assert_eq!(
        (
            DETERMINISTIC_PASS_MIN,
            DETERMINISTIC_FAIL_MAX,
            FINGERPRINT_PASS_MIN,
            FINGERPRINT_FAIL_MAX
        ),
        (9, 7, 8, 5)
    );
    let world = world();
    let row = world
        .store
        .register_window(&registration(&world.baseline_key, start_day()))
        .unwrap();
    assert_eq!(row.observation_run_days, OBSERVATION_RUN_DAYS as u32);
    assert_eq!(row.det_pass_min, DETERMINISTIC_PASS_MIN);
    assert_eq!(row.det_fail_max, DETERMINISTIC_FAIL_MAX);
    assert_eq!(row.fp_pass_min, FINGERPRINT_PASS_MIN);
    assert_eq!(row.fp_fail_max, FINGERPRINT_FAIL_MAX);
    assert_eq!(row.tau_js, "0");
    assert_eq!(row.end_date, "2026-11-03");
    assert_eq!(
        row.end_exclusive_ms,
        start_day()
            .checked_add_days(2)
            .unwrap()
            .beijing_midnight_unix_ms()
            .unwrap()
    );
    assert_eq!(row.status, ObservationWindowStatus::Open);
    let consumed: i64 = world
        .store
        .raw_connection()
        .query_row("SELECT consumed FROM observation_window_intents", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(consumed, 1);
}

#[test]
fn intent_updates_only_consume_zero_to_one() {
    let world = world();
    let row = world
        .store
        .register_window(&registration(&world.baseline_key, start_day()))
        .unwrap();
    let conn = world.store.raw_connection();
    let back = conn.execute(
        "UPDATE observation_window_intents SET consumed = 0 WHERE consumed = 1",
        [],
    );
    assert!(back.is_err());
    let other = conn.execute(
        "UPDATE observation_window_intents SET start = '2026-12-01'",
        [],
    );
    assert!(other.is_err());
    let again = conn
        .execute(
            "UPDATE observation_window_intents SET consumed = 1 WHERE id = (
                SELECT intent_id FROM observation_windows WHERE id = ?1
             ) AND consumed = 0",
            [row.id],
        )
        .unwrap();
    assert_eq!(
        again, 0,
        "a conditional update that hits nothing is not an abort"
    );
    let deleted = conn.execute("DELETE FROM observation_window_intents", []);
    assert!(deleted.is_err());
    let replaced = conn.execute(
        "INSERT OR REPLACE INTO observation_windows (
            id, intent_id, pos_arm, neg_arm, start, end_date, end_exclusive,
            baseline_key, baseline_content_sha256, det_pass_min, det_fail_max,
            fp_pass_min, fp_fail_max, tau_js, det_probe_id, det_probe_version,
            fp_probe_id, fp_probe_version, cap_probe_id, cap_probe_version,
            observation_run_days, created_at
         )
         SELECT id, intent_id, pos_arm, neg_arm, start, end_date, end_exclusive,
                baseline_key, baseline_content_sha256, det_pass_min, det_fail_max,
                fp_pass_min, fp_fail_max, tau_js, det_probe_id, det_probe_version,
                fp_probe_id, fp_probe_version, cap_probe_id, cap_probe_version,
                observation_run_days, created_at
         FROM observation_windows WHERE id = ?1",
        [row.id],
    );
    assert!(replaced.is_err());
}

#[test]
fn preregistration_rejects_a_window_that_has_already_started() {
    let world = world();
    let start = start_day();
    world.clock.set(start.beijing_midnight_unix_ms().unwrap());
    let err = world
        .store
        .register_window(&registration(&world.baseline_key, start))
        .unwrap_err();
    assert!(matches!(
        err,
        StoreError::Conflict("observation window must be registered before its Beijing start")
    ));
    world
        .clock
        .set(start.beijing_midnight_unix_ms().unwrap() - 1);
    assert!(world
        .store
        .register_window(&registration(&world.baseline_key, start))
        .is_ok());
}

#[test]
fn open_windows_cannot_share_an_arm_day_and_a_later_window_can() {
    let world = world();
    let start = start_day();
    world
        .store
        .register_window(&registration(&world.baseline_key, start))
        .unwrap();
    let overlap = world.store.register_window(&registration(
        &world.baseline_key,
        start.checked_add_days(1).unwrap(),
    ));
    assert_eq!(
        conflict(overlap.map(|_| ())),
        "arm day overlaps an open observation window"
    );
    let next = start.checked_add_days(2).unwrap();
    let row = world
        .store
        .register_window(&registration(&world.baseline_key, next))
        .unwrap();
    assert_eq!(row.start, "2026-11-04");
    assert_eq!(row.status, ObservationWindowStatus::Open);
}

#[test]
fn a_new_window_can_be_registered_after_the_previous_one_ends() {
    let world = world();
    let start = start_day();
    let first = world
        .store
        .register_window(&registration(&world.baseline_key, start))
        .unwrap();
    world.clock.set(first.end_exclusive_ms);
    let listed = world.store.list_observation_windows().unwrap();
    assert_eq!(listed[0].status, ObservationWindowStatus::Ended);
    let later = RunDay::parse("2026-11-06").unwrap();
    let second = world
        .store
        .register_window(&registration(&world.baseline_key, later))
        .unwrap();
    assert_eq!(second.status, ObservationWindowStatus::Open);
    assert_eq!(world.store.list_observation_windows().unwrap().len(), 2);
}

#[test]
fn void_before_start_blocks_the_same_start() {
    let world = world();
    let start = start_day();
    let row = world
        .store
        .register_window(&registration(&world.baseline_key, start))
        .unwrap();
    world.store.void_window(row.id).unwrap();
    assert_eq!(
        world.store.list_observation_windows().unwrap()[0].status,
        ObservationWindowStatus::Void
    );
    let again = world
        .store
        .register_window(&registration(&world.baseline_key, start));
    assert!(again.is_err());
    world.clock.set(noon(start.checked_add_days(1).unwrap()));
    let late = world.store.void_window(row.id);
    assert!(late.is_err());
}

#[test]
fn probes_follow_the_match_set_rules() {
    let world = world();
    let start = start_day();
    let row = world
        .store
        .register_window(&registration(&world.baseline_key, start))
        .unwrap();
    world.clock.set(noon(start));
    probe(
        &world.store,
        other(),
        start,
        ProbeClass::Deterministic,
        ProbeVerdict::Pass,
        false,
    );
    probe(
        &world.store,
        positive(),
        start,
        ProbeClass::Deterministic,
        ProbeVerdict::Pass,
        false,
    );
    world.clock.set(row.end_exclusive_ms);
    let late = world.store.insert_probe_result(&ProbeResult {
        probe_id: "det".into(),
        probe_version: "1".into(),
        arm: negative(),
        at_ms: noon(start),
        class: ProbeClass::Deterministic,
        verdict: ProbeVerdict::Fail,
        hard_failure: false,
        evidence_summary: String::new(),
    });
    assert!(matches!(
        late,
        Err(StoreError::Conflict(
            "probe write matches an ended observation window"
        ))
    ));
    let outside = world.store.insert_probe_result(&ProbeResult {
        probe_id: "det".into(),
        probe_version: "1".into(),
        arm: positive(),
        at_ms: noon(start.checked_add_days(3).unwrap()),
        class: ProbeClass::Deterministic,
        verdict: ProbeVerdict::Pass,
        hard_failure: false,
        evidence_summary: String::new(),
    });
    assert!(outside.is_ok(), "{outside:?}");
}

#[test]
fn two_open_windows_reject_a_probe_on_the_shared_arm_day() {
    let world = world();
    let start = start_day();
    let first = world
        .store
        .register_window(&registration(&world.baseline_key, start))
        .unwrap();
    world
        .store
        .raw_connection()
        .execute_batch("DROP TRIGGER temp.obs_clock_window_insert")
        .unwrap();
    let overlap = start.checked_add_days(1).unwrap();
    world.clock.set(noon(start));
    stamp(&world.store, noon(start));
    let conn = world.store.raw_connection();
    conn.execute(
        "INSERT INTO observation_window_intents (pos_arm, neg_arm, start, consumed, created_at)
         SELECT pos_arm, neg_arm, ?1, 0, ?2 FROM observation_windows WHERE id = ?3",
        rusqlite::params![overlap.to_string(), noon(start), first.id],
    )
    .unwrap();
    let intent_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO observation_windows (
            intent_id, pos_arm, neg_arm, start, end_date, end_exclusive,
            baseline_key, baseline_content_sha256, det_pass_min, det_fail_max,
            fp_pass_min, fp_fail_max, tau_js, det_probe_id, det_probe_version,
            fp_probe_id, fp_probe_version, cap_probe_id, cap_probe_version,
            observation_run_days, created_at
         )
         SELECT ?1, pos_arm, neg_arm, ?2, ?3, ?4,
                baseline_key, baseline_content_sha256, det_pass_min, det_fail_max,
                fp_pass_min, fp_fail_max, tau_js, det_probe_id, det_probe_version,
                fp_probe_id, fp_probe_version, cap_probe_id, cap_probe_version,
                observation_run_days, ?5
         FROM observation_windows WHERE id = ?6",
        rusqlite::params![
            intent_id,
            overlap.to_string(),
            overlap.checked_add_days(1).unwrap().to_string(),
            overlap
                .checked_add_days(2)
                .unwrap()
                .beijing_midnight_unix_ms()
                .unwrap(),
            noon(start),
            first.id,
        ],
    )
    .unwrap();
    conn.execute_batch("COMMIT").unwrap();
    world.clock.set(noon(overlap));
    let err = world
        .store
        .insert_probe_result(&ProbeResult {
            probe_id: "det".into(),
            probe_version: "1".into(),
            arm: positive(),
            at_ms: noon(overlap),
            class: ProbeClass::Deterministic,
            verdict: ProbeVerdict::Pass,
            hard_failure: false,
            evidence_summary: String::new(),
        })
        .unwrap_err();
    assert!(matches!(
        err,
        StoreError::Conflict("probe write matches more than one open observation window")
    ));
}

#[test]
fn pollution_negative_hard_and_missing_capability_fail_the_window() {
    let world = world();
    let start = start_day();
    let row = world
        .store
        .register_window(&registration(&world.baseline_key, start))
        .unwrap();
    let fresh = baseline_write(noon(start), "blue");
    assert_eq!(
        world.store.upsert_fingerprint_baseline(&fresh).unwrap(),
        FingerprintBaselineUpsert::Updated
    );
    world.clock.set(noon(start));
    probe(
        &world.store,
        positive(),
        start,
        ProbeClass::Fingerprint,
        ProbeVerdict::Pass,
        false,
    );
    world.clock.set(row.end_exclusive_ms);
    let report = world.store.audit_window(row.id).unwrap();
    assert!(!report.window_ok);
    assert!(report.report.contains("polluted=1"));
    assert!(!world.store.audit_ready().unwrap());

    let hard = world_with_rows(|store, start| {
        probe(
            store,
            negative(),
            start,
            ProbeClass::Fingerprint,
            ProbeVerdict::Fail,
            true,
        );
    });
    let hard_report = hard.store.audit_window(hard.window_id).unwrap();
    assert!(!hard_report.window_ok);
    assert!(hard_report.report.contains("hard=1"));

    let missing = world_with_rows(|store, start| {
        probe(
            store,
            positive(),
            start,
            ProbeClass::Deterministic,
            ProbeVerdict::Pass,
            false,
        );
        probe(
            store,
            positive(),
            start,
            ProbeClass::Fingerprint,
            ProbeVerdict::Pass,
            false,
        );
    });
    let missing_report = missing.store.audit_window(missing.window_id).unwrap();
    assert!(!missing_report.window_ok);
    assert!(missing_report.report.contains("cap=-"));
}

struct Audited {
    store: Store,
    window_id: i64,
}

fn world_with_rows(fill: impl Fn(&Store, RunDay)) -> Audited {
    let world = world();
    let start = start_day();
    let row = world
        .store
        .register_window(&registration(&world.baseline_key, start))
        .unwrap();
    world.clock.set(noon(start));
    fill(&world.store, start);
    world.clock.set(row.end_exclusive_ms);
    Audited {
        store: world.store,
        window_id: row.id,
    }
}

#[test]
fn open_audit_conflicts_and_ready_reads_only_final_snapshots() {
    let world = world();
    let start = start_day();
    let row = world
        .store
        .register_window(&registration(&world.baseline_key, start))
        .unwrap();
    for day in [start, start.checked_add_days(1).unwrap()] {
        world.clock.set(noon(day));
        probe(
            &world.store,
            positive(),
            day,
            ProbeClass::Deterministic,
            ProbeVerdict::Pass,
            false,
        );
        probe(
            &world.store,
            positive(),
            day,
            ProbeClass::Capability,
            ProbeVerdict::NotApplicable,
            false,
        );
        probe(
            &world.store,
            positive(),
            day,
            ProbeClass::Fingerprint,
            ProbeVerdict::Pass,
            false,
        );
        probe(
            &world.store,
            negative(),
            day,
            ProbeClass::Fingerprint,
            ProbeVerdict::Fail,
            false,
        );
    }
    let open = world.store.audit_window(row.id).unwrap_err();
    assert!(matches!(
        open,
        StoreError::Conflict("observation window is still open")
    ));
    assert!(!world.store.audit_ready().unwrap());
    let stored = world.store.list_probe_results().unwrap();
    let pos = DaySummary::build(
        &positive(),
        &stored
            .iter()
            .filter(|row| row.arm == positive())
            .cloned()
            .collect::<Vec<_>>(),
        &[],
    )
    .unwrap();
    let neg = DaySummary::build(
        &negative(),
        &stored
            .iter()
            .filter(|row| row.arm == negative())
            .cloned()
            .collect::<Vec<_>>(),
        &[],
    )
    .unwrap();
    assert_eq!(observation(&pos, &neg, true, true), Observation::Validated);
    assert!(!world.store.audit_ready().unwrap());

    world.clock.set(row.end_exclusive_ms);
    let report = world.store.audit_window(row.id).unwrap();
    assert!(report.window_ok, "{}", report.report);
    assert!(report.report.contains("cap=not-applicable"));
    assert!(world.store.audit_ready().unwrap());
    let again = world.store.audit_window(row.id).unwrap();
    assert_eq!(again, report);
    assert_eq!(
        world.store.audit_report(row.id).unwrap().report,
        report.report
    );
    let missing = world.store.audit_report(row.id + 9).unwrap_err();
    assert!(matches!(
        missing,
        StoreError::Conflict("observation window has no final snapshot")
    ));
}

#[test]
fn wrong_baseline_key_fails_the_day() {
    let world = world();
    let start = start_day();
    let row = world
        .store
        .register_window(&registration(&world.baseline_key, start))
        .unwrap();
    world.clock.set(noon(start));
    stamp(&world.store, noon(start));
    let conn = world.store.raw_connection();
    let arm_id: i64 = conn
        .query_row(
            "SELECT pos_arm FROM observation_windows WHERE id = ?1",
            [row.id],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO probe_results (
            probe_id, probe_version, arm_id, run_day, at_ms, class, verdict, hard_failure,
            evidence_summary, baseline_key, baseline_content_sha256
         ) VALUES ('fp', '1', ?1, ?2, ?3, 'fingerprint', 'pass', 0, '', ?4, ?5)",
        rusqlite::params![
            arm_id,
            start.to_string(),
            noon(start),
            "ab".repeat(32),
            row.baseline_content_sha256,
        ],
    )
    .unwrap();
    conn.execute_batch("COMMIT").unwrap();
    world.clock.set(row.end_exclusive_ms);
    let report = world.store.audit_window(row.id).unwrap();
    assert!(!report.window_ok);
    assert!(report.report.contains("polluted=1"));
}

#[test]
fn controls_are_append_only_and_paired_with_events() {
    let (store, _) = open_at(50);
    let set = store
        .set_control(ControlRole::Positive, &positive())
        .unwrap();
    assert_eq!(set.action, ControlAction::Set);
    assert_eq!(set.arm.as_ref(), Some(&positive()));
    let err = store.set_control(ControlRole::Positive, &negative());
    assert!(matches!(
        err,
        Err(StoreError::Conflict(
            "clear the control before setting another arm"
        ))
    ));
    let cleared = store.clear_control(ControlRole::Positive).unwrap();
    assert_eq!(cleared.action, ControlAction::Clear);
    assert!(cleared.arm.is_none());
    let events: i64 = store
        .raw_connection()
        .query_row("SELECT COUNT(*) FROM observation_control_events", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(events, 2);
    assert_eq!(store.list_observation_controls().unwrap().len(), 2);
    let updated = store
        .raw_connection()
        .execute("UPDATE observation_controls SET action = 'clear'", []);
    assert!(updated.is_err());
    let removed = store
        .raw_connection()
        .execute("DELETE FROM observation_control_events", []);
    assert!(removed.is_err());
}

#[test]
fn concurrent_consume_of_one_intent_changes_one_row() {
    let dir = std::env::temp_dir().join(format!("agent-value-obs-consume-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("obs.sqlite");
    let store = Store::open(&db, key()).unwrap();
    let pos = store.upsert_arm(&positive()).unwrap();
    let neg = store.upsert_arm(&negative()).unwrap();
    stamp(&store, 10);
    store
        .raw_connection()
        .execute(
            "INSERT INTO observation_window_intents
                (pos_arm, neg_arm, start, consumed, created_at)
             VALUES (?1, ?2, '2026-11-02', 0, 10)",
            [pos.0, neg.0],
        )
        .unwrap();
    let intent_id = store.raw_connection().last_insert_rowid();
    store.raw_connection().execute_batch("COMMIT").unwrap();
    drop(store);

    let a = rusqlite::Connection::open(&db).unwrap();
    a.busy_timeout(Duration::from_secs(5)).unwrap();
    a.execute_batch("BEGIN IMMEDIATE").unwrap();
    let changed = a
        .execute(
            "UPDATE observation_window_intents SET consumed = 1
             WHERE id = ?1 AND consumed = 0",
            [intent_id],
        )
        .unwrap();
    assert_eq!(changed, 1);
    let db_for_b: PathBuf = db.clone();
    let handle = thread::spawn(move || {
        let b = rusqlite::Connection::open(&db_for_b).unwrap();
        b.busy_timeout(Duration::from_secs(5)).unwrap();
        b.execute_batch("BEGIN IMMEDIATE").unwrap();
        let changed = b
            .execute(
                "UPDATE observation_window_intents SET consumed = 1
                 WHERE id = ?1 AND consumed = 0",
                [intent_id],
            )
            .unwrap();
        b.execute_batch("ROLLBACK").unwrap();
        changed
    });
    thread::sleep(Duration::from_millis(200));
    a.execute_batch("COMMIT").unwrap();
    assert_eq!(handle.join().unwrap(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

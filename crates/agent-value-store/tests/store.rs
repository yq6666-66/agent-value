//! HANDOFF step 2 acceptance tests for the SQLite data model (v22 §8, §0, §A).

use std::path::PathBuf;

use agent_value_core::arm::{Arm, ModelId, Protocol, ProviderKind, Route};
use agent_value_core::cells::{CellId, CellWeights};
use agent_value_core::privacy::KeyEnvVarName;
use agent_value_core::records::{
    BehaviorEvent, EventKind, MicroYuan, Outcome, ProbeClass, ProbeResult, ProbeVerdict,
    RequestRecord, TaskRecord, Usage,
};
use agent_value_core::scope::{ScopeId, TaskKind};
use agent_value_core::weights::{
    Evidence, FiveLevel, SelfReportAnswer, WeightLedger, WeightSource, Weights,
};
use agent_value_core::window::{FrozenInputs, WindowIndex};
use agent_value_store::{ConfigKind, DigestKey, Store, StoreError, SCHEMA_VERSION};

const DAY_MS: i64 = 86_400_000;
/// 2026-10-19T04:00:00Z (12:00 Beijing time).
const NOW: i64 = 1_792_382_400_000;

fn key() -> DigestKey {
    DigestKey::from_bytes(b"test-only-digest-key-0123456789ab").unwrap()
}

fn mem() -> Store {
    Store::open_in_memory(key()).unwrap()
}

fn temp_db(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("agent-value-store-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("agent-value.sqlite")
}

fn model() -> ModelId {
    ModelId::new("example-model-1").unwrap()
}

fn arm(kind: ProviderKind, host: &str) -> Arm {
    Arm::new(
        model(),
        Route::new(kind, host, Protocol::AnthropicMessages, "example-cli").unwrap(),
    )
}

fn official() -> Arm {
    arm(ProviderKind::Official, "api.vendor.example")
}

fn relay() -> Arm {
    arm(ProviderKind::Relay, "relay.example")
}

fn coding() -> ScopeId {
    ScopeId::new(TaskKind::Coding)
}

fn cell(s: &str) -> CellId {
    CellId::user(s).unwrap()
}

fn request(id: &str, task: Option<&str>, arm: Arm, key_var: &str, start: i64) -> RequestRecord {
    RequestRecord {
        id: id.into(),
        task_id: task.map(Into::into),
        arm,
        auxiliary: false,
        returned_model: Some("example-model-1-2026".into()),
        response_id: Some(format!("resp-{id}")),
        key_env_var: Some(KeyEnvVarName::new(key_var).unwrap()),
        start_ms: start,
        first_token_ms: Some(start + 800),
        end_ms: start + 5_000,
        usage: Usage {
            input: 1200,
            output: 300,
            cache_read: 4000,
            cache_write: 0,
        },
        charged: Some(MicroYuan::from_yuan(0.6).unwrap()),
    }
}

fn file_bytes_contain(db: &std::path::Path, needle: &str) -> bool {
    let mut found = false;
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let p = PathBuf::from(format!("{}{suffix}", db.display()));
        if let Ok(bytes) = std::fs::read(&p) {
            if bytes.windows(needle.len()).any(|w| w == needle.as_bytes()) {
                found = true;
            }
        }
    }
    found
}

// ---- same model, two routes => two arms ----

#[test]
fn same_model_on_two_routes_is_two_arms() {
    let s = mem();
    let a = s.upsert_arm(&official()).unwrap();
    let b = s.upsert_arm(&relay()).unwrap();
    assert_ne!(a, b);
    assert_eq!(s.arm_count().unwrap(), 2);
    assert_eq!(s.arm(a).unwrap(), official());
    assert_eq!(s.arm(b).unwrap(), relay());

    s.insert_request(&request("r1", None, official(), "VENDOR_API_KEY", NOW))
        .unwrap();
    s.insert_request(&request("r2", None, relay(), "RELAY_API_KEY", NOW))
        .unwrap();
    // Same route with a different key variable is the same arm (keys are not part of an arm).
    s.insert_request(&request("r3", None, relay(), "RELAY_API_KEY_2", NOW))
        .unwrap();
    assert_eq!(s.arm_count().unwrap(), 2);
    assert_eq!(s.request_count_for_arm(a).unwrap(), 1);
    assert_eq!(s.request_count_for_arm(b).unwrap(), 2);
}

#[test]
fn every_route_field_separates_arms() {
    let s = mem();
    let base = official();
    let variants = [
        arm(ProviderKind::Subscription, "api.vendor.example"),
        Arm::new(
            model(),
            Route::new(
                ProviderKind::Official,
                "api.vendor.example",
                Protocol::OpenAiResponses,
                "example-cli",
            )
            .unwrap(),
        ),
        Arm::new(
            model(),
            Route::new(
                ProviderKind::Official,
                "api.vendor.example",
                Protocol::AnthropicMessages,
                "other-cli",
            )
            .unwrap(),
        ),
        Arm::new(
            ModelId::new("example-model-2").unwrap(),
            base.route().clone(),
        ),
    ];
    let id0 = s.upsert_arm(&base).unwrap();
    for v in &variants {
        assert_ne!(s.upsert_arm(v).unwrap(), id0);
    }
    assert_eq!(s.arm_count().unwrap(), 5);
    assert_eq!(s.upsert_arm(&base).unwrap(), id0);
    assert_eq!(s.arm_count().unwrap(), 5);
}

// ---- probe cell: omega is always 0 ----

#[test]
fn probe_cell_weight_is_stored_as_zero_and_cannot_be_nonzero() {
    let mut s = mem();
    let w = CellWeights::new(vec![
        (cell("fix"), 0.6),
        (cell("add"), 0.4),
        (CellId::Probe, 0.0),
    ])
    .unwrap();
    s.set_cell_weights(&coding(), WindowIndex(0), &w).unwrap();
    let back = s.cell_weights(&coding(), WindowIndex(0)).unwrap().unwrap();
    assert_eq!(back, w);
    assert_eq!(back.omega(&CellId::Probe), 0.0);
    let stored: f64 = s
        .raw_connection()
        .query_row(
            "SELECT omega FROM cell_weights WHERE cell = '__probe__'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, 0.0);

    // SQL-level guard: even a raw insert cannot give the probe cell weight.
    let err = s.raw_connection().execute(
        "INSERT INTO cell_weights (scope, window_index, cell, omega) VALUES ('coding', 1, '__probe__', 0.1)",
        [],
    );
    assert!(err.is_err());
    // Frozen cell weights cannot be updated or re-set.
    assert!(s
        .raw_connection()
        .execute("UPDATE cell_weights SET omega = 0.5 WHERE cell = 'fix'", [])
        .is_err());
    assert!(matches!(
        s.set_cell_weights(&coding(), WindowIndex(0), &w),
        Err(StoreError::Conflict(_))
    ));
}

#[test]
fn probe_tasks_never_count_as_user_tasks() {
    let s = mem();
    s.insert_task(&TaskRecord::new("t1", coding(), cell("fix"), relay(), NOW))
        .unwrap();
    s.insert_task(&TaskRecord::new(
        "t2",
        coding(),
        cell("fix"),
        relay(),
        NOW + 1,
    ))
    .unwrap();
    s.insert_task(&TaskRecord::new(
        "t3",
        coding(),
        cell("add"),
        official(),
        NOW + 2,
    ))
    .unwrap();
    for i in 0..5 {
        s.insert_task(&TaskRecord::new(
            &format!("p{i}"),
            coding(),
            CellId::Probe,
            relay(),
            NOW + i,
        ))
        .unwrap();
    }
    let counts = s.user_task_counts(&coding(), NOW, NOW + DAY_MS).unwrap();
    assert_eq!(counts, vec![(cell("add"), 1), (cell("fix"), 2)]);
    assert_eq!(
        s.user_task_count_before(&coding(), NOW + DAY_MS).unwrap(),
        3
    );

    // A scope with only probe tasks still has no user history: first window.
    let nc = ScopeId::new(TaskKind::NonCoding);
    s.insert_task(&TaskRecord::new(
        "p-nc",
        nc.clone(),
        CellId::Probe,
        relay(),
        NOW,
    ))
    .unwrap();
    assert_eq!(s.user_task_count_before(&nc, NOW + DAY_MS).unwrap(), 0);

    // The probe flag and the probe cell cannot disagree, even via raw SQL.
    let arm_id = s.upsert_arm(&relay()).unwrap().0;
    for (cell, is_probe) in [("__probe__", 0), ("fix", 1)] {
        let r = s.raw_connection().execute(
            "INSERT INTO tasks (id, scope, cell, is_probe, arm_id, submitted_ms) VALUES ('x', 'coding', ?1, ?2, ?3, 0)",
            rusqlite::params![cell, is_probe, arm_id],
        );
        assert!(r.is_err(), "{cell} {is_probe}");
    }
}

// ---- no key values are stored ----

#[test]
fn key_values_and_prompt_text_are_never_stored() {
    // A fake key value that a real route would read from RELAY_API_KEY.
    // Split so secret scanners do not flag this fake value in the source.
    let fake_key_value = concat!("sk-", "test-FAKE-0123456789abcdefFAKE");
    let prompt = "Please refactor the payment module; secret marker QX7-PROMPT-TEXT";
    let db = temp_db("no-keys");
    {
        let mut s = Store::open(&db, key()).unwrap();
        // The env var *name* is stored; trying to store the value as a name is refused.
        assert!(KeyEnvVarName::new(fake_key_value).is_err());

        let mut t = TaskRecord::new("t1", coding(), cell("refactor"), relay(), NOW);
        t.first_message = Some(s.text_facts("first-message", prompt).unwrap());
        t.outcome = Outcome::Success;
        t.rating = Some(4);
        s.insert_task(&t).unwrap();
        s.insert_request(&request("r1", Some("t1"), relay(), "RELAY_API_KEY", NOW))
            .unwrap();
        s.insert_behavior_event(&BehaviorEvent {
            id: "e1".into(),
            at_ms: NOW,
            kind: EventKind::ManualSwitch,
            scope: coding(),
            cell: cell("refactor"),
            chosen: relay(),
            available: vec![official(), relay()],
        })
        .unwrap();
        s.insert_probe_result(&ProbeResult {
            probe_id: "deterministic-set".into(),
            probe_version: "1".into(),
            arm: relay(),
            at_ms: NOW,
            class: ProbeClass::Deterministic,
            verdict: ProbeVerdict::Pass,
            hard_failure: false,
            evidence_summary: "10/10 correct".into(),
        })
        .unwrap();
        // Secrets are refused in free-text fields too.
        let mut bad = ProbeResult {
            probe_id: "deterministic-set".into(),
            probe_version: "1".into(),
            arm: relay(),
            at_ms: NOW + DAY_MS,
            class: ProbeClass::Deterministic,
            verdict: ProbeVerdict::Pass,
            hard_failure: false,
            evidence_summary: format!("echo {fake_key_value}"),
        };
        assert!(s.insert_probe_result(&bad).is_err());
        bad.evidence_summary = "ok".into();
        assert!(s
            .put_config_version(
                ConfigKind::PriceTable,
                None,
                "p1",
                &format!("key={fake_key_value}"),
                NOW
            )
            .is_err());
        s.put_config_version(
            ConfigKind::PriceTable,
            None,
            "p1",
            "{\"input\": 0.003}",
            NOW,
        )
        .unwrap();
        s.checkpoint().unwrap();

        // The stored digest is the keyed digest, the length is kept, the text is not.
        let (len, digest): (u32, String) = s
            .raw_connection()
            .query_row(
                "SELECT first_msg_chars, first_msg_digest FROM tasks",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(len as usize, prompt.chars().count());
        assert_eq!(digest, key().digest_hex(prompt));
        let other = DigestKey::from_bytes(b"another-install-key-000000000000").unwrap();
        assert_ne!(digest, other.digest_hex(prompt));

        let dump = format!("{:?}", s.export().unwrap());
        assert!(dump.contains("RELAY_API_KEY"));
        assert!(!dump.contains(fake_key_value));
        assert!(!dump.contains("QX7-PROMPT-TEXT"));
    }
    assert!(
        file_bytes_contain(&db, "RELAY_API_KEY"),
        "sanity: the scan sees stored text"
    );
    assert!(!file_bytes_contain(&db, fake_key_value));
    assert!(!file_bytes_contain(&db, "QX7-PROMPT-TEXT"));
    assert!(
        !file_bytes_contain(&db, "test-only-digest-key"),
        "digest key must not be stored"
    );
    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}

#[test]
fn schema_has_no_column_for_secrets_or_text() {
    let s = mem();
    let mut st = s
        .raw_connection()
        .prepare("SELECT m.name, p.name FROM sqlite_master m, pragma_table_info(m.name) p WHERE m.type = 'table'")
        .unwrap();
    let cols: Vec<(String, String)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(cols.len() > 50);
    for (t, c) in cols {
        let c = c.to_lowercase();
        let forbidden = [
            "secret",
            "token_value",
            "api_key",
            "password",
            "prompt",
            "content",
            "body",
            "text",
        ];
        // Reviewed exceptions: an env var NAME, and a hash of config content.
        let allowed = ["key_env_var", "content_sha256"];
        assert!(
            !forbidden.iter().any(|f| c.contains(f)) || allowed.contains(&c.as_str()),
            "{t}.{c} looks like it could hold a secret or raw text"
        );
    }
}

// ---- behaviour events, probes ----

#[test]
fn event_snapshot_is_stored_not_recomputed() {
    let mut s = mem();
    let ev = BehaviorEvent {
        id: "e1".into(),
        at_ms: NOW,
        kind: EventKind::RerouteAfterFailure,
        scope: coding(),
        cell: cell("fix"),
        chosen: official(),
        available: vec![relay(), official()],
    };
    s.insert_behavior_event(&ev).unwrap();
    let mut want = vec![official(), relay()];
    want.sort();
    assert_eq!(s.event_available_arms("e1").unwrap(), want);
    // Adding more arms later does not change the stored snapshot.
    s.upsert_arm(&arm(ProviderKind::Subscription, "sub.example"))
        .unwrap();
    assert_eq!(s.event_available_arms("e1").unwrap().len(), 2);
    // An invalid event writes nothing (atomic).
    let bad = BehaviorEvent {
        id: "e2".into(),
        chosen: arm(ProviderKind::Relay, "other.example"),
        ..ev
    };
    assert!(s.insert_behavior_event(&bad).is_err());
    assert!(s.event_available_arms("e2").unwrap().is_empty());
}

#[test]
fn probe_results_use_beijing_run_days_one_per_day() {
    let s = mem();
    let base = ProbeResult {
        probe_id: "deterministic-set".into(),
        probe_version: "1".into(),
        arm: relay(),
        // 2026-10-19T15:30:00Z = 23:30 Beijing, still 10-19.
        at_ms: 1_792_423_800_000,
        class: ProbeClass::Deterministic,
        verdict: ProbeVerdict::Pass,
        hard_failure: false,
        evidence_summary: "9/10".into(),
    };
    s.insert_probe_result(&base).unwrap();
    // 40 minutes later is 00:10 on 10-20 in Beijing: a new run day.
    let next = ProbeResult {
        at_ms: base.at_ms + 40 * 60_000,
        ..base.clone()
    };
    s.insert_probe_result(&next).unwrap();
    // A second result on the same run day for the same probe set is refused.
    let dup = ProbeResult {
        at_ms: base.at_ms + 60_000,
        ..base.clone()
    };
    assert!(matches!(
        s.insert_probe_result(&dup),
        Err(StoreError::Sqlite(_))
    ));
    let id = s.upsert_arm(&relay()).unwrap();
    assert_eq!(
        s.probe_run_days(id).unwrap(),
        vec!["2026-10-19", "2026-10-20"]
    );
}

// ---- versions ----

fn answers() -> Evidence {
    Evidence::SelfReportAnswers(
        [
            FiveLevel::SlightlyA,
            FiveLevel::AboutSame,
            FiveLevel::SlightlyA,
            FiveLevel::StronglyB,
        ]
        .iter()
        .enumerate()
        .map(|(i, l)| SelfReportAnswer {
            question_id: format!("q{i}"),
            answer: *l,
        })
        .collect(),
    )
}

#[test]
fn weight_ledger_round_trips_and_stays_immutable() {
    let db = temp_db("ledger");
    let mut l = WeightLedger::new(coding());
    l.schedule_official(
        Weights::new(0.2, 0.35, 0.45).unwrap(),
        WeightSource::SelfReport,
        answers(),
        WindowIndex(0),
    )
    .unwrap();
    l.freeze_window(
        WindowIndex(0),
        0,
        FrozenInputs {
            price_table_version: Some("p1".into()),
            ..FrozenInputs::default()
        },
    )
    .unwrap();
    let trial = l
        .create_trial(Weights::new(0.6, 0.2, 0.2).unwrap(), WindowIndex(0))
        .unwrap();
    {
        let mut s = Store::open(&db, key()).unwrap();
        s.save_weight_ledger(&l).unwrap();
        // Confirming the trial later only adds rows.
        l.confirm_trial(trial).unwrap();
        l.freeze_window(WindowIndex(1), 12, FrozenInputs::default())
            .unwrap();
        s.save_weight_ledger(&l).unwrap();
        s.save_weight_ledger(&l).unwrap(); // idempotent
    }
    let s = Store::open(&db, key()).unwrap();
    let back = s.load_weight_ledger(&coding()).unwrap().unwrap();
    assert_eq!(back.versions(), l.versions());
    assert_eq!(back.frozen_windows(), l.frozen_windows());
    assert_eq!(
        back.frozen(WindowIndex(1)).unwrap().weights(),
        Weights::new(0.6, 0.2, 0.2).unwrap()
    );
    assert!(back.frozen(WindowIndex(0)).unwrap().is_first_window());
    assert!(s
        .load_weight_ledger(&ScopeId::new(TaskKind::NonCoding))
        .unwrap()
        .is_none());

    // SQL-level guards on the version graph.
    for sql in [
        "INSERT INTO weight_versions (scope, id, a, b, g, source, status, effective_from, previous) VALUES ('coding', 9, 0.2, 0.3, 0.5, 'manual', 'official', 5, 42)",
        "INSERT INTO weight_versions (scope, id, a, b, g, source, status, effective_from, previous) VALUES ('coding', 9, 0.2, 0.3, 0.5, 'manual', 'official', 5, 9)",
        "INSERT INTO trial_resolutions (scope, version_id, resolution, confirmed_as) VALUES ('coding', 0, 'confirmed', 77)",
    ] {
        assert!(s.raw_connection().execute(sql, []).is_err(), "{sql}");
    }
    for sql in [
        "UPDATE frozen_windows SET weight_version_id = 0",
        "UPDATE weight_versions SET a = 0.5",
        "UPDATE trial_resolutions SET resolution = 'discarded', confirmed_as = NULL",
        "UPDATE weight_evidence SET level = 2",
    ] {
        assert!(s.raw_connection().execute(sql, []).is_err(), "{sql}");
    }
    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}

#[test]
fn stale_or_rewritten_ledgers_are_refused() {
    let mut s = mem();
    let mut l = WeightLedger::new(coding());
    l.freeze_window(WindowIndex(0), 0, FrozenInputs::default())
        .unwrap();
    let stale = l.clone();
    l.freeze_window(WindowIndex(1), 3, FrozenInputs::default())
        .unwrap();
    s.save_weight_ledger(&l).unwrap();
    assert!(matches!(
        s.save_weight_ledger(&stale),
        Err(StoreError::Conflict(_))
    ));

    // A different history for an already stored window is refused.
    let mut other = WeightLedger::new(coding());
    other
        .schedule_official(
            Weights::new(0.2, 0.35, 0.45).unwrap(),
            WeightSource::SelfReport,
            answers(),
            WindowIndex(0),
        )
        .unwrap();
    other
        .freeze_window(WindowIndex(0), 0, FrozenInputs::default())
        .unwrap();
    other
        .freeze_window(WindowIndex(1), 3, FrozenInputs::default())
        .unwrap();
    assert!(matches!(
        s.save_weight_ledger(&other),
        Err(StoreError::Conflict(_))
    ));
}

#[test]
fn config_versions_are_immutable() {
    let s = mem();
    s.put_config_version(
        ConfigKind::ClassificationRules,
        Some(&coding()),
        "rules-1",
        "{}",
        NOW,
    )
    .unwrap();
    let v = s
        .config_version(ConfigKind::ClassificationRules, Some(&coding()), "rules-1")
        .unwrap()
        .unwrap();
    assert_eq!(v.content_sha256, agent_value_store::sha256_hex(b"{}"));
    assert!(s
        .put_config_version(
            ConfigKind::ClassificationRules,
            Some(&coding()),
            "rules-1",
            "{\"x\":1}",
            NOW
        )
        .is_err());
    assert!(s
        .raw_connection()
        .execute("UPDATE config_versions SET payload = 'x'", [])
        .is_err());
    assert!(s
        .config_version(ConfigKind::PriceTable, None, "rules-1")
        .unwrap()
        .is_none());
    assert!(s
        .put_config_version(ConfigKind::PriceTable, None, " ", "{}", NOW)
        .is_err());
}

// ---- retention, export, delete ----

#[test]
fn purge_removes_data_older_than_180_days_and_keeps_versions() {
    let mut s = mem();
    let old = NOW - 181 * DAY_MS;
    let recent = NOW - 179 * DAY_MS;
    s.insert_task(&TaskRecord::new("old", coding(), cell("fix"), relay(), old))
        .unwrap();
    s.insert_task(&TaskRecord::new(
        "new",
        coding(),
        cell("fix"),
        relay(),
        recent,
    ))
    .unwrap();
    s.insert_request(&request(
        "r-old",
        Some("old"),
        relay(),
        "RELAY_API_KEY",
        old,
    ))
    .unwrap();
    s.insert_request(&request(
        "r-new",
        Some("new"),
        relay(),
        "RELAY_API_KEY",
        recent,
    ))
    .unwrap();
    s.insert_request(&request("r-orphan", None, relay(), "RELAY_API_KEY", old))
        .unwrap();
    s.insert_behavior_event(&BehaviorEvent {
        id: "e-old".into(),
        at_ms: old,
        kind: EventKind::Redo,
        scope: coding(),
        cell: cell("fix"),
        chosen: relay(),
        available: vec![relay(), official()],
    })
    .unwrap();
    s.insert_probe_result(&ProbeResult {
        probe_id: "d".into(),
        probe_version: "1".into(),
        arm: relay(),
        at_ms: old,
        class: ProbeClass::Capability,
        verdict: ProbeVerdict::NotApplicable,
        hard_failure: false,
        evidence_summary: String::new(),
    })
    .unwrap();
    let mut l = WeightLedger::new(coding());
    l.freeze_window(WindowIndex(0), 0, FrozenInputs::default())
        .unwrap();
    s.save_weight_ledger(&l).unwrap();

    let c = s.purge_older_than(NOW).unwrap();
    assert_eq!(
        (c.requests, c.tasks, c.behavior_events, c.probe_results),
        (2, 1, 1, 1)
    );
    assert_eq!(s.purge_older_than(NOW).unwrap(), Default::default());
    assert_eq!(s.user_task_count_before(&coding(), NOW).unwrap(), 1);
    assert!(s.event_available_arms("e-old").unwrap().is_empty());
    assert!(s.load_weight_ledger(&coding()).unwrap().is_some());
}

#[test]
fn delete_all_wipes_rows_and_bytes() {
    let db = temp_db("delete-all");
    let marker = "MARKER-EVIDENCE-7781";
    {
        let mut s = Store::open(&db, key()).unwrap();
        s.insert_probe_result(&ProbeResult {
            probe_id: "d".into(),
            probe_version: "1".into(),
            arm: relay(),
            at_ms: NOW,
            class: ProbeClass::Fingerprint,
            verdict: ProbeVerdict::Uncertain,
            hard_failure: false,
            evidence_summary: marker.into(),
        })
        .unwrap();
        s.insert_task(&TaskRecord::new("t", coding(), cell("fix"), relay(), NOW))
            .unwrap();
        let mut l = WeightLedger::new(coding());
        l.freeze_window(WindowIndex(0), 0, FrozenInputs::default())
            .unwrap();
        s.save_weight_ledger(&l).unwrap();
        s.checkpoint().unwrap();
        assert!(file_bytes_contain(&db, marker));
        s.delete_all().unwrap();
        let dump = s.export().unwrap();
        assert_eq!(dump.schema_version, SCHEMA_VERSION);
        assert!(dump.tables.iter().all(|t| t.rows.is_empty()));
        assert_eq!(s.arm_count().unwrap(), 0);
    }
    assert!(!file_bytes_contain(&db, marker));
    // The emptied database still opens and works.
    let s = Store::open(&db, key()).unwrap();
    s.insert_task(&TaskRecord::new("t", coding(), cell("fix"), relay(), NOW))
        .unwrap();
    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}

#[test]
fn export_is_deterministic() {
    let s = mem();
    s.insert_task(&TaskRecord::new("t1", coding(), cell("fix"), relay(), NOW))
        .unwrap();
    s.insert_request(&request("r1", Some("t1"), relay(), "RELAY_API_KEY", NOW))
        .unwrap();
    let a = s.export().unwrap();
    let b = s.export().unwrap();
    assert_eq!(a, b);
    let tasks = a.tables.iter().find(|t| t.name == "tasks").unwrap();
    assert_eq!(tasks.rows.len(), 1);
    let req = a.tables.iter().find(|t| t.name == "requests").unwrap();
    let col = req
        .columns
        .iter()
        .position(|c| c == "charged_micro_yuan")
        .unwrap();
    assert_eq!(req.rows[0][col].as_deref(), Some("600000"));
}

// ---- schema versioning ----

#[test]
fn migrations_are_versioned_and_newer_schemas_refused() {
    let db = temp_db("migrate");
    {
        let s = Store::open(&db, key()).unwrap();
        assert_eq!(s.schema_version().unwrap(), SCHEMA_VERSION);
    }
    {
        // Reopening does not re-run migrations.
        let s = Store::open(&db, key()).unwrap();
        assert_eq!(s.schema_version().unwrap(), SCHEMA_VERSION);
        s.raw_connection()
            .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
    }
    assert!(matches!(
        Store::open(&db, key()),
        Err(StoreError::SchemaTooNew { .. })
    ));
    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}

#[test]
fn invalid_records_are_errors_not_panics() {
    let s = mem();
    let mut t = TaskRecord::new("t", coding(), cell("fix"), relay(), NOW);
    t.rating = Some(9);
    assert!(matches!(s.insert_task(&t), Err(StoreError::Core(_))));
    let mut r = request("r", None, relay(), "RELAY_API_KEY", NOW);
    r.end_ms = NOW - 1;
    assert!(matches!(s.insert_request(&r), Err(StoreError::Core(_))));
    let mut r = request("r", None, relay(), "RELAY_API_KEY", NOW);
    r.usage.input = u64::MAX;
    assert!(s.insert_request(&r).is_err());
    let mut r = request("r", Some("missing-task"), relay(), "RELAY_API_KEY", NOW);
    assert!(matches!(s.insert_request(&r), Err(StoreError::Sqlite(_))));
    r.task_id = None;
    assert!(s.insert_request(&r).is_ok());
    assert!(DigestKey::from_bytes(b"short").is_err());
}

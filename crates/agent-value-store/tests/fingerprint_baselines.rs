//! Fingerprint baseline store: round-trip, expiry, stale upsert, migration.
//! No network and no probe scoring.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use agent_value_core::CoreError;
use agent_value_store::{
    sha256_hex, AnswerCount, DigestKey, FingerprintBaselineIdentity, FingerprintBaselineUpsert,
    FingerprintBaselineWrite, FingerprintDistribution, FixedClock, QuestionAnswers, Store,
    StoreError, DISTRIBUTION_SCHEMA_VERSION, FINGERPRINT_BASELINE_TTL_MS, SCHEMA_VERSION,
};

const COLLECTED: i64 = 1_700_000_000_000;
const QUESTIONS: usize = 10;

fn digest_key() -> DigestKey {
    DigestKey::from_bytes(b"test-only-digest-key-aaaaaaaaaaaa").unwrap()
}

fn prompt_hash() -> String {
    sha256_hex(b"PROMPT-MARKER-NOT-STORED")
}

fn question(answer: &str, count: u32) -> QuestionAnswers {
    QuestionAnswers {
        answers: vec![AnswerCount {
            answer: answer.into(),
            count,
        }],
    }
}

fn distribution(answer: &str) -> FingerprintDistribution {
    FingerprintDistribution {
        run1: vec![question(answer, 20); QUESTIONS],
        run2: vec![question(answer, 20); QUESTIONS],
    }
}

fn identity(sampling: &str) -> FingerprintBaselineIdentity {
    FingerprintBaselineIdentity {
        provider: "example-provider".into(),
        model_id: "example-model".into(),
        model_revision: String::new(),
        protocol: "anthropic-messages".into(),
        connection_kind: "official-direct".into(),
        task_set_version: "probe-set-1".into(),
        system_prompt_hash: prompt_hash(),
        sampling_config: sampling.into(),
        distribution_schema_version: DISTRIBUTION_SCHEMA_VERSION,
    }
}

fn write_at(collected_at_ms: i64, answer: &str) -> FingerprintBaselineWrite {
    FingerprintBaselineWrite {
        identity: identity("temperature=1\ntop_p=1\nmax_tokens=64"),
        collected_at_ms,
        distribution: distribution(answer),
    }
}

fn open_at(now_ms: i64) -> (Store, FixedClock) {
    let clock = FixedClock::new(now_ms);
    let store = Store::open_in_memory_with_clock(digest_key(), clock.clone()).unwrap();
    (store, clock)
}

fn temp_db(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "agent-value-fp-baseline-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("agent-value.sqlite")
}

fn file_contains(db: &Path, needle: &str) -> bool {
    for suffix in ["", "-wal", "-shm"] {
        let path = PathBuf::from(format!("{}{suffix}", db.display()));
        if let Ok(bytes) = std::fs::read(path) {
            if bytes.windows(needle.len()).any(|w| w == needle.as_bytes()) {
                return true;
            }
        }
    }
    false
}

fn master_sql(store: &Store) -> BTreeMap<String, String> {
    let mut st = store
        .raw_connection()
        .prepare("SELECT name, sql FROM sqlite_master WHERE sql IS NOT NULL")
        .unwrap();
    st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn fingerprint_baseline_round_trip() {
    assert_eq!(SCHEMA_VERSION, 4);
    assert_eq!(DISTRIBUTION_SCHEMA_VERSION, 1);
    let (store, _) = open_at(COLLECTED);
    let write = write_at(COLLECTED, "marker-answer");
    assert_eq!(
        store.upsert_fingerprint_baseline(&write).unwrap(),
        FingerprintBaselineUpsert::Inserted
    );
    assert_eq!(
        store.upsert_fingerprint_baseline(&write).unwrap(),
        FingerprintBaselineUpsert::Updated
    );
    let got = store
        .get_fingerprint_baseline(&write.identity)
        .unwrap()
        .expect("fresh");
    assert_eq!(got.identity, write.identity);
    assert_eq!(got.distribution, write.distribution);
    assert_eq!(got.collected_at_ms, COLLECTED);
    assert_eq!(got.expires_at_ms, COLLECTED + FINGERPRINT_BASELINE_TTL_MS);
    assert_eq!(
        got.sampling_hash,
        sha256_hex(write.identity.sampling_config.as_bytes())
    );
    assert_eq!(got.baseline_key.len(), 64);

    let mut other_sampling = write.clone();
    other_sampling.identity.sampling_config = "temperature=0\ntop_p=1\nmax_tokens=64".into();
    assert_eq!(
        store.upsert_fingerprint_baseline(&other_sampling).unwrap(),
        FingerprintBaselineUpsert::Inserted
    );
    let mut other_prompt = write.clone();
    other_prompt.identity.system_prompt_hash = sha256_hex(b"other-prompt");
    assert_eq!(
        store.upsert_fingerprint_baseline(&other_prompt).unwrap(),
        FingerprintBaselineUpsert::Inserted
    );
    let mut revision = write.clone();
    revision.identity.model_revision = "r2".into();
    assert_eq!(
        store.upsert_fingerprint_baseline(&revision).unwrap(),
        FingerprintBaselineUpsert::Inserted
    );
    let listed = store.list_fingerprint_baselines(false).unwrap();
    assert_eq!(listed.len(), 4);
    assert!(listed.iter().all(|row| row.model_id == "example-model"));
    assert!(listed
        .iter()
        .all(|row| row.protocol == "anthropic-messages"));
    let rendered = format!("{listed:?}");
    assert!(!rendered.contains("marker-answer"));
    assert!(!rendered.contains("temperature=1"));
    assert!(format!("{got:?}").contains("marker-answer"));
}

#[test]
fn expiry_boundary_and_list_filter() {
    let expires = COLLECTED + FINGERPRINT_BASELINE_TTL_MS;
    let (store, clock) = open_at(expires - 1);
    let kept = write_at(COLLECTED, "red");
    assert_eq!(
        store.upsert_fingerprint_baseline(&kept).unwrap(),
        FingerprintBaselineUpsert::Inserted
    );
    let mut sibling = write_at(COLLECTED, "blue");
    sibling.identity.model_id = "example-model-2".into();
    sibling.collected_at_ms = expires;
    assert_eq!(
        store.upsert_fingerprint_baseline(&sibling).unwrap(),
        FingerprintBaselineUpsert::Inserted
    );

    assert!(store
        .get_fingerprint_baseline(&kept.identity)
        .unwrap()
        .is_some());
    clock.set(expires);
    assert!(store
        .get_fingerprint_baseline(&kept.identity)
        .unwrap()
        .is_some());
    assert_eq!(store.prune_expired_fingerprint_baselines().unwrap(), 0);
    clock.set(expires + 1);
    assert!(store
        .get_fingerprint_baseline(&kept.identity)
        .unwrap()
        .is_none());
    let hidden = store.list_fingerprint_baselines(false).unwrap();
    assert_eq!(hidden.len(), 1);
    assert_eq!(hidden[0].model_id, "example-model-2");
    let all = store.list_fingerprint_baselines(true).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(
        store
            .raw_connection()
            .query_row("SELECT COUNT(*) FROM fingerprint_baselines", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap(),
        2
    );
    assert_eq!(store.prune_expired_fingerprint_baselines().unwrap(), 1);
    assert_eq!(store.list_fingerprint_baselines(true).unwrap().len(), 1);
    assert!(store
        .get_fingerprint_baseline(&sibling.identity)
        .unwrap()
        .is_some());
}

#[test]
fn older_upsert_does_not_overwrite() {
    let (store, _) = open_at(COLLECTED);
    let mut newer = write_at(COLLECTED + 5_000, "blue");
    assert_eq!(
        store.upsert_fingerprint_baseline(&newer).unwrap(),
        FingerprintBaselineUpsert::Inserted
    );
    let older = write_at(COLLECTED, "red");
    assert_eq!(
        store.upsert_fingerprint_baseline(&older).unwrap(),
        FingerprintBaselineUpsert::StaleIgnored
    );
    let got = store
        .get_fingerprint_baseline(&newer.identity)
        .unwrap()
        .unwrap();
    assert_eq!(got.collected_at_ms, COLLECTED + 5_000);
    assert_eq!(got.distribution.run1[0].answers[0].answer, "blue");

    newer.distribution = distribution("green");
    newer.collected_at_ms = COLLECTED + 5_000;
    assert_eq!(
        store.upsert_fingerprint_baseline(&newer).unwrap(),
        FingerprintBaselineUpsert::Updated
    );
    let got = store
        .get_fingerprint_baseline(&newer.identity)
        .unwrap()
        .unwrap();
    assert_eq!(got.distribution.run1[0].answers[0].answer, "green");
}

#[test]
fn empty_and_invalid_fields_are_refused() {
    let (store, _) = open_at(COLLECTED);
    let mut bad = write_at(COLLECTED, "red");
    bad.identity.provider = "  ".into();
    assert!(matches!(
        store.upsert_fingerprint_baseline(&bad),
        Err(StoreError::Core(CoreError::EmptyField { .. }))
    ));
    for set in [
        |w: &mut FingerprintBaselineWrite| w.identity.model_id.clear(),
        |w: &mut FingerprintBaselineWrite| w.identity.protocol.clear(),
        |w: &mut FingerprintBaselineWrite| w.identity.connection_kind.clear(),
        |w: &mut FingerprintBaselineWrite| w.identity.task_set_version.clear(),
        |w: &mut FingerprintBaselineWrite| w.identity.sampling_config.clear(),
    ] {
        let mut bad = write_at(COLLECTED, "red");
        set(&mut bad);
        assert!(store.upsert_fingerprint_baseline(&bad).is_err());
    }
    let mut bad = write_at(COLLECTED, "red");
    bad.identity.system_prompt_hash = "not-a-hash".into();
    assert!(store.upsert_fingerprint_baseline(&bad).is_err());
    bad.identity.system_prompt_hash = prompt_hash();
    bad.identity.distribution_schema_version = 2;
    assert!(store.upsert_fingerprint_baseline(&bad).is_err());
    bad.identity.distribution_schema_version = DISTRIBUTION_SCHEMA_VERSION;
    bad.collected_at_ms = -1;
    assert!(store.upsert_fingerprint_baseline(&bad).is_err());
    bad.collected_at_ms = COLLECTED;
    bad.distribution.run1.pop();
    assert!(matches!(
        store.upsert_fingerprint_baseline(&bad),
        Err(StoreError::Core(CoreError::LengthMismatch { .. }))
    ));
    let mut bad = write_at(COLLECTED, "red");
    bad.distribution.run2[3].answers[0].count = 0;
    assert!(store.upsert_fingerprint_baseline(&bad).is_err());
    bad.distribution.run2[3].answers[0].count = 20;
    bad.distribution.run2[3].answers[0].answer.clear();
    assert!(store.upsert_fingerprint_baseline(&bad).is_err());
    let mut ok = write_at(COLLECTED, "red");
    ok.identity.model_revision.clear();
    assert!(store.upsert_fingerprint_baseline(&ok).is_ok());
    assert_eq!(
        store
            .raw_connection()
            .query_row("SELECT COUNT(*) FROM fingerprint_baselines", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap(),
        1
    );
}

#[test]
fn sentinel_secret_never_appears_in_db_debug_or_errors() {
    let sentinel = format!("sk-{}", "BASELINESECRET");
    let db = temp_db("secret");
    let clock = FixedClock::new(COLLECTED);
    let store = Store::open_with_clock(&db, digest_key(), clock).unwrap();
    let timeout: i64 = store
        .raw_connection()
        .pragma_query_value(None, "busy_timeout", |r| r.get(0))
        .unwrap();
    assert_eq!(timeout, 5_000);

    let columns = {
        let mut st = store
            .raw_connection()
            .prepare("SELECT name FROM pragma_table_info('fingerprint_baselines')")
            .unwrap();
        st.query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    for name in &columns {
        let lower = name.to_lowercase();
        for needle in ["secret", "token", "password", "credential", "api_key"] {
            assert!(!lower.contains(needle), "{name}");
        }
    }
    assert!(columns.iter().any(|name| name == "baseline_key"));
    assert!(columns.iter().any(|name| name == "distribution_json"));
    assert!(!columns.iter().any(|name| name == "sampling"));

    let good = write_at(COLLECTED, "red");
    let plant = [
        |w: &mut FingerprintBaselineWrite, s: &str| w.identity.provider = s.into(),
        |w: &mut FingerprintBaselineWrite, s: &str| w.identity.model_id = s.into(),
        |w: &mut FingerprintBaselineWrite, s: &str| w.identity.model_revision = s.into(),
        |w: &mut FingerprintBaselineWrite, s: &str| w.identity.protocol = s.into(),
        |w: &mut FingerprintBaselineWrite, s: &str| w.identity.connection_kind = s.into(),
        |w: &mut FingerprintBaselineWrite, s: &str| w.identity.task_set_version = s.into(),
        |w: &mut FingerprintBaselineWrite, s: &str| w.identity.sampling_config = s.into(),
        |w: &mut FingerprintBaselineWrite, s: &str| w.identity.system_prompt_hash = s.into(),
        |w: &mut FingerprintBaselineWrite, s: &str| {
            w.distribution.run1[0].answers[0].answer = s.into();
        },
    ];
    for set in plant {
        let mut bad = good.clone();
        set(&mut bad, &sentinel);
        let err = store.upsert_fingerprint_baseline(&bad).unwrap_err();
        let rendered = format!("{err} {err:?} {:?}", store);
        assert!(!rendered.contains(&sentinel), "{err}");
    }
    assert_eq!(
        store.upsert_fingerprint_baseline(&good).unwrap(),
        FingerprintBaselineUpsert::Inserted
    );
    let dump = format!("{:?}", store.export().unwrap());
    assert!(!dump.contains(&sentinel));
    assert!(!dump.contains("PROMPT-MARKER-NOT-STORED"));
    assert!(dump.contains("example-model"));
    store.checkpoint().unwrap();
    drop(store);
    assert!(!file_contains(&db, &sentinel));
    assert!(!file_contains(&db, "PROMPT-MARKER-NOT-STORED"));
    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}

#[test]
fn migrates_v3_fixture_without_changing_earlier_tables() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/schema-v3.sqlite");
    let db = temp_db("migrate-v3");
    std::fs::copy(&fixture, &db).unwrap();
    let before = {
        let conn = rusqlite::Connection::open(&db).unwrap();
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, 3);
        let mut st = conn
            .prepare("SELECT name, sql FROM sqlite_master WHERE sql IS NOT NULL")
            .unwrap();
        st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .unwrap()
            .collect::<Result<BTreeMap<_, _>, _>>()
            .unwrap()
    };
    assert!(!before.contains_key("fingerprint_baselines"));
    let store = Store::open(&db, digest_key()).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    let after = master_sql(&store);
    for name in [
        "arms",
        "tasks",
        "requests",
        "behavior_events",
        "event_available_arms",
        "probe_results",
        "probe_consent",
        "probe_charges",
        "outcome_judgements",
    ] {
        assert_eq!(
            after.get(name).map(String::as_str),
            before.get(name).map(String::as_str),
            "{name}"
        );
    }
    assert!(after.contains_key("fingerprint_baselines"));
    let model: String = store
        .raw_connection()
        .query_row("SELECT model FROM arms", [], |r| r.get(0))
        .unwrap();
    assert_eq!(model, "fixture-model");
    let accepted: i64 = store
        .raw_connection()
        .query_row("SELECT accepted FROM probe_consent", [], |r| r.get(0))
        .unwrap();
    assert_eq!(accepted, 1);
    let stored = write_at(COLLECTED, "red");
    let clock = FixedClock::new(COLLECTED);
    drop(store);
    let store = Store::open_with_clock(&db, digest_key(), clock).unwrap();
    assert_eq!(store.schema_version().unwrap(), 4);
    assert_eq!(
        store.upsert_fingerprint_baseline(&stored).unwrap(),
        FingerprintBaselineUpsert::Inserted
    );
    assert!(store
        .get_fingerprint_baseline(&stored.identity)
        .unwrap()
        .is_some());
    drop(store);
    let store = Store::open(&db, digest_key()).unwrap();
    assert_eq!(store.schema_version().unwrap(), 4);
    assert_eq!(
        store
            .raw_connection()
            .query_row("SELECT model FROM arms", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "fixture-model"
    );
    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}

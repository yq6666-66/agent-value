//! HANDOFF step 3 storage: outcome judgement history and ratings (v22 §3.1).

use agent_value_core::arm::{Arm, ModelId, Protocol, ProviderKind, Route};
use agent_value_core::cells::CellId;
use agent_value_core::outcome::{judge, Rollback, Rule, TaskSignals, TAU_KEEP_MS};
use agent_value_core::records::{Outcome, TaskRecord};
use agent_value_core::scope::{ScopeId, TaskKind};
use agent_value_store::{DigestKey, Store, StoreError, SCHEMA_VERSION};

const NOW: i64 = 1_792_382_400_000;
const RULES: &str = "outcome-rules-v1";

fn store_with_task(id: &str) -> Store {
    let s =
        Store::open_in_memory(DigestKey::from_bytes(b"test-only-digest-key-aaaaaaaaaaaa").unwrap())
            .unwrap();
    let arm = Arm::new(
        ModelId::new("example-model-1").unwrap(),
        Route::new(
            ProviderKind::Official,
            "api.vendor.example",
            Protocol::AnthropicMessages,
            "example-cli",
        )
        .unwrap(),
    );
    let mut t = TaskRecord::new(
        id,
        ScopeId::new(TaskKind::Coding),
        CellId::user("fix").unwrap(),
        arm,
        NOW - 60_000,
    );
    t.ended_ms = Some(NOW);
    s.insert_task(&t).unwrap();
    s
}

#[test]
fn schema_is_at_version_five() {
    assert_eq!(SCHEMA_VERSION, 5);
    let s = store_with_task("t1");
    assert_eq!(s.schema_version().unwrap(), 5);
}

#[test]
fn retroactive_flip_updates_outcome_and_keeps_history() {
    let mut s = store_with_task("t1");
    let mut sig = TaskSignals::new(TaskKind::Coding);
    sig.ended_ms = Some(NOW);
    sig.agent_reported_completion = true;

    let first = judge(&sig, NOW + 1_000).unwrap();
    assert!(first.provisional);
    s.record_judgement("t1", &first, NOW + 1_000, RULES)
        .unwrap();
    assert_eq!(s.task_outcome("t1").unwrap(), Some(Outcome::Success));

    sig.rollbacks.push(Rollback {
        at_ms: NOW + TAU_KEEP_MS - 1,
        whole_change: true,
    });
    let second = judge(&sig, NOW + TAU_KEEP_MS).unwrap();
    s.record_judgement("t1", &second, NOW + TAU_KEEP_MS, RULES)
        .unwrap();
    assert_eq!(s.task_outcome("t1").unwrap(), Some(Outcome::Failure));

    let h = s.judgement_history("t1").unwrap();
    assert_eq!(h.len(), 2);
    assert_eq!(h[0].judgement, first);
    assert_eq!(h[1].judgement.rule, Rule::RollbackWithinTauKeep);
    assert_eq!(h[1].rules_version, RULES);
}

#[test]
fn judgement_rows_are_immutable() {
    let mut s = store_with_task("t1");
    let mut sig = TaskSignals::new(TaskKind::Coding);
    sig.ended_ms = Some(NOW);
    let j = judge(&sig, NOW + 2 * TAU_KEEP_MS).unwrap();
    s.record_judgement("t1", &j, NOW, RULES).unwrap();
    let r = s
        .raw_connection()
        .execute("UPDATE outcome_judgements SET outcome = 'success'", []);
    assert!(r.is_err());
}

#[test]
fn judgement_requires_known_task_and_clean_rules_version() {
    let mut s = store_with_task("t1");
    let mut sig = TaskSignals::new(TaskKind::Coding);
    sig.ended_ms = Some(NOW);
    let j = judge(&sig, NOW + 2 * TAU_KEEP_MS).unwrap();
    assert!(matches!(
        s.record_judgement("nope", &j, NOW, RULES),
        Err(StoreError::Conflict(_))
    ));
    assert!(s.record_judgement("t1", &j, NOW, " ").is_err());
    let secretish = format!("sk-{}", "a1".repeat(10));
    assert!(s.record_judgement("t1", &j, NOW, &secretish).is_err());
    assert!(s.judgement_history("t1").unwrap().is_empty());
    assert_eq!(s.task_outcome("t1").unwrap(), Some(Outcome::Pending));
}

#[test]
fn deleting_a_task_removes_its_judgements() {
    let mut s = store_with_task("t1");
    let mut sig = TaskSignals::new(TaskKind::Coding);
    sig.ended_ms = Some(NOW);
    let j = judge(&sig, NOW + 2 * TAU_KEEP_MS).unwrap();
    s.record_judgement("t1", &j, NOW, RULES).unwrap();
    s.raw_connection()
        .execute("DELETE FROM tasks WHERE id = 't1'", [])
        .unwrap();
    assert!(s.judgement_history("t1").unwrap().is_empty());
    s.delete_all().unwrap();
}

#[test]
fn latest_rating_wins_and_is_validated() {
    let s = store_with_task("t1");
    s.set_task_rating("t1", 2).unwrap();
    s.set_task_rating("t1", 5).unwrap();
    let r: Option<i64> = s
        .raw_connection()
        .query_row("SELECT rating FROM tasks WHERE id = 't1'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(r, Some(5));
    assert!(s.set_task_rating("t1", 0).is_err());
    assert!(s.set_task_rating("t1", 6).is_err());
    assert!(s.set_task_rating("missing", 3).is_err());
}

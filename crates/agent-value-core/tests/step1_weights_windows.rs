//! HANDOFF step 1 acceptance tests: weight constraints, trial versions,
//! window freezing and the first-window rule (v22 §A, §4, §10.1).

use agent_value_core::constants::WEIGHT_FLOOR;
use agent_value_core::scope::{ScopeId, TaskKind};
use agent_value_core::weights::{
    Evidence, FiveLevel, SelfReportAnswer, TrialResolution, VersionId, VersionStatus, WeightLedger,
    WeightSource, Weights, TRIAL_LABEL,
};
use agent_value_core::window::{FrozenInputs, WindowIndex, FIRST_WINDOW_LABEL};
use agent_value_core::CoreError;

fn w(a: f64, b: f64, g: f64) -> Weights {
    Weights::new(a, b, g).expect("valid weights")
}

fn self_report() -> Evidence {
    Evidence::SelfReportAnswers(
        ["q1", "q2", "q3", "q4"]
            .iter()
            .map(|q| SelfReportAnswer {
                question_id: (*q).to_string(),
                answer: FiveLevel::SlightlyA,
            })
            .collect(),
    )
}

fn ledger() -> WeightLedger {
    WeightLedger::new(ScopeId::new(TaskKind::Coding))
}

// ---- a + b + g = 1 and each >= 0.05 ----

#[test]
fn weights_must_sum_to_one() {
    assert!(Weights::new(0.2, 0.35, 0.45).is_ok());
    assert!(matches!(
        Weights::new(0.2, 0.3, 0.3),
        Err(CoreError::WeightSumNotOne { .. })
    ));
    assert!(matches!(
        Weights::new(0.4, 0.4, 0.4),
        Err(CoreError::WeightSumNotOne { .. })
    ));
}

#[test]
fn each_weight_must_be_at_least_the_floor() {
    assert_eq!(WEIGHT_FLOOR, 0.05);
    // Exactly at the floor is allowed (grid corner).
    assert!(Weights::new(0.05, 0.05, 0.90).is_ok());
    for (a, b, g) in [(0.04, 0.48, 0.48), (0.48, 0.04, 0.48), (0.48, 0.48, 0.04)] {
        assert!(
            matches!(
                Weights::new(a, b, g),
                Err(CoreError::WeightBelowFloor { .. })
            ),
            "({a}, {b}, {g})"
        );
    }
    // Zero and negative weights are below the floor too.
    assert!(matches!(
        Weights::new(0.0, 0.5, 0.5),
        Err(CoreError::WeightBelowFloor { .. })
    ));
    assert!(matches!(
        Weights::new(-0.1, 0.6, 0.5),
        Err(CoreError::WeightBelowFloor { .. })
    ));
}

#[test]
fn non_finite_weights_are_errors_not_panics() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(matches!(
            Weights::new(bad, 0.5, 0.5),
            Err(CoreError::NotFinite { .. })
        ));
    }
}

#[test]
fn every_grid_point_is_valid() {
    let mut n = 0;
    for ai in 1..=18 {
        for bi in 1..=18 {
            let gi = 20 - ai - bi;
            if gi >= 1 {
                let (a, b) = (f64::from(ai) * 0.05, f64::from(bi) * 0.05);
                assert!(Weights::new(a, b, 1.0 - a - b).is_ok());
                n += 1;
            }
        }
    }
    assert_eq!(n, 171);
}

// ---- trial versions never overwrite a window's official result ----

#[test]
fn mid_window_change_creates_trial_and_keeps_official_result() {
    let mut l = ledger();
    let frozen_before = l
        .freeze_window(WindowIndex(0), 0, FrozenInputs::default())
        .unwrap()
        .clone();
    let official_id = frozen_before.weight_version_id();

    let trial = l.create_trial(w(0.6, 0.2, 0.2), WindowIndex(0)).unwrap();
    let tv = l.version(trial).unwrap();
    assert!(tv.is_trial());
    assert_eq!(tv.source(), WeightSource::Manual);
    assert_eq!(tv.previous(), Some(official_id));
    assert_eq!(tv.effective_from(), None);

    // The trial can be previewed right away, labelled as a non-frozen trial,
    // next to the unchanged official version.
    let p = l.trial_preview(trial).unwrap();
    assert_eq!(p.label, TRIAL_LABEL);
    assert_eq!(p.trial.weights(), w(0.6, 0.2, 0.2));
    assert_eq!(p.official.id(), official_id);

    // The window's official inputs are unchanged.
    assert_eq!(l.frozen(WindowIndex(0)).unwrap(), &frozen_before);
    assert_eq!(l.official_for(WindowIndex(0)).id(), official_id);
    assert_eq!(l.official_for(WindowIndex(0)).weights(), Weights::equal());
}

#[test]
fn confirmed_trial_takes_effect_only_from_next_window() {
    let mut l = ledger();
    let frozen0 = l
        .freeze_window(WindowIndex(0), 0, FrozenInputs::default())
        .unwrap()
        .clone();
    let trial = l.create_trial(w(0.6, 0.2, 0.2), WindowIndex(0)).unwrap();
    let official = l.confirm_trial(trial).unwrap();

    let v = l.version(official).unwrap();
    assert_eq!(v.effective_from(), Some(WindowIndex(1)));
    assert_eq!(v.source(), WeightSource::Manual);
    assert!(matches!(
        l.version(trial).unwrap().status(),
        VersionStatus::Trial {
            resolution: TrialResolution::Confirmed(id),
            ..
        } if id == official
    ));

    // Window 0 is still frozen with the old weights.
    assert_eq!(l.frozen(WindowIndex(0)).unwrap(), &frozen0);
    // Window 1 freezes with the confirmed weights.
    let f1 = l
        .freeze_window(WindowIndex(1), 10, FrozenInputs::default())
        .unwrap();
    assert_eq!(f1.weight_version_id(), official);
    assert_eq!(f1.weights(), w(0.6, 0.2, 0.2));
    assert_eq!(l.frozen(WindowIndex(0)).unwrap(), &frozen0);
}

#[test]
fn late_confirmation_goes_to_the_next_unfrozen_window() {
    let mut l = ledger();
    l.freeze_window(WindowIndex(0), 0, FrozenInputs::default())
        .unwrap();
    let trial = l.create_trial(w(0.6, 0.2, 0.2), WindowIndex(0)).unwrap();
    let f1 = l
        .freeze_window(WindowIndex(1), 5, FrozenInputs::default())
        .unwrap()
        .clone();
    let official = l.confirm_trial(trial).unwrap();
    assert_eq!(
        l.version(official).unwrap().effective_from(),
        Some(WindowIndex(2))
    );
    assert_eq!(l.frozen(WindowIndex(1)).unwrap(), &f1);
    assert_eq!(f1.weights(), Weights::equal());
}

#[test]
fn discarded_trial_changes_nothing() {
    let mut l = ledger();
    l.freeze_window(WindowIndex(0), 0, FrozenInputs::default())
        .unwrap();
    let trial = l.create_trial(w(0.6, 0.2, 0.2), WindowIndex(0)).unwrap();
    l.discard_trial(trial).unwrap();
    let f1 = l
        .freeze_window(WindowIndex(1), 3, FrozenInputs::default())
        .unwrap();
    assert_eq!(f1.weight_version_id(), VersionId(0));
    assert!(matches!(
        l.confirm_trial(trial),
        Err(CoreError::TrialAlreadyResolved { .. })
    ));
    assert!(matches!(
        l.discard_trial(trial),
        Err(CoreError::TrialAlreadyResolved { .. })
    ));
}

#[test]
fn official_versions_cannot_target_a_frozen_window() {
    let mut l = ledger();
    l.freeze_window(WindowIndex(0), 0, FrozenInputs::default())
        .unwrap();
    let err = l
        .schedule_official(
            w(0.2, 0.35, 0.45),
            WeightSource::SelfReport,
            self_report(),
            WindowIndex(0),
        )
        .unwrap_err();
    assert_eq!(err, CoreError::WindowAlreadyFrozen { window: 0 });
    // The next window is fine.
    let id = l
        .schedule_official(
            w(0.2, 0.35, 0.45),
            WeightSource::SelfReport,
            self_report(),
            WindowIndex(1),
        )
        .unwrap();
    assert_eq!(l.version(id).unwrap().previous(), Some(VersionId(0)));
    assert_eq!(l.official_for(WindowIndex(0)).id(), VersionId(0));
    assert_eq!(l.official_for(WindowIndex(1)).id(), id);
}

#[test]
fn self_report_before_first_freeze_applies_to_first_window() {
    let mut l = ledger();
    let id = l
        .schedule_official(
            w(0.2, 0.35, 0.45),
            WeightSource::SelfReport,
            self_report(),
            WindowIndex(0),
        )
        .unwrap();
    let f0 = l
        .freeze_window(WindowIndex(0), 0, FrozenInputs::default())
        .unwrap();
    assert_eq!(f0.weight_version_id(), id);
}

#[test]
fn trial_and_freeze_misuse_are_errors() {
    let mut l = ledger();
    // Trial in a window that has not started (not frozen).
    assert!(matches!(
        l.create_trial(w(0.6, 0.2, 0.2), WindowIndex(0)),
        Err(CoreError::WindowNotFrozen { window: 0 })
    ));
    // Freezing out of order, or twice.
    assert!(matches!(
        l.freeze_window(WindowIndex(1), 0, FrozenInputs::default()),
        Err(CoreError::WindowOutOfOrder {
            expected: 0,
            got: 1
        })
    ));
    l.freeze_window(WindowIndex(0), 0, FrozenInputs::default())
        .unwrap();
    assert!(matches!(
        l.freeze_window(WindowIndex(0), 0, FrozenInputs::default()),
        Err(CoreError::WindowAlreadyFrozen { window: 0 })
    ));
    // Confirming something that is not a trial, or does not exist.
    assert!(matches!(
        l.confirm_trial(VersionId(0)),
        Err(CoreError::NotATrial { id: 0 })
    ));
    assert!(matches!(
        l.confirm_trial(VersionId(99)),
        Err(CoreError::UnknownVersion { id: 99 })
    ));
    assert!(matches!(
        l.trial_preview(VersionId(0)),
        Err(CoreError::NotATrial { id: 0 })
    ));
}

#[test]
fn evidence_must_match_source_when_scheduling() {
    let mut l = ledger();
    assert!(matches!(
        l.schedule_official(
            w(0.2, 0.35, 0.45),
            WeightSource::SelfReport,
            Evidence::None,
            WindowIndex(0)
        ),
        Err(CoreError::EvidenceMismatch { .. })
    ));
    assert!(matches!(
        l.schedule_official(
            w(0.2, 0.35, 0.45),
            WeightSource::Default,
            Evidence::None,
            WindowIndex(0)
        ),
        Err(CoreError::EvidenceMismatch { .. })
    ));
    let events = Evidence::BehaviorEvents((0..20).map(|i| format!("event-{i}")).collect());
    assert!(l
        .schedule_official(
            w(0.2, 0.35, 0.45),
            WeightSource::ConfirmedBehaviorSuggestion,
            events,
            WindowIndex(0)
        )
        .is_ok());
}

// ---- first window produces no V ----

#[test]
fn first_window_produces_no_v() {
    let mut l = ledger();
    let f0 = l
        .freeze_window(WindowIndex(0), 0, FrozenInputs::default())
        .unwrap();
    assert!(f0.is_first_window());
    assert_eq!(
        f0.ensure_v_allowed_by_window_rule(),
        Err(CoreError::VWithheldFirstWindow { window: 0 })
    );
    assert_eq!(f0.first_window_label(), Some(FIRST_WINDOW_LABEL));
}

#[test]
fn window_after_history_is_not_withheld_by_window_rule() {
    let mut l = ledger();
    l.freeze_window(WindowIndex(0), 0, FrozenInputs::default())
        .unwrap();
    let f1 = l
        .freeze_window(WindowIndex(1), 42, FrozenInputs::default())
        .unwrap();
    assert!(!f1.is_first_window());
    assert_eq!(f1.ensure_v_allowed_by_window_rule(), Ok(()));
    assert_eq!(f1.first_window_label(), None);
}

#[test]
fn no_history_yet_is_still_a_first_window() {
    // Window 1 of a scope that had no user tasks in window 0 has no history.
    let mut l = WeightLedger::new(ScopeId::new(TaskKind::NonCoding));
    l.freeze_window(WindowIndex(0), 0, FrozenInputs::default())
        .unwrap();
    let f1 = l
        .freeze_window(WindowIndex(1), 0, FrozenInputs::default())
        .unwrap();
    assert!(f1.is_first_window());
    assert!(f1.ensure_v_allowed_by_window_rule().is_err());
}

#[test]
fn scopes_have_independent_ledgers() {
    let mut coding = WeightLedger::new(ScopeId::new(TaskKind::Coding));
    let non_coding = WeightLedger::new(ScopeId::new(TaskKind::NonCoding));
    coding
        .schedule_official(
            w(0.2, 0.35, 0.45),
            WeightSource::SelfReport,
            self_report(),
            WindowIndex(0),
        )
        .unwrap();
    assert_eq!(
        coding.official_for(WindowIndex(0)).weights(),
        w(0.2, 0.35, 0.45)
    );
    assert_eq!(
        non_coding.official_for(WindowIndex(0)).weights(),
        Weights::equal()
    );
    assert_eq!(coding.official_for(WindowIndex(0)).scope(), coding.scope());
}

#[test]
fn frozen_inputs_are_recorded() {
    let mut l = ledger();
    let inputs = FrozenInputs {
        price_table_version: Some("prices-1".into()),
        ..FrozenInputs::default()
    };
    let f0 = l.freeze_window(WindowIndex(0), 0, inputs.clone()).unwrap();
    assert_eq!(f0.inputs(), &inputs);
    assert_eq!(f0.scope(), &ScopeId::new(TaskKind::Coding));
    assert_eq!(f0.window(), WindowIndex(0));
}

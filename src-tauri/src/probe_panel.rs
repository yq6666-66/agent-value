//! Probe consent, budget, and recorded results.
//!
//! Consent and charges come from the local store. Budget decisions,
//! observation-period copy, and authenticity grades come from
//! `agent_value_core`. The fixture source judges the checked-in replay.
//! This module does not send HTTP.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use agent_value_core::arm::{Arm, ModelId, Protocol, ProviderKind, Route};
use agent_value_core::authenticity::{
    assess, budget_month, grade_reference_only, grade_run_days_label, monthly_budget_label,
    observation, over_budget_label, probe_budget, AuthStatus, BudgetDecision, DaySummary,
    GateContext, GRADE_REFERENCE_ONLY_LABEL,
};
use agent_value_core::constants::PROBE_MONTHLY_BUDGET_YUAN;
use agent_value_core::probe::{
    applicable_items, judge_capability, judge_deterministic, parse_replay, run_hard_failures,
    CapabilityClaims, ClassRun, VendorTables, PROBE_SET_VERSION,
};
use agent_value_core::records::{MicroYuan, ProbeClass, ProbeResult};
use serde::Serialize;

use crate::store_cmd::{self, AppState};

/// Seed named in the mixed replay fixture header.
const FIXTURE_SEED: u64 = 20_261_022;
const FIXTURE_REPLAY: &str =
    include_str!("../../crates/agent-value-core/tests/fixtures/step13_replay_mixed.tsv");

/// What a probe run would do. `NotSent` means the policy allows a run and
/// this build still does not send one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeLaunch {
    BlockedConsent,
    BlockedBudget,
    NotSent,
}

impl ProbeLaunch {
    fn as_str(self) -> &'static str {
        match self {
            Self::BlockedConsent => "blocked-consent",
            Self::BlockedBudget => "blocked-budget",
            Self::NotSent => "not-sent",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::BlockedConsent => "尚未同意，不会自动探测。",
            Self::BlockedBudget => "本月停止探测。",
            Self::NotSent => "已同意，且未超预算。本版不发送探测请求。",
        }
    }
}

fn launch_of(accepted: bool, budget: BudgetDecision) -> ProbeLaunch {
    if !accepted {
        ProbeLaunch::BlockedConsent
    } else if budget == BudgetDecision::Stop {
        ProbeLaunch::BlockedBudget
    } else {
        ProbeLaunch::NotSent
    }
}

/// JSON for the probe panel. Grades, verdicts, and the budget and
/// observation sentences are the core's.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProbePanel {
    pub source: String,
    pub fixture: bool,
    pub decided: bool,
    pub accepted: bool,
    pub consent_prompt: String,
    pub monthly_budget_label: String,
    pub spent_label: String,
    pub estimate_label: String,
    pub budget_decision: String,
    pub over_budget_prompt: Option<String>,
    pub observation: String,
    pub observation_label: String,
    pub grade_run_days_label: String,
    pub launch: String,
    pub launch_label: String,
    /// Always false: this build does not send probe HTTP.
    pub sends_http: bool,
    pub arms: Vec<ProbeArmView>,
    pub empty_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProbeArmView {
    pub id: String,
    pub grade: Option<String>,
    pub reference_only: bool,
    pub reference_label: Option<String>,
    pub labels: Vec<String>,
    pub classes: Vec<ClassView>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClassView {
    pub class: String,
    pub verdict: String,
    pub run_day: String,
    pub evidence: String,
}

struct BudgetView {
    decision: BudgetDecision,
    prompt: Option<String>,
}

fn now_ms() -> Result<i64, String> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| err.to_string())?;
    i64::try_from(elapsed.as_millis()).map_err(|_| "clock is out of range".to_string())
}

fn budget_view(spent: MicroYuan, estimate: MicroYuan, at_ms: i64) -> Result<BudgetView, String> {
    let budget = MicroYuan::from_yuan(PROBE_MONTHLY_BUDGET_YUAN).map_err(|err| err.to_string())?;
    let decision = probe_budget(spent, estimate, budget).map_err(|err| err.to_string())?;
    let prompt = if decision == BudgetDecision::Stop {
        let (year, month) = budget_month(at_ms).map_err(|err| err.to_string())?;
        Some(over_budget_label(spent, estimate, year, month).map_err(|err| err.to_string())?)
    } else {
        None
    };
    Ok(BudgetView { decision, prompt })
}

fn spent_this_month(charges: &[(i64, i64)], at_ms: i64) -> Result<MicroYuan, String> {
    let month = budget_month(at_ms).map_err(|err| err.to_string())?;
    let mut total = 0_i64;
    for &(at, micros) in charges {
        if budget_month(at).map_err(|err| err.to_string())? == month {
            total = total
                .checked_add(micros)
                .ok_or("probe spend overflow".to_string())?;
        }
    }
    MicroYuan::new(total).map_err(|err| err.to_string())
}

/// No control runs are stored, so the observation period stays pending.
fn current_observation() -> agent_value_core::authenticity::Observation {
    let empty = DaySummary::default();
    observation(&empty, &empty, false, false)
}

fn class_result(
    arm: &Arm,
    at_ms: i64,
    class: ProbeClass,
    run: ClassRun,
    hard: bool,
) -> ProbeResult {
    ProbeResult {
        probe_id: "step13-replay-mixed".into(),
        probe_version: PROBE_SET_VERSION.into(),
        arm: arm.clone(),
        at_ms,
        class,
        verdict: run.verdict,
        hard_failure: hard,
        evidence_summary: run.evidence_summary(),
    }
}

fn fixture_results() -> Result<(Arm, Vec<ProbeResult>), String> {
    let records = parse_replay(FIXTURE_REPLAY).map_err(|err| err.to_string())?;
    let at_ms = records
        .first()
        .map(|record| record.at_ms)
        .ok_or("fixture replay is empty".to_string())?;
    let arm = Arm::new(
        ModelId::new("claude-haiku-4-5").map_err(|err| err.to_string())?,
        Route::new(
            ProviderKind::Relay,
            "relay.example",
            Protocol::OpenAiChatCompletions,
            "fixture",
        )
        .map_err(|err| err.to_string())?,
    );
    let deterministic =
        judge_deterministic(FIXTURE_SEED, &records).map_err(|err| err.to_string())?;
    let claims = CapabilityClaims {
        tool_call: true,
        json_schema: true,
        ..CapabilityClaims::default()
    };
    let items = applicable_items(&claims, arm.route().protocol());
    let capability =
        judge_capability(FIXTURE_SEED, &items, None, &records).map_err(|err| err.to_string())?;
    let hard = !run_hard_failures(
        &VendorTables::default(),
        arm.model(),
        arm.route().protocol(),
        &claims,
        &records,
    )
    .is_empty();
    let results = vec![
        class_result(&arm, at_ms, ProbeClass::Deterministic, deterministic, hard),
        class_result(&arm, at_ms, ProbeClass::Capability, capability, hard),
    ];
    for result in &results {
        result.validate().map_err(|err| err.to_string())?;
    }
    Ok((arm, results))
}

fn judge_arm(
    arm: &Arm,
    results: &[ProbeResult],
    probes_enabled: bool,
) -> Result<ProbeArmView, String> {
    let observation_state = current_observation();
    let assessment = assess(
        &VendorTables::default(),
        arm,
        results,
        &[],
        GateContext {
            probes_enabled,
            observation: observation_state,
        },
    )
    .map_err(|err| err.to_string())?;
    let reference_only = grade_reference_only(observation_state, assessment.status);
    let grade = match assessment.status {
        AuthStatus::OfficialDirect => None,
        AuthStatus::Graded(grade) => Some(grade.as_str().to_string()),
    };
    let mut classes = Vec::with_capacity(results.len());
    for result in results {
        classes.push(ClassView {
            class: result.class.as_str().to_string(),
            verdict: result.verdict.as_str().to_string(),
            run_day: result.run_day().map_err(|err| err.to_string())?.to_string(),
            evidence: result.evidence_summary.clone(),
        });
    }
    Ok(ProbeArmView {
        id: arm.to_string(),
        grade,
        reference_only,
        reference_label: reference_only.then(|| GRADE_REFERENCE_ONLY_LABEL.to_string()),
        labels: assessment
            .labels
            .iter()
            .map(|label| label.zh().to_string())
            .collect(),
        classes,
    })
}

fn stored_groups(store: &agent_value_store::Store) -> Result<Vec<(Arm, Vec<ProbeResult>)>, String> {
    let mut groups: BTreeMap<Arm, Vec<ProbeResult>> = BTreeMap::new();
    for result in store.list_probe_results().map_err(|err| err.to_string())? {
        groups.entry(result.arm.clone()).or_default().push(result);
    }
    Ok(groups.into_iter().collect())
}

fn decision_text(decision: BudgetDecision) -> &'static str {
    match decision {
        BudgetDecision::Allow => "allow",
        BudgetDecision::Stop => "stop",
    }
}

fn assemble(
    app: &AppState,
    source: &str,
    accepted: bool,
    decided: bool,
) -> Result<ProbePanel, String> {
    let at_ms = now_ms()?;
    let charges = store_cmd::with_store(app, |store| {
        store.probe_charges().map_err(|err| err.to_string())
    })?;
    let spent = spent_this_month(&charges, at_ms)?;
    let estimate = MicroYuan::new(0).map_err(|err| err.to_string())?;
    let budget = budget_view(spent, estimate, at_ms)?;
    let launch = launch_of(accepted, budget.decision);
    let observation_state = current_observation();
    let (fixture, arms, empty_reason) = match source {
        "store" => {
            let groups = store_cmd::with_store(app, stored_groups)?;
            let mut arms = Vec::with_capacity(groups.len());
            for (arm, results) in groups {
                arms.push(judge_arm(&arm, &results, accepted)?);
            }
            let empty_reason = if arms.is_empty() {
                Some("尚无探针记录。".to_string())
            } else {
                None
            };
            (false, arms, empty_reason)
        }
        "fixture" => {
            let (arm, results) = fixture_results()?;
            (true, vec![judge_arm(&arm, &results, accepted)?], None)
        }
        _ => return Err(format!("unknown probe source: {source}")),
    };
    let budget_label = monthly_budget_label().map_err(|err| err.to_string())?;
    Ok(ProbePanel {
        source: source.to_string(),
        fixture,
        decided,
        accepted,
        consent_prompt: format!("{budget_label}。同意之前不会自动探测。本版不发送网络请求。"),
        monthly_budget_label: budget_label,
        spent_label: format!("本月已花 {} 元", spent.format_yuan()),
        estimate_label: format!("这次估价 {} 元", estimate.format_yuan()),
        budget_decision: decision_text(budget.decision).to_string(),
        over_budget_prompt: budget.prompt,
        observation: observation_state.as_str().to_string(),
        observation_label: observation_state.label(),
        grade_run_days_label: grade_run_days_label(),
        launch: launch.as_str().to_string(),
        launch_label: launch.label().to_string(),
        sends_http: false,
        arms,
        empty_reason,
    })
}

/// `source` is `store` or `fixture`.
pub fn view(app: &AppState, source: &str) -> Result<ProbePanel, String> {
    let consent = store_cmd::with_store(app, |store| {
        store.probe_consent().map_err(|err| err.to_string())
    })?;
    let (decided, accepted) = match consent {
        Some(consent) => (true, consent.accepted),
        None => (false, false),
    };
    assemble(app, source, accepted, decided)
}

/// Stores the first consent decision, then returns the panel for `source`.
pub fn set_consent(app: &AppState, accepted: bool, source: &str) -> Result<ProbePanel, String> {
    let at_ms = now_ms()?;
    store_cmd::with_store(app, |store| {
        store
            .set_probe_consent(accepted, at_ms)
            .map_err(|err| err.to_string())?;
        Ok(())
    })?;
    view(app, source)
}

#[cfg(test)]
mod tests {
    use agent_value_core::authenticity::{
        assess, budget_month, monthly_budget_label, observation, over_budget_label, AuthLabel,
        BudgetDecision, DaySummary, GateContext, GRADE_REFERENCE_ONLY_LABEL,
    };
    use agent_value_core::constants::PROBE_MONTHLY_BUDGET_YUAN;
    use agent_value_core::probe::{judge_capability, judge_deterministic, parse_replay};
    use agent_value_core::records::{MicroYuan, ProbeClass, ProbeResult, ProbeVerdict};

    use super::*;

    const OCTOBER: i64 = 1_792_382_400_000;

    fn app() -> AppState {
        AppState::open_in_memory().unwrap()
    }

    #[test]
    fn budget_prompt_matches_the_core_boundaries() {
        let spent = MicroYuan::from_yuan(9.99).unwrap();
        let allow = budget_view(spent, MicroYuan::from_yuan(0.01).unwrap(), OCTOBER).unwrap();
        assert_eq!(allow.decision, BudgetDecision::Allow);
        assert!(allow.prompt.is_none());
        let stop = budget_view(spent, MicroYuan::from_yuan(0.02).unwrap(), OCTOBER).unwrap();
        assert_eq!(stop.decision, BudgetDecision::Stop);
        let (year, month) = budget_month(OCTOBER).unwrap();
        assert_eq!(
            stop.prompt.unwrap(),
            over_budget_label(spent, MicroYuan::from_yuan(0.02).unwrap(), year, month).unwrap()
        );
        let full = budget_view(
            MicroYuan::from_yuan(PROBE_MONTHLY_BUDGET_YUAN).unwrap(),
            MicroYuan::new(0).unwrap(),
            OCTOBER,
        )
        .unwrap();
        assert_eq!(full.decision, BudgetDecision::Stop);
        assert!(full.prompt.unwrap().contains("本月停止探测"));
    }

    #[test]
    fn undecided_store_does_not_allow_a_run_or_invent_a_grade() {
        let panel = view(&app(), "store").unwrap();
        assert!(!panel.decided);
        assert!(!panel.accepted);
        assert!(!panel.sends_http);
        assert_eq!(panel.launch, "blocked-consent");
        assert_eq!(panel.launch_label, ProbeLaunch::BlockedConsent.label());
        assert!(panel.arms.is_empty());
        assert_eq!(panel.empty_reason.as_deref(), Some("尚无探针记录。"));
        assert!(panel.over_budget_prompt.is_none());
        let budget = monthly_budget_label().unwrap();
        assert_eq!(panel.monthly_budget_label, budget);
        assert_eq!(
            panel.consent_prompt,
            format!("{budget}。同意之前不会自动探测。本版不发送网络请求。")
        );
        let empty = DaySummary::default();
        let state = observation(&empty, &empty, false, false);
        assert_eq!(panel.observation, state.as_str());
        assert_eq!(panel.observation_label, state.label());
        assert_eq!(panel.grade_run_days_label, grade_run_days_label());
        assert_eq!(
            launch_of(false, BudgetDecision::Allow),
            ProbeLaunch::BlockedConsent
        );
        assert_eq!(
            launch_of(true, BudgetDecision::Stop),
            ProbeLaunch::BlockedBudget
        );
        assert_eq!(launch_of(true, BudgetDecision::Allow), ProbeLaunch::NotSent);
    }

    #[test]
    fn first_consent_sticks_and_still_sends_nothing() {
        let app = app();
        let accepted = set_consent(&app, true, "store").unwrap();
        assert!(accepted.decided && accepted.accepted);
        assert_eq!(accepted.launch, "not-sent");
        assert!(!accepted.sends_http);
        assert!(accepted.launch_label.contains("不发送探测请求"));
        let declined = set_consent(&app, false, "store").unwrap();
        assert!(declined.accepted);
        assert_eq!(declined.launch, "not-sent");
    }

    #[test]
    fn a_full_budget_charge_shows_the_stop_prompt() {
        let app = app();
        set_consent(&app, true, "store").unwrap();
        let at = now_ms().unwrap();
        let spent = MicroYuan::from_yuan(PROBE_MONTHLY_BUDGET_YUAN).unwrap();
        store_cmd::with_store(&app, |store| {
            store
                .record_probe_charge(at, spent)
                .map_err(|err| err.to_string())
        })
        .unwrap();
        let panel = view(&app, "store").unwrap();
        assert_eq!(panel.budget_decision, "stop");
        assert_eq!(panel.launch, "blocked-budget");
        assert!(!panel.sends_http);
        let (year, month) = budget_month(at).unwrap();
        assert_eq!(
            panel.over_budget_prompt.unwrap(),
            over_budget_label(spent, MicroYuan::new(0).unwrap(), year, month).unwrap()
        );
        assert!(panel.spent_label.contains(&spent.format_yuan()));
    }

    #[test]
    fn spend_below_the_budget_and_outside_the_month_does_not_stop() {
        let app = app();
        set_consent(&app, true, "store").unwrap();
        store_cmd::with_store(&app, |store| {
            store
                .record_probe_charge(now_ms().unwrap(), MicroYuan::from_yuan(9.99).unwrap())
                .map_err(|err| err.to_string())?;
            store
                .record_probe_charge(0, MicroYuan::from_yuan(PROBE_MONTHLY_BUDGET_YUAN).unwrap())
                .map_err(|err| err.to_string())
        })
        .unwrap();
        let panel = view(&app, "store").unwrap();
        assert_eq!(panel.budget_decision, "allow");
        assert!(panel.over_budget_prompt.is_none());
        assert_eq!(panel.launch, "not-sent");
    }

    #[test]
    fn fixture_verdicts_and_grade_come_from_the_core() {
        let app = app();
        let panel = view(&app, "fixture").unwrap();
        assert!(panel.fixture);
        assert_eq!(panel.arms.len(), 1);
        let records = parse_replay(FIXTURE_REPLAY).unwrap();
        let deterministic = judge_deterministic(FIXTURE_SEED, &records).unwrap();
        let claims = CapabilityClaims {
            tool_call: true,
            json_schema: true,
            ..CapabilityClaims::default()
        };
        let (arm, results) = fixture_results().unwrap();
        let capability = judge_capability(
            FIXTURE_SEED,
            &applicable_items(&claims, arm.route().protocol()),
            None,
            &records,
        )
        .unwrap();
        let row = &panel.arms[0];
        let det = row
            .classes
            .iter()
            .find(|class| class.class == ProbeClass::Deterministic.as_str())
            .unwrap();
        let cap = row
            .classes
            .iter()
            .find(|class| class.class == ProbeClass::Capability.as_str())
            .unwrap();
        assert_eq!(det.verdict, deterministic.verdict.as_str());
        assert_eq!(det.evidence, deterministic.evidence_summary());
        assert_eq!(cap.verdict, capability.verdict.as_str());
        assert_eq!(cap.evidence, capability.evidence_summary());
        let assessed = assess(
            &VendorTables::default(),
            &arm,
            &results,
            &[],
            GateContext {
                probes_enabled: false,
                observation: current_observation(),
            },
        )
        .unwrap();
        let AuthStatus::Graded(grade) = assessed.status else {
            panic!("fixture arm is graded");
        };
        assert_eq!(row.grade.as_deref(), Some(grade.as_str()));
        assert!(row.reference_only);
        assert_eq!(
            row.reference_label.as_deref(),
            Some(GRADE_REFERENCE_ONLY_LABEL)
        );
        assert!(row
            .labels
            .iter()
            .any(|label| label == AuthLabel::UnverifiedProbesOff.zh()));
        assert!(row
            .labels
            .iter()
            .any(|label| label == AuthLabel::ObservationOnly.zh()));
        assert!(!panel.sends_http);
    }

    #[test]
    fn an_official_direct_arm_is_not_a_reference_grade() {
        let app = app();
        let arm = Arm::new(
            ModelId::new("claude-haiku-4-5").unwrap(),
            Route::new(
                ProviderKind::Official,
                "api.anthropic.com",
                Protocol::AnthropicMessages,
                "cli",
            )
            .unwrap(),
        );
        let stored = ProbeResult {
            probe_id: "deterministic-set".into(),
            probe_version: "1".into(),
            arm: arm.clone(),
            at_ms: OCTOBER,
            class: ProbeClass::Deterministic,
            verdict: ProbeVerdict::Pass,
            hard_failure: false,
            evidence_summary: "recorded".into(),
        };
        store_cmd::with_store(&app, |store| {
            store
                .insert_probe_result(&stored)
                .map_err(|err| err.to_string())
        })
        .unwrap();
        let panel = view(&app, "store").unwrap();
        let row = panel
            .arms
            .iter()
            .find(|row| row.id == arm.to_string())
            .unwrap();
        assert!(row.grade.is_none());
        assert!(!row.reference_only);
        assert!(row.reference_label.is_none());
        assert!(row
            .labels
            .iter()
            .any(|label| label == AuthLabel::OfficialDirect.zh()));
        assert!(row
            .classes
            .iter()
            .any(|class| class.verdict == ProbeVerdict::Pass.as_str()));
    }

    #[test]
    fn unknown_source_is_rejected() {
        let err = view(&app(), "live").unwrap_err();
        assert!(err.contains("unknown probe source"));
    }
}

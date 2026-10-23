//! Probe consent, budget, and recorded results.
//!
//! Consent and charges come from the local store. Budget decisions,
//! observation-period copy, and authenticity grades come from
//! `agent_value_core`. The fixture source judges the checked-in replay.
//! This view does not send HTTP. [`crate::probe_run::run_from_webview`] can,
//! when the webview asks or when an in-app schedule tick is due, and that
//! call runs on a blocking thread. `sends_http` is true because those paths
//! can send. Opening the page does not run a probe and does not arm the
//! schedule. Observation thresholds are unchanged.

use std::collections::{BTreeMap, BTreeSet};
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
use agent_value_probe_run::{spent_in_month, typical_probe_estimate};
use serde::Serialize;

use crate::probe_run::{self, RunOnceInput};
use crate::probe_schedule::{ScheduleInput, SCHEDULE_BUDGET_STOP_LABEL, SCHEDULE_NO_CONSENT_LABEL};
use crate::store_cmd::{self, AppState};

/// Seed named in the mixed replay fixture header.
const FIXTURE_SEED: u64 = 20_261_022;
const FIXTURE_REPLAY: &str =
    include_str!("../../crates/agent-value-core/tests/fixtures/step13_replay_mixed.tsv");

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
    /// True in this build: `run_probe_once` and a due in-app tick can send
    /// when consent and the monthly budget allow. This view command does
    /// not send. Opening the app does not arm a schedule.
    pub sends_http: bool,
    /// True only after the user enables the in-app schedule. Process exit
    /// clears it. This view command does not set it.
    pub schedule_active: bool,
    /// Read-only next fire time when [`Self::schedule_active`].
    pub next_planned_label: Option<String>,
    /// Budget stop, or a refusal to arm. None when there is nothing to say.
    pub schedule_status_label: Option<String>,
    /// Provisional interval sentence. The webview does not choose the hours.
    pub schedule_hint: String,
    pub arms: Vec<ProbeArmView>,
    /// Arms the webview may pass to `run_probe_once`. Stored arms, then any
    /// probe-result arm not already listed. The id is `Arm`'s display
    /// string. Empty for the fixture source. No key material.
    pub run_targets: Vec<ProbeRunTarget>,
    pub empty_reason: Option<String>,
}

/// One arm the run command can resolve. `id` is not a secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProbeRunTarget {
    pub id: String,
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
    spent_in_month(charges, at_ms).map_err(|err| err.to_string())
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

fn push_target(seen: &mut BTreeSet<String>, targets: &mut Vec<ProbeRunTarget>, arm: &Arm) {
    let id = arm.to_string();
    if seen.insert(id.clone()) {
        targets.push(ProbeRunTarget { id });
    }
}

/// Store arms first, so a route with no probe result yet can still be run.
/// Probe-result arms are included when they are not already in that list.
fn run_target_list(store: &agent_value_store::Store) -> Result<Vec<ProbeRunTarget>, String> {
    let mut seen = BTreeSet::new();
    let mut targets = Vec::new();
    for (_, arm) in store.list_arms().map_err(|err| err.to_string())? {
        push_target(&mut seen, &mut targets, &arm);
    }
    for result in store.list_probe_results().map_err(|err| err.to_string())? {
        push_target(&mut seen, &mut targets, &result.arm);
    }
    Ok(targets)
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
    let estimate = typical_probe_estimate().map_err(|err| err.to_string())?;
    let budget = budget_view(spent, estimate, at_ms)?;
    let launch = app.flight.launch(accepted, budget.decision);
    let observation_state = current_observation();
    let (fixture, arms, run_targets, empty_reason) = match source {
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
            let run_targets = store_cmd::with_store(app, run_target_list)?;
            (false, arms, run_targets, empty_reason)
        }
        "fixture" => {
            let (arm, results) = fixture_results()?;
            (
                true,
                vec![judge_arm(&arm, &results, accepted)?],
                Vec::new(),
                None,
            )
        }
        _ => return Err(format!("unknown probe source: {source}")),
    };
    let budget_label = monthly_budget_label().map_err(|err| err.to_string())?;
    let schedule = app.schedule.snapshot();
    let schedule_status_label = if budget.decision == BudgetDecision::Stop {
        Some(SCHEDULE_BUDGET_STOP_LABEL.to_string())
    } else {
        schedule.status_label.clone()
    };
    Ok(ProbePanel {
        source: source.to_string(),
        fixture,
        decided,
        accepted,
        consent_prompt: format!(
            "{budget_label}。同意之前不会自动探测。同意之后可以手动跑一次。定时重跑要另外启用，启用时不会马上发送。"
        ),
        monthly_budget_label: budget_label,
        spent_label: format!("本月已花 {} 元", spent.format_yuan()),
        estimate_label: format!("典型计划估价 {} 元", estimate.format_yuan()),
        budget_decision: decision_text(budget.decision).to_string(),
        over_budget_prompt: budget.prompt,
        observation: observation_state.as_str().to_string(),
        observation_label: observation_state.label(),
        grade_run_days_label: grade_run_days_label(),
        launch: launch.as_str().to_string(),
        launch_label: launch.label().to_string(),
        sends_http: true,
        schedule_active: schedule.active,
        next_planned_label: schedule.next_planned_label,
        schedule_status_label,
        schedule_hint: schedule.hint,
        arms,
        run_targets,
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

/// Enables or stops the in-app schedule, then returns the store panel.
///
/// Enabling does not send. The first tick is one provisional interval
/// later. No stored consent, a budget stop, or a host mismatch leaves the
/// schedule inactive and does not send. Stopping does not send.
pub fn set_schedule(app: &AppState, input: &ScheduleInput) -> Result<ProbePanel, String> {
    set_schedule_at(app, input, now_ms()?)
}

pub(crate) fn set_schedule_at(
    app: &AppState,
    input: &ScheduleInput,
    now_ms: i64,
) -> Result<ProbePanel, String> {
    if !input.enabled {
        app.schedule.disable();
        return view(app, "store");
    }
    let consent = store_cmd::with_store(app, |store| {
        store.probe_consent().map_err(|err| err.to_string())
    })?;
    let accepted = consent.is_some_and(|consent| consent.accepted);
    if !accepted {
        app.schedule.note(SCHEDULE_NO_CONSENT_LABEL);
        return view(app, "store");
    }
    let charges = store_cmd::with_store(app, |store| {
        store.probe_charges().map_err(|err| err.to_string())
    })?;
    let spent = spent_this_month(&charges, now_ms)?;
    let estimate = typical_probe_estimate().map_err(|err| err.to_string())?;
    let budget = budget_view(spent, estimate, now_ms)?;
    if budget.decision == BudgetDecision::Stop {
        app.schedule.stop_for_budget();
        return view(app, "store");
    }
    let run_input = RunOnceInput {
        arm_id: input.arm_id.clone(),
        key_env: input.key_env.clone(),
        base_url: input.base_url.clone(),
    };
    match probe_run::schedule_binding(app, &run_input) {
        Ok(binding) => app.schedule.enable(now_ms, binding),
        Err(probe_run::BindFail::Rejected(rejected)) => app.schedule.note(&rejected.detail),
        Err(probe_run::BindFail::Store(err)) => return Err(err),
    }
    view(app, "store")
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
    use agent_value_probe_run::{typical_probe_estimate, Flight, ProbeLaunch};

    use crate::probe_run::scheduled_tick;
    use crate::probe_schedule::{ScheduleBinding, TickClass, PROBE_SCHEDULE_INTERVAL_MS};

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
        assert!(panel.sends_http);
        assert!(panel.run_targets.is_empty());
        assert_eq!(panel.launch, "blocked-consent");
        assert_eq!(panel.launch_label, ProbeLaunch::BlockedConsent.label());
        assert!(panel.arms.is_empty());
        assert_eq!(panel.empty_reason.as_deref(), Some("尚无探针记录。"));
        assert!(panel.over_budget_prompt.is_none());
        assert!(!panel.schedule_active);
        assert!(panel.next_planned_label.is_none());
        assert!(panel.schedule_status_label.is_none());
        assert!(panel.schedule_hint.contains("暂定"));
        assert!(panel.schedule_hint.contains("不是系统定时任务"));
        assert!(!panel.schedule_hint.contains("已校准"));
        assert!(!panel.consent_prompt.contains("已校准"));
        let budget = monthly_budget_label().unwrap();
        assert_eq!(panel.monthly_budget_label, budget);
        assert_eq!(
            panel.consent_prompt,
            format!(
                "{budget}。同意之前不会自动探测。同意之后可以手动跑一次。定时重跑要另外启用，启用时不会马上发送。"
            )
        );
        let estimate = typical_probe_estimate().unwrap();
        assert!(estimate.micros() > 0);
        assert_eq!(
            panel.estimate_label,
            format!("典型计划估价 {} 元", estimate.format_yuan())
        );
        let empty = DaySummary::default();
        let state = observation(&empty, &empty, false, false);
        assert_eq!(panel.observation, state.as_str());
        assert_eq!(panel.observation_label, state.label());
        assert_eq!(panel.grade_run_days_label, grade_run_days_label());
        let flight = Flight::new();
        assert_eq!(
            flight.launch(false, BudgetDecision::Allow),
            ProbeLaunch::BlockedConsent
        );
        assert_eq!(
            flight.launch(true, BudgetDecision::Stop),
            ProbeLaunch::BlockedBudget
        );
        assert_eq!(
            flight.launch(true, BudgetDecision::Allow),
            ProbeLaunch::Ready
        );
    }

    #[test]
    fn first_consent_sticks_and_a_manual_run_is_allowed() {
        let app = app();
        let accepted = set_consent(&app, true, "store").unwrap();
        assert!(accepted.decided && accepted.accepted);
        assert_eq!(accepted.launch, ProbeLaunch::Ready.as_str());
        assert_eq!(accepted.launch_label, ProbeLaunch::Ready.label());
        assert!(accepted.sends_http);
        assert!(!accepted.launch_label.contains("已校准"));
        assert!(!accepted.consent_prompt.contains("已校准"));
        let declined = set_consent(&app, false, "store").unwrap();
        assert!(declined.accepted);
        assert_eq!(declined.launch, ProbeLaunch::Ready.as_str());
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
        assert!(panel.sends_http);
        let (year, month) = budget_month(at).unwrap();
        let estimate = typical_probe_estimate().unwrap();
        assert_eq!(
            panel.over_budget_prompt.unwrap(),
            over_budget_label(spent, estimate, year, month).unwrap()
        );
        assert!(panel.spent_label.contains(&spent.format_yuan()));
        assert!(panel.estimate_label.contains(&estimate.format_yuan()));
        assert!(!panel.schedule_active);
        assert_eq!(
            panel.schedule_status_label.as_deref(),
            Some(SCHEDULE_BUDGET_STOP_LABEL)
        );
        assert!(panel.sends_http);
    }

    #[test]
    fn a_near_full_budget_stops_on_the_nonzero_estimate() {
        let app = app();
        set_consent(&app, true, "store").unwrap();
        let at = now_ms().unwrap();
        let budget = MicroYuan::from_yuan(PROBE_MONTHLY_BUDGET_YUAN).unwrap();
        let estimate = typical_probe_estimate().unwrap();
        assert!(estimate.micros() > 0);
        // One micro under the cap. An estimate of 0 would still allow it.
        let spent = MicroYuan::new(budget.micros() - 1).unwrap();
        store_cmd::with_store(&app, |store| {
            store
                .record_probe_charge(at, spent)
                .map_err(|err| err.to_string())
        })
        .unwrap();
        let panel = view(&app, "store").unwrap();
        assert_eq!(panel.budget_decision, "stop");
        assert_eq!(panel.launch, ProbeLaunch::BlockedBudget.as_str());
        assert!(panel.sends_http);
        let (year, month) = budget_month(at).unwrap();
        assert_eq!(
            panel.over_budget_prompt.unwrap(),
            over_budget_label(spent, estimate, year, month).unwrap()
        );
    }

    #[test]
    fn spend_below_the_budget_and_outside_the_month_does_not_stop() {
        let app = app();
        set_consent(&app, true, "store").unwrap();
        store_cmd::with_store(&app, |store| {
            store
                .record_probe_charge(now_ms().unwrap(), MicroYuan::from_yuan(1.0).unwrap())
                .map_err(|err| err.to_string())?;
            store
                .record_probe_charge(0, MicroYuan::from_yuan(PROBE_MONTHLY_BUDGET_YUAN).unwrap())
                .map_err(|err| err.to_string())
        })
        .unwrap();
        let panel = view(&app, "store").unwrap();
        assert_eq!(panel.budget_decision, "allow");
        assert!(panel.over_budget_prompt.is_none());
        assert_eq!(panel.launch, ProbeLaunch::Ready.as_str());
        let estimate = typical_probe_estimate().unwrap();
        assert!(estimate.micros() > 0);
        assert_eq!(
            panel.estimate_label,
            format!("典型计划估价 {} 元", estimate.format_yuan())
        );
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
        assert!(panel.sends_http);
    }

    #[test]
    fn a_stored_arm_is_a_run_target_and_the_fixture_is_not() {
        let app = app();
        store_cmd::save_arm(
            &app,
            store_cmd::ArmInput {
                model: "probe-model".into(),
                provider_kind: "relay".into(),
                host: "relay.example".into(),
                protocol: "openai-chat-completions".into(),
                client: "probe-run".into(),
            },
        )
        .unwrap();
        let panel = view(&app, "store").unwrap();
        assert!(panel.sends_http);
        assert!(panel.arms.is_empty());
        let arm = Arm::new(
            ModelId::new("probe-model").unwrap(),
            Route::new(
                ProviderKind::Relay,
                "relay.example",
                Protocol::OpenAiChatCompletions,
                "probe-run",
            )
            .unwrap(),
        );
        assert_eq!(
            panel.run_targets,
            vec![ProbeRunTarget {
                id: arm.to_string(),
            }]
        );
        let fixture = view(&app, "fixture").unwrap();
        assert!(fixture.run_targets.is_empty());
        assert!(fixture.sends_http);
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
        assert!(panel
            .run_targets
            .iter()
            .any(|target| target.id == arm.to_string()));
    }

    #[test]
    fn unknown_source_is_rejected() {
        let err = view(&app(), "live").unwrap_err();
        assert!(err.contains("unknown probe source"));
    }

    fn relay_id() -> String {
        Arm::new(
            ModelId::new("probe-model").unwrap(),
            Route::new(
                ProviderKind::Relay,
                "127.0.0.1",
                Protocol::OpenAiChatCompletions,
                "probe-run",
            )
            .unwrap(),
        )
        .to_string()
    }

    fn save_local(app: &AppState) -> String {
        store_cmd::save_arm(
            app,
            store_cmd::ArmInput {
                model: "probe-model".into(),
                provider_kind: "relay".into(),
                host: "127.0.0.1".into(),
                protocol: "openai-chat-completions".into(),
                client: "probe-run".into(),
            },
        )
        .unwrap();
        relay_id()
    }

    fn enable_input(arm_id: &str, key_env: &str, base_url: &str) -> ScheduleInput {
        ScheduleInput {
            enabled: true,
            arm_id: arm_id.to_string(),
            key_env: key_env.to_string(),
            base_url: base_url.to_string(),
        }
    }

    #[test]
    fn enabling_without_consent_does_not_arm_or_send() {
        let app = app();
        let arm_id = save_local(&app);
        let panel = set_schedule_at(
            &app,
            &enable_input(&arm_id, "RELAY_API_KEY", "http://127.0.0.1:9"),
            OCTOBER,
        )
        .unwrap();
        assert!(!panel.schedule_active);
        assert!(panel.next_planned_label.is_none());
        assert_eq!(
            panel.schedule_status_label.as_deref(),
            Some(SCHEDULE_NO_CONSENT_LABEL)
        );
        assert!(panel.sends_http);
        assert_eq!(panel.launch, ProbeLaunch::BlockedConsent.as_str());
        let report = scheduled_tick(&app, OCTOBER + PROBE_SCHEDULE_INTERVAL_MS * 4);
        assert!(!report.ran);
        assert!(charges_of(&app).is_empty());
        assert!(results_of(&app).is_empty());
    }

    #[test]
    fn an_armed_schedule_still_does_not_tick_without_consent() {
        let app = app();
        app.schedule.enable(
            0,
            ScheduleBinding {
                arm_id: relay_id(),
                key_env: "RELAY_API_KEY".into(),
                base_url: "http://127.0.0.1:9".into(),
            },
        );
        let report = scheduled_tick(&app, PROBE_SCHEDULE_INTERVAL_MS);
        assert!(!report.ran);
        assert_eq!(report.class, Some(TickClass::Consent));
        assert!(!app.schedule.snapshot().active);
        assert!(charges_of(&app).is_empty());
        assert!(results_of(&app).is_empty());
    }

    #[test]
    fn enabling_with_consent_shows_the_next_plan_and_does_not_send() {
        let app = app();
        set_consent(&app, true, "store").unwrap();
        let arm_id = save_local(&app);
        let now = now_ms().unwrap();
        let panel = set_schedule_at(
            &app,
            &enable_input(&arm_id, "RELAY_API_KEY", "http://127.0.0.1:9"),
            now,
        )
        .unwrap();
        assert!(panel.schedule_active);
        assert!(panel.sends_http);
        let label = panel.next_planned_label.unwrap();
        assert!(label.starts_with("下次计划："));
        assert!(label.contains("暂定"));
        assert!(label.contains("错过不补跑"));
        assert!(!label.contains("已校准"));
        assert!(panel.schedule_status_label.is_none());
        assert_eq!(
            app.flight.launch(true, BudgetDecision::Allow),
            ProbeLaunch::Ready
        );
        assert!(charges_of(&app).is_empty());
        assert!(results_of(&app).is_empty());
        let early = scheduled_tick(&app, now);
        assert!(!early.ran);
        assert!(charges_of(&app).is_empty());
    }

    #[test]
    fn a_scheduled_tick_stops_after_the_budget_gate_and_does_not_send_again() {
        let app = app();
        set_consent(&app, true, "store").unwrap();
        let arm_id = save_local(&app);
        let now = now_ms().unwrap();
        set_schedule_at(
            &app,
            &enable_input(&arm_id, "RELAY_API_KEY", "http://127.0.0.1:9"),
            now,
        )
        .unwrap();
        let spent = MicroYuan::from_yuan(PROBE_MONTHLY_BUDGET_YUAN).unwrap();
        store_cmd::with_store(&app, |store| {
            store
                .record_probe_charge(now, spent)
                .map_err(|err| err.to_string())
        })
        .unwrap();
        let due = scheduled_tick(&app, now + PROBE_SCHEDULE_INTERVAL_MS);
        assert!(due.ran);
        assert_eq!(due.class, Some(TickClass::Budget));
        assert_eq!(charges_of(&app).len(), 1);
        assert!(results_of(&app).is_empty());
        let panel = view(&app, "store").unwrap();
        assert!(!panel.schedule_active);
        assert!(panel.next_planned_label.is_none());
        assert_eq!(
            panel.schedule_status_label.as_deref(),
            Some(SCHEDULE_BUDGET_STOP_LABEL)
        );
        assert!(panel.sends_http);
        assert_eq!(panel.launch, ProbeLaunch::BlockedBudget.as_str());
        let again = scheduled_tick(&app, now + PROBE_SCHEDULE_INTERVAL_MS * 5);
        assert!(!again.ran);
        assert_eq!(charges_of(&app).len(), 1);
        assert!(results_of(&app).is_empty());
    }

    #[test]
    fn a_mismatched_host_is_not_scheduled_and_the_detail_does_not_echo_it() {
        let app = app();
        set_consent(&app, true, "store").unwrap();
        let arm_id = save_local(&app);
        let submitted = "https://evil.example/probe-path";
        let panel = set_schedule_at(
            &app,
            &enable_input(&arm_id, "RELAY_API_KEY", submitted),
            OCTOBER,
        )
        .unwrap();
        assert!(!panel.schedule_active);
        assert_eq!(
            panel.schedule_status_label.as_deref(),
            Some("基址的主机必须和所选臂一致。")
        );
        let detail = panel.schedule_status_label.unwrap();
        assert!(!detail.contains(submitted));
        assert!(!detail.contains("evil.example"));
        assert!(!detail.contains("RELAY_API_KEY"));
        assert!(charges_of(&app).is_empty());
        let report = scheduled_tick(&app, OCTOBER + PROBE_SCHEDULE_INTERVAL_MS * 3);
        assert!(!report.ran);
    }

    #[test]
    fn a_secret_typed_as_the_variable_name_is_not_scheduled_or_echoed() {
        let app = app();
        set_consent(&app, true, "store").unwrap();
        let arm_id = save_local(&app);
        let secret = "sk-test-Ab1cdEf2Gh3ijKl4Mn5";
        let panel = set_schedule_at(
            &app,
            &enable_input(&arm_id, secret, "http://127.0.0.1:9"),
            OCTOBER,
        )
        .unwrap();
        assert!(!panel.schedule_active);
        let detail = panel.schedule_status_label.unwrap();
        assert!(!detail.contains(secret));
        assert!(!detail.to_lowercase().contains("sk-"));
        assert!(charges_of(&app).is_empty());
    }

    fn charges_of(app: &AppState) -> Vec<(i64, i64)> {
        store_cmd::with_store(app, |store| {
            store.probe_charges().map_err(|err| err.to_string())
        })
        .unwrap()
    }

    fn results_of(app: &AppState) -> Vec<ProbeResult> {
        store_cmd::with_store(app, |store| {
            store.list_probe_results().map_err(|err| err.to_string())
        })
        .unwrap()
    }
}

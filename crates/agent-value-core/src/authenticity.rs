//! Authenticity grade, gate and log-integrity gate per arm and window
//! (v22 §11.3, §11.3′, §11.4, §5 rule 4, §9 probe budget; DECISIONS 13.3,
//! 13.7–13.15).
//!
//! Grades only say how strong the evidence of consistency with the
//! baseline is; none of them means "proven fake" (§11 preamble). The grade
//! is a gate only: it never enters `Q*` or any score (§11.5).

use std::collections::{BTreeMap, BTreeSet};

use crate::arm::{Arm, ModelId, Protocol, ProviderKind, Route};
use crate::constants::PROBE_MONTHLY_BUDGET_YUAN;
use crate::eligibility::AuthenticityGrade;
use crate::error::{CoreError, Result};
use crate::probe::VendorTables;
use crate::records::{MicroYuan, ProbeClass, ProbeResult, ProbeVerdict};
use crate::runday::RunDay;

/// §11.3 "≥2 run days".
pub const GRADE_RUN_DAYS: usize = 2;
/// §11.3 observation period: 2 run days of positive and negative controls.
pub const OBSERVATION_RUN_DAYS: usize = 2;
/// Shown beside a grade band while the observation period is still pending.
/// The band is display-only until the controls validate the thresholds.
pub const GRADE_REFERENCE_ONLY_LABEL: &str = "仅供参考";
/// §11.3′ "model missing > 0.2" flag, as the integer test 5·M > P.
const MISSING_FLAG_DEN: u64 = 5;
/// §11.4 pairing window: ±1 hour.
pub const OVERHEAD_WINDOW_MS: i64 = 3_600_000;

/// Day-level verdicts of one arm in one window (DECISIONS 13.3).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DaySummary {
    /// (class, run day) → merged verdict.
    pub verdicts: BTreeMap<(ProbeClassKey, RunDay), ProbeVerdict>,
    /// Run days with a hard failure (probe runs or passive request logs).
    pub hard_failure_days: BTreeSet<RunDay>,
}

/// `ProbeClass` as an ordered map key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProbeClassKey {
    Deterministic,
    Fingerprint,
    Capability,
}

impl From<ProbeClass> for ProbeClassKey {
    fn from(c: ProbeClass) -> Self {
        match c {
            ProbeClass::Deterministic => Self::Deterministic,
            ProbeClass::Fingerprint => Self::Fingerprint,
            ProbeClass::Capability => Self::Capability,
        }
    }
}

fn merge(a: ProbeVerdict, b: ProbeVerdict) -> ProbeVerdict {
    use ProbeVerdict::*;
    match (a, b) {
        (Fail, _) | (_, Fail) => Fail,
        (Pass, _) | (_, Pass) => Pass,
        (Uncertain, _) | (_, Uncertain) => Uncertain,
        (NotApplicable, NotApplicable) => NotApplicable,
    }
}

impl DaySummary {
    /// Merges the probe results of one arm in one window, plus the run days
    /// on which passive request logs showed a hard failure (another model
    /// family in the returned `model`, §11.3′).
    pub fn build(arm: &Arm, results: &[ProbeResult], passive_hard_days: &[RunDay]) -> Result<Self> {
        let mut s = Self::default();
        for r in results {
            r.validate()?;
            if &r.arm != arm {
                return Err(CoreError::InvalidField {
                    field: "probe result",
                    reason: "belongs to another arm",
                });
            }
            let day = r.run_day()?;
            let e = s.verdicts.entry((r.class.into(), day)).or_insert(r.verdict);
            *e = merge(*e, r.verdict);
            if r.hard_failure {
                s.hard_failure_days.insert(day);
            }
        }
        s.hard_failure_days
            .extend(passive_hard_days.iter().copied());
        Ok(s)
    }

    fn days(&self, class: ProbeClassKey, f: impl Fn(ProbeVerdict) -> bool) -> usize {
        self.verdicts
            .iter()
            .filter(|((c, _), v)| *c == class && f(**v))
            .count()
    }

    /// Run days on which the class passed. For the capability class a
    /// not-applicable day counts as passed (DECISIONS 13.5).
    pub fn pass_days(&self, class: ProbeClassKey) -> usize {
        self.days(class, |v| {
            v == ProbeVerdict::Pass
                || (class == ProbeClassKey::Capability && v == ProbeVerdict::NotApplicable)
        })
    }

    pub fn fail_days(&self, class: ProbeClassKey) -> usize {
        self.days(class, |v| v == ProbeVerdict::Fail)
    }

    pub fn ran(&self, class: ProbeClassKey) -> bool {
        self.days(class, |_| true) > 0
    }
}

/// What an arm is, before any gate decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthStatus {
    /// §11.3 rule 1: the route host is the model vendor's official host.
    OfficialDirect,
    Graded(AuthenticityGrade),
}

/// §11.3 rules 2–5 (first match wins) for a non-official arm.
///
/// A− fingerprint is monotonic (DECISIONS 13.13): it qualifies unless
/// there are zero pass days and at least one fail day. Not run, only
/// uncertain, or at least one pass day all qualify. Other A− guards: no
/// hard-failure day, and deterministic and capability each on at least
/// one pass day.
///
/// Capability fail days cap both A and A− (DECISIONS 13.15): `cf ≤ 1`.
/// If `cf ≥ 2` and the arm is not already C, the grade is at most B.
/// Capability fails do not count toward C.
pub fn grade(s: &DaySummary) -> AuthenticityGrade {
    use ProbeClassKey::*;
    let hard = s.hard_failure_days.len();
    if hard >= GRADE_RUN_DAYS
        || s.fail_days(Deterministic) >= GRADE_RUN_DAYS
        || s.fail_days(Fingerprint) >= GRADE_RUN_DAYS
    {
        return AuthenticityGrade::C;
    }
    // cf does not count toward C. Two or more capability fail days cap
    // both A and A− at B (DECISIONS 13.15).
    let cf = s.fail_days(Capability);
    if hard == 0
        && cf <= 1
        && s.pass_days(Deterministic) >= GRADE_RUN_DAYS
        && s.pass_days(Fingerprint) >= GRADE_RUN_DAYS
        && s.pass_days(Capability) >= GRADE_RUN_DAYS
    {
        return AuthenticityGrade::A;
    }
    // ¬(pass days = 0 ∧ fail days ≥ 1). Stronger fingerprint evidence
    // must not drop A− to B (DECISIONS 13.13).
    let fp_ok = !(s.pass_days(Fingerprint) == 0 && s.fail_days(Fingerprint) >= 1);
    if hard == 0
        && cf <= 1
        && s.pass_days(Deterministic) >= 1
        && s.pass_days(Capability) >= 1
        && fp_ok
    {
        return AuthenticityGrade::AMinus;
    }
    AuthenticityGrade::B
}

/// Official direct (DECISIONS 13.7): the route host is in the official
/// table and its vendor is the claimed model's vendor. Both sides are the
/// trimmed, lower-cased table values, so a caller-supplied spelling such
/// as `"Anthropic"` still matches.
pub fn is_official_direct(tables: &VendorTables, arm: &Arm) -> bool {
    match (
        tables.official_vendor(arm.route().host()),
        tables.vendor(arm.model().as_str()),
    ) {
        (Some(h), Some(m)) => h == m,
        _ => false,
    }
}

/// §11.3 observation period state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observation {
    /// Thresholds not validated yet: grades are shown only, never a gate.
    Pending,
    Validated,
}

impl Observation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Validated => "validated",
        }
    }

    /// Copy for the panel. The run-day count is [`OBSERVATION_RUN_DAYS`].
    pub fn label(self) -> String {
        match self {
            Self::Pending => format!(
                "观察期未校验：正对照与负对照各 {OBSERVATION_RUN_DAYS} 个运行日。档位{GRADE_REFERENCE_ONLY_LABEL}。"
            ),
            Self::Validated => {
                format!("观察期已校验：正对照与负对照各满 {OBSERVATION_RUN_DAYS} 个运行日。")
            }
        }
    }
}

/// A graded band is reference-only until the observation period is validated.
/// Official direct is not a grade band.
pub fn grade_reference_only(observation: Observation, status: AuthStatus) -> bool {
    observation == Observation::Pending && matches!(status, AuthStatus::Graded(_))
}

/// How many run days A and C require. The count is [`GRADE_RUN_DAYS`].
pub fn grade_run_days_label() -> String {
    format!("档位运行日：A 与 C 至少 {GRADE_RUN_DAYS} 个运行日")
}

/// Observation-period check (DECISIONS 13.8). `positive`: the official
/// direct route of the model; `negative`: a smaller model presented as the
/// tested model, compared with the tested model's baseline.
pub fn observation(
    positive: &DaySummary,
    negative: &DaySummary,
    positive_has_baseline: bool,
    negative_has_baseline: bool,
) -> Observation {
    use ProbeClassKey::*;
    let pos = positive.hard_failure_days.is_empty()
        && positive.pass_days(Deterministic) >= OBSERVATION_RUN_DAYS
        && positive.pass_days(Capability) >= OBSERVATION_RUN_DAYS
        && (!positive_has_baseline || positive.pass_days(Fingerprint) >= OBSERVATION_RUN_DAYS);
    let neg = negative_has_baseline && negative.fail_days(Fingerprint) >= OBSERVATION_RUN_DAYS;
    if pos && neg {
        Observation::Validated
    } else {
        Observation::Pending
    }
}

/// Which arm of an observation window a day predicate is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationArmRole {
    Positive,
    Negative,
}

/// Probe id and version frozen onto one observation window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationProbeRef {
    pub probe_id: String,
    pub probe_version: String,
}

/// The three probe identities a window's day predicates read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationProbeSet {
    pub deterministic: ObservationProbeRef,
    pub fingerprint: ObservationProbeRef,
    pub capability: ObservationProbeRef,
}

/// Baseline identity frozen when the window was preregistered.
///
/// Both fields are 64 lower-case hex digits. A fingerprint row matches only
/// when key and content are both equal to these values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrozenObservationBaseline {
    pub baseline_key: String,
    pub content_sha256: String,
}

/// One probe row already restricted to a single arm and Beijing day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationDayRow {
    pub probe_id: String,
    pub probe_version: String,
    pub class: ProbeClass,
    pub verdict: ProbeVerdict,
    pub hard_failure: bool,
    pub baseline_key: Option<String>,
    pub baseline_content_sha256: Option<String>,
}

/// Full-day predicate for one arm (plan N=1).
///
/// `deterministic` / `capability` / `fingerprint` are `None` when that
/// preregistered probe has no row. An explicit `not-applicable` verdict is
/// `Some`, not a missing row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationDayVerdict {
    pub ok: bool,
    pub hard: bool,
    pub polluted: bool,
    pub deterministic: Option<ProbeVerdict>,
    pub capability: Option<ProbeVerdict>,
    pub fingerprint: Option<ProbeVerdict>,
}

impl ObservationDayVerdict {
    fn line(&self, label: &str) -> String {
        format!(
            "{label}: ok={} hard={} polluted={} det={} cap={} fp={}",
            u8::from(self.ok),
            u8::from(self.hard),
            u8::from(self.polluted),
            verdict_token(self.deterministic),
            verdict_token(self.capability),
            verdict_token(self.fingerprint),
        )
    }
}

fn verdict_token(verdict: Option<ProbeVerdict>) -> &'static str {
    match verdict {
        None => "-",
        Some(v) => v.as_str(),
    }
}

/// Both Beijing days of one window, after the window has ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationWindowVerdict {
    pub start_positive: ObservationDayVerdict,
    pub start_negative: ObservationDayVerdict,
    pub end_positive: ObservationDayVerdict,
    pub end_negative: ObservationDayVerdict,
    pub window_ok: bool,
}

impl ObservationWindowVerdict {
    /// Frozen text stored with the final snapshot.
    pub fn report(&self) -> String {
        [
            self.start_positive.line("start_positive"),
            self.start_negative.line("start_negative"),
            self.end_positive.line("end_positive"),
            self.end_negative.line("end_negative"),
            format!("window_ok={}", u8::from(self.window_ok)),
        ]
        .join("\n")
    }
}

fn predicate_arm() -> Arm {
    Arm::new(
        ModelId::new("observation-day").expect("static model id"),
        Route::new(
            ProviderKind::Official,
            "observation.example",
            Protocol::OpenAiChatCompletions,
            "observation",
        )
        .expect("static route"),
    )
}

fn row_matches(row: &ObservationDayRow, probes: &ObservationProbeSet) -> bool {
    let expected = match row.class {
        ProbeClass::Deterministic => &probes.deterministic,
        ProbeClass::Fingerprint => &probes.fingerprint,
        ProbeClass::Capability => &probes.capability,
    };
    row.probe_id == expected.probe_id && row.probe_version == expected.probe_version
}

fn fields_match(row: &ObservationDayRow, frozen: &FrozenObservationBaseline) -> bool {
    row.baseline_key.as_deref() == Some(frozen.baseline_key.as_str())
        && row.baseline_content_sha256.as_deref() == Some(frozen.content_sha256.as_str())
}

/// Day predicate for one arm on one Beijing day.
///
/// Rows are merged with [`DaySummary::build`] (Fail beats Pass beats
/// uncertain beats not-applicable; any hard failure makes the day hard).
/// Only rows whose probe id and version match `probes` count.
///
/// Positive: not hard, deterministic pass, capability pass or
/// not-applicable, fingerprint pass, and all three classes present.
/// A fingerprint row whose key or content differs from `frozen` fails the day.
///
/// Negative: not hard, fingerprint fail, and that row's key and content
/// match `frozen`. Deterministic and capability rows are not required.
///
/// This is not [`observation`]. That helper counts pass days and does not
/// read frozen baselines.
pub fn observation_day_verdict(
    rows: &[ObservationDayRow],
    probes: &ObservationProbeSet,
    role: ObservationArmRole,
    frozen: &FrozenObservationBaseline,
    day_ms: i64,
) -> Result<ObservationDayVerdict> {
    let counted: Vec<&ObservationDayRow> =
        rows.iter().filter(|row| row_matches(row, probes)).collect();
    let polluted = counted
        .iter()
        .any(|row| row.class == ProbeClass::Fingerprint && !fields_match(row, frozen));
    let arm = predicate_arm();
    let results = counted
        .iter()
        .map(|row| ProbeResult {
            probe_id: row.probe_id.clone(),
            probe_version: row.probe_version.clone(),
            arm: arm.clone(),
            at_ms: day_ms,
            class: row.class,
            verdict: row.verdict,
            hard_failure: row.hard_failure,
            evidence_summary: String::new(),
        })
        .collect::<Vec<_>>();
    let summary = DaySummary::build(&arm, &results, &[])?;
    let day = RunDay::from_unix_ms(day_ms)?;
    let class_verdict = |class: ProbeClassKey| summary.verdicts.get(&(class, day)).copied();
    let deterministic = class_verdict(ProbeClassKey::Deterministic);
    let capability = class_verdict(ProbeClassKey::Capability);
    let fingerprint = class_verdict(ProbeClassKey::Fingerprint);
    let hard = summary.hard_failure_days.contains(&day);
    let ok = match role {
        ObservationArmRole::Positive => {
            !hard
                && !polluted
                && deterministic == Some(ProbeVerdict::Pass)
                && matches!(
                    capability,
                    Some(ProbeVerdict::Pass) | Some(ProbeVerdict::NotApplicable)
                )
                && fingerprint == Some(ProbeVerdict::Pass)
        }
        ObservationArmRole::Negative => {
            !hard && !polluted && fingerprint == Some(ProbeVerdict::Fail)
        }
    };
    Ok(ObservationDayVerdict {
        ok,
        hard,
        polluted,
        deterministic,
        capability,
        fingerprint,
    })
}

/// The four arm-days of one preregistered window, plus the frozen probe set.
pub struct ObservationWindowInput<'a> {
    pub start_positive: &'a [ObservationDayRow],
    pub start_negative: &'a [ObservationDayRow],
    pub end_positive: &'a [ObservationDayRow],
    pub end_negative: &'a [ObservationDayRow],
    pub probes: &'a ObservationProbeSet,
    pub frozen: &'a FrozenObservationBaseline,
    pub start_ms: i64,
    pub end_ms: i64,
}

/// `window_ok` when both days pass for the positive arm and the negative arm.
pub fn observation_window_verdict(
    input: &ObservationWindowInput<'_>,
) -> Result<ObservationWindowVerdict> {
    let start_positive = observation_day_verdict(
        input.start_positive,
        input.probes,
        ObservationArmRole::Positive,
        input.frozen,
        input.start_ms,
    )?;
    let start_negative = observation_day_verdict(
        input.start_negative,
        input.probes,
        ObservationArmRole::Negative,
        input.frozen,
        input.start_ms,
    )?;
    let end_positive = observation_day_verdict(
        input.end_positive,
        input.probes,
        ObservationArmRole::Positive,
        input.frozen,
        input.end_ms,
    )?;
    let end_negative = observation_day_verdict(
        input.end_negative,
        input.probes,
        ObservationArmRole::Negative,
        input.frozen,
        input.end_ms,
    )?;
    let window_ok = start_positive.ok && start_negative.ok && end_positive.ok && end_negative.ok;
    Ok(ObservationWindowVerdict {
        start_positive,
        start_negative,
        end_positive,
        end_negative,
        window_ok,
    })
}

/// Display labels (§5 rule 4, §11.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AuthLabel {
    /// "官方直连".
    OfficialDirect,
    /// "官方路线异常": a hard failure on an official route; no downgrade.
    OfficialRouteAnomaly,
    /// "与基线不一致的嫌疑" (C).
    SuspectedInconsistent,
    /// "未验证" (B with probes on; the minimal probe has not passed yet).
    Unverified,
    /// "未验证（探针已关闭）".
    UnverifiedProbesOff,
    /// Observation period: the grade is shown only (DECISIONS 13.8).
    ObservationOnly,
}

impl AuthLabel {
    /// The display phrase from §5 rule 4 and §11.3. The shell shows this
    /// string; it does not invent another one.
    pub fn zh(self) -> &'static str {
        match self {
            Self::OfficialDirect => "官方直连",
            Self::OfficialRouteAnomaly => "官方路线异常",
            Self::SuspectedInconsistent => "与基线不一致的嫌疑",
            Self::Unverified => "未验证",
            Self::UnverifiedProbesOff => "未验证（探针已关闭）",
            Self::ObservationOnly => "真实性未定",
        }
    }
}

/// Gate context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateContext {
    /// The user allowed probes (§11.1 switch).
    pub probes_enabled: bool,
    pub observation: Observation,
}

/// Authenticity of one arm in one window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticityAssessment {
    pub status: AuthStatus,
    /// Passes §5 rule 4 (authenticity half).
    pub passes: bool,
    /// Must be shown as "authenticity undetermined".
    pub undetermined: bool,
    pub labels: Vec<AuthLabel>,
}

impl AuthenticityAssessment {
    /// Input for the step 8 gate interface (DECISIONS 13.11): only
    /// `passes()` and `undetermined()` of it are ever read. Display must use
    /// `status` and `labels`, never this value.
    pub fn gate_input(&self) -> AuthenticityGrade {
        match (self.passes, self.undetermined) {
            (true, true) => AuthenticityGrade::B,
            (true, false) => AuthenticityGrade::A,
            (false, _) => AuthenticityGrade::C,
        }
    }
}

/// Grade and gate for one arm in one window.
pub fn assess(
    tables: &VendorTables,
    arm: &Arm,
    results: &[ProbeResult],
    passive_hard_days: &[RunDay],
    ctx: GateContext,
) -> Result<AuthenticityAssessment> {
    let s = DaySummary::build(arm, results, passive_hard_days)?;
    if is_official_direct(tables, arm) {
        let mut labels = vec![AuthLabel::OfficialDirect];
        if !s.hard_failure_days.is_empty() {
            labels.push(AuthLabel::OfficialRouteAnomaly);
        }
        return Ok(AuthenticityAssessment {
            status: AuthStatus::OfficialDirect,
            passes: true,
            undetermined: false,
            labels,
        });
    }
    let g = grade(&s);
    let mut labels = Vec::new();
    match g {
        AuthenticityGrade::C => labels.push(AuthLabel::SuspectedInconsistent),
        AuthenticityGrade::B if ctx.probes_enabled => labels.push(AuthLabel::Unverified),
        AuthenticityGrade::B => labels.push(AuthLabel::UnverifiedProbesOff),
        _ => {}
    }
    let (passes, undetermined) = match ctx.observation {
        Observation::Pending => {
            labels.push(AuthLabel::ObservationOnly);
            (true, true)
        }
        Observation::Validated => match g {
            AuthenticityGrade::A | AuthenticityGrade::AMinus => (true, false),
            AuthenticityGrade::C => (false, false),
            AuthenticityGrade::B => (!ctx.probes_enabled, true),
        },
    };
    labels.sort();
    Ok(AuthenticityAssessment {
        status: AuthStatus::Graded(g),
        passes,
        undetermined,
        labels,
    })
}

// ------------------------------------------------------ log integrity

/// One logged request of the arm, as far as §11.3′ needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoggedModel {
    /// The log line could not be parsed.
    Unparseable,
    /// Parsed, no `model` field.
    Missing,
    Returned(String),
}

/// §11.3′ result (DECISIONS 13.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogIntegrity {
    pub passes: bool,
    pub requests: u64,
    pub parseable: u64,
    pub missing: u64,
    /// Other model family: counted as hard failures, not here.
    pub other_family: u64,
    pub version_mismatch: u64,
    /// "无法核对返回模型": missing share above 0.2. A flag, not a failure.
    pub cannot_verify_model: bool,
    /// No request in the window: nothing to check.
    pub no_requests: bool,
}

fn inc(x: &mut u64) -> Result<()> {
    *x = x.checked_add(1).ok_or(CoreError::Overflow {
        what: "log integrity count",
    })?;
    Ok(())
}

/// §11.3′ log-integrity gate over the arm's requests in the window.
pub fn log_integrity(
    tables: &VendorTables,
    arm: &Arm,
    requests: &[LoggedModel],
) -> Result<LogIntegrity> {
    let claimed = tables.normalize(arm.model().as_str());
    let mut li = LogIntegrity {
        passes: true,
        requests: 0,
        parseable: 0,
        missing: 0,
        other_family: 0,
        version_mismatch: 0,
        cannot_verify_model: false,
        no_requests: requests.is_empty(),
    };
    for r in requests {
        inc(&mut li.requests)?;
        match r {
            LoggedModel::Unparseable => {}
            LoggedModel::Missing => {
                inc(&mut li.parseable)?;
                inc(&mut li.missing)?;
            }
            LoggedModel::Returned(m) => {
                inc(&mut li.parseable)?;
                if crate::probe::other_family(tables, arm.model(), m) {
                    inc(&mut li.other_family)?;
                } else if tables.normalize(m) != claimed {
                    inc(&mut li.version_mismatch)?;
                }
            }
        }
    }
    let mul = |a: u64, k: u64| {
        a.checked_mul(k).ok_or(CoreError::Overflow {
            what: "log integrity ratio",
        })
    };
    let parse_ok = mul(li.parseable, 20)? >= mul(li.requests, 19)?;
    let checked = li
        .parseable
        .checked_sub(li.missing)
        .and_then(|x| x.checked_sub(li.other_family))
        .ok_or(CoreError::Overflow {
            what: "log integrity count",
        })?;
    let drift_ok = checked == 0 || mul(li.version_mismatch, 20)? <= checked;
    li.cannot_verify_model = mul(li.missing, MISSING_FLAG_DEN)? > li.parseable;
    li.passes = parse_ok && drift_ok;
    Ok(li)
}

// ------------------------------------------------------------ budget

/// §9 probe monthly budget decision (DECISIONS 13.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetDecision {
    Allow,
    /// Stop probing for the rest of this Beijing-time month.
    Stop,
}

/// Allows a probe run only if `spent + estimate <= budget`; stops once
/// `spent >= budget`.
pub fn probe_budget(
    spent: MicroYuan,
    estimate: MicroYuan,
    budget: MicroYuan,
) -> Result<BudgetDecision> {
    if spent.micros() >= budget.micros() {
        return Ok(BudgetDecision::Stop);
    }
    let total = spent
        .micros()
        .checked_add(estimate.micros())
        .ok_or(CoreError::Overflow {
            what: "probe budget",
        })?;
    Ok(if total <= budget.micros() {
        BudgetDecision::Allow
    } else {
        BudgetDecision::Stop
    })
}

/// Monthly probe budget as a display label. The amount is
/// [`PROBE_MONTHLY_BUDGET_YUAN`].
pub fn monthly_budget_label() -> Result<String> {
    let amount = MicroYuan::from_yuan(PROBE_MONTHLY_BUDGET_YUAN)?;
    Ok(format!(
        "探针月预算 {} 元（北京时间自然月）",
        amount.format_yuan()
    ))
}

/// Shown when [`probe_budget`] returns [`BudgetDecision::Stop`].
/// The three amounts are the spent total, the next-run estimate, and
/// [`PROBE_MONTHLY_BUDGET_YUAN`]. `year` and `month` are a Beijing-time
/// calendar month from [`budget_month`].
pub fn over_budget_label(
    spent: MicroYuan,
    estimate: MicroYuan,
    year: i32,
    month: u8,
) -> Result<String> {
    let budget = MicroYuan::from_yuan(PROBE_MONTHLY_BUDGET_YUAN)?;
    Ok(format!(
        "已花 {} 元，这次估价 {} 元，月预算 {} 元（北京时间 {year}-{month:02}）。本月停止探测。",
        spent.format_yuan(),
        estimate.format_yuan(),
        budget.format_yuan()
    ))
}

/// Beijing-time calendar month (year, month) of a timestamp, the budget period.
pub fn budget_month(at_ms: i64) -> Result<(i32, u8)> {
    let d = RunDay::from_unix_ms(at_ms)?;
    Ok((d.year(), d.month()))
}

// ------------------------------------------------------------ overhead

/// One timed request of a probe (§11.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimingSample {
    pub start_ms: i64,
    pub first_token_ms: Option<i64>,
    pub end_ms: i64,
    pub output_tokens: u64,
}

/// §11.4 route overhead: route median − official median. Diagnostic only.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Overhead {
    pub ttft_ms: Option<f64>,
    pub total_ms: Option<f64>,
    /// Output tokens per second.
    pub speed_tps: Option<f64>,
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    })
}

fn check_sample(s: &TimingSample) -> Result<()> {
    let ok = s.end_ms >= s.start_ms
        && s.first_token_ms
            .is_none_or(|f| f >= s.start_ms && f <= s.end_ms);
    if ok {
        Ok(())
    } else {
        Err(CoreError::InvalidField {
            field: "timing sample",
            reason: "times must be ordered start <= first token <= end",
        })
    }
}

/// Same model, same probe. Official samples count only when within ±1 hour
/// (inclusive) of some route sample (DECISIONS 13.12).
pub fn route_overhead(route: &[TimingSample], official: &[TimingSample]) -> Result<Overhead> {
    for s in route.iter().chain(official) {
        check_sample(s)?;
    }
    let near: Vec<&TimingSample> = official
        .iter()
        .filter(|o| {
            route.iter().any(|r| {
                r.start_ms
                    .checked_sub(o.start_ms)
                    .is_some_and(|d| d.unsigned_abs() <= OVERHEAD_WINDOW_MS.unsigned_abs())
            })
        })
        .collect();
    // Differences of ordered timestamps; checked so that extreme inputs
    // cannot wrap (they are dropped instead, and validated above anyway).
    let diff = |a: i64, b: i64| a.checked_sub(b).map(|d| d as f64);
    let ttft = |xs: &[&TimingSample]| {
        median(
            xs.iter()
                .filter_map(|s| diff(s.first_token_ms?, s.start_ms))
                .collect(),
        )
    };
    let total = |xs: &[&TimingSample]| {
        median(
            xs.iter()
                .filter_map(|s| diff(s.end_ms, s.start_ms))
                .collect(),
        )
    };
    let speed = |xs: &[&TimingSample]| {
        median(
            xs.iter()
                .filter_map(|s| {
                    let gen = diff(s.end_ms, s.first_token_ms?)?;
                    (gen > 0.0).then(|| s.output_tokens as f64 * 1000.0 / gen)
                })
                .collect(),
        )
    };
    let r: Vec<&TimingSample> = route.iter().collect();
    let d = |a: Option<f64>, b: Option<f64>| Some(a? - b?);
    Ok(Overhead {
        ttft_ms: d(ttft(&r), ttft(&near)),
        total_ms: d(total(&r), total(&near)),
        speed_tps: d(speed(&r), speed(&near)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observation_copy_uses_the_run_day_constant() {
        assert_eq!(
            Observation::Pending.label(),
            format!(
                "观察期未校验：正对照与负对照各 {OBSERVATION_RUN_DAYS} 个运行日。档位{GRADE_REFERENCE_ONLY_LABEL}。"
            )
        );
        assert_eq!(
            Observation::Validated.label(),
            format!("观察期已校验：正对照与负对照各满 {OBSERVATION_RUN_DAYS} 个运行日。")
        );
        assert!(!Observation::Validated
            .label()
            .contains(GRADE_REFERENCE_ONLY_LABEL));
        assert_eq!(
            grade_run_days_label(),
            format!("档位运行日：A 与 C 至少 {GRADE_RUN_DAYS} 个运行日")
        );
    }

    #[test]
    fn reference_only_applies_to_pending_grades() {
        assert!(grade_reference_only(
            Observation::Pending,
            AuthStatus::Graded(AuthenticityGrade::B)
        ));
        assert!(grade_reference_only(
            Observation::Pending,
            AuthStatus::Graded(AuthenticityGrade::A)
        ));
        assert!(!grade_reference_only(
            Observation::Pending,
            AuthStatus::OfficialDirect
        ));
        assert!(!grade_reference_only(
            Observation::Validated,
            AuthStatus::Graded(AuthenticityGrade::C)
        ));
    }

    #[test]
    fn auth_labels_match_the_spec_phrases() {
        assert_eq!(AuthLabel::OfficialDirect.zh(), "官方直连");
        assert_eq!(AuthLabel::OfficialRouteAnomaly.zh(), "官方路线异常");
        assert_eq!(AuthLabel::SuspectedInconsistent.zh(), "与基线不一致的嫌疑");
        assert_eq!(AuthLabel::Unverified.zh(), "未验证");
        assert_eq!(AuthLabel::UnverifiedProbesOff.zh(), "未验证（探针已关闭）");
        assert_eq!(AuthLabel::ObservationOnly.zh(), "真实性未定");
    }

    #[test]
    fn budget_labels_use_the_monthly_constant() {
        let budget = MicroYuan::from_yuan(PROBE_MONTHLY_BUDGET_YUAN).unwrap();
        assert_eq!(
            monthly_budget_label().unwrap(),
            format!("探针月预算 {} 元（北京时间自然月）", budget.format_yuan())
        );
        let spent = MicroYuan::new(9_990_000).unwrap();
        let estimate = MicroYuan::from_yuan(0.02).unwrap();
        let text = over_budget_label(spent, estimate, 2026, 10).unwrap();
        assert_eq!(
            text,
            format!(
                "已花 {} 元，这次估价 {} 元，月预算 {} 元（北京时间 2026-10）。本月停止探测。",
                spent.format_yuan(),
                estimate.format_yuan(),
                budget.format_yuan()
            )
        );
    }

    fn probes() -> ObservationProbeSet {
        let r = |id: &str| ObservationProbeRef {
            probe_id: id.into(),
            probe_version: "1".into(),
        };
        ObservationProbeSet {
            deterministic: r("det"),
            fingerprint: r("fp"),
            capability: r("cap"),
        }
    }

    fn frozen() -> FrozenObservationBaseline {
        FrozenObservationBaseline {
            baseline_key: "ab".repeat(32),
            content_sha256: "cd".repeat(32),
        }
    }

    fn row(
        class: ProbeClass,
        verdict: ProbeVerdict,
        hard: bool,
        key: Option<&str>,
        content: Option<&str>,
    ) -> ObservationDayRow {
        let id = match class {
            ProbeClass::Deterministic => "det",
            ProbeClass::Fingerprint => "fp",
            ProbeClass::Capability => "cap",
        };
        ObservationDayRow {
            probe_id: id.into(),
            probe_version: "1".into(),
            class,
            verdict,
            hard_failure: hard,
            baseline_key: key.map(str::to_string),
            baseline_content_sha256: content.map(str::to_string),
        }
    }

    fn positive_day() -> Vec<ObservationDayRow> {
        let base = frozen();
        vec![
            row(
                ProbeClass::Deterministic,
                ProbeVerdict::Pass,
                false,
                None,
                None,
            ),
            row(
                ProbeClass::Capability,
                ProbeVerdict::NotApplicable,
                false,
                None,
                None,
            ),
            row(
                ProbeClass::Fingerprint,
                ProbeVerdict::Pass,
                false,
                Some(&base.baseline_key),
                Some(&base.content_sha256),
            ),
        ]
    }

    fn negative_day() -> Vec<ObservationDayRow> {
        let base = frozen();
        vec![row(
            ProbeClass::Fingerprint,
            ProbeVerdict::Fail,
            false,
            Some(&base.baseline_key),
            Some(&base.content_sha256),
        )]
    }

    #[test]
    fn day_predicate_requires_each_class_and_treats_not_applicable_as_present() {
        assert_eq!(OBSERVATION_RUN_DAYS, 2);
        let day = 1_792_425_600_000;
        let ok = observation_day_verdict(
            &positive_day(),
            &probes(),
            ObservationArmRole::Positive,
            &frozen(),
            day,
        )
        .unwrap();
        assert!(ok.ok);
        assert_eq!(ok.capability, Some(ProbeVerdict::NotApplicable));

        let mut missing_cap = positive_day();
        missing_cap.retain(|row| row.class != ProbeClass::Capability);
        let missing = observation_day_verdict(
            &missing_cap,
            &probes(),
            ObservationArmRole::Positive,
            &frozen(),
            day,
        )
        .unwrap();
        assert!(!missing.ok);
        assert_eq!(missing.capability, None);
    }

    #[test]
    fn pollution_of_either_baseline_field_fails_the_day() {
        let day = 1_792_425_600_000;
        let base = frozen();
        for (key, content) in [
            (Some("ff".repeat(32)), Some(base.content_sha256.clone())),
            (Some(base.baseline_key.clone()), Some("ee".repeat(32))),
        ] {
            let mut rows = positive_day();
            rows[2].baseline_key = key;
            rows[2].baseline_content_sha256 = content;
            let verdict =
                observation_day_verdict(&rows, &probes(), ObservationArmRole::Positive, &base, day)
                    .unwrap();
            assert!(verdict.polluted, "{}", verdict.line("day"));
            assert!(!verdict.ok);
        }
    }

    #[test]
    fn negative_day_fails_on_hard_and_ignores_other_classes() {
        let day = 1_792_425_600_000;
        let ok = observation_day_verdict(
            &negative_day(),
            &probes(),
            ObservationArmRole::Negative,
            &frozen(),
            day,
        )
        .unwrap();
        assert!(ok.ok);
        assert_eq!(ok.deterministic, None);

        let mut hard = negative_day();
        hard[0].hard_failure = true;
        let verdict = observation_day_verdict(
            &hard,
            &probes(),
            ObservationArmRole::Negative,
            &frozen(),
            day,
        )
        .unwrap();
        assert!(verdict.hard);
        assert!(!verdict.ok);
    }

    #[test]
    fn legacy_observation_does_not_apply_the_frozen_baseline() {
        let arm = Arm::new(
            ModelId::new("example-model-1").unwrap(),
            Route::new(
                ProviderKind::Official,
                "api.vendor.example",
                Protocol::AnthropicMessages,
                "cli",
            )
            .unwrap(),
        );
        let mut positive = Vec::new();
        let mut negative = Vec::new();
        for offset in [0, 1] {
            let at = RunDay::parse("2026-10-20")
                .unwrap()
                .checked_add_days(offset)
                .unwrap()
                .beijing_midnight_unix_ms()
                .unwrap()
                + 3_600_000;
            for (class, verdict) in [
                (ProbeClass::Deterministic, ProbeVerdict::Pass),
                (ProbeClass::Capability, ProbeVerdict::Pass),
                (ProbeClass::Fingerprint, ProbeVerdict::Pass),
            ] {
                positive.push(ProbeResult {
                    probe_id: "p".into(),
                    probe_version: "1".into(),
                    arm: arm.clone(),
                    at_ms: at,
                    class,
                    verdict,
                    hard_failure: false,
                    evidence_summary: String::new(),
                });
            }
            negative.push(ProbeResult {
                probe_id: "p".into(),
                probe_version: "1".into(),
                arm: arm.clone(),
                at_ms: at,
                class: ProbeClass::Fingerprint,
                verdict: ProbeVerdict::Fail,
                hard_failure: false,
                evidence_summary: String::new(),
            });
        }
        let pos = DaySummary::build(&arm, &positive, &[]).unwrap();
        let neg = DaySummary::build(&arm, &negative, &[]).unwrap();
        assert_eq!(observation(&pos, &neg, true, true), Observation::Validated);

        let start = RunDay::parse("2026-10-20").unwrap();
        let start_ms = start.beijing_midnight_unix_ms().unwrap();
        let end_ms = start
            .checked_add_days(1)
            .unwrap()
            .beijing_midnight_unix_ms()
            .unwrap();
        let mut bad = positive_day();
        bad[2].baseline_content_sha256 = Some("11".repeat(32));
        let probes = probes();
        let frozen = frozen();
        let window = observation_window_verdict(&ObservationWindowInput {
            start_positive: &bad,
            start_negative: &negative_day(),
            end_positive: &positive_day(),
            end_negative: &negative_day(),
            probes: &probes,
            frozen: &frozen,
            start_ms,
            end_ms,
        })
        .unwrap();
        assert!(!window.window_ok);
        assert!(window.report().contains("window_ok=0"));
    }
}

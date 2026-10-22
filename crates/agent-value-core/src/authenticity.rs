//! Authenticity grade, gate and log-integrity gate per arm and window
//! (v22 §11.3, §11.3′, §11.4, §5 rule 4, §9 probe budget; DECISIONS 13.3,
//! 13.7–13.13).
//!
//! Grades only say how strong the evidence of consistency with the
//! baseline is; none of them means "proven fake" (§11 preamble). The grade
//! is a gate only: it never enters `Q*` or any score (§11.5).

use std::collections::{BTreeMap, BTreeSet};

use crate::arm::Arm;
use crate::eligibility::AuthenticityGrade;
use crate::error::{CoreError, Result};
use crate::probe::VendorTables;
use crate::records::{MicroYuan, ProbeClass, ProbeResult, ProbeVerdict};
use crate::runday::RunDay;

/// §11.3 "≥2 run days".
pub const GRADE_RUN_DAYS: usize = 2;
/// §11.3 observation period: 2 run days of positive and negative controls.
pub const OBSERVATION_RUN_DAYS: usize = 2;
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
/// uncertain, or at least one pass day all qualify. The other A− guards
/// are unchanged: no hard-failure day, and deterministic and capability
/// each on at least one pass day. A is unchanged.
pub fn grade(s: &DaySummary) -> AuthenticityGrade {
    use ProbeClassKey::*;
    let hard = s.hard_failure_days.len();
    if hard >= GRADE_RUN_DAYS
        || s.fail_days(Deterministic) >= GRADE_RUN_DAYS
        || s.fail_days(Fingerprint) >= GRADE_RUN_DAYS
    {
        return AuthenticityGrade::C;
    }
    if hard == 0
        && s.pass_days(Deterministic) >= GRADE_RUN_DAYS
        && s.pass_days(Fingerprint) >= GRADE_RUN_DAYS
        && s.pass_days(Capability) >= GRADE_RUN_DAYS
    {
        return AuthenticityGrade::A;
    }
    // ¬(pass days = 0 ∧ fail days ≥ 1). Stronger fingerprint evidence
    // must not drop A− to B (DECISIONS 13.13).
    let fp_ok = !(s.pass_days(Fingerprint) == 0 && s.fail_days(Fingerprint) >= 1);
    if hard == 0 && s.pass_days(Deterministic) >= 1 && s.pass_days(Capability) >= 1 && fp_ok {
        return AuthenticityGrade::AMinus;
    }
    AuthenticityGrade::B
}

/// Official direct (DECISIONS 13.7): the route host is in the official
/// table and its vendor is the claimed model's vendor.
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

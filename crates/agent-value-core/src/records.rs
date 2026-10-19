//! Data-model records persisted by the store (v22 §8, §A, §10.3, §11).
//!
//! Records hold only what §8 allows: classifications, lengths, digests and
//! numbers. There is no field for a prompt, code text, or API key value.
//! Each record has a `validate` method; the store calls it before writing.

use crate::arm::Arm;
use crate::cells::CellId;
use crate::error::{finite, CoreError, Result};
use crate::privacy::{check_evidence_summary, KeyEnvVarName, TextFacts};
use crate::runday::RunDay;
use crate::scope::ScopeId;

/// Money in millionths of a yuan (CNY). Integer, never a float, so sums are
/// exact. Non-negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MicroYuan(i64);

impl MicroYuan {
    pub fn new(micros: i64) -> Result<Self> {
        if micros < 0 {
            return Err(CoreError::OutOfRange {
                what: "amount (micro-yuan)",
                value: micros as f64,
                min: 0.0,
                max: i64::MAX as f64,
            });
        }
        Ok(Self(micros))
    }

    /// From yuan, rounded to the nearest micro-yuan.
    pub fn from_yuan(yuan: f64) -> Result<Self> {
        let y = finite("amount (yuan)", yuan)?;
        let m = (y * 1e6).round();
        if !(0.0..=9.0e15).contains(&m) {
            return Err(CoreError::OutOfRange {
                what: "amount (yuan)",
                value: y,
                min: 0.0,
                max: 9.0e9,
            });
        }
        Ok(Self(m as i64))
    }

    pub fn micros(&self) -> i64 {
        self.0
    }

    pub fn yuan(&self) -> f64 {
        self.0 as f64 / 1e6
    }
}

/// Token usage breakdown of one request (§8).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Usage {
    pub fn total(&self) -> Result<u64> {
        [self.input, self.output, self.cache_read, self.cache_write]
            .iter()
            .try_fold(0u64, |a, &b| a.checked_add(b))
            .ok_or(CoreError::Overflow {
                what: "token total",
            })
    }
}

fn check_id(field: &'static str, id: &str) -> Result<()> {
    if id.trim().is_empty() {
        return Err(CoreError::EmptyField { field });
    }
    if id.len() > 128 || id.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(CoreError::InvalidField {
            field,
            reason: "must be at most 128 characters without whitespace",
        });
    }
    Ok(())
}

fn check_order(what: &'static str, earlier: i64, later: i64) -> Result<()> {
    if later < earlier {
        return Err(CoreError::OutOfRange {
            what,
            value: later as f64,
            min: earlier as f64,
            max: i64::MAX as f64,
        });
    }
    Ok(())
}

/// Route metadata of one model request (§8).
///
/// `arm` is the (requested model, route) pair. Auxiliary-model requests made
/// inside a task point at their own arm and are flagged `auxiliary`.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestRecord {
    pub id: String,
    pub task_id: Option<String>,
    pub arm: Arm,
    pub auxiliary: bool,
    /// The raw `model` field the endpoint returned, if any.
    pub returned_model: Option<String>,
    pub response_id: Option<String>,
    /// The NAME of the env var holding the key. Never the value.
    pub key_env_var: Option<KeyEnvVarName>,
    pub start_ms: i64,
    pub first_token_ms: Option<i64>,
    pub end_ms: i64,
    pub usage: Usage,
    /// Actual charge, if known (§2: preferred over token x price table).
    pub charged: Option<MicroYuan>,
}

impl RequestRecord {
    pub fn validate(&self) -> Result<()> {
        check_id("request id", &self.id)?;
        if let Some(t) = &self.task_id {
            check_id("task id", t)?;
        }
        if let Some(r) = &self.response_id {
            check_id("response id", r)?;
        }
        if let Some(m) = &self.returned_model {
            check_id("returned model", m)?;
        }
        match self.first_token_ms {
            Some(ft) => {
                check_order("first token time", self.start_ms, ft)?;
                check_order("request end time", ft, self.end_ms)?;
            }
            None => check_order("request end time", self.start_ms, self.end_ms)?,
        }
        self.usage.total()?;
        Ok(())
    }
}

/// Task outcome (§3.1). The rules that decide it come in a later step;
/// a new task starts as `Pending`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Outcome {
    Pending,
    Success,
    Failure,
    Cancel,
    Undetermined,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Cancel => "cancel",
            Self::Undetermined => "undetermined",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "pending" => Self::Pending,
            "success" => Self::Success,
            "failure" => Self::Failure,
            "cancel" => Self::Cancel,
            "undetermined" => Self::Undetermined,
            _ => {
                return Err(CoreError::Parse {
                    what: "outcome",
                    value: s.to_string(),
                })
            }
        })
    }
}

/// One task (§A): from the user's submission to delivery, abandonment or
/// cancellation. Probe calls are tasks in the probe cell.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskRecord {
    pub id: String,
    pub scope: ScopeId,
    pub cell: CellId,
    /// The arm of the primary model the user picked.
    pub arm: Arm,
    pub submitted_ms: i64,
    pub ended_ms: Option<i64>,
    pub outcome: Outcome,
    /// Facts about the first user message (category, length, digest).
    pub first_message: Option<TextFacts>,
    pub active_ms: Option<i64>,
    pub wait_ms: Option<i64>,
    pub interruptions: u32,
    /// User rating 1-5 (explicit or spot rating). Only the score is kept.
    pub rating: Option<u8>,
}

impl TaskRecord {
    /// A pending task with no measurements yet.
    pub fn new(id: &str, scope: ScopeId, cell: CellId, arm: Arm, submitted_ms: i64) -> Self {
        Self {
            id: id.to_string(),
            scope,
            cell,
            arm,
            submitted_ms,
            ended_ms: None,
            outcome: Outcome::Pending,
            first_message: None,
            active_ms: None,
            wait_ms: None,
            interruptions: 0,
            rating: None,
        }
    }

    pub fn is_probe(&self) -> bool {
        self.cell.is_probe()
    }

    pub fn validate(&self) -> Result<()> {
        check_id("task id", &self.id)?;
        if let Some(e) = self.ended_ms {
            check_order("task end time", self.submitted_ms, e)?;
        }
        for (what, v) in [("active time", self.active_ms), ("wait time", self.wait_ms)] {
            if let Some(v) = v {
                if v < 0 {
                    return Err(CoreError::OutOfRange {
                        what,
                        value: v as f64,
                        min: 0.0,
                        max: i64::MAX as f64,
                    });
                }
            }
        }
        if let Some(r) = self.rating {
            if !(1..=5).contains(&r) {
                return Err(CoreError::OutOfRange {
                    what: "rating",
                    value: f64::from(r),
                    min: 1.0,
                    max: 5.0,
                });
            }
            if self.is_probe() {
                return Err(CoreError::InvalidField {
                    field: "rating",
                    reason: "probe tasks are never rated",
                });
            }
        }
        Ok(())
    }
}

/// Kinds of behaviour events (§8, §10.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventKind {
    ManualSwitch,
    RerouteAfterFailure,
    Cancel,
    Redo,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ManualSwitch => "manual-switch",
            Self::RerouteAfterFailure => "reroute-after-failure",
            Self::Cancel => "cancel",
            Self::Redo => "redo",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "manual-switch" => Self::ManualSwitch,
            "reroute-after-failure" => Self::RerouteAfterFailure,
            "cancel" => Self::Cancel,
            "redo" => Self::Redo,
            _ => {
                return Err(CoreError::Parse {
                    what: "event kind",
                    value: s.to_string(),
                })
            }
        })
    }
}

/// A behaviour event with a snapshot of the arms available at that moment
/// (configured, and no error, rate limit or quota warning in the previous 10
/// minutes). The snapshot is stored, never recomputed later.
#[derive(Debug, Clone, PartialEq)]
pub struct BehaviorEvent {
    pub id: String,
    pub at_ms: i64,
    pub kind: EventKind,
    pub scope: ScopeId,
    pub cell: CellId,
    pub chosen: Arm,
    pub available: Vec<Arm>,
}

impl BehaviorEvent {
    pub fn validate(&self) -> Result<()> {
        check_id("event id", &self.id)?;
        if self.cell.is_probe() {
            return Err(CoreError::InvalidField {
                field: "event cell",
                reason: "probe calls are not user behaviour",
            });
        }
        let mut seen = self.available.clone();
        seen.sort();
        seen.dedup();
        if seen.len() != self.available.len() {
            return Err(CoreError::InvalidField {
                field: "available arms",
                reason: "contains duplicates",
            });
        }
        if !self.available.contains(&self.chosen) {
            return Err(CoreError::InvalidField {
                field: "chosen arm",
                reason: "must be one of the available arms",
            });
        }
        Ok(())
    }
}

/// Probe classes (§11.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProbeClass {
    Deterministic,
    Fingerprint,
    Capability,
}

impl ProbeClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deterministic => "deterministic",
            Self::Fingerprint => "fingerprint",
            Self::Capability => "capability",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "deterministic" => Self::Deterministic,
            "fingerprint" => Self::Fingerprint,
            "capability" => Self::Capability,
            _ => {
                return Err(CoreError::Parse {
                    what: "probe class",
                    value: s.to_string(),
                })
            }
        })
    }
}

/// Per-class verdict of one run day (§11.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProbeVerdict {
    Pass,
    Fail,
    Uncertain,
    NotApplicable,
}

impl ProbeVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Uncertain => "uncertain",
            Self::NotApplicable => "not-applicable",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "pass" => Self::Pass,
            "fail" => Self::Fail,
            "uncertain" => Self::Uncertain,
            "not-applicable" => Self::NotApplicable,
            _ => {
                return Err(CoreError::Parse {
                    what: "probe verdict",
                    value: s.to_string(),
                })
            }
        })
    }
}

/// One probe result: one probe set, one arm, one run day (§8, §11).
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeResult {
    pub probe_id: String,
    pub probe_version: String,
    pub arm: Arm,
    pub at_ms: i64,
    pub class: ProbeClass,
    pub verdict: ProbeVerdict,
    /// A §11.2 hard failure was observed in this run.
    pub hard_failure: bool,
    pub evidence_summary: String,
}

impl ProbeResult {
    /// The Beijing-time run day of this result.
    pub fn run_day(&self) -> Result<RunDay> {
        RunDay::from_unix_ms(self.at_ms)
    }

    pub fn validate(&self) -> Result<()> {
        check_id("probe id", &self.probe_id)?;
        check_id("probe version", &self.probe_version)?;
        check_evidence_summary(&self.evidence_summary)?;
        self.run_day()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arm::{ModelId, Protocol, ProviderKind, Route};
    use crate::scope::TaskKind;

    fn arm(host: &str) -> Arm {
        Arm::new(
            ModelId::new("example-model-1").unwrap(),
            Route::new(
                ProviderKind::Relay,
                host,
                Protocol::AnthropicMessages,
                "cli",
            )
            .unwrap(),
        )
    }

    #[test]
    fn money_is_integer_micros() {
        assert_eq!(MicroYuan::from_yuan(0.15).unwrap().micros(), 150_000);
        assert_eq!(MicroYuan::from_yuan(1.8273).unwrap().micros(), 1_827_300);
        assert!(MicroYuan::from_yuan(-0.01).is_err());
        assert!(MicroYuan::from_yuan(f64::NAN).is_err());
        assert!(MicroYuan::new(-1).is_err());
        assert!((MicroYuan::new(2_850_000).unwrap().yuan() - 2.85).abs() <= 1e-9);
    }

    #[test]
    fn request_validation() {
        let mut r = RequestRecord {
            id: "req-1".into(),
            task_id: Some("t-1".into()),
            arm: arm("relay.example"),
            auxiliary: false,
            returned_model: Some("example-model-1-2026".into()),
            response_id: Some("msg_01".into()),
            key_env_var: Some(KeyEnvVarName::new("RELAY_API_KEY").unwrap()),
            start_ms: 100,
            first_token_ms: Some(150),
            end_ms: 300,
            usage: Usage::default(),
            charged: None,
        };
        assert!(r.validate().is_ok());
        r.first_token_ms = Some(400);
        assert!(r.validate().is_err());
        r.first_token_ms = None;
        r.end_ms = 50;
        assert!(r.validate().is_err());
        r.end_ms = 300;
        r.id = " ".into();
        assert!(r.validate().is_err());
        r.id = "req-1".into();
        r.usage.input = u64::MAX;
        r.usage.output = 1;
        assert!(r.validate().is_err());
    }

    #[test]
    fn task_validation() {
        let scope = ScopeId::new(TaskKind::Coding);
        let mut t = TaskRecord::new(
            "t-1",
            scope.clone(),
            CellId::user("fix").unwrap(),
            arm("r.example"),
            10,
        );
        assert!(t.validate().is_ok());
        t.rating = Some(6);
        assert!(t.validate().is_err());
        t.rating = Some(5);
        t.ended_ms = Some(5);
        assert!(t.validate().is_err());
        t.ended_ms = Some(20);
        t.wait_ms = Some(-1);
        assert!(t.validate().is_err());
        let mut p = TaskRecord::new("p-1", scope, CellId::Probe, arm("r.example"), 10);
        assert!(p.is_probe());
        p.rating = Some(3);
        assert!(p.validate().is_err());
        for o in [
            Outcome::Pending,
            Outcome::Success,
            Outcome::Failure,
            Outcome::Cancel,
            Outcome::Undetermined,
        ] {
            assert_eq!(Outcome::parse(o.as_str()), Ok(o));
        }
        assert!(Outcome::parse("ok").is_err());
    }

    #[test]
    fn event_validation() {
        let a = arm("a.example");
        let b = arm("b.example");
        let mut e = BehaviorEvent {
            id: "e-1".into(),
            at_ms: 0,
            kind: EventKind::RerouteAfterFailure,
            scope: ScopeId::new(TaskKind::Coding),
            cell: CellId::user("fix").unwrap(),
            chosen: b.clone(),
            available: vec![a.clone(), b.clone()],
        };
        assert!(e.validate().is_ok());
        e.available = vec![a.clone()];
        assert!(e.validate().is_err());
        e.available = vec![a.clone(), b.clone(), b.clone()];
        assert!(e.validate().is_err());
        e.available = vec![a, b];
        e.cell = CellId::Probe;
        assert!(e.validate().is_err());
        for k in [
            EventKind::ManualSwitch,
            EventKind::RerouteAfterFailure,
            EventKind::Cancel,
            EventKind::Redo,
        ] {
            assert_eq!(EventKind::parse(k.as_str()), Ok(k));
        }
    }

    #[test]
    fn probe_validation() {
        let mut p = ProbeResult {
            probe_id: "deterministic-set".into(),
            probe_version: "1".into(),
            arm: arm("relay.example"),
            at_ms: 1_792_425_599_999,
            class: ProbeClass::Deterministic,
            verdict: ProbeVerdict::Pass,
            hard_failure: false,
            evidence_summary: "9/10 correct".into(),
        };
        assert!(p.validate().is_ok());
        assert_eq!(p.run_day().unwrap().to_string(), "2026-10-19");
        p.evidence_summary = concat!("leaked sk-", "abcdefgh12345").into();
        assert!(p.validate().is_err());
        for c in [
            ProbeClass::Deterministic,
            ProbeClass::Fingerprint,
            ProbeClass::Capability,
        ] {
            assert_eq!(ProbeClass::parse(c.as_str()), Ok(c));
        }
        for v in [
            ProbeVerdict::Pass,
            ProbeVerdict::Fail,
            ProbeVerdict::Uncertain,
            ProbeVerdict::NotApplicable,
        ] {
            assert_eq!(ProbeVerdict::parse(v.as_str()), Ok(v));
        }
    }
}

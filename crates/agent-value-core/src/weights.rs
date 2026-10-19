//! Weights `(a, b, g)` and weight versions `W_k` (v22 §4, §10.1, §A).
//!
//! Each scoring scope has a ledger of weight versions. Official versions only
//! take effect at a window boundary. A weight change in the middle of a window
//! creates a *trial* version: it can be previewed immediately, labelled as a
//! non-frozen trial, and never overwrites the window's official inputs. If the
//! user confirms it, it becomes an official version from the next window.

use crate::constants::{
    DEFAULT_WEIGHT, N_EV, SELF_REPORT_MAX_QUESTIONS, SELF_REPORT_MIN_QUESTIONS, WEIGHT_FLOOR,
};
use crate::error::{finite, CoreError, Result};
use crate::scope::ScopeId;
use crate::window::{FrozenInputs, FrozenWindow, WindowIndex};

/// Allowed absolute floating-point slack for `a + b + g = 1` and for the
/// `>= 0.05` floor (so grid points computed as `1 - a - b` are accepted).
pub const WEIGHT_SUM_TOLERANCE: f64 = 1e-9;

/// Label shown next to results computed with a trial version (§10.1).
pub const TRIAL_LABEL: &str = "Non-frozen trial";

/// The exponents `(a, b, g)` for cost, time and quality.
///
/// Invariant (checked on construction): all finite, each `>= 0.05` and
/// `a + b + g = 1`, both up to [`WEIGHT_SUM_TOLERANCE`] of floating-point slack.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Weights {
    a: f64,
    b: f64,
    g: f64,
}

impl Weights {
    pub fn new(a: f64, b: f64, g: f64) -> Result<Self> {
        let a = finite("weight a", a)?;
        let b = finite("weight b", b)?;
        let g = finite("weight g", g)?;
        for (dim, v) in [("a", a), ("b", b), ("g", g)] {
            if v < WEIGHT_FLOOR - WEIGHT_SUM_TOLERANCE {
                return Err(CoreError::WeightBelowFloor {
                    dim,
                    value: v,
                    floor: WEIGHT_FLOOR,
                });
            }
        }
        let sum = a + b + g;
        if (sum - 1.0).abs() > WEIGHT_SUM_TOLERANCE {
            return Err(CoreError::WeightSumNotOne { sum });
        }
        Ok(Self { a, b, g })
    }

    /// `(1/3, 1/3, 1/3)`: the weights used when there is no evidence.
    pub fn equal() -> Self {
        Self {
            a: DEFAULT_WEIGHT,
            b: DEFAULT_WEIGHT,
            g: DEFAULT_WEIGHT,
        }
    }

    /// Cost exponent.
    pub fn a(&self) -> f64 {
        self.a
    }

    /// Time exponent.
    pub fn b(&self) -> f64 {
        self.b
    }

    /// Quality exponent.
    pub fn g(&self) -> f64 {
        self.g
    }

    fn is_equal_default(&self) -> bool {
        [self.a, self.b, self.g]
            .iter()
            .all(|v| (v - DEFAULT_WEIGHT).abs() <= WEIGHT_SUM_TOLERANCE)
    }
}

/// Where a weight version came from (§10.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WeightSource {
    /// No evidence: `(1/3, 1/3, 1/3)`.
    Default,
    /// Five-level pairwise self-report (§10.2).
    SelfReport,
    /// A behaviour suggestion (§10.3) that the user confirmed.
    ConfirmedBehaviorSuggestion,
    /// Weights the user typed in.
    Manual,
}

impl WeightSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::SelfReport => "self-report",
            Self::ConfirmedBehaviorSuggestion => "confirmed-behavior-suggestion",
            Self::Manual => "manual",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "default" => Self::Default,
            "self-report" => Self::SelfReport,
            "confirmed-behavior-suggestion" => Self::ConfirmedBehaviorSuggestion,
            "manual" => Self::Manual,
            _ => {
                return Err(CoreError::Parse {
                    what: "weight source",
                    value: s.to_string(),
                })
            }
        })
    }
}

/// A five-level answer (§10.2): strongly A / slightly A / about the same /
/// slightly B / strongly B, i.e. `l = +2, +1, 0, -1, -2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FiveLevel {
    StronglyA,
    SlightlyA,
    AboutSame,
    SlightlyB,
    StronglyB,
}

impl FiveLevel {
    pub fn level(self) -> i8 {
        match self {
            Self::StronglyA => 2,
            Self::SlightlyA => 1,
            Self::AboutSame => 0,
            Self::SlightlyB => -1,
            Self::StronglyB => -2,
        }
    }

    pub fn from_level(level: i8) -> Result<Self> {
        match level {
            2 => Ok(Self::StronglyA),
            1 => Ok(Self::SlightlyA),
            0 => Ok(Self::AboutSame),
            -1 => Ok(Self::SlightlyB),
            -2 => Ok(Self::StronglyB),
            other => Err(CoreError::OutOfRange {
                what: "five-level answer",
                value: f64::from(other),
                min: -2.0,
                max: 2.0,
            }),
        }
    }
}

/// One self-report answer: which question, and the answer level.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SelfReportAnswer {
    pub question_id: String,
    pub answer: FiveLevel,
}

/// The evidence behind a weight version (§10.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Evidence {
    /// No evidence (default or manual weights).
    None,
    /// The self-report answers.
    SelfReportAnswers(Vec<SelfReportAnswer>),
    /// The behaviour event ids the suggestion was estimated from.
    BehaviorEvents(Vec<String>),
}

fn validate_evidence(source: WeightSource, weights: &Weights, evidence: &Evidence) -> Result<()> {
    match (source, evidence) {
        (WeightSource::Default, Evidence::None) => {
            if weights.is_equal_default() {
                Ok(())
            } else {
                Err(CoreError::EvidenceMismatch {
                    reason: "default source requires weights (1/3, 1/3, 1/3)",
                })
            }
        }
        (WeightSource::Manual, Evidence::None) => Ok(()),
        (WeightSource::SelfReport, Evidence::SelfReportAnswers(answers)) => {
            if answers.len() < SELF_REPORT_MIN_QUESTIONS
                || answers.len() > SELF_REPORT_MAX_QUESTIONS
            {
                return Err(CoreError::EvidenceMismatch {
                    reason: "a finished self-report has between 4 and 10 answers",
                });
            }
            if answers.iter().any(|a| a.question_id.trim().is_empty()) {
                return Err(CoreError::EmptyField {
                    field: "self-report question id",
                });
            }
            Ok(())
        }
        (WeightSource::ConfirmedBehaviorSuggestion, Evidence::BehaviorEvents(ids)) => {
            if ids.len() < N_EV {
                return Err(CoreError::EvidenceMismatch {
                    reason: "a behaviour suggestion needs at least n_ev = 20 events",
                });
            }
            if ids.iter().any(|id| id.trim().is_empty()) {
                return Err(CoreError::EmptyField {
                    field: "behaviour event id",
                });
            }
            Ok(())
        }
        _ => Err(CoreError::EvidenceMismatch {
            reason: "evidence kind does not match the weight source",
        }),
    }
}

/// Identifier of a weight version within a ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VersionId(pub u64);

/// What happened to a trial version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrialResolution {
    Pending,
    /// Confirmed; the official version it became.
    Confirmed(VersionId),
    Discarded,
}

/// Official versus trial.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionStatus {
    /// Official; takes effect from the start of `effective_from`.
    Official { effective_from: WindowIndex },
    /// Trial created during `created_in`; never used for official results.
    Trial {
        created_in: WindowIndex,
        resolution: TrialResolution,
    },
}

/// A weight version `W_k = (a, b, g; source, evidence, effective window,
/// previous version)` (§10.1).
#[derive(Debug, Clone, PartialEq)]
pub struct WeightVersion {
    id: VersionId,
    scope: ScopeId,
    weights: Weights,
    source: WeightSource,
    evidence: Evidence,
    status: VersionStatus,
    previous: Option<VersionId>,
}

impl WeightVersion {
    /// Rebuilds a version from stored parts, re-checking its invariants:
    /// official versions must have evidence that matches their source; trial
    /// versions are always manual with no evidence.
    pub fn from_parts(
        id: VersionId,
        scope: ScopeId,
        weights: Weights,
        source: WeightSource,
        evidence: Evidence,
        status: VersionStatus,
        previous: Option<VersionId>,
    ) -> Result<Self> {
        match status {
            VersionStatus::Official { .. } => validate_evidence(source, &weights, &evidence)?,
            VersionStatus::Trial { .. } => {
                if source != WeightSource::Manual || evidence != Evidence::None {
                    return Err(CoreError::EvidenceMismatch {
                        reason: "trial versions are manual and carry no evidence",
                    });
                }
            }
        }
        Ok(Self {
            id,
            scope,
            weights,
            source,
            evidence,
            status,
            previous,
        })
    }

    pub fn id(&self) -> VersionId {
        self.id
    }
    pub fn scope(&self) -> &ScopeId {
        &self.scope
    }
    pub fn weights(&self) -> Weights {
        self.weights
    }
    pub fn source(&self) -> WeightSource {
        self.source
    }
    pub fn evidence(&self) -> &Evidence {
        &self.evidence
    }
    pub fn status(&self) -> VersionStatus {
        self.status
    }
    /// The official version that was in effect when this one was created.
    pub fn previous(&self) -> Option<VersionId> {
        self.previous
    }
    pub fn is_trial(&self) -> bool {
        matches!(self.status, VersionStatus::Trial { .. })
    }
    /// The window an official version takes effect from.
    pub fn effective_from(&self) -> Option<WindowIndex> {
        match self.status {
            VersionStatus::Official { effective_from } => Some(effective_from),
            VersionStatus::Trial { .. } => None,
        }
    }
}

/// What a trial preview shows: the trial weights, clearly labelled, next to
/// the untouched official version of the same window.
#[derive(Debug, Clone, PartialEq)]
pub struct TrialPreview {
    pub trial: WeightVersion,
    pub official: WeightVersion,
    pub label: &'static str,
}

/// Stored form of a frozen window, used by [`WeightLedger::from_parts`].
#[derive(Debug, Clone, PartialEq)]
pub struct FrozenWindowParts {
    pub window: WindowIndex,
    pub weight_version: VersionId,
    pub is_first_window: bool,
    pub inputs: FrozenInputs,
}

/// All weight versions and frozen windows of one scoring scope.
#[derive(Debug, Clone)]
pub struct WeightLedger {
    scope: ScopeId,
    versions: Vec<WeightVersion>,
    frozen: Vec<FrozenWindow>,
}

impl WeightLedger {
    /// A ledger whose only version is the default `(1/3, 1/3, 1/3)`,
    /// official from window 0.
    pub fn new(scope: ScopeId) -> Self {
        let v0 = WeightVersion {
            id: VersionId(0),
            scope: scope.clone(),
            weights: Weights::equal(),
            source: WeightSource::Default,
            evidence: Evidence::None,
            status: VersionStatus::Official {
                effective_from: WindowIndex::FIRST,
            },
            previous: None,
        };
        Self {
            scope,
            versions: vec![v0],
            frozen: Vec::new(),
        }
    }

    pub fn scope(&self) -> &ScopeId {
        &self.scope
    }

    pub fn versions(&self) -> &[WeightVersion] {
        &self.versions
    }

    /// All frozen windows, in window order.
    pub fn frozen_windows(&self) -> &[FrozenWindow] {
        &self.frozen
    }

    /// Rebuilds a ledger from stored parts and checks that it is one a
    /// sequence of ledger operations could have produced:
    /// - version ids are 0, 1, 2, ... and version 0 is the default, official
    ///   from window 0, with no previous version;
    /// - every version belongs to `scope`, and `previous` points at an
    ///   earlier official version;
    /// - a confirmed trial points at a later official manual version with the
    ///   same weights;
    /// - frozen windows are 0, 1, 2, ... and each was frozen with the official
    ///   version that applies to it.
    pub fn from_parts(
        scope: ScopeId,
        versions: Vec<WeightVersion>,
        frozen: Vec<FrozenWindowParts>,
    ) -> Result<Self> {
        let corrupt = |reason| Err(CoreError::CorruptLedger { reason });
        let Some(v0) = versions.first() else {
            return corrupt("no versions");
        };
        if v0.source != WeightSource::Default
            || v0.status
                != (VersionStatus::Official {
                    effective_from: WindowIndex::FIRST,
                })
            || v0.previous.is_some()
        {
            return corrupt("version 0 must be the default, official from window 0");
        }
        for (i, v) in versions.iter().enumerate() {
            if v.id != VersionId(i as u64) {
                return corrupt("version ids must be 0, 1, 2, ...");
            }
            if v.scope != scope {
                return corrupt("version belongs to another scope");
            }
            if let Some(p) = v.previous {
                let ok = p.0 < v.id.0 && !versions[p.0 as usize].is_trial();
                if !ok {
                    return corrupt("previous must be an earlier official version");
                }
            }
            if let VersionStatus::Trial {
                resolution: TrialResolution::Confirmed(o),
                ..
            } = v.status
            {
                let ok = o.0 > v.id.0
                    && versions.get(o.0 as usize).is_some_and(|ov| {
                        !ov.is_trial()
                            && ov.source == WeightSource::Manual
                            && ov.weights == v.weights
                    });
                if !ok {
                    return corrupt("confirmed trial must point at its official version");
                }
            }
        }
        let mut ledger = Self {
            scope: scope.clone(),
            versions,
            frozen: Vec::new(),
        };
        for (i, f) in frozen.into_iter().enumerate() {
            if f.window != WindowIndex(i as u32) {
                return corrupt("frozen windows must be 0, 1, 2, ...");
            }
            let expected = ledger.official_for(f.window).id;
            if f.weight_version != expected {
                return corrupt("frozen window does not use the version that applies to it");
            }
            let weight_version = ledger.version(expected)?.clone();
            ledger.frozen.push(FrozenWindow {
                scope: scope.clone(),
                window: f.window,
                weight_version,
                is_first_window: f.is_first_window,
                inputs: f.inputs,
            });
        }
        Ok(ledger)
    }

    pub fn version(&self, id: VersionId) -> Result<&WeightVersion> {
        self.versions
            .iter()
            .find(|v| v.id == id)
            .ok_or(CoreError::UnknownVersion { id: id.0 })
    }

    /// The last frozen window, if any.
    pub fn last_frozen(&self) -> Option<WindowIndex> {
        self.frozen.last().map(|f| f.window)
    }

    /// The first window that is not frozen yet.
    pub fn next_unfrozen(&self) -> Result<WindowIndex> {
        match self.last_frozen() {
            None => Ok(WindowIndex::FIRST),
            Some(w) => w.next(),
        }
    }

    /// The frozen (official) inputs of `window`.
    pub fn frozen(&self, window: WindowIndex) -> Result<&FrozenWindow> {
        self.frozen
            .iter()
            .find(|f| f.window == window)
            .ok_or(CoreError::WindowNotFrozen { window: window.0 })
    }

    /// The official version that applies to `window`: the latest-created
    /// official version whose effective window is `<= window`. For a frozen
    /// window, this is always the version it was frozen with.
    pub fn official_for(&self, window: WindowIndex) -> &WeightVersion {
        if let Ok(f) = self.frozen(window) {
            return &f.weight_version;
        }
        self.versions
            .iter()
            .filter(|v| matches!(v.effective_from(), Some(w) if w <= window))
            .max_by_key(|v| (v.effective_from(), v.id))
            // Version 0 is official from window 0, so this never falls through.
            .unwrap_or(&self.versions[0])
    }

    fn next_id(&self) -> VersionId {
        VersionId(self.versions.len() as u64)
    }

    /// Records a new official version taking effect from `effective_from`.
    ///
    /// `effective_from` must not be frozen yet: official versions only take
    /// effect at a window boundary and never rewrite a frozen window.
    pub fn schedule_official(
        &mut self,
        weights: Weights,
        source: WeightSource,
        evidence: Evidence,
        effective_from: WindowIndex,
    ) -> Result<VersionId> {
        validate_evidence(source, &weights, &evidence)?;
        if let Some(last) = self.last_frozen() {
            if effective_from <= last {
                return Err(CoreError::WindowAlreadyFrozen {
                    window: effective_from.0,
                });
            }
        }
        let id = self.next_id();
        let previous = Some(self.official_for(effective_from).id);
        self.versions.push(WeightVersion {
            id,
            scope: self.scope.clone(),
            weights,
            source,
            evidence,
            status: VersionStatus::Official { effective_from },
            previous,
        });
        Ok(id)
    }

    /// A manual weight change during `current` (which must be frozen, i.e.
    /// in progress). Creates a trial version only; the window's official
    /// inputs stay as they are.
    pub fn create_trial(&mut self, weights: Weights, current: WindowIndex) -> Result<VersionId> {
        let official_id = self.frozen(current)?.weight_version.id;
        let id = self.next_id();
        self.versions.push(WeightVersion {
            id,
            scope: self.scope.clone(),
            weights,
            source: WeightSource::Manual,
            evidence: Evidence::None,
            status: VersionStatus::Trial {
                created_in: current,
                resolution: TrialResolution::Pending,
            },
            previous: Some(official_id),
        });
        Ok(id)
    }

    /// The trial weights next to the untouched official version of the
    /// window the trial was created in.
    pub fn trial_preview(&self, trial: VersionId) -> Result<TrialPreview> {
        let t = self.version(trial)?;
        let VersionStatus::Trial { created_in, .. } = t.status else {
            return Err(CoreError::NotATrial { id: trial.0 });
        };
        Ok(TrialPreview {
            trial: t.clone(),
            official: self.frozen(created_in)?.weight_version.clone(),
            label: TRIAL_LABEL,
        })
    }

    fn pending_trial_index(&self, trial: VersionId) -> Result<(usize, WindowIndex)> {
        let idx = self
            .versions
            .iter()
            .position(|v| v.id == trial)
            .ok_or(CoreError::UnknownVersion { id: trial.0 })?;
        match self.versions[idx].status {
            VersionStatus::Trial {
                created_in,
                resolution: TrialResolution::Pending,
            } => Ok((idx, created_in)),
            VersionStatus::Trial { .. } => Err(CoreError::TrialAlreadyResolved { id: trial.0 }),
            VersionStatus::Official { .. } => Err(CoreError::NotATrial { id: trial.0 }),
        }
    }

    /// The user confirms a trial: it becomes a new official version (source
    /// "manual") from the next window that is not frozen yet. Returns the id
    /// of the new official version.
    pub fn confirm_trial(&mut self, trial: VersionId) -> Result<VersionId> {
        let (idx, _created_in) = self.pending_trial_index(trial)?;
        let effective_from = self.next_unfrozen()?;
        let weights = self.versions[idx].weights;
        let official = self.schedule_official(
            weights,
            WeightSource::Manual,
            Evidence::None,
            effective_from,
        )?;
        if let VersionStatus::Trial { created_in, .. } = self.versions[idx].status {
            self.versions[idx].status = VersionStatus::Trial {
                created_in,
                resolution: TrialResolution::Confirmed(official),
            };
        }
        Ok(official)
    }

    /// The user drops a trial. Nothing official changes.
    pub fn discard_trial(&mut self, trial: VersionId) -> Result<()> {
        let (idx, created_in) = self.pending_trial_index(trial)?;
        self.versions[idx].status = VersionStatus::Trial {
            created_in,
            resolution: TrialResolution::Discarded,
        };
        Ok(())
    }

    /// Freezes `window` at its start.
    ///
    /// Windows are frozen in order without gaps. `prior_user_task_count` is
    /// the number of user (non-probe) tasks this scope recorded in all earlier
    /// windows; zero makes this a first window, in which no V is produced.
    pub fn freeze_window(
        &mut self,
        window: WindowIndex,
        prior_user_task_count: u64,
        inputs: FrozenInputs,
    ) -> Result<&FrozenWindow> {
        let expected = self.next_unfrozen()?;
        if window < expected {
            return Err(CoreError::WindowAlreadyFrozen { window: window.0 });
        }
        if window != expected {
            return Err(CoreError::WindowOutOfOrder {
                expected: expected.0,
                got: window.0,
            });
        }
        let weight_version = self.official_for(window).clone();
        self.frozen.push(FrozenWindow {
            scope: self.scope.clone(),
            window,
            weight_version,
            is_first_window: prior_user_task_count == 0,
            inputs,
        });
        self.frozen
            .last()
            .ok_or(CoreError::WindowNotFrozen { window: window.0 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::TaskKind;

    #[test]
    fn equal_weights_sum_to_one() {
        let w = Weights::equal();
        assert!((w.a() + w.b() + w.g() - 1.0).abs() <= WEIGHT_SUM_TOLERANCE);
        assert!(Weights::new(w.a(), w.b(), w.g()).is_ok());
    }

    #[test]
    fn five_level_round_trip() {
        for l in -2..=2 {
            assert_eq!(FiveLevel::from_level(l).unwrap().level(), l);
        }
        assert!(FiveLevel::from_level(3).is_err());
        assert!(FiveLevel::from_level(-3).is_err());
    }

    #[test]
    fn evidence_must_match_source() {
        let w = Weights::new(0.2, 0.35, 0.45).unwrap();
        let ans = |n: usize| {
            Evidence::SelfReportAnswers(
                (0..n)
                    .map(|i| SelfReportAnswer {
                        question_id: format!("q{i}"),
                        answer: FiveLevel::SlightlyA,
                    })
                    .collect(),
            )
        };
        let events = |n: usize| Evidence::BehaviorEvents((0..n).map(|i| format!("e{i}")).collect());
        assert!(validate_evidence(WeightSource::SelfReport, &w, &ans(4)).is_ok());
        assert!(validate_evidence(WeightSource::SelfReport, &w, &ans(10)).is_ok());
        assert!(validate_evidence(WeightSource::SelfReport, &w, &ans(3)).is_err());
        assert!(validate_evidence(WeightSource::SelfReport, &w, &ans(11)).is_err());
        assert!(validate_evidence(WeightSource::SelfReport, &w, &Evidence::None).is_err());
        assert!(
            validate_evidence(WeightSource::ConfirmedBehaviorSuggestion, &w, &events(20)).is_ok()
        );
        assert!(
            validate_evidence(WeightSource::ConfirmedBehaviorSuggestion, &w, &events(19)).is_err()
        );
        assert!(validate_evidence(WeightSource::Manual, &w, &Evidence::None).is_ok());
        assert!(validate_evidence(WeightSource::Manual, &w, &events(20)).is_err());
        assert!(validate_evidence(WeightSource::Default, &w, &Evidence::None).is_err());
        assert!(
            validate_evidence(WeightSource::Default, &Weights::equal(), &Evidence::None).is_ok()
        );
        let blank = Evidence::SelfReportAnswers(vec![
            SelfReportAnswer {
                question_id: " ".into(),
                answer: FiveLevel::AboutSame,
            };
            4
        ]);
        assert!(validate_evidence(WeightSource::SelfReport, &w, &blank).is_err());
    }

    #[test]
    fn new_ledger_starts_with_default_version() {
        let l = WeightLedger::new(ScopeId::new(TaskKind::Coding));
        let v = l.official_for(WindowIndex(0));
        assert_eq!(v.id(), VersionId(0));
        assert_eq!(v.source(), WeightSource::Default);
        assert_eq!(v.weights(), Weights::equal());
        assert_eq!(v.previous(), None);
        assert_eq!(l.next_unfrozen(), Ok(WindowIndex(0)));
    }
}

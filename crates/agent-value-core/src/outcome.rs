//! Task outcome judgement (v22 §3.1).
//!
//! Rules are applied top to bottom; the first that applies decides:
//! 1. explicit user abort => cancel;
//! 2. the last executable check at task end did not pass => failure;
//! 3. a user "redo / correction" message within `tau_re` (30 min) after the
//!    end, in the same session or working directory => failure;
//! 4. coding tasks only: the task's change is rolled back as a whole within
//!    `tau_keep` (24 h) after the end => failure (retroactively);
//! 5. the agent reported completion and the last check (if any) passed =>
//!    success;
//! 6. otherwise => undetermined (never success by default).
//!
//! The judgement is a pure function of the signals and an ingestion
//! watermark `observed_until_ms`: all events up to that time have been seen.
//! Outcomes from rules 5 and 6 are *provisional* while a rule 3/4
//! observation window is still open; re-judging with later events can flip
//! them to failure.

use crate::constants::{TAU_KEEP_HOURS, TAU_RE_MINUTES};
use crate::error::{CoreError, Result};
use crate::records::Outcome;
use crate::scope::TaskKind;

/// `tau_re` in milliseconds.
pub const TAU_RE_MS: i64 = TAU_RE_MINUTES as i64 * 60_000;
/// `tau_keep` in milliseconds.
pub const TAU_KEEP_MS: i64 = TAU_KEEP_HOURS as i64 * 3_600_000;

/// Result of one executable check (tests, build, type check).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Passed,
    Failed,
    /// Errored or timed out: counts as not passed.
    Errored,
}

/// One executable check recognised in the agent log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckRun {
    pub at_ms: i64,
    pub status: CheckStatus,
}

/// A message after the task ended, as classified by the frozen rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FollowUpMessage {
    pub at_ms: i64,
    /// Sent by the user (not the agent).
    pub from_user: bool,
    /// Same session or same working directory as the task.
    pub same_session_or_workdir: bool,
    /// Classified as "redo / correction" (a new feature request is not).
    pub classified_as_redo: bool,
}

/// A rollback of the task's change (git revert / reset / undo of its commit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rollback {
    pub at_ms: i64,
    /// The whole change was rolled back (a partial revert does not count).
    pub whole_change: bool,
}

/// Everything known about a task for judging its outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSignals {
    pub kind: TaskKind,
    /// Delivery / end time; `None` while running. For an interrupted
    /// session the caller sets it to the last activity, if known.
    pub ended_ms: Option<i64>,
    pub session_interrupted: bool,
    pub explicit_abort: bool,
    pub agent_reported_completion: bool,
    pub checks: Vec<CheckRun>,
    pub follow_ups: Vec<FollowUpMessage>,
    pub rollbacks: Vec<Rollback>,
}

impl TaskSignals {
    /// A running task with no signals yet.
    pub fn new(kind: TaskKind) -> Self {
        Self {
            kind,
            ended_ms: None,
            session_interrupted: false,
            explicit_abort: false,
            agent_reported_completion: false,
            checks: Vec::new(),
            follow_ups: Vec::new(),
            rollbacks: Vec::new(),
        }
    }
}

/// Which rule decided the outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rule {
    /// Not ended yet: pending.
    StillRunning,
    ExplicitAbort,
    FailedCheck,
    RedoWithinTauRe,
    RollbackWithinTauKeep,
    Completed,
    NoCompletionSignal,
}

impl Rule {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StillRunning => "still-running",
            Self::ExplicitAbort => "explicit-abort",
            Self::FailedCheck => "failed-check",
            Self::RedoWithinTauRe => "redo-within-tau-re",
            Self::RollbackWithinTauKeep => "rollback-within-tau-keep",
            Self::Completed => "completed",
            Self::NoCompletionSignal => "no-completion-signal",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "still-running" => Self::StillRunning,
            "explicit-abort" => Self::ExplicitAbort,
            "failed-check" => Self::FailedCheck,
            "redo-within-tau-re" => Self::RedoWithinTauRe,
            "rollback-within-tau-keep" => Self::RollbackWithinTauKeep,
            "completed" => Self::Completed,
            "no-completion-signal" => Self::NoCompletionSignal,
            _ => {
                return Err(CoreError::Parse {
                    what: "outcome rule",
                    value: s.to_string(),
                })
            }
        })
    }
}

/// The outcome, the rule that decided it, and whether it can still change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Judgement {
    pub outcome: Outcome,
    pub rule: Rule,
    /// True while a later redo (rule 3) or rollback (rule 4) could still
    /// turn this outcome into a failure.
    pub provisional: bool,
    /// When the outcome becomes final (end of the last open window), if
    /// it is provisional.
    pub final_after_ms: Option<i64>,
}

fn add(a: i64, b: i64) -> Result<i64> {
    a.checked_add(b).ok_or(CoreError::Overflow {
        what: "observation window end",
    })
}

/// `end < t <= end + tau` (events at or before the end do not count).
fn within(t: i64, end: i64, tau: i64) -> Result<bool> {
    Ok(t > end && t <= add(end, tau)?)
}

/// Judges a task's outcome from `signals`, given that every event up to
/// `observed_until_ms` has been ingested. Events later than the watermark
/// are an input error.
pub fn judge(signals: &TaskSignals, observed_until_ms: i64) -> Result<Judgement> {
    let late = signals
        .checks
        .iter()
        .map(|c| c.at_ms)
        .chain(signals.follow_ups.iter().map(|m| m.at_ms))
        .chain(signals.rollbacks.iter().map(|r| r.at_ms))
        .chain(signals.ended_ms);
    for t in late {
        if t > observed_until_ms {
            return Err(CoreError::OutOfRange {
                what: "event time (after the ingestion watermark)",
                value: t as f64,
                min: f64::NEG_INFINITY,
                max: observed_until_ms as f64,
            });
        }
    }
    let fixed = |outcome, rule| Judgement {
        outcome,
        rule,
        provisional: false,
        final_after_ms: None,
    };

    // Rule 1.
    if signals.explicit_abort {
        return Ok(fixed(Outcome::Cancel, Rule::ExplicitAbort));
    }
    let end = match signals.ended_ms {
        Some(e) => e,
        // Without an end time "the last check at task end" and the
        // observation windows of rules 3 and 4 are undefined: an
        // interrupted task with no known end is undetermined.
        None if signals.session_interrupted => {
            return Ok(fixed(Outcome::Undetermined, Rule::NoCompletionSignal))
        }
        None => return Ok(fixed(Outcome::Pending, Rule::StillRunning)),
    };

    // Rule 2: the last check at task end; ties go to the non-passing result.
    let last_check = signals
        .checks
        .iter()
        .filter(|c| c.at_ms <= end)
        .max_by_key(|c| (c.at_ms, c.status != CheckStatus::Passed));
    if let Some(c) = last_check {
        if c.status != CheckStatus::Passed {
            return Ok(fixed(Outcome::Failure, Rule::FailedCheck));
        }
    }

    // Rule 3.
    for m in &signals.follow_ups {
        if m.from_user
            && m.same_session_or_workdir
            && m.classified_as_redo
            && within(m.at_ms, end, TAU_RE_MS)?
        {
            return Ok(fixed(Outcome::Failure, Rule::RedoWithinTauRe));
        }
    }

    // Rule 4 (coding only).
    let coding = signals.kind == TaskKind::Coding;
    if coding {
        for r in &signals.rollbacks {
            if r.whole_change && within(r.at_ms, end, TAU_KEEP_MS)? {
                return Ok(fixed(Outcome::Failure, Rule::RollbackWithinTauKeep));
            }
        }
    }

    // Rules 5 and 6 stay provisional until the last window closes.
    let closes = add(end, if coding { TAU_KEEP_MS } else { TAU_RE_MS })?;
    let provisional = observed_until_ms < closes;
    let (outcome, rule) = if signals.agent_reported_completion {
        (Outcome::Success, Rule::Completed)
    } else {
        (Outcome::Undetermined, Rule::NoCompletionSignal)
    };
    Ok(Judgement {
        outcome,
        rule,
        provisional,
        final_after_ms: provisional.then_some(closes),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_names_round_trip() {
        for r in [
            Rule::StillRunning,
            Rule::ExplicitAbort,
            Rule::FailedCheck,
            Rule::RedoWithinTauRe,
            Rule::RollbackWithinTauKeep,
            Rule::Completed,
            Rule::NoCompletionSignal,
        ] {
            assert_eq!(Rule::parse(r.as_str()), Ok(r));
        }
        assert!(Rule::parse("x").is_err());
    }

    #[test]
    fn tau_constants() {
        assert_eq!(TAU_RE_MS, 1_800_000);
        assert_eq!(TAU_KEEP_MS, 86_400_000);
    }
}

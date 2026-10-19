//! Time windows and freezing (v22 §A, §8).
//!
//! Scoring runs per window (default 30 days). The inputs window `t` uses
//! (cell weights, weight version `W_k`, reference `R_k`, fallback `F_k`,
//! price table, classification rules) are determined only from window `t-1`
//! and earlier, and are frozen when window `t` starts. A frozen window is
//! immutable: later changes create new versions for later windows, or trial
//! versions, but never rewrite it.

use crate::constants::{DRIFT_TV_THRESHOLD, WINDOW_DAYS};
use crate::error::{finite, CoreError, Result};
use crate::scope::ScopeId;
use crate::weights::{VersionId, WeightVersion, Weights};

const SECONDS_PER_DAY: i64 = 86_400;

/// Index of a window within one scope; 0 is the scope's first window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowIndex(pub u32);

impl WindowIndex {
    pub const FIRST: WindowIndex = WindowIndex(0);

    /// The following window.
    pub fn next(self) -> Result<Self> {
        self.0
            .checked_add(1)
            .map(WindowIndex)
            .ok_or(CoreError::Overflow {
                what: "window index",
            })
    }
}

/// Fixed-length windows starting at `origin` (Unix seconds).
///
/// Windows are half-open `[start, end)`. Callers choose the origin, for
/// example a local midnight in the user's time zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowSchedule {
    origin_unix_s: i64,
    length_s: i64,
}

impl WindowSchedule {
    /// A schedule with windows of `length_days` days (must be >= 1).
    pub fn new(origin_unix_s: i64, length_days: u32) -> Result<Self> {
        if length_days == 0 {
            return Err(CoreError::OutOfRange {
                what: "window length (days)",
                value: 0.0,
                min: 1.0,
                max: f64::from(u32::MAX),
            });
        }
        Ok(Self {
            origin_unix_s,
            length_s: i64::from(length_days) * SECONDS_PER_DAY,
        })
    }

    /// A schedule with the §9 default length of 30 days.
    pub fn with_default_length(origin_unix_s: i64) -> Self {
        Self {
            origin_unix_s,
            length_s: i64::from(WINDOW_DAYS) * SECONDS_PER_DAY,
        }
    }

    pub fn length_days(&self) -> i64 {
        self.length_s / SECONDS_PER_DAY
    }

    /// The window containing `ts_unix_s`.
    pub fn window_of(&self, ts_unix_s: i64) -> Result<WindowIndex> {
        if ts_unix_s < self.origin_unix_s {
            return Err(CoreError::BeforeOrigin {
                timestamp: ts_unix_s,
                origin: self.origin_unix_s,
            });
        }
        let offset = ts_unix_s
            .checked_sub(self.origin_unix_s)
            .ok_or(CoreError::Overflow {
                what: "timestamp offset",
            })?;
        let idx = offset / self.length_s;
        u32::try_from(idx)
            .map(WindowIndex)
            .map_err(|_| CoreError::Overflow {
                what: "window index",
            })
    }

    /// `[start, end)` of `window`, in Unix seconds.
    pub fn bounds(&self, window: WindowIndex) -> Result<(i64, i64)> {
        let overflow = CoreError::Overflow {
            what: "window bounds",
        };
        let start = i64::from(window.0)
            .checked_mul(self.length_s)
            .and_then(|o| o.checked_add(self.origin_unix_s))
            .ok_or(overflow.clone())?;
        let end = start.checked_add(self.length_s).ok_or(overflow)?;
        Ok((start, end))
    }
}

/// Version labels of the other inputs frozen at a window start.
///
/// Their contents are defined by later steps (cell weights §0, reference and
/// fallback §13, price table §2, classification rules §A); this step only
/// records which version each window used. `None` means "not configured".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FrozenInputs {
    pub cell_weights_version: Option<String>,
    pub reference_version: Option<String>,
    pub fallback_version: Option<String>,
    pub price_table_version: Option<String>,
    pub classification_rules_version: Option<String>,
}

/// Label shown instead of V in a first window (§A).
pub const FIRST_WINDOW_LABEL: &str =
    "First window: collecting data only; weights and reference are not calibrated by history.";

/// The official, immutable inputs of one window of one scope.
#[derive(Debug, Clone, PartialEq)]
pub struct FrozenWindow {
    pub(crate) scope: ScopeId,
    pub(crate) window: WindowIndex,
    pub(crate) weight_version: WeightVersion,
    pub(crate) is_first_window: bool,
    pub(crate) inputs: FrozenInputs,
}

impl FrozenWindow {
    pub fn scope(&self) -> &ScopeId {
        &self.scope
    }

    pub fn window(&self) -> WindowIndex {
        self.window
    }

    /// Snapshot of the official weight version this window uses.
    pub fn weight_version(&self) -> &WeightVersion {
        &self.weight_version
    }

    pub fn weight_version_id(&self) -> VersionId {
        self.weight_version.id()
    }

    pub fn weights(&self) -> Weights {
        self.weight_version.weights()
    }

    pub fn inputs(&self) -> &FrozenInputs {
        &self.inputs
    }

    /// True when the scope had no user tasks before this window (§A).
    pub fn is_first_window(&self) -> bool {
        self.is_first_window
    }

    /// §A: a first window outputs per-arm triples and the frontier only,
    /// never V. Other rules (for example the §7 tiers) may still withhold V
    /// in later windows; this checks the window rule only.
    pub fn ensure_v_allowed_by_window_rule(&self) -> Result<()> {
        if self.is_first_window {
            Err(CoreError::VWithheldFirstWindow {
                window: self.window.0,
            })
        } else {
            Ok(())
        }
    }

    /// The first-window label, if it applies.
    pub fn first_window_label(&self) -> Option<&'static str> {
        self.is_first_window.then_some(FIRST_WINDOW_LABEL)
    }
}

/// §A drift check: total variation distance between the frozen cell weights
/// and this window's observed task mix.
///
/// Returns `Ok(None)` when there are no observed tasks yet.
pub fn task_mix_total_variation(
    frozen_omega: &[f64],
    observed_counts: &[u64],
) -> Result<Option<f64>> {
    if frozen_omega.len() != observed_counts.len() {
        return Err(CoreError::LengthMismatch {
            left: frozen_omega.len(),
            right: observed_counts.len(),
        });
    }
    let mut sum = 0.0;
    for &w in frozen_omega {
        let w = finite("cell weight", w)?;
        if w < 0.0 {
            return Err(CoreError::OutOfRange {
                what: "cell weight",
                value: w,
                min: 0.0,
                max: 1.0,
            });
        }
        sum += w;
    }
    if (sum - 1.0).abs() > 1e-9 {
        return Err(CoreError::OutOfRange {
            what: "sum of cell weights",
            value: sum,
            min: 1.0,
            max: 1.0,
        });
    }
    let total: u64 = observed_counts
        .iter()
        .try_fold(0u64, |acc, &n| acc.checked_add(n))
        .ok_or(CoreError::Overflow {
            what: "observed task count",
        })?;
    if total == 0 {
        return Ok(None);
    }
    let tv = frozen_omega
        .iter()
        .zip(observed_counts)
        .map(|(&w, &n)| (w - n as f64 / total as f64).abs())
        .sum::<f64>()
        / 2.0;
    Ok(Some(tv))
}

/// §A / §9: alert "task mix changed noticeably" when TV distance > 0.3.
pub fn drift_alert(total_variation: f64) -> bool {
    total_variation > DRIFT_TV_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_of_and_bounds() {
        let s = WindowSchedule::with_default_length(1_000);
        assert_eq!(s.length_days(), 30);
        let len = 30 * SECONDS_PER_DAY;
        assert_eq!(s.window_of(1_000), Ok(WindowIndex(0)));
        assert_eq!(s.window_of(1_000 + len - 1), Ok(WindowIndex(0)));
        assert_eq!(s.window_of(1_000 + len), Ok(WindowIndex(1)));
        assert_eq!(s.bounds(WindowIndex(1)), Ok((1_000 + len, 1_000 + 2 * len)));
    }

    #[test]
    fn bad_window_inputs_are_errors() {
        assert!(WindowSchedule::new(0, 0).is_err());
        let s = WindowSchedule::new(100, 1).unwrap();
        assert!(matches!(
            s.window_of(99),
            Err(CoreError::BeforeOrigin { .. })
        ));
        let far = WindowSchedule::new(0, 1).unwrap();
        assert!(matches!(
            far.window_of(i64::MAX),
            Err(CoreError::Overflow { .. })
        ));
        let late = WindowSchedule::new(i64::MAX - 10, 1).unwrap();
        assert!(matches!(
            late.bounds(WindowIndex(0)),
            Err(CoreError::Overflow { .. })
        ));
        assert!(WindowIndex(u32::MAX).next().is_err());
    }

    #[test]
    fn drift_total_variation() {
        let tv = task_mix_total_variation(&[0.5, 0.5], &[5, 5]).unwrap();
        assert_eq!(tv, Some(0.0));
        let tv = task_mix_total_variation(&[0.5, 0.5], &[9, 1])
            .unwrap()
            .unwrap();
        assert!((tv - 0.4).abs() < 1e-12);
        assert!(drift_alert(tv));
        assert!(!drift_alert(0.3));
        assert_eq!(task_mix_total_variation(&[1.0], &[0]), Ok(None));
        assert!(task_mix_total_variation(&[0.5], &[1, 2]).is_err());
        assert!(task_mix_total_variation(&[0.7, 0.7], &[1, 1]).is_err());
        assert!(task_mix_total_variation(&[-0.5, 1.5], &[1, 1]).is_err());
        assert!(task_mix_total_variation(&[f64::NAN, 1.0], &[1, 1]).is_err());
    }
}

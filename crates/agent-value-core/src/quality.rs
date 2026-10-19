//! Quality signals (v22 §3.1, §3).
//!
//! Quality `q_j` in `[0, 1]` exists only for successful tasks and comes only
//! from user ratings: explicit 1-5 stars, `q = (s - 1) / 4`, plus spot
//! ratings asked for on a random 10% of successful tasks, at most 3 per day.
//!
//! Each layer, each window, runs in one of two modes:
//! - rating mode: every candidate arm of the layer's `E_d` has at least
//!   `n_q >= 3` ratings on every positive-weight cell (v22 §3.1, literal
//!   wording: the reference and fallback arms are gated only when they are
//!   themselves candidates); then
//!   `q_hat = clip((sum q_j + m0 q_B) / (n_q + m0), 0.01, 0.99)`, where
//!   `q_B` is the pooled mean of all arms' ratings in the same cell in the
//!   previous window (0.5 when there are none);
//! - success-rate mode otherwise: every arm and the reference use
//!   `q_hat = 0.99`, so quality reads as "delivery success rate".

use crate::arm::Arm;
use crate::cells::{CellId, CellWeights};
use crate::constants::{M0, RATING_MODE_MIN_RATINGS, SPOT_RATING_MAX_PER_DAY, SPOT_RATING_RATE};
use crate::error::{CoreError, Result};
use crate::records::Outcome;

/// Lower clip of `q_hat`.
pub const Q_HAT_MIN: f64 = 0.01;
/// Upper clip of `q_hat`; also the success-rate-mode value.
pub const Q_HAT_MAX: f64 = 0.99;
/// `q_B` when the previous window has no ratings in the cell.
pub const Q_B_DEFAULT: f64 = 0.5;
/// Label shown when a layer runs in success-rate mode.
pub const SUCCESS_RATE_MODE_LABEL: &str = "quality = delivery success rate (not enough ratings)";

/// Maps a 1-5 star rating to `q = (s - 1) / 4`.
pub fn stars_to_quality(stars: u8) -> Result<f64> {
    if !(1..=5).contains(&stars) {
        return Err(CoreError::OutOfRange {
            what: "rating stars",
            value: f64::from(stars),
            min: 1.0,
            max: 5.0,
        });
    }
    Ok(f64::from(stars - 1) / 4.0)
}

/// One user rating of a task (one per task; the latest rating wins and is
/// what the caller passes). `outcome` is the task's current outcome: a
/// rating counts only while that is success, so a retroactive failure
/// drops it automatically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rating {
    pub arm: Arm,
    pub cell: CellId,
    pub outcome: Outcome,
    pub stars: u8,
}

impl Rating {
    fn counts(&self) -> bool {
        self.outcome == Outcome::Success && !self.cell.is_probe()
    }
}

/// The qualities `q_j` of the counted ratings of `arm` on `cell`.
pub fn qualities(ratings: &[Rating], arm: &Arm, cell: &CellId) -> Result<Vec<f64>> {
    ratings
        .iter()
        .filter(|r| r.counts() && &r.arm == arm && &r.cell == cell)
        .map(|r| stars_to_quality(r.stars))
        .collect()
}

/// Quality mode of one layer in one window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityMode {
    Rating,
    SuccessRate,
}

impl QualityMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rating => "rating",
            Self::SuccessRate => "success-rate",
        }
    }

    /// The label to show next to the layer's quality, if any.
    pub fn label(self) -> Option<&'static str> {
        match self {
            Self::Rating => None,
            Self::SuccessRate => Some(SUCCESS_RATE_MODE_LABEL),
        }
    }
}

/// Decides the layer's quality mode from this window's ratings. The gate
/// covers exactly the candidate arms of the layer's `E_d`, on every cell
/// with positive weight (§3.1). The reference arm is not added to the gate:
/// its `q_hat` only enters the numerator shared by all arms. No candidates
/// or no positive-weight cell means success-rate mode.
pub fn quality_mode(
    candidates: &[Arm],
    cells: &CellWeights,
    ratings: &[Rating],
) -> Result<QualityMode> {
    let support: Vec<&CellId> = cells.support().collect();
    if candidates.is_empty() || support.is_empty() {
        return Ok(QualityMode::SuccessRate);
    }
    for arm in candidates {
        for cell in &support {
            let n = qualities(ratings, arm, cell)?.len();
            if n < RATING_MODE_MIN_RATINGS as usize {
                return Ok(QualityMode::SuccessRate);
            }
        }
    }
    Ok(QualityMode::Rating)
}

/// `q_B`: pooled mean quality of all arms' counted ratings on `cell` in the
/// previous window; 0.5 when there are none.
pub fn pooled_prior(previous_window: &[Rating], cell: &CellId) -> Result<f64> {
    let mut sum = 0.0;
    let mut n = 0usize;
    for r in previous_window
        .iter()
        .filter(|r| r.counts() && &r.cell == cell)
    {
        sum += stars_to_quality(r.stars)?;
        n += 1;
    }
    Ok(if n == 0 { Q_B_DEFAULT } else { sum / n as f64 })
}

/// Shrunk quality `q_hat = clip((sum q + m0 q_B) / (n + m0), 0.01, 0.99)`.
pub fn shrunk_quality(qualities: &[f64], q_b: f64) -> Result<f64> {
    for &q in qualities.iter().chain(std::iter::once(&q_b)) {
        if !q.is_finite() {
            return Err(CoreError::NotFinite {
                what: "quality",
                value: q,
            });
        }
        if !(0.0..=1.0).contains(&q) {
            return Err(CoreError::OutOfRange {
                what: "quality",
                value: q,
                min: 0.0,
                max: 1.0,
            });
        }
    }
    let sum: f64 = qualities.iter().sum();
    let raw = (sum + M0 * q_b) / (qualities.len() as f64 + M0);
    Ok(raw.clamp(Q_HAT_MIN, Q_HAT_MAX))
}

/// `q_hat` of `arm` on `cell` under the layer's `mode`.
pub fn q_hat(
    mode: QualityMode,
    arm: &Arm,
    cell: &CellId,
    window: &[Rating],
    previous_window: &[Rating],
) -> Result<f64> {
    match mode {
        QualityMode::SuccessRate => Ok(Q_HAT_MAX),
        QualityMode::Rating => {
            let qs = qualities(window, arm, cell)?;
            shrunk_quality(&qs, pooled_prior(previous_window, cell)?)
        }
    }
}

/// Spot-rating decision for one eligible task: ask when the task's outcome
/// is success, the uniform draw `u` in `[0, 1)` is below 10%, and fewer
/// than 3 asks were made on the same Beijing-time day. The random source
/// stays outside the core.
pub fn should_ask_spot_rating(outcome: Outcome, u: f64, asked_today: u32) -> Result<bool> {
    if !u.is_finite() {
        return Err(CoreError::NotFinite {
            what: "spot rating draw",
            value: u,
        });
    }
    if !(0.0..1.0).contains(&u) {
        return Err(CoreError::OutOfRange {
            what: "spot rating draw",
            value: u,
            min: 0.0,
            max: 1.0,
        });
    }
    Ok(
        outcome == Outcome::Success
            && u < SPOT_RATING_RATE
            && asked_today < SPOT_RATING_MAX_PER_DAY,
    )
}

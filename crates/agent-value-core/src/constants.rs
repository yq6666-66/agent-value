//! Numeric constants from v22 §9, transcribed exactly.
//!
//! Each item cites its §9 row. `tests/constants_table.rs` checks every row of
//! a verbatim copy of the §9 table against these values.
//!
//! Units: money in CNY (yuan), time in seconds unless the name says otherwise.

use crate::error::{finite, CoreError, Result};

/// §9 `c_0`: cost noise floor, 0.15 yuan.
pub const C0_YUAN: f64 = 0.15;
/// §9 `t_0`: time noise floor, 15 seconds.
pub const T0_SECONDS: f64 = 15.0;
/// §9 `epsilon`: quality-defect noise floor, 0.02.
pub const EPSILON: f64 = 0.02;
/// §9 `a, b, g`: default exponent for each dimension, 1/3 each (a + b + g = 1).
pub const DEFAULT_WEIGHT: f64 = 1.0 / 3.0;
/// §9 `K`: maximum attempts in the self-built segment.
pub const K_MAX_ATTEMPTS: u32 = 3;
/// §9 `K` row: worst-case total attempts per task, 2K = 6.
pub const WORST_CASE_TOTAL_ATTEMPTS: u32 = 2 * K_MAX_ATTEMPTS;
// §9 `pi` (conditional retry success rate) is "estimated from data": no constant.
/// §9 `kappa_C`: shrinkage strength of cost/time towards the benchmark.
pub const KAPPA_C: f64 = 5.0;
/// §9 `kappa_F`: shrinkage strength of fallback quality.
pub const KAPPA_F: f64 = 5.0;
/// §9 `m_0`: shrinkage strength of the quality score.
pub const M0: f64 = 0.5;
/// §9 `kappa_p = 20 rho`: the factor 20. Use [`kappa_p`] to evaluate.
pub const KAPPA_P_PER_RHO: f64 = 20.0;
/// §9 `theta`: relative clamp width of the tier-II anchor (unrelated to the
/// Beta posterior's alpha, beta in §1).
pub const THETA: f64 = 0.10;
/// §9 "valid reference cells gate": at least 3, otherwise the anchor is
/// undefined and the layer falls to tier III.
pub const MIN_VALID_REFERENCE_CELLS: usize = 3;
/// §9 cancel-rate hard gate: `r_C <= 0.20`.
pub const CANCEL_RATE_MAX: f64 = 0.20;
/// §9 completion-rate hard gate: 0.50.
pub const COMPLETION_RATE_MIN: f64 = 0.50;
/// §9 sample gate `max(30, 3 U_d)`: the floor 30.
pub const SAMPLE_GATE_FLOOR: usize = 30;
/// §9 sample gate `max(30, 3 U_d)`: the per-cell multiplier 3.
pub const SAMPLE_GATE_PER_CELL: usize = 3;
/// §9 lower bound on each of a, b, g: 0.05.
pub const WEIGHT_FLOOR: f64 = 0.05;
/// §9 time window: 30 days.
pub const WINDOW_DAYS: u32 = 30;
/// §9 `tau_re`: 30-minute observation period for "redo means failure".
pub const TAU_RE_MINUTES: u32 = 30;
/// §9 `tau_keep`: 24-hour observation period for retroactive rollback failure.
pub const TAU_KEEP_HOURS: u32 = 24;
/// §9 spot-rating rate: 10% of tasks.
pub const SPOT_RATING_RATE: f64 = 0.10;
/// §9 spot-rating cap: at most 3 per day.
pub const SPOT_RATING_MAX_PER_DAY: u32 = 3;
/// §9 rating-mode threshold: `n_q >= 3` per arm per cell, else success-rate mode.
pub const RATING_MODE_MIN_RATINGS: u32 = 3;
/// DECISIONS G item 4: a user-specified fallback arm needs `n_q >= 3`
/// quality ratings in every positive-weight cell of the previous window.
pub const FALLBACK_MIN_RATINGS_PER_CELL: u32 = 3;
/// §9 five-level self-report cut point `delta_1` (in units of the question's Delta).
pub const SELF_REPORT_DELTA1: f64 = 0.25;
/// §9 five-level self-report cut point `delta_2` (in units of the question's Delta).
pub const SELF_REPORT_DELTA2: f64 = 0.50;
/// §9 self-report stop rule: at least 4 answered questions ...
pub const SELF_REPORT_MIN_QUESTIONS: usize = 4;
/// ... and `|Omega_s| <= 15` grid points ...
pub const SELF_REPORT_MAX_REGION_POINTS: usize = 15;
/// ... or 10 answered questions.
pub const SELF_REPORT_MAX_QUESTIONS: usize = 10;
/// §9 `Omega_s` tolerates one question answered one level off.
pub const SELF_REPORT_REGION_SLACK_LEVELS: u32 = 1;
/// §9 `n_ev`: minimum behaviour events for calibration.
pub const N_EV: usize = 20;
/// §9 behaviour no-signal gate: `lambda_hat < 0.5` ...
pub const BEHAVIOR_MIN_LAMBDA: f64 = 0.5;
/// ... or attribute-difference matrix rank `< 3`.
pub const BEHAVIOR_MIN_RANK: usize = 3;
/// §9 grid step 0.05.
pub const GRID_STEP: f64 = 0.05;
/// §9 grid size: 171 points with every dimension >= 0.05.
pub const GRID_POINTS: usize = 171;
/// §9 deterministic probe class: one run of 10 questions ...
pub const PROBE_QUESTIONS_PER_RUN: u32 = 10;
/// ... passes with at least 9 correct. Provisional (暂定) until the §11.3
/// observation period validates it (DECISIONS 13.1).
pub const DETERMINISTIC_PASS_MIN: u32 = 9;
/// §9 fingerprint probe class: at least 8 of 10 questions with JS <= tau_JS
/// (needs an official baseline; tau_JS itself is not fixed in §9).
/// Provisional (暂定) until the §11.3 observation period validates it
/// (DECISIONS 13.1).
pub const FINGERPRINT_PASS_MIN: u32 = 8;
/// §9 authenticity-gate statistical unit: the run day, a natural day in
/// Beijing time. This is the UTC offset of Beijing time in seconds (UTC+8).
pub const RUN_DAY_UTC_OFFSET_SECONDS: i32 = 8 * 3600;
/// §9 returned-model-version drift ceiling: 0.05 (by request count).
pub const MODEL_VERSION_DRIFT_MAX: f64 = 0.05;
/// §9 log parse-rate floor: 0.95.
pub const LOG_PARSE_RATE_MIN: f64 = 0.95;
/// §9 probe monthly budget: 10 yuan by default; probing stops when exceeded.
pub const PROBE_MONTHLY_BUDGET_YUAN: f64 = 10.0;
/// §9 statistical-robustness bootstrap resamples: 200.
pub const BOOTSTRAP_RESAMPLES: u32 = 200;
/// §9 drift alert gate: total variation distance `> 0.3`.
pub const DRIFT_TV_THRESHOLD: f64 = 0.3;

/// §9 `kappa_p = 20 rho` (§1). `rho` must be finite and in `[0, 1]`.
pub fn kappa_p(rho: f64) -> Result<f64> {
    let rho = finite("rho", rho)?;
    if !(0.0..=1.0).contains(&rho) {
        return Err(CoreError::OutOfRange {
            what: "rho",
            value: rho,
            min: 0.0,
            max: 1.0,
        });
    }
    Ok(KAPPA_P_PER_RHO * rho)
}

/// §9 layer sample gate `max(30, 3 U_d)` for `U_d` positive-weight cells.
pub fn layer_sample_gate(u_d: usize) -> usize {
    SAMPLE_GATE_FLOOR.max(SAMPLE_GATE_PER_CELL.saturating_mul(u_d))
}

/// §9 arm sample gate `max(30, 3 U_i)` for `U_i` positive-weight cells.
pub fn arm_sample_gate(u_i: usize) -> usize {
    SAMPLE_GATE_FLOOR.max(SAMPLE_GATE_PER_CELL.saturating_mul(u_i))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kappa_p_scales_rho() {
        assert_eq!(kappa_p(1.0), Ok(20.0));
        assert_eq!(kappa_p(0.0), Ok(0.0));
        assert_eq!(kappa_p(0.25), Ok(5.0));
    }

    #[test]
    fn kappa_p_rejects_bad_rho() {
        assert!(matches!(kappa_p(-0.1), Err(CoreError::OutOfRange { .. })));
        assert!(matches!(kappa_p(1.5), Err(CoreError::OutOfRange { .. })));
        assert!(matches!(
            kappa_p(f64::NAN),
            Err(CoreError::NotFinite { .. })
        ));
    }

    #[test]
    fn sample_gates() {
        assert_eq!(layer_sample_gate(0), 30);
        assert_eq!(layer_sample_gate(5), 30);
        assert_eq!(layer_sample_gate(10), 30);
        assert_eq!(layer_sample_gate(12), 36);
        assert_eq!(arm_sample_gate(11), 33);
        assert_eq!(arm_sample_gate(usize::MAX), usize::MAX);
    }
}

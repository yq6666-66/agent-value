//! Diagnostic table for arms with miss or fail cells (v22 §5; HANDOFF step 9).
//!
//! - Admission: `ESS = n_comp + κ_eff ≥ 3` and at least one valid cell
//!   (DECISIONS O: `κ_eff` = sum of per-cell prior κ over present `d+` rows).
//! - Miss cells: optimistic = weighted median of valid cells (DECISIONS E);
//!   conservative = worst valid per dimension (DECISIONS F).
//! - Fail cells: worse of measured and worst-valid, per dimension.
//! - Fill-in uses plain `(C*, T*, Q*)`, not cancel-robust (DECISIONS P).
//! - Sort key: `cov = 1 - w_miss - w_fail` descending, then model name ascending.
//!
//! Aggregates on [`DiagnosticRow`] are **post-fill**. The raw measured layer
//! mean (example 8's `A_C = 0.165`, or example 6's unclamped diagnostic
//! anchor when fail-fill leaves the measured thin cost in place) is computed
//! by the caller from the measured triples, not stored here.

use crate::cell_values::Triple;
use crate::cells::{CellId, CellWeights};
use crate::error::{CoreError, Result};
use crate::retry::Prior;
use crate::status::{CellStatus, LayerStatus};
use crate::value::{value_score, Floors, Point};
use crate::weights::Weights;

/// Tolerance on the cumulative-weight half for the lower median (DECISIONS E).
const MEDIAN_HALF: f64 = 0.5;
const MEDIAN_EPS: f64 = 1e-9;

/// One valid cell's plain triple and its layer weight.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidCell {
    pub cell: CellId,
    pub weight: f64,
    pub triple: Triple,
}

/// Filled values for one miss or fail cell.
#[derive(Debug, Clone, PartialEq)]
pub struct FilledCell {
    pub cell: CellId,
    pub status: CellStatus,
    /// Optimistic fill (miss only); equal to point for fail.
    pub optimistic: Triple,
    /// Conservative fill (miss) or the single fail fill.
    pub conservative: Triple,
}

/// Outcome of diagnosing one arm in one layer.
#[derive(Debug, Clone, PartialEq)]
pub struct DiagnosticRow {
    /// `ESS = n_comp + κ_eff`.
    pub ess: f64,
    pub admitted: bool,
    pub cov: f64,
    pub w_miss: f64,
    pub w_fail: f64,
    pub n_valid: usize,
    /// Per-cell fills when admitted; empty when not.
    pub filled: Vec<FilledCell>,
    /// Layer aggregate from optimistic fills (miss→opt, fail→point, valid→as-is).
    pub aggregate_opt: Option<Triple>,
    /// Layer aggregate from conservative fills.
    pub aggregate_cons: Option<Triple>,
    /// `V` from optimistic aggregate vs the caller-supplied reference, when both complete.
    pub v_opt: Option<f64>,
    /// `V` from conservative aggregate.
    pub v_cons: Option<f64>,
}

/// `κ_eff`: sum of prior κ over present rows whose cell is in `d+` (DECISIONS O).
pub fn kappa_eff(weights: &CellWeights, present: &[(CellId, Prior)]) -> Result<f64> {
    let mut seen = std::collections::BTreeSet::new();
    let mut sum = 0.0;
    for (cell, prior) in present {
        if weights.omega(cell) <= 0.0 {
            continue;
        }
        if !seen.insert(cell.clone()) {
            return Err(CoreError::DuplicateCell {
                cell: cell.as_str().to_string(),
            });
        }
        let k = prior.kappa()?;
        if !k.is_finite() || k < 0.0 {
            return Err(CoreError::OutOfRange {
                what: "kappa",
                value: k,
                min: 0.0,
                max: f64::INFINITY,
            });
        }
        sum += k;
    }
    Ok(sum)
}

/// Admission gate: `n_comp + κ_eff ≥ 3` and at least one valid cell.
pub fn diagnostic_admitted(status: &LayerStatus, kappa_eff: f64) -> bool {
    let ess = status.n_comp as f64 + kappa_eff;
    ess >= 3.0 && status.n_valid >= 1
}

/// Coverage `cov = 1 - w_miss - w_fail`.
pub fn coverage(status: &LayerStatus) -> f64 {
    1.0 - status.w_miss - status.w_fail
}

/// Lower weighted median (DECISIONS E).
///
/// `items` are `(weight, value)` with positive weights. Values are sorted
/// ascending; the first cumulative weight ≥ `0.5 - 1e-9` of the total wins.
pub fn weighted_median(items: &[(f64, f64)]) -> Result<Option<f64>> {
    let mut parts: Vec<(f64, f64)> = Vec::new();
    let mut total = 0.0;
    for &(w, v) in items {
        if !w.is_finite() || w < 0.0 {
            return Err(CoreError::OutOfRange {
                what: "median weight",
                value: w,
                min: 0.0,
                max: f64::INFINITY,
            });
        }
        if !v.is_finite() {
            return Err(CoreError::NotFinite {
                what: "median value",
                value: v,
            });
        }
        if w == 0.0 {
            continue;
        }
        total += w;
        parts.push((w, v));
    }
    if parts.is_empty() || total <= 0.0 {
        return Ok(None);
    }
    parts.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    let threshold = (MEDIAN_HALF - MEDIAN_EPS) * total;
    let mut acc = 0.0;
    for &(w, v) in &parts {
        acc += w;
        if acc >= threshold {
            return Ok(Some(v));
        }
    }
    Ok(parts.last().map(|(_, v)| *v))
}

/// Optimistic fill for a miss cell: per-dimension weighted median of valid cells.
/// Quality uses defect `D = 1 - Q` then maps back (DECISIONS E).
pub fn optimistic_miss(valid: &[ValidCell]) -> Result<Option<Triple>> {
    if valid.is_empty() {
        return Ok(None);
    }
    let mut c_items = Vec::new();
    let mut t_items = Vec::new();
    let mut d_items = Vec::new();
    for v in valid {
        let Some(c) = v.triple.c else {
            return Ok(None);
        };
        let Some(t) = v.triple.t else {
            return Ok(None);
        };
        let Some(q) = v.triple.q else {
            return Ok(None);
        };
        c_items.push((v.weight, c));
        t_items.push((v.weight, t));
        d_items.push((v.weight, 1.0 - q));
    }
    let c = match weighted_median(&c_items)? {
        Some(v) => v,
        None => return Ok(None),
    };
    let t = match weighted_median(&t_items)? {
        Some(v) => v,
        None => return Ok(None),
    };
    let d = match weighted_median(&d_items)? {
        Some(v) => v,
        None => return Ok(None),
    };
    Ok(Some(Triple::new(c, t, 1.0 - d)))
}

/// Conservative / worst-valid fill (DECISIONS F): max C, max T, min Q.
pub fn worst_valid(valid: &[ValidCell]) -> Option<Triple> {
    let mut c_max: Option<f64> = None;
    let mut t_max: Option<f64> = None;
    let mut q_min: Option<f64> = None;
    for v in valid {
        if v.weight <= 0.0 {
            continue;
        }
        let (Some(c), Some(t), Some(q)) = (v.triple.c, v.triple.t, v.triple.q) else {
            return None;
        };
        c_max = Some(c_max.map_or(c, |m| m.max(c)));
        t_max = Some(t_max.map_or(t, |m| m.max(t)));
        q_min = Some(q_min.map_or(q, |m| m.min(q)));
    }
    Some(Triple {
        c: c_max,
        t: t_max,
        q: q_min,
    })
}

/// Fail-cell fill: worse of measured and worst-valid, per dimension.
pub fn fail_fill(measured: &Triple, worst: &Triple) -> Option<Triple> {
    let c = match (measured.c, worst.c) {
        (Some(m), Some(w)) => Some(m.max(w)),
        _ => return None,
    };
    let t = match (measured.t, worst.t) {
        (Some(m), Some(w)) => Some(m.max(w)),
        _ => return None,
    };
    let q = match (measured.q, worst.q) {
        (Some(m), Some(w)) => Some(m.min(w)),
        _ => return None,
    };
    Some(Triple { c, t, q })
}

fn finite_weight(w: f64) -> Result<f64> {
    if !w.is_finite() || w < 0.0 {
        return Err(CoreError::OutOfRange {
            what: "cell weight",
            value: w,
            min: 0.0,
            max: f64::INFINITY,
        });
    }
    Ok(w)
}

/// Builds the diagnostic row for one arm.
///
/// `cell_triples` must cover every `d+` cell that is not miss (valid and fail
/// need a measured plain triple). Miss cells may be omitted. `reference` is
/// the diagnostic reference aggregate (example 6: unclamped `A_C`); when
/// `None`, `V` fields stay `None`.
pub fn diagnose_arm(
    status: &LayerStatus,
    weights: &CellWeights,
    cell_triples: &[(CellId, Triple)],
    present_priors: &[(CellId, Prior)],
    reference: Option<Point>,
    pref: &Weights,
    floors: &Floors,
) -> Result<DiagnosticRow> {
    let k_eff = kappa_eff(weights, present_priors)?;
    let ess = status.n_comp as f64 + k_eff;
    let cov = coverage(status);
    let admitted = diagnostic_admitted(status, k_eff);

    let mut triple_map = std::collections::BTreeMap::new();
    for (c, t) in cell_triples {
        if triple_map.insert(c.clone(), *t).is_some() {
            return Err(CoreError::DuplicateCell {
                cell: c.as_str().to_string(),
            });
        }
    }

    let mut valid = Vec::new();
    for (cell, assess) in &status.cells {
        if assess.status != CellStatus::Valid {
            continue;
        }
        let w = finite_weight(weights.omega(cell))?;
        let Some(t) = triple_map.get(cell).cloned() else {
            // Valid without a triple → cannot fill; treat as not admitted path.
            return Ok(DiagnosticRow {
                ess,
                admitted: false,
                cov,
                w_miss: status.w_miss,
                w_fail: status.w_fail,
                n_valid: status.n_valid,
                filled: Vec::new(),
                aggregate_opt: None,
                aggregate_cons: None,
                v_opt: None,
                v_cons: None,
            });
        };
        if !t.is_complete() {
            return Ok(DiagnosticRow {
                ess,
                admitted: false,
                cov,
                w_miss: status.w_miss,
                w_fail: status.w_fail,
                n_valid: status.n_valid,
                filled: Vec::new(),
                aggregate_opt: None,
                aggregate_cons: None,
                v_opt: None,
                v_cons: None,
            });
        }
        valid.push(ValidCell {
            cell: cell.clone(),
            weight: w,
            triple: t,
        });
    }

    if !admitted {
        return Ok(DiagnosticRow {
            ess,
            admitted: false,
            cov,
            w_miss: status.w_miss,
            w_fail: status.w_fail,
            n_valid: status.n_valid,
            filled: Vec::new(),
            aggregate_opt: None,
            aggregate_cons: None,
            v_opt: None,
            v_cons: None,
        });
    }

    let opt_miss = optimistic_miss(&valid)?;
    let worst = worst_valid(&valid);
    let (Some(opt_miss), Some(worst)) = (opt_miss, worst) else {
        return Ok(DiagnosticRow {
            ess,
            admitted: false,
            cov,
            w_miss: status.w_miss,
            w_fail: status.w_fail,
            n_valid: status.n_valid,
            filled: Vec::new(),
            aggregate_opt: None,
            aggregate_cons: None,
            v_opt: None,
            v_cons: None,
        });
    };

    let mut filled = Vec::new();
    let mut agg_opt_parts = Vec::new();
    let mut agg_cons_parts = Vec::new();

    for (cell, assess) in &status.cells {
        let w = finite_weight(weights.omega(cell))?;
        match assess.status {
            CellStatus::Valid => {
                let t = triple_map.get(cell).cloned().unwrap();
                agg_opt_parts.push((w, t));
                agg_cons_parts.push((w, t));
            }
            CellStatus::Miss => {
                filled.push(FilledCell {
                    cell: cell.clone(),
                    status: CellStatus::Miss,
                    optimistic: opt_miss,
                    conservative: worst,
                });
                agg_opt_parts.push((w, opt_miss));
                agg_cons_parts.push((w, worst));
            }
            CellStatus::FailThin | CellStatus::FailBad => {
                let Some(measured) = triple_map.get(cell).copied() else {
                    return Err(CoreError::InvalidField {
                        field: "fail cell triple",
                        reason: "fail cells need a complete measured plain triple",
                    });
                };
                if !measured.is_complete() {
                    return Err(CoreError::InvalidField {
                        field: "fail cell triple",
                        reason: "fail cells need a complete measured plain triple",
                    });
                }
                let Some(point) = fail_fill(&measured, &worst) else {
                    return Err(CoreError::InvalidField {
                        field: "fail cell triple",
                        reason: "fail fill requires complete measured and worst-valid triples",
                    });
                };
                filled.push(FilledCell {
                    cell: cell.clone(),
                    status: assess.status,
                    optimistic: point,
                    conservative: point,
                });
                agg_opt_parts.push((w, point));
                agg_cons_parts.push((w, point));
            }
        }
    }

    let aggregate_opt = weighted_triple(&agg_opt_parts)?;
    let aggregate_cons = weighted_triple(&agg_cons_parts)?;

    let v_opt = match (reference, Point::from_triple(&aggregate_opt)) {
        (Some(r), Some(a)) => Some(value_score(r, a, pref, floors)?),
        _ => None,
    };
    let v_cons = match (reference, Point::from_triple(&aggregate_cons)) {
        (Some(r), Some(a)) => Some(value_score(r, a, pref, floors)?),
        _ => None,
    };

    Ok(DiagnosticRow {
        ess,
        admitted: true,
        cov,
        w_miss: status.w_miss,
        w_fail: status.w_fail,
        n_valid: status.n_valid,
        filled,
        aggregate_opt: Some(aggregate_opt),
        aggregate_cons: Some(aggregate_cons),
        v_opt,
        v_cons,
    })
}

fn weighted_triple(parts: &[(f64, Triple)]) -> Result<Triple> {
    let mut sum_w = 0.0;
    let mut sc = 0.0;
    let mut st = 0.0;
    let mut sq = 0.0;
    for &(w, ref t) in parts {
        let (Some(c), Some(t_), Some(q)) = (t.c, t.t, t.q) else {
            return Ok(Triple::NA);
        };
        sum_w += w;
        sc += w * c;
        st += w * t_;
        sq += w * q;
    }
    if sum_w <= 0.0 {
        return Ok(Triple::NA);
    }
    Ok(Triple::new(sc / sum_w, st / sum_w, sq / sum_w))
}

/// Sort diagnostic rows by `cov` descending, then by `model_name` ascending.
pub fn sort_diagnostic_rows<T>(rows: &mut [(String, T, DiagnosticRow)]) {
    rows.sort_by(|a, b| {
        b.2.cov
            .partial_cmp(&a.2.cov)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
}

#[cfg(test)]
mod median_tests {
    use super::*;

    #[test]
    fn decisions_e_equal_weight_cost_median_is_0_6() {
        let items = [(1.0, 0.4), (1.0, 0.6), (1.0, 0.9), (1.0, 1.5)];
        assert_eq!(weighted_median(&items).unwrap(), Some(0.6));
    }

    #[test]
    fn decisions_e_equal_weight_quality_via_defect() {
        // Q = [0.9, 0.8, 0.7, 0.6] → D = [0.1, 0.2, 0.3, 0.4]; lower median D = 0.2 → Q = 0.8
        let d = [(1.0, 0.1), (1.0, 0.2), (1.0, 0.3), (1.0, 0.4)];
        assert_eq!(weighted_median(&d).unwrap(), Some(0.2));
    }

    #[test]
    fn decisions_e_uneven_weights_take_third() {
        let items = [(0.1, 1.0), (0.2, 2.0), (0.3, 3.0), (0.4, 4.0)];
        // sorted by value already; cum: 0.1, 0.3, 0.6 ≥ 0.5-eps → 3.0
        assert_eq!(weighted_median(&items).unwrap(), Some(3.0));
    }

    #[test]
    fn decisions_e_twelve_equal_weights_stable() {
        let items: Vec<(f64, f64)> = (0..12).map(|i| (1.0 / 12.0, i as f64)).collect();
        // half of 1 is 0.5; cum after 6th (0-indexed 5) = 6/12 = 0.5 ≥ 0.5-eps → value 5
        assert_eq!(weighted_median(&items).unwrap(), Some(5.0));
    }

    #[test]
    fn decisions_f_worst_valid_per_dimension() {
        let valid = vec![
            ValidCell {
                cell: CellId::user("a").unwrap(),
                weight: 0.5,
                triple: Triple::new(0.5, 30.0, 0.9),
            },
            ValidCell {
                cell: CellId::user("b").unwrap(),
                weight: 0.5,
                triple: Triple::new(0.8, 20.0, 0.7),
            },
        ];
        let w = worst_valid(&valid).unwrap();
        assert_eq!(w, Triple::new(0.8, 30.0, 0.7));
    }
}

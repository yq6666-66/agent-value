//! Reference anchor certification, tiers I / II / III (v22 §7; HANDOFF
//! step 10).
//!
//! - The reference uses the same miss / fail / valid and `F_thin` / `F_bad`
//!   classification as candidates ([`crate::status::layer_status`]).
//! - Tiers are decided in the order III → II → I; the first match wins
//!   (DECISIONS T). A layer with fewer than 3 valid reference cells is
//!   tier III even when every cell is valid.
//! - Tier II clamps the layer anchor, never cell values:
//!   `X^anchor = clip(A_X, (1−θ) a_X, (1+θ) a_X)` for cost and time, and
//!   `Q^anchor = 1 − clip(A_D, (1−θ) a_D, (1+θ) a_D)` with `D = 1 − Q`.
//! - The diagnostic reference anchor is unclamped and uses the symmetric
//!   worst value per cell (DECISIONS Y).

use std::collections::BTreeMap;

use crate::cell_values::Triple;
use crate::cells::{CellId, CellWeights};
use crate::constants::{MIN_VALID_REFERENCE_CELLS, THETA};
use crate::error::{CoreError, Result};
use crate::status::{CellStatus, LayerStatus};
use crate::value::{aggregate_layer, value_score, Floors, Point};
use crate::weights::Weights;

/// Certification tier of the reference in one layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AnchorTier {
    /// Fully certified: anchor = measured layer values.
    I,
    /// Clamped: the anchor is clipped to `θ` around the valid-cell mean.
    II,
    /// Not certified: no `V` in this layer.
    III,
}

/// Why a layer is tier III (spec order; every match is listed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TierIIIReason {
    /// `N^miss_{R,d} > 0`.
    Miss,
    /// `N^bad_{R,d} > 0`.
    Bad,
    /// Some `d+` cell has an NA dimension.
    Na,
    /// Fewer than 3 valid reference cells.
    FewValid,
}

/// Clamp details of one dimension (tier II). For quality the values are
/// defects `D = 1 − Q`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DimClamp {
    /// `A_X = Σ_{d+} ω_u X_u`.
    pub measured: f64,
    /// `a_X = Σ_valid ω_u X_u / Σ_valid ω_u`.
    pub valid_mean: f64,
    pub lower: f64,
    pub upper: f64,
    /// `clip(A_X, lower, upper)`.
    pub clipped: f64,
    /// True when `A_X` was outside the window.
    pub clamped: bool,
}

/// Result of certifying the reference in one layer.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerAnchor {
    pub tier: AnchorTier,
    /// Non-empty iff `tier == III`.
    pub tier_iii_reasons: Vec<TierIIIReason>,
    /// Anchor `(C, T, Q)` used as the `V` numerator; `None` in tier III.
    pub anchor: Option<Point>,
    /// Measured layer values `(A_C, A_T, 1 − A_D)` when complete.
    pub measured: Option<Point>,
    /// Tier II clamp metadata for cost, time and defect.
    pub clamp: Option<[DimClamp; 3]>,
    pub w_miss: f64,
    pub w_thin: f64,
    pub w_bad: f64,
}

fn cell_map(cells: &[(CellId, Triple)]) -> Result<BTreeMap<&CellId, &Triple>> {
    let mut map = BTreeMap::new();
    for (c, t) in cells {
        if map.insert(c, t).is_some() {
            return Err(CoreError::DuplicateCell {
                cell: c.as_str().to_string(),
            });
        }
    }
    Ok(map)
}

/// `clip(x, lo, hi) = min(max(x, lo), hi)`.
pub fn clip(x: f64, lo: f64, hi: f64) -> f64 {
    x.max(lo).min(hi)
}

fn dim_clamp(measured: f64, valid_mean: f64) -> DimClamp {
    let lower = (1.0 - THETA) * valid_mean;
    let upper = (1.0 + THETA) * valid_mean;
    let clipped = clip(measured, lower, upper);
    DimClamp {
        measured,
        valid_mean,
        lower,
        upper,
        clipped,
        clamped: clipped != measured,
    }
}

/// Certifies the reference in one layer (§7). `status` is the reference's
/// [`LayerStatus`]; `reference_cells` are its cell `(C*, T*, Q*)`. A `d+`
/// cell without a row counts as NA.
pub fn certify_reference_layer(
    status: &LayerStatus,
    weights: &CellWeights,
    reference_cells: &[(CellId, Triple)],
) -> Result<LayerAnchor> {
    let map = cell_map(reference_cells)?;
    let any_na = weights
        .support()
        .any(|c| !map.get(c).is_some_and(|t| t.is_complete()));

    let mut reasons = Vec::new();
    if status.n_miss > 0 {
        reasons.push(TierIIIReason::Miss);
    }
    if status.n_bad > 0 {
        reasons.push(TierIIIReason::Bad);
    }
    if any_na {
        reasons.push(TierIIIReason::Na);
    }
    if status.n_valid < MIN_VALID_REFERENCE_CELLS {
        reasons.push(TierIIIReason::FewValid);
    }

    let layer = aggregate_layer(weights, reference_cells)?;
    let measured = Point::from_triple(&layer);
    let mut out = LayerAnchor {
        tier: AnchorTier::III,
        tier_iii_reasons: Vec::new(),
        anchor: None,
        measured,
        clamp: None,
        w_miss: status.w_miss,
        w_thin: status.w_thin,
        w_bad: status.w_bad,
    };
    if !reasons.is_empty() {
        out.tier_iii_reasons = reasons;
        return Ok(out);
    }
    // Not tier III: every d+ cell is complete, so `measured` is Some.
    let m = measured.ok_or(CoreError::InvalidField {
        field: "reference cells",
        reason: "complete cells but NA layer aggregate",
    })?;
    if status.n_fail == 0 {
        // §7 line 561: N^miss = 0, N^fail = 0, no NA. Otherwise every fail
        // cell is F_thin (N^bad = 0 here): tier II.
        out.tier = AnchorTier::I;
        out.anchor = Some(m);
        return Ok(out);
    }

    debug_assert!(status.n_thin >= 1 && status.n_thin + status.n_bad == status.n_fail);
    // Tier II: valid-cell means.
    let (mut sw, mut sc, mut st, mut sd) = (0.0, 0.0, 0.0, 0.0);
    for (cell, a) in &status.cells {
        if a.status != CellStatus::Valid {
            continue;
        }
        let w = weights.omega(cell);
        if w <= 0.0 {
            continue;
        }
        let p =
            map.get(cell)
                .and_then(|t| Point::from_triple(t))
                .ok_or(CoreError::InvalidField {
                    field: "reference cells",
                    reason: "valid cell without a complete triple",
                })?;
        sw += w;
        sc += w * p.c;
        st += w * p.t;
        sd += w * (1.0 - p.q);
    }
    if sw <= 0.0 {
        return Err(CoreError::InvalidField {
            field: "reference cells",
            reason: "no positive valid weight in tier II",
        });
    }
    let c = dim_clamp(m.c, sc / sw);
    let t = dim_clamp(m.t, st / sw);
    let d = dim_clamp(1.0 - m.q, sd / sw);
    out.tier = AnchorTier::II;
    out.anchor = Some(Point {
        c: c.clipped,
        t: t.clipped,
        q: 1.0 - d.clipped,
    });
    out.clamp = Some([c, t, d]);
    Ok(out)
}

/// Layer `V_{i,d}` against the certified anchor; `None` in tier III.
pub fn layer_value(
    anchor: &LayerAnchor,
    arm: Point,
    w: &Weights,
    floors: &Floors,
) -> Result<Option<f64>> {
    match anchor.anchor {
        Some(a) => Ok(Some(value_score(a, arm, w, floors)?)),
        None => Ok(None),
    }
}

/// Unclamped diagnostic reference anchor (§7 "诊断表锚点"):
/// `C^{*,diag}_{R,u} = max(C*_{R,u}, max_valid C*_{R,v})`, the same for
/// time, `min` for quality, then aggregated over `d+`. A cell with an NA
/// dimension uses the worst valid value (DECISIONS Y). `None` when there
/// is no valid cell with a complete triple.
pub fn diagnostic_reference_anchor(
    status: &LayerStatus,
    weights: &CellWeights,
    reference_cells: &[(CellId, Triple)],
) -> Result<Option<Point>> {
    let map = cell_map(reference_cells)?;
    let mut worst: Option<Point> = None;
    for (cell, a) in &status.cells {
        if a.status != CellStatus::Valid || weights.omega(cell) <= 0.0 {
            continue;
        }
        if let Some(p) = map.get(cell).and_then(|t| Point::from_triple(t)) {
            worst = Some(match worst {
                None => p,
                Some(w) => Point {
                    c: w.c.max(p.c),
                    t: w.t.max(p.t),
                    q: w.q.min(p.q),
                },
            });
        }
    }
    let Some(wv) = worst else {
        return Ok(None);
    };
    let cells: Vec<(CellId, Triple)> = weights
        .support()
        .map(|c| {
            let t = map.get(c).copied().copied().unwrap_or(Triple::NA);
            let diag = Triple::new(
                t.c.map_or(wv.c, |v| v.max(wv.c)),
                t.t.map_or(wv.t, |v| v.max(wv.t)),
                t.q.map_or(wv.q, |v| v.min(wv.q)),
            );
            (c.clone(), diag)
        })
        .collect();
    Ok(Point::from_triple(&aggregate_layer(weights, &cells)?))
}

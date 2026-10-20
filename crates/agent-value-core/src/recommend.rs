//! Layer official pick, overall closure and the coarse-bill tag (v22 §6;
//! DECISIONS U, V, W).
//!
//! - [`layer_arm_from_cells`] builds a [`FrontierArm`] from cell values:
//!   `robust_enabled` is true only when every `d+` cell has
//!   `robust_dominance`; the coarse-bill tag is true when any `d+` cell
//!   carries [`CellNote::CancelLowerBoundOnly`] (the source of truth,
//!   DECISIONS W).
//! - Layer official #1: the tie group of highest `V_{i,d}` within `F_rec`,
//!   only when the layer is tier I or II. When `F_rec` is empty, `F_rob` is
//!   listed as the cancel-robust alternative and nobody is #1 (DECISIONS V).
//! - Overall: the reference must be tier I or II in every positive-weight
//!   layer; the overall anchor is the `ω_d`-weighted sum of layer anchors
//!   in all three dimensions (DECISIONS U). An arm gets an overall `V` and
//!   can be recommended only when it passes the grid gates and is in
//!   `F_all` (§6 closure 2).

use std::collections::BTreeMap;

use crate::anchor::{AnchorTier, LayerAnchor};
use crate::cell_values::{CellNote, CellValues, Triple};
use crate::cells::{CellId, CellWeights};
use crate::eligibility::{cancel_rate_ok, completion_rate_ok, sample_gate_ok, AuthenticityGrade};
use crate::error::{CoreError, Result};
use crate::frontier::{highest_v_group, FrontierArm, LayerFrontiers};
use crate::status::LayerStatus;
use crate::value::{aggregate_layer, aggregate_overall, value_score, Floors, Point};
use crate::weights::Weights;

/// An arm's layer values ready for the frontier, with its tag.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerArm {
    pub arm: FrontierArm,
    /// Shown as "账单过粗" wherever this arm's row in this layer is shown.
    pub coarse_bill: bool,
}

/// Builds one arm's layer triples and flags from its cell values.
pub fn layer_arm_from_cells(
    id: &str,
    weights: &CellWeights,
    cells: &[(CellId, CellValues)],
) -> Result<LayerArm> {
    let mut map: BTreeMap<&CellId, &CellValues> = BTreeMap::new();
    for (c, v) in cells {
        if map.insert(c, v).is_some() {
            return Err(CoreError::DuplicateCell {
                cell: c.as_str().to_string(),
            });
        }
    }
    let star: Vec<(CellId, Triple)> = cells.iter().map(|(c, v)| (c.clone(), v.star)).collect();
    let robust: Vec<(CellId, Triple)> = cells.iter().map(|(c, v)| (c.clone(), v.robust)).collect();
    let mut robust_enabled = true;
    let mut coarse_bill = false;
    for c in weights.support() {
        match map.get(c) {
            Some(v) => {
                robust_enabled &= v.robust_dominance;
                coarse_bill |= v.notes.contains(&CellNote::CancelLowerBoundOnly);
            }
            None => robust_enabled = false,
        }
    }
    Ok(LayerArm {
        arm: FrontierArm {
            id: id.to_string(),
            star: aggregate_layer(weights, &star)?,
            robust: aggregate_layer(weights, &robust)?,
            robust_enabled,
        },
        coarse_bill,
    })
}

/// Official pick of one layer.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerPick {
    /// Tie group of highest `V_{i,d}` within `F_rec`; empty in tier III or
    /// when `F_rec` is empty.
    pub official_top: Vec<String>,
    /// Members of `official_top` that carry the coarse-bill tag.
    pub official_top_coarse_bill: Vec<String>,
    /// `F_rob` when `F_rec` is empty ("取消稳健备选"); never #1.
    pub robust_alternative: Vec<String>,
}

/// Picks the layer official #1. `scores` must hold `V_{i,d}` for every
/// `F_rec` member when the layer is tier I or II. The coarse-bill tag of a
/// top arm comes from its [`LayerArm`] (cell notes), not from
/// `frontiers.coarse_bill`, which is a derived view (DECISIONS W).
pub fn layer_pick(
    arms: &[LayerArm],
    frontiers: &LayerFrontiers,
    anchor: &LayerAnchor,
    scores: &[(String, f64)],
) -> Result<LayerPick> {
    for (i, (id, _)) in scores.iter().enumerate() {
        if scores[..i].iter().any(|(s, _)| s == id) {
            return Err(CoreError::DuplicateCell { cell: id.clone() });
        }
    }
    let mut pick = LayerPick {
        official_top: Vec::new(),
        official_top_coarse_bill: Vec::new(),
        robust_alternative: Vec::new(),
    };
    if frontiers.rec.is_empty() {
        pick.robust_alternative = frontiers.rob.clone();
        return Ok(pick);
    }
    if anchor.tier == AnchorTier::III {
        return Ok(pick);
    }
    let mut rec_scores = Vec::new();
    for id in &frontiers.rec {
        let v = scores
            .iter()
            .find(|(s, _)| s == id)
            .map(|(_, v)| *v)
            .ok_or(CoreError::InvalidField {
                field: "layer scores",
                reason: "missing V for an F_rec member",
            })?;
        rec_scores.push((id.clone(), v));
    }
    pick.official_top = highest_v_group(&rec_scores)?;
    pick.official_top_coarse_bill = pick
        .official_top
        .iter()
        .filter(|id| arms.iter().any(|a| &a.arm.id == *id && a.coarse_bill))
        .cloned()
        .collect();
    Ok(pick)
}

/// Overall reference anchor (§6 closure 1).
#[derive(Debug, Clone, PartialEq)]
pub struct OverallReference {
    /// `Σ_d ω_d X^anchor_{R,d}`; `None` when any positive-weight layer is
    /// tier III.
    pub anchor: Option<Point>,
    /// Certified and some positive-weight layer is tier II ("含 Ⅱ 档夹逼层");
    /// always false when the overall reference is NA.
    pub includes_tier_ii: bool,
    /// Indexes (into the input) of positive-weight tier III layers.
    pub uncertified_layers: Vec<usize>,
}

/// Overall reference from `(ω_d, layer anchor)`; zero-weight layers are
/// ignored; positive weights must sum to 1.
pub fn overall_reference(layers: &[(f64, &LayerAnchor)]) -> Result<OverallReference> {
    let mut out = OverallReference {
        anchor: None,
        includes_tier_ii: false,
        uncertified_layers: Vec::new(),
    };
    let mut parts = Vec::new();
    for (i, (w, a)) in layers.iter().enumerate() {
        parts.push((
            *w,
            a.anchor.map_or(Triple::NA, |p| Triple::new(p.c, p.t, p.q)),
        ));
        if *w <= 0.0 {
            continue;
        }
        match a.tier {
            AnchorTier::III => out.uncertified_layers.push(i),
            AnchorTier::II => out.includes_tier_ii = true,
            AnchorTier::I => {}
        }
    }
    let agg = aggregate_overall(&parts)?;
    if out.uncertified_layers.is_empty() {
        out.anchor = Point::from_triple(&agg);
    } else {
        // Only one header applies: "部分正权重层参照未认证" wins.
        out.includes_tier_ii = false;
    }
    Ok(out)
}

/// One positive-weight layer of an arm for the overall closure.
#[derive(Debug, Clone, PartialEq)]
pub struct OverallArmLayer {
    pub omega_d: f64,
    pub status: LayerStatus,
    /// `U_d`: positive-weight cells in this layer.
    pub u_d: usize,
    /// The arm's layer triple `(C*_{i,d}, T*_{i,d}, Q*_{i,d})`.
    pub triple: Triple,
    /// The arm is in `F^off_d`.
    pub in_off: bool,
}

/// Overall grid gates (§6 closure 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OverallGate {
    CompleteCase,
    Sample,
    CancelRate,
    CompletionRate,
    Authenticity,
    NotInFAll,
}

/// Overall result for one arm.
#[derive(Debug, Clone, PartialEq)]
pub struct OverallArm {
    pub id: String,
    /// Overall triple, shown whenever every positive-weight layer triple is
    /// complete (not tied to complete-case).
    pub triple: Option<Point>,
    /// `cov_i = Σ_d ω_d (1 − w^miss − w^fail)`.
    pub cov: f64,
    /// `cov_i < 1` (with a 1e-9 tolerance for float sums of weights):
    /// diagnostic only, no frontier, no recommendation, no rank. Such an arm
    /// always fails the complete-case gate, so it never gets an overall `V`.
    pub partial_coverage: bool,
    pub failed: Vec<OverallGate>,
    /// Passes every grid gate (eligible for the overall recommendation
    /// when the reference is certified).
    pub qualified: bool,
    /// Overall `V_i`; `None` unless the reference is certified and the arm
    /// is qualified.
    pub v: Option<f64>,
}

/// Overall closure for one arm.
pub fn overall_arm(
    id: &str,
    layers: &[OverallArmLayer],
    authenticity: AuthenticityGrade,
    reference: &OverallReference,
    w: &Weights,
    floors: &Floors,
) -> Result<OverallArm> {
    let mut parts = Vec::new();
    let (mut n_comp, mut u, mut n_c, mut n_sub) = (0u64, 0usize, 0u64, 0u64);
    let (mut n_miss, mut n_fail) = (0usize, 0usize);
    let mut cov = 0.0;
    let mut in_all = false;
    for l in layers {
        parts.push((l.omega_d, l.triple));
        if l.omega_d <= 0.0 {
            continue;
        }
        n_comp = n_comp
            .checked_add(l.status.n_comp)
            .ok_or(CoreError::Overflow {
                what: "overall n_comp",
            })?;
        u = u
            .checked_add(l.u_d)
            .ok_or(CoreError::Overflow { what: "overall u" })?;
        n_c = n_c.checked_add(l.status.n_c).ok_or(CoreError::Overflow {
            what: "overall n_c",
        })?;
        n_sub = n_sub
            .checked_add(l.status.n_sub)
            .ok_or(CoreError::Overflow {
                what: "overall n_sub",
            })?;
        n_miss = n_miss
            .checked_add(l.status.n_miss)
            .ok_or(CoreError::Overflow {
                what: "overall n_miss",
            })?;
        n_fail = n_fail
            .checked_add(l.status.n_fail)
            .ok_or(CoreError::Overflow {
                what: "overall n_fail",
            })?;
        cov += l.omega_d * (1.0 - l.status.w_miss - l.status.w_fail);
        in_all |= l.in_off;
    }
    let triple = Point::from_triple(&aggregate_overall(&parts)?);
    let mut failed = Vec::new();
    if n_miss > 0 || n_fail > 0 {
        failed.push(OverallGate::CompleteCase);
    }
    if !sample_gate_ok(n_comp, u) {
        failed.push(OverallGate::Sample);
    }
    if !cancel_rate_ok(n_c, n_sub) {
        failed.push(OverallGate::CancelRate);
    }
    if !completion_rate_ok(n_comp, n_sub) {
        failed.push(OverallGate::CompletionRate);
    }
    if !authenticity.passes() {
        failed.push(OverallGate::Authenticity);
    }
    if !in_all {
        failed.push(OverallGate::NotInFAll);
    }
    let qualified = failed.is_empty();
    let v = match (qualified, reference.anchor, triple) {
        (true, Some(r), Some(t)) => Some(value_score(r, t, w, floors)?),
        _ => None,
    };
    Ok(OverallArm {
        id: id.to_string(),
        triple,
        cov,
        partial_coverage: cov < 1.0 - 1e-9,
        failed,
        qualified,
        v,
    })
}

/// Overall official #1: tie group of highest overall `V` among arms that
/// have one.
pub fn overall_top(arms: &[OverallArm]) -> Result<Vec<String>> {
    let scores: Vec<(String, f64)> = arms
        .iter()
        .filter_map(|a| a.v.map(|v| (a.id.clone(), v)))
        .collect();
    highest_v_group(&scores)
}

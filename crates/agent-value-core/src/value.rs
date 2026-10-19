//! Aggregation over cells and layers and the value score `V` (v22 §4;
//! HANDOFF step 7).
//!
//! `C*_{i,d} = sum_{u in d+} omega_u C*_{i,u}` (same for T*, Q*), overall
//! `sum_{d in D+} omega_d (...)`. Any NA cell makes the aggregate NA; there
//! is no renormalization over the remaining cells.
//!
//! `V = 100 ((C_ref + c0)/(C + c0))^a ((T_ref + t0)/(T + t0))^b
//! ((1 - Q_ref + eps)/(1 - Q + eps))^g`, evaluated in log space. The
//! numerator anchor is supplied by the caller (tier I: measured reference;
//! tier II: clamped anchor; tier III: no V; §7, a later step).

use std::collections::BTreeMap;

use crate::cell_values::Triple;
use crate::cells::{CellId, CellWeights, CELL_WEIGHT_SUM_TOLERANCE};
use crate::constants::{C0_YUAN, EPSILON, T0_SECONDS};
use crate::error::{finite, CoreError, Result};
use crate::weights::Weights;

/// Noise floors `(c0, t0, eps)` of a scoring scope (§A: default §9).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Floors {
    c0: f64,
    t0: f64,
    eps: f64,
}

impl Default for Floors {
    fn default() -> Self {
        Self {
            c0: C0_YUAN,
            t0: T0_SECONDS,
            eps: EPSILON,
        }
    }
}

impl Floors {
    /// Custom floors; each must be finite and > 0.
    pub fn new(c0: f64, t0: f64, eps: f64) -> Result<Self> {
        for (what, v) in [("c0", c0), ("t0", t0), ("epsilon", eps)] {
            let v = finite(what, v)?;
            if v <= 0.0 {
                return Err(CoreError::OutOfRange {
                    what,
                    value: v,
                    min: 0.0,
                    max: f64::INFINITY,
                });
            }
        }
        Ok(Self { c0, t0, eps })
    }

    pub fn c0(&self) -> f64 {
        self.c0
    }
    pub fn t0(&self) -> f64 {
        self.t0
    }
    pub fn eps(&self) -> f64 {
        self.eps
    }
}

fn weighted(parts: &[(f64, Triple)]) -> Triple {
    let mut acc = Triple::new(0.0, 0.0, 0.0);
    for (w, t) in parts {
        acc.c = acc.c.zip(t.c).map(|(a, v)| a + w * v);
        acc.t = acc.t.zip(t.t).map(|(a, v)| a + w * v);
        acc.q = acc.q.zip(t.q).map(|(a, v)| a + w * v);
    }
    acc
}

/// Layer aggregate over `d+`. A `d+` cell without values counts as NA;
/// cells outside `d+` are ignored; duplicates are an error.
pub fn aggregate_layer(weights: &CellWeights, cells: &[(CellId, Triple)]) -> Result<Triple> {
    let mut map: BTreeMap<&CellId, &Triple> = BTreeMap::new();
    for (c, t) in cells {
        if map.insert(c, t).is_some() {
            return Err(CoreError::DuplicateCell {
                cell: c.as_str().to_string(),
            });
        }
    }
    let parts: Vec<(f64, Triple)> = weights
        .support()
        .map(|c| (weights.omega(c), map.get(c).map_or(Triple::NA, |t| **t)))
        .collect();
    Ok(weighted(&parts))
}

/// Overall aggregate over layers `(omega_d, layer triple)`; zero-weight
/// layers are skipped; the positive weights must sum to 1.
pub fn aggregate_overall(layers: &[(f64, Triple)]) -> Result<Triple> {
    let mut sum = 0.0;
    let mut parts = Vec::new();
    for &(w, t) in layers {
        let w = finite("layer weight", w)?;
        if w < 0.0 {
            return Err(CoreError::OutOfRange {
                what: "layer weight",
                value: w,
                min: 0.0,
                max: 1.0,
            });
        }
        if w > 0.0 {
            sum += w;
            parts.push((w, t));
        }
    }
    if (sum - 1.0).abs() > CELL_WEIGHT_SUM_TOLERANCE {
        return Err(CoreError::OutOfRange {
            what: "sum of layer weights",
            value: sum,
            min: 1.0,
            max: 1.0,
        });
    }
    Ok(weighted(&parts))
}

/// A complete `(C, T, Q)` point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub c: f64,
    pub t: f64,
    pub q: f64,
}

impl Point {
    fn check(&self, what: &'static str) -> Result<()> {
        for v in [self.c, self.t, self.q] {
            finite(what, v)?;
        }
        if self.c < 0.0 || self.t < 0.0 {
            return Err(CoreError::OutOfRange {
                what,
                value: self.c.min(self.t),
                min: 0.0,
                max: f64::INFINITY,
            });
        }
        if !(0.0..=1.0).contains(&self.q) {
            return Err(CoreError::OutOfRange {
                what,
                value: self.q,
                min: 0.0,
                max: 1.0,
            });
        }
        Ok(())
    }

    /// The point of a complete triple; `None` if any dimension is NA.
    pub fn from_triple(t: &Triple) -> Option<Self> {
        Some(Self {
            c: t.c?,
            t: t.t?,
            q: t.q?,
        })
    }
}

/// `ln V - ln 100` (the log of the three weighted ratios).
pub fn log_value_ratio(reference: Point, arm: Point, w: &Weights, floors: &Floors) -> Result<f64> {
    reference.check("reference anchor")?;
    arm.check("arm aggregate")?;
    let ln_c = (reference.c + floors.c0).ln() - (arm.c + floors.c0).ln();
    let ln_t = (reference.t + floors.t0).ln() - (arm.t + floors.t0).ln();
    let ln_d = (1.0 - reference.q + floors.eps).ln() - (1.0 - arm.q + floors.eps).ln();
    Ok(w.a() * ln_c + w.b() * ln_t + w.g() * ln_d)
}

/// The value score `V` (§4).
pub fn value_score(reference: Point, arm: Point, w: &Weights, floors: &Floors) -> Result<f64> {
    Ok(100.0 * log_value_ratio(reference, arm, w, floors)?.exp())
}

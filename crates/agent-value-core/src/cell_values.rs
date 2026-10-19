//! Per-cell delivered cost, time and quality `C*`, `T*`, `Q*` (v22 §2,
//! §3; HANDOFF step 6).
//!
//! - Unit prices per side: winsorized mean (k(m) = 0 below 10 samples,
//!   else max(1, floor(0.01 m))), shrunk to the direct benchmark with
//!   `kappa_C = 5`; no benchmark => no shrinkage. Empty side: benchmark ->
//!   layer pool -> NA. NA is never replaced by 0.
//! - Cancel amortization as in v21 (`nC * mean / (n1 + n0)`), with the
//!   cancel-mean degrade chain.
//! - The fallback term uses the fallback arm `F_k` (default `F_k = R_k`).
//!   The reference and the fallback arm themselves are terminal: no
//!   fallback term.
//! - NA propagation: a candidate's dimension is NA when the reference's is;
//!   with `F_k != R_k`, any NA dimension of `F` makes the whole candidate
//!   cell NA. `miss` cells are NA in every dimension.
//! - Cancel-robust versions with the monotone guards.

use crate::constants::{KAPPA_C, KAPPA_F};
use crate::error::{finite, CoreError, Result};
use crate::retry::RetryStats;
use crate::status::CellCounts;

/// Below this many samples a side mean is not winsorized.
pub const WINSOR_MIN_SAMPLES: usize = 10;

/// `(C*, T*, Q*)` of one cell; `None` is NA.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Triple {
    pub c: Option<f64>,
    pub t: Option<f64>,
    pub q: Option<f64>,
}

impl Triple {
    /// All three NA.
    pub const NA: Self = Self {
        c: None,
        t: None,
        q: None,
    };

    pub fn new(c: f64, t: f64, q: f64) -> Self {
        Self {
            c: Some(c),
            t: Some(t),
            q: Some(q),
        }
    }

    /// No dimension is NA.
    pub fn is_complete(&self) -> bool {
        self.c.is_some() && self.t.is_some() && self.q.is_some()
    }

    /// Any dimension is NA.
    pub fn has_na(&self) -> bool {
        !self.is_complete()
    }
}

/// `k(m)`: number of values winsorized on each side.
pub fn winsor_k(m: usize) -> usize {
    if m < WINSOR_MIN_SAMPLES {
        0
    } else {
        (m / 100).max(1)
    }
}

/// A side mean and whether it was winsorized.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SideMean {
    pub value: f64,
    pub m: usize,
    /// False below 10 samples (shown as "not winsorized").
    pub winsorized: bool,
}

fn check_samples(what: &'static str, samples: &[f64]) -> Result<()> {
    for &v in samples {
        let v = finite(what, v)?;
        if v < 0.0 {
            return Err(CoreError::OutOfRange {
                what,
                value: v,
                min: 0.0,
                max: f64::INFINITY,
            });
        }
    }
    Ok(())
}

/// Two-sided winsorized mean with `k(m)`; raw mean below 10 samples;
/// `None` for no samples.
pub fn side_mean(samples: &[f64]) -> Result<Option<SideMean>> {
    check_samples("sample", samples)?;
    let m = samples.len();
    if m == 0 {
        return Ok(None);
    }
    let k = winsor_k(m);
    let mut v = samples.to_vec();
    v.sort_by(f64::total_cmp);
    if k > 0 {
        let (lo, hi) = (v[k], v[m - 1 - k]);
        for x in v.iter_mut().take(k) {
            *x = lo;
        }
        for x in v.iter_mut().skip(m - k) {
            *x = hi;
        }
    }
    Ok(Some(SideMean {
        value: v.iter().sum::<f64>() / m as f64,
        m,
        winsorized: k > 0,
    }))
}

/// Where a unit price came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceSource {
    /// Own samples shrunk to the direct benchmark.
    Shrunk,
    /// Own samples, no benchmark (no shrinkage).
    Own,
    /// No own samples: the direct benchmark.
    Benchmark,
    /// No own samples and no benchmark: the layer pool.
    LayerPool,
}

/// A unit price `C~` (or `T~`) with its provenance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnitPrice {
    pub value: f64,
    pub source: PriceSource,
    /// Own samples were winsorized (false when below 10 or not own).
    pub winsorized: bool,
}

fn opt_non_negative(what: &'static str, v: Option<f64>) -> Result<Option<f64>> {
    match v {
        None => Ok(None),
        Some(x) => {
            check_samples(what, &[x])?;
            Ok(Some(x))
        }
    }
}

/// `C~ = (m Cbar^w + kappa_C C^B) / (m + kappa_C)`, with the empty-sample
/// chain benchmark -> layer pool -> NA. `m` is the number of available
/// samples (tasks whose value is NA are left out by the caller).
pub fn unit_price(
    samples: &[f64],
    benchmark: Option<f64>,
    layer_pool: Option<f64>,
) -> Result<Option<UnitPrice>> {
    let benchmark = opt_non_negative("benchmark", benchmark)?;
    let layer_pool = opt_non_negative("layer pool mean", layer_pool)?;
    Ok(match (side_mean(samples)?, benchmark, layer_pool) {
        (Some(s), Some(b), _) => Some(UnitPrice {
            value: (s.m as f64 * s.value + KAPPA_C * b) / (s.m as f64 + KAPPA_C),
            source: PriceSource::Shrunk,
            winsorized: s.winsorized,
        }),
        (Some(s), None, _) => Some(UnitPrice {
            value: s.value,
            source: PriceSource::Own,
            winsorized: s.winsorized,
        }),
        (None, Some(b), _) => Some(UnitPrice {
            value: b,
            source: PriceSource::Benchmark,
            winsorized: false,
        }),
        (None, None, Some(p)) => Some(UnitPrice {
            value: p,
            source: PriceSource::LayerPool,
            winsorized: false,
        }),
        (None, None, None) => None,
    })
}

/// Billing of cancelled tasks for one dimension (money or duration).
#[derive(Debug, Clone, PartialEq, Default)]
pub enum CancelBills {
    /// No cancel bill at all.
    #[default]
    None,
    /// One value per cancelled task (length must equal `nC`).
    PerTask(Vec<f64>),
    /// Only an aggregate total, optionally with a per-task cap.
    Aggregate { total: f64, cap: Option<f64> },
}

/// Inputs of one dimension (cost in yuan, or time in seconds) of a cell.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DimInputs {
    /// Values of successful tasks with a known value.
    pub success: Vec<f64>,
    /// Values of failed tasks with a known value.
    pub failure: Vec<f64>,
    pub bench_success: Option<f64>,
    pub bench_failure: Option<f64>,
    /// Pooled winsorized means of the same arm over the layer's
    /// positive-weight cells (see [`side_mean`]).
    pub pool_success: Option<f64>,
    pub pool_failure: Option<f64>,
    pub cancel: CancelBills,
    /// Pooled cancel mean of the layer.
    pub pool_cancel: Option<f64>,
}

/// Retry delay `D` (time only): own -> layer pool -> benchmark -> NA.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DelayInputs {
    pub own: Option<f64>,
    pub layer_pool: Option<f64>,
    pub benchmark: Option<f64>,
}

impl DelayInputs {
    fn resolve(&self) -> Result<Option<f64>> {
        Ok(opt_non_negative("retry delay D", self.own)?
            .or(opt_non_negative("pooled retry delay", self.layer_pool)?)
            .or(opt_non_negative("benchmark retry delay", self.benchmark)?))
    }
}

/// Everything needed for one (arm, cell).
#[derive(Debug, Clone, PartialEq)]
pub struct CellInputs {
    pub counts: CellCounts,
    /// From the posterior with `n0` failures (§1).
    pub retry: RetryStats,
    /// From the cancel-robust posterior (`n0 + nC` failures).
    pub retry_c: RetryStats,
    pub cost: DimInputs,
    pub time: DimInputs,
    pub delay: DelayInputs,
    /// `q_hat` of the cell (§3, §3.1).
    pub q_hat: f64,
    /// Size `f` of the matched-failure subset.
    pub matched_failures: u64,
    /// `Q^match`, required when `f > 0` (NA otherwise).
    pub q_match: Option<f64>,
}

/// Role of the arm whose cell is computed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Role<'a> {
    /// The reference arm, or the fallback arm itself: no fallback term.
    Terminal,
    /// A candidate arm. `reference` is the reference cell (NA propagates
    /// from it); `fallback` is `F_k`'s cell when `F_k != R_k`, `None` when
    /// `F_k = R_k` (then the reference cell is the fallback).
    Candidate {
        reference: &'a CellValues,
        fallback: Option<&'a CellValues>,
    },
}

/// Remarks attached to a cell's values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CellNote {
    /// A side mean used fewer than 10 samples ("not winsorized").
    NotWinsorized,
    /// A unit price came from the benchmark or the layer pool.
    BorrowedUnitPrice,
    /// Only an aggregate cancel bill without cap: lower bound used, robust
    /// dominance disabled ("bill too coarse").
    CancelLowerBoundOnly,
}

/// `C*, T*, Q*` and their cancel-robust versions.
#[derive(Debug, Clone, PartialEq)]
pub struct CellValues {
    pub star: Triple,
    pub robust: Triple,
    /// False when a cancel excess was only a lower bound: the cell must
    /// not take part in robust dominance.
    pub robust_dominance: bool,
    pub notes: Vec<CellNote>,
}

impl CellValues {
    /// A cell that is NA everywhere (e.g. `miss`).
    pub fn na() -> Self {
        Self {
            star: Triple::NA,
            robust: Triple::NA,
            robust_dominance: false,
            notes: Vec::new(),
        }
    }
}

struct DimResult {
    star: Option<f64>,
    robust: Option<f64>,
    lower_bound_only: bool,
}

fn note_price(p: &UnitPrice, notes: &mut Vec<CellNote>) {
    if matches!(p.source, PriceSource::Own | PriceSource::Shrunk) && !p.winsorized {
        notes.push(CellNote::NotWinsorized);
    }
    if matches!(p.source, PriceSource::Benchmark | PriceSource::LayerPool) {
        notes.push(CellNote::BorrowedUnitPrice);
    }
}

/// Cancel mean `hat Cbar_C`: bills -> max(C~_F, pool) -> max(C~_F, C_F^B) -> NA.
fn cancel_mean(d: &DimInputs, n_c: u64, f_price: f64) -> Result<Option<f64>> {
    Ok(match &d.cancel {
        CancelBills::PerTask(v) => {
            if v.len() as u64 != n_c {
                return Err(CoreError::LengthMismatch {
                    left: v.len(),
                    right: n_c as usize,
                });
            }
            check_samples("cancel bill", v)?;
            Some(v.iter().sum::<f64>() / n_c as f64)
        }
        CancelBills::Aggregate { total, .. } => {
            check_samples("cancel bill total", &[*total])?;
            Some(total / n_c as f64)
        }
        CancelBills::None => match (
            opt_non_negative("pooled cancel mean", d.pool_cancel)?,
            opt_non_negative("benchmark failure mean", d.bench_failure)?,
        ) {
            (Some(p), _) => Some(f_price.max(p)),
            (None, Some(b)) => Some(f_price.max(b)),
            (None, None) => None,
        },
    })
}

/// Cancel excess `E_C` and whether it is only a lower bound.
fn cancel_excess(d: &DimInputs, n_c: u64, f_price: f64, mean: f64) -> Result<(f64, bool)> {
    Ok(match &d.cancel {
        CancelBills::PerTask(v) => (v.iter().map(|c| (c - f_price).max(0.0)).sum(), false),
        CancelBills::Aggregate {
            total,
            cap: Some(cap),
        } => {
            let cap = finite("cancel cap", *cap)?;
            if cap <= 0.0 {
                return Err(CoreError::OutOfRange {
                    what: "cancel cap",
                    value: cap,
                    min: 0.0,
                    max: f64::INFINITY,
                });
            }
            let full = (total / cap).floor();
            let rest = total - full * cap;
            (
                full * (cap - f_price).max(0.0) + (rest - f_price).max(0.0),
                false,
            )
        }
        // Aggregate without cap, or no bill (degraded mean): lower bound.
        CancelBills::Aggregate { cap: None, .. } | CancelBills::None => {
            (n_c as f64 * (mean - f_price).max(0.0), true)
        }
    })
}

fn dimension(
    d: &DimInputs,
    counts: &CellCounts,
    retry: &RetryStats,
    retry_c: &RetryStats,
    wait: Option<Option<f64>>,
    fallback: Option<Option<f64>>,
    notes: &mut Vec<CellNote>,
) -> Result<DimResult> {
    let na = DimResult {
        star: None,
        robust: None,
        lower_bound_only: false,
    };
    let s = unit_price(&d.success, d.bench_success, d.pool_success)?;
    let f = unit_price(&d.failure, d.bench_failure, d.pool_failure)?;
    let (Some(s), Some(f)) = (s, f) else {
        return Ok(na);
    };
    note_price(&s, notes);
    note_price(&f, notes);
    let completed = counts.completed()? as f64;
    let (amort, excess, lb) = if counts.n_c == 0 {
        (0.0, 0.0, false)
    } else {
        let Some(mean) = cancel_mean(d, counts.n_c, f.value)? else {
            return Ok(na);
        };
        let (e, lb) = cancel_excess(d, counts.n_c, f.value, mean)?;
        (counts.n_c as f64 * mean / completed, e / completed, lb)
    };
    // Retry wait term (time only).
    let (wait_star, wait_robust) = match wait {
        None => (0.0, 0.0),
        Some(None) => return Ok(na),
        Some(Some(dly)) => (retry.e_n_w * dly, retry_c.e_n_w * dly),
    };
    let (fb_star, fb_robust) = match fallback {
        None => (0.0, 0.0),
        Some(None) => return Ok(na),
        Some(Some(v)) => (retry.q_k * v, retry_c.q_k * v),
    };
    let star = retry.p_k * s.value + retry.e_n_f * f.value + wait_star + fb_star + amort;
    let robust = retry_c.p_k * s.value + retry_c.e_n_f * f.value + wait_robust + fb_robust + excess;
    if lb {
        notes.push(CellNote::CancelLowerBoundOnly);
    }
    Ok(DimResult {
        star: Some(star),
        robust: Some(robust.max(star)),
        lower_bound_only: lb,
    })
}

fn unit_interval(what: &'static str, v: f64) -> Result<f64> {
    let v = finite(what, v)?;
    if !(0.0..=1.0).contains(&v) {
        return Err(CoreError::OutOfRange {
            what,
            value: v,
            min: 0.0,
            max: 1.0,
        });
    }
    Ok(v)
}

/// Computes `C*, T*, Q*` (and robust versions) of one cell (§2, §3).
pub fn cell_values(inputs: &CellInputs, role: Role<'_>) -> Result<CellValues> {
    let counts = &inputs.counts;
    let q_hat = unit_interval("q_hat", inputs.q_hat)?;
    if let Some(q) = inputs.q_match {
        unit_interval("Q_match", q)?;
    }
    if counts.completed()? == 0 {
        return Ok(CellValues::na());
    }
    // F != R with any NA dimension: the whole candidate cell is NA.
    if let Role::Candidate {
        fallback: Some(f), ..
    } = role
    {
        if f.star.has_na() {
            return Ok(CellValues::na());
        }
    }
    let fb: Option<&Triple> = match role {
        Role::Terminal => None,
        Role::Candidate {
            reference,
            fallback,
        } => Some(&fallback.unwrap_or(reference).star),
    };
    let mut notes = Vec::new();
    let r = &inputs.retry;
    let rc = &inputs.retry_c;
    let cost = dimension(
        &inputs.cost,
        counts,
        r,
        rc,
        None,
        fb.map(|t| t.c),
        &mut notes,
    )?;
    let time = dimension(
        &inputs.time,
        counts,
        r,
        rc,
        Some(inputs.delay.resolve()?),
        fb.map(|t| t.t),
        &mut notes,
    )?;
    // Quality.
    let (q_star, q_robust) = match fb.map(|t| t.q) {
        None => (
            Some(r.p_k * q_hat),
            Some((rc.p_k * q_hat).min(r.p_k * q_hat)),
        ),
        Some(None) => (None, None),
        Some(Some(q_f)) => {
            let f = inputs.matched_failures as f64;
            let q_bar = if inputs.matched_failures == 0 {
                Some(q_f)
            } else {
                inputs
                    .q_match
                    .map(|qm| (f * qm + KAPPA_F * q_f) / (f + KAPPA_F))
            };
            match q_bar {
                None => (None, None),
                Some(qb) => {
                    let s = r.p_k * q_hat + r.q_k * qb;
                    (Some(s), Some((rc.p_k * q_hat + rc.q_k * qb).min(s)))
                }
            }
        }
    };
    let mut star = Triple {
        c: cost.star,
        t: time.star,
        q: q_star,
    };
    let mut robust = Triple {
        c: cost.robust,
        t: time.robust,
        q: q_robust,
    };
    // NA propagation from the reference, per dimension.
    if let Role::Candidate { reference, .. } = role {
        let rs = &reference.star;
        if rs.c.is_none() {
            star.c = None;
            robust.c = None;
        }
        if rs.t.is_none() {
            star.t = None;
            robust.t = None;
        }
        if rs.q.is_none() {
            star.q = None;
            robust.q = None;
        }
    }
    notes.sort();
    notes.dedup();
    Ok(CellValues {
        star,
        robust,
        robust_dominance: !(cost.lower_bound_only || time.lower_bound_only),
        notes,
    })
}

//! Positive-weight grid, cell states and `omega_u` (v22 §0; HANDOFF step 4).
//!
//! - `omega_u` for window t: share of the cell's user tasks in window t-1
//!   (probe tasks excluded). No user task in t-1 => carry the last valid
//!   version, else equal weights over the configured cells; the source is
//!   returned so it can go into the version note. Never divides by zero.
//! - States over `d+` only (cells with `omega_u > 0`): `miss` if
//!   `n1 + n0 = 0`; else `F_thin` if `n1 + n0 < 3` (checked first, without
//!   looking at the lower bound); else `F_bad` if `p_K^lo < 0.75` or
//!   `p_K^{C,lo} < 0.75`; else `valid`.

use std::collections::BTreeMap;

use crate::cells::{CellId, CellWeights};
use crate::error::{CoreError, Result};
use crate::retry::{PassStats, Prior, RetryModel};

/// Below this many completed samples a cell is `F_thin`.
pub const MIN_COMPLETED_PER_CELL: u64 = 3;

/// Outcome counts of one (arm, cell) in the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellCounts {
    pub n1: u64,
    pub n0: u64,
    pub n_c: u64,
    /// Undetermined.
    pub m: u64,
}

impl CellCounts {
    /// `n1 + n0`.
    pub fn completed(&self) -> Result<u64> {
        self.n1
            .checked_add(self.n0)
            .ok_or(CoreError::Overflow { what: "n1 + n0" })
    }

    /// `N^sub = n1 + n0 + nC + m`.
    pub fn submitted(&self) -> Result<u64> {
        [self.n0, self.n_c, self.m]
            .iter()
            .try_fold(self.n1, |acc, &v| acc.checked_add(v))
            .ok_or(CoreError::Overflow { what: "N_sub" })
    }
}

/// State of a cell in `d+`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellStatus {
    Miss,
    /// Fail, too few samples (`n1 + n0 < 3`).
    FailThin,
    /// Fail, enough samples but a lower bound below 0.75.
    FailBad,
    Valid,
}

impl CellStatus {
    pub fn is_fail(self) -> bool {
        matches!(self, Self::FailThin | Self::FailBad)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Miss => "miss",
            Self::FailThin => "fail-thin",
            Self::FailBad => "fail-bad",
            Self::Valid => "valid",
        }
    }
}

/// State of one cell, with its pass statistics when it has samples.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellAssessment {
    pub status: CellStatus,
    /// `None` for `miss` (no data; its C*, T*, Q* are NA).
    pub pass: Option<PassStats>,
}

/// Classifies one cell (§0), in the fixed order miss -> F_thin -> F_bad.
pub fn assess_cell(
    counts: &CellCounts,
    prior: &Prior,
    model: &RetryModel,
) -> Result<CellAssessment> {
    let completed = counts.completed()?;
    counts.submitted()?;
    if completed == 0 {
        return Ok(CellAssessment {
            status: CellStatus::Miss,
            pass: None,
        });
    }
    let pass = PassStats::new(prior, counts.n1, counts.n0, counts.n_c, model)?;
    let status = if completed < MIN_COMPLETED_PER_CELL {
        CellStatus::FailThin
    } else if pass.any_lower_fails() {
        CellStatus::FailBad
    } else {
        CellStatus::Valid
    };
    Ok(CellAssessment {
        status,
        pass: Some(pass),
    })
}

/// Input row of one (arm, cell).
#[derive(Debug, Clone, PartialEq)]
pub struct CellInput {
    pub cell: CellId,
    pub counts: CellCounts,
    pub prior: Prior,
}

/// States, counts and weight sums of one arm (or the reference) in a layer.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerStatus {
    /// Every cell of `d+`, sorted by id.
    pub cells: Vec<(CellId, CellAssessment)>,
    pub n_miss: usize,
    pub n_fail: usize,
    pub n_thin: usize,
    pub n_bad: usize,
    pub n_valid: usize,
    pub w_miss: f64,
    pub w_fail: f64,
    pub w_thin: f64,
    pub w_bad: f64,
    /// `n^comp = sum over d+ of (n1 + n0)`.
    pub n_comp: u64,
    /// `n^C` summed over present `d+` rows (miss cells contribute 0).
    pub n_c: u64,
    /// `N^sub = n1 + n0 + nC + m` summed over present `d+` rows
    /// (miss cells contribute 0; with complete-case this equals the §5 sum).
    pub n_sub: u64,
}

impl LayerStatus {
    pub fn status(&self, cell: &CellId) -> Option<CellStatus> {
        self.cells
            .iter()
            .find(|(c, _)| c == cell)
            .map(|(_, a)| a.status)
    }
}

/// Classifies every cell of `d+`. Rows for cells outside `d+` (zero
/// weight, unknown, probe) are ignored: their tasks are not counted
/// anywhere. A `d+` cell without a row is `miss`. Duplicate rows are an
/// error.
pub fn layer_status(
    weights: &CellWeights,
    inputs: &[CellInput],
    model: &RetryModel,
) -> Result<LayerStatus> {
    let mut rows: BTreeMap<&CellId, &CellInput> = BTreeMap::new();
    for row in inputs {
        if rows.insert(&row.cell, row).is_some() {
            return Err(CoreError::DuplicateCell {
                cell: row.cell.as_str().to_string(),
            });
        }
    }
    let mut out = LayerStatus {
        cells: Vec::new(),
        n_miss: 0,
        n_fail: 0,
        n_thin: 0,
        n_bad: 0,
        n_valid: 0,
        w_miss: 0.0,
        w_fail: 0.0,
        w_thin: 0.0,
        w_bad: 0.0,
        n_comp: 0,
        n_c: 0,
        n_sub: 0,
    };
    for cell in weights.support() {
        let w = weights.omega(cell);
        let a = match rows.get(cell) {
            Some(row) => {
                out.n_comp = out
                    .n_comp
                    .checked_add(row.counts.completed()?)
                    .ok_or(CoreError::Overflow { what: "n_comp" })?;
                out.n_c = out
                    .n_c
                    .checked_add(row.counts.n_c)
                    .ok_or(CoreError::Overflow { what: "n_c" })?;
                out.n_sub = out
                    .n_sub
                    .checked_add(row.counts.submitted()?)
                    .ok_or(CoreError::Overflow { what: "n_sub" })?;
                assess_cell(&row.counts, &row.prior, model)?
            }
            None => CellAssessment {
                status: CellStatus::Miss,
                pass: None,
            },
        };
        match a.status {
            CellStatus::Miss => {
                out.n_miss += 1;
                out.w_miss += w;
            }
            CellStatus::FailThin => {
                out.n_fail += 1;
                out.n_thin += 1;
                out.w_fail += w;
                out.w_thin += w;
            }
            CellStatus::FailBad => {
                out.n_fail += 1;
                out.n_bad += 1;
                out.w_fail += w;
                out.w_bad += w;
            }
            CellStatus::Valid => out.n_valid += 1,
        }
        out.cells.push((cell.clone(), a));
    }
    Ok(out)
}

/// Where window t's `omega_u` came from (goes into the version note).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OmegaSource {
    /// Task shares of window t-1.
    PreviousWindow,
    /// Window t-1 had no user task in the layer: last valid version kept.
    CarriedOver,
    /// No data and no earlier version: equal over the configured cells.
    Equal,
}

/// `omega_u` for window t from the user-task counts of window t-1.
///
/// `configured` are the layer's sub-classes. `previous_counts` may list the
/// probe cell (ignored) but no unconfigured user cell (an error: the
/// classifier and the configuration are frozen together). Configured cells
/// without tasks in t-1 get weight 0 when the layer had any task.
pub fn derive_cell_weights(
    configured: &[CellId],
    previous_counts: &[(CellId, u64)],
    last_valid: Option<&CellWeights>,
) -> Result<(CellWeights, OmegaSource)> {
    let mut users: Vec<&CellId> = configured.iter().filter(|c| !c.is_probe()).collect();
    users.sort();
    if users.is_empty() {
        return Err(CoreError::EmptyField {
            field: "configured cells",
        });
    }
    if let Some(p) = users.windows(2).find(|p| p[0] == p[1]) {
        return Err(CoreError::DuplicateCell {
            cell: p[0].as_str().to_string(),
        });
    }
    let mut counts: BTreeMap<&CellId, u64> = users.iter().map(|&c| (c, 0)).collect();
    let mut seen: Vec<&CellId> = Vec::new();
    for (cell, n) in previous_counts {
        if cell.is_probe() {
            continue;
        }
        if seen.contains(&cell) {
            return Err(CoreError::DuplicateCell {
                cell: cell.as_str().to_string(),
            });
        }
        seen.push(cell);
        match counts.get_mut(cell) {
            Some(slot) => *slot = *n,
            None => {
                return Err(CoreError::InvalidField {
                    field: "previous window counts",
                    reason: "names a cell that is not configured for the layer",
                })
            }
        }
    }
    let total = counts
        .values()
        .try_fold(0u64, |acc, &n| acc.checked_add(n))
        .ok_or(CoreError::Overflow {
            what: "layer task count",
        })?;
    if total == 0 {
        return Ok(match last_valid {
            Some(w) => (w.clone(), OmegaSource::CarriedOver),
            None => (
                CellWeights::equal(&users.into_iter().cloned().collect::<Vec<_>>())?,
                OmegaSource::Equal,
            ),
        });
    }
    let entries = counts
        .into_iter()
        .map(|(c, n)| (c.clone(), n as f64 / total as f64))
        .collect();
    Ok((CellWeights::new(entries)?, OmegaSource::PreviousWindow))
}

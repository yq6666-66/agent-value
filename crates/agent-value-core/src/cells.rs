//! Design cells `u` and frozen cell weights `omega_u` (v22 §0, §A).
//!
//! Probe calls (§11.1) go to a dedicated cell whose weight is always 0: they
//! never enter success/failure counts, pass rates or any score, and never
//! count as user tasks, but they are still logged.

use crate::error::{finite, CoreError, Result};

/// Reserved id of the probe cell.
pub const PROBE_CELL_ID: &str = "__probe__";
/// User cell ids may not start with this prefix (reserved for system cells).
pub const RESERVED_CELL_PREFIX: &str = "__";
/// Allowed slack for `sum omega_u = 1`.
pub const CELL_WEIGHT_SUM_TOLERANCE: f64 = 1e-9;

/// A design cell: a user task sub-class, or the probe cell.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CellId {
    User(String),
    Probe,
}

impl CellId {
    /// A user cell. The name is trimmed and lower-cased; it must be
    /// non-empty, contain no whitespace, and not use the reserved prefix.
    pub fn user(name: &str) -> Result<Self> {
        let s = name.trim().to_lowercase();
        if s.is_empty() {
            return Err(CoreError::EmptyField { field: "cell id" });
        }
        if s.starts_with(RESERVED_CELL_PREFIX) {
            return Err(CoreError::InvalidField {
                field: "cell id",
                reason: "the \"__\" prefix is reserved",
            });
        }
        if s.chars().any(char::is_whitespace) {
            return Err(CoreError::InvalidField {
                field: "cell id",
                reason: "must not contain whitespace",
            });
        }
        Ok(Self::User(s))
    }

    /// Parses a stored id: [`PROBE_CELL_ID`] is the probe cell, anything
    /// else must be a valid user cell.
    pub fn parse(s: &str) -> Result<Self> {
        if s == PROBE_CELL_ID {
            Ok(Self::Probe)
        } else {
            Self::user(s)
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::User(s) => s,
            Self::Probe => PROBE_CELL_ID,
        }
    }

    pub fn is_probe(&self) -> bool {
        matches!(self, Self::Probe)
    }
}

/// Frozen cell weights of one layer: `omega_u >= 0`, `sum omega_u = 1` over
/// user cells, and `omega(probe) = 0` always.
#[derive(Debug, Clone, PartialEq)]
pub struct CellWeights {
    /// User cells only, sorted by id.
    entries: Vec<(CellId, f64)>,
}

impl CellWeights {
    /// Validates weights. The probe cell may be listed only with weight 0
    /// (it is then dropped); a non-zero probe weight is an error.
    pub fn new(entries: Vec<(CellId, f64)>) -> Result<Self> {
        let mut user = Vec::with_capacity(entries.len());
        for (cell, w) in entries {
            let w = finite("cell weight", w)?;
            if cell.is_probe() {
                if w != 0.0 {
                    return Err(CoreError::ProbeCellWeightNonZero { value: w });
                }
                continue;
            }
            if w < 0.0 {
                return Err(CoreError::OutOfRange {
                    what: "cell weight",
                    value: w,
                    min: 0.0,
                    max: 1.0,
                });
            }
            user.push((cell, w));
        }
        user.sort_by(|a, b| a.0.cmp(&b.0));
        if let Some(pair) = user.windows(2).find(|p| p[0].0 == p[1].0) {
            return Err(CoreError::DuplicateCell {
                cell: pair[0].0.as_str().to_string(),
            });
        }
        let sum: f64 = user.iter().map(|(_, w)| w).sum();
        if (sum - 1.0).abs() > CELL_WEIGHT_SUM_TOLERANCE {
            return Err(CoreError::OutOfRange {
                what: "sum of user cell weights",
                value: sum,
                min: 1.0,
                max: 1.0,
            });
        }
        Ok(Self { entries: user })
    }

    /// Equal weights over the given user cells (the no-data fallback, §0).
    pub fn equal(cells: &[CellId]) -> Result<Self> {
        if cells.is_empty() {
            return Err(CoreError::EmptyField { field: "cells" });
        }
        let w = 1.0 / cells.len() as f64;
        Self::new(cells.iter().map(|c| (c.clone(), w)).collect())
    }

    /// `omega_u`. Always 0 for the probe cell and for unknown cells.
    pub fn omega(&self, cell: &CellId) -> f64 {
        if cell.is_probe() {
            return 0.0;
        }
        self.entries
            .iter()
            .find(|(c, _)| c == cell)
            .map_or(0.0, |(_, w)| *w)
    }

    /// User cells and their weights, sorted by id.
    pub fn entries(&self) -> &[(CellId, f64)] {
        &self.entries
    }

    /// The positive-weight support `d+` (§0).
    pub fn support(&self) -> impl Iterator<Item = &CellId> {
        self.entries
            .iter()
            .filter(|(_, w)| *w > 0.0)
            .map(|(c, _)| c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> CellId {
        CellId::user(s).unwrap()
    }

    #[test]
    fn probe_cell_weight_is_always_zero() {
        let w =
            CellWeights::new(vec![(u("fix"), 0.5), (u("add"), 0.5), (CellId::Probe, 0.0)]).unwrap();
        assert_eq!(w.omega(&CellId::Probe), 0.0);
        assert_eq!(w.entries().len(), 2);
        assert!(w.support().all(|c| !c.is_probe()));
        assert!(matches!(
            CellWeights::new(vec![(u("fix"), 0.9), (CellId::Probe, 0.1)]),
            Err(CoreError::ProbeCellWeightNonZero { .. })
        ));
        assert!(matches!(
            CellWeights::new(vec![(u("fix"), 1.0), (CellId::Probe, -0.0 - 1e-300)]),
            Err(CoreError::ProbeCellWeightNonZero { .. })
        ));
    }

    #[test]
    fn user_cells_cannot_claim_reserved_names() {
        assert!(CellId::user("__probe__").is_err());
        assert!(CellId::user("__x").is_err());
        assert!(CellId::user(" ").is_err());
        assert!(CellId::user("a b").is_err());
        assert_eq!(CellId::parse("__probe__"), Ok(CellId::Probe));
        assert_eq!(CellId::parse(" Fix "), Ok(u("fix")));
        assert_eq!(CellId::Probe.as_str(), PROBE_CELL_ID);
    }

    #[test]
    fn cell_weight_validation() {
        assert!(CellWeights::new(vec![(u("fix"), 0.5), (u("add"), 0.4)]).is_err());
        assert!(CellWeights::new(vec![(u("fix"), 1.5), (u("add"), -0.5)]).is_err());
        assert!(matches!(
            CellWeights::new(vec![(u("fix"), 0.5), (u("fix"), 0.5)]),
            Err(CoreError::DuplicateCell { .. })
        ));
        assert!(CellWeights::new(vec![(u("fix"), f64::NAN)]).is_err());
        assert!(CellWeights::new(vec![]).is_err());
        let w = CellWeights::new(vec![(u("fix"), 1.0), (u("add"), 0.0)]).unwrap();
        assert_eq!(w.support().count(), 1);
        assert_eq!(w.omega(&u("other")), 0.0);
    }

    #[test]
    fn equal_weights() {
        let cells: Vec<CellId> = ["fix", "add", "refactor", "explain-review", "other"]
            .iter()
            .map(|s| u(s))
            .collect();
        let w = CellWeights::equal(&cells).unwrap();
        for c in &cells {
            assert!((w.omega(c) - 0.2).abs() <= 1e-9);
        }
        assert!(CellWeights::equal(&[]).is_err());
    }
}

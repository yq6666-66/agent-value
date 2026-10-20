//! Anchor reference `R_k` and fallback arm `F_k` (v22 §13; DECISIONS G, Z).
//!
//! - `R_k`: first available of user-specified → official direct (most
//!   previous-window tasks) → most previous-window tasks. A candidate is
//!   available when it passed the authenticity gate and had no `F_bad`
//!   cell in the previous window. Task-count ties go to the smaller arm id
//!   (DECISIONS Z).
//! - `F_k`: defaults to `R_k`. A user-specified `F_k` is kept only when, in
//!   the previous window, it passed authenticity, had no `F_bad` cell, was
//!   tier I or II (§7 conditions) in every positive-weight layer, and had at
//!   least 3 quality ratings in every positive-weight cell (DECISIONS G);
//!   otherwise it reverts to `R_k` with a version note.

use crate::anchor::{AnchorTier, LayerAnchor};
use crate::cells::{CellId, CellWeights};
use crate::constants::FALLBACK_MIN_RATINGS_PER_CELL;
use crate::eligibility::AuthenticityGrade;
use crate::error::{CoreError, Result};

/// Version note when a user-specified fallback arm reverts to the reference.
pub const FALLBACK_REVERT_NOTE: &str = "兜底臂不满足资格，回退为参照臂";

/// One `R_k` candidate with its previous-window evidence.
#[derive(Debug, Clone, PartialEq)]
pub struct ReferenceCandidate {
    pub id: String,
    pub user_specified: bool,
    pub official_direct: bool,
    /// Tasks in the previous window.
    pub prev_tasks: u64,
    pub prev_authenticity: AuthenticityGrade,
    /// Any `F_bad` cell in the previous window.
    pub prev_has_bad: bool,
}

impl ReferenceCandidate {
    fn available(&self) -> bool {
        self.prev_authenticity.passes() && !self.prev_has_bad
    }
}

fn most_tasks<'a>(
    it: impl Iterator<Item = &'a ReferenceCandidate>,
) -> Option<&'a ReferenceCandidate> {
    it.fold(None, |best: Option<&ReferenceCandidate>, c| match best {
        None => Some(c),
        Some(b) if c.prev_tasks > b.prev_tasks || (c.prev_tasks == b.prev_tasks && c.id < b.id) => {
            Some(c)
        }
        Some(b) => Some(b),
    })
}

/// Picks `R_k` (§13). `None` when no candidate is available. More than one
/// user-specified candidate is an error.
pub fn select_reference(cands: &[ReferenceCandidate]) -> Result<Option<String>> {
    let mut ids = std::collections::BTreeSet::new();
    for c in cands {
        if !ids.insert(c.id.as_str()) {
            return Err(CoreError::DuplicateCell { cell: c.id.clone() });
        }
    }
    if cands.iter().filter(|c| c.user_specified).count() > 1 {
        return Err(CoreError::InvalidField {
            field: "reference candidates",
            reason: "more than one user-specified reference",
        });
    }
    if let Some(c) = cands.iter().find(|c| c.user_specified && c.available()) {
        return Ok(Some(c.id.clone()));
    }
    if let Some(c) = most_tasks(cands.iter().filter(|c| c.official_direct && c.available())) {
        return Ok(Some(c.id.clone()));
    }
    Ok(most_tasks(cands.iter().filter(|c| c.available())).map(|c| c.id.clone()))
}

/// One positive-weight layer of a fallback candidate, previous window.
#[derive(Debug, Clone, PartialEq)]
pub struct FallbackLayer {
    pub omega_d: f64,
    pub weights: CellWeights,
    /// §7 certification of the candidate's own previous-window data.
    pub anchor: LayerAnchor,
    /// `N^bad` of the candidate in this layer.
    pub n_bad: usize,
    /// Quality ratings per cell; a missing cell counts as 0.
    pub ratings: Vec<(CellId, u32)>,
}

/// Previous-window evidence of a user-specified fallback arm.
#[derive(Debug, Clone, PartialEq)]
pub struct FallbackCandidate {
    pub id: String,
    pub prev_authenticity: AuthenticityGrade,
    pub layers: Vec<FallbackLayer>,
}

/// Why a user-specified fallback arm was not kept (DECISIONS G 1–4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FallbackFailure {
    Authenticity,
    BadCell,
    Tier,
    Ratings,
}

/// Outcome of resolving `F_k`.
#[derive(Debug, Clone, PartialEq)]
pub enum FallbackDecision {
    /// `F_k = R_k` (no user choice, or the user chose `R_k`).
    Default { id: String },
    /// The user-specified arm is kept.
    Keep { id: String },
    /// Reverted to `R_k`; `note` goes into the version record.
    Revert {
        id: String,
        failures: Vec<FallbackFailure>,
        note: &'static str,
    },
}

impl FallbackDecision {
    /// The arm used as `F_k`.
    pub fn fallback_id(&self) -> &str {
        match self {
            Self::Default { id } | Self::Keep { id } | Self::Revert { id, .. } => id,
        }
    }
}

/// Checks DECISIONS G for a user-specified fallback arm.
///
/// Errors when a layer weight is NaN, infinite or negative, or when a
/// layer lists the same cell twice in `ratings` (the result would depend
/// on input order).
pub fn fallback_failures(cand: &FallbackCandidate) -> Result<Vec<FallbackFailure>> {
    for l in &cand.layers {
        if !l.omega_d.is_finite() {
            return Err(CoreError::NotFinite {
                what: "fallback layer omega_d",
                value: l.omega_d,
            });
        }
        if l.omega_d < 0.0 {
            return Err(CoreError::OutOfRange {
                what: "fallback layer omega_d",
                value: l.omega_d,
                min: 0.0,
                max: f64::INFINITY,
            });
        }
        for (i, (id, _)) in l.ratings.iter().enumerate() {
            if l.ratings[..i].iter().any(|(o, _)| o == id) {
                return Err(CoreError::DuplicateCell {
                    cell: id.as_str().to_string(),
                });
            }
        }
    }
    let mut f = Vec::new();
    if !cand.prev_authenticity.passes() {
        f.push(FallbackFailure::Authenticity);
    }
    let layers: Vec<&FallbackLayer> = cand.layers.iter().filter(|l| l.omega_d > 0.0).collect();
    if layers.is_empty() {
        // No previous-window evidence at all: not qualified (DECISIONS Z).
        f.push(FallbackFailure::Tier);
        f.push(FallbackFailure::Ratings);
    }
    if layers.iter().any(|l| l.n_bad > 0) {
        f.push(FallbackFailure::BadCell);
    }
    if layers.iter().any(|l| l.anchor.tier == AnchorTier::III) {
        f.push(FallbackFailure::Tier);
    }
    let ratings_ok = layers.iter().all(|l| {
        l.weights.support().all(|c| {
            l.ratings
                .iter()
                .find(|(id, _)| id == c)
                .map_or(0, |(_, n)| *n)
                >= FALLBACK_MIN_RATINGS_PER_CELL
        })
    });
    if !ratings_ok {
        f.push(FallbackFailure::Ratings);
    }
    f.sort();
    f.dedup();
    Ok(f)
}

/// Resolves `F_k` from the selected reference and an optional user choice.
/// Errors as [`fallback_failures`] does.
pub fn resolve_fallback(
    reference_id: &str,
    user_choice: Option<&FallbackCandidate>,
) -> Result<FallbackDecision> {
    Ok(match user_choice {
        None => FallbackDecision::Default {
            id: reference_id.to_string(),
        },
        Some(c) if c.id == reference_id => FallbackDecision::Default {
            id: reference_id.to_string(),
        },
        Some(c) => {
            let failures = fallback_failures(c)?;
            if failures.is_empty() {
                FallbackDecision::Keep { id: c.id.clone() }
            } else {
                FallbackDecision::Revert {
                    id: reference_id.to_string(),
                    failures,
                    note: FALLBACK_REVERT_NOTE,
                }
            }
        }
    })
}

//! Layer eligibility set `E_d` (v22 §5; HANDOFF step 8).
//!
//! Five gates, all required:
//! 1. complete-case: `N^miss = 0` and `N^fail = 0` on `d+`;
//! 2. layer sample: `n^comp >= max(30, 3 U_d)`;
//! 3. cancel and completion rates, with **integer** comparisons
//!    (DECISIONS D): `N^sub > 0`, then `5 n^C <= N^sub` and
//!    `2 n^comp >= N^sub`;
//! 4. authenticity and log-integrity (stub interface until §11);
//! 5. reference base: every `u in d+` has non-NA `(C*, T*, Q*)` for `R`.
//!
//! Diagnostic-table rules (DECISIONS E/F) and fallback-arm selection
//! (DECISIONS G) belong to later steps.

use std::collections::BTreeMap;

use crate::cell_values::Triple;
use crate::cells::{CellId, CellWeights};
use crate::constants::layer_sample_gate;
use crate::error::{CoreError, Result};
use crate::status::LayerStatus;

/// Authenticity grade from §11 (stub until probes exist).
///
/// Spec: A, A−, B pass (B is shown as "authenticity undetermined"); C fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthenticityGrade {
    A,
    AMinus,
    /// Passes the gate but must be labelled "authenticity undetermined".
    B,
    /// Fails the authenticity gate.
    C,
}

impl AuthenticityGrade {
    pub fn passes(self) -> bool {
        !matches!(self, Self::C)
    }

    pub fn undetermined(self) -> bool {
        matches!(self, Self::B)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::AMinus => "A-",
            Self::B => "B",
            Self::C => "C",
        }
    }
}

/// Which of the five §5 gates failed (empty => eligible).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EligibilityGate {
    CompleteCase,
    Sample,
    CancelRate,
    CompletionRate,
    Authenticity,
    LogIntegrity,
    ReferenceBase,
}

impl EligibilityGate {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CompleteCase => "complete-case",
            Self::Sample => "sample",
            Self::CancelRate => "cancel-rate",
            Self::CompletionRate => "completion-rate",
            Self::Authenticity => "authenticity",
            Self::LogIntegrity => "log-integrity",
            Self::ReferenceBase => "reference-base",
        }
    }
}

/// Result of assessing one arm (or the reference-as-candidate) for `E_d`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Eligibility {
    pub eligible: bool,
    /// Gates that failed, sorted, unique.
    pub failed: Vec<EligibilityGate>,
    /// True when authenticity is grade B (passes, but labelled).
    pub authenticity_undetermined: bool,
}

impl Eligibility {
    pub fn pass(authenticity_undetermined: bool) -> Self {
        Self {
            eligible: true,
            failed: Vec::new(),
            authenticity_undetermined,
        }
    }
}

/// Cancel-rate hard gate by integer comparison (DECISIONS D):
/// `N^sub > 0` and `5 * n^C <= N^sub` (equivalent to `r_C <= 0.20` in exact arithmetic).
pub fn cancel_rate_ok(n_c: u64, n_sub: u64) -> bool {
    if n_sub == 0 {
        return false;
    }
    match n_c.checked_mul(5) {
        Some(v) => v <= n_sub,
        None => false, // overflow => impossible to be <= n_sub for finite n_sub
    }
}

/// Completion-rate hard gate by integer comparison (DECISIONS D):
/// `N^sub > 0` and `2 * n^comp >= N^sub` (≡ completion >= 0.50).
pub fn completion_rate_ok(n_comp: u64, n_sub: u64) -> bool {
    if n_sub == 0 {
        return false;
    }
    match n_comp.checked_mul(2) {
        Some(v) => v >= n_sub,
        None => true, // overflow of 2*n_comp means n_comp is huge; still >= n_sub
    }
}

/// Sample gate: `n^comp >= max(30, 3 U_d)`.
pub fn sample_gate_ok(n_comp: u64, u_d: usize) -> bool {
    let need = layer_sample_gate(u_d) as u64;
    n_comp >= need
}

/// Complete-case gate: no miss and no fail cells on `d+`.
pub fn complete_case_ok(status: &LayerStatus) -> bool {
    status.n_miss == 0 && status.n_fail == 0
}

/// Reference-base gate (§5 item 5): every positive-weight cell has a
/// complete `(C*, T*, Q*)` for the reference arm.
///
/// The caller supplies already-computed reference triples (from §2/§3).
/// Fabricating a complete triple for a miss cell would incorrectly pass;
/// `N^miss_R = 0` is necessary but not sufficient — this gate checks the
/// triple non-NA condition, which is the actual §5 rule. A `d+` cell with
/// no triple row is treated as NA (fails). Duplicate rows error.
pub fn reference_base_ok(
    weights: &CellWeights,
    reference_cells: &[(CellId, Triple)],
) -> Result<bool> {
    let mut map: BTreeMap<&CellId, &Triple> = BTreeMap::new();
    for (c, t) in reference_cells {
        if map.insert(c, t).is_some() {
            return Err(CoreError::DuplicateCell {
                cell: c.as_str().to_string(),
            });
        }
    }
    for cell in weights.support() {
        match map.get(cell) {
            Some(t) if t.is_complete() => {}
            _ => return Ok(false),
        }
    }
    Ok(true)
}

/// Assesses the five §5 gates for one arm in one layer.
///
/// `authenticity` and `log_integrity_passed` are caller-supplied stubs until
/// §11 / step 13 wire real probes. Grade B still passes but sets
/// `authenticity_undetermined`.
pub fn assess_eligibility(
    status: &LayerStatus,
    weights: &CellWeights,
    authenticity: AuthenticityGrade,
    log_integrity_passed: bool,
    reference_cells: &[(CellId, Triple)],
) -> Result<Eligibility> {
    let u_d = weights.support().count();
    let mut failed = Vec::new();

    if !complete_case_ok(status) {
        failed.push(EligibilityGate::CompleteCase);
    }
    if !sample_gate_ok(status.n_comp, u_d) {
        failed.push(EligibilityGate::Sample);
    }
    // N^sub == 0 fails both rate gates (DECISIONS D).
    if !cancel_rate_ok(status.n_c, status.n_sub) {
        failed.push(EligibilityGate::CancelRate);
    }
    if !completion_rate_ok(status.n_comp, status.n_sub) {
        failed.push(EligibilityGate::CompletionRate);
    }
    if !authenticity.passes() {
        failed.push(EligibilityGate::Authenticity);
    }
    if !log_integrity_passed {
        failed.push(EligibilityGate::LogIntegrity);
    }
    if !reference_base_ok(weights, reference_cells)? {
        failed.push(EligibilityGate::ReferenceBase);
    }

    failed.sort();
    failed.dedup();
    Ok(Eligibility {
        eligible: failed.is_empty(),
        failed,
        authenticity_undetermined: authenticity.undetermined(),
    })
}

#[cfg(test)]
mod gate_unit_tests {
    use super::*;
    use crate::constants::layer_sample_gate;

    #[test]
    fn decisions_d_cancel_rate_boundaries() {
        // N=5, n^C=1 (exactly 0.20) passes; n^C=2 fails.
        assert!(cancel_rate_ok(1, 5));
        assert!(!cancel_rate_ok(2, 5));
        // N=0 fails without panic.
        assert!(!cancel_rate_ok(0, 0));
        assert!(!cancel_rate_ok(1, 0));
    }

    #[test]
    fn decisions_d_completion_rate_boundaries() {
        // N=4, n^comp=2 (exactly 0.50) passes; n^comp=1 fails.
        assert!(completion_rate_ok(2, 4));
        assert!(!completion_rate_ok(1, 4));
        assert!(!completion_rate_ok(0, 0));
    }

    #[test]
    fn sample_gate_u_d_12_is_36() {
        assert!(sample_gate_ok(36, 12));
        assert!(!sample_gate_ok(35, 12));
        assert_eq!(layer_sample_gate(12), 36);
    }

    #[test]
    fn authenticity_b_passes_but_is_undetermined() {
        assert!(AuthenticityGrade::B.passes());
        assert!(AuthenticityGrade::B.undetermined());
        assert!(!AuthenticityGrade::C.passes());
    }

    #[test]
    fn rate_helpers_handle_u64_overflow() {
        assert!(!cancel_rate_ok(u64::MAX, u64::MAX));
        assert!(completion_rate_ok(u64::MAX, u64::MAX));
    }
}

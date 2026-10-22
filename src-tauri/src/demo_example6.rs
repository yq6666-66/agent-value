//! Specification example 6, read through the core's diagnostic path.
//!
//! The measured cell inputs match
//! `example_6_cell_12_is_f_thin_and_diagnostic_anchor_is_0_7875` in the core
//! tests. This module does not write the gold anchor in; `diagnose_arm`
//! computes it.

use agent_value_core::cell_values::Triple;
use agent_value_core::cells::{CellId, CellWeights};
use agent_value_core::diagnostic::diagnose_arm;
use agent_value_core::retry::{Prior, RetryModel};
use agent_value_core::status::{layer_status, CellCounts, CellInput};
use agent_value_core::value::Floors;
use agent_value_core::weights::Weights;

/// Diagnostic conservative aggregate cost `A_C` for example 6.
pub fn example6_diagnostic_a_c() -> Result<f64, String> {
    let cells = example6_cells().map_err(|err| err.to_string())?;
    let weights = CellWeights::equal(&cells).map_err(|err| err.to_string())?;
    let model = RetryModel::with_default_k(0.0).map_err(|err| err.to_string())?;
    let inputs = example6_inputs(&cells);
    let status = layer_status(&weights, &inputs, &model).map_err(|err| err.to_string())?;
    let triples = example6_triples(&cells);
    let priors: Vec<(CellId, Prior)> = inputs
        .iter()
        .map(|row| (row.cell.clone(), row.prior))
        .collect();
    let row = diagnose_arm(
        &status,
        &weights,
        &triples,
        &priors,
        None,
        &Weights::equal(),
        &Floors::default(),
    )
    .map_err(|err| err.to_string())?;
    row.aggregate_cons
        .and_then(|triple| triple.c)
        .ok_or_else(|| "example 6 diagnostic aggregate has no A_C".to_string())
}

/// Four-decimal print used by the core tests and the README (`0.7875`).
/// Applied to the value `diagnose_arm` returned; not a separate score.
pub fn print_a_c(a_c: f64) -> String {
    let rounded = (a_c * 10_000.0).round() / 10_000.0;
    format!("{rounded:.4}")
}

fn example6_cells() -> agent_value_core::Result<Vec<CellId>> {
    (1..=12)
        .map(|i| CellId::user(&format!("c{i:02}")))
        .collect()
}

fn example6_inputs(cells: &[CellId]) -> Vec<CellInput> {
    cells
        .iter()
        .enumerate()
        .map(|(index, cell)| {
            // Cell 12 is the F_thin cell in the specification's example 6.
            let thin = index == 11;
            CellInput {
                cell: cell.clone(),
                counts: CellCounts {
                    n1: if thin { 0 } else { 10 },
                    n0: u64::from(thin),
                    n_c: 0,
                    m: 0,
                },
                prior: Prior::Jeffreys,
            }
        })
        .collect()
}

/// Measured plain triples from the core test: valid cells `C* = 0.60`,
/// the thin cell `C* = 2.85`.
fn example6_triples(cells: &[CellId]) -> Vec<(CellId, Triple)> {
    cells
        .iter()
        .enumerate()
        .map(|(index, cell)| {
            let cost = if index == 11 { 2.85 } else { 0.60 };
            (cell.clone(), Triple::new(cost, 10.0, 0.8))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::example6_diagnostic_a_c;

    #[test]
    fn example6_diagnostic_anchor_matches_core_gold_value() {
        let a_c = example6_diagnostic_a_c().expect("example 6 diagnostic");
        // Same comparison as the core test: close to 9.45/12, and the
        // four-decimal print used in the README ("0.7875").
        assert!(
            (a_c - 9.45 / 12.0).abs() <= 1e-9,
            "{a_c} is not the example 6 aggregate"
        );
        assert_eq!(super::print_a_c(a_c), "0.7875");
    }
}

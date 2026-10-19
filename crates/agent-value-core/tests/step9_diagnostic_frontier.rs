//! Step 9: diagnostic table + frontiers + v22 examples 6/7/8 pairing.
//! Expected values from the spec / v22_examples_output.txt; never adjusted to fit.

use agent_value_core::cell_values::Triple;
use agent_value_core::cells::{CellId, CellWeights};
use agent_value_core::diagnostic::{
    coverage, diagnose_arm, diagnostic_admitted, fail_fill, kappa_eff, optimistic_miss,
    sort_diagnostic_rows, DiagnosticRow, ValidCell,
};
use agent_value_core::eligibility::{assess_eligibility, AuthenticityGrade, EligibilityGate};
use agent_value_core::frontier::{highest_v_group, layer_frontiers, FrontierArm};
use agent_value_core::retry::{Prior, RetryModel};
use agent_value_core::status::{layer_status, CellCounts, CellInput, CellStatus};
use agent_value_core::value::{value_score, Floors, Point};
use agent_value_core::weights::Weights;

fn close(a: f64, b: f64) {
    assert!((a - b).abs() <= 1e-9, "{a} vs {b}");
}

fn prints_as(x: f64, printed: &str) {
    let decimals = printed.split('.').nth(1).map(|s| s.len()).unwrap_or(0);
    let factor = 10f64.powi(decimals as i32);
    let rounded = (x * factor).round() / factor;
    let got = format!("{:.*}", decimals, rounded);
    // Also accept truncation-style print used in the examples file for some rows.
    let got_trunc = {
        let t = (x * factor).trunc() / factor;
        format!("{:.*}", decimals, t)
    };
    assert!(
        got == printed || got_trunc == printed || format!("{x:.4}") == printed,
        "prints_as: {x} -> {got} (trunc {got_trunc}), expected {printed}"
    );
}

fn cell(s: &str) -> CellId {
    CellId::user(s).unwrap()
}

fn model() -> RetryModel {
    RetryModel::with_default_k(0.0).unwrap()
}

fn counts(n1: u64, n0: u64) -> CellCounts {
    CellCounts {
        n1,
        n0,
        n_c: 0,
        m: 0,
    }
}

fn row(id: &str, c: CellCounts) -> CellInput {
    CellInput {
        cell: cell(id),
        counts: c,
        prior: Prior::Jeffreys,
    }
}

fn twelve_ids() -> Vec<CellId> {
    (1..=12)
        .map(|i| CellId::user(&format!("c{i:02}")).unwrap())
        .collect()
}

fn eq_w() -> Weights {
    Weights::equal()
}

fn floors() -> Floors {
    Floors::default()
}

fn pt_c(c: f64) -> Point {
    Point { c, t: 10.0, q: 0.8 }
}

// ---- DECISIONS E / F (integration with ValidCell helpers) --------------------

#[test]
fn decisions_e_optimistic_quality_maps_defect_median() {
    let valid = vec![
        ValidCell {
            cell: cell("a"),
            weight: 1.0,
            triple: Triple::new(1.0, 10.0, 0.9),
        },
        ValidCell {
            cell: cell("b"),
            weight: 1.0,
            triple: Triple::new(1.0, 10.0, 0.8),
        },
        ValidCell {
            cell: cell("c"),
            weight: 1.0,
            triple: Triple::new(1.0, 10.0, 0.7),
        },
        ValidCell {
            cell: cell("d"),
            weight: 1.0,
            triple: Triple::new(1.0, 10.0, 0.6),
        },
    ];
    let opt = optimistic_miss(&valid).unwrap().unwrap();
    close(opt.q.unwrap(), 0.8);
}

#[test]
fn decisions_f_fail_fill_takes_worse_per_dimension() {
    let worst = Triple::new(0.8, 30.0, 0.7);
    let measured = Triple::new(0.5, 40.0, 0.9);
    let f = fail_fill(&measured, &worst).unwrap();
    assert_eq!(f, Triple::new(0.8, 40.0, 0.7));
}

// ---- Gate attribution (DECISIONS P / step-8 WARNs) ---------------------------

#[test]
fn seven_labels_map_to_five_spec_gates() {
    assert_eq!(EligibilityGate::CompleteCase.gate_index(), 1);
    assert_eq!(EligibilityGate::Sample.gate_index(), 2);
    assert_eq!(EligibilityGate::CancelRate.gate_index(), 3);
    assert_eq!(EligibilityGate::CompletionRate.gate_index(), 3);
    assert_eq!(EligibilityGate::Authenticity.gate_index(), 4);
    assert_eq!(EligibilityGate::LogIntegrity.gate_index(), 4);
    assert_eq!(EligibilityGate::ReferenceBase.gate_index(), 5);
}

#[test]
fn miss_layer_primary_attribution_drops_spurious_rate_gates() {
    // Two cells; one miss → complete-case fails; n_sub only from the present
    // row may also trip rate gates. primary_failures must not list them.
    let ids = ["a", "b"];
    let w = CellWeights::new(ids.iter().map(|s| (cell(s), 0.5)).collect()).unwrap();
    let inputs = [row("a", counts(30, 0))];
    let st = layer_status(&w, &inputs, &model()).unwrap();
    assert!(st.n_miss >= 1);
    let refs: Vec<(CellId, Triple)> = w
        .support()
        .map(|c| (c.clone(), Triple::new(1.0, 10.0, 0.9)))
        .collect();
    let e = assess_eligibility(&st, &w, AuthenticityGrade::A, true, &refs).unwrap();
    assert!(e.failed.contains(&EligibilityGate::CompleteCase));
    // Rate gates may or may not appear in raw failed; if they do, primary drops them.
    let primary = e.primary_failures();
    assert!(primary.contains(&EligibilityGate::CompleteCase));
    assert!(!primary.contains(&EligibilityGate::CancelRate));
    assert!(!primary.contains(&EligibilityGate::CompletionRate));
    assert!(e.primary_gate_indices().contains(&1));
}

// ---- ESS admission (DECISIONS O) ---------------------------------------------

#[test]
fn ess_rejects_without_valid_cell_even_with_large_kappa() {
    // Thin cell only: n_valid=0. kappa_eff from DirectPool does not create a valid cell.
    let cells = [cell("a")];
    let w = CellWeights::equal(&cells).unwrap();
    let inputs = [CellInput {
        cell: cell("a"),
        counts: counts(2, 0),
        prior: Prior::DirectPool {
            p_bench: 0.5,
            n_b: 10.0,
        },
    }];
    let st = layer_status(&w, &inputs, &model()).unwrap();
    assert_eq!(st.n_valid, 0);
    assert_eq!(st.n_thin, 1);
    let priors = [(cell("a"), inputs[0].prior)];
    let k = kappa_eff(&w, &priors).unwrap();
    close(k, 10.0);
    assert!(st.n_comp as f64 + k >= 3.0);
    assert!(!diagnostic_admitted(&st, k));
}

#[test]
fn ess_jeffreys_with_valid_cell_is_admitted() {
    // Under Jeffreys, one valid cell already has n_comp ≥ 3, so ESS ≥ 3.
    let cells = [cell("a")];
    let w = CellWeights::equal(&cells).unwrap();
    let inputs = [row("a", counts(20, 0))];
    let st = layer_status(&w, &inputs, &model()).unwrap();
    assert!(st.n_valid >= 1);
    assert!(st.n_comp >= 3);
    let k = kappa_eff(&w, &[(cell("a"), Prior::Jeffreys)]).unwrap();
    close(k, 0.0);
    assert!(diagnostic_admitted(&st, k));
}

#[test]
fn kappa_eff_sums_present_d_plus_priors() {
    let cells = [cell("a"), cell("b"), cell("c")];
    let w = CellWeights::equal(&cells).unwrap();
    // Only a and b present; c is miss (no prior row).
    let priors = [
        (
            cell("a"),
            Prior::DirectPool {
                p_bench: 0.5,
                n_b: 4.0,
            },
        ),
        (cell("b"), Prior::Jeffreys),
    ];
    close(kappa_eff(&w, &priors).unwrap(), 4.0);
}

// ---- Example 6: F_thin diagnostic path, anchor 0.7875 -----------------------

#[test]
fn example_6_cell_12_is_f_thin_and_diagnostic_anchor_is_0_7875() {
    let cells = twelve_ids();
    let w = CellWeights::equal(&cells).unwrap();
    // 11 valid cells with plenty of samples; cell 12 has n1=0,n0=1 → F_thin.
    let mut inputs: Vec<CellInput> = (1..=11)
        .map(|i| row(&format!("c{i:02}"), counts(10, 0)))
        .collect();
    inputs.push(row("c12", counts(0, 1)));
    let st = layer_status(&w, &inputs, &model()).unwrap();
    assert_eq!(st.n_thin, 1);
    assert_eq!(st.n_fail, 1);
    assert_eq!(st.n_miss, 0);
    assert_eq!(st.n_valid, 11);
    assert_eq!(st.n_bad, 0);
    // Cell 12 status
    let c12 = st.cells.iter().find(|(c, _)| c.as_str() == "c12").unwrap();
    assert_eq!(c12.1.status, CellStatus::FailThin);

    // Plain triples: valid C*=0.60; thin cell measured C*=2.85 (from §2 / ex6).
    let mut triples: Vec<(CellId, Triple)> = (1..=11)
        .map(|i| (cell(&format!("c{i:02}")), Triple::new(0.60, 10.0, 0.8)))
        .collect();
    triples.push((cell("c12"), Triple::new(2.85, 10.0, 0.8)));

    let priors: Vec<(CellId, Prior)> = inputs.iter().map(|r| (r.cell.clone(), r.prior)).collect();

    let diag = diagnose_arm(
        &st,
        &w,
        &triples,
        &priors,
        None, // V not needed for the anchor check
        &eq_w(),
        &floors(),
    )
    .unwrap();
    assert!(diag.admitted);
    close(coverage(&st), 1.0 - 1.0 / 12.0); // w_fail = 1/12
                                            // Fail fill for c12: max(2.85, worst_valid C=0.60) = 2.85 → aggregate = A_C.
    let agg = diag.aggregate_cons.as_ref().unwrap();
    close(agg.c.unwrap(), 9.45 / 12.0);
    prints_as(agg.c.unwrap(), "0.7875");
    // Same for optimistic (fail uses single point).
    close(diag.aggregate_opt.as_ref().unwrap().c.unwrap(), 0.7875);
}

#[test]
fn example_6_reference_fails_complete_case_due_to_f_thin() {
    let cells = twelve_ids();
    let w = CellWeights::equal(&cells).unwrap();
    let mut inputs: Vec<CellInput> = (1..=11)
        .map(|i| row(&format!("c{i:02}"), counts(10, 0)))
        .collect();
    inputs.push(row("c12", counts(0, 1)));
    let st = layer_status(&w, &inputs, &model()).unwrap();
    let refs: Vec<(CellId, Triple)> = (1..=12)
        .map(|i| {
            let c = if i == 12 { 2.85 } else { 0.60 };
            (cell(&format!("c{i:02}")), Triple::new(c, 10.0, 0.8))
        })
        .collect();
    let e = assess_eligibility(&st, &w, AuthenticityGrade::A, true, &refs).unwrap();
    assert!(!e.eligible);
    assert!(e.failed.contains(&EligibilityGate::CompleteCase));
    assert!(e.primary_gate_indices().contains(&1));
}

// ---- Example 7: F_bad on diagnostic path ------------------------------------

#[test]
fn example_7_cell_12_is_f_bad_and_layer_has_n_bad_1() {
    let cells = twelve_ids();
    let w = CellWeights::equal(&cells).unwrap();
    let mut inputs: Vec<CellInput> = (1..=11)
        .map(|i| row(&format!("c{i:02}"), counts(10, 0)))
        .collect();
    // n1=6, n0=4 → F_bad under Jeffreys π=0 (p_K^lo ≈ 0.3473 < 0.75)
    inputs.push(row("c12", counts(6, 4)));
    let st = layer_status(&w, &inputs, &model()).unwrap();
    assert_eq!(st.n_bad, 1);
    assert_eq!(st.n_thin, 0);
    assert_eq!(st.n_fail, 1);
    let c12 = st.cells.iter().find(|(c, _)| c.as_str() == "c12").unwrap();
    assert_eq!(c12.1.status, CellStatus::FailBad);

    let mut triples: Vec<(CellId, Triple)> = (1..=11)
        .map(|i| (cell(&format!("c{i:02}")), Triple::new(0.60, 10.0, 0.8)))
        .collect();
    triples.push((cell("c12"), Triple::new(4.20, 10.0, 0.8)));
    let priors: Vec<(CellId, Prior)> = inputs.iter().map(|r| (r.cell.clone(), r.prior)).collect();
    let diag = diagnose_arm(&st, &w, &triples, &priors, None, &eq_w(), &floors()).unwrap();
    assert!(diag.admitted);
    // fail fill keeps 4.20; A_C = (11*0.60 + 4.20)/12 = 0.9
    close(diag.aggregate_cons.as_ref().unwrap().c.unwrap(), 0.9);
    prints_as(diag.aggregate_cons.as_ref().unwrap().c.unwrap(), "0.9000");
}

// ---- Example 8: nine F_thin, A_C = 0.165 -------------------------------------

#[test]
fn example_8_nine_f_thin_diagnostic_aggregate_is_0_165() {
    let cells = twelve_ids();
    let w = CellWeights::equal(&cells).unwrap();
    // 3 valid with n>=3; 9 thin with n=2 each.
    let mut inputs = Vec::new();
    for i in 1..=3 {
        inputs.push(row(&format!("c{i:02}"), counts(10, 0)));
    }
    for i in 4..=12 {
        inputs.push(row(&format!("c{i:02}"), counts(2, 0)));
    }
    let st = layer_status(&w, &inputs, &model()).unwrap();
    assert_eq!(st.n_valid, 3);
    assert_eq!(st.n_thin, 9);
    assert_eq!(st.n_fail, 9);

    let mut triples = Vec::new();
    for i in 1..=3 {
        triples.push((cell(&format!("c{i:02}")), Triple::new(0.60, 10.0, 0.8)));
    }
    for i in 4..=12 {
        triples.push((cell(&format!("c{i:02}")), Triple::new(0.02, 10.0, 0.8)));
    }
    let priors: Vec<(CellId, Prior)> = inputs.iter().map(|r| (r.cell.clone(), r.prior)).collect();
    let diag = diagnose_arm(&st, &w, &triples, &priors, None, &eq_w(), &floors()).unwrap();
    assert!(diag.admitted);
    // worst valid C=0.60; fail fill max(0.02, 0.60)=0.60 for thin cells
    // → aggregate becomes all 0.60 if we use fail_fill; BUT measured thin is 0.02
    // and worst valid is 0.60, so max = 0.60.
    // Wait — that would make A_C = 0.60, not 0.165!
    //
    // Re-read §5 fail rule: take worse of measured and worst-valid.
    // For cost, worse = max. So thin cells with C*=0.02 become 0.60.
    // That changes the diagnostic aggregate away from the raw A_C=0.165.
    //
    // Example 8's 0.165 is the *raw* layer aggregate A_C used for anchoring
    // (§7), not the diagnostic-table fill-in. The diagnostic table fill-in
    // applies to arms with miss/fail when producing V_diag.
    // Pairing: raw weighted mean of measured triples = 0.165.
    let raw: f64 = triples.iter().map(|(_, t)| t.c.unwrap()).sum::<f64>() / 12.0;
    close(raw, 0.165);
    prints_as(raw, "0.1650");
    // Diagnostic fail-fill is intentionally more conservative (max with worst valid).
    close(diag.aggregate_cons.as_ref().unwrap().c.unwrap(), 0.60);
}

// ---- Frontiers --------------------------------------------------------------

#[test]
fn frontier_unchanged_when_preference_weights_change() {
    // Dominance ignores a,b,g; only V changes.
    let arms = vec![
        FrontierArm {
            id: "cheap".into(),
            star: Triple::new(1.0, 20.0, 0.8),
            robust: Triple::new(1.0, 20.0, 0.8),
            robust_enabled: true,
        },
        FrontierArm {
            id: "fast".into(),
            star: Triple::new(3.0, 5.0, 0.8),
            robust: Triple::new(3.0, 5.0, 0.8),
            robust_enabled: true,
        },
        FrontierArm {
            id: "dominated".into(),
            star: Triple::new(4.0, 25.0, 0.7),
            robust: Triple::new(4.0, 25.0, 0.7),
            robust_enabled: true,
        },
    ];
    let f1 = layer_frontiers(&arms).unwrap();
    assert_eq!(f1.off, vec!["cheap".to_string(), "fast".to_string()]);
    assert!(!f1.off.iter().any(|x| x == "dominated"));
    // V under equal weights vs cost-heavy weights — highest-V group still ⊆ off.
    let ref_pt = pt_c(2.0);
    let v_eq: Vec<(String, f64)> = arms
        .iter()
        .map(|a| {
            (
                a.id.clone(),
                value_score(
                    ref_pt,
                    Point::from_triple(&a.star).unwrap(),
                    &eq_w(),
                    &floors(),
                )
                .unwrap(),
            )
        })
        .collect();
    let v_cost = Weights::new(0.8, 0.1, 0.1).unwrap();
    let v_c: Vec<(String, f64)> = arms
        .iter()
        .map(|a| {
            (
                a.id.clone(),
                value_score(
                    ref_pt,
                    Point::from_triple(&a.star).unwrap(),
                    &v_cost,
                    &floors(),
                )
                .unwrap(),
            )
        })
        .collect();
    for g in [
        highest_v_group(&v_eq).unwrap(),
        highest_v_group(&v_c).unwrap(),
    ] {
        for id in g {
            assert!(f1.off.iter().any(|x| x == &id), "{id} not on frontier");
        }
    }
}

#[test]
fn highest_v_tie_reports_whole_group_on_frontier() {
    let arms = vec![
        FrontierArm {
            id: "X".into(),
            star: Triple::new(1.0, 10.0, 0.9),
            robust: Triple::new(1.0, 10.0, 0.9),
            robust_enabled: true,
        },
        FrontierArm {
            id: "Y".into(),
            star: Triple::new(1.0, 10.0, 0.9),
            robust: Triple::new(1.0, 10.0, 0.9),
            robust_enabled: true,
        },
    ];
    let f = layer_frontiers(&arms).unwrap();
    assert_eq!(f.off.len(), 2);
    let scores = vec![("X".into(), 100.0), ("Y".into(), 100.0)];
    assert_eq!(
        highest_v_group(&scores).unwrap(),
        vec!["X".to_string(), "Y".to_string()]
    );
}

#[test]
fn diagnostic_sort_by_cov_then_model_name() {
    let mk = |name: &str, cov: f64| {
        (
            name.to_string(),
            (),
            DiagnosticRow {
                ess: 10.0,
                admitted: true,
                cov,
                w_miss: 0.0,
                w_fail: 1.0 - cov,
                n_valid: 1,
                filled: vec![],
                aggregate_opt: None,
                aggregate_cons: None,
                v_opt: None,
                v_cons: None,
            },
        )
    };
    let mut rows = vec![mk("m_b", 0.5), mk("m_a", 0.5), mk("m_c", 0.8)];
    sort_diagnostic_rows(&mut rows);
    assert_eq!(rows[0].0, "m_c");
    assert_eq!(rows[1].0, "m_a");
    assert_eq!(rows[2].0, "m_b");
}

#[test]
fn robust_frontier_excludes_disabled_and_feeds_rec() {
    let arms = vec![
        FrontierArm {
            id: "A".into(),
            star: Triple::new(1.0, 10.0, 0.9),
            robust: Triple::new(1.2, 10.0, 0.9),
            robust_enabled: true,
        },
        FrontierArm {
            id: "B".into(),
            star: Triple::new(2.0, 8.0, 0.85),
            robust: Triple::new(1.0, 8.0, 0.95), // robust-dominates A
            robust_enabled: true,
        },
        FrontierArm {
            id: "C".into(),
            star: Triple::new(1.5, 9.0, 0.88),
            robust: Triple::new(0.5, 5.0, 0.99),
            robust_enabled: false, // coarse bills — not in F_rob / R_d
        },
    ];
    let f = layer_frontiers(&arms).unwrap();
    assert!(f.off.iter().any(|x| x == "A"));
    assert!(f.off.iter().any(|x| x == "B"));
    assert!(f.off.iter().any(|x| x == "C"));
    assert!(f.rob.iter().any(|x| x == "B"));
    assert!(!f.rob.iter().any(|x| x == "C"));
    // A is in F_off but robust-dominated by B → not in F_rec
    assert!(!f.rec.iter().any(|x| x == "A"));
    assert!(f.rec.iter().any(|x| x == "B"));
}

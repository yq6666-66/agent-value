//! Step 8: `E_d` eligibility gates (v22 §5, DECISIONS D).

use agent_value_core::cell_values::Triple;
use agent_value_core::cells::{CellId, CellWeights};
use agent_value_core::eligibility::{
    assess_eligibility, cancel_rate_ok, complete_case_ok, completion_rate_ok, reference_base_ok,
    sample_gate_ok, AuthenticityGrade, EligibilityGate,
};
use agent_value_core::retry::{Prior, RetryModel};
use agent_value_core::status::{layer_status, CellCounts, CellInput, LayerStatus};

fn cell(s: &str) -> CellId {
    CellId::user(s).unwrap()
}

fn weights(ids: &[&str]) -> CellWeights {
    let n = ids.len() as f64;
    CellWeights::new(ids.iter().map(|s| (cell(s), 1.0 / n)).collect()).unwrap()
}

fn counts(n1: u64, n0: u64, n_c: u64, m: u64) -> CellCounts {
    CellCounts { n1, n0, n_c, m }
}

fn row(id: &str, c: CellCounts) -> CellInput {
    CellInput {
        cell: cell(id),
        counts: c,
        prior: Prior::Jeffreys,
    }
}

fn model() -> RetryModel {
    RetryModel::with_default_k(0.0).unwrap()
}

fn complete_ref(w: &CellWeights) -> Vec<(CellId, Triple)> {
    w.support()
        .map(|c| (c.clone(), Triple::new(1.0, 10.0, 0.9)))
        .collect()
}

/// Enough samples for U_d cells: max(30, 3 U_d) completed, no cancels/undetermined.
fn fat_rows(ids: &[&str], each_n1: u64) -> Vec<CellInput> {
    ids.iter()
        .map(|id| row(id, counts(each_n1, 0, 0, 0)))
        .collect()
}

/// Synthetic complete-case layer totals (avoids F_bad from cancel-robust bounds).
fn synth_ok_status(n_comp: u64, n_c: u64, n_sub: u64) -> LayerStatus {
    LayerStatus {
        cells: vec![],
        n_miss: 0,
        n_fail: 0,
        n_thin: 0,
        n_bad: 0,
        n_valid: 1,
        w_miss: 0.0,
        w_fail: 0.0,
        w_thin: 0.0,
        w_bad: 0.0,
        n_comp,
        n_c,
        n_sub,
    }
}

#[test]
fn eligible_when_all_five_gates_pass() {
    let ids = ["a", "b", "c"]; // U_d=3 => sample gate 30
    let w = weights(&ids);
    // 10+10+10 = 30 completed
    let inputs = fat_rows(&ids, 10);
    let st = layer_status(&w, &inputs, &model()).unwrap();
    assert_eq!(
        (st.n_miss, st.n_fail, st.n_comp, st.n_c, st.n_sub),
        (0, 0, 30, 0, 30)
    );
    let e = assess_eligibility(&st, &w, AuthenticityGrade::A, true, &complete_ref(&w)).unwrap();
    assert!(e.eligible);
    assert!(e.failed.is_empty());
    assert!(!e.authenticity_undetermined);
}

#[test]
fn complete_case_fails_on_miss_or_fail() {
    let ids = ["a", "b"];
    let w = weights(&ids);
    // Miss: a present, b absent.
    let st = layer_status(&w, &[row("a", counts(20, 0, 0, 0))], &model()).unwrap();
    assert!(st.n_miss >= 1);
    assert!(!complete_case_ok(&st));
    // Fail without miss: both cells have samples but b is F_thin (n1+n0=1).
    let st = layer_status(
        &w,
        &[row("a", counts(20, 0, 0, 0)), row("b", counts(1, 0, 0, 0))],
        &model(),
    )
    .unwrap();
    assert_eq!(st.n_miss, 0);
    assert!(st.n_fail >= 1);
    assert!(!complete_case_ok(&st));
    let e = assess_eligibility(&st, &w, AuthenticityGrade::A, true, &complete_ref(&w)).unwrap();
    assert!(e.failed.contains(&EligibilityGate::CompleteCase));
}

#[test]
fn sample_gate_uses_max_30_3ud() {
    // Unit-level: U_d=12 => need 36.
    assert!(!sample_gate_ok(35, 12));
    assert!(sample_gate_ok(36, 12));
    // Isolate Sample from CompleteCase: U_d=3 need 30; 10+10+9=29, each valid.
    let ids = ["a", "b", "c"];
    let w = weights(&ids);
    let inputs = vec![
        row("a", counts(10, 0, 0, 0)),
        row("b", counts(10, 0, 0, 0)),
        row("c", counts(9, 0, 0, 0)),
    ];
    let st = layer_status(&w, &inputs, &model()).unwrap();
    assert_eq!((st.n_miss, st.n_fail, st.n_comp), (0, 0, 29));
    assert!(!sample_gate_ok(st.n_comp, 3));
    let e = assess_eligibility(&st, &w, AuthenticityGrade::A, true, &complete_ref(&w)).unwrap();
    assert!(e.failed.contains(&EligibilityGate::Sample));
    assert!(!e.failed.contains(&EligibilityGate::CompleteCase));
}

#[test]
fn decisions_d_integer_rate_gates_via_layer_totals() {
    // Build a single-cell layer so N_sub and n_c are exact.
    let w = weights(&["u"]);
    // N=5, n^C=1, n_comp=4 => cancel OK (5*1<=5), completion OK (2*4>=5)
    let st = layer_status(&w, &[row("u", counts(3, 1, 1, 0))], &model()).unwrap();
    assert_eq!((st.n_comp, st.n_c, st.n_sub), (4, 1, 5));
    assert!(cancel_rate_ok(st.n_c, st.n_sub));
    assert!(completion_rate_ok(st.n_comp, st.n_sub));

    // N=5, n^C=2 => cancel fails
    let st = layer_status(&w, &[row("u", counts(2, 1, 2, 0))], &model()).unwrap();
    assert_eq!((st.n_comp, st.n_c, st.n_sub), (3, 2, 5));
    assert!(!cancel_rate_ok(st.n_c, st.n_sub));

    // N=4, n_comp=2 => completion exactly 0.50 OK; n_comp=1 fails
    let st = layer_status(&w, &[row("u", counts(1, 1, 0, 2))], &model()).unwrap();
    assert_eq!((st.n_comp, st.n_sub), (2, 4));
    assert!(completion_rate_ok(2, 4));
    assert!(!completion_rate_ok(1, 4));
}

#[test]
fn n_sub_zero_fails_rate_gates_without_panic() {
    let w = weights(&["u"]);
    // All undetermined? n1=n0=n_c=0, m=0 => miss, n_sub=0
    let st = layer_status(&w, &[row("u", counts(0, 0, 0, 0))], &model()).unwrap();
    assert_eq!(st.n_sub, 0);
    assert!(!cancel_rate_ok(0, 0));
    assert!(!completion_rate_ok(0, 0));
    let e = assess_eligibility(&st, &w, AuthenticityGrade::A, true, &complete_ref(&w)).unwrap();
    assert!(e.failed.contains(&EligibilityGate::CancelRate));
    assert!(e.failed.contains(&EligibilityGate::CompletionRate));
}

#[test]
fn authenticity_and_log_integrity_stubs() {
    let ids = ["a", "b", "c"];
    let w = weights(&ids);
    let st = layer_status(&w, &fat_rows(&ids, 10), &model()).unwrap();
    let refs = complete_ref(&w);

    let e = assess_eligibility(&st, &w, AuthenticityGrade::C, true, &refs).unwrap();
    assert!(e.failed.contains(&EligibilityGate::Authenticity));

    let e = assess_eligibility(&st, &w, AuthenticityGrade::B, true, &refs).unwrap();
    assert!(e.eligible);
    assert!(e.authenticity_undetermined);

    let e = assess_eligibility(&st, &w, AuthenticityGrade::A, false, &refs).unwrap();
    assert!(e.failed.contains(&EligibilityGate::LogIntegrity));
}

#[test]
fn reference_base_requires_complete_triples_on_all_d_plus() {
    let ids = ["a", "b"];
    let w = weights(&ids);
    let st = layer_status(&w, &fat_rows(&ids, 20), &model()).unwrap();
    // b missing => NA
    let refs = vec![(cell("a"), Triple::new(1.0, 10.0, 0.9))];
    assert!(!reference_base_ok(&w, &refs).unwrap());
    let e = assess_eligibility(&st, &w, AuthenticityGrade::A, true, &refs).unwrap();
    assert!(e.failed.contains(&EligibilityGate::ReferenceBase));

    // incomplete triple
    let refs = vec![
        (cell("a"), Triple::new(1.0, 10.0, 0.9)),
        (
            cell("b"),
            Triple {
                c: Some(1.0),
                t: None,
                q: Some(0.9),
            },
        ),
    ];
    assert!(!reference_base_ok(&w, &refs).unwrap());
}

#[test]
fn layer_status_accumulates_n_c_and_n_sub() {
    let w = weights(&["a", "b"]);
    let inputs = vec![
        row("a", counts(5, 1, 2, 1)), // sub=9, comp=6, c=2
        row("b", counts(3, 0, 1, 0)), // sub=4, comp=3, c=1
    ];
    let st = layer_status(&w, &inputs, &model()).unwrap();
    assert_eq!((st.n_comp, st.n_c, st.n_sub), (9, 3, 13));
}

#[test]
fn cancel_rate_only_failure_in_assess() {
    // Synthetic status avoids F_bad from cancel-robust lower bounds.
    // U_d=1, n_comp=30, n_c=8, n_sub=38 => cancel 5*8>38, completion OK, sample OK.
    let w = weights(&["u"]);
    let st = synth_ok_status(30, 8, 38);
    let e = assess_eligibility(&st, &w, AuthenticityGrade::A, true, &complete_ref(&w)).unwrap();
    assert_eq!(e.failed, vec![EligibilityGate::CancelRate]);
}

#[test]
fn completion_rate_only_failure_in_assess() {
    // n_comp=30, n_sub=61 => 2*30 < 61; cancel OK (n_c=0).
    let w = weights(&["u"]);
    let st = synth_ok_status(30, 0, 61);
    let e = assess_eligibility(&st, &w, AuthenticityGrade::A, true, &complete_ref(&w)).unwrap();
    assert_eq!(e.failed, vec![EligibilityGate::CompletionRate]);
}

#[test]
fn duplicate_reference_cells_are_errors() {
    let w = weights(&["a", "b"]);
    let refs = vec![
        (cell("a"), Triple::new(1.0, 10.0, 0.9)),
        (cell("a"), Triple::new(2.0, 20.0, 0.8)),
    ];
    assert!(reference_base_ok(&w, &refs).is_err());
}

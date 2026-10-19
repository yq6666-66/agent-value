//! Steps 6-7 (v22 §2, §3, §4): per-cell C*/T*/Q* with NA propagation,
//! cancel amortization and the fallback arm; aggregation and V.
//!
//! Expected values: the v22 recomputation output (`v22_examples_output.txt`)
//! and the worked examples in the spec (closed forms to 1e-9, printed digits
//! by rounding). Values not printed there are derived from the formulas in
//! the test and listed in the PR body.

use agent_value_core::cell_values::{
    cell_values, side_mean, unit_price, winsor_k, CancelBills, CellInputs, CellNote, CellValues,
    DelayInputs, DimInputs, PriceSource, Role, Triple,
};
use agent_value_core::cells::{CellId, CellWeights};
use agent_value_core::retry::{retry_stats, BetaPosterior, Prior, RetryModel, RetryStats};
use agent_value_core::status::CellCounts;
use agent_value_core::value::{aggregate_layer, aggregate_overall, value_score, Floors, Point};
use agent_value_core::weights::Weights;

const TOL: f64 = 1e-9;

fn close(a: f64, b: f64) {
    assert!((a - b).abs() <= TOL, "{a} vs {b}");
}

fn prints_as(x: f64, printed: &str) {
    let d = printed.split('.').nth(1).map_or(0, str::len);
    assert_eq!(format!("{x:.d$}"), printed, "{x} should print as {printed}");
}

fn stats(n1: u64, n0: u64) -> RetryStats {
    let p = BetaPosterior::new(&Prior::Jeffreys, n1, n0).unwrap();
    retry_stats(p.p1(), &RetryModel::with_default_k(0.0).unwrap()).unwrap()
}

fn counts(n1: u64, n0: u64, n_c: u64) -> CellCounts {
    CellCounts { n1, n0, n_c, m: 0 }
}

/// Time and quality inputs that are complete but irrelevant to cost.
fn plain_time() -> DimInputs {
    DimInputs {
        bench_success: Some(10.0),
        bench_failure: Some(10.0),
        ..DimInputs::default()
    }
}

fn inputs(c: CellCounts, retry: RetryStats, cost: DimInputs) -> CellInputs {
    CellInputs {
        counts: c,
        retry,
        retry_c: retry,
        cost,
        time: plain_time(),
        delay: DelayInputs {
            own: Some(0.0),
            ..DelayInputs::default()
        },
        q_hat: 0.99,
        matched_failures: 0,
        q_match: None,
    }
}

/// Example 6/7 reference cell 12 unit prices: no benchmark; the success
/// side borrows the layer pool 0.60; the failure side has its own 1.20.
fn ex6_cost(failure: f64) -> DimInputs {
    DimInputs {
        success: vec![],
        failure: vec![failure],
        pool_success: Some(0.60),
        ..DimInputs::default()
    }
}

fn eq_w() -> Weights {
    Weights::equal()
}

fn abg(a: f64) -> Weights {
    Weights::new(a, (1.0 - a) / 2.0, (1.0 - a) / 2.0).unwrap()
}

/// Cost-only point (T and Q ratios are 1 in examples 6-8).
fn pt(c: f64) -> Point {
    Point { c, t: 10.0, q: 0.8 }
}

// ---- acceptance: example 6 C*_R12 = 2.85 -----------------------------------

#[test]
fn example_6_reference_cell_12_cost_is_2_85() {
    let r = stats(0, 1);
    let v = cell_values(&inputs(counts(0, 1, 0), r, ex6_cost(1.20)), Role::Terminal).unwrap();
    let c = v.star.c.unwrap();
    // 0.25 * 0.60 + 2.25 * 1.20 + 0.75 * 0 (C^fb = 0 for the reference).
    close(c, 0.25 * 0.60 + 2.25 * 1.20);
    close(c, 2.85);
    prints_as(c, "2.8500");
    assert!(v.notes.contains(&CellNote::BorrowedUnitPrice));
    assert!(v.notes.contains(&CellNote::NotWinsorized));
}

#[test]
fn example_7_cell_12_recomputation_and_given_input() {
    // v22 output: with example 6's unit prices C*_R12 recomputes to 1.8273.
    let r = stats(6, 4);
    let v = cell_values(&inputs(counts(6, 4, 0), r, ex6_cost(1.20)), Role::Terminal).unwrap();
    let c = v.star.c.unwrap();
    close(c, (6.5 / 11.0) * 0.60 + (13.5 / 11.0) * 1.20);
    prints_as(c, "1.8273");
    // 4.20 is a given input; with C~_S = 0.60 it implies C~_F ~ 3.13.
    let implied_f = (4.20 - r.p_k * 0.60) / r.e_n_f;
    prints_as(implied_f, "3.13");
    let v = cell_values(
        &inputs(counts(6, 4, 0), r, ex6_cost(implied_f)),
        Role::Terminal,
    )
    .unwrap();
    close(v.star.c.unwrap(), 4.20);
}

// ---- acceptance: miss cells NA in all three dimensions ----------------------

#[test]
fn miss_cells_are_na_in_all_three_dimensions() {
    // n1 + n0 = 0 even though cancels, benchmarks and pools exist.
    let mut i = inputs(
        counts(0, 0, 3),
        stats(0, 0),
        DimInputs {
            bench_success: Some(0.5),
            bench_failure: Some(0.9),
            pool_cancel: Some(0.4),
            ..DimInputs::default()
        },
    );
    i.q_hat = 0.7;
    let v = cell_values(&i, Role::Terminal).unwrap();
    assert_eq!(v.star, Triple::NA);
    assert_eq!(v.robust, Triple::NA);
    for x in [v.star.c, v.star.t, v.star.q] {
        assert_ne!(x, Some(0.0));
        assert!(x.is_none());
    }
    // A candidate on a cell where the reference is miss is NA everywhere.
    let ok = inputs(
        counts(5, 0, 0),
        stats(5, 0),
        DimInputs {
            success: vec![0.5; 5],
            bench_failure: Some(0.9),
            ..DimInputs::default()
        },
    );
    let cand = cell_values(
        &ok,
        Role::Candidate {
            reference: &v,
            fallback: None,
        },
    )
    .unwrap();
    assert_eq!(cand.star, Triple::NA);
    // And the NA cell keeps the layer aggregate NA (never summed as 0).
    let w = CellWeights::equal(&[CellId::user("a").unwrap(), CellId::user("b").unwrap()]).unwrap();
    let agg = aggregate_layer(
        &w,
        &[
            (CellId::user("a").unwrap(), Triple::new(0.6, 10.0, 0.8)),
            (CellId::user("b").unwrap(), cand.star),
        ],
    )
    .unwrap();
    assert_eq!(agg, Triple::NA);
}

#[test]
fn na_is_never_filled_with_zero() {
    // No success samples, no benchmark, no pool: C~_S is NA => C* NA.
    let i = inputs(
        counts(0, 2, 0),
        stats(0, 2),
        DimInputs {
            failure: vec![1.0, 1.2],
            ..DimInputs::default()
        },
    );
    let r = cell_values(&i, Role::Terminal).unwrap();
    assert_eq!(r.star.c, None);
    assert!(r.star.t.is_some() && r.star.q.is_some());
    // A candidate inherits the reference's NA cost only (F = R).
    let ok = inputs(
        counts(5, 0, 0),
        stats(5, 0),
        DimInputs {
            success: vec![0.5; 5],
            bench_failure: Some(0.9),
            ..DimInputs::default()
        },
    );
    let cand = cell_values(
        &ok,
        Role::Candidate {
            reference: &r,
            fallback: None,
        },
    )
    .unwrap();
    assert_eq!(cand.star.c, None);
    assert!(cand.star.t.is_some() && cand.star.q.is_some());
    // nC > 0 without any cancel mean: C* NA.
    let mut c = ok.clone();
    c.counts.n_c = 1;
    c.cost.bench_failure = None;
    c.cost.pool_failure = Some(0.9);
    assert_eq!(cell_values(&c, Role::Terminal).unwrap().star.c, None);
    // Missing retry delay: time not comparable.
    let mut d = ok;
    d.delay = DelayInputs::default();
    assert_eq!(cell_values(&d, Role::Terminal).unwrap().star.t, None);
}

// ---- acceptance: example 6 A_C = 0.7875, V = 102.60 ---------------------------

fn twelve() -> (CellWeights, Vec<CellId>) {
    let cells: Vec<CellId> = (1..=12)
        .map(|i| CellId::user(&format!("c{i:02}")).unwrap())
        .collect();
    (CellWeights::equal(&cells).unwrap(), cells)
}

fn layer_cost(cells: &[CellId], w: &CellWeights, values: &[f64]) -> f64 {
    let rows: Vec<(CellId, Triple)> = cells
        .iter()
        .zip(values)
        .map(|(c, &v)| (c.clone(), Triple::new(v, 10.0, 0.8)))
        .collect();
    aggregate_layer(w, &rows).unwrap().c.unwrap()
}

#[test]
fn example_6_a_c_is_0_7875_and_v_is_102_60() {
    let (w, cells) = twelve();
    let r12 = cell_values(
        &inputs(counts(0, 1, 0), stats(0, 1), ex6_cost(1.20)),
        Role::Terminal,
    )
    .unwrap()
    .star
    .c
    .unwrap();
    let mut vals = vec![0.60; 11];
    vals.push(r12);
    let a_c = layer_cost(&cells, &w, &vals);
    close(a_c, 9.45 / 12.0);
    prints_as(a_c, "0.7875");
    // Same-level self-built arm (q_K = 0, C* = 0.60) against the tier-II
    // anchor 0.66 (given here; the clamp itself is step 10).
    let v = value_score(pt(0.66), pt(0.60), &eq_w(), &Floors::default()).unwrap();
    close(v, 100.0 * 1.08f64.powf(1.0 / 3.0));
    prints_as(v, "102.5986");
    prints_as(v, "102.60");
    // Pure-route arm: its layer cost is A_C.
    let route = value_score(pt(0.66), pt(a_c), &eq_w(), &Floors::default()).unwrap();
    close(route, 100.0 * 0.864f64.powf(1.0 / 3.0));
    prints_as(route, "95.2441");
    // Weights change the exponent (v22 §4 general form).
    prints_as(
        value_score(pt(0.66), pt(a_c), &abg(0.5), &Floors::default()).unwrap(),
        "92.95",
    );
    prints_as(
        value_score(pt(0.66), pt(a_c), &abg(0.2), &Floors::default()).unwrap(),
        "97.12",
    );
}

// ---- acceptance: example 8 A_C = 0.165, V = 74.89 -------------------------------

#[test]
fn example_8_a_c_is_0_165_and_v_is_74_89() {
    let (w, cells) = twelve();
    let mut vals = vec![0.60; 3];
    vals.extend([0.02; 9]);
    let a_c = layer_cost(&cells, &w, &vals);
    close(a_c, 0.165);
    prints_as(a_c, "0.1650");
    // Without the lower clamp the anchor stays 0.165.
    let v = value_score(pt(a_c), pt(0.60), &eq_w(), &Floors::default()).unwrap();
    close(v, 100.0 * 0.42f64.powf(1.0 / 3.0));
    prints_as(v, "74.8887");
    prints_as(v, "74.89");
    // With the clamped anchor 0.54 (step 10), the pure-route arm.
    let route = value_score(pt(0.54), pt(a_c), &eq_w(), &Floors::default()).unwrap();
    prints_as(route, "129.8712");
}

#[test]
fn example_7_wrongly_clamped_comparison() {
    // v22 output: A_C = 0.9; unclamped 111.87 -> clamped to 0.66: 102.60.
    let (w, cells) = twelve();
    let mut vals = vec![0.60; 11];
    vals.push(4.20);
    let a_c = layer_cost(&cells, &w, &vals);
    prints_as(a_c, "0.9000");
    prints_as(
        value_score(pt(a_c), pt(0.60), &eq_w(), &Floors::default()).unwrap(),
        "111.87",
    );
    prints_as(
        value_score(pt(0.66), pt(0.60), &eq_w(), &Floors::default()).unwrap(),
        "102.60",
    );
}

// ---- fallback arm, self-built arm (1 + q_K) ------------------------------------

/// p1 = 0.875 at pi = 0, K = 3: q_K = 0.125, p_K = 0.875, E[n_F] = 0.375.
fn q_eighth() -> RetryStats {
    retry_stats(0.875, &RetryModel::with_default_k(0.0).unwrap()).unwrap()
}

#[test]
fn self_built_arm_pays_q_k_times_the_reference_cost() {
    // v22 output: a = 1/3, C*_R = 0.3 / 0.6 / 1.0 / 2.0 / 10.0 =>
    // 97.37 / 96.87 / 96.62 / 96.40 / 96.20; c0 -> 0 limit 96.15.
    let r = q_eighth();
    for (c_r, printed) in [
        (0.3, "97.37"),
        (0.6, "96.87"),
        (1.0, "96.62"),
        (2.0, "96.40"),
        (10.0, "96.20"),
    ] {
        // Same unit price x on both sides: C*_R = x (p_K + E[n_F]) = 1.25 x.
        let x = c_r / 1.25;
        let dim = DimInputs {
            bench_success: Some(x),
            bench_failure: Some(x),
            ..DimInputs::default()
        };
        let i = inputs(counts(7, 1, 0), r, dim);
        let reference = cell_values(&i, Role::Terminal).unwrap();
        close(reference.star.c.unwrap(), c_r);
        let cand = cell_values(
            &i,
            Role::Candidate {
                reference: &reference,
                fallback: None,
            },
        )
        .unwrap();
        let c_i = cand.star.c.unwrap();
        close(c_i, 1.125 * c_r);
        prints_as(
            value_score(pt(c_r), pt(c_i), &eq_w(), &Floors::default()).unwrap(),
            printed,
        );
    }
    let tiny = Floors::new(1e-12, 15.0, 0.02).unwrap();
    prints_as(
        value_score(pt(1.0), pt(1.125), &eq_w(), &tiny).unwrap(),
        "96.15",
    );
}

#[test]
fn fallback_arm_values_replace_the_reference_in_the_fallback_term() {
    let r = q_eighth();
    let dim = DimInputs {
        bench_success: Some(0.48),
        bench_failure: Some(0.48),
        ..DimInputs::default()
    };
    let i = inputs(counts(7, 1, 0), r, dim);
    let reference = cell_values(&i, Role::Terminal).unwrap(); // C* = 0.6
    let f = CellValues {
        star: Triple::new(1.2, 10.0, 0.9),
        robust: Triple::new(1.2, 10.0, 0.9),
        robust_dominance: true,
        notes: vec![],
    };
    let cand = cell_values(
        &i,
        Role::Candidate {
            reference: &reference,
            fallback: Some(&f),
        },
    )
    .unwrap();
    close(cand.star.c.unwrap(), 0.6 + 0.125 * 1.2);
    // Quality: f = 0 => Qbar_F = Q*_F.
    close(cand.star.q.unwrap(), 0.875 * 0.99 + 0.125 * 0.9);
    // Any NA dimension of F (F != R) makes the whole candidate cell NA.
    let f_na = CellValues {
        star: Triple { t: None, ..f.star },
        ..f.clone()
    };
    let cand = cell_values(
        &i,
        Role::Candidate {
            reference: &reference,
            fallback: Some(&f_na),
        },
    )
    .unwrap();
    assert_eq!(cand.star, Triple::NA);
    // The reference (terminal) has no fallback term: Q*_R = p_K q_hat.
    close(reference.star.q.unwrap(), 0.875 * 0.99);
}

// ---- unit prices, winsorizing, cancels, time, quality ---------------------------

#[test]
fn winsorizing_and_shrinkage() {
    assert_eq!([9, 10, 199, 200, 350].map(winsor_k), [0, 1, 1, 2, 3]);
    let mut v: Vec<f64> = (0..9).map(f64::from).collect(); // 0..=8
    v.push(100.0);
    let m = side_mean(&v).unwrap().unwrap();
    assert!(m.winsorized);
    close(m.value, 4.5); // 0 -> 1 and 100 -> 8
    let raw = side_mean(&v[..9]).unwrap().unwrap();
    assert!(!raw.winsorized);
    close(raw.value, 4.0);
    // (2 * 1.0 + 5 * 0.5) / 7
    let p = unit_price(&[0.9, 1.1], Some(0.5), None).unwrap().unwrap();
    assert_eq!(p.source, PriceSource::Shrunk);
    close(p.value, 4.5 / 7.0);
    assert_eq!(
        unit_price(&[], Some(0.5), Some(0.7))
            .unwrap()
            .unwrap()
            .source,
        PriceSource::Benchmark
    );
    assert_eq!(
        unit_price(&[], None, Some(0.7)).unwrap().unwrap().source,
        PriceSource::LayerPool
    );
    assert_eq!(unit_price(&[], None, None).unwrap(), None);
    assert!(unit_price(&[-1.0], None, None).is_err());
    assert!(side_mean(&[f64::NAN]).is_err());
}

fn cost_with(cancel: CancelBills, pool_cancel: Option<f64>, bench_f: Option<f64>) -> DimInputs {
    DimInputs {
        success: vec![0.5; 3],
        failure: vec![0.3],
        bench_failure: bench_f,
        cancel,
        pool_cancel,
        ..DimInputs::default()
    }
}

#[test]
fn cancel_amortization_and_its_degrade_chain() {
    let r = stats(3, 1);
    let base = |cancel, pool, bench| {
        cell_values(
            &inputs(counts(3, 1, 2), r, cost_with(cancel, pool, bench)),
            Role::Terminal,
        )
        .unwrap()
    };
    let own = r.p_k * 0.5 + r.e_n_f * 0.3;
    // Per-task bills: mean 0.4, amortized 2 * 0.4 / 4.
    let v = base(CancelBills::PerTask(vec![0.3, 0.5]), None, None);
    close(v.star.c.unwrap(), own + 0.2);
    // No bill: max(C~_F, pool) then max(C~_F, C_F^B), C~_F = 0.3 here
    // (benchmark shrinks it: (0.3 + 5 * 0.9) / 6 = 0.8 when present).
    let v = base(CancelBills::None, Some(0.9), None);
    close(v.star.c.unwrap(), own + 2.0 * 0.9 / 4.0);
    let v = base(CancelBills::None, None, Some(0.9));
    let f = (0.3 + 5.0 * 0.9) / 6.0;
    close(
        v.star.c.unwrap(),
        r.p_k * 0.5 + r.e_n_f * f + 2.0 * f.max(0.9) / 4.0,
    );
    assert_eq!(base(CancelBills::None, None, None).star.c, None);
    // Wrong number of per-task bills is an input error.
    assert!(cell_values(
        &inputs(
            counts(3, 1, 2),
            r,
            cost_with(CancelBills::PerTask(vec![0.3]), None, None)
        ),
        Role::Terminal
    )
    .is_err());
}

#[test]
fn cancel_robust_excess_and_guards() {
    let r = stats(3, 1);
    let rc = stats(3, 3); // n0 + nC failures
    let mk = |cancel| {
        let mut i = inputs(counts(3, 1, 2), r, cost_with(cancel, None, None));
        i.retry_c = rc;
        // Time has per-task cancel durations, so only cost decides the flag.
        i.time.cancel = CancelBills::PerTask(vec![10.0, 10.0]);
        cell_values(&i, Role::Terminal).unwrap()
    };
    let own_c = rc.p_k * 0.5 + rc.e_n_f * 0.3;
    // Per task: sum max(0, c - C~_F) = 0.2 + 0 with C~_F = 0.3.
    let v = mk(CancelBills::PerTask(vec![0.5, 0.1]));
    close(
        v.robust.c.unwrap(),
        (own_c + 0.2 / 4.0).max(v.star.c.unwrap()),
    );
    assert!(v.robust_dominance);
    assert!(v.robust.c.unwrap() >= v.star.c.unwrap());
    // Aggregate with cap: S = 1.0, U = 0.4 => 2 * (0.4 - 0.3) + max(0, 0.2 - 0.3) = 0.2.
    let v = mk(CancelBills::Aggregate {
        total: 1.0,
        cap: Some(0.4),
    });
    close(
        v.robust.c.unwrap(),
        (own_c + 0.2 / 4.0).max(v.star.c.unwrap()),
    );
    assert!(v.robust_dominance);
    // Aggregate without cap: lower bound 2 * max(0, 0.5 - 0.3); robust dominance off.
    let v = mk(CancelBills::Aggregate {
        total: 1.0,
        cap: None,
    });
    close(
        v.robust.c.unwrap(),
        (own_c + 0.4 / 4.0).max(v.star.c.unwrap()),
    );
    assert!(!v.robust_dominance);
    assert!(v.notes.contains(&CellNote::CancelLowerBoundOnly));
    // Robust quality never exceeds Q*.
    assert!(v.robust.q.unwrap() <= v.star.q.unwrap());
}

#[test]
fn time_includes_the_retry_wait_term_and_delay_chain() {
    let r = stats(3, 1);
    let mut i = inputs(counts(3, 1, 0), r, cost_with(CancelBills::None, None, None));
    i.time = DimInputs {
        success: vec![60.0; 3],
        failure: vec![90.0],
        ..DimInputs::default()
    };
    for (delay, d) in [
        (
            DelayInputs {
                own: Some(5.0),
                layer_pool: Some(7.0),
                benchmark: Some(9.0),
            },
            5.0,
        ),
        (
            DelayInputs {
                own: None,
                layer_pool: Some(7.0),
                benchmark: Some(9.0),
            },
            7.0,
        ),
        (
            DelayInputs {
                own: None,
                layer_pool: None,
                benchmark: Some(9.0),
            },
            9.0,
        ),
    ] {
        i.delay = delay;
        let t = cell_values(&i, Role::Terminal).unwrap().star.t.unwrap();
        close(t, r.p_k * 60.0 + r.e_n_f * 90.0 + r.e_n_w * d);
    }
}

#[test]
fn matched_failure_quality_shrinks_to_the_fallback_quality() {
    let r = q_eighth();
    let mut i = inputs(
        counts(7, 1, 0),
        r,
        DimInputs {
            bench_success: Some(0.5),
            bench_failure: Some(0.5),
            ..DimInputs::default()
        },
    );
    i.q_hat = 0.7;
    let reference = CellValues {
        star: Triple::new(0.6, 10.0, 0.8),
        robust: Triple::new(0.6, 10.0, 0.8),
        robust_dominance: true,
        notes: vec![],
    };
    i.matched_failures = 5;
    i.q_match = Some(0.4);
    let cand = cell_values(
        &i,
        Role::Candidate {
            reference: &reference,
            fallback: None,
        },
    )
    .unwrap();
    // Qbar_F = (5 * 0.4 + 5 * 0.8) / 10 = 0.6
    close(cand.star.q.unwrap(), 0.875 * 0.7 + 0.125 * 0.6);
    // f > 0 without Q^match: quality NA.
    i.q_match = None;
    let cand = cell_values(
        &i,
        Role::Candidate {
            reference: &reference,
            fallback: None,
        },
    )
    .unwrap();
    assert_eq!(cand.star.q, None);
    i.q_hat = 1.5;
    assert!(cell_values(&i, Role::Terminal).is_err());
}

// ---- aggregation and V ------------------------------------------------------------

#[test]
fn aggregation_uses_positive_weight_cells_only() {
    let (a, b, z) = (
        CellId::user("a").unwrap(),
        CellId::user("b").unwrap(),
        CellId::user("z").unwrap(),
    );
    let w = CellWeights::new(vec![(a.clone(), 0.25), (b.clone(), 0.75), (z.clone(), 0.0)]).unwrap();
    let rows = vec![
        (a.clone(), Triple::new(1.0, 100.0, 0.6)),
        (b.clone(), Triple::new(2.0, 200.0, 0.8)),
        (z.clone(), Triple::NA), // zero weight: ignored
    ];
    let agg = aggregate_layer(&w, &rows).unwrap();
    close(agg.c.unwrap(), 1.75);
    close(agg.t.unwrap(), 175.0);
    close(agg.q.unwrap(), 0.75);
    // A d+ cell without values is NA.
    assert_eq!(aggregate_layer(&w, &rows[..1]).unwrap(), Triple::NA);
    let dup = vec![rows[0].clone(), rows[0].clone()];
    assert!(aggregate_layer(&w, &dup).is_err());
    // Overall over layers.
    let all = aggregate_overall(&[
        (0.5, agg),
        (0.5, Triple::new(1.25, 75.0, 0.95)),
        (0.0, Triple::NA),
    ])
    .unwrap();
    close(all.c.unwrap(), 1.5);
    close(all.q.unwrap(), 0.85);
    assert!(aggregate_overall(&[(0.5, agg)]).is_err());
}

#[test]
fn value_score_properties_and_input_checks() {
    let f = Floors::default();
    let p = Point {
        c: 0.7,
        t: 120.0,
        q: 0.9,
    };
    assert_eq!(value_score(p, p, &eq_w(), &f).unwrap(), 100.0);
    // Better on every dimension => V > 100; worse => V < 100.
    let better = Point {
        c: 0.5,
        t: 90.0,
        q: 0.95,
    };
    assert!(value_score(p, better, &eq_w(), &f).unwrap() > 100.0);
    assert!(value_score(better, p, &eq_w(), &f).unwrap() < 100.0);
    // Full three-dimensional closed form.
    let w = Weights::new(0.5, 0.3, 0.2).unwrap();
    let want = 100.0
        * ((0.7f64 + 0.15) / (0.5 + 0.15)).powf(0.5)
        * ((120.0f64 + 15.0) / (90.0 + 15.0)).powf(0.3)
        * ((0.1f64 + 0.02) / (0.05 + 0.02)).powf(0.2);
    close(value_score(p, better, &w, &f).unwrap(), want);
    assert!(value_score(p, Point { q: 1.1, ..p }, &eq_w(), &f).is_err());
    assert!(value_score(p, Point { c: -0.1, ..p }, &eq_w(), &f).is_err());
    assert!(value_score(Point { t: f64::NAN, ..p }, p, &eq_w(), &f).is_err());
    assert!(Floors::new(0.0, 15.0, 0.02).is_err());
    assert_eq!(
        Point::from_triple(&Triple {
            q: None,
            ..Triple::new(1.0, 1.0, 0.5)
        }),
        None
    );
}

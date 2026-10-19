//! Steps 4-5 (v22 §0, §1): cell states, omega_u, prior, Beta posterior,
//! K-truncated retries and lower bounds.
//!
//! Sources of expected values:
//! - v22 recomputation output (`v22_examples_output.txt`): examples 6 and 7.
//!   Closed forms are checked to 1e-9 and the printed digits by rounding.
//! - 40-digit mpmath references (listed in the PR body) for Beta CDF /
//!   quantile values, kappa at rho < 1 and E[n_F] at tiny pi.
//! - The formula reviewer's cross-check (`pr45_check.py`, review notes): pi
//!   edge cases, the kappa jump, 7/14/21 minimum samples.

use agent_value_core::beta_dist::{beta_cdf, beta_quantile};
use agent_value_core::cells::{CellId, CellWeights};
use agent_value_core::retry::{
    lower_bound_fails, p_k_lower, retry_stats, BetaPosterior, PassStats, Prior, RetryModel,
};
use agent_value_core::status::{
    assess_cell, derive_cell_weights, layer_status, CellCounts, CellInput, CellStatus, OmegaSource,
};

const TOL: f64 = 1e-9;

fn close(a: f64, b: f64, tol: f64) {
    assert!((a - b).abs() <= tol, "{a} vs {b} (tol {tol})");
}

/// The value rounds to the digits printed in the v22 output.
fn prints_as(x: f64, printed: &str) {
    let d = printed.split('.').nth(1).map_or(0, str::len);
    assert_eq!(format!("{x:.d$}"), printed, "{x} should print as {printed}");
}

fn k3(pi: f64) -> RetryModel {
    RetryModel::with_default_k(pi).unwrap()
}

fn counts(n1: u64, n0: u64) -> CellCounts {
    CellCounts {
        n1,
        n0,
        ..CellCounts::default()
    }
}

fn cell(s: &str) -> CellId {
    CellId::user(s).unwrap()
}

// ---- example 6 / 7 (v22 output) ------------------------------------------

#[test]
fn example_6_cell_12_values_and_thin_state() {
    // Jeffreys (kappa = 0), K = 3, pi = 0, n1 = 0, n0 = 1.
    let post = BetaPosterior::new(&Prior::Jeffreys, 0, 1).unwrap();
    close(post.alpha, 0.5, TOL);
    close(post.beta, 1.5, TOL);
    let r = retry_stats(post.p1(), &k3(0.0)).unwrap();
    close(r.p1, 0.25, TOL);
    close(r.p_k, 0.25, TOL);
    close(r.e_n_f, 2.25, TOL);
    close(r.q_k, 0.75, TOL);
    for (v, p) in [
        (r.p1, "0.2500"),
        (r.p_k, "0.2500"),
        (r.e_n_f, "2.2500"),
        (r.q_k, "0.7500"),
    ] {
        prints_as(v, p);
    }
    // n1 + n0 = 1 < 3 => F_thin, decided before any lower bound.
    let a = assess_cell(&counts(0, 1), &Prior::Jeffreys, &k3(0.0)).unwrap();
    assert_eq!(a.status, CellStatus::FailThin);
}

#[test]
fn example_7_cell_12_values_and_bad_state() {
    let post = BetaPosterior::new(&Prior::Jeffreys, 6, 4).unwrap();
    close(post.alpha, 6.5, TOL);
    close(post.beta, 4.5, TOL);
    close(post.p1(), 6.5 / 11.0, TOL);
    prints_as(post.p1(), "0.5909");
    let q5 = post.p1_lower().unwrap();
    close(q5, 0.347_322_391_878_224_4, TOL); // mpmath
    prints_as(q5, "0.3473");
    let lo = p_k_lower(&post, &k3(0.0)).unwrap();
    close(lo, q5, TOL); // pi = 0 => p_K^lo = p1^{5%}
    prints_as(lo, "0.3473");
    assert!(lower_bound_fails(&post, &k3(0.0)).unwrap());
    let r = retry_stats(post.p1(), &k3(0.0)).unwrap();
    close(r.e_n_f, 13.5 / 11.0, TOL);
    close(r.q_k, 4.5 / 11.0, TOL);
    prints_as(r.e_n_f, "1.2273");
    prints_as(r.q_k, "0.4091");
    let a = assess_cell(&counts(6, 4), &Prior::Jeffreys, &k3(0.0)).unwrap();
    assert_eq!(a.status, CellStatus::FailBad);
}

#[test]
fn example_7_control_beta_6_4() {
    // v22 output: Beta(6,4) 5% quantile 0.3449, mean 0.6000.
    let q = beta_quantile(0.05, 6.0, 4.0).unwrap();
    close(q, 0.344_941_365_943_703_1, TOL);
    prints_as(q, "0.3449");
}

#[test]
fn example_6_layer_counts() {
    let cells: Vec<CellId> = (1..=12).map(|i| cell(&format!("c{i:02}"))).collect();
    let w = CellWeights::equal(&cells).unwrap();
    let mut inputs: Vec<CellInput> = cells[..11]
        .iter()
        .map(|c| CellInput {
            cell: c.clone(),
            counts: counts(10, 0),
            prior: Prior::Jeffreys,
        })
        .collect();
    inputs.push(CellInput {
        cell: cells[11].clone(),
        counts: counts(0, 1),
        prior: Prior::Jeffreys,
    });
    let s = layer_status(&w, &inputs, &k3(0.0)).unwrap();
    assert_eq!(
        (s.n_miss, s.n_fail, s.n_thin, s.n_bad, s.n_valid),
        (0, 1, 1, 0, 11)
    );
    close(s.w_thin, 1.0 / 12.0, TOL);
    close(s.w_fail, 1.0 / 12.0, TOL);
    assert_eq!(s.w_bad, 0.0);
    assert_eq!(s.n_comp, 111);
    assert_eq!(s.status(&cells[11]), Some(CellStatus::FailThin));
}

// ---- decision A: geometric sum, p_K = 1 - q_K ----------------------------

#[test]
fn pi_zero_limit() {
    let r = retry_stats(0.3, &k3(0.0)).unwrap();
    close(r.p_k, 0.3, 1e-15);
    close(r.q_k, 0.7, 1e-15);
    close(r.e_n_f, 3.0 * 0.7, 1e-15);
    close(r.e_n_w, 2.0 * 0.7, 1e-15);
    assert_eq!(r.p_k + r.q_k, 1.0);
}

#[test]
fn tiny_pi_has_no_cancellation_error() {
    // mpmath references for p1 = 0.3, K = 3.
    for (pi, want) in [
        (1e-6, 2.099_997_900_000_7),
        (1e-10, 2.099_999_999_79),
        (1e-13, 2.099_999_999_999_79),
        // Not discriminating: closed form and sum differ by ~2.1e-15 < 1e-14 here; 1e-13 is the case that does.
        (1e-15, 2.099_999_999_999_998),
    ] {
        let r = retry_stats(0.3, &k3(pi)).unwrap();
        close(r.e_n_f, want, 1e-14);
        // The spec's closed form (1-p1)(1-(1-pi)^K)/pi is visibly off at 1e-13.
        if pi == 1e-13 {
            let naive = 0.7 * (1.0 - (1.0 - pi).powi(3)) / pi;
            assert!((naive - want).abs() > 1e-5, "naive {naive}");
        }
    }
}

#[test]
fn p_k_and_q_k_are_complements_and_pi_one_is_safe() {
    for pi in [0.0, 1e-13, 0.2, 0.5, 1.0] {
        for p1 in [0.0, 0.25, 0.5909, 1.0] {
            let r = retry_stats(p1, &k3(pi)).unwrap();
            assert_eq!(r.p_k, 1.0 - r.q_k);
            close(r.e_n_w, r.e_n_f - r.q_k, 1e-15);
        }
    }
    // pi = 1: the second attempt always succeeds.
    let r = retry_stats(0.25, &k3(1.0)).unwrap();
    assert_eq!((r.q_k, r.p_k), (0.0, 1.0));
    close(r.e_n_f, 0.75, 1e-15);
    // K = 1: (1-pi)^0 = 1 even for pi = 1.
    let r = retry_stats(0.25, &RetryModel::new(1.0, 1).unwrap()).unwrap();
    close(r.q_k, 0.75, 1e-15);
    assert!(RetryModel::new(0.1, 0).is_err());
    assert!(RetryModel::new(1.1, 3).is_err());
    assert!(RetryModel::new(f64::NAN, 3).is_err());
    assert!(retry_stats(1.2, &k3(0.0)).is_err());
}

// ---- decision B: fail via CDF, quantile for display ----------------------

#[test]
fn beta_cdf_matches_high_precision_references() {
    for (x, a, b, want) in [
        (0.3, 6.5, 4.5, 0.023_439_276_839_533_336),
        (0.5, 0.5, 1.5, 0.818_309_886_183_790_7),
        (0.9, 50.5, 3.5, 0.143_014_309_757_897_5),
        (0.1, 2.5, 30.5, 0.746_385_409_152_252_6),
        (0.75, 7.5, 0.5, 0.040_968_955_955_836_18),
        (0.999, 200.5, 1.5, 0.939_891_024_317_093_7),
        (0.6, 1000.5, 700.5, 0.838_957_665_578_696_9),
    ] {
        close(beta_cdf(x, a, b).unwrap(), want, 1e-12);
    }
}

#[test]
fn beta_quantiles_match_high_precision_references() {
    for (a, b, want) in [
        (0.5, 1.5, 0.001_542_919_302_672_943),
        (6.5, 4.5, 0.347_322_391_878_224_4),
        (7.5, 0.5, 0.767_535_218_387_144_3),
        (6.5, 0.5, 0.735_827_145_677_815_5),
        (13.5, 1.5, 0.752_486_257_334_745_5),
        (21.5, 2.5, 0.779_533_689_468_618_6),
    ] {
        close(beta_quantile(0.05, a, b).unwrap(), want, 1e-12);
    }
}

#[test]
fn cdf_test_agrees_with_quantile_test_on_a_fixed_grid() {
    let mut checked = 0;
    for ai in 0..24 {
        for bi in 0..24 {
            let (a, b) = (0.5 + 2.5 * ai as f64, 0.5 + 2.5 * bi as f64);
            for pi in [0.0, 0.05, 0.3] {
                let post = BetaPosterior { alpha: a, beta: b };
                let m = k3(pi);
                let lo = p_k_lower(&post, &m).unwrap();
                if (lo - 0.75).abs() < 1e-10 {
                    continue;
                }
                assert_eq!(
                    lower_bound_fails(&post, &m).unwrap(),
                    lo < 0.75,
                    "a={a} b={b} pi={pi}"
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 1500);
}

#[test]
fn x_star_non_positive_always_passes() {
    // pi = 0.5, K = 3: (1-pi)^2 = 0.25 => x* = 0; pi = 0.6 => x* < 0.
    for pi in [0.5, 0.6, 1.0] {
        let post = BetaPosterior::new(&Prior::Jeffreys, 0, 5).unwrap();
        assert!(!lower_bound_fails(&post, &k3(pi)).unwrap());
        assert!(p_k_lower(&post, &k3(pi)).unwrap() >= 0.75);
        // So five failures with a high retry rate is a valid cell.
        let a = assess_cell(&counts(0, 5), &Prior::Jeffreys, &k3(pi)).unwrap();
        assert_eq!(a.status, CellStatus::Valid);
    }
}

#[test]
fn minimum_samples_for_valid_under_jeffreys() {
    // Reviewer note: 0 failures need 7 successes, 1 failure 14 tasks,
    // 2 failures 21 tasks (pi = 0, K = 3).
    for (n0, n1_min) in [(0, 7), (1, 13), (2, 19)] {
        let m = k3(0.0);
        assert_eq!(
            assess_cell(&counts(n1_min - 1, n0), &Prior::Jeffreys, &m)
                .unwrap()
                .status,
            CellStatus::FailBad
        );
        assert_eq!(
            assess_cell(&counts(n1_min, n0), &Prior::Jeffreys, &m)
                .unwrap()
                .status,
            CellStatus::Valid
        );
    }
}

#[test]
fn cancel_robust_bound_alone_can_fail_a_cell() {
    let c = CellCounts {
        n1: 7,
        n0: 0,
        n_c: 2,
        m: 0,
    };
    let p = PassStats::new(&Prior::Jeffreys, c.n1, c.n0, c.n_c, &k3(0.0)).unwrap();
    assert!(!p.lower_fails);
    assert!(p.lower_fails_c);
    close(p.posterior_c.beta, 2.5, TOL);
    assert_eq!(
        assess_cell(&c, &Prior::Jeffreys, &k3(0.0)).unwrap().status,
        CellStatus::FailBad
    );
}

// ---- decision C: kappa as written ------------------------------------------

#[test]
fn kappa_cases_and_the_jump_at_rho_one() {
    let direct = Prior::DirectPool {
        p_bench: 0.8,
        n_b: 50.0,
    };
    close(direct.kappa().unwrap(), 50.0, TOL);
    for (rho, want) in [
        (0.9, 13.235_294_117_647_06),
        (0.99, 14.183_381_088_825_215),
        (0.999_999, 14.285_704_081_629_738),
    ] {
        let p = Prior::Discounted {
            p_bench: 0.8,
            n_b: 50.0,
            rho,
        };
        close(p.kappa().unwrap(), want, TOL);
    }
    // The jump is kept: just below 1 vs direct pooling.
    let below = Prior::Discounted {
        p_bench: 0.8,
        n_b: 50.0,
        rho: 0.999_999,
    };
    assert!(direct.kappa().unwrap() - below.kappa().unwrap() > 35.0);
    // rho = 1 is not accepted by the discounted branch.
    let one = Prior::Discounted {
        p_bench: 0.8,
        n_b: 50.0,
        rho: 1.0,
    };
    assert!(one.kappa().is_err());
}

#[test]
fn kappa_zero_over_zero_and_other_cases() {
    let p = Prior::Discounted {
        p_bench: 0.5,
        n_b: 0.0,
        rho: 0.0,
    };
    assert_eq!(p.kappa(), Ok(0.0));
    let post = BetaPosterior::new(&p, 1, 1).unwrap();
    assert!(post.p1().is_finite());
    assert_eq!(
        Prior::Discounted {
            p_bench: 0.5,
            n_b: 10.0,
            rho: 0.0
        }
        .kappa(),
        Ok(0.0)
    );
    assert_eq!(Prior::LayerMean { p0: 0.7, rho: 1.0 }.kappa(), Ok(3.0));
    close(
        Prior::LayerMean { p0: 0.7, rho: 0.1 }.kappa().unwrap(),
        2.0,
        TOL,
    );
    assert_eq!(Prior::Jeffreys.kappa(), Ok(0.0));
    assert_eq!(Prior::Jeffreys.p_b(), Ok(0.5));
    for bad in [
        Prior::Discounted {
            p_bench: 0.5,
            n_b: 10.0,
            rho: -0.1,
        },
        Prior::DirectPool {
            p_bench: 0.5,
            n_b: -1.0,
        },
        Prior::LayerMean { p0: 0.5, rho: 1.5 },
    ] {
        assert!(bad.kappa().is_err());
    }
    assert!(Prior::DirectPool {
        p_bench: 1.5,
        n_b: 1.0
    }
    .p_b()
    .is_err());
}

#[test]
fn prior_enters_alpha_and_beta() {
    // kappa = 50, p_B = 0.8 => alpha = 40 + n1 + 0.5, beta = 10 + n0 + 0.5.
    let post = BetaPosterior::new(
        &Prior::DirectPool {
            p_bench: 0.8,
            n_b: 50.0,
        },
        3,
        2,
    )
    .unwrap();
    close(post.alpha, 43.5, TOL);
    close(post.beta, 12.5, TOL);
}

// ---- step 4: omega_u and d+ --------------------------------------------------

fn five() -> Vec<CellId> {
    ["fix", "add", "refactor", "explain-review", "other"]
        .iter()
        .map(|s| cell(s))
        .collect()
}

#[test]
fn omega_from_previous_window_excludes_probes() {
    let prev = vec![(cell("fix"), 6), (cell("add"), 3), (CellId::Probe, 100)];
    let (w, src) = derive_cell_weights(&five(), &prev, None).unwrap();
    assert_eq!(src, OmegaSource::PreviousWindow);
    close(w.omega(&cell("fix")), 2.0 / 3.0, TOL);
    close(w.omega(&cell("add")), 1.0 / 3.0, TOL);
    assert_eq!(w.omega(&cell("refactor")), 0.0);
    assert_eq!(w.omega(&CellId::Probe), 0.0);
    assert_eq!(w.support().count(), 2);
}

#[test]
fn omega_without_data_never_divides_by_zero() {
    // Only probe tasks / no tasks => no user tasks in t-1.
    let (w, src) = derive_cell_weights(&five(), &[(CellId::Probe, 9)], None).unwrap();
    assert_eq!(src, OmegaSource::Equal);
    for c in five() {
        close(w.omega(&c), 0.2, TOL);
    }
    let (w2, src2) = derive_cell_weights(&five(), &[(cell("fix"), 0)], Some(&w)).unwrap();
    assert_eq!(src2, OmegaSource::CarriedOver);
    assert_eq!(w2, w);
    assert!(w.entries().iter().all(|(_, x)| x.is_finite()));
}

#[test]
fn omega_input_errors() {
    assert!(derive_cell_weights(&five(), &[(cell("unknown"), 1)], None).is_err());
    assert!(derive_cell_weights(&five(), &[(cell("fix"), 1), (cell("fix"), 2)], None).is_err());
    assert!(derive_cell_weights(&[], &[], None).is_err());
    assert!(derive_cell_weights(&[CellId::Probe], &[], None).is_err());
}

#[test]
fn only_positive_weight_cells_are_classified() {
    let w = CellWeights::new(vec![
        (cell("fix"), 0.75),
        (cell("add"), 0.25),
        (cell("refactor"), 0.0),
    ])
    .unwrap();
    let inputs = vec![
        CellInput {
            cell: cell("fix"),
            counts: counts(10, 0),
            prior: Prior::Jeffreys,
        },
        // Zero-weight cell: its tasks are not counted anywhere.
        CellInput {
            cell: cell("refactor"),
            counts: counts(0, 9),
            prior: Prior::Jeffreys,
        },
    ];
    let s = layer_status(&w, &inputs, &k3(0.0)).unwrap();
    assert_eq!(s.cells.len(), 2);
    assert_eq!(s.status(&cell("refactor")), None);
    // "add" has weight but no row => miss.
    assert_eq!(s.status(&cell("add")), Some(CellStatus::Miss));
    assert_eq!((s.n_miss, s.n_valid, s.n_comp), (1, 1, 10));
    close(s.w_miss, 0.25, TOL);
    let dup = vec![inputs[0].clone(), inputs[0].clone()];
    assert!(layer_status(&w, &dup, &k3(0.0)).is_err());
}

#[test]
fn miss_has_no_pass_statistics() {
    let a = assess_cell(
        &CellCounts {
            n1: 0,
            n0: 0,
            n_c: 4,
            m: 2,
        },
        &Prior::Jeffreys,
        &k3(0.0),
    )
    .unwrap();
    assert_eq!(a.status, CellStatus::Miss);
    assert!(a.pass.is_none());
    let overflow = CellCounts {
        n1: u64::MAX,
        n0: 1,
        n_c: 0,
        m: 0,
    };
    assert!(assess_cell(&overflow, &Prior::Jeffreys, &k3(0.0)).is_err());
}

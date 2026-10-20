//! Step 12 (v22 §12; DECISIONS K and the step-12 section): winner map,
//! weight-robust winner, statistical robustness (task bootstrap), flip
//! line and noise-floor robustness.
//! Expected values come from the spec text, `scripts/v22_examples.py`
//! (output `review-2026-10-19/v22_examples_output.txt` lines 36–38) and
//! `scripts/elicit5.py`; never fitted.

use agent_value_core::cell_values::Triple;
use agent_value_core::cells::CellId;
use agent_value_core::constants::{BOOTSTRAP_RESAMPLES, GRID_POINTS};
use agent_value_core::error::CoreError;
use agent_value_core::frontier::{layer_frontiers, FrontierArm};
use agent_value_core::sensitivity::{
    bootstrap_strata, flip_line, flip_sentence, grid_points, printed_ratio, sensitivity,
    statistical_robustness, winners, BehaviourRegion, BootstrapTask, Dim, FlipKind, FlipLine,
    FlipStatement, FloorVariant, NoFlipLine, ResampleLayer, RobustRegion, SensitivityArm,
    SensitivityInput, SensitivityReport, Withheld, Xorshift64, ALWAYS_WITHIN_TIE_NOTE,
    CURRENT_ESTIMATE_NOTE, WEIGHT_ROBUST_EVERYWHERE,
};
use agent_value_core::value::{value_score, Floors, Point};
use agent_value_core::weights::Weights;

const TOL: f64 = 1e-9;

fn close(a: f64, b: f64) {
    assert!((a - b).abs() <= TOL, "{a} vs {b}");
}

fn arm(id: &str, c: f64, t: f64, q: f64) -> SensitivityArm {
    SensitivityArm {
        id: id.into(),
        point: Point { c, t, q },
    }
}

fn w(a: f64, b: f64, g: f64) -> Weights {
    Weights::new(a, b, g).unwrap()
}

/// v22_examples.py "敏感度网格": one layer, two arms, reference (0.60, 90, 0.80).
fn example_arms() -> Vec<SensitivityArm> {
    vec![arm("X", 0.30, 120.0, 0.80), arm("Y", 0.90, 60.0, 0.85)]
}

fn example_reference() -> Point {
    Point {
        c: 0.60,
        t: 90.0,
        q: 0.80,
    }
}

/// `elicit5.run((0.20, 0.35, 0.45))`: 4 questions, `Ω_s` of 14 points
/// (logs/pr12/fixtures.log; v22_examples_output.txt line 36).
fn example_omega_s() -> Vec<Weights> {
    [
        (1, 5, 14),
        (1, 6, 13),
        (2, 6, 12),
        (2, 9, 9),
        (3, 7, 10),
        (3, 8, 9),
        (3, 9, 8),
        (4, 6, 10),
        (4, 7, 9),
        (4, 8, 8),
        (5, 5, 10),
        (5, 6, 9),
        (5, 7, 8),
        (6, 5, 9),
    ]
    .iter()
    .map(|&(i, j, k)| {
        w(
            f64::from(i) / 20.0,
            f64::from(j) / 20.0,
            f64::from(k) / 20.0,
        )
    })
    .collect()
}

fn input<'a>(
    arms: &'a [SensitivityArm],
    current: Weights,
    omega_s: Option<&'a [Weights]>,
) -> SensitivityInput<'a> {
    SensitivityInput {
        arms,
        reference: Some(example_reference()),
        first_window: false,
        current,
        omega_s,
        omega_b: None,
        floors: Floors::default(),
    }
}

fn report(i: &SensitivityInput<'_>) -> SensitivityReport {
    sensitivity(i).unwrap().unwrap()
}

fn count_sole(r: &SensitivityReport, id: &str) -> usize {
    r.map
        .iter()
        .filter(|p| p.winners.len() == 1 && p.winners[0] == id)
        .count()
}

#[test]
fn grid_has_171_points_on_twentieths_each_at_least_0_05() {
    let g = grid_points().unwrap();
    assert_eq!(g.len(), GRID_POINTS);
    assert_eq!(g.len(), 171);
    let mut keys = std::collections::BTreeSet::new();
    for p in &g {
        let k: Vec<i64> = [p.a(), p.b(), p.g()]
            .iter()
            .map(|v| (v * 20.0).round() as i64)
            .collect();
        assert!(k.iter().all(|&x| x >= 1));
        assert_eq!(k.iter().sum::<i64>(), 20);
        // Bit-exact: built as integer twentieths, not by adding 0.05
        // (DECISIONS 12.2; 10 of the 18 accumulated values differ by 1 ulp).
        for (v, kk) in [p.a(), p.b(), p.g()].iter().zip(&k) {
            assert_eq!(v.to_bits(), (*kk as f64 / 20.0).to_bits(), "{v} vs {kk}/20");
        }
        assert!(keys.insert(k));
    }
}

#[test]
fn spec_example_y_wins_93_points_x_wins_78() {
    let arms = example_arms();
    let r = report(&input(&arms, Weights::equal(), None));
    assert_eq!(r.map.len(), 171);
    assert_eq!(count_sole(&r, "Y"), 93);
    assert_eq!(count_sole(&r, "X"), 78);
    assert!(r.map.iter().all(|p| p.winners.len() == 1), "no ties here");
}

#[test]
fn spec_example_y_wins_14_of_14_in_self_report_region() {
    let arms = example_arms();
    let omega = example_omega_s();
    let r = report(&input(&arms, w(0.20, 0.35, 0.45), Some(&omega)));
    let wr = &r.weight_robust;
    assert_eq!(wr.region, RobustRegion::SelfReport);
    assert_eq!(wr.total, 14);
    assert_eq!(wr.shares.len(), 2);
    assert_eq!(wr.shares[0].id, "X");
    assert_eq!((wr.shares[0].points, wr.shares[0].sole_points), (0, 0));
    assert_eq!(wr.shares[1].id, "Y");
    assert_eq!((wr.shares[1].points, wr.shares[1].sole_points), (14, 14));
    close(wr.shares[1].share(wr.total), 1.0);
    assert_eq!(wr.everywhere.as_deref(), Some("Y"));
    assert_eq!(WEIGHT_ROBUST_EVERYWHERE, "在你的权重范围内都是它");
    assert_eq!(r.map.iter().filter(|p| p.in_omega_s).count(), 14);
    assert!(r.map.iter().all(|p| !p.in_omega_b));
    assert_eq!(r.current_winners, vec!["Y".to_string()]);
}

#[test]
fn no_self_report_uses_neighbourhood_of_current_weights() {
    // W_k = (1/3, 1/3, 1/3) is not a grid point; max|w − W_k| ≤ 0.10 keeps
    // the 12 points with every weight in {0.25, …, 0.40}.
    let arms = example_arms();
    let r = report(&input(&arms, Weights::equal(), None));
    let wr = &r.weight_robust;
    assert_eq!(wr.region, RobustRegion::Neighbourhood);
    assert_eq!(wr.total, 12);
    assert_eq!((wr.shares[0].id.as_str(), wr.shares[0].points), ("X", 6));
    assert_eq!((wr.shares[1].id.as_str(), wr.shares[1].points), ("Y", 6));
    assert_eq!(wr.everywhere, None);
}

#[test]
fn neighbourhood_radius_is_inclusive_at_0_10() {
    // W_k on the grid at (0.30, 0.30, 0.40): points with every coordinate
    // within 0.10 (two grid steps), boundary included.
    let arms = example_arms();
    let r = report(&input(&arms, w(0.30, 0.30, 0.40), None));
    let g = grid_points().unwrap();
    let expect = g
        .iter()
        .filter(|p| {
            let k = [
                (p.a() * 20.0).round() as i64 - 6,
                (p.b() * 20.0).round() as i64 - 6,
                (p.g() * 20.0).round() as i64 - 8,
            ];
            k.iter().all(|d| d.abs() <= 2)
        })
        .count();
    assert_eq!(r.weight_robust.total as usize, expect);
    assert_eq!(expect, 19);
}

#[test]
fn no_arms_withholds_the_map_instead_of_claiming_robustness() {
    let none: Vec<SensitivityArm> = Vec::new();
    assert_eq!(
        sensitivity(&input(&none, Weights::equal(), None)).unwrap(),
        Err(Withheld::NoArms)
    );
}

#[test]
fn first_window_and_tier_iii_withhold_the_map() {
    let arms = example_arms();
    let mut i = input(&arms, Weights::equal(), None);
    i.first_window = true;
    assert_eq!(sensitivity(&i).unwrap(), Err(Withheld::FirstWindow));
    i.first_window = false;
    i.reference = None;
    assert_eq!(sensitivity(&i).unwrap(), Err(Withheld::NoReference));
}

#[test]
fn ties_are_reported_as_a_whole_group_never_broken_by_name() {
    let arms = vec![arm("B", 0.5, 60.0, 0.9), arm("A", 0.5, 60.0, 0.9)];
    let r = report(&input(&arms, Weights::equal(), None));
    for p in &r.map {
        assert_eq!(p.winners, vec!["A".to_string(), "B".to_string()]);
    }
    let wr = &r.weight_robust;
    assert!(wr
        .shares
        .iter()
        .all(|s| s.points == wr.total && s.sole_points == 0));
    assert_eq!(wr.everywhere, None, "a shared win is not '都是它'");
}

fn frontier_off(arms: &[SensitivityArm]) -> Vec<String> {
    let fa: Vec<FrontierArm> = arms
        .iter()
        .map(|a| {
            let t = Triple::new(a.point.c, a.point.t, a.point.q);
            FrontierArm {
                id: a.id.clone(),
                star: t,
                robust: t,
                robust_enabled: true,
            }
        })
        .collect();
    layer_frontiers(&fa).unwrap().off
}

fn four_arms() -> Vec<SensitivityArm> {
    vec![
        arm("cheap", 0.10, 200.0, 0.70),
        arm("fast", 1.20, 30.0, 0.80),
        arm("good", 0.80, 150.0, 0.97),
        arm("dominated", 1.30, 210.0, 0.69),
    ]
}

#[test]
fn property_1_winners_are_always_on_the_official_frontier() {
    let arms = four_arms();
    let off = frontier_off(&arms);
    assert!(!off.contains(&"dominated".to_string()));
    let r = report(&input(&arms, Weights::equal(), None));
    let mut seen = std::collections::BTreeSet::new();
    for p in &r.map {
        for id in &p.winners {
            assert!(off.contains(id), "{id} at {:?}", p.weights);
            seen.insert(id.clone());
        }
    }
    // Changing the weights only changes who wins inside the frontier.
    assert!(seen.len() >= 2, "{seen:?}");
}

#[test]
fn property_2_winners_do_not_depend_on_the_numerator_anchor() {
    let arms = four_arms();
    let floors = Floors::default();
    let anchors = [
        Point {
            c: 0.6,
            t: 90.0,
            q: 0.8,
        },
        Point {
            c: 0.05,
            t: 10.0,
            q: 0.99,
        },
        Point {
            c: 5.0,
            t: 900.0,
            q: 0.1,
        },
    ];
    for g in grid_points().unwrap() {
        let base = winners(&arms, anchors[0], &g, &floors).unwrap();
        for a in &anchors[1..] {
            assert_eq!(winners(&arms, *a, &g, &floors).unwrap(), base);
        }
    }
}

#[test]
fn omega_b_points_are_flagged_on_the_map() {
    let arms = example_arms();
    let b = vec![w(0.05, 0.05, 0.90), w(0.20, 0.35, 0.45)];
    let mut i = input(&arms, Weights::equal(), None);
    i.omega_b = Some(&b);
    let r = report(&i);
    let flagged: Vec<Weights> = r
        .map
        .iter()
        .filter(|p| p.in_omega_b)
        .map(|p| p.weights)
        .collect();
    assert_eq!(flagged.len(), 2);
    let keys: Vec<(i64, i64)> = flagged
        .iter()
        .map(|p| ((p.a() * 20.0).round() as i64, (p.b() * 20.0).round() as i64))
        .collect();
    assert_eq!(keys, vec![(1, 1), (4, 7)]);
    assert!(r.map.iter().all(|p| !p.in_omega_s));
}

#[test]
fn empty_omega_b_means_a_signal_with_no_grid_point() {
    // Step 11 can produce an empty Ω_b (behaviour has a signal but no grid
    // point qualifies, e.g. w_b off the grid). Accepted: no point is
    // flagged, and the report says so, distinct from "no signal".
    let arms = example_arms();
    let empty: Vec<Weights> = Vec::new();
    let mut i = input(&arms, Weights::equal(), None);
    i.omega_b = Some(&empty);
    let r = report(&i);
    assert_eq!(r.behaviour_region, BehaviourRegion::NoGridPoint);
    assert!(r.map.iter().all(|p| !p.in_omega_b));
    // The rest of the report is the same as without Ω_b.
    i.omega_b = None;
    let none = report(&i);
    assert_eq!(none.behaviour_region, BehaviourRegion::NoSignal);
    assert!(none.map.iter().all(|p| !p.in_omega_b));
    assert_eq!(r.map, none.map);
    assert_eq!(r.weight_robust, none.weight_robust);
    assert_eq!(r.current_winners, none.current_winners);
    // With points: counted.
    let b = vec![w(0.05, 0.05, 0.90), w(0.20, 0.35, 0.45)];
    i.omega_b = Some(&b);
    assert_eq!(report(&i).behaviour_region, BehaviourRegion::Points(2));
}

#[test]
fn empty_omega_s_is_an_error() {
    // Ω_s always holds the Δℓ = m points, so empty means a caller bug.
    let arms = example_arms();
    let empty: Vec<Weights> = Vec::new();
    assert!(matches!(
        sensitivity(&input(&arms, Weights::equal(), Some(&empty))),
        Err(CoreError::InvalidField {
            field: "omega_s",
            ..
        })
    ));
}

#[test]
fn bad_regions_and_arm_ids_are_errors() {
    let arms = example_arms();
    let empty: Vec<Weights> = Vec::new();
    assert!(sensitivity(&input(&arms, Weights::equal(), Some(&empty))).is_err());
    let off_grid = vec![Weights::equal()];
    assert!(sensitivity(&input(&arms, Weights::equal(), Some(&off_grid))).is_err());
    let dup = vec![w(0.2, 0.35, 0.45), w(0.2, 0.35, 0.45)];
    assert!(sensitivity(&input(&arms, Weights::equal(), Some(&dup))).is_err());
    let mut i = input(&arms, Weights::equal(), None);
    i.omega_b = Some(&off_grid);
    assert!(sensitivity(&i).is_err());

    let twice = vec![arm("X", 0.3, 120.0, 0.8), arm("X", 0.9, 60.0, 0.85)];
    assert!(matches!(
        sensitivity(&input(&twice, Weights::equal(), None)),
        Err(CoreError::DuplicateCell { .. })
    ));
    i.omega_b = Some(&dup);
    assert!(sensitivity(&i).is_err());
    let blank = vec![arm(" ", 0.3, 120.0, 0.8)];
    assert!(matches!(
        sensitivity(&input(&blank, Weights::equal(), None)),
        Err(CoreError::EmptyField { .. })
    ));
    let nan = vec![arm("X", f64::NAN, 120.0, 0.8)];
    assert!(sensitivity(&input(&nan, Weights::equal(), None)).is_err());
}

// ---- noise floors (DECISIONS K) ----

#[test]
fn decisions_k_floor_flip_example_is_not_robust_and_lists_seven_runs() {
    // floor_sens.py item 4: X cheap with more defects, Y dearer with fewer.
    let arms = vec![arm("X", 0.10, 120.0, 0.92), arm("Y", 0.40, 120.0, 0.97)];
    let mut i = input(&arms, Weights::equal(), None);
    i.reference = Some(Point {
        c: 0.6,
        t: 120.0,
        q: 0.90,
    });
    let r = report(&i);
    let fr = &r.floor_robust;
    assert!(!fr.robust);
    let got: Vec<(FloorVariant, &str)> = fr
        .runs
        .iter()
        .map(|(v, w)| {
            assert_eq!(w.len(), 1);
            (*v, w[0].as_str())
        })
        .collect();
    assert_eq!(
        got,
        vec![
            (FloorVariant::Base, "X"),
            (FloorVariant::C0Half, "X"),
            (FloorVariant::C0Double, "Y"),
            (FloorVariant::T0Half, "X"),
            (FloorVariant::T0Double, "X"),
            (FloorVariant::EpsHalf, "Y"),
            (FloorVariant::EpsDouble, "X"),
        ]
    );
    // The frontier does not use floors: every run's winner is on it.
    let off = frontier_off(&arms);
    for (_, w) in &fr.runs {
        assert!(w.iter().all(|id| off.contains(id)));
    }
}

#[test]
fn decisions_k_only_halves_and_doubles() {
    let f = Floors::default();
    let vals: Vec<(f64, f64, f64)> = FloorVariant::ALL
        .iter()
        .map(|v| {
            let g = v.apply(&f).unwrap();
            (g.c0(), g.t0(), g.eps())
        })
        .collect();
    let expect = [
        (0.15, 15.0, 0.02),
        (0.075, 15.0, 0.02),
        (0.30, 15.0, 0.02),
        (0.15, 7.5, 0.02),
        (0.15, 30.0, 0.02),
        (0.15, 15.0, 0.01),
        (0.15, 15.0, 0.04),
    ];
    assert_eq!(vals.len(), 7);
    for (g, e) in vals.iter().zip(expect) {
        close(g.0, e.0);
        close(g.1, e.1);
        close(g.2, e.2);
    }
    // The base is the scope's floors (§A override), not the §9 constants.
    let custom = Floors::new(0.4, 20.0, 0.05).unwrap();
    let d = FloorVariant::C0Double.apply(&custom).unwrap();
    close(d.c0(), 0.8);
    close(d.t0(), 20.0);
    close(d.eps(), 0.05);
    let h = FloorVariant::EpsHalf.apply(&custom).unwrap();
    close(h.eps(), 0.025);
}

#[test]
fn dominating_arm_is_floor_robust() {
    let arms = vec![arm("A", 0.2, 30.0, 0.95), arm("B", 0.5, 60.0, 0.80)];
    let r = report(&input(&arms, Weights::equal(), None));
    assert!(r.floor_robust.robust);
    assert!(r
        .floor_robust
        .runs
        .iter()
        .all(|(_, w)| *w == vec!["A".to_string()]));
}

// ---- flip line ----

fn flip(arms: &[SensitivityArm], current: Weights) -> FlipLine {
    flip_line(arms, example_reference(), &current, &Floors::default())
        .unwrap()
        .unwrap()
}

#[test]
fn flip_line_deltas_follow_the_spec_formula() {
    let f = flip(&example_arms(), Weights::equal());
    // At (1/3, 1/3, 1/3): V_X = 109.04 > V_Y = 108.97.
    assert_eq!((f.leader.as_str(), f.runner_up.as_str()), ("X", "Y"));
    assert!(!f.tied_now);
    close(f.delta[0], (0.45f64 / 1.05).ln());
    close(f.delta[1], (135.0f64 / 75.0).ln());
    close(f.delta[2], (0.22f64 / 0.17).ln());
    // aΔ_C + bΔ_T + gΔ_D = −(ln V_X − ln V_Y) ≤ 0 at the current point.
    let s: f64 = f.delta.iter().sum::<f64>() / 3.0;
    let vx = value_score(
        example_reference(),
        example_arms()[0].point,
        &Weights::equal(),
        &Floors::default(),
    )
    .unwrap();
    let vy = value_score(
        example_reference(),
        example_arms()[1].point,
        &Weights::equal(),
        &Floors::default(),
    )
    .unwrap();
    close(s, -(vx.ln() - vy.ln()));
}

#[test]
fn flip_statements_with_each_dimension_fixed() {
    let f = flip(&example_arms(), Weights::equal());
    assert_eq!(f.statements.len(), 3);
    let kinds: Vec<(Dim, FlipKind)> = f.statements.iter().map(|s| (s.fixed, s.kind)).collect();
    match kinds[2] {
        (Dim::Quality, FlipKind::Crosses { over, under, ratio }) => {
            assert_eq!((over, under), (Dim::Time, Dim::Cost));
            assert!((ratio - 1.002_346_98).abs() < 1e-8, "{ratio}");
        }
        k => panic!("{k:?}"),
    }
    match kinds[0] {
        (Dim::Cost, FlipKind::Crosses { over, under, ratio }) => {
            assert_eq!((over, under), (Dim::Time, Dim::Quality));
            assert!((ratio - 1.010_248_02).abs() < 1e-8, "{ratio}");
        }
        k => panic!("{k:?}"),
    }
    match kinds[1] {
        (Dim::Time, FlipKind::Crosses { over, under, ratio }) => {
            assert_eq!((over, under), (Dim::Quality, Dim::Cost));
            assert!((ratio - 1.003_048_79).abs() < 1e-8, "{ratio}");
        }
        k => panic!("{k:?}"),
    }
    // 1.0023… is printed rounded up (1.01), so the sentence stays true.
    let s = flip_sentence(&f, &f.statements[2]);
    assert_eq!(
        s,
        "保持质量权重不变，只要时间权重超过成本权重的 1.01 倍，Y 就会超过 X；这是在当前估计值下的结论"
    );
    assert_eq!(CURRENT_ESTIMATE_NOTE, "这是在当前估计值下的结论");
}

/// For each crossing statement, step just past the threshold on both sides
/// and check who is ahead with the real `V`.
fn check_statements(arms: &[SensitivityArm], current: Weights) -> usize {
    let f = flip(arms, current);
    let reference = example_reference();
    let floors = Floors::default();
    let by_id = |id: &str| arms.iter().find(|a| a.id == id).unwrap().point;
    let (pi, pj) = (by_id(&f.leader), by_id(&f.runner_up));
    let cur = [current.a(), current.b(), current.g()];
    let mut checked = 0;
    for s in &f.statements {
        assert_eq!(s.fixed_weight, cur[s.fixed as usize], "{s:?}");
        let FlipKind::Crosses { over, under, ratio } = s.kind else {
            continue;
        };
        let rest = 1.0 - s.fixed_weight;
        // Either side of the exact threshold, and just above the printed
        // (rounded-up) one: the sentence must be a sufficient condition.
        let printed = printed_ratio(ratio);
        assert!(printed >= ratio);
        for (r, runner_ahead) in [
            (ratio * 1.001, true),
            (ratio * 0.999, false),
            (printed * (1.0 + 1e-9), true),
        ] {
            let wu = rest / (1.0 + r);
            let wo = rest - wu;
            if wu < 0.05 || wo < 0.05 {
                continue;
            }
            let mut v = cur;
            v[s.fixed as usize] = s.fixed_weight;
            v[over as usize] = wo;
            v[under as usize] = wu;
            let wt = w(v[0], v[1], v[2]);
            let vi = value_score(reference, pi, &wt, &floors).unwrap();
            let vj = value_score(reference, pj, &wt, &floors).unwrap();
            assert_eq!(vj > vi, runner_ahead, "{s:?} at ratio {r}");
            checked += 1;
        }
    }
    checked
}

#[test]
fn flip_statements_agree_with_v_on_both_sides() {
    // Each example has crossings inside the segment, so probes really run
    // (3 statements × 3 probes for the two-arm example).
    assert_eq!(check_statements(&example_arms(), Weights::equal()), 9);
    assert!(check_statements(&example_arms(), w(0.2, 0.35, 0.45)) > 0);
    assert!(check_statements(&four_arms(), w(0.5, 0.2, 0.3)) > 0);
    assert!(check_statements(&four_arms(), w(0.1, 0.1, 0.8)) > 0);
}

#[test]
fn flip_never_when_runner_up_cannot_catch_up() {
    // B is worse in every dimension: it is never ahead on any segment.
    let arms = vec![arm("A", 0.2, 30.0, 0.95), arm("B", 0.5, 60.0, 0.80)];
    let f = flip(&arms, Weights::equal());
    assert!(f.statements.iter().all(|s| s.kind == FlipKind::Never));
    let s = flip_sentence(&f, &f.statements[0]);
    assert!(s.contains("B 在任何可行权重下都不会超过 A"), "{s}");
}

#[test]
fn flip_line_needs_two_arms_and_an_unambiguous_runner_up() {
    let one = vec![arm("A", 0.2, 30.0, 0.95)];
    assert_eq!(
        flip_line(
            &one,
            example_reference(),
            &Weights::equal(),
            &Floors::default()
        )
        .unwrap(),
        Err(NoFlipLine::TooFewArms)
    );
    let three = vec![
        arm("A", 0.2, 30.0, 0.95),
        arm("B", 0.5, 60.0, 0.80),
        arm("C", 0.5, 60.0, 0.80),
    ];
    assert_eq!(
        flip_line(
            &three,
            example_reference(),
            &Weights::equal(),
            &Floors::default()
        )
        .unwrap(),
        Err(NoFlipLine::AmbiguousRunnerUp)
    );
    let tied_top = vec![arm("B", 0.5, 60.0, 0.80), arm("A", 0.5, 60.0, 0.80)];
    let f = flip(&tied_top, Weights::equal());
    assert!(f.tied_now);
    assert_eq!((f.leader.as_str(), f.runner_up.as_str()), ("A", "B"));
    // Identical arms tie on every segment.
    assert!(f.statements.iter().all(|s| s.kind == FlipKind::Tied));
    assert!(flip_sentence(&f, &f.statements[0]).contains("B 与 A 在任何可行权重下都并列"));
    // A top tie group of three is not a pair either.
    let three_top = vec![
        arm("C", 0.5, 60.0, 0.80),
        arm("A", 0.5, 60.0, 0.80),
        arm("B", 0.5, 60.0, 0.80),
    ];
    assert_eq!(
        flip_line(
            &three_top,
            example_reference(),
            &Weights::equal(),
            &Floors::default()
        )
        .unwrap(),
        Err(NoFlipLine::AmbiguousRunnerUp)
    );
}

#[test]
fn near_tie_at_the_top_is_ordered_by_id_not_by_float_noise() {
    // "A" is a few ulps more expensive than "B": V_B > V_A by ~1e-14,
    // inside the tie tolerance, so the pair is tied and listed A, B.
    let a = arm("A", 0.5 + 4.0 * f64::EPSILON, 60.0, 0.80);
    let b = arm("B", 0.5, 60.0, 0.80);
    let fl = Floors::default();
    let va = value_score(example_reference(), a.point, &Weights::equal(), &fl).unwrap();
    let vb = value_score(example_reference(), b.point, &Weights::equal(), &fl).unwrap();
    assert!(vb > va && vb - va <= 1e-12, "{va} {vb}");
    let f = flip(&[b, a], Weights::equal());
    assert!(f.tied_now);
    assert_eq!((f.leader.as_str(), f.runner_up.as_str()), ("A", "B"));
}

// ---- flip-line segment ends and the parallel branch (DECISIONS 12.9) ----

/// Two arms with equal quality whose deltas are exactly `(dc, dt, 0)`
/// under the default floors: `j` at (1.0 元, 45 s), `i` scaled from it.
fn arms_with_deltas(dc: f64, dt: f64) -> Vec<SensitivityArm> {
    let (c0, t0) = (0.15, 15.0);
    let (cj, tj) = (1.0, 45.0);
    let ci = (cj + c0) * dc.exp() - c0;
    let ti = (tj + t0) * dt.exp() - t0;
    vec![arm("i", ci, ti, 0.8), arm("j", cj, tj, 0.8)]
}

fn quality_fixed(arms: &[SensitivityArm]) -> FlipKind {
    let f = flip(arms, Weights::equal());
    assert_eq!((f.leader.as_str(), f.runner_up.as_str()), ("i", "j"));
    assert_eq!(f.delta[2], 0.0);
    let s = f
        .statements
        .iter()
        .find(|s| s.fixed == Dim::Quality)
        .unwrap();
    s.kind
}

#[test]
fn crossing_below_the_lower_end_0_05_is_never() {
    // Quality fixed at 1/3, S = 2/3: j is ahead only for w_T < S·Δ_C = 0.03,
    // which is below the 0.05 floor. The conclusion must be "never",
    // not "time weight more than r times …" with an infeasible r.
    let arms = arms_with_deltas(0.045, -0.955);
    assert_eq!(quality_fixed(&arms), FlipKind::Never);
    // Control: move the crossing inside the segment (w_T < 0.06).
    let inside = arms_with_deltas(0.09, -0.91);
    assert!(matches!(
        quality_fixed(&inside),
        FlipKind::Crosses {
            over: Dim::Cost,
            under: Dim::Time,
            ..
        }
    ));
}

#[test]
fn lower_end_is_pinned_at_0_05_to_within_1e_4() {
    // Crossing at w_T = S·Δ_C: 0.0499 is outside, 0.0501 inside.
    let s = 2.0 / 3.0;
    let out = 0.0499 / s;
    assert_eq!(
        quality_fixed(&arms_with_deltas(out, out - 1.0)),
        FlipKind::Never
    );
    let inn = 0.0501 / s;
    assert!(matches!(
        quality_fixed(&arms_with_deltas(inn, inn - 1.0)),
        FlipKind::Crosses {
            over: Dim::Cost,
            under: Dim::Time,
            ..
        }
    ));
}

#[test]
fn upper_end_is_pinned_at_s_minus_0_05_to_within_1e_4() {
    let s = 2.0 / 3.0;
    let out = (s - 0.05 + 1e-4) / s;
    assert_eq!(
        quality_fixed(&arms_with_deltas(-out, 1.0 - out)),
        FlipKind::Never
    );
    let inn = (s - 0.05 - 1e-4) / s;
    assert!(matches!(
        quality_fixed(&arms_with_deltas(-inn, 1.0 - inn)),
        FlipKind::Crosses {
            over: Dim::Time,
            under: Dim::Cost,
            ..
        }
    ));
}

#[test]
fn crossing_above_the_upper_end_s_minus_0_05_is_never() {
    // j is ahead only for w_T > S·0.955 = 0.6367 > S − 0.05 = 0.6167.
    let arms = arms_with_deltas(-0.955, 0.045);
    assert_eq!(quality_fixed(&arms), FlipKind::Never);
    // Control: crossing at S·0.91 = 0.6067, just inside.
    let inside = arms_with_deltas(-0.91, 0.09);
    assert!(matches!(
        quality_fixed(&inside),
        FlipKind::Crosses {
            over: Dim::Time,
            under: Dim::Cost,
            ..
        }
    ));
}

#[test]
fn crossing_at_exactly_the_end_points_is_never() {
    // Root exactly at 0.05 or S − 0.05: j is ahead only strictly beyond
    // it, where the other weight would be below 0.05.
    let s = 2.0 / 3.0;
    for (dc, dt) in [
        (0.05 / s, 0.05 / s - 1.0),
        (-(s - 0.05) / s, 1.0 - (s - 0.05) / s),
    ] {
        let k = quality_fixed(&arms_with_deltas(dc, dt));
        if let FlipKind::Crosses { ratio, .. } = k {
            // Float noise may land a hair inside; then the ratio must
            // still describe a feasible split (both weights ≥ 0.05).
            let wu = s / (1.0 + ratio);
            assert!(wu >= 0.05 - 1e-9 && s - wu >= 0.05 - 1e-9, "{k:?}");
        } else {
            assert_eq!(k, FlipKind::Never);
        }
    }
}

#[test]
fn parallel_segment_with_strictly_worse_runner_up_is_never() {
    // Same cost and time, B has lower quality: with quality fixed the
    // segment is parallel to the line (slope 0) and B is behind on all of
    // it. It must not say "B 在任何可行权重下都高于 A".
    let arms = vec![arm("A", 0.5, 60.0, 0.90), arm("B", 0.5, 60.0, 0.80)];
    let f = flip(&arms, Weights::equal());
    assert_eq!((f.leader.as_str(), f.runner_up.as_str()), ("A", "B"));
    assert_eq!(f.delta[0], 0.0);
    assert_eq!(f.delta[1], 0.0);
    let q = f
        .statements
        .iter()
        .find(|s| s.fixed == Dim::Quality)
        .unwrap();
    assert_eq!(q.kind, FlipKind::Never);
    assert_eq!(
        flip_sentence(&f, q),
        "保持质量权重不变，B 在任何可行权重下都不会超过 A；这是在当前估计值下的结论"
    );
    assert!(f.statements.iter().all(|s| s.kind == FlipKind::Never));
}

#[test]
fn printed_ratio_rounds_up_but_keeps_exact_two_decimals() {
    // 1.1 · 100 = 110.00000000000001 in floats: ceil alone would print 1.11.
    assert_eq!(printed_ratio(1.1), 1.1);
    assert_eq!(printed_ratio(1.4), 1.4);
    assert_eq!(printed_ratio(0.29), 0.29);
    assert_eq!(printed_ratio(1.1 + 1e-6), 1.11);
    assert_eq!(printed_ratio(1.0023), 1.01);
    // Within 1e-11 of x.yz the value prints as x.yz (documented tolerance).
    assert_eq!(printed_ratio(1.1 + 1e-12), 1.1);
    // The tolerance is 1e-9 on ratio·100, i.e. 1e-11 on the ratio.
    assert_eq!(printed_ratio(1.1 + 5e-12), 1.1);
    assert_eq!(printed_ratio(1.1 + 5e-11), 1.11);
}

#[test]
fn off_grid_tolerance_is_1e_9_not_a_fraction_of_a_step() {
    let arms = example_arms();
    // A point 1e-6 off the grid (still summing to 1) is rejected …
    let off = vec![w(0.2 + 1e-6, 0.35 - 1e-6, 0.45)];
    assert!(sensitivity(&input(&arms, Weights::equal(), Some(&off))).is_err());
    // … and so is a quarter step off.
    let quarter = vec![w(0.2125, 0.3375, 0.45)];
    assert!(sensitivity(&input(&arms, Weights::equal(), Some(&quarter))).is_err());
    // A point within float noise (1e-12) of a grid point is that point.
    let near = vec![w(0.2 + 1e-12, 0.35 - 1e-12, 0.45)];
    let r = report(&input(&arms, Weights::equal(), Some(&near)));
    let flagged: Vec<_> = r.map.iter().filter(|p| p.in_omega_s).collect();
    assert_eq!(flagged.len(), 1);
    assert_eq!(flagged[0].weights, w(0.2, 0.35, 0.45));
    // The tolerance is 1e-9 per weight: 1e-10 snaps, 1e-8 is rejected.
    let snap = vec![w(0.2 + 1e-10, 0.35 - 1e-10, 0.45)];
    let r = report(&input(&arms, Weights::equal(), Some(&snap)));
    assert_eq!(r.map.iter().filter(|p| p.in_omega_s).count(), 1);
    let reject = vec![w(0.2 + 1e-8, 0.35 - 1e-8, 0.45)];
    assert!(sensitivity(&input(&arms, Weights::equal(), Some(&reject))).is_err());
}

#[test]
fn floor_variants_through_sensitivity_use_the_scope_floors() {
    // Same X/Y as the DECISIONS K example, but the scope overrides the
    // floors to (0.05, 15, 0.005). Halving / doubling those keeps X on
    // top in all seven runs; with the §9 default floors some variant
    // flips, so using the defaults by mistake would read "not robust".
    let arms = vec![arm("X", 0.10, 120.0, 0.92), arm("Y", 0.40, 120.0, 0.97)];
    let mut i = input(&arms, Weights::equal(), None);
    i.reference = Some(Point {
        c: 0.6,
        t: 120.0,
        q: 0.90,
    });
    i.floors = Floors::new(0.05, 15.0, 0.005).unwrap();
    let r = report(&i);
    assert!(r.floor_robust.robust);
    assert_eq!(r.floor_robust.runs.len(), 7);
    assert!(r
        .floor_robust
        .runs
        .iter()
        .all(|(_, w)| *w == vec!["X".to_string()]));
    assert_eq!(r.current_winners, vec!["X".to_string()]);
    i.floors = Floors::default();
    let base = report(&i);
    assert!(!base.floor_robust.robust);
    // The map itself also uses the scope floors: it differs from the map
    // under the default floors.
    assert!(r
        .map
        .iter()
        .zip(&base.map)
        .any(|(a, b)| a.winners != b.winners));
}

#[test]
fn near_tie_dominated_twin_shares_the_group_and_is_not_an_error() {
    // "good2" is "good" made 1e-13 元 dearer: dominated (not on F_off), but
    // its V is within the 1e-12 tie tolerance of good's, so §12 reports
    // them as one tie group. The runtime property-1 check must accept a
    // group that also holds its F_off dominator.
    let mut arms = four_arms();
    arms.push(arm("good2", 0.80 + 1e-13, 150.0, 0.97));
    let off = frontier_off(&arms);
    assert!(!off.contains(&"good2".to_string()));
    let r = report(&input(&arms, Weights::equal(), None));
    let mut shared = 0;
    for p in &r.map {
        if p.winners.contains(&"good2".to_string()) {
            assert!(p.winners.contains(&"good".to_string()), "{:?}", p.weights);
            shared += 1;
        }
    }
    assert!(shared > 0);
}

#[test]
fn single_task_stratum_is_always_drawn_as_itself() {
    // n_iu = 1 is common; below(1) must return 0 every time.
    let tasks = vec![
        BootstrapTask {
            arm: "X".into(),
            cell: cell("fix"),
        },
        BootstrapTask {
            arm: "X".into(),
            cell: cell("fix"),
        },
        BootstrapTask {
            arm: "Y".into(),
            cell: cell("fix"),
        },
    ];
    assert_eq!(bootstrap_strata(&tasks).unwrap(), vec![vec![0, 1], vec![2]]);
    let mut draws = Vec::new();
    statistical_robustness(
        &tasks,
        7,
        &ids(),
        &Weights::equal(),
        &Floors::default(),
        |d| {
            draws.push(d.to_vec());
            Ok(ResampleLayer {
                arms: Vec::new(),
                reference: None,
            })
        },
    )
    .unwrap();
    assert_eq!(draws.len(), 200);
    assert!(draws.iter().all(|d| d.len() == 3 && d[2] == 2));
}

#[test]
fn blank_id_in_arm_ids_is_an_error() {
    let fx = fixture();
    let r = statistical_robustness(
        &fx.tasks,
        1,
        &["X".to_string(), " ".to_string()],
        &Weights::equal(),
        &Floors::default(),
        |d| Ok(layer_from(&fx, d)),
    );
    assert!(r.is_err());
}

#[test]
fn slightly_dominated_arm_never_wins_and_property_1_is_checked_at_runtime() {
    // "good2" is "good" made 1e-6 元 dearer: dominated, never on F_off.
    let mut arms = four_arms();
    arms.push(arm("good2", 0.80 + 1e-6, 150.0, 0.97));
    let off = frontier_off(&arms);
    assert!(!off.contains(&"good2".to_string()));
    assert!(off.contains(&"good".to_string()));
    let r = report(&input(&arms, Weights::equal(), None));
    let mut good_wins = 0;
    for p in &r.map {
        assert!(!p.winners.contains(&"good2".to_string()), "{:?}", p.weights);
        good_wins += usize::from(p.winners.contains(&"good".to_string()));
    }
    // "good" does win somewhere, so the dominated twin had chances to.
    assert!(good_wins > 0);
}

// ---- statistical robustness (task bootstrap, DECISIONS 12.6) ----

fn cell(name: &str) -> CellId {
    CellId::user(name).unwrap()
}

/// Two arms × two cells. Observations are per task (cost, time, quality);
/// a resample's arm triple is the cell means averaged with ω = (0.5, 0.5).
struct Fixture {
    tasks: Vec<BootstrapTask>,
    obs: Vec<(f64, f64, f64)>,
}

fn fixture() -> Fixture {
    let rows = [
        ("X", "fix", (0.2, 100.0, 0.80)),
        ("X", "fix", (0.4, 140.0, 0.80)),
        ("X", "new", (0.3, 120.0, 0.85)),
        ("X", "new", (0.3, 120.0, 0.75)),
        ("X", "new", (0.35, 110.0, 0.80)),
        ("Y", "fix", (0.9, 50.0, 0.90)),
        ("Y", "fix", (0.8, 70.0, 0.80)),
        ("Y", "new", (1.0, 60.0, 0.85)),
        ("Y", "new", (0.9, 60.0, 0.85)),
    ];
    Fixture {
        tasks: rows
            .iter()
            .map(|(a, c, _)| BootstrapTask {
                arm: (*a).into(),
                cell: cell(c),
            })
            .collect(),
        obs: rows.iter().map(|r| r.2).collect(),
    }
}

fn layer_from(fx: &Fixture, draw: &[usize]) -> ResampleLayer {
    let mut arms = Vec::new();
    for a in ["X", "Y"] {
        let mut p = Point {
            c: 0.0,
            t: 0.0,
            q: 0.0,
        };
        for c in ["fix", "new"] {
            let xs: Vec<_> = draw
                .iter()
                .filter(|&&k| fx.tasks[k].arm == a && fx.tasks[k].cell == cell(c))
                .map(|&k| fx.obs[k])
                .collect();
            let n = xs.len() as f64;
            p.c += 0.5 * xs.iter().map(|x| x.0).sum::<f64>() / n;
            p.t += 0.5 * xs.iter().map(|x| x.1).sum::<f64>() / n;
            p.q += 0.5 * xs.iter().map(|x| x.2).sum::<f64>() / n;
        }
        arms.push(SensitivityArm {
            id: a.into(),
            point: p,
        });
    }
    ResampleLayer {
        arms,
        reference: Some(example_reference()),
    }
}

fn ids() -> Vec<String> {
    vec!["X".to_string(), "Y".to_string()]
}

/// Runs the bootstrap and returns the result plus every resample's draw.
fn run_bootstrap(
    seed: u64,
) -> (
    agent_value_core::sensitivity::StatisticalRobustness,
    Vec<Vec<usize>>,
) {
    let fx = fixture();
    let mut draws = Vec::new();
    let r = statistical_robustness(
        &fx.tasks,
        seed,
        &ids(),
        &Weights::equal(),
        &Floors::default(),
        |d| {
            draws.push(d.to_vec());
            Ok(layer_from(&fx, d))
        },
    )
    .unwrap();
    (r, draws)
}

#[test]
fn strata_are_arm_cell_groups_in_key_order() {
    let fx = fixture();
    assert_eq!(
        bootstrap_strata(&fx.tasks).unwrap(),
        vec![vec![0, 1], vec![2, 3, 4], vec![5, 6], vec![7, 8]]
    );
}

#[test]
fn every_resample_keeps_each_arm_cell_count_and_stays_in_its_stratum() {
    let fx = fixture();
    let (_, draws) = run_bootstrap(20261020);
    assert_eq!(draws.len(), 200);
    // Expected groups written out, not taken from the function under test.
    let strata = [vec![0usize, 1], vec![2, 3, 4], vec![5, 6], vec![7, 8]];
    for d in &draws {
        assert_eq!(d.len(), fx.tasks.len());
        let mut at = 0;
        for st in &strata {
            for &k in &d[at..at + st.len()] {
                assert!(st.contains(&k), "{k} drawn outside {st:?}");
            }
            at += st.len();
        }
    }
}

#[test]
fn every_task_including_the_last_of_each_stratum_is_drawn() {
    let fx = fixture();
    let (_, draws) = run_bootstrap(20261020);
    let mut seen = vec![0u32; fx.tasks.len()];
    for d in &draws {
        for &k in d {
            seen[k] += 1;
        }
    }
    assert!(seen.iter().all(|&h| h > 0), "{seen:?}");
    // Last positions of the four strata: 1, 4, 6, 8.
    for last in [1usize, 4, 6, 8] {
        assert!(seen[last] > 0);
    }
}

#[test]
fn bootstrap_runs_200_resamples_and_is_reproducible_by_seed() {
    let (a, da) = run_bootstrap(20261020);
    let (b, db) = run_bootstrap(20261020);
    assert_eq!(a, b);
    assert_eq!(da, db);
    assert_eq!(a.seed, 20261020);
    assert_eq!(a.resamples, BOOTSTRAP_RESAMPLES);
    assert_eq!(a.resamples, 200);
    assert_eq!(a.with_value, 200);
    let wins: u32 = a.arms.iter().map(|x| x.winner).sum();
    assert_eq!(wins, 200 + a.tied);
    assert!(a.arms.iter().all(|x| x.frontier == 200));
    assert!(
        a.arms.iter().all(|x| x.winner > 0 && x.winner < 200),
        "{a:?}"
    );
    // The seed really drives the draws: compare the drawn task positions,
    // not the result struct (which carries the seed itself).
    let (c, dc) = run_bootstrap(7);
    assert_eq!(c.seed, 7);
    assert_ne!(da, dc);
    assert_ne!(da[0], dc[0]);
}

#[test]
fn xorshift64_known_answers_match_independent_python() {
    // logs/pr12/xorshift_kat.py (written from Marsaglia 2003, shifts 13/7/17).
    let mut r = Xorshift64::new(1).unwrap();
    assert_eq!(r.next_u64(), 1_082_269_761);
    assert_eq!(r.next_u64(), 1_152_992_998_833_853_505);
    assert_eq!(r.next_u64(), 11_177_516_664_432_764_457);
    // n = 2^63 + 1 rejects about half of all outputs, so the draws differ
    // from plain modulo from the third one on.
    let mut r = Xorshift64::new(1).unwrap();
    let n = (1u64 << 63) + 1;
    let got: Vec<u64> = (0..6).map(|_| r.below(n).unwrap()).collect();
    assert_eq!(
        got,
        vec![
            1_082_269_761,
            1_152_992_998_833_853_505,
            9_024_654_201_992_055_039,
            8_332_670_729_032_836_398,
            2_074_683_623_940_568_804,
            5_632_491_372_296_299_238,
        ]
    );
}

#[test]
fn bootstrap_counts_frontier_without_anchor_and_dominant_wins_all() {
    let tasks: Vec<BootstrapTask> = ["A", "A", "B"]
        .iter()
        .map(|a| BootstrapTask {
            arm: (*a).into(),
            cell: cell("fix"),
        })
        .collect();
    let ids = vec!["A".to_string(), "B".to_string()];
    let mut calls = 0u32;
    let mut with_anchor = false;
    let r = statistical_robustness(
        &tasks,
        1,
        &ids,
        &Weights::equal(),
        &Floors::default(),
        |_| {
            calls += 1;
            with_anchor = !with_anchor;
            Ok(ResampleLayer {
                arms: vec![arm("A", 0.2, 30.0, 0.95), arm("B", 0.5, 60.0, 0.80)],
                reference: with_anchor.then(example_reference),
            })
        },
    )
    .unwrap();
    assert_eq!(calls, 200);
    assert_eq!(r.with_value, 100);
    assert_eq!((r.arms[0].winner, r.arms[0].frontier), (100, 200));
    assert_eq!((r.arms[1].winner, r.arms[1].frontier), (0, 0));
}

#[test]
fn bootstrap_rejects_bad_inputs() {
    let one = |a: &str| BootstrapTask {
        arm: a.into(),
        cell: cell("fix"),
    };
    let tasks = vec![one("A"), one("A")];
    let ids = vec!["A".to_string()];
    let ok = |_: &[usize]| {
        Ok(ResampleLayer {
            arms: vec![arm("A", 0.2, 30.0, 0.95)],
            reference: Some(example_reference()),
        })
    };
    let eq = Weights::equal();
    let fl = Floors::default();
    assert!(
        statistical_robustness(&tasks, 0, &ids, &eq, &fl, ok).is_err(),
        "seed 0"
    );
    let dup = vec!["A".to_string(), "A".to_string()];
    assert!(statistical_robustness(&tasks, 1, &dup, &eq, &fl, ok).is_err());
    let stranger = |_: &[usize]| {
        Ok(ResampleLayer {
            arms: vec![arm("Z", 0.2, 30.0, 0.95)],
            reference: Some(example_reference()),
        })
    };
    assert!(statistical_robustness(&tasks, 1, &ids, &eq, &fl, stranger).is_err());
    // A task of an arm that is not listed is an error, as is a blank arm.
    assert!(statistical_robustness(&[one("Z")], 1, &ids, &eq, &fl, ok).is_err());
    assert!(bootstrap_strata(&[one(" ")]).is_err());
    // No tasks: nothing is drawn, not an error.
    let r = statistical_robustness(&[], 1, &ids, &eq, &fl, |d| {
        assert!(d.is_empty());
        ok(d)
    })
    .unwrap();
    assert_eq!(r.arms[0].winner, 200);
    let failing = |_: &[usize]| -> agent_value_core::Result<ResampleLayer> {
        Err(CoreError::NoConvergence { what: "test" })
    };
    assert!(statistical_robustness(&tasks, 1, &ids, &eq, &fl, failing).is_err());
}

#[test]
fn xorshift_draws_cover_every_index() {
    let mut r = Xorshift64::new(12345).unwrap();
    let mut hits = [0u32; 5];
    for _ in 0..5000 {
        hits[r.below(5).unwrap() as usize] += 1;
    }
    assert!(hits.iter().all(|&h| (800..=1200).contains(&h)), "{hits:?}");
    assert!(Xorshift64::new(0).is_err());
}

#[test]
fn bootstrap_three_way_tie_counts_every_member() {
    let ids = vec!["A".to_string(), "B".to_string(), "C".to_string()];
    let tasks: Vec<BootstrapTask> = ["A", "B", "C"]
        .iter()
        .map(|a| BootstrapTask {
            arm: (*a).into(),
            cell: cell("fix"),
        })
        .collect();
    let r = statistical_robustness(
        &tasks,
        3,
        &ids,
        &Weights::equal(),
        &Floors::default(),
        |_| {
            Ok(ResampleLayer {
                arms: vec![
                    arm("A", 0.5, 60.0, 0.8),
                    arm("B", 0.5, 60.0, 0.8),
                    arm("C", 0.5, 60.0, 0.8),
                ],
                reference: Some(example_reference()),
            })
        },
    )
    .unwrap();
    assert_eq!((r.with_value, r.tied), (200, 200));
    let wins: u32 = r.arms.iter().map(|x| x.winner).sum();
    assert_eq!(wins, 600, "Σ|Win| = 3 × 200, not with_value + tied");
    assert!(r.arms.iter().all(|x| x.winner == 200 && x.frontier == 200));
}

#[test]
fn bootstrap_needs_arm_ids_and_skips_empty_resamples_for_winners() {
    let fl = Floors::default();
    let eq = Weights::equal();
    let empty: Vec<String> = Vec::new();
    let nothing = |_: &[usize]| {
        Ok(ResampleLayer {
            arms: Vec::new(),
            reference: Some(example_reference()),
        })
    };
    assert!(matches!(
        statistical_robustness(&[], 1, &empty, &eq, &fl, nothing),
        Err(CoreError::EmptyField { .. })
    ));
    let ids = vec!["A".to_string()];
    let r = statistical_robustness(&[], 1, &ids, &eq, &fl, nothing).unwrap();
    assert_eq!((r.resamples, r.with_value, r.tied), (200, 0, 0));
    assert_eq!((r.arms[0].winner, r.arms[0].frontier), (0, 0));
}

/// Low-V fixture for "always ahead" (DECISIONS 12.9). With V ≈ 0.02–0.06
/// the absolute 1e-12 tie tolerance on V is a log gap of ~1e-11, far above
/// SLOPE_TOL (1e-12) and far above the f64 noise of V (~1e-17 here), so
/// these cases do not depend on the last bit of libm.
fn low_v_reference() -> Point {
    Point {
        c: 0.05,
        t: 10.0,
        q: 0.99,
    }
}

/// B = (1e4 元, 1e6 s, quality 0); A = B with `ln((C+c0)/(C_B+c0)) = dc` and
/// `ln((T+t0)/(T_B+t0)) = dt`.
fn low_v_pair(dc: f64, dt: f64) -> Vec<SensitivityArm> {
    let fl = Floors::default();
    vec![
        arm(
            "A",
            (1e4 + fl.c0()) * dc.exp() - fl.c0(),
            (1e6 + fl.t0()) * dt.exp() - fl.t0(),
            0.0,
        ),
        arm("B", 1e4, 1e6, 0.0),
    ]
}

fn low_v_flip(arms: &[SensitivityArm], wk: Weights) -> FlipLine {
    flip_line(arms, low_v_reference(), &wk, &Floors::default())
        .unwrap()
        .unwrap()
}

fn low_v_gap(arms: &[SensitivityArm], wk: &Weights) -> f64 {
    let fl = Floors::default();
    let va = value_score(low_v_reference(), arms[0].point, wk, &fl).unwrap();
    let vb = value_score(low_v_reference(), arms[1].point, wk, &fl).unwrap();
    vb - va
}

fn dim_label(d: Dim) -> &'static str {
    match d {
        Dim::Cost => "成本",
        Dim::Time => "时间",
        Dim::Quality => "质量",
    }
}

/// B is not behind A anywhere on the segment of `st` (99 interior points).
fn assert_ahead_along(arms: &[SensitivityArm], st: &FlipStatement) {
    let free: Vec<Dim> = [Dim::Cost, Dim::Time, Dim::Quality]
        .into_iter()
        .filter(|d| *d != st.fixed)
        .collect();
    let s_free = 1.0 - st.fixed_weight;
    for k in 1..=99 {
        let u = 0.05 + (s_free - 0.10) * f64::from(k) / 100.0;
        let mut ws = [0.0; 3];
        for (d, x) in [
            (st.fixed, st.fixed_weight),
            (free[0], u),
            (free[1], s_free - u),
        ] {
            ws[d as usize] = x;
        }
        let ww = Weights::new(ws[0], ws[1], ws[2]).unwrap();
        assert!(low_v_gap(arms, &ww) >= -1e-12, "{:?} {u}", st.fixed);
    }
}

#[test]
fn runner_up_ahead_on_the_whole_segment_is_always() {
    // W_k has a weight at the 0.05 edge. A is made so that aΔ_C + bΔ_T is a
    // small positive number (~5e-12) at W_k, whose free weight sits on the
    // segment end: B is ahead by ~3e-13 in V (inside the 1e-12 tie
    // tolerance, ~40 000 ulp of V away from 0 and 3× away from the
    // tolerance), so A leads by id. The root lies just past that end
    // (beyond hi, or below lo), the "always" branch; the other free
    // direction crosses inside the segment at ratio ≈ 0.425.
    let cases = [
        // Quality and time fixed are "always"; cost fixed crosses.
        (
            w(0.05, 0.50, 0.45),
            (4e-10, -3e-11),
            [Dim::Time, Dim::Quality],
            (Dim::Cost, Dim::Time),
        ),
        // Mirror: quality and cost fixed are "always"; time fixed crosses.
        (
            w(0.50, 0.05, 0.45),
            (-3e-11, 4e-10),
            [Dim::Cost, Dim::Quality],
            (Dim::Time, Dim::Cost),
        ),
    ];
    for (wk, (dc, dt), always, crosses) in cases {
        let arms = low_v_pair(dc, dt);
        let gap = low_v_gap(&arms, &wk);
        // ≫ the f64 noise of V here (ulp ≈ 7e-18), and inside the tolerance.
        assert!(gap > 1e-14 && gap < 1e-12, "{gap:e}");
        let f = low_v_flip(&arms, wk);
        assert!(f.tied_now);
        assert_eq!((f.leader.as_str(), f.runner_up.as_str()), ("A", "B"));
        assert_eq!(f.statements.len(), 3);
        assert_eq!(
            f.statements
                .iter()
                .filter(|s| s.kind == FlipKind::Always)
                .count(),
            2
        );
        for st in &f.statements {
            if always.contains(&st.fixed) {
                assert_eq!(st.kind, FlipKind::Always, "{:?}", st);
                assert_eq!(
                    flip_sentence(&f, st),
                    format!(
                        "保持{}权重不变，B 在任何可行权重下都高于 A{ALWAYS_WITHIN_TIE_NOTE}；这是在当前估计值下的结论",
                        dim_label(st.fixed)
                    )
                );
                assert_ahead_along(&arms, st);
            } else {
                assert_eq!(st.fixed, crosses.0);
                let FlipKind::Crosses { over, under, ratio } = st.kind else {
                    panic!("{st:?}");
                };
                assert_eq!((over, under), (Dim::Quality, crosses.1));
                // Closed form with the exact Δ: root = 8.5e-12 / 3e-11, ratio
                // = root / (0.95 − root) = 0.425; Δ carries ~1e-16 absolute
                // error, i.e. ~1e-5 relative on the ratio.
                assert!((ratio - 0.425).abs() < 1e-4, "{ratio}");
                // No tie note on a crossing sentence, even when tied now.
                assert!(!flip_sentence(&f, st).contains(ALWAYS_WITHIN_TIE_NOTE));
            }
        }
    }
}

#[test]
fn parallel_segment_with_runner_up_a_hair_ahead_is_always() {
    // The tie tolerance is absolute (1e-12 on V), so at small V a log gap of
    // ~2e-11 is still "tied now" while being ≫ SLOPE_TOL. A = B with cost
    // and time × e^{3e-11}: with quality fixed, Δ_C ≈ Δ_T, the segment is
    // parallel to the line (slope ≈ 1.8e-15) and B is ahead on all of it.
    let fl = Floors::default();
    let b = arm("B", 1e4, 1e6, 0.0);
    let a = arm(
        "A",
        (1e4 + fl.c0()) * 3e-11_f64.exp() - fl.c0(),
        (1e6 + fl.t0()) * 3e-11_f64.exp() - fl.t0(),
        0.0,
    );
    let arms = vec![a, b];
    let wk = Weights::equal();
    let gap = low_v_gap(&arms, &wk);
    assert!(gap > 1e-14 && gap < 1e-12, "{gap:e}");
    let f = low_v_flip(&arms, wk);
    assert!(f.tied_now);
    assert_eq!((f.leader.as_str(), f.runner_up.as_str()), ("A", "B"));
    // Quality fixed: slope = Δ_T − Δ_C, within SLOPE_TOL (1e-12), so the
    // parallel branch is the one taken.
    assert!((f.delta[0] - f.delta[1]).abs() <= 1e-12, "{:?}", f.delta);
    for st in &f.statements {
        assert_eq!(st.kind, FlipKind::Always, "{st:?}");
        assert!(flip_sentence(&f, st).contains(ALWAYS_WITHIN_TIE_NOTE));
        assert_ahead_along(&arms, st);
    }
}

#[test]
fn tie_note_only_when_the_gap_is_within_the_tolerance() {
    // Outside the tolerance: B behind A by ~1.2e-11 in V. Not tied, no
    // sentence carries the note.
    let arms = low_v_pair(3e-11, -4e-10);
    let wk = w(0.05, 0.50, 0.45);
    let f = low_v_flip(&arms, wk);
    assert!(!f.tied_now);
    for st in &f.statements {
        assert!(!flip_sentence(&f, st).contains(ALWAYS_WITHIN_TIE_NOTE));
    }
    // The note follows `tied_now`, not the kind alone: the same "always"
    // statement without a tie (as a caller could build it) has no note,
    // and with a tie it has the note.
    let st = FlipStatement {
        fixed: Dim::Quality,
        fixed_weight: 0.45,
        kind: FlipKind::Always,
    };
    let mut line = f.clone();
    line.statements = vec![st.clone()];
    assert_eq!(
        flip_sentence(&line, &st),
        format!(
            "保持质量权重不变，{} 在任何可行权重下都高于 {}；这是在当前估计值下的结论",
            line.runner_up, line.leader
        )
    );
    line.tied_now = true;
    assert_eq!(
        flip_sentence(&line, &st),
        format!(
            "保持质量权重不变，{} 在任何可行权重下都高于 {}（当前差距在并列容差内，按 id 排序）；这是在当前估计值下的结论",
            line.runner_up, line.leader
        )
    );
    // Other kinds never carry it, tied or not.
    for kind in [
        FlipKind::Never,
        FlipKind::Tied,
        FlipKind::Crosses {
            over: Dim::Cost,
            under: Dim::Time,
            ratio: 1.5,
        },
    ] {
        let st = FlipStatement { kind, ..st.clone() };
        assert!(!flip_sentence(&line, &st).contains(ALWAYS_WITHIN_TIE_NOTE));
    }
}

/// Runs the bootstrap on `tasks` and returns every resample's draw.
fn draws_of(tasks: &[BootstrapTask], seed: u64, arm_ids: &[String]) -> Vec<Vec<usize>> {
    let mut draws = Vec::new();
    statistical_robustness(
        tasks,
        seed,
        arm_ids,
        &Weights::equal(),
        &Floors::default(),
        |d| {
            draws.push(d.to_vec());
            Ok(ResampleLayer {
                arms: Vec::new(),
                reference: None,
            })
        },
    )
    .unwrap();
    draws
}

/// For each stratum (as written out by the test), whether some resample
/// drew a repeated task in it (an untouched copy never repeats), and
/// whether two seeds give different draws in it.
fn assert_every_stratum_is_resampled(
    tasks: &[BootstrapTask],
    arm_ids: &[String],
    strata: &[Vec<usize>],
) {
    let a = draws_of(tasks, 20261020, arm_ids);
    let b = draws_of(tasks, 7, arm_ids);
    assert_eq!(a.len(), 200);
    let mut at = 0;
    for st in strata {
        let n = st.len();
        let slices = |d: &Vec<Vec<usize>>| -> Vec<Vec<usize>> {
            d.iter().map(|x| x[at..at + n].to_vec()).collect()
        };
        let (sa, sb) = (slices(&a), slices(&b));
        for x in sa.iter().chain(&sb) {
            assert!(x.iter().all(|k| st.contains(k)), "{x:?} outside {st:?}");
        }
        // A single-task stratum can never repeat; the checks below need n ≥ 2.
        assert!(n >= 2, "fixture strata must have at least two tasks");
        let repeated = sa.iter().any(|x| {
            let mut y = x.clone();
            y.sort_unstable();
            y.dedup();
            y.len() < n
        });
        assert!(repeated, "stratum {st:?} never resampled: {:?}", &sa[..3]);
        // Resampled in EVERY round, not just some: count the rounds with a
        // repeated task. Per round P(repeat) = 1 − n!/nⁿ (1/2 for n = 2,
        // 7/9 for n = 3), so over 200 rounds the count is Binomial(200, p):
        // mean 100 / 155.6. Bounds 70 / 120 fail for a correct draw with
        // probability 6.9e-6 / 5.2e-9 per stratum and seed (exact binomial
        // tail). A stratum copied untouched in half the rounds never repeats
        // there, so its count is Binomial(100, p): mean 50 / 77.8, at most
        // 100; above the bound with probability 3.9e-5 / 0.
        for (seed, slices) in [("a", &sa), ("b", &sb)] {
            let reps = slices
                .iter()
                .filter(|x| {
                    let mut y = (*x).clone();
                    y.sort_unstable();
                    y.dedup();
                    y.len() < n
                })
                .count();
            assert_eq!(slices.len(), 200);
            let bound = match n {
                2 => 70,
                3 => 120,
                _ => panic!("fixture strata have 2 or 3 tasks"),
            };
            assert!(reps >= bound, "stratum {st:?} seed {seed}: {reps} repeats");
        }
        assert!(sa.iter().any(|x| x != st), "stratum {st:?} always copied");
        assert_ne!(sa, sb, "stratum {st:?} ignores the seed");
        at += n;
    }
    assert_eq!(at, tasks.len());
}

#[test]
fn every_stratum_is_actually_resampled() {
    let fx = fixture();
    let strata = vec![vec![0, 1], vec![2, 3, 4], vec![5, 6], vec![7, 8]];
    assert_eq!(bootstrap_strata(&fx.tasks).unwrap(), strata);
    assert_every_stratum_is_resampled(&fx.tasks, &ids(), &strata);
}

#[test]
fn reference_and_fallback_arms_are_resampled_like_the_others() {
    // The core does not know which arm is R or F; they are ordinary arms
    // in arm_ids. Put R first and F last in arm_ids (a frozen first or last
    // arm would show), and give each a 2- and a 3-task cell.
    let mut tasks = Vec::new();
    for (a, c, n) in [
        ("R", "fix", 2),
        ("R", "new", 3),
        ("X", "fix", 2),
        ("X", "new", 2),
        ("F", "fix", 3),
        ("F", "new", 2),
    ] {
        for _ in 0..n {
            tasks.push(BootstrapTask {
                arm: a.into(),
                cell: cell(c),
            });
        }
    }
    let arm_ids = vec!["R".to_string(), "X".to_string(), "F".to_string()];
    // Key order: F < R < X.
    let strata = vec![
        vec![9, 10, 11],
        vec![12, 13],
        vec![0, 1],
        vec![2, 3, 4],
        vec![5, 6],
        vec![7, 8],
    ];
    assert_eq!(bootstrap_strata(&tasks).unwrap(), strata);
    assert_every_stratum_is_resampled(&tasks, &arm_ids, &strata);
}

#[test]
fn tie_tolerance_is_pinned_at_1e_12_on_both_sides() {
    // Low-V pair (V ≈ 0.0245, ulp ≈ 3.5e-18), weights 1/3: A = B with cost
    // and time × e^{d}. The gaps are ≈8.17e-13 and ≈1.143e-12; the windows
    // asserted below keep both ≥ 4e4 ulp from the 1e-12 tolerance, so no
    // libm last-bit dependence. A 2e-12 or 5e-13 tolerance would flip one of
    // them. A gap of exactly 1e-12 is deliberately not tested (DECISIONS
    // 12.9: either side is acceptable).
    let wk = Weights::equal();
    // d = 5e-11: gap ≈ 8.2e-13, inside → tied, A first by id, B "always"
    // ahead with the note.
    let inside = low_v_pair(5e-11, 5e-11);
    let gap = low_v_gap(&inside, &wk);
    assert!(gap > 8.0e-13 && gap < 8.5e-13, "{gap:e}");
    let f = low_v_flip(&inside, wk);
    assert!(f.tied_now);
    let fixed: Vec<Dim> = f.statements.iter().map(|st| st.fixed).collect();
    assert_eq!(fixed, [Dim::Cost, Dim::Time, Dim::Quality]);
    assert_eq!((f.leader.as_str(), f.runner_up.as_str()), ("A", "B"));
    for st in &f.statements {
        assert_eq!(st.kind, FlipKind::Always, "{st:?}");
        assert_eq!(
            flip_sentence(&f, st),
            format!(
                "保持{}权重不变，B 在任何可行权重下都高于 A{ALWAYS_WITHIN_TIE_NOTE}；{CURRENT_ESTIMATE_NOTE}",
                dim_label(st.fixed)
            )
        );
    }
    // d = 7e-11: gap ≈ 1.14e-12, outside → not tied, B leads, A never
    // overtakes, no note.
    let outside = low_v_pair(7e-11, 7e-11);
    let gap = low_v_gap(&outside, &wk);
    assert!(gap > 1.14e-12 && gap < 1.2e-12, "{gap:e}");
    let f = low_v_flip(&outside, wk);
    assert!(!f.tied_now);
    let fixed: Vec<Dim> = f.statements.iter().map(|st| st.fixed).collect();
    assert_eq!(fixed, [Dim::Cost, Dim::Time, Dim::Quality]);
    assert_eq!((f.leader.as_str(), f.runner_up.as_str()), ("B", "A"));
    for st in &f.statements {
        assert_eq!(st.kind, FlipKind::Never, "{st:?}");
        assert_eq!(
            flip_sentence(&f, st),
            format!(
                "保持{}权重不变，A 在任何可行权重下都不会超过 B；{CURRENT_ESTIMATE_NOTE}",
                dim_label(st.fixed)
            )
        );
    }
}

// Generated outside the repo by logs/pr12/bootstrap_golden.py (engineering
// log folder; an independent Python implementation written from DECISIONS
// 12.6, not from this code), seed 20261021, 200 rounds, the 7 tasks of
// `bootstrap_golden_draws_match_independent_reference`. Strata (arm, cell)
// in key order: X/fix [2, 5], X/new [1, 3, 6], Y/fix [0, 4].
const GOLDEN_DRAWS: [[usize; 7]; 200] = [
    [2, 5, 6, 1, 6, 4, 4],
    [2, 5, 3, 6, 6, 0, 0],
    [2, 5, 1, 6, 3, 4, 0],
    [5, 2, 3, 1, 6, 0, 4],
    [2, 2, 6, 3, 1, 0, 0],
    [2, 5, 3, 6, 3, 0, 0],
    [5, 5, 1, 3, 3, 4, 0],
    [2, 2, 6, 6, 3, 0, 0],
    [2, 2, 6, 1, 3, 4, 4],
    [2, 5, 1, 1, 1, 4, 4],
    [2, 5, 3, 3, 3, 0, 4],
    [2, 5, 3, 1, 6, 0, 4],
    [2, 2, 1, 6, 6, 4, 4],
    [5, 2, 6, 3, 3, 4, 4],
    [2, 2, 1, 1, 3, 4, 0],
    [5, 5, 6, 3, 3, 4, 4],
    [5, 5, 3, 1, 3, 4, 0],
    [5, 5, 6, 6, 1, 4, 4],
    [5, 2, 1, 6, 6, 0, 4],
    [5, 2, 3, 6, 6, 0, 0],
    [2, 5, 3, 6, 1, 0, 0],
    [5, 2, 3, 1, 3, 0, 0],
    [2, 2, 6, 3, 1, 0, 4],
    [5, 2, 3, 1, 1, 4, 0],
    [2, 5, 6, 3, 3, 4, 4],
    [5, 2, 3, 1, 3, 0, 0],
    [5, 2, 3, 1, 3, 4, 0],
    [2, 5, 1, 3, 1, 4, 0],
    [5, 5, 6, 3, 6, 0, 0],
    [2, 2, 6, 1, 6, 4, 0],
    [2, 2, 1, 3, 3, 4, 0],
    [5, 5, 1, 6, 6, 0, 4],
    [2, 5, 3, 3, 1, 4, 4],
    [2, 2, 1, 1, 1, 0, 4],
    [5, 5, 1, 1, 1, 0, 0],
    [2, 2, 3, 1, 1, 0, 4],
    [5, 5, 3, 6, 6, 0, 0],
    [5, 2, 3, 3, 3, 0, 4],
    [2, 5, 6, 6, 1, 0, 4],
    [5, 5, 1, 1, 6, 4, 4],
    [2, 5, 3, 1, 3, 0, 0],
    [2, 5, 6, 1, 1, 0, 4],
    [2, 5, 1, 3, 3, 4, 0],
    [5, 2, 6, 6, 1, 0, 4],
    [2, 2, 6, 6, 6, 4, 0],
    [5, 5, 1, 3, 1, 0, 4],
    [2, 2, 6, 6, 6, 4, 4],
    [2, 5, 1, 6, 1, 0, 0],
    [5, 2, 6, 6, 1, 0, 0],
    [2, 5, 1, 1, 3, 4, 0],
    [2, 2, 6, 3, 6, 4, 4],
    [5, 2, 1, 1, 3, 4, 0],
    [5, 2, 6, 6, 6, 4, 0],
    [2, 2, 3, 1, 3, 0, 4],
    [5, 5, 1, 3, 6, 0, 4],
    [2, 2, 1, 1, 3, 0, 0],
    [2, 2, 1, 6, 3, 0, 0],
    [5, 5, 1, 6, 6, 0, 0],
    [2, 2, 6, 1, 3, 0, 4],
    [2, 2, 1, 6, 3, 0, 4],
    [2, 5, 1, 3, 1, 0, 0],
    [5, 2, 3, 3, 1, 4, 0],
    [5, 5, 6, 1, 1, 4, 0],
    [5, 2, 3, 6, 1, 0, 0],
    [2, 2, 6, 3, 6, 4, 0],
    [2, 5, 6, 6, 1, 4, 4],
    [5, 5, 1, 6, 6, 4, 0],
    [2, 5, 6, 6, 1, 0, 4],
    [5, 2, 3, 6, 1, 0, 0],
    [2, 2, 3, 1, 1, 0, 0],
    [5, 5, 3, 6, 6, 0, 4],
    [5, 2, 3, 3, 3, 4, 0],
    [5, 2, 6, 3, 3, 4, 0],
    [5, 5, 6, 3, 1, 4, 4],
    [2, 5, 1, 6, 3, 0, 4],
    [2, 5, 3, 6, 6, 4, 4],
    [5, 2, 1, 6, 6, 4, 4],
    [2, 2, 1, 3, 1, 4, 4],
    [2, 2, 1, 3, 3, 0, 4],
    [2, 5, 1, 3, 3, 4, 4],
    [5, 5, 6, 6, 1, 4, 4],
    [2, 5, 1, 1, 3, 0, 0],
    [2, 2, 6, 1, 3, 4, 4],
    [2, 2, 3, 6, 3, 0, 0],
    [2, 2, 6, 1, 3, 4, 4],
    [2, 2, 1, 1, 6, 0, 4],
    [5, 2, 1, 1, 1, 0, 0],
    [2, 2, 3, 3, 1, 0, 4],
    [2, 5, 1, 1, 3, 4, 0],
    [2, 2, 6, 3, 1, 4, 0],
    [5, 2, 3, 1, 1, 0, 0],
    [2, 5, 1, 6, 1, 0, 4],
    [5, 5, 1, 3, 6, 0, 4],
    [2, 5, 6, 6, 1, 0, 4],
    [5, 5, 1, 3, 1, 0, 4],
    [2, 2, 3, 1, 1, 4, 4],
    [2, 2, 1, 6, 3, 0, 0],
    [2, 5, 1, 1, 1, 4, 4],
    [5, 2, 1, 1, 1, 0, 0],
    [5, 5, 6, 6, 1, 0, 0],
    [2, 2, 3, 3, 1, 0, 4],
    [2, 5, 6, 6, 3, 4, 4],
    [2, 2, 6, 6, 6, 0, 4],
    [5, 5, 3, 1, 1, 4, 0],
    [5, 2, 1, 6, 6, 0, 0],
    [2, 5, 6, 6, 1, 4, 0],
    [2, 5, 3, 1, 6, 0, 4],
    [2, 5, 3, 6, 6, 0, 4],
    [2, 5, 1, 6, 3, 0, 4],
    [5, 2, 1, 1, 6, 4, 0],
    [5, 5, 3, 1, 1, 0, 4],
    [5, 2, 3, 3, 3, 0, 0],
    [5, 2, 6, 6, 6, 0, 4],
    [2, 5, 1, 3, 6, 4, 4],
    [2, 5, 6, 6, 3, 4, 4],
    [5, 5, 1, 3, 6, 4, 4],
    [2, 5, 1, 6, 3, 0, 0],
    [5, 2, 3, 1, 3, 0, 0],
    [2, 5, 3, 1, 3, 4, 4],
    [5, 2, 1, 3, 1, 0, 0],
    [5, 2, 6, 3, 3, 4, 4],
    [2, 2, 1, 6, 6, 4, 4],
    [5, 5, 3, 6, 3, 0, 0],
    [2, 2, 3, 6, 6, 4, 0],
    [2, 5, 1, 3, 6, 4, 0],
    [5, 2, 6, 6, 1, 0, 0],
    [2, 2, 1, 6, 1, 0, 4],
    [2, 2, 6, 6, 3, 4, 4],
    [2, 5, 1, 3, 6, 4, 4],
    [2, 2, 3, 6, 6, 4, 0],
    [5, 2, 3, 3, 6, 0, 4],
    [2, 2, 6, 1, 1, 0, 4],
    [2, 5, 1, 6, 3, 4, 4],
    [5, 5, 1, 6, 1, 0, 4],
    [5, 2, 6, 3, 6, 0, 0],
    [5, 2, 1, 3, 1, 4, 4],
    [5, 2, 3, 1, 1, 4, 4],
    [2, 5, 1, 6, 3, 4, 0],
    [5, 2, 3, 6, 3, 4, 0],
    [2, 5, 1, 3, 1, 4, 0],
    [2, 2, 6, 1, 1, 4, 4],
    [5, 2, 6, 3, 6, 0, 0],
    [5, 5, 3, 1, 1, 4, 4],
    [2, 2, 6, 3, 1, 4, 0],
    [5, 5, 1, 3, 1, 4, 4],
    [2, 2, 1, 3, 6, 4, 0],
    [2, 5, 6, 6, 3, 4, 0],
    [5, 2, 3, 3, 3, 4, 4],
    [2, 2, 3, 6, 6, 0, 4],
    [2, 5, 3, 3, 3, 0, 4],
    [2, 2, 1, 6, 6, 4, 0],
    [2, 2, 6, 1, 3, 0, 0],
    [2, 5, 3, 1, 1, 0, 0],
    [2, 5, 1, 3, 1, 4, 0],
    [2, 5, 3, 3, 3, 0, 0],
    [5, 5, 3, 6, 3, 4, 4],
    [2, 5, 6, 1, 6, 0, 0],
    [2, 5, 6, 1, 6, 4, 4],
    [2, 5, 3, 6, 6, 4, 0],
    [5, 5, 1, 3, 3, 4, 4],
    [5, 2, 6, 1, 1, 4, 0],
    [5, 2, 3, 1, 6, 0, 0],
    [5, 2, 3, 6, 6, 0, 4],
    [5, 5, 3, 3, 6, 4, 4],
    [5, 2, 3, 3, 6, 0, 4],
    [2, 2, 3, 1, 3, 4, 0],
    [2, 2, 6, 1, 6, 4, 0],
    [5, 2, 3, 1, 1, 0, 4],
    [2, 2, 6, 3, 6, 0, 4],
    [5, 5, 3, 1, 1, 0, 4],
    [2, 2, 3, 1, 1, 4, 4],
    [5, 5, 6, 3, 1, 0, 4],
    [5, 5, 1, 3, 1, 4, 4],
    [2, 2, 6, 6, 6, 0, 0],
    [5, 2, 1, 6, 3, 4, 0],
    [5, 2, 6, 3, 1, 4, 4],
    [5, 2, 3, 6, 6, 0, 4],
    [2, 2, 3, 3, 1, 0, 4],
    [5, 5, 6, 6, 1, 4, 0],
    [5, 5, 6, 3, 3, 4, 4],
    [5, 5, 1, 1, 1, 0, 0],
    [5, 2, 3, 3, 3, 4, 0],
    [5, 2, 1, 3, 1, 4, 0],
    [5, 5, 6, 6, 3, 4, 0],
    [2, 2, 1, 1, 3, 4, 0],
    [2, 5, 3, 6, 3, 4, 0],
    [5, 5, 1, 1, 1, 4, 0],
    [2, 2, 1, 6, 3, 0, 0],
    [2, 2, 3, 3, 6, 4, 0],
    [2, 5, 6, 3, 6, 0, 0],
    [2, 2, 6, 1, 1, 4, 4],
    [5, 5, 1, 6, 3, 4, 0],
    [2, 2, 1, 6, 6, 0, 0],
    [5, 5, 3, 6, 3, 0, 4],
    [2, 2, 1, 3, 3, 4, 4],
    [2, 2, 3, 1, 1, 0, 4],
    [5, 5, 1, 6, 1, 4, 0],
    [2, 5, 3, 6, 1, 0, 4],
    [2, 2, 3, 6, 3, 0, 4],
    [2, 2, 1, 3, 1, 0, 4],
];

#[test]
fn bootstrap_golden_draws_match_independent_reference() {
    // Every round, every stratum, exactly: a stratum left untouched in any
    // single round (the first, the last, every k-th) changes that round's
    // positions and, because it skips draws, every later round too.
    let tasks: Vec<BootstrapTask> = [
        ("Y", "fix"),
        ("X", "new"),
        ("X", "fix"),
        ("X", "new"),
        ("Y", "fix"),
        ("X", "fix"),
        ("X", "new"),
    ]
    .iter()
    .map(|(a, c)| BootstrapTask {
        arm: (*a).into(),
        cell: cell(c),
    })
    .collect();
    assert_eq!(
        bootstrap_strata(&tasks).unwrap(),
        vec![vec![2, 5], vec![1, 3, 6], vec![0, 4]]
    );
    let draws = draws_of(&tasks, 20261021, &ids());
    assert_eq!(draws.len(), GOLDEN_DRAWS.len());
    for (round, (got, want)) in draws.iter().zip(GOLDEN_DRAWS.iter()).enumerate() {
        assert_eq!(got.as_slice(), want.as_slice(), "round {round}");
    }
    // Spot checks named in the review: rounds 0, 1 and 199.
    assert_eq!(draws[0], vec![2, 5, 6, 1, 6, 4, 4]);
    assert_eq!(draws[1], vec![2, 5, 3, 6, 6, 0, 0]);
    assert_eq!(draws[199], vec![2, 2, 1, 3, 1, 0, 4]);
}

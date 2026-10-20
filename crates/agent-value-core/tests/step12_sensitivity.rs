//! Step 12 (v22 §12; DECISIONS K and the step-12 section): winner map,
//! weight-robust winner, statistical robustness (task bootstrap), flip
//! line and noise-floor robustness.
//! Expected values come from the spec text, `scripts/v22_examples.py`
//! (output `review-2026-10-19/v22_examples_output.txt` lines 36–38) and
//! `scripts/elicit5.py`; never fitted.

use agent_value_core::cell_values::Triple;
use agent_value_core::constants::{BOOTSTRAP_RESAMPLES, GRID_POINTS};
use agent_value_core::error::CoreError;
use agent_value_core::frontier::{layer_frontiers, FrontierArm};
use agent_value_core::sensitivity::{
    flip_line, flip_sentence, grid_points, printed_ratio, sensitivity, statistical_robustness,
    winners, Dim, FlipKind, FlipLine, FloorVariant, NoFlipLine, ResampleLayer, RobustRegion,
    SensitivityArm, SensitivityInput, SensitivityReport, Withheld, Xorshift64,
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
        for (v, kk) in [p.a(), p.b(), p.g()].iter().zip(&k) {
            close(*v, *kk as f64 / 20.0);
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
    i.omega_b = Some(&empty);
    assert!(sensitivity(&i).is_err());
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

// ---- statistical robustness (task bootstrap) ----

/// Tasks of two arms: arm 0 = stratum 0, arm 1 = stratum 1. Each task is a
/// (cost, time, quality) observation; the resample triple is the mean.
fn tasks() -> [Vec<(f64, f64, f64)>; 2] {
    [
        vec![
            (0.2, 100.0, 0.80),
            (0.4, 140.0, 0.80),
            (0.3, 120.0, 0.85),
            (0.3, 120.0, 0.75),
        ],
        vec![
            (0.9, 50.0, 0.90),
            (0.8, 70.0, 0.80),
            (1.0, 60.0, 0.85),
            (0.9, 60.0, 0.85),
        ],
    ]
}

fn mean(obs: &[(f64, f64, f64)], idx: &[usize]) -> Point {
    let n = idx.len() as f64;
    let (mut c, mut t, mut q) = (0.0, 0.0, 0.0);
    for &k in idx {
        c += obs[k].0;
        t += obs[k].1;
        q += obs[k].2;
    }
    Point {
        c: c / n,
        t: t / n,
        q: q / n,
    }
}

fn run_bootstrap(seed: u64) -> agent_value_core::sensitivity::StatisticalRobustness {
    let obs = tasks();
    let ids = vec!["X".to_string(), "Y".to_string()];
    statistical_robustness(
        &[4, 4],
        seed,
        &ids,
        &Weights::equal(),
        &Floors::default(),
        |draw| {
            assert_eq!(draw.len(), 2);
            for d in draw {
                assert_eq!(d.len(), 4);
                assert!(d.iter().all(|&k| k < 4));
            }
            Ok(ResampleLayer {
                arms: vec![
                    SensitivityArm {
                        id: "X".into(),
                        point: mean(&obs[0], &draw[0]),
                    },
                    SensitivityArm {
                        id: "Y".into(),
                        point: mean(&obs[1], &draw[1]),
                    },
                ],
                reference: Some(example_reference()),
            })
        },
    )
    .unwrap()
}

#[test]
fn bootstrap_runs_200_resamples_and_is_reproducible_by_seed() {
    let a = run_bootstrap(20261020);
    let b = run_bootstrap(20261020);
    assert_eq!(a, b);
    assert_eq!(a.seed, 20261020);
    assert_eq!(a.resamples, BOOTSTRAP_RESAMPLES);
    assert_eq!(a.resamples, 200);
    assert_eq!(a.with_value, 200);
    let wins: u32 = a.arms.iter().map(|x| x.winner).sum();
    assert_eq!(wins, 200 + a.tied);
    // Both arms trade off cost against time in every resample.
    assert!(a.arms.iter().all(|x| x.frontier == 200));
    // The bootstrap is not degenerate: both arms win sometimes.
    assert!(
        a.arms.iter().all(|x| x.winner > 0 && x.winner < 200),
        "{a:?}"
    );
    let c = run_bootstrap(7);
    assert_eq!(c.seed, 7);
    assert_ne!(a, c);
}

#[test]
fn bootstrap_counts_frontier_without_anchor_and_dominant_wins_all() {
    let ids = vec!["A".to_string(), "B".to_string()];
    let mut calls = 0u32;
    let mut with_anchor = false;
    let r = statistical_robustness(&[3], 1, &ids, &Weights::equal(), &Floors::default(), |_| {
        calls += 1;
        with_anchor = !with_anchor;
        Ok(ResampleLayer {
            arms: vec![arm("A", 0.2, 30.0, 0.95), arm("B", 0.5, 60.0, 0.80)],
            reference: with_anchor.then(example_reference),
        })
    })
    .unwrap();
    assert_eq!(calls, 200);
    assert_eq!(r.with_value, 100);
    assert_eq!((r.arms[0].winner, r.arms[0].frontier), (100, 200));
    assert_eq!((r.arms[1].winner, r.arms[1].frontier), (0, 0));
}

#[test]
fn bootstrap_rejects_bad_inputs() {
    let ids = vec!["A".to_string()];
    let ok = |_: &[Vec<usize>]| {
        Ok(ResampleLayer {
            arms: vec![arm("A", 0.2, 30.0, 0.95)],
            reference: Some(example_reference()),
        })
    };
    let eq = Weights::equal();
    let fl = Floors::default();
    assert!(
        statistical_robustness(&[2], 0, &ids, &eq, &fl, ok).is_err(),
        "seed 0"
    );
    let dup = vec!["A".to_string(), "A".to_string()];
    assert!(statistical_robustness(&[2], 1, &dup, &eq, &fl, ok).is_err());
    let stranger = |_: &[Vec<usize>]| {
        Ok(ResampleLayer {
            arms: vec![arm("Z", 0.2, 30.0, 0.95)],
            reference: Some(example_reference()),
        })
    };
    assert!(statistical_robustness(&[2], 1, &ids, &eq, &fl, stranger).is_err());
    // An empty stratum draws nothing and is not an error.
    let r = statistical_robustness(&[0, 2], 1, &ids, &eq, &fl, |d| {
        assert!(d[0].is_empty());
        assert_eq!(d[1].len(), 2);
        ok(d)
    })
    .unwrap();
    assert_eq!(r.arms[0].winner, 200);
    // Errors from the caller's recomputation propagate.
    let failing = |_: &[Vec<usize>]| -> agent_value_core::Result<ResampleLayer> {
        Err(CoreError::NoConvergence { what: "test" })
    };
    assert!(statistical_robustness(&[2], 1, &ids, &eq, &fl, failing).is_err());
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
    let r = statistical_robustness(&[2], 3, &ids, &Weights::equal(), &Floors::default(), |_| {
        Ok(ResampleLayer {
            arms: vec![
                arm("A", 0.5, 60.0, 0.8),
                arm("B", 0.5, 60.0, 0.8),
                arm("C", 0.5, 60.0, 0.8),
            ],
            reference: Some(example_reference()),
        })
    })
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
    let nothing = |_: &[Vec<usize>]| {
        Ok(ResampleLayer {
            arms: Vec::new(),
            reference: Some(example_reference()),
        })
    };
    assert!(matches!(
        statistical_robustness(&[1], 1, &empty, &eq, &fl, nothing),
        Err(CoreError::EmptyField { .. })
    ));
    let ids = vec!["A".to_string()];
    let r = statistical_robustness(&[1], 1, &ids, &eq, &fl, nothing).unwrap();
    assert_eq!((r.resamples, r.with_value, r.tied), (200, 0, 0));
    assert_eq!((r.arms[0].winner, r.arms[0].frontier), (0, 0));
}

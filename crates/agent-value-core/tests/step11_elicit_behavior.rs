//! Step 11: five-level self-report (§10.2), behaviour calibration (§10.3),
//! merge rule (§10.4), and the same-id fallback validation (DECISIONS G11).
//! Expected values come from v22 §10.2, HANDOFF step 11 and the reference
//! script output frozen under `tests/fixtures/step11_*`.

use agent_value_core::behavior::{
    bootstrap_unstable, calibrate, check_windows, profile_loglik, Behavior, ChoiceEvent,
    EventOption, NoSignalReason, SplitMix64, SUPPORT_DROP,
};
use agent_value_core::cells::{CellId, CellWeights};
use agent_value_core::constants::{BOOTSTRAP_RESAMPLES, C0_YUAN, EPSILON, T0_SECONDS};
use agent_value_core::elicit::{
    band, bank, level, pick, regions, self_report, should_stop, show, Answer, QuestionBase,
    Session, BANK_SIZE,
};
use agent_value_core::error::CoreError;
use agent_value_core::grid::{grid, nearest_to_centroid, GridPoint};
use agent_value_core::suggest::{merge, Suggestion};
use agent_value_core::value::Point;
use agent_value_core::weights::FiveLevel;
use agent_value_core::window::WindowIndex;

const BANK_CSV: &str = include_str!("fixtures/step11_bank.csv");
const LEVELS: &str = include_str!("fixtures/step11_levels.txt");
const RUNS_CSV: &str = include_str!("fixtures/step11_honest_runs.csv");
const BOUNDARY_CSV: &str = include_str!("fixtures/step11_boundary_528.csv");

fn gp(i: u8, j: u8) -> GridPoint {
    GridPoint::new(i, j).unwrap()
}

fn parse_point(s: &str) -> GridPoint {
    let (i, j) = s.split_once('-').unwrap();
    gp(i.parse().unwrap(), j.parse().unwrap())
}

fn five(l: i8) -> FiveLevel {
    FiveLevel::from_level(l).unwrap()
}

fn ans(pairs: &[(usize, i8)]) -> Vec<Answer> {
    pairs
        .iter()
        .map(|&(question, l)| Answer {
            question,
            answer: five(l),
        })
        .collect()
}

/// Honest session for a true weight: answers what the true weight predicts.
fn honest_session(truth: GridPoint) -> Session {
    let b = bank();
    let mut s = Session::new();
    while let Some(q) = s.next_question() {
        s.answer(q, five(level(truth, &b[q]))).unwrap();
    }
    s
}

// ---- 1. grid ----

#[test]
fn grid_has_171_exact_points() {
    let g = grid();
    assert_eq!(g.len(), 171);
    for p in &g {
        let (i, j, k) = p.twentieths();
        assert!(i >= 1 && j >= 1 && k >= 1);
        assert_eq!(u16::from(i) + u16::from(j) + u16::from(k), 20);
        assert_eq!(p.a(), f64::from(i) / 20.0);
        assert_eq!(p.b(), f64::from(j) / 20.0);
        assert_eq!(p.g(), f64::from(k) / 20.0);
        assert_eq!(p.as_array(), [p.a(), p.b(), p.g()]);
    }
    assert!(GridPoint::new(0, 5).is_none());
    assert!(GridPoint::new(10, 10).is_none());
}

// ---- 2. bank ----

#[test]
fn bank_matches_reference_order_and_values() {
    let b = bank();
    assert_eq!(b.len(), BANK_SIZE);
    let rows: Vec<&str> = BANK_CSV.lines().skip(1).collect();
    assert_eq!(rows.len(), 96);
    for (n, row) in rows.iter().enumerate() {
        let f: Vec<&str> = row.split(',').collect();
        assert_eq!(f[0].parse::<usize>().unwrap(), n);
        for k in 0..3 {
            // Bit-exact: ratios are built as 1.0 / x, like the reference.
            assert_eq!(
                b[n].rho()[k],
                f[k + 1].parse::<f64>().unwrap(),
                "q{n} dim {k}"
            );
        }
    }
    assert_eq!(b[0].rho(), [1.3, 1.0 / 1.3, 1.0]);
}

// ---- 3. G1 banding ----

#[test]
fn every_level_matches_reference_table() {
    let b = bank();
    let g = grid();
    let lines: Vec<&str> = LEVELS.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(lines.len(), 96);
    for (qi, line) in lines.iter().enumerate() {
        for (wi, ch) in line.chars().enumerate() {
            let want = ch.to_digit(10).unwrap() as i8 - 2;
            assert_eq!(level(g[wi], &b[qi]), want, "q{qi} w{:?}", g[wi]);
        }
    }
}

#[test]
fn all_528_boundary_pairs_match_reference_levels() {
    let b = bank();
    let mut n = 0;
    let mut would_move = 0;
    for row in BOUNDARY_CSV.lines().skip(1) {
        // i,j,bank_idx,"(rho)",u_rounded,u_raw,level_expected,level_if_unrounded
        let (head, tail) = row.split_once(",\"").unwrap();
        let h: Vec<&str> = head.split(',').collect();
        let t: Vec<&str> = tail.split_once("\",").unwrap().1.split(',').collect();
        let w = gp(h[0].parse().unwrap(), h[1].parse().unwrap());
        let q = &b[h[2].parse::<usize>().unwrap()];
        let expected: i8 = t[2].parse().unwrap();
        let unrounded: i8 = t[3].parse().unwrap();
        let u: f64 = t[0].parse().unwrap();
        assert!(u.abs() == 0.25 || u.abs() == 0.5);
        assert_eq!(level(w, q), expected, "{row}");
        n += 1;
        would_move += usize::from(expected != unrounded);
    }
    assert_eq!(n, 528);
    assert_eq!(would_move, 150);
}

#[test]
fn band_rounds_to_nine_decimals_before_closed_bands() {
    // Within 1e-9 of a cut point: on the cut point, closed band.
    assert_eq!(band(0.25 + 1e-10), 0);
    assert_eq!(band(-0.25 - 1e-10), 0);
    assert_eq!(band(0.5 + 1e-10), 1);
    // 4e-7 past the cut point is a real difference (coarser rounding, e.g.
    // to 1e-6, would wrongly pull it back onto the cut point).
    assert_eq!(band(0.2500004), 1);
    assert_eq!(band(-0.5000004), -2);
    assert_eq!(band(0.0), 0);
    assert_eq!(band(0.26), 1);
    assert_eq!(band(0.51), 2);
}

#[test]
fn equal_ratio_question_on_exact_cut_points() {
    let b = bank();
    // q20 = (2, 1/2, 1): u = b - a exactly.
    assert_eq!(b[20].rho(), [2.0, 0.5, 1.0]);
    // b - a = 0.25 (raw u is 0.25000000000000006): "about the same".
    assert_eq!(level(gp(7, 12), &b[20]), 0);
    assert_eq!(level(gp(12, 7), &b[20]), 0);
    // q0 = (1.3, 1/1.3, 1), b - a = 0.50 (raw u 0.5000000000000001): +1.
    assert_eq!(level(gp(1, 11), &b[0]), 1);
    assert_eq!(level(gp(4, 14), &b[0]), 1);
}

#[test]
fn level_split_and_band_disjointness_from_spec() {
    let b = bank();
    let g = grid();
    let mut counts = [0usize; 5];
    for w in &g {
        counts[(level(*w, &b[20]) + 2) as usize] += 1;
    }
    assert_eq!(counts, [16, 26, 87, 26, 16]);
    // Same answer "slightly A": (2, 1/2, 1) leaves 26, (1.3, 1/2, 1) leaves 35.
    assert_eq!(b[4].rho(), [1.3, 0.5, 1.0]);
    let slight = |q: usize| g.iter().filter(|w| level(**w, &b[q]) == 1).count();
    assert_eq!(slight(20), 26);
    assert_eq!(slight(4), 35);
    // "slightly A" and "strongly A" on one question never share a point.
    for q in &b {
        assert!(!g.iter().any(|w| level(*w, q) == 1 && level(*w, q) == 2));
    }
}

// ---- 4/5. regions, adaptive picking, stop rule, the HANDOFF example ----

#[test]
fn handoff_example_four_questions_fourteen_points() {
    let s = honest_session(gp(4, 7));
    assert_eq!(
        s.answers(),
        ans(&[(2, 1), (72, 0), (6, 1), (34, 1)]).as_slice()
    );
    let b = bank();
    assert_eq!(b[2].rho(), [1.3, 1.0 / 1.5, 1.0]);
    assert_eq!(s.wide().len(), 14);
    let r = self_report(s.answers()).unwrap();
    assert_eq!(r.regions.core, vec![gp(4, 7)]);
    assert_eq!(r.regions.wide.len(), 14);
    assert_eq!(r.point, gp(4, 7));
    assert_eq!(r.point.as_array(), [0.2, 0.35, 0.45]);
    assert!(!r.contradiction);
}

#[test]
fn all_171_honest_runs_match_reference() {
    let mut total_q = 0;
    let mut contains = 0;
    for row in RUNS_CSV.lines().skip(1) {
        let f: Vec<&str> = row.split(',').collect();
        let truth = parse_point(f[0]);
        let s = honest_session(truth);
        let want: Vec<Answer> = f[2]
            .split(' ')
            .map(|p| {
                let (q, l) = p.split_once(':').unwrap();
                Answer {
                    question: q.parse().unwrap(),
                    answer: five(l.parse().unwrap()),
                }
            })
            .collect();
        assert_eq!(s.answers(), want.as_slice(), "truth {}", f[0]);
        assert_eq!(s.answers().len(), f[1].parse::<usize>().unwrap());
        let r = regions(s.answers()).unwrap();
        let core: Vec<GridPoint> = f[3].split(' ').map(parse_point).collect();
        let wide: Vec<GridPoint> = f[4].split(' ').map(parse_point).collect();
        assert_eq!(r.core, core, "truth {}", f[0]);
        assert_eq!(r.wide, wide, "truth {}", f[0]);
        total_q += s.answers().len();
        contains += usize::from(r.wide.contains(&truth));
    }
    // §10.2 table, error rate 0: mean 4.27 questions, 171/171 contain truth.
    assert_eq!(contains, 171);
    assert_eq!(format!("{:.2}", total_q as f64 / 171.0), "4.27");
}

#[test]
fn stop_rule_bounds() {
    assert!(!should_stop(3, 1));
    assert!(should_stop(4, 15));
    assert!(!should_stop(4, 16));
    assert!(should_stop(10, 171));
    for row in RUNS_CSV.lines().skip(1) {
        let n: usize = row.split(',').nth(1).unwrap().parse().unwrap();
        assert!((4..=10).contains(&n));
    }
}

#[test]
fn pick_ties_go_to_bank_order_and_session_rejects_wrong_question() {
    // On the full grid the first pick is the reference's first question.
    assert_eq!(pick(&grid(), &[]), Some(2));
    // A single point: every question gives sum n^2 = 1, so the first
    // unanswered question in bank order wins.
    assert_eq!(pick(&[gp(4, 7)], &[]), Some(0));
    assert_eq!(pick(&[gp(4, 7)], &[0, 1]), Some(2));
    let mut s = Session::new();
    assert!(s.answer(0, FiveLevel::AboutSame).is_err());
    assert!(s.answer(2, FiveLevel::SlightlyA).is_ok());
}

#[test]
fn skipped_question_is_not_asked_again() {
    let mut s = Session::new();
    assert_eq!(s.next_question(), Some(2));
    assert!(s.skip(5).is_err());
    s.skip(2).unwrap();
    assert_eq!(s.skipped(), &[2]);
    assert!(s.answers().is_empty());
    let next = s.next_question().unwrap();
    assert_ne!(next, 2);
    assert_eq!(next, pick(&grid(), &[2]).unwrap());
}

#[test]
fn profile_weights_are_validated() {
    let ev = synthetic(30, [1.0, 1.0, 1.0], 3);
    assert!(profile_loglik(&ev, [-0.1, 0.6, 0.5]).is_err());
    assert!(profile_loglik(&ev, [f64::NAN, 0.5, 0.5]).is_err());
    assert!(profile_loglik(&ev, [0.2, 0.3, 0.5]).is_ok());
}

#[test]
fn answers_are_validated() {
    assert!(regions(&ans(&[(96, 0)])).is_err());
    assert!(regions(&ans(&[(3, 0), (3, 1)])).is_err());
    assert!(self_report(&[]).is_err());
}

// ---- G3 ----

#[test]
fn centroid_snaps_to_nearest_point_with_lexicographic_ties() {
    // Two-point core: centroid at the midpoint, both equidistant.
    assert_eq!(nearest_to_centroid(&[gp(4, 7), gp(5, 7)]), Some(gp(4, 7)));
    assert_eq!(nearest_to_centroid(&[gp(5, 7), gp(4, 7)]), Some(gp(4, 7)));
    assert_eq!(nearest_to_centroid(&[gp(3, 3)]), Some(gp(3, 3)));
    // Centroid of three points is the middle one.
    assert_eq!(
        nearest_to_centroid(&[gp(2, 2), gp(3, 3), gp(4, 4)]),
        Some(gp(3, 3))
    );
    assert_eq!(nearest_to_centroid(&[]), None);
}

// ---- G4 ----

#[test]
fn contradiction_lists_questions_off_by_two_somewhere_in_core() {
    let r = self_report(&ans(&[(32, 2), (94, -2), (45, 1), (88, -1)])).unwrap();
    assert_eq!(r.regions.m, 4);
    assert_eq!(r.regions.core.len(), 15);
    assert!(r.contradiction);
    assert_eq!(r.contradicted, vec![32]);
    // m = 2 from two one-level misses: flagged, list empty.
    let r = self_report(&ans(&[(32, 1), (58, -1), (83, -2)])).unwrap();
    assert_eq!(r.regions.m, 2);
    assert_eq!(r.regions.core.len(), 11);
    assert!(r.contradiction);
    assert!(r.contradicted.is_empty());
    // "Some point" reading: off by 2 somewhere in the core, not everywhere.
    let r = self_report(&ans(&[(78, -1), (47, -1), (34, -2)])).unwrap();
    assert_eq!((r.regions.m, r.regions.core.len()), (3, 41));
    assert_eq!(r.contradicted, vec![47, 34]);
    // Honest answers: no flag.
    let s = honest_session(gp(4, 7));
    assert!(!self_report(s.answers()).unwrap().contradiction);
}

#[test]
fn ranges_cover_the_wide_region() {
    let s = honest_session(gp(4, 7));
    let r = self_report(s.answers()).unwrap();
    for (d, (lo, hi)) in r.ranges.iter().enumerate() {
        for w in &r.regions.wide {
            let v = w.as_array()[d];
            assert!(*lo <= v && v <= *hi);
        }
    }
}

// ---- G5 / DECISIONS J ----

#[test]
fn shown_values_are_positive_and_reproduce_rho() {
    let b = bank();
    // Shown-question counts: every question at D_B = 0.1; at D_B = 0.4 the
    // 16 questions with rho_D in {3, 1/3} are skipped (worse defect > 1).
    for &(c, t, d, shown) in &[
        (0.05, 120.0, 0.1, 96),
        (0.5, 120.0, 0.1, 96),
        (0.05, 120.0, 0.4, 80),
        (0.5, 120.0, 0.4, 80),
    ] {
        let base = QuestionBase {
            cost: c,
            time: t,
            defect: d,
        };
        let mut count = 0;
        for q in &b {
            let Some(s) = show(q, base).unwrap() else {
                assert!(q.rho()[2] == 3.0 || q.rho()[2] == 1.0 / 3.0, "{q:?}");
                continue;
            };
            count += 1;
            for v in s.cost.iter().chain(&s.time).chain(&s.defect) {
                assert!(*v > 0.0);
            }
            let r = [
                (s.cost[0] + C0_YUAN) / (s.cost[1] + C0_YUAN),
                (s.time[0] + T0_SECONDS) / (s.time[1] + T0_SECONDS),
                (s.defect[0] + EPSILON) / (s.defect[1] + EPSILON),
            ];
            for k in 0..3 {
                assert!((r[k] - q.rho()[k]).abs() <= 1e-9, "{q:?} {r:?}");
            }
        }
        assert_eq!(count, shown, "base {c} {t} {d}");
    }
    // rho_D = 3 at D_B = 0.4: worse defect 3 * 0.42 - 0.02 = 1.24 > 1, skipped.
    let q = b.iter().find(|q| q.rho()[2] == 3.0).unwrap();
    let base = QuestionBase {
        cost: 0.5,
        time: 120.0,
        defect: 0.4,
    };
    assert_eq!(show(q, base).unwrap(), None);
    assert!(show(q, QuestionBase { cost: 0.0, ..base }).is_err());
    assert!(show(
        q,
        QuestionBase {
            defect: 1.5,
            ..base
        }
    )
    .is_err());
    // A zero defect rate (Q = 1) is a valid base.
    assert!(show(
        q,
        QuestionBase {
            defect: 0.0,
            ..base
        }
    )
    .is_ok());
    assert!(show(q, QuestionBase { time: 0.0, ..base }).is_err());
}

// ---- behaviour (§10.3, G6–G10) ----

fn pt(c: f64, t: f64, q: f64) -> Point {
    Point { c, t, q }
}

fn x_of(p: &Point) -> [f64; 3] {
    [
        (p.c + C0_YUAN).ln(),
        (p.t + T0_SECONDS).ln(),
        (1.0 - p.q + EPSILON).ln(),
    ]
}

fn uniform(rng: &mut SplitMix64) -> f64 {
    (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64
}

/// `n` events with 3 random options, chosen by the logit model at `theta`.
fn synthetic(n: usize, theta: [f64; 3], seed: u64) -> Vec<ChoiceEvent> {
    let mut rng = SplitMix64::new(seed);
    (0..n)
        .map(|e| {
            let opts: Vec<Point> = (0..3)
                .map(|_| {
                    pt(
                        0.05 + 2.0 * uniform(&mut rng),
                        5.0 + 200.0 * uniform(&mut rng),
                        0.5 + 0.49 * uniform(&mut rng),
                    )
                })
                .collect();
            let us: Vec<f64> = opts
                .iter()
                .map(|p| {
                    let x = x_of(p);
                    (-(theta[0] * x[0] + theta[1] * x[1] + theta[2] * x[2])).exp()
                })
                .collect();
            let z: f64 = us.iter().sum();
            let mut r = uniform(&mut rng) * z;
            let mut chosen = 2;
            for (k, u) in us.iter().enumerate() {
                if r < *u {
                    chosen = k;
                    break;
                }
                r -= u;
            }
            ChoiceEvent {
                id: format!("e{e}"),
                options: opts
                    .into_iter()
                    .enumerate()
                    .map(|(k, p)| EventOption {
                        arm: format!("arm{k}"),
                        value: Some(p),
                    })
                    .collect(),
                chosen: format!("arm{chosen}"),
            }
        })
        .collect()
}

const W0: WindowIndex = WindowIndex(4);
const W1: WindowIndex = WindowIndex(5);

#[test]
fn behaviour_recovers_known_weights() {
    let ev = synthetic(600, [2.0, 3.0, 5.0], 7);
    let b = calibrate(&ev, W0, W1, 1).unwrap();
    assert!(b.has_signal(), "{:?}", b.no_signal);
    let w = b.w_b.unwrap();
    for (got, want) in w.iter().zip([0.2, 0.3, 0.5]) {
        assert!((got - want).abs() < 0.08, "{w:?}");
    }
    assert!(b.lambda.unwrap() > 5.0);
    assert_eq!(b.events_used, 600);
    // Omega_b: threshold L(theta_hat) - 3 (DECISIONS G9); no grid point
    // beats the unconstrained maximum.
    let ll_hat = b.ll_hat.unwrap();
    for (p, l) in &b.profile {
        assert!(*l <= ll_hat + 1e-9);
        assert_eq!(b.support.contains(p), *l >= ll_hat - SUPPORT_DROP);
    }
    assert!(!b.support.is_empty() && b.support.len() < 171);
    let near = grid()
        .into_iter()
        .min_by(|p, q| {
            let d = |g: &GridPoint| {
                g.as_array()
                    .iter()
                    .zip(w)
                    .map(|(a, b)| (a - b) * (a - b))
                    .sum::<f64>()
            };
            d(p).total_cmp(&d(q))
        })
        .unwrap();
    assert!(b.support.contains(&near));
}

#[test]
fn behaviour_w_b_may_leave_the_grid_floor() {
    // True theta has a zero time weight: w_b's time component is below the
    // 0.05 grid floor and is reported as is, not moved to the grid.
    let ev = synthetic(600, [6.0, 0.0, 6.0], 11);
    let b = calibrate(&ev, W0, W1, 1).unwrap();
    assert!(b.has_signal(), "{:?}", b.no_signal);
    let w = b.w_b.unwrap();
    assert!(w[1] < 0.05, "{w:?}");
    // The threshold is L(theta_hat) itself, so with a sharp likelihood and
    // w_b off the grid, Omega_b may even be empty.
    let ll_hat = b.ll_hat.unwrap();
    for (p, l) in &b.profile {
        assert!(*l < ll_hat);
        assert_eq!(b.support.contains(p), *l >= ll_hat - SUPPORT_DROP);
    }
}

#[test]
fn no_signal_gates_each_trigger() {
    // 19 events.
    let b = calibrate(&synthetic(19, [2.0, 3.0, 5.0], 3), W0, W1, 1).unwrap();
    assert!(b.no_signal.contains(&NoSignalReason::TooFewEvents));
    assert!(b.profile.is_empty() && b.bootstrap.is_none());
    // Quality identical in every option: rank 2.
    let mut ev = synthetic(100, [2.0, 3.0, 0.0], 5);
    for e in &mut ev {
        for o in &mut e.options {
            o.value.as_mut().unwrap().q = 0.9;
        }
    }
    let b = calibrate(&ev, W0, W1, 1).unwrap();
    assert_eq!(b.no_signal, vec![NoSignalReason::RankBelowThree]);
    // One chosen arm only.
    let mut ev = synthetic(100, [2.0, 3.0, 5.0], 5);
    for e in &mut ev {
        e.chosen = "arm0".into();
    }
    let b = calibrate(&ev, W0, W1, 1).unwrap();
    assert!(b.no_signal.contains(&NoSignalReason::OneChosenArm));
    // Choices unrelated to the attributes: lambda below 0.5.
    let b = calibrate(&synthetic(400, [0.0, 0.0, 0.0], 9), W0, W1, 1).unwrap();
    assert!(
        b.no_signal.contains(&NoSignalReason::LambdaBelowMin),
        "{:?} {:?}",
        b.no_signal,
        b.lambda
    );
}

#[test]
fn separated_choices_are_not_identifiable() {
    // The chosen arm is better in all three dimensions in every event: the
    // likelihood grows without bound.
    let mut ev = synthetic(60, [1.0, 1.0, 1.0], 13);
    for e in &mut ev {
        // Make option 0 dominate the others.
        let mut v0 = e.options[0].value.unwrap();
        for o in e.options.iter().skip(1) {
            let w = o.value.unwrap();
            v0.c = v0.c.min(w.c * 0.5);
            v0.t = v0.t.min(w.t * 0.5);
            v0.q = v0.q.max(w.q).max(0.99);
        }
        e.options[0].value = Some(v0);
        e.chosen = "arm0".into();
    }
    // Two chosen arms are needed to pass the arm gate; flip one event so a
    // different, also dominating, arm is chosen.
    let e = &mut ev[0];
    let v0 = e.options[0].value.unwrap();
    e.options[1].value = Some(pt(v0.c * 0.5, v0.t * 0.5, 0.995));
    e.chosen = "arm1".into();
    let b = calibrate(&ev, W0, W1, 1).unwrap();
    assert!(
        b.no_signal.contains(&NoSignalReason::NotIdentifiable),
        "{:?} {:?}",
        b.no_signal,
        b.lambda
    );
}

#[test]
fn rank_uses_a_relative_tolerance() {
    // Quality differs only by 1e-13 between options: numerically rank 2.
    let mut ev = synthetic(100, [2.0, 3.0, 0.0], 5);
    for (n, e) in ev.iter_mut().enumerate() {
        for (k, o) in e.options.iter_mut().enumerate() {
            o.value.as_mut().unwrap().q = 0.9 + 1e-13 * ((n + k) % 3) as f64;
        }
    }
    let b = calibrate(&ev, W0, W1, 1).unwrap();
    assert_eq!(b.no_signal, vec![NoSignalReason::RankBelowThree]);
}

#[test]
fn rank_tolerance_scales_with_the_data() {
    // Every attribute difference is about 1e-10, below an absolute 1e-9
    // tolerance, but the three directions are independent: relative to the
    // largest entry the rank is 3.
    let ev: Vec<ChoiceEvent> = (0..30)
        .map(|e| {
            let k = (e % 3) as f64;
            let base = pt(1.0, 60.0, 0.8);
            let mut alt = base;
            match e % 3 {
                0 => alt.c += 1e-10 * (1.0 + k),
                1 => alt.t += 1e-8,
                _ => alt.q -= 1e-10,
            }
            ChoiceEvent {
                id: format!("e{e}"),
                options: vec![
                    EventOption {
                        arm: "A".into(),
                        value: Some(base),
                    },
                    EventOption {
                        arm: "B".into(),
                        value: Some(alt),
                    },
                ],
                chosen: if e % 2 == 0 { "A".into() } else { "B".into() },
            }
        })
        .collect();
    let b = calibrate(&ev, W0, W1, 1).unwrap();
    assert!(
        !b.no_signal.contains(&NoSignalReason::RankBelowThree),
        "{:?}",
        b.no_signal
    );
}

#[test]
fn bootstrap_counts_failed_resamples_and_flags_instability() {
    // 28 events vary only in cost; one event varies only in time and one
    // only in quality (the chosen arm is the worse one there, so theta stays
    // finite at 0 in that dimension). A resample has rank 3 only if it
    // draws both special events, about 40% of the time, so many resamples
    // fail and fewer than 100 succeed.
    let mut rng = SplitMix64::new(77);
    let mut ev: Vec<ChoiceEvent> = (0..28)
        .map(|e| {
            let ca = 0.2 + uniform(&mut rng);
            let cb = 0.2 + uniform(&mut rng);
            let xa = (ca + C0_YUAN).ln();
            let xb = (cb + C0_YUAN).ln();
            let pa = 1.0 / (1.0 + (-5.0 * (xb - xa)).exp());
            let chosen = if uniform(&mut rng) < pa { "A" } else { "B" };
            ChoiceEvent {
                id: format!("c{e}"),
                options: vec![
                    EventOption {
                        arm: "A".into(),
                        value: Some(pt(ca, 60.0, 0.8)),
                    },
                    EventOption {
                        arm: "B".into(),
                        value: Some(pt(cb, 60.0, 0.8)),
                    },
                ],
                chosen: chosen.into(),
            }
        })
        .collect();
    ev.push(ChoiceEvent {
        id: "time".into(),
        options: vec![
            EventOption {
                arm: "A".into(),
                value: Some(pt(0.5, 60.0, 0.8)),
            },
            EventOption {
                arm: "B".into(),
                value: Some(pt(0.5, 90.0, 0.8)),
            },
        ],
        chosen: "B".into(),
    });
    ev.push(ChoiceEvent {
        id: "quality".into(),
        options: vec![
            EventOption {
                arm: "A".into(),
                value: Some(pt(0.5, 60.0, 0.9)),
            },
            EventOption {
                arm: "B".into(),
                value: Some(pt(0.5, 60.0, 0.7)),
            },
        ],
        chosen: "B".into(),
    });
    let b = calibrate(&ev, W0, W1, 5).unwrap();
    assert!(b.has_signal(), "{:?} {:?}", b.no_signal, b.lambda);
    let bs = b.bootstrap.unwrap();
    assert!(bs.failed > 0, "{bs:?}");
    assert_eq!(bs.ok + bs.failed, BOOTSTRAP_RESAMPLES as usize);
    assert!(bs.ok < 100, "{bs:?}");
    assert!(bs.unstable);
}

#[test]
fn bootstrap_instability_threshold_is_100_successes() {
    assert!(bootstrap_unstable(0));
    assert!(bootstrap_unstable(99));
    assert!(!bootstrap_unstable(100));
    assert!(!bootstrap_unstable(200));
}

#[test]
fn empty_support_with_self_report_is_the_conflict_row() {
    // Sharp likelihood with w_b off the grid: Omega_b is empty, so the
    // intersection with Omega_s is empty (§10.4 row 3), not an error.
    let ev = synthetic(600, [6.0, 0.0, 6.0], 11);
    let b = calibrate(&ev, W0, W1, 1).unwrap();
    assert!(b.has_signal());
    assert!(b.support.is_empty());
    let s = self_report(honest_session(gp(4, 7)).answers()).unwrap();
    let m = merge(Some(&s), Some(&b));
    assert_eq!(m.effective, [0.2, 0.35, 0.45]);
    assert_eq!(
        m.suggestion,
        Suggestion::Conflict {
            optional: b.w_b.unwrap()
        }
    );
    // Without a self-report the "behaviour has a signal" row applies.
    let m = merge(None, Some(&b));
    assert_eq!(
        m.suggestion,
        Suggestion::AdoptBehavior {
            optional: b.w_b.unwrap()
        }
    );
}

#[test]
fn omega_b_threshold_is_three_below_the_maximum() {
    let ev = synthetic(60, [2.0, 3.0, 5.0], 31);
    let b = calibrate(&ev, W0, W1, 1).unwrap();
    assert!(b.has_signal(), "{:?}", b.no_signal);
    let ll_hat = b.ll_hat.unwrap();
    // Some grid points lie just outside the threshold, so a looser one
    // would change Omega_b.
    let near_miss = b
        .profile
        .iter()
        .filter(|(_, l)| *l < ll_hat - SUPPORT_DROP && *l >= ll_hat - SUPPORT_DROP - 1.0)
        .count();
    assert!(near_miss > 0);
    for (p, l) in &b.profile {
        assert_eq!(b.support.contains(p), *l >= ll_hat - SUPPORT_DROP);
    }
}

#[test]
fn event_with_a_missing_triple_is_dropped_whole() {
    let mut ev = synthetic(25, [2.0, 3.0, 5.0], 17);
    ev[0].options[2].value = None;
    ev[1].options.truncate(1);
    ev[1].chosen = "arm0".into();
    let b = calibrate(&ev, W0, W1, 1).unwrap();
    assert_eq!(b.events_used, 23);
    assert_eq!(b.events_dropped, 2);
    // Chosen arm not among the options, duplicate arms or ids: errors.
    let mut bad = synthetic(3, [1.0, 1.0, 1.0], 1);
    bad[0].chosen = "nope".into();
    assert!(calibrate(&bad, W0, W1, 1).is_err());
    let mut bad = synthetic(3, [1.0, 1.0, 1.0], 1);
    bad[1].id = bad[0].id.clone();
    assert!(calibrate(&bad, W0, W1, 1).is_err());
    let mut bad = synthetic(3, [1.0, 1.0, 1.0], 1);
    bad[0].options[1].arm = "arm0".into();
    assert!(calibrate(&bad, W0, W1, 1).is_err());
    let mut bad = synthetic(3, [1.0, 1.0, 1.0], 1);
    bad[0].options[1].value = Some(pt(f64::NAN, 1.0, 0.5));
    assert!(calibrate(&bad, W0, W1, 1).is_err());
}

#[test]
fn profile_at_lambda_zero_end_is_the_uniform_likelihood() {
    // Every chosen arm costs more and is otherwise equal: for a cost-heavy w
    // the right derivative at 0 is negative, so L^prof(w) = L(0).
    let ev: Vec<ChoiceEvent> = (0..30)
        .map(|e| ChoiceEvent {
            id: format!("e{e}"),
            options: vec![
                EventOption {
                    arm: "A".into(),
                    value: Some(pt(1.0 + e as f64 * 0.01, 60.0, 0.8)),
                },
                EventOption {
                    arm: "B".into(),
                    value: Some(pt(0.2, 60.0, 0.8)),
                },
                EventOption {
                    arm: "C".into(),
                    value: Some(pt(0.3, 60.0, 0.8)),
                },
            ],
            chosen: "A".into(),
        })
        .collect();
    // L(0) = -sum_e ln |S_e|, summed in event order: returned exactly, no
    // search towards lambda = 0.
    let l0: f64 = (0..30).map(|_| -(3f64.ln())).sum();
    let l = profile_loglik(&ev, [0.9, 0.05, 0.05]).unwrap();
    assert_eq!(l, l0);
    // Choosing the cheap arm instead: interior maximum, above L(0).
    let mut ev2 = ev.clone();
    for e in &mut ev2 {
        e.chosen = "B".into();
    }
    assert!(profile_loglik(&ev2, [0.9, 0.05, 0.05]).unwrap() > l0);
}

#[test]
fn bootstrap_is_deterministic_per_seed() {
    let ev = synthetic(200, [2.0, 3.0, 5.0], 21);
    let a = calibrate(&ev, W0, W1, 42).unwrap();
    let b = calibrate(&ev, W0, W1, 42).unwrap();
    let c = calibrate(&ev, W0, W1, 43).unwrap();
    let (ba, bb, bc) = (
        a.bootstrap.unwrap(),
        b.bootstrap.unwrap(),
        c.bootstrap.unwrap(),
    );
    assert_eq!(ba, bb);
    assert_ne!(ba.quantiles, bc.quantiles);
    assert_eq!(ba.ok + ba.failed, BOOTSTRAP_RESAMPLES as usize);
    assert!(!ba.unstable);
    for d in 0..3 {
        assert!(ba.quantiles[d][0] <= ba.quantiles[d][1]);
        assert!(ba.quantiles[d][1] <= ba.quantiles[d][2]);
    }
}

#[test]
fn behaviour_windows_guard() {
    assert!(check_windows(WindowIndex(3), WindowIndex(3)).is_err());
    assert!(check_windows(WindowIndex(4), WindowIndex(3)).is_err());
    assert!(check_windows(WindowIndex(2), WindowIndex(3)).is_ok());
    let ev = synthetic(30, [2.0, 3.0, 5.0], 2);
    assert!(calibrate(&ev, WindowIndex(5), WindowIndex(5), 1).is_err());
    // First window: nothing from t-1, no signal.
    let b = calibrate(&[], WindowIndex::FIRST, WindowIndex(1), 1).unwrap();
    assert!(b.no_signal.contains(&NoSignalReason::TooFewEvents));
    assert!(!b.has_signal());
}

// ---- §10.4 merge (G9 ties) ----

fn behaviour_with(support: Vec<GridPoint>, profile: Vec<(GridPoint, f64)>) -> Behavior {
    Behavior {
        events_used: 50,
        events_dropped: 0,
        no_signal: vec![],
        theta: Some([1.0, 1.0, 1.0]),
        lambda: Some(3.0),
        w_b: Some([0.6, 0.2, 0.2]),
        ll_hat: Some(-1.0),
        profile,
        support,
        bootstrap: None,
    }
}

#[test]
fn merge_table_all_five_rows_and_behaviour_never_applies() {
    let s = self_report(honest_session(gp(4, 7)).answers()).unwrap();
    let ws = [0.2, 0.35, 0.45];
    let eq = [1.0 / 3.0; 3];
    // Self-report, no signal.
    let mut none = behaviour_with(vec![], vec![]);
    none.no_signal = vec![NoSignalReason::TooFewEvents];
    let m = merge(Some(&s), Some(&none));
    assert_eq!((m.effective, m.suggestion), (ws, Suggestion::None));
    let m = merge(Some(&s), None);
    assert_eq!((m.effective, m.suggestion), (ws, Suggestion::None));
    // Overlap: consistent; optional point is the best profile in Omega_s.
    let prof: Vec<(GridPoint, f64)> = grid()
        .into_iter()
        .map(|w| (w, if w == gp(5, 7) { -10.0 } else { -20.0 }))
        .collect();
    let b = behaviour_with(vec![gp(4, 7), gp(10, 2)], prof);
    let m = merge(Some(&s), Some(&b));
    assert_eq!(m.effective, ws);
    assert!(s.regions.wide.contains(&gp(5, 7)));
    assert_eq!(m.suggestion, Suggestion::Consistent { optional: gp(5, 7) });
    // No overlap: conflict, optional w_b.
    let b = behaviour_with(vec![gp(16, 2)], vec![]);
    let m = merge(Some(&s), Some(&b));
    assert_eq!(m.effective, ws);
    assert_eq!(
        m.suggestion,
        Suggestion::Conflict {
            optional: [0.6, 0.2, 0.2]
        }
    );
    // No self-report, signal.
    let m = merge(None, Some(&b));
    assert_eq!(m.effective, eq);
    assert_eq!(
        m.suggestion,
        Suggestion::AdoptBehavior {
            optional: [0.6, 0.2, 0.2]
        }
    );
    // Neither.
    let m = merge(None, Some(&none));
    assert_eq!((m.effective, m.suggestion), (eq, Suggestion::DoSelfReport));
    let m = merge(None, None);
    assert_eq!(m.effective, eq);
}

#[test]
fn argmax_in_self_report_region_ties_go_to_smallest_point() {
    let s = self_report(honest_session(gp(4, 7)).answers()).unwrap();
    let wide = s.regions.wide.clone();
    let (p, q) = (wide[0], wide[wide.len() - 1]);
    assert!(p < q);
    let prof: Vec<(GridPoint, f64)> = grid()
        .into_iter()
        .map(|w| (w, if w == p || w == q { -5.0 } else { -9.0 }))
        .collect();
    let b = behaviour_with(vec![q], prof);
    assert_eq!(
        merge(Some(&s), Some(&b)).suggestion,
        Suggestion::Consistent { optional: p }
    );
    // A difference below the relative tolerance is still a tie.
    let prof: Vec<(GridPoint, f64)> = grid()
        .into_iter()
        .map(|w| {
            (
                w,
                if w == p {
                    -5.0
                } else if w == q {
                    -5.0 + 1e-12
                } else {
                    -9.0
                },
            )
        })
        .collect();
    let b = behaviour_with(vec![q], prof);
    assert_eq!(
        merge(Some(&s), Some(&b)).suggestion,
        Suggestion::Consistent { optional: p }
    );
}

// ---- G11: resolve_fallback with F = R ----

mod fallback {
    use super::*;
    use agent_value_core::anchor::{certify_reference_layer, LayerAnchor};
    use agent_value_core::cell_values::Triple;
    use agent_value_core::eligibility::AuthenticityGrade;
    use agent_value_core::retry::{Prior, RetryModel};
    use agent_value_core::selection::{
        resolve_fallback, FallbackCandidate, FallbackDecision, FallbackLayer,
    };
    use agent_value_core::status::{layer_status, CellCounts, CellInput};

    fn layer(omega_d: f64, ratings: u32) -> FallbackLayer {
        let cells: Vec<CellId> = (1..=3)
            .map(|k| CellId::user(&format!("c{k:02}")).unwrap())
            .collect();
        let w = CellWeights::equal(&cells).unwrap();
        let rows: Vec<CellInput> = cells
            .iter()
            .map(|c| CellInput {
                cell: c.clone(),
                counts: CellCounts {
                    n1: 10,
                    n0: 0,
                    n_c: 0,
                    m: 0,
                },
                prior: Prior::Jeffreys,
            })
            .collect();
        let model = RetryModel::with_default_k(0.0).unwrap();
        let st = layer_status(&w, &rows, &model).unwrap();
        let tr: Vec<(CellId, Triple)> = cells
            .iter()
            .map(|c| (c.clone(), Triple::new(1.0, 10.0, 0.8)))
            .collect();
        let anchor: LayerAnchor = certify_reference_layer(&st, &w, &tr).unwrap();
        FallbackLayer {
            omega_d,
            weights: w,
            n_bad: 0,
            anchor,
            ratings: cells.iter().map(|c| (c.clone(), ratings)).collect(),
        }
    }

    fn cand(id: &str, l: FallbackLayer, auth: AuthenticityGrade) -> FallbackCandidate {
        FallbackCandidate {
            id: id.into(),
            prev_authenticity: auth,
            layers: vec![l],
        }
    }

    #[test]
    fn same_id_still_validates_input() {
        for bad in [f64::NAN, f64::INFINITY, -0.5] {
            let c = cand("R", layer(bad, 3), AuthenticityGrade::A);
            assert!(resolve_fallback("R", Some(&c)).is_err(), "omega_d {bad}");
        }
        let mut l = layer(1.0, 3);
        let first = l.ratings[0].0.clone();
        l.ratings.push((first, 3));
        let c = cand("R", l, AuthenticityGrade::A);
        assert!(matches!(
            resolve_fallback("R", Some(&c)),
            Err(CoreError::DuplicateCell { .. })
        ));
    }

    #[test]
    fn same_id_valid_input_is_default_even_when_unqualified() {
        // Zero ratings and a failing authenticity grade would make another
        // arm revert; F = R needs no qualification.
        let c = cand("R", layer(1.0, 0), AuthenticityGrade::C);
        assert_eq!(
            resolve_fallback("R", Some(&c)).unwrap(),
            FallbackDecision::Default { id: "R".into() }
        );
        let other = cand("F", layer(1.0, 0), AuthenticityGrade::C);
        assert!(matches!(
            resolve_fallback("R", Some(&other)).unwrap(),
            FallbackDecision::Revert { .. }
        ));
    }
}

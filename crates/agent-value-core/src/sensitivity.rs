//! Weight sensitivity display data (v22 §12; HANDOFF step 12).
//!
//! For one layer of a scoring scope (or the scope overall) and the arms of
//! `E_d` (or the arms with an overall `V`):
//!
//! - **Winner map** — on every point `w` of the 171-point grid `G` (§10.2
//!   line 696), `Win(w)` is the tie group of highest `V_{i,d}(w)`; ties are
//!   reported as a whole group, never broken by name (§12 line 798).
//!   Withheld in the first window and when the reference is tier III
//!   (no numerator anchor).
//! - **Current point and regions** — every grid point carries whether it
//!   lies in `Ω_s` (self-report) and `Ω_b` (behaviour support). Both sets
//!   come from step 11 and are plain inputs here.
//! - **Weight-robust winner** — over `Ω = Ω_s`, or the neighbourhood
//!   `{w ∈ G : max|w − W_k| ≤ 0.10}` when there is no self-report, the share
//!   of points at which each arm is in `Win(w)`; an arm that is the sole
//!   winner at every point gets "在你的权重范围内都是它".
//! - **Statistical robustness** — task bootstrap, 200 resamples, winner and
//!   frontier frequency at `W_k`, reported separately from the weight
//!   robustness. The caller recomputes `E_d`, triples and the anchor for
//!   each resample.
//! - **Flip line** — for the top two arms `i, j` at `W_k`, the line
//!   `aΔ_C + bΔ_T + gΔ_D = 0`, and one statement per fixed dimension.
//! - **Noise-floor robustness** (DECISIONS K) — `Win(W_k)` under the scope
//!   floors and six one-at-a-time perturbations (`c0`, `t0`, `ε` × 0.5 and
//!   × 2).
//!
//! All counters use checked arithmetic and return [`CoreError::Overflow`]
//! instead of wrapping.

use std::collections::{BTreeMap, BTreeSet};

use crate::cell_values::Triple;
use crate::cells::CellId;
use crate::constants::BOOTSTRAP_RESAMPLES;
use crate::error::{finite, CoreError, Result};
use crate::frontier::{highest_v_group, layer_frontiers, FrontierArm};
use crate::value::{value_score, Floors, Point};
use crate::weights::Weights;

/// "在你的权重范围内都是它" (§12 display item 3).
pub const WEIGHT_ROBUST_EVERYWHERE: &str = "在你的权重范围内都是它";
/// "底噪稳健" (DECISIONS K).
pub const FLOOR_ROBUST: &str = "底噪稳健";
/// Suffix every flip statement carries (§12 display item 5).
pub const CURRENT_ESTIMATE_NOTE: &str = "这是在当前估计值下的结论";

/// Grid denominator: weights are multiples of `1/20 = 0.05` (§9 grid step).
const GRID_DEN: u32 = 20;
/// Each weight is at least `1/20` (§9 floor 0.05).
const GRID_MIN: u32 = 1;
/// Neighbourhood radius of the no-self-report region, in grid steps
/// (`0.10 = 2 × 0.05`, §12 line 809).
const NEIGHBOURHOOD_RADIUS: f64 = 0.10;
/// Slack for comparing weights with grid values and the radius.
const GRID_TOL: f64 = 1e-9;
/// Below this `|Δ_q − Δ_p|` a flip statement treats the line as parallel
/// to the fixed-weight segment.
const SLOPE_TOL: f64 = 1e-12;

fn bump(counter: &mut u32, what: &'static str) -> Result<()> {
    *counter = counter.checked_add(1).ok_or(CoreError::Overflow { what })?;
    Ok(())
}

/// The 171 grid points `G` (§10.2 line 696), built from integer twentieths
/// `(i, j, 20 − i − j)/20`, ordered by `a` then `b` ascending.
pub fn grid_points() -> Result<Vec<Weights>> {
    let mut out = Vec::new();
    for i in GRID_MIN..=GRID_DEN - 2 * GRID_MIN {
        for j in GRID_MIN..=GRID_DEN - GRID_MIN - i {
            let k = GRID_DEN - i - j;
            let d = f64::from(GRID_DEN);
            out.push(Weights::new(
                f64::from(i) / d,
                f64::from(j) / d,
                f64::from(k) / d,
            )?);
        }
    }
    Ok(out)
}

/// Integer twentieths of a grid weight; `None` when it is not on `G`.
fn grid_key(w: &Weights) -> Option<(u32, u32, u32)> {
    let mut key = [0u32; 3];
    for (slot, v) in key.iter_mut().zip([w.a(), w.b(), w.g()]) {
        let scaled = v * f64::from(GRID_DEN);
        let r = scaled.round();
        if (scaled - r).abs() > GRID_TOL * f64::from(GRID_DEN)
            || r < f64::from(GRID_MIN)
            || r > f64::from(GRID_DEN - 2 * GRID_MIN)
        {
            return None;
        }
        // r is an integer in [1, 18] here, so the cast is exact.
        *slot = r as u32;
    }
    // Checked explicitly rather than relying on the `Weights` invariant.
    let sum = key.iter().try_fold(0u32, |acc, &k| acc.checked_add(k))?;
    if sum != GRID_DEN {
        return None;
    }
    Some((key[0], key[1], key[2]))
}

fn region_keys(points: &[Weights], what: &'static str) -> Result<BTreeSet<(u32, u32, u32)>> {
    if points.is_empty() {
        return Err(CoreError::InvalidField {
            field: what,
            reason: "region must not be empty",
        });
    }
    let mut keys = BTreeSet::new();
    for w in points {
        let k = grid_key(w).ok_or(CoreError::InvalidField {
            field: what,
            reason: "point is not on the 0.05 grid",
        })?;
        if !keys.insert(k) {
            return Err(CoreError::InvalidField {
                field: what,
                reason: "duplicate point",
            });
        }
    }
    Ok(keys)
}

/// One arm of the compared set with its complete triple.
#[derive(Debug, Clone, PartialEq)]
pub struct SensitivityArm {
    pub id: String,
    pub point: Point,
}

fn check_ids(arms: &[SensitivityArm]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for a in arms {
        if a.id.trim().is_empty() {
            return Err(CoreError::EmptyField {
                field: "sensitivity arm id",
            });
        }
        if !seen.insert(a.id.as_str()) {
            return Err(CoreError::DuplicateCell { cell: a.id.clone() });
        }
    }
    Ok(())
}

/// `Win(w)`: the sorted tie group of highest `V(w)` (§12 line 798). Empty
/// when `arms` is empty. Ties use the same tolerance as the official pick
/// (`frontier::highest_v_group`).
pub fn winners(
    arms: &[SensitivityArm],
    reference: Point,
    w: &Weights,
    floors: &Floors,
) -> Result<Vec<String>> {
    check_ids(arms)?;
    let mut scores = Vec::with_capacity(arms.len());
    for a in arms {
        scores.push((a.id.clone(), value_score(reference, a.point, w, floors)?));
    }
    highest_v_group(&scores)
}

/// Why the sensitivity display is not produced (§12 line 798).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Withheld {
    /// First window of the scope: no `V` (§A line 53).
    FirstWindow,
    /// The reference is tier III for this layer (or the overall reference
    /// is NA): no `V` (§7).
    NoReference,
    /// No arm to compare (empty `E_d`): nothing to colour, and no
    /// robustness label may be shown.
    NoArms,
}

/// Inputs of one layer (or the scope overall).
#[derive(Debug, Clone, PartialEq)]
pub struct SensitivityInput<'a> {
    /// `E_d` members (layer) or arms with an overall `V` (overall).
    pub arms: &'a [SensitivityArm],
    /// Numerator anchor: tier I measured or tier II clamped; `None` in
    /// tier III / overall NA.
    pub reference: Option<Point>,
    pub first_window: bool,
    /// The effective weights `W_k`.
    pub current: Weights,
    /// `Ω_s` from the self-report (step 11), if any.
    pub omega_s: Option<&'a [Weights]>,
    /// `Ω_b` from behaviour calibration (step 11), if any.
    pub omega_b: Option<&'a [Weights]>,
    /// Floors of the scope (§A).
    pub floors: Floors,
}

/// One coloured grid point.
#[derive(Debug, Clone, PartialEq)]
pub struct MapPoint {
    pub weights: Weights,
    pub winners: Vec<String>,
    pub in_omega_s: bool,
    pub in_omega_b: bool,
}

/// Which region the weight-robust winner was computed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RobustRegion {
    /// `Ω_s`.
    SelfReport,
    /// `{w ∈ G : max|w − W_k| ≤ 0.10}` (no self-report).
    Neighbourhood,
}

/// Share of one arm over the region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArmShare {
    pub id: String,
    /// Points where the arm is in `Win(w)` (alone or tied).
    pub points: u32,
    /// Points where `Win(w)` is exactly this arm.
    pub sole_points: u32,
}

/// Weight-robust winner (§12 display item 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeightRobustness {
    pub region: RobustRegion,
    /// `|Ω|`.
    pub total: u32,
    /// One entry per arm, sorted by id (zero shares included).
    pub shares: Vec<ArmShare>,
    /// The arm that is the sole winner at every point of `Ω`, shown with
    /// [`WEIGHT_ROBUST_EVERYWHERE`].
    pub everywhere: Option<String>,
}

impl ArmShare {
    /// `points / total` (0 when `total` is 0).
    pub fn share(&self, total: u32) -> f64 {
        if total == 0 {
            0.0
        } else {
            f64::from(self.points) / f64::from(total)
        }
    }
}

/// The three dimensions in the order cost, time, quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Dim {
    Cost,
    Time,
    Quality,
}

impl Dim {
    const ALL: [Dim; 3] = [Dim::Cost, Dim::Time, Dim::Quality];

    /// Display name: 成本 / 时间 / 质量.
    pub fn label(self) -> &'static str {
        match self {
            Dim::Cost => "成本",
            Dim::Time => "时间",
            Dim::Quality => "质量",
        }
    }

    fn index(self) -> usize {
        match self {
            Dim::Cost => 0,
            Dim::Time => 1,
            Dim::Quality => 2,
        }
    }
}

/// What happens on the segment where one weight is held at its current
/// value and the other two vary (each `≥ 0.05`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FlipKind {
    /// The runner-up overtakes the leader exactly when
    /// `w_over / w_under > ratio`, and that happens inside the segment.
    Crosses { over: Dim, under: Dim, ratio: f64 },
    /// The runner-up never overtakes the leader on the segment.
    Never,
    /// The runner-up is ahead on the whole segment. Only reachable when the
    /// two are within the tie tolerance at `W_k` (the leader is then the
    /// smaller id, not necessarily the higher `V`).
    Always,
    /// The two arms tie on the whole segment (the line contains it).
    Tied,
}

/// One statement with a fixed dimension (§12 display item 5).
#[derive(Debug, Clone, PartialEq)]
pub struct FlipStatement {
    pub fixed: Dim,
    pub fixed_weight: f64,
    pub kind: FlipKind,
}

/// Flip line between the top two arms at `W_k`.
#[derive(Debug, Clone, PartialEq)]
pub struct FlipLine {
    /// Rank 1 at `W_k` (`i`).
    pub leader: String,
    /// Rank 2 at `W_k` (`j`).
    pub runner_up: String,
    /// The two are tied at `W_k`; `leader` / `runner_up` are then in id
    /// order and the line passes through `W_k`.
    pub tied_now: bool,
    /// `(Δ_C, Δ_T, Δ_D)` with `Δ_C = ln((C_i + c0)/(C_j + c0))`, `Δ_T`
    /// likewise and `Δ_D = ln((1 − Q_i + ε)/(1 − Q_j + ε))`. The runner-up
    /// is ahead exactly where `aΔ_C + bΔ_T + gΔ_D > 0`.
    pub delta: [f64; 3],
    /// One statement per fixed dimension: cost, time, quality.
    pub statements: Vec<FlipStatement>,
}

/// Why no flip line is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoFlipLine {
    /// Fewer than two arms.
    TooFewArms,
    /// Rank 2 is tied with rank 3 at `W_k`, so "the top two" is not a pair.
    AmbiguousRunnerUp,
}

/// One floor variant of DECISIONS K.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloorVariant {
    Base,
    C0Half,
    C0Double,
    T0Half,
    T0Double,
    EpsHalf,
    EpsDouble,
}

impl FloorVariant {
    /// Base first, then the six one-at-a-time perturbations.
    pub const ALL: [FloorVariant; 7] = [
        FloorVariant::Base,
        FloorVariant::C0Half,
        FloorVariant::C0Double,
        FloorVariant::T0Half,
        FloorVariant::T0Double,
        FloorVariant::EpsHalf,
        FloorVariant::EpsDouble,
    ];

    /// The perturbed floors (× 0.5 or × 2 of the scope floors).
    pub fn apply(self, f: &Floors) -> Result<Floors> {
        let (c0, t0, eps) = (f.c0(), f.t0(), f.eps());
        match self {
            FloorVariant::Base => Ok(*f),
            FloorVariant::C0Half => Floors::new(c0 * 0.5, t0, eps),
            FloorVariant::C0Double => Floors::new(c0 * 2.0, t0, eps),
            FloorVariant::T0Half => Floors::new(c0, t0 * 0.5, eps),
            FloorVariant::T0Double => Floors::new(c0, t0 * 2.0, eps),
            FloorVariant::EpsHalf => Floors::new(c0, t0, eps * 0.5),
            FloorVariant::EpsDouble => Floors::new(c0, t0, eps * 2.0),
        }
    }
}

/// Noise-floor robustness at `W_k` (DECISIONS K).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FloorRobustness {
    /// `Win(W_k)` for each of the seven variants, base first.
    pub runs: Vec<(FloorVariant, Vec<String>)>,
    /// All seven equal: shown with [`FLOOR_ROBUST`]; otherwise every run is
    /// listed.
    pub robust: bool,
}

/// Everything §12 needs from the core for one layer (or overall), except
/// the bootstrap, which is [`statistical_robustness`].
#[derive(Debug, Clone, PartialEq)]
pub struct SensitivityReport {
    /// 171 points in [`grid_points`] order.
    pub map: Vec<MapPoint>,
    pub current: Weights,
    /// `Win(W_k)`.
    pub current_winners: Vec<String>,
    pub weight_robust: WeightRobustness,
    pub flip: std::result::Result<FlipLine, NoFlipLine>,
    pub floor_robust: FloorRobustness,
}

/// The §12 display for one layer, or why it is withheld.
///
/// When withheld (first window, tier III) §12 still shows the frontier;
/// this function does not produce it — take it from
/// [`crate::frontier::layer_frontiers`].
pub fn sensitivity(
    input: &SensitivityInput<'_>,
) -> Result<std::result::Result<SensitivityReport, Withheld>> {
    check_ids(input.arms)?;
    let s_keys = input
        .omega_s
        .map(|p| region_keys(p, "omega_s"))
        .transpose()?;
    let b_keys = input
        .omega_b
        .map(|p| region_keys(p, "omega_b"))
        .transpose()?;
    if input.first_window {
        return Ok(Err(Withheld::FirstWindow));
    }
    let Some(reference) = input.reference else {
        return Ok(Err(Withheld::NoReference));
    };
    if input.arms.is_empty() {
        return Ok(Err(Withheld::NoArms));
    }

    let grid = grid_points()?;
    let mut map = Vec::with_capacity(grid.len());
    for w in &grid {
        let key = grid_key(w);
        map.push(MapPoint {
            weights: *w,
            winners: winners(input.arms, reference, w, &input.floors)?,
            in_omega_s: matches!((&s_keys, key), (Some(s), Some(k)) if s.contains(&k)),
            in_omega_b: matches!((&b_keys, key), (Some(s), Some(k)) if s.contains(&k)),
        });
    }

    let weight_robust = weight_robustness(input.arms, &map, &input.current, s_keys.is_some())?;
    let current_winners = winners(input.arms, reference, &input.current, &input.floors)?;
    let flip = flip_line(input.arms, reference, &input.current, &input.floors)?;
    let floor_robust = floor_robustness(input.arms, reference, &input.current, &input.floors)?;

    // §12 property 1: with a, b, g > 0 every winner is on F_off. It holds
    // by construction; checked in every build so a violation is an error,
    // never a silently wrong map.
    let off = layer_frontiers(&frontier_arms_of(input.arms))?.off;
    let all_winners = map
        .iter()
        .map(|p| &p.winners)
        .chain(std::iter::once(&current_winners))
        .chain(floor_robust.runs.iter().map(|(_, w)| w));
    for ws in all_winners {
        if ws.iter().any(|id| !off.contains(id)) {
            return Err(CoreError::InvalidField {
                field: "winner map",
                reason: "a winner is outside F_off (§12 property 1)",
            });
        }
    }
    Ok(Ok(SensitivityReport {
        map,
        current: input.current,
        current_winners,
        weight_robust,
        flip,
        floor_robust,
    }))
}

fn frontier_arms_of(arms: &[SensitivityArm]) -> Vec<FrontierArm> {
    arms.iter()
        .map(|a| {
            let t = Triple::new(a.point.c, a.point.t, a.point.q);
            FrontierArm {
                id: a.id.clone(),
                star: t,
                robust: t,
                robust_enabled: true,
            }
        })
        .collect()
}

fn in_neighbourhood(w: &Weights, current: &Weights) -> bool {
    let d = (w.a() - current.a())
        .abs()
        .max((w.b() - current.b()).abs())
        .max((w.g() - current.g()).abs());
    d <= NEIGHBOURHOOD_RADIUS + GRID_TOL
}

fn weight_robustness(
    arms: &[SensitivityArm],
    map: &[MapPoint],
    current: &Weights,
    has_self_report: bool,
) -> Result<WeightRobustness> {
    let region = if has_self_report {
        RobustRegion::SelfReport
    } else {
        RobustRegion::Neighbourhood
    };
    let mut shares: BTreeMap<&str, (u32, u32)> =
        arms.iter().map(|a| (a.id.as_str(), (0, 0))).collect();
    let mut total = 0u32;
    for p in map {
        let inside = match region {
            RobustRegion::SelfReport => p.in_omega_s,
            RobustRegion::Neighbourhood => in_neighbourhood(&p.weights, current),
        };
        if !inside {
            continue;
        }
        bump(&mut total, "weight-robust region size")?;
        for id in &p.winners {
            let e = shares.get_mut(id.as_str()).ok_or(CoreError::InvalidField {
                field: "winner map",
                reason: "winner is not a compared arm",
            })?;
            bump(&mut e.0, "weight-robust winner points")?;
            if p.winners.len() == 1 {
                bump(&mut e.1, "weight-robust sole-winner points")?;
            }
        }
    }
    if total == 0 {
        return Err(CoreError::InvalidField {
            field: "weight-robust region",
            reason: "region has no grid point",
        });
    }
    let everywhere = shares
        .iter()
        .find(|(_, &(_, sole))| sole == total)
        .map(|(id, _)| (*id).to_string());
    Ok(WeightRobustness {
        region,
        total,
        shares: shares
            .into_iter()
            .map(|(id, (points, sole_points))| ArmShare {
                id: id.to_string(),
                points,
                sole_points,
            })
            .collect(),
        everywhere,
    })
}

fn floor_robustness(
    arms: &[SensitivityArm],
    reference: Point,
    current: &Weights,
    floors: &Floors,
) -> Result<FloorRobustness> {
    let mut runs = Vec::with_capacity(FloorVariant::ALL.len());
    for v in FloorVariant::ALL {
        runs.push((v, winners(arms, reference, current, &v.apply(floors)?)?));
    }
    let robust = runs.iter().all(|(_, w)| *w == runs[0].1);
    Ok(FloorRobustness { runs, robust })
}

fn log_deltas(i: Point, j: Point, f: &Floors) -> Result<[f64; 3]> {
    let d = [
        (i.c + f.c0()).ln() - (j.c + f.c0()).ln(),
        (i.t + f.t0()).ln() - (j.t + f.t0()).ln(),
        (1.0 - i.q + f.eps()).ln() - (1.0 - j.q + f.eps()).ln(),
    ];
    for v in d {
        finite("flip line delta", v)?;
    }
    Ok(d)
}

/// The segment statement with dimension `fixed` held at `w_k`.
fn flip_statement(fixed: Dim, current: &Weights, delta: [f64; 3]) -> FlipStatement {
    let w = [current.a(), current.b(), current.g()];
    let k = fixed.index();
    let others: Vec<Dim> = Dim::ALL.into_iter().filter(|d| *d != fixed).collect();
    let (p, q) = (others[0], others[1]);
    let wk = w[k];
    let s = 1.0 - wk;
    let lo = crate::constants::WEIGHT_FLOOR;
    let hi = s - lo;
    let (dp, dq, dk) = (delta[p.index()], delta[q.index()], delta[k]);
    // f(w_q) = w_q Δ_q + (S − w_q) Δ_p + w_k Δ_k; the runner-up is ahead
    // where f > 0.
    let f = |wq: f64| wq * dq + (s - wq) * dp + wk * dk;
    let slope = dq - dp;
    let kind = if slope.abs() <= SLOPE_TOL {
        let v = f(lo);
        if v.abs() <= SLOPE_TOL {
            FlipKind::Tied
        } else if v > 0.0 {
            FlipKind::Always
        } else {
            FlipKind::Never
        }
    } else {
        let root = -(s * dp + wk * dk) / slope;
        if slope > 0.0 {
            // ahead for w_q > root
            if root < lo {
                FlipKind::Always
            } else if root < hi {
                FlipKind::Crosses {
                    over: q,
                    under: p,
                    ratio: root / (s - root),
                }
            } else {
                FlipKind::Never
            }
        } else if root > hi {
            // ahead for w_q < root
            FlipKind::Always
        } else if root > lo {
            FlipKind::Crosses {
                over: p,
                under: q,
                ratio: (s - root) / root,
            }
        } else {
            FlipKind::Never
        }
    };
    FlipStatement {
        fixed,
        fixed_weight: wk,
        kind,
    }
}

/// Flip line between the top two arms at `W_k` (§12 line 811).
pub fn flip_line(
    arms: &[SensitivityArm],
    reference: Point,
    current: &Weights,
    floors: &Floors,
) -> Result<std::result::Result<FlipLine, NoFlipLine>> {
    check_ids(arms)?;
    if arms.len() < 2 {
        return Ok(Err(NoFlipLine::TooFewArms));
    }
    let mut scored = Vec::with_capacity(arms.len());
    for a in arms {
        scored.push((value_score(reference, a.point, current, floors)?, a));
    }
    scored.sort_by(|x, y| y.0.total_cmp(&x.0).then_with(|| x.1.id.cmp(&y.1.id)));
    let tie = |x: f64, y: f64| -> Result<bool> {
        let g = highest_v_group(&[("x".to_string(), x), ("y".to_string(), y)])?;
        Ok(g.len() == 2)
    };
    // Rank 2 tied with rank 3 (this includes a top tie group of three or
    // more): "the top two" is not a pair.
    if scored.len() >= 3 && tie(scored[1].0, scored[2].0)? {
        return Ok(Err(NoFlipLine::AmbiguousRunnerUp));
    }
    let tied_now = tie(scored[0].0, scored[1].0)?;
    if tied_now && scored[1].1.id < scored[0].1.id {
        // Within the tie tolerance the order is display-only: by id.
        scored.swap(0, 1);
    }
    let (i, j) = (scored[0].1, scored[1].1);
    let delta = log_deltas(i.point, j.point, floors)?;
    let statements = Dim::ALL
        .into_iter()
        .map(|d| flip_statement(d, current, delta))
        .collect();
    Ok(Ok(FlipLine {
        leader: i.id.clone(),
        runner_up: j.id.clone(),
        tied_now,
        delta,
        statements,
    }))
}

/// The threshold as printed: rounded **up** to two decimals, so "超过
/// 印出的倍数" is a sufficient condition (a rounded-down or nearest value
/// could make the sentence false just above it).
pub fn printed_ratio(ratio: f64) -> f64 {
    let up = (ratio * 100.0).ceil() / 100.0;
    // ceil of a value that is already x.yz (up to float noise) stays x.yz.
    if (ratio * 100.0 - (ratio * 100.0).round()).abs() <= GRID_TOL {
        (ratio * 100.0).round() / 100.0
    } else {
        up
    }
}

/// One-sentence explanation of a flip statement, e.g. "保持质量权重不变，
/// 只要时间权重超过成本权重的 1.40 倍，B 就会超过 A；这是在当前估计值下的结论".
/// The ratio is [`printed_ratio`].
pub fn flip_sentence(line: &FlipLine, s: &FlipStatement) -> String {
    let (i, j) = (&line.leader, &line.runner_up);
    let head = format!("保持{}权重不变，", s.fixed.label());
    let body = match s.kind {
        FlipKind::Crosses { over, under, ratio } => format!(
            "只要{}权重超过{}权重的 {:.2} 倍，{j} 就会超过 {i}",
            over.label(),
            under.label(),
            printed_ratio(ratio)
        ),
        FlipKind::Never => format!("{j} 在任何可行权重下都不会超过 {i}"),
        FlipKind::Always => format!("{j} 在任何可行权重下都高于 {i}"),
        FlipKind::Tied => format!("{j} 与 {i} 在任何可行权重下都并列"),
    };
    format!("{head}{body}；{CURRENT_ESTIMATE_NOTE}")
}

/// One bootstrap resample as recomputed by the caller: the arms that are
/// in `E_d` for that resample with their triples, and the numerator anchor
/// (`None` when the reference falls to tier III in that resample).
#[derive(Debug, Clone, PartialEq)]
pub struct ResampleLayer {
    pub arms: Vec<SensitivityArm>,
    pub reference: Option<Point>,
}

/// Frequencies of one arm over the resamples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArmFrequency {
    pub id: String,
    /// Resamples in which the arm is in `Win(W_k)` (alone or tied).
    pub winner: u32,
    /// Resamples in which the arm is in `F_off`.
    pub frontier: u32,
}

/// Statistical robustness (§12 display item 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatisticalRobustness {
    pub seed: u64,
    /// Always [`BOOTSTRAP_RESAMPLES`] (200).
    pub resamples: u32,
    /// Resamples with a numerator anchor and at least one arm, i.e. with a
    /// non-empty winner set.
    pub with_value: u32,
    /// Resamples whose `Win(W_k)` had more than one arm. The winner counts
    /// add up to `Σ |Win(W_k)|` over resamples, not `with_value + tied`.
    pub tied: u32,
    /// One entry per arm of `arm_ids`, in that order.
    pub arms: Vec<ArmFrequency>,
}

/// xorshift64 (Marsaglia 2003): shifts and xors only, no wrapping
/// arithmetic. The state is never 0.
#[derive(Debug, Clone)]
pub struct Xorshift64(u64);

impl Xorshift64 {
    /// Seed must be non-zero (a zero state stays zero).
    pub fn new(seed: u64) -> Result<Self> {
        if seed == 0 {
            return Err(CoreError::InvalidField {
                field: "bootstrap seed",
                reason: "must be non-zero",
            });
        }
        Ok(Self(seed))
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform in `0..n` by rejection (no modulo bias); `n > 0`.
    pub fn below(&mut self, n: u64) -> Result<u64> {
        if n == 0 {
            return Err(CoreError::InvalidField {
                field: "bootstrap stratum",
                reason: "cannot draw from an empty range",
            });
        }
        // Largest multiple of n that fits; values at or above it are
        // rejected so every residue is equally likely.
        let zone = (u64::MAX / n).checked_mul(n).ok_or(CoreError::Overflow {
            what: "bootstrap rejection zone",
        })?;
        loop {
            let x = self.next_u64();
            if x < zone {
                return Ok(x % n);
            }
        }
    }
}

/// One task of the layer, as seen by the bootstrap: its arm and its design
/// cell. Probe tasks (`ω = 0`) must not be passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapTask {
    pub arm: String,
    pub cell: CellId,
}

/// The bootstrap strata: task positions grouped by `(arm, cell)`, strata in
/// `(arm, cell)` order, positions ascending (DECISIONS 12.6).
pub fn bootstrap_strata(tasks: &[BootstrapTask]) -> Result<Vec<Vec<usize>>> {
    let mut groups: BTreeMap<(&str, &CellId), Vec<usize>> = BTreeMap::new();
    for (pos, t) in tasks.iter().enumerate() {
        if t.arm.trim().is_empty() {
            return Err(CoreError::EmptyField {
                field: "bootstrap task arm",
            });
        }
        groups
            .entry((t.arm.as_str(), &t.cell))
            .or_default()
            .push(pos);
    }
    Ok(groups.into_values().collect())
}

/// Task bootstrap (§12 line 810): 200 resamples, stratified by
/// `(arm, cell)` (DECISIONS 12.6). In each resample every `(arm, cell)`
/// stratum of `n_iu` tasks is redrawn with replacement to exactly `n_iu`
/// tasks, so `ω_u`, the design grid and every `n_iu` stay fixed and only
/// task outcomes, costs, times and ratings vary. The reference and fallback
/// arms are resampled the same way (their tasks are in `tasks` like any
/// other arm's).
///
/// `eval` gets the drawn task positions (into `tasks`; strata in
/// [`bootstrap_strata`] order) and returns that resample's recomputed
/// `E_d` arms and anchor. `arm_ids` lists every arm to report; every task
/// arm and every arm returned by `eval` must be in it.
pub fn statistical_robustness<F>(
    tasks: &[BootstrapTask],
    seed: u64,
    arm_ids: &[String],
    current: &Weights,
    floors: &Floors,
    mut eval: F,
) -> Result<StatisticalRobustness>
where
    F: FnMut(&[usize]) -> Result<ResampleLayer>,
{
    if arm_ids.is_empty() {
        return Err(CoreError::EmptyField {
            field: "bootstrap arm ids",
        });
    }
    let mut seen = BTreeSet::new();
    for id in arm_ids {
        if id.trim().is_empty() {
            return Err(CoreError::EmptyField {
                field: "bootstrap arm id",
            });
        }
        if !seen.insert(id.as_str()) {
            return Err(CoreError::DuplicateCell { cell: id.clone() });
        }
    }
    for t in tasks {
        if !seen.contains(t.arm.as_str()) {
            return Err(CoreError::InvalidField {
                field: "bootstrap task",
                reason: "task arm is not in arm_ids",
            });
        }
    }
    let strata = bootstrap_strata(tasks)?;
    let mut rng = Xorshift64::new(seed)?;
    let mut counts: BTreeMap<&str, (u32, u32)> =
        arm_ids.iter().map(|id| (id.as_str(), (0, 0))).collect();
    let mut resamples = 0u32;
    let mut with_value = 0u32;
    let mut tied = 0u32;
    for _ in 0..BOOTSTRAP_RESAMPLES {
        let mut draw = Vec::with_capacity(tasks.len());
        for stratum in &strata {
            let n64 = u64::try_from(stratum.len()).map_err(|_| CoreError::Overflow {
                what: "bootstrap stratum size",
            })?;
            for _ in 0..stratum.len() {
                let k = rng.below(n64)?;
                let at = usize::try_from(k).map_err(|_| CoreError::Overflow {
                    what: "bootstrap index",
                })?;
                draw.push(*stratum.get(at).ok_or(CoreError::OutOfRange {
                    what: "bootstrap index",
                    value: k as f64,
                    min: 0.0,
                    max: n64 as f64,
                })?);
            }
        }
        let layer = eval(&draw)?;
        check_ids(&layer.arms)?;
        for a in &layer.arms {
            if !counts.contains_key(a.id.as_str()) {
                return Err(CoreError::InvalidField {
                    field: "bootstrap resample",
                    reason: "arm is not in arm_ids",
                });
            }
        }
        bump(&mut resamples, "bootstrap resamples")?;
        let frontier_arms = frontier_arms_of(&layer.arms);
        for id in layer_frontiers(&frontier_arms)?.off {
            if let Some(e) = counts.get_mut(id.as_str()) {
                bump(&mut e.1, "bootstrap frontier count")?;
            }
        }
        if let (Some(reference), false) = (layer.reference, layer.arms.is_empty()) {
            bump(&mut with_value, "bootstrap resamples with V")?;
            let win = winners(&layer.arms, reference, current, floors)?;
            if win.len() > 1 {
                bump(&mut tied, "bootstrap tied resamples")?;
            }
            for id in win {
                if let Some(e) = counts.get_mut(id.as_str()) {
                    bump(&mut e.0, "bootstrap winner count")?;
                }
            }
        }
    }
    Ok(StatisticalRobustness {
        seed,
        resamples,
        with_value,
        tied,
        arms: arm_ids
            .iter()
            .map(|id| {
                let (winner, frontier) = counts[id.as_str()];
                ArmFrequency {
                    id: id.clone(),
                    winner,
                    frontier,
                }
            })
            .collect(),
    })
}

#[cfg(test)]
mod sensitivity_unit_tests {
    use super::*;

    #[test]
    fn bump_overflow_is_an_error_not_a_wrap() {
        let mut c = u32::MAX;
        assert_eq!(
            bump(&mut c, "test counter"),
            Err(CoreError::Overflow {
                what: "test counter"
            })
        );
        assert_eq!(c, u32::MAX);
        let mut c = u32::MAX - 1;
        bump(&mut c, "test counter").unwrap();
        assert_eq!(c, u32::MAX);
    }

    #[test]
    fn grid_key_rejects_off_grid_weights() {
        let w = Weights::new(0.2, 0.35, 0.45).unwrap();
        assert_eq!(grid_key(&w), Some((4, 7, 9)));
        assert_eq!(grid_key(&Weights::equal()), None);
    }

    #[test]
    fn below_is_in_range_and_rejects_zero() {
        let mut r = Xorshift64::new(7).unwrap();
        for n in [1u64, 2, 3, 171, u64::MAX] {
            for _ in 0..100 {
                assert!(r.below(n).unwrap() < n);
            }
        }
        assert!(r.below(0).is_err());
    }
}

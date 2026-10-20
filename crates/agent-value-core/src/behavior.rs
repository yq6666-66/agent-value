//! Behaviour calibration (§10.3): suggestions only, never applied to the
//! weights automatically.
//!
//! Model: `Pr(j | S) = exp(-theta . x_j) / sum_l exp(-theta . x_l)` with
//! `x = (ln(C* + c0), ln(T* + t0), ln(1 - Q* + eps))` and `theta >= 0`.
//! `lambda_hat = |theta|_1`, `w_b = theta / lambda_hat`.

// Small fixed-size matrix code reads best with explicit indices.
#![allow(clippy::needless_range_loop)]

use crate::constants::{
    BEHAVIOR_MIN_LAMBDA, BEHAVIOR_MIN_RANK, BOOTSTRAP_RESAMPLES, C0_YUAN, EPSILON, N_EV, T0_SECONDS,
};
use crate::grid::{grid, GridPoint};
use crate::value::Point;
use crate::window::WindowIndex;
use crate::{CoreError, Result};

/// `Omega_b` width: half the 95% quantile of chi-square with 2 d.o.f.
pub const SUPPORT_DROP: f64 = 3.0;
/// Relative pivot tolerance of the rank test (DECISIONS G6).
pub const RANK_TOLERANCE: f64 = 1e-9;
/// Projected-gradient tolerance per event (DECISIONS G7).
pub const FIT_TOLERANCE: f64 = 1e-9;
/// Newton iteration cap (DECISIONS G7).
pub const FIT_MAX_ITERATIONS: usize = 200;
/// `|theta|_1` above this means the likelihood has no maximum (DECISIONS G7).
pub const THETA_DIVERGENCE: f64 = 1e6;
/// Smallest eigenvalue of the information matrix per event below this means
/// the maximum is not identified (DECISIONS G7).
pub const INFORMATION_FLOOR: f64 = 1e-8;
/// Tolerance of the separation pre-check (DECISIONS G7). Each constraint
/// `x_l - x_chosen` is scaled so its largest component is 1, and directions
/// lie on the simplex, so the tolerance is on a dimensionless quantity of
/// order 1: far above double rounding in the polygon clipping (about 1e-15)
/// and far below any meaningful log difference.
pub const SEPARATION_TOLERANCE: f64 = 1e-9;
/// Fewer successful bootstrap resamples than this is flagged (DECISIONS G8).
pub const BOOTSTRAP_MIN_OK: usize = 100;

/// One option of a choice event with its window `t-1` triple in the event's
/// cell. `None` when the arm has no complete triple there.
#[derive(Debug, Clone, PartialEq)]
pub struct EventOption {
    pub arm: String,
    pub value: Option<Point>,
}

/// One choice event `e`: options `S_e` (forced choices already removed by
/// the caller) and the arm actually chosen.
#[derive(Debug, Clone, PartialEq)]
pub struct ChoiceEvent {
    pub id: String,
    pub options: Vec<EventOption>,
    pub chosen: String,
}

/// Why behaviour gives no suggestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NoSignalReason {
    /// Fewer than `n_ev = 20` usable events.
    TooFewEvents,
    /// Fewer than 2 distinct chosen arms (DECISIONS G6).
    OneChosenArm,
    /// Attribute-difference matrix rank `< 3`.
    RankBelowThree,
    /// `lambda_hat < 0.5`.
    LambdaBelowMin,
    /// No finite maximum, a flat maximum, or no convergence (DECISIONS G7).
    NotIdentifiable,
}

/// A usable event: attribute vectors of its options, chosen index first
/// recorded separately.
#[derive(Debug, Clone, PartialEq)]
struct Prepared {
    xs: Vec<[f64; 3]>,
    chosen: usize,
    chosen_arm: String,
}

fn attributes(p: &Point) -> Result<[f64; 3]> {
    for (what, v) in [
        ("event cost", p.c),
        ("event time", p.t),
        ("event quality", p.q),
    ] {
        if !v.is_finite() {
            return Err(CoreError::NotFinite { what, value: v });
        }
    }
    if p.c < 0.0 || p.t < 0.0 || !(0.0..=1.0).contains(&p.q) {
        return Err(CoreError::OutOfRange {
            what: "event triple",
            value: if p.c < 0.0 {
                p.c
            } else if p.t < 0.0 {
                p.t
            } else {
                p.q
            },
            min: 0.0,
            max: f64::INFINITY,
        });
    }
    Ok([
        (p.c + C0_YUAN).ln(),
        (p.t + T0_SECONDS).ln(),
        (1.0 - p.q + EPSILON).ln(),
    ])
}

/// Validates events and drops whole events with a missing triple (§10.3).
/// Returns the usable events and the number dropped.
fn prepare(events: &[ChoiceEvent]) -> Result<(Vec<Prepared>, usize)> {
    let mut out = Vec::new();
    let mut dropped = 0;
    for (n, e) in events.iter().enumerate() {
        if events[..n].iter().any(|o| o.id == e.id) {
            return Err(CoreError::InvalidField {
                field: "events",
                reason: "an event id appears twice",
            });
        }
        for (i, o) in e.options.iter().enumerate() {
            if e.options[..i].iter().any(|p| p.arm == o.arm) {
                return Err(CoreError::DuplicateCell {
                    cell: o.arm.clone(),
                });
            }
        }
        let Some(chosen) = e.options.iter().position(|o| o.arm == e.chosen) else {
            return Err(CoreError::InvalidField {
                field: "events",
                reason: "the chosen arm is not among the options",
            });
        };
        if e.options.len() < 2 || e.options.iter().any(|o| o.value.is_none()) {
            dropped += 1;
            continue;
        }
        let xs = e
            .options
            .iter()
            .map(|o| attributes(o.value.as_ref().expect("checked above")))
            .collect::<Result<Vec<_>>>()?;
        out.push(Prepared {
            xs,
            chosen,
            chosen_arm: e.chosen.clone(),
        });
    }
    Ok((out, dropped))
}

/// Rank of the stacked `x_chosen - x_l` rows, full-pivot elimination with
/// relative tolerance [`RANK_TOLERANCE`] (DECISIONS G6).
fn difference_rank(events: &[Prepared]) -> usize {
    let mut rows: Vec<[f64; 3]> = Vec::new();
    for e in events {
        let xj = e.xs[e.chosen];
        for (l, xl) in e.xs.iter().enumerate() {
            if l != e.chosen {
                rows.push([xj[0] - xl[0], xj[1] - xl[1], xj[2] - xl[2]]);
            }
        }
    }
    let scale = rows
        .iter()
        .flat_map(|r| r.iter())
        .fold(0.0f64, |m, v| m.max(v.abs()));
    if scale == 0.0 {
        return 0;
    }
    let tol = RANK_TOLERANCE * scale;
    let mut cols: Vec<usize> = vec![0, 1, 2];
    let mut rank = 0;
    let mut start = 0;
    while !cols.is_empty() && start < rows.len() {
        let mut best = (0.0f64, start, cols[0]);
        for (r, row) in rows.iter().enumerate().skip(start) {
            for &c in &cols {
                if row[c].abs() > best.0 {
                    best = (row[c].abs(), r, c);
                }
            }
        }
        if best.0 <= tol {
            break;
        }
        let (_, pr, pc) = best;
        rows.swap(start, pr);
        let pivot = rows[start];
        for row in rows.iter_mut().skip(start + 1) {
            let f = row[pc] / pivot[pc];
            for c in 0..3 {
                row[c] -= f * pivot[c];
            }
        }
        cols.retain(|&c| c != pc);
        rank += 1;
        start += 1;
    }
    rank
}

/// Log-likelihood, gradient and Hessian at `theta`.
fn loglik(events: &[Prepared], theta: [f64; 3]) -> (f64, [f64; 3], [[f64; 3]; 3]) {
    let mut ll = 0.0;
    let mut g = [0.0; 3];
    let mut h = [[0.0; 3]; 3];
    for e in events {
        let us: Vec<f64> =
            e.xs.iter()
                .map(|x| -(theta[0] * x[0] + theta[1] * x[1] + theta[2] * x[2]))
                .collect();
        let mx = us.iter().fold(f64::NEG_INFINITY, |m, v| m.max(*v));
        let ws: Vec<f64> = us.iter().map(|u| (u - mx).exp()).collect();
        let z: f64 = ws.iter().sum();
        ll += us[e.chosen] - mx - z.ln();
        let mut mean = [0.0; 3];
        for (w, x) in ws.iter().zip(&e.xs) {
            for k in 0..3 {
                mean[k] += w / z * x[k];
            }
        }
        for k in 0..3 {
            g[k] += mean[k] - e.xs[e.chosen][k];
        }
        for (w, x) in ws.iter().zip(&e.xs) {
            let p = w / z;
            for a in 0..3 {
                for b in 0..3 {
                    h[a][b] -= p * (x[a] - mean[a]) * (x[b] - mean[b]);
                }
            }
        }
    }
    (ll, g, h)
}

/// Solves `m d = r` for the free coordinates (positive definite `m`) by
/// Cholesky. `None` when `m` is not positive definite.
fn solve_spd(m: &[[f64; 3]; 3], r: &[f64; 3], free: &[usize]) -> Option<[f64; 3]> {
    let n = free.len();
    let mut l = [[0.0f64; 3]; 3];
    for i in 0..n {
        for j in 0..=i {
            let mut s = m[free[i]][free[j]];
            for k in 0..j {
                s -= l[i][k] * l[j][k];
            }
            if i == j {
                if s <= 0.0 {
                    return None;
                }
                l[i][i] = s.sqrt();
            } else {
                l[i][j] = s / l[j][j];
            }
        }
    }
    let mut y = [0.0; 3];
    for i in 0..n {
        let mut s = r[free[i]];
        for k in 0..i {
            s -= l[i][k] * y[k];
        }
        y[i] = s / l[i][i];
    }
    let mut x = [0.0; 3];
    for i in (0..n).rev() {
        let mut s = y[i];
        for k in i + 1..n {
            s -= l[k][i] * x[k];
        }
        x[i] = s / l[i][i];
    }
    let mut d = [0.0; 3];
    for (i, &c) in free.iter().enumerate() {
        d[c] = x[i];
    }
    Some(d)
}

/// Result of the constrained maximum-likelihood fit.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Fit {
    theta: [f64; 3],
    ll: f64,
}

fn dot(d: &[f64; 3], v: &[f64; 3]) -> f64 {
    d[0] * v[0] + d[1] * v[1] + d[2] * v[2]
}

/// Separation pre-check (DECISIONS G7). The maximum of the likelihood on
/// `theta >= 0` is not attained exactly when some `d >= 0`, `d != 0`, has
/// `d . (x_l - x_chosen) >= 0` for every event and every other option, with
/// at least one inequality strict: moving along `d` then never lowers and
/// somewhere raises the likelihood. Identical values give differences of
/// exactly 0, which constrain nothing.
fn separated(events: &[Prepared]) -> bool {
    let mut constraints = Vec::new();
    for e in events {
        let xj = e.xs[e.chosen];
        for (l, x) in e.xs.iter().enumerate() {
            if l == e.chosen {
                continue;
            }
            let v = [x[0] - xj[0], x[1] - xj[1], x[2] - xj[2]];
            let m = v.iter().fold(0.0f64, |m, c| m.max(c.abs()));
            if m > 0.0 {
                constraints.push([v[0] / m, v[1] / m, v[2] / m]);
            }
        }
    }
    separating_direction(&constraints)
}

/// Is there `d` on the simplex with `d . v >= 0` for every `v` and `> 0`
/// for some? The feasible set is the simplex cut by one half-plane per
/// constraint, kept as a convex polygon (possibly an edge or a point) of
/// barycentric vertices. A linear function that is 0 at every vertex is 0
/// on the whole polygon, so strictness is checked at the vertices.
fn separating_direction(constraints: &[[f64; 3]]) -> bool {
    let tol = SEPARATION_TOLERANCE;
    let mut poly: Vec<[f64; 3]> = vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for v in constraints {
        let f: Vec<f64> = poly.iter().map(|d| dot(d, v)).collect();
        if f.iter().all(|&x| x >= -tol) {
            continue;
        }
        let k = poly.len();
        let mut out: Vec<[f64; 3]> = Vec::with_capacity(k + 1);
        for i in 0..k {
            let (a, fa) = (poly[i], f[i]);
            let (b, fb) = (poly[(i + 1) % k], f[(i + 1) % k]);
            if fa >= -tol {
                out.push(a);
            }
            if (fa > tol && fb < -tol) || (fa < -tol && fb > tol) {
                let t = fa / (fa - fb);
                out.push([
                    a[0] + t * (b[0] - a[0]),
                    a[1] + t * (b[1] - a[1]),
                    a[2] + t * (b[2] - a[2]),
                ]);
            }
        }
        out.dedup();
        while out.len() > 1 && out.first() == out.last() {
            out.pop();
        }
        if out.is_empty() {
            return false;
        }
        poly = out;
    }
    poly.iter()
        .any(|d| constraints.iter().any(|v| dot(d, v) > tol))
}

/// Information on the free coordinates (`-H`) minus
/// `INFORMATION_FLOOR * n * I` must be positive definite (DECISIONS G7).
fn identified(h: &[[f64; 3]; 3], free: &[usize], n: f64) -> bool {
    let mut info = [[0.0; 3]; 3];
    for a in 0..3 {
        for b in 0..3 {
            info[a][b] = -h[a][b];
        }
    }
    for &k in free {
        info[k][k] -= INFORMATION_FLOOR * n;
    }
    free.is_empty() || solve_spd(&info, &[0.0; 3], free).is_some()
}

/// Projected Newton on `theta >= 0` (DECISIONS G7). `None` when the
/// maximum is not identified.
fn fit(events: &[Prepared]) -> Option<Fit> {
    if separated(events) {
        return None;
    }
    let n = events.len().max(1) as f64;
    let mut theta = [1.0 / 3.0; 3];
    let (mut ll, mut g, mut h) = loglik(events, theta);
    for _ in 0..FIT_MAX_ITERATIONS {
        let free: Vec<usize> = (0..3).filter(|&k| theta[k] > 0.0 || g[k] > 0.0).collect();
        let pg = free.iter().fold(0.0f64, |m, &k| m.max(g[k].abs()));
        if pg <= FIT_TOLERANCE * n {
            return identified(&h, &free, n).then_some(Fit { theta, ll });
        }
        let mut neg_h = [[0.0; 3]; 3];
        for a in 0..3 {
            for b in 0..3 {
                neg_h[a][b] = -h[a][b];
            }
        }
        let dir = solve_spd(&neg_h, &g, &free).unwrap_or_else(|| {
            let mut d = [0.0; 3];
            for &k in &free {
                d[k] = g[k];
            }
            d
        });
        let mut step = 1.0;
        let mut moved = false;
        for _ in 0..60 {
            let cand = [
                (theta[0] + step * dir[0]).max(0.0),
                (theta[1] + step * dir[1]).max(0.0),
                (theta[2] + step * dir[2]).max(0.0),
            ];
            let (cll, cg, ch) = loglik(events, cand);
            let gain: f64 = (0..3).map(|k| g[k] * (cand[k] - theta[k])).sum();
            if cll.is_finite() && cll >= ll + 1e-4 * gain && cll >= ll {
                theta = cand;
                ll = cll;
                g = cg;
                h = ch;
                moved = true;
                break;
            }
            step /= 2.0;
        }
        if theta.iter().sum::<f64>() > THETA_DIVERGENCE {
            return None;
        }
        if !moved {
            // No further ascent possible in floating point: accept only if
            // already essentially stationary.
            return (pg <= 1e3 * FIT_TOLERANCE * n && identified(&h, &free, n))
                .then_some(Fit { theta, ll });
        }
    }
    None
}

/// Profile log-likelihood `L^prof(w) = sup_{lambda > 0} L(lambda w)`
/// (DECISIONS G9). `f(lambda) = L(lambda w)` is concave. When the right
/// derivative at 0 is `<= 0`, `f` does not increase on `lambda > 0` and the
/// supremum is `f(0) = -sum_e ln |S_e|`. Otherwise the maximum is interior
/// and is found by bisection on `f'`, keeping `lambda > 0`.
pub fn profile_loglik(events: &[ChoiceEvent], w: [f64; 3]) -> Result<f64> {
    for v in w {
        if !v.is_finite() {
            return Err(CoreError::NotFinite {
                what: "profile weight",
                value: v,
            });
        }
        if v < 0.0 {
            return Err(CoreError::OutOfRange {
                what: "profile weight",
                value: v,
                min: 0.0,
                max: 1.0,
            });
        }
    }
    let (prepared, _) = prepare(events)?;
    Ok(profile(&prepared, w))
}

fn profile(events: &[Prepared], w: [f64; 3]) -> f64 {
    let at = |lam: f64| loglik(events, [lam * w[0], lam * w[1], lam * w[2]]);
    let slope = |lam: f64| {
        let (_, g, _) = at(lam);
        g[0] * w[0] + g[1] * w[1] + g[2] * w[2]
    };
    if slope(0.0) <= 0.0 {
        return at(0.0).0;
    }
    let mut hi = 1.0;
    while slope(hi) > 0.0 && hi < THETA_DIVERGENCE {
        hi *= 2.0;
    }
    let mut lo = 0.0;
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if slope(mid) > 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
        if hi - lo <= 1e-12 * hi.max(1.0) {
            break;
        }
    }
    at(0.5 * (lo + hi)).0
}

/// SplitMix64, the bootstrap generator (DECISIONS G8).
#[derive(Debug, Clone)]
pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform index in `0..n` (`n > 0`).
    pub fn below(&mut self, n: usize) -> usize {
        ((self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64) * n as f64) as usize % n
    }
}

/// Bootstrap summary of `w_b` (DECISIONS G8).
#[derive(Debug, Clone, PartialEq)]
pub struct Bootstrap {
    pub ok: usize,
    pub failed: usize,
    /// Per dimension (a, b, g): 5%, 50%, 95% quantiles (lower quantile rule).
    pub quantiles: [[f64; 3]; 3],
    /// Fewer than [`BOOTSTRAP_MIN_OK`] successful resamples.
    pub unstable: bool,
}

/// Fewer than [`BOOTSTRAP_MIN_OK`] successful resamples is flagged as
/// unstable (DECISIONS G8).
pub fn bootstrap_unstable(ok: usize) -> bool {
    ok < BOOTSTRAP_MIN_OK
}

fn lower_quantile(sorted: &[f64], p: f64) -> f64 {
    let n = sorted.len();
    for (k, v) in sorted.iter().enumerate() {
        if (k + 1) as f64 / n as f64 >= p - 1e-12 {
            return *v;
        }
    }
    sorted[n - 1]
}

fn normalized(theta: [f64; 3]) -> [f64; 3] {
    let s = theta[0] + theta[1] + theta[2];
    [theta[0] / s, theta[1] / s, theta[2] / s]
}

fn bootstrap(events: &[Prepared], seed: u64) -> Bootstrap {
    let mut rng = SplitMix64::new(seed);
    let mut samples: Vec<[f64; 3]> = Vec::new();
    let mut failed = 0;
    for _ in 0..BOOTSTRAP_RESAMPLES {
        let pick: Vec<Prepared> = (0..events.len())
            .map(|_| events[rng.below(events.len())].clone())
            .collect();
        let res = if difference_rank(&pick) < BEHAVIOR_MIN_RANK {
            None
        } else {
            fit(&pick)
        };
        match res {
            Some(f) if f.theta.iter().sum::<f64>() > 0.0 => samples.push(normalized(f.theta)),
            _ => failed += 1,
        }
    }
    let mut quantiles = [[f64::NAN; 3]; 3];
    if !samples.is_empty() {
        for d in 0..3 {
            let mut v: Vec<f64> = samples.iter().map(|s| s[d]).collect();
            v.sort_by(f64::total_cmp);
            quantiles[d] = [
                lower_quantile(&v, 0.05),
                lower_quantile(&v, 0.50),
                lower_quantile(&v, 0.95),
            ];
        }
    }
    Bootstrap {
        ok: samples.len(),
        failed,
        quantiles,
        unstable: bootstrap_unstable(samples.len()),
    }
}

/// Behaviour calibration result.
#[derive(Debug, Clone, PartialEq)]
pub struct Behavior {
    pub events_used: usize,
    pub events_dropped: usize,
    /// Empty when there is a signal.
    pub no_signal: Vec<NoSignalReason>,
    pub theta: Option<[f64; 3]>,
    pub lambda: Option<f64>,
    /// `w_b = theta / lambda`, not moved to the grid (may be below 0.05).
    pub w_b: Option<[f64; 3]>,
    /// `L(theta_hat) = L^prof(w_b)`, the `Omega_b` reference (DECISIONS G9).
    pub ll_hat: Option<f64>,
    /// `L^prof(w)` on every grid point (only with a signal).
    pub profile: Vec<(GridPoint, f64)>,
    /// `Omega_b` (only with a signal). Can be empty when `w_b` lies off the
    /// grid and the likelihood is sharp.
    pub support: Vec<GridPoint>,
    pub bootstrap: Option<Bootstrap>,
}

impl Behavior {
    pub fn has_signal(&self) -> bool {
        self.no_signal.is_empty()
    }
}

/// Checks that behaviour from `events_window` may feed weights used in
/// `scored_window` (§10.3: only window `t-1` and earlier; DECISIONS G10).
pub fn check_windows(events_window: WindowIndex, scored_window: WindowIndex) -> Result<()> {
    if events_window.0 >= scored_window.0 {
        return Err(CoreError::InvalidField {
            field: "events window",
            reason: "behaviour must come from a window before the scored window",
        });
    }
    Ok(())
}

/// Runs §10.3 on the events of window `t-1`. `seed` drives the bootstrap.
pub fn calibrate(
    events: &[ChoiceEvent],
    events_window: WindowIndex,
    scored_window: WindowIndex,
    seed: u64,
) -> Result<Behavior> {
    check_windows(events_window, scored_window)?;
    let (prepared, dropped) = prepare(events)?;
    let mut reasons = Vec::new();
    if prepared.len() < N_EV {
        reasons.push(NoSignalReason::TooFewEvents);
    }
    let mut chosen: Vec<&str> = prepared.iter().map(|e| e.chosen_arm.as_str()).collect();
    chosen.sort_unstable();
    chosen.dedup();
    if chosen.len() < 2 {
        reasons.push(NoSignalReason::OneChosenArm);
    }
    let rank_ok = difference_rank(&prepared) >= BEHAVIOR_MIN_RANK;
    if !rank_ok {
        reasons.push(NoSignalReason::RankBelowThree);
    }
    let fitted = if rank_ok { fit(&prepared) } else { None };
    let (mut theta, mut lambda, mut w_b, mut ll_hat) = (None, None, None, None);
    if rank_ok {
        match fitted {
            None => reasons.push(NoSignalReason::NotIdentifiable),
            Some(f) => {
                let lam: f64 = f.theta.iter().sum();
                theta = Some(f.theta);
                lambda = Some(lam);
                ll_hat = Some(f.ll);
                if lam < BEHAVIOR_MIN_LAMBDA {
                    reasons.push(NoSignalReason::LambdaBelowMin);
                }
                if lam > 0.0 {
                    w_b = Some(normalized(f.theta));
                }
            }
        }
    }
    reasons.sort();
    reasons.dedup();
    let mut out = Behavior {
        events_used: prepared.len(),
        events_dropped: dropped,
        no_signal: reasons,
        theta,
        lambda,
        w_b,
        ll_hat,
        profile: Vec::new(),
        support: Vec::new(),
        bootstrap: None,
    };
    if out.has_signal() {
        let best = fitted.expect("signal implies a fit").ll;
        // theta = 0 is rejected by the lambda gate and never reaches Omega_b.
        debug_assert!(lambda.is_some_and(|l| l >= BEHAVIOR_MIN_LAMBDA));
        // Omega_b threshold is L(theta_hat) = L^prof(w_b); w_b is not moved
        // to the grid (DECISIONS G9).
        out.profile = grid()
            .into_iter()
            .map(|w| (w, profile(&prepared, w.as_array())))
            .collect();
        out.support = out
            .profile
            .iter()
            .filter(|(_, l)| *l >= best - SUPPORT_DROP)
            .map(|(w, _)| *w)
            .collect();
        out.bootstrap = Some(bootstrap(&prepared, seed));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn neg(info: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
        let mut h = info;
        for row in &mut h {
            for v in row.iter_mut() {
                *v = -*v;
            }
        }
        h
    }

    /// The check is on the smallest eigenvalue of the information on the
    /// free coordinates, against an absolute floor of 1e-8 * n (DECISIONS
    /// G7): a matrix that is singular along one direction only fails it,
    /// and so does one that shrinks evenly in every direction.
    #[test]
    fn identified_is_a_smallest_eigenvalue_floor() {
        let n = 30.0;
        let all = [0, 1, 2];
        // Rank 3 but tiny along e1 (quasi-complete separation).
        assert!(!identified(
            &neg([[1e-9, 0.0, 0.0], [0.0, 1.0, 0.2], [0.0, 0.2, 1.0]]),
            &all,
            n
        ));
        // Same, tiny along (1, 1, 0) / sqrt(2): eigenvalues 1e-9, 2, 1.
        let (a, b) = ((2.0 + 1e-9) / 2.0, (1e-9 - 2.0) / 2.0);
        assert!(!identified(
            &neg([[a, b, 0.0], [b, a, 0.0], [0.0, 0.0, 1.0]]),
            &all,
            n
        ));
        // Shrinks evenly (complete separation): min/max ratio is 1, still rejected.
        assert!(!identified(
            &neg([[1e-8, 0.0, 0.0], [0.0, 1e-8, 0.0], [0.0, 0.0, 1e-8]]),
            &all,
            n
        ));
        // Just above and below the floor 3e-7 along e1.
        assert!(identified(
            &neg([[3.1e-7, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]),
            &all,
            n
        ));
        assert!(!identified(
            &neg([[2.9e-7, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]),
            &all,
            n
        ));
        // Only free coordinates count.
        assert!(identified(
            &neg([[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]),
            &[1, 2],
            n
        ));
        assert!(identified(&[[0.0; 3]; 3], &[], n));
    }

    fn sep(c: &[[f64; 3]]) -> bool {
        separating_direction(c)
    }

    #[test]
    fn separation_polygon_edge_cases() {
        // No constraints, or only zero differences: nothing strict.
        assert!(!sep(&[]));
        // One strict constraint: the whole simplex is feasible.
        assert!(sep(&[[1.0, 0.0, 0.0]]));
        // Every direction lowers some choice: empty.
        assert!(!sep(&[[-1.0, -1.0, -1.0]]));
        assert!(!sep(&[
            [1.0, -1.0, 0.0],
            [-1.0, 0.0, 1.0],
            [0.0, 1.0, -1.0],
            [-0.5, -0.5, 0.2]
        ]));
        // Cut through a vertex: d1 >= d2 keeps a triangle with strict points.
        assert!(sep(&[[0.0, 1.0, -1.0]]));
        // Two opposite cuts through vertex e0 leave the segment d1 = d2, on
        // which both are equalities: not separated.
        assert!(!sep(&[[0.0, 1.0, -1.0], [0.0, -1.0, 1.0]]));
        // ... plus a constraint strict at e0: separated.
        assert!(sep(&[[0.0, 1.0, -1.0], [0.0, -1.0, 1.0], [1.0, 0.0, 0.0]]));
        // Reduced to the single vertex e0, only equalities there.
        assert!(!sep(&[[0.0, -1.0, 0.0], [0.0, 0.0, -1.0]]));
        // ... and with a constraint strict at e0 (the e^25 shape).
        assert!(sep(&[
            [0.0, -1.0, 0.0],
            [0.0, 0.0, -1.0],
            [1.0, -1.0, -1.0]
        ]));
        // Reduced to the centre point, where every constraint is 0.
        let centre = [
            [1.0, -1.0, 0.0],
            [-1.0, 1.0, 0.0],
            [0.0, 1.0, -1.0],
            [0.0, -1.0, 1.0],
        ];
        assert!(!sep(&centre));
        let mut more = centre.to_vec();
        more.push([1.0, 1.0, -2.0]);
        assert!(!sep(&more));
        more.push([1.0, 0.5, 0.0]);
        assert!(sep(&more));
        // Order of constraints does not matter.
        let mut rev = more.clone();
        rev.reverse();
        assert!(sep(&rev));
    }

    fn prep(xs: Vec<[f64; 3]>, chosen: usize) -> Prepared {
        Prepared {
            xs,
            chosen,
            chosen_arm: chosen.to_string(),
        }
    }

    #[test]
    fn identical_values_give_exact_zero_differences() {
        // Same cell values give the same logs, so the difference is exactly
        // 0 and is not a constraint; only the real difference counts.
        let x = attributes(&Point {
            c: 0.37,
            t: 61.0,
            q: 0.83,
        })
        .unwrap();
        let y = attributes(&Point {
            c: 0.37,
            t: 75.0,
            q: 0.83,
        })
        .unwrap();
        // Chosen has the lower time: separated along time.
        assert!(separated(&[prep(vec![x, y], 0)]));
        // Equal options: no constraint at all.
        assert!(!separated(&[prep(vec![x, x], 0)]));
        // One choice each way along time: not separated.
        assert!(!separated(&[prep(vec![x, y], 0), prep(vec![x, y], 1)]));
    }

    #[test]
    fn separation_is_scale_invariant() {
        // Differences of about 1e-10 still separate: each constraint is
        // scaled to a largest component of 1 before the tolerance applies.
        let a = [0.5, 4.0, -1.0];
        let b = [0.5 + 1e-10, 4.0, -1.0];
        assert!(separated(&[prep(vec![a, b], 0)]));
        assert!(!separated(&[prep(vec![a, b], 0), prep(vec![a, b], 1)]));
    }

    #[test]
    fn equalities_only_at_a_rounded_point_are_not_separation() {
        // Each pair of options is chosen once each way, so the feasible set
        // is the single direction orthogonal to both differences (inside
        // the simplex for these values), where every constraint is 0 up to
        // rounding. The tolerance keeps rounding from counting as strict.
        let sets = [
            [(0.13, 129.0, 0.75), (0.79, 30.0, 0.84), (0.34, 88.0, 0.81)],
            [(0.19, 111.0, 0.61), (0.12, 168.0, 0.60), (0.89, 85.0, 0.94)],
            [
                (0.47, 121.0, 0.75),
                (0.10, 168.0, 0.82),
                (0.65, 147.0, 0.81),
            ],
            [(0.37, 112.0, 0.77), (0.20, 109.0, 0.74), (0.48, 78.0, 0.69)],
            [(0.94, 118.0, 0.92), (0.85, 166.0, 0.77), (0.99, 90.0, 0.62)],
        ];
        for set in sets {
            let x: Vec<[f64; 3]> = set
                .iter()
                .map(|&(c, t, q)| attributes(&Point { c, t, q }).unwrap())
                .collect();
            let events = [
                prep(vec![x[0], x[1]], 0),
                prep(vec![x[0], x[1]], 1),
                prep(vec![x[0], x[2]], 0),
                prep(vec![x[0], x[2]], 1),
            ];
            assert!(!separated(&events));
            // One more event strict at that direction makes it separated.
            let mut more = events.to_vec();
            let n = cross(sub(x[1], x[0]), sub(x[2], x[0]));
            let s = if n[0] > 0.0 { 1.0 } else { -1.0 };
            let better = [x[0][0] - s * n[0], x[0][1] - s * n[1], x[0][2] - s * n[2]];
            more.push(prep(vec![better, x[0]], 0));
            assert!(separated(&more));
        }
    }

    fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
        [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
    }

    fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    }
}

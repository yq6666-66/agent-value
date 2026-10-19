//! Pass rate, retries and prior (v22 §1; HANDOFF step 5).
//!
//! - Prior: `p_B` and the equivalent sample size `kappa`, exactly as written
//!   (decision C). `rho = 1` is the explicit [`Prior::DirectPool`] variant,
//!   never a float comparison; `kappa_p n_B / (kappa_p + n_B)` is 0 when
//!   both are 0. The jump at `rho = 1` is kept (no smoothing).
//! - Beta posterior `alpha = kappa p_B + n1 + 0.5`, `beta = kappa (1 - p_B)
//!   + n0 + 0.5`, `p1 = alpha / (alpha + beta)`.
//! - K-truncated geometric retries (decision A): `q_K = (1-p1)(1-pi)^(K-1)`
//!   first, `p_K = 1 - q_K`, `E[n_F] = (1-p1) sum_{j<K} (1-pi)^j`,
//!   `E[n_W] = E[n_F] - q_K`. No `pi = 0` branch and no cancellation.
//! - Lower bound (decision B): `p_K^lo < 0.75` is decided as
//!   `BetaCDF(x*; alpha, beta) > 0.05` with `x* = 1 - 0.25 / (1-pi)^(K-1)`
//!   (`x* <= 0` passes). The 5% quantile itself is only for display.
//! - Cancel-robust scenario: `n0 -> n0 + nC`, same pipeline.

use crate::beta_dist::{beta_cdf, beta_quantile};
use crate::constants::{kappa_p, K_MAX_ATTEMPTS};
use crate::error::{finite, CoreError, Result};

/// Lower-bound probability of the fail test (5%).
pub const LOWER_BOUND_PROB: f64 = 0.05;
/// `p_K^lo` (and `p_K^{C,lo}`) below this is a failing cell (§0).
pub const PASS_LOWER_BOUND_MIN: f64 = 0.75;
/// Cap of `min(kappa_p, 3)` when the prior is the layer mean `p_0`.
pub const LAYER_MEAN_KAPPA_CAP: f64 = 3.0;

fn unit(what: &'static str, v: f64) -> Result<f64> {
    let v = finite(what, v)?;
    if !(0.0..=1.0).contains(&v) {
        return Err(CoreError::OutOfRange {
            what,
            value: v,
            min: 0.0,
            max: 1.0,
        });
    }
    Ok(v)
}

fn non_negative(what: &'static str, v: f64) -> Result<f64> {
    let v = finite(what, v)?;
    if v < 0.0 {
        return Err(CoreError::OutOfRange {
            what,
            value: v,
            min: 0.0,
            max: f64::INFINITY,
        });
    }
    Ok(v)
}

/// The prior of one (arm, cell): which base rate `p_B` and which `kappa`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Prior {
    /// Direct benchmark with `rho = 1` (direct pooling): `kappa = n_B`.
    DirectPool { p_bench: f64, n_b: f64 },
    /// Direct benchmark with similarity `rho` in `[0, 1)`:
    /// `kappa = kappa_p n_B / (kappa_p + n_B)`, `kappa_p = 20 rho`.
    Discounted { p_bench: f64, n_b: f64, rho: f64 },
    /// No direct benchmark, layer mean `p_0`: `kappa = min(kappa_p, 3)`.
    LayerMean { p0: f64, rho: f64 },
    /// Neither: Jeffreys, `p_B = 0.5`, `kappa = 0`.
    Jeffreys,
}

impl Prior {
    /// `p_B`.
    pub fn p_b(&self) -> Result<f64> {
        match *self {
            Self::DirectPool { p_bench, .. } | Self::Discounted { p_bench, .. } => {
                unit("benchmark pass rate p_B", p_bench)
            }
            Self::LayerMean { p0, .. } => unit("layer mean pass rate p_0", p0),
            Self::Jeffreys => Ok(0.5),
        }
    }

    /// `kappa` (§1).
    pub fn kappa(&self) -> Result<f64> {
        match *self {
            Self::DirectPool { n_b, .. } => non_negative("benchmark sample size n_B", n_b),
            Self::Discounted { n_b, rho, .. } => {
                let n_b = non_negative("benchmark sample size n_B", n_b)?;
                let rho = finite("rho", rho)?;
                if !(0.0..1.0).contains(&rho) {
                    // rho = 1 is DirectPool, never this branch.
                    return Err(CoreError::OutOfRange {
                        what: "rho (discounted prior, use DirectPool for rho = 1)",
                        value: rho,
                        min: 0.0,
                        max: 1.0,
                    });
                }
                let kp = kappa_p(rho)?;
                let den = kp + n_b;
                Ok(if den == 0.0 { 0.0 } else { kp * n_b / den })
            }
            Self::LayerMean { rho, .. } => Ok(kappa_p(rho)?.min(LAYER_MEAN_KAPPA_CAP)),
            Self::Jeffreys => Ok(0.0),
        }
    }
}

/// Beta posterior of the first-attempt pass rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BetaPosterior {
    pub alpha: f64,
    pub beta: f64,
}

impl BetaPosterior {
    /// `alpha = kappa p_B + n1 + 0.5`, `beta = kappa (1 - p_B) + n0 + 0.5`.
    pub fn new(prior: &Prior, n1: u64, n0: u64) -> Result<Self> {
        let p_b = prior.p_b()?;
        let kappa = prior.kappa()?;
        Ok(Self {
            alpha: kappa * p_b + n1 as f64 + 0.5,
            beta: kappa * (1.0 - p_b) + n0 as f64 + 0.5,
        })
    }

    /// `p1 = alpha / (alpha + beta)`.
    pub fn p1(&self) -> f64 {
        self.alpha / (self.alpha + self.beta)
    }

    /// `p1^{5%} = Beta^{-1}(0.05; alpha, beta)` (display only).
    pub fn p1_lower(&self) -> Result<f64> {
        beta_quantile(LOWER_BOUND_PROB, self.alpha, self.beta)
    }
}

/// Retry model: conditional retry success rate `pi` and attempt cap `K`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetryModel {
    pi: f64,
    k: u32,
}

impl RetryModel {
    pub fn new(pi: f64, k: u32) -> Result<Self> {
        let pi = unit("retry success rate pi", pi)?;
        if k == 0 {
            return Err(CoreError::OutOfRange {
                what: "attempt cap K",
                value: 0.0,
                min: 1.0,
                max: f64::INFINITY,
            });
        }
        Ok(Self { pi, k })
    }

    /// `K = 3` (§9) with the given `pi`.
    pub fn with_default_k(pi: f64) -> Result<Self> {
        Self::new(pi, K_MAX_ATTEMPTS)
    }

    pub fn pi(&self) -> f64 {
        self.pi
    }

    pub fn k(&self) -> u32 {
        self.k
    }

    /// `(1 - pi)^(K-1)`, defined as 1 when `K = 1` (also for `pi = 1`).
    fn decay(&self) -> f64 {
        let r = 1.0 - self.pi;
        let mut v = 1.0;
        for _ in 1..self.k {
            v *= r;
        }
        v
    }

    /// `sum_{j=0}^{K-1} (1 - pi)^j` by direct accumulation.
    fn geometric_sum(&self) -> f64 {
        let r = 1.0 - self.pi;
        let (mut s, mut t) = (0.0, 1.0);
        for _ in 0..self.k {
            s += t;
            t *= r;
        }
        s
    }
}

/// `p_K`, `q_K`, `E[n_F]`, `E[n_W]` of the self-built segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetryStats {
    pub p1: f64,
    pub p_k: f64,
    pub q_k: f64,
    pub e_n_f: f64,
    pub e_n_w: f64,
}

/// Retry quantities for first-attempt pass rate `p1` (decision A).
pub fn retry_stats(p1: f64, model: &RetryModel) -> Result<RetryStats> {
    let p1 = unit("first-attempt pass rate p1", p1)?;
    let q_k = (1.0 - p1) * model.decay();
    let e_n_f = (1.0 - p1) * model.geometric_sum();
    Ok(RetryStats {
        p1,
        p_k: 1.0 - q_k,
        q_k,
        e_n_f,
        e_n_w: e_n_f - q_k,
    })
}

/// `p_K^lo = 1 - (1 - p1^{5%}) (1 - pi)^(K-1)` (display only).
pub fn p_k_lower(post: &BetaPosterior, model: &RetryModel) -> Result<f64> {
    Ok(1.0 - (1.0 - post.p1_lower()?) * model.decay())
}

/// `p_K^lo < 0.75`, decided via the CDF (decision B): with
/// `x* = 1 - 0.25 / (1-pi)^(K-1)`, the bound fails iff
/// `BetaCDF(x*) > 0.05`; `x* <= 0` (or a zero decay) always passes.
/// Exactly 0.75 passes (strict inequality, as in §0).
pub fn lower_bound_fails(post: &BetaPosterior, model: &RetryModel) -> Result<bool> {
    let decay = model.decay();
    if decay <= 0.0 {
        return Ok(false);
    }
    let x_star = 1.0 - (1.0 - PASS_LOWER_BOUND_MIN) / decay;
    if x_star <= 0.0 {
        return Ok(false);
    }
    Ok(beta_cdf(x_star, post.alpha, post.beta)? > LOWER_BOUND_PROB)
}

/// Pass statistics of one (arm, cell), plain and cancel-robust.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PassStats {
    pub posterior: BetaPosterior,
    pub retry: RetryStats,
    /// `p_K^lo < 0.75`.
    pub lower_fails: bool,
    /// Cancel-robust posterior (`n0 + nC` failures).
    pub posterior_c: BetaPosterior,
    pub retry_c: RetryStats,
    /// `p_K^{C,lo} < 0.75`.
    pub lower_fails_c: bool,
}

impl PassStats {
    /// Computes both scenarios for counts `n1`, `n0`, `nC`.
    pub fn new(prior: &Prior, n1: u64, n0: u64, n_c: u64, model: &RetryModel) -> Result<Self> {
        let n0_c = n0
            .checked_add(n_c)
            .ok_or(CoreError::Overflow { what: "n0 + nC" })?;
        let posterior = BetaPosterior::new(prior, n1, n0)?;
        let posterior_c = BetaPosterior::new(prior, n1, n0_c)?;
        Ok(Self {
            posterior,
            retry: retry_stats(posterior.p1(), model)?,
            lower_fails: lower_bound_fails(&posterior, model)?,
            posterior_c,
            retry_c: retry_stats(posterior_c.p1(), model)?,
            lower_fails_c: lower_bound_fails(&posterior_c, model)?,
        })
    }

    /// Either lower bound below 0.75.
    pub fn any_lower_fails(&self) -> bool {
        self.lower_fails || self.lower_fails_c
    }
}

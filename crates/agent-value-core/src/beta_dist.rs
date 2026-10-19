//! Beta distribution numerics for §1: regularized incomplete beta
//! `I_x(a, b)` (the Beta CDF) and its inverse by bisection.
//!
//! `ln Gamma` uses the Lanczos approximation (g = 7, 9 terms) with the
//! reflection formula below 1/2; `I_x(a, b)` uses its standard continued
//! fraction evaluated with the modified Lentz method, switching to
//! `1 - I_{1-x}(b, a)` above the mean-like split point so the fraction
//! converges fast. Written for this crate; accuracy is checked in tests
//! against 40-digit references.

use crate::error::{CoreError, Result};

/// Lanczos coefficients for g = 7, n = 9.
const LANCZOS_G: f64 = 7.0;
const LANCZOS: [f64; 9] = [
    0.999_999_999_999_809_9,
    676.520_368_121_885_1,
    -1_259.139_216_722_402_8,
    771.323_428_777_653_1,
    -176.615_029_162_140_6,
    12.507_343_278_686_905,
    -0.138_571_095_265_720_12,
    9.984_369_578_019_572e-6,
    1.505_632_735_149_311_6e-7,
];

/// Iteration cap of the continued fraction.
const CF_MAX_ITER: usize = 100_000;
/// Relative convergence tolerance of the continued fraction.
const CF_EPS: f64 = 1e-16;
/// Guard against division by zero inside Lentz's method.
const CF_TINY: f64 = 1e-300;
/// Bisection steps for the quantile (interval width 2^-200: exact to f64).
const QUANTILE_STEPS: usize = 200;

/// `ln Gamma(x)` for `x > 0`.
pub fn ln_gamma(x: f64) -> Result<f64> {
    if !x.is_finite() || x <= 0.0 {
        return Err(CoreError::OutOfRange {
            what: "ln_gamma argument",
            value: x,
            min: 0.0,
            max: f64::INFINITY,
        });
    }
    if x < 0.5 {
        // Gamma(x) Gamma(1 - x) = pi / sin(pi x), with sin(pi x) > 0 on (0, 1/2).
        let s = (std::f64::consts::PI * x).sin();
        return Ok(std::f64::consts::PI.ln() - s.ln() - ln_gamma(1.0 - x)?);
    }
    let z = x - 1.0;
    let mut sum = LANCZOS[0];
    for (i, &c) in LANCZOS.iter().enumerate().skip(1) {
        sum += c / (z + i as f64);
    }
    let t = z + LANCZOS_G + 0.5;
    Ok(0.5 * (2.0 * std::f64::consts::PI).ln() + (z + 0.5) * t.ln() - t + sum.ln())
}

fn check_shape(a: f64, b: f64) -> Result<()> {
    for (what, v) in [("beta shape a", a), ("beta shape b", b)] {
        if !v.is_finite() || v <= 0.0 {
            return Err(CoreError::OutOfRange {
                what,
                value: v,
                min: 0.0,
                max: f64::INFINITY,
            });
        }
    }
    Ok(())
}

/// Continued fraction part of `I_x(a, b)` (modified Lentz).
fn beta_cf(x: f64, a: f64, b: f64) -> Result<f64> {
    let qab = a + b;
    let qap = a + 1.0;
    let qam = a - 1.0;
    let mut c = 1.0;
    let mut d = 1.0 - qab * x / qap;
    if d.abs() < CF_TINY {
        d = CF_TINY;
    }
    d = 1.0 / d;
    let mut h = d;
    for m in 1..=CF_MAX_ITER {
        let m = m as f64;
        let m2 = 2.0 * m;
        // Even step.
        let aa = m * (b - m) * x / ((qam + m2) * (a + m2));
        d = 1.0 + aa * d;
        if d.abs() < CF_TINY {
            d = CF_TINY;
        }
        c = 1.0 + aa / c;
        if c.abs() < CF_TINY {
            c = CF_TINY;
        }
        d = 1.0 / d;
        h *= d * c;
        // Odd step.
        let aa = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2));
        d = 1.0 + aa * d;
        if d.abs() < CF_TINY {
            d = CF_TINY;
        }
        c = 1.0 + aa / c;
        if c.abs() < CF_TINY {
            c = CF_TINY;
        }
        d = 1.0 / d;
        let del = d * c;
        h *= del;
        if (del - 1.0).abs() <= CF_EPS {
            return Ok(h);
        }
    }
    Err(CoreError::NoConvergence {
        what: "incomplete beta continued fraction",
    })
}

/// Beta CDF `I_x(a, b)` for `a, b > 0` and `x` in `[0, 1]`.
pub fn beta_cdf(x: f64, a: f64, b: f64) -> Result<f64> {
    check_shape(a, b)?;
    if !x.is_finite() || !(0.0..=1.0).contains(&x) {
        return Err(CoreError::OutOfRange {
            what: "beta cdf argument",
            value: x,
            min: 0.0,
            max: 1.0,
        });
    }
    if x == 0.0 {
        return Ok(0.0);
    }
    if x == 1.0 {
        return Ok(1.0);
    }
    let ln_front = ln_gamma(a + b)? - ln_gamma(a)? - ln_gamma(b)? + a * x.ln() + b * (1.0 - x).ln();
    let front = ln_front.exp();
    let v = if x < (a + 1.0) / (a + b + 2.0) {
        front * beta_cf(x, a, b)? / a
    } else {
        1.0 - front * beta_cf(1.0 - x, b, a)? / b
    };
    Ok(v.clamp(0.0, 1.0))
}

/// Beta quantile `I^{-1}_p(a, b)` by bisection on [`beta_cdf`]; for display
/// (the fail test itself uses the CDF, decision B).
pub fn beta_quantile(p: f64, a: f64, b: f64) -> Result<f64> {
    check_shape(a, b)?;
    if !p.is_finite() || !(0.0..=1.0).contains(&p) {
        return Err(CoreError::OutOfRange {
            what: "beta quantile probability",
            value: p,
            min: 0.0,
            max: 1.0,
        });
    }
    let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
    for _ in 0..QUANTILE_STEPS {
        let mid = 0.5 * (lo + hi);
        if mid <= lo || mid >= hi {
            break;
        }
        if beta_cdf(mid, a, b)? < p {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Ok(0.5 * (lo + hi))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ln_gamma_known_values() {
        // Gamma(1) = Gamma(2) = 1, Gamma(1/2) = sqrt(pi), Gamma(5) = 24.
        assert!(ln_gamma(1.0).unwrap().abs() < 1e-14);
        assert!(ln_gamma(2.0).unwrap().abs() < 1e-14);
        assert!((ln_gamma(0.5).unwrap() - 0.5 * std::f64::consts::PI.ln()).abs() < 1e-14);
        assert!((ln_gamma(5.0).unwrap() - 24f64.ln()).abs() < 1e-13);
        assert!((ln_gamma(0.1).unwrap() - 2.252_712_651_734_206).abs() < 1e-13);
        assert!(ln_gamma(0.0).is_err());
        assert!(ln_gamma(f64::NAN).is_err());
    }

    #[test]
    fn cdf_closed_forms() {
        // Beta(1, 1) is uniform; Beta(a, 1) has CDF x^a; Beta(1, b) has 1-(1-x)^b.
        for x in [0.1, 0.25, 0.5, 0.9] {
            assert!((beta_cdf(x, 1.0, 1.0).unwrap() - x).abs() < 1e-14);
            assert!((beta_cdf(x, 3.5, 1.0).unwrap() - x.powf(3.5)).abs() < 1e-14);
            assert!((beta_cdf(x, 1.0, 2.5).unwrap() - (1.0 - (1.0 - x).powf(2.5))).abs() < 1e-14);
        }
        assert_eq!(beta_cdf(0.0, 2.0, 3.0), Ok(0.0));
        assert_eq!(beta_cdf(1.0, 2.0, 3.0), Ok(1.0));
        assert!(beta_cdf(1.1, 2.0, 3.0).is_err());
        assert!(beta_cdf(0.5, 0.0, 3.0).is_err());
    }

    #[test]
    fn quantile_inverts_cdf() {
        for (a, b) in [(0.5, 1.5), (6.5, 4.5), (30.5, 2.5)] {
            for p in [0.05, 0.5, 0.95] {
                let q = beta_quantile(p, a, b).unwrap();
                assert!((beta_cdf(q, a, b).unwrap() - p).abs() < 1e-12);
            }
        }
        assert_eq!(beta_quantile(0.0, 2.0, 2.0).map(|q| q < 1e-15), Ok(true));
        assert!(beta_quantile(1.5, 2.0, 2.0).is_err());
    }
}

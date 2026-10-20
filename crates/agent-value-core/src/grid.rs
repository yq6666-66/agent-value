//! The §9 / §10.2 weight grid `G`: `a, b, g` in `{0.05, 0.10, ..., 0.90}`
//! with `a + b + g = 1`, 171 points.
//!
//! Points are stored as integer twentieths and the float weights are
//! computed as `i / 20`, never by adding 0.05 repeatedly (DECISIONS G1).

use crate::constants::GRID_POINTS;
use crate::weights::Weights;
use crate::Result;

/// One grid point `(i/20, j/20, k/20)` with `i, j, k >= 1` and
/// `i + j + k = 20`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GridPoint {
    i: u8,
    j: u8,
}

impl GridPoint {
    /// `None` unless `i, j >= 1` and `i + j <= 19`.
    pub fn new(i: u8, j: u8) -> Option<Self> {
        if i >= 1 && j >= 1 && u16::from(i) + u16::from(j) <= 19 {
            Some(Self { i, j })
        } else {
            None
        }
    }

    /// Twentieths `(i, j, k)`.
    pub fn twentieths(self) -> (u8, u8, u8) {
        (self.i, self.j, 20 - self.i - self.j)
    }

    pub fn a(self) -> f64 {
        f64::from(self.i) / 20.0
    }

    pub fn b(self) -> f64 {
        f64::from(self.j) / 20.0
    }

    pub fn g(self) -> f64 {
        f64::from(20 - self.i - self.j) / 20.0
    }

    /// `[a, b, g]`.
    pub fn as_array(self) -> [f64; 3] {
        [self.a(), self.b(), self.g()]
    }

    /// The point as a weight triple.
    pub fn weights(self) -> Result<Weights> {
        Weights::new(self.a(), self.b(), self.g())
    }
}

/// The 171 grid points, ordered by `i` then `j` (the order of the reference
/// script's `GRID`, which is also lexicographic in `(a, b)`).
pub fn grid() -> Vec<GridPoint> {
    let mut out = Vec::with_capacity(GRID_POINTS);
    for i in 1u8..=18 {
        for j in 1u8..=(19 - i) {
            out.push(GridPoint { i, j });
        }
    }
    out
}

/// Grid point nearest to the centroid of `points` (DECISIONS G3).
///
/// Distances are compared exactly in integer twentieths (squared distance
/// times `n^2`); ties go to the smallest `(a, b)`. `None` for an empty set.
pub fn nearest_to_centroid(points: &[GridPoint]) -> Option<GridPoint> {
    if points.is_empty() {
        return None;
    }
    let n = points.len() as i64;
    let (mut si, mut sj, mut sk) = (0i64, 0i64, 0i64);
    for p in points {
        let (i, j, k) = p.twentieths();
        si += i64::from(i);
        sj += i64::from(j);
        sk += i64::from(k);
    }
    let mut best: Option<(i64, GridPoint)> = None;
    for p in grid() {
        let (i, j, k) = p.twentieths();
        let d = |x: u8, s: i64| {
            let v = n * i64::from(x) - s;
            v * v
        };
        let dist = d(i, si) + d(j, sj) + d(k, sk);
        if best.is_none_or(|(b, _)| dist < b) {
            best = Some((dist, p));
        }
    }
    best.map(|(_, p)| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_has_171_points_in_lexicographic_order() {
        let g = grid();
        assert_eq!(g.len(), GRID_POINTS);
        assert!(g.windows(2).all(|w| w[0] < w[1]));
    }
}

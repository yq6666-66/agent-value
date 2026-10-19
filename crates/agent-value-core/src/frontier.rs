//! Layer frontiers `F_off`, `F_rob`, `F_rec` (v22 §6; HANDOFF step 9).
//!
//! Dominance is on triples among `E_d` only: lower cost and time are better,
//! higher quality is better, and at least one dimension must be strict.
//! Preference weights `a,b,g` do not enter the frontier.

use crate::cell_values::Triple;
use crate::error::{CoreError, Result};
use crate::value::Point;

/// One eligible arm's layer triples for frontier membership.
#[derive(Debug, Clone, PartialEq)]
pub struct FrontierArm {
    /// Stable sort / identity key (typically the model id string).
    pub id: String,
    pub star: Triple,
    pub robust: Triple,
    /// When false, the arm is excluded from the robust frontier and from
    /// robust-dominance sets (e.g. cancel bills too coarse).
    pub robust_enabled: bool,
}

/// The three frontier sets for one layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerFrontiers {
    /// Official frontier on plain triples.
    pub off: Vec<String>,
    /// Cancel-robust frontier.
    pub rob: Vec<String>,
    /// Recommended = off members with empty robust-dominator set.
    pub rec: Vec<String>,
}

fn point(t: &Triple) -> Option<Point> {
    Point::from_triple(t)
}

/// `a ≻ b` on a complete triple: a is better or equal on every dimension and
/// strictly better on at least one (cost/time lower, quality higher).
pub fn dominates(a: Point, b: Point) -> bool {
    let c_le = a.c <= b.c;
    let t_le = a.t <= b.t;
    let q_ge = a.q >= b.q;
    let strict = a.c < b.c || a.t < b.t || a.q > b.q;
    c_le && t_le && q_ge && strict
}

fn dominates_triple(a: &Triple, b: &Triple) -> bool {
    match (point(a), point(b)) {
        (Some(pa), Some(pb)) => dominates(pa, pb),
        _ => false,
    }
}

/// Builds `F_off`, `F_rob`, `F_rec` from eligible arms only.
///
/// Arms whose plain triple is incomplete are skipped for `F_off` (and thus
/// cannot enter `F_rec`). Arms with `robust_enabled = false` or an incomplete
/// robust triple are skipped for `F_rob` and never appear in anyone's
/// robust-dominator set (DECISIONS Q).
pub fn layer_frontiers(arms: &[FrontierArm]) -> Result<LayerFrontiers> {
    let mut ids = std::collections::BTreeSet::new();
    for a in arms {
        if a.id.trim().is_empty() {
            return Err(CoreError::EmptyField {
                field: "frontier arm id",
            });
        }
        if !ids.insert(a.id.clone()) {
            return Err(CoreError::DuplicateCell { cell: a.id.clone() });
        }
    }

    let mut off = Vec::new();
    for a in arms {
        if point(&a.star).is_none() {
            continue;
        }
        let dominated = arms
            .iter()
            .any(|j| j.id != a.id && dominates_triple(&j.star, &a.star));
        if !dominated {
            off.push(a.id.clone());
        }
    }
    off.sort();

    let mut rob = Vec::new();
    for a in arms {
        if !a.robust_enabled || point(&a.robust).is_none() {
            continue;
        }
        let dominated = arms.iter().any(|j| {
            j.id != a.id
                && j.robust_enabled
                && point(&j.robust).is_some()
                && dominates_triple(&j.robust, &a.robust)
        });
        if !dominated {
            rob.push(a.id.clone());
        }
    }
    rob.sort();

    // R_d(i) = { j in E_d : j ≻_rob i }; F_rec = { i in F_off : R_d(i) = empty }
    let mut rec = Vec::new();
    for id in &off {
        let a = arms.iter().find(|x| x.id == *id).unwrap();
        let r_empty = !arms.iter().any(|j| {
            j.id != a.id
                && j.robust_enabled
                && point(&j.robust).is_some()
                && a.robust_enabled
                && point(&a.robust).is_some()
                && dominates_triple(&j.robust, &a.robust)
        });
        // If a has no robust triple / disabled, R_d is treated as empty only
        // when no j can dominate it — which they cannot without a's point.
        // Spec: R_d uses ≻_rob; if a has no robust point, nothing dominates it
        // under ≻_rob, so it stays in F_rec when in F_off.
        if r_empty {
            rec.push(id.clone());
        }
    }
    rec.sort();

    Ok(LayerFrontiers { off, rob, rec })
}

/// Arms tied for the highest finite `V` among `scores` (id, V).
pub fn highest_v_group(scores: &[(String, f64)]) -> Result<Vec<String>> {
    let mut best: Option<f64> = None;
    for (_, v) in scores {
        if !v.is_finite() {
            return Err(CoreError::NotFinite {
                what: "V",
                value: *v,
            });
        }
        best = Some(best.map_or(*v, |b| b.max(*v)));
    }
    let Some(b) = best else {
        return Ok(Vec::new());
    };
    let mut ids: Vec<String> = scores
        .iter()
        .filter(|(_, v)| (*v - b).abs() <= 1e-12)
        .map(|(id, _)| id.clone())
        .collect();
    ids.sort();
    Ok(ids)
}

#[cfg(test)]
mod frontier_unit_tests {
    use super::*;

    fn arm(id: &str, c: f64, t: f64, q: f64) -> FrontierArm {
        let tpl = Triple::new(c, t, q);
        FrontierArm {
            id: id.into(),
            star: tpl,
            robust: tpl,
            robust_enabled: true,
        }
    }

    #[test]
    fn dominates_requires_strict_improvement() {
        assert!(dominates(
            Point {
                c: 1.0,
                t: 10.0,
                q: 0.9
            },
            Point {
                c: 2.0,
                t: 10.0,
                q: 0.9
            }
        ));
        assert!(!dominates(
            Point {
                c: 1.0,
                t: 10.0,
                q: 0.9
            },
            Point {
                c: 1.0,
                t: 10.0,
                q: 0.9
            }
        ));
        assert!(!dominates(
            Point {
                c: 1.0,
                t: 12.0,
                q: 0.9
            },
            Point {
                c: 2.0,
                t: 10.0,
                q: 0.8
            }
        ));
        // Cost tie: equal cost, faster, equal quality still dominates.
        assert!(dominates(
            Point {
                c: 1.0,
                t: 5.0,
                q: 0.9
            },
            Point {
                c: 1.0,
                t: 10.0,
                q: 0.9
            }
        ));
    }

    #[test]
    fn off_front_keeps_pareto_members() {
        let arms = vec![
            arm("A", 1.0, 10.0, 0.9),
            arm("B", 2.0, 5.0, 0.8),
            arm("C", 3.0, 20.0, 0.7), // dominated by A on all
        ];
        let f = layer_frontiers(&arms).unwrap();
        assert_eq!(f.off, vec!["A".to_string(), "B".to_string()]);
        assert!(f.off.iter().any(|x| x == "A"));
        assert!(!f.off.iter().any(|x| x == "C"));
    }

    #[test]
    fn highest_v_reports_whole_tie_group() {
        let g = highest_v_group(&[("X".into(), 100.0), ("Y".into(), 100.0), ("Z".into(), 90.0)])
            .unwrap();
        assert_eq!(g, vec!["X".to_string(), "Y".to_string()]);
    }
}

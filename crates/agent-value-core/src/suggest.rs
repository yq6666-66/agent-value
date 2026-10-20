//! §10.4 merge rule: behaviour never changes the weights by itself; it only
//! produces a suggestion the user may confirm.

use crate::behavior::Behavior;
use crate::elicit::SelfReport;
use crate::grid::GridPoint;

/// Relative tie tolerance for the argmax over `Omega_s` (DECISIONS G9).
pub const PROFILE_TIE_TOLERANCE: f64 = 1e-9;

/// What the user is offered (§10.4 table).
#[derive(Debug, Clone, PartialEq)]
pub enum Suggestion {
    /// Self-report present, behaviour has no signal.
    None,
    /// `Omega_s ∩ Omega_b` not empty: "behaviour agrees with the self-report";
    /// optional switch to the best-supported point of `Omega_s`.
    Consistent { optional: GridPoint },
    /// Intersection empty: show the conflict; optional switch to `w_b`.
    Conflict { optional: [f64; 3] },
    /// No self-report, behaviour has a signal: optional switch to `w_b`.
    AdoptBehavior { optional: [f64; 3] },
    /// Neither: suggest doing the self-report questions.
    DoSelfReport,
}

/// Weights in effect plus the suggestion.
#[derive(Debug, Clone, PartialEq)]
pub struct Merge {
    /// `w_s` or `(1/3, 1/3, 1/3)`; never taken from behaviour.
    pub effective: [f64; 3],
    pub suggestion: Suggestion,
}

const EQUAL: [f64; 3] = [1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0];

/// `argmax_{w in Omega_s} L^prof(w)`; ties within the relative tolerance go
/// to the smallest `(a, b)` (DECISIONS G9).
fn best_in(region: &[GridPoint], behavior: &Behavior) -> Option<GridPoint> {
    let vals: Vec<(GridPoint, f64)> = behavior
        .profile
        .iter()
        .filter(|(w, _)| region.contains(w))
        .copied()
        .collect();
    let max = vals
        .iter()
        .map(|(_, l)| *l)
        .fold(f64::NEG_INFINITY, f64::max);
    let tol = PROFILE_TIE_TOLERANCE * max.abs().max(1.0);
    vals.iter()
        .filter(|(_, l)| *l >= max - tol)
        .map(|(w, _)| *w)
        .min()
}

/// Applies the §10.4 table.
pub fn merge(self_report: Option<&SelfReport>, behavior: Option<&Behavior>) -> Merge {
    let signal = behavior.filter(|b| b.has_signal());
    match (self_report, signal) {
        (Some(s), None) => Merge {
            effective: s.point.as_array(),
            suggestion: Suggestion::None,
        },
        (Some(s), Some(b)) => {
            let overlap = s.regions.wide.iter().any(|w| b.support.contains(w));
            let suggestion = if overlap {
                match best_in(&s.regions.wide, b) {
                    Some(optional) => Suggestion::Consistent { optional },
                    None => Suggestion::None,
                }
            } else {
                match b.w_b {
                    Some(optional) => Suggestion::Conflict { optional },
                    None => Suggestion::None,
                }
            };
            Merge {
                effective: s.point.as_array(),
                suggestion,
            }
        }
        (None, Some(b)) => Merge {
            effective: EQUAL,
            suggestion: match b.w_b {
                Some(optional) => Suggestion::AdoptBehavior { optional },
                None => Suggestion::DoSelfReport,
            },
        },
        (None, None) => Merge {
            effective: EQUAL,
            suggestion: Suggestion::DoSelfReport,
        },
    }
}

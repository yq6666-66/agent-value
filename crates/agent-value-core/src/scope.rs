//! Scoring scopes `S_k` (v22 §A).
//!
//! Each task type is an independent scoring scope with its own grid, cell
//! weights, reference, fallback and weight version. By default there are only
//! two: coding and non-coding. No V is ever combined across scopes.

use std::fmt;

use crate::constants::layer_sample_gate;
use crate::error::{CoreError, Result};

/// The two default task types (§A).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TaskKind {
    Coding,
    NonCoding,
}

impl TaskKind {
    /// Stable machine name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Coding => "coding",
            Self::NonCoding => "non-coding",
        }
    }
}

/// Identifies one scoring scope `S_k`: a task kind, optionally subdivided.
///
/// A subdivision (for example splitting non-coding into "q-and-a" and
/// "writing") is only allowed once [`subdivision_allowed`] holds for the
/// previous window.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ScopeId {
    kind: TaskKind,
    subtype: Option<String>,
}

impl ScopeId {
    /// The undivided scope for `kind`.
    pub fn new(kind: TaskKind) -> Self {
        Self {
            kind,
            subtype: None,
        }
    }

    /// A subdivided scope. The subtype is trimmed and lower-cased and must
    /// not be empty.
    pub fn subdivided(kind: TaskKind, subtype: &str) -> Result<Self> {
        let s = subtype.trim().to_lowercase();
        if s.is_empty() {
            return Err(CoreError::EmptyField {
                field: "scope subtype",
            });
        }
        Ok(Self {
            kind,
            subtype: Some(s),
        })
    }

    /// The default scopes: coding and non-coding.
    pub fn defaults() -> [ScopeId; 2] {
        [Self::new(TaskKind::Coding), Self::new(TaskKind::NonCoding)]
    }

    pub fn kind(&self) -> TaskKind {
        self.kind
    }

    pub fn subtype(&self) -> Option<&str> {
        self.subtype.as_deref()
    }
}

impl fmt::Display for ScopeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.subtype {
            None => f.write_str(self.kind.as_str()),
            Some(s) => write!(f, "{}/{}", self.kind.as_str(), s),
        }
    }
}

/// Default design cells `u` (task sub-classes) for a scope's single layer.
///
/// Coding follows §A: fix, add, refactor, explain-and-review, other.
/// The spec lists no sub-classes for non-coding, so it defaults to a single
/// `general` cell until the user configures more.
pub fn default_cells(kind: TaskKind) -> &'static [&'static str] {
    match kind {
        TaskKind::Coding => &["fix", "add", "refactor", "explain-review", "other"],
        TaskKind::NonCoding => &["general"],
    }
}

/// §A subdivision gate: a scope may be split only if, in the previous window,
/// at least 2 arms each completed at least `max(30, 3 U_d)` tasks, where
/// `U_d` is the number of positive-weight cells of the proposed sub-scope.
///
/// `completed_per_arm` holds one completed-task count per arm.
pub fn subdivision_allowed(completed_per_arm: &[u64], u_d: usize) -> bool {
    let gate = layer_sample_gate(u_d) as u64;
    completed_per_arm.iter().filter(|&&n| n >= gate).count() >= 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_coding_and_non_coding() {
        let d = ScopeId::defaults();
        assert_eq!(d[0].kind(), TaskKind::Coding);
        assert_eq!(d[1].kind(), TaskKind::NonCoding);
        assert_ne!(d[0], d[1]);
        assert_eq!(d[0].to_string(), "coding");
        assert_eq!(d[1].to_string(), "non-coding");
    }

    #[test]
    fn subdivided_scope_is_distinct_and_validated() {
        let w = ScopeId::subdivided(TaskKind::NonCoding, " Writing ").unwrap();
        assert_eq!(w.subtype(), Some("writing"));
        assert_ne!(w, ScopeId::new(TaskKind::NonCoding));
        assert_eq!(w.to_string(), "non-coding/writing");
        assert!(matches!(
            ScopeId::subdivided(TaskKind::NonCoding, "  "),
            Err(CoreError::EmptyField { .. })
        ));
    }

    #[test]
    fn coding_default_cells() {
        assert_eq!(default_cells(TaskKind::Coding).len(), 5);
        assert_eq!(default_cells(TaskKind::NonCoding), &["general"]);
    }

    #[test]
    fn subdivision_gate() {
        // U_d = 5 -> gate max(30, 15) = 30.
        assert!(!subdivision_allowed(&[], 5));
        assert!(!subdivision_allowed(&[100], 5));
        assert!(!subdivision_allowed(&[30, 29], 5));
        assert!(subdivision_allowed(&[30, 30], 5));
        // U_d = 12 -> gate 36.
        assert!(!subdivision_allowed(&[35, 40, 20], 12));
        assert!(subdivision_allowed(&[36, 40, 1], 12));
    }
}

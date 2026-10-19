//! Error type for the core crate. Every fallible public function returns
//! [`Result`]; bad input never panics.

use std::fmt;

/// Result alias used throughout the core crate.
pub type Result<T> = std::result::Result<T, CoreError>;

/// All errors the core crate can report.
#[derive(Debug, Clone, PartialEq)]
pub enum CoreError {
    /// A numeric input was NaN or infinite.
    NotFinite { what: &'static str, value: f64 },
    /// A numeric input was outside its allowed range.
    OutOfRange {
        what: &'static str,
        value: f64,
        min: f64,
        max: f64,
    },
    /// One of a, b, g is below the §9 floor of 0.05.
    WeightBelowFloor {
        dim: &'static str,
        value: f64,
        floor: f64,
    },
    /// a + b + g differs from 1 by more than the tolerance.
    WeightSumNotOne { sum: f64 },
    /// A required text field was empty after trimming.
    EmptyField { field: &'static str },
    /// A text field had a disallowed form.
    InvalidField {
        field: &'static str,
        reason: &'static str,
    },
    /// Weight evidence does not match the declared weight source.
    EvidenceMismatch { reason: &'static str },
    /// Two collections that must have equal length did not.
    LengthMismatch { left: usize, right: usize },
    /// A timestamp lies before the window schedule origin.
    BeforeOrigin { timestamp: i64, origin: i64 },
    /// Integer arithmetic on windows or timestamps overflowed.
    Overflow { what: &'static str },
    /// The window has already been frozen; its official inputs are fixed.
    WindowAlreadyFrozen { window: u32 },
    /// The window has not been frozen yet.
    WindowNotFrozen { window: u32 },
    /// Windows must be frozen in order without gaps.
    WindowOutOfOrder { expected: u32, got: u32 },
    /// No weight version with this id exists in the ledger.
    UnknownVersion { id: u64 },
    /// The version exists but is not a trial version.
    NotATrial { id: u64 },
    /// The trial version was already confirmed or discarded.
    TrialAlreadyResolved { id: u64 },
    /// V is not produced in the first window of a scope (§A).
    VWithheldFirstWindow { window: u32 },
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFinite { what, value } => write!(f, "{what} must be finite, got {value}"),
            Self::OutOfRange {
                what,
                value,
                min,
                max,
            } => write!(f, "{what} = {value} is outside [{min}, {max}]"),
            Self::WeightBelowFloor { dim, value, floor } => {
                write!(f, "weight {dim} = {value} is below the floor {floor}")
            }
            Self::WeightSumNotOne { sum } => write!(f, "weights must sum to 1, got {sum}"),
            Self::EmptyField { field } => write!(f, "{field} must not be empty"),
            Self::InvalidField { field, reason } => write!(f, "invalid {field}: {reason}"),
            Self::EvidenceMismatch { reason } => write!(f, "weight evidence mismatch: {reason}"),
            Self::LengthMismatch { left, right } => {
                write!(f, "length mismatch: {left} vs {right}")
            }
            Self::BeforeOrigin { timestamp, origin } => {
                write!(f, "timestamp {timestamp} is before window origin {origin}")
            }
            Self::Overflow { what } => write!(f, "arithmetic overflow computing {what}"),
            Self::WindowAlreadyFrozen { window } => write!(f, "window {window} is already frozen"),
            Self::WindowNotFrozen { window } => write!(f, "window {window} is not frozen yet"),
            Self::WindowOutOfOrder { expected, got } => {
                write!(
                    f,
                    "windows must be frozen in order: expected {expected}, got {got}"
                )
            }
            Self::UnknownVersion { id } => write!(f, "unknown weight version {id}"),
            Self::NotATrial { id } => write!(f, "weight version {id} is not a trial version"),
            Self::TrialAlreadyResolved { id } => {
                write!(f, "trial version {id} was already confirmed or discarded")
            }
            Self::VWithheldFirstWindow { window } => write!(
                f,
                "V is not produced in window {window}: first window, data collection only"
            ),
        }
    }
}

impl std::error::Error for CoreError {}

/// Returns `Ok(v)` if `v` is finite, otherwise [`CoreError::NotFinite`].
pub(crate) fn finite(what: &'static str, v: f64) -> Result<f64> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(CoreError::NotFinite { what, value: v })
    }
}

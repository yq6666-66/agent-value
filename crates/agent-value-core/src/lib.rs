//! agent-value-core: the pure-Rust scoring core of agent-value.
//!
//! All formula logic lives in this crate. It has no Tauri, GUI, network or
//! database dependencies so it can be built and tested anywhere.
//!
//! Implemented so far (v22 spec, steps 1-7): §9 constants, scoring scopes
//! `S_k`, arms `(model, route)`, time windows with freezing, weight versions
//! `W_k` with trial versions, design cells with the zero-weight probe cell,
//! privacy rules, the data-model records persisted by the
//! `agent-value-store` crate, task outcome judgement and quality signals
//! (§3.1), the Beta prior/posterior with K-truncated retries and lower
//! bounds (§1), cell states with `omega_u` (§0), per-cell `C*`, `T*`, `Q*`
//! with NA propagation and cancel-robust versions (§2, §3), and layer /
//! overall aggregation with the value score `V` (§4), and the layer
//! eligibility set `E_d` (§5; authenticity / log-integrity are stub
//! inputs until §11). The anchor tiers (§7), frontier and diagnostic
//! table are not implemented yet.

#![forbid(unsafe_code)]

pub mod arm;
pub mod beta_dist;
pub mod cell_values;
pub mod cells;
pub mod constants;
pub mod eligibility;
pub mod error;
pub mod outcome;
pub mod privacy;
pub mod quality;
pub mod records;
pub mod retry;
pub mod runday;
pub mod scope;
pub mod status;
pub mod value;
pub mod weights;
pub mod window;

pub use error::{CoreError, Result};

/// Crate version, exposed so the desktop shell can show which core it links.
pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_version_is_set() {
        assert!(!CORE_VERSION.is_empty());
    }
}

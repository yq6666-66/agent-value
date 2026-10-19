//! agent-value-core: the pure-Rust scoring core of agent-value.
//!
//! All formula logic lives in this crate. It has no Tauri, GUI, network or
//! database dependencies so it can be built and tested anywhere.
//!
//! Implemented so far (v22 spec, step 1): §9 constants, scoring scopes
//! `S_k`, arms `(model, route)`, time windows with freezing, and weight
//! versions `W_k` with trial versions. Nothing is scored yet.

#![forbid(unsafe_code)]

pub mod arm;
pub mod constants;
pub mod error;
pub mod scope;
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

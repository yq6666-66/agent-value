//! agent-value-core: the pure-Rust scoring core of agent-value.
//!
//! All formula logic lives in this crate. It has no Tauri, GUI, network or
//! database dependencies so it can be built and tested anywhere.
//!
//! Status: skeleton only. No scoring is implemented yet.

#![forbid(unsafe_code)]

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

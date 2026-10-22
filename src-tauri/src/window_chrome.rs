//! Sample window chrome for the weight panel.
//!
//! A first window and a later window are frozen through `WeightLedger`,
//! then the label is whatever `FrozenWindow::first_window_label` returns.
//! This module does not invent a second copy of that sentence.

use agent_value_core::scope::{ScopeId, TaskKind};
use agent_value_core::weights::WeightLedger;
use agent_value_core::window::{FrozenInputs, FrozenWindow, WindowIndex};
use serde::Serialize;

/// What the weight panel is allowed to show for one sample window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WindowChrome {
    pub window_index: u32,
    pub is_first: bool,
    /// Core label when this window withholds V. Absent otherwise.
    pub first_window_label: Option<String>,
}

/// `sample` is `"first"` or `"later"`. The shell picks the core window;
/// the webview only displays the returned label.
pub fn sample_window_chrome(sample: &str) -> Result<WindowChrome, String> {
    let mut ledger = WeightLedger::new(ScopeId::new(TaskKind::Coding));
    let first_chrome = WindowChrome::from_frozen(
        ledger
            .freeze_window(WindowIndex::FIRST, 0, FrozenInputs::default())
            .map_err(|err| err.to_string())?,
    );
    if sample == "first" {
        return Ok(first_chrome);
    }
    if sample != "later" {
        return Err(format!("unknown window sample {sample}"));
    }
    let next = WindowIndex::FIRST.next().map_err(|err| err.to_string())?;
    let later = ledger
        .freeze_window(next, 1, FrozenInputs::default())
        .map_err(|err| err.to_string())?;
    Ok(WindowChrome::from_frozen(later))
}

impl WindowChrome {
    fn from_frozen(frozen: &FrozenWindow) -> Self {
        Self {
            window_index: frozen.window().0,
            is_first: frozen.is_first_window(),
            first_window_label: frozen.first_window_label().map(str::to_owned),
        }
    }
}

#[cfg(test)]
mod tests {
    use agent_value_core::window::FIRST_WINDOW_LABEL;

    use super::sample_window_chrome;

    #[test]
    fn first_sample_returns_the_core_label_and_withholds_v() {
        let chrome = sample_window_chrome("first").unwrap();
        assert!(chrome.is_first);
        assert_eq!(chrome.window_index, 0);
        assert_eq!(
            chrome.first_window_label.as_deref(),
            Some(FIRST_WINDOW_LABEL)
        );
    }

    #[test]
    fn later_sample_has_no_first_window_label() {
        let chrome = sample_window_chrome("later").unwrap();
        assert!(!chrome.is_first);
        assert_eq!(chrome.window_index, 1);
        assert_eq!(chrome.first_window_label, None);
    }
}

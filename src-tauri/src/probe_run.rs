//! One probe run for a caller-selected arm.
//!
//! The webview does not invoke this. Panel commands stay read-only, and
//! `sends_http` stays false. [`run_once`] blocks for the HTTP exchanges;
//! do not call it on the Tauri async runtime. The single-flight lock and
//! the budget snapshot are per process.
//!
//! `request.key_env` is the variable name. The token is read by
//! `agent-value-probe-http` and is not written to the store.

use agent_value_probe_run::{ProbeRunOutcome, ProbeRunRequest};

use crate::store_cmd::AppState;

/// Consent not accepted, a budget stop, or a result already stored for
/// today's run day returns that launch and does not send. A second
/// overlapping call returns `running` and does not send. A send that later
/// fails still records the provisional charge.
///
/// The webview does not call this yet, so the binary may not reference it.
#[allow(dead_code)]
pub fn run_once(app: &AppState, request: &ProbeRunRequest<'_>) -> Result<ProbeRunOutcome, String> {
    agent_value_probe_run::run_once(&app.flight, app.store(), request)
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use agent_value_core::arm::{Arm, ModelId, Protocol, ProviderKind, Route};
    use agent_value_core::probe::CapabilityClaims;
    use agent_value_probe_run::{ProbeLaunch, ProbeRunRequest};

    use super::*;

    #[test]
    fn the_shell_entry_blocks_before_consent_without_a_server() {
        let app = AppState::open_in_memory().unwrap();
        let selected = Arm::new(
            ModelId::new("probe-model").unwrap(),
            Route::new(
                ProviderKind::Relay,
                "relay.example",
                Protocol::OpenAiChatCompletions,
                "probe-run",
            )
            .unwrap(),
        );
        let request = ProbeRunRequest {
            arm: &selected,
            key_env: "PROBE_RUN_SHELL_KEY",
            base_url: "http://127.0.0.1:9",
            seed: 1,
            claims: CapabilityClaims::default(),
            dotenv: Path::new("/tmp/agent-value-probe-run-missing.env"),
            timeout: Duration::from_millis(200),
        };
        let outcome = run_once(&app, &request).unwrap();
        assert_eq!(outcome.launch, ProbeLaunch::BlockedConsent);
        let rows = crate::store_cmd::with_store(&app, |store| {
            store.list_probe_results().map_err(|err| err.to_string())
        })
        .unwrap();
        assert!(rows.is_empty());
        let charges = crate::store_cmd::with_store(&app, |store| {
            store.probe_charges().map_err(|err| err.to_string())
        })
        .unwrap();
        assert!(charges.is_empty());
    }
}

//! One probe run for a caller-selected arm.
//!
//! [`run_from_webview`] is the desktop command. It calls [`run_once`], which
//! blocks for the HTTP exchanges. The command runs that call with
//! `spawn_blocking` so it stays off the Tauri async runtime. The
//! single-flight lock and the budget snapshot are per process.
//!
//! The webview sends an arm id, the environment-variable **name**, and the
//! base URL. It does not send a seed, claims, or a token. The seed and
//! claims are the H3 typical path ([`IN_APP_RUN_SEED`],
//! [`typical_claims`](agent_value_probe_run::typical_claims)). The token is
//! read by `agent-value-probe-http` from that variable or gitignored `.env`
//! and is not written to the store or returned to the page.

use std::path::Path;
use std::time::Duration;

use agent_value_core::arm::Arm;
use agent_value_core::privacy::KeyEnvVarName;
use agent_value_probe_http::base_url_allowed;
use agent_value_probe_run::{typical_claims, ProbeLaunch, ProbeRunOutcome, ProbeRunRequest};
use serde::{Deserialize, Serialize};

use crate::store_cmd::AppState;

/// Seed for the in-app manual run.
///
/// Same non-zero seed the H3 `run_once` tests send with [`typical_claims`].
/// Plan row count does not depend on it; the questions do. The webview
/// does not choose it. Not an observation-period threshold.
pub const IN_APP_RUN_SEED: u64 = 20261022;

/// Per-request timeout. Matches the `tools/probe_live.py` `--timeout` default.
const IN_APP_TIMEOUT: Duration = Duration::from_secs(60);

/// Gitignored dotenv, relative to the process working directory.
/// `load_key_from` reads the environment first and opens this file only
/// when the variable is unset.
const IN_APP_DOTENV: &str = ".env";

/// What the webview sends. `key_env` is a variable name, never the token.
#[derive(Debug, Deserialize)]
pub struct RunOnceInput {
    pub arm_id: String,
    pub key_env: String,
    pub base_url: String,
}

/// Launch state plus a sentence that is safe to show. `detail` does not
/// include a bearer token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunOnceView {
    pub launch: String,
    pub detail: String,
}

impl RunOnceView {
    fn failed(detail: &str) -> Self {
        Self {
            launch: ProbeLaunch::Failed.as_str().to_string(),
            detail: detail.to_string(),
        }
    }

    fn from_outcome(outcome: ProbeRunOutcome) -> Self {
        Self {
            launch: outcome.launch.as_str().to_string(),
            detail: outcome.detail,
        }
    }
}

/// Consent not accepted, a budget stop, or a result already stored for
/// today's run day returns that launch and does not send. A second
/// overlapping call returns `running` and does not send. A send that later
/// fails still records the provisional charge.
pub fn run_once(app: &AppState, request: &ProbeRunRequest<'_>) -> Result<ProbeRunOutcome, String> {
    agent_value_probe_run::run_once(&app.flight, app.store(), request)
        .map_err(|err| err.to_string())
}

/// Resolves `input.arm_id` against stored arms and probe-result arms, then
/// calls [`run_once`].
///
/// Seed and claims are [`IN_APP_RUN_SEED`] and [`typical_claims`]. A rejected
/// name, base URL, or unknown arm returns [`ProbeLaunch::Failed`] and does
/// not open a socket. Error text never includes the submitted name, URL, or
/// token.
pub fn run_from_webview(app: &AppState, input: &RunOnceInput) -> Result<RunOnceView, String> {
    let key_env = input.key_env.trim();
    let base_url = input.base_url.trim();
    let arm_id = input.arm_id.trim();
    if key_env.is_empty() || base_url.is_empty() || arm_id.is_empty() {
        return Ok(RunOnceView::failed(
            "要选择一条臂，并填写密钥变量名和基址。",
        ));
    }
    if KeyEnvVarName::new(key_env).is_err() {
        return Ok(RunOnceView::failed(
            "密钥只填写变量名，例如 RELAY_API_KEY。不要填写密钥本身。",
        ));
    }
    if !base_is_allowed(base_url) {
        return Ok(RunOnceView::failed(
            "基址必须是 https，或本机模拟用的 http。不能带用户信息、查询或片段。",
        ));
    }
    let Some(arm) = find_arm(app, arm_id)? else {
        return Ok(RunOnceView::failed("没有这条臂。"));
    };
    let request = ProbeRunRequest {
        arm: &arm,
        key_env,
        base_url,
        seed: IN_APP_RUN_SEED,
        claims: typical_claims(),
        dotenv: Path::new(IN_APP_DOTENV),
        timeout: IN_APP_TIMEOUT,
    };
    let outcome = match run_once(app, &request) {
        Ok(outcome) => outcome,
        // The page does not render invoke errors. Keep this sentence free of
        // anything the caller typed.
        Err(_) => return Ok(RunOnceView::failed("这次探测没有完成。")),
    };
    Ok(RunOnceView::from_outcome(outcome))
}

fn base_is_allowed(base: &str) -> bool {
    if base.is_empty()
        || base.chars().any(char::is_whitespace)
        || base.contains('@')
        || base.contains('?')
        || base.contains('#')
    {
        return false;
    }
    base_url_allowed(base)
}

fn find_arm(app: &AppState, arm_id: &str) -> Result<Option<Arm>, String> {
    crate::store_cmd::with_store(app, |store| {
        for (_, arm) in store.list_arms().map_err(|err| err.to_string())? {
            if arm.to_string() == arm_id {
                return Ok(Some(arm));
            }
        }
        for result in store.list_probe_results().map_err(|err| err.to_string())? {
            if result.arm.to_string() == arm_id {
                return Ok(Some(result.arm));
            }
        }
        Ok(None)
    })
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

    fn relay_arm() -> Arm {
        Arm::new(
            ModelId::new("probe-model").unwrap(),
            Route::new(
                ProviderKind::Relay,
                "relay.example",
                Protocol::OpenAiChatCompletions,
                "probe-run",
            )
            .unwrap(),
        )
    }

    fn save_relay(app: &AppState) -> String {
        crate::store_cmd::save_arm(
            app,
            crate::store_cmd::ArmInput {
                model: "probe-model".into(),
                provider_kind: "relay".into(),
                host: "relay.example".into(),
                protocol: "openai-chat-completions".into(),
                client: "probe-run".into(),
            },
        )
        .unwrap();
        relay_arm().to_string()
    }

    fn input(arm_id: &str, key_env: &str, base_url: &str) -> RunOnceInput {
        RunOnceInput {
            arm_id: arm_id.to_string(),
            key_env: key_env.to_string(),
            base_url: base_url.to_string(),
        }
    }

    #[test]
    fn the_in_app_seed_matches_the_h3_typical_run() {
        assert_eq!(IN_APP_RUN_SEED, 20261022);
        let claims = typical_claims();
        assert!(claims.tool_call);
        assert!(claims.json_schema);
        assert!(!claims.thinking);
        assert!(!claims.long_context_due);
    }

    #[test]
    fn the_webview_command_accepts_snake_case_fields() {
        let input: RunOnceInput = serde_json::from_value(serde_json::json!({
            "arm_id": "probe-model via relay:relay.example/openai-chat-completions/probe-run",
            "key_env": "RELAY_API_KEY",
            "base_url": "https://relay.example"
        }))
        .unwrap();
        assert_eq!(input.key_env, "RELAY_API_KEY");
        let view = RunOnceView {
            launch: "blocked-consent".into(),
            detail: "detail".into(),
        };
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["launch"], "blocked-consent");
        assert_eq!(json["detail"], "detail");
    }

    #[test]
    fn the_webview_entry_blocks_before_consent_without_a_server() {
        let app = AppState::open_in_memory().unwrap();
        let arm_id = save_relay(&app);
        let view =
            run_from_webview(&app, &input(&arm_id, "RELAY_API_KEY", "http://127.0.0.1:9")).unwrap();
        assert_eq!(view.launch, ProbeLaunch::BlockedConsent.as_str());
        assert_eq!(view.detail, ProbeLaunch::BlockedConsent.label());
        assert!(!view.detail.contains("RELAY_API_KEY"));
        let rows = crate::store_cmd::with_store(&app, |store| {
            store.list_probe_results().map_err(|err| err.to_string())
        })
        .unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn the_webview_entry_refuses_a_secret_typed_as_the_variable_name() {
        let app = AppState::open_in_memory().unwrap();
        let arm_id = save_relay(&app);
        let secret = "sk-test-Ab1cdEf2Gh3ijKl4Mn5";
        let view =
            run_from_webview(&app, &input(&arm_id, secret, "https://relay.example")).unwrap();
        assert_eq!(view.launch, ProbeLaunch::Failed.as_str());
        assert!(!view.detail.contains(secret));
        assert!(!view.detail.to_lowercase().contains("sk-"));
        let rows = crate::store_cmd::with_store(&app, |store| {
            store.list_probe_results().map_err(|err| err.to_string())
        })
        .unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn the_webview_entry_rejects_userinfo_without_echoing_it() {
        let app = AppState::open_in_memory().unwrap();
        let arm_id = save_relay(&app);
        let secret = "super-secret-token-value";
        let base = format!("https://user:{secret}@relay.example");
        let view = run_from_webview(&app, &input(&arm_id, "RELAY_API_KEY", &base)).unwrap();
        assert_eq!(view.launch, ProbeLaunch::Failed.as_str());
        assert!(!view.detail.contains(secret));
        assert!(!view.detail.contains("user:"));
    }

    #[test]
    fn the_webview_entry_rejects_an_unknown_arm_without_a_server() {
        let app = AppState::open_in_memory().unwrap();
        let view = run_from_webview(
            &app,
            &input(
                "missing via relay:relay.example/openai-chat-completions/probe-run",
                "RELAY_API_KEY",
                "http://127.0.0.1:9",
            ),
        )
        .unwrap();
        assert_eq!(view.detail, "没有这条臂。");
        assert!(!view.detail.contains("missing"));
    }
}

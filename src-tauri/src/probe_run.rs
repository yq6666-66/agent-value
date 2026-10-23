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
/// name, base URL, unknown arm, or base host that is not the arm's host
/// returns [`ProbeLaunch::Failed`] and does not open a socket. Error text
/// never includes the submitted name, URL, host, or token.
///
/// `key_env` is still the caller's choice of variable name. The host check
/// below is what binds the send to the selected arm.
pub fn run_from_webview(app: &AppState, input: &RunOnceInput) -> Result<RunOnceView, String> {
    let key_env = input.key_env.trim();
    let base_url = input.base_url.trim().trim_end_matches('/');
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
    // Do not send to a free-typed host. Path and a trailing slash are
    // ignored; the host must be the arm route host.
    if !base_host_matches(base_url, arm.route().host()) {
        return Ok(RunOnceView::failed("基址的主机必须和所选臂一致。"));
    }
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

/// Hostname only. A route host may include `:port` or `[ipv6]:port`.
fn route_hostname(route_host: &str) -> &str {
    if let Some(inner) = route_host.strip_prefix('[') {
        if let Some(end) = inner.find(']') {
            return &inner[..end];
        }
    }
    if let Some((host, port)) = route_host.rsplit_once(':') {
        if !host.is_empty() && !host.contains(':') && port.bytes().all(|byte| byte.is_ascii_digit())
        {
            return host;
        }
    }
    route_host
}

fn url_hostname(base: &str) -> Option<&str> {
    let rest = base
        .strip_prefix("https://")
        .or_else(|| base.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    if authority.is_empty() {
        return None;
    }
    Some(route_hostname(authority))
}

fn base_host_matches(base: &str, route_host: &str) -> bool {
    url_hostname(base).is_some_and(|host| host.eq_ignore_ascii_case(route_hostname(route_host)))
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
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;
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
        let view = run_from_webview(
            &app,
            &input(&arm_id, "RELAY_API_KEY", "https://relay.example/"),
        )
        .unwrap();
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
    fn the_webview_entry_rejects_a_mismatched_host_without_sending() {
        let app = AppState::open_in_memory().unwrap();
        let arm_id = save_relay(&app);
        let submitted = "https://evil.example/probe-path";
        let view = run_from_webview(&app, &input(&arm_id, "RELAY_API_KEY", submitted)).unwrap();
        assert_eq!(view.launch, ProbeLaunch::Failed.as_str());
        assert_eq!(view.detail, "基址的主机必须和所选臂一致。");
        assert!(!view.detail.contains(submitted));
        assert!(!view.detail.contains("evil.example"));
        assert!(!view.detail.contains("RELAY_API_KEY"));
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

    /// A send that fails because the reply repeats the bearer token must not
    /// put that token in [`RunOnceView::detail`]. The page renders `detail`.
    #[test]
    fn a_send_failure_does_not_place_the_key_value_in_the_view_detail() {
        let key = "mock-webview-echo-6c5b4a392817";
        let (base, hits) = spawn_echo_server();
        let app = AppState::open_in_memory().unwrap();
        app.store()
            .lock()
            .unwrap()
            .set_probe_consent(true, 1_792_382_400_000)
            .unwrap();
        let arm_id = save_local(&app);
        let _env = EnvVar::set("PROBE_RUN_WEBVIEW_ECHO", key);
        let view =
            run_from_webview(&app, &input(&arm_id, "PROBE_RUN_WEBVIEW_ECHO", &base)).unwrap();
        assert!(hits.load(Ordering::SeqCst) >= 1);
        assert_eq!(view.launch, ProbeLaunch::Failed.as_str());
        assert!(
            view.detail.contains("refusing to write") && !view.detail.contains(key),
            "send failure detail leaked or was not the refusal sentence"
        );
        let rows = crate::store_cmd::with_store(&app, |store| {
            store.list_probe_results().map_err(|err| err.to_string())
        })
        .unwrap();
        assert!(rows.is_empty());
    }

    fn save_local(app: &AppState) -> String {
        crate::store_cmd::save_arm(
            app,
            crate::store_cmd::ArmInput {
                model: "probe-model".into(),
                provider_kind: "relay".into(),
                host: "127.0.0.1".into(),
                protocol: "openai-chat-completions".into(),
                client: "probe-run".into(),
            },
        )
        .unwrap();
        Arm::new(
            ModelId::new("probe-model").unwrap(),
            Route::new(
                ProviderKind::Relay,
                "127.0.0.1",
                Protocol::OpenAiChatCompletions,
                "probe-run",
            )
            .unwrap(),
        )
        .to_string()
    }

    fn spawn_echo_server() -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&hits);
        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut stream) = conn else { break };
                let counter = Arc::clone(&counter);
                thread::spawn(move || {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                    let Some(request) = read_http(&mut stream) else {
                        return;
                    };
                    counter.fetch_add(1, Ordering::SeqCst);
                    let token = request.auth.strip_prefix("Bearer ").unwrap_or("");
                    let payload = serde_json::json!({
                        "model": "mock-model-1",
                        "choices": [{"message": {"role": "assistant", "content": format!("echo {token}")}}],
                        "usage": {"prompt_tokens": 1}
                    });
                    let body = serde_json::to_vec(&payload).unwrap();
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&body);
                });
            }
        });
        (format!("http://127.0.0.1:{port}"), hits)
    }

    struct RawRequest {
        auth: String,
    }

    fn read_http(stream: &mut TcpStream) -> Option<RawRequest> {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        let header_end = loop {
            if buf.len() > 64 * 1024 {
                return None;
            }
            let n = stream.read(&mut tmp).ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&tmp[..n]);
            if let Some(index) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
                break index;
            }
        };
        let head = String::from_utf8_lossy(&buf[..header_end]);
        let mut auth = String::new();
        let mut content_length = 0usize;
        for line in head.split("\r\n").skip(1) {
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            if name.eq_ignore_ascii_case("authorization") {
                auth = value.trim().to_string();
            }
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
        if content_length > 1_000_000 {
            return None;
        }
        let mut rest = buf[header_end + 4..].to_vec();
        while rest.len() < content_length {
            let n = stream.read(&mut tmp).ok()?;
            if n == 0 {
                break;
            }
            rest.extend_from_slice(&tmp[..n]);
        }
        Some(RawRequest { auth })
    }

    struct EnvVar {
        name: String,
        prev: Option<std::ffi::OsString>,
    }

    impl EnvVar {
        fn set(name: &str, value: &str) -> Self {
            let prev = std::env::var_os(name);
            std::env::set_var(name, value);
            Self {
                name: name.to_string(),
                prev,
            }
        }
    }

    impl Drop for EnvVar {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(value) => std::env::set_var(&self.name, value),
                None => std::env::remove_var(&self.name),
            }
        }
    }
}

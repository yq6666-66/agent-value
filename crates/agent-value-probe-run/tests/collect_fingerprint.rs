//! Official-direct fingerprint baseline collection against a mock HTTP server.
//!
//! Nothing here opens a route to the public network. The bearer token is a
//! test stand-in loaded by environment-variable name. It must not appear in
//! the store or in an error.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use agent_value_core::arm::{Arm, ModelId, Protocol, ProviderKind, Route};
use agent_value_core::constants::PROBE_MONTHLY_BUDGET_YUAN;
use agent_value_core::probe::{FINGERPRINT_PROMPTS, FINGERPRINT_SAMPLES, PROBE_SYSTEM_PROMPT};
use agent_value_core::records::MicroYuan;
use agent_value_probe_run::{
    collect_fingerprint_baseline, fingerprint_collect_estimate, FingerprintBudgetSnapshot,
    FingerprintCollectError, FingerprintCollectRequest, FINGERPRINT_SAMPLING_CONFIG,
    FINGERPRINT_TASK_SET_VERSION,
};
use agent_value_store::{
    sha256_hex, DigestKey, FingerprintBaselineUpsert, FixedClock, Store,
    DISTRIBUTION_SCHEMA_VERSION,
};

/// Not a real credential. `sk-` shape so a leak is obvious.
const KEY: &str = "sk-mock-fingerprint-test-key";
const KEY_ENV: &str = "FINGERPRINT_COLLECT_TEST_KEY";
const NOW_MS: i64 = 1_700_000_000_000;

fn lock_tests() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|err| err.into_inner())
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

fn require_unset(var: &str) {
    if std::env::var_os(var).is_some() {
        panic!("{var} is set in the environment; refusing to run this test");
    }
}

fn open_store(now_ms: i64) -> (Store, FixedClock) {
    let clock = FixedClock::new(now_ms);
    let key = DigestKey::from_bytes(&[9u8; 32]).unwrap();
    let store = Store::open_in_memory_with_clock(key, clock.clone()).unwrap();
    (store, clock)
}

fn arm(kind: ProviderKind, host: &str, protocol: Protocol) -> Arm {
    Arm::new(
        ModelId::new("example-model").unwrap(),
        Route::new(kind, host, protocol, "fingerprint-collect").unwrap(),
    )
}

fn allowing_budget() -> FingerprintBudgetSnapshot {
    FingerprintBudgetSnapshot {
        spent: MicroYuan::new(0).unwrap(),
        budget: fingerprint_collect_estimate().unwrap(),
    }
}

fn monthly_budget() -> FingerprintBudgetSnapshot {
    FingerprintBudgetSnapshot {
        spent: MicroYuan::new(0).unwrap(),
        budget: MicroYuan::from_yuan(PROBE_MONTHLY_BUDGET_YUAN).unwrap(),
    }
}

enum Fail {
    None,
    Status(u16),
    Garbage,
    Delay(Duration),
}

struct Script {
    content: String,
    echo: bool,
    fail_at: Option<usize>,
    fail: Fail,
}

struct Mock {
    base: String,
    hits: Arc<AtomicUsize>,
    script: Arc<Mutex<Script>>,
    bodies: Arc<Mutex<Vec<String>>>,
}

impl Mock {
    fn spawn(script: Script) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let script = Arc::new(Mutex::new(script));
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let hits_bg = Arc::clone(&hits);
        let script_bg = Arc::clone(&script);
        let bodies_bg = Arc::clone(&bodies);
        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(stream) = conn else { break };
                let hits_bg = Arc::clone(&hits_bg);
                let script_bg = Arc::clone(&script_bg);
                let bodies_bg = Arc::clone(&bodies_bg);
                thread::spawn(move || handle(stream, &hits_bg, &script_bg, &bodies_bg));
            }
        });
        Self {
            base: format!("http://127.0.0.1:{port}"),
            hits,
            script,
            bodies,
        }
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

fn handle(
    mut stream: TcpStream,
    hits: &AtomicUsize,
    script: &Mutex<Script>,
    bodies: &Mutex<Vec<String>>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let Some(request) = read_http(&mut stream) else {
        return;
    };
    let n = hits.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut saved) = bodies.lock() {
        saved.push(request.body);
    }
    let script = script.lock().unwrap_or_else(|err| err.into_inner());
    if script.fail_at == Some(n) {
        match script.fail {
            Fail::Status(status) => {
                let _ = write_http(&mut stream, status, b"{}");
                return;
            }
            Fail::Garbage => {
                let _ = write_http(&mut stream, 200, b"not-json");
                return;
            }
            Fail::Delay(delay) => {
                drop(script);
                thread::sleep(delay);
                let _ = write_http(&mut stream, 200, &ok_body("late"));
                return;
            }
            Fail::None => {}
        }
    }
    let content = if script.echo {
        request
            .auth
            .strip_prefix("Bearer ")
            .unwrap_or("")
            .to_string()
    } else {
        script.content.clone()
    };
    drop(script);
    let _ = write_http(&mut stream, 200, &ok_body(&content));
}

fn ok_body(content: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "model": "mock-model",
        "choices": [{"message": {"role": "assistant", "content": content}}],
        "usage": {"prompt_tokens": 1}
    }))
    .unwrap()
}

struct RawRequest {
    auth: String,
    body: String,
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
    rest.truncate(content_length);
    Some(RawRequest {
        auth,
        body: String::from_utf8_lossy(&rest).into_owned(),
    })
}

fn write_http(stream: &mut TcpStream, status: u16, body: &[u8]) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    Ok(())
}

fn text_script(content: &str) -> Script {
    Script {
        content: content.to_string(),
        echo: false,
        fail_at: None,
        fail: Fail::None,
    }
}

#[allow(clippy::too_many_arguments)]
fn collect<'a>(
    store: &Store,
    clock: &FixedClock,
    arm: &'a Arm,
    base: &'a str,
    key_env: &'a str,
    consent: bool,
    budget: FingerprintBudgetSnapshot,
    timeout: Duration,
) -> Result<agent_value_probe_run::FingerprintCollectOutcome, FingerprintCollectError> {
    collect_fingerprint_baseline(
        store,
        clock,
        &FingerprintCollectRequest {
            arm,
            base_url: base,
            key_env,
            consent,
            budget,
            timeout,
        },
    )
}

fn assert_untouched(store: &Store) {
    assert!(store.list_fingerprint_baselines(true).unwrap().is_empty());
    assert!(store.list_probe_results().unwrap().is_empty());
    assert!(store.probe_charges().unwrap().is_empty());
}

fn assert_no_secret(store: &Store, rendered: &str) {
    assert!(
        !rendered.contains(KEY),
        "rendered error included the test token"
    );
    assert!(!dump_contains(store, KEY), "store included the test token");
}

fn dump_contains(store: &Store, needle: &str) -> bool {
    let dump = store.export().unwrap();
    dump.tables.iter().any(|table| {
        table.rows.iter().any(|row| {
            row.iter()
                .any(|cell| cell.as_ref().is_some_and(|text| text.contains(needle)))
        })
    })
}

fn rendered(err: &FingerprintCollectError) -> String {
    format!("{err} {err:?}")
}

#[test]
fn success_writes_a_readable_baseline_and_a_later_collect_overwrites_it() {
    let _guard = lock_tests();
    require_unset(KEY_ENV);
    let _env = EnvVar::set(KEY_ENV, KEY);
    let mock = Mock::spawn(text_script("Red"));
    let (store, clock) = open_store(NOW_MS);
    let arm = arm(
        ProviderKind::Official,
        "127.0.0.1",
        Protocol::OpenAiChatCompletions,
    );
    let outcome = collect(
        &store,
        &clock,
        &arm,
        &mock.base,
        KEY_ENV,
        true,
        allowing_budget(),
        Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(outcome.upsert, FingerprintBaselineUpsert::Inserted);
    assert_eq!(outcome.requests_sent, 400);
    assert_eq!(mock.hits(), 400);
    assert_eq!(outcome.collected_at_ms, NOW_MS);
    assert_eq!(outcome.identity.provider, "127.0.0.1");
    assert_eq!(outcome.identity.model_id, "example-model");
    assert_eq!(outcome.identity.model_revision, "");
    assert_eq!(outcome.identity.protocol, "openai-chat-completions");
    assert_eq!(outcome.identity.connection_kind, "official-direct");
    assert_eq!(
        outcome.identity.task_set_version,
        FINGERPRINT_TASK_SET_VERSION
    );
    assert_eq!(
        outcome.identity.system_prompt_hash,
        sha256_hex(PROBE_SYSTEM_PROMPT.as_bytes())
    );
    assert_eq!(
        outcome.identity.sampling_config,
        FINGERPRINT_SAMPLING_CONFIG
    );
    assert_eq!(
        outcome.identity.distribution_schema_version,
        DISTRIBUTION_SCHEMA_VERSION
    );
    let got = store
        .get_fingerprint_baseline(&outcome.identity)
        .unwrap()
        .expect("stored baseline");
    assert_eq!(got.collected_at_ms, NOW_MS);
    for run in [&got.distribution.run1, &got.distribution.run2] {
        assert_eq!(run.len(), FINGERPRINT_PROMPTS.len());
        for question in run {
            assert_eq!(question.answers.len(), 1);
            assert_eq!(question.answers[0].answer, "red");
            assert_eq!(question.answers[0].count, FINGERPRINT_SAMPLES);
        }
    }
    assert!(store.list_probe_results().unwrap().is_empty());
    assert!(store.probe_charges().unwrap().is_empty());
    assert!(!dump_contains(&store, KEY));
    assert!(!dump_contains(&store, PROBE_SYSTEM_PROMPT));
    assert!(!dump_contains(&store, FINGERPRINT_PROMPTS[0]));
    let bodies = mock.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 400);
    let first: serde_json::Value = serde_json::from_str(&bodies[0]).unwrap();
    let sampling: serde_json::Value = serde_json::from_str(FINGERPRINT_SAMPLING_CONFIG).unwrap();
    assert_eq!(first["temperature"], sampling["temperature"]);
    assert_eq!(first["top_p"], sampling["top_p"]);
    assert_eq!(first["max_tokens"], sampling["max_tokens"]);
    assert_eq!(first["messages"][0]["content"], PROBE_SYSTEM_PROMPT);
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for body in bodies.iter() {
        let value: serde_json::Value = serde_json::from_str(body).unwrap();
        let prompt = value["messages"][1]["content"]
            .as_str()
            .unwrap()
            .to_string();
        *counts.entry(prompt).or_insert(0) += 1;
    }
    drop(bodies);
    for prompt in FINGERPRINT_PROMPTS {
        assert_eq!(counts.get(prompt).copied().unwrap_or(0), 40, "{prompt}");
    }

    mock.script.lock().unwrap().content = "Blue".into();
    let again = collect(
        &store,
        &clock,
        &arm,
        &mock.base,
        KEY_ENV,
        true,
        allowing_budget(),
        Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(again.upsert, FingerprintBaselineUpsert::Updated);
    assert_eq!(again.identity, outcome.identity);
    let got = store
        .get_fingerprint_baseline(&outcome.identity)
        .unwrap()
        .unwrap();
    assert_eq!(got.distribution.run1[0].answers[0].answer, "blue");
    assert_eq!(store.list_fingerprint_baselines(true).unwrap().len(), 1);
    assert!(store.list_probe_results().unwrap().is_empty());

    mock.script.lock().unwrap().fail_at = Some(mock.hits());
    mock.script.lock().unwrap().fail = Fail::Status(500);
    let err = collect(
        &store,
        &clock,
        &arm,
        &mock.base,
        KEY_ENV,
        true,
        allowing_budget(),
        Duration::from_secs(5),
    )
    .unwrap_err();
    assert_eq!(
        err,
        FingerprintCollectError::SampleFailed { kind: "http_error" }
    );
    let got = store
        .get_fingerprint_baseline(&outcome.identity)
        .unwrap()
        .unwrap();
    assert_eq!(got.distribution.run1[0].answers[0].answer, "blue");
    assert_eq!(store.list_fingerprint_baselines(true).unwrap().len(), 1);
    assert_no_secret(&store, &rendered(&err));
}

#[test]
fn precondition_failures_do_not_send_or_write() {
    let _guard = lock_tests();
    require_unset(KEY_ENV);
    let _env = EnvVar::set(KEY_ENV, KEY);
    let mock = Mock::spawn(text_script("Red"));
    let official = arm(
        ProviderKind::Official,
        "127.0.0.1",
        Protocol::OpenAiChatCompletions,
    );
    let (store, clock) = open_store(NOW_MS);
    let timeout = Duration::from_secs(2);

    let err = collect(
        &store,
        &clock,
        &official,
        &mock.base,
        KEY_ENV,
        false,
        allowing_budget(),
        timeout,
    )
    .unwrap_err();
    assert_eq!(err, FingerprintCollectError::ConsentDenied);
    assert_eq!(mock.hits(), 0);
    assert_untouched(&store);

    let relay = arm(
        ProviderKind::Relay,
        "127.0.0.1",
        Protocol::OpenAiChatCompletions,
    );
    let err = collect(
        &store,
        &clock,
        &relay,
        &mock.base,
        KEY_ENV,
        true,
        allowing_budget(),
        timeout,
    )
    .unwrap_err();
    assert_eq!(err, FingerprintCollectError::NotOfficialDirect);
    assert_eq!(mock.hits(), 0);

    let anthropic = arm(
        ProviderKind::Official,
        "127.0.0.1",
        Protocol::AnthropicMessages,
    );
    let err = collect(
        &store,
        &clock,
        &anthropic,
        &mock.base,
        KEY_ENV,
        true,
        allowing_budget(),
        timeout,
    )
    .unwrap_err();
    assert_eq!(err, FingerprintCollectError::UnsupportedProtocol);
    assert_eq!(mock.hits(), 0);

    let elsewhere = arm(
        ProviderKind::Official,
        "api.openai.com",
        Protocol::OpenAiChatCompletions,
    );
    let err = collect(
        &store,
        &clock,
        &elsewhere,
        &mock.base,
        KEY_ENV,
        true,
        allowing_budget(),
        timeout,
    )
    .unwrap_err();
    assert_eq!(err, FingerprintCollectError::HostRejected);
    assert_eq!(mock.hits(), 0);

    let plain = arm(
        ProviderKind::Official,
        "192.0.2.1",
        Protocol::OpenAiChatCompletions,
    );
    let err = collect(
        &store,
        &clock,
        &plain,
        "http://192.0.2.1/v1",
        KEY_ENV,
        true,
        allowing_budget(),
        timeout,
    )
    .unwrap_err();
    assert_eq!(err, FingerprintCollectError::HostRejected);
    assert_eq!(mock.hits(), 0);

    let userinfo = format!("http://{KEY}@127.0.0.1:9/");
    let err = collect(
        &store,
        &clock,
        &official,
        &userinfo,
        KEY_ENV,
        true,
        allowing_budget(),
        timeout,
    )
    .unwrap_err();
    assert_eq!(err, FingerprintCollectError::HostRejected);
    assert!(!rendered(&err).contains(KEY));
    assert_eq!(mock.hits(), 0);

    let err = collect(
        &store,
        &clock,
        &official,
        &mock.base,
        KEY_ENV,
        true,
        monthly_budget(),
        timeout,
    )
    .unwrap_err();
    assert_eq!(err, FingerprintCollectError::BudgetStop);
    assert_eq!(mock.hits(), 0);

    let estimate = fingerprint_collect_estimate().unwrap();
    let short = FingerprintBudgetSnapshot {
        spent: MicroYuan::new(0).unwrap(),
        budget: MicroYuan::new(estimate.micros() - 1).unwrap(),
    };
    let err = collect(
        &store, &clock, &official, &mock.base, KEY_ENV, true, short, timeout,
    )
    .unwrap_err();
    assert_eq!(err, FingerprintCollectError::BudgetStop);
    assert_eq!(mock.hits(), 0);

    let spent_all = FingerprintBudgetSnapshot {
        spent: MicroYuan::from_yuan(1.0).unwrap(),
        budget: MicroYuan::from_yuan(1.0).unwrap(),
    };
    let err = collect(
        &store, &clock, &official, &mock.base, KEY_ENV, true, spent_all, timeout,
    )
    .unwrap_err();
    assert_eq!(err, FingerprintCollectError::BudgetStop);
    assert_eq!(mock.hits(), 0);

    let (early, early_clock) = open_store(-1);
    let err = collect(
        &early,
        &early_clock,
        &official,
        &mock.base,
        KEY_ENV,
        true,
        allowing_budget(),
        timeout,
    )
    .unwrap_err();
    assert_eq!(err, FingerprintCollectError::ClockOutOfRange);
    assert_eq!(mock.hits(), 0);
    assert_untouched(&store);
    assert_untouched(&early);
    assert_no_secret(&store, &rendered(&err));
}

#[test]
fn unusable_key_does_not_send_and_is_not_in_the_error() {
    let _guard = lock_tests();
    require_unset(KEY_ENV);
    let mock = Mock::spawn(text_script("Red"));
    let (store, clock) = open_store(NOW_MS);
    let official = arm(
        ProviderKind::Official,
        "127.0.0.1",
        Protocol::OpenAiChatCompletions,
    );
    let timeout = Duration::from_secs(2);

    let err = collect(
        &store,
        &clock,
        &official,
        &mock.base,
        KEY_ENV,
        true,
        allowing_budget(),
        timeout,
    )
    .unwrap_err();
    assert_eq!(
        err,
        FingerprintCollectError::KeyNotSet {
            name: KEY_ENV.into()
        }
    );
    assert!(!rendered(&err).contains(KEY));
    assert_eq!(mock.hits(), 0);

    let err = collect(
        &store,
        &clock,
        &official,
        &mock.base,
        KEY,
        true,
        allowing_budget(),
        timeout,
    )
    .unwrap_err();
    assert_eq!(err, FingerprintCollectError::BadVarName);
    assert!(!rendered(&err).contains(KEY));
    assert!(!rendered(&err).contains("sk-"));
    assert_eq!(mock.hits(), 0);

    let _env = EnvVar::set(KEY_ENV, "bad key");
    let err = collect(
        &store,
        &clock,
        &official,
        &mock.base,
        KEY_ENV,
        true,
        allowing_budget(),
        timeout,
    )
    .unwrap_err();
    assert_eq!(err, FingerprintCollectError::InvalidKey);
    assert!(!rendered(&err).contains("bad key"));
    assert!(!rendered(&err).contains(KEY));
    assert_eq!(mock.hits(), 0);
    assert_untouched(&store);
}

#[test]
fn partial_sample_failure_writes_nothing() {
    let _guard = lock_tests();
    require_unset(KEY_ENV);
    let _env = EnvVar::set(KEY_ENV, KEY);
    let cases = [
        (Fail::Status(500), "http_error"),
        (Fail::Status(429), "rate_limited"),
        (Fail::Garbage, "unparseable"),
        (Fail::Delay(Duration::from_millis(1_000)), "timeout"),
    ];
    for (fail, kind) in cases {
        let mock = Mock::spawn(Script {
            content: "Red".into(),
            echo: false,
            fail_at: Some(1),
            fail,
        });
        let (store, clock) = open_store(NOW_MS);
        let official = arm(
            ProviderKind::Official,
            "127.0.0.1",
            Protocol::OpenAiChatCompletions,
        );
        let timeout = if kind == "timeout" {
            Duration::from_millis(300)
        } else {
            Duration::from_secs(5)
        };
        let err = collect(
            &store,
            &clock,
            &official,
            &mock.base,
            KEY_ENV,
            true,
            allowing_budget(),
            timeout,
        )
        .unwrap_err();
        assert_eq!(
            err,
            FingerprintCollectError::SampleFailed { kind },
            "{kind}"
        );
        assert_eq!(mock.hits(), 2, "{kind}");
        assert_untouched(&store);
        assert_no_secret(&store, &rendered(&err));
    }
}

#[test]
fn a_reply_that_echoes_the_key_is_not_stored_or_reported() {
    let _guard = lock_tests();
    require_unset(KEY_ENV);
    let _env = EnvVar::set(KEY_ENV, KEY);
    let mock = Mock::spawn(Script {
        content: String::new(),
        echo: true,
        fail_at: None,
        fail: Fail::None,
    });
    let (store, clock) = open_store(NOW_MS);
    let official = arm(
        ProviderKind::Official,
        "127.0.0.1",
        Protocol::OpenAiChatCompletions,
    );
    let err = collect(
        &store,
        &clock,
        &official,
        &mock.base,
        KEY_ENV,
        true,
        allowing_budget(),
        Duration::from_secs(5),
    )
    .unwrap_err();
    assert_eq!(err, FingerprintCollectError::AnswerRejected);
    assert_eq!(mock.hits(), 1);
    assert_untouched(&store);
    assert_no_secret(&store, &rendered(&err));
}

#[test]
fn an_empty_or_secret_shaped_answer_writes_nothing() {
    let _guard = lock_tests();
    require_unset(KEY_ENV);
    let _env = EnvVar::set(KEY_ENV, KEY);
    for content in ["   ", "sk-other-token-value"] {
        let mock = Mock::spawn(text_script(content));
        let (store, clock) = open_store(NOW_MS);
        let official = arm(
            ProviderKind::Official,
            "127.0.0.1",
            Protocol::OpenAiChatCompletions,
        );
        let err = collect(
            &store,
            &clock,
            &official,
            &mock.base,
            KEY_ENV,
            true,
            allowing_budget(),
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert_eq!(err, FingerprintCollectError::AnswerRejected);
        assert_eq!(mock.hits(), 1);
        assert_untouched(&store);
        let text = rendered(&err);
        assert!(!text.contains("sk-other-token-value"));
        assert!(!text.contains(KEY));
    }
}

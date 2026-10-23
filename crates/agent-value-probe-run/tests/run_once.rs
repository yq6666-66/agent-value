//! Consent and budget gates around one mock HTTP probe run.
//!
//! The server binds `127.0.0.1` on an ephemeral port. The bearer token is a
//! test stand-in loaded by environment-variable name. Nothing here is a real
//! credential, and the stand-in must not appear in the store or the outcome.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use agent_value_core::arm::{Arm, ModelId, Protocol, ProviderKind, Route};
use agent_value_core::constants::PROBE_MONTHLY_BUDGET_YUAN;
use agent_value_core::probe::CapabilityClaims;
use agent_value_core::records::MicroYuan;
use agent_value_probe_run::{
    run_once, typical_claims, typical_plan_rows, typical_probe_estimate, Flight, ProbeLaunch,
    ProbeRunRequest, PROBE_ESTIMATE_PER_ITEM_MICROS,
};
use agent_value_store::{DigestKey, Store};

/// Not a real credential. Same stand-in shape as the probe HTTP tests.
const KEY: &str = "mock-test-key-0f1e2d3c4b5a";
/// Reply text that must stay out of evidence summaries.
const REPLY_MARKER: &str = "count-only-marker";

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

fn store() -> Mutex<Store> {
    let key = DigestKey::from_bytes(&[9u8; 32]).unwrap();
    Mutex::new(Store::open_in_memory(key).unwrap())
}

fn arm(protocol: Protocol) -> Arm {
    Arm::new(
        ModelId::new("probe-model").unwrap(),
        Route::new(ProviderKind::Relay, "relay.example", protocol, "probe-run").unwrap(),
    )
}

fn request<'a>(
    arm: &'a Arm,
    key_env: &'a str,
    base: &'a str,
    claims: CapabilityClaims,
) -> ProbeRunRequest<'a> {
    ProbeRunRequest {
        arm,
        key_env,
        base_url: base,
        seed: 20261022,
        claims,
        dotenv: Path::new("/tmp/agent-value-probe-run-missing.env"),
        timeout: Duration::from_secs(5),
    }
}

struct Mock {
    base: String,
    hits: Arc<AtomicUsize>,
}

impl Mock {
    fn spawn(delay_first: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&hits);
        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(stream) = conn else { break };
                let counter = Arc::clone(&counter);
                thread::spawn(move || handle(stream, &counter, delay_first));
            }
        });
        Self {
            base: format!("http://127.0.0.1:{port}"),
            hits,
        }
    }
}

fn handle(mut stream: TcpStream, hits: &AtomicUsize, delay_first: Duration) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let Some(request) = read_http(&mut stream) else {
        return;
    };
    let n = hits.fetch_add(1, Ordering::SeqCst);
    if n == 0 && !delay_first.is_zero() {
        thread::sleep(delay_first);
    }
    let token = request
        .auth
        .strip_prefix("Bearer ")
        .unwrap_or("")
        .to_string();
    let body = if request.body.contains("echo-key") {
        ok_body(&format!("echo {token}"))
    } else {
        ok_body(REPLY_MARKER)
    };
    let _ = write_http(&mut stream, &body);
}

fn ok_body(content: &str) -> Vec<u8> {
    let payload = serde_json::json!({
        "model": "mock-model-1",
        "choices": [{"message": {"role": "assistant", "content": content}}],
        "usage": {"prompt_tokens": 1}
    });
    serde_json::to_vec(&payload).unwrap()
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

fn write_http(stream: &mut TcpStream, body: &[u8]) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    Ok(())
}

fn rendered_store(store: &Store) -> String {
    let mut out = String::new();
    for result in store.list_probe_results().unwrap() {
        out.push_str(&format!("{result:?}\n"));
        out.push_str(&result.evidence_summary);
        out.push('\n');
    }
    for (at, micros) in store.probe_charges().unwrap() {
        out.push_str(&format!("charge {at} {micros}\n"));
    }
    out
}

fn assert_no_secret(text: &str) {
    assert!(!text.contains(KEY), "output contains the test token");
    assert!(
        !text.contains("Bearer "),
        "output contains an authorization header"
    );
    assert!(!text.contains(REPLY_MARKER), "output contains reply text");
}

#[test]
fn typical_estimate_is_plan_rows_times_the_provisional_rate() {
    let rows = typical_plan_rows().unwrap();
    let estimate = typical_probe_estimate().unwrap();
    assert!(rows > 10, "a typical plan includes capability rows");
    assert_eq!(
        estimate.micros(),
        i64::try_from(rows).unwrap() * PROBE_ESTIMATE_PER_ITEM_MICROS
    );
    assert!(estimate.micros() > 0);
    let budget = MicroYuan::from_yuan(PROBE_MONTHLY_BUDGET_YUAN).unwrap();
    assert!(estimate.micros() < budget.micros());
}

#[test]
fn no_consent_blocks_without_http_or_rows() {
    let _guard = lock_tests();
    let mock = Mock::spawn(Duration::ZERO);
    let db = store();
    let flight = Flight::new();
    let selected = arm(Protocol::OpenAiChatCompletions);
    let _env = EnvVar::set("PROBE_RUN_TEST_UNDECIDED", KEY);
    let outcome = run_once(
        &flight,
        &db,
        &request(
            &selected,
            "PROBE_RUN_TEST_UNDECIDED",
            &mock.base,
            typical_claims(),
        ),
    )
    .unwrap();
    assert_eq!(outcome.launch, ProbeLaunch::BlockedConsent);
    assert_eq!(mock.hits.load(Ordering::SeqCst), 0);
    let store = db.lock().unwrap();
    assert!(store.list_probe_results().unwrap().is_empty());
    assert!(store.probe_charges().unwrap().is_empty());
    assert_no_secret(&format!("{outcome:?} {}", rendered_store(&store)));

    drop(store);
    db.lock()
        .unwrap()
        .set_probe_consent(false, 1_792_382_400_000)
        .unwrap();
    let declined = run_once(
        &flight,
        &db,
        &request(
            &selected,
            "PROBE_RUN_TEST_UNDECIDED",
            &mock.base,
            typical_claims(),
        ),
    )
    .unwrap();
    assert_eq!(declined.launch, ProbeLaunch::BlockedConsent);
    assert_eq!(mock.hits.load(Ordering::SeqCst), 0);
    let store = db.lock().unwrap();
    assert!(store.list_probe_results().unwrap().is_empty());
    assert_no_secret(&format!("{declined:?} {}", rendered_store(&store)));
}

#[test]
fn over_budget_blocks_without_http_or_new_rows() {
    let _guard = lock_tests();
    let mock = Mock::spawn(Duration::ZERO);
    let db = store();
    let flight = Flight::new();
    let at = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    {
        let store = db.lock().unwrap();
        store.set_probe_consent(true, at).unwrap();
        let budget = MicroYuan::from_yuan(PROBE_MONTHLY_BUDGET_YUAN).unwrap();
        // One micro under the cap. A zero estimate would still allow this.
        let spent = MicroYuan::new(budget.micros() - 1).unwrap();
        store.record_probe_charge(at, spent).unwrap();
    }
    let selected = arm(Protocol::OpenAiChatCompletions);
    let _env = EnvVar::set("PROBE_RUN_TEST_BUDGET", KEY);
    let outcome = run_once(
        &flight,
        &db,
        &request(
            &selected,
            "PROBE_RUN_TEST_BUDGET",
            &mock.base,
            typical_claims(),
        ),
    )
    .unwrap();
    assert_eq!(outcome.launch, ProbeLaunch::BlockedBudget);
    assert_eq!(mock.hits.load(Ordering::SeqCst), 0);
    let store = db.lock().unwrap();
    assert!(store.list_probe_results().unwrap().is_empty());
    assert_eq!(store.probe_charges().unwrap().len(), 1);
    assert_no_secret(&format!("{outcome:?} {}", rendered_store(&store)));
}

#[test]
fn consent_and_budget_write_results_and_a_charge_without_the_key() {
    let _guard = lock_tests();
    let mock = Mock::spawn(Duration::ZERO);
    let db = store();
    let flight = Flight::new();
    db.lock()
        .unwrap()
        .set_probe_consent(true, 1_792_382_400_000)
        .unwrap();
    let selected = arm(Protocol::OpenAiChatCompletions);
    let _env = EnvVar::set("PROBE_RUN_TEST_OK", KEY);
    let outcome = run_once(
        &flight,
        &db,
        &request(&selected, "PROBE_RUN_TEST_OK", &mock.base, typical_claims()),
    )
    .unwrap();
    assert_eq!(outcome.launch, ProbeLaunch::Completed);
    let rows = typical_plan_rows().unwrap();
    assert_eq!(mock.hits.load(Ordering::SeqCst), rows);
    let store = db.lock().unwrap();
    let results = store.list_probe_results().unwrap();
    assert_eq!(results.len(), 2);
    let classes: Vec<_> = results.iter().map(|result| result.class.as_str()).collect();
    assert!(classes.contains(&"deterministic"));
    assert!(classes.contains(&"capability"));
    for result in &results {
        assert_eq!(result.arm, selected);
        assert!(!result.evidence_summary.contains(KEY));
        assert!(!result.evidence_summary.contains(REPLY_MARKER));
        assert!(result.evidence_summary.contains("correct"));
        assert!(result.evidence_summary.contains("unanswered"));
    }
    let charges = store.probe_charges().unwrap();
    assert_eq!(charges.len(), 1);
    assert_eq!(
        charges[0].1,
        i64::try_from(rows).unwrap() * PROBE_ESTIMATE_PER_ITEM_MICROS
    );
    assert_no_secret(&format!("{outcome:?}\n{}", rendered_store(&store)));
    assert_eq!(
        flight.launch(true, agent_value_core::authenticity::BudgetDecision::Allow),
        ProbeLaunch::Completed
    );
}

#[test]
fn a_second_call_while_running_does_not_send_again() {
    let _guard = lock_tests();
    let mock = Mock::spawn(Duration::from_millis(400));
    let db = Arc::new(store());
    let flight = Arc::new(Flight::new());
    db.lock()
        .unwrap()
        .set_probe_consent(true, 1_792_382_400_000)
        .unwrap();
    let selected = arm(Protocol::OpenAiChatCompletions);
    let _env = EnvVar::set("PROBE_RUN_TEST_FLIGHT", KEY);
    let first_db = Arc::clone(&db);
    let first_flight = Arc::clone(&flight);
    let base = mock.base.clone();
    let first = thread::spawn(move || {
        let selected = arm(Protocol::OpenAiChatCompletions);
        run_once(
            &first_flight,
            &first_db,
            &request(
                &selected,
                "PROBE_RUN_TEST_FLIGHT",
                &base,
                CapabilityClaims::default(),
            ),
        )
        .unwrap()
    });
    let started = Instant::now();
    while mock.hits.load(Ordering::SeqCst) == 0 {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "mock was not hit"
        );
        thread::sleep(Duration::from_millis(10));
    }
    let overlap = Instant::now();
    let second = run_once(
        &flight,
        &db,
        &request(
            &selected,
            "PROBE_RUN_TEST_FLIGHT",
            &mock.base,
            CapabilityClaims::default(),
        ),
    )
    .unwrap();
    assert!(
        overlap.elapsed() < Duration::from_millis(250),
        "the overlapping call waited on the in-flight run"
    );
    assert_eq!(second.launch, ProbeLaunch::Running);
    let first_outcome = first.join().unwrap();
    assert_eq!(first_outcome.launch, ProbeLaunch::Completed);
    assert_eq!(
        mock.hits.load(Ordering::SeqCst),
        typical_plan_rows_for_default_claims()
    );
    let text = format!(
        "{second:?} {first_outcome:?} {}",
        rendered_store(&db.lock().unwrap())
    );
    assert_no_secret(&text);
}

fn typical_plan_rows_for_default_claims() -> usize {
    agent_value_core::probe::build_openai_chat_plan(
        20261022,
        "probe-model",
        &CapabilityClaims::default(),
    )
    .unwrap()
    .len()
}

#[test]
fn a_non_openai_arm_is_rejected_without_http() {
    let _guard = lock_tests();
    let mock = Mock::spawn(Duration::ZERO);
    let db = store();
    let flight = Flight::new();
    db.lock()
        .unwrap()
        .set_probe_consent(true, 1_792_382_400_000)
        .unwrap();
    let selected = arm(Protocol::AnthropicMessages);
    let _env = EnvVar::set("PROBE_RUN_TEST_PROTO", KEY);
    let outcome = run_once(
        &flight,
        &db,
        &request(
            &selected,
            "PROBE_RUN_TEST_PROTO",
            &mock.base,
            typical_claims(),
        ),
    )
    .unwrap();
    assert_eq!(outcome.launch, ProbeLaunch::Failed);
    assert_eq!(mock.hits.load(Ordering::SeqCst), 0);
    let store = db.lock().unwrap();
    assert!(store.list_probe_results().unwrap().is_empty());
    assert!(store.probe_charges().unwrap().is_empty());
    assert_no_secret(&format!("{outcome:?} {}", rendered_store(&store)));
}

#[test]
fn echoed_key_refusal_writes_nothing() {
    let _guard = lock_tests();
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
                let body = ok_body(&format!("echo {token}"));
                let _ = write_http(&mut stream, &body);
            });
        }
    });
    let base = format!("http://127.0.0.1:{port}");
    let db = store();
    let flight = Flight::new();
    db.lock()
        .unwrap()
        .set_probe_consent(true, 1_792_382_400_000)
        .unwrap();
    let selected = arm(Protocol::OpenAiChatCompletions);
    let _env = EnvVar::set("PROBE_RUN_TEST_ECHO2", KEY);
    let outcome = run_once(
        &flight,
        &db,
        &request(
            &selected,
            "PROBE_RUN_TEST_ECHO2",
            &base,
            CapabilityClaims::default(),
        ),
    )
    .unwrap();
    assert_eq!(outcome.launch, ProbeLaunch::Failed);
    assert!(hits.load(Ordering::SeqCst) > 0);
    let store = db.lock().unwrap();
    assert!(store.list_probe_results().unwrap().is_empty());
    assert!(store.probe_charges().unwrap().is_empty());
    assert_no_secret(&format!("{outcome:?} {}", rendered_store(&store)));
}

#[test]
fn a_key_passed_as_the_variable_name_is_not_echoed_or_sent() {
    let _guard = lock_tests();
    let mock = Mock::spawn(Duration::ZERO);
    let db = store();
    let flight = Flight::new();
    db.lock()
        .unwrap()
        .set_probe_consent(true, 1_792_382_400_000)
        .unwrap();
    let selected = arm(Protocol::OpenAiChatCompletions);
    let outcome = run_once(
        &flight,
        &db,
        &request(&selected, KEY, &mock.base, typical_claims()),
    )
    .unwrap();
    assert_eq!(outcome.launch, ProbeLaunch::Failed);
    assert_eq!(mock.hits.load(Ordering::SeqCst), 0);
    let store = db.lock().unwrap();
    assert!(store.list_probe_results().unwrap().is_empty());
    assert_no_secret(&format!("{outcome:?} {}", rendered_store(&store)));
}

//! In-process sender for one minimal probe run.
//!
//! This is the desktop counterpart of `tools/probe_live.py`. The core crate
//! still only judges recorded exchanges. The probe panel does not call this
//! crate yet.
//!
//! The bearer token is read by environment-variable name from the process
//! environment or a gitignored dotenv file. It is never written to the replay
//! or the log. Request headers are not recorded. Before each replay line and
//! log line is written, the raw reply fields, the tab-separated line, and the
//! JSON log line are checked for the token, its tab-separated escaping, and
//! its JSON escaping (the same three needles as the Python runner). A match
//! refuses the write.
//!
//! Plain `http` is accepted only for `127.0.0.1`, `localhost`, and `::1`.
//! Every other base must be `https`. Redirects are not followed, so the
//! bearer token is not sent to another host. `HTTP_PROXY`, `HTTPS_PROXY`, and
//! `ALL_PROXY` are ignored for the same reason.
//!
//! Do not log [`SendResult`] or the string returned by [`load_key`]: the reply
//! text and the token are in those values. A debug logger attached to `ureq`
//! can also print request headers; leave that target off.

#![forbid(unsafe_code)]

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;

/// Replay header shared with `agent_value_core::probe::REPLAY_HEADER` and
/// `tools/probe_live.py`.
pub const REPLAY_HEADER: &str =
    "question_id\tat_ms\tlatency_ms\tkind\tstatus\tmodel\tusage_fields\ttext\ttool_name\ttool_args";

pub const KIND_OK: &str = "ok";
pub const KIND_RATE_LIMITED: &str = "rate_limited";
pub const KIND_TIMEOUT: &str = "timeout";
pub const KIND_HTTP_ERROR: &str = "http_error";
pub const KIND_UNPARSEABLE: &str = "unparseable";

/// Status stored for a transport failure that is not a timeout.
/// `tools/probe_live.py` uses the same sentinel.
const TRANSPORT_STATUS: u16 = 599;

/// Cap on a 2xx body. Larger bodies are `unparseable` and are not kept.
const MAX_BODY_BYTES: u64 = 2_000_000;

const MAX_DOTENV_BYTES: u64 = 1_000_000;

/// Failure before a request is classified. Display text never includes the
/// token or a response body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeHttpError {
    /// `name` is the variable name, not the token.
    KeyNotSet {
        name: String,
    },
    /// The name was not an environment-variable name. The rejected text is
    /// omitted so a token passed by mistake is not echoed.
    BadVarName,
    /// The token is empty or cannot be placed in a header. The token is
    /// omitted.
    InvalidKey,
    BaseNotHttps,
    /// Dotenv could not be read. `message` is the I/O error, not the file.
    Dotenv {
        message: String,
    },
    /// Replay or log I/O failed. `message` is the I/O error, not a reply.
    Write {
        message: String,
    },
    RefusingToWrite,
}

impl std::fmt::Display for ProbeHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KeyNotSet { name } => {
                write!(f, "{name} is not set (environment or .env)")
            }
            Self::BadVarName => write!(
                f,
                "key variable name must match ^[A-Z][A-Z0-9_]{{1,63}}$ and contain '_'"
            ),
            Self::InvalidKey => write!(
                f,
                "bearer token is empty or contains a character that cannot go in a header"
            ),
            Self::BaseNotHttps => write!(
                f,
                "--base must be https:// (plain http only for a local mock server)"
            ),
            Self::Dotenv { message } => write!(f, "reading .env: {message}"),
            Self::Write { message } => write!(f, "failed to write probe output: {message}"),
            Self::RefusingToWrite => {
                write!(f, "refusing to write: output would contain the key")
            }
        }
    }
}

impl std::error::Error for ProbeHttpError {}

/// One plan row: a question id and the JSON body to POST.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanItem {
    pub id: String,
    pub body: Value,
}

/// Where to send a plan, and where to write the replay and the redacted log.
///
/// `key_env` is the variable name. The token is loaded inside [`run`].
#[derive(Debug, Clone, Copy)]
pub struct RunOptions<'a> {
    pub base: &'a str,
    pub key_env: &'a str,
    /// Dotenv file used when the variable is unset or empty. The Python
    /// runner reads `./.env`.
    pub dotenv: &'a Path,
    pub plan: &'a [PlanItem],
    pub replay_path: &'a Path,
    pub log_path: &'a Path,
    /// How long to wait for one response, including the body. The
    /// command-line runner defaults to 60 seconds.
    pub timeout: Duration,
    /// Pause between requests after the first. `0.0` skips the pause so a
    /// caller can disable it. A positive value draws uniformly between
    /// `min(1.0, max_delay_secs)` and `max(1.0, max_delay_secs)`, the same
    /// span as Python `random.uniform(1.0, max_delay_secs)`.
    pub max_delay_secs: f64,
}

/// One classified exchange. `kind` matches `tools/probe_live.send`.
///
/// On `ok`, `text` is present (possibly empty). Other kinds leave the reply
/// fields empty and do not keep the response body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendResult {
    pub at_ms: i64,
    pub latency_ms: i64,
    pub kind: &'static str,
    pub status: Option<u16>,
    pub model: Option<String>,
    pub usage_fields: Vec<String>,
    pub text: Option<String>,
    pub tool_name: Option<String>,
    pub tool_args: Option<String>,
}

/// `true` when `base` may be used. `https` is always allowed. `http` is
/// allowed only for host `127.0.0.1`, `localhost` (any letter case), or
/// `::1`, matching `urllib.parse` in the Python runner.
pub fn base_url_allowed(base: &str) -> bool {
    let Ok(url) = url::Url::parse(base) else {
        return false;
    };
    match url.scheme() {
        "https" => true,
        "http" => url.host_str().is_some_and(local_http_host),
        _ => false,
    }
}

/// Loads the bearer token for `var` from the environment, then `./.env`.
///
/// `var` is the variable name. The returned string is the token; do not log
/// it.
pub fn load_key(var: &str) -> Result<String, ProbeHttpError> {
    load_key_from(var, Path::new(".env"))
}

/// Same as [`load_key`], reading `dotenv` instead of `./.env`.
pub fn load_key_from(var: &str, dotenv: &Path) -> Result<String, ProbeHttpError> {
    let from_env = env_value(var);
    let dotenv_text = if from_env.is_some() {
        None
    } else if dotenv.is_file() {
        Some(read_dotenv(dotenv)?)
    } else {
        None
    };
    lookup_key(var, from_env.as_deref(), dotenv_text.as_deref())
}

/// Sends `body` to `{base}/v1/chat/completions` and classifies the outcome.
///
/// Transport failures become a [`SendResult`]. The token and the base are
/// rejected before a socket is opened.
pub fn send(
    base: &str,
    key: &str,
    body: &Value,
    timeout: Duration,
) -> Result<SendResult, ProbeHttpError> {
    if !base_url_allowed(base) {
        return Err(ProbeHttpError::BaseNotHttps);
    }
    if !key_is_sendable(key) {
        return Err(ProbeHttpError::InvalidKey);
    }
    let payload = serde_json::to_vec(body).unwrap_or_default();
    if payload.is_empty() {
        return Ok(classified(KIND_HTTP_ERROR, Some(TRANSPORT_STATUS)));
    }
    let at_ms = now_ms();
    let started = Instant::now();
    let mut result = post_json(&completions_url(base), key, &payload, timeout);
    result.at_ms = at_ms;
    result.latency_ms = elapsed_ms(started);
    Ok(result)
}

/// Loads the token, sends every plan row, and writes the replay and log.
///
/// The base and the token are checked before either file is created. Send
/// order is shuffled. A key-echo refusal leaves any earlier rows in place
/// and does not write the offending row. The returned error does not contain
/// the token.
pub fn run(opts: &RunOptions<'_>) -> Result<(), ProbeHttpError> {
    if !base_url_allowed(opts.base) {
        return Err(ProbeHttpError::BaseNotHttps);
    }
    let key = load_key_from(opts.key_env, opts.dotenv)?;
    if !key_is_sendable(&key) {
        return Err(ProbeHttpError::InvalidKey);
    }

    let mut replay = File::create(opts.replay_path).map_err(write_err)?;
    let mut log = File::create(opts.log_path).map_err(write_err)?;
    writeln!(replay, "{REPLAY_HEADER}").map_err(write_err)?;
    replay.flush().map_err(write_err)?;

    let mut order: Vec<usize> = (0..opts.plan.len()).collect();
    shuffle(&mut order);
    for (n, index) in order.into_iter().enumerate() {
        if n > 0 {
            pause(opts.max_delay_secs);
        }
        let item = &opts.plan[index];
        let result = send(opts.base, &key, &item.body, opts.timeout)?;
        let line = format_replay_line(&item.id, &result);
        let log_line = format_log_line(&item.id, &result)?;
        if output_contains_key(&key, &line, &log_line, &result) {
            replay.flush().map_err(write_err)?;
            log.flush().map_err(write_err)?;
            return Err(ProbeHttpError::RefusingToWrite);
        }
        writeln!(replay, "{line}").map_err(write_err)?;
        writeln!(log, "{log_line}").map_err(write_err)?;
        replay.flush().map_err(write_err)?;
        log.flush().map_err(write_err)?;
    }
    Ok(())
}

/// `host_str` keeps the brackets around an IPv6 address. Python's
/// `urlsplit().hostname` does not, and only the exact names below are local.
fn local_http_host(host: &str) -> bool {
    let host = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);
    host == "127.0.0.1" || host == "::1" || host.eq_ignore_ascii_case("localhost")
}

fn completions_url(base: &str) -> String {
    format!("{}/v1/chat/completions", base.trim_end_matches('/'))
}

fn post_json(url: &str, key: &str, payload: &[u8], timeout: Duration) -> SendResult {
    // Config::default() would copy HTTP(S)_PROXY / ALL_PROXY. Override it so
    // the bearer token cannot be sent to a proxy.
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .max_redirects(0)
        .proxy(None)
        .http_status_as_error(false)
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let response = agent
        .post(url)
        .header("Authorization", format!("Bearer {key}"))
        .content_type("application/json")
        .send(payload);
    match response {
        Ok(mut response) => classify_response(&mut response),
        Err(err) if is_timeout(&err) => classified(KIND_TIMEOUT, None),
        Err(ureq::Error::StatusCode(status)) if status == 429 => {
            classified(KIND_RATE_LIMITED, Some(status))
        }
        Err(ureq::Error::StatusCode(status)) => classified(KIND_HTTP_ERROR, Some(status)),
        Err(_) => classified(KIND_HTTP_ERROR, Some(TRANSPORT_STATUS)),
    }
}

fn classify_response(response: &mut ureq::http::Response<ureq::Body>) -> SendResult {
    let status = response.status().as_u16();
    if status == 429 {
        return classified(KIND_RATE_LIMITED, Some(status));
    }
    if !(200..300).contains(&status) {
        return classified(KIND_HTTP_ERROR, Some(status));
    }
    match response
        .body_mut()
        .with_config()
        .limit(MAX_BODY_BYTES)
        .read_to_vec()
    {
        Ok(bytes) => parse_ok(status, &bytes),
        Err(err) if is_timeout(&err) => classified(KIND_TIMEOUT, None),
        Err(_) => classified(KIND_UNPARSEABLE, Some(status)),
    }
}

fn parse_ok(status: u16, bytes: &[u8]) -> SendResult {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return classified(KIND_UNPARSEABLE, Some(status));
    };
    let Some(message) = value
        .pointer("/choices/0/message")
        .filter(|m| m.is_object())
    else {
        return classified(KIND_UNPARSEABLE, Some(status));
    };
    let text = match message.get("content") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(_) => return classified(KIND_UNPARSEABLE, Some(status)),
    };
    let tool = match message.get("tool_calls") {
        None | Some(Value::Null) => None,
        Some(Value::Array(calls)) if calls.is_empty() => None,
        Some(Value::Array(calls)) => {
            let Some(function) = calls.first().and_then(|call| call.get("function")) else {
                return classified(KIND_UNPARSEABLE, Some(status));
            };
            let Some(name) = function.get("name").and_then(Value::as_str) else {
                return classified(KIND_UNPARSEABLE, Some(status));
            };
            let args = match function.get("arguments") {
                None | Some(Value::Null) => String::new(),
                Some(Value::String(args)) => args.clone(),
                Some(_) => return classified(KIND_UNPARSEABLE, Some(status)),
            };
            Some((name.to_string(), args))
        }
        Some(_) => return classified(KIND_UNPARSEABLE, Some(status)),
    };
    let model = value
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut usage_fields = match value.get("usage") {
        Some(Value::Object(map)) => map.keys().cloned().collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    usage_fields.sort();
    let (tool_name, tool_args) = match tool {
        Some((name, args)) => (Some(name), Some(args)),
        None => (None, None),
    };
    SendResult {
        at_ms: 0,
        latency_ms: 0,
        kind: KIND_OK,
        status: Some(status),
        model,
        usage_fields,
        text: Some(text),
        tool_name,
        tool_args,
    }
}

fn classified(kind: &'static str, status: Option<u16>) -> SendResult {
    SendResult {
        at_ms: 0,
        latency_ms: 0,
        kind,
        status,
        model: None,
        usage_fields: Vec::new(),
        text: None,
        tool_name: None,
        tool_args: None,
    }
}

fn is_timeout(err: &ureq::Error) -> bool {
    match err {
        ureq::Error::Timeout(_) => true,
        ureq::Error::Io(io) => io.kind() == std::io::ErrorKind::TimedOut,
        _ => false,
    }
}

fn format_replay_line(id: &str, result: &SendResult) -> String {
    let status = match result.status {
        Some(status) => status.to_string(),
        None => String::new(),
    };
    [
        id,
        &result.at_ms.to_string(),
        &result.latency_ms.to_string(),
        result.kind,
        &status,
        &tsv_escape(result.model.as_deref().unwrap_or("")),
        &result.usage_fields.join(","),
        &tsv_escape(result.text.as_deref().unwrap_or("")),
        &tsv_escape(result.tool_name.as_deref().unwrap_or("")),
        &tsv_escape(result.tool_args.as_deref().unwrap_or("")),
    ]
    .join("\t")
}

fn format_log_line(id: &str, result: &SendResult) -> Result<String, ProbeHttpError> {
    let record = LogRecord {
        at_ms: result.at_ms,
        id,
        kind: result.kind,
        status: result.status,
        latency_ms: result.latency_ms,
        model: result.model.as_deref(),
        usage_fields: &result.usage_fields,
    };
    serde_json::to_string(&record).map_err(|_| ProbeHttpError::Write {
        message: "could not encode the log line".to_string(),
    })
}

#[derive(Serialize)]
struct LogRecord<'a> {
    at_ms: i64,
    id: &'a str,
    kind: &'a str,
    status: Option<u16>,
    latency_ms: i64,
    model: Option<&'a str>,
    usage_fields: &'a [String],
}

fn output_contains_key(key: &str, line: &str, log_line: &str, result: &SendResult) -> bool {
    let mut parts = vec![line, log_line];
    for field in [
        result.model.as_deref(),
        result.text.as_deref(),
        result.tool_name.as_deref(),
        result.tool_args.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        parts.push(field);
    }
    if parts.iter().any(|part| leaks_key(key, part)) {
        return true;
    }
    result
        .usage_fields
        .iter()
        .any(|field| leaks_key(key, field))
}

fn leaks_key(key: &str, text: &str) -> bool {
    if key.is_empty() || text.is_empty() {
        return false;
    }
    let escaped = tsv_escape(key);
    let json_escaped = json_escape_contents(key);
    for needle in [key, escaped.as_str(), json_escaped.as_str()] {
        if !needle.is_empty() && text.contains(needle) {
            return true;
        }
    }
    false
}

/// Tab-separated escaping from `tools/probe_live.py`.
fn tsv_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// The inside of Python `json.dumps(value)` (`ensure_ascii=True`), without
/// the surrounding quotes. That is the third needle the Python runner uses.
fn json_escape_contents(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c if (c as u32) > 0x7f => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{:04x}", unit));
                }
            }
            c => out.push(c),
        }
    }
    out
}

fn lookup_key(
    var: &str,
    from_env: Option<&str>,
    dotenv_text: Option<&str>,
) -> Result<String, ProbeHttpError> {
    if !acceptable_var_name(var) {
        return Err(ProbeHttpError::BadVarName);
    }
    if let Some(value) = from_env {
        if !value.is_empty() {
            return Ok(value.to_string());
        }
    }
    if let Some(text) = dotenv_text {
        if let Some(value) = dotenv_lookup(text, var) {
            if !value.is_empty() {
                return Ok(value);
            }
        }
    }
    Err(ProbeHttpError::KeyNotSet {
        name: var.to_string(),
    })
}

fn env_value(var: &str) -> Option<String> {
    match std::env::var(var) {
        Ok(value) if !value.is_empty() => Some(value),
        _ => None,
    }
}

fn read_dotenv(path: &Path) -> Result<String, ProbeHttpError> {
    let len = std::fs::metadata(path)
        .map_err(|err| ProbeHttpError::Dotenv {
            message: err.to_string(),
        })?
        .len();
    if len > MAX_DOTENV_BYTES {
        return Err(ProbeHttpError::Dotenv {
            message: "file is too large".to_string(),
        });
    }
    std::fs::read_to_string(path).map_err(|err| ProbeHttpError::Dotenv {
        message: err.to_string(),
    })
}

/// Same shape as `agent_value_core::privacy::KeyEnvVarName`: an uppercase
/// name that contains `_`. A token passed in place of a name does not match,
/// and [`ProbeHttpError::BadVarName`] does not echo it.
fn acceptable_var_name(name: &str) -> bool {
    if name.len() < 2 || name.len() > 64 || !name.contains('_') {
        return false;
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_uppercase()
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn dotenv_lookup(text: &str, var: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key == var && !value.is_empty() {
            return Some(unquote(value));
        }
    }
    None
}

fn unquote(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string()
}

/// Visible ASCII without spaces. Anything else is refused before it can be
/// placed in an `Authorization` header or an error string.
fn key_is_sendable(key: &str) -> bool {
    !key.is_empty() && key.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
}

fn write_err(err: std::io::Error) -> ProbeHttpError {
    ProbeHttpError::Write {
        message: err.to_string(),
    }
}

fn now_ms() -> i64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    i64::try_from(millis).unwrap_or(i64::MAX)
}

fn elapsed_ms(start: Instant) -> i64 {
    i64::try_from(start.elapsed().as_millis()).unwrap_or(i64::MAX)
}

fn next_u64() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static STATE: AtomicU64 = AtomicU64::new(0xA076_1D64_78BD_642F);
    let mut x = STATE.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn shuffle<T>(items: &mut [T]) {
    if items.len() < 2 {
        return;
    }
    for index in (1..items.len()).rev() {
        let swap_with = (next_u64() as usize) % (index + 1);
        items.swap(index, swap_with);
    }
}

fn pause(max_delay_secs: f64) {
    if !(max_delay_secs.is_finite() && max_delay_secs > 0.0) {
        return;
    }
    let low = 1.0_f64.min(max_delay_secs);
    let high = 1.0_f64.max(max_delay_secs);
    let unit = (next_u64() as f64) / (u64::MAX as f64);
    let seconds = low + (high - low) * unit;
    if seconds > 0.0 {
        std::thread::sleep(Duration::from_secs_f64(seconds));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::OnceLock;
    use std::thread;

    use agent_value_core::probe::{parse_replay, Exchange, Reply};
    use serde_json::json;

    use super::*;

    /// Not a real credential. Same stand-in as `tools/test_probe_live.py`.
    const KEY: &str = "mock-test-key-0f1e2d3c4b5a";
    const KEY_ENV: &str = "PROBE_HTTP_TEST_KEY";
    const BODY_MARKER: &str = "BODY-NOT-FOR-REPLAY";

    struct Tmp {
        path: std::path::PathBuf,
    }

    impl Tmp {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static N: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "probe-http-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn require_unset(var: &str) {
        if std::env::var_os(var).is_some() {
            panic!("{var} is set in the environment; refusing to run this test");
        }
    }

    struct EnvSet {
        key: &'static str,
        prev: Option<std::ffi::OsString>,
    }

    impl EnvSet {
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, prev }
        }
    }

    impl Drop for EnvSet {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    fn mock_base() -> &'static str {
        static BASE: OnceLock<String> = OnceLock::new();
        BASE.get_or_init(|| {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            thread::spawn(move || {
                for conn in listener.incoming() {
                    let Ok(stream) = conn else { break };
                    thread::spawn(move || handle_client(stream));
                }
            });
            format!("http://127.0.0.1:{port}")
        })
    }

    fn handle_client(mut stream: TcpStream) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        let Some(request) = read_http(&mut stream) else {
            return;
        };
        let ok_path = request.ok_path;
        let headers = &request.headers;
        let body = &request.body;
        if !ok_path {
            let _ = write_http(&mut stream, 404, None, b"{}");
            return;
        }
        let value: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        let case = value
            .pointer("/messages")
            .and_then(Value::as_array)
            .and_then(|messages| messages.last())
            .and_then(|message| message.get("content"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let auth = headers
            .iter()
            .find(|(name, _)| name == "authorization")
            .map(|(_, value)| value.as_str())
            .unwrap_or("");
        let (status, raw, delay_ms, location) = mock_reply(case, auth);
        if delay_ms > 0 {
            thread::sleep(Duration::from_millis(delay_ms));
        }
        let _ = write_http(&mut stream, status, location, &raw);
    }

    fn mock_reply(case: &str, auth: &str) -> (u16, Vec<u8>, u64, Option<&'static str>) {
        match case {
            "ok" => (
                200,
                ok_body(
                    Some("42"),
                    "mock-model-1",
                    None,
                    Some(&[("prompt_tokens", 3), ("completion_tokens", 1)]),
                ),
                0,
                None,
            ),
            "tool" => (
                200,
                ok_body(
                    None,
                    "mock-model-1",
                    Some(("record_number", r#"{"n": 7}"#)),
                    None,
                ),
                0,
                None,
            ),
            "429" => (429, marked("rate limited"), 0, None),
            "timeout" => (200, ok_body(Some("late"), "mock-model-1", None, None), 2_000, None),
            "503" => (503, marked("unavailable"), 0, None),
            "502" => (502, BODY_MARKER.as_bytes().to_vec(), 0, None),
            "400" => (400, marked("bad request"), 0, None),
            "401" => (401, BODY_MARKER.as_bytes().to_vec(), 0, None),
            "not-json" => (
                200,
                format!("<html>{BODY_MARKER}</html>").into_bytes(),
                0,
                None,
            ),
            "no-choices" => (
                200,
                format!(r#"{{"model":"mock-model-1","marker":"{BODY_MARKER}"}}"#).into_bytes(),
                0,
                None,
            ),
            "content-not-text" => (
                200,
                format!(
                    r#"{{"choices":[{{"message":{{"content":{{"x":1,"marker":"{BODY_MARKER}"}}}}}}]}}"#
                )
                .into_bytes(),
                0,
                None,
            ),
            "redirect" => (302, Vec::new(), 0, Some("http://192.0.2.1/v1/chat/completions")),
            "echo-key" => {
                let token = auth.split_once(' ').map(|(_, token)| token).unwrap_or("");
                (200, ok_body(Some(&format!("echo {token}")), "mock-model-1", None, None), 0, None)
            }
            "auth-check" => {
                if auth == format!("Bearer {KEY}") {
                    (200, ok_body(Some("auth ok"), "mock-model-1", None, None), 0, None)
                } else {
                    (401, BODY_MARKER.as_bytes().to_vec(), 0, None)
                }
            }
            "tab-text" => (
                200,
                ok_body(Some("a\tb\nc\\d"), "mock-model-1", None, None),
                0,
                None,
            ),
            _ => (500, marked("unknown case"), 0, None),
        }
    }

    fn marked(reason: &str) -> Vec<u8> {
        format!(r#"{{"error":"{reason}","marker":"{BODY_MARKER}"}}"#).into_bytes()
    }

    fn ok_body(
        content: Option<&str>,
        model: &str,
        tool: Option<(&str, &str)>,
        usage: Option<&[(&str, u64)]>,
    ) -> Vec<u8> {
        let mut message = json!({
            "role": "assistant",
            "content": content,
        });
        if let Some((name, arguments)) = tool {
            message["tool_calls"] = json!([
                {"type": "function", "function": {"name": name, "arguments": arguments}}
            ]);
        }
        let usage_value = if let Some(pairs) = usage {
            let mut map = serde_json::Map::new();
            for (name, count) in pairs {
                map.insert((*name).to_string(), json!(count));
            }
            Value::Object(map)
        } else {
            json!({"prompt_tokens": 1})
        };
        serde_json::to_vec(&json!({
            "model": model,
            "choices": [{"message": message}],
            "usage": usage_value,
        }))
        .unwrap()
    }

    struct RawRequest {
        ok_path: bool,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
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
        let mut lines = head.split("\r\n");
        let request_line = lines.next().unwrap_or("");
        let ok_path = request_line.split_whitespace().nth(1).is_some_and(|path| {
            path.split('?')
                .next()
                .is_some_and(|path| path.ends_with("/v1/chat/completions"))
        });
        let mut headers = Vec::new();
        let mut content_length = 0usize;
        for line in lines {
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_string();
            if name == "content-length" {
                content_length = value.parse().unwrap_or(0);
            }
            headers.push((name, value));
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
            ok_path,
            headers,
            body: rest,
        })
    }

    fn write_http(
        stream: &mut TcpStream,
        status: u16,
        location: Option<&str>,
        body: &[u8],
    ) -> std::io::Result<()> {
        let reason = match status {
            200 => "OK",
            302 => "Found",
            400 => "Bad Request",
            401 => "Unauthorized",
            404 => "Not Found",
            429 => "Too Many Requests",
            502 => "Bad Gateway",
            503 => "Service Unavailable",
            _ => "Error",
        };
        let mut head = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
            body.len()
        );
        if let Some(location) = location {
            head.push_str("Location: ");
            head.push_str(location);
            head.push_str("\r\n");
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes())?;
        stream.write_all(body)?;
        Ok(())
    }

    fn body(case: &str) -> Value {
        json!({
            "model": "mock-model-1",
            "messages": [{"role": "user", "content": case}]
        })
    }

    fn send_case(case: &str) -> SendResult {
        send(mock_base(), KEY, &body(case), Duration::from_secs(5)).unwrap()
    }

    fn assert_no_secret(rendered: &str) {
        assert!(!rendered.contains(KEY), "output contains the test token");
        assert!(
            !rendered.contains(BODY_MARKER),
            "output contains a response body"
        );
        assert!(!rendered.contains("Authorization"));
    }

    #[test]
    fn base_url_matches_the_python_host_rule() {
        for base in [
            "http://127.0.0.1:9/x",
            "http://127.0.0.1",
            "http://localhost/x",
            "http://LocalHost/x",
            "HTTP://127.0.0.1/",
            "http://[::1]:9/",
            // The URL parser folds this spelling to ::1. It is still loopback.
            "http://[0:0:0:0:0:0:0:1]/",
            "http://user:pw@127.0.0.1/",
            "https://relay.example/v1",
            "https://127.0.0.1/v1",
        ] {
            assert!(base_url_allowed(base), "{base}");
        }
        for base in [
            "http://127.0.0.2/",
            "http://localhost./",
            "http://relay.example",
            "http://192.0.2.1/",
            "ftp://127.0.0.1/",
            "http://",
            "",
            "127.0.0.1",
            "https://",
        ] {
            assert!(!base_url_allowed(base), "{base}");
        }
    }

    #[test]
    fn needles_match_python_json_and_tsv_escaping() {
        let samples = [
            "plain",
            "a\"b",
            "a\\b",
            "a\nb",
            "a\tb",
            "é",
            "😀",
            "a'b",
            "line\r\n",
            "tab\tquote\"\\end",
        ];
        let payload = serde_json::to_string(&samples).unwrap();
        let script = r#"
import json, sys
def esc(s):
    return s.replace("\\", "\\\\").replace("\t", "\\t").replace("\n", "\\n").replace("\r", "\\r")
items = json.load(sys.stdin)
json.dump({
    "json": [json.dumps(s)[1:-1] for s in items],
    "tsv": [esc(s) for s in items],
}, sys.stdout)
"#;
        let mut child = std::process::Command::new("python3")
            .arg("-c")
            .arg(script)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .env("PYTHONIOENCODING", "utf-8")
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        let done = child.wait_with_output().unwrap();
        assert!(
            done.status.success(),
            "{}",
            String::from_utf8_lossy(&done.stderr)
        );
        let parsed: Value = serde_json::from_slice(&done.stdout).unwrap();
        let json_needles = parsed["json"].as_array().unwrap();
        let tsv_needles = parsed["tsv"].as_array().unwrap();
        for (index, sample) in samples.iter().enumerate() {
            assert_eq!(
                json_escape_contents(sample),
                json_needles[index].as_str().unwrap()
            );
            assert_eq!(tsv_escape(sample), tsv_needles[index].as_str().unwrap());
        }
        let key = "a\"b";
        assert!(leaks_key(key, "a\"b"));
        assert!(leaks_key(key, r#"a\"b"#));
        assert!(!leaks_key(key, "ab"));
    }

    #[test]
    fn key_lookup_prefers_env_then_dotenv_quotes() {
        assert_eq!(
            lookup_key(
                "RELAY_API_KEY",
                Some("from-env"),
                Some("RELAY_API_KEY=from-file\n")
            )
            .unwrap(),
            "from-env"
        );
        assert_eq!(
            lookup_key(
                "RELAY_API_KEY",
                Some(""),
                Some("RELAY_API_KEY=\"from-file\"\n")
            )
            .unwrap(),
            "from-file"
        );
        assert_eq!(
            lookup_key(
                "RELAY_API_KEY",
                None,
                Some("  RELAY_API_KEY='from-file'  \n")
            )
            .unwrap(),
            "from-file"
        );
        assert!(matches!(
            lookup_key(
                "RELAY_API_KEY",
                None,
                Some("RELAY_API_KEY = no\n# RELAY_API_KEY=no\n")
            ),
            Err(ProbeHttpError::KeyNotSet { .. })
        ));
        let bad = lookup_key(KEY, None, None).unwrap_err();
        let rendered = format!("{bad} {bad:?}");
        assert!(!rendered.contains(KEY));
        assert!(matches!(bad, ProbeHttpError::BadVarName));
    }

    #[test]
    fn environment_is_used_without_opening_dotenv() {
        let tmp = Tmp::new();
        let missing = tmp.path().join("missing.env");
        let got = load_key_from("CARGO_MANIFEST_DIR", &missing).unwrap();
        assert!(Path::new(&got).is_dir());
        assert_ne!(got, "from-dotenv-file");
    }

    #[test]
    fn dotenv_file_is_used_when_the_variable_is_unset() {
        require_unset(KEY_ENV);
        let tmp = Tmp::new();
        let dotenv = tmp.path().join(".env");
        std::fs::write(
            &dotenv,
            format!("# comment\nOTHER_KEY=nope\n{KEY_ENV}=\"{KEY}\"\n"),
        )
        .unwrap();
        assert_eq!(load_key_from(KEY_ENV, &dotenv).unwrap(), KEY);
    }

    #[test]
    fn send_kind_table_and_ok_fields() {
        let cases = [
            ("429", KIND_RATE_LIMITED, Some(429u16)),
            ("503", KIND_HTTP_ERROR, Some(503)),
            ("502", KIND_HTTP_ERROR, Some(502)),
            ("400", KIND_HTTP_ERROR, Some(400)),
            ("401", KIND_HTTP_ERROR, Some(401)),
            ("not-json", KIND_UNPARSEABLE, Some(200)),
            ("no-choices", KIND_UNPARSEABLE, Some(200)),
            ("content-not-text", KIND_UNPARSEABLE, Some(200)),
            ("redirect", KIND_HTTP_ERROR, Some(302)),
        ];
        for (case, kind, status) in cases {
            let result = send_case(case);
            assert_eq!((result.kind, result.status), (kind, status), "{case}");
            assert!(result.text.is_none(), "{case}");
            assert_no_secret(&format!("{result:?}"));
            let line = format_replay_line("det-01", &result);
            assert_no_secret(&line);
            let parsed = parse_replay(&format!("{REPLAY_HEADER}\n{line}\n")).unwrap();
            assert_eq!(parsed.len(), 1, "{case}");
        }

        let ok = send_case("ok");
        assert_eq!(ok.kind, KIND_OK);
        assert_eq!(
            (ok.status, ok.text.as_deref(), ok.model.as_deref()),
            (Some(200), Some("42"), Some("mock-model-1"))
        );
        assert_eq!(ok.usage_fields, ["completion_tokens", "prompt_tokens"]);
        assert!(ok.tool_name.is_none());
        assert_no_secret(&format!("{ok:?}"));

        let tool = send_case("tool");
        assert_eq!(
            (
                tool.kind,
                tool.text.as_deref(),
                tool.tool_name.as_deref(),
                tool.tool_args.as_deref()
            ),
            (
                KIND_OK,
                Some(""),
                Some("record_number"),
                Some(r#"{"n": 7}"#)
            )
        );

        let auth = send_case("auth-check");
        assert_eq!(
            (auth.kind, auth.text.as_deref()),
            (KIND_OK, Some("auth ok"))
        );
        let wrong = send(
            mock_base(),
            "mock-test-key-ffffffffffffffff",
            &body("auth-check"),
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!((wrong.kind, wrong.status), (KIND_HTTP_ERROR, Some(401)));
        assert_no_secret(&format!("{wrong:?}"));
        assert!(!format!("{wrong:?}").contains("ffffffffffffffff"));
    }

    #[test]
    fn timeout_and_connection_refused_match_python_kinds() {
        let timed = send(
            mock_base(),
            KEY,
            &body("timeout"),
            Duration::from_millis(400),
        )
        .unwrap();
        assert_eq!((timed.kind, timed.status), (KIND_TIMEOUT, None));
        assert!(timed.latency_ms >= 200, "{}", timed.latency_ms);
        assert_no_secret(&format!("{timed:?}"));

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let refused = send(
            &format!("http://127.0.0.1:{port}"),
            KEY,
            &body("ok"),
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(
            (refused.kind, refused.status),
            (KIND_HTTP_ERROR, Some(TRANSPORT_STATUS))
        );
        assert_no_secret(&format!("{refused:?}"));
    }

    #[test]
    fn disallowed_base_and_unsafe_token_do_not_become_replies() {
        let err = send(
            "http://192.0.2.1/v1",
            KEY,
            &body("ok"),
            Duration::from_millis(200),
        )
        .unwrap_err();
        assert_eq!(err, ProbeHttpError::BaseNotHttps);
        assert!(!format!("{err}").contains(KEY));

        let err = send(
            mock_base(),
            "mock\nkey",
            &body("ok"),
            Duration::from_millis(200),
        )
        .unwrap_err();
        assert_eq!(err, ProbeHttpError::InvalidKey);
        assert!(!format!("{err} {err:?}").contains("mock"));
    }

    #[test]
    fn send_ignores_proxy_environment() {
        let _guards = [
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
        ]
        .map(|name| EnvSet::set(name, "http://192.0.2.1:9"));
        let result = send_case("ok");
        assert_eq!(result.kind, KIND_OK);
        assert_eq!(result.text.as_deref(), Some("42"));
    }

    fn run_plan(plan: &[(&str, &str)], dotenv_value: &str) -> (Tmp, Result<(), ProbeHttpError>) {
        require_unset(KEY_ENV);
        let tmp = Tmp::new();
        let dotenv = tmp.path().join(".env");
        std::fs::write(&dotenv, format!("{KEY_ENV}={dotenv_value}\n")).unwrap();
        let items: Vec<PlanItem> = plan
            .iter()
            .map(|(id, case)| PlanItem {
                id: (*id).to_string(),
                body: body(case),
            })
            .collect();
        let replay = tmp.path().join("replay.tsv");
        let log = tmp.path().join("run.jsonl");
        let result = run(&RunOptions {
            base: mock_base(),
            key_env: KEY_ENV,
            dotenv: &dotenv,
            plan: &items,
            replay_path: &replay,
            log_path: &log,
            timeout: Duration::from_secs(5),
            max_delay_secs: 0.0,
        });
        (tmp, result)
    }

    fn read_outputs(tmp: &Tmp) -> (String, String) {
        let replay = std::fs::read_to_string(tmp.path().join("replay.tsv")).unwrap_or_default();
        let log = std::fs::read_to_string(tmp.path().join("run.jsonl")).unwrap_or_default();
        (replay, log)
    }

    #[test]
    fn run_writes_an_ok_replay_the_core_can_parse() {
        let (tmp, result) = run_plan(
            &[("det-01", "ok"), ("cap-tool", "tool")],
            &format!("\"{KEY}\""),
        );
        assert!(result.is_ok(), "{result:?}");
        let (replay, log) = read_outputs(&tmp);
        assert_no_secret(&replay);
        assert_no_secret(&log);
        assert_eq!(REPLAY_HEADER, agent_value_core::probe::REPLAY_HEADER);
        let records = parse_replay(&replay).unwrap();
        assert_eq!(records.len(), 2);
        let by_id: BTreeSet<_> = records
            .iter()
            .map(|record| record.question_id.as_str())
            .collect();
        assert_eq!(by_id, BTreeSet::from(["det-01", "cap-tool"]));
        for record in &records {
            match record.question_id.as_str() {
                "det-01" => assert_eq!(
                    record.exchange,
                    Exchange::Answered(Reply {
                        text: "42".to_string(),
                        returned_model: Some("mock-model-1".to_string()),
                        usage_fields: vec![
                            "completion_tokens".to_string(),
                            "prompt_tokens".to_string()
                        ],
                        tool_call: None,
                    })
                ),
                "cap-tool" => assert_eq!(
                    record.exchange,
                    Exchange::Answered(Reply {
                        text: String::new(),
                        returned_model: Some("mock-model-1".to_string()),
                        usage_fields: vec!["prompt_tokens".to_string()],
                        tool_call: Some(("record_number".to_string(), r#"{"n": 7}"#.to_string())),
                    })
                ),
                other => panic!("unexpected id {other}"),
            }
        }
        for line in log.lines() {
            let value: Value = serde_json::from_str(line).unwrap();
            let keys: BTreeSet<_> = value.as_object().unwrap().keys().cloned().collect();
            assert_eq!(
                keys,
                BTreeSet::from([
                    "at_ms".to_string(),
                    "id".to_string(),
                    "kind".to_string(),
                    "status".to_string(),
                    "latency_ms".to_string(),
                    "model".to_string(),
                    "usage_fields".to_string(),
                ])
            );
            assert_eq!(value["kind"], "ok");
            assert_no_secret(line);
        }
    }

    #[test]
    fn run_records_rate_limit_without_the_error_body() {
        let (tmp, result) = run_plan(&[("det-01", "429")], KEY);
        assert!(result.is_ok(), "{result:?}");
        let (replay, log) = read_outputs(&tmp);
        assert_no_secret(&format!("{replay}\n{log}"));
        let records = parse_replay(&replay).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].exchange, Exchange::RateLimited);
        let value: Value = serde_json::from_str(log.lines().next().unwrap()).unwrap();
        assert_eq!(value["kind"], "rate_limited");
        assert_eq!(value["status"], 429);
        assert!(value["model"].is_null());
        assert_eq!(value["usage_fields"], json!([]));
    }

    #[test]
    fn run_refuses_to_write_when_the_reply_echoes_the_key() {
        let (tmp, result) = run_plan(&[("det-01", "echo-key")], KEY);
        let err = result.unwrap_err();
        let rendered = format!("{err} {err:?}");
        assert!(rendered.contains("refusing to write"));
        assert!(!rendered.contains(KEY));
        let (replay, log) = read_outputs(&tmp);
        assert_eq!(replay.lines().count(), 1);
        assert!(replay.starts_with(REPLAY_HEADER));
        assert_eq!(log, "");
        assert_no_secret(&format!("{replay}\n{log}\n{rendered}"));
    }

    #[test]
    fn plain_http_elsewhere_does_not_create_output_files() {
        require_unset(KEY_ENV);
        let tmp = Tmp::new();
        let dotenv = tmp.path().join(".env");
        std::fs::write(&dotenv, format!("{KEY_ENV}={KEY}\n")).unwrap();
        let replay = tmp.path().join("replay.tsv");
        let log = tmp.path().join("run.jsonl");
        let started = Instant::now();
        let err = run(&RunOptions {
            base: "http://192.0.2.1",
            key_env: KEY_ENV,
            dotenv: &dotenv,
            plan: &[PlanItem {
                id: "det-01".to_string(),
                body: body("ok"),
            }],
            replay_path: &replay,
            log_path: &log,
            timeout: Duration::from_millis(200),
            max_delay_secs: 0.0,
        })
        .unwrap_err();
        assert_eq!(err, ProbeHttpError::BaseNotHttps);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!replay.exists());
        assert!(!log.exists());
        assert!(!format!("{err}").contains(KEY));
    }

    #[test]
    fn escaped_reply_text_roundtrips_through_the_core() {
        let result = send_case("tab-text");
        let line = format_replay_line("det-01", &result);
        let records = parse_replay(&format!("{REPLAY_HEADER}\n{line}\n")).unwrap();
        match &records[0].exchange {
            Exchange::Answered(reply) => assert_eq!(reply.text, "a\tb\nc\\d"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn quoted_dotenv_token_is_sent_without_the_quotes() {
        let (tmp, result) = run_plan(&[("det-01", "auth-check")], &format!("\"{KEY}\""));
        assert!(result.is_ok(), "{result:?}");
        let (replay, log) = read_outputs(&tmp);
        assert_no_secret(&format!("{replay}\n{log}"));
        let records = parse_replay(&replay).unwrap();
        match &records[0].exchange {
            Exchange::Answered(reply) => assert_eq!(reply.text, "auth ok"),
            other => panic!("quotes were not stripped: {other:?}"),
        }
    }
}

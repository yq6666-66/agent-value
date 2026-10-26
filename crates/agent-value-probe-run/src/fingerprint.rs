//! Official-direct fingerprint baseline collection.
//!
//! [`collect_fingerprint_baseline`] sends the fingerprint prompt set and
//! upserts one row into the local fingerprint baseline store. It does not
//! call [`crate::run_once`], does not insert a daily probe result, does not
//! record a probe charge, and does not change observation thresholds.
//! There is no desktop command and no command-line entry.
//!
//! The bearer token is loaded by environment-variable name inside
//! `agent-value-probe-http`. The name is what the caller passes. The token
//! is not stored and is not copied into errors.
//!
//! This blocks the calling thread for every sample. Do not call it on the
//! Tauri async runtime.

use std::collections::BTreeMap;
use std::time::Duration;

use agent_value_core::arm::{Arm, Protocol};
use agent_value_core::authenticity::{probe_budget, BudgetDecision};
use agent_value_core::privacy::has_secret_prefix;
use agent_value_core::probe::{
    FINGERPRINT_PROMPTS, FINGERPRINT_SAMPLES, PROBE_MAX_TOKENS, PROBE_SYSTEM_PROMPT,
};
use agent_value_core::records::MicroYuan;
use agent_value_core::CoreError;
use agent_value_probe_http::{
    base_url_allowed, load_key, send, ProbeHttpError, SendResult, KIND_OK,
};
use agent_value_store::{
    sha256_hex, AnswerCount, Clock, FingerprintBaselineIdentity, FingerprintBaselineUpsert,
    FingerprintBaselineWrite, FingerprintDistribution, QuestionAnswers, Store, StoreError,
    DISTRIBUTION_SCHEMA_VERSION,
};

/// Task-set label stored on a collected baseline.
///
/// Human-readable form of probe set `"1"`. Bump this when the fingerprint
/// prompt set changes.
pub const FINGERPRINT_TASK_SET_VERSION: &str = "probe-set-1";

/// Canonical sampling JSON stored on a collected baseline.
///
/// These are the sampling fields sent on every sample: temperature `1`,
/// top_p `1`, and [`PROBE_MAX_TOKENS`]. The store hashes this string; it is
/// not reduced to a summary. Fingerprint samples use temperature 1, the
/// route default this collect pins, rather than the deterministic probe
/// temperature of 0.
pub const FINGERPRINT_SAMPLING_CONFIG: &str = "{\"temperature\":1,\"top_p\":1,\"max_tokens\":64}";

/// Provisional price of one fingerprint sample, in micro-yuan.
///
/// 0.02 yuan. Not a measured vendor price and not an observation threshold.
/// A full collect is 400 samples, 8 yuan, which is under the 10 yuan monthly
/// cap. This is not [`crate::PROBE_ESTIMATE_PER_ITEM_MICROS`]: that constant
/// prices one `run_once` plan row at 0.05 yuan, and using it per sample
/// would price a full collect at 20 yuan.
pub const FINGERPRINT_ESTIMATE_PER_REQUEST_MICROS: i64 = 20_000;

const OFFICIAL_DIRECT: &str = "official-direct";
const MAX_ANSWER_CHARS: usize = 200;

/// Spent total and cap for one [`probe_budget`] check.
///
/// `spent` is advisory input from the caller. This collect does not read or
/// write H-track probe charges, so a caller who always passes `spent` of 0
/// is not stopped by earlier collects. `run_once` still accounts its own
/// charges separately. The product cap is
/// [`agent_value_core::constants::PROBE_MONTHLY_BUDGET_YUAN`]. This function
/// does not change that cap. With spent 0 and that cap, a full collect is
/// allowed: [`fingerprint_collect_estimate`] is strictly under the cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FingerprintBudgetSnapshot {
    pub spent: MicroYuan,
    pub budget: MicroYuan,
}

/// Inputs for one collect. `key_env` is the variable name, never the token.
/// `base_url` must be allowed by `base_url_allowed` and its host must be the
/// arm's route host.
#[derive(Debug)]
pub struct FingerprintCollectRequest<'a> {
    pub arm: &'a Arm,
    pub base_url: &'a str,
    pub key_env: &'a str,
    pub consent: bool,
    pub budget: FingerprintBudgetSnapshot,
    /// How long to wait for one sample, including the body.
    pub timeout: Duration,
}

/// A baseline that was stored. The token and the reply text are not here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FingerprintCollectOutcome {
    pub upsert: FingerprintBaselineUpsert,
    pub identity: FingerprintBaselineIdentity,
    /// Clock reading taken immediately before the upsert, not before the
    /// first sample was sent.
    pub collected_at_ms: i64,
    /// How many [`agent_value_probe_http::send`] calls returned `Ok`.
    /// This outcome exists only after every sample was accepted, so the
    /// count is the full collect. A rejected sample does not produce an
    /// outcome, and calls that returned `Err` are not included.
    pub requests_sent: u32,
}

/// A collect that stored nothing. Display text does not include a token,
/// a base URL, or reply text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FingerprintCollectError {
    ConsentDenied,
    BudgetStop,
    HostRejected,
    NotOfficialDirect,
    UnsupportedProtocol,
    /// `name` is the variable name, not the token.
    KeyNotSet {
        name: String,
    },
    BadVarName,
    InvalidKey,
    /// `kind` is a probe HTTP class such as `timeout` or `http_error`.
    SampleFailed {
        kind: &'static str,
    },
    AnswerRejected,
    ClockOutOfRange,
    DistributionRejected,
    NotStored,
}

impl std::fmt::Display for FingerprintCollectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConsentDenied => write!(f, "consent was not given"),
            Self::BudgetStop => write!(f, "monthly probe budget stops this collect"),
            Self::HostRejected => write!(f, "base url host is not allowed for this arm"),
            Self::NotOfficialDirect => write!(f, "arm is not official-direct"),
            Self::UnsupportedProtocol => {
                write!(f, "protocol is not OpenAI Chat Completions")
            }
            Self::KeyNotSet { name } => write!(f, "{name} is not set"),
            Self::BadVarName => write!(f, "key variable name is not usable"),
            Self::InvalidKey => write!(f, "bearer token is not usable"),
            Self::SampleFailed { kind } => write!(f, "fingerprint sample failed ({kind})"),
            Self::AnswerRejected => write!(f, "fingerprint answer was rejected"),
            Self::ClockOutOfRange => write!(f, "clock is out of range"),
            Self::DistributionRejected => write!(f, "fingerprint distribution was rejected"),
            Self::NotStored => write!(f, "fingerprint baseline was not stored"),
        }
    }
}

impl std::error::Error for FingerprintCollectError {}

/// HTTP requests in one collect: 10 prompts, 20 samples, two runs.
pub fn fingerprint_request_count() -> usize {
    FINGERPRINT_PROMPTS
        .len()
        .saturating_mul(FINGERPRINT_SAMPLES as usize)
        .saturating_mul(2)
}

/// Provisional charge of one full collect: request count times
/// [`FINGERPRINT_ESTIMATE_PER_REQUEST_MICROS`]. Not a measured price and
/// not an observation threshold. Strictly under the monthly cap.
pub fn fingerprint_collect_estimate() -> Result<MicroYuan, FingerprintCollectError> {
    let rows = i64::try_from(fingerprint_request_count())
        .map_err(|_| FingerprintCollectError::NotStored)?;
    let micros = rows
        .checked_mul(FINGERPRINT_ESTIMATE_PER_REQUEST_MICROS)
        .ok_or(FingerprintCollectError::NotStored)?;
    MicroYuan::new(micros).map_err(|_| FingerprintCollectError::NotStored)
}

/// Collects one official-direct baseline, or rejects it without writing.
///
/// The stored identity is the arm route host (`provider`; this function does
/// not hardcode a vendor list), the arm model id, an empty model revision,
/// the arm protocol, `connection_kind` `official-direct`,
/// [`FINGERPRINT_TASK_SET_VERSION`], the SHA-256 hex of the system prompt
/// that was sent, [`FINGERPRINT_SAMPLING_CONFIG`], and the store's
/// [`DISTRIBUTION_SCHEMA_VERSION`].
///
/// Preconditions are checked before a socket is opened: consent, an
/// official-direct arm, OpenAI Chat Completions, an allowed base whose host
/// is the arm host, a non-negative clock, the monthly budget gate, then the
/// key. Any failed, timed-out, or unparseable sample, or a reply that
/// contains the token, discards the batch. The store is written only after
/// every sample has been accepted, in one upsert. A later successful collect
/// of the same identity overwrites that row when its timestamp is not older.
pub fn collect_fingerprint_baseline(
    store: &Store,
    clock: &dyn Clock,
    request: &FingerprintCollectRequest<'_>,
) -> Result<FingerprintCollectOutcome, FingerprintCollectError> {
    if !request.consent {
        return Err(FingerprintCollectError::ConsentDenied);
    }
    if !request.arm.route().is_official_direct() {
        return Err(FingerprintCollectError::NotOfficialDirect);
    }
    if request.arm.route().protocol() != &Protocol::OpenAiChatCompletions {
        return Err(FingerprintCollectError::UnsupportedProtocol);
    }
    if host_rejected(request.base_url, request.arm.route().host()) {
        return Err(FingerprintCollectError::HostRejected);
    }
    if clock.now_ms() < 0 {
        return Err(FingerprintCollectError::ClockOutOfRange);
    }
    let estimate = fingerprint_collect_estimate()?;
    match probe_budget(request.budget.spent, estimate, request.budget.budget) {
        Ok(BudgetDecision::Allow) => {}
        Ok(BudgetDecision::Stop) => return Err(FingerprintCollectError::BudgetStop),
        Err(_) => return Err(FingerprintCollectError::NotStored),
    }
    let (distribution, requests_sent) = collect_distribution(request)?;
    // Stamp the row at upsert time. The earlier read only refuses a clock
    // that is already negative, so the 400 samples are not sent in that case.
    let collected_at_ms = clock.now_ms();
    if collected_at_ms < 0 {
        return Err(FingerprintCollectError::ClockOutOfRange);
    }
    let identity = identity_for(request.arm)?;
    let write = FingerprintBaselineWrite {
        identity: identity.clone(),
        collected_at_ms,
        distribution,
    };
    let upsert = commit(store, &write)?;
    Ok(FingerprintCollectOutcome {
        upsert,
        identity,
        collected_at_ms,
        requests_sent,
    })
}

fn identity_for(arm: &Arm) -> Result<FingerprintBaselineIdentity, FingerprintCollectError> {
    if !arm.route().is_official_direct() {
        return Err(FingerprintCollectError::NotOfficialDirect);
    }
    Ok(FingerprintBaselineIdentity {
        provider: arm.route().host().to_string(),
        model_id: arm.model().as_str().to_string(),
        model_revision: String::new(),
        protocol: arm.route().protocol().as_str().to_string(),
        connection_kind: OFFICIAL_DIRECT.to_string(),
        task_set_version: FINGERPRINT_TASK_SET_VERSION.to_string(),
        system_prompt_hash: system_prompt_hash(PROBE_SYSTEM_PROMPT),
        sampling_config: FINGERPRINT_SAMPLING_CONFIG.to_string(),
        distribution_schema_version: DISTRIBUTION_SCHEMA_VERSION,
    })
}

/// SHA-256 hex of the system prompt string that is sent. An empty prompt
/// hashes the empty string.
fn system_prompt_hash(prompt: &str) -> String {
    sha256_hex(prompt.as_bytes())
}

fn collect_distribution(
    request: &FingerprintCollectRequest<'_>,
) -> Result<(FingerprintDistribution, u32), FingerprintCollectError> {
    let key = load_key(request.key_env).map_err(map_http)?;
    let mut sent = 0u32;
    let mut run1 = Vec::with_capacity(FINGERPRINT_PROMPTS.len());
    let mut run2 = Vec::with_capacity(FINGERPRINT_PROMPTS.len());
    for questions in [&mut run1, &mut run2] {
        for prompt in FINGERPRINT_PROMPTS {
            let mut counts = BTreeMap::new();
            for _ in 0..FINGERPRINT_SAMPLES {
                let body = chat_body(request.arm.model().as_str(), prompt);
                let result =
                    send(request.base_url, &key, &body, request.timeout).map_err(map_http)?;
                sent = sent
                    .checked_add(1)
                    .ok_or(FingerprintCollectError::NotStored)?;
                if echoes_key(&key, &result) {
                    return Err(FingerprintCollectError::AnswerRejected);
                }
                if result.kind != KIND_OK {
                    return Err(FingerprintCollectError::SampleFailed { kind: result.kind });
                }
                let text = result.text.as_deref().unwrap_or("");
                let answer = normalize_answer(text)?;
                let entry = counts.entry(answer).or_insert(0u32);
                *entry = entry
                    .checked_add(1)
                    .ok_or(FingerprintCollectError::NotStored)?;
            }
            questions.push(QuestionAnswers {
                answers: counts
                    .into_iter()
                    .map(|(answer, count)| AnswerCount { answer, count })
                    .collect(),
            });
        }
    }
    Ok((FingerprintDistribution { run1, run2 }, sent))
}

fn chat_body(model: &str, user: &str) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "messages": [
            {"role": "system", "content": PROBE_SYSTEM_PROMPT},
            {"role": "user", "content": user},
        ],
        "temperature": 1,
        "top_p": 1,
        "max_tokens": PROBE_MAX_TOKENS,
    })
}

fn echoes_key(key: &str, result: &SendResult) -> bool {
    if key.is_empty() {
        return false;
    }
    let echoed = |field: &str| field.contains(key);
    result.model.as_deref().is_some_and(echoed)
        || result.text.as_deref().is_some_and(echoed)
        || result.tool_name.as_deref().is_some_and(echoed)
        || result.tool_args.as_deref().is_some_and(echoed)
        || result.usage_fields.iter().any(|field| echoed(field))
}

fn normalize_answer(text: &str) -> Result<String, FingerprintCollectError> {
    let answer = text.trim().to_lowercase();
    if answer.is_empty()
        || answer.chars().count() > MAX_ANSWER_CHARS
        || has_disallowed_control(&answer)
        || has_secret_prefix(&answer)
    {
        return Err(FingerprintCollectError::AnswerRejected);
    }
    Ok(answer)
}

fn has_disallowed_control(value: &str) -> bool {
    value.chars().any(|c| {
        let u = c as u32;
        u < 0x20 || u == 0x7f
    })
}

fn commit(
    store: &Store,
    write: &FingerprintBaselineWrite,
) -> Result<FingerprintBaselineUpsert, FingerprintCollectError> {
    match store.upsert_fingerprint_baseline(write) {
        Ok(upsert) => Ok(upsert),
        Err(StoreError::Core(CoreError::InvalidField {
            field: "distribution",
            ..
        })) => Err(FingerprintCollectError::DistributionRejected),
        Err(_) => Err(FingerprintCollectError::NotStored),
    }
}

fn map_http(err: ProbeHttpError) -> FingerprintCollectError {
    match err {
        ProbeHttpError::KeyNotSet { name } => FingerprintCollectError::KeyNotSet { name },
        ProbeHttpError::BadVarName => FingerprintCollectError::BadVarName,
        ProbeHttpError::InvalidKey => FingerprintCollectError::InvalidKey,
        ProbeHttpError::BaseNotHttps => FingerprintCollectError::HostRejected,
        ProbeHttpError::Dotenv { .. }
        | ProbeHttpError::Write { .. }
        | ProbeHttpError::RefusingToWrite => FingerprintCollectError::InvalidKey,
    }
}

fn host_rejected(base: &str, route_host: &str) -> bool {
    if base.is_empty()
        || base.chars().any(char::is_whitespace)
        || base.contains('@')
        || base.contains('?')
        || base.contains('#')
        || !base_url_allowed(base)
    {
        return true;
    }
    !base_host_matches(base, route_host)
}

fn base_host_matches(base: &str, route_host: &str) -> bool {
    url_hostname(base).is_some_and(|host| host.eq_ignore_ascii_case(route_hostname(route_host)))
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
    let idx = base.find("://")?;
    let scheme = &base[..idx];
    if !scheme.eq_ignore_ascii_case("https") && !scheme.eq_ignore_ascii_case("http") {
        return None;
    }
    let rest = &base[idx + 3..];
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    Some(route_hostname(authority))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_value_core::constants::PROBE_MONTHLY_BUDGET_YUAN;
    use agent_value_store::{DigestKey, FixedClock};

    #[test]
    fn sampling_config_matches_the_fields_that_are_sent() {
        assert_eq!(
            FINGERPRINT_SAMPLING_CONFIG,
            format!(
                "{{\"temperature\":1,\"top_p\":1,\"max_tokens\":{}}}",
                PROBE_MAX_TOKENS
            )
        );
        let body = chat_body("example-model", FINGERPRINT_PROMPTS[0]);
        let sampling: serde_json::Value =
            serde_json::from_str(FINGERPRINT_SAMPLING_CONFIG).unwrap();
        assert_eq!(body["temperature"], sampling["temperature"]);
        assert_eq!(body["top_p"], sampling["top_p"]);
        assert_eq!(body["max_tokens"], sampling["max_tokens"]);
        assert_eq!(body["messages"][0]["content"], PROBE_SYSTEM_PROMPT);
        assert_eq!(body["messages"][1]["content"], FINGERPRINT_PROMPTS[0]);
        assert!(!FINGERPRINT_SAMPLING_CONFIG.contains(PROBE_SYSTEM_PROMPT));
    }

    #[test]
    fn system_prompt_hash_covers_the_empty_string_and_the_sent_prompt() {
        assert_eq!(system_prompt_hash(""), sha256_hex(b""));
        assert_eq!(
            system_prompt_hash(PROBE_SYSTEM_PROMPT),
            sha256_hex(PROBE_SYSTEM_PROMPT.as_bytes())
        );
        assert_eq!(system_prompt_hash(PROBE_SYSTEM_PROMPT).len(), 64);
    }

    #[test]
    fn provisional_estimate_fits_under_the_monthly_cap() {
        assert_eq!(fingerprint_request_count(), 400);
        assert_eq!(
            400 * FINGERPRINT_ESTIMATE_PER_REQUEST_MICROS,
            fingerprint_collect_estimate().unwrap().micros()
        );
        let estimate = fingerprint_collect_estimate().unwrap();
        let cap = MicroYuan::from_yuan(PROBE_MONTHLY_BUDGET_YUAN).unwrap();
        assert!(estimate.micros() < cap.micros());
        assert_eq!(
            probe_budget(MicroYuan::new(0).unwrap(), estimate, cap).unwrap(),
            BudgetDecision::Allow
        );
        let short = MicroYuan::new(estimate.micros() - 1).unwrap();
        assert_eq!(
            probe_budget(MicroYuan::new(0).unwrap(), estimate, short).unwrap(),
            BudgetDecision::Stop
        );
    }

    #[test]
    fn host_rule_rejects_a_mismatch_userinfo_and_a_non_local_http_base() {
        assert!(!host_rejected("http://127.0.0.1:9/v1", "127.0.0.1"));
        assert!(!host_rejected(
            "https://api.openai.com/v1",
            "api.openai.com"
        ));
        assert!(!host_rejected("https://API.OPENAI.COM", "api.openai.com"));
        assert!(host_rejected("http://127.0.0.1:9/", "api.openai.com"));
        assert!(host_rejected("http://192.0.2.1/", "192.0.2.1"));
        assert!(host_rejected(
            "http://user:sk-test-secret-value@127.0.0.1/",
            "127.0.0.1"
        ));
        assert!(host_rejected("", "127.0.0.1"));
    }

    #[test]
    fn oversize_distribution_is_rejected_and_the_previous_row_stays() {
        let clock = FixedClock::new(1_700_000_000_000);
        let store = Store::open_in_memory_with_clock(test_digest_key(), clock).unwrap();
        let mut kept = test_write(small_distribution());
        kept.collected_at_ms = 1_700_000_000_000;
        assert_eq!(
            commit(&store, &kept).unwrap(),
            FingerprintBaselineUpsert::Inserted
        );

        let mut huge = test_write(huge_distribution());
        huge.collected_at_ms = kept.collected_at_ms + 1;
        let err = commit(&store, &huge).unwrap_err();
        assert_eq!(err, FingerprintCollectError::DistributionRejected);
        let rendered = format!("{err} {err:?}");
        assert!(!rendered.contains("size-marker"));
        assert!(!rendered.contains("sk-"));
        let got = store
            .get_fingerprint_baseline(&kept.identity)
            .unwrap()
            .expect("rejected upsert does not replace the row");
        assert_eq!(got.distribution, kept.distribution);
        assert_eq!(store.list_fingerprint_baselines(true).unwrap().len(), 1);
    }

    fn test_digest_key() -> DigestKey {
        DigestKey::from_bytes(b"test-only-digest-key-aaaaaaaaaaaa").unwrap()
    }

    fn test_write(distribution: FingerprintDistribution) -> FingerprintBaselineWrite {
        FingerprintBaselineWrite {
            identity: FingerprintBaselineIdentity {
                provider: "example-provider".into(),
                model_id: "example-model".into(),
                model_revision: String::new(),
                protocol: "openai-chat-completions".into(),
                connection_kind: OFFICIAL_DIRECT.into(),
                task_set_version: FINGERPRINT_TASK_SET_VERSION.into(),
                system_prompt_hash: sha256_hex(b"prompt"),
                sampling_config: FINGERPRINT_SAMPLING_CONFIG.into(),
                distribution_schema_version: DISTRIBUTION_SCHEMA_VERSION,
            },
            collected_at_ms: 1_700_000_000_000,
            distribution,
        }
    }

    fn small_distribution() -> FingerprintDistribution {
        let question = QuestionAnswers {
            answers: vec![AnswerCount {
                answer: "red".into(),
                count: 20,
            }],
        };
        FingerprintDistribution {
            run1: vec![question.clone(); 10],
            run2: vec![question; 10],
        }
    }

    fn huge_distribution() -> FingerprintDistribution {
        let one_run = |run: &str| {
            (0..10)
                .map(|q| {
                    let answers = (0..64)
                        .map(|a| {
                            let tag = if q == 0 && a == 0 {
                                format!("{run}-size-marker")
                            } else {
                                format!("{run}-q{q}a{a}")
                            };
                            AnswerCount {
                                answer: sized_answer(&tag),
                                count: 1,
                            }
                        })
                        .collect();
                    QuestionAnswers { answers }
                })
                .collect()
        };
        FingerprintDistribution {
            run1: one_run("r1"),
            run2: one_run("r2"),
        }
    }

    fn sized_answer(tag: &str) -> String {
        let mut answer = tag.to_string();
        answer.extend(std::iter::repeat_n('b', 200 - answer.chars().count()));
        answer
    }
}

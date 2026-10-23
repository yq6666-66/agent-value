//! Probes and relay checks, single runs (v22 §11.1–§11.2; DECISIONS 13.1–13.7).
//!
//! Pure functions only: this module builds the probe set, judges the
//! replies an outside runner collected, and classifies one run of one probe
//! class. Sending requests is the caller's job (CI never touches the
//! network); see `tools/probe_live.py` and `examples/`.
//! [`build_openai_chat_plan`] only builds the OpenAI Chat Completions
//! bodies that `probe_plan` prints.
//!
//! The grade per arm and window, the gate and the log-integrity gate are in
//! [`crate::authenticity`].

use std::collections::{BTreeMap, BTreeSet};

use crate::arm::{ModelId, Protocol};
use crate::behavior::SplitMix64;
use crate::constants::{DETERMINISTIC_PASS_MIN, FINGERPRINT_PASS_MIN, PROBE_QUESTIONS_PER_RUN};
use crate::error::{CoreError, Result};
use crate::records::ProbeVerdict;

/// Version of the fixed probe set (§11.1 "版本化"). Bump on any change to
/// the templates, the fixed system prompt or the fixed parameters.
pub const PROBE_SET_VERSION: &str = "1";

/// Fixed, minimal system prompt for every probe request (§11.1).
pub const PROBE_SYSTEM_PROMPT: &str = "You are a test endpoint. Answer exactly as asked.";

/// Fixed sampling parameters for deterministic and capability probes.
pub const PROBE_TEMPERATURE: f64 = 0.0;
pub const PROBE_MAX_TOKENS: u32 = 64;

/// §11.2 deterministic class: at most 7 correct fails. Provisional, like
/// the pass threshold 9: pending the observation period (DECISIONS 13.1).
pub const DETERMINISTIC_FAIL_MAX: u32 = 7;
/// §11.2 fingerprint class: at most 5 consistent questions fails.
/// Provisional (DECISIONS 13.1).
pub const FINGERPRINT_FAIL_MAX: u32 = 5;
/// §11.1 fingerprint class: 20 samples per question.
pub const FINGERPRINT_SAMPLES: u32 = 20;
/// §11.2 a fingerprint baseline older than 30 days must be re-collected.
pub const BASELINE_MAX_AGE_MS: i64 = 30 * 86_400_000;

// ---------------------------------------------------------------- answers

/// How one probe question came back (DECISIONS 13.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerState {
    Correct,
    Wrong,
    /// Rate limit, timeout, other HTTP error, unparseable response or no
    /// record: no evidence either way.
    Unanswered(Unanswered),
}

/// Why a question is unanswered (DECISIONS 13.2: the evidence summary
/// states the rate-limit and timeout counts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unanswered {
    /// HTTP 429.
    RateLimited,
    /// No response within the runner's timeout.
    Timeout,
    /// HTTP 5xx (`tools/probe_live.py` records a connection error as 599).
    ServerError,
    /// 2xx, but the body could not be parsed.
    Unparseable,
    /// Any other non-2xx status, such as a 4xx other than 429. A 400 on the
    /// thinking round trip is not unanswered but a rejection (DECISIONS 13.5).
    OtherHttpError,
    /// No record for the question (the runner stopped early), or a
    /// fingerprint question with fewer than 20 valid samples.
    Missing,
}

/// Reason of an exchange that is not a 2xx reply. Callers handle
/// `Answered` first; it never reaches here.
fn transport_failure(ex: &Exchange) -> Unanswered {
    match ex {
        Exchange::RateLimited => Unanswered::RateLimited,
        Exchange::Timeout => Unanswered::Timeout,
        Exchange::HttpError(s) if (500..600).contains(s) => Unanswered::ServerError,
        Exchange::Unparseable => Unanswered::Unparseable,
        Exchange::HttpError(_) | Exchange::Answered(_) => Unanswered::OtherHttpError,
    }
}

/// Unanswered questions of one class run, by reason. The fields add up to
/// [`ClassRun::unanswered`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UnansweredCounts {
    pub rate_limited: u32,
    pub timeout: u32,
    pub server_error: u32,
    pub unparseable: u32,
    pub other_http_error: u32,
    pub missing: u32,
}

/// One request/response exchange as an outside runner recorded it.
#[derive(Debug, Clone, PartialEq)]
pub enum Exchange {
    /// HTTP 2xx with a parsed body.
    Answered(Reply),
    /// HTTP 429.
    RateLimited,
    /// No response within the runner's timeout.
    Timeout,
    /// Any other non-2xx status.
    HttpError(u16),
    /// 2xx, but the body could not be parsed.
    Unparseable,
}

/// The parts of a parsed reply the checks use. No prompt or key material.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Reply {
    pub text: String,
    /// The raw `model` field, if present.
    pub returned_model: Option<String>,
    /// Names of the fields under `usage` (values are not needed).
    pub usage_fields: Vec<String>,
    /// First tool call, if any: (function name, raw JSON arguments).
    pub tool_call: Option<(String, String)>,
}

/// Counts of one class run and its verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassRun {
    pub verdict: ProbeVerdict,
    pub correct: u32,
    pub wrong: u32,
    pub unanswered: u32,
    /// `unanswered` by reason (DECISIONS 13.2).
    pub unanswered_by: UnansweredCounts,
}

impl ClassRun {
    /// Evidence summary of the run (`ProbeResult::evidence_summary`):
    /// verdict and counts only, with the unanswered questions by reason
    /// (DECISIONS 13.2). No reply text, no key material.
    pub fn evidence_summary(&self) -> String {
        let u = &self.unanswered_by;
        format!(
            "{}: correct {}, wrong {}, unanswered {} (429 {}, timeout {}, 5xx {}, unparseable {}, other HTTP error {}, missing {})",
            self.verdict.as_str(),
            self.correct,
            self.wrong,
            self.unanswered,
            u.rate_limited,
            u.timeout,
            u.server_error,
            u.unparseable,
            u.other_http_error,
            u.missing
        )
    }
}

fn bump(x: &mut u32) -> Result<()> {
    *x = x.checked_add(1).ok_or(CoreError::Overflow {
        what: "probe answer count",
    })?;
    Ok(())
}

fn count(states: &[AnswerState]) -> Result<(u32, u32, u32, UnansweredCounts)> {
    let (mut correct, mut wrong, mut unanswered) = (0u32, 0u32, 0u32);
    let mut by = UnansweredCounts::default();
    for s in states {
        match s {
            AnswerState::Correct => bump(&mut correct)?,
            AnswerState::Wrong => bump(&mut wrong)?,
            AnswerState::Unanswered(why) => {
                bump(&mut unanswered)?;
                bump(match why {
                    Unanswered::RateLimited => &mut by.rate_limited,
                    Unanswered::Timeout => &mut by.timeout,
                    Unanswered::ServerError => &mut by.server_error,
                    Unanswered::Unparseable => &mut by.unparseable,
                    Unanswered::OtherHttpError => &mut by.other_http_error,
                    Unanswered::Missing => &mut by.missing,
                })?;
            }
        }
    }
    Ok((correct, wrong, unanswered, by))
}

fn ten(n: usize) -> Result<()> {
    if n != PROBE_QUESTIONS_PER_RUN as usize {
        return Err(CoreError::LengthMismatch {
            left: n,
            right: PROBE_QUESTIONS_PER_RUN as usize,
        });
    }
    Ok(())
}

/// Bounds rule of DECISIONS 13.2: pass when `good >= pass_min` even if every
/// open item went bad; fail when `n - bad <= fail_max` even if every open
/// item went good; otherwise uncertain.
fn bounded(good: u32, bad: u32, n: u32, pass_min: u32, fail_max: u32) -> ProbeVerdict {
    if good >= pass_min {
        ProbeVerdict::Pass
    } else if n.saturating_sub(bad) <= fail_max {
        ProbeVerdict::Fail
    } else {
        ProbeVerdict::Uncertain
    }
}

/// §11.2 deterministic class, one run of exactly 10 questions: ≥9 correct
/// passes, ≤7 fails, 8 is uncertain. Unanswered questions only widen the
/// bounds (DECISIONS 13.2).
pub fn deterministic_verdict(states: &[AnswerState]) -> Result<ClassRun> {
    ten(states.len())?;
    let (correct, wrong, unanswered, unanswered_by) = count(states)?;
    Ok(ClassRun {
        verdict: bounded(
            correct,
            wrong,
            PROBE_QUESTIONS_PER_RUN,
            DETERMINISTIC_PASS_MIN,
            DETERMINISTIC_FAIL_MAX,
        ),
        correct,
        wrong,
        unanswered,
        unanswered_by,
    })
}

/// §11.2 fingerprint class, one run of exactly 10 questions, each `Some(true)`
/// (JS ≤ τ_JS), `Some(false)` or `None` (fewer than 20 valid samples).
/// Without an official baseline the class is uncertain.
pub fn fingerprint_verdict(per_question: &[Option<bool>], has_baseline: bool) -> Result<ClassRun> {
    ten(per_question.len())?;
    let states: Vec<AnswerState> = per_question
        .iter()
        .map(|q| match q {
            Some(true) => AnswerState::Correct,
            Some(false) => AnswerState::Wrong,
            None => AnswerState::Unanswered(Unanswered::Missing),
        })
        .collect();
    let (correct, wrong, unanswered, unanswered_by) = count(&states)?;
    let verdict = if has_baseline {
        bounded(
            correct,
            wrong,
            PROBE_QUESTIONS_PER_RUN,
            FINGERPRINT_PASS_MIN,
            FINGERPRINT_FAIL_MAX,
        )
    } else {
        ProbeVerdict::Uncertain
    };
    Ok(ClassRun {
        verdict,
        correct,
        wrong,
        unanswered,
        unanswered_by,
    })
}

/// §11.2 capability class over the applicable items: any wrong fails; else
/// any unanswered is uncertain; else pass. No applicable item: not
/// applicable (DECISIONS 13.5).
pub fn capability_verdict(applicable: &[AnswerState]) -> Result<ClassRun> {
    let (correct, wrong, unanswered, unanswered_by) = count(applicable)?;
    let verdict = if applicable.is_empty() {
        ProbeVerdict::NotApplicable
    } else if wrong > 0 {
        ProbeVerdict::Fail
    } else if unanswered > 0 {
        ProbeVerdict::Uncertain
    } else {
        ProbeVerdict::Pass
    };
    Ok(ClassRun {
        verdict,
        correct,
        wrong,
        unanswered,
        unanswered_by,
    })
}

// --------------------------------------------------- deterministic probe set

/// One deterministic question. `expected` never leaves the machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetQuestion {
    pub id: String,
    pub prompt: String,
    pub expected: String,
}

struct Rng(SplitMix64);

impl Rng {
    /// Uniform in `lo..=hi` by rejection (no modulo bias).
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        let n = hi - lo + 1;
        let zone = (u64::MAX / n) * n;
        loop {
            let x = self.0.next_u64();
            if x < zone {
                return lo + x % n;
            }
        }
    }

    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.range(0, xs.len() as u64 - 1) as usize]
    }

    fn letters(&mut self, alphabet: &[u8], len: usize) -> String {
        (0..len)
            .map(|_| alphabet[self.range(0, alphabet.len() as u64 - 1) as usize] as char)
            .collect()
    }
}

const ANSWER_ONLY: &str = "Reply with the answer only.";

/// The 10 deterministic questions for `seed` (§11.1 class 1): random numbers,
/// one unique answer each, and one of two wordings per question so that the
/// text differs between runs. Seed 0 is rejected (it would make the run
/// reproducible by anyone who guesses it).
pub fn deterministic_set(seed: u64) -> Result<Vec<DetQuestion>> {
    if seed == 0 {
        return Err(CoreError::InvalidField {
            field: "probe seed",
            reason: "must be non-zero",
        });
    }
    let mut r = Rng(SplitMix64::new(seed));
    let mut out = Vec::with_capacity(PROBE_QUESTIONS_PER_RUN as usize);
    let mut push = |n: usize, prompt: String, expected: String| {
        out.push(DetQuestion {
            id: format!("det-{n:02}"),
            prompt: format!("{prompt} {ANSWER_ONLY}"),
            expected,
        });
    };
    // 1. addition
    let (a, b) = (r.range(100, 999), r.range(100, 999));
    let w = r.pick(&["What is {a} + {b}?", "Add {a} and {b}."]);
    push(1, fill(w, &[a, b]), (a + b).to_string());
    // 2. subtraction
    let (a, b) = (r.range(500, 999), r.range(100, 499));
    let w = r.pick(&["What is {a} - {b}?", "Subtract {b} from {a}."]);
    push(2, fill(w, &[a, b]), (a - b).to_string());
    // 3. multiplication
    let (a, b) = (r.range(12, 99), r.range(12, 99));
    let w = r.pick(&["What is {a} * {b}?", "Multiply {a} by {b}."]);
    push(3, fill(w, &[a, b]), (a * b).to_string());
    // 4. remainder
    let (a, b) = (r.range(1000, 9999), r.range(7, 97));
    let w = r.pick(&[
        "What is the remainder of {a} divided by {b}?",
        "Compute {a} mod {b}.",
    ]);
    push(4, fill(w, &[a, b]), (a % b).to_string());
    // 5. reverse the digits of a 6-digit number with no zero digit
    let digits: String = (0..6)
        .map(|_| char::from(b'0' + r.range(1, 9) as u8))
        .collect();
    let w = r.pick(&[
        "Write the digits of {s} in reverse order.",
        "Reverse the digit string {s}.",
    ]);
    push(5, w.replace("{s}", &digits), digits.chars().rev().collect());
    // 6. upper case
    let word = r.letters(b"bcdfghjkmnpqrstvwxz", 7);
    let w = r.pick(&[
        "Write {s} in upper case.",
        "Convert {s} to capital letters.",
    ]);
    push(6, w.replace("{s}", &word), word.to_uppercase());
    // 7. maximum of five distinct numbers
    let mut xs: Vec<u64> = Vec::new();
    while xs.len() < 5 {
        let x = r.range(10, 999);
        if !xs.contains(&x) {
            xs.push(x);
        }
    }
    let list = join(&xs);
    let w = r.pick(&["Which is the largest of {s}?", "Give the maximum of {s}."]);
    push(
        7,
        w.replace("{s}", &list),
        xs.iter().max().unwrap_or(&0).to_string(),
    );
    // 8. sort four distinct numbers ascending
    let mut ys: Vec<u64> = Vec::new();
    while ys.len() < 4 {
        let y = r.range(10, 99);
        if !ys.contains(&y) {
            ys.push(y);
        }
    }
    // An already ascending list would be a trivial question: show it reversed
    // (no extra random draw, so the rest of the set is unchanged).
    if ys.windows(2).all(|p| p[0] < p[1]) {
        ys.reverse();
    }
    let shown = join(&ys);
    ys.sort_unstable();
    let w = r.pick(&[
        "Sort {s} in ascending order, comma-separated.",
        "List {s} from smallest to largest, separated by commas.",
    ]);
    push(8, w.replace("{s}", &shown), join(&ys));
    // 9. count a letter
    let s = r.letters(b"abc", 9);
    let n = s.chars().filter(|c| *c == 'a').count();
    let w = r.pick(&[
        "How many times does the letter a appear in {s}?",
        "Count the letter a in {s}.",
    ]);
    push(9, w.replace("{s}", &s), n.to_string());
    // 10. digit sum of a 7-digit number
    let v = r.range(1_000_000, 9_999_999);
    let sum: u64 = v.to_string().bytes().map(|d| u64::from(d - b'0')).sum();
    let w = r.pick(&[
        "What is the sum of the digits of {a}?",
        "Add up the digits of {a}.",
    ]);
    push(10, fill(w, &[v]), sum.to_string());
    Ok(out)
}

fn fill(template: &str, v: &[u64]) -> String {
    let mut s = template.replace("{a}", &v[0].to_string());
    if let Some(b) = v.get(1) {
        s = s.replace("{b}", &b.to_string());
    }
    s
}

fn join(xs: &[u64]) -> String {
    xs.iter().map(u64::to_string).collect::<Vec<_>>().join(",")
}

fn normalize_reply(expected: &str, reply: &str) -> String {
    let mut t = reply.trim();
    for _ in 0..2 {
        t = t
            .trim_matches(|c| matches!(c, '`' | '"' | '\'' | '*'))
            .trim();
        t = t.strip_suffix('.').unwrap_or(t).trim();
    }
    let mut s: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    if !expected.contains(',') {
        s.retain(|c| c != ',');
    }
    s
}

/// Judges a deterministic or long-context reply: exact match after trimming
/// whitespace, one level of quotes/backticks/bold and a final period; a
/// thousands separator is ignored when the answer has no comma.
pub fn check_answer(expected: &str, reply: &str) -> bool {
    normalize_reply(expected, reply) == expected
}

/// State of a deterministic question from its exchange.
pub fn answer_state(expected: &str, ex: &Exchange) -> AnswerState {
    match ex {
        Exchange::Answered(r) => {
            if check_answer(expected, &r.text) {
                AnswerState::Correct
            } else {
                AnswerState::Wrong
            }
        }
        other => AnswerState::Unanswered(transport_failure(other)),
    }
}

// ------------------------------------------------------------ capability

/// §11.1 class 3 items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CapabilityItem {
    ToolCall,
    JsonSchema,
    ThinkingSignature,
    LongContext,
}

/// What the route and model claim to support (§11.2: only these are checked).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CapabilityClaims {
    pub tool_call: bool,
    pub json_schema: bool,
    pub thinking: bool,
    /// Long-context recall is run once a month; `true` when this run includes it.
    pub long_context_due: bool,
}

/// The applicable items, in a fixed order. Thinking-signature round trip
/// applies only on the Anthropic Messages protocol with thinking claimed.
pub fn applicable_items(claims: &CapabilityClaims, protocol: &Protocol) -> Vec<CapabilityItem> {
    let mut v = Vec::new();
    if claims.tool_call {
        v.push(CapabilityItem::ToolCall);
    }
    if claims.json_schema {
        v.push(CapabilityItem::JsonSchema);
    }
    if claims.thinking && matches!(protocol, Protocol::AnthropicMessages) {
        v.push(CapabilityItem::ThinkingSignature);
    }
    if claims.long_context_due {
        v.push(CapabilityItem::LongContext);
    }
    v
}

/// Fixed tool name of the tool-call probe; its single integer argument is `n`.
pub const TOOL_NAME: &str = "record_number";

/// The random integer the capability probes ask for (tool argument `n`,
/// JSON field `answer`), from the run seed.
pub fn capability_number(seed: u64) -> u64 {
    let mut r = Rng(SplitMix64::new(seed ^ 0x00C0_FFEE));
    r.range(100, 999)
}

fn compact(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// State of one capability item from its exchange.
///
/// - Tool call: a call to [`TOOL_NAME`] whose arguments are exactly
///   `{"n":<number>}` (whitespace ignored).
/// - JSON schema: the content is exactly `{"answer":<number>}`.
/// - Thinking signature: the round trip with the signed thinking block sent
///   back is accepted (any 2xx, whatever the body); HTTP 400 is a rejection
///   (wrong and a §11.2 hard failure, see [`hard_failures`]).
/// - Long context: the recalled key, as [`check_answer`].
///
/// 429, timeout, other HTTP errors and unparseable replies are unanswered
/// for every item (DECISIONS 13.2, 13.5).
pub fn capability_state(item: CapabilityItem, expected: &str, ex: &Exchange) -> AnswerState {
    match (item, ex) {
        (CapabilityItem::ThinkingSignature, Exchange::HttpError(400)) => AnswerState::Wrong,
        (_, Exchange::Answered(r)) => {
            let ok = match item {
                CapabilityItem::ToolCall => r.tool_call.as_ref().is_some_and(|(n, a)| {
                    n == TOOL_NAME && compact(a) == format!("{{\"n\":{expected}}}")
                }),
                CapabilityItem::JsonSchema => {
                    compact(&r.text) == format!("{{\"answer\":{expected}}}")
                }
                CapabilityItem::LongContext => check_answer(expected, &r.text),
                CapabilityItem::ThinkingSignature => true,
            };
            if ok {
                AnswerState::Correct
            } else {
                AnswerState::Wrong
            }
        }
        (_, other) => AnswerState::Unanswered(transport_failure(other)),
    }
}

// ------------------------------------------------- OpenAI chat plan bodies

/// Description of the single function tool on the tool-call probe.
const PROBE_TOOL_DESCRIPTION: &str = "Record one integer.";
/// `tool_choice` sent with that tool.
const PROBE_TOOL_CHOICE: &str = "required";
/// Integer argument of [`TOOL_NAME`].
const PROBE_TOOL_ARGUMENT: &str = "n";
/// `response_format.json_schema.name` of the strict-JSON probe.
const PROBE_JSON_SCHEMA_NAME: &str = "answer";
/// The one integer property of that schema.
const PROBE_JSON_SCHEMA_FIELD: &str = "answer";

/// One message in an OpenAI Chat Completions request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiChatMessage {
    pub role: String,
    pub content: String,
}

/// The function tool attached to the tool-call probe (`tools` plus
/// `tool_choice`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiProbeTool {
    pub name: String,
    pub description: String,
    /// Integer argument name (`n`).
    pub argument: String,
    /// Sent as `tool_choice` (`required`).
    pub tool_choice: String,
}

/// Strict JSON Schema response format of the JSON probe: an object with
/// one integer field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiProbeJsonSchema {
    /// `response_format.json_schema.name`.
    pub name: String,
    /// The single integer property.
    pub field: String,
}

/// Chat Completions request body for one minimal-probe question.
///
/// `tool` and `json_schema` are set by [`build_openai_chat_plan`] for
/// `cap-tool` and `cap-json` only, and never both. Sampling is the fixed
/// probe temperature and max tokens.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenAiChatBody {
    pub model: String,
    pub messages: Vec<OpenAiChatMessage>,
    pub temperature: f64,
    pub max_tokens: u32,
    pub tool: Option<OpenAiProbeTool>,
    pub json_schema: Option<OpenAiProbeJsonSchema>,
}

impl OpenAiChatBody {
    /// Compact JSON object, the `body` value of a `probe_plan` line.
    ///
    /// No extra spaces. Key order is `model`, `messages`, `temperature`,
    /// `max_tokens`, then `tools` / `tool_choice` and `response_format`
    /// when present. `temperature` uses [`std::fmt::Display`], so `0.0`
    /// is printed as `0`. `tools/probe_live.py` parses this and
    /// re-serializes it when it sends; this crate does not send it.
    pub fn to_json(&self) -> String {
        let mut extra = String::new();
        if let Some(tool) = &self.tool {
            extra.push_str(&tool_json_suffix(tool));
        }
        if let Some(schema) = &self.json_schema {
            extra.push_str(&json_schema_suffix(schema));
        }
        format!(
            "{{\"model\":{},\"messages\":{},\"temperature\":{},\"max_tokens\":{}{}}}",
            json_string(&self.model),
            messages_json(&self.messages),
            self.temperature,
            self.max_tokens,
            extra
        )
    }
}

/// One row of the OpenAI Chat Completions plan: `{"id","body"}`.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenAiChatPlanRow {
    /// `det-01` … `det-10`, `cap-tool`, or `cap-json`.
    pub id: String,
    pub body: OpenAiChatBody,
}

impl OpenAiChatPlanRow {
    /// One plan line, no trailing newline: `{"id":"...","body":{...}}`.
    pub fn to_json_line(&self) -> String {
        format!(
            "{{\"id\":{},\"body\":{}}}",
            json_string(&self.id),
            self.body.to_json()
        )
    }
}

/// OpenAI Chat Completions bodies for the minimal probe (§11.1:
/// deterministic class, plus the capability items this plan can express).
///
/// Order is fixed: `det-01` … `det-10` from [`deterministic_set`], then
/// `cap-tool` when `claims.tool_call`, then `cap-json` when
/// `claims.json_schema`. Those capability ids are [`capability_id`].
/// Thinking-signature and long-context items are not generated, even if
/// claimed. Expected
/// answers are not included. `model` is copied as given (not normalised).
/// Seed 0 is rejected. This function does not send a request.
pub fn build_openai_chat_plan(
    seed: u64,
    model: &str,
    claims: &CapabilityClaims,
) -> Result<Vec<OpenAiChatPlanRow>> {
    let questions = deterministic_set(seed)?;
    let mut rows = Vec::with_capacity(questions.len() + 2);
    for q in questions {
        rows.push(OpenAiChatPlanRow {
            id: q.id,
            body: plain_chat_body(model, q.prompt),
        });
    }
    let items = applicable_items(claims, &Protocol::OpenAiChatCompletions);
    if items.contains(&CapabilityItem::ToolCall) || items.contains(&CapabilityItem::JsonSchema) {
        let n = capability_number(seed);
        if items.contains(&CapabilityItem::ToolCall) {
            rows.push(OpenAiChatPlanRow {
                id: capability_id(CapabilityItem::ToolCall).to_string(),
                body: OpenAiChatBody {
                    tool: Some(OpenAiProbeTool {
                        name: TOOL_NAME.to_string(),
                        description: PROBE_TOOL_DESCRIPTION.to_string(),
                        argument: PROBE_TOOL_ARGUMENT.to_string(),
                        tool_choice: PROBE_TOOL_CHOICE.to_string(),
                    }),
                    ..plain_chat_body(model, format!("Call {TOOL_NAME} with n = {n}."))
                },
            });
        }
        if items.contains(&CapabilityItem::JsonSchema) {
            rows.push(OpenAiChatPlanRow {
                id: capability_id(CapabilityItem::JsonSchema).to_string(),
                body: OpenAiChatBody {
                    json_schema: Some(OpenAiProbeJsonSchema {
                        name: PROBE_JSON_SCHEMA_NAME.to_string(),
                        field: PROBE_JSON_SCHEMA_FIELD.to_string(),
                    }),
                    ..plain_chat_body(
                        model,
                        format!("Return a JSON object whose answer field is {n}."),
                    )
                },
            });
        }
    }
    Ok(rows)
}

fn plain_chat_body(model: &str, user: String) -> OpenAiChatBody {
    OpenAiChatBody {
        model: model.to_string(),
        messages: vec![
            OpenAiChatMessage {
                role: "system".to_string(),
                content: PROBE_SYSTEM_PROMPT.to_string(),
            },
            OpenAiChatMessage {
                role: "user".to_string(),
                content: user,
            },
        ],
        temperature: PROBE_TEMPERATURE,
        max_tokens: PROBE_MAX_TOKENS,
        tool: None,
        json_schema: None,
    }
}

/// JSON string literal, same escapes `probe_plan` used before this lived
/// in the core: quotes, backslashes, `\n`, `\t`, and `\u00xx` for every
/// other char below U+0020. Other characters are copied as UTF-8.
fn json_string(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn messages_json(messages: &[OpenAiChatMessage]) -> String {
    let mut out = String::from("[");
    for (i, m) in messages.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"role\":{},\"content\":{}}}",
            json_string(&m.role),
            json_string(&m.content)
        ));
    }
    out.push(']');
    out
}

fn tool_json_suffix(tool: &OpenAiProbeTool) -> String {
    let name = json_string(&tool.name);
    let description = json_string(&tool.description);
    let argument = json_string(&tool.argument);
    let tool_choice = json_string(&tool.tool_choice);
    format!(
        ",\"tools\":[{{\"type\":\"function\",\"function\":{{\"name\":{name},\"description\":{description},\"parameters\":{{\"type\":\"object\",\"properties\":{{{argument}:{{\"type\":\"integer\"}}}},\"required\":[{argument}],\"additionalProperties\":false}}}}}}],\"tool_choice\":{tool_choice}"
    )
}

fn json_schema_suffix(schema: &OpenAiProbeJsonSchema) -> String {
    let name = json_string(&schema.name);
    let field = json_string(&schema.field);
    format!(
        ",\"response_format\":{{\"type\":\"json_schema\",\"json_schema\":{{\"name\":{name},\"strict\":true,\"schema\":{{\"type\":\"object\",\"properties\":{{{field}:{{\"type\":\"integer\"}}}},\"required\":[{field}],\"additionalProperties\":false}}}}}}"
    )
}

// ------------------------------------------------------------ fingerprint

/// The 10 fingerprint prompts (§11.1 class 2): one-token answers, sampled 20
/// times each at the route's default temperature.
pub const FINGERPRINT_PROMPTS: [&str; 10] = [
    "Say a random number from 1 to 100. Reply with the number only.",
    "Name a random colour. Reply with one word only.",
    "Pick a random letter of the English alphabet. Reply with the letter only.",
    "Name a random fruit. Reply with one word only.",
    "Name a random animal. Reply with one word only.",
    "Say a random number from 1 to 10. Reply with the number only.",
    "Name a random country. Reply with one word only.",
    "Name a random day of the week. Reply with one word only.",
    "Pick a random planet of the solar system. Reply with one word only.",
    "Name a random musical instrument. Reply with one word only.",
];

/// Empirical answer distribution: answer (trimmed, lower-cased) → count.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Distribution(BTreeMap<String, u32>);

impl Distribution {
    pub fn from_answers<'a>(answers: impl IntoIterator<Item = &'a str>) -> Result<Self> {
        let mut m = BTreeMap::new();
        for a in answers {
            let k = a.trim().to_lowercase();
            let e = m.entry(k).or_insert(0u32);
            *e = e.checked_add(1).ok_or(CoreError::Overflow {
                what: "fingerprint count",
            })?;
        }
        Ok(Self(m))
    }

    pub fn total(&self) -> Result<u32> {
        self.0.values().try_fold(0u32, |s, v| {
            s.checked_add(*v).ok_or(CoreError::Overflow {
                what: "fingerprint total",
            })
        })
    }

    /// Pools two distributions (DECISIONS 13.4: the baseline reference).
    pub fn pooled(&self, other: &Self) -> Result<Self> {
        let mut m = self.0.clone();
        for (k, v) in &other.0 {
            let e = m.entry(k.clone()).or_insert(0);
            *e = e.checked_add(*v).ok_or(CoreError::Overflow {
                what: "fingerprint count",
            })?;
        }
        Ok(Self(m))
    }
}

/// Jensen–Shannon divergence in bits (log₂), in `[0, 1]` (DECISIONS 13.4).
pub fn js_divergence(p: &Distribution, q: &Distribution) -> Result<f64> {
    let (np, nq) = (f64::from(p.total()?), f64::from(q.total()?));
    if np == 0.0 || nq == 0.0 {
        return Err(CoreError::InvalidField {
            field: "fingerprint distribution",
            reason: "must not be empty",
        });
    }
    let keys: BTreeSet<&String> = p.0.keys().chain(q.0.keys()).collect();
    let mut js = 0.0;
    for k in keys {
        let a = f64::from(*p.0.get(k).unwrap_or(&0)) / np;
        let b = f64::from(*q.0.get(k).unwrap_or(&0)) / nq;
        let m = 0.5 * (a + b);
        if a > 0.0 {
            js += 0.5 * a * (a / m).log2();
        }
        if b > 0.0 {
            js += 0.5 * b * (b / m).log2();
        }
    }
    Ok(js.clamp(0.0, 1.0))
}

/// Nearest-rank quantile: the `ceil(p n)`-th smallest value (DECISIONS 13.4).
pub fn nearest_rank(values: &[f64], p: f64) -> Result<f64> {
    if values.is_empty() || !(p > 0.0 && p <= 1.0) {
        return Err(CoreError::InvalidField {
            field: "quantile",
            reason: "needs values and 0 < p <= 1",
        });
    }
    let mut v = values.to_vec();
    if v.iter().any(|x| !x.is_finite()) {
        return Err(CoreError::NotFinite {
            what: "quantile value",
            value: f64::NAN,
        });
    }
    v.sort_by(f64::total_cmp);
    let rank = (p * v.len() as f64).ceil() as usize;
    Ok(v[rank.clamp(1, v.len()) - 1])
}

/// What a fingerprint baseline is kept for (§11.2): model version,
/// protocol and sampling parameters. A route's samples are compared only
/// with a baseline of the same key.
///
/// Sampling is not parsed. [`BaselineKey::new`] only trims `sampling` and
/// then compares that text, so `"Temperature=1"` and `"temperature=1"` are
/// different keys and the fingerprint class is uncertain, not a false fail.
/// The canonical form is lower-case `key=value` parameters joined in a
/// stable order chosen by the caller, for example `"temperature=1"`.
/// Callers should lowercase and join before building the key; this type
/// does not rewrite case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineKey {
    /// Normalized model name ([`VendorTables::normalize`]).
    pub model_version: String,
    pub protocol: Protocol,
    /// Sampling parameters in the canonical form described on [`BaselineKey`].
    /// Only surrounding whitespace is removed; letter case is significant.
    pub sampling: String,
}

impl BaselineKey {
    /// Builds a key. `model` goes through [`VendorTables::normalize`].
    ///
    /// `sampling` is trimmed and otherwise stored unchanged. Pass the
    /// lower-case `key=value` form, pairs joined in a stable order
    /// (for example `"temperature=1"`), so two runs of the same parameters
    /// share a key. See [`BaselineKey`].
    pub fn new(tables: &VendorTables, model: &str, protocol: Protocol, sampling: &str) -> Self {
        Self {
            model_version: tables.normalize(model),
            protocol,
            sampling: sampling.trim().to_string(),
        }
    }
}

/// Official-direct fingerprint baseline for one [`BaselineKey`]: two
/// independent runs of the 10 questions.
#[derive(Debug, Clone, PartialEq)]
pub struct FingerprintBaseline {
    pub key: BaselineKey,
    pub collected_ms: i64,
    pub run1: Vec<Distribution>,
    pub run2: Vec<Distribution>,
}

impl FingerprintBaseline {
    fn check(&self) -> Result<()> {
        ten(self.run1.len())?;
        ten(self.run2.len())
    }

    /// τ_JS: 95% nearest-rank quantile of the 10 per-question JS values
    /// between the two baseline runs (with 10 values, the largest).
    /// Provisional, like the 9/10 and 8/10 thresholds: pending the
    /// observation period (DECISIONS 13.1, 13.4).
    pub fn tau_js(&self) -> Result<f64> {
        self.check()?;
        let js: Vec<f64> = self
            .run1
            .iter()
            .zip(&self.run2)
            .map(|(a, b)| js_divergence(a, b))
            .collect::<Result<_>>()?;
        nearest_rank(&js, 0.95)
    }

    /// Usable at `at_ms`: not older than 30 days (exactly 30 days is still usable).
    pub fn fresh_at(&self, at_ms: i64) -> Result<bool> {
        let age = at_ms
            .checked_sub(self.collected_ms)
            .ok_or(CoreError::Overflow {
                what: "baseline age",
            })?;
        Ok((0..=BASELINE_MAX_AGE_MS).contains(&age))
    }

    /// Per question: `Some(JS ≤ τ_JS)` against the pooled baseline, or `None`
    /// when the question has fewer than 20 valid samples.
    pub fn compare(&self, samples: &[Distribution]) -> Result<Vec<Option<bool>>> {
        self.check()?;
        ten(samples.len())?;
        let tau = self.tau_js()?;
        samples
            .iter()
            .zip(self.run1.iter().zip(&self.run2))
            .map(|(s, (a, b))| {
                if s.total()? < FINGERPRINT_SAMPLES {
                    return Ok(None);
                }
                Ok(Some(js_divergence(s, &a.pooled(b)?)? <= tau))
            })
            .collect()
    }
}

/// Fingerprint run of samples taken under `key`: compare against a fresh
/// baseline of the same key, else uncertain (a baseline of another model
/// version, protocol or sampling parameters counts as no baseline).
pub fn fingerprint_run(
    samples: &[Distribution],
    key: &BaselineKey,
    baseline: Option<&FingerprintBaseline>,
    at_ms: i64,
) -> Result<ClassRun> {
    ten(samples.len())?;
    match baseline {
        Some(b) if b.key == *key && b.fresh_at(at_ms)? => {
            fingerprint_verdict(&b.compare(samples)?, true)
        }
        _ => fingerprint_verdict(&[None; 10], false),
    }
}

// ------------------------------------------------- model names and vendors

/// Trim and lower-case one caller-supplied table token (DECISIONS 13.6, 13.7).
fn table_token(s: &str) -> String {
    s.trim().to_lowercase()
}

/// Model-family and vendor tables (DECISIONS 13.6, 13.7). `Default` holds
/// the built-in tables; callers may replace or extend them.
///
/// Prefixes, host keys and vendor names are trimmed and lower-cased on
/// lookup ([`VendorTables::vendor`], [`VendorTables::official_vendor`]),
/// the same way alias values are. A caller cannot bypass that by writing
/// `"Anthropic"` or `" API.Anthropic.com "` into the public fields
/// (DECISIONS 13.6, 13.7). An entry that is empty after trimming is ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VendorTables {
    /// Exact alias → canonical name. The lookup key is the trimmed,
    /// lower-cased model name, so keys must be lower case; the mapped value
    /// is trimmed and lower-cased again on use (DECISIONS 13.6).
    pub aliases: BTreeMap<String, String>,
    /// Family-name prefix → vendor; the longest matching prefix wins.
    /// Both sides are trimmed and lower-cased on lookup. The same
    /// normalized length keeps the later entry, so `push` overrides a
    /// built-in prefix (DECISIONS 13.6).
    pub family_vendor: Vec<(String, String)>,
    /// Official host → vendor. The host key and the vendor are trimmed
    /// and lower-cased on lookup. A key that is not already that
    /// canonical host overrides the built-in row, so a mixed-case insert
    /// is not lost to `BTreeMap` order (DECISIONS 13.7).
    pub official_hosts: BTreeMap<String, String>,
}

impl Default for VendorTables {
    fn default() -> Self {
        let fv = [
            ("claude", "anthropic"),
            ("gpt", "openai"),
            ("o", "openai"),
            ("codex", "openai"),
            ("gemini", "google"),
            ("deepseek", "deepseek"),
            ("glm", "zhipu"),
            ("grok", "xai"),
            ("minimax", "minimax"),
            ("qwen", "alibaba"),
        ];
        let hosts = [
            ("api.anthropic.com", "anthropic"),
            ("api.openai.com", "openai"),
            ("generativelanguage.googleapis.com", "google"),
            ("api.deepseek.com", "deepseek"),
            ("open.bigmodel.cn", "zhipu"),
            ("api.x.ai", "xai"),
            ("api.minimax.chat", "minimax"),
            ("dashscope.aliyuncs.com", "alibaba"),
        ];
        Self {
            aliases: BTreeMap::new(),
            family_vendor: fv
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
            official_hosts: hosts
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
        }
    }
}

fn is_version(tok: &str) -> bool {
    let t = tok.strip_prefix('v').unwrap_or(tok);
    !t.is_empty() && t.chars().all(|c| c.is_ascii_digit() || c == '.')
}

impl VendorTables {
    /// Lower case, trim, alias (its value lower-cased and trimmed too, so
    /// a caller's table cannot reintroduce case), then drop a trailing date
    /// (YYYYMMDD, YYYY-MM-DD or MMDD).
    pub fn normalize(&self, name: &str) -> String {
        let mut s = name.trim().to_lowercase();
        if let Some(a) = self.aliases.get(&s) {
            s = a.trim().to_lowercase();
        }
        let mut toks: Vec<&str> = s.split('-').collect();
        let digits = |t: &str, n: usize| t.len() == n && t.chars().all(|c| c.is_ascii_digit());
        if toks.len() > 3 {
            let k = toks.len();
            if digits(toks[k - 3], 4) && digits(toks[k - 2], 2) && digits(toks[k - 1], 2) {
                toks.truncate(k - 3);
            }
        }
        if toks.len() > 1 {
            let last = toks[toks.len() - 1];
            if digits(last, 8) || digits(last, 4) {
                toks.pop();
            }
        }
        toks.join("-")
    }

    /// Family: the non-version `-` segments of the normalized name, in order.
    pub fn family(&self, name: &str) -> String {
        self.normalize(name)
            .split('-')
            .filter(|t| !is_version(t))
            .collect::<Vec<_>>()
            .join("-")
    }

    /// Vendor by the longest family prefix (a whole `-` segment or a
    /// version-attached prefix such as `o3` or `qwen3`).
    ///
    /// The prefix and the vendor are trimmed and lower-cased before the
    /// match, and the returned vendor is that lower-cased form, so a
    /// caller-supplied table stays aligned with [`protocol_vendor`] and
    /// [`usage_field_vendor`] (DECISIONS 13.6). Equal lengths keep the
    /// later entry in [`VendorTables::family_vendor`].
    pub fn vendor(&self, name: &str) -> Option<String> {
        let fam = self.family(name);
        let first = self.normalize(name);
        let head = first.split('-').next().unwrap_or("");
        self.family_vendor
            .iter()
            .filter_map(|(prefix, vendor)| {
                let prefix = table_token(prefix);
                let vendor = table_token(vendor);
                if prefix.is_empty() || vendor.is_empty() {
                    return None;
                }
                let matches = fam == prefix
                    || fam.starts_with(&format!("{prefix}-"))
                    || head
                        .strip_prefix(prefix.as_str())
                        .is_some_and(|rest| rest.is_empty() || is_version(rest));
                matches.then_some((prefix, vendor))
            })
            .max_by_key(|(prefix, _)| prefix.len())
            .map(|(_, vendor)| vendor)
    }

    /// Vendor of an official host, if the host is in the table.
    ///
    /// The stored host and the query are both trimmed and lower-cased, and
    /// the returned vendor is trimmed and lower-cased (DECISIONS 13.7).
    /// The map is not rewritten: the fields are public, so a caller can
    /// insert after [`VendorTables::default`]. A stored key that is not
    /// already the canonical host overrides one that is, which is how
    /// `" API.Anthropic.com "` replaces `"api.anthropic.com"` instead of
    /// losing on `BTreeMap` order. If several such overrides match, the
    /// last in `BTreeMap` order wins.
    pub fn official_vendor(&self, host: &str) -> Option<String> {
        let host = table_token(host);
        if host.is_empty() {
            return None;
        }
        let mut canonical = None;
        let mut override_vendor = None;
        for (key, vendor) in &self.official_hosts {
            let key_norm = table_token(key);
            let vendor = table_token(vendor);
            if key_norm.is_empty() || vendor.is_empty() || key_norm != host {
                continue;
            }
            if key.as_str() == key_norm {
                canonical = Some(vendor);
            } else {
                override_vendor = Some(vendor);
            }
        }
        override_vendor.or(canonical)
    }
}

/// Vendor whose wire protocol this is (its own usage names are always allowed).
pub fn protocol_vendor(p: &Protocol) -> Option<&'static str> {
    match p {
        Protocol::AnthropicMessages => Some("anthropic"),
        Protocol::OpenAiChatCompletions | Protocol::OpenAiResponses => Some("openai"),
        Protocol::Other(_) => None,
    }
}

/// The vendor a usage field name belongs to exclusively, if any (DECISIONS 13.6).
pub fn usage_field_vendor(field: &str) -> Option<&'static str> {
    match field {
        "cache_creation_input_tokens" | "cache_read_input_tokens" => Some("anthropic"),
        "promptTokenCount" | "candidatesTokenCount" | "cachedContentTokenCount" => Some("google"),
        "prompt_cache_hit_tokens" | "prompt_cache_miss_tokens" => Some("deepseek"),
        f if f.starts_with("claude_") => Some("anthropic"),
        _ => None,
    }
}

/// §11.2 hard failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HardFailure {
    /// The returned `model` belongs to another model family.
    OtherFamily { returned: String },
    /// A usage field exclusive to another vendor.
    ForeignUsage { field: String, vendor: String },
    /// The signed thinking block was rejected on round trip.
    ThinkingSignatureRejected,
}

/// True when `returned` is in another family than `claimed`.
pub fn other_family(tables: &VendorTables, claimed: &ModelId, returned: &str) -> bool {
    tables.family(returned) != tables.family(claimed.as_str())
}

/// Hard failures of one exchange (§11.2 第 766 行, after excluding protocol
/// differences: the protocol's own usage names are allowed).
///
/// `item` is the capability item the exchange answered, if any. A 400 on
/// the thinking round trip is a hard failure only where that item applies
/// ("适用前提下"): Anthropic Messages protocol with thinking claimed
/// (DECISIONS 13.5, [`applicable_items`]).
pub fn hard_failures(
    tables: &VendorTables,
    claimed: &ModelId,
    protocol: &Protocol,
    claims: &CapabilityClaims,
    item: Option<CapabilityItem>,
    ex: &Exchange,
) -> Vec<HardFailure> {
    let mut v = Vec::new();
    match ex {
        Exchange::Answered(r) => {
            if let Some(m) = &r.returned_model {
                if other_family(tables, claimed, m) {
                    v.push(HardFailure::OtherFamily {
                        returned: m.clone(),
                    });
                }
            }
            let own = tables.vendor(claimed.as_str());
            let proto = protocol_vendor(protocol);
            for f in &r.usage_fields {
                if let Some(vendor) = usage_field_vendor(f) {
                    if own.as_deref() != Some(vendor) && Some(vendor) != proto {
                        v.push(HardFailure::ForeignUsage {
                            field: f.clone(),
                            vendor: vendor.to_string(),
                        });
                    }
                }
            }
        }
        Exchange::HttpError(400)
            if item == Some(CapabilityItem::ThinkingSignature)
                && applicable_items(claims, protocol)
                    .contains(&CapabilityItem::ThinkingSignature) =>
        {
            v.push(HardFailure::ThinkingSignatureRejected);
        }
        _ => {}
    }
    v
}

// ----------------------------------------------------------- replay format

/// One recorded exchange: the fixed replay format shared by the tests and
/// the local live runner (tab-separated, one line per exchange).
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayRecord {
    pub question_id: String,
    pub at_ms: i64,
    pub latency_ms: i64,
    pub exchange: Exchange,
}

/// Header line of the replay format.
pub const REPLAY_HEADER: &str =
    "question_id\tat_ms\tlatency_ms\tkind\tstatus\tmodel\tusage_fields\ttext\ttool_name\ttool_args";

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some(o) => {
                    out.push('\\');
                    out.push(o);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Parses the replay format. Lines starting with `#` and the header are
/// skipped. `kind` is one of `ok` (status must be 2xx), `rate_limited`,
/// `timeout`, `http_error` (status ≥ 300 and not 429), `unparseable`;
/// empty optional fields are `None`.
pub fn parse_replay(text: &str) -> Result<Vec<ReplayRecord>> {
    let bad = |line: &str| CoreError::Parse {
        what: "probe replay line",
        value: line.chars().take(80).collect(),
    };
    let mut out = Vec::new();
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') || line == REPLAY_HEADER {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != 10 {
            return Err(bad(line));
        }
        let at_ms: i64 = f[1].parse().map_err(|_| bad(line))?;
        let latency_ms: i64 = f[2].parse().map_err(|_| bad(line))?;
        if latency_ms < 0 || f[0].is_empty() {
            return Err(bad(line));
        }
        let status: u16 = if f[4].is_empty() {
            0
        } else {
            f[4].parse().map_err(|_| bad(line))?
        };
        let opt = |s: &str| (!s.is_empty()).then(|| unescape(s));
        let exchange = match f[3] {
            "ok" if (200..300).contains(&status) => Exchange::Answered(Reply {
                text: unescape(f[7]),
                returned_model: opt(f[5]),
                usage_fields: f[6]
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect(),
                tool_call: opt(f[8]).map(|n| (n, unescape(f[9]))),
            }),
            "rate_limited" => Exchange::RateLimited,
            "timeout" => Exchange::Timeout,
            "http_error" if status >= 300 && status != 429 => Exchange::HttpError(status),
            "unparseable" => Exchange::Unparseable,
            _ => return Err(bad(line)),
        };
        out.push(ReplayRecord {
            question_id: f[0].to_string(),
            at_ms,
            latency_ms,
            exchange,
        });
    }
    Ok(out)
}

/// Question ids of the capability items in replays.
pub fn capability_id(item: CapabilityItem) -> &'static str {
    match item {
        CapabilityItem::ToolCall => "cap-tool",
        CapabilityItem::JsonSchema => "cap-json",
        CapabilityItem::ThinkingSignature => "cap-thinking",
        CapabilityItem::LongContext => "cap-long",
    }
}

fn by_id(records: &[ReplayRecord]) -> Result<BTreeMap<&str, &ReplayRecord>> {
    let mut m = BTreeMap::new();
    for r in records {
        if m.insert(r.question_id.as_str(), r).is_some() {
            return Err(CoreError::InvalidField {
                field: "probe replay",
                reason: "duplicate question id",
            });
        }
    }
    Ok(m)
}

/// Judges a deterministic run recorded for `seed`. A question with no
/// record (the runner stopped early) is unanswered.
pub fn judge_deterministic(seed: u64, records: &[ReplayRecord]) -> Result<ClassRun> {
    let m = by_id(records)?;
    let states: Vec<AnswerState> = deterministic_set(seed)?
        .iter()
        .map(|q| {
            m.get(q.id.as_str())
                .map_or(AnswerState::Unanswered(Unanswered::Missing), |r| {
                    answer_state(&q.expected, &r.exchange)
                })
        })
        .collect();
    deterministic_verdict(&states)
}

/// Judges the applicable capability items of a run recorded for `seed`.
/// `long_context_expected` is the key the long-context prompt hid.
pub fn judge_capability(
    seed: u64,
    items: &[CapabilityItem],
    long_context_expected: Option<&str>,
    records: &[ReplayRecord],
) -> Result<ClassRun> {
    let m = by_id(records)?;
    let n = capability_number(seed).to_string();
    let states: Vec<AnswerState> = items
        .iter()
        .map(|it| {
            let expected = match it {
                CapabilityItem::LongContext => {
                    long_context_expected.ok_or(CoreError::EmptyField {
                        field: "long-context expected key",
                    })?
                }
                _ => n.as_str(),
            };
            Ok(m.get(capability_id(*it))
                .map_or(AnswerState::Unanswered(Unanswered::Missing), |r| {
                    capability_state(*it, expected, &r.exchange)
                }))
        })
        .collect::<Result<_>>()?;
    capability_verdict(&states)
}

/// All hard failures in a run's records. `claims` are the route's
/// capability claims (DECISIONS 13.14): a rejected thinking round trip
/// counts only where that item applies (DECISIONS 13.5).
pub fn run_hard_failures(
    tables: &VendorTables,
    claimed: &ModelId,
    protocol: &Protocol,
    claims: &CapabilityClaims,
    records: &[ReplayRecord],
) -> Vec<HardFailure> {
    records
        .iter()
        .flat_map(|r| {
            let item = (r.question_id == capability_id(CapabilityItem::ThinkingSignature))
                .then_some(CapabilityItem::ThinkingSignature);
            hard_failures(tables, claimed, protocol, claims, item, &r.exchange)
        })
        .collect()
}

#[cfg(test)]
mod probe_unit_tests {
    use super::*;

    #[test]
    fn answer_counts_use_checked_addition() {
        // A run never has more than a handful of answers; the counter type is
        // u32 and must not wrap (exercised through the public API).
        let r = capability_verdict(&[AnswerState::Correct; 3]).unwrap();
        assert_eq!((r.correct, r.wrong, r.unanswered), (3, 0, 0));
        assert_eq!(r.unanswered_by, UnansweredCounts::default());
    }

    #[test]
    fn rejection_sampler_stays_in_range() {
        let mut r = Rng(SplitMix64::new(7));
        for _ in 0..1000 {
            let x = r.range(3, 5);
            assert!((3..=5).contains(&x));
        }
    }
}

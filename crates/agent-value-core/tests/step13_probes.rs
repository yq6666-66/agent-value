//! Step 13: probes and relay detection (v22 §11, §5 rule 4, §9 budget;
//! DECISIONS 13.1–13.14). No network: every probe exchange comes from a
//! fixed replay file or is built in the test.

use agent_value_core::arm::{Arm, ModelId, Protocol, ProviderKind, Route};
use agent_value_core::authenticity::{
    assess, budget_month, grade, is_official_direct, log_integrity, observation, probe_budget,
    route_overhead, AuthLabel, AuthStatus, BudgetDecision, DaySummary, GateContext, LoggedModel,
    Observation, ProbeClassKey, TimingSample,
};
use agent_value_core::cell_values::Triple;
use agent_value_core::cells::{CellId, CellWeights};
use agent_value_core::constants::{
    DETERMINISTIC_PASS_MIN, FINGERPRINT_PASS_MIN, PROBE_MONTHLY_BUDGET_YUAN,
    PROBE_QUESTIONS_PER_RUN,
};
use agent_value_core::eligibility::{assess_eligibility, AuthenticityGrade, EligibilityGate};
use agent_value_core::probe::{
    answer_state, applicable_items, capability_number, capability_state, check_answer,
    deterministic_set, deterministic_verdict, fingerprint_run, fingerprint_verdict, hard_failures,
    js_divergence, judge_capability, judge_deterministic, nearest_rank, other_family, parse_replay,
    run_hard_failures, AnswerState, BaselineKey, CapabilityClaims, CapabilityItem, Distribution,
    Exchange, FingerprintBaseline, HardFailure, Reply, Unanswered, UnansweredCounts, VendorTables,
    BASELINE_MAX_AGE_MS, DETERMINISTIC_FAIL_MAX, FINGERPRINT_FAIL_MAX, TOOL_NAME,
};
use agent_value_core::records::{MicroYuan, ProbeClass, ProbeResult, ProbeVerdict};
use agent_value_core::retry::{Prior, RetryModel};
use agent_value_core::runday::RunDay;
use agent_value_core::status::{layer_status, CellCounts, CellInput};

use AnswerState::{Correct as C, Wrong as W};
/// An unanswered question (no record); the reason only matters for the counts.
const U: AnswerState = AnswerState::Unanswered(Unanswered::Missing);

const MIXED: &str = include_str!("fixtures/step13_replay_mixed.tsv");
const LIVE: &str = include_str!("fixtures/step13_live_replay.tsv");

const DAY: i64 = 86_400_000;
/// 2026-10-21 12:00 Beijing time.
const NOON0: i64 = 1_792_512_000_000 + 12 * 3_600_000;

// ------------------------------------------------------------ helpers

fn arm(model: &str, host: &str, protocol: Protocol) -> Arm {
    let kind = if host.starts_with("api.") {
        ProviderKind::Official
    } else {
        ProviderKind::Relay
    };
    Arm::new(
        ModelId::new(model).unwrap(),
        Route::new(kind, host, protocol, "cli").unwrap(),
    )
}

fn relay() -> Arm {
    arm(
        "claude-haiku-4-5",
        "kapibala.asia",
        Protocol::OpenAiChatCompletions,
    )
}

fn pr(a: &Arm, day: i64, class: ProbeClass, verdict: ProbeVerdict, hard: bool) -> ProbeResult {
    ProbeResult {
        probe_id: "minimal".into(),
        probe_version: "1".into(),
        arm: a.clone(),
        at_ms: NOON0 + day * DAY,
        class,
        verdict,
        hard_failure: hard,
        evidence_summary: "test".into(),
    }
}

use ProbeClass::{Capability as Cap, Deterministic as Det, Fingerprint as Fp};
use ProbeVerdict::{Fail, NotApplicable as Na, Pass, Uncertain};

fn summary(a: &Arm, rs: &[(i64, ProbeClass, ProbeVerdict)]) -> DaySummary {
    let v: Vec<ProbeResult> = rs
        .iter()
        .map(|(d, c, x)| pr(a, *d, *c, *x, false))
        .collect();
    DaySummary::build(a, &v, &[]).unwrap()
}

fn g(rs: &[(i64, ProbeClass, ProbeVerdict)]) -> AuthenticityGrade {
    grade(&summary(&relay(), rs))
}

fn rep(n: usize, s: AnswerState) -> Vec<AnswerState> {
    vec![s; n]
}

fn states(c: usize, x: usize, u: usize) -> Vec<AnswerState> {
    let mut v = rep(c, C);
    v.extend(rep(x, W));
    v.extend(rep(u, U));
    v
}

fn dist(pairs: &[(&str, usize)]) -> Distribution {
    let answers: Vec<&str> = pairs
        .iter()
        .flat_map(|(a, n)| std::iter::repeat_n(*a, *n))
        .collect();
    Distribution::from_answers(answers).unwrap()
}

fn answered(text: &str) -> Exchange {
    Exchange::Answered(Reply {
        text: text.into(),
        ..Reply::default()
    })
}

// ------------------------------------------ 13.1 thresholds (provisional)

#[test]
fn thresholds_are_the_spec_values_marked_provisional() {
    // §11.2: deterministic >=9 / <=7, fingerprint >=8 / <=5, 10 questions.
    // DECISIONS 13.1: provisional until the observation period.
    assert_eq!(PROBE_QUESTIONS_PER_RUN, 10);
    assert_eq!((DETERMINISTIC_PASS_MIN, DETERMINISTIC_FAIL_MAX), (9, 7));
    assert_eq!((FINGERPRINT_PASS_MIN, FINGERPRINT_FAIL_MAX), (8, 5));
}

#[test]
fn deterministic_verdict_thresholds_without_transport_failures() {
    let v = |c| {
        deterministic_verdict(&states(c, 10 - c, 0))
            .unwrap()
            .verdict
    };
    assert_eq!(v(10), Pass);
    assert_eq!(v(9), Pass);
    assert_eq!(v(8), Uncertain);
    assert_eq!(v(7), Fail);
    assert_eq!(v(0), Fail);
}

#[test]
fn fingerprint_verdict_thresholds() {
    let v = |t: usize| {
        let mut q = vec![Some(true); t];
        q.extend(vec![Some(false); 10 - t]);
        fingerprint_verdict(&q, true).unwrap().verdict
    };
    assert_eq!(v(10), Pass);
    assert_eq!(v(8), Pass);
    assert_eq!(v(7), Uncertain);
    assert_eq!(v(6), Uncertain);
    assert_eq!(v(5), Fail);
    // No baseline: always uncertain (§11.2).
    assert_eq!(
        fingerprint_verdict(&[Some(true); 10], false)
            .unwrap()
            .verdict,
        Uncertain
    );
}

// ------------------------------------- 13.2 transport failures: bounds rule

#[test]
fn bounds_rule_cases_from_decisions_13_2() {
    let r = deterministic_verdict(&states(9, 0, 1)).unwrap();
    assert_eq!((r.verdict, r.correct, r.unanswered), (Pass, 9, 1));
    assert_eq!(
        deterministic_verdict(&states(8, 0, 2)).unwrap().verdict,
        Uncertain
    );
    assert_eq!(
        deterministic_verdict(&states(0, 3, 7)).unwrap().verdict,
        Fail
    );
    assert_eq!(
        deterministic_verdict(&states(0, 2, 8)).unwrap().verdict,
        Uncertain
    );
    let all = deterministic_verdict(&states(0, 0, 10)).unwrap();
    assert_eq!((all.verdict, all.unanswered), (Uncertain, 10));
    // Exactly 10 questions.
    assert!(deterministic_verdict(&states(9, 0, 0)).is_err());
    assert!(deterministic_verdict(&states(11, 0, 0)).is_err());
}

#[test]
fn fingerprint_bounds_with_undetermined_questions() {
    let mut q = vec![Some(true); 8];
    q.extend([None, None]);
    assert_eq!(fingerprint_verdict(&q, true).unwrap().verdict, Pass);
    let mut q = vec![Some(true); 7];
    q.extend([None, None, None]);
    assert_eq!(fingerprint_verdict(&q, true).unwrap().verdict, Uncertain);
    let mut q = vec![Some(false); 5];
    q.extend([None; 5]);
    assert_eq!(fingerprint_verdict(&q, true).unwrap().verdict, Fail);
    let mut q = vec![Some(false); 4];
    q.extend([None; 6]);
    assert_eq!(fingerprint_verdict(&q, true).unwrap().verdict, Uncertain);
}

#[test]
fn capability_verdict_rules() {
    assert_eq!(capability_verdict_of(&[C, C]), Pass);
    assert_eq!(capability_verdict_of(&[C, W]), Fail);
    assert_eq!(capability_verdict_of(&[W, U]), Fail);
    assert_eq!(capability_verdict_of(&[C, U]), Uncertain);
    assert_eq!(capability_verdict_of(&[]), Na);
}

fn capability_verdict_of(s: &[AnswerState]) -> ProbeVerdict {
    agent_value_core::probe::capability_verdict(s)
        .unwrap()
        .verdict
}

#[test]
fn transport_failures_are_unanswered_not_wrong() {
    for (ex, why) in [
        (Exchange::RateLimited, Unanswered::RateLimited),
        (Exchange::Timeout, Unanswered::Timeout),
        (Exchange::HttpError(503), Unanswered::ServerError),
        (Exchange::HttpError(500), Unanswered::ServerError),
        (Exchange::HttpError(599), Unanswered::ServerError),
        (Exchange::HttpError(400), Unanswered::OtherHttpError),
        (Exchange::HttpError(404), Unanswered::OtherHttpError),
        (Exchange::HttpError(600), Unanswered::OtherHttpError),
        (Exchange::Unparseable, Unanswered::Unparseable),
    ] {
        let u = AnswerState::Unanswered(why);
        assert_eq!(answer_state("1095", &ex), u, "{ex:?}");
        for item in [
            CapabilityItem::ToolCall,
            CapabilityItem::JsonSchema,
            CapabilityItem::LongContext,
        ] {
            assert_eq!(capability_state(item, "991", &ex), u, "{item:?} {ex:?}");
        }
    }
    assert_eq!(answer_state("1095", &answered("1095")), C);
    assert_eq!(answer_state("1095", &answered("1096")), W);
}

// ------------------------------------------------- replay parsing (fixture)

#[test]
fn replay_fixture_covers_every_branch() {
    let r = parse_replay(MIXED).unwrap();
    assert_eq!(r.len(), 12);
    assert_eq!(r[0].exchange, Exchange::RateLimited);
    assert_eq!(r[1].exchange, Exchange::Timeout);
    assert_eq!(r[2].exchange, Exchange::HttpError(503));
    assert_eq!(r[3].exchange, Exchange::Unparseable);
    match &r[10].exchange {
        Exchange::Answered(rep) => {
            assert_eq!(
                rep.tool_call,
                Some((TOOL_NAME.to_string(), "{\"n\": 991}".to_string()))
            );
            assert_eq!(
                rep.returned_model.as_deref(),
                Some("claude-haiku-4-5-20251001")
            );
            assert_eq!(rep.usage_fields.len(), 4);
        }
        other => panic!("{other:?}"),
    }
    match &r[11].exchange {
        Exchange::Answered(rep) => assert_eq!(rep.text, "```json\n{\"answer\": 991}\n```"),
        other => panic!("{other:?}"),
    }
    assert_eq!((r[0].at_ms, r[0].latency_ms), (1_792_601_568_000, 1000));
}

#[test]
fn replay_parse_rejects_bad_lines() {
    let h = "question_id\tat_ms\tlatency_ms\tkind\tstatus\tmodel\tusage_fields\ttext\ttool_name\ttool_args\n";
    let ok = format!("{h}det-01\t1\t2\tok\t200\t\t\t1\t\t\n");
    assert_eq!(parse_replay(&ok).unwrap().len(), 1);
    for bad in [
        "det-01\t1\t2\tok\t200\t\t\t1\t\n",          // 9 columns
        "det-01\t1\t-2\tok\t200\t\t\t1\t\t\n",       // negative latency
        "\t1\t2\tok\t200\t\t\t1\t\t\n",              // empty id
        "det-01\tx\t2\tok\t200\t\t\t1\t\t\n",        // bad time
        "det-01\t1\t2\thttp_error\t429\t\t\t\t\t\n", // 429 is rate_limited
        "det-01\t1\t2\thttp_error\t200\t\t\t\t\t\n", // 2xx is not an error
        "det-01\t1\t2\tmaybe\t200\t\t\t\t\t\n",      // unknown kind
        "det-01\t1\t2\tok\t2x0\t\t\t\t\t\n",
        "det-01\t1\t2\tok\t500\t\t\t1\t\t\n", // ok needs 2xx
        "det-01\t1\t2\tok\t429\t\t\t1\t\t\n",
        "det-01\t1\t2\tok\t\t\t\t1\t\t\n", // bad status
    ] {
        assert!(parse_replay(&format!("{h}{bad}")).is_err(), "{bad:?}");
    }
    // Escapes: \t, \n, \\ ; an unknown escape is kept.
    let esc = format!("{h}det-01\t1\t2\tok\t200\t\t\ta\\tb\\nc\\\\d\\q\t\t\n");
    match &parse_replay(&esc).unwrap()[0].exchange {
        Exchange::Answered(r) => assert_eq!(r.text, "a\tb\nc\\d\\q"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn judging_the_mixed_fixture() {
    // Seed 20261022: 429, timeout, 503, unparseable (4 unanswered); five
    // correct after normalisation ("**JCXPTRJ**", "683.", "10, 51, 53, 67");
    // det-09 answered 5 instead of 4. c=5, x=1, u=4 => 10-1 = 9 > 7: uncertain.
    let r = parse_replay(MIXED).unwrap();
    let det = judge_deterministic(20261022, &r).unwrap();
    assert_eq!(
        (det.verdict, det.correct, det.wrong, det.unanswered),
        (Uncertain, 5, 1, 4)
    );
    // Tool call is right; fenced JSON is not strict JSON (DECISIONS 13.14).
    let cap = judge_capability(
        20261022,
        &[CapabilityItem::ToolCall, CapabilityItem::JsonSchema],
        None,
        &r,
    )
    .unwrap();
    assert_eq!((cap.verdict, cap.correct, cap.wrong), (Fail, 1, 1));
    let tool_only = judge_capability(20261022, &[CapabilityItem::ToolCall], None, &r).unwrap();
    assert_eq!(tool_only.verdict, Pass);
    // Wrong seed: every answer is wrong or unanswered.
    let other = judge_deterministic(7, &r).unwrap();
    assert_eq!(other.correct, 0);
}

#[test]
fn missing_and_duplicate_records() {
    let r = parse_replay(LIVE).unwrap();
    let without_last: Vec<_> = r
        .iter()
        .filter(|x| x.question_id != "det-10")
        .cloned()
        .collect();
    let det = judge_deterministic(20261022, &without_last).unwrap();
    assert_eq!((det.verdict, det.correct, det.unanswered), (Pass, 9, 1));
    let only8: Vec<_> = r
        .iter()
        .filter(|x| x.question_id != "det-10" && x.question_id != "det-09")
        .cloned()
        .collect();
    assert_eq!(
        judge_deterministic(20261022, &only8).unwrap().verdict,
        Uncertain
    );
    assert_eq!(
        judge_deterministic(20261022, &[]).unwrap().verdict,
        Uncertain
    );
    let mut dup = r.clone();
    dup.push(r[0].clone());
    assert!(judge_deterministic(20261022, &dup).is_err());
    assert!(judge_capability(20261022, &[CapabilityItem::LongContext], None, &r).is_err());
}

#[test]
fn all_rate_limited_run_is_uncertain() {
    let h = "question_id\tat_ms\tlatency_ms\tkind\tstatus\tmodel\tusage_fields\ttext\ttool_name\ttool_args\n";
    let mut t = h.to_string();
    for i in 1..=10 {
        t.push_str(&format!(
            "det-{i:02}\t{i}\t5\trate_limited\t429\t\t\t\t\t\n"
        ));
    }
    t.push_str("cap-tool\t11\t5\ttimeout\t\t\t\t\t\t\n");
    let r = parse_replay(&t).unwrap();
    let det = judge_deterministic(20261022, &r).unwrap();
    assert_eq!((det.verdict, det.unanswered), (Uncertain, 10));
    let cap = judge_capability(20261022, &[CapabilityItem::ToolCall], None, &r).unwrap();
    assert_eq!((cap.verdict, cap.unanswered), (Uncertain, 1));
}

// ------------------------------------------------- the real-relay replay

#[test]
fn live_relay_replay_verdicts() {
    // The one real minimal-probe run (redacted replay, see fixtures/README).
    let r = parse_replay(LIVE).unwrap();
    assert_eq!(r.len(), 12);
    let det = judge_deterministic(20261022, &r).unwrap();
    assert_eq!(
        (det.verdict, det.correct, det.wrong, det.unanswered),
        (Pass, 10, 0, 0)
    );
    let cap = judge_capability(
        20261022,
        &[CapabilityItem::ToolCall, CapabilityItem::JsonSchema],
        None,
        &r,
    )
    .unwrap();
    assert_eq!((cap.verdict, cap.correct, cap.wrong), (Fail, 1, 1));
    let model = ModelId::new("claude-haiku-4-5").unwrap();
    let hard = run_hard_failures(
        &VendorTables::default(),
        &model,
        &Protocol::OpenAiChatCompletions,
        &CapabilityClaims {
            tool_call: true,
            json_schema: true,
            ..CapabilityClaims::default()
        },
        &r,
    );
    assert!(hard.is_empty(), "{hard:?}");
    let days: std::collections::BTreeSet<RunDay> = r
        .iter()
        .map(|x| RunDay::from_unix_ms(x.at_ms).unwrap())
        .collect();
    assert_eq!(days.len(), 1);
}

// ------------------------------------------------ deterministic probe set

/// Expected answers and prompts from the independent Python reference
/// generator `det_golden.py` (engineering log of step 13), seeds 20261022
/// and 7. The Rust code was not used to produce these values.
const GOLDEN_20261022: [(&str, &str, &str); 10] = [
    ("det-01", "1095", "What is 283 + 812? Reply with the answer only."),
    ("det-02", "507", "Subtract 147 from 654. Reply with the answer only."),
    ("det-03", "1160", "What is 29 * 40? Reply with the answer only."),
    ("det-04", "63", "What is the remainder of 4031 divided by 64? Reply with the answer only."),
    ("det-05", "338474", "Reverse the digit string 474833. Reply with the answer only."),
    ("det-06", "JCXPTRJ", "Write jcxptrj in upper case. Reply with the answer only."),
    ("det-07", "683", "Which is the largest of 683,187,474,100,529? Reply with the answer only."),
    ("det-08", "10,51,53,67", "List 67,53,51,10 from smallest to largest, separated by commas. Reply with the answer only."),
    ("det-09", "4", "How many times does the letter a appear in bbbbaaaab? Reply with the answer only."),
    ("det-10", "29", "Add up the digits of 1525835. Reply with the answer only."),
];

const GOLDEN_7: [(&str, &str, &str); 10] = [
    ("det-01", "1091", "What is 787 + 304? Reply with the answer only."),
    ("det-02", "529", "Subtract 174 from 703. Reply with the answer only."),
    ("det-03", "8820", "Multiply 90 by 98. Reply with the answer only."),
    ("det-04", "60", "What is the remainder of 4425 divided by 97? Reply with the answer only."),
    ("det-05", "924154", "Reverse the digit string 451429. Reply with the answer only."),
    ("det-06", "ZDQGMVH", "Write zdqgmvh in upper case. Reply with the answer only."),
    ("det-07", "705", "Which is the largest of 619,325,705,482,510? Reply with the answer only."),
    ("det-08", "28,31,48,49", "List 49,48,31,28 from smallest to largest, separated by commas. Reply with the answer only."),
    ("det-09", "3", "How many times does the letter a appear in bbbbcabaa? Reply with the answer only."),
    ("det-10", "33", "What is the sum of the digits of 7461096? Reply with the answer only."),
];

#[test]
fn deterministic_set_matches_the_reference_generator() {
    for (seed, golden, cap) in [(20261022u64, &GOLDEN_20261022, 991u64), (7, &GOLDEN_7, 193)] {
        let set = deterministic_set(seed).unwrap();
        assert_eq!(set.len(), 10);
        for (q, (id, expected, prompt)) in set.iter().zip(golden.iter()) {
            assert_eq!((q.id.as_str(), q.expected.as_str()), (*id, *expected));
            assert_eq!(q.prompt, *prompt);
            assert!(check_answer(&q.expected, &q.expected));
        }
        assert_eq!(capability_number(seed), cap);
    }
    assert!(deterministic_set(0).is_err());
}

#[test]
fn deterministic_set_varies_wording_and_numbers() {
    let mut first_words: Vec<std::collections::BTreeSet<String>> = vec![Default::default(); 10];
    let mut answers = std::collections::BTreeSet::new();
    for seed in 1..=60u64 {
        let set = deterministic_set(seed).unwrap();
        answers.insert(set[0].expected.clone());
        for (i, q) in set.iter().enumerate() {
            first_words[i].insert(q.prompt.split(' ').next().unwrap().to_string());
            // Every expected answer is unique and well formed.
            assert!(!q.expected.is_empty());
        }
        // det-08 never shows an already sorted list.
        let shown = set[7].prompt.split(' ').nth(1).unwrap().to_string();
        assert_ne!(shown, set[7].expected, "seed {seed}");
    }
    for (i, w) in first_words.iter().enumerate() {
        assert_eq!(w.len(), 2, "question {i}: {w:?}");
    }
    assert!(answers.len() > 40);
}

#[test]
fn answer_normalisation() {
    assert!(check_answer("1095", " 1095 "));
    assert!(check_answer("1095", "`1095`"));
    assert!(check_answer("1095", "**1095**."));
    assert!(check_answer("1095", "1,095"));
    assert!(check_answer("1095", "\"1095\""));
    assert!(!check_answer("1095", "10951"));
    assert!(!check_answer("1095", "The answer is 1095"));
    assert!(check_answer("10,51,53,67", "10, 51, 53, 67"));
    assert!(!check_answer("10,51,53,67", "10515367"));
    assert!(check_answer("JCXPTRJ", "JCXPTRJ."));
    assert!(!check_answer("JCXPTRJ", "jcxptrj"));
}

// ------------------------------------------------------------ capability

#[test]
fn capability_items_and_checks() {
    let all = CapabilityClaims {
        tool_call: true,
        json_schema: true,
        thinking: true,
        long_context_due: true,
    };
    use CapabilityItem::*;
    assert_eq!(
        applicable_items(&all, &Protocol::AnthropicMessages),
        vec![ToolCall, JsonSchema, ThinkingSignature, LongContext]
    );
    assert_eq!(
        applicable_items(&all, &Protocol::OpenAiChatCompletions),
        vec![ToolCall, JsonSchema, LongContext]
    );
    assert!(
        applicable_items(&CapabilityClaims::default(), &Protocol::AnthropicMessages).is_empty()
    );

    let tool = |name: &str, args: &str| {
        Exchange::Answered(Reply {
            tool_call: Some((name.into(), args.into())),
            ..Reply::default()
        })
    };
    assert_eq!(
        capability_state(ToolCall, "991", &tool(TOOL_NAME, "{ \"n\" : 991 }")),
        C
    );
    assert_eq!(
        capability_state(ToolCall, "991", &tool(TOOL_NAME, "{\"n\":992}")),
        W
    );
    assert_eq!(
        capability_state(ToolCall, "991", &tool("other", "{\"n\":991}")),
        W
    );
    assert_eq!(capability_state(ToolCall, "991", &answered("991")), W);
    assert_eq!(
        capability_state(JsonSchema, "991", &answered("{\n \"answer\": 991\n}")),
        C
    );
    assert_eq!(
        capability_state(
            JsonSchema,
            "991",
            &answered("```json\n{\"answer\":991}\n```")
        ),
        W
    );
    assert_eq!(
        capability_state(JsonSchema, "991", &answered("{\"answer\":\"991\"}")),
        W
    );
    assert_eq!(capability_state(LongContext, "K7Q", &answered("K7Q")), C);
    assert_eq!(capability_state(LongContext, "K7Q", &answered("K7R")), W);
    // Thinking round trip: 2xx accepted, 400 rejected, 429 unanswered (13.5).
    assert_eq!(capability_state(ThinkingSignature, "", &answered("")), C);
    assert_eq!(
        capability_state(ThinkingSignature, "", &Exchange::HttpError(400)),
        W
    );
}

#[test]
fn thinking_transport_failures_are_unanswered_and_any_2xx_conforms() {
    // DECISIONS 13.5: on the thinking round trip only a 400 is a rejection;
    // 429, timeout, 5xx, other errors and unparseable stay unanswered (13.2).
    use CapabilityItem::ThinkingSignature;
    for (ex, why) in [
        (Exchange::RateLimited, Unanswered::RateLimited),
        (Exchange::Timeout, Unanswered::Timeout),
        (Exchange::HttpError(500), Unanswered::ServerError),
        (Exchange::HttpError(503), Unanswered::ServerError),
        (Exchange::HttpError(401), Unanswered::OtherHttpError),
        (Exchange::Unparseable, Unanswered::Unparseable),
    ] {
        assert_eq!(
            capability_state(ThinkingSignature, "", &ex),
            AnswerState::Unanswered(why),
            "{ex:?}"
        );
    }
    // A 2xx is accepted whatever its body: empty text, no model, no usage.
    assert_eq!(
        capability_state(ThinkingSignature, "", &Exchange::Answered(Reply::default())),
        C
    );
    assert_eq!(
        capability_state(ThinkingSignature, "991", &answered("unrelated")),
        C
    );
    // Judged through a replay: an empty 2xx body passes the class.
    let h = "question_id\tat_ms\tlatency_ms\tkind\tstatus\tmodel\tusage_fields\ttext\ttool_name\ttool_args\n";
    let r = parse_replay(&format!("{h}cap-thinking\t1\t2\tok\t200\t\t\t\t\t\n")).unwrap();
    let cap = judge_capability(1, &[ThinkingSignature], None, &r).unwrap();
    assert_eq!((cap.verdict, cap.correct), (Pass, 1));
    let r = parse_replay(&format!("{h}cap-thinking\t1\t2\ttimeout\t\t\t\t\t\t\n")).unwrap();
    let cap = judge_capability(1, &[ThinkingSignature], None, &r).unwrap();
    assert_eq!(
        (cap.verdict, cap.wrong, cap.unanswered_by.timeout),
        (Uncertain, 0, 1)
    );
}

// ----------------------------------------------------------- fingerprint

#[test]
fn js_divergence_closed_forms() {
    let a = dist(&[("a", 10), ("b", 10)]);
    let b = dist(&[("A ", 20)]); // trimmed and lower-cased
    let expected = 1.5 - 0.75 * 3f64.log2(); // 0.311278...
    assert!((js_divergence(&a, &b).unwrap() - expected).abs() < 1e-12);
    assert!((expected - 0.311_278).abs() < 1e-6);
    assert!((js_divergence(&b, &a).unwrap() - expected).abs() < 1e-12);
    assert_eq!(js_divergence(&a, &a).unwrap(), 0.0);
    let c = dist(&[("x", 3)]);
    assert!((js_divergence(&b, &c).unwrap() - 1.0).abs() < 1e-12);
    assert!(js_divergence(&a, &Distribution::default()).is_err());
}

#[test]
fn nearest_rank_quantile() {
    let v: Vec<f64> = (1..=20).map(f64::from).collect();
    assert_eq!(nearest_rank(&v, 0.95).unwrap(), 19.0);
    let ten: Vec<f64> = (1..=10).rev().map(f64::from).collect();
    assert_eq!(nearest_rank(&ten, 0.95).unwrap(), 10.0); // n=10: the maximum
    assert_eq!(nearest_rank(&ten, 0.5).unwrap(), 5.0);
    assert!(nearest_rank(&[], 0.95).is_err());
    assert!(nearest_rank(&v, 0.0).is_err());
    assert!(nearest_rank(&v, 1.5).is_err());
    assert!(nearest_rank(&[f64::NAN], 0.5).is_err());
}

fn baseline(collected_ms: i64) -> FingerprintBaseline {
    // Run 1 and run 2 agree on 9 questions; question 0 differs by a
    // half/half vs all-one split, so tau_JS = 0.311278... (the maximum).
    let mut run1 = vec![dist(&[("a", 20)]); 10];
    let mut run2 = run1.clone();
    run1[0] = dist(&[("a", 10), ("b", 10)]);
    run2[0] = dist(&[("a", 20)]);
    FingerprintBaseline {
        key: key(),
        collected_ms,
        run1,
        run2,
    }
}

fn key() -> BaselineKey {
    BaselineKey::new(
        &VendorTables::default(),
        "claude-haiku-4-5-20251001",
        Protocol::AnthropicMessages,
        "temperature=1",
    )
}

#[test]
fn baseline_tau_freshness_and_comparison() {
    let b = baseline(NOON0);
    let tau = b.tau_js().unwrap();
    assert!((tau - (1.5 - 0.75 * 3f64.log2())).abs() < 1e-12);
    assert!(b.fresh_at(NOON0).unwrap());
    assert!(b.fresh_at(NOON0 + BASELINE_MAX_AGE_MS).unwrap());
    assert!(!b.fresh_at(NOON0 + BASELINE_MAX_AGE_MS + 1).unwrap());
    assert!(!b.fresh_at(NOON0 - 1).unwrap());
    assert_eq!(BASELINE_MAX_AGE_MS, 30 * DAY);

    // Same as the baseline on all 10 questions: pass.
    let same = vec![dist(&[("a", 20)]); 10];
    let r = fingerprint_run(&same, &key(), Some(&b), NOON0).unwrap();
    assert_eq!((r.verdict, r.correct), (Pass, 10));
    // Disjoint answers on 5 questions: 10 - 5 = 5 <= 5, fail.
    let mut five = same.clone();
    for d in five.iter_mut().take(5) {
        *d = dist(&[("z", 20)]);
    }
    assert_eq!(
        fingerprint_run(&five, &key(), Some(&b), NOON0)
            .unwrap()
            .verdict,
        Fail
    );
    // 19 valid samples: question undetermined.
    let mut short = same.clone();
    short[3] = dist(&[("a", 19)]);
    assert_eq!(b.compare(&short).unwrap()[3], None);
    // Stale or missing baseline: uncertain.
    assert_eq!(
        fingerprint_run(&same, &key(), Some(&b), NOON0 + BASELINE_MAX_AGE_MS + 1)
            .unwrap()
            .verdict,
        Uncertain
    );
    assert_eq!(
        fingerprint_run(&same, &key(), None, NOON0).unwrap().verdict,
        Uncertain
    );
    assert!(fingerprint_run(&same[..9], &key(), Some(&b), NOON0).is_err());
}

#[test]
fn baseline_of_another_key_counts_as_no_baseline() {
    let b = baseline(NOON0);
    let same = vec![dist(&[("a", 20)]); 10];
    let t = VendorTables::default();
    // Same model version written differently: same key.
    let alias = BaselineKey::new(
        &t,
        " Claude-Haiku-4-5 ",
        Protocol::AnthropicMessages,
        " temperature=1 ",
    );
    assert_eq!(alias, key());
    assert_eq!(
        fingerprint_run(&same, &alias, Some(&b), NOON0)
            .unwrap()
            .verdict,
        Pass
    );
    for other in [
        BaselineKey::new(
            &t,
            "claude-haiku-4-6",
            Protocol::AnthropicMessages,
            "temperature=1",
        ),
        BaselineKey::new(
            &t,
            "claude-haiku-4-5",
            Protocol::OpenAiChatCompletions,
            "temperature=1",
        ),
        BaselineKey::new(
            &t,
            "claude-haiku-4-5",
            Protocol::AnthropicMessages,
            "temperature=0.7",
        ),
    ] {
        let r = fingerprint_run(&same, &other, Some(&b), NOON0).unwrap();
        assert_eq!(r.verdict, Uncertain, "{other:?}");
    }
}

#[test]
fn comparison_uses_the_pooled_baseline() {
    // Question 0 pooled baseline: a=30, b=10. A route answering a=15, b=5
    // has JS 0 against the pool, so it agrees even though it matches
    // neither run exactly.
    let b = baseline(NOON0);
    let mut s = vec![dist(&[("a", 20)]); 10];
    s[0] = dist(&[("a", 15), ("b", 5)]);
    assert_eq!(b.compare(&s).unwrap()[0], Some(true));
    // Run order swapped (run 1 = {a:20}, run 2 = {a:10, b:10}; tau_JS is
    // still 0.311278...). A route answering a=5, b=15 has JS 0.1887 against
    // the pool (agrees) but 0.5488 against run 1 alone (would disagree).
    let mut swapped = baseline(NOON0);
    std::mem::swap(&mut swapped.run1, &mut swapped.run2);
    s[0] = dist(&[("a", 5), ("b", 15)]);
    assert_eq!(swapped.compare(&s).unwrap()[0], Some(true));
    // Far from the pool (JS 0.5488 > tau_JS): disagrees.
    s[0] = dist(&[("b", 20)]);
    assert_eq!(swapped.compare(&s).unwrap()[0], Some(false));
}

#[test]
fn zero_tau_baseline_still_accepts_the_same_distribution() {
    // §11.2 "JS ≤ τ_JS 算一致": equality counts. Two identical baseline runs
    // (temperature 0, or a single answer) give τ_JS = 0; a route with the
    // same distribution has JS = 0 against the pool and must agree.
    let run = vec![dist(&[("a", 20)]); 10];
    let mut mixed = run.clone();
    mixed[4] = dist(&[("a", 14), ("b", 6)]);
    let b = FingerprintBaseline {
        key: key(),
        collected_ms: NOON0,
        run1: mixed.clone(),
        run2: mixed.clone(),
    };
    assert_eq!(b.tau_js().unwrap(), 0.0);
    let r = fingerprint_run(&mixed, &key(), Some(&b), NOON0).unwrap();
    assert_eq!((r.verdict, r.correct, r.wrong), (Pass, 10, 0));
    // Same proportions at twice the sample count: still JS = 0, agrees.
    let mut doubled = run.clone();
    doubled[4] = dist(&[("a", 28), ("b", 12)]);
    assert_eq!(b.compare(&doubled).unwrap(), vec![Some(true); 10]);
    // Any other distribution on that question: JS > 0 = τ_JS, disagrees.
    let mut off = mixed.clone();
    off[4] = dist(&[("a", 13), ("b", 7)]);
    assert_eq!(b.compare(&off).unwrap()[4], Some(false));
}

#[test]
fn unanswered_counts_by_reason_reach_the_evidence_summary() {
    // DECISIONS 13.2: an all-unanswered run is uncertain and its evidence
    // summary states the rate-limit and timeout counts.
    let h = "question_id\tat_ms\tlatency_ms\tkind\tstatus\tmodel\tusage_fields\ttext\ttool_name\ttool_args\n";
    let mut t = h.to_string();
    for i in 1..=10 {
        t.push_str(&format!(
            "det-{i:02}\t{i}\t5\trate_limited\t429\t\t\t\t\t\n"
        ));
    }
    let r = parse_replay(&t).unwrap();
    let det = judge_deterministic(20261022, &r).unwrap();
    assert_eq!(
        det.unanswered_by,
        UnansweredCounts {
            rate_limited: 10,
            ..UnansweredCounts::default()
        }
    );
    let sum = det.evidence_summary();
    assert_eq!(
        sum,
        "uncertain: correct 0, wrong 0, unanswered 10 (429 10, timeout 0, 5xx 0, unparseable 0, other HTTP error 0, missing 0)"
    );
    // The summary is a valid ProbeResult evidence summary.
    let mut p = pr(&relay(), 0, Det, det.verdict, false);
    p.evidence_summary = sum;
    p.validate().unwrap();

    // Every reason, from the mixed fixture plus missing records.
    let r = parse_replay(MIXED).unwrap();
    let det = judge_deterministic(20261022, &r).unwrap();
    assert_eq!(
        det.unanswered_by,
        UnansweredCounts {
            rate_limited: 1,
            timeout: 1,
            server_error: 1,
            unparseable: 1,
            ..UnansweredCounts::default()
        }
    );
    let mut t = h.to_string();
    t.push_str("det-01\t1\t5\ttimeout\t\t\t\t\t\t\n");
    t.push_str("det-02\t2\t5\ttimeout\t\t\t\t\t\t\n");
    t.push_str("det-03\t3\t5\thttp_error\t502\t\t\t\t\t\n");
    t.push_str("det-04\t4\t5\thttp_error\t404\t\t\t\t\t\n");
    t.push_str("det-05\t5\t5\tunparseable\t200\t\t\t\t\t\n");
    t.push_str("det-06\t6\t5\trate_limited\t429\t\t\t\t\t\n");
    t.push_str("cap-tool\t7\t5\thttp_error\t503\t\t\t\t\t\n");
    let r = parse_replay(&t).unwrap();
    let det = judge_deterministic(20261022, &r).unwrap();
    let want = UnansweredCounts {
        rate_limited: 1,
        timeout: 2,
        server_error: 1,
        unparseable: 1,
        other_http_error: 1,
        missing: 4,
    };
    assert_eq!(
        (det.verdict, det.unanswered, det.unanswered_by),
        (Uncertain, 10, want)
    );
    assert_eq!(
        det.evidence_summary(),
        "uncertain: correct 0, wrong 0, unanswered 10 (429 1, timeout 2, 5xx 1, unparseable 1, other HTTP error 1, missing 4)"
    );
    let cap = judge_capability(
        20261022,
        &[CapabilityItem::ToolCall, CapabilityItem::JsonSchema],
        None,
        &r,
    )
    .unwrap();
    assert_eq!(
        cap.unanswered_by,
        UnansweredCounts {
            server_error: 1,
            missing: 1,
            ..UnansweredCounts::default()
        }
    );
    // Fingerprint: a question with fewer than 20 samples counts as missing.
    let b = baseline(NOON0);
    let mut short = vec![dist(&[("a", 20)]); 10];
    short[3] = dist(&[("a", 19)]);
    short[7] = dist(&[("a", 1)]);
    let fp = fingerprint_run(&short, &key(), Some(&b), NOON0).unwrap();
    assert_eq!(
        (fp.verdict, fp.unanswered, fp.unanswered_by.missing),
        (Pass, 2, 2)
    );
    // Counts add up for verdicts built from states directly.
    let run = deterministic_verdict(&[
        C,
        W,
        U,
        AnswerState::Unanswered(Unanswered::RateLimited),
        AnswerState::Unanswered(Unanswered::Timeout),
        AnswerState::Unanswered(Unanswered::ServerError),
        AnswerState::Unanswered(Unanswered::Unparseable),
        AnswerState::Unanswered(Unanswered::OtherHttpError),
        AnswerState::Unanswered(Unanswered::Timeout),
        C,
    ])
    .unwrap();
    assert_eq!((run.correct, run.wrong, run.unanswered), (2, 1, 7));
    assert_eq!(
        run.unanswered_by,
        UnansweredCounts {
            rate_limited: 1,
            timeout: 2,
            server_error: 1,
            unparseable: 1,
            other_http_error: 1,
            missing: 1,
        }
    );
}

// ---------------------------------------------- 13.6 models and vendors

#[test]
fn alias_values_with_upper_case_still_classify() {
    // A caller's alias table may map to a mixed-case or padded name; the
    // value is lower-cased and trimmed on use, so family, vendor and the
    // other-family check still work (DECISIONS 13.6).
    let mut t = VendorTables::default();
    t.aliases
        .insert("my-haiku".into(), " Claude-Haiku-4-5-20251001 ".into());
    assert_eq!(t.normalize("My-Haiku"), "claude-haiku-4-5");
    assert_eq!(t.family("my-haiku"), "claude-haiku");
    assert_eq!(t.vendor("my-haiku"), Some("anthropic"));
    let claimed = ModelId::new("claude-haiku-4-5").unwrap();
    assert!(!other_family(&t, &claimed, "my-haiku"));
    assert!(other_family(&t, &claimed, "gpt-5.4"));
    // Baseline keys agree whichever spelling is used.
    let k = |m: &str| BaselineKey::new(&t, m, Protocol::AnthropicMessages, "temperature=1");
    assert_eq!(k("my-haiku"), k("claude-haiku-4-5"));
    // Keys are looked up after lower-casing the name: a lower-case key
    // matches any spelling of the name.
    assert_eq!(t.normalize(" MY-HAIKU "), "claude-haiku-4-5");
}

#[test]
fn model_normalisation_family_and_vendor() {
    let t = VendorTables::default();
    assert_eq!(t.normalize("Claude-Haiku-4-5-20251001"), "claude-haiku-4-5");
    assert_eq!(
        t.normalize("claude-3-5-sonnet-20241022"),
        "claude-3-5-sonnet"
    );
    assert_eq!(t.normalize("gpt-4o-2024-08-06"), "gpt-4o");
    assert_eq!(t.normalize("glm-4-0520"), "glm-4");
    assert_eq!(t.normalize("1234"), "1234");
    assert_eq!(t.family("claude-haiku-4-5"), "claude-haiku");
    assert_eq!(t.family("claude-3-5-sonnet-20241022"), "claude-sonnet");
    assert_eq!(t.family("gpt-5.4-mini"), "gpt-mini");
    assert_eq!(t.family("gpt-5.4"), "gpt");
    assert_eq!(t.family("deepseek-v3"), "deepseek");
    for (m, v) in [
        ("claude-opus-4-1", "anthropic"),
        ("gpt-5.4", "openai"),
        ("o3", "openai"),
        ("o4-mini", "openai"),
        ("gemini-2.5-pro", "google"),
        ("deepseek-chat", "deepseek"),
        ("glm-4.6", "zhipu"),
        ("grok-4", "xai"),
        ("qwen3-coder-plus", "alibaba"),
        ("qwen-max", "alibaba"),
    ] {
        assert_eq!(t.vendor(m), Some(v), "{m}");
    }
    assert_eq!(t.vendor("openchat-3.5"), None);
    assert_eq!(t.vendor("llama-3"), None);
    let mut aliased = VendorTables::default();
    aliased
        .aliases
        .insert("haiku".into(), "claude-haiku-4-5".into());
    assert_eq!(aliased.family("Haiku"), "claude-haiku");
    let claimed = ModelId::new("claude-haiku-4-5").unwrap();
    assert!(!other_family(&t, &claimed, "claude-haiku-4-5-20251001"));
    assert!(other_family(&t, &claimed, "claude-opus-4-1"));
    assert!(other_family(&t, &claimed, "gpt-5.4"));
}

#[test]
fn hard_failures_by_family_usage_and_signature() {
    let t = VendorTables::default();
    let haiku = ModelId::new("claude-haiku-4-5").unwrap();
    let gpt = ModelId::new("gpt-5.4").unwrap();
    let live_usage: Vec<String> = [
        "billing_usage",
        "claude_cache_creation_1_h_tokens",
        "claude_cache_creation_5_m_tokens",
        "completion_tokens",
        "prompt_tokens",
        "total_tokens",
        "usage_source",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let reply = |model: &str, usage: &[String]| {
        Exchange::Answered(Reply {
            text: "1".into(),
            returned_model: Some(model.into()),
            usage_fields: usage.to_vec(),
            tool_call: None,
        })
    };
    let oc = Protocol::OpenAiChatCompletions;
    let none = CapabilityClaims::default();
    let think = CapabilityClaims {
        thinking: true,
        ..CapabilityClaims::default()
    };
    // Real relay: claude model on the OpenAI protocol with claude_* fields.
    assert!(hard_failures(
        &t,
        &haiku,
        &oc,
        &none,
        None,
        &reply("claude-haiku-4-5-20251001", &live_usage)
    )
    .is_empty());
    // A gpt model carrying claude_* fields: foreign usage.
    let h = hard_failures(&t, &gpt, &oc, &none, None, &reply("gpt-5.4", &live_usage));
    assert_eq!(h.len(), 2);
    assert!(h
        .iter()
        .all(|x| matches!(x, HardFailure::ForeignUsage { vendor, .. } if vendor == "anthropic")));
    // The protocol's own names are allowed: anthropic fields on the
    // Anthropic protocol for a gpt model are not foreign.
    let anth = vec!["cache_read_input_tokens".to_string()];
    assert!(hard_failures(
        &t,
        &gpt,
        &Protocol::AnthropicMessages,
        &none,
        None,
        &reply("gpt-5.4", &anth)
    )
    .is_empty());
    // DeepSeek-only names on a claude model, Anthropic protocol.
    let ds = vec!["prompt_cache_hit_tokens".to_string()];
    assert_eq!(
        hard_failures(
            &t,
            &haiku,
            &Protocol::AnthropicMessages,
            &none,
            None,
            &reply("claude-haiku-4-5", &ds)
        ),
        vec![HardFailure::ForeignUsage {
            field: "prompt_cache_hit_tokens".into(),
            vendor: "deepseek".into()
        }]
    );
    // Claimed opus, returned haiku: other family.
    let opus = ModelId::new("claude-opus-4-1").unwrap();
    assert_eq!(
        hard_failures(&t, &opus, &oc, &none, None, &reply("claude-haiku-4-5", &[])),
        vec![HardFailure::OtherFamily {
            returned: "claude-haiku-4-5".into()
        }]
    );
    // Thinking signature rejected: HTTP 400 on the thinking item, where it
    // applies (Anthropic protocol, thinking claimed; DECISIONS 13.5).
    let pa = Protocol::AnthropicMessages;
    let thinking = Some(CapabilityItem::ThinkingSignature);
    let e400 = Exchange::HttpError(400);
    assert_eq!(
        hard_failures(&t, &haiku, &pa, &think, thinking, &e400),
        vec![HardFailure::ThinkingSignatureRejected]
    );
    assert!(hard_failures(&t, &haiku, &pa, &think, None, &e400).is_empty());
    assert!(hard_failures(
        &t,
        &haiku,
        &pa,
        &think,
        Some(CapabilityItem::ToolCall),
        &e400
    )
    .is_empty());
    assert!(hard_failures(&t, &haiku, &pa, &think, thinking, &Exchange::RateLimited).is_empty());
    // Not applicable: no thinking claim, or the OpenAI protocol.
    assert!(hard_failures(&t, &haiku, &pa, &none, thinking, &e400).is_empty());
    assert!(hard_failures(&t, &haiku, &oc, &think, thinking, &e400).is_empty());
    // In a replay, the cap-thinking id carries the item.
    let h = "question_id\tat_ms\tlatency_ms\tkind\tstatus\tmodel\tusage_fields\ttext\ttool_name\ttool_args\n";
    let r = parse_replay(&format!("{h}cap-thinking\t1\t2\thttp_error\t400\t\t\t\t\t\ndet-01\t1\t2\thttp_error\t400\t\t\t\t\t\n")).unwrap();
    assert_eq!(
        run_hard_failures(&t, &haiku, &pa, &think, &r),
        vec![HardFailure::ThinkingSignatureRejected]
    );
}

#[test]
fn thinking_rejection_without_its_precondition_is_no_hard_failure() {
    // §11.2 第 766 行 "适用前提下": a 400 on cap-thinking under the OpenAI
    // protocol, or without a thinking claim, is not a hard failure; it
    // matches applicable_items, which leaves the item out there.
    let t = VendorTables::default();
    let haiku = ModelId::new("claude-haiku-4-5").unwrap();
    let h = "question_id\tat_ms\tlatency_ms\tkind\tstatus\tmodel\tusage_fields\ttext\ttool_name\ttool_args\n";
    let r = parse_replay(&format!(
        "{h}cap-thinking\t1\t2\thttp_error\t400\t\t\t\t\t\n"
    ))
    .unwrap();
    let think = CapabilityClaims {
        thinking: true,
        ..CapabilityClaims::default()
    };
    let oc = Protocol::OpenAiChatCompletions;
    let pa = Protocol::AnthropicMessages;
    assert!(applicable_items(&think, &oc).is_empty());
    assert!(run_hard_failures(&t, &haiku, &oc, &think, &r).is_empty());
    assert!(run_hard_failures(&t, &haiku, &oc, &CapabilityClaims::default(), &r).is_empty());
    assert!(run_hard_failures(&t, &haiku, &pa, &CapabilityClaims::default(), &r).is_empty());
    assert_eq!(
        run_hard_failures(&t, &haiku, &pa, &think, &r),
        vec![HardFailure::ThinkingSignatureRejected]
    );
}

// --------------------------------------------------- 13.7 official hosts

#[test]
fn official_direct_by_host_and_vendor() {
    let t = VendorTables::default();
    assert!(is_official_direct(
        &t,
        &arm(
            "claude-haiku-4-5",
            "api.anthropic.com",
            Protocol::AnthropicMessages
        )
    ));
    assert!(is_official_direct(
        &t,
        &arm("gpt-5.4", "API.OpenAI.com", Protocol::OpenAiResponses)
    ));
    assert!(!is_official_direct(
        &t,
        &arm("gpt-5.4", "api.anthropic.com", Protocol::AnthropicMessages)
    ));
    assert!(!is_official_direct(&t, &relay()));
    assert!(!is_official_direct(
        &t,
        &arm("llama-3", "api.openai.com", Protocol::OpenAiChatCompletions)
    ));
    assert_eq!(t.official_vendor("api.deepseek.com"), Some("deepseek"));
    assert_eq!(t.official_vendor("kapibala.asia"), None);
}

// ------------------------------------------------------- 13.3 / grades

#[test]
fn same_day_merge_and_hard_failure_days() {
    let a = relay();
    let s = summary(&a, &[(0, Det, Pass), (0, Det, Fail), (0, Det, Uncertain)]);
    assert_eq!(s.fail_days(ProbeClassKey::Deterministic), 1);
    assert_eq!(s.pass_days(ProbeClassKey::Deterministic), 0);
    let s = summary(
        &a,
        &[
            (0, Det, Uncertain),
            (0, Det, Pass),
            (0, Cap, Na),
            (0, Cap, Na),
        ],
    );
    assert_eq!(s.pass_days(ProbeClassKey::Deterministic), 1);
    assert_eq!(s.pass_days(ProbeClassKey::Capability), 1); // N/A counts (13.5)
    assert_eq!(s.pass_days(ProbeClassKey::Fingerprint), 0);
    assert!(!s.ran(ProbeClassKey::Fingerprint));
    let s = summary(&a, &[(0, Cap, Uncertain), (0, Cap, Na)]);
    assert_eq!(s.pass_days(ProbeClassKey::Capability), 0);
    // Two hard failures on one day are one day; a passive-log day adds one.
    let rs = vec![pr(&a, 0, Det, Pass, true), pr(&a, 0, Cap, Pass, true)];
    let s = DaySummary::build(&a, &rs, &[]).unwrap();
    assert_eq!(s.hard_failure_days.len(), 1);
    let d1 = RunDay::from_unix_ms(NOON0 + DAY).unwrap();
    let s = DaySummary::build(&a, &rs, &[d1]).unwrap();
    assert_eq!(s.hard_failure_days.len(), 2);
    assert_eq!(grade(&s), AuthenticityGrade::C);
    // Results from another arm are rejected.
    let other = arm(
        "claude-opus-4-1",
        "kapibala.asia",
        Protocol::OpenAiChatCompletions,
    );
    assert!(DaySummary::build(&a, &[pr(&other, 0, Det, Pass, false)], &[]).is_err());
}

#[test]
fn run_day_is_the_beijing_natural_day() {
    // 23:59:59.999 and 00:00 Beijing time fall on different run days.
    let a = relay();
    let midnight = 1_792_512_000_000 + DAY; // 2026-10-22 00:00 Beijing
    let mut r1 = pr(&a, 0, Det, Fail, false);
    r1.at_ms = midnight - 1;
    let mut r2 = pr(&a, 0, Det, Fail, false);
    r2.at_ms = midnight;
    let s = DaySummary::build(&a, &[r1.clone(), r2], &[]).unwrap();
    assert_eq!(s.fail_days(ProbeClassKey::Deterministic), 2);
    let mut r3 = pr(&a, 0, Det, Fail, false);
    r3.at_ms = 1_792_512_000_000; // 2026-10-21 00:00 Beijing, same day as r1
    let s = DaySummary::build(&a, &[r1, r3], &[]).unwrap();
    assert_eq!(s.fail_days(ProbeClassKey::Deterministic), 1);
}

#[test]
fn grade_c() {
    assert_eq!(g(&[(0, Det, Fail), (1, Det, Fail)]), AuthenticityGrade::C);
    assert_eq!(g(&[(0, Fp, Fail), (1, Fp, Fail)]), AuthenticityGrade::C);
    // C wins over A (first match).
    assert_eq!(
        g(&[
            (0, Det, Pass),
            (1, Det, Pass),
            (2, Det, Fail),
            (3, Det, Fail),
            (0, Fp, Pass),
            (1, Fp, Pass),
            (0, Cap, Pass),
            (1, Cap, Pass)
        ]),
        AuthenticityGrade::C
    );
    // One fail day is not C.
    assert_ne!(g(&[(0, Det, Fail)]), AuthenticityGrade::C);
    // Capability fails on two days: not C (only hard / det / fp count).
    assert_eq!(g(&[(0, Cap, Fail), (1, Cap, Fail)]), AuthenticityGrade::B);
}

#[test]
fn grade_a() {
    let full = [
        (0, Det, Pass),
        (1, Det, Pass),
        (0, Fp, Pass),
        (1, Fp, Pass),
        (0, Cap, Pass),
        (1, Cap, Pass),
    ];
    assert_eq!(g(&full), AuthenticityGrade::A);
    // Capability N/A on both days (13.5 assertion).
    let na = [
        (0, Det, Pass),
        (1, Det, Pass),
        (0, Fp, Pass),
        (1, Fp, Pass),
        (0, Cap, Na),
        (1, Cap, Na),
    ];
    assert_eq!(g(&na), AuthenticityGrade::A);
    // A is unchanged (13.13): a capability or deterministic fail day next
    // to two pass days does not block A. C counts only hard, deterministic
    // and fingerprint failures on >=2 days. Capability fail days do not
    // cap A or A−.
    let mut extra = full.to_vec();
    extra.extend([(2, Cap, Fail), (3, Det, Fail)]);
    assert_eq!(g(&extra), AuthenticityGrade::A);
    // One hard-failure day: neither A nor A-.
    let a = relay();
    let mut rs: Vec<ProbeResult> = full
        .iter()
        .map(|(d, c, v)| pr(&a, *d, *c, *v, false))
        .collect();
    rs[0].hard_failure = true;
    assert_eq!(
        grade(&DaySummary::build(&a, &rs, &[]).unwrap()),
        AuthenticityGrade::B
    );
}

#[test]
fn grade_a_minus() {
    // Minimal probe passed once, fingerprint not run.
    assert_eq!(
        g(&[(0, Det, Pass), (0, Cap, Pass)]),
        AuthenticityGrade::AMinus
    );
    assert_eq!(
        g(&[(0, Det, Pass), (0, Cap, Na)]),
        AuthenticityGrade::AMinus
    );
    // Fingerprint uncertain only.
    assert_eq!(
        g(&[
            (0, Det, Pass),
            (0, Cap, Pass),
            (0, Fp, Uncertain),
            (1, Fp, Uncertain)
        ]),
        AuthenticityGrade::AMinus
    );
    // Fingerprint passed exactly 1 day (even with 1 fail day).
    assert_eq!(
        g(&[(0, Det, Pass), (0, Cap, Pass), (0, Fp, Pass)]),
        AuthenticityGrade::AMinus
    );
    assert_eq!(
        g(&[(0, Det, Pass), (0, Cap, Pass), (0, Fp, Pass), (1, Fp, Fail)]),
        AuthenticityGrade::AMinus
    );
    // A needs every class on >=2 pass days. One capability pass day stays
    // A−, including when fingerprint has passed on 2 days (13.13).
    assert_eq!(
        g(&[
            (0, Det, Pass),
            (1, Det, Pass),
            (0, Cap, Pass),
            (0, Fp, Pass)
        ]),
        AuthenticityGrade::AMinus
    );
    assert_eq!(
        g(&[
            (0, Det, Pass),
            (1, Det, Pass),
            (0, Cap, Pass),
            (1, Cap, Pass),
            (0, Fp, Pass)
        ]),
        AuthenticityGrade::AMinus
    );
    assert_eq!(
        g(&[
            (0, Det, Pass),
            (1, Det, Pass),
            (0, Cap, Pass),
            (0, Fp, Pass),
            (1, Fp, Pass)
        ]),
        AuthenticityGrade::AMinus
    );
    // A hard-failure day blocks A- too.
    let a = relay();
    let rs = vec![pr(&a, 0, Det, Pass, true), pr(&a, 0, Cap, Pass, false)];
    assert_eq!(
        grade(&DaySummary::build(&a, &rs, &[]).unwrap()),
        AuthenticityGrade::B
    );
    // Deterministic passes on 2 days but capability never: B.
    assert_eq!(g(&[(0, Det, Pass), (1, Det, Pass)]), AuthenticityGrade::B);
}

#[test]
fn grade_b_fingerprint_fail_day_and_thin_evidence() {
    // Fingerprint failed on 1 day and never passed: still B, not yet C
    // (13.13). Two fingerprint pass days with one deterministic and one
    // capability pass day are A−; see
    // `grade_stronger_fingerprint_stays_a_minus`.
    assert_eq!(
        g(&[(0, Det, Pass), (0, Cap, Pass), (0, Fp, Fail)]),
        AuthenticityGrade::B
    );
    // Nothing ran.
    assert_eq!(g(&[]), AuthenticityGrade::B);
    assert_eq!(
        g(&[(0, Det, Uncertain), (0, Cap, Pass)]),
        AuthenticityGrade::B
    );
}

#[test]
fn grade_stronger_fingerprint_stays_a_minus() {
    // DECISIONS 13.13: with no hard failure, extra fingerprint pass days
    // must not drop A− to B. A still needs >=2 pass days in every class.
    let days = |n: i64, c: ProbeClass| -> Vec<(i64, ProbeClass, ProbeVerdict)> {
        (0..n).map(|d| (d, c, Pass)).collect()
    };
    // Exactly one fingerprint pass day is still A−.
    for (det, cap) in [(1, 1), (1, 2), (2, 1)] {
        let mut fp1: Vec<_> = days(det, Det).into_iter().chain(days(cap, Cap)).collect();
        fp1.extend(days(1, Fp));
        assert_eq!(
            g(&fp1),
            AuthenticityGrade::AMinus,
            "det {det}, cap {cap}, fp 1"
        );
    }
    // The three combinations the literal reading graded B.
    for (det, cap, fp) in [
        (1, 1, 2),
        (1, 1, 3),
        (1, 2, 2),
        (1, 3, 2),
        (1, 2, 3),
        (2, 1, 2),
        (3, 1, 2),
        (2, 1, 3),
    ] {
        let mut more: Vec<_> = days(det, Det).into_iter().chain(days(cap, Cap)).collect();
        more.extend(days(fp, Fp));
        assert_eq!(
            g(&more),
            AuthenticityGrade::AMinus,
            "det {det}, cap {cap}, fp {fp}"
        );
    }
    // With >=2 pass days in every class, the grade is A.
    let mut full = days(2, Det);
    full.extend(days(2, Cap));
    full.extend(days(2, Fp));
    assert_eq!(g(&full), AuthenticityGrade::A);
}

// ------------------------------------------------- 13.8 observation

#[test]
fn observation_validation_criteria() {
    let off = arm(
        "claude-haiku-4-5",
        "api.anthropic.com",
        Protocol::AnthropicMessages,
    );
    let small = arm(
        "claude-haiku-4-5",
        "control.local",
        Protocol::AnthropicMessages,
    );
    let pos = summary(
        &off,
        &[(0, Det, Pass), (1, Det, Pass), (0, Cap, Pass), (1, Cap, Na)],
    );
    let neg = summary(&small, &[(0, Fp, Fail), (1, Fp, Fail)]);
    assert_eq!(observation(&pos, &neg, false, true), Observation::Validated);
    // Negative control without a baseline cannot validate.
    assert_eq!(observation(&pos, &neg, false, false), Observation::Pending);
    // Negative control only uncertain: not validated.
    let neg_u = summary(&small, &[(0, Fp, Uncertain), (1, Fp, Fail)]);
    assert_eq!(observation(&pos, &neg_u, false, true), Observation::Pending);
    // Positive control with a baseline must also pass the fingerprint twice.
    assert_eq!(observation(&pos, &neg, true, true), Observation::Pending);
    let pos_fp = summary(
        &off,
        &[
            (0, Det, Pass),
            (1, Det, Pass),
            (0, Cap, Pass),
            (1, Cap, Pass),
            (0, Fp, Pass),
            (1, Fp, Pass),
        ],
    );
    assert_eq!(
        observation(&pos_fp, &neg, true, true),
        Observation::Validated
    );
    // Positive control with a hard failure, or only one pass day.
    let pos1 = summary(&off, &[(0, Det, Pass), (0, Cap, Pass), (1, Cap, Pass)]);
    assert_eq!(observation(&pos1, &neg, false, true), Observation::Pending);
    let pos_cap1 = summary(&off, &[(0, Det, Pass), (1, Det, Pass), (0, Cap, Pass)]);
    assert_eq!(
        observation(&pos_cap1, &neg, false, true),
        Observation::Pending
    );
    let rs = vec![
        pr(&off, 0, Det, Pass, true),
        pr(&off, 1, Det, Pass, false),
        pr(&off, 0, Cap, Pass, false),
        pr(&off, 1, Cap, Pass, false),
    ];
    let pos_h = DaySummary::build(&off, &rs, &[]).unwrap();
    assert_eq!(observation(&pos_h, &neg, false, true), Observation::Pending);
}

#[test]
fn positive_control_with_a_baseline_needs_two_fingerprint_pass_days() {
    // DECISIONS 13.8: with a baseline the official route must pass the
    // fingerprint on ≥2 run days; exactly 1 is not enough.
    let off = arm(
        "claude-haiku-4-5",
        "api.anthropic.com",
        Protocol::AnthropicMessages,
    );
    let small = arm(
        "claude-haiku-4-5",
        "control.local",
        Protocol::AnthropicMessages,
    );
    let neg = summary(&small, &[(0, Fp, Fail), (1, Fp, Fail)]);
    let base = [
        (0, Det, Pass),
        (1, Det, Pass),
        (0, Cap, Pass),
        (1, Cap, Pass),
    ];
    let with = |fp: &[(i64, ProbeClass, ProbeVerdict)]| {
        let mut v = base.to_vec();
        v.extend_from_slice(fp);
        observation(&summary(&off, &v), &neg, true, true)
    };
    assert_eq!(with(&[(0, Fp, Pass)]), Observation::Pending);
    assert_eq!(
        with(&[(0, Fp, Pass), (1, Fp, Uncertain)]),
        Observation::Pending
    );
    assert_eq!(
        with(&[(0, Fp, Pass), (1, Fp, Pass)]),
        Observation::Validated
    );
    // Without a baseline the fingerprint is not required.
    assert_eq!(
        observation(&summary(&off, &base), &neg, false, true),
        Observation::Validated
    );
}

#[test]
fn not_applicable_is_for_the_capability_class_only() {
    // DECISIONS 13.5: only capability can be not applicable. Deterministic
    // and fingerprint results marked N/A are rejected where they enter.
    let a = relay();
    for class in [Det, Fp] {
        let p = pr(&a, 0, class, Na, false);
        assert!(p.validate().is_err(), "{class:?}");
        assert!(DaySummary::build(&a, &[p], &[]).is_err(), "{class:?}");
    }
    let p = pr(&a, 0, Cap, Na, false);
    p.validate().unwrap();
    let s = DaySummary::build(&a, &[p], &[]).unwrap();
    assert_eq!(s.pass_days(ProbeClassKey::Capability), 1);
    // A summary assembled by hand: N/A is a pass day for capability only.
    let mut s = DaySummary::default();
    for (i, k) in [
        ProbeClassKey::Deterministic,
        ProbeClassKey::Fingerprint,
        ProbeClassKey::Capability,
    ]
    .into_iter()
    .enumerate()
    {
        let day = RunDay::from_unix_ms(NOON0 + i as i64 * DAY).unwrap();
        s.verdicts.insert((k, day), Na);
    }
    assert_eq!(s.pass_days(ProbeClassKey::Deterministic), 0);
    assert_eq!(s.pass_days(ProbeClassKey::Fingerprint), 0);
    assert_eq!(s.pass_days(ProbeClassKey::Capability), 1);
}

// ------------------------------------------- §5 rule 4 gate, 13.11 mapping

fn ctx(probes_enabled: bool, validated: bool) -> GateContext {
    GateContext {
        probes_enabled,
        observation: if validated {
            Observation::Validated
        } else {
            Observation::Pending
        },
    }
}

fn results(a: &Arm, rs: &[(i64, ProbeClass, ProbeVerdict)]) -> Vec<ProbeResult> {
    rs.iter()
        .map(|(d, c, v)| pr(a, *d, *c, *v, false))
        .collect()
}

fn cell(s: &str) -> CellId {
    CellId::user(s).unwrap()
}

/// A layer that passes every other §5 gate (as in step 8's tests).
fn eligibility_with(grade: AuthenticityGrade) -> agent_value_core::eligibility::Eligibility {
    let ids = ["a", "b", "c"];
    let w = CellWeights::new(ids.iter().map(|s| (cell(s), 1.0 / 3.0)).collect()).unwrap();
    let inputs: Vec<CellInput> = ids
        .iter()
        .map(|id| CellInput {
            cell: cell(id),
            counts: CellCounts {
                n1: 10,
                n0: 0,
                n_c: 0,
                m: 0,
            },
            prior: Prior::Jeffreys,
        })
        .collect();
    let st = layer_status(&w, &inputs, &RetryModel::with_default_k(0.0).unwrap()).unwrap();
    let reference: Vec<(CellId, Triple)> = w
        .support()
        .map(|c| (c.clone(), Triple::new(1.0, 10.0, 0.9)))
        .collect();
    assess_eligibility(&st, &w, grade, true, &reference).unwrap()
}

#[test]
fn gate_after_validation() {
    let t = VendorTables::default();
    let a = relay();
    let full = results(
        &a,
        &[
            (0, Det, Pass),
            (1, Det, Pass),
            (0, Fp, Pass),
            (1, Fp, Pass),
            (0, Cap, Pass),
            (1, Cap, Pass),
        ],
    );
    let x = assess(&t, &a, &full, &[], ctx(true, true)).unwrap();
    assert_eq!(x.status, AuthStatus::Graded(AuthenticityGrade::A));
    assert_eq!((x.passes, x.undetermined), (true, false));
    assert!(x.labels.is_empty());
    assert_eq!(x.gate_input(), AuthenticityGrade::A);

    let am = results(&a, &[(0, Det, Pass), (0, Cap, Pass)]);
    let x = assess(&t, &a, &am, &[], ctx(true, true)).unwrap();
    assert_eq!(x.status, AuthStatus::Graded(AuthenticityGrade::AMinus));
    assert_eq!((x.passes, x.undetermined), (true, false));

    let c = results(&a, &[(0, Det, Fail), (1, Det, Fail)]);
    let x = assess(&t, &a, &c, &[], ctx(true, true)).unwrap();
    assert_eq!(x.status, AuthStatus::Graded(AuthenticityGrade::C));
    assert!(!x.passes);
    assert_eq!(x.labels, vec![AuthLabel::SuspectedInconsistent]);
    assert_eq!(x.gate_input(), AuthenticityGrade::C);
    let e = eligibility_with(x.gate_input());
    assert_eq!(e.failed, vec![EligibilityGate::Authenticity]);

    // B with probes on: fails (minimal probe not passed yet).
    let x = assess(&t, &a, &[], &[], ctx(true, true)).unwrap();
    assert_eq!(x.status, AuthStatus::Graded(AuthenticityGrade::B));
    assert_eq!((x.passes, x.undetermined), (false, true));
    assert_eq!(x.labels, vec![AuthLabel::Unverified]);
    let e = eligibility_with(x.gate_input());
    assert!(!e.eligible);
    assert_eq!(e.failed, vec![EligibilityGate::Authenticity]);

    // B with probes off: passes, labelled "未验证（探针已关闭）".
    let x = assess(&t, &a, &[], &[], ctx(false, true)).unwrap();
    assert_eq!((x.passes, x.undetermined), (true, true));
    assert_eq!(x.labels, vec![AuthLabel::UnverifiedProbesOff]);
    assert_eq!(x.gate_input(), AuthenticityGrade::B);
    let e = eligibility_with(x.gate_input());
    assert!(e.eligible);
    assert!(e.authenticity_undetermined);
}

#[test]
fn gate_during_observation_is_display_only() {
    let t = VendorTables::default();
    let a = relay();
    let c = results(&a, &[(0, Det, Fail), (1, Det, Fail)]);
    let x = assess(&t, &a, &c, &[], ctx(true, false)).unwrap();
    assert_eq!(x.status, AuthStatus::Graded(AuthenticityGrade::C));
    assert_eq!((x.passes, x.undetermined), (true, true));
    assert_eq!(
        x.labels,
        vec![AuthLabel::SuspectedInconsistent, AuthLabel::ObservationOnly]
    );
    assert!(eligibility_with(x.gate_input()).eligible);
    // B with probes on also passes during observation.
    let x = assess(&t, &a, &[], &[], ctx(true, false)).unwrap();
    assert_eq!((x.passes, x.undetermined), (true, true));
    assert_eq!(
        x.labels,
        vec![AuthLabel::Unverified, AuthLabel::ObservationOnly]
    );
    // A is shown as A but undetermined during observation.
    let full = results(
        &a,
        &[
            (0, Det, Pass),
            (1, Det, Pass),
            (0, Fp, Pass),
            (1, Fp, Pass),
            (0, Cap, Pass),
            (1, Cap, Pass),
        ],
    );
    let x = assess(&t, &a, &full, &[], ctx(true, false)).unwrap();
    assert_eq!(x.status, AuthStatus::Graded(AuthenticityGrade::A));
    assert_eq!((x.passes, x.undetermined), (true, true));
    assert_eq!(x.labels, vec![AuthLabel::ObservationOnly]);
}

#[test]
fn official_direct_is_not_graded() {
    let t = VendorTables::default();
    let a = arm(
        "claude-haiku-4-5",
        "api.anthropic.com",
        Protocol::AnthropicMessages,
    );
    let rs = vec![pr(&a, 0, Det, Fail, true), pr(&a, 1, Det, Fail, true)];
    for c in [ctx(true, true), ctx(true, false), ctx(false, true)] {
        let x = assess(&t, &a, &rs, &[], c).unwrap();
        assert_eq!(x.status, AuthStatus::OfficialDirect);
        assert_eq!((x.passes, x.undetermined), (true, false));
        assert_eq!(
            x.labels,
            vec![AuthLabel::OfficialDirect, AuthLabel::OfficialRouteAnomaly]
        );
        assert_eq!(x.gate_input(), AuthenticityGrade::A);
    }
    let x = assess(&t, &a, &[], &[], ctx(true, true)).unwrap();
    assert_eq!(x.labels, vec![AuthLabel::OfficialDirect]);
}

#[test]
fn probe_results_never_change_anything_but_the_authenticity_gate() {
    // §11.5: the grade is a gate only. Same layer, grades A and C: the only
    // difference is the Authenticity gate.
    let a = eligibility_with(AuthenticityGrade::A);
    let c = eligibility_with(AuthenticityGrade::C);
    assert!(a.eligible && a.failed.is_empty());
    assert_eq!(c.failed, vec![EligibilityGate::Authenticity]);
    assert_eq!(a.authenticity_undetermined, c.authenticity_undetermined);
}

// ------------------------------------------------ 13.9 log integrity

fn logged(ok: usize, other: &[LoggedModel]) -> Vec<LoggedModel> {
    let mut v = vec![LoggedModel::Returned("claude-haiku-4-5-20251001".into()); ok];
    v.extend(other.iter().cloned());
    v
}

#[test]
fn log_integrity_parse_rate() {
    let t = VendorTables::default();
    let a = relay();
    let li = log_integrity(&t, &a, &logged(19, &[LoggedModel::Unparseable])).unwrap();
    assert!(li.passes);
    assert_eq!((li.requests, li.parseable), (20, 19));
    let li = log_integrity(
        &t,
        &a,
        &logged(18, &[LoggedModel::Unparseable, LoggedModel::Unparseable]),
    )
    .unwrap();
    assert!(!li.passes);
    let li = log_integrity(&t, &a, &[]).unwrap();
    assert!(li.passes && li.no_requests);
}

#[test]
fn log_integrity_version_drift_and_other_family() {
    let t = VendorTables::default();
    let a = relay();
    let v = LoggedModel::Returned("claude-haiku-4-6".into());
    let li = log_integrity(&t, &a, &logged(19, std::slice::from_ref(&v))).unwrap();
    assert_eq!(li.version_mismatch, 1);
    assert!(li.passes); // 1/20 <= 0.05
    let li = log_integrity(&t, &a, &logged(18, &[v.clone(), v.clone()])).unwrap();
    assert!(!li.passes); // 2/20 > 0.05
                         // Other family is a hard failure, not counted here.
    let o = LoggedModel::Returned("gpt-5.4".into());
    let li = log_integrity(&t, &a, &logged(18, &[o.clone(), o])).unwrap();
    assert_eq!((li.other_family, li.version_mismatch), (2, 0));
    assert!(li.passes);
    // The drift denominator leaves out other-family and missing-model
    // requests: 1 mismatch among 19 comparable requests is above 0.05.
    let v1 = LoggedModel::Returned("claude-haiku-4-6".into());
    let li = log_integrity(
        &t,
        &a,
        &logged(18, &[v1.clone(), LoggedModel::Returned("gpt-5.4".into())]),
    )
    .unwrap();
    assert!(!li.passes);
    let li = log_integrity(&t, &a, &logged(18, &[v1, LoggedModel::Missing])).unwrap();
    assert!(!li.passes);
    // Only missing models: nothing to compare, passes with the flag.
    let li = log_integrity(&t, &a, &vec![LoggedModel::Missing; 3]).unwrap();
    assert!(li.passes && li.cannot_verify_model);
}

#[test]
fn log_integrity_missing_model_flag() {
    let t = VendorTables::default();
    let a = relay();
    let li = log_integrity(&t, &a, &logged(79, &vec![LoggedModel::Missing; 21])).unwrap();
    assert!(li.cannot_verify_model && li.passes);
    let li = log_integrity(&t, &a, &logged(80, &vec![LoggedModel::Missing; 20])).unwrap();
    assert!(!li.cannot_verify_model && li.passes);
    assert_eq!(li.missing, 20);
}

// ------------------------------------------------------ 13.10 budget

#[test]
fn probe_budget_boundaries() {
    assert_eq!(PROBE_MONTHLY_BUDGET_YUAN, 10.0);
    let y = |x: f64| MicroYuan::from_yuan(x).unwrap();
    let b = y(PROBE_MONTHLY_BUDGET_YUAN);
    assert_eq!(
        probe_budget(y(9.99), y(0.01), b).unwrap(),
        BudgetDecision::Allow
    );
    assert_eq!(
        probe_budget(y(9.99), y(0.02), b).unwrap(),
        BudgetDecision::Stop
    );
    assert_eq!(
        probe_budget(y(10.0), y(0.0), b).unwrap(),
        BudgetDecision::Stop
    );
    assert_eq!(
        probe_budget(y(0.0), y(10.0), b).unwrap(),
        BudgetDecision::Allow
    );
    assert!(probe_budget(y(1.0), MicroYuan::new(i64::MAX).unwrap(), b).is_err());
}

#[test]
fn budget_month_is_beijing_calendar_month() {
    // 2026-10-31 16:00 UTC = 2026-11-01 00:00 Beijing.
    let nov1 = 1_792_512_000_000 + 11 * DAY;
    assert_eq!(budget_month(nov1).unwrap(), (2026, 11));
    assert_eq!(budget_month(nov1 - 1).unwrap(), (2026, 10));
}

// ---------------------------------------------------- 13.12 overhead

fn ts(start: i64, ttft: i64, total: i64, tokens: u64) -> TimingSample {
    TimingSample {
        start_ms: start,
        first_token_ms: Some(start + ttft),
        end_ms: start + total,
        output_tokens: tokens,
    }
}

#[test]
fn overhead_window_and_medians() {
    let t0 = NOON0;
    let route = vec![
        ts(t0, 500, 2500, 100),
        ts(t0 + 10, 700, 3000, 100),
        ts(t0 + 20, 900, 4000, 100),
    ];
    // Official: one exactly 1 h after the last route sample (in), one 1 h
    // + 1 ms after it (out, would change every median).
    let official = vec![
        ts(t0 + 20 + 3_600_000, 200, 1200, 100),
        ts(t0 - 3_600_000, 400, 1400, 100),
        ts(t0 + 20 + 3_600_001, 99_000, 99_500, 1),
    ];
    let o = route_overhead(&route, &official).unwrap();
    // Route medians (odd): ttft 700, total 3000, speed 100/2.3 s.
    // Official medians (even, two samples): ttft 300, total 1300, speed 100 t/s.
    assert_eq!(o.ttft_ms, Some(400.0));
    assert_eq!(o.total_ms, Some(1700.0));
    let speed = 100.0 * 1000.0 / 2300.0 - 100.0;
    assert!((o.speed_tps.unwrap() - speed).abs() < 1e-9);
    // No official sample in the window: no data.
    let far = vec![ts(t0 + 3_600_021, 1, 2, 1)];
    let o = route_overhead(&route, &far).unwrap();
    assert_eq!((o.ttft_ms, o.total_ms, o.speed_tps), (None, None, None));
    // Unordered times are rejected.
    let bad = TimingSample {
        start_ms: t0,
        first_token_ms: Some(t0 - 1),
        end_ms: t0 + 1,
        output_tokens: 1,
    };
    assert!(route_overhead(&[bad], &official).is_err());
    // Extreme timestamps: the difference is i64::MIN, out of the window,
    // no overflow.
    let extreme = TimingSample {
        start_ms: i64::MIN,
        first_token_ms: None,
        end_ms: i64::MIN + 10,
        output_tokens: 1,
    };
    let at0 = TimingSample {
        start_ms: 0,
        first_token_ms: None,
        end_ms: 10,
        output_tokens: 1,
    };
    let o = route_overhead(&[extreme], &[at0]).unwrap();
    assert_eq!(o.total_ms, None);
    // First token at the end: not counted in the speed.
    let flat = vec![ts(t0, 100, 100, 5)];
    let o = route_overhead(&flat, &official[..2]).unwrap();
    assert_eq!(o.speed_tps, None);
    // Pairing is against this route's samples: only the official sample
    // 1 h before t0 is in the window now (ttft 400).
    assert_eq!(o.ttft_ms, Some(100.0 - 400.0));
}

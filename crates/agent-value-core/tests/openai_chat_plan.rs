//! `build_openai_chat_plan`: row ids and the JSONL shape `probe_plan`
//! prints. The fixtures are that example's stdout for model `probe-model`
//! and claims `tool,json` (seeds 20261022 and 7), frozen so
//! `tools/probe_live.py` keeps seeing the same objects. No network.

use agent_value_core::error::CoreError;
use agent_value_core::probe::{
    build_openai_chat_plan, capability_number, deterministic_set, CapabilityClaims, OpenAiChatBody,
    OpenAiChatMessage, OpenAiChatPlanRow, OpenAiProbeJsonSchema, OpenAiProbeTool, PROBE_MAX_TOKENS,
    PROBE_SYSTEM_PROMPT, PROBE_TEMPERATURE, TOOL_NAME,
};

const BOTH_20261022: &str = include_str!("fixtures/openai_chat_plan_20261022.jsonl");
const BOTH_7: &str = include_str!("fixtures/openai_chat_plan_7.jsonl");

fn claims(tool: bool, json: bool) -> CapabilityClaims {
    CapabilityClaims {
        tool_call: tool,
        json_schema: json,
        ..CapabilityClaims::default()
    }
}

fn render(rows: &[OpenAiChatPlanRow]) -> String {
    let mut s = String::new();
    for row in rows {
        s.push_str(&row.to_json_line().unwrap());
        s.push('\n');
    }
    s
}

fn line(row: &OpenAiChatPlanRow) -> String {
    row.to_json_line().unwrap()
}

fn ids(seed: u64, tool: bool, json: bool) -> Vec<String> {
    build_openai_chat_plan(seed, "probe-model", &claims(tool, json))
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect()
}

#[test]
fn plan_matches_probe_plan_fixtures() {
    let rows = build_openai_chat_plan(20261022, "probe-model", &claims(true, true)).unwrap();
    assert_eq!(render(&rows), BOTH_20261022);
    let rows7 = build_openai_chat_plan(7, "probe-model", &claims(true, true)).unwrap();
    assert_eq!(render(&rows7), BOTH_7);
    for line in BOTH_20261022.lines().chain(BOTH_7.lines()) {
        assert!(line.starts_with("{\"id\":"));
        assert!(line.contains(",\"body\":{"));
        assert!(line.contains("\"temperature\":0,"));
        assert!(line.contains("\"max_tokens\":64"));
        assert!(!line.contains("\"expected\""));
    }
}

#[test]
fn row_count_and_ids_follow_claims() {
    let det: Vec<String> = (1..=10).map(|n| format!("det-{n:02}")).collect();
    assert_eq!(ids(20261022, false, false), det);
    assert_eq!(ids(7, false, false).len(), 10);

    let mut tool = det.clone();
    tool.push("cap-tool".into());
    assert_eq!(ids(20261022, true, false), tool);

    let mut json = det.clone();
    json.push("cap-json".into());
    assert_eq!(ids(20261022, false, true), json);

    let mut both = det;
    both.push("cap-tool".into());
    both.push("cap-json".into());
    assert_eq!(ids(20261022, true, true), both);
    assert_eq!(ids(7, true, true), both);
}

#[test]
fn claim_subsets_reuse_the_full_plan_lines() {
    let full = build_openai_chat_plan(20261022, "probe-model", &claims(true, true)).unwrap();
    let lines: Vec<String> = full.iter().map(line).collect();
    assert_eq!(lines.len(), 12);

    let none = build_openai_chat_plan(20261022, "probe-model", &claims(false, false)).unwrap();
    assert_eq!(none.iter().map(line).collect::<Vec<_>>(), lines[..10]);

    let tool = build_openai_chat_plan(20261022, "probe-model", &claims(true, false)).unwrap();
    let mut tool_lines = lines[..10].to_vec();
    tool_lines.push(lines[10].clone());
    assert_eq!(tool.iter().map(line).collect::<Vec<_>>(), tool_lines);

    let json = build_openai_chat_plan(20261022, "probe-model", &claims(false, true)).unwrap();
    let mut json_lines = lines[..10].to_vec();
    json_lines.push(lines[11].clone());
    assert_eq!(json.iter().map(line).collect::<Vec<_>>(), json_lines);
}

#[test]
fn rows_use_deterministic_prompts_and_fixed_sampling() {
    let seed = 20261022u64;
    let set = deterministic_set(seed).unwrap();
    let rows = build_openai_chat_plan(seed, "probe-model", &claims(true, true)).unwrap();
    for (row, q) in rows.iter().take(10).zip(&set) {
        assert_eq!(row.id, q.id);
        assert_eq!(row.body.model, "probe-model");
        assert_eq!(row.body.temperature, PROBE_TEMPERATURE);
        assert_eq!(row.body.max_tokens, PROBE_MAX_TOKENS);
        assert_eq!(row.body.messages.len(), 2);
        assert_eq!(row.body.messages[0].role, "system");
        assert_eq!(row.body.messages[0].content, PROBE_SYSTEM_PROMPT);
        assert_eq!(row.body.messages[1].role, "user");
        assert_eq!(row.body.messages[1].content, q.prompt);
        assert!(row.body.tool().is_none());
        assert!(row.body.json_schema().is_none());
    }
    let n = capability_number(seed);
    let tool = &rows[10];
    assert_eq!(tool.id, "cap-tool");
    assert_eq!(
        tool.body.messages[1].content,
        format!("Call {TOOL_NAME} with n = {n}.")
    );
    let t = tool.body.tool().unwrap();
    assert_eq!(t.name, TOOL_NAME);
    assert_eq!(t.description, "Record one integer.");
    assert_eq!(t.argument, "n");
    assert_eq!(t.tool_choice, "required");
    assert!(tool.body.json_schema().is_none());

    let js = &rows[11];
    assert_eq!(js.id, "cap-json");
    assert_eq!(
        js.body.messages[1].content,
        format!("Return a JSON object whose answer field is {n}.")
    );
    assert!(js.body.tool().is_none());
    let schema = js.body.json_schema().unwrap();
    assert_eq!(
        (schema.name.as_str(), schema.field.as_str()),
        ("answer", "answer")
    );
}

#[test]
fn thinking_and_long_context_claims_add_no_rows() {
    let c = CapabilityClaims {
        thinking: true,
        long_context_due: true,
        ..CapabilityClaims::default()
    };
    let rows = build_openai_chat_plan(7, "probe-model", &c).unwrap();
    assert_eq!(rows.len(), 10);
    assert!(rows.iter().all(|r| r.id.starts_with("det-")));
}

#[test]
fn thinking_and_long_context_stay_out_when_tool_or_json_are_claimed() {
    let with = |tool: bool, json: bool| CapabilityClaims {
        tool_call: tool,
        json_schema: json,
        thinking: true,
        long_context_due: true,
    };
    let tool_rows = build_openai_chat_plan(20261022, "probe-model", &with(true, false)).unwrap();
    let json_rows = build_openai_chat_plan(20261022, "probe-model", &with(false, true)).unwrap();
    let both_rows = build_openai_chat_plan(20261022, "probe-model", &with(true, true)).unwrap();
    assert_eq!(render(&both_rows), BOTH_20261022);
    assert_eq!(
        render(&tool_rows),
        render(&build_openai_chat_plan(20261022, "probe-model", &claims(true, false)).unwrap())
    );
    assert_eq!(
        render(&json_rows),
        render(&build_openai_chat_plan(20261022, "probe-model", &claims(false, true)).unwrap())
    );
    for rows in [&tool_rows, &json_rows, &both_rows] {
        assert!(rows.iter().any(|r| r.id.starts_with("det-")));
        for row in rows {
            assert!(
                row.id.starts_with("det-") || row.id == "cap-tool" || row.id == "cap-json",
                "{}",
                row.id
            );
            let text = line(row);
            assert!(!text.contains("cap-thinking"));
            assert!(!text.contains("cap-long"));
            let has_tool = text.contains("\"tools\"");
            let has_schema = text.contains("\"response_format\"");
            assert!(!(has_tool && has_schema), "{text}");
        }
    }
}

#[test]
fn try_new_rejects_tool_and_response_format_together() {
    let err = OpenAiChatBody::try_new(
        "probe-model".into(),
        vec![OpenAiChatMessage {
            role: "user".into(),
            content: "n".into(),
        }],
        PROBE_TEMPERATURE,
        PROBE_MAX_TOKENS,
        Some(OpenAiProbeTool {
            name: TOOL_NAME.into(),
            description: "Record one integer.".into(),
            argument: "n".into(),
            tool_choice: "required".into(),
        }),
        Some(OpenAiProbeJsonSchema {
            name: "answer".into(),
            field: "answer".into(),
        }),
    )
    .unwrap_err();
    assert!(matches!(
        err,
        CoreError::InvalidField {
            field: "openai chat body",
            reason: "tool and response_format are mutually exclusive",
        }
    ));
}

#[test]
fn seed_zero_is_rejected() {
    let err = build_openai_chat_plan(0, "probe-model", &claims(true, true)).unwrap_err();
    assert!(matches!(
        err,
        CoreError::InvalidField {
            field: "probe seed",
            ..
        }
    ));
}

#[test]
fn repeated_calls_match_and_model_text_is_preserved() {
    let c = claims(true, true);
    assert_eq!(
        build_openai_chat_plan(42, "probe-model", &c).unwrap(),
        build_openai_chat_plan(42, "probe-model", &c).unwrap()
    );
    let raw = "A\"B\\C\n\t\r café";
    let rows = build_openai_chat_plan(1, raw, &claims(false, false)).unwrap();
    assert_eq!(rows[0].body.model, raw);
    let line = line(&rows[0]);
    assert!(line.contains(r#""model":"A\"B\\C\n\t\u000d café""#));
    assert!(!line.contains("\"temperature\":0.0"));
}

//! Prints the minimal probe (§11.1: deterministic class + capability class)
//! as OpenAI Chat Completions request bodies, one JSON object per line:
//! `{"id": ..., "body": {...}}`. No network, no key: the local runner
//! (`tools/probe_live.py`) sends them. Expected answers are not printed.
//!
//! Only the capability items the route claims are printed (§11.2):
//! `<claims>` is a comma-separated subset of `tool,json` (default both;
//! `none` for neither). Thinking and long context are not part of this
//! OpenAI Chat Completions plan.
//!
//! Usage: `cargo run -p agent-value-core --example probe_plan -- <seed> <model> [claims]`

use agent_value_core::arm::Protocol;
use agent_value_core::probe::{
    applicable_items, capability_number, deterministic_set, CapabilityClaims, CapabilityItem,
    PROBE_MAX_TOKENS, PROBE_SYSTEM_PROMPT, PROBE_TEMPERATURE, TOOL_NAME,
};

/// Parses `tool,json` / `none` into claims (shared wording with probe_judge).
fn claims(arg: Option<&String>) -> CapabilityClaims {
    let s = arg.map_or("tool,json", String::as_str);
    let mut c = CapabilityClaims::default();
    for part in s.split(',').filter(|p| !p.is_empty() && *p != "none") {
        match part {
            "tool" => c.tool_call = true,
            "json" => c.json_schema = true,
            other => {
                eprintln!("unknown claim {other:?} (use tool, json or none)");
                std::process::exit(2);
            }
        }
    }
    c
}

fn json_str(s: &str) -> String {
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

fn body(model: &str, user: &str, extra: &str) -> String {
    format!(
        "{{\"model\":{},\"messages\":[{{\"role\":\"system\",\"content\":{}}},{{\"role\":\"user\",\"content\":{}}}],\"temperature\":{},\"max_tokens\":{}{}}}",
        json_str(model),
        json_str(PROBE_SYSTEM_PROMPT),
        json_str(user),
        PROBE_TEMPERATURE,
        PROBE_MAX_TOKENS,
        extra
    )
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 && args.len() != 4 {
        eprintln!("usage: probe_plan <seed> <model> [claims]");
        std::process::exit(2);
    }
    let seed: u64 = args[1].parse().expect("seed must be an integer");
    let model = &args[2];
    for q in deterministic_set(seed).expect("seed must be non-zero") {
        println!(
            "{{\"id\":{},\"body\":{}}}",
            json_str(&q.id),
            body(model, &q.prompt, "")
        );
    }
    let items = applicable_items(&claims(args.get(3)), &Protocol::OpenAiChatCompletions);
    let n = capability_number(seed);
    let tool = format!(
        ",\"tools\":[{{\"type\":\"function\",\"function\":{{\"name\":\"{TOOL_NAME}\",\"description\":\"Record one integer.\",\"parameters\":{{\"type\":\"object\",\"properties\":{{\"n\":{{\"type\":\"integer\"}}}},\"required\":[\"n\"],\"additionalProperties\":false}}}}}}],\"tool_choice\":\"required\""
    );
    if items.contains(&CapabilityItem::ToolCall) {
        println!(
            "{{\"id\":\"cap-tool\",\"body\":{}}}",
            body(model, &format!("Call {TOOL_NAME} with n = {n}."), &tool)
        );
    }
    let schema = ",\"response_format\":{\"type\":\"json_schema\",\"json_schema\":{\"name\":\"answer\",\"strict\":true,\"schema\":{\"type\":\"object\",\"properties\":{\"answer\":{\"type\":\"integer\"}},\"required\":[\"answer\"],\"additionalProperties\":false}}}";
    if items.contains(&CapabilityItem::JsonSchema) {
        println!(
            "{{\"id\":\"cap-json\",\"body\":{}}}",
            body(
                model,
                &format!("Return a JSON object whose answer field is {n}."),
                schema
            )
        );
    }
}

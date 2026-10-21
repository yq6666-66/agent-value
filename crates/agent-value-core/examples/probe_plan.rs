//! Prints the minimal probe (§11.1: deterministic class + capability class)
//! as OpenAI Chat Completions request bodies, one JSON object per line:
//! `{"id": ..., "body": {...}}`. No network, no key: the local runner
//! (`tools/probe_live.py`) sends them. Expected answers are not printed.
//!
//! Usage: `cargo run -p agent-value-core --example probe_plan -- <seed> <model>`

use agent_value_core::probe::{
    capability_number, deterministic_set, PROBE_MAX_TOKENS, PROBE_SYSTEM_PROMPT, PROBE_TEMPERATURE,
    TOOL_NAME,
};

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
    if args.len() != 3 {
        eprintln!("usage: probe_plan <seed> <model>");
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
    let n = capability_number(seed);
    let tool = format!(
        ",\"tools\":[{{\"type\":\"function\",\"function\":{{\"name\":\"{TOOL_NAME}\",\"description\":\"Record one integer.\",\"parameters\":{{\"type\":\"object\",\"properties\":{{\"n\":{{\"type\":\"integer\"}}}},\"required\":[\"n\"],\"additionalProperties\":false}}}}}}],\"tool_choice\":\"required\""
    );
    println!(
        "{{\"id\":\"cap-tool\",\"body\":{}}}",
        body(model, &format!("Call {TOOL_NAME} with n = {n}."), &tool)
    );
    let schema = ",\"response_format\":{\"type\":\"json_schema\",\"json_schema\":{\"name\":\"answer\",\"strict\":true,\"schema\":{\"type\":\"object\",\"properties\":{\"answer\":{\"type\":\"integer\"}},\"required\":[\"answer\"],\"additionalProperties\":false}}}";
    println!(
        "{{\"id\":\"cap-json\",\"body\":{}}}",
        body(
            model,
            &format!("Return a JSON object whose answer field is {n}."),
            schema
        )
    );
}

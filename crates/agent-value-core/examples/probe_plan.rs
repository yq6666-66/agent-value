//! Prints the minimal probe (§11.1: deterministic class + capability class)
//! as OpenAI Chat Completions request bodies, one JSON object per line:
//! `{"id": ..., "body": {...}}`. No network, no key: the local runner
//! (`tools/probe_live.py`) sends them. Expected answers are not printed.
//!
//! The bodies come from `probe::build_openai_chat_plan`. Only the
//! capability items the route claims are printed (§11.2): `<claims>` is a
//! comma-separated subset of `tool,json` (default both; `none` for
//! neither). Thinking and long context are not part of this OpenAI Chat
//! Completions plan.
//!
//! Usage: `cargo run -p agent-value-core --example probe_plan -- <seed> <model> [claims]`

use agent_value_core::probe::{build_openai_chat_plan, CapabilityClaims};

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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 && args.len() != 4 {
        eprintln!("usage: probe_plan <seed> <model> [claims]");
        std::process::exit(2);
    }
    let seed: u64 = args[1].parse().expect("seed must be an integer");
    let model = &args[2];
    let plan =
        build_openai_chat_plan(seed, model, &claims(args.get(3))).expect("seed must be non-zero");
    for row in plan {
        println!("{}", row.to_json_line());
    }
}

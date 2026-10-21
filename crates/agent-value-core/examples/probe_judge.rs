//! Judges a recorded minimal-probe run (replay format of
//! `agent_value_core::probe::parse_replay`) for one model on an OpenAI
//! Chat Completions route. Prints counts, verdicts and hard failures only:
//! no reply text, no key.
//!
//! `[claims]` as in `probe_plan` (default `tool,json`).
//!
//! Usage: `cargo run -p agent-value-core --example probe_judge -- <seed> <model> <replay.tsv> [claims]`

use agent_value_core::arm::{ModelId, Protocol};
use agent_value_core::probe::{
    applicable_items, judge_capability, judge_deterministic, parse_replay, run_hard_failures,
    CapabilityClaims, VendorTables,
};

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
    if args.len() != 4 && args.len() != 5 {
        eprintln!("usage: probe_judge <seed> <model> <replay.tsv> [claims]");
        std::process::exit(2);
    }
    let seed: u64 = args[1].parse().expect("seed must be an integer");
    let model = ModelId::new(&args[2]).expect("model id");
    let text = std::fs::read_to_string(&args[3]).expect("replay file");
    let records = parse_replay(&text).expect("replay format");
    let det = judge_deterministic(seed, &records).expect("deterministic run");
    let items = applicable_items(&claims(args.get(4)), &Protocol::OpenAiChatCompletions);
    let cap = judge_capability(seed, &items, None, &records).expect("capability run");
    let hard = run_hard_failures(
        &VendorTables::default(),
        &model,
        &Protocol::OpenAiChatCompletions,
        &records,
    );
    println!(
        "deterministic\t{}\tcorrect={}\twrong={}\tunanswered={}",
        det.verdict.as_str(),
        det.correct,
        det.wrong,
        det.unanswered
    );
    println!(
        "capability\t{}\tcorrect={}\twrong={}\tunanswered={}",
        cap.verdict.as_str(),
        cap.correct,
        cap.wrong,
        cap.unanswered
    );
    println!("hard_failures\t{}\t{:?}", hard.len(), hard);
}

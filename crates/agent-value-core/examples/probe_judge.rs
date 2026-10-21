//! Judges a recorded minimal-probe run (replay format of
//! `agent_value_core::probe::parse_replay`) for one model on an OpenAI
//! Chat Completions route. Prints counts, verdicts and hard failures only:
//! no reply text, no key.
//!
//! Usage: `cargo run -p agent-value-core --example probe_judge -- <seed> <model> <replay.tsv>`

use agent_value_core::arm::{ModelId, Protocol};
use agent_value_core::probe::{
    judge_capability, judge_deterministic, parse_replay, run_hard_failures, CapabilityItem,
    VendorTables,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: probe_judge <seed> <model> <replay.tsv>");
        std::process::exit(2);
    }
    let seed: u64 = args[1].parse().expect("seed must be an integer");
    let model = ModelId::new(&args[2]).expect("model id");
    let text = std::fs::read_to_string(&args[3]).expect("replay file");
    let records = parse_replay(&text).expect("replay format");
    let det = judge_deterministic(seed, &records).expect("deterministic run");
    let cap = judge_capability(
        seed,
        &[CapabilityItem::ToolCall, CapabilityItem::JsonSchema],
        None,
        &records,
    )
    .expect("capability run");
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

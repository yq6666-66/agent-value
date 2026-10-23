//! One probe run for one selected arm.
//!
//! The caller passes the arm, the environment-variable **name** the bearer
//! token is read from, and the base URL. The token is loaded inside
//! `agent-value-probe-http` and is not stored. Consent must already be
//! accepted. The monthly budget gate uses a provisional estimate:
//! plan rows times [`PROBE_ESTIMATE_PER_ITEM_MICROS`]. A stop writes
//! nothing and does not open a socket. Otherwise this builds an OpenAI
//! Chat Completions plan, sends it on a blocking client (off the Tauri
//! async runtime), judges the replay, and stores the class results plus
//! that estimate as the charge.
//!
//! Thinking-signature and long-context rows are not in the plan. Fingerprint
//! rows are not invented, and the observation period is not marked complete.
//! The webview is not wired to [`run_once`].
//!
//! # Provisional estimate
//!
//! [`PROBE_ESTIMATE_PER_ITEM_MICROS`] is 0.05 yuan per plan row. It is not a
//! measured vendor price and not an observation-period threshold. It is only
//! a small non-zero gate so a run near the monthly cap stops, and the same
//! amount is what a stored run records as spent. Replace it when a price
//! table is wired. Under-estimating can overspend; over-estimating can stop
//! a run that would have fit. (Probe estimate, provisional.)

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent_value_core::arm::{Arm, Protocol};
use agent_value_core::authenticity::{budget_month, probe_budget, BudgetDecision};
use agent_value_core::constants::PROBE_MONTHLY_BUDGET_YUAN;
use agent_value_core::probe::{
    applicable_items, build_openai_chat_plan, judge_capability, judge_deterministic,
    run_hard_failures, CapabilityClaims, CapabilityItem, ClassRun, OpenAiChatPlanRow, VendorTables,
    PROBE_SET_VERSION,
};
use agent_value_core::records::{MicroYuan, ProbeClass, ProbeResult};
use agent_value_probe_http::{run, PlanItem, RunOptions};
use agent_value_store::Store;

/// Provisional price of one OpenAI chat probe request, in micro-yuan.
///
/// 0.05 yuan. See the crate docs. Not a measured price.
pub const PROBE_ESTIMATE_PER_ITEM_MICROS: i64 = 50_000;

/// Seed used only to count a typical plan. Any non-zero seed has the same
/// row count.
const TYPICAL_PLAN_SEED: u64 = 1;
const TYPICAL_PLAN_MODEL: &str = "probe-model";

/// What the panel, or a finished `run_once`, should say about launching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeLaunch {
    BlockedConsent,
    BlockedBudget,
    Ready,
    Running,
    Completed,
    Failed,
}

impl ProbeLaunch {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BlockedConsent => "blocked-consent",
            Self::BlockedBudget => "blocked-budget",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::BlockedConsent => "尚未同意，不会自动探测。",
            Self::BlockedBudget => "本月停止探测。",
            Self::Ready => "已同意，且未超预算。就绪时可运行；本版 UI 尚未接线。",
            Self::Running => "正在探测。",
            Self::Completed => "这次探测已写入本地记录。",
            Self::Failed => "这次探测没有完成，没有写入新的探针记录。",
        }
    }
}

/// Failure before a launch state can be chosen. The message does not
/// include a bearer token.
#[derive(Debug)]
pub struct RunError {
    message: String,
}

impl RunError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    fn core(err: impl std::fmt::Display) -> Self {
        Self::new(err.to_string())
    }
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RunError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunTerminal {
    Completed,
    Failed,
}

/// Single-flight state for probe runs. One process-wide value is not
/// assumed: the shell keeps one of these next to the store.
pub struct Flight {
    in_flight: AtomicBool,
    last: Mutex<Option<RunTerminal>>,
}

impl Flight {
    pub fn new() -> Self {
        Self {
            in_flight: AtomicBool::new(false),
            last: Mutex::new(None),
        }
    }

    /// Panel launch. An in-flight run wins over the gates. Consent and the
    /// budget win over the previous result. `Ready` is allowed and not yet
    /// run.
    pub fn launch(&self, accepted: bool, budget: BudgetDecision) -> ProbeLaunch {
        if self.in_flight.load(Ordering::Acquire) {
            return ProbeLaunch::Running;
        }
        if !accepted {
            return ProbeLaunch::BlockedConsent;
        }
        if budget == BudgetDecision::Stop {
            return ProbeLaunch::BlockedBudget;
        }
        match *self.last.lock().unwrap_or_else(|err| err.into_inner()) {
            Some(RunTerminal::Completed) => ProbeLaunch::Completed,
            Some(RunTerminal::Failed) => ProbeLaunch::Failed,
            None => ProbeLaunch::Ready,
        }
    }

    fn try_begin(&self) -> Option<FlightGuard<'_>> {
        if self.in_flight.swap(true, Ordering::AcqRel) {
            None
        } else {
            Some(FlightGuard { flight: self })
        }
    }

    fn note(&self, launch: ProbeLaunch) {
        let terminal = match launch {
            ProbeLaunch::Completed => RunTerminal::Completed,
            ProbeLaunch::Failed => RunTerminal::Failed,
            _ => return,
        };
        *self.last.lock().unwrap_or_else(|err| err.into_inner()) = Some(terminal);
    }
}

impl Default for Flight {
    fn default() -> Self {
        Self::new()
    }
}

struct FlightGuard<'a> {
    flight: &'a Flight,
}

impl Drop for FlightGuard<'_> {
    fn drop(&mut self) {
        self.flight.in_flight.store(false, Ordering::Release);
    }
}

/// One selected arm. `key_env` is the variable name, never the token.
/// `base_url` is the probe HTTP base (`https`, or `http` for a local mock).
#[derive(Debug)]
pub struct ProbeRunRequest<'a> {
    pub arm: &'a Arm,
    pub key_env: &'a str,
    pub base_url: &'a str,
    pub seed: u64,
    pub claims: CapabilityClaims,
    pub dotenv: &'a Path,
    pub timeout: Duration,
}

/// Policy or run result. `detail` is safe to show: it is a launch label or
/// a sender error that does not include the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeRunOutcome {
    pub launch: ProbeLaunch,
    pub detail: String,
}

impl ProbeRunOutcome {
    fn of(launch: ProbeLaunch) -> Self {
        Self {
            launch,
            detail: launch.label().to_string(),
        }
    }

    fn failed(detail: impl Into<String>) -> Self {
        Self {
            launch: ProbeLaunch::Failed,
            detail: detail.into(),
        }
    }
}

/// Claims this plan can actually send. Tool call and JSON schema only.
pub fn typical_claims() -> CapabilityClaims {
    CapabilityClaims {
        tool_call: true,
        json_schema: true,
        ..CapabilityClaims::default()
    }
}

/// Rows in a typical OpenAI chat plan (deterministic set plus tool and JSON).
pub fn typical_plan_rows() -> Result<usize, RunError> {
    Ok(
        build_openai_chat_plan(TYPICAL_PLAN_SEED, TYPICAL_PLAN_MODEL, &typical_claims())
            .map_err(RunError::core)?
            .len(),
    )
}

/// Estimate shown for the next run when a typical plan would be sent.
pub fn typical_probe_estimate() -> Result<MicroYuan, RunError> {
    estimate_for_rows(typical_plan_rows()?)
}

/// Probe charges in the Beijing-time month of `at_ms`.
pub fn spent_in_month(charges: &[(i64, i64)], at_ms: i64) -> Result<MicroYuan, RunError> {
    let month = budget_month(at_ms).map_err(RunError::core)?;
    let mut total = 0_i64;
    for &(at, micros) in charges {
        if budget_month(at).map_err(RunError::core)? == month {
            total = total
                .checked_add(micros)
                .ok_or_else(|| RunError::new("probe spend overflow"))?;
        }
    }
    MicroYuan::new(total).map_err(RunError::core)
}

/// Runs one probe, or rejects it.
///
/// Blocks the calling thread for every HTTP exchange. Do not call it on
/// the Tauri async runtime. A second call while one is in flight returns
/// [`ProbeLaunch::Running`] and does not send.
pub fn run_once(
    flight: &Flight,
    store: &Mutex<Store>,
    request: &ProbeRunRequest<'_>,
) -> Result<ProbeRunOutcome, RunError> {
    let Some(guard) = flight.try_begin() else {
        return Ok(ProbeRunOutcome {
            launch: ProbeLaunch::Running,
            detail: "已有一次探测在进行。".to_string(),
        });
    };
    let outcome = match execute(store, request) {
        Ok(outcome) => outcome,
        Err(err) => ProbeRunOutcome::failed(err.to_string()),
    };
    flight.note(outcome.launch);
    drop(guard);
    Ok(outcome)
}

fn execute(
    store: &Mutex<Store>,
    request: &ProbeRunRequest<'_>,
) -> Result<ProbeRunOutcome, RunError> {
    let at_ms = now_ms()?;
    let (accepted, spent) = {
        let db = lock_store(store)?;
        let accepted = db
            .probe_consent()
            .map_err(RunError::core)?
            .is_some_and(|consent| consent.accepted);
        let charges = db.probe_charges().map_err(RunError::core)?;
        let spent = spent_in_month(&charges, at_ms)?;
        (accepted, spent)
    };
    if !accepted {
        return Ok(ProbeRunOutcome::of(ProbeLaunch::BlockedConsent));
    }
    if request.arm.route().protocol() != &Protocol::OpenAiChatCompletions {
        return Ok(ProbeRunOutcome::failed(
            "这条路线的协议不是 OpenAI Chat Completions。",
        ));
    }

    let rows = build_openai_chat_plan(request.seed, request.arm.model().as_str(), &request.claims)
        .map_err(RunError::core)?;
    let estimate = estimate_for_rows(rows.len())?;
    let budget = MicroYuan::from_yuan(PROBE_MONTHLY_BUDGET_YUAN).map_err(RunError::core)?;
    if probe_budget(spent, estimate, budget).map_err(RunError::core)? == BudgetDecision::Stop {
        return Ok(ProbeRunOutcome::of(ProbeLaunch::BlockedBudget));
    }

    let items = plan_items(&rows)?;
    let dir = TempRun::new()?;
    let replay_path = dir.path.join("replay.tsv");
    let log_path = dir.path.join("run.jsonl");
    let sent = run(&RunOptions {
        base: request.base_url,
        key_env: request.key_env,
        dotenv: request.dotenv,
        plan: &items,
        replay_path: &replay_path,
        log_path: &log_path,
        timeout: request.timeout,
        max_delay_secs: 0.0,
    });
    let replay = match &sent {
        Ok(()) => std::fs::read_to_string(&replay_path)
            .map_err(|err| RunError::new(format!("failed to read probe replay: {err}")))?,
        Err(_) => String::new(),
    };
    drop(dir);
    if let Err(err) = sent {
        return Ok(ProbeRunOutcome::failed(err.to_string()));
    }

    let records = agent_value_core::probe::parse_replay(&replay)
        .map_err(|_| RunError::new("回放无法判定。"))?;
    let deterministic = judge_deterministic(request.seed, &records).map_err(RunError::core)?;
    let items = applicable_items(&request.claims, request.arm.route().protocol())
        .into_iter()
        .filter(|item| matches!(item, CapabilityItem::ToolCall | CapabilityItem::JsonSchema))
        .collect::<Vec<_>>();
    let capability =
        judge_capability(request.seed, &items, None, &records).map_err(RunError::core)?;
    let hard = !run_hard_failures(
        &VendorTables::default(),
        request.arm.model(),
        request.arm.route().protocol(),
        &request.claims,
        &records,
    )
    .is_empty();
    let results = [
        class_result(
            request.arm,
            at_ms,
            ProbeClass::Deterministic,
            deterministic,
            hard,
        ),
        class_result(request.arm, at_ms, ProbeClass::Capability, capability, hard),
    ];
    lock_store(store)?
        .record_probe_run(&results, at_ms, estimate)
        .map_err(RunError::core)?;
    Ok(ProbeRunOutcome::of(ProbeLaunch::Completed))
}

fn estimate_for_rows(rows: usize) -> Result<MicroYuan, RunError> {
    let n = i64::try_from(rows).map_err(|_| RunError::new("probe plan is too long"))?;
    let micros = n
        .checked_mul(PROBE_ESTIMATE_PER_ITEM_MICROS)
        .ok_or_else(|| RunError::new("probe estimate overflow"))?;
    MicroYuan::new(micros).map_err(RunError::core)
}

fn plan_items(rows: &[OpenAiChatPlanRow]) -> Result<Vec<PlanItem>, RunError> {
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let json = row.body.to_json().map_err(RunError::core)?;
        let body = serde_json::from_str(&json)
            .map_err(|_| RunError::new("probe plan body is not JSON"))?;
        items.push(PlanItem {
            id: row.id.clone(),
            body,
        });
    }
    Ok(items)
}

/// The store's unique key does not include the class, so each class uses
/// its own probe id. Version stays [`PROBE_SET_VERSION`].
fn class_result(
    arm: &Arm,
    at_ms: i64,
    class: ProbeClass,
    run: ClassRun,
    hard: bool,
) -> ProbeResult {
    let probe_id = match class {
        ProbeClass::Deterministic => "openai-chat-deterministic",
        ProbeClass::Capability => "openai-chat-capability",
        ProbeClass::Fingerprint => "openai-chat-fingerprint",
    };
    ProbeResult {
        probe_id: probe_id.to_string(),
        probe_version: PROBE_SET_VERSION.to_string(),
        arm: arm.clone(),
        at_ms,
        class,
        verdict: run.verdict,
        hard_failure: hard,
        evidence_summary: run.evidence_summary(),
    }
}

fn lock_store(store: &Mutex<Store>) -> Result<std::sync::MutexGuard<'_, Store>, RunError> {
    store
        .lock()
        .map_err(|_| RunError::new("store lock poisoned"))
}

fn now_ms() -> Result<i64, RunError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| RunError::new(err.to_string()))?;
    i64::try_from(elapsed.as_millis()).map_err(|_| RunError::new("clock is out of range"))
}

struct TempRun {
    path: PathBuf,
}

impl TempRun {
    fn new() -> Result<Self, RunError> {
        static N: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "agent-value-probe-run-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).map_err(|err| RunError::new(err.to_string()))?;
        Ok(Self { path })
    }
}

impl Drop for TempRun {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

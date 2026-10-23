//! Window results for the coding scope.
//!
//! The shell reads tasks and the frozen window from the local store, then
//! asks `agent_value_core` for status, aggregation, eligibility, the
//! frontier, and the recommendation. Example fixtures use that same scorer.
//! Printed numbers are the core's values; this module does not keep a second
//! formula.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_value_core::anchor::{certify_reference_layer, layer_value, AnchorTier, TierIIIReason};
use agent_value_core::arm::{Arm, ModelId, Protocol, ProviderKind, Route};
use agent_value_core::cell_values::{
    cell_values, unit_price, CellInputs, CellValues, DimInputs, PriceSource, Role, Triple,
};
use agent_value_core::cells::{CellId, CellWeights};
use agent_value_core::diagnostic::{
    diagnose_arm, sort_diagnostic_rows, DiagnosticRow, NotAdmittedReason,
};
use agent_value_core::eligibility::{assess_eligibility, AuthenticityGrade};
use agent_value_core::frontier::{layer_frontiers, LayerFrontiers};
use agent_value_core::labels::{self, tier_ii_header, tier_iii_header};
use agent_value_core::quality::{self, QualityMode, Rating};
use agent_value_core::recommend::{layer_arm_from_cells, layer_pick, LayerArm};
use agent_value_core::records::{MicroYuan, Outcome};
use agent_value_core::retry::{PassStats, Prior, RetryModel};
use agent_value_core::scope::{ScopeId, TaskKind};
use agent_value_core::sensitivity::SensitivityArm;
use agent_value_core::status::{
    derive_cell_weights, layer_status, CellCounts, CellInput, LayerStatus, OmegaSource,
};
use agent_value_core::value::{aggregate_layer, Floors, Point};
use agent_value_core::weights::{Evidence, WeightLedger, Weights};
use agent_value_core::window::{FrozenInputs, WindowIndex, FIRST_WINDOW_LABEL};
use agent_value_store::{ConfigKind, UserTaskRow};
use serde::Serialize;

use crate::store_cmd::{self, AppState};

const FIXTURE_SOURCE: &str = "example-fixture";
const PROBES_OFF: &str = "probes-off";

/// One arm as the page may show it. Texts are the core values, already printed.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ArmView {
    pub id: String,
    pub n_comp: u64,
    pub n_valid: usize,
    pub n_fail: usize,
    pub eligible: bool,
    pub failed_gates: Vec<String>,
    pub cost_text: Option<String>,
    pub time_text: Option<String>,
    pub quality_text: Option<String>,
    pub v_text: Option<String>,
    pub diagnostic_c_text: Option<String>,
    pub diagnostic_admitted: bool,
    pub not_admitted: Vec<String>,
    pub on_off: bool,
    pub on_rob: bool,
    pub on_rec: bool,
    pub coarse_bill: bool,
    pub cost_source: Option<String>,
}

/// JSON for one window. `official_top` is the whole tie group when `tied`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WindowResult {
    pub source: String,
    pub fixture: bool,
    pub scope: String,
    pub window_index: Option<u32>,
    pub is_first: bool,
    pub first_window_label: Option<String>,
    pub weight_source: Option<String>,
    pub weight_a: Option<String>,
    pub weight_b: Option<String>,
    pub weight_g: Option<String>,
    pub omega_source: Option<String>,
    pub cell_weights_version: Option<String>,
    pub reference_version: Option<String>,
    pub fallback_version: Option<String>,
    pub price_table_version: Option<String>,
    pub classification_rules_version: Option<String>,
    pub authenticity_grade: String,
    pub authenticity_source: String,
    pub authenticity_undetermined: bool,
    pub log_integrity_source: String,
    pub anchor_tier: Option<String>,
    pub anchor_headers: Vec<String>,
    pub tier_ii_legend: Option<String>,
    pub diagnostic_anchor_c_text: Option<String>,
    pub diagnostic_anchor_t_text: Option<String>,
    pub diagnostic_anchor_q_text: Option<String>,
    pub measured_anchor_c_text: Option<String>,
    pub reference_id: Option<String>,
    pub arms: Vec<ArmView>,
    pub eligible: Vec<String>,
    pub frontier_off: Vec<String>,
    pub frontier_rob: Vec<String>,
    pub frontier_rec: Vec<String>,
    pub coarse_bill: Vec<String>,
    pub official_top: Vec<String>,
    pub tied: bool,
    pub robust_alternative: Vec<String>,
    pub v_withheld: Option<String>,
    pub empty_reason: Option<String>,
}

struct CellSnap {
    cell: CellId,
    counts: CellCounts,
    values: CellValues,
    cost_source: Option<String>,
}

struct ArmSnap {
    id: String,
    cells: Vec<CellSnap>,
}

struct Snapshot {
    source: String,
    fixture: bool,
    scope: String,
    window_index: Option<u32>,
    is_first: bool,
    weight_source: Option<String>,
    weights: Weights,
    omega_source: Option<String>,
    inputs: FrozenInputs,
    authenticity: AuthenticityGrade,
    authenticity_source: String,
    log_integrity: bool,
    log_integrity_source: String,
    cell_weights: CellWeights,
    reference_id: Option<String>,
    reference: Vec<CellSnap>,
    arms: Vec<ArmSnap>,
}

fn coding_scope() -> ScopeId {
    ScopeId::new(TaskKind::Coding)
}

fn now_ms() -> Result<i64, String> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| err.to_string())?;
    i64::try_from(elapsed.as_millis()).map_err(|_| "clock is out of range".to_string())
}

fn print_decimals(value: f64, digits: usize) -> String {
    let scale = 10_f64.powi(i32::try_from(digits).unwrap_or(4));
    let rounded = (value * scale).round() / scale;
    format!("{rounded:.digits$}")
}

fn print4(value: f64) -> String {
    print_decimals(value, 4)
}

fn print2(value: f64) -> String {
    print_decimals(value, 2)
}

fn show_weight(value: f64) -> String {
    format!("{value}")
}

fn tier_text(tier: AnchorTier) -> &'static str {
    match tier {
        AnchorTier::I => "I",
        AnchorTier::II => "II",
        AnchorTier::III => "III",
    }
}

fn omega_text(source: OmegaSource) -> &'static str {
    match source {
        OmegaSource::PreviousWindow => "previous-window",
        OmegaSource::CarriedOver => "carried-over",
        OmegaSource::Equal => "equal",
    }
}

fn price_text(source: PriceSource) -> &'static str {
    match source {
        PriceSource::Shrunk => "shrunk",
        PriceSource::Own => "own",
        PriceSource::Benchmark => "benchmark",
        PriceSource::LayerPool => "layer-pool",
    }
}

fn reason_text(reason: NotAdmittedReason) -> &'static str {
    match reason {
        NotAdmittedReason::EssBelowThree => "ess-below-three",
        NotAdmittedReason::NoValidCell => "no-valid-cell",
        NotAdmittedReason::ValidCellIncomplete => "valid-cell-incomplete",
    }
}

fn triple_text(triple: &Triple) -> (Option<String>, Option<String>, Option<String>) {
    (
        triple.c.map(print4),
        triple.t.map(print4),
        triple.q.map(print4),
    )
}

fn blank(source: &str, fixture: bool) -> WindowResult {
    WindowResult {
        source: source.to_string(),
        fixture,
        scope: coding_scope().to_string(),
        window_index: None,
        is_first: false,
        first_window_label: None,
        weight_source: None,
        weight_a: None,
        weight_b: None,
        weight_g: None,
        omega_source: None,
        cell_weights_version: None,
        reference_version: None,
        fallback_version: None,
        price_table_version: None,
        classification_rules_version: None,
        authenticity_grade: String::new(),
        authenticity_source: String::new(),
        authenticity_undetermined: false,
        log_integrity_source: String::new(),
        anchor_tier: None,
        anchor_headers: Vec::new(),
        tier_ii_legend: None,
        diagnostic_anchor_c_text: None,
        diagnostic_anchor_t_text: None,
        diagnostic_anchor_q_text: None,
        measured_anchor_c_text: None,
        reference_id: None,
        arms: Vec::new(),
        eligible: Vec::new(),
        frontier_off: Vec::new(),
        frontier_rob: Vec::new(),
        frontier_rec: Vec::new(),
        coarse_bill: Vec::new(),
        official_top: Vec::new(),
        tied: false,
        robust_alternative: Vec::new(),
        v_withheld: None,
        empty_reason: None,
    }
}

fn inputs_of(cell: &CellSnap) -> CellInput {
    CellInput {
        cell: cell.cell.clone(),
        counts: cell.counts,
        prior: Prior::Jeffreys,
    }
}

fn complete_triples(cells: &[CellSnap]) -> Vec<(CellId, Triple)> {
    cells
        .iter()
        .filter(|cell| cell.values.star.is_complete())
        .map(|cell| (cell.cell.clone(), cell.values.star))
        .collect()
}

fn aligned(weights: &CellWeights, cells: &[CellSnap]) -> Vec<(CellId, CellValues)> {
    weights
        .support()
        .map(|cell| {
            let values = cells
                .iter()
                .find(|snap| &snap.cell == cell)
                .map(|snap| snap.values.clone())
                .unwrap_or_else(CellValues::na);
            (cell.clone(), values)
        })
        .collect()
}

fn cost_source_of(cells: &[CellSnap]) -> Option<String> {
    let mut sources = cells.iter().filter_map(|cell| cell.cost_source.clone());
    let first = sources.next()?;
    if sources.any(|source| source != first) {
        Some("mixed".to_string())
    } else {
        Some(first)
    }
}

struct ScoredArm {
    id: String,
    status: LayerStatus,
    eligible: bool,
    failed_gates: Vec<String>,
    diag: DiagnosticRow,
    aggregate: Triple,
    layer_arm: Option<LayerArm>,
    cost_source: Option<String>,
}

fn score_arm(
    snap: &ArmSnap,
    weights: &CellWeights,
    model: &RetryModel,
    reference_triples: &[(CellId, Triple)],
    authenticity: AuthenticityGrade,
    log_integrity: bool,
    preference: &Weights,
) -> Result<ScoredArm, String> {
    let inputs: Vec<CellInput> = snap.cells.iter().map(inputs_of).collect();
    let status = layer_status(weights, &inputs, model).map_err(|err| err.to_string())?;
    let triples = complete_triples(&snap.cells);
    let priors: Vec<(CellId, Prior)> = inputs
        .iter()
        .map(|row| (row.cell.clone(), row.prior))
        .collect();
    let diag = diagnose_arm(
        &status,
        weights,
        &triples,
        &priors,
        None,
        preference,
        &Floors::default(),
    )
    .map_err(|err| err.to_string())?;
    let aggregate = aggregate_layer(weights, &triples).map_err(|err| err.to_string())?;
    let eligibility = assess_eligibility(
        &status,
        weights,
        authenticity,
        log_integrity,
        reference_triples,
    )
    .map_err(|err| err.to_string())?;
    let layer_arm = if eligibility.eligible {
        Some(
            layer_arm_from_cells(&snap.id, weights, &aligned(weights, &snap.cells))
                .map_err(|err| err.to_string())?,
        )
    } else {
        None
    };
    Ok(ScoredArm {
        id: snap.id.clone(),
        status,
        eligible: eligibility.eligible,
        failed_gates: eligibility
            .failed
            .iter()
            .map(|gate| gate.as_str().to_string())
            .collect(),
        diag,
        aggregate,
        layer_arm,
        cost_source: cost_source_of(&snap.cells),
    })
}

fn headers_for(
    tier: AnchorTier,
    reasons: &[TierIIIReason],
    anchor: &agent_value_core::anchor::LayerAnchor,
) -> Vec<String> {
    match tier {
        AnchorTier::I => Vec::new(),
        AnchorTier::II => vec![tier_ii_header(anchor)],
        AnchorTier::III => reasons
            .iter()
            .map(|reason| tier_iii_header(*reason, anchor))
            .collect(),
    }
}

struct Certified {
    anchor: agent_value_core::anchor::LayerAnchor,
    reference_diag: DiagnosticRow,
    measured: Triple,
    scored: Vec<ScoredArm>,
    frontiers: LayerFrontiers,
    scores: Vec<(String, f64)>,
    pick: agent_value_core::recommend::LayerPick,
    sensitivity_arms: Vec<SensitivityArm>,
}

fn certify(snapshot: &Snapshot) -> Result<Certified, String> {
    let model = RetryModel::with_default_k(0.0).map_err(|err| err.to_string())?;
    let reference_inputs: Vec<CellInput> = snapshot.reference.iter().map(inputs_of).collect();
    let reference_status = layer_status(&snapshot.cell_weights, &reference_inputs, &model)
        .map_err(|err| err.to_string())?;
    let reference_triples = complete_triples(&snapshot.reference);
    let priors: Vec<(CellId, Prior)> = reference_inputs
        .iter()
        .map(|row| (row.cell.clone(), row.prior))
        .collect();
    let reference_diag = diagnose_arm(
        &reference_status,
        &snapshot.cell_weights,
        &reference_triples,
        &priors,
        None,
        &snapshot.weights,
        &Floors::default(),
    )
    .map_err(|err| err.to_string())?;
    let measured = aggregate_layer(&snapshot.cell_weights, &reference_triples)
        .map_err(|err| err.to_string())?;
    let anchor = certify_reference_layer(
        &reference_status,
        &snapshot.cell_weights,
        &reference_triples,
    )
    .map_err(|err| err.to_string())?;

    let mut scored = Vec::new();
    for arm in &snapshot.arms {
        scored.push(score_arm(
            arm,
            &snapshot.cell_weights,
            &model,
            &reference_triples,
            snapshot.authenticity,
            snapshot.log_integrity,
            &snapshot.weights,
        )?);
    }

    let eligible_arms: Vec<LayerArm> = scored
        .iter()
        .filter_map(|arm| arm.layer_arm.clone())
        .collect();
    let frontier_arms: Vec<_> = eligible_arms.iter().map(|arm| arm.arm.clone()).collect();
    let frontiers = if frontier_arms.is_empty() {
        LayerFrontiers {
            off: Vec::new(),
            rob: Vec::new(),
            rec: Vec::new(),
            coarse_bill: Vec::new(),
        }
    } else {
        layer_frontiers(&frontier_arms).map_err(|err| err.to_string())?
    };
    let mut scores = Vec::new();
    for arm in &eligible_arms {
        if let Some(point) = Point::from_triple(&arm.arm.star) {
            if let Some(value) = layer_value(&anchor, point, &snapshot.weights, &Floors::default())
                .map_err(|err| err.to_string())?
            {
                scores.push((arm.arm.id.clone(), value));
            }
        }
    }
    let pick =
        layer_pick(&eligible_arms, &frontiers, &anchor, &scores).map_err(|err| err.to_string())?;
    let mut sensitivity_arms = Vec::new();
    for arm in &eligible_arms {
        if let Some(point) = Point::from_triple(&arm.arm.star) {
            sensitivity_arms.push(SensitivityArm {
                id: arm.arm.id.clone(),
                point,
            });
        }
    }
    Ok(Certified {
        anchor,
        reference_diag,
        measured,
        scored,
        frontiers,
        scores,
        pick,
        sensitivity_arms,
    })
}

fn score(snapshot: &Snapshot) -> Result<WindowResult, String> {
    let Certified {
        anchor,
        reference_diag,
        measured,
        scored,
        frontiers,
        scores,
        pick,
        sensitivity_arms: _,
    } = certify(snapshot)?;
    let show_v = !snapshot.is_first && anchor.tier != AnchorTier::III;
    let v_of = |id: &str| {
        show_v
            .then(|| {
                scores
                    .iter()
                    .find(|(arm, _)| arm == id)
                    .map(|(_, value)| print2(*value))
            })
            .flatten()
    };

    let mut order: Vec<(String, usize, DiagnosticRow)> = scored
        .iter()
        .enumerate()
        .map(|(index, arm)| (arm.id.clone(), index, arm.diag.clone()))
        .collect();
    sort_diagnostic_rows(&mut order);
    let mut arms = Vec::new();
    for (_, index, _) in order {
        let arm = &scored[index];
        let id = arm.id.clone();
        let (cost_text, time_text, quality_text) = triple_text(&arm.aggregate);
        let coarse = arm
            .layer_arm
            .as_ref()
            .is_some_and(|layer| layer.coarse_bill);
        arms.push(ArmView {
            id: id.clone(),
            n_comp: arm.status.n_comp,
            n_valid: arm.status.n_valid,
            n_fail: arm.status.n_fail,
            eligible: arm.eligible,
            failed_gates: arm.failed_gates.clone(),
            cost_text,
            time_text,
            quality_text,
            v_text: v_of(&id),
            diagnostic_c_text: arm
                .diag
                .aggregate_cons
                .as_ref()
                .and_then(|triple| triple.c.map(print4)),
            diagnostic_admitted: arm.diag.admitted,
            not_admitted: arm
                .diag
                .not_admitted
                .iter()
                .copied()
                .map(reason_text)
                .map(str::to_string)
                .collect(),
            on_off: frontiers.off.iter().any(|member| member == &id),
            on_rob: frontiers.rob.iter().any(|member| member == &id),
            on_rec: frontiers.rec.iter().any(|member| member == &id),
            coarse_bill: coarse,
            cost_source: arm.cost_source.clone(),
        });
    }

    let (diag_c, diag_t, diag_q) = reference_diag
        .aggregate_cons
        .as_ref()
        .map(triple_text)
        .unwrap_or((None, None, None));
    let mut official_top = if show_v {
        pick.official_top
    } else {
        Vec::new()
    };
    let mut v_withheld = if snapshot.is_first {
        Some(FIRST_WINDOW_LABEL.to_string())
    } else if anchor.tier == AnchorTier::III {
        Some(
            headers_for(anchor.tier, &anchor.tier_iii_reasons, &anchor)
                .into_iter()
                .next()
                .unwrap_or_else(|| "tier-iii".to_string()),
        )
    } else {
        None
    };
    if official_top.is_empty()
        && v_withheld.is_none()
        && scores.is_empty()
        && snapshot.reference_id.is_none()
    {
        v_withheld = Some("没有参照臂，不计算 V，也不进入前沿。".to_string());
    }
    official_top.sort();

    let mut view = blank(&snapshot.source, snapshot.fixture);
    view.scope = snapshot.scope.clone();
    view.window_index = snapshot.window_index;
    view.is_first = snapshot.is_first;
    view.first_window_label = snapshot.is_first.then(|| FIRST_WINDOW_LABEL.to_string());
    view.weight_source = snapshot.weight_source.clone();
    view.weight_a = Some(show_weight(snapshot.weights.a()));
    view.weight_b = Some(show_weight(snapshot.weights.b()));
    view.weight_g = Some(show_weight(snapshot.weights.g()));
    view.omega_source = snapshot.omega_source.clone();
    view.cell_weights_version = snapshot.inputs.cell_weights_version.clone();
    view.reference_version = snapshot.inputs.reference_version.clone();
    view.fallback_version = snapshot.inputs.fallback_version.clone();
    view.price_table_version = snapshot.inputs.price_table_version.clone();
    view.classification_rules_version = snapshot.inputs.classification_rules_version.clone();
    view.authenticity_grade = snapshot.authenticity.as_str().to_string();
    view.authenticity_source = snapshot.authenticity_source.clone();
    view.authenticity_undetermined = snapshot.authenticity.undetermined();
    view.log_integrity_source = snapshot.log_integrity_source.clone();
    view.anchor_tier = Some(tier_text(anchor.tier).to_string());
    view.anchor_headers = headers_for(anchor.tier, &anchor.tier_iii_reasons, &anchor);
    view.tier_ii_legend =
        (anchor.tier == AnchorTier::II).then(|| labels::TIER_II_LEGEND.to_string());
    view.diagnostic_anchor_c_text = diag_c;
    view.diagnostic_anchor_t_text = diag_t;
    view.diagnostic_anchor_q_text = diag_q;
    view.measured_anchor_c_text = measured.c.map(print4);
    view.reference_id = snapshot.reference_id.clone();
    view.arms = arms;
    view.eligible = scored
        .iter()
        .filter(|arm| arm.eligible)
        .map(|arm| arm.id.clone())
        .collect();
    view.eligible.sort();
    view.frontier_off = frontiers.off;
    view.frontier_rob = frontiers.rob;
    view.frontier_rec = frontiers.rec;
    view.coarse_bill = frontiers.coarse_bill;
    view.tied = official_top.len() > 1;
    view.official_top = official_top;
    view.robust_alternative = if pick.robust_alternative.is_empty() {
        Vec::new()
    } else {
        pick.robust_alternative
    };
    view.v_withheld = v_withheld;
    Ok(view)
}

fn ids(n: usize) -> Vec<CellId> {
    (1..=n)
        .map(|i| CellId::user(&format!("c{i:02}")).expect("cell"))
        .collect()
}

fn plain(triple: Triple) -> CellValues {
    CellValues {
        star: triple,
        robust: triple,
        robust_dominance: true,
        notes: Vec::new(),
    }
}

fn tc(cost: f64) -> Triple {
    Triple::new(cost, 10.0, 0.8)
}

const VALID: CellCounts = CellCounts {
    n1: 10,
    n0: 0,
    n_c: 0,
    m: 0,
};
const THIN: CellCounts = CellCounts {
    n1: 0,
    n0: 1,
    n_c: 0,
    m: 0,
};
const BAD: CellCounts = CellCounts {
    n1: 6,
    n0: 4,
    n_c: 0,
    m: 0,
};

fn snap(cell: &CellId, counts: CellCounts, triple: Triple) -> CellSnap {
    CellSnap {
        cell: cell.clone(),
        counts,
        values: plain(triple),
        cost_source: Some(FIXTURE_SOURCE.to_string()),
    }
}

fn uniform(cells: &[CellId], counts: CellCounts, triple: Triple) -> Vec<CellSnap> {
    cells
        .iter()
        .map(|cell| snap(cell, counts, triple))
        .collect()
}

fn varied(cells: &[CellId], counts: &[CellCounts], triples: &[Triple]) -> Vec<CellSnap> {
    cells
        .iter()
        .zip(counts)
        .zip(triples)
        .map(|((cell, counts), triple)| snap(cell, *counts, *triple))
        .collect()
}

fn fixture_inputs(reference: &str) -> FrozenInputs {
    FrozenInputs {
        cell_weights_version: Some(FIXTURE_SOURCE.to_string()),
        reference_version: Some(reference.to_string()),
        fallback_version: Some(FIXTURE_SOURCE.to_string()),
        price_table_version: Some(FIXTURE_SOURCE.to_string()),
        classification_rules_version: Some(FIXTURE_SOURCE.to_string()),
    }
}

fn fixture_base(source: &str, reference_label: &str, cells: &[CellId]) -> Snapshot {
    Snapshot {
        source: source.to_string(),
        fixture: true,
        scope: coding_scope().to_string(),
        window_index: Some(1),
        is_first: false,
        weight_source: Some(FIXTURE_SOURCE.to_string()),
        weights: Weights::equal(),
        omega_source: Some(FIXTURE_SOURCE.to_string()),
        inputs: fixture_inputs(reference_label),
        authenticity: AuthenticityGrade::A,
        authenticity_source: FIXTURE_SOURCE.to_string(),
        log_integrity: true,
        log_integrity_source: FIXTURE_SOURCE.to_string(),
        cell_weights: CellWeights::equal(cells).expect("equal cell weights"),
        reference_id: Some("reference".to_string()),
        reference: Vec::new(),
        arms: Vec::new(),
    }
}

fn arm(id: &str, cells: Vec<CellSnap>) -> ArmSnap {
    ArmSnap {
        id: id.to_string(),
        cells,
    }
}

fn example6() -> Snapshot {
    let cells = ids(12);
    let mut counts = vec![VALID; 11];
    counts.push(THIN);
    let mut triples = vec![tc(0.60); 11];
    triples.push(tc(2.85));
    let mut snapshot = fixture_base("example6", "example-6", &cells);
    snapshot.reference = varied(&cells, &counts, &triples);
    snapshot.arms = vec![
        arm("own", uniform(&cells, VALID, tc(0.60))),
        arm("b", uniform(&cells, VALID, Triple::new(0.55, 12.0, 0.75))),
        arm("route", varied(&cells, &counts, &vec![tc(0.7875); 12])),
    ];
    snapshot
}

fn example7() -> Snapshot {
    let cells = ids(12);
    let mut counts = vec![VALID; 11];
    counts.push(BAD);
    let mut triples = vec![tc(0.60); 11];
    triples.push(tc(4.20));
    let mut snapshot = fixture_base("example7", "example-7", &cells);
    snapshot.reference = varied(&cells, &counts, &triples);
    snapshot.arms = vec![arm("own", uniform(&cells, VALID, tc(0.60)))];
    snapshot
}

fn example8() -> Snapshot {
    let cells = ids(12);
    let mut counts = vec![VALID; 3];
    counts.extend(std::iter::repeat_n(THIN, 9));
    let mut triples = vec![tc(0.60); 3];
    triples.extend(std::iter::repeat_n(tc(0.02), 9));
    let mut snapshot = fixture_base("example8", "example-8", &cells);
    snapshot.reference = varied(&cells, &counts, &triples);
    snapshot.arms = vec![
        arm("own", uniform(&cells, VALID, tc(0.60))),
        arm("cheap", uniform(&cells, VALID, tc(0.165))),
    ];
    snapshot
}

fn tie(first: bool) -> Snapshot {
    let cells = ids(4);
    let mut snapshot = fixture_base("tie", "tie", &cells);
    snapshot.is_first = first;
    snapshot.reference = uniform(&cells, VALID, tc(1.0));
    let tied = Triple::new(0.8, 10.0, 0.8);
    snapshot.arms = vec![
        arm("X", uniform(&cells, VALID, tied)),
        arm("Y", uniform(&cells, VALID, tied)),
    ];
    snapshot
}

enum StoreLoad {
    Empty(Box<WindowResult>),
    Ready(Box<Snapshot>),
}

/// `source` is `store`, `example6`, `example7`, `example8`, or `tie`.
pub fn result(app: &AppState, source: &str) -> Result<WindowResult, String> {
    match source {
        "store" => store_result(app),
        "example6" => score(&example6()),
        "example7" => score(&example7()),
        "example8" => score(&example8()),
        "tie" => score(&tie(false)),
        other => Err(format!("unknown window result source {other}")),
    }
}

/// One coding layer, ready for `sensitivity`. The chart decision stays in
/// the core: this only gathers the inputs.
pub(crate) struct SensitivityParts {
    pub first_window: bool,
    pub current: Weights,
    pub reference: Option<Point>,
    pub arms: Vec<SensitivityArm>,
    pub omega_s: Option<Vec<Weights>>,
    /// Anchor header from `headers_for` when the numerator anchor is missing.
    /// Display only; not a sensitivity sentence.
    pub no_reference_text: Option<String>,
}

pub(crate) enum LayerLoad {
    /// Nothing to score. `show_chart` stays false.
    Empty {
        reason: String,
        is_first: bool,
        first_label: Option<String>,
    },
    Ready(SensitivityParts),
}

/// `source` is `store`, `example6`, `example7`, `example8`, `tie`, or `first`.
pub(crate) fn load_sensitivity(app: &AppState, source: &str) -> Result<LayerLoad, String> {
    let (snapshot, omega_s) = match source {
        "store" => match load_store(app)? {
            StoreLoad::Empty(view) => {
                return Ok(LayerLoad::Empty {
                    reason: view.empty_reason.clone().unwrap_or_default(),
                    is_first: view.is_first,
                    first_label: view.first_window_label.clone(),
                });
            }
            StoreLoad::Ready(snapshot) => (*snapshot, omega_s_from_store(app)?),
        },
        "example6" => (example6(), None),
        "example7" => (example7(), None),
        "example8" => (example8(), None),
        "tie" => (tie(false), None),
        "first" => (tie(true), None),
        other => return Err(format!("unknown sensitivity source {other}")),
    };
    parts_of(&snapshot, omega_s).map(LayerLoad::Ready)
}

fn parts_of(
    snapshot: &Snapshot,
    omega_s: Option<Vec<Weights>>,
) -> Result<SensitivityParts, String> {
    let certified = certify(snapshot)?;
    let no_reference_text = (certified.anchor.anchor.is_none()).then(|| {
        headers_for(
            certified.anchor.tier,
            &certified.anchor.tier_iii_reasons,
            &certified.anchor,
        )
        .into_iter()
        .next()
        .unwrap_or_else(|| "tier-iii".to_string())
    });
    Ok(SensitivityParts {
        first_window: snapshot.is_first,
        current: snapshot.weights,
        reference: certified.anchor.anchor,
        arms: certified.sensitivity_arms,
        omega_s,
        no_reference_text,
    })
}

fn omega_s_from_store(app: &AppState) -> Result<Option<Vec<Weights>>, String> {
    store_cmd::with_store(app, |store| {
        let Some(ledger) = store
            .load_weight_ledger(&coding_scope())
            .map_err(|err| err.to_string())?
        else {
            return Ok(None);
        };
        let Some(index) = ledger.last_frozen() else {
            return Ok(None);
        };
        let frozen = ledger.frozen(index).map_err(|err| err.to_string())?;
        omega_s_from_evidence(frozen.weight_version().evidence())
    })
}

fn omega_s_from_evidence(evidence: &Evidence) -> Result<Option<Vec<Weights>>, String> {
    let Evidence::SelfReportAnswers(answers) = evidence else {
        return Ok(None);
    };
    let mut parsed = Vec::with_capacity(answers.len());
    for item in answers {
        let question = item
            .question_id
            .parse::<usize>()
            .map_err(|_| "self-report question id is not a bank index".to_string())?;
        parsed.push(agent_value_core::elicit::Answer {
            question,
            answer: item.answer,
        });
    }
    let found = agent_value_core::elicit::regions(&parsed).map_err(|err| err.to_string())?;
    let mut weights = Vec::with_capacity(found.wide.len());
    for point in found.wide {
        weights.push(point.weights().map_err(|err| err.to_string())?);
    }
    Ok(Some(weights))
}

fn payload(
    store: &agent_value_store::Store,
    kind: ConfigKind,
    scope: &ScopeId,
    label: &str,
) -> Result<Option<String>, String> {
    if let Some(row) = store
        .config_version(kind, Some(scope), label)
        .map_err(|err| err.to_string())?
    {
        return Ok(Some(row.payload));
    }
    Ok(store
        .config_version(kind, None, label)
        .map_err(|err| err.to_string())?
        .map(|row| row.payload))
}

fn latest_label(
    store: &agent_value_store::Store,
    kind: ConfigKind,
    scope: &ScopeId,
) -> Result<Option<String>, String> {
    store
        .latest_config_label(kind, scope)
        .map_err(|err| err.to_string())
}

fn frozen_input_labels(
    store: &agent_value_store::Store,
    scope: &ScopeId,
) -> Result<FrozenInputs, String> {
    Ok(FrozenInputs {
        cell_weights_version: None,
        reference_version: latest_label(store, ConfigKind::Reference, scope)?,
        fallback_version: latest_label(store, ConfigKind::Fallback, scope)?,
        price_table_version: latest_label(store, ConfigKind::PriceTable, scope)?,
        classification_rules_version: latest_label(store, ConfigKind::ClassificationRules, scope)?,
    })
}

fn open_label(window: WindowIndex) -> String {
    format!("window-open-{}", window.0)
}

fn open_ms(
    store: &agent_value_store::Store,
    scope: &ScopeId,
    window: WindowIndex,
) -> Result<Option<i64>, String> {
    let Some(body) = payload(store, ConfigKind::ScopeConfig, scope, &open_label(window))? else {
        return Ok(None);
    };
    body.parse::<i64>()
        .map(Some)
        .map_err(|_| format!("window open time is not an integer: {body}"))
}

fn previous_counts(
    store: &agent_value_store::Store,
    scope: &ScopeId,
    window: WindowIndex,
    now: i64,
) -> Result<Vec<(CellId, u64)>, String> {
    let (from, to) = if window == WindowIndex::FIRST {
        (i64::MIN, now)
    } else {
        let prev = WindowIndex(window.0 - 1);
        let Some(start) = open_ms(store, scope, prev)? else {
            return Ok(Vec::new());
        };
        (start, now)
    };
    store
        .user_task_counts(scope, from, to)
        .map_err(|err| err.to_string())
}

fn prepare_cell_weights(
    store: &mut agent_value_store::Store,
    scope: &ScopeId,
    window: WindowIndex,
    now: i64,
) -> Result<Option<String>, String> {
    if store
        .cell_weights(scope, window)
        .map_err(|err| err.to_string())?
        .is_some()
    {
        return Ok(Some("frozen".to_string()));
    }
    let counts = previous_counts(store, scope, window, now)?;
    if counts.is_empty() && window == WindowIndex::FIRST {
        return Ok(None);
    }
    let mut configured: Vec<CellId> = counts.iter().map(|(cell, _)| cell.clone()).collect();
    let last = if window == WindowIndex::FIRST {
        None
    } else {
        store
            .cell_weights(scope, WindowIndex(window.0 - 1))
            .map_err(|err| err.to_string())?
    };
    if configured.is_empty() {
        if let Some(last) = last.as_ref() {
            configured = last.support().cloned().collect();
        }
    }
    if configured.is_empty() {
        return Ok(None);
    }
    let (weights, source) =
        derive_cell_weights(&configured, &counts, last.as_ref()).map_err(|err| err.to_string())?;
    store
        .set_cell_weights(scope, window, &weights)
        .map_err(|err| err.to_string())?;
    Ok(Some(omega_text(source).to_string()))
}

/// Freezes the next coding window with the store's prior task count and the
/// configuration labels already saved. An absent label stays absent.
pub fn freeze_coding_window(app: &AppState) -> Result<(), String> {
    let scope = coding_scope();
    let now = now_ms()?;
    store_cmd::with_store_mut(app, |store| {
        let mut ledger = match store
            .load_weight_ledger(&scope)
            .map_err(|err| err.to_string())?
        {
            Some(ledger) => ledger,
            None => WeightLedger::new(scope.clone()),
        };
        let window = ledger.next_unfrozen().map_err(|err| err.to_string())?;
        let prior = store
            .user_task_count_before(&scope, now)
            .map_err(|err| err.to_string())?;
        let mut inputs = frozen_input_labels(store, &scope)?;
        if let Some(label) = prepare_cell_weights(store, &scope, window, now)? {
            inputs.cell_weights_version = Some(label);
        }
        ledger
            .freeze_window(window, prior, inputs)
            .map_err(|err| err.to_string())?;
        store
            .save_weight_ledger(&ledger)
            .map_err(|err| err.to_string())?;
        store
            .put_config_version(
                ConfigKind::ScopeConfig,
                Some(&scope),
                &open_label(window),
                &now.to_string(),
                now,
            )
            .map_err(|err| err.to_string())?;
        Ok(())
    })
}

fn arm_of(row: &UserTaskRow) -> Result<Arm, String> {
    let model = ModelId::new(&row.model).map_err(|err| err.to_string())?;
    let route = Route::new(
        ProviderKind::parse(&row.provider_kind).map_err(|err| err.to_string())?,
        &row.host,
        Protocol::parse(&row.protocol).map_err(|err| err.to_string())?,
        &row.client,
    )
    .map_err(|err| err.to_string())?;
    Ok(Arm::new(model, route))
}

fn add_count(counts: &mut CellCounts, outcome: Outcome) -> Result<(), String> {
    let slot = match outcome {
        Outcome::Success => &mut counts.n1,
        Outcome::Failure => &mut counts.n0,
        Outcome::Cancel => &mut counts.n_c,
        // Still running is not a finished success or failure. It counts in
        // the submitted total the same way an undetermined task does.
        Outcome::Undetermined | Outcome::Pending => &mut counts.m,
    };
    *slot = slot
        .checked_add(1)
        .ok_or_else(|| "task count overflow".to_string())?;
    Ok(())
}

fn duration_seconds(row: &UserTaskRow) -> Option<f64> {
    if let Some(active) = row.active_ms {
        if active >= 0 {
            return Some(active as f64 / 1000.0);
        }
    }
    row.ended_ms
        .filter(|end| *end >= row.submitted_ms)
        .map(|end| (end - row.submitted_ms) as f64 / 1000.0)
}

struct Bucket {
    counts: CellCounts,
    cost_success: Vec<f64>,
    cost_failure: Vec<f64>,
    time_success: Vec<f64>,
    time_failure: Vec<f64>,
    waits: Vec<f64>,
}

fn bucket_of(rows: &[UserTaskRow]) -> Result<Bucket, String> {
    let mut bucket = Bucket {
        counts: CellCounts::default(),
        cost_success: Vec::new(),
        cost_failure: Vec::new(),
        time_success: Vec::new(),
        time_failure: Vec::new(),
        waits: Vec::new(),
    };
    for row in rows {
        add_count(&mut bucket.counts, row.outcome)?;
        let yuan = row
            .charged_micros
            .map(|micros| MicroYuan::new(micros).map(|money| money.yuan()))
            .transpose()
            .map_err(|err| err.to_string())?;
        let seconds = duration_seconds(row);
        match row.outcome {
            Outcome::Success => {
                if let Some(yuan) = yuan {
                    bucket.cost_success.push(yuan);
                }
                if let Some(seconds) = seconds {
                    bucket.time_success.push(seconds);
                }
            }
            Outcome::Failure => {
                if let Some(yuan) = yuan {
                    bucket.cost_failure.push(yuan);
                }
                if let Some(seconds) = seconds {
                    bucket.time_failure.push(seconds);
                }
            }
            _ => {}
        }
        if let Some(wait) = row.wait_ms {
            if wait >= 0 {
                bucket.waits.push(wait as f64 / 1000.0);
            }
        }
    }
    Ok(bucket)
}

fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<f64>() / values.len() as f64)
    }
}

enum MeasureAs<'a> {
    Terminal,
    Candidate {
        reference: &'a [CellSnap],
        fallback: &'a [CellSnap],
        missing: &'a CellValues,
    },
}

fn measure_arm(
    buckets: &BTreeMap<Arm, BTreeMap<CellId, Bucket>>,
    arm: &Arm,
    mode: QualityMode,
    ratings: &[Rating],
    model: &RetryModel,
    role: MeasureAs<'_>,
) -> Result<Vec<CellSnap>, String> {
    let Some(cells) = buckets.get(arm) else {
        return Ok(Vec::new());
    };
    let mut snaps = Vec::new();
    for (cell, bucket) in cells {
        let measured = match &role {
            MeasureAs::Terminal => Role::Terminal,
            MeasureAs::Candidate {
                reference,
                fallback,
                missing,
            } => {
                let reference = reference
                    .iter()
                    .find(|snap| &snap.cell == cell)
                    .map(|snap| &snap.values)
                    .unwrap_or(*missing);
                let fallback = fallback
                    .iter()
                    .find(|snap| &snap.cell == cell)
                    .map(|snap| &snap.values);
                Role::Candidate {
                    reference,
                    fallback,
                }
            }
        };
        snaps.push(cell_from_bucket(
            bucket, arm, cell, mode, ratings, measured, model,
        )?);
    }
    Ok(snaps)
}

fn cell_from_bucket(
    bucket: &Bucket,
    arm: &Arm,
    cell: &CellId,
    mode: QualityMode,
    ratings: &[Rating],
    role: Role<'_>,
    model: &RetryModel,
) -> Result<CellSnap, String> {
    if bucket.counts.completed().map_err(|err| err.to_string())? == 0 {
        return Ok(CellSnap {
            cell: cell.clone(),
            counts: bucket.counts,
            values: CellValues::na(),
            cost_source: None,
        });
    }
    let pass = PassStats::new(
        &Prior::Jeffreys,
        bucket.counts.n1,
        bucket.counts.n0,
        bucket.counts.n_c,
        model,
    )
    .map_err(|err| err.to_string())?;
    let priced = unit_price(&bucket.cost_success, None, None).map_err(|err| err.to_string())?;
    let q_hat = quality::q_hat(mode, arm, cell, ratings, &[]).map_err(|err| err.to_string())?;
    let inputs = CellInputs {
        counts: bucket.counts,
        retry: pass.retry,
        retry_c: pass.retry_c,
        cost: DimInputs {
            success: bucket.cost_success.clone(),
            failure: bucket.cost_failure.clone(),
            ..DimInputs::default()
        },
        time: DimInputs {
            success: bucket.time_success.clone(),
            failure: bucket.time_failure.clone(),
            ..DimInputs::default()
        },
        delay: agent_value_core::cell_values::DelayInputs {
            own: mean(&bucket.waits),
            ..agent_value_core::cell_values::DelayInputs::default()
        },
        q_hat,
        matched_failures: 0,
        q_match: None,
    };
    let values = cell_values(&inputs, role).map_err(|err| err.to_string())?;
    Ok(CellSnap {
        cell: cell.clone(),
        counts: bucket.counts,
        values,
        cost_source: priced.map(|price| price_text(price.source).to_string()),
    })
}

fn store_result(app: &AppState) -> Result<WindowResult, String> {
    match load_store(app)? {
        StoreLoad::Empty(view) => Ok(*view),
        StoreLoad::Ready(snapshot) => score(&snapshot),
    }
}

fn load_store(app: &AppState) -> Result<StoreLoad, String> {
    store_cmd::with_store(app, |store| {
        let scope = coding_scope();
        let Some(ledger) = store
            .load_weight_ledger(&scope)
            .map_err(|err| err.to_string())?
        else {
            let mut view = blank("store", false);
            view.empty_reason = Some("还没有开始的窗".to_string());
            view.authenticity_grade = AuthenticityGrade::B.as_str().to_string();
            view.authenticity_source = PROBES_OFF.to_string();
            view.authenticity_undetermined = true;
            view.log_integrity_source = PROBES_OFF.to_string();
            return Ok(StoreLoad::Empty(Box::new(view)));
        };
        let Some(index) = ledger.last_frozen() else {
            let mut view = blank("store", false);
            view.empty_reason = Some("还没有开始的窗".to_string());
            view.authenticity_grade = AuthenticityGrade::B.as_str().to_string();
            view.authenticity_source = PROBES_OFF.to_string();
            view.authenticity_undetermined = true;
            view.log_integrity_source = PROBES_OFF.to_string();
            return Ok(StoreLoad::Empty(Box::new(view)));
        };
        let frozen = ledger.frozen(index).map_err(|err| err.to_string())?;
        let Some(start) = open_ms(store, &scope, index)? else {
            let mut view = blank("store", false);
            view.window_index = Some(index.0);
            view.is_first = frozen.is_first_window();
            view.first_window_label = frozen.first_window_label().map(str::to_owned);
            view.empty_reason = Some("这个窗没有记下起点，不能把任务归到窗里。".to_string());
            return Ok(StoreLoad::Empty(Box::new(view)));
        };
        let end = open_ms(store, &scope, index.next().map_err(|err| err.to_string())?)?
            .unwrap_or(i64::MAX);
        score_store_window(store, &ledger, index, start, end)
    })
}

fn score_store_window(
    store: &agent_value_store::Store,
    ledger: &WeightLedger,
    index: WindowIndex,
    start: i64,
    end: i64,
) -> Result<StoreLoad, String> {
    let scope = coding_scope();
    let frozen = ledger.frozen(index).map_err(|err| err.to_string())?;
    let tasks = store
        .list_user_tasks(&scope, start, end)
        .map_err(|err| err.to_string())?;
    let mut grouped: BTreeMap<Arm, BTreeMap<CellId, Vec<UserTaskRow>>> = BTreeMap::new();
    for row in tasks {
        let arm = arm_of(&row)?;
        grouped
            .entry(arm)
            .or_default()
            .entry(row.cell.clone())
            .or_default()
            .push(row);
    }
    let stored_weights = store
        .cell_weights(&scope, index)
        .map_err(|err| err.to_string())?;
    let (cell_weights, omega_source) = if let Some(weights) = stored_weights {
        let source = frozen
            .inputs()
            .cell_weights_version
            .clone()
            .unwrap_or_else(|| "frozen".to_string());
        (weights, Some(source))
    } else {
        let mut configured = BTreeSet::new();
        for cells in grouped.values() {
            configured.extend(cells.keys().cloned());
        }
        if configured.is_empty() {
            let mut view = blank("store", false);
            view.window_index = Some(index.0);
            view.is_first = frozen.is_first_window();
            view.first_window_label = frozen.first_window_label().map(str::to_owned);
            view.weight_source = Some(frozen.weight_version().source().as_str().to_string());
            view.empty_reason = Some("本窗还没有用户任务。".to_string());
            view.reference_version = frozen.inputs().reference_version.clone();
            view.fallback_version = frozen.inputs().fallback_version.clone();
            view.price_table_version = frozen.inputs().price_table_version.clone();
            view.classification_rules_version =
                frozen.inputs().classification_rules_version.clone();
            view.cell_weights_version = frozen.inputs().cell_weights_version.clone();
            view.authenticity_grade = AuthenticityGrade::B.as_str().to_string();
            view.authenticity_source = PROBES_OFF.to_string();
            view.authenticity_undetermined = true;
            view.log_integrity_source = PROBES_OFF.to_string();
            return Ok(StoreLoad::Empty(Box::new(view)));
        }
        let previous = if index == WindowIndex::FIRST {
            Vec::new()
        } else {
            let prev = WindowIndex(index.0 - 1);
            match open_ms(store, &scope, prev)? {
                Some(prev_start) => store
                    .user_task_counts(&scope, prev_start, start)
                    .map_err(|err| err.to_string())?,
                None => Vec::new(),
            }
        };
        let last = if index == WindowIndex::FIRST {
            None
        } else {
            store
                .cell_weights(&scope, WindowIndex(index.0 - 1))
                .map_err(|err| err.to_string())?
        };
        // A cell that had tasks last window still belongs to the layer, even
        // when this window has not seen it yet.
        for (cell, _) in &previous {
            configured.insert(cell.clone());
        }
        if let Some(weights) = last.as_ref() {
            configured.extend(weights.support().cloned());
        }
        let configured: Vec<CellId> = configured.into_iter().collect();
        let (weights, source) = derive_cell_weights(&configured, &previous, last.as_ref())
            .map_err(|err| err.to_string())?;
        (weights, Some(omega_text(source).to_string()))
    };

    let reference_name = match frozen.inputs().reference_version.as_deref() {
        Some(label) => payload(store, ConfigKind::Reference, &scope, label)?,
        None => None,
    };
    let fallback_name = match frozen.inputs().fallback_version.as_deref() {
        Some(label) => payload(store, ConfigKind::Fallback, &scope, label)?,
        None => None,
    };
    let model = RetryModel::with_default_k(0.0).map_err(|err| err.to_string())?;
    let mut ratings = Vec::new();
    let mut buckets: BTreeMap<Arm, BTreeMap<CellId, Bucket>> = BTreeMap::new();
    for (arm, cells) in &grouped {
        for (cell, rows) in cells {
            for row in rows {
                if let Some(stars) = row.rating {
                    ratings.push(Rating {
                        arm: arm.clone(),
                        cell: cell.clone(),
                        outcome: row.outcome,
                        stars,
                    });
                }
            }
            buckets
                .entry(arm.clone())
                .or_default()
                .insert(cell.clone(), bucket_of(rows)?);
        }
    }
    let arms: Vec<Arm> = buckets.keys().cloned().collect();
    let mode = if arms.is_empty() {
        QualityMode::SuccessRate
    } else {
        quality::quality_mode(&arms, &cell_weights, &ratings).map_err(|err| err.to_string())?
    };

    let reference_arm = grouped
        .keys()
        .find(|arm| {
            reference_name
                .as_ref()
                .is_some_and(|name| arm.to_string() == *name)
        })
        .cloned();
    let fallback_arm = grouped
        .keys()
        .find(|arm| {
            fallback_name
                .as_ref()
                .is_some_and(|name| arm.to_string() == *name)
        })
        .cloned();
    let reference_cells = if let Some(arm) = &reference_arm {
        measure_arm(&buckets, arm, mode, &ratings, &model, MeasureAs::Terminal)?
    } else {
        Vec::new()
    };
    let fallback_cells = if let Some(arm) = &fallback_arm {
        if reference_arm.as_ref() != Some(arm) {
            measure_arm(&buckets, arm, mode, &ratings, &model, MeasureAs::Terminal)?
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    let missing = CellValues::na();
    let mut arm_snaps = Vec::new();
    for arm in &arms {
        let terminal = reference_arm.as_ref() == Some(arm)
            || fallback_arm.as_ref() == Some(arm)
            || reference_cells.is_empty();
        let cells = if terminal {
            measure_arm(&buckets, arm, mode, &ratings, &model, MeasureAs::Terminal)?
        } else {
            measure_arm(
                &buckets,
                arm,
                mode,
                &ratings,
                &model,
                MeasureAs::Candidate {
                    reference: &reference_cells,
                    fallback: &fallback_cells,
                    missing: &missing,
                },
            )?
        };
        arm_snaps.push(ArmSnap {
            id: arm.to_string(),
            cells,
        });
    }

    let mut snapshot = Snapshot {
        source: "store".to_string(),
        fixture: false,
        scope: scope.to_string(),
        window_index: Some(index.0),
        is_first: frozen.is_first_window(),
        weight_source: Some(frozen.weight_version().source().as_str().to_string()),
        weights: frozen.weights(),
        omega_source,
        inputs: frozen.inputs().clone(),
        authenticity: AuthenticityGrade::B,
        authenticity_source: PROBES_OFF.to_string(),
        log_integrity: true,
        log_integrity_source: PROBES_OFF.to_string(),
        cell_weights,
        reference_id: reference_arm.as_ref().map(ToString::to_string),
        reference: reference_cells,
        arms: arm_snaps,
    };
    if snapshot.reference_id.is_none() {
        snapshot.reference = Vec::new();
    }
    Ok(StoreLoad::Ready(Box::new(snapshot)))
}

#[cfg(test)]
mod tests {
    use agent_value_core::window::FIRST_WINDOW_LABEL;
    use agent_value_store::ConfigKind;

    use super::{example6, result, score, tie, FIRST_WINDOW_LABEL as LABEL};
    use crate::elicit_cmd::begin_window;
    use crate::store_cmd::{self, AppState};

    fn arm_v<'a>(view: &'a super::WindowResult, id: &str) -> Option<&'a str> {
        view.arms
            .iter()
            .find(|arm| arm.id == id)
            .and_then(|arm| arm.v_text.as_deref())
    }

    #[test]
    fn example6_frontier_and_diagnostic_anchor_match_core() {
        let view = score(&example6()).unwrap();
        assert!(view.fixture);
        assert_eq!(view.frontier_off, vec!["b".to_string(), "own".to_string()]);
        assert_eq!(view.frontier_rec, view.frontier_off);
        assert_eq!(view.official_top, vec!["own".to_string()]);
        assert!(!view.tied);
        assert_eq!(view.diagnostic_anchor_c_text.as_deref(), Some("0.7875"));
        assert_eq!(arm_v(&view, "own"), Some("102.60"));
        assert!(view.v_withheld.is_none());
        assert_eq!(view.anchor_tier.as_deref(), Some("II"));
        assert_eq!(
            view.tier_ii_legend.as_deref(),
            Some(agent_value_core::labels::TIER_II_LEGEND)
        );
        assert_eq!(view.weight_source.as_deref(), Some("example-fixture"));
        assert_eq!(view.omega_source.as_deref(), Some("example-fixture"));
        assert_eq!(view.reference_version.as_deref(), Some("example-6"));
        assert!(view
            .arms
            .iter()
            .any(|arm| arm.id == "route" && !arm.eligible));
    }

    #[test]
    fn example7_keeps_the_frontier_and_withholds_the_pick() {
        let view = result(&AppState::open_in_memory().unwrap(), "example7").unwrap();
        assert_eq!(view.frontier_off, vec!["own".to_string()]);
        assert!(view.official_top.is_empty());
        assert!(!view.tied);
        assert_eq!(view.diagnostic_anchor_c_text.as_deref(), Some("0.9000"));
        assert_eq!(view.measured_anchor_c_text.as_deref(), Some("0.9000"));
        assert_eq!(view.anchor_tier.as_deref(), Some("III"));
        assert!(view
            .anchor_headers
            .iter()
            .any(|header| header.contains("真不合格")));
        assert!(arm_v(&view, "own").is_none());
    }

    #[test]
    fn example8_frontier_and_anchors_match_core() {
        let view = result(&AppState::open_in_memory().unwrap(), "example8").unwrap();
        assert_eq!(view.frontier_off, vec!["cheap".to_string()]);
        assert_eq!(view.official_top, vec!["cheap".to_string()]);
        assert_eq!(view.measured_anchor_c_text.as_deref(), Some("0.1650"));
        assert_eq!(view.diagnostic_anchor_c_text.as_deref(), Some("0.6000"));
        assert_eq!(arm_v(&view, "cheap"), Some("129.87"));
    }

    #[test]
    fn tie_reports_the_whole_group() {
        let view = score(&tie(false)).unwrap();
        assert_eq!(view.frontier_off, vec!["X".to_string(), "Y".to_string()]);
        assert_eq!(view.official_top, vec!["X".to_string(), "Y".to_string()]);
        assert!(view.tied);
        assert_eq!(arm_v(&view, "X"), arm_v(&view, "Y"));
        assert!(arm_v(&view, "X").is_some());
    }

    #[test]
    fn first_window_keeps_the_frontier_and_withholds_v() {
        let view = score(&tie(true)).unwrap();
        assert!(view.is_first);
        assert_eq!(view.first_window_label.as_deref(), Some(LABEL));
        assert_eq!(view.v_withheld.as_deref(), Some(FIRST_WINDOW_LABEL));
        assert_eq!(view.frontier_off, vec!["X".to_string(), "Y".to_string()]);
        assert!(view.official_top.is_empty());
        assert!(!view.tied);
        assert!(view.arms.iter().all(|arm| arm.v_text.is_none()));
        assert!(view.arms.iter().all(|arm| arm.cost_text.is_some()));
    }

    #[test]
    fn begin_window_records_the_store_prior_count_and_config_labels() {
        let app = AppState::open_in_memory().unwrap();
        let scope = super::coding_scope();
        store_cmd::with_store(&app, |store| {
            store
                .put_config_version(ConfigKind::Reference, Some(&scope), "ref-v1", "ref-arm", 1)
                .map_err(|err| err.to_string())?;
            store
                .put_config_version(ConfigKind::PriceTable, Some(&scope), "prices-1", "table", 1)
                .map_err(|err| err.to_string())?;
            let mut task = agent_value_core::records::TaskRecord::new(
                "prior",
                scope.clone(),
                agent_value_core::cells::CellId::user("fix").unwrap(),
                sample_arm(),
                1,
            );
            task.outcome = agent_value_core::records::Outcome::Success;
            store.insert_task(&task).map_err(|err| err.to_string())?;
            Ok(())
        })
        .unwrap();
        begin_window(&app).unwrap();
        let ledger = store_cmd::read_weight_ledger(&app, &scope).unwrap();
        let frozen = ledger
            .frozen(agent_value_core::window::WindowIndex::FIRST)
            .unwrap();
        assert!(!frozen.is_first_window());
        assert_eq!(frozen.inputs().reference_version.as_deref(), Some("ref-v1"));
        assert_eq!(
            frozen.inputs().price_table_version.as_deref(),
            Some("prices-1")
        );
        assert_eq!(frozen.inputs().fallback_version, None);
        assert_eq!(
            frozen.inputs().cell_weights_version.as_deref(),
            Some("previous-window")
        );
    }

    fn sample_arm() -> agent_value_core::arm::Arm {
        agent_value_core::arm::Arm::new(
            agent_value_core::arm::ModelId::new("example-model-1").unwrap(),
            agent_value_core::arm::Route::new(
                agent_value_core::arm::ProviderKind::Official,
                "api.vendor.example",
                agent_value_core::arm::Protocol::AnthropicMessages,
                "example-cli",
            )
            .unwrap(),
        )
    }

    #[test]
    fn store_window_counts_tasks_inside_the_window_and_prices_them_with_the_core() {
        let app = AppState::open_in_memory().unwrap();
        let scope = super::coding_scope();
        let arm = sample_arm();
        store_cmd::with_store(&app, |store| {
            store
                .put_config_version(
                    ConfigKind::Reference,
                    Some(&scope),
                    "ref-v1",
                    &arm.to_string(),
                    1,
                )
                .map_err(|err| err.to_string())?;
            Ok(())
        })
        .unwrap();
        begin_window(&app).unwrap();
        let opened = store_cmd::with_store(&app, |store| {
            super::open_ms(store, &scope, agent_value_core::window::WindowIndex::FIRST)
        })
        .unwrap()
        .unwrap();
        store_cmd::with_store(&app, |store| {
            insert_priced(store, &scope, &arm, "before", opened - 1, true, 9.0);
            for n in 0..8 {
                insert_priced(
                    store,
                    &scope,
                    &arm,
                    &format!("ok-{n}"),
                    opened + n,
                    true,
                    0.60,
                );
            }
            for n in 0..2 {
                insert_priced(
                    store,
                    &scope,
                    &arm,
                    &format!("bad-{n}"),
                    opened + 100 + n,
                    false,
                    1.20,
                );
            }
            Ok(())
        })
        .unwrap();
        let view = result(&app, "store").unwrap();
        assert!(view.is_first);
        assert_eq!(view.reference_version.as_deref(), Some("ref-v1"));
        assert_eq!(view.authenticity_source, "probes-off");
        assert!(view.authenticity_undetermined);
        assert_eq!(view.weight_source.as_deref(), Some("default"));
        let row = view
            .arms
            .iter()
            .find(|row| row.id == arm.to_string())
            .unwrap();
        assert_eq!(row.n_comp, 10);
        assert!(row.v_text.is_none());
        assert_eq!(view.v_withheld.as_deref(), Some(FIRST_WINDOW_LABEL));
        let expected = expected_cost();
        assert_eq!(row.cost_text.as_deref(), Some(expected.as_str()));
        assert_eq!(row.cost_source.as_deref(), Some("own"));
        assert!(view.frontier_off.is_empty() || view.frontier_off.iter().all(|id| !id.is_empty()));
    }

    fn insert_priced(
        store: &agent_value_store::Store,
        scope: &agent_value_core::scope::ScopeId,
        arm: &agent_value_core::arm::Arm,
        id: &str,
        at: i64,
        success: bool,
        yuan: f64,
    ) {
        let mut task = agent_value_core::records::TaskRecord::new(
            id,
            scope.clone(),
            agent_value_core::cells::CellId::user("fix").unwrap(),
            arm.clone(),
            at,
        );
        task.outcome = if success {
            agent_value_core::records::Outcome::Success
        } else {
            agent_value_core::records::Outcome::Failure
        };
        task.ended_ms = Some(at + 2_000);
        task.active_ms = Some(if success { 1_000 } else { 2_000 });
        task.wait_ms = Some(100);
        store.insert_task(&task).unwrap();
        let request = agent_value_core::records::RequestRecord {
            id: format!("req-{id}"),
            task_id: Some(id.to_string()),
            arm: arm.clone(),
            auxiliary: false,
            returned_model: None,
            response_id: None,
            key_env_var: None,
            start_ms: at,
            first_token_ms: None,
            end_ms: at + 1_000,
            usage: agent_value_core::records::Usage::default(),
            charged: Some(agent_value_core::records::MicroYuan::from_yuan(yuan).unwrap()),
        };
        store.insert_request(&request).unwrap();
    }

    fn expected_cost() -> String {
        use agent_value_core::cell_values::{cell_values, CellInputs, DimInputs, Role};
        use agent_value_core::cells::{CellId, CellWeights};
        use agent_value_core::quality::{q_hat, QualityMode};
        use agent_value_core::retry::{PassStats, Prior, RetryModel};
        use agent_value_core::status::CellCounts;
        use agent_value_core::value::aggregate_layer;

        let counts = CellCounts {
            n1: 8,
            n0: 2,
            n_c: 0,
            m: 0,
        };
        let model = RetryModel::with_default_k(0.0).unwrap();
        let pass = PassStats::new(&Prior::Jeffreys, 8, 2, 0, &model).unwrap();
        let cell = CellId::user("fix").unwrap();
        let arm = sample_arm();
        let q = q_hat(QualityMode::SuccessRate, &arm, &cell, &[], &[]).unwrap();
        let inputs = CellInputs {
            counts,
            retry: pass.retry,
            retry_c: pass.retry_c,
            cost: DimInputs {
                success: vec![0.60; 8],
                failure: vec![1.20; 2],
                ..DimInputs::default()
            },
            time: DimInputs {
                success: vec![1.0; 8],
                failure: vec![2.0; 2],
                ..DimInputs::default()
            },
            delay: agent_value_core::cell_values::DelayInputs {
                own: Some(0.1),
                ..Default::default()
            },
            q_hat: q,
            matched_failures: 0,
            q_match: None,
        };
        let values = cell_values(&inputs, Role::Terminal).unwrap();
        let weights = CellWeights::equal(std::slice::from_ref(&cell)).unwrap();
        let aggregate = aggregate_layer(&weights, &[(cell, values.star)]).unwrap();
        super::print4(aggregate.c.unwrap())
    }
}

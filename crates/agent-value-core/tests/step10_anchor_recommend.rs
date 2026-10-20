//! Step 10 (v22 §6 closure, §7 tiers and clamp, §13 R/F selection;
//! DECISIONS G, T–Z): reference certification, layer and overall
//! recommendation, the coarse-bill tag, `not_admitted`, and examples 6/7/8
//! composed from E_d to the official pick.
//! Expected values come from the spec text and its examples; never fitted.

use agent_value_core::anchor::{
    certify_reference_layer, clip, diagnostic_reference_anchor, layer_value, AnchorTier,
    LayerAnchor, TierIIIReason,
};
use agent_value_core::cell_values::{CellNote, CellValues, Triple};
use agent_value_core::cells::{CellId, CellWeights};
use agent_value_core::diagnostic::{
    diagnose_arm, diagnostic_admitted, not_admitted_reasons, DiagnosticRow, NotAdmittedReason,
};
use agent_value_core::eligibility::{assess_eligibility, AuthenticityGrade};
use agent_value_core::frontier::{layer_frontiers, FrontierArm};
use agent_value_core::labels;
use agent_value_core::recommend::{
    layer_arm_from_cells, layer_pick, overall_arm, overall_reference, overall_top, LayerArm,
    OverallArmLayer, OverallGate,
};
use agent_value_core::retry::{Prior, RetryModel};
use agent_value_core::selection::{
    fallback_failures, resolve_fallback, select_reference, FallbackCandidate, FallbackDecision,
    FallbackFailure, FallbackLayer, ReferenceCandidate, FALLBACK_REVERT_NOTE,
};
use agent_value_core::status::{layer_status, CellCounts, CellInput, LayerStatus};
use agent_value_core::value::{value_score, Floors, Point};
use agent_value_core::weights::Weights;

const TOL: f64 = 1e-9;

fn close(a: f64, b: f64) {
    assert!((a - b).abs() <= TOL, "{a} vs {b}");
}

fn prints_as(x: f64, printed: &str) {
    let d = printed.split('.').nth(1).map_or(0, str::len);
    assert_eq!(format!("{x:.d$}"), printed, "{x} should print as {printed}");
}

fn cell(s: &str) -> CellId {
    CellId::user(s).unwrap()
}

fn ids(n: usize) -> Vec<CellId> {
    (1..=n).map(|i| cell(&format!("c{i:02}"))).collect()
}

fn eq_w() -> Weights {
    Weights::equal()
}

fn abg(a: f64) -> Weights {
    Weights::new(a, (1.0 - a) / 2.0, (1.0 - a) / 2.0).unwrap()
}

fn floors() -> Floors {
    Floors::default()
}

fn pt(c: f64) -> Point {
    Point { c, t: 10.0, q: 0.8 }
}

fn model() -> RetryModel {
    RetryModel::with_default_k(0.0).unwrap()
}

fn counts(n1: u64, n0: u64, n_c: u64) -> CellCounts {
    CellCounts { n1, n0, n_c, m: 0 }
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
/// n1 = 6, n0 = 4: F_bad under Jeffreys, π = 0 (step 9 example 7).
const BAD: CellCounts = CellCounts {
    n1: 6,
    n0: 4,
    n_c: 0,
    m: 0,
};

/// Status for cells with the given counts (`None` = miss).
fn status(w: &CellWeights, cells: &[CellId], c: &[Option<CellCounts>]) -> LayerStatus {
    let rows: Vec<CellInput> = cells
        .iter()
        .zip(c)
        .filter_map(|(id, c)| {
            c.map(|counts| CellInput {
                cell: id.clone(),
                counts,
                prior: Prior::Jeffreys,
            })
        })
        .collect();
    layer_status(w, &rows, &model()).unwrap()
}

fn triples(cells: &[CellId], v: &[Triple]) -> Vec<(CellId, Triple)> {
    cells.iter().cloned().zip(v.iter().copied()).collect()
}

fn tc(c: f64) -> Triple {
    Triple::new(c, 10.0, 0.8)
}

/// Example 6 reference: 11 valid cells at 0.60, cell 12 F_thin at 2.85.
fn ex6() -> (CellWeights, Vec<CellId>, LayerStatus, Vec<(CellId, Triple)>) {
    let cells = ids(12);
    let w = CellWeights::equal(&cells).unwrap();
    let mut c = vec![Some(VALID); 11];
    c.push(Some(THIN));
    let st = status(&w, &cells, &c);
    let mut v = vec![tc(0.60); 11];
    v.push(tc(2.85));
    let t = triples(&cells, &v);
    (w, cells, st, t)
}

/// Example 8 reference: 3 valid cells at 0.60, 9 F_thin cells at 0.02.
fn ex8() -> (CellWeights, Vec<CellId>, LayerStatus, Vec<(CellId, Triple)>) {
    let cells = ids(12);
    let w = CellWeights::equal(&cells).unwrap();
    let mut c = vec![Some(VALID); 3];
    c.extend([Some(THIN); 9]);
    let st = status(&w, &cells, &c);
    let mut v = vec![tc(0.60); 3];
    v.extend([tc(0.02); 9]);
    (w, cells.clone(), st, triples(&cells, &v))
}

fn all_valid(
    n: usize,
    t: Triple,
) -> (CellWeights, Vec<CellId>, LayerStatus, Vec<(CellId, Triple)>) {
    let cells = ids(n);
    let w = CellWeights::equal(&cells).unwrap();
    let st = status(&w, &cells, &vec![Some(VALID); n]);
    let tr = triples(&cells, &vec![t; n]);
    (w, cells, st, tr)
}

fn cv(t: Triple, robust: Triple, dominance: bool, lower_bound: bool) -> CellValues {
    CellValues {
        star: t,
        robust,
        robust_dominance: dominance,
        notes: if lower_bound {
            vec![CellNote::CancelLowerBoundOnly]
        } else {
            vec![]
        },
    }
}

fn arm_cells(cells: &[CellId], v: CellValues) -> Vec<(CellId, CellValues)> {
    cells.iter().map(|c| (c.clone(), v.clone())).collect()
}

// ---- §7 tiers (DECISIONS T) ---------------------------------------------------

#[test]
fn tier_i_anchor_is_measured_and_route_arm_scores_100() {
    let (w, _, st, tr) = all_valid(4, Triple::new(0.5, 8.0, 0.9));
    let a = certify_reference_layer(&st, &w, &tr).unwrap();
    assert_eq!(a.tier, AnchorTier::I);
    assert!(a.tier_iii_reasons.is_empty());
    assert_eq!(a.anchor, a.measured);
    assert!(a.clamp.is_none());
    // DECISIONS H: tier I pure-route V == 100.
    let v = layer_value(&a, a.measured.unwrap(), &eq_w(), &floors()).unwrap();
    close(v.unwrap(), 100.0);
}

#[test]
fn fewer_than_three_valid_cells_is_tier_iii_even_when_all_valid() {
    // §7 line 484 "先命中者为准", line 493 "有效参照单元数 <3".
    let (w, _, st, tr) = all_valid(2, tc(0.5));
    let a = certify_reference_layer(&st, &w, &tr).unwrap();
    assert_eq!(a.tier, AnchorTier::III);
    assert_eq!(a.tier_iii_reasons, vec![TierIIIReason::FewValid]);
    assert!(a.anchor.is_none());
    assert!(layer_value(&a, pt(0.5), &eq_w(), &floors())
        .unwrap()
        .is_none());
    // Exactly 3 valid cells certify.
    let (w, _, st, tr) = all_valid(3, tc(0.5));
    assert_eq!(
        certify_reference_layer(&st, &w, &tr).unwrap().tier,
        AnchorTier::I
    );
}

#[test]
fn tier_iii_lists_every_reason_in_spec_order() {
    let cells = ids(4);
    let w = CellWeights::equal(&cells).unwrap();
    // c01 miss, c02 bad, c03 thin, c04 valid; c04 has an NA dimension.
    let st = status(&w, &cells, &[None, Some(BAD), Some(THIN), Some(VALID)]);
    let tr = triples(
        &cells[1..],
        &[tc(1.0), tc(1.0), Triple { q: None, ..tc(1.0) }],
    );
    let a = certify_reference_layer(&st, &w, &tr).unwrap();
    assert_eq!(a.tier, AnchorTier::III);
    assert_eq!(
        a.tier_iii_reasons,
        vec![
            TierIIIReason::Miss,
            TierIIIReason::Bad,
            TierIIIReason::Na,
            TierIIIReason::FewValid
        ]
    );
    let hdr: Vec<String> = a
        .tier_iii_reasons
        .iter()
        .map(|r| labels::tier_iii_header(*r, &a))
        .collect();
    assert_eq!(hdr[0], "参照组缺单元（0.2500）");
    assert_eq!(hdr[1], "参照组存在真不合格单元（0.2500），需补测或更换参照");
    assert_eq!(hdr[2], "参照组该层存在无法定价的单元（NA）");
    assert_eq!(hdr[3], "参照组有效单元不足 3 个");
}

#[test]
fn thin_plus_bad_is_tier_iii_not_tier_ii() {
    // Order III → II: one F_bad overrides the F_thin clamp path.
    let cells = ids(12);
    let w = CellWeights::equal(&cells).unwrap();
    let mut c = vec![Some(VALID); 10];
    c.push(Some(THIN));
    c.push(Some(BAD));
    let st = status(&w, &cells, &c);
    let a = certify_reference_layer(&st, &w, &triples(&cells, &[tc(0.6); 12])).unwrap();
    assert_eq!(a.tier, AnchorTier::III);
    assert_eq!(a.tier_iii_reasons, vec![TierIIIReason::Bad]);
}

// ---- §7 tier II clamp -----------------------------------------------------------

#[test]
fn clip_is_min_max() {
    close(clip(0.7875, 0.54, 0.66), 0.66);
    close(clip(0.165, 0.54, 0.66), 0.54);
    close(clip(0.6, 0.54, 0.66), 0.6);
}

#[test]
fn example_6_tier_ii_upper_clamp_and_route_scores() {
    let (w, _, st, tr) = ex6();
    let a = certify_reference_layer(&st, &w, &tr).unwrap();
    assert_eq!(a.tier, AnchorTier::II);
    let [c, t, d] = a.clamp.unwrap();
    close(c.measured, 0.7875);
    prints_as(c.measured, "0.7875");
    close(c.valid_mean, 0.60);
    close(c.lower, 0.54);
    close(c.upper, 0.66);
    close(c.clipped, 0.66);
    assert!(c.clamped);
    assert!(!t.clamped && !d.clamped);
    let anchor = a.anchor.unwrap();
    close(anchor.c, 0.66);
    close(anchor.t, 10.0);
    close(anchor.q, 0.8);
    close(a.w_thin, 1.0 / 12.0);
    assert_eq!(
        labels::tier_ii_header(&a),
        "参照组含样本不足单元（0.0833），锚点已夹逼（θ=0.10）"
    );
    // Pure-route arm: layer triple = measured (A_C, A_T, 1 − A_D).
    let route = a.measured.unwrap();
    let v = |w: &Weights| layer_value(&a, route, w, &floors()).unwrap().unwrap();
    close(v(&eq_w()), 100.0 * 0.864f64.powf(1.0 / 3.0));
    prints_as(v(&eq_w()), "95.24");
    prints_as(v(&abg(0.5)), "92.95");
    prints_as(v(&abg(0.2)), "97.12");
    // Same-level own arm at 0.60.
    let own = layer_value(&a, pt(0.60), &eq_w(), &floors())
        .unwrap()
        .unwrap();
    close(own, 100.0 * 1.08f64.powf(1.0 / 3.0));
    prints_as(own, "102.60");
    // Cell values are never changed: the measured layer cost is still A_C.
    close(route.c, 0.7875);
}

#[test]
fn example_8_tier_ii_lower_clamp() {
    let (w, _, st, tr) = ex8();
    let a = certify_reference_layer(&st, &w, &tr).unwrap();
    assert_eq!(a.tier, AnchorTier::II);
    let c = a.clamp.unwrap()[0];
    close(c.measured, 0.165);
    close(c.valid_mean, 0.60);
    close(c.clipped, 0.54);
    let route = layer_value(&a, a.measured.unwrap(), &eq_w(), &floors())
        .unwrap()
        .unwrap();
    prints_as(route, "129.87");
    // The 0.60 arm against the clamped anchor; 74.89 only without the clamp.
    let own = layer_value(&a, pt(0.60), &eq_w(), &floors())
        .unwrap()
        .unwrap();
    close(own, 100.0 * (0.69f64 / 0.75).powf(1.0 / 3.0));
    let unclamped = value_score(a.measured.unwrap(), pt(0.60), &eq_w(), &floors()).unwrap();
    prints_as(unclamped, "74.89");
    assert!((own - unclamped).abs() > 1.0);
}

#[test]
fn tier_ii_clamps_time_and_defect_not_quality() {
    let cells = ids(12);
    let w = CellWeights::equal(&cells).unwrap();
    let mut c = vec![Some(VALID); 11];
    c.push(Some(THIN));
    let st = status(&w, &cells, &c);
    let mut v = vec![Triple::new(0.6, 10.0, 0.8); 11];
    // Thin cell: slow and poor quality.
    v.push(Triple::new(0.6, 70.0, 0.2));
    let a = certify_reference_layer(&st, &w, &triples(&cells, &v)).unwrap();
    assert_eq!(a.tier, AnchorTier::II);
    let [cc, t, d] = a.clamp.unwrap();
    assert!(!cc.clamped);
    // A_T = (110 + 70)/12 = 15 → clip to 11.
    close(t.measured, 15.0);
    close(t.clipped, 11.0);
    // A_D = (11·0.2 + 0.8)/12 = 0.25, a_D = 0.2 → 0.22, Q = 0.78.
    close(d.measured, 0.25);
    close(d.valid_mean, 0.2);
    close(d.clipped, 0.22);
    let anchor = a.anchor.unwrap();
    close(anchor.t, 11.0);
    close(anchor.q, 0.78);
    // Clamping Q directly would give clip(0.75, 0.72, 0.88) = 0.75.
    assert!((anchor.q - 0.75).abs() > 1e-6);
}

#[test]
fn theta_window_boundaries_are_inclusive_and_exact() {
    // Inside and outside the window (the exact edge is tested in
    // theta_window_exact_edges_are_inclusive).
    let cells = ids(10);
    let w = CellWeights::equal(&cells).unwrap();
    let mut c = vec![Some(VALID); 9];
    c.push(Some(THIN));
    let st = status(&w, &cells, &c);
    // a = 1.0; A_C = (9 + x)/10 = 1.05 → x = 1.5 (inside the 0.10 window).
    let mut v = vec![tc(1.0); 9];
    v.push(tc(1.5));
    let a = certify_reference_layer(&st, &w, &triples(&cells, &v)).unwrap();
    let k = a.clamp.unwrap()[0];
    close(k.clipped, 1.05);
    assert!(!k.clamped);
    // x = 2.05 → A_C = 1.105 > 1.10: clamped to 1.10 (θ = 0.10, not 0.11).
    v[9] = tc(2.05);
    let a = certify_reference_layer(&st, &w, &triples(&cells, &v)).unwrap();
    let k = a.clamp.unwrap()[0];
    assert!(k.clamped);
    close(k.clipped, 1.1);
}

#[test]
fn theta_window_exact_edges_are_inclusive() {
    // 5 cells, 4 valid at a = 2.0, one thin cell x. A_C = (8 + x)/5:
    // x = 3.0 → A_C = 2.2 = (1+θ)a exactly; x = 1.0 → 1.8 = (1−θ)a.
    let cells = ids(5);
    let w = CellWeights::equal(&cells).unwrap();
    let mut c = vec![Some(VALID); 4];
    c.push(Some(THIN));
    let st = status(&w, &cells, &c);
    for (x, edge) in [(3.0, 2.2), (1.0, 1.8)] {
        let mut v = vec![tc(2.0); 4];
        v.push(tc(x));
        let a = certify_reference_layer(&st, &w, &triples(&cells, &v)).unwrap();
        let k = a.clamp.unwrap()[0];
        // The setup really sits on the edge, bit for bit.
        assert_eq!(k.measured, if x > 2.0 { k.upper } else { k.lower });
        close(k.measured, edge);
        // Closed window: kept as measured, not flagged as clamped.
        assert_eq!(k.clipped, k.measured);
        assert!(!k.clamped, "x = {x}");
    }
    // Just past each edge: clamped to the edge.
    for (x, edge) in [(3.05, 2.2), (0.95, 1.8)] {
        let mut v = vec![tc(2.0); 4];
        v.push(tc(x));
        let k = certify_reference_layer(&st, &w, &triples(&cells, &v))
            .unwrap()
            .clamp
            .unwrap()[0];
        assert!(k.clamped, "x = {x}");
        close(k.clipped, edge);
    }
}

// ---- diagnostic reference anchor (DECISIONS Y) ------------------------------------

#[test]
fn diagnostic_reference_anchor_is_unclamped_symmetric_worst() {
    let (w, _, st, tr) = ex6();
    let d = diagnostic_reference_anchor(&st, &w, &tr).unwrap().unwrap();
    close(d.c, 0.7875);
    // Mixed valid cells: every cell is lifted to the worst valid value.
    let cells = ids(4);
    let w = CellWeights::equal(&cells).unwrap();
    let st = status(
        &w,
        &cells,
        &[Some(VALID), Some(VALID), Some(VALID), Some(THIN)],
    );
    let tr = triples(
        &cells,
        &[
            Triple::new(0.4, 8.0, 0.9),
            Triple::new(0.8, 12.0, 0.7),
            Triple::new(0.6, 10.0, 0.8),
            Triple::new(2.0, 5.0, 0.95),
        ],
    );
    let d = diagnostic_reference_anchor(&st, &w, &tr).unwrap().unwrap();
    close(d.c, (0.8 * 3.0 + 2.0) / 4.0);
    close(d.t, 12.0);
    close(d.q, 0.7);
    // NA thin cell → worst valid value.
    let mut tr2 = tr.clone();
    tr2[3].1 = Triple::NA;
    let d = diagnostic_reference_anchor(&st, &w, &tr2).unwrap().unwrap();
    close(d.c, 0.8);
}

#[test]
fn diagnostic_reference_anchor_takes_each_dimension_on_its_own() {
    // c01 is valid with time NA: its cost 0.9 and quality 0.9 still count
    // (line 570 is per dimension; DECISIONS F, Y).
    let cells = ids(4);
    let w = CellWeights::equal(&cells).unwrap();
    let st = status(
        &w,
        &cells,
        &[Some(VALID), Some(VALID), Some(VALID), Some(THIN)],
    );
    let tr = triples(
        &cells,
        &[
            Triple {
                t: None,
                ..Triple::new(0.9, 0.0, 0.9)
            },
            Triple::new(0.6, 10.0, 0.8),
            Triple::new(0.6, 12.0, 0.7),
            Triple::new(0.5, 5.0, 0.95),
        ],
    );
    let d = diagnostic_reference_anchor(&st, &w, &tr).unwrap().unwrap();
    // Worst valid: C 0.9 (from c01), T 12, Q 0.7; every cell is lifted.
    close(d.c, 0.9);
    close(d.t, 12.0);
    close(d.q, 0.7);
    // Only partial valid cells, each dimension defined somewhere: Some.
    let tr2 = triples(
        &cells,
        &[
            Triple {
                t: None,
                ..Triple::new(0.9, 0.0, 0.9)
            },
            Triple {
                c: None,
                ..Triple::new(0.0, 10.0, 0.8)
            },
            Triple {
                q: None,
                ..Triple::new(0.6, 12.0, 0.0)
            },
            Triple::new(0.5, 5.0, 0.95),
        ],
    );
    let d = diagnostic_reference_anchor(&st, &w, &tr2).unwrap().unwrap();
    close(d.c, 0.9);
    close(d.t, 12.0);
    close(d.q, 0.8);
    // Time NA in every valid cell: no anchor.
    let tr3 = triples(
        &cells,
        &[
            Triple { t: None, ..tc(0.9) },
            Triple { t: None, ..tc(0.6) },
            Triple { t: None, ..tc(0.6) },
            tc(0.5),
        ],
    );
    assert!(diagnostic_reference_anchor(&st, &w, &tr3)
        .unwrap()
        .is_none());
}

#[test]
fn diagnostic_reference_anchor_without_valid_cell_is_none() {
    let cells = ids(3);
    let w = CellWeights::equal(&cells).unwrap();
    let st = status(&w, &cells, &[Some(THIN); 3]);
    let tr = triples(&cells, &[tc(1.0); 3]);
    assert!(diagnostic_reference_anchor(&st, &w, &tr).unwrap().is_none());
}

// ---- coarse-bill tag and robust_enabled from cell notes (DECISIONS W) -----------

#[test]
fn robust_enabled_and_coarse_bill_come_from_cell_values() {
    let cells = ids(3);
    let w = CellWeights::equal(&cells).unwrap();
    let ok = cv(tc(1.0), tc(1.1), true, false);
    let lb = cv(tc(1.0), tc(1.1), false, true);
    let a = layer_arm_from_cells("A", &w, &arm_cells(&cells, ok.clone())).unwrap();
    assert!(a.arm.robust_enabled && !a.coarse_bill);
    // One lower-bound cell: robust dominance off for the layer, tag on.
    let mut mixed = arm_cells(&cells, ok.clone());
    mixed[1].1 = lb.clone();
    let b = layer_arm_from_cells("B", &w, &mixed).unwrap();
    assert!(!b.arm.robust_enabled);
    assert!(b.coarse_bill);
    // Disabled without the note (robust NA): not tagged.
    let mut na = arm_cells(&cells, ok.clone());
    na[0].1 = cv(tc(1.0), Triple::NA, false, false);
    let c = layer_arm_from_cells("C", &w, &na).unwrap();
    assert!(!c.arm.robust_enabled && !c.coarse_bill);
    // Missing d+ cell disables robust dominance.
    let d = layer_arm_from_cells("D", &w, &arm_cells(&cells[..2], ok)).unwrap();
    assert!(!d.arm.robust_enabled);
}

#[test]
fn tagged_arm_can_be_official_top_and_carries_its_tag() {
    let (w, cells, st, tr) = all_valid(3, tc(1.0));
    let anchor = certify_reference_layer(&st, &w, &tr).unwrap();
    // C is best on V but has a lower-bound-only bill.
    let arms: Vec<LayerArm> = vec![
        layer_arm_from_cells(
            "A",
            &w,
            &arm_cells(&cells, cv(tc(1.0), tc(1.0), true, false)),
        )
        .unwrap(),
        layer_arm_from_cells(
            "C",
            &w,
            &arm_cells(
                &cells,
                cv(Triple::new(0.5, 10.0, 0.7), tc(2.0), false, true),
            ),
        )
        .unwrap(),
    ];
    let fa: Vec<FrontierArm> = arms.iter().map(|a| a.arm.clone()).collect();
    let f = layer_frontiers(&fa).unwrap();
    assert_eq!(f.rec, vec!["A".to_string(), "C".to_string()]);
    // The derived view agrees with the cell-note tag.
    assert_eq!(f.coarse_bill, vec!["C".to_string()]);
    let scores: Vec<(String, f64)> = fa
        .iter()
        .map(|a| {
            let p = Point::from_triple(&a.star).unwrap();
            (
                a.id.clone(),
                layer_value(&anchor, p, &eq_w(), &floors())
                    .unwrap()
                    .unwrap(),
            )
        })
        .collect();
    let pick = layer_pick(&arms, &f, &anchor, &scores).unwrap();
    assert_eq!(pick.official_top, vec!["C".to_string()]);
    assert_eq!(pick.official_top_coarse_bill, vec!["C".to_string()]);
    assert!(pick.robust_alternative.is_empty());
    assert_eq!(labels::COARSE_BILL, "账单过粗");
}

#[test]
fn tag_shows_on_the_arm_row_outside_f_rec() {
    // A dominates B on plain triples; B is tagged and not in F_off/F_rec,
    // but its row still carries the tag.
    let cells = ids(3);
    let w = CellWeights::equal(&cells).unwrap();
    let a = layer_arm_from_cells(
        "A",
        &w,
        &arm_cells(&cells, cv(tc(0.5), tc(0.5), true, false)),
    )
    .unwrap();
    let b = layer_arm_from_cells(
        "B",
        &w,
        &arm_cells(&cells, cv(tc(1.0), tc(1.0), false, true)),
    )
    .unwrap();
    let f = layer_frontiers(&[a.arm.clone(), b.arm.clone()]).unwrap();
    assert_eq!(f.off, vec!["A".to_string()]);
    assert!(!f.rec.contains(&"B".to_string()));
    assert!(f.coarse_bill.is_empty());
    assert!(b.coarse_bill);
}

#[test]
fn empty_rec_lists_robust_alternative_and_no_top() {
    let (w, _, st, tr) = all_valid(3, tc(1.0));
    let anchor = certify_reference_layer(&st, &w, &tr).unwrap();
    // A and B: each off-frontier, each robust-dominated by the other's
    // robust triple? Build: A plain best, B robust best and dominates A's
    // robust; A dominates B on plain → F_off={A}, R(A)={B} → F_rec=∅.
    let fa = vec![
        FrontierArm {
            id: "A".into(),
            star: tc(0.5),
            robust: tc(3.0),
            robust_enabled: true,
        },
        FrontierArm {
            id: "B".into(),
            star: tc(1.0),
            robust: tc(1.0),
            robust_enabled: true,
        },
    ];
    let f = layer_frontiers(&fa).unwrap();
    assert_eq!(f.off, vec!["A".to_string()]);
    assert!(f.rec.is_empty());
    assert_eq!(f.rob, vec!["B".to_string()]);
    let arms: Vec<LayerArm> = fa
        .iter()
        .map(|a| LayerArm {
            arm: a.clone(),
            coarse_bill: false,
        })
        .collect();
    let scores = vec![("A".to_string(), 120.0), ("B".to_string(), 100.0)];
    let pick = layer_pick(&arms, &f, &anchor, &scores).unwrap();
    assert!(pick.official_top.is_empty());
    assert_eq!(pick.robust_alternative, vec!["B".to_string()]);
    assert!(!pick.official_top.contains(&"B".to_string()));
    assert_eq!(labels::ROBUST_ALTERNATIVE, "取消稳健备选");
}

#[test]
fn official_top_comes_from_rec_not_off() {
    let (w, _, st, tr) = all_valid(3, tc(1.0));
    let anchor = certify_reference_layer(&st, &w, &tr).unwrap();
    // A in F_off but not F_rec (robust-dominated by C); A has the top V.
    let fa = vec![
        FrontierArm {
            id: "A".into(),
            star: Triple::new(0.5, 10.0, 0.8),
            robust: tc(3.0),
            robust_enabled: true,
        },
        FrontierArm {
            id: "C".into(),
            star: Triple::new(1.0, 5.0, 0.8),
            robust: Triple::new(1.0, 5.0, 0.8),
            robust_enabled: true,
        },
    ];
    let f = layer_frontiers(&fa).unwrap();
    assert_eq!(f.off, vec!["A".to_string(), "C".to_string()]);
    assert_eq!(f.rec, vec!["C".to_string()]);
    let arms: Vec<LayerArm> = fa
        .iter()
        .map(|a| LayerArm {
            arm: a.clone(),
            coarse_bill: false,
        })
        .collect();
    let scores = vec![("A".to_string(), 150.0), ("C".to_string(), 101.0)];
    let pick = layer_pick(&arms, &f, &anchor, &scores).unwrap();
    assert_eq!(pick.official_top, vec!["C".to_string()]);
}

fn two_rec_arms() -> (Vec<LayerArm>, agent_value_core::frontier::LayerFrontiers) {
    // A cheaper, B faster: neither dominates on plain or robust triples.
    let fa = vec![
        FrontierArm {
            id: "A".into(),
            star: Triple::new(0.5, 12.0, 0.8),
            robust: Triple::new(0.5, 12.0, 0.8),
            robust_enabled: true,
        },
        FrontierArm {
            id: "B".into(),
            star: Triple::new(0.8, 6.0, 0.8),
            robust: Triple::new(0.8, 6.0, 0.8),
            robust_enabled: true,
        },
    ];
    let f = layer_frontiers(&fa).unwrap();
    let arms = fa
        .into_iter()
        .map(|arm| LayerArm {
            arm,
            coarse_bill: false,
        })
        .collect();
    (arms, f)
}

#[test]
fn layer_official_top_keeps_tie_group() {
    let (w, _, st, tr) = all_valid(3, tc(1.0));
    let anchor = certify_reference_layer(&st, &w, &tr).unwrap();
    assert_eq!(anchor.tier, AnchorTier::I);
    let (arms, f) = two_rec_arms();
    assert_eq!(f.rec, vec!["A".to_string(), "B".to_string()]);
    let tie = vec![("A".to_string(), 101.0), ("B".to_string(), 101.0)];
    let pick = layer_pick(&arms, &f, &anchor, &tie).unwrap();
    assert_eq!(pick.official_top, vec!["A".to_string(), "B".to_string()]);
    // Control: B ahead by 1e-6 → B alone; A ahead → A alone.
    let b_ahead = vec![("A".to_string(), 101.0), ("B".to_string(), 101.000001)];
    let pick = layer_pick(&arms, &f, &anchor, &b_ahead).unwrap();
    assert_eq!(pick.official_top, vec!["B".to_string()]);
    let a_ahead = vec![("A".to_string(), 101.000001), ("B".to_string(), 101.0)];
    let pick = layer_pick(&arms, &f, &anchor, &a_ahead).unwrap();
    assert_eq!(pick.official_top, vec!["A".to_string()]);
    // Tie tolerance is 1e-12: a 1e-9 gap is not a tie ...
    let gap_1e9 = vec![("A".to_string(), 101.0), ("B".to_string(), 101.0 + 1e-9)];
    let pick = layer_pick(&arms, &f, &anchor, &gap_1e9).unwrap();
    assert_eq!(pick.official_top, vec!["B".to_string()]);
    // ... while a gap of a few ulps (≈ 5.7e-14 at 101) still is.
    let gap_ulp = vec![("A".to_string(), 101.0), ("B".to_string(), 101.0 + 5e-14)];
    let pick = layer_pick(&arms, &f, &anchor, &gap_ulp).unwrap();
    assert_eq!(pick.official_top, vec!["A".to_string(), "B".to_string()]);
    // Boundary around 1e-12, in exact ulps (ulp(101) = 2^-46): 70 ulps
    // ≈ 9.95e-13 ties, 71 ulps ≈ 1.009e-12 does not.
    let up = |n: u64| f64::from_bits(101.0f64.to_bits() + n);
    let tied = vec![("A".to_string(), 101.0), ("B".to_string(), up(70))];
    let pick = layer_pick(&arms, &f, &anchor, &tied).unwrap();
    assert_eq!(pick.official_top, vec!["A".to_string(), "B".to_string()]);
    let apart = vec![("A".to_string(), 101.0), ("B".to_string(), up(71))];
    let pick = layer_pick(&arms, &f, &anchor, &apart).unwrap();
    assert_eq!(pick.official_top, vec!["B".to_string()]);
}

#[test]
fn tier_iii_with_empty_rec_lists_robust_alternative_without_top() {
    // Tier III does not suppress the frontier output (line 495), so the
    // F_rob alternative (line 461) is listed too; nobody is #1.
    let (w, _, st, tr) = all_valid(2, tc(1.0));
    let anchor = certify_reference_layer(&st, &w, &tr).unwrap();
    assert_eq!(anchor.tier, AnchorTier::III);
    let fa = vec![
        FrontierArm {
            id: "A".into(),
            star: tc(0.5),
            robust: tc(3.0),
            robust_enabled: true,
        },
        FrontierArm {
            id: "B".into(),
            star: tc(1.0),
            robust: tc(1.0),
            robust_enabled: true,
        },
    ];
    let f = layer_frontiers(&fa).unwrap();
    assert!(f.rec.is_empty());
    let arms: Vec<LayerArm> = fa
        .iter()
        .map(|a| LayerArm {
            arm: a.clone(),
            coarse_bill: false,
        })
        .collect();
    let pick = layer_pick(&arms, &f, &anchor, &[]).unwrap();
    assert!(pick.official_top.is_empty());
    assert_eq!(pick.robust_alternative, vec!["B".to_string()]);
}

#[test]
fn tier_iii_layer_has_frontier_but_no_top() {
    let (w, _, st, tr) = all_valid(2, tc(1.0));
    let anchor = certify_reference_layer(&st, &w, &tr).unwrap();
    assert_eq!(anchor.tier, AnchorTier::III);
    let fa = vec![FrontierArm {
        id: "A".into(),
        star: tc(0.5),
        robust: tc(0.5),
        robust_enabled: true,
    }];
    let f = layer_frontiers(&fa).unwrap();
    assert_eq!(f.off, vec!["A".to_string()]);
    let arms = vec![LayerArm {
        arm: fa[0].clone(),
        coarse_bill: false,
    }];
    let pick = layer_pick(&arms, &f, &anchor, &[]).unwrap();
    assert!(pick.official_top.is_empty());
    assert!(pick.robust_alternative.is_empty());
}

// ---- overall closure (DECISIONS U, V) ---------------------------------------------

fn tier_i(t: Triple) -> LayerAnchor {
    let (w, _, st, tr) = all_valid(3, t);
    certify_reference_layer(&st, &w, &tr).unwrap()
}

fn tier_ii_ex6() -> LayerAnchor {
    let (w, _, st, tr) = ex6();
    certify_reference_layer(&st, &w, &tr).unwrap()
}

fn tier_iii() -> LayerAnchor {
    let (w, _, st, tr) = all_valid(2, tc(1.0));
    certify_reference_layer(&st, &w, &tr).unwrap()
}

#[test]
fn overall_reference_sums_layer_anchors_and_marks_tier_ii() {
    let l1 = tier_i(Triple::new(1.0, 20.0, 0.9));
    let l2 = tier_ii_ex6();
    let r = overall_reference(&[(0.5, &l1), (0.5, &l2)]).unwrap();
    let a = r.anchor.unwrap();
    close(a.c, 0.5 * 1.0 + 0.5 * 0.66);
    close(a.t, 0.5 * 20.0 + 0.5 * 10.0);
    close(a.q, 0.5 * 0.9 + 0.5 * 0.8);
    assert!(r.includes_tier_ii);
    assert!(r.uncertified_layers.is_empty());
    assert_eq!(labels::OVERALL_INCLUDES_TIER_II, "含 Ⅱ 档夹逼层");
}

#[test]
fn any_positive_weight_tier_iii_makes_overall_reference_na() {
    let l1 = tier_i(tc(1.0));
    let l3 = tier_iii();
    let r = overall_reference(&[(0.7, &l1), (0.3, &l3)]).unwrap();
    assert!(r.anchor.is_none());
    assert_eq!(r.uncertified_layers, vec![1]);
    // Tier II beside tier III: only the "uncertified" header applies.
    let l2 = tier_ii_ex6();
    let r = overall_reference(&[(0.5, &l2), (0.5, &l3)]).unwrap();
    assert!(r.anchor.is_none() && !r.includes_tier_ii);
    // A zero-weight tier III layer is ignored.
    let r = overall_reference(&[(1.0, &l1), (0.0, &l3)]).unwrap();
    close(r.anchor.unwrap().c, 1.0);
    assert!(r.uncertified_layers.is_empty());
    assert!(!r.includes_tier_ii);
    // Zero-weight tier II does not set the header either.
    let l2 = tier_ii_ex6();
    let r = overall_reference(&[(1.0, &l1), (0.0, &l2)]).unwrap();
    assert!(!r.includes_tier_ii);
}

fn arm_layer(omega: f64, c: CellCounts, n: usize, t: Triple, in_off: bool) -> OverallArmLayer {
    let cells = ids(n);
    let w = CellWeights::equal(&cells).unwrap();
    OverallArmLayer {
        omega_d: omega,
        status: status(&w, &cells, &vec![Some(c); n]),
        u_d: n,
        triple: t,
        in_off,
    }
}

#[test]
fn overall_arm_qualifies_and_scores_against_overall_anchor() {
    let l1 = tier_i(tc(1.0));
    let l2 = tier_ii_ex6();
    let r = overall_reference(&[(0.5, &l1), (0.5, &l2)]).unwrap();
    // 12 cells × (400 + 0 + 100 cancels): r_C = 0.2 exactly, completion 0.8.
    let layers = vec![
        arm_layer(0.5, counts(400, 0, 100), 12, tc(0.8), true),
        arm_layer(0.5, counts(400, 0, 100), 12, tc(0.6), false),
    ];
    let a = overall_arm("X", &layers, AuthenticityGrade::A, &r, &eq_w(), &floors()).unwrap();
    assert!(a.failed.is_empty(), "{:?}", a.failed);
    assert!(a.qualified);
    close(a.cov, 1.0);
    assert!(!a.partial_coverage);
    let t = a.triple.unwrap();
    close(t.c, 0.7);
    close(
        a.v.unwrap(),
        value_score(r.anchor.unwrap(), t, &eq_w(), &floors()).unwrap(),
    );
}

#[test]
fn overall_gates_each_fail() {
    let l1 = tier_i(tc(1.0));
    let r = overall_reference(&[(1.0, &l1)]).unwrap();
    let run = |layers: Vec<OverallArmLayer>, g: AuthenticityGrade| {
        overall_arm("X", &layers, g, &r, &eq_w(), &floors()).unwrap()
    };
    // Cancel rate just above 0.20: 12 cells × (399 + 0 + 100): 100/499.
    let a = run(
        vec![arm_layer(1.0, counts(399, 0, 100), 12, tc(1.0), true)],
        AuthenticityGrade::A,
    );
    assert_eq!(a.failed, vec![OverallGate::CancelRate]);
    assert!(a.v.is_none());
    // Not in F_all.
    let a = run(
        vec![arm_layer(1.0, VALID, 12, tc(1.0), false)],
        AuthenticityGrade::A,
    );
    assert_eq!(a.failed, vec![OverallGate::NotInFAll]);
    assert!(!a.qualified && a.v.is_none());
    // Authenticity C.
    let a = run(
        vec![arm_layer(1.0, VALID, 12, tc(1.0), true)],
        AuthenticityGrade::C,
    );
    assert_eq!(a.failed, vec![OverallGate::Authenticity]);
    // Grade B passes.
    let a = run(
        vec![arm_layer(1.0, VALID, 12, tc(1.0), true)],
        AuthenticityGrade::B,
    );
    assert!(a.qualified);
    // Sample: 12 cells × 2 = 24 < 36; the cells are thin → also complete-case
    // and coverage < 1.
    let a = run(
        vec![arm_layer(1.0, counts(2, 0, 0), 12, tc(1.0), true)],
        AuthenticityGrade::A,
    );
    assert!(a.failed.contains(&OverallGate::Sample));
    assert!(a.failed.contains(&OverallGate::CompleteCase));
    assert!(a.partial_coverage);
    close(a.cov, 0.0);
    // Triple is still shown when every layer triple is complete.
    assert!(a.triple.is_some());
    // Sample gate across layers uses Σ n_comp vs max(30, 3 Σ U_d): two
    // layers of 6 cells × 3 = 36 ≥ 36 pass.
    let a = run(
        vec![
            arm_layer(0.5, counts(3, 0, 0), 6, tc(1.0), true),
            arm_layer(0.5, counts(3, 0, 0), 6, tc(1.0), false),
        ],
        AuthenticityGrade::A,
    );
    assert!(!a.failed.contains(&OverallGate::Sample), "{:?}", a.failed);
    // Completion below 0.5: 12 × (3 + 0 + 0 + m=4).
    let c = CellCounts {
        n1: 3,
        n0: 0,
        n_c: 0,
        m: 4,
    };
    let a = run(
        vec![arm_layer(1.0, c, 12, tc(1.0), true)],
        AuthenticityGrade::A,
    );
    assert!(
        a.failed.contains(&OverallGate::CompletionRate),
        "{:?}",
        a.failed
    );
    // NA layer triple → no overall triple.
    let a = run(
        vec![arm_layer(1.0, VALID, 12, Triple::NA, true)],
        AuthenticityGrade::A,
    );
    assert!(a.triple.is_none() && a.v.is_none());
    assert_eq!(
        labels::partial_coverage(0.75),
        "非全覆盖（0.7500），仅供诊断，不进前沿、不进推荐、不参与排序"
    );
}

#[test]
fn uncertified_overall_reference_gives_no_overall_v_or_top() {
    let l1 = tier_i(tc(1.0));
    let l3 = tier_iii();
    let r = overall_reference(&[(0.5, &l1), (0.5, &l3)]).unwrap();
    let layers = vec![
        arm_layer(0.5, VALID, 12, tc(0.5), true),
        arm_layer(0.5, VALID, 12, tc(0.5), true),
    ];
    let a = overall_arm("X", &layers, AuthenticityGrade::A, &r, &eq_w(), &floors()).unwrap();
    assert!(a.qualified);
    assert!(a.triple.is_some());
    assert!(a.v.is_none());
    assert!(overall_top(&[a]).unwrap().is_empty());
    assert!(labels::OVERALL_REFERENCE_UNCERTIFIED.starts_with("部分正权重层参照未认证"));
}

#[test]
fn overall_top_is_highest_v_tie_group_of_qualified_arms() {
    let l1 = tier_i(tc(1.0));
    let r = overall_reference(&[(1.0, &l1)]).unwrap();
    let mk = |id: &str, c: f64, off: bool| {
        overall_arm(
            id,
            &[arm_layer(1.0, VALID, 12, tc(c), off)],
            AuthenticityGrade::A,
            &r,
            &eq_w(),
            &floors(),
        )
        .unwrap()
    };
    // Z is cheapest but not in F_all: no V, cannot be #1.
    let arms = vec![
        mk("X", 0.5, true),
        mk("Y", 0.5, true),
        mk("W", 0.8, true),
        mk("Z", 0.1, false),
    ];
    assert_eq!(
        overall_top(&arms).unwrap(),
        vec!["X".to_string(), "Y".to_string()]
    );
}

#[test]
fn zero_weight_layer_does_not_enter_overall_gates() {
    let l1 = tier_i(tc(1.0));
    let r = overall_reference(&[(1.0, &l1)]).unwrap();
    let layers = vec![
        arm_layer(1.0, VALID, 12, tc(0.5), true),
        // Zero-weight layer with thin cells and a high cancel rate.
        arm_layer(0.0, counts(1, 0, 5), 12, tc(0.5), false),
    ];
    let mut layers = layers;
    let a0 = overall_arm("X", &layers, AuthenticityGrade::A, &r, &eq_w(), &floors()).unwrap();
    assert!(a0.failed.is_empty(), "{:?}", a0.failed);
    // A zero-weight NA layer triple does not hide the overall triple.
    layers[1].triple = Triple::NA;
    let a = overall_arm("X", &layers, AuthenticityGrade::A, &r, &eq_w(), &floors()).unwrap();
    assert!(a.failed.is_empty(), "{:?}", a.failed);
    assert!(a.triple.is_some() && a.v.is_some());
    close(a.cov, 1.0);
}

// ---- §13 R_k and F_k (DECISIONS G, Z) ----------------------------------------------

fn rc(id: &str, user: bool, official: bool, tasks: u64) -> ReferenceCandidate {
    ReferenceCandidate {
        id: id.into(),
        user_specified: user,
        official_direct: official,
        prev_tasks: tasks,
        prev_authenticity: AuthenticityGrade::A,
        prev_has_bad: false,
    }
}

#[test]
fn reference_order_user_then_official_then_most_tasks() {
    let c = vec![
        rc("a", false, false, 900),
        rc("b", false, true, 10),
        rc("u", true, false, 1),
    ];
    assert_eq!(select_reference(&c).unwrap().as_deref(), Some("u"));
    // User choice unavailable (F_bad) → official direct even with fewer tasks.
    let mut c2 = c.clone();
    c2[2].prev_has_bad = true;
    assert_eq!(select_reference(&c2).unwrap().as_deref(), Some("b"));
    // Official fails authenticity → most tasks.
    c2[1].prev_authenticity = AuthenticityGrade::C;
    assert_eq!(select_reference(&c2).unwrap().as_deref(), Some("a"));
    // Everything unavailable → None.
    c2[0].prev_has_bad = true;
    assert_eq!(select_reference(&c2).unwrap(), None);
}

#[test]
fn reference_skips_unavailable_top_official_to_next_official() {
    // DECISIONS Z: several official direct arms; the one with most tasks is
    // unavailable → the next official direct arm by tasks, not step 3.
    let mut c = vec![
        rc("o1", false, true, 500),
        rc("o2", false, true, 20),
        rc("x", false, false, 900),
    ];
    c[0].prev_has_bad = true;
    assert_eq!(select_reference(&c).unwrap().as_deref(), Some("o2"));
    c[0].prev_has_bad = false;
    c[0].prev_authenticity = AuthenticityGrade::C;
    assert_eq!(select_reference(&c).unwrap().as_deref(), Some("o2"));
    // Only when no official direct arm is available does step 3 apply.
    c[1].prev_has_bad = true;
    assert_eq!(select_reference(&c).unwrap().as_deref(), Some("x"));
}

#[test]
fn reference_ties_go_to_smaller_id_and_grade_b_is_available() {
    let mut c = vec![
        rc("z", false, true, 50),
        rc("m", false, true, 50),
        rc("q", false, true, 49),
    ];
    c[1].prev_authenticity = AuthenticityGrade::B;
    assert_eq!(select_reference(&c).unwrap().as_deref(), Some("m"));
    let c = vec![rc("z", false, false, 7), rc("k", false, false, 7)];
    assert_eq!(select_reference(&c).unwrap().as_deref(), Some("k"));
}

#[test]
fn reference_rejects_two_user_choices_and_duplicates() {
    assert!(select_reference(&[rc("a", true, false, 1), rc("b", true, false, 1)]).is_err());
    assert!(select_reference(&[rc("a", false, false, 1), rc("a", false, true, 1)]).is_err());
}

fn fb_layer(c: &[Option<CellCounts>], ratings: u32) -> FallbackLayer {
    let cells = ids(c.len());
    let w = CellWeights::equal(&cells).unwrap();
    let st = status(&w, &cells, c);
    let present: Vec<CellId> = cells
        .iter()
        .zip(c)
        .filter(|(_, c)| c.is_some())
        .map(|(id, _)| id.clone())
        .collect();
    let anchor =
        certify_reference_layer(&st, &w, &triples(&present, &vec![tc(1.0); present.len()]))
            .unwrap();
    FallbackLayer {
        omega_d: 1.0,
        weights: w,
        anchor,
        n_bad: st.n_bad,
        ratings: cells.iter().map(|c| (c.clone(), ratings)).collect(),
    }
}

fn fb(grade: AuthenticityGrade, layer: FallbackLayer) -> FallbackCandidate {
    FallbackCandidate {
        id: "F".into(),
        prev_authenticity: grade,
        layers: vec![layer],
    }
}

#[test]
fn fallback_defaults_to_reference() {
    assert_eq!(
        resolve_fallback("R", None).unwrap(),
        FallbackDecision::Default { id: "R".into() }
    );
    let mut same = fb(AuthenticityGrade::C, fb_layer(&[Some(VALID); 3], 0));
    same.id = "R".into();
    assert_eq!(
        resolve_fallback("R", Some(&same)).unwrap().fallback_id(),
        "R"
    );
}

#[test]
fn qualified_fallback_is_kept_with_exactly_three_ratings() {
    let f = fb(AuthenticityGrade::A, fb_layer(&[Some(VALID); 3], 3));
    assert!(fallback_failures(&f).unwrap().is_empty());
    assert_eq!(
        resolve_fallback("R", Some(&f)).unwrap(),
        FallbackDecision::Keep { id: "F".into() }
    );
    // Tier II (thin cell, ≥ 3 valid) qualifies.
    let f = fb(
        AuthenticityGrade::AMinus,
        fb_layer(&[Some(VALID), Some(VALID), Some(VALID), Some(THIN)], 3),
    );
    assert_eq!(f.layers[0].anchor.tier, AnchorTier::II);
    assert!(fallback_failures(&f).unwrap().is_empty());
}

#[test]
fn unqualified_fallback_reverts_with_note() {
    let check = |f: FallbackCandidate, want: Vec<FallbackFailure>| {
        let d = resolve_fallback("R", Some(&f)).unwrap();
        assert_eq!(
            d,
            FallbackDecision::Revert {
                id: "R".into(),
                failures: want,
                note: FALLBACK_REVERT_NOTE,
            }
        );
        assert_eq!(d.fallback_id(), "R");
    };
    check(
        fb(AuthenticityGrade::C, fb_layer(&[Some(VALID); 3], 3)),
        vec![FallbackFailure::Authenticity],
    );
    check(
        fb(
            AuthenticityGrade::A,
            fb_layer(&[Some(VALID), Some(VALID), Some(VALID), Some(BAD)], 3),
        ),
        vec![FallbackFailure::BadCell, FallbackFailure::Tier],
    );
    check(
        fb(
            AuthenticityGrade::A,
            fb_layer(&[Some(VALID), Some(VALID), Some(VALID), None], 3),
        ),
        vec![FallbackFailure::Tier],
    );
    // G item 4: one cell with n_q = 2.
    let mut l = fb_layer(&[Some(VALID); 3], 3);
    l.ratings[2].1 = 2;
    check(fb(AuthenticityGrade::A, l), vec![FallbackFailure::Ratings]);
    // A cell without a ratings row counts as 0.
    let mut l = fb_layer(&[Some(VALID); 3], 3);
    l.ratings.pop();
    check(fb(AuthenticityGrade::A, l), vec![FallbackFailure::Ratings]);
    assert_eq!(FALLBACK_REVERT_NOTE, "兜底臂不满足资格，回退为参照臂");
}

#[test]
fn fallback_zero_weight_layer_is_ignored() {
    let good = fb_layer(&[Some(VALID); 3], 3);
    let mut bad = fb_layer(&[Some(BAD); 3], 0);
    bad.omega_d = 0.0;
    let f = FallbackCandidate {
        id: "F".into(),
        prev_authenticity: AuthenticityGrade::A,
        layers: vec![good, bad],
    };
    assert!(fallback_failures(&f).unwrap().is_empty());
    // No positive-weight layer at all: no evidence, reverts.
    let none = FallbackCandidate {
        id: "F".into(),
        prev_authenticity: AuthenticityGrade::A,
        layers: vec![],
    };
    assert_eq!(
        fallback_failures(&none).unwrap(),
        vec![FallbackFailure::Tier, FallbackFailure::Ratings]
    );
}

// ---- not_admitted (DECISIONS X) ------------------------------------------------------

#[test]
fn not_admitted_lists_each_reason() {
    let cells = ids(3);
    let w = CellWeights::equal(&cells).unwrap();
    // All thin with n_comp = 3 and κ = 0: ESS ok, no valid cell.
    let st = status(&w, &cells, &[Some(THIN); 3]);
    assert_eq!(
        not_admitted_reasons(&st, 0.0),
        vec![NotAdmittedReason::NoValidCell]
    );
    // κ_eff alone cannot help without a valid cell.
    assert!(!diagnostic_admitted(&st, 10.0));
    // One valid cell with n_comp = 2 + ... : use 2 thin + κ.
    let two = ids(2);
    let w2 = CellWeights::equal(&two).unwrap();
    let st2 = status(&w2, &two, &[Some(THIN); 2]);
    assert_eq!(
        not_admitted_reasons(&st2, 0.5),
        vec![
            NotAdmittedReason::EssBelowThree,
            NotAdmittedReason::NoValidCell
        ]
    );
    assert_eq!(
        not_admitted_reasons(&st2, 1.0),
        vec![NotAdmittedReason::NoValidCell]
    );
    let st3 = status(&w, &cells, &[Some(VALID); 3]);
    assert!(not_admitted_reasons(&st3, 0.0).is_empty());
    assert!(diagnostic_admitted(&st3, 0.0));
}

fn diag_row(c: &[Option<CellCounts>], t: &[Triple]) -> DiagnosticRow {
    let cells = ids(c.len());
    let w = CellWeights::equal(&cells).unwrap();
    let st = status(&w, &cells, c);
    let present: Vec<(CellId, Triple)> = cells
        .iter()
        .cloned()
        .zip(t.iter().copied())
        .filter(|(_, t)| *t != Triple::NA)
        .collect();
    // No priors passed: κ_eff = 0, so ESS = n_comp.
    diagnose_arm(&st, &w, &present, &[], None, &eq_w(), &floors()).unwrap()
}

#[test]
fn diagnose_arm_reports_valid_cell_incomplete() {
    // ESS = 30 ≥ 3 and three valid cells, but c02 lacks its time value.
    let row = diag_row(
        &[Some(VALID); 3],
        &[tc(1.0), Triple { t: None, ..tc(1.0) }, tc(1.0)],
    );
    assert!(!row.admitted);
    assert_eq!(
        row.not_admitted,
        vec![NotAdmittedReason::ValidCellIncomplete]
    );
    assert!(row.filled.is_empty() && row.aggregate_cons.is_none());
}

#[test]
fn diagnostic_rows_admitted_iff_no_reason() {
    let rows = [
        // Admitted: one thin cell, two valid, complete triples.
        (
            "admitted",
            diag_row(
                &[Some(VALID), Some(VALID), Some(THIN)],
                &[tc(1.0), tc(1.0), tc(2.0)],
            ),
            vec![],
        ),
        // Valid cell missing one dimension.
        (
            "incomplete dimension",
            diag_row(
                &[Some(VALID); 3],
                &[tc(1.0), tc(1.0), Triple { q: None, ..tc(1.0) }],
            ),
            vec![NotAdmittedReason::ValidCellIncomplete],
        ),
        // Valid cell with no triple row at all.
        (
            "missing triple row",
            diag_row(&[Some(VALID); 3], &[tc(1.0), Triple::NA, tc(1.0)]),
            vec![NotAdmittedReason::ValidCellIncomplete],
        ),
        // Three thin cells: ESS = 3, no valid cell.
        (
            "no valid",
            diag_row(&[Some(THIN); 3], &[tc(1.0); 3]),
            vec![NotAdmittedReason::NoValidCell],
        ),
        // Two thin cells: ESS = 2 < 3 and no valid cell.
        (
            "ess and no valid",
            diag_row(&[Some(THIN); 2], &[tc(1.0); 2]),
            vec![
                NotAdmittedReason::EssBelowThree,
                NotAdmittedReason::NoValidCell,
            ],
        ),
    ];
    for (name, row, want) in rows {
        assert_eq!(row.not_admitted, want, "{name}");
        assert_eq!(row.admitted, row.not_admitted.is_empty(), "{name}");
    }
}

// ---- composed: E_d → frontier → official pick, examples 6/7/8 ----------------------

/// Builds E_d from candidate statuses and returns the layer pick.
fn compose(
    w: &CellWeights,
    cells: &[CellId],
    reference: &[(CellId, Triple)],
    anchor: &LayerAnchor,
    candidates: &[(&str, Vec<Option<CellCounts>>, CellValues)],
) -> (
    Vec<String>,
    agent_value_core::frontier::LayerFrontiers,
    agent_value_core::recommend::LayerPick,
    Vec<(String, f64)>,
) {
    let mut arms = Vec::new();
    let mut e_d = Vec::new();
    for (id, c, v) in candidates {
        let st = status(w, cells, c);
        let e = assess_eligibility(&st, w, AuthenticityGrade::A, true, reference).unwrap();
        if e.eligible {
            e_d.push(id.to_string());
            arms.push(layer_arm_from_cells(id, w, &arm_cells(cells, v.clone())).unwrap());
        }
    }
    let fa: Vec<FrontierArm> = arms.iter().map(|a| a.arm.clone()).collect();
    let f = layer_frontiers(&fa).unwrap();
    let scores: Vec<(String, f64)> = fa
        .iter()
        .filter_map(|a| {
            layer_value(
                anchor,
                Point::from_triple(&a.star).unwrap(),
                &eq_w(),
                &floors(),
            )
            .unwrap()
            .map(|v| (a.id.clone(), v))
        })
        .collect();
    let pick = layer_pick(&arms, &f, anchor, &scores).unwrap();
    (e_d, f, pick, scores)
}

fn plain(t: Triple) -> CellValues {
    cv(t, t, true, false)
}

#[test]
fn example_6_composed_from_e_d_to_official_top() {
    let (w, cells, st, tr) = ex6();
    let anchor = certify_reference_layer(&st, &w, &tr).unwrap();
    let mut route_counts = vec![Some(VALID); 11];
    route_counts.push(Some(THIN));
    let (e_d, f, pick, scores) = compose(
        &w,
        &cells,
        &tr,
        &anchor,
        &[
            ("own", vec![Some(VALID); 12], plain(tc(0.60))),
            (
                "b",
                vec![Some(VALID); 12],
                plain(Triple::new(0.55, 12.0, 0.75)),
            ),
            // Shares the reference's thin cell: not in E_d.
            ("route", route_counts, plain(tc(0.7875))),
        ],
    );
    assert_eq!(e_d, vec!["own".to_string(), "b".to_string()]);
    assert_eq!(f.off, vec!["b".to_string(), "own".to_string()]);
    assert_eq!(f.rec, f.off);
    let v = |id: &str| scores.iter().find(|(s, _)| s == id).unwrap().1;
    prints_as(v("own"), "102.60");
    close(
        v("b"),
        100.0 * (0.81f64 / 0.70 * 25.0 / 27.0 * 0.22 / 0.27).powf(1.0 / 3.0),
    );
    assert_eq!(pick.official_top, vec!["own".to_string()]);
    // The route arm's V is still defined against the clamped anchor.
    prints_as(
        layer_value(&anchor, anchor.measured.unwrap(), &eq_w(), &floors())
            .unwrap()
            .unwrap(),
        "95.24",
    );
    assert_eq!(
        labels::TIER_II_LEGEND,
        "Ⅱ 档下不存在“100 分 = 与参照组持平”这个基准点"
    );
}

#[test]
fn example_7_composed_tier_iii_keeps_frontier_without_top() {
    let cells = ids(12);
    let w = CellWeights::equal(&cells).unwrap();
    let mut c = vec![Some(VALID); 11];
    c.push(Some(BAD));
    let st = status(&w, &cells, &c);
    let mut v = vec![tc(0.60); 11];
    v.push(tc(4.20));
    let tr = triples(&cells, &v);
    let anchor = certify_reference_layer(&st, &w, &tr).unwrap();
    assert_eq!(anchor.tier, AnchorTier::III);
    assert_eq!(anchor.tier_iii_reasons, vec![TierIIIReason::Bad]);
    close(anchor.w_bad, 1.0 / 12.0);
    assert_eq!(
        labels::tier_iii_header(TierIIIReason::Bad, &anchor),
        "参照组存在真不合格单元（0.0833），需补测或更换参照"
    );
    let (e_d, f, pick, scores) = compose(
        &w,
        &cells,
        &tr,
        &anchor,
        &[("own", vec![Some(VALID); 12], plain(tc(0.60)))],
    );
    assert_eq!(e_d, vec!["own".to_string()]);
    assert_eq!(f.off, vec!["own".to_string()]);
    assert!(scores.is_empty());
    assert!(pick.official_top.is_empty());
    // Counterfactuals: unclamped A_C = 0.9 gives 111.87; wrongly tier II
    // (anchor 0.66) gives 102.60. Neither is produced.
    let m = anchor.measured.unwrap();
    close(m.c, 0.9);
    prints_as(
        value_score(m, pt(0.60), &eq_w(), &floors()).unwrap(),
        "111.87",
    );
    prints_as(
        value_score(pt(0.66), pt(0.60), &eq_w(), &floors()).unwrap(),
        "102.60",
    );
    let r = overall_reference(&[(1.0, &anchor)]).unwrap();
    assert!(r.anchor.is_none());
}

#[test]
fn example_8_composed_lower_clamp() {
    let (w, cells, st, tr) = ex8();
    let anchor = certify_reference_layer(&st, &w, &tr).unwrap();
    let (e_d, f, pick, scores) = compose(
        &w,
        &cells,
        &tr,
        &anchor,
        &[
            ("own", vec![Some(VALID); 12], plain(tc(0.60))),
            ("cheap", vec![Some(VALID); 12], plain(tc(0.165))),
        ],
    );
    assert_eq!(e_d.len(), 2);
    assert_eq!(f.off, vec!["cheap".to_string()]);
    let v = |id: &str| scores.iter().find(|(s, _)| s == id).unwrap().1;
    prints_as(v("cheap"), "129.87");
    close(v("own"), 100.0 * (0.69f64 / 0.75).powf(1.0 / 3.0));
    assert_eq!(pick.official_top, vec!["cheap".to_string()]);
}

// ---- PR #9 review follow-ups (Codex comments, 2026-10-20) ---------------------------

#[test]
fn overall_arm_counter_overflow_is_an_error_not_a_wrap() {
    let l1 = tier_i(tc(1.0));
    let l2 = tier_ii_ex6();
    let r = overall_reference(&[(0.5, &l1), (0.5, &l2)]).unwrap();
    let mut big = arm_layer(0.5, counts(400, 0, 100), 12, tc(0.6), false);
    big.status.n_comp = u64::MAX;
    let layers = vec![arm_layer(0.5, counts(400, 0, 100), 12, tc(0.8), true), big];
    assert!(matches!(
        overall_arm("X", &layers, AuthenticityGrade::A, &r, &eq_w(), &floors()),
        Err(agent_value_core::error::CoreError::Overflow { .. })
    ));
}

#[test]
fn nan_kappa_eff_is_not_admitted() {
    let cells = ids(3);
    let w = CellWeights::equal(&cells).unwrap();
    let st = status(&w, &cells, &[Some(VALID); 3]);
    assert!(not_admitted_reasons(&st, 0.0).is_empty());
    assert_eq!(
        not_admitted_reasons(&st, f64::NAN),
        vec![NotAdmittedReason::EssBelowThree]
    );
    assert!(!diagnostic_admitted(&st, f64::NAN));
    assert!(!diagnostic_admitted(&st, f64::INFINITY));
}

#[test]
fn fallback_rejects_non_finite_or_negative_layer_weight() {
    for bad in [f64::NAN, f64::INFINITY, -0.5] {
        let mut l = fb_layer(&[Some(VALID); 3], 3);
        l.omega_d = bad;
        let f = fb(AuthenticityGrade::A, l);
        assert!(fallback_failures(&f).is_err(), "omega_d = {bad}");
        assert!(resolve_fallback("R", Some(&f)).is_err(), "omega_d = {bad}");
    }
}

#[test]
fn fallback_rejects_duplicate_rating_rows() {
    let mut l = fb_layer(&[Some(VALID); 3], 3);
    let first = l.ratings[0].0.clone();
    l.ratings.push((first, 0));
    let f = fb(AuthenticityGrade::A, l);
    assert!(matches!(
        fallback_failures(&f),
        Err(agent_value_core::error::CoreError::DuplicateCell { .. })
    ));
}

#[test]
fn layer_pick_rejects_duplicate_score_ids() {
    let (w, _, st, tr) = all_valid(3, tc(1.0));
    let anchor = certify_reference_layer(&st, &w, &tr).unwrap();
    let (arms, f) = two_rec_arms();
    let dup = vec![
        ("A".to_string(), 101.0),
        ("B".to_string(), 100.0),
        ("A".to_string(), 99.0),
    ];
    assert!(matches!(
        layer_pick(&arms, &f, &anchor, &dup),
        Err(agent_value_core::error::CoreError::DuplicateCell { .. })
    ));
}

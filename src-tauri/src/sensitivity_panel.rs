//! Sensitivity panel for one coding layer.
//!
//! The shell asks `agent_value_core::sensitivity` for the winner map, the
//! weight-robust label, and the flip sentences. Printed copy is the core's
//! constants and `flip_sentence`. This module does not colour the map and
//! does not keep a second formula.

use agent_value_core::sensitivity::{
    flip_sentence, sensitivity, BehaviourRegion, NoFlipLine, RobustRegion, SensitivityArm,
    SensitivityInput, SensitivityReport, Withheld, FLOOR_ROBUST, WEIGHT_ROBUST_EVERYWHERE,
};
use agent_value_core::value::{Floors, Point};
use agent_value_core::weights::Weights;
use agent_value_core::window::FIRST_WINDOW_LABEL;
use serde::Serialize;

use crate::store_cmd::AppState;
use crate::window_result::{self, LayerLoad, SensitivityParts};

/// JSON for the sensitivity panel. `show_chart` is false when the core
/// withholds the map (first window, no anchor, no arms) or there is no layer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SensitivityView {
    pub source: String,
    pub fixture: bool,
    pub show_chart: bool,
    pub withheld: Option<String>,
    pub withheld_text: Option<String>,
    pub empty_reason: Option<String>,
    pub chart: Option<SensitivityChart>,
}

/// Display payload for one chart. Winner lists and sentences come from the core.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SensitivityChart {
    pub points: Vec<SensitivityPoint>,
    pub current_winners: Vec<String>,
    /// `Ω_s` when the robust region is the self-report, otherwise `Ω`.
    pub omega_mark: String,
    /// [`WEIGHT_ROBUST_EVERYWHERE`] when one arm wins the region alone.
    pub omega_copy: Option<String>,
    pub everywhere_id: Option<String>,
    pub shares: Vec<ShareView>,
    pub behaviour_mark: Option<String>,
    pub behaviour_points: Option<u32>,
    pub sentences: Vec<SentenceView>,
    pub no_flip_text: Option<String>,
    /// [`FLOOR_ROBUST`] when all seven floor runs agree.
    pub floor_copy: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SensitivityPoint {
    pub a: f64,
    pub b: f64,
    pub g: f64,
    pub winners: Vec<String>,
    pub in_omega_s: bool,
    pub in_omega_b: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ShareView {
    pub id: String,
    pub fraction: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SentenceView {
    /// Dimension label from [`agent_value_core::sensitivity::Dim::label`].
    pub fixed: String,
    /// [`flip_sentence`].
    pub text: String,
}

/// `source` is `store`, `example6`, `example7`, `example8`, `tie`, `first`, or `grid`.
pub fn view(app: &AppState, source: &str) -> Result<SensitivityView, String> {
    if source == "grid" {
        return grid_view();
    }
    let fixture = source != "store";
    match window_result::load_sensitivity(app, source)? {
        LayerLoad::Empty {
            reason,
            is_first,
            first_label,
        } => Ok(empty_view(source, fixture, reason, is_first, first_label)),
        LayerLoad::Ready(parts) => from_parts(source, fixture, &parts),
    }
}

fn empty_view(
    source: &str,
    fixture: bool,
    reason: String,
    is_first: bool,
    first_label: Option<String>,
) -> SensitivityView {
    let mut view = blank(source, fixture);
    view.empty_reason = Some(reason);
    if is_first {
        view.withheld = Some("first-window".to_string());
        view.withheld_text = Some(first_label.unwrap_or_else(|| FIRST_WINDOW_LABEL.to_string()));
    }
    view
}

fn blank(source: &str, fixture: bool) -> SensitivityView {
    SensitivityView {
        source: source.to_string(),
        fixture,
        show_chart: false,
        withheld: None,
        withheld_text: None,
        empty_reason: None,
        chart: None,
    }
}

fn from_parts(
    source: &str,
    fixture: bool,
    parts: &SensitivityParts,
) -> Result<SensitivityView, String> {
    let input = SensitivityInput {
        arms: &parts.arms,
        reference: parts.reference,
        first_window: parts.first_window,
        current: parts.current,
        omega_s: parts.omega_s.as_deref(),
        omega_b: None,
        floors: Floors::default(),
    };
    match sensitivity(&input).map_err(|err| err.to_string())? {
        Err(Withheld::FirstWindow) => {
            let mut view = blank(source, fixture);
            view.withheld = Some("first-window".to_string());
            view.withheld_text = Some(FIRST_WINDOW_LABEL.to_string());
            Ok(view)
        }
        Err(Withheld::NoReference) => {
            let mut view = blank(source, fixture);
            view.withheld = Some("no-reference".to_string());
            view.withheld_text = parts.no_reference_text.clone();
            Ok(view)
        }
        Err(Withheld::NoArms) => {
            let mut view = blank(source, fixture);
            view.withheld = Some("no-arms".to_string());
            view.withheld_text = Some("没有可比较的臂。".to_string());
            Ok(view)
        }
        Ok(report) => Ok(SensitivityView {
            source: source.to_string(),
            fixture,
            show_chart: true,
            withheld: None,
            withheld_text: None,
            empty_reason: None,
            chart: Some(chart_of(&report)),
        }),
    }
}

fn chart_of(report: &SensitivityReport) -> SensitivityChart {
    let robust = &report.weight_robust;
    let (behaviour_mark, behaviour_points) = match report.behaviour_region {
        BehaviourRegion::NoSignal => (None, None),
        BehaviourRegion::NoGridPoint => (Some("Ω_b".to_string()), Some(0)),
        BehaviourRegion::Points(n) => (Some("Ω_b".to_string()), u32::try_from(n).ok()),
    };
    let (sentences, no_flip_text) = match &report.flip {
        Ok(line) => {
            let sentences = line
                .statements
                .iter()
                .map(|statement| SentenceView {
                    fixed: statement.fixed.label().to_string(),
                    text: flip_sentence(line, statement),
                })
                .collect();
            (sentences, None)
        }
        Err(NoFlipLine::TooFewArms) => (Vec::new(), Some("臂不足两条，没有翻转线。".to_string())),
        Err(NoFlipLine::AmbiguousRunnerUp) => {
            (Vec::new(), Some("第二和第三并列，没有翻转线。".to_string()))
        }
    };
    SensitivityChart {
        points: report
            .map
            .iter()
            .map(|point| SensitivityPoint {
                a: point.weights.a(),
                b: point.weights.b(),
                g: point.weights.g(),
                winners: point.winners.clone(),
                in_omega_s: point.in_omega_s,
                in_omega_b: point.in_omega_b,
            })
            .collect(),
        current_winners: report.current_winners.clone(),
        omega_mark: match robust.region {
            RobustRegion::SelfReport => "Ω_s".to_string(),
            RobustRegion::Neighbourhood => "Ω".to_string(),
        },
        omega_copy: robust
            .everywhere
            .as_ref()
            .map(|_| WEIGHT_ROBUST_EVERYWHERE.to_string()),
        everywhere_id: robust.everywhere.clone(),
        shares: robust
            .shares
            .iter()
            .map(|share| ShareView {
                id: share.id.clone(),
                fraction: format!("{}/{}", share.points, robust.total),
            })
            .collect(),
        behaviour_mark,
        behaviour_points,
        sentences,
        no_flip_text,
        floor_copy: report.floor_robust.robust.then(|| FLOOR_ROBUST.to_string()),
    }
}

fn twentieths(i: u32, j: u32, k: u32) -> Result<Weights, String> {
    let d = 20.0;
    Weights::new(f64::from(i) / d, f64::from(j) / d, f64::from(k) / d)
        .map_err(|err| err.to_string())
}

/// Spec grid: X and Y, equal weights, and the 14-point self-report region.
fn grid_view() -> Result<SensitivityView, String> {
    let arms = vec![
        SensitivityArm {
            id: "X".to_string(),
            point: Point {
                c: 0.30,
                t: 120.0,
                q: 0.80,
            },
        },
        SensitivityArm {
            id: "Y".to_string(),
            point: Point {
                c: 0.90,
                t: 60.0,
                q: 0.85,
            },
        },
    ];
    let keys = [
        (1, 5, 14),
        (1, 6, 13),
        (2, 6, 12),
        (2, 9, 9),
        (3, 7, 10),
        (3, 8, 9),
        (3, 9, 8),
        (4, 6, 10),
        (4, 7, 9),
        (4, 8, 8),
        (5, 5, 10),
        (5, 6, 9),
        (5, 7, 8),
        (6, 5, 9),
    ];
    let mut omega = Vec::with_capacity(keys.len());
    for (i, j, k) in keys {
        omega.push(twentieths(i, j, k)?);
    }
    let parts = SensitivityParts {
        first_window: false,
        current: Weights::equal(),
        reference: Some(Point {
            c: 0.60,
            t: 90.0,
            q: 0.80,
        }),
        arms,
        omega_s: Some(omega),
        no_reference_text: None,
    };
    from_parts("grid", true, &parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_value_core::sensitivity::CURRENT_ESTIMATE_NOTE;

    fn chart(source: &str) -> SensitivityChart {
        let app = AppState::open_in_memory().unwrap();
        let view = view(&app, source).unwrap();
        assert!(view.show_chart, "{source}");
        view.chart.expect("chart")
    }

    fn sole(chart: &SensitivityChart, id: &str) -> usize {
        chart
            .points
            .iter()
            .filter(|point| point.winners.len() == 1 && point.winners[0] == id)
            .count()
    }

    fn copy_matches_core(chart: &SensitivityChart) {
        match &chart.omega_copy {
            Some(copy) => {
                assert_eq!(copy, WEIGHT_ROBUST_EVERYWHERE);
                assert!(chart.everywhere_id.is_some());
            }
            None => assert!(chart.everywhere_id.is_none()),
        }
        if let Some(copy) = &chart.floor_copy {
            assert_eq!(copy, FLOOR_ROBUST);
        }
        for sentence in &chart.sentences {
            assert!(
                sentence.text.ends_with(CURRENT_ESTIMATE_NOTE),
                "{}",
                sentence.text
            );
        }
    }

    #[test]
    fn grid_colours_from_the_core_and_uses_its_copy() {
        let chart = chart("grid");
        assert_eq!(chart.points.len(), 171);
        assert_eq!(sole(&chart, "Y"), 93);
        assert_eq!(sole(&chart, "X"), 78);
        assert_eq!(
            chart.points.iter().filter(|point| point.in_omega_s).count(),
            14
        );
        assert!(chart.points.iter().all(|point| !point.in_omega_b));
        assert_eq!(chart.omega_mark, "Ω_s");
        assert_eq!(chart.everywhere_id.as_deref(), Some("Y"));
        assert_eq!(chart.omega_copy.as_deref(), Some(WEIGHT_ROBUST_EVERYWHERE));
        assert_eq!(chart.shares[0].fraction, "0/14");
        assert_eq!(chart.shares[1].id, "Y");
        assert_eq!(chart.shares[1].fraction, "14/14");
        assert_eq!(chart.behaviour_mark, None);
        let quality = chart
            .sentences
            .iter()
            .find(|sentence| sentence.fixed == "质量")
            .unwrap();
        assert_eq!(
            quality.text,
            "保持质量权重不变，只要时间权重超过成本权重的 1.01 倍，Y 就会超过 X；这是在当前估计值下的结论"
        );
        copy_matches_core(&chart);
    }

    #[test]
    fn first_window_and_missing_anchor_withhold_the_chart() {
        let app = AppState::open_in_memory().unwrap();
        let first = view(&app, "first").unwrap();
        assert!(!first.show_chart);
        assert!(first.chart.is_none());
        assert_eq!(first.withheld.as_deref(), Some("first-window"));
        assert_eq!(first.withheld_text.as_deref(), Some(FIRST_WINDOW_LABEL));

        let missing = view(&app, "example7").unwrap();
        assert!(!missing.show_chart);
        assert!(missing.chart.is_none());
        assert_eq!(missing.withheld.as_deref(), Some("no-reference"));
        assert!(missing
            .withheld_text
            .as_deref()
            .is_some_and(|text| text.contains("真不合格")));
    }

    #[test]
    fn tie_keeps_the_whole_winner_group_and_the_core_sentence() {
        let chart = chart("tie");
        assert!(chart.points.iter().all(|point| point.winners == ["X", "Y"]));
        assert!(chart
            .sentences
            .iter()
            .any(|sentence| sentence.text.contains("并列")));
        copy_matches_core(&chart);
    }

    #[test]
    fn example6_and_example8_show_a_chart_from_the_core() {
        for source in ["example6", "example8"] {
            let chart = chart(source);
            assert_eq!(chart.points.len(), 171, "{source}");
            assert!(!chart.sentences.is_empty(), "{source}");
            copy_matches_core(&chart);
        }
    }

    #[test]
    fn store_without_a_window_has_no_chart() {
        let app = AppState::open_in_memory().unwrap();
        let view = view(&app, "store").unwrap();
        assert!(!view.show_chart);
        assert!(view.chart.is_none());
        assert_eq!(view.empty_reason.as_deref(), Some("还没有开始的窗"));
    }

    #[test]
    fn a_started_first_window_with_no_tasks_has_no_chart() {
        let app = AppState::open_in_memory().unwrap();
        crate::elicit_cmd::begin_window(&app).unwrap();
        let view = view(&app, "store").unwrap();
        assert!(!view.show_chart);
        assert!(view.chart.is_none());
        assert_eq!(view.withheld.as_deref(), Some("first-window"));
        assert_eq!(view.withheld_text.as_deref(), Some(FIRST_WINDOW_LABEL));
    }

    #[test]
    fn unknown_source_is_an_error() {
        let app = AppState::open_in_memory().unwrap();
        assert!(view(&app, "nope").is_err());
    }
}

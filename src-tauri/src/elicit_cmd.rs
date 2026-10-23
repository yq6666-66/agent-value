//! Five-level self-report commands.
//!
//! Question wording stays in the webview. This module asks
//! `agent_value_core::elicit` for the next question, the regions and the
//! centroid, then stores the result on the coding scope's `WeightLedger`.
//! A change while a window is frozen is a trial and does not rewrite that
//! window's official version.

use std::sync::Mutex;

use agent_value_core::elicit::{
    bank, regions, self_report, show, Answer, Question, QuestionBase, Session, Shown,
    FIRST_WINDOW_COST, FIRST_WINDOW_TIME,
};
use agent_value_core::grid::GridPoint;
use agent_value_core::scope::{ScopeId, TaskKind};
use agent_value_core::weights::{
    Evidence, FiveLevel, SelfReportAnswer, TrialPreview, TrialResolution, VersionStatus,
    WeightLedger, WeightSource, WeightVersion, Weights,
};
use agent_value_core::window::{FrozenInputs, WindowIndex};
use serde::Deserialize;
use serde::Serialize;

use crate::store_cmd::{self, AppState};

/// Defect rate used when the webview does not pass a base.
///
/// Cost and time for the first window come from the core. The defect base is
/// caller-supplied; 0.1 is the shell default that keeps every bank question
/// showable.
const DEFAULT_DISPLAY_DEFECT: f64 = 0.1;

/// Display base the webview may override. Omitted fields are not accepted;
/// pass nothing to use the shell default.
#[derive(Debug, Deserialize)]
pub struct BaseInput {
    pub cost: f64,
    pub time: f64,
    pub defect: f64,
}

/// The question index the session is asking, and the five-level answer.
#[derive(Debug, Deserialize)]
pub struct AnswerInput {
    pub question: usize,
    pub level: i8,
}

/// Manual weights. The core checks the floor and the sum.
#[derive(Debug, Deserialize)]
pub struct WeightInput {
    pub a: f64,
    pub b: f64,
    pub g: f64,
}

/// One grid weight, copied from a core point. The webview does not rebuild it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WeightTriple {
    pub a: f64,
    pub b: f64,
    pub g: f64,
}

/// Option A then option B, as [`Shown`] returns them.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ShownView {
    pub cost: [f64; 2],
    pub time: [f64; 2],
    pub defect: [f64; 2],
}

/// The question to display. Wording is not included.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuestionView {
    pub index: usize,
    pub rho: [f64; 3],
    pub shown: ShownView,
}

/// One step of the wizard.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StepView {
    pub finished: bool,
    pub answered: usize,
    pub skipped: Vec<usize>,
    pub wide_len: usize,
    pub core_len: usize,
    pub m: u32,
    pub question: Option<QuestionView>,
}

/// `Omega_core`, `Omega_s`, and the centroid once any answer exists.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RegionsView {
    pub core: Vec<WeightTriple>,
    pub wide: Vec<WeightTriple>,
    pub m: u32,
    pub point: Option<WeightTriple>,
}

/// Finished self-report. `regions.point` is the core centroid.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReportView {
    pub regions: RegionsView,
    pub contradiction: bool,
    pub contradicted: Vec<usize>,
    pub ranges: [[f64; 2]; 3],
}

/// A ledger version the webview may show.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VersionView {
    pub id: u64,
    pub a: f64,
    pub b: f64,
    pub g: f64,
    pub source: String,
    pub trial: bool,
    pub effective_from: Option<u32>,
}

/// Trial weights beside the official version of the same window.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TrialView {
    pub label: String,
    pub trial: VersionView,
    pub official: VersionView,
}

/// What `elicit_save` wrote.
///
/// `outcome` is `"trial"` or `"official"`. A trial leaves `official` equal to
/// the frozen version that was already in effect.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SaveView {
    pub outcome: String,
    pub label: Option<String>,
    pub applied: VersionView,
    pub official: VersionView,
    pub point: WeightTriple,
}

/// Whether the coding scope is inside a frozen window, and the weights to show.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WeightContext {
    pub scope: String,
    pub in_window: bool,
    pub window_index: Option<u32>,
    pub official: VersionView,
    pub trial: Option<TrialView>,
}

struct ActiveSession {
    session: Session,
    base: QuestionBase,
    saved: bool,
}

pub struct ElicitState {
    active: Mutex<Option<ActiveSession>>,
}

impl ElicitState {
    pub fn new() -> Self {
        Self {
            active: Mutex::new(None),
        }
    }
}

fn lock(state: &ElicitState) -> Result<std::sync::MutexGuard<'_, Option<ActiveSession>>, String> {
    state
        .active
        .lock()
        .map_err(|_| "elicit lock poisoned".to_string())
}

fn coding_scope() -> ScopeId {
    ScopeId::new(TaskKind::Coding)
}

fn question_base(input: Option<BaseInput>) -> Result<QuestionBase, String> {
    let base = match input {
        None => QuestionBase {
            cost: FIRST_WINDOW_COST,
            time: FIRST_WINDOW_TIME,
            defect: DEFAULT_DISPLAY_DEFECT,
        },
        Some(input) => QuestionBase {
            cost: input.cost,
            time: input.time,
            defect: input.defect,
        },
    };
    // `show` rejects a base the core cannot display. The question itself is unused.
    show(&bank()[0], base).map_err(|err| err.to_string())?;
    Ok(base)
}

fn triple(point: GridPoint) -> WeightTriple {
    let [a, b, g] = point.as_array();
    WeightTriple { a, b, g }
}

fn version_view(version: &WeightVersion) -> VersionView {
    let weights = version.weights();
    VersionView {
        id: version.id().0,
        a: weights.a(),
        b: weights.b(),
        g: weights.g(),
        source: version.source().as_str().to_string(),
        trial: version.is_trial(),
        effective_from: version.effective_from().map(|window| window.0),
    }
}

fn trial_view(preview: &TrialPreview) -> TrialView {
    TrialView {
        label: preview.label.to_string(),
        trial: version_view(&preview.trial),
        official: version_view(&preview.official),
    }
}

fn question_view(index: usize, question: &Question, shown: Shown) -> QuestionView {
    QuestionView {
        index,
        rho: question.rho(),
        shown: ShownView {
            cost: shown.cost,
            time: shown.time,
            defect: shown.defect,
        },
    }
}

fn step_view(session: &Session, question: Option<QuestionView>) -> Result<StepView, String> {
    let regions = regions(session.answers()).map_err(|err| err.to_string())?;
    Ok(StepView {
        finished: session.finished(),
        answered: session.answers().len(),
        skipped: session.skipped().to_vec(),
        wide_len: regions.wide.len(),
        core_len: regions.core.len(),
        m: regions.m,
        question,
    })
}

/// Skips questions [`show`] cannot display, then returns the next one.
fn advance(session: &mut Session, base: QuestionBase) -> Result<StepView, String> {
    let questions = bank();
    loop {
        if session.finished() {
            return step_view(session, None);
        }
        let Some(index) = session.next_question() else {
            return Err("no question left before the stop rule".to_string());
        };
        match show(&questions[index], base).map_err(|err| err.to_string())? {
            None => session.skip(index).map_err(|err| err.to_string())?,
            Some(shown) => {
                return step_view(
                    session,
                    Some(question_view(index, &questions[index], shown)),
                );
            }
        }
    }
}

fn regions_view(answers: &[Answer]) -> Result<RegionsView, String> {
    let found = regions(answers).map_err(|err| err.to_string())?;
    let point = if answers.is_empty() {
        None
    } else {
        Some(triple(
            self_report(answers).map_err(|err| err.to_string())?.point,
        ))
    };
    Ok(RegionsView {
        core: found.core.iter().copied().map(triple).collect(),
        wide: found.wide.iter().copied().map(triple).collect(),
        m: found.m,
        point,
    })
}

fn report_view(answers: &[Answer]) -> Result<ReportView, String> {
    let report = self_report(answers).map_err(|err| err.to_string())?;
    Ok(ReportView {
        regions: RegionsView {
            core: report.regions.core.iter().copied().map(triple).collect(),
            wide: report.regions.wide.iter().copied().map(triple).collect(),
            m: report.regions.m,
            point: Some(triple(report.point)),
        },
        contradiction: report.contradiction,
        contradicted: report.contradicted,
        ranges: report.ranges.map(|(lo, hi)| [lo, hi]),
    })
}

fn require_active(
    state: &ElicitState,
) -> Result<std::sync::MutexGuard<'_, Option<ActiveSession>>, String> {
    let guard = lock(state)?;
    if guard.is_none() {
        return Err("no self-report in progress".to_string());
    }
    Ok(guard)
}

/// Starts a session. Replaces one that was already running.
pub fn start(state: &ElicitState, base: Option<BaseInput>) -> Result<StepView, String> {
    let base = question_base(base)?;
    let mut session = Session::new();
    let step = advance(&mut session, base)?;
    *lock(state)? = Some(ActiveSession {
        session,
        base,
        saved: false,
    });
    Ok(step)
}

/// Drops the in-memory session. Stored ledger rows are left to the store wipe.
pub fn reset(state: &ElicitState) -> Result<(), String> {
    *lock(state)? = None;
    Ok(())
}

/// The current question, after skipping any that cannot be shown.
pub fn next_step(state: &ElicitState) -> Result<StepView, String> {
    let mut guard = require_active(state)?;
    let active = guard.as_mut().expect("checked");
    advance(&mut active.session, active.base)
}

pub fn answer(state: &ElicitState, input: AnswerInput) -> Result<StepView, String> {
    let mut guard = require_active(state)?;
    let active = guard.as_mut().expect("checked");
    if active.saved {
        return Err("self-report already saved".to_string());
    }
    let level = FiveLevel::from_level(input.level).map_err(|err| err.to_string())?;
    active
        .session
        .answer(input.question, level)
        .map_err(|err| err.to_string())?;
    advance(&mut active.session, active.base)
}

pub fn skip(state: &ElicitState, question: usize) -> Result<StepView, String> {
    let mut guard = require_active(state)?;
    let active = guard.as_mut().expect("checked");
    if active.saved {
        return Err("self-report already saved".to_string());
    }
    active
        .session
        .skip(question)
        .map_err(|err| err.to_string())?;
    advance(&mut active.session, active.base)
}

pub fn regions_of(state: &ElicitState) -> Result<RegionsView, String> {
    let guard = require_active(state)?;
    let active = guard.as_ref().expect("checked");
    regions_view(active.session.answers())
}

pub fn finish(state: &ElicitState) -> Result<ReportView, String> {
    let guard = require_active(state)?;
    let active = guard.as_ref().expect("checked");
    if !active.session.finished() {
        return Err("self-report is not finished".to_string());
    }
    report_view(active.session.answers())
}

fn self_report_evidence(answers: &[Answer]) -> Vec<SelfReportAnswer> {
    answers
        .iter()
        .map(|item| SelfReportAnswer {
            question_id: item.question.to_string(),
            answer: item.answer,
        })
        .collect()
}

fn persist_trial(
    ledger: &mut WeightLedger,
    weights: Weights,
    current: WindowIndex,
    point: WeightTriple,
) -> Result<SaveView, String> {
    let official_before = ledger
        .frozen(current)
        .map_err(|err| err.to_string())?
        .weight_version()
        .id();
    let official_count = ledger.versions().iter().filter(|v| !v.is_trial()).count();
    let id = ledger
        .create_trial(weights, current)
        .map_err(|err| err.to_string())?;
    let preview = ledger.trial_preview(id).map_err(|err| err.to_string())?;
    if preview.official.id() != official_before {
        return Err("frozen official version changed".to_string());
    }
    if ledger.versions().iter().filter(|v| !v.is_trial()).count() != official_count {
        return Err("trial save created an official version".to_string());
    }
    Ok(SaveView {
        outcome: "trial".to_string(),
        label: Some(preview.label.to_string()),
        applied: version_view(&preview.trial),
        official: version_view(&preview.official),
        point,
    })
}

fn persist_official(
    ledger: &mut WeightLedger,
    weights: Weights,
    answers: &[Answer],
    point: WeightTriple,
) -> Result<SaveView, String> {
    let effective = ledger.next_unfrozen().map_err(|err| err.to_string())?;
    let id = ledger
        .schedule_official(
            weights,
            WeightSource::SelfReport,
            Evidence::SelfReportAnswers(self_report_evidence(answers)),
            effective,
        )
        .map_err(|err| err.to_string())?;
    if ledger.versions().iter().any(|version| version.is_trial()) {
        return Err("official save created a trial".to_string());
    }
    let applied = version_view(ledger.version(id).map_err(|err| err.to_string())?);
    Ok(SaveView {
        outcome: "official".to_string(),
        label: None,
        official: applied.clone(),
        applied,
        point,
    })
}

/// Writes the finished self-report.
///
/// Inside a frozen window this is only a trial. Before any window is frozen
/// it is an official self-report from the next unfrozen window.
pub fn save(app: &AppState, elicit: &ElicitState) -> Result<SaveView, String> {
    let mut guard = require_active(elicit)?;
    let active = guard.as_mut().expect("checked");
    if active.saved {
        return Err("self-report already saved".to_string());
    }
    if !active.session.finished() {
        return Err("self-report is not finished".to_string());
    }
    let answers = active.session.answers().to_vec();
    let report = self_report(&answers).map_err(|err| err.to_string())?;
    let weights = report.point.weights().map_err(|err| err.to_string())?;
    let point = triple(report.point);
    let view = store_cmd::update_weight_ledger(app, &coding_scope(), |ledger| {
        match ledger.last_frozen() {
            Some(current) => persist_trial(ledger, weights, current, point.clone()),
            None => persist_official(ledger, weights, &answers, point.clone()),
        }
    })?;
    active.saved = true;
    Ok(view)
}

fn pending_trial(ledger: &WeightLedger, window: WindowIndex) -> Option<TrialView> {
    let id = ledger
        .versions()
        .iter()
        .rev()
        .find_map(|version| match version.status() {
            VersionStatus::Trial {
                created_in,
                resolution: TrialResolution::Pending,
            } if created_in == window => Some(version.id()),
            _ => None,
        })?;
    ledger
        .trial_preview(id)
        .ok()
        .map(|preview| trial_view(&preview))
}

fn context_from(ledger: &WeightLedger) -> WeightContext {
    let current = ledger.last_frozen();
    let official = ledger.official_for(current.unwrap_or(WindowIndex::FIRST));
    WeightContext {
        scope: ledger.scope().to_string(),
        in_window: current.is_some(),
        window_index: current.map(|window| window.0),
        official: version_view(official),
        trial: current.and_then(|window| pending_trial(ledger, window)),
    }
}

pub fn context(app: &AppState) -> Result<WeightContext, String> {
    let ledger = store_cmd::read_weight_ledger(app, &coding_scope())?;
    Ok(context_from(&ledger))
}

/// Freezes the next window so later weight edits take the trial path.
///
/// Window 0 is a first window. A later window is marked as not first; the
/// real prior-task count arrives with window results.
pub fn begin_window(app: &AppState) -> Result<WeightContext, String> {
    store_cmd::update_weight_ledger(app, &coding_scope(), |ledger| {
        let window = ledger.next_unfrozen().map_err(|err| err.to_string())?;
        let prior = u64::from(window != WindowIndex::FIRST);
        ledger
            .freeze_window(window, prior, FrozenInputs::default())
            .map_err(|err| err.to_string())?;
        Ok(())
    })?;
    context(app)
}

/// A weight edit during the frozen window. Refuses to write when no window
/// is in progress, so this command cannot create an official version.
pub fn edit_weights(app: &AppState, input: WeightInput) -> Result<TrialView, String> {
    let weights = Weights::new(input.a, input.b, input.g).map_err(|err| err.to_string())?;
    store_cmd::update_weight_ledger(app, &coding_scope(), |ledger| {
        let Some(current) = ledger.last_frozen() else {
            return Err("no-window-in-progress".to_string());
        };
        let before = ledger
            .frozen(current)
            .map_err(|err| err.to_string())?
            .weight_version()
            .id();
        let official_count = ledger.versions().iter().filter(|v| !v.is_trial()).count();
        let id = ledger
            .create_trial(weights, current)
            .map_err(|err| err.to_string())?;
        let preview = ledger.trial_preview(id).map_err(|err| err.to_string())?;
        if preview.official.id() != before || !preview.trial.is_trial() {
            return Err("weight edit did not stay a trial of the frozen version".to_string());
        }
        if ledger.versions().iter().filter(|v| !v.is_trial()).count() != official_count {
            return Err("weight edit created an official version".to_string());
        }
        Ok(trial_view(&preview))
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use agent_value_core::elicit::{
        bank, level, regions, self_report, show, QuestionBase, Session, BANK_SIZE,
    };
    use agent_value_core::grid::GridPoint;
    use agent_value_core::weights::{
        Evidence, FiveLevel, VersionId, WeightSource, Weights, TRIAL_LABEL,
    };
    use agent_value_core::window::WindowIndex;

    use super::{
        answer, begin_window, coding_scope, context, edit_weights, finish, next_step,
        question_base, regions_of, reset, save, skip, start, store_cmd, AnswerInput, AppState,
        BaseInput, ElicitState, WeightInput, WeightTriple, DEFAULT_DISPLAY_DEFECT,
        FIRST_WINDOW_COST, FIRST_WINDOW_TIME,
    };

    fn truth() -> GridPoint {
        GridPoint::new(4, 7).unwrap()
    }

    fn assert_same_points(got: &[WeightTriple], expected: &[GridPoint]) {
        assert_eq!(got.len(), expected.len());
        for (got, expected) in got.iter().zip(expected) {
            assert_eq!(got.a, expected.a());
            assert_eq!(got.b, expected.b());
            assert_eq!(got.g, expected.g());
        }
    }

    fn drive_honest(elicit: &ElicitState, truth: GridPoint) -> Session {
        let questions = bank();
        let base = question_base(None).unwrap();
        let mut oracle = Session::new();
        let mut step = start(elicit, None).unwrap();
        while !step.finished {
            let question = step
                .question
                .as_ref()
                .expect("unfinished session has a question");
            assert_eq!(oracle.next_question(), Some(question.index));
            assert_eq!(question.rho, questions[question.index].rho());
            let shown = show(&questions[question.index], base).unwrap().unwrap();
            assert_eq!(question.shown.cost, shown.cost);
            assert_eq!(question.shown.time, shown.time);
            assert_eq!(question.shown.defect, shown.defect);
            let lvl = level(truth, &questions[question.index]);
            oracle
                .answer(question.index, FiveLevel::from_level(lvl).unwrap())
                .unwrap();
            step = answer(
                elicit,
                AnswerInput {
                    question: question.index,
                    level: lvl,
                },
            )
            .unwrap();
            let expected = regions(oracle.answers()).unwrap();
            assert_eq!(step.core_len, expected.core.len());
            assert_eq!(step.wide_len, expected.wide.len());
            assert_eq!(step.m, expected.m);
            assert_eq!(step.finished, oracle.finished());
            assert_eq!(step.answered, oracle.answers().len());
        }
        assert!(step.question.is_none());
        oracle
    }

    fn temp_app(name: &str) -> (AppState, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("agent-value-elicit-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let app = AppState::open(&dir).unwrap();
        (app, dir)
    }

    #[test]
    fn start_matches_the_bank_and_leaves_the_centroid_empty() {
        let elicit = ElicitState::new();
        let step = start(&elicit, None).unwrap();
        let question = step.question.as_ref().expect("first question");
        let questions = bank();
        assert_eq!(question.rho, questions[question.index].rho());
        let base = question_base(None).unwrap();
        assert_eq!(base.cost, FIRST_WINDOW_COST);
        assert_eq!(base.time, FIRST_WINDOW_TIME);
        assert_eq!(base.defect, DEFAULT_DISPLAY_DEFECT);
        let shown = show(&questions[question.index], base).unwrap().unwrap();
        assert_eq!(question.shown.cost, shown.cost);
        let regions = regions_of(&elicit).unwrap();
        assert!(regions.point.is_none());
        assert_eq!(regions.wide.len(), regions.core.len());
        let json = serde_json::to_value(&step).unwrap();
        assert!(json["finished"].is_boolean());
        assert_eq!(json["question"]["rho"].as_array().unwrap().len(), 3);
        assert_eq!(
            json["question"]["shown"]["cost"].as_array().unwrap().len(),
            2
        );
        assert!(json["question"]["shown"].get("time").is_some());
        assert!(json["question"]["shown"].get("defect").is_some());
    }

    #[test]
    fn honest_session_matches_core_regions_and_centroid() {
        let elicit = ElicitState::new();
        let oracle = drive_honest(&elicit, truth());
        let expected = self_report(oracle.answers()).unwrap();
        let regions = regions_of(&elicit).unwrap();
        assert_same_points(&regions.core, &expected.regions.core);
        assert_same_points(&regions.wide, &expected.regions.wide);
        assert_eq!(regions.m, expected.regions.m);
        let point = regions.point.expect("centroid");
        assert_eq!(point.a, expected.point.a());
        assert_eq!(point.b, expected.point.b());
        assert_eq!(point.g, expected.point.g());

        let report = finish(&elicit).unwrap();
        assert_eq!(report.contradiction, expected.contradiction);
        assert_eq!(report.contradicted, expected.contradicted);
        assert_same_points(&report.regions.core, &expected.regions.core);
        assert_eq!(report.regions.point.unwrap().a, expected.point.a());
        for (got, (lo, hi)) in report.ranges.iter().zip(expected.ranges) {
            assert_eq!(got[0], lo);
            assert_eq!(got[1], hi);
        }
    }

    #[test]
    fn wrong_question_is_rejected_and_the_session_stays() {
        let elicit = ElicitState::new();
        let step = start(&elicit, None).unwrap();
        let index = step.question.as_ref().unwrap().index;
        let err = answer(
            &elicit,
            AnswerInput {
                question: index + 1,
                level: 0,
            },
        )
        .unwrap_err();
        assert!(err.contains("not the question"));
        let again = next_step(&elicit).unwrap();
        assert_eq!(again.question.unwrap().index, index);
    }

    #[test]
    fn invalid_base_does_not_start_a_session() {
        let elicit = ElicitState::new();
        let err = start(
            &elicit,
            Some(BaseInput {
                cost: 0.0,
                time: FIRST_WINDOW_TIME,
                defect: DEFAULT_DISPLAY_DEFECT,
            }),
        )
        .unwrap_err();
        assert!(!err.is_empty());
        assert!(regions_of(&elicit).is_err());
    }

    #[test]
    fn unshowable_questions_are_skipped_before_they_are_returned() {
        let elicit = ElicitState::new();
        let base = QuestionBase {
            cost: 0.5,
            time: 120.0,
            defect: 0.4,
        };
        let questions = bank();
        let mut step = start(
            &elicit,
            Some(BaseInput {
                cost: base.cost,
                time: base.time,
                defect: base.defect,
            }),
        )
        .unwrap();
        let mut saw_gap = false;
        for _ in 0..BANK_SIZE {
            let Some(question) = step.question.as_ref() else {
                break;
            };
            assert!(show(&questions[question.index], base).unwrap().is_some());
            let before = step.skipped.len();
            step = skip(&elicit, question.index).unwrap();
            if step.skipped.len() > before + 1 {
                for index in &step.skipped[before + 1..] {
                    assert!(show(&questions[*index], base).unwrap().is_none());
                }
                saw_gap = true;
                break;
            }
        }
        assert!(saw_gap, "an unshowable question should be skipped");
        if let Some(question) = step.question.as_ref() {
            assert!(show(&questions[question.index], base).unwrap().is_some());
        }
    }

    #[test]
    fn reset_drops_the_session() {
        let elicit = ElicitState::new();
        start(&elicit, None).unwrap();
        reset(&elicit).unwrap();
        assert!(regions_of(&elicit).is_err());
        assert!(finish(&elicit).is_err());
    }

    #[test]
    fn save_before_a_window_is_an_official_self_report() {
        let (app, dir) = temp_app("official");
        let elicit = ElicitState::new();
        let oracle = drive_honest(&elicit, truth());
        let expected = self_report(oracle.answers()).unwrap();
        let view = save(&app, &elicit).unwrap();
        assert_eq!(view.outcome, "official");
        assert_eq!(view.label, None);
        assert_eq!(view.applied.source, "self-report");
        assert!(!view.applied.trial);
        assert_eq!(view.applied.id, view.official.id);
        assert_eq!(view.point.a, expected.point.a());
        assert_eq!(view.applied.a, expected.point.weights().unwrap().a());

        let again = save(&app, &elicit).unwrap_err();
        assert!(again.contains("already saved"));

        drop(app);
        let app = AppState::open(&dir).unwrap();
        let ledger = store_cmd::read_weight_ledger(&app, &coding_scope()).unwrap();
        assert!(ledger.frozen_windows().is_empty());
        assert!(ledger.versions().iter().all(|version| !version.is_trial()));
        let stored = ledger.version(VersionId(view.applied.id)).unwrap();
        assert_eq!(stored.source(), WeightSource::SelfReport);
        assert_eq!(stored.effective_from(), Some(WindowIndex::FIRST));
        match stored.evidence() {
            Evidence::SelfReportAnswers(answers) => {
                assert_eq!(answers.len(), oracle.answers().len());
                for (stored, given) in answers.iter().zip(oracle.answers()) {
                    assert_eq!(stored.question_id, given.question.to_string());
                    assert_eq!(stored.answer, given.answer);
                }
            }
            other => panic!("expected self-report evidence, got {other:?}"),
        }
        let ctx = context(&app).unwrap();
        assert!(!ctx.in_window);
        assert_eq!(ctx.official.id, view.applied.id);
        assert!(ctx.trial.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_during_a_window_is_only_a_trial() {
        let (app, dir) = temp_app("trial-save");
        begin_window(&app).unwrap();
        let elicit = ElicitState::new();
        let oracle = drive_honest(&elicit, truth());
        let expected = self_report(oracle.answers()).unwrap();
        let view = save(&app, &elicit).unwrap();
        assert_eq!(view.outcome, "trial");
        assert_eq!(view.label.as_deref(), Some(TRIAL_LABEL));
        assert_eq!(view.official.id, 0);
        assert!(!view.official.trial);
        assert!(view.applied.trial);
        assert_eq!(view.applied.source, "manual");
        assert_eq!(view.official.source, "default");
        assert_eq!(view.applied.a, expected.point.weights().unwrap().a());
        assert_eq!(view.official.a, Weights::equal().a());

        drop(app);
        let app = AppState::open(&dir).unwrap();
        let ledger = store_cmd::read_weight_ledger(&app, &coding_scope()).unwrap();
        let frozen = ledger.frozen(WindowIndex::FIRST).unwrap();
        assert_eq!(frozen.weight_version().id().0, 0);
        assert_eq!(frozen.weight_version().weights(), Weights::equal());
        assert_eq!(frozen.weight_version().source(), WeightSource::Default);
        let trials: Vec<_> = ledger.versions().iter().filter(|v| v.is_trial()).collect();
        assert_eq!(trials.len(), 1);
        assert_eq!(trials[0].source(), WeightSource::Manual);
        assert_eq!(trials[0].evidence(), &Evidence::None);
        assert_eq!(
            ledger.versions().iter().filter(|v| !v.is_trial()).count(),
            1
        );
        let ctx = context(&app).unwrap();
        assert!(ctx.in_window);
        assert_eq!(ctx.official.id, 0);
        let trial = ctx.trial.expect("pending trial");
        assert_eq!(trial.label, TRIAL_LABEL);
        assert_eq!(trial.official.id, 0);
        assert_ne!(trial.trial.id, trial.official.id);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn mid_window_weight_edit_creates_a_trial_and_leaves_the_official_version() {
        let (app, dir) = temp_app("edit");
        let err = edit_weights(
            &app,
            WeightInput {
                a: 0.5,
                b: 0.25,
                g: 0.25,
            },
        )
        .unwrap_err();
        assert!(err.contains("no-window-in-progress"));

        let opened = begin_window(&app).unwrap();
        assert!(opened.in_window);
        assert_eq!(opened.window_index, Some(0));
        assert_eq!(opened.official.source, "default");
        assert!(opened.trial.is_none());

        let err = edit_weights(
            &app,
            WeightInput {
                a: 0.5,
                b: 0.5,
                g: 0.5,
            },
        )
        .unwrap_err();
        assert!(!err.contains("no-window-in-progress"));
        let unchanged = store_cmd::read_weight_ledger(&app, &coding_scope()).unwrap();
        assert_eq!(unchanged.versions().len(), 1);

        let view = edit_weights(
            &app,
            WeightInput {
                a: 0.5,
                b: 0.25,
                g: 0.25,
            },
        )
        .unwrap();
        assert_eq!(view.label, TRIAL_LABEL);
        assert!(view.trial.trial);
        assert!(!view.official.trial);
        assert_eq!(view.official.id, 0);
        assert_eq!(view.official.a, Weights::equal().a());
        assert_eq!(view.trial.a, 0.5);
        assert_eq!(view.trial.b, 0.25);
        assert_eq!(view.trial.g, 0.25);
        assert_eq!(view.trial.source, "manual");

        drop(app);
        let app = AppState::open(&dir).unwrap();
        let ledger = store_cmd::read_weight_ledger(&app, &coding_scope()).unwrap();
        assert_eq!(
            ledger
                .frozen(WindowIndex::FIRST)
                .unwrap()
                .weight_version()
                .id()
                .0,
            0
        );
        assert_eq!(
            ledger
                .frozen(WindowIndex::FIRST)
                .unwrap()
                .weight_version()
                .weights(),
            Weights::equal()
        );
        assert_eq!(
            ledger.versions().iter().filter(|v| !v.is_trial()).count(),
            1
        );
        assert_eq!(ledger.versions().iter().filter(|v| v.is_trial()).count(), 1);
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["label"], TRIAL_LABEL);
        assert_eq!(json["official"]["trial"], false);
        assert_eq!(json["trial"]["trial"], true);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn finish_before_the_stop_rule_is_rejected() {
        let elicit = ElicitState::new();
        start(&elicit, None).unwrap();
        let err = finish(&elicit).unwrap_err();
        assert!(err.contains("not finished"));
        let app = AppState::open_in_memory().unwrap();
        let err = save(&app, &elicit).unwrap_err();
        assert!(err.contains("not finished"));
        let ledger = store_cmd::read_weight_ledger(&app, &coding_scope()).unwrap();
        assert_eq!(ledger.versions().len(), 1);
        assert!(ledger.frozen_windows().is_empty());
    }
}

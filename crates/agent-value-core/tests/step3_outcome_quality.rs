//! Step 3 (v22 §3.1): task outcome judgement and quality signals.
//!
//! The v22 recomputation output has no step-3 numbers; the expected q_hat
//! values below are derived by hand from the §3 formula and checked to 1e-9.

use agent_value_core::arm::{Arm, ModelId, Protocol, ProviderKind, Route};
use agent_value_core::cells::{CellId, CellWeights};
use agent_value_core::outcome::{
    judge, CheckRun, CheckStatus, FollowUpMessage, Rollback, Rule, TaskSignals, TAU_KEEP_MS,
    TAU_RE_MS,
};
use agent_value_core::quality::{
    pooled_prior, q_hat, quality_mode, should_ask_spot_rating, shrunk_quality, stars_to_quality,
    QualityMode, Rating, Q_HAT_MAX, SUCCESS_RATE_MODE_LABEL,
};
use agent_value_core::records::Outcome;
use agent_value_core::scope::TaskKind;

const TOL: f64 = 1e-9;
const END: i64 = 1_800_000_000_000;

fn close(a: f64, b: f64) {
    assert!((a - b).abs() <= TOL, "{a} vs {b}");
}

fn ended(kind: TaskKind) -> TaskSignals {
    let mut s = TaskSignals::new(kind);
    s.ended_ms = Some(END);
    s
}

fn completed(kind: TaskKind) -> TaskSignals {
    let mut s = ended(kind);
    s.agent_reported_completion = true;
    s
}

fn check(at_ms: i64, status: CheckStatus) -> CheckRun {
    CheckRun { at_ms, status }
}

fn redo(at_ms: i64) -> FollowUpMessage {
    FollowUpMessage {
        at_ms,
        from_user: true,
        same_session_or_workdir: true,
        classified_as_redo: true,
    }
}

fn rollback(at_ms: i64) -> Rollback {
    Rollback {
        at_ms,
        whole_change: true,
    }
}

const LATER: i64 = END + 2 * TAU_KEEP_MS;

// ---- outcome rules ----------------------------------------------------

#[test]
fn explicit_abort_wins_over_passing_checks() {
    let mut s = completed(TaskKind::Coding);
    s.checks.push(check(END - 5, CheckStatus::Passed));
    s.explicit_abort = true;
    let j = judge(&s, LATER).unwrap();
    assert_eq!((j.outcome, j.rule), (Outcome::Cancel, Rule::ExplicitAbort));
    assert!(!j.provisional);
}

#[test]
fn explicit_abort_wins_over_failed_check_and_rollback() {
    let mut s = completed(TaskKind::Coding);
    s.checks.push(check(END - 5, CheckStatus::Failed));
    s.rollbacks.push(rollback(END + 10));
    s.explicit_abort = true;
    assert_eq!(judge(&s, LATER).unwrap().outcome, Outcome::Cancel);
}

#[test]
fn abort_of_a_running_task_is_cancel() {
    let mut s = TaskSignals::new(TaskKind::NonCoding);
    s.explicit_abort = true;
    assert_eq!(judge(&s, END).unwrap().outcome, Outcome::Cancel);
}

#[test]
fn no_completion_signal_is_undetermined_not_success() {
    let mut s = ended(TaskKind::Coding);
    s.checks.push(check(END - 5, CheckStatus::Passed));
    let j = judge(&s, LATER).unwrap();
    assert_eq!(
        (j.outcome, j.rule),
        (Outcome::Undetermined, Rule::NoCompletionSignal)
    );
    assert_ne!(j.outcome, Outcome::Success);
    assert!(!j.provisional);
}

#[test]
fn interrupted_session_without_completion_is_undetermined() {
    let mut s = ended(TaskKind::NonCoding);
    s.session_interrupted = true;
    assert_eq!(judge(&s, LATER).unwrap().outcome, Outcome::Undetermined);

    // Rules apply in order: a reported completion (rule 5) comes before
    // the "interrupted session" example of rule 6.
    let mut s = completed(TaskKind::NonCoding);
    s.session_interrupted = true;
    assert_eq!(judge(&s, LATER).unwrap().outcome, Outcome::Success);

    // No known end: rule 2 and the windows are undefined => undetermined,
    // even with a failed check or a reported completion.
    let mut no_end = TaskSignals::new(TaskKind::Coding);
    no_end.session_interrupted = true;
    no_end.agent_reported_completion = true;
    no_end.checks.push(check(END, CheckStatus::Failed));
    let j = judge(&no_end, LATER).unwrap();
    assert_eq!(j.outcome, Outcome::Undetermined);
    assert!(!j.provisional);
}

#[test]
fn running_task_is_pending() {
    let mut s = TaskSignals::new(TaskKind::Coding);
    s.agent_reported_completion = true;
    let j = judge(&s, END).unwrap();
    assert_eq!((j.outcome, j.rule), (Outcome::Pending, Rule::StillRunning));
}

#[test]
fn last_check_at_end_decides_rule_two() {
    // Fails then passes: the last one passed.
    let mut s = completed(TaskKind::Coding);
    s.checks.push(check(END - 20, CheckStatus::Failed));
    s.checks.push(check(END - 10, CheckStatus::Passed));
    assert_eq!(judge(&s, LATER).unwrap().outcome, Outcome::Success);

    // Passes then fails: failure even though the agent reported completion.
    let mut s = completed(TaskKind::Coding);
    s.checks.push(check(END - 20, CheckStatus::Passed));
    s.checks.push(check(END - 10, CheckStatus::Failed));
    let j = judge(&s, LATER).unwrap();
    assert_eq!((j.outcome, j.rule), (Outcome::Failure, Rule::FailedCheck));

    // An errored / timed-out check is not a pass.
    let mut s = completed(TaskKind::Coding);
    s.checks.push(check(END, CheckStatus::Errored));
    assert_eq!(judge(&s, LATER).unwrap().outcome, Outcome::Failure);

    // Same timestamp: the non-passing result wins.
    let mut s = completed(TaskKind::Coding);
    s.checks.push(check(END, CheckStatus::Passed));
    s.checks.push(check(END, CheckStatus::Failed));
    assert_eq!(judge(&s, LATER).unwrap().outcome, Outcome::Failure);

    // A check after the task end is not "the last check at task end".
    let mut s = completed(TaskKind::Coding);
    s.checks.push(check(END - 10, CheckStatus::Passed));
    s.checks.push(check(END + 10, CheckStatus::Failed));
    assert_eq!(judge(&s, LATER).unwrap().outcome, Outcome::Success);
}

#[test]
fn failed_check_without_completion_is_failure_not_undetermined() {
    let mut s = ended(TaskKind::Coding);
    s.checks.push(check(END - 1, CheckStatus::Failed));
    assert_eq!(judge(&s, LATER).unwrap().outcome, Outcome::Failure);
}

#[test]
fn redo_within_thirty_minutes_is_failure() {
    for at in [END + 1, END + TAU_RE_MS] {
        let mut s = completed(TaskKind::NonCoding);
        s.follow_ups.push(redo(at));
        let j = judge(&s, LATER).unwrap();
        assert_eq!(
            (j.outcome, j.rule),
            (Outcome::Failure, Rule::RedoWithinTauRe)
        );
    }
    // Just outside the window, or at/before the end: not a redo of this task.
    for at in [END + TAU_RE_MS + 1, END, END - 1] {
        let mut s = completed(TaskKind::NonCoding);
        s.follow_ups.push(redo(at));
        assert_eq!(judge(&s, LATER).unwrap().outcome, Outcome::Success, "{at}");
    }
}

#[test]
fn only_user_redo_messages_in_the_same_place_count() {
    let mut new_feature = redo(END + 60_000);
    new_feature.classified_as_redo = false;
    let mut elsewhere = redo(END + 60_000);
    elsewhere.same_session_or_workdir = false;
    let mut from_agent = redo(END + 60_000);
    from_agent.from_user = false;
    for m in [new_feature, elsewhere, from_agent] {
        let mut s = completed(TaskKind::NonCoding);
        s.follow_ups.push(m);
        assert_eq!(judge(&s, LATER).unwrap().outcome, Outcome::Success);
    }
}

#[test]
fn rollback_within_24h_retroactively_fails() {
    let mut s = completed(TaskKind::Coding);
    // First judged shortly after the end: provisional success.
    let early = judge(&s, END + 60_000).unwrap();
    assert_eq!(early.outcome, Outcome::Success);
    assert!(early.provisional);
    assert_eq!(early.final_after_ms, Some(END + TAU_KEEP_MS));

    // The rollback arrives 23 h later; re-judging flips it to failure.
    s.rollbacks.push(rollback(END + 23 * 3_600_000));
    let late = judge(&s, END + 23 * 3_600_000).unwrap();
    assert_eq!(
        (late.outcome, late.rule),
        (Outcome::Failure, Rule::RollbackWithinTauKeep)
    );
    assert!(!late.provisional);
}

#[test]
fn rollback_window_boundaries() {
    let mut at_edge = completed(TaskKind::Coding);
    at_edge.rollbacks.push(rollback(END + TAU_KEEP_MS));
    assert_eq!(judge(&at_edge, LATER).unwrap().outcome, Outcome::Failure);

    let mut past = completed(TaskKind::Coding);
    past.rollbacks.push(rollback(END + TAU_KEEP_MS + 1));
    let j = judge(&past, LATER).unwrap();
    assert_eq!(j.outcome, Outcome::Success);
    assert!(!j.provisional);

    let mut partial = completed(TaskKind::Coding);
    partial.rollbacks.push(Rollback {
        at_ms: END + 1,
        whole_change: false,
    });
    assert_eq!(judge(&partial, LATER).unwrap().outcome, Outcome::Success);
}

#[test]
fn rollback_does_not_apply_to_non_coding_tasks() {
    let mut s = completed(TaskKind::NonCoding);
    s.rollbacks.push(rollback(END + 60_000));
    assert_eq!(judge(&s, LATER).unwrap().outcome, Outcome::Success);
    // Non-coding success only waits for the 30-minute redo window.
    let j = judge(&completed(TaskKind::NonCoding), END + 60_000).unwrap();
    assert_eq!(j.final_after_ms, Some(END + TAU_RE_MS));
    assert!(
        !judge(&completed(TaskKind::NonCoding), END + TAU_RE_MS)
            .unwrap()
            .provisional
    );
}

#[test]
fn undetermined_task_can_still_be_failed_by_a_rollback() {
    let mut s = ended(TaskKind::Coding);
    assert!(judge(&s, END + 1).unwrap().provisional);
    s.rollbacks.push(rollback(END + 1_000));
    assert_eq!(judge(&s, LATER).unwrap().outcome, Outcome::Failure);
}

#[test]
fn events_after_the_watermark_are_rejected() {
    let mut s = completed(TaskKind::Coding);
    s.rollbacks.push(rollback(END + 10));
    assert!(judge(&s, END + 5).is_err());
    assert!(judge(&completed(TaskKind::Coding), END - 1).is_err());
}

#[test]
fn window_overflow_is_an_error_not_a_panic() {
    let mut s = completed(TaskKind::Coding);
    s.ended_ms = Some(i64::MAX - 1);
    assert!(judge(&s, i64::MAX).is_err());
}

// ---- quality signals ---------------------------------------------------

fn arm(name: &str) -> Arm {
    Arm::new(
        ModelId::new(name).unwrap(),
        Route::new(
            ProviderKind::Official,
            "api.example.com",
            Protocol::AnthropicMessages,
            "example-cli",
        )
        .unwrap(),
    )
}

fn cell(name: &str) -> CellId {
    CellId::user(name).unwrap()
}

fn rate(a: &Arm, c: &CellId, stars: &[u8]) -> Vec<Rating> {
    stars
        .iter()
        .map(|&s| Rating {
            arm: a.clone(),
            cell: c.clone(),
            outcome: Outcome::Success,
            stars: s,
        })
        .collect()
}

#[test]
fn stars_map_to_quarters() {
    for (s, q) in [(1, 0.0), (2, 0.25), (3, 0.5), (4, 0.75), (5, 1.0)] {
        close(stars_to_quality(s).unwrap(), q);
    }
    assert!(stars_to_quality(0).is_err());
    assert!(stars_to_quality(6).is_err());
}

#[test]
fn shrinkage_matches_hand_values() {
    // (1 + 0.75 + 0.5 + 0.5*0.5) / 3.5 = 2.5 / 3.5
    close(shrunk_quality(&[1.0, 0.75, 0.5], 0.5).unwrap(), 2.5 / 3.5);
    close(
        shrunk_quality(&[1.0, 0.75, 0.5], 0.5).unwrap(),
        0.714_285_714_285_714_3,
    );
    // q_B = mean(0.75, 0.75, 1.0) = 5/6; (2.25 + 5/12) / 3.5 = 16/21
    close(
        shrunk_quality(&[1.0, 0.75, 0.5], 5.0 / 6.0).unwrap(),
        0.761_904_761_904_761_9,
    );
    // No ratings: q_hat = q_B.
    close(shrunk_quality(&[], 0.5).unwrap(), 0.5);
    // Clipped to [0.01, 0.99].
    close(shrunk_quality(&[1.0, 1.0, 1.0], 1.0).unwrap(), 0.99);
    close(shrunk_quality(&[0.0, 0.0, 0.0], 0.0).unwrap(), 0.01);
    assert!(shrunk_quality(&[1.5], 0.5).is_err());
    assert!(shrunk_quality(&[f64::NAN], 0.5).is_err());
}

#[test]
fn pooled_prior_uses_all_arms_and_defaults_to_half() {
    let (x, y, fix, add) = (arm("model-x"), arm("model-y"), cell("fix"), cell("add"));
    let mut prev = rate(&x, &fix, &[4, 4]);
    prev.extend(rate(&y, &fix, &[5]));
    prev.extend(rate(&x, &add, &[1]));
    close(pooled_prior(&prev, &fix).unwrap(), 5.0 / 6.0);
    close(pooled_prior(&prev, &cell("refactor")).unwrap(), 0.5);
    close(pooled_prior(&[], &fix).unwrap(), 0.5);
}

#[test]
fn ratings_count_only_on_successful_tasks() {
    let (x, fix) = (arm("model-x"), cell("fix"));
    let mut prev = rate(&x, &fix, &[5]);
    let mut flipped = rate(&x, &fix, &[1]);
    flipped[0].outcome = Outcome::Failure;
    prev.extend(flipped);
    close(pooled_prior(&prev, &fix).unwrap(), 1.0);
}

#[test]
fn insufficient_ratings_switch_whole_layer_to_success_rate_mode() {
    let (x, y, r) = (arm("model-x"), arm("model-y"), arm("model-ref"));
    let (fix, add) = (cell("fix"), cell("add"));
    let cells = CellWeights::equal(&[fix.clone(), add.clone()]).unwrap();
    let candidates = vec![x.clone(), y.clone(), r.clone()];

    let mut window = Vec::new();
    for a in [&x, &y, &r] {
        window.extend(rate(a, &fix, &[5, 4, 3]));
        window.extend(rate(a, &add, &[4, 4, 4]));
    }
    assert_eq!(
        quality_mode(&candidates, &r, &cells, &window).unwrap(),
        QualityMode::Rating
    );
    close(
        q_hat(QualityMode::Rating, &x, &fix, &window, &[]).unwrap(),
        2.5 / 3.5,
    );

    // Drop one rating of arm y on cell "add": y has 2 < 3 there.
    let pos = window
        .iter()
        .position(|g| g.arm == y && g.cell == add)
        .unwrap();
    window.remove(pos);
    let mode = quality_mode(&candidates, &r, &cells, &window).unwrap();
    assert_eq!(mode, QualityMode::SuccessRate);
    assert_eq!(mode.label(), Some(SUCCESS_RATE_MODE_LABEL));
    // Every arm and the reference, on every cell, use 0.99 - including
    // x, which had enough ratings.
    for a in [&x, &y, &r] {
        for c in [&fix, &add] {
            assert_eq!(q_hat(mode, a, c, &window, &[]).unwrap(), Q_HAT_MAX);
        }
    }
}

#[test]
fn reference_arm_is_part_of_the_rating_gate() {
    let (x, r, fix) = (arm("model-x"), arm("model-ref"), cell("fix"));
    let cells = CellWeights::equal(std::slice::from_ref(&fix)).unwrap();
    let mut window = rate(&x, &fix, &[5, 5, 5]);
    window.extend(rate(&r, &fix, &[3, 3]));
    assert_eq!(
        quality_mode(std::slice::from_ref(&x), &r, &cells, &window).unwrap(),
        QualityMode::SuccessRate
    );
    window.extend(rate(&r, &fix, &[3]));
    assert_eq!(
        quality_mode(std::slice::from_ref(&x), &r, &cells, &window).unwrap(),
        QualityMode::Rating
    );
}

#[test]
fn zero_weight_cells_and_failed_ratings_do_not_count_toward_the_gate() {
    let (x, fix, add) = (arm("model-x"), cell("fix"), cell("add"));
    let cells = CellWeights::new(vec![(fix.clone(), 1.0), (add.clone(), 0.0)]).unwrap();
    // "add" has zero weight: it is not in d+ and needs no ratings.
    let window = rate(&x, &fix, &[4, 4, 4]);
    assert_eq!(
        quality_mode(std::slice::from_ref(&x), &x, &cells, &window).unwrap(),
        QualityMode::Rating
    );
    // A rating on a task that later failed does not count.
    let mut window = rate(&x, &fix, &[4, 4, 4]);
    window[2].outcome = Outcome::Failure;
    assert_eq!(
        quality_mode(std::slice::from_ref(&x), &x, &cells, &window).unwrap(),
        QualityMode::SuccessRate
    );
    // No candidates at all: success-rate mode.
    assert_eq!(
        quality_mode(&[], &x, &cells, &window).unwrap(),
        QualityMode::SuccessRate
    );
}

#[test]
fn spot_rating_rate_and_daily_cap() {
    assert!(should_ask_spot_rating(Outcome::Success, 0.0, 0).unwrap());
    assert!(should_ask_spot_rating(Outcome::Success, 0.099_999, 2).unwrap());
    assert!(!should_ask_spot_rating(Outcome::Success, 0.10, 0).unwrap());
    assert!(!should_ask_spot_rating(Outcome::Success, 0.05, 3).unwrap());
    for o in [
        Outcome::Failure,
        Outcome::Cancel,
        Outcome::Undetermined,
        Outcome::Pending,
    ] {
        assert!(!should_ask_spot_rating(o, 0.0, 0).unwrap());
    }
    assert!(should_ask_spot_rating(Outcome::Success, 1.0, 0).is_err());
    assert!(should_ask_spot_rating(Outcome::Success, f64::NAN, 0).is_err());
}

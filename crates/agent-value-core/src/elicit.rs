//! Five-level self-report (§10.2): paired questions, answer levels,
//! weight regions, adaptive question choice and the stop rule.
//!
//! Numerics follow the reference script `elicit5.py` exactly so the two
//! can be compared point by point (DECISIONS G1–G5).

use crate::constants::{
    C0_YUAN, EPSILON, SELF_REPORT_DELTA1, SELF_REPORT_DELTA2, SELF_REPORT_MAX_QUESTIONS,
    SELF_REPORT_MAX_REGION_POINTS, SELF_REPORT_MIN_QUESTIONS, SELF_REPORT_REGION_SLACK_LEVELS,
    T0_SECONDS,
};
use crate::grid::{grid, nearest_to_centroid, GridPoint};
use crate::weights::FiveLevel;
use crate::{CoreError, Result};

/// Ratio multipliers used by the question bank.
pub const BANK_RATIOS: [f64; 4] = [1.3, 1.5, 2.0, 3.0];
/// Number of questions in the bank: 3 dimension pairs × 16 ratio pairs × 2
/// directions.
pub const BANK_SIZE: usize = 96;

/// One paired question: `rho = (rho_C, rho_T, rho_D)`, each option A over
/// option B. Exactly two entries differ from 1 and pull in opposite
/// directions; the third is exactly 1.
///
/// Questions only come from [`bank`], so `Delta > 0` and every ratio is
/// positive and finite.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Question {
    rho: [f64; 3],
}

impl Question {
    /// `(rho_C, rho_T, rho_D)`.
    pub fn rho(&self) -> [f64; 3] {
        self.rho
    }
}

/// The 96-question bank in the reference order (DECISIONS G2): pairs
/// (C,T), (C,D), (T,D); `x`, `y` over [`BANK_RATIOS`]; for each, first A is
/// `x` times worse in the first dimension and `y` times better in the
/// second (`1.0 / y`), then the reverse. Ratios are built literally as
/// `1.0 / x`, not rewritten (DECISIONS G1).
pub fn bank() -> Vec<Question> {
    let mut out = Vec::with_capacity(BANK_SIZE);
    for (i, j) in [(0usize, 1usize), (0, 2), (1, 2)] {
        for x in BANK_RATIOS {
            for y in BANK_RATIOS {
                for flip in [false, true] {
                    let mut rho = [1.0f64; 3];
                    if flip {
                        rho[i] = 1.0 / x;
                        rho[j] = y;
                    } else {
                        rho[i] = x;
                        rho[j] = 1.0 / y;
                    }
                    out.push(Question { rho });
                }
            }
        }
    }
    out
}

/// Question scale `Delta = (|ln rho_C| + |ln rho_T| + |ln rho_D|) / 2`.
pub fn scale(q: &Question) -> f64 {
    q.rho.iter().map(|r| r.ln().abs()).sum::<f64>() / 2.0
}

/// Rounds to 9 decimals, as the reference script does before banding
/// (DECISIONS G1).
fn round9(x: f64) -> f64 {
    (x * 1e9).round() / 1e9
}

fn u_raw(w: GridPoint, q: &Question) -> f64 {
    let s: f64 = w
        .as_array()
        .iter()
        .zip(q.rho.iter())
        .map(|(wk, r)| wk * r.ln())
        .sum();
    -s / scale(q)
}

/// `u(w) = -s(w) / Delta`, rounded to 9 decimals; positive means A is better.
pub fn u_of(w: GridPoint, q: &Question) -> f64 {
    round9(u_raw(w, q))
}

/// Five-level band of an unrounded `u` (§10.2, DECISIONS G1): round to 9
/// decimals, then closed bands `|u| <= delta_1` → 0, `|u| <= delta_2` → ±1,
/// otherwise ±2.
pub fn band(u: f64) -> i8 {
    let u = round9(u);
    let sign = if u > 0.0 { 1 } else { -1 };
    if u.abs() <= SELF_REPORT_DELTA1 {
        0
    } else if u.abs() <= SELF_REPORT_DELTA2 {
        sign
    } else {
        2 * sign
    }
}

/// Level predicted by `w` for question `q`.
pub fn level(w: GridPoint, q: &Question) -> i8 {
    band(u_raw(w, q))
}

/// One answered question: bank index and the five-level answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Answer {
    pub question: usize,
    pub answer: FiveLevel,
}

fn check_answers(answers: &[Answer]) -> Result<()> {
    for (n, a) in answers.iter().enumerate() {
        if a.question >= BANK_SIZE {
            return Err(CoreError::OutOfRange {
                what: "question index",
                value: a.question as f64,
                min: 0.0,
                max: (BANK_SIZE - 1) as f64,
            });
        }
        if answers[..n].iter().any(|b| b.question == a.question) {
            return Err(CoreError::InvalidField {
                field: "answers",
                reason: "a question is answered twice",
            });
        }
    }
    Ok(())
}

/// Total level deviation `sum_j |l(w; q_j) - l_j|` for one point.
fn deviation(w: GridPoint, answers: &[Answer], bank: &[Question]) -> u32 {
    answers
        .iter()
        .map(|a| u32::from((level(w, &bank[a.question]) - a.answer.level()).unsigned_abs()))
        .sum()
}

/// `Omega_core` (deviation equal to the minimum `m`) and `Omega_s`
/// (deviation at most `m + 1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Regions {
    pub core: Vec<GridPoint>,
    pub wide: Vec<GridPoint>,
    pub m: u32,
}

/// Regions for a set of answers. With no answers every point has
/// deviation 0, so both regions are the whole grid.
pub fn regions(answers: &[Answer]) -> Result<Regions> {
    check_answers(answers)?;
    let bank = bank();
    let devs: Vec<(GridPoint, u32)> = grid()
        .into_iter()
        .map(|w| (w, deviation(w, answers, &bank)))
        .collect();
    let m = devs.iter().map(|(_, d)| *d).min().unwrap_or(0);
    let core = devs
        .iter()
        .filter(|(_, d)| *d == m)
        .map(|(w, _)| *w)
        .collect();
    let wide = devs
        .iter()
        .filter(|(_, d)| *d <= m + SELF_REPORT_REGION_SLACK_LEVELS)
        .map(|(w, _)| *w)
        .collect();
    Ok(Regions { core, wide, m })
}

/// Next question: the unanswered one that splits `wide` most evenly over
/// the five levels (smallest `sum n_l^2`); ties go to the earliest in bank
/// order (DECISIONS G2). `None` when every question is answered.
pub fn pick(wide: &[GridPoint], asked: &[usize]) -> Option<usize> {
    let bank = bank();
    let mut best: Option<(usize, usize)> = None;
    for (idx, q) in bank.iter().enumerate() {
        if asked.contains(&idx) {
            continue;
        }
        let mut counts = [0usize; 5];
        for w in wide {
            counts[(level(*w, q) + 2) as usize] += 1;
        }
        let score: usize = counts.iter().map(|c| c * c).sum();
        if best.is_none_or(|(s, _)| score < s) {
            best = Some((score, idx));
        }
    }
    best.map(|(_, idx)| idx)
}

/// Stop rule (§10.2): at least 4 answered and `|Omega_s| <= 15`, or 10
/// answered.
pub fn should_stop(answered: usize, wide_len: usize) -> bool {
    answered >= SELF_REPORT_MAX_QUESTIONS
        || (answered >= SELF_REPORT_MIN_QUESTIONS && wide_len <= SELF_REPORT_MAX_REGION_POINTS)
}

/// An adaptive self-report session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    answers: Vec<Answer>,
    skipped: Vec<usize>,
    wide: Vec<GridPoint>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        Self {
            answers: Vec::new(),
            skipped: Vec::new(),
            wide: grid(),
        }
    }

    pub fn answers(&self) -> &[Answer] {
        &self.answers
    }

    /// Current `Omega_s`.
    pub fn wide(&self) -> &[GridPoint] {
        &self.wide
    }

    pub fn finished(&self) -> bool {
        should_stop(self.answers.len(), self.wide.len())
    }

    /// The question to ask next; `None` once the stop rule holds.
    pub fn next_question(&self) -> Option<usize> {
        if self.finished() {
            return None;
        }
        let mut asked: Vec<usize> = self.answers.iter().map(|a| a.question).collect();
        asked.extend(&self.skipped);
        pick(&self.wide, &asked)
    }

    /// Questions skipped because they cannot be shown ([`show`] gave
    /// `None`, DECISIONS J).
    pub fn skipped(&self) -> &[usize] {
        &self.skipped
    }

    /// Skips the question returned by [`Session::next_question`]; it is not
    /// asked again and does not count as answered.
    pub fn skip(&mut self, question: usize) -> Result<()> {
        if self.next_question() != Some(question) {
            return Err(CoreError::InvalidField {
                field: "question",
                reason: "not the question this session asks next",
            });
        }
        self.skipped.push(question);
        Ok(())
    }

    /// Records an answer to the question returned by
    /// [`Session::next_question`].
    pub fn answer(&mut self, question: usize, answer: FiveLevel) -> Result<()> {
        if self.next_question() != Some(question) {
            return Err(CoreError::InvalidField {
                field: "question",
                reason: "not the question this session asks next",
            });
        }
        self.answers.push(Answer { question, answer });
        self.wide = regions(&self.answers)?.wide;
        Ok(())
    }
}

/// Self-report result (§10.2).
#[derive(Debug, Clone, PartialEq)]
pub struct SelfReport {
    pub regions: Regions,
    /// Per-dimension `[min, max]` of `Omega_s` for `a`, `b`, `g`.
    pub ranges: [(f64, f64); 3],
    /// `w_s`: centroid of `Omega_core` moved to the nearest grid point
    /// (DECISIONS G3). Default value only.
    pub point: GridPoint,
    /// `m >= 2`: "answers contradict each other".
    pub contradiction: bool,
    /// Bank indices of questions off by at least 2 levels at some point of
    /// `Omega_core` (DECISIONS G4). May be empty when `contradiction`.
    pub contradicted: Vec<usize>,
}

/// Builds the self-report result from the answers.
pub fn self_report(answers: &[Answer]) -> Result<SelfReport> {
    if answers.is_empty() {
        return Err(CoreError::EmptyField { field: "answers" });
    }
    let regions = regions(answers)?;
    let bank = bank();
    let mut ranges = [(f64::INFINITY, f64::NEG_INFINITY); 3];
    for w in &regions.wide {
        for (r, v) in ranges.iter_mut().zip(w.as_array()) {
            r.0 = r.0.min(v);
            r.1 = r.1.max(v);
        }
    }
    let point = nearest_to_centroid(&regions.core).ok_or(CoreError::EmptyField {
        field: "core region",
    })?;
    let contradiction = regions.m >= 2;
    let contradicted = if contradiction {
        answers
            .iter()
            .filter(|a| {
                regions
                    .core
                    .iter()
                    .any(|w| (level(*w, &bank[a.question]) - a.answer.level()).unsigned_abs() >= 2)
            })
            .map(|a| a.question)
            .collect()
    } else {
        Vec::new()
    };
    Ok(SelfReport {
        regions,
        ranges,
        point,
        contradiction,
        contradicted,
    })
}

/// Base values for showing a question: the previous window's medians
/// (first window 0.5 yuan and 120 s) and a defect rate `1 - Q`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuestionBase {
    pub cost: f64,
    pub time: f64,
    pub defect: f64,
}

/// First-window base (§10.2): 0.5 yuan, 120 s. The defect base is not
/// fixed by the spec; callers pass their own.
pub const FIRST_WINDOW_COST: f64 = 0.5;
pub const FIRST_WINDOW_TIME: f64 = 120.0;

/// Displayed values of options A and B: `[A, B]` per dimension.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shown {
    pub cost: [f64; 2],
    pub time: [f64; 2],
    pub defect: [f64; 2],
}

/// Turns a question into displayed values (DECISIONS J, G5). The better
/// side shows the base; only the worse side is solved from `rho >= 1`, so
/// every value stays positive. `Ok(None)` when the worse defect rate would
/// exceed 1 (the question cannot be shown and is skipped, not clamped).
pub fn show(q: &Question, base: QuestionBase) -> Result<Option<Shown>> {
    for (what, v) in [
        ("base cost", base.cost),
        ("base time", base.time),
        ("base defect", base.defect),
    ] {
        if !v.is_finite() {
            return Err(CoreError::NotFinite { what, value: v });
        }
        // Cost and time must be positive (DECISIONS J); a defect rate of 0
        // (Q = 1) is valid.
        if v < 0.0 || (v == 0.0 && what != "base defect") {
            return Err(CoreError::OutOfRange {
                what,
                value: v,
                min: 0.0,
                max: f64::INFINITY,
            });
        }
    }
    if base.defect > 1.0 {
        return Err(CoreError::OutOfRange {
            what: "base defect",
            value: base.defect,
            min: 0.0,
            max: 1.0,
        });
    }
    let pair = |rho: f64, b: f64, floor: f64| -> [f64; 2] {
        if rho >= 1.0 {
            [rho * (b + floor) - floor, b]
        } else {
            [b, (b + floor) / rho - floor]
        }
    };
    let shown = Shown {
        cost: pair(q.rho[0], base.cost, C0_YUAN),
        time: pair(q.rho[1], base.time, T0_SECONDS),
        defect: pair(q.rho[2], base.defect, EPSILON),
    };
    if shown.defect.iter().any(|d| *d > 1.0) {
        return Ok(None);
    }
    Ok(Some(shown))
}

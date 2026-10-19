//! Checks every constant against a verbatim copy of the v22 §9 table
//! (`fixtures/v22-section9-table.md`).
//!
//! For each table row we list the constants that implement it. The test
//! extracts every number from the row's value cell and requires it to equal,
//! in order, the numbers implied by our constants. It also requires that
//! every row of the table is covered, so a new row in the spec fails the test
//! until it is implemented.

use agent_value_core::constants::*;

const TABLE: &str = include_str!("fixtures/v22-section9-table.md");

struct Row {
    name: String,
    value: String,
    meaning: String,
}

fn rows() -> Vec<Row> {
    TABLE
        .lines()
        .filter(|l| l.starts_with("| "))
        .skip(1) // header
        .map(|l| {
            let cells: Vec<&str> = l.split('|').map(str::trim).collect();
            Row {
                name: cells[1].to_string(),
                value: cells[2].to_string(),
                meaning: cells.get(3).copied().unwrap_or("").to_string(),
            }
        })
        .collect()
}

/// All ASCII decimal numbers in `s`, in order.
fn numbers(s: &str) -> Vec<f64> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in s.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_digit() || (ch == '.' && !cur.is_empty()) {
            cur.push(ch);
        } else if !cur.is_empty() {
            out.push(cur.trim_end_matches('.').parse().expect("number"));
            cur.clear();
        }
    }
    out
}

/// (table name cell, numbers our constants imply for the value cell).
fn expected() -> Vec<(&'static str, Vec<f64>)> {
    vec![
        ("$c_0$", vec![C0_YUAN]),
        ("$t_0$", vec![T0_SECONDS]),
        ("$\\epsilon$", vec![EPSILON]),
        // "各 1/3": one over three.
        ("$a,b,g$", vec![1.0, 1.0 / DEFAULT_WEIGHT]),
        ("$K$", vec![f64::from(K_MAX_ATTEMPTS)]),
        // "由数据估计" (estimated from data): no number, no constant.
        ("$\\pi$", vec![]),
        ("$\\kappa_C$", vec![KAPPA_C]),
        ("$\\kappa_F$", vec![KAPPA_F]),
        ("$m_0$", vec![M0]),
        ("$\\kappa_p$", vec![KAPPA_P_PER_RHO]),
        ("$\\theta$", vec![THETA]),
        ("有效参照单元门", vec![MIN_VALID_REFERENCE_CELLS as f64]),
        ("取消率硬门", vec![CANCEL_RATE_MAX]),
        ("完成率硬门", vec![COMPLETION_RATE_MIN]),
        (
            "样本门",
            vec![SAMPLE_GATE_FLOOR as f64, SAMPLE_GATE_PER_CELL as f64],
        ),
        (
            "臂级样本门",
            vec![SAMPLE_GATE_FLOOR as f64, SAMPLE_GATE_PER_CELL as f64],
        ),
        ("$a,b,g$ 下限", vec![WEIGHT_FLOOR]),
        ("时间窗", vec![f64::from(WINDOW_DAYS)]),
        ("$\\tau_{\\rm re}$", vec![f64::from(TAU_RE_MINUTES)]),
        ("$\\tau_{\\rm keep}$", vec![f64::from(TAU_KEEP_HOURS)]),
        (
            "抽样评分率",
            vec![SPOT_RATING_RATE * 100.0, f64::from(SPOT_RATING_MAX_PER_DAY)],
        ),
        ("评分模式门槛", vec![f64::from(RATING_MODE_MIN_RATINGS)]),
        (
            "自报五档切点 $\\delta_1,\\delta_2$",
            vec![SELF_REPORT_DELTA1, SELF_REPORT_DELTA2],
        ),
        (
            "自报停止条件",
            vec![
                SELF_REPORT_MIN_QUESTIONS as f64,
                SELF_REPORT_MAX_REGION_POINTS as f64,
                SELF_REPORT_MAX_QUESTIONS as f64,
            ],
        ),
        ("$n_{\\rm ev}$", vec![N_EV as f64]),
        (
            "行为无信号门",
            vec![BEHAVIOR_MIN_LAMBDA, BEHAVIOR_MIN_RANK as f64],
        ),
        ("网格步长", vec![GRID_STEP]),
        (
            "确定性类通过",
            vec![
                f64::from(PROBE_QUESTIONS_PER_RUN),
                f64::from(DETERMINISTIC_PASS_MIN),
            ],
        ),
        (
            "指纹类通过",
            vec![
                f64::from(PROBE_QUESTIONS_PER_RUN),
                f64::from(FINGERPRINT_PASS_MIN),
            ],
        ),
        ("真伪门统计单位", vec![]),
        ("返回模型版本漂移上限", vec![MODEL_VERSION_DRIFT_MAX]),
        ("日志可解析率下限", vec![LOG_PARSE_RATE_MIN]),
        ("探针月预算", vec![PROBE_MONTHLY_BUDGET_YUAN]),
        ("统计稳健重抽样次数", vec![f64::from(BOOTSTRAP_RESAMPLES)]),
        ("漂移提示门", vec![DRIFT_TV_THRESHOLD]),
    ]
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-12 * a.abs().max(b.abs()).max(1.0)
}

#[test]
fn every_section9_row_is_covered_in_order() {
    let names: Vec<String> = rows().into_iter().map(|r| r.name).collect();
    let want: Vec<&str> = expected().into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, want, "§9 table rows changed; update constants.rs");
    assert_eq!(names.len(), 35);
}

#[test]
fn constants_equal_section9_values() {
    for (row, (name, want)) in rows().iter().zip(expected()) {
        let got = numbers(&row.value);
        assert_eq!(
            got.len(),
            want.len(),
            "row {name}: value cell {:?}",
            row.value
        );
        for (g, w) in got.iter().zip(&want) {
            assert!(close(*g, *w), "row {name}: table {g} != constant {w}");
        }
    }
}

#[test]
fn non_numeric_rows_match_their_text() {
    let rows = rows();
    let find = |n: &str| rows.iter().find(|r| r.name == n).expect(n);
    assert_eq!(find("$\\pi$").value, "由数据估计");
    assert_eq!(find("$\\kappa_p$").value, "$20\\rho$");
    assert_eq!(find("$a,b,g$").value, "各 $1/3$");
    assert_eq!(find("样本门").value, "$\\max(30,3U_d)$");
    assert_eq!(find("臂级样本门").value, "$\\max(30,3U_i)$");
    assert_eq!(find("真伪门统计单位").value, "运行日（北京时间自然日）");
    // Beijing time is UTC+8.
    assert_eq!(RUN_DAY_UTC_OFFSET_SECONDS, 8 * 3600);
    // "共 171 点" lives in the meaning column of the grid-step row.
    assert!(find("网格步长").meaning.contains("171"));
    assert_eq!(GRID_POINTS, 171);
    // K row: "最坏总 2K=6 次".
    assert!(find("$K$").meaning.contains("2K=6"));
    assert_eq!(WORST_CASE_TOTAL_ATTEMPTS, 6);
    // Stop-condition row: "Ω_s 容许一题偏一档".
    assert!(find("自报停止条件").value.contains("容许一题偏一档"));
    assert_eq!(SELF_REPORT_REGION_SLACK_LEVELS, 1);
}

#[test]
fn literal_values() {
    // Same values written out, independent of the fixture.
    assert_eq!(C0_YUAN, 0.15);
    assert_eq!(T0_SECONDS, 15.0);
    assert_eq!(EPSILON, 0.02);
    assert_eq!(DEFAULT_WEIGHT, 1.0 / 3.0);
    assert_eq!(K_MAX_ATTEMPTS, 3);
    assert_eq!(KAPPA_C, 5.0);
    assert_eq!(KAPPA_F, 5.0);
    assert_eq!(M0, 0.5);
    assert_eq!(kappa_p(1.0), Ok(20.0));
    assert_eq!(THETA, 0.10);
    assert_eq!(MIN_VALID_REFERENCE_CELLS, 3);
    assert_eq!(CANCEL_RATE_MAX, 0.20);
    assert_eq!(COMPLETION_RATE_MIN, 0.50);
    assert_eq!(layer_sample_gate(4), 30);
    assert_eq!(arm_sample_gate(20), 60);
    assert_eq!(WEIGHT_FLOOR, 0.05);
    assert_eq!(WINDOW_DAYS, 30);
    assert_eq!(TAU_RE_MINUTES, 30);
    assert_eq!(TAU_KEEP_HOURS, 24);
    assert_eq!(SPOT_RATING_RATE, 0.10);
    assert_eq!(SPOT_RATING_MAX_PER_DAY, 3);
    assert_eq!(RATING_MODE_MIN_RATINGS, 3);
    assert_eq!((SELF_REPORT_DELTA1, SELF_REPORT_DELTA2), (0.25, 0.50));
    assert_eq!(
        (
            SELF_REPORT_MIN_QUESTIONS,
            SELF_REPORT_MAX_REGION_POINTS,
            SELF_REPORT_MAX_QUESTIONS
        ),
        (4, 15, 10)
    );
    assert_eq!(N_EV, 20);
    assert_eq!((BEHAVIOR_MIN_LAMBDA, BEHAVIOR_MIN_RANK), (0.5, 3));
    assert_eq!(GRID_STEP, 0.05);
    assert_eq!(
        (
            PROBE_QUESTIONS_PER_RUN,
            DETERMINISTIC_PASS_MIN,
            FINGERPRINT_PASS_MIN
        ),
        (10, 9, 8)
    );
    assert_eq!(MODEL_VERSION_DRIFT_MAX, 0.05);
    assert_eq!(LOG_PARSE_RATE_MIN, 0.95);
    assert_eq!(PROBE_MONTHLY_BUDGET_YUAN, 10.0);
    assert_eq!(BOOTSTRAP_RESAMPLES, 200);
    assert_eq!(DRIFT_TV_THRESHOLD, 0.3);
}

#[test]
fn grid_of_171_points_follows_from_step_and_floor() {
    // a, b, g in {0.05, ..., 0.90}, a + b + g = 1: count in integer units of the step.
    let units = (1.0 / GRID_STEP).round() as i32; // 20
    let floor = (WEIGHT_FLOOR / GRID_STEP).round() as i32; // 1
    let mut n = 0;
    for a in floor..=units {
        for b in floor..=units {
            let g = units - a - b;
            if g >= floor {
                n += 1;
            }
        }
    }
    assert_eq!(n, GRID_POINTS);
}

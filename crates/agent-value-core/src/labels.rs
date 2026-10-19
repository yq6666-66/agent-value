//! Fixed display strings for headers, legends and tags (v22 §2, §6, §7;
//! DECISIONS H, W). There is no UI yet; result types carry flags and these
//! strings are what a display layer shows for them.

/// §2: arm whose cancel bill is only a lower bound (robust dominance off).
pub const COARSE_BILL: &str = "账单过粗";
/// §6: `F_rob` listed when `F_rec` is empty; never the official #1.
pub const ROBUST_ALTERNATIVE: &str = "取消稳健备选";
/// §6 line 471: overall row with `cov_i < 1`.
pub fn partial_coverage(cov: f64) -> String {
    format!("非全覆盖（{cov:.4}），仅供诊断，不进前沿、不进推荐、不参与排序")
}
/// §6: overall reference includes a tier II layer.
pub const OVERALL_INCLUDES_TIER_II: &str = "含 Ⅱ 档夹逼层";
/// §6: some positive-weight layer is tier III.
pub const OVERALL_REFERENCE_UNCERTIFIED: &str =
    "部分正权重层参照未认证，仅展示各臂总体 (C*,T*,Q*) 与各层独立前沿";
/// §7 tier III header fixed text for reasons without a weight.
pub const TIER_III_NA: &str = "参照组该层存在无法定价的单元（NA）";
pub const TIER_III_FEW_VALID: &str = "参照组有效单元不足 3 个";
/// §7 tier II legend: required wording.
pub const TIER_II_LEGEND: &str = "Ⅱ 档下不存在“100 分 = 与参照组持平”这个基准点";
/// DECISIONS H legend.
pub const TERMINAL_ARMS_LEGEND: &str = "参照臂与兜底臂自身不计兜底开销";

use crate::anchor::{LayerAnchor, TierIIIReason};
use crate::constants::THETA;

fn w4(x: f64) -> String {
    format!("{x:.4}")
}

/// §7 tier III header for one reason (lines 497–500); weights are printed
/// with 4 decimals.
pub fn tier_iii_header(r: TierIIIReason, anchor: &LayerAnchor) -> String {
    match r {
        TierIIIReason::Miss => format!("参照组缺单元（{}）", w4(anchor.w_miss)),
        TierIIIReason::Bad => format!(
            "参照组存在真不合格单元（{}），需补测或更换参照",
            w4(anchor.w_bad)
        ),
        TierIIIReason::Na => TIER_III_NA.to_string(),
        TierIIIReason::FewValid => TIER_III_FEW_VALID.to_string(),
    }
}

/// §7 tier II header (line 533).
pub fn tier_ii_header(anchor: &LayerAnchor) -> String {
    format!(
        "参照组含样本不足单元（{}），锚点已夹逼（θ={:.2}）",
        w4(anchor.w_thin),
        THETA
    )
}

//! 容差体系（设计文档 5.2）—— 数值稳定的命门。
//!
//! **规矩**：容差**集中定义在这里**，不许各调用点各写各的。
//!
//! - **过松**：把本不相交的两个面判成相交，布尔结果出错；
//! - **过紧**：退化情形（相切、共面）下布尔直接失败。
//!
//! ## 规范名与别名（M3 对齐设计文档 §5.2）
//!
//! 设计文档 §5.2 的代码块给出规范名 [`TOL_POINT`] / [`TOL_FACE`] / [`TOL_ANGLE`]。
//! 本模块以它们为**规范名**，同时把历史名 [`POINT_COINCIDENT`] /
//! [`FACE_COINCIDENT`] / [`SELF_INTERSECTION`] 保留为 **`pub const` 别名**（值相同）。
//! 保留别名而不是直接改名：既有调用点（`core-interop/src/flow.rs` 等）零改动，
//! 且避免一次"纯改名 PR"污染里程碑 diff。
//!
//! ## ⚠ 角度容差独立于长度容差区间
//!
//! 设计文档 §5.2 正文说"点重合、面贴合、自交判定统一用 **1e-6 ~ 1e-8**"，
//! 但紧接着的代码块给出 `TOL_ANGLE = 1e-9`——**1e-9 落在所述区间之外**。
//! 判定：正文那句讲的是**长度**容差；角度容差是另一码事
//! （`1e-9` rad ≈ `5.7e-8` 度，对角度/参数比较合理）。故 [`TOL_ANGLE`] **不在**
//! `1e-8 ~ 1e-6` 区间内，这是**有意为之**，不是笔误。

/// 点重合判定容差（单位：mm）。规范名（设计文档 §5.2）。
pub const TOL_POINT: f64 = 1e-7;

/// 面贴合判定容差（单位：mm）。规范名（设计文档 §5.2）。
pub const TOL_FACE: f64 = 1e-6;

/// 角度判定容差（单位：rad）。规范名（设计文档 §5.2）。
///
/// ⚠ **独立于长度容差区间**：它比 [`TOL_SELF_INTERSECTION`] 更紧，见模块头注释。
pub const TOL_ANGLE: f64 = 1e-9;

/// 自交判定容差（单位：mm）。规范名（设计文档 §5.2 的 "1e-6~1e-8"）。
pub const TOL_SELF_INTERSECTION: f64 = 1e-8;

// ---- 历史名保留为别名（值指向规范名；既有调用点零改动，语义不变）----

/// 点重合判定容差（单位：mm）。**[`TOL_POINT`] 的别名**。
pub const POINT_COINCIDENT: f64 = TOL_POINT;

/// 面贴合判定容差（单位：mm）。**[`TOL_FACE`] 的别名**。
pub const FACE_COINCIDENT: f64 = TOL_FACE;

/// 自交判定容差（单位：mm）。**[`TOL_SELF_INTERSECTION`] 的别名**。
pub const SELF_INTERSECTION: f64 = TOL_SELF_INTERSECTION;

/// 网格化的默认线性挠度（单位：mm）。越小越精细、越慢。
pub const DEFAULT_LINEAR_DEFLECTION: f64 = 0.1;

/// 网格化的默认角度挠度（单位：弧度）。
pub const DEFAULT_ANGULAR_DEFLECTION: f64 = 0.5;

/// 判定"数值上是否为零"。
#[must_use]
pub fn is_zero(v: f64) -> bool {
    v.abs() < SELF_INTERSECTION
}

/// 判定两点是否重合。
#[must_use]
pub fn points_coincident(a: [f64; 3], b: [f64; 3]) -> bool {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    (dx * dx + dy * dy + dz * dz).sqrt() < TOL_POINT
}

/// 判定角度（rad）是否为零。
///
/// 类比 [`is_zero`]（长度）：角度用独立的 [`TOL_ANGLE`] 容差。
#[must_use]
pub fn is_zero_angle(a: f64) -> bool {
    a.abs() < TOL_ANGLE
}

/// 判定两角度（rad）是否在容差内相等（**按 `2π` 周期比较**）。
///
/// `% TAU` + `min(d, TAU - d)` 把差值归到 `[0, π]`，从而正确处理 `2π` 周期与
/// 负差。对 `TOL_ANGLE = 1e-9` 这种极小容差，wrap 多数时候不起作用，但
/// **参数化曲线角度可能跨 `0`/`2π` 比较**，写上更稳，且能防"两角差 `2π` 却被
/// 判不等"的隐性 bug。
#[must_use]
pub fn angles_coincident(a: f64, b: f64) -> bool {
    let d = (a - b).abs() % std::f64::consts::TAU;
    let d = d.min(std::f64::consts::TAU - d); // 归到 [0, π]
    d < TOL_ANGLE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_detection_matches_expectation() {
        assert!(is_zero(0.0));
        assert!(is_zero(1e-9));
        assert!(!is_zero(1e-6));
    }

    #[test]
    fn coincidence_is_at_millimetre_scale() {
        assert!(points_coincident([0.0, 0.0, 0.0], [0.0, 0.0, 1e-9]));
        assert!(!points_coincident([0.0, 0.0, 0.0], [0.0, 0.0, 1e-3]));
    }

    /// 把设计意图钉成断言：**角度最紧 < 自交 ≤ 点 < 面**（面最松）。
    ///
    /// 断言的是**常量之间的序关系**——这正是本钉子想守住的东西（万一有人改值
    /// 改坏了序，这里立刻报红）。故显式允许 `assertions_on_constants`。
    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn tolerance_ordering_matches_the_design_intent() {
        assert!(
            TOL_ANGLE < TOL_SELF_INTERSECTION
                && TOL_SELF_INTERSECTION <= TOL_POINT
                && TOL_POINT < TOL_FACE,
            "序关系应为 TOL_ANGLE < TOL_SELF_INTERSECTION <= TOL_POINT < TOL_FACE，\
             实得 {TOL_ANGLE} / {TOL_SELF_INTERSECTION} / {TOL_POINT} / {TOL_FACE}"
        );
        // 具体值也与设计文档 §5.2 代码块一致
        assert_eq!(TOL_POINT, 1e-7);
        assert_eq!(TOL_FACE, 1e-6);
        assert_eq!(TOL_ANGLE, 1e-9);
        assert_eq!(TOL_SELF_INTERSECTION, 1e-8);
    }

    /// 旧名是别名：**值必须与规范名逐个相等**（不是"差不多"）。
    #[test]
    fn legacy_names_are_exact_aliases_of_the_canonical_names() {
        assert_eq!(POINT_COINCIDENT, TOL_POINT);
        assert_eq!(FACE_COINCIDENT, TOL_FACE);
        assert_eq!(SELF_INTERSECTION, TOL_SELF_INTERSECTION);
    }

    #[test]
    fn zero_angle_detection_uses_the_angle_tolerance() {
        assert!(is_zero_angle(0.0));
        assert!(is_zero_angle(1e-10));
        assert!(!is_zero_angle(1e-6));
    }

    /// `angles_coincident` 必须处理 `2π` 周期（wrap 钉子）。
    #[test]
    fn angle_coincidence_handles_wraparound() {
        assert!(angles_coincident(0.0, 1e-10), "极小差应判等");
        assert!(!angles_coincident(0.0, 1e-3), "明显差应判不等");
        // 0 与 2π 是同一个角
        assert!(
            angles_coincident(0.0, std::f64::consts::TAU),
            "0 与 2π 应判等（按周期比较）"
        );
    }
}

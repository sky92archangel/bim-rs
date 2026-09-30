//! **国标线型段长表：单一权威来源**（GB/T 4457.4-2002 / GB/T 17450-1998）。
//!
//! ## 为什么要有这个模块
//!
//! 在此之前，DXF 与 SVG 两条出口各写一份"拍脑袋的固定段长"，既不符合国标，
//! 又容易走散（改了一处忘另一处）。这里把国标线型定义收成**一份表**，
//! 两个出口都从它派生。
//!
//! ## 段长以 `d` 为单位
//!
//! GB 线型的段长以**粗线基准宽 `d`** 为计量单位（[`GB_BASE_WIDTH_MM`] = 0.5 mm，
//! GB 优先值）。段长系数里 **正数 = 画、负数 = 间隔**（与 DXF 组码 49 的符号语义一致）。
//!
//! ⚠ **段长按 `d` 算，与线条自身线宽无关**：例如细虚线的线宽是 `d/2`，
//! 但它的段长仍按 `d` 计（`12d` 画 / `3d` 间隔）。
//!
//! ## 取值依据（如实写明争议）
//!
//! | 线型 | 段长（以 d 为单位） | 依据 |
//! |---|---|---|
//! | [`Continuous`] 实线 | 无段长 | GB/T 4457.4 No.01 |
//! | [`Dashed`] 细虚线 | `12d` / `3d` | GB/T 4457.4 No.02 |
//! | [`DashDot`] 细点画线 | `24d` / `3d` / `0.5d` / `3d` | GB/T 4457.4 No.04 |
//! | [`DoubleDashDot`] 细双点画线 | `24d` / `3d` / `0.5d` / `3d` / `0.5d` / `3d` | 见下 |
//! | [`Dotted`] 细点线 | `0.5d` / `3d` | GB/T 17450 No.07 |
//!
//! **双点画线的长画取值有分歧（据实说明，不假装无争议）**：不同二手资料里
//! 细双点画线的长画有 `12d` 与 `24d` 两说。本表**取 `24d`**——与 ISO 128-2
//! 线型组别 No.05「long dash–double dot」以及多数来源一致。
//!
//! [`Continuous`]: crate::linetype::GB_LINE_TYPES
//! [`Dashed`]: crate::linetype::GB_LINE_TYPES
//! [`DashDot`]: crate::linetype::GB_LINE_TYPES
//! [`DoubleDashDot`]: crate::linetype::GB_LINE_TYPES
//! [`Dotted`]: crate::linetype::GB_LINE_TYPES

/// 粗线基准宽 `d`（mm）——GB 优先值。段长以此为计量单位。
pub const GB_BASE_WIDTH_MM: f64 = 0.5;

/// 一种 GB 线型的定义。
#[derive(Debug, Clone, Copy)]
pub struct GbLineType {
    /// 我们的线型名（`core_doc::standard::LineType::name()` 的三层契约字符串）
    pub name: &'static str,
    /// DXF LTYPE 名（大写，全表唯一）
    pub dxf_name: &'static str,
    /// DXF 组码 3 的描述文字
    pub description: &'static str,
    /// 段长系数（以 `d` 为单位，**正 = 画、负 = 间隔**）；空 = 实线
    pub elements_in_d: &'static [f64],
}

/// 全表（**顺序即 DXF `TABLES` 段的写出顺序**）。
///
/// `name` 与 `core_doc::standard::LineType::name()` 严格一致；
/// `dxf_name` 全大写且互不相同。
pub const GB_LINE_TYPES: &[GbLineType] = &[
    GbLineType {
        name: "Continuous",
        dxf_name: "CONTINUOUS",
        description: "Solid line",
        elements_in_d: &[],
    },
    GbLineType {
        name: "Dashed",
        dxf_name: "DASHED",
        description: "Dashed __ __ __",
        elements_in_d: &[12.0, -3.0],
    },
    GbLineType {
        name: "DashDot",
        dxf_name: "DASHDOT",
        description: "Dash dot __ . __ .",
        elements_in_d: &[24.0, -3.0, 0.5, -3.0],
    },
    GbLineType {
        name: "DoubleDashDot",
        dxf_name: "DASHDOT2",
        description: "Dash double-dot __ .. __ ..",
        elements_in_d: &[24.0, -3.0, 0.5, -3.0, 0.5, -3.0],
    },
    GbLineType {
        name: "Dotted",
        dxf_name: "DOT",
        description: "Dot . . .",
        elements_in_d: &[0.5, -3.0],
    },
];

/// 按我们的线型名查表（未知线型 → `None`）。
#[must_use]
pub fn gb_line_type(name: &str) -> Option<&'static GbLineType> {
    GB_LINE_TYPES.iter().find(|t| t.name == name)
}

/// 段长（图纸 mm；**正 = 画、负 = 间隔**）。实线或未知线型 → `None`。
#[must_use]
pub fn gb_dash_pattern_mm(name: &str, base_width_mm: f64) -> Option<Vec<f64>> {
    let t = gb_line_type(name)?;
    if t.elements_in_d.is_empty() {
        return None; // 实线无段长
    }
    Some(t.elements_in_d.iter().map(|e| e * base_width_mm).collect())
}

/// 段长总长（一个周期 = `Σ|元素|·d`，mm）；实线 / 未知线型 → `0.0`（**正零**）。
///
/// 注意：空段长表（实线）经 `Sum for f64` 求和会得到 **`-0.0`**（其单位元是负零），
/// 故此处统一 `+ 0.0` 规范化为正零，避免把 `-0.000000` 写进 DXF 的 `40` 组码。
#[must_use]
pub fn gb_total_length_mm(name: &str, base_width_mm: f64) -> f64 {
    gb_line_type(name).map_or(0.0, |t| {
        // `-0.0 + 0.0 == +0.0`；非零值不受影响
        t.elements_in_d.iter().map(|e| e.abs()).sum::<f64>() * base_width_mm + 0.0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn approx_slice(a: &[f64], b: &[f64]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| approx(*x, *y))
    }

    #[test]
    fn gb_table_covers_all_five_line_types() {
        assert_eq!(GB_LINE_TYPES.len(), 5, "国标线型表应覆盖 5 种线型");
        for name in ["Continuous", "Dashed", "DashDot", "DoubleDashDot", "Dotted"] {
            assert!(gb_line_type(name).is_some(), "线型 {name} 未在表中登记");
        }
        assert!(gb_line_type("Nope").is_none(), "未知线型应返回 None");
    }

    #[test]
    fn gb_dashed_pattern_matches_standard() {
        let p = gb_dash_pattern_mm("Dashed", GB_BASE_WIDTH_MM).unwrap();
        assert!(
            approx_slice(&p, &[6.0, -1.5]),
            "细虚线应为 [6,-1.5]，实得 {p:?}"
        );
    }

    #[test]
    fn gb_dash_dot_pattern_matches_standard() {
        let p = gb_dash_pattern_mm("DashDot", GB_BASE_WIDTH_MM).unwrap();
        assert!(
            approx_slice(&p, &[12.0, -1.5, 0.25, -1.5]),
            "细点画线应为 [12,-1.5,0.25,-1.5]，实得 {p:?}"
        );
    }

    #[test]
    fn gb_double_dash_dot_pattern_matches_standard() {
        let p = gb_dash_pattern_mm("DoubleDashDot", GB_BASE_WIDTH_MM).unwrap();
        assert!(
            approx_slice(&p, &[12.0, -1.5, 0.25, -1.5, 0.25, -1.5]),
            "细双点画线应为 [12,-1.5,0.25,-1.5,0.25,-1.5]，实得 {p:?}"
        );
    }

    #[test]
    fn gb_dotted_pattern_matches_standard() {
        let p = gb_dash_pattern_mm("Dotted", GB_BASE_WIDTH_MM).unwrap();
        assert!(
            approx_slice(&p, &[0.25, -1.5]),
            "细点线应为 [0.25,-1.5]，实得 {p:?}"
        );
    }

    #[test]
    fn gb_continuous_and_unknown_have_no_pattern() {
        assert!(gb_dash_pattern_mm("Continuous", GB_BASE_WIDTH_MM).is_none());
        assert!(gb_dash_pattern_mm("Whatever", GB_BASE_WIDTH_MM).is_none());
        assert!(approx(
            gb_total_length_mm("Continuous", GB_BASE_WIDTH_MM),
            0.0
        ));
        assert!(approx(
            gb_total_length_mm("Whatever", GB_BASE_WIDTH_MM),
            0.0
        ));
        // 实线总长必须是**正零**：空段长表求和会得 -0.0，函数须规范化为 +0.0，
        // 否则 DXF 的 `40` 组码会写出 `-0.000000`。
        assert!(
            gb_total_length_mm("Continuous", GB_BASE_WIDTH_MM).is_sign_positive(),
            "实线总长应为正零，而非 -0.0"
        );
        assert!(
            gb_total_length_mm("Whatever", GB_BASE_WIDTH_MM).is_sign_positive(),
            "未知线型总长应为正零"
        );
    }

    #[test]
    fn gb_pattern_scales_with_base_width() {
        let p = gb_dash_pattern_mm("Dashed", 1.0).unwrap();
        assert!(
            approx_slice(&p, &[12.0, -3.0]),
            "d=1.0 时细虚线应为 [12,-3]，实得 {p:?}"
        );
        // 总长随 d 线性
        assert!(approx(gb_total_length_mm("Dashed", 1.0), 15.0));
    }

    #[test]
    fn dxf_names_are_uppercase_and_unique() {
        let mut seen = Vec::new();
        for t in GB_LINE_TYPES {
            assert_eq!(
                t.dxf_name,
                t.dxf_name.to_ascii_uppercase(),
                "DXF 名必须全大写：{}",
                t.dxf_name
            );
            assert!(!seen.contains(&t.dxf_name), "DXF 名重复：{}", t.dxf_name);
            seen.push(t.dxf_name);
        }
    }
}

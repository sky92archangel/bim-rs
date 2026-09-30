//! DXF 出口（**零新依赖** —— 手写 ASCII DXF 兼容子集）。
//!
//! ## 范围声明：**兼容子集，不是全量 AutoCAD 语义**
//!
//! 本项目导出的是**工程图**（线 + 文字 + 图层），不是 AutoCAD 的完整图纸
//! 数据库（无动态块、无 XREF、无 annotative scaling、无自定义对象）。
//! 所以本模块只写三个段：
//!
//! | 段 | 内容 | 为什么够用 |
//! |---|---|---|
//! | `HEADER` | `$ACADVER = AC1015` | 声明为 R2000，兼容面最广 |
//! | `TABLES` | `LTYPE` + `LAYER` | 图纸要能"看起来对"，必须带图层与线型 |
//! | `ENTITIES` | `LINE` + `TEXT` | 二维图元全部拆成线段 |
//!
//! ## 几个刻意的取舍
//!
//! - **折线拆成 `LINE` 而不是 `LWPOLYLINE`**：`LWPOLYLINE` 是 R13+ 才有的，
//!   而 `LINE` 从 R10 到最新版通吃。拆线段换的是**最大兼容性**，
//!   代价是文件大一点、编辑时不是整条多段线——对"交付看图"是可接受的。
//! - **颜色用 ACI 索引而不是真彩色**：真彩色（组码 420）在旧 CAD 里会被忽略。
//!   这里做一次 RGB → ACI 最近色映射（见 [`aci_index`]），保证至少"颜色是那个意思"。
//! - **弧 / 圆 / 椭圆写解析实体**：`CIRCLE` / `ARC` / `ELLIPSE`（R10 起即支持），
//!   角单位**度**（组码 50/51）；椭圆短长半轴比走组码 40。

use crate::drawing2d::{Analytic2D, Drawing2D};
use crate::linetype::{GB_BASE_WIDTH_MM, GB_LINE_TYPES, gb_line_type};

/// 一份 DXF 输出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DxfOutput {
    /// DXF 正文（ASCII）
    pub text: String,
    /// 写出的 `LINE` 实体数
    pub line_count: usize,
    /// 写出的 `TEXT` 实体数
    pub text_count: usize,
    /// 写出的解析 `CIRCLE` 实体数
    pub circle_count: usize,
    /// 写出的解析 `ARC` 实体数
    pub arc_count: usize,
    /// 写出的解析 `ELLIPSE` 实体数
    pub ellipse_count: usize,
    /// 写出的图层名
    pub layers: Vec<String>,
}

/// RGB → AutoCAD 颜色索引（ACI）的**最近色**映射。
///
/// 只覆盖制图常用的 7 个标准色；其余一律落到 `7`（ACI 7 是"随背景反转"的黑/白，
/// 是 DXF 里最安全的兜底色）。
#[must_use]
pub fn aci_index(color: [u8; 3]) -> u8 {
    match color {
        [0, 0, 0] => 7,       // 黑（纸上）/ 白（黑底）
        [255, 255, 255] => 7, // 白 → 同 ACI 7
        [255, 0, 0] => 1,     // 红
        [255, 255, 0] => 2,   // 黄
        [0, 255, 0] => 3,     // 绿
        [0, 255, 255] => 4,   // 青
        [0, 0, 255] => 5,     // 蓝
        [255, 0, 255] => 6,   // 洋红
        _ => 7,
    }
}

/// 线型名 → DXF LTYPE 名（大写）。
///
/// 查 [`crate::linetype::GB_LINE_TYPES`] 单一权威表：该表的 `dxf_name`
/// 即 DXF 内置线型名，且与国标段长表一一对应（[`crate::linetype`] 有完整依据）。
/// 未知线型 → `"CONTINUOUS"`。
#[must_use]
pub fn dxf_linetype(line_type: &str) -> &'static str {
    gb_line_type(line_type).map_or("CONTINUOUS", |t| t.dxf_name)
}

fn num(v: f64) -> String {
    // DXF 不接受科学计数法，统一定点输出
    format!("{v:.6}")
}

fn tag(out: &mut String, code: u16, value: &str) {
    out.push_str(&code.to_string());
    out.push('\n');
    out.push_str(value);
    out.push('\n');
}

fn tag_num(out: &mut String, code: u16, value: f64) {
    tag(out, code, &num(value));
}

/// 序列化为 ASCII DXF。
#[must_use]
pub fn to_dxf(drawing: &Drawing2D) -> DxfOutput {
    let mut out = String::with_capacity(2048 + drawing.entity_count() * 128);

    let layers = drawing.layer_names();
    let mut line_count = 0;
    let mut text_count = 0;
    let mut circle_count = 0;
    let mut arc_count = 0;
    let mut ellipse_count = 0;

    // ---------- HEADER ----------
    tag(&mut out, 0, "SECTION");
    tag(&mut out, 2, "HEADER");
    tag(&mut out, 9, "$ACADVER");
    tag(&mut out, 1, "AC1015"); // R2000：兼容面与实体表达力之间最稳的一档
    tag(&mut out, 9, "$INSUNITS");
    tag(&mut out, 70, "4"); // 4 = 毫米
    tag(&mut out, 0, "ENDSEC");

    // ---------- TABLES ----------
    tag(&mut out, 0, "SECTION");
    tag(&mut out, 2, "TABLES");

    // LTYPE 表：由国标段长表（单一权威来源）驱动
    tag(&mut out, 0, "TABLE");
    tag(&mut out, 2, "LTYPE");
    tag(&mut out, 70, &GB_LINE_TYPES.len().to_string());
    for lt in GB_LINE_TYPES {
        tag(&mut out, 0, "LTYPE");
        tag(&mut out, 2, lt.dxf_name);
        tag(&mut out, 70, "0");
        tag(&mut out, 3, lt.description);
        tag(&mut out, 72, "65"); // 'A' 型对齐
        tag(&mut out, 73, &lt.elements_in_d.len().to_string());
        tag_num(
            &mut out,
            40,
            crate::linetype::gb_total_length_mm(lt.name, GB_BASE_WIDTH_MM),
        );
        // 组码 49：**正 = 画、负 = 间隔**（保留国标表里的符号）
        for e in lt.elements_in_d {
            tag_num(&mut out, 49, e * GB_BASE_WIDTH_MM);
        }
    }
    tag(&mut out, 0, "ENDTAB");

    // LAYER 表
    tag(&mut out, 0, "TABLE");
    tag(&mut out, 2, "LAYER");
    tag(&mut out, 70, &layers.len().to_string());
    for layer in &layers {
        // 找该图层上第一条曲线，用它的颜色 / 线型当图层默认
        let (color, ltype) = drawing
            .curves
            .iter()
            .find(|c| c.layer == *layer)
            .map_or((7u8, "CONTINUOUS"), |c| {
                (aci_index(c.color), dxf_linetype(&c.line_type))
            });
        tag(&mut out, 0, "LAYER");
        tag(&mut out, 2, layer);
        tag(&mut out, 70, "0");
        tag(&mut out, 62, &color.to_string());
        tag(&mut out, 6, ltype);
    }
    tag(&mut out, 0, "ENDTAB");

    tag(&mut out, 0, "ENDSEC");

    // ---------- ENTITIES ----------
    tag(&mut out, 0, "SECTION");
    tag(&mut out, 2, "ENTITIES");

    for curve in &drawing.curves {
        let color = aci_index(curve.color);
        let ltype = dxf_linetype(&curve.line_type);

        // 解析图元（圆/弧）优先走 CIRCLE / ARC 实体
        if let Some(ref a) = curve.analytic {
            match a {
                Analytic2D::Circle { center, radius } => {
                    tag(&mut out, 0, "CIRCLE");
                    tag(&mut out, 8, &curve.layer);
                    tag(&mut out, 6, ltype);
                    tag(&mut out, 62, &color.to_string());
                    tag_num(&mut out, 10, center[0]);
                    tag_num(&mut out, 20, center[1]);
                    tag_num(&mut out, 30, 0.0);
                    tag_num(&mut out, 40, *radius);
                    circle_count += 1;
                    continue;
                }
                Analytic2D::Arc {
                    center,
                    radius,
                    start_angle_rad,
                    end_angle_rad,
                } => {
                    tag(&mut out, 0, "ARC");
                    tag(&mut out, 8, &curve.layer);
                    tag(&mut out, 6, ltype);
                    tag(&mut out, 62, &color.to_string());
                    tag_num(&mut out, 10, center[0]);
                    tag_num(&mut out, 20, center[1]);
                    tag_num(&mut out, 30, 0.0);
                    tag_num(&mut out, 40, *radius);
                    tag_num(&mut out, 50, start_angle_rad.to_degrees());
                    tag_num(&mut out, 51, end_angle_rad.to_degrees());
                    arc_count += 1;
                    continue;
                }
                Analytic2D::Ellipse {
                    center,
                    rx,
                    ry,
                    rotation_rad,
                } => {
                    // DXF ELLIPSE：中心(10/20/30) + 长轴端点向量(11/21/31，相对中心)
                    //             + 法向(210/220/230，平面为 +Z) + 短长半轴比(40)
                    //             + 主轴方位角(50，度) + 起止参数(41/42)
                    let (sr, cr) = rotation_rad.sin_cos();
                    tag(&mut out, 0, "ELLIPSE");
                    tag(&mut out, 8, &curve.layer);
                    tag(&mut out, 6, ltype);
                    tag(&mut out, 62, &color.to_string());
                    tag_num(&mut out, 10, center[0]);
                    tag_num(&mut out, 20, center[1]);
                    tag_num(&mut out, 30, 0.0);
                    tag_num(&mut out, 11, rx * cr);
                    tag_num(&mut out, 21, rx * sr);
                    tag_num(&mut out, 31, 0.0);
                    tag_num(&mut out, 210, 0.0);
                    tag_num(&mut out, 220, 0.0);
                    tag_num(&mut out, 230, 1.0);
                    // 组码 40 是"短轴 / 长轴"之比；rx 已被约定为长半轴
                    let ratio = if rx.abs() > f64::EPSILON {
                        ry / rx
                    } else {
                        0.0
                    };
                    tag_num(&mut out, 40, ratio);
                    tag_num(&mut out, 50, rotation_rad.to_degrees());
                    tag_num(&mut out, 41, 0.0);
                    tag_num(&mut out, 42, std::f64::consts::TAU);
                    ellipse_count += 1;
                    continue;
                }
            }
        }

        let pts = &curve.points;
        if pts.len() < 2 {
            continue;
        }
        let seg_count = if curve.closed {
            pts.len()
        } else {
            pts.len() - 1
        };
        for i in 0..seg_count {
            let a = pts[i];
            let b = pts[(i + 1) % pts.len()];
            tag(&mut out, 0, "LINE");
            tag(&mut out, 8, &curve.layer);
            tag(&mut out, 6, ltype);
            tag(&mut out, 62, &color.to_string());
            tag_num(&mut out, 10, a[0]);
            tag_num(&mut out, 20, a[1]);
            tag_num(&mut out, 30, 0.0);
            tag_num(&mut out, 11, b[0]);
            tag_num(&mut out, 21, b[1]);
            tag_num(&mut out, 31, 0.0);
            line_count += 1;
        }
    }

    for text in &drawing.texts {
        tag(&mut out, 0, "TEXT");
        tag(&mut out, 8, &text.layer);
        tag(&mut out, 62, &aci_index(text.color).to_string());
        tag_num(&mut out, 10, text.at[0]);
        tag_num(&mut out, 20, text.at[1]);
        tag_num(&mut out, 30, 0.0);
        tag_num(&mut out, 40, text.height_mm);
        tag_num(&mut out, 50, text.rotation_deg);
        // 组码 1 是文本内容本身；DXF 里换行/回车必须转义成 ^J / ^M
        tag(
            &mut out,
            1,
            &text.content.replace('\n', "^J").replace('\r', "^M"),
        );
        text_count += 1;
    }

    tag(&mut out, 0, "ENDSEC");
    tag(&mut out, 0, "EOF");

    DxfOutput {
        text: out,
        line_count,
        text_count,
        circle_count,
        arc_count,
        ellipse_count,
        layers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawing2d::{Curve2D, Text2D};

    fn sample() -> Drawing2D {
        let mut d = Drawing2D::new(420.0, 297.0);
        d.push_curve(
            Curve2D::new(vec![[0.0, 0.0], [100.0, 0.0]], "OUTLINE-可见轮廓")
                .with_style(0.5, [255, 0, 0]),
        );
        d.push_curve(
            Curve2D::new(
                vec![[0.0, 0.0], [0.0, 50.0], [50.0, 50.0]],
                "HIDDEN-不可见轮廓",
            )
            .with_line_type("Dashed"),
        );
        d.push_text(Text2D::new([10.0, 5.0], "一层平面图", 3.5).on_layer("TEXT-文字"));
        d
    }

    #[test]
    fn dxf_has_the_three_required_sections() {
        let out = to_dxf(&sample());
        assert!(out.text.contains("\nHEADER\n") || out.text.starts_with("0\nSECTION"));
        assert!(out.text.contains("\nTABLES\n"));
        assert!(out.text.contains("\nENTITIES\n"));
        assert!(out.text.trim_end().ends_with("EOF"));
    }

    #[test]
    fn polyline_is_split_into_line_entities() {
        let out = to_dxf(&sample());
        // 第 1 条：2 点 → 1 段；第 2 条：3 点 → 2 段
        assert_eq!(out.line_count, 3);
        assert_eq!(out.text.matches("\nLINE\n").count(), 3);
    }

    #[test]
    fn closed_curve_emits_the_closing_segment() {
        let mut d = Drawing2D::new(10.0, 10.0);
        d.push_curve(Curve2D::new(vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]], "L").closed());
        assert_eq!(to_dxf(&d).line_count, 3, "三角形应有 3 段");
    }

    #[test]
    fn declared_acadver_is_r2000_and_units_are_mm() {
        let out = to_dxf(&sample());
        assert!(out.text.contains("AC1015"));
        assert!(out.text.contains("$INSUNITS"));
    }

    #[test]
    fn layers_are_all_declared_in_the_layer_table() {
        let out = to_dxf(&sample());
        assert_eq!(out.layers.len(), 3); // OUTLINE / HIDDEN / TEXT
        for layer in &out.layers {
            let marker = format!("\n{layer}\n");
            // LAYER 表里出现过
            assert!(
                out.text.contains(&marker),
                "图层 {layer} 未在 LAYER 表中声明"
            );
        }
    }

    #[test]
    fn dashed_curve_maps_to_the_dashed_linetype() {
        let out = to_dxf(&sample());
        assert!(out.text.contains("DASHED"));
    }

    #[test]
    fn standard_colors_map_to_aci_indices() {
        assert_eq!(aci_index([0, 0, 0]), 7);
        assert_eq!(aci_index([255, 255, 255]), 7);
        assert_eq!(aci_index([255, 0, 0]), 1);
        assert_eq!(aci_index([255, 255, 0]), 2);
        assert_eq!(aci_index([0, 255, 0]), 3);
        assert_eq!(aci_index([0, 255, 255]), 4);
        assert_eq!(aci_index([255, 0, 255]), 6);
        // 非标准色兜底为 ACI 7，不报错
        assert_eq!(aci_index([12, 34, 56]), 7);
    }

    #[test]
    fn numbers_are_written_without_scientific_notation() {
        let mut d = Drawing2D::new(10.0, 10.0);
        d.push_curve(Curve2D::new(vec![[0.0, 0.0], [1.0e-9, 0.0]], "L"));
        let out = to_dxf(&d);
        assert!(!out.text.contains("e-9"), "DXF 不接受科学计数法");
    }

    #[test]
    fn text_newlines_are_escaped() {
        let mut d = Drawing2D::new(10.0, 10.0);
        d.push_text(Text2D::new([0.0, 0.0], "第一行\n第二行", 3.5));
        let out = to_dxf(&d);
        assert!(out.text.contains("第一行^J第二行"));
    }

    #[test]
    fn degenerate_curves_are_skipped() {
        let mut d = Drawing2D::new(10.0, 10.0);
        d.push_curve(Curve2D::new(vec![[5.0, 5.0]], "L"));
        assert_eq!(to_dxf(&d).line_count, 0);
    }

    #[test]
    fn empty_drawing_produces_a_wellformed_shell() {
        let out = to_dxf(&Drawing2D::new(210.0, 297.0));
        assert_eq!(out.line_count, 0);
        assert_eq!(out.text_count, 0);
        assert!(out.text.trim_end().ends_with("EOF"));
    }

    /// B3-8 钉子：解析圆写出真正的 CIRCLE 实体，不再拆 LINE。
    #[test]
    fn dxf_export_emits_analytic_circle() {
        let mut d = Drawing2D::new(100.0, 100.0);
        let mut curve = Curve2D::new(vec![[10.0, 20.0]], "L");
        curve.analytic = Some(Analytic2D::Circle {
            center: [10.0, 20.0],
            radius: 5.0,
        });
        d.push_curve(curve);

        let out = to_dxf(&d);
        assert!(out.text.contains("\nCIRCLE\n"), "应含 CIRCLE 实体");
        assert!(out.text.contains("40\n5.000000"), "组码 40 应为半径 5");
        assert!(out.text.contains("10\n10.000000"), "圆心 X=10");
        assert!(out.text.contains("20\n20.000000"), "圆心 Y=20");
        assert_eq!(out.circle_count, 1);
        assert_eq!(out.line_count, 0, "该圆不得再贡献任何 LINE");
        assert!(!out.text.contains("\nLINE\n"), "全文不应有 LINE");
    }

    /// B3-8 钉子：解析弧写出 ARC 实体，角单位为度（组码 50/51）。
    #[test]
    fn dxf_export_emits_analytic_arc_with_degrees() {
        let mut d = Drawing2D::new(100.0, 100.0);
        let mut curve = Curve2D::new(vec![[0.0, 0.0]], "L");
        curve.analytic = Some(Analytic2D::Arc {
            center: [0.0, 0.0],
            radius: 4.0,
            start_angle_rad: std::f64::consts::FRAC_PI_6,
            end_angle_rad: std::f64::consts::FRAC_PI_2,
        });
        d.push_curve(curve);

        let out = to_dxf(&d);
        assert!(out.text.contains("\nARC\n"), "应含 ARC 实体");
        assert!(
            out.text.contains("50\n30.000000"),
            "起始角应为 30.000000 度（π/6 rad，非 0 加强）"
        );
        assert!(
            out.text.contains("51\n90.000000"),
            "终止角应为 90.000000 度（π/2 rad），实文：\n{}",
            out.text
        );
        assert_eq!(out.arc_count, 1);
        assert_eq!(out.line_count, 0);
    }

    /// B5.1 钉子：解析椭圆写出真正的 `ELLIPSE` 实体（短长半轴比 40 非 1），不再拆 LINE。
    #[test]
    fn dxf_export_emits_ellipse_entity() {
        let mut d = Drawing2D::new(200.0, 200.0);
        let mut curve = Curve2D::new(vec![[10.0, 20.0]], "L");
        curve.analytic = Some(Analytic2D::Ellipse {
            center: [10.0, 20.0],
            rx: 50.0,
            ry: 20.0,
            rotation_rad: 0.0,
        });
        d.push_curve(curve);

        let out = to_dxf(&d);
        assert!(
            out.text.contains("\nELLIPSE\n"),
            "应含 ELLIPSE 实体，实文：\n{}",
            out.text
        );
        assert!(
            out.text.contains("40\n0.400000"),
            "组码 40 应为短长半轴比 0.4（区别于同半径圆的 1）"
        );
        assert!(out.text.contains("11\n50.000000"), "长轴端点 X 应为 rx=50");
        assert!(out.text.contains("21\n0.000000"), "长轴端点 Y 应为 0");
        assert!(
            out.text.contains("50\n0.000000"),
            "组码 50 应为主轴方位角（度）"
        );
        assert_eq!(out.ellipse_count, 1);
        assert_eq!(out.line_count, 0, "该椭圆不得再贡献任何 LINE");
        assert!(!out.text.contains("\nLINE\n"), "全文不应有 LINE");
    }

    /// 从 DXF 正文里截出某个 LTYPE 记录块：从 `dxf_name` 行之后，到下一个
    /// `0\nLTYPE\n` 记录或 `0\nENDTAB\n` 为止（**域内断言**，避免跨段巧合命中）。
    fn lt_record<'a>(text: &'a str, dxf_name: &str) -> &'a str {
        let marker = format!("\n{dxf_name}\n");
        let rest = text
            .split_once(&marker)
            .unwrap_or_else(|| panic!("未找到线型 {dxf_name} 的 LTYPE 记录"))
            .1;
        let end = rest
            .find("0\nLTYPE\n")
            .unwrap_or(rest.len())
            .min(rest.find("0\nENDTAB\n").unwrap_or(rest.len()));
        &rest[..end]
    }

    /// B5.3 钉子 ⑨：LTYPE 表由国标表驱动，段长符号正确（**正 = 画、负 = 间隔**）。
    #[test]
    fn dxf_declares_gb_linetypes_with_signed_segments() {
        let out = to_dxf(&sample());
        // 5 种国标线型全部登记（含此前漏掉的 DASHDOT2）
        for lt in crate::linetype::GB_LINE_TYPES {
            let marker = format!("\n{}\n", lt.dxf_name);
            assert!(
                out.text.contains(&marker),
                "LTYPE 表缺线型 {}，实文：\n{}",
                lt.dxf_name,
                out.text
            );
        }

        // 组码 49：细虚线 12d/-3d；d=0.5 → 画 6.0、间隔 -1.5（间隔必须带负号）。
        // 缺 2：域内断言（限定在 DASHED 记录块内），不再用非锚定全文 contains。
        let dashed = lt_record(&out.text, "DASHED");
        assert!(
            dashed.contains("49\n6.000000\n49\n-1.500000\n"),
            "细虚线组码 49 应为 6/-1.5（间隔带负号），DASHED 记录块：\n{dashed}"
        );

        // 实线无组码 49
        let continuous = lt_record(&out.text, "CONTINUOUS");
        assert!(
            !continuous.contains("\n49\n"),
            "实线不应有组码 49，CONTINUOUS 记录块：\n{continuous}"
        );

        // 缺 1：用**字面量**钉住组码 40（总段长）—— 此前完全无断言（QA 变异基准→1.0 时 125 全绿）。
        // 期望 = Σ|元素|·d（d=0.5）：DASHED (12+3)·0.5=7.5；DASHDOT (24+3+0.5+3)·0.5=15.25；
        // DASHDOT2 (24+3+0.5+3+0.5+3)·0.5=17.0；DOT (0.5+3)·0.5=1.75。逐条域内断言。
        for (name, total) in [
            ("DASHED", "40\n7.500000\n"),
            ("DASHDOT", "40\n15.250000\n"),
            ("DASHDOT2", "40\n17.000000\n"),
            ("DOT", "40\n1.750000\n"),
        ] {
            let rec = lt_record(&out.text, name);
            assert!(
                rec.contains(total),
                "线型 {name} 的组码 40 应为 {total:?}，记录块：\n{rec}"
            );
        }
        // CONTINUOUS：空段长表 → 总长 0（`gb_total_length_mm` 已把空表求和的 -0.0 规范化为 +0.0）。
        assert!(
            continuous.contains("40\n0.000000\n"),
            "CONTINUOUS 组码 40 应为 0.000000（空段长表总长规范化为 +0.0），记录块：\n{continuous}"
        );

        // 表头计数与国标表长度一致
        assert!(
            out.text
                .contains(&format!("70\n{}\n", crate::linetype::GB_LINE_TYPES.len())),
            "LTYPE 表 70 计数应为 {}",
            crate::linetype::GB_LINE_TYPES.len()
        );
    }

    /// B5.3 钉子 ⑩：细双点画线映射到**自己的** LTYPE（不再塌到 DASHDOT）。
    #[test]
    fn dxf_maps_double_dash_dot_to_its_own_linetype() {
        assert_eq!(dxf_linetype("DoubleDashDot"), "DASHDOT2");
        assert_ne!(dxf_linetype("DoubleDashDot"), dxf_linetype("DashDot"));
    }

    /// B5.3 钉子 ⑪：未知线型兜底为 CONTINUOUS。
    #[test]
    fn dxf_unknown_line_type_falls_back_to_continuous() {
        assert_eq!(dxf_linetype("NoSuchLineType"), "CONTINUOUS");
        assert_eq!(dxf_linetype(""), "CONTINUOUS");
    }
}

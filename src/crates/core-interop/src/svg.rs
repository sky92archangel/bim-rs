//! SVG 出口（**零新依赖** —— 手写序列化 + 将来接 `kurbo` 做曲线细分）。
//!
//! ## 为什么 SVG 不需要库
//!
//! SVG 是纯文本 XML，一条折线就是 `<polyline points="...">`。
//! 把二维点列写进 XML 是**序列化**，不是**几何算法**——
//! 后者（消隐、投影）已经在 `core-kernel` 做完了。
//!
//! `kurbo` / `lyon` 的用武之地在**绘制层**（曲线细分、描边、填充、字体轮廓），
//! 那是"把弧变成折线"那一步。本模块对**带解析图元**（`CIRCLE`/`ARC`）的曲线已能
//! **现场离散化**成折线（见 [`curve_points_for_svg`]），避免"有几何却不画"；
//! **M5 接入 `kurbo` 后再升级细分质量**（自适应弧长、更少段数、更平滑）。

use std::fmt::Write as _;

use crate::drawing2d::{Analytic2D, Curve2D, Drawing2D};

/// 一份 SVG 输出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SvgOutput {
    /// SVG 正文
    pub text: String,
    /// 曲线数
    pub curve_count: usize,
    /// 文字数
    pub text_count: usize,
}

/// XML 特殊字符转义。
fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

/// 把图纸坐标（**Y 朝上**、原点左下）换成 SVG 坐标（**Y 朝下**、原点左上）。
fn flip_y(y: f64, height_mm: f64) -> f64 {
    height_mm - y
}

/// 把线条颜色写成 `#rrggbb`。
fn hex(color: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2])
}

/// 把线型名映射成 SVG 的 `stroke-dasharray`（`None` 表示实线）。
///
/// 段长取自**国标线型表**（[`crate::linetype`] 单一权威来源），与 DXF 出口同源；
/// SVG 的 `stroke-dasharray` **不区分画/间隔的符号**，故取绝对值后空格连接。
/// 实线与未知线型 → `None`（按实线导出，尽力而为）。
fn dash_array(line_type: &str) -> Option<String> {
    let pattern =
        crate::linetype::gb_dash_pattern_mm(line_type, crate::linetype::GB_BASE_WIDTH_MM)?;
    Some(
        pattern
            .iter()
            .map(|v| format!("{}", v.abs()))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// 把一条曲线离散成用于 SVG 绘制的点列。
///
/// **优先用曲线自带点列**；当点列不足以成线（`< 2` 点）但曲线带**解析图元**
/// （DXF 导入的 `CIRCLE` / `ARC` 只带"圆心兜底点"）时，**现场离散化解析图元**。
///
/// 为什么必须这样做（QA P1-3）：`read_dxf → to_svg` 这条路径上，圆/弧只有 1 个
/// 兜底点，旧逻辑 `if points.len() < 2 { continue }` 会**静默丢弃**它们——
/// 文件里明明有圆，导出的 SVG 里却没有，且没有任何警告。这正是"看着成功、实则丢几何"。
fn curve_points_for_svg(curve: &Curve2D) -> Vec<[f64; 2]> {
    if curve.points.len() >= 2 {
        return curve.points.clone();
    }
    // 点列不足 → 用解析图元兜底离散化（仅在无法成线时才补，避免与自带点列重复）
    match curve.analytic {
        Some(Analytic2D::Circle { center, radius }) => {
            const SEGMENTS: usize = 64;
            (0..SEGMENTS)
                .map(|i| {
                    let t = std::f64::consts::TAU * (i as f64) / (SEGMENTS as f64);
                    [center[0] + radius * t.cos(), center[1] + radius * t.sin()]
                })
                .collect()
        }
        Some(Analytic2D::Arc {
            center,
            radius,
            start_angle_rad,
            end_angle_rad,
        }) => {
            // 取 CCW 扫角；结束角不大于起始角时按跨 0° 处理
            let mut sweep = end_angle_rad - start_angle_rad;
            while sweep <= 0.0 {
                sweep += std::f64::consts::TAU;
            }
            let n = ((sweep / std::f64::consts::TAU * 64.0).ceil() as usize).max(2);
            (0..=n)
                .map(|i| {
                    let t = start_angle_rad + sweep * (i as f64) / (n as f64);
                    [center[0] + radius * t.cos(), center[1] + radius * t.sin()]
                })
                .collect()
        }
        Some(Analytic2D::Ellipse {
            center,
            rx,
            ry,
            rotation_rad,
        }) => {
            // 按椭圆参数离散化（rx·cos t、ry·sin t），再绕中心旋转 rotation_rad
            const SEGMENTS: usize = 64;
            let (sr, cr) = rotation_rad.sin_cos();
            (0..SEGMENTS)
                .map(|i| {
                    let t = std::f64::consts::TAU * (i as f64) / (SEGMENTS as f64);
                    let (st, ct) = t.sin_cos();
                    let x = rx * ct;
                    let y = ry * st;
                    [center[0] + cr * x - sr * y, center[1] + sr * x + cr * y]
                })
                .collect()
        }
        None => Vec::new(),
    }
}

/// 序列化为 SVG。
///
/// 输出使用 `mm` 作为用户单位，`viewBox` 直接用毫米——这样在浏览器里
/// 打开就是**真实图幅**，打印时无需再换算。
#[must_use]
pub fn to_svg(drawing: &Drawing2D) -> SvgOutput {
    let mut svg = String::with_capacity(1024 + drawing.entity_count() * 64);
    let w = drawing.width_mm;
    let h = drawing.height_mm;

    let _ = writeln!(svg, r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    let _ = writeln!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" version="1.1" width="{w}mm" height="{h}mm" viewBox="0 0 {w} {h}">"#
    );
    // 背景：国标图纸是白底黑线；这里给白底，避免黑底白线在浏览器里看不见
    let _ = writeln!(
        svg,
        r##"  <rect x="0" y="0" width="{w}" height="{h}" fill="#ffffff"/>"##
    );
    let _ = writeln!(
        svg,
        r#"  <g fill="none" stroke-linecap="round" stroke-linejoin="round">"#
    );

    for curve in &drawing.curves {
        // 点列不足时用解析图元兜底离散化；仍不足（既无点列也无解析图元）才跳过
        let points = curve_points_for_svg(curve);
        if points.len() < 2 {
            continue;
        }
        let pts: Vec<String> = points
            .iter()
            .map(|p| format!("{:.6},{:.6}", p[0], flip_y(p[1], h)))
            .collect();

        let width = if curve.width_mm > 0.0 {
            curve.width_mm
        } else {
            // SVG 默认 1px；给个最细线宽，避免"消失"
            0.13
        };

        // 解析圆 / 椭圆本身是闭合的（`curve.closed` 对其可能未置位）→ 强制按闭合多边形画
        let is_closed = curve.closed
            || matches!(
                curve.analytic,
                Some(Analytic2D::Circle { .. }) | Some(Analytic2D::Ellipse { .. })
            );
        let tag = if is_closed { "polygon" } else { "polyline" };
        let mut attrs = format!(
            r#"points="{}" stroke="{}" stroke-width="{width}""#,
            pts.join(" "),
            hex(curve.color)
        );
        if let Some(dash) = dash_array(&curve.line_type) {
            let _ = write!(attrs, r#" stroke-dasharray="{dash}""#);
        }
        let _ = writeln!(
            svg,
            r#"    <{tag} {attrs} data-layer="{}"/>"#,
            escape_xml(&curve.layer)
        );
    }
    let _ = writeln!(svg, "  </g>");

    for text in &drawing.texts {
        let y = flip_y(text.at[1], h);
        let rotate = if text.rotation_deg.abs() > f64::EPSILON {
            // SVG 的旋转是顺时针为正，制图角度是逆时针为正 → 取负
            format!(
                r#" transform="rotate({:.4} {:.6} {:.6})""#,
                -text.rotation_deg, text.at[0], y
            )
        } else {
            String::new()
        };
        let _ = writeln!(
            svg,
            r#"  <text x="{:.6}" y="{:.6}" font-size="{}" fill="{}"{rotate} data-layer="{}">{}</text>"#,
            text.at[0],
            y,
            text.height_mm,
            hex(text.color),
            escape_xml(&text.layer),
            escape_xml(&text.content)
        );
    }

    let _ = writeln!(svg, "</svg>");

    SvgOutput {
        text: svg,
        curve_count: drawing.curves.len(),
        text_count: drawing.texts.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawing2d::{Curve2D, Text2D};

    fn sample() -> Drawing2D {
        let mut d = Drawing2D::new(297.0, 210.0);
        d.push_curve(
            Curve2D::new(vec![[10.0, 10.0], [100.0, 10.0]], "OUTLINE-可见轮廓")
                .with_style(0.5, [0, 0, 0]),
        );
        d.push_curve(
            Curve2D::new(vec![[10.0, 10.0], [10.0, 60.0]], "HIDDEN-不可见轮廓")
                .with_line_type("Dashed")
                .with_style(0.25, [0, 255, 255]),
        );
        d.push_text(Text2D::new([50.0, 20.0], "600", 3.5).on_layer("TEXT-文字"));
        d
    }

    #[test]
    fn svg_has_the_expected_skeleton() {
        let out = to_svg(&sample());
        assert!(out.text.starts_with("<?xml"));
        assert!(out.text.contains(r#"viewBox="0 0 297 210""#));
        assert!(out.text.contains(r#"width="297mm""#));
        assert!(out.text.trim_end().ends_with("</svg>"));
        assert_eq!(out.curve_count, 2);
        assert_eq!(out.text_count, 1);
    }

    #[test]
    fn y_axis_is_flipped_for_svg() {
        let mut d = Drawing2D::new(100.0, 200.0);
        // 图纸坐标 y=200（顶部）→ SVG y=0
        d.push_curve(Curve2D::new(vec![[0.0, 200.0], [10.0, 200.0]], "L"));
        let out = to_svg(&d);
        assert!(
            out.text.contains("0.000000,0.000000"),
            "顶部点在 SVG 里应是 y=0，实际输出：{}",
            out.text
        );
    }

    #[test]
    fn closed_curves_use_polygon() {
        let mut d = Drawing2D::new(100.0, 100.0);
        d.push_curve(Curve2D::new(vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]], "L").closed());
        let out = to_svg(&d);
        assert!(out.text.contains("<polygon"));
        assert!(!out.text.contains("<polyline"));
    }

    #[test]
    fn dashed_line_type_becomes_a_dash_array() {
        let out = to_svg(&sample());
        assert!(
            out.text.contains(r#"stroke-dasharray="6 1.5""#),
            "虚线应按国标段长 12d/3d（d=0.5 → 6/1.5），实文：{}",
            out.text
        );
    }

    /// B5.3 钉子 ⑬：细点画线按国标段长派生 dasharray（24d/3d/0.5d/3d → 12 1.5 0.25 1.5）。
    #[test]
    fn dash_dot_line_type_uses_gb_segments() {
        let mut d = Drawing2D::new(100.0, 100.0);
        d.push_curve(Curve2D::new(vec![[0.0, 0.0], [50.0, 0.0]], "L").with_line_type("DashDot"));
        let out = to_svg(&d);
        assert!(
            out.text.contains(r#"stroke-dasharray="12 1.5 0.25 1.5""#),
            "细点画线 dasharray 应按国标段长，实文：{}",
            out.text
        );
    }

    /// B5.3 钉子 ⑭：细双点画线与细点线同样走国标表（且双点画线不再塌到点画线）。
    #[test]
    fn double_dash_dot_and_dotted_use_gb_segments() {
        let mut d = Drawing2D::new(100.0, 100.0);
        d.push_curve(
            Curve2D::new(vec![[0.0, 0.0], [50.0, 0.0]], "L").with_line_type("DoubleDashDot"),
        );
        d.push_curve(Curve2D::new(vec![[0.0, 10.0], [50.0, 10.0]], "L").with_line_type("Dotted"));
        let out = to_svg(&d);
        assert!(
            out.text
                .contains(r#"stroke-dasharray="12 1.5 0.25 1.5 0.25 1.5""#),
            "细双点画线 dasharray 应按国标段长，实文：{}",
            out.text
        );
        assert!(
            out.text.contains(r#"stroke-dasharray="0.25 1.5""#),
            "细点线 dasharray 应按国标段长，实文：{}",
            out.text
        );
    }

    #[test]
    fn continuous_line_type_has_no_dash_array() {
        let mut d = Drawing2D::new(10.0, 10.0);
        d.push_curve(Curve2D::new(vec![[0.0, 0.0], [1.0, 1.0]], "L"));
        let out = to_svg(&d);
        assert!(!out.text.contains("stroke-dasharray"));
    }

    /// B5.3 钉子 ⑮：**未知线型**按实线导出（无 dasharray），且不 panic。
    #[test]
    fn unknown_line_type_has_no_dash_array_and_does_not_panic() {
        let mut d = Drawing2D::new(10.0, 10.0);
        d.push_curve(Curve2D::new(vec![[0.0, 0.0], [1.0, 1.0]], "L").with_line_type("Whatever"));
        let out = to_svg(&d);
        assert!(
            !out.text.contains("stroke-dasharray"),
            "未知线型不应产生 dasharray，实文：{}",
            out.text
        );
        assert_eq!(out.curve_count, 1, "曲线仍应导出（尽力而为，不丢几何）");
    }

    #[test]
    fn curve_layer_is_emitted_as_a_data_attribute() {
        let out = to_svg(&sample());
        assert!(out.text.contains(r#"data-layer="OUTLINE-可见轮廓""#));
    }

    #[test]
    fn text_content_is_xml_escaped() {
        let mut d = Drawing2D::new(10.0, 10.0);
        d.push_text(Text2D::new([0.0, 0.0], "宽<600> & \"高\"", 3.5));
        let out = to_svg(&d);
        assert!(out.text.contains("宽&lt;600&gt; &amp; &quot;高&quot;"));
        assert!(!out.text.contains("宽<600>"));
    }

    #[test]
    fn degenerate_curves_are_skipped_not_crashing() {
        let mut d = Drawing2D::new(10.0, 10.0);
        d.push_curve(Curve2D::new(vec![[1.0, 1.0]], "L")); // 单点
        let out = to_svg(&d);
        assert!(!out.text.contains("<polyline"));
    }

    #[test]
    fn empty_drawing_still_produces_valid_svg() {
        let out = to_svg(&Drawing2D::new(210.0, 297.0));
        assert!(out.text.contains("<svg"));
        assert!(out.text.contains("</svg>"));
        assert_eq!(out.curve_count, 0);
    }

    /// QA P1-3 回归钉：**只带解析图元的圆/弧**（DXF 导入的形态：1 个圆心兜底点）
    /// 必须被**画出来**，而不是被静默丢弃。
    #[test]
    fn analytic_only_circle_is_tessellated_not_dropped() {
        let mut d = Drawing2D::new(100.0, 100.0);
        let mut c = Curve2D::new(vec![[50.0, 50.0]], "L"); // 只有圆心兜底点
        c.analytic = Some(Analytic2D::Circle {
            center: [50.0, 50.0],
            radius: 25.0,
        });
        d.push_curve(c);

        let out = to_svg(&d);
        // 圆应被画成闭合多边形（64 段），而不是"没有输出"
        assert!(
            out.text.contains("<polygon"),
            "解析圆必须被画出来，实际输出：{}",
            out.text
        );
        // 半径 25 + 圆心 (50,50) → 圆上应有 x=75 的点（50+25）
        assert!(
            out.text.contains("75.000000"),
            "离散化应覆盖圆周（含 x=75），实际输出：{}",
            out.text
        );
    }

    #[test]
    fn analytic_only_arc_is_tessellated_not_dropped() {
        let mut d = Drawing2D::new(100.0, 100.0);
        let mut c = Curve2D::new(vec![[0.0, 0.0]], "L");
        c.analytic = Some(Analytic2D::Arc {
            center: [0.0, 0.0],
            radius: 10.0,
            start_angle_rad: 0.0,
            end_angle_rad: std::f64::consts::FRAC_PI_2,
        });
        d.push_curve(c);

        let out = to_svg(&d);
        // 弧不是闭合的 → polyline；且包含 (10,0) 与 (0,10) 两端点
        assert!(
            out.text.contains("<polyline"),
            "解析弧必须被画出来，实际输出：{}",
            out.text
        );
        assert!(out.text.contains("10.000000"), "弧应含 x=10 端点");
    }

    /// 只带解析椭圆的曲线必须被画出来（闭合多边形），而不是被静默丢弃。
    #[test]
    fn analytic_only_ellipse_is_tessellated_not_dropped() {
        let mut d = Drawing2D::new(200.0, 200.0);
        let mut c = Curve2D::new(vec![[100.0, 100.0]], "L"); // 只有中心兜底点
        c.analytic = Some(Analytic2D::Ellipse {
            center: [100.0, 100.0],
            rx: 50.0,
            ry: 20.0,
            rotation_rad: 0.0,
        });
        d.push_curve(c);

        let out = to_svg(&d);
        assert!(
            out.text.contains("<polygon"),
            "解析椭圆必须被画成闭合多边形，实际输出：{}",
            out.text
        );
        // rx=50 + 中心 (100,100) → 椭圆上应有 x=150 的点
        assert!(
            out.text.contains("150.000000"),
            "离散化应覆盖长轴端点 x=150，实际输出：{}",
            out.text
        );
    }
}

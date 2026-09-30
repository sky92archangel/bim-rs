//! DXF 导入器：用 `dxf` crate 读入 DXF 文件，产出中性 Drawing2D。
//!
//! ## 范围声明
//!
//! 只支持**工程图兼容子集**（LINE / CIRCLE / ARC / LWPOLYLINE / TEXT），
//! 不做全量 AutoCAD 语义。其余实体类型 warn 跳过，不整包失败。
//!
//! ## 圆 / 弧：解析实体（§7.4 硬要求）
//!
//! CIRCLE / ARC 读入后**同时**填 `analytic`（解析描述）与 `points`（圆心
//! 兜底点列）——导出器优先用 `analytic` 写真正的 `CIRCLE` / `ARC` 实体。

use dxf::Drawing as DxfDrawing;
use dxf::entities::*;

use crate::drawing2d::{Analytic2D, Curve2D, Drawing2D, Text2D};
use crate::error::{InteropError, InteropResult};

/// 未支持实体 warn 时从 `EntityType` 的 Debug 变体名提取类型名。
fn entity_type_name(specific: &EntityType) -> String {
    // EntityType 没有 Display；Debug 形如 `Ellipse(Ellipse { .. })`，取变体名
    let debug = format!("{specific:?}");
    debug
        .split('(')
        .next()
        .unwrap_or("UNKNOWN")
        .trim()
        .to_string()
}

/// 用 dxf crate 读入 DXF 文件，产出中性 Drawing2D 与 warnings。
///
/// # 错误
///
/// 只阻断：dxf crate 自身读不了（格式损坏 / 非 DXF 内容）。
/// 不认识的实体类型 warn 跳过，不阻断整包导入。
pub fn read_dxf(source: &[u8]) -> InteropResult<(Drawing2D, Vec<String>)> {
    let mut cursor = std::io::Cursor::new(source);
    let dxf = DxfDrawing::load(&mut cursor)
        .map_err(|e| InteropError::Parse(format!("DXF 解析失败：{e}")))?;

    let mut out = Drawing2D::new(420.0, 297.0);
    let mut warnings: Vec<String> = Vec::new();

    for entity in dxf.entities() {
        let layer = entity.common.layer.clone();
        match &entity.specific {
            EntityType::Circle(c) => {
                let center = [c.center.x, c.center.y];
                let mut curve =
                    Curve2D::new(vec![center], layer.clone()).with_style(0.0, [0, 0, 0]);
                curve.analytic = Some(Analytic2D::Circle {
                    center,
                    radius: c.radius,
                });
                out.push_curve(curve);
            }
            EntityType::Arc(a) => {
                let center = [a.center.x, a.center.y];
                let mut curve =
                    Curve2D::new(vec![center], layer.clone()).with_style(0.0, [0, 0, 0]);
                curve.analytic = Some(Analytic2D::Arc {
                    center,
                    radius: a.radius,
                    // dxf crate 的角度单位是度 → 统一转弧度
                    start_angle_rad: a.start_angle.to_radians(),
                    end_angle_rad: a.end_angle.to_radians(),
                });
                out.push_curve(curve);
            }
            EntityType::Line(l) => {
                let pts = vec![[l.p1.x, l.p1.y], [l.p2.x, l.p2.y]];
                out.push_curve(Curve2D::new(pts, layer.clone()).with_style(0.0, [0, 0, 0]));
            }
            EntityType::LwPolyline(pl) => {
                let pts: Vec<[f64; 2]> = pl.vertices.iter().map(|v| [v.x, v.y]).collect();
                if pts.len() < 2 {
                    continue;
                }
                let mut curve = Curve2D::new(pts, layer.clone()).with_style(0.0, [0, 0, 0]);
                // flags bit 0 = 1 表示闭合
                if pl.flags & 1 != 0 {
                    curve = curve.closed();
                }
                out.push_curve(curve);
            }
            EntityType::Text(t) => {
                out.push_text(
                    Text2D::new([t.location.x, t.location.y], &t.value, t.text_height)
                        .on_layer(layer.clone()),
                );
            }
            other => {
                // 兼容子集之外的实体：warn 跳过，不整包失败
                warnings.push(format!(
                    "DXF 实体 {} 未支持，已跳过",
                    entity_type_name(other)
                ));
            }
        }
    }

    Ok((out, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一段最小 DXF（R2000，含 LINE / CIRCLE / ARC）。
    fn minimal_dxf_bytes() -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1015\n0\nENDSEC\n");
        out.extend_from_slice(b"0\nSECTION\n2\nENTITIES\n");
        out.extend_from_slice(b"0\nLINE\n8\n0\n10\n0.0\n20\n0.0\n11\n100.0\n21\n0.0\n");
        out.extend_from_slice(b"0\nCIRCLE\n8\n0\n10\n50.0\n20\n50.0\n40\n25.0\n");
        out.extend_from_slice(b"0\nARC\n8\n0\n10\n0.0\n20\n0.0\n40\n10.0\n50\n30.0\n51\n90.0\n");
        out.extend_from_slice(b"0\nENDSEC\n0\nEOF\n");
        out
    }

    #[test]
    fn minimal_dxf_roundtrips_with_circle_and_arc() {
        let bytes = minimal_dxf_bytes();
        let (drawing, warnings) = read_dxf(&bytes).unwrap();
        assert!(warnings.is_empty(), "兼容子集内不应有 warn");
        // LINE + CIRCLE + ARC = 3 curves
        assert_eq!(drawing.curves.len(), 3);
        // 检查 LINE
        assert!(drawing.curves[0].analytic.is_none());
        assert_eq!(drawing.curves[0].points.len(), 2);
        // 检查 CIRCLE
        assert!(drawing.curves[1].as_circle().is_some());
        let (center, radius) = drawing.curves[1].as_circle().unwrap();
        assert!((center[0] - 50.0).abs() < 1e-9);
        assert!((center[1] - 50.0).abs() < 1e-9);
        assert!((radius - 25.0).abs() < 1e-9);
        // 检查 ARC
        assert!(drawing.curves[2].as_arc().is_some());
        let (_c, r, s, e) = drawing.curves[2].as_arc().unwrap();
        assert!((r - 10.0).abs() < 1e-9);
        assert!(
            (s - 30.0_f64.to_radians()).abs() < 1e-9,
            "起始角应为 30°（加强：非 0）"
        );
        assert!((e - 90.0_f64.to_radians()).abs() < 1e-9);
    }

    #[test]
    fn garbage_input_returns_parse_error() {
        assert!(read_dxf(b"not a dxf file").is_err());
    }

    #[test]
    fn empty_dxf_returns_empty_drawing() {
        let bytes = b"0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1015\n0\nENDSEC\n0\nEOF\n";
        let (drawing, warnings) = read_dxf(bytes).unwrap();
        assert!(drawing.curves.is_empty());
        assert!(warnings.is_empty());
    }

    /// B3-8 钉子：自写 to_dxf → read_dxf 的交叉校验，圆数守恒。
    #[test]
    fn dxf_import_roundtrip_preserves_circle_count() {
        let mut d = Drawing2D::new(100.0, 100.0);
        d.push_curve({
            let mut c = Curve2D::new(vec![[0.0, 0.0]], "L");
            c.analytic = Some(Analytic2D::Circle {
                center: [0.0, 0.0],
                radius: 5.0,
            });
            c
        });
        d.push_curve({
            let mut c = Curve2D::new(vec![[10.0, 0.0]], "L");
            c.analytic = Some(Analytic2D::Circle {
                center: [10.0, 0.0],
                radius: 8.0,
            });
            c
        });
        d.push_curve(Curve2D::new(vec![[0.0, 0.0], [1.0, 1.0]], "L"));

        let out = crate::dxf::to_dxf(&d);
        assert_eq!(out.circle_count, 2);
        let (read_back, warnings) = read_dxf(out.text.as_bytes()).unwrap();
        assert!(warnings.is_empty());
        let circles = read_back
            .curves
            .iter()
            .filter(|c| c.as_circle().is_some())
            .count();
        assert_eq!(circles, 2, "roundtrip 后圆数应守恒");
        assert_eq!(read_back.curves.len(), 3, "LINE + 2×CIRCLE");
    }

    /// B3-8 钉子：导入的弧是解析实体且角度为弧度。
    #[test]
    fn dxf_import_of_arc_is_analytic() {
        // 用自己的导出器生成含 ARC 的 DXF（start π/6 rad, end π/2 rad）
        let mut d = Drawing2D::new(100.0, 100.0);
        d.push_curve({
            let mut c = Curve2D::new(vec![[5.0, 5.0]], "L");
            c.analytic = Some(Analytic2D::Arc {
                center: [5.0, 5.0],
                radius: 3.0,
                start_angle_rad: std::f64::consts::FRAC_PI_6,
                end_angle_rad: std::f64::consts::FRAC_PI_2,
            });
            c
        });
        let out = crate::dxf::to_dxf(&d);
        assert_eq!(out.arc_count, 1);

        let (read_back, _warnings) = read_dxf(out.text.as_bytes()).unwrap();
        let arc = read_back
            .curves
            .iter()
            .find_map(|c| c.as_arc())
            .expect("应读回解析弧");
        let (_c, r, s, e) = arc;
        assert!((r - 3.0).abs() < 1e-9);
        assert!(
            (s - std::f64::consts::FRAC_PI_6).abs() < 1e-9,
            "起始角应为 π/6 rad（加强：非 0）"
        );
        assert!(
            (e - std::f64::consts::FRAC_PI_2).abs() < 1e-9,
            "终止角应为 π/2 rad"
        );
    }

    /// B3-8 钉子：未支持实体 warn 但不整包失败，已知图元照常导入。
    #[test]
    fn dxf_import_unknown_entity_warns_but_continues() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1015\n0\nENDSEC\n");
        bytes.extend_from_slice(b"0\nSECTION\n2\nENTITIES\n");
        bytes.extend_from_slice(b"0\nLINE\n8\n0\n10\n0.0\n20\n0.0\n11\n100.0\n21\n0.0\n");
        // ELLIPSE 不在兼容子集里 → 应 warn 跳过
        bytes.extend_from_slice(
            b"0\nELLIPSE\n8\n0\n10\n0.0\n20\n0.0\n11\n10.0\n21\n0.0\n210\n0.0\n220\n0.0\n230\n1.0\n40\n0.5\n41\n0.0\n42\n6.283185307179586\n",
        );
        bytes.extend_from_slice(b"0\nENDSEC\n0\nEOF\n");

        let (drawing, warnings) = read_dxf(&bytes).unwrap();
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("未支持") && w.contains("Ellipse")),
            "未支持实体应 warn，实得：{warnings:?}"
        );
        assert_eq!(drawing.curves.len(), 1, "LINE 应照常导入");
    }
}

//! 具体导出器实现（均为**零新依赖**的自写序列化）。
//!
//! | 实现 | 状态 | 依赖 |
//! |---|---|---|
//! | [`SvgExporter`] | ✅ 可用 | 无（手写 XML；M5 接 `kurbo` 补曲线细分）|
//! | [`DxfExporter`] | ✅ 可用 | 无（手写 ASCII DXF 兼容子集；圆/弧写解析实体）|
//! | STEP 导出 | ⏳ 推迟 | `occt-wasm` 原生，**不在此层**（推迟至 OCCT sidecar 里程碑：上游 4.x 构建形态缺陷，文件系统未编入产物，host 侧不可修）|
//!
//! **STEP 为什么没有导出器实现**：它不需要手写——`occt-wasm` 自己就能把
//! B-Rep 写成 STEP。手写一遍只会引入格式偏差。所以 STEP 导出走
//! `core-app` → `core-kernel`，不经过本模块的 [`Exporter`] 抽象。

use crate::capability::Format;
use crate::drawing2d::Drawing2D;
use crate::error::{InteropError, InteropResult};
use crate::flow::Exporter;
use crate::{dxf, svg};

/// SVG 导出器（二维出图的通用出口：网页预览、打印、贴进文档）。
pub struct SvgExporter;

impl Exporter for SvgExporter {
    fn format(&self) -> Format {
        Format::Svg
    }

    fn export(&self, drawing: &Drawing2D) -> InteropResult<Vec<u8>> {
        if drawing.width_mm <= 0.0 || drawing.height_mm <= 0.0 {
            return Err(InteropError::Write(format!(
                "图幅尺寸非法：{}×{} mm",
                drawing.width_mm, drawing.height_mm
            )));
        }
        Ok(svg::to_svg(drawing).text.into_bytes())
    }
}

/// DXF 导出器（与存量 CAD 工具链对接）。
pub struct DxfExporter;

impl Exporter for DxfExporter {
    fn format(&self) -> Format {
        Format::Dxf
    }

    fn export(&self, drawing: &Drawing2D) -> InteropResult<Vec<u8>> {
        if drawing.width_mm <= 0.0 || drawing.height_mm <= 0.0 {
            return Err(InteropError::Write(format!(
                "图幅尺寸非法：{}×{} mm",
                drawing.width_mm, drawing.height_mm
            )));
        }
        Ok(dxf::to_dxf(drawing).text.into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawing2d::{Curve2D, Text2D};

    fn sample() -> Drawing2D {
        let mut d = Drawing2D::new(420.0, 297.0);
        d.push_curve(Curve2D::new(
            vec![[10.0, 10.0], [200.0, 10.0]],
            "OUTLINE-可见轮廓",
        ));
        d.push_text(Text2D::new([10.0, 20.0], "一层平面图", 7.0));
        d
    }

    #[test]
    fn svg_exporter_produces_utf8_bytes() {
        let out = SvgExporter.export(&sample()).unwrap();
        let text = std::str::from_utf8(&out).unwrap();
        assert!(text.contains("<svg"));
        assert!(text.contains("一层平面图"));
        assert_eq!(SvgExporter.format(), Format::Svg);
    }

    #[test]
    fn dxf_exporter_produces_utf8_bytes() {
        let out = DxfExporter.export(&sample()).unwrap();
        let text = std::str::from_utf8(&out).unwrap();
        assert!(text.contains("AC1015"));
        assert!(text.trim_end().ends_with("EOF"));
        assert_eq!(DxfExporter.format(), Format::Dxf);
    }

    #[test]
    fn invalid_sheet_size_is_rejected_for_both_exporters() {
        let bad = Drawing2D::new(0.0, 297.0);
        assert!(matches!(
            SvgExporter.export(&bad),
            Err(InteropError::Write(_))
        ));
        assert!(matches!(
            DxfExporter.export(&bad),
            Err(InteropError::Write(_))
        ));
    }

    #[test]
    fn exporters_are_object_safe_for_the_router() {
        let exporters: Vec<Box<dyn Exporter>> = vec![Box::new(SvgExporter), Box::new(DxfExporter)];
        let formats: Vec<Format> = exporters.iter().map(|e| e.format()).collect();
        assert_eq!(formats, vec![Format::Svg, Format::Dxf]);
    }
}

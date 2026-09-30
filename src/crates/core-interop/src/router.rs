//! 格式路由器 —— 本层的总入口，也是**合规门禁的落点**。
//!
//! ## 两件事
//!
//! 1. **按格式分发**：嗅探 → 找实现 → 调用。加格式不动这里的代码。
//! 2. **拦截违规调用**：任何 `isolation == SeparateProcess` 的格式
//!    （当前只有 DWG），**在分发之前就被挡掉**。这一层是**冗余防线**——
//!    [`crate::readers::DwgImporter`] 自己也会拒，但路由层再挡一道，
//!    意味着将来就算有人注册了一个"真能读 DWG"的实现，也照样过不去。
//!
//! ```text
//! import(filename, bytes)
//!   ├─ guard_isolation(detected)  ← 红线：DWG 到此为止
//!   ├─ detect(filename, bytes)    ← 认不出 → UnknownFormat
//!   ├─ find importer              ← 没有 → Unsupported
//!   └─ importer.import(...)
//! ```

use crate::capability::{Format, Isolation, Support};
use crate::detect::detect;
use crate::drawing2d::Drawing2D;
use crate::error::{InteropError, InteropResult};
use crate::flow::{ExportOutput, Exporter, ImportContext, ImportReport, Importer};
use crate::readers::{DwgImporter, DxfImporter, GltfImporter, IfcImporter, StepImporter};
use crate::writers::{DxfExporter, SvgExporter};

/// 格式路由器。
#[derive(Default)]
pub struct FormatRouter {
    importers: Vec<Box<dyn Importer>>,
    exporters: Vec<Box<dyn Exporter>>,
}

impl FormatRouter {
    /// 空的（不给它注册任何实现就什么也干不了）。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// **内置实现全注册**：STEP / glTF / DXF / IFC / DWG 导入 + SVG / DXF 导出。
    #[must_use]
    pub fn with_builtin() -> Self {
        let mut r = Self::new();
        r.register_importer(Box::new(StepImporter));
        r.register_importer(Box::new(GltfImporter));
        r.register_importer(Box::new(DxfImporter));
        r.register_importer(Box::new(IfcImporter));
        r.register_importer(Box::new(DwgImporter)); // 只会拒绝，但要出现在"认识的格式"里
        r.register_exporter(Box::new(SvgExporter));
        r.register_exporter(Box::new(DxfExporter));
        r
    }

    /// 注册一个导入器（后注册的优先被选中）。
    pub fn register_importer(&mut self, importer: Box<dyn Importer>) {
        self.importers.insert(0, importer);
    }

    /// 注册一个导出器（后注册的优先被选中）。
    pub fn register_exporter(&mut self, exporter: Box<dyn Exporter>) {
        self.exporters.insert(0, exporter);
    }

    /// 已注册的导入格式（去重后按枚举顺序）。
    #[must_use]
    pub fn import_formats(&self) -> Vec<Format> {
        let mut v: Vec<Format> = self.importers.iter().map(|i| i.format()).collect();
        v.sort();
        v.dedup();
        v
    }

    /// 已注册的导出格式。
    #[must_use]
    pub fn export_formats(&self) -> Vec<Format> {
        let mut v: Vec<Format> = self.exporters.iter().map(|e| e.format()).collect();
        v.sort();
        v.dedup();
        v
    }

    /// **合规门禁**：该格式是否被允许在本进程内处理。
    fn guard_isolation(format: Format) -> InteropResult<()> {
        let spec = format.spec();
        if spec.isolation == Isolation::SeparateProcess {
            return Err(InteropError::RequiresIsolation {
                format,
                library: spec.library,
                license: spec.license,
            });
        }
        Ok(())
    }

    /// 导入一份数据。
    ///
    /// `filename` 只用于嗅探兜底与报错信息；`source` 是完整文件内容。
    pub fn import(&self, filename: &str, source: &[u8]) -> InteropResult<ImportReport> {
        let Some(format) = detect(filename, source) else {
            return Err(InteropError::UnknownFormat(filename.to_string()));
        };

        // 红线先于分发
        Self::guard_isolation(format)?;

        let importer = self
            .importers
            .iter()
            .find(|i| i.format() == format && i.can_import(source));

        match importer {
            Some(i) => {
                let mut ctx = ImportContext::new(filename);
                let report = i.import(source, &mut ctx)?;
                Ok(report)
            }
            None => Err(InteropError::Unsupported {
                format,
                direction: "导入",
            }),
        }
    }

    /// 导出为指定格式。
    pub fn export(&self, format: Format, drawing: &Drawing2D) -> InteropResult<ExportOutput> {
        Self::guard_isolation(format)?;

        let exporter = self.exporters.iter().find(|e| e.format() == format);
        match exporter {
            Some(e) => Ok(ExportOutput {
                bytes: e.export(drawing)?,
                format,
            }),
            None => Err(InteropError::Unsupported {
                format,
                direction: "导出",
            }),
        }
    }

    /// 某格式在本阶段是否"可用"（能导入或能导出，且不受隔离限制）。
    ///
    /// ⚠ 这里**同时**查两样东西，缺一不可（QA P1-1 订正）：
    /// 1. **路由器注册表**——有没有实现；
    /// 2. **能力声明**（[`Format::spec`]）——该格式是否**任一方向进程内已实现**
    ///    （`Support::Native`）。
    ///
    /// 只查注册表会漏判：`StepImporter` **注册在案**（为了让 STEP 出现在"认识的格式"
    /// 里并给出明确推迟说明），但它的 `import` 恒 `Err(Unsupported)`、能力声明也是
    /// `NotPlanned` → 若只看注册表就会把 STEP 误判为"可用"，与声明层打架。
    #[must_use]
    pub fn is_format_usable(&self, format: Format) -> bool {
        if format.requires_isolation() {
            return false;
        }
        let spec = format.spec();
        let declared_in_process = spec.import == Support::Native || spec.export == Support::Native;
        declared_in_process
            && (self.import_formats().contains(&format) || self.export_formats().contains(&format))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawing2d::{Curve2D, Text2D};

    const STEP: &[u8] = b"ISO-10303-21;\nHEADER;\nFILE_NAME('a.step','',(''),(''),'','','');\nFILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n";
    const IFC: &[u8] = b"ISO-10303-21;\nHEADER;\nFILE_NAME('a.ifc','',(''),(''),'','','');\nFILE_SCHEMA(('IFC4X3'));\nENDSEC;\nDATA;\n";

    #[test]
    fn builtin_router_knows_seven_formats_overall() {
        let r = FormatRouter::with_builtin();
        assert_eq!(r.import_formats().len(), 5, "STEP/glTF/DXF/IFC/DWG 导入器");
        assert_eq!(r.export_formats().len(), 2, "SVG/DXF 导出器");
    }

    #[test]
    fn step_is_dispatched_to_the_step_importer_and_reports_unsupported() {
        // STEP 仍注册在案（要给出"推迟至 OCCT sidecar"的明确说明），
        // 但 M4 不交付其读写 → 必须显式 Unsupported，且错误**来自 StepImporter**
        // （direction 带 sidecar 字样），以区别于"根本没有导入器"。
        let r = FormatRouter::with_builtin();
        let err = r.import("a.step", STEP).unwrap_err();
        match err {
            InteropError::Unsupported { format, direction } => {
                assert_eq!(format, Format::Step);
                assert!(
                    direction.contains("OCCT sidecar"),
                    "必须由 StepImporter 给出推迟说明（证明确实分发到了它），实得：{direction}"
                );
            }
            other => panic!("应为显式 Unsupported，实得 {other:?}"),
        }
    }

    #[test]
    fn an_empty_router_reports_unsupported_without_the_step_importer_message() {
        // 对照组：空路由器认得出 STEP，但**没有** StepImporter →
        // 错误 direction 是笼统的"导入"（不含 sidecar 字样）。
        // 这条与上一条共同钉住"分发到 StepImporter"与"没有导入器"可区分。
        let r = FormatRouter::new();
        let err = r.import("a.step", STEP).unwrap_err();
        match err {
            InteropError::Unsupported { direction, .. } => {
                assert!(
                    !direction.contains("OCCT sidecar"),
                    "没有导入器时不应出现 StepImporter 的专用说明，实得：{direction}"
                );
            }
            other => panic!("应为 Unsupported，实得 {other:?}"),
        }
    }

    #[test]
    fn import_dispatches_ifc_by_header_not_extension() {
        let r = FormatRouter::with_builtin();
        // 文件名故意写错扩展名，仍应按 FILE_SCHEMA 判成 IFC
        let report = r.import("wrong_name.step", IFC).unwrap();
        assert_eq!(report.format, Format::Ifc4x3);
    }

    #[test]
    fn unknown_format_is_reported_with_the_filename() {
        let r = FormatRouter::with_builtin();
        let err = r.import("mystery.bin", b"\x01\x02").unwrap_err();
        match err {
            InteropError::UnknownFormat(name) => assert_eq!(name, "mystery.bin"),
            other => panic!("应为 UnknownFormat，实得 {other:?}"),
        }
    }

    #[test]
    fn dwg_is_blocked_by_the_router_guard_before_dispatch() {
        let r = FormatRouter::with_builtin();
        let mut dwg = b"AC1015".to_vec();
        dwg.extend_from_slice(&[0, 0, 0, 1]);
        let err = r.import("legacy.dwg", &dwg).unwrap_err();
        assert!(
            matches!(err, InteropError::RequiresIsolation { .. }),
            "DWG 必须在分发前被路由层拦下，实得 {err:?}"
        );
    }

    #[test]
    fn export_svg_and_dxf_work_end_to_end() {
        let r = FormatRouter::with_builtin();
        let mut d = Drawing2D::new(420.0, 297.0);
        d.push_curve(Curve2D::new(vec![[0.0, 0.0], [100.0, 0.0]], "OUTLINE"));
        d.push_text(Text2D::new([10.0, 10.0], "平面图", 5.0));

        let svg = r.export(Format::Svg, &d).unwrap();
        assert_eq!(svg.format, Format::Svg);
        assert!(svg.as_text().unwrap().contains("<svg"));

        let dxf = r.export(Format::Dxf, &d).unwrap();
        assert!(dxf.as_text().unwrap().contains("ENTITIES"));
    }

    #[test]
    fn exporting_an_unregistered_format_is_unsupported() {
        let r = FormatRouter::with_builtin();
        let err = r
            .export(Format::Gim, &Drawing2D::new(210.0, 297.0))
            .unwrap_err();
        assert!(matches!(err, InteropError::Unsupported { .. }));
    }

    #[test]
    fn exporting_dwg_is_blocked_by_the_isolation_guard() {
        let r = FormatRouter::with_builtin();
        let err = r
            .export(Format::Dwg, &Drawing2D::new(210.0, 297.0))
            .unwrap_err();
        assert!(matches!(err, InteropError::RequiresIsolation { .. }));
    }

    #[test]
    fn importing_unrecognizable_content_is_unknown_format() {
        // `detect()` 基于内容魔数 / 文本特征 / 扩展名，与路由器注册表无关。
        // 三条线索都认不出 → UnknownFormat（区别于"认得格式但没有导入器"）。
        let r = FormatRouter::new();
        assert!(matches!(
            r.import("mystery.bin", b"not a cad file at all"),
            Err(InteropError::UnknownFormat(_))
        ));
    }

    #[test]
    fn an_empty_router_knows_the_format_but_has_no_importer() {
        // 内容嗅探认得出 STEP，但空路由器没注册任何导入器 → Unsupported。
        // 这两条常被混为一谈，故各写一个测试钉住区别。
        let r = FormatRouter::new();
        assert!(matches!(
            r.import("a.step", STEP),
            Err(InteropError::Unsupported { .. })
        ));
    }

    #[test]
    fn a_router_without_importers_reports_unsupported_for_a_known_format() {
        // 只注册导出器，不注册导入器
        let mut r = FormatRouter::new();
        r.register_exporter(Box::new(SvgExporter));
        assert!(matches!(
            r.import("a.step", STEP),
            Err(InteropError::Unsupported { .. })
        ));
    }

    #[test]
    fn usability_query_reflects_the_compliance_rules() {
        let r = FormatRouter::with_builtin();
        // DXF / SVG / IFC / glTF 是 M4 真实交付（能力声明 Native）→ 可用
        assert!(r.is_format_usable(Format::Dxf));
        assert!(r.is_format_usable(Format::Svg));
        assert!(r.is_format_usable(Format::Ifc4x3));
        assert!(r.is_format_usable(Format::Gltf));
        // STEP 虽注册在案，但能力声明 NotPlanned 且 import 恒 Err → 不可用
        // （QA P1-1：注册表 ≠ 可用，必须同时看能力声明，否则与声明层打架）
        assert!(
            !r.is_format_usable(Format::Step),
            "STEP 未交付（声明 NotPlanned），不得判为可用"
        );
        assert!(!r.is_format_usable(Format::Dwg), "DWG 不受隔离则不可用");
        assert!(!r.is_format_usable(Format::Gim), "GIM 尚未注册");
    }

    #[test]
    fn last_registered_importer_wins() {
        /// 一个假实现，用来验证"后注册优先"。
        struct Fake;
        impl Importer for Fake {
            fn format(&self) -> Format {
                Format::Step
            }
            fn import(
                &self,
                _source: &[u8],
                _ctx: &mut ImportContext,
            ) -> InteropResult<ImportReport> {
                let mut r = ImportReport::new(Format::Step);
                r.warn("fake importer was used");
                Ok(r)
            }
        }

        let mut router = FormatRouter::with_builtin();
        router.register_importer(Box::new(Fake));
        let report = router.import("a.step", STEP).unwrap();
        assert!(report.warnings.iter().any(|w| w.contains("fake")));
    }
}

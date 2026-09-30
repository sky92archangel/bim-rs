//! 导入 / 导出的契约与产物（设计文档 7.2 / 7.3）。
//!
//! [`Importer`] / [`Exporter`] 是本层的**唯一扩展点**：加一个格式 = 加一个
//! 实现 + 在 [`crate::FormatRouter`] 里注册，**不改任何其它代码**。
//!
//! ## 导入产物的形状
//!
//! 一次导入产出两样东西：
//! 1. **几何**（[`ImportedShape`]）—— 落成 `core-model` 的
//!    [`GeometryRecord`]（BREP / STEP 文本 + 持久 `ShapeId`），
//!    与内核句柄无关。**惰性重建**：真正要算的时候才经 `core-kernel` 读回
//!    （设计文档 3.3）。
//! 2. **元素**（[`core_model::Element`]）—— 语义对象。
//!
//! 二者分开正是设计文档 4.1 的核心：**元素与几何不是一回事**。

use core_model::{Document, Element, GeomFormat, ShapeId};
use serde::{Deserialize, Serialize};

use crate::capability::Format;
use crate::error::{InteropError, InteropResult};

/// 导入上下文。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportContext {
    /// 源文件名（报错信息用）
    pub source_name: String,
    /// 单位换算系数：**源文件单位 → 毫米**。
    ///
    /// STEP 的 `SI_UNIT` 表、glTF 的 `asset.extras`、DXF 的 `$INSUNITS`
    /// 都可能给出非毫米单位，导入时必须换算到位（本项目内部一律 mm）。
    pub unit_to_mm: f64,
    /// 缝合 / 去重容差（mm），默认取内核的集中容差值。
    pub tolerance: f64,
    /// 是否导入隐藏线（一般不导入——出图时再算）
    pub import_hidden: bool,
}

impl ImportContext {
    /// 默认上下文（毫米、内核默认容差、不导隐藏线）。
    #[must_use]
    pub fn new(source_name: impl Into<String>) -> Self {
        Self {
            source_name: source_name.into(),
            unit_to_mm: 1.0,
            tolerance: core_kernel::tolerance::POINT_COINCIDENT,
            import_hidden: false,
        }
    }

    /// 指定单位换算（链式）。
    #[must_use]
    pub fn with_unit_scale(mut self, unit_to_mm: f64) -> Self {
        self.unit_to_mm = unit_to_mm;
        self
    }
}

/// 导入出来的一份几何。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportedShape {
    /// 分配好的持久几何 id
    pub shape_id: ShapeId,
    /// 存成什么格式（决定写进 `geom/<shapeid>.brep` 还是 `.step`）
    pub format: GeomFormat,
    /// 几何正文
    pub text: String,
    /// 来源描述（如 STEP 里的实体名），报错与追溯用
    pub source: String,
}

impl ImportedShape {
    /// 一份 BREP 文本几何。
    #[must_use]
    pub fn brep(source: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            shape_id: ShapeId::new(),
            format: GeomFormat::BrepText,
            text: text.into(),
            source: source.into(),
        }
    }

    /// 一份 STEP 文本几何。
    #[must_use]
    pub fn step(source: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            shape_id: ShapeId::new(),
            format: GeomFormat::Step,
            text: text.into(),
            source: source.into(),
        }
    }
}

/// 一次导入的产物。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportReport {
    /// 来源格式
    pub format: Format,
    /// 几何
    pub shapes: Vec<ImportedShape>,
    /// 语义元素
    pub elements: Vec<Element>,
    /// 元信息（键值对，如 STEP 头里的作者、时间戳）
    pub metadata: std::collections::BTreeMap<String, String>,
    /// 警告（不阻断导入，但用户必须看得到）
    pub warnings: Vec<String>,
}

impl ImportReport {
    /// 空报告。
    #[must_use]
    pub fn new(format: Format) -> Self {
        Self {
            format,
            shapes: Vec::new(),
            elements: Vec::new(),
            metadata: std::collections::BTreeMap::new(),
            warnings: Vec::new(),
        }
    }

    /// 记一条元信息。
    pub fn note(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.metadata.insert(key.into(), value.into());
    }

    /// 记一条警告。
    pub fn warn(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }

    /// 几何数。
    #[must_use]
    pub fn shape_count(&self) -> usize {
        self.shapes.len()
    }

    /// 元素数。
    #[must_use]
    pub fn element_count(&self) -> usize {
        self.elements.len()
    }

    /// 是否什么都没导入进来。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty() && self.elements.is_empty()
    }

    /// 把产物**并入**一个文档（供 `core-app` 的事务用）。
    ///
    /// 全部插入成功才返回；任一失败由调用方的事务负责回滚。
    pub fn apply_to(&self, doc: &mut Document) -> InteropResult<()> {
        for shape in &self.shapes {
            doc.insert_geometry(core_model::GeometryRecord {
                shape_id: shape.shape_id,
                format: shape.format,
                text: shape.text.clone(),
            })?;
        }
        for element in &self.elements {
            doc.insert_element(element.clone())?;
        }
        Ok(())
    }
}

/// 格式导入器。
pub trait Importer: Send + Sync {
    /// 本实现负责哪个格式。
    fn format(&self) -> Format;

    /// 用文件头**快速判断**这个实现能不能吃这份数据。
    ///
    /// 默认按 [`crate::detect::detect`] 的结果比对；实现可覆写为更强的前置检查。
    fn can_import(&self, head: &[u8]) -> bool {
        crate::detect::detect("", head) == Some(self.format())
    }

    /// 执行导入。
    fn import(&self, source: &[u8], ctx: &mut ImportContext) -> InteropResult<ImportReport>;
}

/// 格式导出器。
pub trait Exporter: Send + Sync {
    /// 本实现负责哪个格式。
    fn format(&self) -> Format;

    /// 导出。
    fn export(&self, drawing: &crate::drawing2d::Drawing2D) -> InteropResult<Vec<u8>>;
}

/// 把导出结果包成"文本 + 统计"，便于命令行与测试断言。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportOutput {
    /// 输出字节
    pub bytes: Vec<u8>,
    /// 输出格式
    pub format: Format,
}

impl ExportOutput {
    /// 以 UTF-8 取正文（DXF / SVG 等文本格式用）。
    pub fn as_text(&self) -> InteropResult<&str> {
        std::str::from_utf8(&self.bytes)
            .map_err(|e| InteropError::Write(format!("输出不是合法 UTF-8：{e}")))
    }

    /// 字节数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// 是否为空输出。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::{Category, GeometryRecord};

    #[test]
    fn default_context_is_millimetre_and_uses_kernel_tolerance() {
        let ctx = ImportContext::new("a.step");
        assert_eq!(ctx.unit_to_mm, 1.0);
        assert_eq!(ctx.tolerance, core_kernel::tolerance::POINT_COINCIDENT);
        assert!(!ctx.import_hidden);
    }

    #[test]
    fn unit_scale_can_be_overridden_for_inch_files() {
        // 英制 DXF：1 in = 25.4 mm
        let ctx = ImportContext::new("inch.dxf").with_unit_scale(25.4);
        assert!((ctx.unit_to_mm - 25.4).abs() < 1e-12);
    }

    #[test]
    fn empty_report_is_empty() {
        let r = ImportReport::new(Format::Step);
        assert!(r.is_empty());
        assert_eq!(r.shape_count(), 0);
        assert_eq!(r.element_count(), 0);
    }

    #[test]
    fn report_collects_metadata_and_warnings() {
        let mut r = ImportReport::new(Format::Ifc4x3);
        r.note("schema", "IFC4X3");
        r.warn("未解析 IfcSpace");
        assert_eq!(r.metadata.get("schema").map(String::as_str), Some("IFC4X3"));
        assert_eq!(r.warnings.len(), 1);
    }

    #[test]
    fn apply_to_inserts_geometry_and_elements() {
        let mut doc = Document::new("d");
        let mut r = ImportReport::new(Format::Step);
        r.shapes
            .push(ImportedShape::brep("box", "CASCADE Topology V3"));
        r.elements
            .push(Element::new(Category::generic(), "导入构件"));

        r.apply_to(&mut doc).unwrap();
        assert_eq!(doc.element_count(), 1);
        assert_eq!(doc.geometry_count(), 1);
    }

    #[test]
    fn apply_to_propagates_duplicate_id_conflicts() {
        let mut doc = Document::new("d");
        let mut r = ImportReport::new(Format::Step);
        r.shapes.push(ImportedShape::brep("a", "x"));
        r.apply_to(&mut doc).unwrap();
        // 同一份报告再并一次 → 几何 id 冲突，必须报错而不是静默覆盖
        assert!(r.apply_to(&mut doc).is_err());
    }

    #[test]
    fn imported_shape_records_its_format() {
        let b = ImportedShape::brep("s", "t");
        assert_eq!(b.format, GeomFormat::BrepText);
        let s = ImportedShape::step("s", "t");
        assert_eq!(s.format, GeomFormat::Step);
        assert_ne!(b.shape_id, s.shape_id);
    }

    #[test]
    fn export_output_exposes_text_for_textual_formats() {
        let out = ExportOutput {
            bytes: b"<svg/>".to_vec(),
            format: Format::Svg,
        };
        assert_eq!(out.as_text().unwrap(), "<svg/>");
        assert_eq!(out.len(), 6);
        assert!(!out.is_empty());
    }

    #[test]
    fn report_roundtrips_through_json() {
        let mut r = ImportReport::new(Format::Dxf);
        r.shapes.push(ImportedShape::brep("a", "xyz"));
        r.note("k", "v");
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<ImportReport>(&json).unwrap(), r);
    }

    #[test]
    fn document_type_is_usable_from_this_layer() {
        // 编译期确认：本层可以直接构造 GeometryRecord 并塞进文档
        let rec = GeometryRecord::brep(ShapeId::new(), "t");
        let mut doc = Document::new("d");
        doc.insert_geometry(rec).unwrap();
        assert_eq!(doc.geometry_count(), 1);
    }
}

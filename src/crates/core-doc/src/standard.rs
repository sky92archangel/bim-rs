//! 制图标准（设计文档 7.7 的**语义层**）—— 能力④里最重的一段。
//!
//! ## 为什么这一节是"规则"而不是"格式"
//!
//! 图幅、比例、线型线宽、图层、文字样式、会签栏、图纸集**都不是文件格式**，
//! 而是**制图规则**。换成 DXF 还是 PDF 导出，这些规则不变。
//! 所以它没有库可买，只能自建成**可配置规则引擎**——这就是 [`DraftingStandard`]。
//!
//! ## 三层划分（与设计文档 7.7 一致）
//!
//! | 层 | 内容 | 归属 |
//! |---|---|---|
//! | 几何层 | 曲线、点、投影矩阵 | `core-kernel`（`project_edges`）+ `core-doc` |
//! | 绘制层 | 线、弧、填充 → 屏幕 / SVG | `kurbo` / `lyon`（零自研）|
//! | **语义层** | **图层 / 线型 / 线宽 / 图幅 / 比例 / 文字样式** | **本模块** |
//!
//! **内置 ≠ 写死**：出厂带一套国标工业厂房默认值，但每个值都可被院标 / 项目覆盖。

use core_model::LayerId;
use serde::{Deserialize, Serialize};

use crate::entity::EntityKind;
use crate::error::{DocError, DocResult};

// ---------- 图层名常量 ----------
//
// 用常量而非散落的字符串字面量：改名只需改一处，出图时按名找图层才不会找错。

/// 图层名：轴网
pub const LAYER_AXIS: &str = "AXIS-轴网";
/// 图层名：可见轮廓
pub const LAYER_OUTLINE: &str = "OUTLINE-可见轮廓";
/// 图层名：不可见轮廓（虚线）
pub const LAYER_HIDDEN: &str = "HIDDEN-不可见轮廓";
/// 图层名：标注
pub const LAYER_DIM: &str = "DIM-标注";
/// 图层名：文字
pub const LAYER_TEXT: &str = "TEXT-文字";
/// 图层名：填充 / 剖面线
pub const LAYER_HATCH: &str = "HATCH-填充";
/// 图层名：电缆桥架（示例专业图层，由模块经 SDK 追加）
pub const LAYER_CABLE_TRAY: &str = "TRAY-电缆桥架";

/// RGB 颜色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgb {
    /// 红
    pub r: u8,
    /// 绿
    pub g: u8,
    /// 蓝
    pub b: u8,
}

impl Rgb {
    /// 由三分量构造。
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// 白（国标图纸底色为黑，白线用于黑底屏幕显示）。
    pub const WHITE: Self = Self::new(255, 255, 255);
    /// 黑。
    pub const BLACK: Self = Self::new(0, 0, 0);
    /// 红（国标：标注常用红）。
    pub const RED: Self = Self::new(255, 0, 0);
    /// 黄。
    pub const YELLOW: Self = Self::new(255, 255, 0);
    /// 绿。
    pub const GREEN: Self = Self::new(0, 255, 0);
    /// 青。
    pub const CYAN: Self = Self::new(0, 255, 255);
    /// 洋红。
    pub const MAGENTA: Self = Self::new(255, 0, 255);
}

/// 线型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LineType {
    /// 实线（连续）
    Continuous,
    /// 虚线
    Dashed,
    /// 点划线（轴线、中心线）
    DashDot,
    /// 双点划线（假想投影轮廓线）
    DoubleDashDot,
    /// 点线
    Dotted,
}

impl LineType {
    /// 是否为国标允许用于"不可见轮廓"的线型。
    #[must_use]
    pub const fn is_dashed_family(self) -> bool {
        matches!(self, Self::Dashed | Self::Dotted)
    }

    /// 线型名（**导出用的稳定名**）。
    ///
    /// 这个名字是三层之间的**契约字符串**：
    /// `core-doc` 出这个名字 → `core-interop` 的 SVG / DXF 序列化器认它。
    /// 改名会同时打断两边，所以这里是 `const` 且不允许局部特判。
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Continuous => "Continuous",
            Self::Dashed => "Dashed",
            Self::DashDot => "DashDot",
            Self::DoubleDashDot => "DoubleDashDot",
            Self::Dotted => "Dotted",
        }
    }

    /// 全部线型名（导出前校验用）。
    #[must_use]
    pub const fn all_names() -> [&'static str; 5] {
        ["Continuous", "Dashed", "DashDot", "DoubleDashDot", "Dotted"]
    }
}

/// 图层定义。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerDef {
    /// 图层 id
    pub id: LayerId,
    /// 图层名（按名查找，见 [`DraftingStandard::layer_by_name`]）
    pub name: String,
    /// 颜色
    pub color: Rgb,
    /// 线型
    pub line_type: LineType,
    /// 线宽（mm，图纸上的实际宽度；不是屏幕像素）
    pub width_mm: f64,
}

impl LayerDef {
    /// 构造一个图层。
    #[must_use]
    pub fn new(name: impl Into<String>, color: Rgb, line_type: LineType, width_mm: f64) -> Self {
        Self {
            id: LayerId::new(),
            name: name.into(),
            color,
            line_type,
            width_mm,
        }
    }
}

/// 文字样式。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextStyle {
    /// 样式名
    pub name: String,
    /// 字高（mm，图纸上的实际字高）
    pub height_mm: f64,
    /// 宽高比（国标常用 0.7）
    pub width_factor: f64,
    /// 字体族
    pub font_family: String,
}

impl TextStyle {
    /// 按国标常用参数构造（字高 3.5mm、宽高比 0.7、长仿宋）。
    #[must_use]
    pub fn gb_default(name: impl Into<String>, height_mm: f64) -> Self {
        Self {
            name: name.into(),
            height_mm,
            width_factor: 0.7,
            font_family: "SimSun".to_string(),
        }
    }
}

/// 图幅（图纸尺寸）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SheetFormat {
    /// A0
    A0,
    /// A1
    A1,
    /// A2
    A2,
    /// A3
    A3,
    /// A4
    A4,
    /// A4 加长（国标常用 210×594，用于长条图纸）
    A4Extended,
}

impl SheetFormat {
    /// 按国标取值 (宽 mm, 高 mm)，横放。
    #[must_use]
    pub const fn size_mm(self) -> (f64, f64) {
        match self {
            Self::A0 => (1189.0, 841.0),
            Self::A1 => (841.0, 594.0),
            Self::A2 => (594.0, 420.0),
            Self::A3 => (420.0, 297.0),
            Self::A4 => (297.0, 210.0),
            Self::A4Extended => (594.0, 210.0),
        }
    }

    /// 国标图幅代号（如 `"A3"`）。
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::A0 => "A0",
            Self::A1 => "A1",
            Self::A2 => "A2",
            Self::A3 => "A3",
            Self::A4 => "A4",
            Self::A4Extended => "A4x3",
        }
    }

    /// GB/T 14689-2008 表1 的**留装订边**格式留边尺寸，返回 `(a, c)`（mm）。
    ///
    /// `a` = 装订边（**左侧**，所有幅面均为 25 mm）；
    /// `c` = 非装订边（**右 / 上 / 下**）。
    ///
    /// | 幅面 | a | c |
    /// |---|---|---|
    /// | A0 / A1 / A2 | 25 | 10 |
    /// | A3 / A4 | 25 | 5 |
    /// | A4Extended（按"大一号"的 A3 取） | 25 | 5 |
    ///
    /// **取值依据与争议**：`c` 对 A2 有二手资料的 10 / 5 两说；本实现以
    /// GB/T 14689-2008 表1 及《机械设计手册》《技术制图》教材表为准，取 **A2 = 10**。
    /// 另：国标"**不留装订边**"格式（四周尺寸 `e` = A0/A1→20、A2/A3/A4→10）**本批次不实现**，
    /// 如需再补。
    #[must_use]
    pub const fn margins_mm(self) -> (f64, f64) {
        match self {
            Self::A0 | Self::A1 | Self::A2 => (25.0, 10.0),
            Self::A3 | Self::A4 | Self::A4Extended => (25.0, 5.0),
        }
    }
}

/// 会签栏 / 标题栏定义（**规则**，不是图形）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TitleBlockDef {
    /// 名称
    pub name: String,
    /// 宽（mm）
    pub width_mm: u32,
    /// 高（mm）
    pub height_mm: u32,
    /// 位置：右下角
    pub at_bottom_right: bool,
}

impl Default for TitleBlockDef {
    /// 国标常用标题栏：180 × 56 mm，置于右下角。
    fn default() -> Self {
        Self {
            name: "GB-标题栏".to_string(),
            width_mm: 180,
            height_mm: 56,
            at_bottom_right: true,
        }
    }
}

/// 制图标准：**一套可配置的规则**。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DraftingStandard {
    /// 标准名（如 `"GB-工业厂房-院标2024"`）
    pub name: String,
    /// 图层表
    pub layers: Vec<LayerDef>,
    /// 文字样式表
    pub text_styles: Vec<TextStyle>,
    /// 标题栏
    pub title_block: TitleBlockDef,
    /// 常用比例序列（分母，如 100 表示 1:100）
    pub scale_series: Vec<u32>,
    /// 标注默认小数位
    pub dim_decimals: u8,
}

impl Default for DraftingStandard {
    fn default() -> Self {
        Self::gb_industrial()
    }
}

impl DraftingStandard {
    /// 出厂默认：一套国标工业厂房制图标准。
    #[must_use]
    pub fn gb_industrial() -> Self {
        Self {
            name: "GB-工业厂房".to_string(),
            layers: vec![
                LayerDef::new(LAYER_AXIS, Rgb::RED, LineType::DashDot, 0.15),
                LayerDef::new(LAYER_OUTLINE, Rgb::BLACK, LineType::Continuous, 0.50),
                LayerDef::new(LAYER_HIDDEN, Rgb::CYAN, LineType::Dashed, 0.25),
                LayerDef::new(LAYER_DIM, Rgb::YELLOW, LineType::Continuous, 0.18),
                LayerDef::new(LAYER_TEXT, Rgb::WHITE, LineType::Continuous, 0.18),
                LayerDef::new(LAYER_HATCH, Rgb::GREEN, LineType::Continuous, 0.13),
            ],
            text_styles: vec![
                TextStyle::gb_default("标准", 3.5),
                TextStyle::gb_default("小字", 2.5),
                TextStyle::gb_default("大字", 7.0),
            ],
            title_block: TitleBlockDef::default(),
            scale_series: vec![1, 2, 5, 10, 20, 25, 50, 100, 200, 500, 1000],
            dim_decimals: 0,
        }
    }

    /// 按名查图层。
    #[must_use]
    pub fn layer_by_name(&self, name: &str) -> Option<&LayerDef> {
        self.layers.iter().find(|l| l.name == name)
    }

    /// 按 **id** 查图层。
    ///
    /// 图元存的是 `LayerId`（一个 UUID），出图导出时必须换回**具体的**
    /// 颜色 / 线型 / 线宽——这就是"ByLayer 在出口处一次性解开"那一步。
    #[must_use]
    pub fn layer_by_id(&self, id: LayerId) -> Option<&LayerDef> {
        self.layers.iter().find(|l| l.id == id)
    }

    /// 按 id 查图层，找不到时报错。
    pub fn require_layer_by_id(&self, id: LayerId) -> DocResult<&LayerDef> {
        self.layer_by_id(id)
            .ok_or_else(|| DocError::Standard(format!("图元引用了未定义的图层 id：{id}")))
    }

    /// 按名查图层，找不到报错（导出前校验用）。
    pub fn require_layer(&self, name: &str) -> DocResult<&LayerDef> {
        self.layer_by_name(name)
            .ok_or_else(|| DocError::Standard(format!("未定义的图层：{name}")))
    }

    /// 追加 / 覆盖一个图层（模块经 SDK 加专业图层时用）。
    pub fn upsert_layer(&mut self, layer: LayerDef) {
        if let Some(slot) = self.layers.iter_mut().find(|l| l.name == layer.name) {
            *slot = layer;
        } else {
            self.layers.push(layer);
        }
    }

    /// 按名查文字样式。
    #[must_use]
    pub fn text_style(&self, name: &str) -> Option<&TextStyle> {
        self.text_styles.iter().find(|t| t.name == name)
    }

    /// 给一类二维图元推荐默认图层名。
    ///
    /// 这是"规则"的落点：图元只记 [`EntityKind`]，由标准决定它落在哪个图层。
    #[must_use]
    pub fn default_layer_for(&self, kind: EntityKind) -> &'static str {
        match kind {
            EntityKind::Dimension | EntityKind::Leader => LAYER_DIM,
            EntityKind::Text | EntityKind::Symbol => LAYER_TEXT,
            EntityKind::Hatch => LAYER_HATCH,
            EntityKind::Axis => LAYER_AXIS,
            EntityKind::Line
            | EntityKind::Polyline
            | EntityKind::Arc
            | EntityKind::Circle
            | EntityKind::Ellipse => LAYER_OUTLINE,
        }
    }

    /// 比例是否为标准序列中的常用比例（导出提示用，不阻断）。
    #[must_use]
    pub fn is_standard_scale(&self, denominator: u32) -> bool {
        self.scale_series.contains(&denominator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_standard_has_the_expected_gb_layers() {
        let s = DraftingStandard::gb_industrial();
        assert!(s.layer_by_name(LAYER_AXIS).is_some());
        assert!(s.layer_by_name(LAYER_OUTLINE).is_some());
        assert!(s.layer_by_name(LAYER_HIDDEN).is_some());
        assert!(s.layer_by_name(LAYER_DIM).is_some());
        assert_eq!(s.layers.len(), 6);
    }

    #[test]
    fn axis_layer_uses_dash_dot_line_type() {
        let s = DraftingStandard::gb_industrial();
        let axis = s.require_layer(LAYER_AXIS).unwrap();
        assert_eq!(axis.line_type, LineType::DashDot);
        assert!(!axis.line_type.is_dashed_family(), "点划线不属于虚线族");
    }

    #[test]
    fn missing_layer_is_reported() {
        let s = DraftingStandard::gb_industrial();
        assert!(matches!(
            s.require_layer("不存在的图层"),
            Err(DocError::Standard(_))
        ));
    }

    #[test]
    fn entity_kinds_map_to_the_right_default_layer() {
        let s = DraftingStandard::gb_industrial();
        assert_eq!(s.default_layer_for(EntityKind::Dimension), LAYER_DIM);
        assert_eq!(s.default_layer_for(EntityKind::Axis), LAYER_AXIS);
        assert_eq!(s.default_layer_for(EntityKind::Line), LAYER_OUTLINE);
        assert_eq!(s.default_layer_for(EntityKind::Text), LAYER_TEXT);
    }

    #[test]
    fn upsert_layer_replaces_in_place_and_appends_new() {
        let mut s = DraftingStandard::gb_industrial();
        let before = s.layers.len();
        s.upsert_layer(LayerDef::new(
            LAYER_AXIS,
            Rgb::MAGENTA,
            LineType::Continuous,
            0.2,
        ));
        assert_eq!(s.layers.len(), before, "同名应覆盖而非追加");
        assert_eq!(
            s.require_layer(LAYER_AXIS).unwrap().line_type,
            LineType::Continuous
        );

        s.upsert_layer(LayerDef::new(
            LAYER_CABLE_TRAY,
            Rgb::YELLOW,
            LineType::Continuous,
            0.35,
        ));
        assert_eq!(s.layers.len(), before + 1, "新名应追加（模块加专业图层）");
    }

    #[test]
    fn sheet_formats_match_gb_series() {
        assert_eq!(SheetFormat::A0.size_mm(), (1189.0, 841.0));
        assert_eq!(SheetFormat::A3.size_mm(), (420.0, 297.0));
        assert_eq!(SheetFormat::A3.code(), "A3");
        // 国标 A4 加长（长条图）确实比 A4 长
        let (w_ext, _) = SheetFormat::A4Extended.size_mm();
        let (w_a4, _) = SheetFormat::A4.size_mm();
        assert!(w_ext > w_a4);
    }

    #[test]
    fn standard_scale_series_is_checkable() {
        let s = DraftingStandard::gb_industrial();
        assert!(s.is_standard_scale(100));
        assert!(!s.is_standard_scale(37), "37 不是国标常用比例");
    }

    #[test]
    fn standard_roundtrips_through_json() {
        let s = DraftingStandard::gb_industrial();
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<DraftingStandard>(&json).unwrap(), s);
    }

    #[test]
    fn layers_can_be_found_by_id_for_export() {
        let s = DraftingStandard::gb_industrial();
        let axis = s.layer_by_name(LAYER_AXIS).unwrap();
        let found = s.layer_by_id(axis.id).unwrap();
        assert_eq!(found.name, LAYER_AXIS);
        assert_eq!(found.line_type, LineType::DashDot);
    }

    #[test]
    fn unknown_layer_id_is_reported_for_export() {
        let s = DraftingStandard::gb_industrial();
        let orphan = LayerId::new();
        assert!(s.layer_by_id(orphan).is_none());
        assert!(matches!(
            s.require_layer_by_id(orphan),
            Err(DocError::Standard(_))
        ));
    }

    #[test]
    fn line_type_names_are_the_contract_strings_used_by_exporters() {
        assert_eq!(LineType::Continuous.name(), "Continuous");
        assert_eq!(LineType::DashDot.name(), "DashDot");
        assert_eq!(LineType::Dotted.name(), "Dotted");
        // 这五个名字必须与 core-interop 的 SVG/DXF 序列化器认得的名字一致
        assert_eq!(
            LineType::all_names(),
            ["Continuous", "Dashed", "DashDot", "DoubleDashDot", "Dotted"]
        );
    }

    /// B5.4 钉子 ①：图框留边按 GB/T 14689-2008 表1（**留装订边**格式）。
    #[test]
    fn sheet_margins_follow_gb_14689() {
        // A0/A1/A2：装订边 25、其余 10
        for f in [SheetFormat::A0, SheetFormat::A1, SheetFormat::A2] {
            assert_eq!(f.margins_mm(), (25.0, 10.0), "{f:?} 留边应为 (25,10)");
        }
        // A3/A4 及加长 A4（按大一号的 A3）：装订边 25、其余 5
        for f in [SheetFormat::A3, SheetFormat::A4, SheetFormat::A4Extended] {
            assert_eq!(f.margins_mm(), (25.0, 5.0), "{f:?} 留边应为 (25,5)");
        }
    }
}

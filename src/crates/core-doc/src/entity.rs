//! 二维图元（设计文档 5.1）。
//!
//! ## 一条关键设计：**图元带"溯源"**
//!
//! 每条二维图元都可以记一个 [`Entity2D::source`]——它是**从模型的哪个子形状投影来的**
//! （[`GeometryKey`]）。这是关联更新的锚点：
//!
//! ```text
//! 模型改了一个面 → EvolutionData 告知哪个 GeometryKey 变了
//!                → 查 DependencyGraph 得到受影响的 EntityId 集合
//!                → 只重投这些图元，其余图纸内容一动不动
//! ```
//!
//! ⚠ **注意**：`GeometryKey` 里的 `SubShapeRef` 目前是"拓扑序号"占位。
//! 内核的 `hash_code` 跨会话不稳定（设计文档 6.6），**不能当持久 id**。
//! 稳定的拓扑绑定方案是 M5 的课题，见 [`core_model::SubShapeRef`] 的文档。

use core_model::{GeometryKey, LayerId};
use glam::DVec2;
use serde::{Deserialize, Serialize};

use crate::ids::EntityId;
use crate::standard::{LineType, Rgb};

/// 二维图元的种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EntityKind {
    /// 直线段（两点）
    Line,
    /// 折线（多点）
    Polyline,
    /// 圆弧
    Arc,
    /// 整圆
    Circle,
    /// 椭圆（含椭圆弧）
    Ellipse,
    /// 填充 / 剖面线
    Hatch,
    /// 尺寸标注
    Dimension,
    /// 引线
    Leader,
    /// 文字
    Text,
    /// 符号（轴号圈、标高符号 △ 等**显示层**构件）
    Symbol,
    /// 轴线（轴网的平面投影）
    Axis,
}

impl EntityKind {
    /// 是否是"从模型投影来的"几何类图元（决定生成时是否需要 `project_edges`）。
    #[must_use]
    pub const fn is_projected_geometry(self) -> bool {
        matches!(
            self,
            Self::Line | Self::Polyline | Self::Arc | Self::Circle | Self::Ellipse
        )
    }

    /// 是否属于**显示层符号**（不存几何来源，由出图时现画，设计文档 4.6.2）。
    #[must_use]
    pub const fn is_display_symbol(self) -> bool {
        matches!(self, Self::Symbol | Self::Axis)
    }
}

/// 图元的绘制样式（语义层，可被 `DraftingStandard` 覆盖）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityStyle {
    /// 线型（`None` 表示跟随图层）
    pub line_type: Option<LineType>,
    /// 线宽 mm（`None` 表示跟随图层）
    pub width_mm: Option<f64>,
    /// 颜色（`None` 表示跟随图层，即"ByLayer"）
    pub color: Option<Rgb>,
}

impl EntityStyle {
    /// 全部跟随图层（最常用）。
    #[must_use]
    pub const fn by_layer() -> Self {
        Self {
            line_type: None,
            width_mm: None,
            color: None,
        }
    }

    /// 指定线型。
    #[must_use]
    pub const fn with_line_type(mut self, line_type: LineType) -> Self {
        self.line_type = Some(line_type);
        self
    }

    /// 指定颜色。
    #[must_use]
    pub const fn with_color(mut self, color: Rgb) -> Self {
        self.color = Some(color);
        self
    }
}

impl Default for EntityStyle {
    fn default() -> Self {
        Self::by_layer()
    }
}

/// 一条二维图元。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entity2D {
    /// 持久 id
    pub id: EntityId,
    /// 种类
    pub kind: EntityKind,
    /// 所在图层（按 id 引用 [`crate::DraftingStandard`] 的图层表）
    pub layer: LayerId,
    /// 二维点列（直线两点；折线多点；圆弧为圆心+起终点；圆/椭圆为圆心）
    pub points: Vec<DVec2>,
    /// 半径（圆弧、圆用；**椭圆不复用它**，见 [`Self::semi_major`]）
    pub radius: Option<f64>,
    /// 椭圆长半轴（`Ellipse` 用）。
    ///
    /// 字段是 `Option`，**缺失即 `None`（旧 JSON 天然兼容）**；
    /// `#[serde(default)]` 仅作显式声明，并非兼容机制。
    #[serde(default)]
    pub semi_major: Option<f64>,
    /// 椭圆短半轴（`Ellipse` 用）。缺失即 `None`（旧 JSON 天然兼容）。
    #[serde(default)]
    pub semi_minor: Option<f64>,
    /// 椭圆主轴方位角（弧度，`Ellipse` 用）。缺失即 `None`（旧 JSON 天然兼容）。
    #[serde(default)]
    pub rotation_rad: Option<f64>,
    /// 文字内容（`Text` / `Symbol` 用）
    pub text: Option<String>,
    /// **溯源**：由模型的哪个子形状投影而来（关联更新的锚）
    pub source: Option<GeometryKey>,
    /// 绘制样式
    pub style: EntityStyle,
}

impl Entity2D {
    /// 由点列构造一条几何图元。
    #[must_use]
    pub fn new(kind: EntityKind, layer: LayerId, points: Vec<DVec2>) -> Self {
        Self {
            id: EntityId::new(),
            kind,
            layer,
            points,
            radius: None,
            semi_major: None,
            semi_minor: None,
            rotation_rad: None,
            text: None,
            source: None,
            style: EntityStyle::by_layer(),
        }
    }

    /// 一条直线段。
    #[must_use]
    pub fn line(layer: LayerId, from: DVec2, to: DVec2) -> Self {
        Self::new(EntityKind::Line, layer, vec![from, to])
    }

    /// 一段折线。
    #[must_use]
    pub fn polyline(layer: LayerId, points: Vec<DVec2>) -> Self {
        Self::new(EntityKind::Polyline, layer, points)
    }

    /// 一个整圆。
    #[must_use]
    pub fn circle(layer: LayerId, center: DVec2, radius: f64) -> Self {
        let mut e = Self::new(EntityKind::Circle, layer, vec![center]);
        e.radius = Some(radius);
        e
    }

    /// 一个椭圆（长轴沿 `rotation_rad` 方位；`semi_major >= semi_minor` 由调用方保证）。
    ///
    /// **不复用 `radius`**：椭圆的长短半轴各有其位，塞进单个 `radius` 会与圆语义混淆
    /// （旧代码"椭圆当同半径圆"正是这么来的）。`points` 只放圆心，几何由半轴表达。
    #[must_use]
    pub fn ellipse(
        layer: LayerId,
        center: DVec2,
        semi_major: f64,
        semi_minor: f64,
        rotation_rad: f64,
    ) -> Self {
        let mut e = Self::new(EntityKind::Ellipse, layer, vec![center]);
        e.semi_major = Some(semi_major);
        e.semi_minor = Some(semi_minor);
        e.rotation_rad = Some(rotation_rad);
        e
    }

    /// 输入文字。
    #[must_use]
    pub fn text(layer: LayerId, at: DVec2, content: impl Into<String>) -> Self {
        let mut e = Self::new(EntityKind::Text, layer, vec![at]);
        e.text = Some(content.into());
        e
    }

    /// 绑定溯源（链式）。
    #[must_use]
    pub fn from_geometry(mut self, source: GeometryKey) -> Self {
        self.source = Some(source);
        self
    }

    /// 指定样式（链式）。
    #[must_use]
    pub fn with_style(mut self, style: EntityStyle) -> Self {
        self.style = style;
        self
    }

    /// 是否有溯源（能否参与关联更新）。
    #[must_use]
    pub const fn is_traceable(&self) -> bool {
        self.source.is_some()
    }

    /// 图元长度（仅对直线段与折线有定义；其它返回 `None`）。
    ///
    /// 用来做"投影结果是否退化"的粗检验。
    #[must_use]
    pub fn polyline_length(&self) -> Option<f64> {
        if !matches!(self.kind, EntityKind::Line | EntityKind::Polyline) {
            return None;
        }
        if self.points.len() < 2 {
            return Some(0.0);
        }
        Some(
            self.points
                .windows(2)
                .map(|w| (w[1] - w[0]).length())
                .sum::<f64>(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::ShapeId;

    #[test]
    fn line_length_is_computed_from_its_two_points() {
        let e = Entity2D::line(LayerId::new(), DVec2::ZERO, DVec2::new(3.0, 4.0));
        assert_eq!(e.polyline_length(), Some(5.0));
    }

    #[test]
    fn polyline_length_sums_all_segments() {
        let e = Entity2D::polyline(
            LayerId::new(),
            vec![DVec2::ZERO, DVec2::new(10.0, 0.0), DVec2::new(10.0, 10.0)],
        );
        assert_eq!(e.polyline_length(), Some(20.0));
    }

    #[test]
    fn length_is_undefined_for_non_linear_kinds() {
        let c = Entity2D::circle(LayerId::new(), DVec2::ZERO, 5.0);
        assert_eq!(c.polyline_length(), None);
        assert_eq!(c.radius, Some(5.0));
    }

    #[test]
    fn entities_start_untraceable_and_can_be_bound() {
        let mut e = Entity2D::line(LayerId::new(), DVec2::ZERO, DVec2::new(1.0, 0.0));
        assert!(!e.is_traceable());
        e = e.from_geometry(GeometryKey::edge(ShapeId::new(), 4));
        assert!(e.is_traceable());
    }

    #[test]
    fn kind_classification_is_correct() {
        assert!(EntityKind::Circle.is_projected_geometry());
        assert!(!EntityKind::Symbol.is_projected_geometry());
        assert!(EntityKind::Symbol.is_display_symbol());
        assert!(EntityKind::Axis.is_display_symbol());
        assert!(!EntityKind::Dimension.is_projected_geometry());
    }

    #[test]
    fn style_defaults_to_by_layer() {
        let s = EntityStyle::by_layer();
        assert!(s.line_type.is_none() && s.width_mm.is_none() && s.color.is_none());
        let s2 = s.with_color(Rgb::RED);
        assert_eq!(s2.color, Some(Rgb::RED));
    }

    #[test]
    fn entity_roundtrips_through_json() {
        let e = Entity2D::circle(LayerId::new(), DVec2::new(1.0, 2.0), 3.0)
            .from_geometry(GeometryKey::face(ShapeId::new(), 0));
        let json = serde_json::to_string(&e).unwrap();
        assert_eq!(serde_json::from_str::<Entity2D>(&json).unwrap(), e);
    }

    #[test]
    fn ellipse_entity_keeps_semi_axes() {
        let e = Entity2D::ellipse(LayerId::new(), DVec2::ZERO, 50.0, 20.0, 0.0);
        assert_eq!(e.kind, EntityKind::Ellipse);
        assert_eq!(e.semi_major, Some(50.0));
        assert_eq!(e.semi_minor, Some(20.0));
        assert_ne!(e.semi_major, e.semi_minor, "长短半轴必须可分");
        assert_eq!(e.radius, None, "椭圆不得复用 radius（会与圆语义混淆）");
        assert_eq!(e.points, vec![DVec2::ZERO], "只放圆心");
    }

    #[test]
    fn ellipse_json_roundtrips_with_semi_axes() {
        // ① 新 JSON：半轴往返不丢
        let e = Entity2D::ellipse(LayerId::new(), DVec2::new(1.0, 2.0), 50.0, 20.0, 0.5);
        let json = serde_json::to_string(&e).unwrap();
        let back: Entity2D = serde_json::from_str(&json).unwrap();
        assert_eq!(back, e);
        assert_eq!(back.semi_major, Some(50.0));
        assert_eq!(back.semi_minor, Some(20.0));
        assert_eq!(back.rotation_rad, Some(0.5));

        // ② 旧 JSON（不含半轴字段）仍可反序列化：字段是 `Option`，缺失自然落到 `None`
        //    （`#[serde(default)]` 只是显式声明；本断言钉的是兼容**行为**，有效）
        let mut v = serde_json::to_value(&e).unwrap();
        let obj = v.as_object_mut().unwrap();
        obj.remove("semi_major");
        obj.remove("semi_minor");
        obj.remove("rotation_rad");
        let legacy = serde_json::to_string(&v).unwrap();
        assert!(
            !legacy.contains("semi_major"),
            "旧 JSON 不应含半轴字段：{legacy}"
        );
        let back: Entity2D = serde_json::from_str(&legacy).unwrap();
        assert_eq!(back.semi_major, None);
        assert_eq!(back.semi_minor, None);
        assert_eq!(back.rotation_rad, None);
    }
}

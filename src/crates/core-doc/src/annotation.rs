//! 标注（设计文档 6.3 / 6.4）。
//!
//! ## 标注与"画线"的本质区别：**它有宿主**
//!
//! 一根线是"画上去就不管了"；一个尺寸标注必须记得"我量的是哪条边"。
//! 所以 [`Annotation`] 的核心字段不是文字，而是 [`Annotation::bound_to`]——
//! **它绑定的是模型拓扑**（[`GeometryKey`]），不是 HLR 输出的一根二维线。
//!
//! ## 为什么绑模型拓扑而不绑 HLR 输出（设计文档 6.2 / 6.4）
//!
//! `occt-wasm` 的 `project_edges` 输出的是**B-Rep 精确曲线**，且把
//! "轮廓母线 / 光顺边 / 棱线"三类分开输出——这是它能作标注依据的原因。
//! **但两库都不给**"HLR 输出 → 原始模型边"的溯源。
//! 因此关联标注的正确做法是：**自己记住模型子形状（`GeometryKey`），
//! 需要时自行投影**；HLR 只用来决定可见性与线型（实线 / 虚线）分类。

use core_model::GeometryKey;
use glam::DVec2;
use serde::{Deserialize, Serialize};

use crate::entity::Entity2D;
use crate::ids::EntityId;

/// 标注种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AnnotationKind {
    /// 线性尺寸（长度、间距）
    Linear,
    /// 角度尺寸
    Angular,
    /// 半径
    Radial,
    /// 直径
    Diametric,
    /// 标高（直接引用 [`core_model::DatumElement`] 的 `elevation`）
    Elevation,
    /// 轴号（轴网标注，直接引用轴网 `name`）
    GridTag,
    /// 纯文字说明
    Text,
    /// 引线 + 说明
    Leader,
}

impl AnnotationKind {
    /// 是否**依据基准元素**（建筑标高 / 轴号）——这类标注必须随基准元素联动。
    #[must_use]
    pub const fn is_datum_based(self) -> bool {
        matches!(self, Self::Elevation | Self::GridTag)
    }

    /// 是否**依据模型几何**——这类标注随构件改动而更新。
    #[must_use]
    pub const fn is_geometry_based(self) -> bool {
        matches!(
            self,
            Self::Linear | Self::Angular | Self::Radial | Self::Diametric
        )
    }
}

/// 标注样式（语义层，受 `DraftingStandard` 约束）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnnotationStyle {
    /// 引用文字样式名
    pub text_style: String,
    /// 箭头大小（mm）
    pub arrow_size_mm: u32,
    /// 数值小数位（`None` 表示跟随标准）
    pub decimals: Option<u8>,
    /// 尺寸界线与尺寸线的偏移（mm）
    pub extension_offset_mm: u32,
}

impl Default for AnnotationStyle {
    fn default() -> Self {
        Self {
            text_style: "标准".to_string(),
            arrow_size_mm: 3,
            decimals: None,
            extension_offset_mm: 2,
        }
    }
}

/// 一个标注。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    /// 持久 id
    pub id: EntityId,
    /// 种类
    pub kind: AnnotationKind,
    /// 量测点（二维，视图坐标系）
    pub points: Vec<DVec2>,
    /// 显示文字（`None` 表示由量测值自动生成）
    pub text: Option<String>,
    /// **宿主**：绑定的模型拓扑（关联更新的锚）
    pub bound_to: Option<GeometryKey>,
    /// 依据的基准元素（标高 / 轴号标注用）
    pub datum_ref: Option<core_model::ElementId>,
    /// 关联的二维图元（标注尺寸界线指向哪几条线）
    pub associates: Vec<EntityId>,
    /// 样式
    pub style: AnnotationStyle,
}

impl Annotation {
    /// 构造一个标注。
    #[must_use]
    pub fn new(kind: AnnotationKind, points: Vec<DVec2>) -> Self {
        Self {
            id: EntityId::new(),
            kind,
            points,
            text: None,
            bound_to: None,
            datum_ref: None,
            associates: Vec::new(),
            style: AnnotationStyle::default(),
        }
    }

    /// 一个线性尺寸标注。
    #[must_use]
    pub fn linear(from: DVec2, to: DVec2) -> Self {
        Self::new(AnnotationKind::Linear, vec![from, to])
    }

    /// 绑定到模型拓扑（链式）。
    #[must_use]
    pub fn bound_to(mut self, key: GeometryKey) -> Self {
        self.bound_to = Some(key);
        self
    }

    /// 依据某个基准元素（链式）。
    #[must_use]
    pub fn on_datum(mut self, datum: core_model::ElementId) -> Self {
        self.datum_ref = Some(datum);
        self
    }

    /// 覆盖显示文字（链式）。
    #[must_use]
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// 关联若干二维图元（链式）。
    #[must_use]
    pub fn associating(mut self, entities: Vec<EntityId>) -> Self {
        self.associates = entities;
        self
    }

    /// 计算自动量测值（线性标注：两点距离；其它返回 `None`）。
    ///
    /// 尺寸文字的实际生成还要过单位换算与精度格式化，这里只给**原始量测值**。
    #[must_use]
    pub fn measured_value(&self) -> Option<f64> {
        match self.kind {
            AnnotationKind::Linear if self.points.len() == 2 => {
                Some((self.points[1] - self.points[0]).length())
            }
            AnnotationKind::Radial | AnnotationKind::Diametric if self.points.len() == 2 => {
                let r = (self.points[1] - self.points[0]).length();
                Some(if self.kind == AnnotationKind::Diametric {
                    r * 2.0
                } else {
                    r
                })
            }
            _ => None,
        }
    }

    /// 是否需要随模型改动而更新。
    #[must_use]
    pub const fn needs_model_tracking(&self) -> bool {
        self.bound_to.is_some() && self.kind.is_geometry_based()
    }

    /// 依据**关联图元的新几何**重算尺寸点与量测值。
    ///
    /// 只处理线性尺寸（[`AnnotationKind::Linear`]）：在 [`Annotation::associates`]
    /// 指到的图元**点集**里取**最近的一对点**作为新的 `p1` / `p2`——即"尺寸界线
    /// 指向的那两条棱线之间量得的最小间距"。其余标注种类一律不动。
    ///
    /// 返回 `true` 表示**量测值发生了变化**（供上层判定"要不要标脏"）；几何没动
    /// （最近点对与旧 `p1/p2` 等距）或无法量测（无关联点 / 点不足）时返回 `false`。
    pub fn remeasure_from_entities(&mut self, entities: &[&Entity2D]) -> bool {
        if self.kind != AnnotationKind::Linear {
            return false;
        }
        let mut points: Vec<DVec2> = Vec::new();
        for entity in entities {
            if self.associates.contains(&entity.id) {
                points.extend(entity.points.iter().copied());
            }
        }
        let Some((p1, p2)) = closest_pair(&points) else {
            return false;
        };

        let before = self.measured_value();
        self.points = vec![p1, p2];
        let after = self.measured_value();
        before != after
    }
}

/// 点集里**最近的一对点**（点数不足 2 无解 → `None`）。
///
/// 用欧氏距离平方比较（避免开方），遍历顺序固定 → 结果可复现。
fn closest_pair(points: &[DVec2]) -> Option<(DVec2, DVec2)> {
    if points.len() < 2 {
        return None;
    }
    let mut best: Option<(DVec2, DVec2, f64)> = None;
    for (i, &a) in points.iter().enumerate() {
        for &b in &points[i + 1..] {
            let d = (b - a).length_squared();
            let take = match best {
                Some((_, _, best_d)) => d < best_d,
                None => true,
            };
            if take {
                best = Some((a, b, d));
            }
        }
    }
    best.map(|(a, b, _)| (a, b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::{ElementId, LayerId, ShapeId};

    #[test]
    fn linear_annotation_measures_distance() {
        let a = Annotation::linear(DVec2::ZERO, DVec2::new(3.0, 4.0));
        assert_eq!(a.measured_value(), Some(5.0));
    }

    #[test]
    fn diametric_annotation_doubles_the_radius() {
        let r = Annotation::new(
            AnnotationKind::Radial,
            vec![DVec2::ZERO, DVec2::new(10.0, 0.0)],
        );
        assert_eq!(r.measured_value(), Some(10.0));
        let d = Annotation::new(
            AnnotationKind::Diametric,
            vec![DVec2::ZERO, DVec2::new(10.0, 0.0)],
        );
        assert_eq!(d.measured_value(), Some(20.0));
    }

    #[test]
    fn datum_based_annotations_do_not_need_model_tracking() {
        let level = ElementId::new();
        let a = Annotation::new(AnnotationKind::Elevation, vec![DVec2::ZERO]).on_datum(level);
        assert!(a.kind.is_datum_based());
        assert!(
            !a.needs_model_tracking(),
            "标高标注跟基准元素走，不跟几何走"
        );
        assert_eq!(a.datum_ref, Some(level));
    }

    #[test]
    fn geometry_based_annotation_tracks_only_when_bound() {
        let mut a = Annotation::linear(DVec2::ZERO, DVec2::new(100.0, 0.0));
        assert!(!a.needs_model_tracking(), "还没绑宿主，谈不上跟随");
        a = a.bound_to(GeometryKey::edge(ShapeId::new(), 2));
        assert!(a.needs_model_tracking());
    }

    #[test]
    fn text_can_be_overridden() {
        let a = Annotation::linear(DVec2::ZERO, DVec2::new(600.0, 0.0)).with_text("600(宽)");
        assert_eq!(a.text.as_deref(), Some("600(宽)"));
    }

    // ---------- M5 / B3-c：标注重标 ----------

    /// 一条竖直棱线（`id` 指定，便于充当 `associates` 里的引用）。
    fn vertical_edge(id: EntityId, x: f64) -> Entity2D {
        let mut e = Entity2D::line(LayerId::new(), DVec2::new(x, 0.0), DVec2::new(x, 3000.0));
        e.id = id;
        e
    }

    #[test]
    fn remeasure_updates_linear_dimension_value() {
        let shape = ShapeId::new();
        let left_id = EntityId::new();
        let right_id = EntityId::new();
        let mut ann = Annotation::linear(DVec2::ZERO, DVec2::new(400.0, 0.0))
            .bound_to(GeometryKey::edge(shape, 0))
            .associating(vec![left_id, right_id]);
        assert_eq!(ann.measured_value(), Some(400.0));

        // 柱宽 400 → 600：两条棱线外移到 x = 600。
        let left = vertical_edge(left_id, 0.0);
        let right = vertical_edge(right_id, 600.0);
        let entities = [&left, &right];

        assert!(
            ann.remeasure_from_entities(&entities),
            "图元动了，量测值应变"
        );
        assert_eq!(ann.measured_value(), Some(600.0), "量测值应自动 400 → 600");
    }

    #[test]
    fn remeasure_is_noop_when_geometry_unchanged() {
        let shape = ShapeId::new();
        let left_id = EntityId::new();
        let right_id = EntityId::new();
        let mut ann = Annotation::linear(DVec2::ZERO, DVec2::new(400.0, 0.0))
            .bound_to(GeometryKey::edge(shape, 0))
            .associating(vec![left_id, right_id]);

        let left = vertical_edge(left_id, 0.0);
        let right = vertical_edge(right_id, 400.0);
        let entities = [&left, &right];

        assert!(
            !ann.remeasure_from_entities(&entities),
            "几何没动，不应判脏"
        );
        assert_eq!(ann.measured_value(), Some(400.0));
    }

    #[test]
    fn remeasure_ignores_entities_not_listed_in_associates() {
        let left_id = EntityId::new();
        let right_id = EntityId::new();
        let mut ann = Annotation::linear(DVec2::ZERO, DVec2::new(400.0, 0.0))
            .associating(vec![left_id, right_id]);

        let left = vertical_edge(left_id, 0.0);
        let right = vertical_edge(right_id, 400.0);
        // 一条没被 associates 指到的线：位置再怪也不该影响量测。
        let mut stray = Entity2D::line(
            LayerId::new(),
            DVec2::new(9_000.0, 0.0),
            DVec2::new(9_000.0, 1.0),
        );
        stray.id = EntityId::new();
        let entities = [&left, &right, &stray];

        assert!(!ann.remeasure_from_entities(&entities));
        assert_eq!(ann.measured_value(), Some(400.0));
    }

    #[test]
    fn remeasure_leaves_non_linear_annotations_alone() {
        let mut ann = Annotation::new(
            AnnotationKind::Angular,
            vec![DVec2::ZERO, DVec2::new(1.0, 0.0)],
        );
        let edge = vertical_edge(EntityId::new(), 0.0);
        assert!(!ann.remeasure_from_entities(&[&edge]), "非线性标注不动");
    }

    #[test]
    fn annotation_roundtrips_through_json() {
        let a = Annotation::linear(DVec2::ZERO, DVec2::new(600.0, 0.0))
            .bound_to(GeometryKey::edge(ShapeId::new(), 1))
            .associating(vec![EntityId::new()]);
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(serde_json::from_str::<Annotation>(&json).unwrap(), a);
    }
}

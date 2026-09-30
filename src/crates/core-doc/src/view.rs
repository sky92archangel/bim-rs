//! 视图（设计文档 5.2 / 4.6.1）。
//!
//! ## 视图是"算出来的"，不是"画出来的"
//!
//! 这是模型驱动出图的核心：用户不画平面图，用户画模型；平面图是
//! **"在某个标高剖切 + 沿某方向投影 + 消隐"** 的结果。
//!
//! 而**"在哪个标高剖切"、"沿哪个方向投影"恰恰由基准元素决定**
//! （设计文档 4.6.1 理由③）。所以 [`View::datum_ref`] 不是可选项：
//!
//! | 视图类型 | 怎么生成 |
//! |---|---|
//! | 平面图 Plan | 取一个**标高**，在该高度水平剖切后向下投影 |
//! | 立面图 Elevation | 沿一个**轴网方向**水平投影 |
//! | 剖面图 Section | 垂直于某根轴 / 某条参考线剖切 |
//! | 三维视图 ThreeD | 任意视点，不做消隐（或做体着色消隐）|
//!
//! **没有基准元素，就没有平立剖视图体系**——这是"基准必须内置"的最硬理由。

use core_model::{DatumKind, ElementId, ViewId};
use glam::DVec3;
use serde::{Deserialize, Serialize};

use crate::annotation::Annotation;
use crate::entity::Entity2D;
use crate::error::{DocError, DocResult};

/// 视图类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ViewKind {
    /// 平面图（俯视，水平剖切）
    Plan,
    /// 立面图（正投影）
    Elevation,
    /// 剖面图（剖切 + 投影）
    Section,
    /// 详图（局部放大）
    Detail,
    /// 三维视图
    ThreeD,
    /// 纯绘图视图（不挂模型，手工画）
    Drafting,
}

impl ViewKind {
    /// 该类型是否**必须**有基准元素才能生成（设计文档 4.6.1 理由③）。
    #[must_use]
    pub const fn requires_datum(self) -> bool {
        matches!(self, Self::Plan | Self::Elevation | Self::Section)
    }

    /// 该类型是否做工程图级消隐（`HLRBRep`）。
    ///
    /// 三维视图通常只做体着色，不产生可编辑的二维线。
    #[must_use]
    pub const fn needs_hlr(self) -> bool {
        matches!(
            self,
            Self::Plan | Self::Elevation | Self::Section | Self::Detail
        )
    }

    /// 中文名（界面 / 图纸名用）。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Plan => "平面图",
            Self::Elevation => "立面图",
            Self::Section => "剖面图",
            Self::Detail => "详图",
            Self::ThreeD => "三维视图",
            Self::Drafting => "绘图视图",
        }
    }
}

/// 视图朝向：**投影矩阵的源头**。
///
/// 「投影与消隐是同一件事的两半」——这个刚体变换既喂给 HLR（决定消隐方向），
/// 也喂给二维投影（决定落到图纸上的坐标）。所以它就是能力④"二维投影"里
/// **唯一需要自写的那部分**（视图变换矩阵），其余交给
/// `occt-wasm::project_edges` 与 `kurbo`。
///
/// ⚠ 这里**刻意不复用 `core_kernel::ViewDef`**：那样 `core-doc` 就要依赖内核层。
/// 由 `core-app` 负责二者的转换（保持依赖方向单一）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ViewOrientation {
    /// 视点（射线起点）
    pub origin: DVec3,
    /// 投影方向（由模型指向观察者）
    pub direction: DVec3,
    /// 视图"向右"的参考轴
    pub x_axis: DVec3,
}

impl ViewOrientation {
    /// 由三分量构造。
    #[must_use]
    pub fn new(origin: DVec3, direction: DVec3, x_axis: DVec3) -> Self {
        Self {
            origin,
            direction,
            x_axis,
        }
    }

    /// 俯视（平面图）：从上往下看，世界 +X 向右。
    #[must_use]
    pub fn top() -> Self {
        Self::new(DVec3::new(0.0, 0.0, 1.0), DVec3::NEG_Z, DVec3::X)
    }

    /// 正立面（前视）：从 -Y 方向看，世界 +X 向右，世界 +Z 向上。
    #[must_use]
    pub fn front() -> Self {
        Self::new(DVec3::new(0.0, -1.0, 0.0), DVec3::Y, DVec3::X)
    }

    /// 视图平面的法向（= 投影方向）。
    #[must_use]
    pub fn normal(&self) -> DVec3 {
        self.direction
    }
}

impl Default for ViewOrientation {
    fn default() -> Self {
        Self::top()
    }
}

/// 一个视图。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct View {
    /// 持久 id（同时登记在 `Document::views`）
    pub id: ViewId,
    /// 视图名（如 `"一层平面图"`）
    pub name: String,
    /// 类型
    pub kind: ViewKind,
    /// 朝向（投影矩阵源头）
    pub orientation: ViewOrientation,
    /// **生成依据的基准元素**（标高 / 轴网 / 参考面 / 参考线）
    pub datum_ref: Option<ElementId>,
    /// 剖切高度（mm，绝对标高；平面图与剖面图用）
    pub cut_height_mm: Option<f64>,
    /// 视图比例分母（如 100 表示 1:100）
    pub scale_denominator: u32,
    /// 二维图元
    pub entities: Vec<Entity2D>,
    /// 标注
    pub annotations: Vec<Annotation>,
    /// 该视图是否已随模型更新过（关联更新状态位）
    pub up_to_date: bool,
}

impl View {
    /// 构造一个空视图。
    #[must_use]
    pub fn new(name: impl Into<String>, kind: ViewKind) -> Self {
        Self {
            id: ViewId::new(),
            name: name.into(),
            kind,
            orientation: ViewOrientation::default(),
            datum_ref: None,
            cut_height_mm: None,
            scale_denominator: 100,
            entities: Vec::new(),
            annotations: Vec::new(),
            up_to_date: false,
        }
    }

    /// 一张"一层平面图"。
    #[must_use]
    pub fn plan(name: impl Into<String>, level: ElementId, cut_height_mm: f64) -> Self {
        let mut v = Self::new(name, ViewKind::Plan);
        v.orientation = ViewOrientation::top();
        v.datum_ref = Some(level);
        v.cut_height_mm = Some(cut_height_mm);
        v
    }

    /// 从**标高**生成一张平面图（设计文档 4.6.1）。
    ///
    /// 这是 [`View::plan`] 的"带校验 + 从标高读高度"强化版：它**在构造时**就确认
    /// `level` 确实是文档里的一张标高，而不是一张轴网 / 参考面。
    ///
    /// - 校验 `level` 确为文档中的 [`DatumKind::Level`]；否则返回
    ///   [`DocError::WrongDatumKind`]。
    /// - `cut_above_level_mm`：剖切高度 = **该标高的标高值 + 此增量**（常见取 1200 mm）。
    /// - 朝向取 [`ViewOrientation::top()`]（俯视、水平剖切）。
    ///
    /// # Errors
    ///
    /// - 文档里**没有**这个 id（或它不是基准元素）→ [`DocError::MissingDatum`]。
    /// - 该 id 存在但不是标高 → [`DocError::WrongDatumKind`]。
    /// - 该标高**没有标高值**（非经 [`core_model::DatumElement::level`] 构造）→
    ///   [`DocError::MissingDatum`]。
    pub fn from_level(
        name: impl Into<String>,
        doc: &core_model::Document,
        level: core_model::ElementId,
        cut_above_level_mm: f64,
    ) -> crate::DocResult<Self> {
        let datum = doc.datum(level).ok_or(DocError::MissingDatum(level))?;
        if !datum.is_kind(DatumKind::Level) {
            return Err(DocError::WrongDatumKind {
                id: level,
                expected: DatumKind::Level,
                found: datum.kind,
            });
        }
        let elevation = datum.elevation().ok_or(DocError::MissingDatum(level))?;

        let mut v = Self::new(name, ViewKind::Plan);
        v.orientation = ViewOrientation::top();
        v.datum_ref = Some(level);
        v.cut_height_mm = Some(elevation + cut_above_level_mm);
        Ok(v)
    }

    /// 从**轴网**生成一张立面图（设计文档 4.6.1）。
    ///
    /// - 校验 `grid` 确为文档中的 [`DatumKind::Grid`]；否则返回
    ///   [`DocError::WrongDatumKind`]。
    /// - `direction` **由调用方显式传入**：`DatumElement` **不存储**轴网方向
    ///   （见规划 §0.7），故不能从文档里读，必须由调用方给出。
    ///   - [`GridDirection::Horizontal`] 横轴 → 从 `-Y` 侧看（[`ViewOrientation::front()`]）；
    ///   - [`GridDirection::Vertical`] 纵轴 → 从 `+X` 侧看（视线方向 `-X`，等价于
    ///     `ViewOrientation::new(DVec3::X, DVec3::NEG_X, DVec3::Y)`）。
    ///
    /// > ⚠ 这里的朝向是**约定**而非几何推导（轴网无存储方向，其 `geom_ref` 也可能为空）。
    /// > P1 若轴网挂了 `geom_ref`，再从线几何推真实方位。
    ///
    /// # Errors
    ///
    /// - 文档里**没有**这个 id（或它不是基准元素）→ [`DocError::MissingDatum`]。
    /// - 该 id 存在但不是轴网 → [`DocError::WrongDatumKind`]。
    pub fn from_grid(
        name: impl Into<String>,
        doc: &core_model::Document,
        grid: core_model::ElementId,
        direction: core_model::GridDirection,
    ) -> crate::DocResult<Self> {
        let datum = doc.datum(grid).ok_or(DocError::MissingDatum(grid))?;
        if !datum.is_kind(DatumKind::Grid) {
            return Err(DocError::WrongDatumKind {
                id: grid,
                expected: DatumKind::Grid,
                found: datum.kind,
            });
        }

        let orientation = match direction {
            core_model::GridDirection::Horizontal => ViewOrientation::front(),
            // 纵轴 → 从 +X 侧看：视线方向 -X，向右轴取 +Y（与 +Z 朝上构成右手系）。
            core_model::GridDirection::Vertical => {
                ViewOrientation::new(DVec3::X, DVec3::NEG_X, DVec3::Y)
            }
        };

        let mut v = Self::new(name, ViewKind::Elevation);
        v.orientation = orientation;
        v.datum_ref = Some(grid);
        Ok(v)
    }

    /// 指定朝向（链式）。
    #[must_use]
    pub fn with_orientation(mut self, orientation: ViewOrientation) -> Self {
        self.orientation = orientation;
        self
    }

    /// 指定比例（链式）。
    #[must_use]
    pub fn with_scale(mut self, denominator: u32) -> Self {
        self.scale_denominator = denominator;
        self
    }

    /// 加入一条图元。
    pub fn push_entity(&mut self, entity: Entity2D) {
        self.entities.push(entity);
        self.up_to_date = false;
    }

    /// 加入一个标注。
    pub fn push_annotation(&mut self, annotation: Annotation) {
        self.annotations.push(annotation);
        self.up_to_date = false;
    }

    /// 校验收图前的前置条件：需要基准元素的视图**必须**挂上基准元素。
    ///
    /// 这是把设计文档 4.6.1 理由③变成**可执行断言**的地方——
    /// 而不是一句写在文档里没人管的话。
    pub fn ensure_generatable(&self) -> DocResult<()> {
        if self.kind.requires_datum() && self.datum_ref.is_none() {
            return Err(DocError::UnsupportedViewKind(format!(
                "{}（{}）缺少基准元素，无法生成",
                self.name,
                self.kind.label()
            )));
        }
        Ok(())
    }

    /// 该视图里所有带溯源的图元和标注所引用的模型拓扑键。
    ///
    /// 喂给 [`crate::DependencyGraph`] 建索引。
    #[must_use]
    pub fn geometry_keys(&self) -> Vec<core_model::GeometryKey> {
        self.entities
            .iter()
            .filter_map(|e| e.source.clone())
            .chain(self.annotations.iter().filter_map(|a| a.bound_to.clone()))
            .collect()
    }

    /// 标出"需要重新生成"（模型变了之后由关联更新引擎调用）。
    pub fn mark_stale(&mut self) {
        self.up_to_date = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::GridDirection;

    #[test]
    fn plan_view_requires_a_datum() {
        let bare = View::new("未挂标高的平面", ViewKind::Plan);
        assert!(matches!(
            bare.ensure_generatable(),
            Err(DocError::UnsupportedViewKind(_))
        ));
    }

    #[test]
    fn plan_view_with_a_level_is_generatable() {
        let v = View::plan("一层平面图", ElementId::new(), 0.0);
        assert!(v.ensure_generatable().is_ok());
        assert_eq!(v.kind, ViewKind::Plan);
        assert_eq!(v.cut_height_mm, Some(0.0));
        assert_eq!(v.scale_denominator, 100);
    }

    #[test]
    fn three_d_and_drafting_views_need_no_datum() {
        assert!(
            View::new("三维", ViewKind::ThreeD)
                .ensure_generatable()
                .is_ok()
        );
        assert!(
            View::new("手绘", ViewKind::Drafting)
                .ensure_generatable()
                .is_ok()
        );
    }

    #[test]
    fn hlr_is_needed_for_engineering_views_only() {
        assert!(ViewKind::Plan.needs_hlr());
        assert!(ViewKind::Section.needs_hlr());
        assert!(!ViewKind::ThreeD.needs_hlr());
        assert!(!ViewKind::Drafting.needs_hlr());
    }

    #[test]
    fn pushing_content_marks_the_view_stale() {
        let mut v = View::plan("一层", ElementId::new(), 0.0);
        v.up_to_date = true;
        v.push_annotation(crate::annotation::Annotation::linear(
            glam::DVec2::ZERO,
            glam::DVec2::new(1.0, 0.0),
        ));
        assert!(!v.up_to_date, "内容变了就要标脏，等关联更新重算");
    }

    #[test]
    fn geometry_keys_collects_entity_and_annotation_sources() {
        use core_model::{GeometryKey, ShapeId};

        let shape = ShapeId::new();
        let mut v = View::plan("一层", ElementId::new(), 0.0);
        v.push_entity(
            crate::entity::Entity2D::line(
                core_model::LayerId::new(),
                glam::DVec2::ZERO,
                glam::DVec2::new(1.0, 0.0),
            )
            .from_geometry(GeometryKey::edge(shape, 0)),
        );
        v.push_annotation(
            crate::annotation::Annotation::linear(glam::DVec2::ZERO, glam::DVec2::new(1.0, 0.0))
                .bound_to(GeometryKey::edge(shape, 1)),
        );
        assert_eq!(v.geometry_keys().len(), 2);
    }

    #[test]
    fn orientation_presets_are_orthogonal_and_sane() {
        let top = ViewOrientation::top();
        assert_eq!(top.direction, DVec3::NEG_Z);
        assert_eq!(top.normal(), DVec3::NEG_Z);

        let front = ViewOrientation::front();
        assert_eq!(front.direction, DVec3::Y);
        // 视线方向与"向右"轴必须正交，否则投影矩阵会歪
        assert!((front.direction.dot(front.x_axis)).abs() < 1e-12);
    }

    #[test]
    fn view_roundtrips_through_json() {
        let v = View::plan("一层平面图", ElementId::new(), 0.0).with_scale(50);
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(serde_json::from_str::<View>(&json).unwrap(), v);
    }

    // ---------- M5 / B2：视图生成器 from_level / from_grid ----------

    /// 建一份只含一张标高的文档，返回 `(doc, 标高 id)`。
    fn doc_with_level(elevation_mm: f64) -> (core_model::Document, ElementId) {
        let mut doc = core_model::Document::new("测试文档");
        let level = core_model::DatumElement::level("F1", elevation_mm);
        let id = level.id;
        doc.insert_datum(level).unwrap();
        (doc, id)
    }

    /// 建一份只含一根轴网的文档，返回 `(doc, 轴网 id)`。
    fn doc_with_grid() -> (core_model::Document, ElementId) {
        let mut doc = core_model::Document::new("测试文档");
        let grid = core_model::DatumElement::grid("1");
        let id = grid.id;
        doc.insert_datum(grid).unwrap();
        (doc, id)
    }

    #[test]
    fn from_level_rejects_a_grid_datum() {
        // 类型校验钉子：拿轴网的 id 去生成平面图，必须当场报错，
        // 而不是放一张"挂错基准"的图出去。
        let (doc, grid_id) = doc_with_grid();
        let err = View::from_level("错挂的平面图", &doc, grid_id, 1200.0).unwrap_err();
        assert!(
            matches!(
                err,
                DocError::WrongDatumKind {
                    id,
                    expected: DatumKind::Level,
                    found: DatumKind::Grid,
                } if id == grid_id
            ),
            "传轴网给 from_level 必须返回 WrongDatumKind，实际：{err:?}"
        );
    }

    #[test]
    fn from_grid_rejects_a_level_datum() {
        // 类型校验钉子：拿标高的 id 去生成立面图，必须当场报错。
        let (doc, level_id) = doc_with_level(3000.0);
        let err =
            View::from_grid("错挂的立面图", &doc, level_id, GridDirection::Horizontal).unwrap_err();
        assert!(
            matches!(
                err,
                DocError::WrongDatumKind {
                    id,
                    expected: DatumKind::Grid,
                    found: DatumKind::Level,
                } if id == level_id
            ),
            "传标高给 from_grid 必须返回 WrongDatumKind，实际：{err:?}"
        );
    }

    #[test]
    fn from_level_reads_elevation_and_sets_cut_height() {
        let (doc, level_id) = doc_with_level(3000.0);
        let v = View::from_level("一层平面图", &doc, level_id, 1200.0).unwrap();
        assert_eq!(
            v.cut_height_mm,
            Some(4200.0),
            "剖切高度 = 标高 3000 + 增量 1200"
        );
        assert_eq!(v.kind, ViewKind::Plan);
        assert_eq!(v.datum_ref, Some(level_id));
        assert_eq!(v.orientation, ViewOrientation::top());
    }

    #[test]
    fn from_grid_horizontal_uses_front_orientation() {
        let (doc, grid_id) = doc_with_grid();
        let v = View::from_grid("A 立面图", &doc, grid_id, GridDirection::Horizontal).unwrap();
        assert_eq!(v.kind, ViewKind::Elevation);
        assert_eq!(v.orientation, ViewOrientation::front());
        assert_eq!(v.datum_ref, Some(grid_id));
    }

    #[test]
    fn from_grid_vertical_uses_positive_x_orientation() {
        let (doc, grid_id) = doc_with_grid();
        let v = View::from_grid("1 立面图", &doc, grid_id, GridDirection::Vertical).unwrap();
        assert_eq!(v.kind, ViewKind::Elevation);
        // 视线方向必须与世界 +X 轴共线（从 +X 侧看向原点）。
        assert_eq!(
            v.orientation.direction.dot(DVec3::X).abs(),
            1.0,
            "纵轴立面视线方向应与 +X 轴共线，实际：{:?}",
            v.orientation.direction
        );
        assert_eq!(v.orientation.direction, DVec3::NEG_X);
        // "向右"轴必须与视线方向正交，否则投影矩阵会歪。
        assert!(v.orientation.direction.dot(v.orientation.x_axis).abs() < 1e-12);
    }

    #[test]
    fn from_level_on_unknown_element_is_an_error() {
        let (doc, _) = doc_with_level(0.0);
        // 文档里不存在的 id：既不是标高也不是轴网，必须 Err（不许静默造一张图）。
        let missing = ElementId::new();
        assert!(View::from_level("幽灵平面图", &doc, missing, 1200.0).is_err());
    }
}

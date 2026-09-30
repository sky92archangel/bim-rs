//! HLR 结果 → 二维实体接线（设计文档 6.2）。
//!
//! ## 为什么在 core-app
//!
//! `core-doc` 铁律**不得依赖 `core-kernel`**（`core-doc/src/lib.rs:19`）；
//! 本模块是唯一同时看得见两层的落点。转换后把 [`Entity2D`] 写回 [`View`]。
//!
//! ```text
//! core_kernel::Projection ──(本模块)──▶ Vec<core_doc::Entity2D>
//!        ▲                                     │ 每条带 source: GeometryKey
//!        └── project_edges（HLR 算法，内核全权负责）
//! ```
//!
//! ## 三条映射（设计文档 6.2）
//!
//! - 可见 · `Sharp`（真实棱线）→ `LAYER_OUTLINE`，**粗**实线；
//! - 可见 · `Outline`（轮廓）/ `Smooth`（光顺）→ `LAYER_OUTLINE`，**细**实线；
//! - 隐藏（任意类）→ `LAYER_HIDDEN`，虚线。
//!
//! ## 溯源到 shape 级（诚实边界）
//!
//! `project_edges` 一次只投一个形状，故本批曲线**必然**属于该形状，
//! `source = GeometryKey::whole(shape)` 是**真值可判定**的。
//! edge / face 级溯源被设计文档 6.6.5 的缺口挡住（facade 不暴露"HLR 输出 ← 原始边"的
//! 映射），**不做假绑定**（假绑定会让关联更新"看起来对、实际乱指"）。
//!
//! ## 能力缺口（绝不静默）
//!
//! - **椭圆**：M5 B5.1 起 `Entity2D` 已带长短半轴、`Analytic2D` 已有 `Ellipse`，
//!   故投影出的椭圆构造为**精确** [`Entity2D::ellipse`]（不再降级为折线）。
//! - **B 样条 / 其它**：仍**丢弃并计入警告**（不引入未就绪的采样依赖）；属 B5 后续。

use core_doc::standard::{LAYER_HIDDEN, LAYER_OUTLINE};
use core_doc::{
    DraftingStandard, Entity2D, EntityKind, EntityStyle, LineType, View, ViewOrientation,
};
use core_kernel::{
    CurveGeometry, CurveKind, Kernel, LineClass, ProjectedCurve, Projection, ViewDef,
};
use core_model::{Document, GeometryKey, LayerId, ShapeId};
use glam::DVec2;

use crate::error::AppResult;
use crate::geometry_service::ShapeLoader;
use crate::loader::BrepShapeLoader;

/// 粗实线线宽（mm，图纸实际宽度）——可见棱线用。
const THICK_WIDTH_MM: f64 = 0.50;
/// 细实线线宽（mm，图纸实际宽度）——可见轮廓 / 光顺边用。
const THIN_WIDTH_MM: f64 = 0.25;

/// core-doc 的视图朝向 → 内核的视图定义。
///
/// ★ **本函数是方向符号的唯一约定点**（m5 §0.1）。
///
/// # 方向约定（据实核实，非照抄）
///
/// 两侧的符号约定**相反**，证据：
///
/// - `core_doc::ViewOrientation`：文档字面写"由模型指向观察者"，但
///   `ViewOrientation::top()` 返回 `direction = NEG_Z`（`core-doc/src/view.rs:109-111`）。
///   俯视时观察者在上方，"模型→观察者"应为 `+Z`——**字面自相矛盾**；
///   唯一自洽的读法是它其实表示**视线方向（观察者→物体）**：观察者在上往下看
///   → `-Z`。本函数采此读法。
/// - `core_kernel::ViewDef`：文档写"由物体指向观察者"（`core-kernel/src/types.rs:134-141`），
///   `ViewDef::top()` 返回 `direction = +Z`（`types.rs:163-165`）——**与 core-doc 相反**。
/// - `core-kernel/src/occt.rs:618-643` 把 `view.direction` **原样**透传给内核
///   `project_edges`；既有内核钉子 `projection_of_holed_plate_yields_analytic_circle`
///   （`occt.rs:968`）用 `ViewDef::top(origin_z=1000)`（direction `+Z`）能沿孔轴判出解析圆，
///   佐证内核域内 `+Z` 才是俯视的"物体→观察者"。
///
/// 结论：**取反**。core-doc 的"观察者→物体"取反即得内核要的"物体→观察者"。
/// 该符号由钉子测试 `view_def_conversion_flips_direction_per_kernel_convention` 焊死：
/// 谁改错方向它立刻红。`origin` / `x_axis` 语义一致，原样透传。
#[must_use]
pub fn view_def_of(orientation: &ViewOrientation) -> ViewDef {
    ViewDef {
        origin: orientation.origin,
        // ★ 唯一取反点：观察者→物体 ——(取反)——▶ 物体→观察者
        direction: -orientation.direction,
        x_axis: Some(orientation.x_axis),
    }
}

/// 该视图 P0 应当投影的几何集合。
///
/// P0 口径：文档中**被元素引用**的全部几何记录；按 `ShapeId` 升序返回（结果可复现）。
#[must_use]
pub fn projectable_shapes(doc: &Document) -> Vec<ShapeId> {
    use std::collections::BTreeSet;
    let mut shapes: BTreeSet<ShapeId> = BTreeSet::new();
    for element in doc.elements().values() {
        if let Some(shape) = element.geometry_ref {
            shapes.insert(shape);
        }
    }
    shapes.into_iter().collect()
}

/// 投影一个形状 → 二维图元（每条带 shape 级溯源）。
///
/// 返回 `(图元, 警告)`；警告非空表示有曲线被**降级 / 丢弃**，或该形状投影为空
/// （绝不静默）。
///
/// # 与规划的一处偏差（据实）
///
/// m5 §2.1 的原签名是 `project_shape(kernel, shape: ShapeId, view, standard)`。
/// 但 `ShapeId`（持久 UUID）→ `ShapeHandle`（会话句柄）的解析必须读
/// `Document` 里的 `GeometryRecord` 才能装载（见 `GeometryService::resolve`），
/// 单凭 `kernel` 无法完成。故此处**增加 `doc: &Document` 参数**：
/// `project_shape` 自行从文档装载 BREP → 投影 → 释放句柄。
pub fn project_shape(
    kernel: &mut Kernel,
    doc: &Document,
    shape: ShapeId,
    view: &View,
    standard: &DraftingStandard,
) -> AppResult<(Vec<Entity2D>, Vec<String>)> {
    require_standard_layers(standard)?;

    let record = doc
        .geometry_record(shape)
        .ok_or(core_model::ModelError::GeometryNotFound(shape))?;

    // 装载（惰性重建的一半）→ 投影 → **无论成败都释放句柄**（避免 arena 泄漏）。
    let handle = BrepShapeLoader::new().load(kernel, record)?;
    let projected = kernel.project_edges(handle, view_def_of(&view.orientation));
    let _ = kernel.release(handle);
    let projection = projected?;

    let mut warnings = Vec::new();
    if projection.is_empty() {
        warnings.push(format!(
            "形状 {shape} 在本视图没有任何可见 / 隐藏线（合法状态，不报错）"
        ));
    }

    let (entities, mut convert_warnings) = entities_from_projection(&projection, shape, standard);
    warnings.append(&mut convert_warnings);
    Ok((entities, warnings))
}

/// 投影整张视图（**全量重投**，供 B2 初生成与 B3 兜底路径）。
///
/// 不直接改 `view`——返回图元列表与警告，由调用方决定写回（便于测试）。
pub fn project_view(
    kernel: &mut Kernel,
    doc: &Document,
    view: &View,
    standard: &DraftingStandard,
) -> AppResult<(Vec<Entity2D>, Vec<String>)> {
    require_standard_layers(standard)?;

    let mut entities = Vec::new();
    let mut warnings = Vec::new();
    for shape in projectable_shapes(doc) {
        let (mut es, mut ws) = project_shape(kernel, doc, shape, view, standard)?;
        entities.append(&mut es);
        warnings.append(&mut ws);
    }
    Ok((entities, warnings))
}

/// **全量重投并写回**：`view.entities` 清空重建，返回 `(写入条数, 警告)`。
///
/// 调用后由调用方（B3）调 `Drawing::rebuild_index` 重建关联索引。
/// 写回后视图即"已随模型更新"，故置 `up_to_date = true`。
///
/// 警告（曲线降级 / 丢弃 / 投影为空）**不再被吞**：如实返回给调用方
/// （修 P1-2：原先内部 `_warnings` 被丢弃，降级 / 丢弃无从观测）。
pub fn reproject_into(
    kernel: &mut Kernel,
    doc: &Document,
    view: &mut View,
    standard: &DraftingStandard,
) -> AppResult<(usize, Vec<String>)> {
    let (entities, warnings) = project_view(kernel, doc, view, standard)?;
    let written = entities.len();
    view.entities = entities;
    view.up_to_date = true;
    Ok((written, warnings))
}

/// 单条投影曲线 → 图元（分型 + 分层 + 溯源）。
///
/// 返回 `None` 表示**无法表达**（会被计入警告）：P0 丢弃 `BSpline` 与 `Other`。
#[must_use]
pub fn curve_to_entity(
    curve: &ProjectedCurve,
    hidden: bool,
    source: GeometryKey,
    standard: &DraftingStandard,
) -> Option<Entity2D> {
    let (layer, style) = layer_and_style(curve.layer_hint, hidden, standard);

    let entity = match (curve.kind, &curve.data) {
        (CurveKind::Line, CurveGeometry::Line { a, b }) => Entity2D::line(layer, *a, *b),
        (CurveKind::Circle, CurveGeometry::Circle { center, radius }) => {
            Entity2D::circle(layer, *center, *radius)
        }
        (
            CurveKind::Arc,
            CurveGeometry::Arc {
                center,
                radius,
                start,
                end,
            },
        ) => {
            let mut e = Entity2D::new(
                EntityKind::Arc,
                layer,
                vec![
                    *center,
                    arc_point(*center, *radius, *start),
                    arc_point(*center, *radius, *end),
                ],
            );
            e.radius = Some(*radius);
            e
        }
        // 精确椭圆：`Entity2D` 已带长短半轴（M5 B5.1），不再降级为折线。
        (
            CurveKind::Ellipse,
            CurveGeometry::Ellipse {
                center,
                rx,
                ry,
                rot,
            },
        ) => {
            // 约定 `semi_major >= semi_minor`；若内核给的 rx < ry 则交换半轴，
            // 并把主轴方位角加 π/2，保证旋转后几何与采样一致。
            let (major, minor, rotation) = if *rx >= *ry {
                (*rx, *ry, *rot)
            } else {
                (*ry, *rx, *rot + std::f64::consts::FRAC_PI_2)
            };
            Entity2D::ellipse(layer, *center, major, minor, rotation)
        }
        // P0 丢弃：无法表达（绝不放行假几何）。
        (CurveKind::BSpline, _) | (CurveKind::Other, _) => return None,
        // 种类与几何不一致（不应发生）——按"无法表达"处理，绝不猜。
        _ => return None,
    };

    Some(entity.with_style(style).from_geometry(source))
}

/// 按 `CurveKind` 把一批投影曲线转成图元，并收集丢弃警告。
///
/// 警告规则：`BSpline` / `Other` → "被丢弃"。椭圆已能精确表达（M5 B5.1），
/// **不再降级、不再计入警告**。**绝不静默丢东西**。
fn entities_from_projection(
    projection: &Projection,
    shape: ShapeId,
    standard: &DraftingStandard,
) -> (Vec<Entity2D>, Vec<String>) {
    let source = GeometryKey::whole(shape);
    let mut entities = Vec::new();
    let mut warnings = Vec::new();

    for (curves, hidden) in [(&projection.visible, false), (&projection.hidden, true)] {
        for curve in curves {
            match curve_to_entity(curve, hidden, source.clone(), standard) {
                Some(entity) => entities.push(entity),
                None => warnings.push(format!(
                    "形状 {shape}：无法表达的曲线被丢弃（绝不静默）；\
                     kind={:?}, layer_hint={:?}",
                    curve.kind, curve.layer_hint
                )),
            }
        }
    }

    (entities, warnings)
}

/// 线型分类 + 可见性 → 图层 id 与样式（设计文档 6.2 的三条映射）。
///
/// - 可见 · `Sharp`（棱线）→ `LAYER_OUTLINE`，**粗**实线；
/// - 可见 · `Outline`（轮廓）/ `Smooth`（光顺）→ `LAYER_OUTLINE`，**细**实线；
/// - 隐藏（任意类）→ `LAYER_HIDDEN`，虚线。
fn layer_and_style(
    class: LineClass,
    hidden: bool,
    standard: &DraftingStandard,
) -> (LayerId, EntityStyle) {
    if hidden {
        let layer = layer_id(standard, LAYER_HIDDEN);
        let style = EntityStyle {
            line_type: Some(LineType::Dashed),
            width_mm: Some(THIN_WIDTH_MM),
            color: None,
        };
        (layer, style)
    } else {
        let layer = layer_id(standard, LAYER_OUTLINE);
        let width = match class {
            LineClass::Sharp => THICK_WIDTH_MM,
            LineClass::Outline | LineClass::Smooth => THIN_WIDTH_MM,
        };
        let style = EntityStyle {
            line_type: Some(LineType::Continuous),
            width_mm: Some(width),
            color: None,
        };
        (layer, style)
    }
}

/// 按名取图层 id；标准未定义该图层时退化为一次性 id（由 [`require_standard_layers`] 前置拦截）。
fn layer_id(standard: &DraftingStandard, name: &str) -> LayerId {
    standard
        .layer_by_name(name)
        .map_or_else(LayerId::new, |l| l.id)
}

/// 前置校验：投影必须用到 `LAYER_OUTLINE` / `LAYER_HIDDEN` 两个图层。
///
/// 缺图层是**配置错误**，须响亮报错，而不是静默把图元塞进一个凭空新造的图层。
fn require_standard_layers(standard: &DraftingStandard) -> AppResult<()> {
    standard.require_layer(LAYER_OUTLINE)?;
    standard.require_layer(LAYER_HIDDEN)?;
    Ok(())
}

/// 圆弧上角度 `angle`（弧度）处的点。
fn arc_point(center: DVec2, radius: f64, angle: f64) -> DVec2 {
    center + DVec2::new(radius * angle.cos(), radius * angle.sin())
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::ShapeId;
    use glam::DVec3;
    use std::f64::consts::FRAC_PI_2;

    fn standard() -> DraftingStandard {
        DraftingStandard::gb_industrial()
    }

    fn line_curve(a: (f64, f64), b: (f64, f64), class: LineClass) -> ProjectedCurve {
        ProjectedCurve {
            kind: CurveKind::Line,
            data: CurveGeometry::Line {
                a: DVec2::new(a.0, a.1),
                b: DVec2::new(b.0, b.1),
            },
            layer_hint: class,
        }
    }

    fn circle_curve(center: (f64, f64), radius: f64) -> ProjectedCurve {
        ProjectedCurve {
            kind: CurveKind::Circle,
            data: CurveGeometry::Circle {
                center: DVec2::new(center.0, center.1),
                radius,
            },
            layer_hint: LineClass::Outline,
        }
    }

    #[test]
    fn view_def_conversion_flips_direction_per_kernel_convention() {
        // ★ 方向符号钉子：改错方向此测试立刻红。
        let top = view_def_of(&ViewOrientation::top());
        assert_eq!(
            top.direction,
            DVec3::Z,
            "core-doc 俯视 direction=NEG_Z（观察者→物体），取反应得 +Z；与内核 ViewDef::top 同域"
        );
        assert_eq!(
            top.direction,
            ViewDef::top(DVec3::ZERO).direction,
            "必须与内核自身的 ViewDef::top 落在同一符号域"
        );

        let front = view_def_of(&ViewOrientation::front());
        assert_eq!(
            front.direction,
            DVec3::NEG_Y,
            "core-doc 正视 direction=+Y，取反应得 -Y"
        );
        assert_eq!(front.direction, ViewDef::front(DVec3::ZERO).direction);
    }

    #[test]
    fn visible_sharp_curves_go_to_outline_layer_and_thick_style() {
        let std = standard();
        let curve = line_curve((0.0, 0.0), (1.0, 0.0), LineClass::Sharp);
        let e = curve_to_entity(&curve, false, GeometryKey::whole(ShapeId::new()), &std).unwrap();

        let outline = std.layer_by_name(LAYER_OUTLINE).unwrap();
        assert_eq!(e.layer, outline.id, "Sharp 可见应落在 OUTLINE");
        assert_eq!(e.style.width_mm, Some(THICK_WIDTH_MM), "Sharp 应为粗实线");
        assert_eq!(e.style.line_type, Some(LineType::Continuous));
    }

    #[test]
    fn visible_smooth_and_outline_go_to_outline_layer_thin() {
        let std = standard();
        let outline = std.layer_by_name(LAYER_OUTLINE).unwrap();
        for class in [LineClass::Smooth, LineClass::Outline] {
            let curve = line_curve((0.0, 0.0), (1.0, 0.0), class);
            let e =
                curve_to_entity(&curve, false, GeometryKey::whole(ShapeId::new()), &std).unwrap();
            assert_eq!(e.layer, outline.id, "{class:?} 可见应落在 OUTLINE");
            assert_eq!(
                e.style.width_mm,
                Some(THIN_WIDTH_MM),
                "{class:?} 应为细实线"
            );
        }
        // 粗必须真的比细宽：用两条实际图元的线宽比，而非常量断言
        let sharp = curve_to_entity(
            &line_curve((0.0, 0.0), (1.0, 0.0), LineClass::Sharp),
            false,
            GeometryKey::whole(ShapeId::new()),
            &std,
        )
        .unwrap();
        let thin = curve_to_entity(
            &line_curve((0.0, 0.0), (1.0, 0.0), LineClass::Smooth),
            false,
            GeometryKey::whole(ShapeId::new()),
            &std,
        )
        .unwrap();
        assert!(
            sharp.style.width_mm > thin.style.width_mm,
            "Sharp 线宽必须大于 Smooth：{:?} vs {:?}",
            sharp.style.width_mm,
            thin.style.width_mm
        );
    }

    #[test]
    fn hidden_curves_go_to_hidden_layer_dashed() {
        let std = standard();
        let hidden = std.layer_by_name(LAYER_HIDDEN).unwrap();
        for class in [LineClass::Sharp, LineClass::Outline, LineClass::Smooth] {
            let curve = line_curve((0.0, 0.0), (1.0, 0.0), class);
            let e =
                curve_to_entity(&curve, true, GeometryKey::whole(ShapeId::new()), &std).unwrap();
            assert_eq!(e.layer, hidden.id, "{class:?} 隐藏应落在 HIDDEN");
            assert_eq!(e.style.line_type, Some(LineType::Dashed), "隐藏线必须虚线");
        }
    }

    #[test]
    fn analytic_circle_projection_becomes_circle_entity_not_polyline() {
        // 守验收③"圆是真圆"的上游：解析圆必须保持 Circle 形态。
        let std = standard();
        let curve = circle_curve((3.0, 4.0), 15.0);
        let e = curve_to_entity(&curve, false, GeometryKey::whole(ShapeId::new()), &std).unwrap();

        assert_eq!(e.kind, EntityKind::Circle);
        assert_ne!(e.kind, EntityKind::Polyline, "圆绝不能退化成折线");
        assert_eq!(e.radius, Some(15.0));
        assert_eq!(e.points, vec![DVec2::new(3.0, 4.0)], "圆以圆心为唯一点");
    }

    #[test]
    fn arc_projection_keeps_center_start_end_and_radius() {
        let std = standard();
        let curve = ProjectedCurve {
            kind: CurveKind::Arc,
            data: CurveGeometry::Arc {
                center: DVec2::ZERO,
                radius: 2.0,
                start: 0.0,
                end: FRAC_PI_2,
            },
            layer_hint: LineClass::Outline,
        };
        let e = curve_to_entity(&curve, false, GeometryKey::whole(ShapeId::new()), &std).unwrap();

        assert_eq!(e.kind, EntityKind::Arc);
        assert_eq!(e.radius, Some(2.0));
        assert_eq!(e.points.len(), 3, "圆弧为圆心 + 起终点");
        assert_eq!(e.points[0], DVec2::ZERO);
        assert!(
            (e.points[1] - DVec2::new(2.0, 0.0)).length() < 1e-9,
            "起点在 0°"
        );
        assert!(
            (e.points[2] - DVec2::new(0.0, 2.0)).length() < 1e-9,
            "终点在 90°"
        );
    }

    #[test]
    fn ellipse_projection_is_exact_entity_not_polyline() {
        let std = standard();
        let shape = ShapeId::new();
        let projection = Projection {
            visible: vec![ProjectedCurve {
                kind: CurveKind::Ellipse,
                data: CurveGeometry::Ellipse {
                    center: DVec2::ZERO,
                    rx: 10.0,
                    ry: 4.0,
                    rot: 0.0,
                },
                layer_hint: LineClass::Outline,
            }],
            hidden: Vec::new(),
        };

        let (entities, warnings) = entities_from_projection(&projection, shape, &std);
        assert_eq!(entities.len(), 1);
        assert_eq!(
            entities[0].kind,
            EntityKind::Ellipse,
            "椭圆必须是精确 Ellipse 实体，绝不降级为折线"
        );
        assert_ne!(entities[0].kind, EntityKind::Polyline, "绝不能是折线");
        assert_eq!(entities[0].semi_major, Some(10.0));
        assert_eq!(entities[0].semi_minor, Some(4.0));
        assert_eq!(
            entities[0].radius, None,
            "椭圆不得复用 radius（会退化成同半径圆的语义）"
        );
        assert!(
            warnings.is_empty(),
            "椭圆不再降级，不应产生降级警告：{warnings:?}"
        );
    }

    #[test]
    fn bspline_projection_is_dropped_with_warning() {
        let std = standard();
        let shape = ShapeId::new();
        let projection = Projection {
            visible: vec![ProjectedCurve {
                kind: CurveKind::BSpline,
                data: CurveGeometry::BSpline {
                    ctrl: vec![DVec2::ZERO, DVec2::new(1.0, 1.0)],
                    degree: 1,
                    knots: vec![0.0, 1.0],
                },
                layer_hint: LineClass::Smooth,
            }],
            hidden: Vec::new(),
        };

        let (entities, warnings) = entities_from_projection(&projection, shape, &std);
        assert!(entities.is_empty(), "P0 丢弃 B 样条");
        assert!(!warnings.is_empty(), "丢弃必须计入警告，绝不静默");
    }

    #[test]
    fn every_projected_entity_carries_whole_shape_source() {
        let std = standard();
        let shape = ShapeId::new();
        let projection = Projection {
            visible: vec![
                line_curve((0.0, 0.0), (1.0, 0.0), LineClass::Sharp),
                circle_curve((0.0, 0.0), 5.0),
            ],
            hidden: vec![line_curve((0.0, 0.0), (0.0, 1.0), LineClass::Outline)],
        };

        let (entities, _warnings) = entities_from_projection(&projection, shape, &std);
        assert_eq!(entities.len(), 3);
        for e in &entities {
            assert_eq!(
                e.source,
                Some(GeometryKey::whole(shape)),
                "每条图元都须带 shape 级溯源（P0 不给假绑定）"
            );
        }
    }

    #[test]
    fn project_shape_is_deterministic() {
        let std = standard();
        let shape = ShapeId::new();
        let projection = Projection {
            visible: vec![
                line_curve((0.0, 0.0), (1.0, 2.0), LineClass::Sharp),
                circle_curve((3.0, 3.0), 2.0),
            ],
            hidden: vec![line_curve((0.0, 0.0), (0.0, 1.0), LineClass::Outline)],
        };

        let (first, _) = entities_from_projection(&projection, shape, &std);
        let (second, _) = entities_from_projection(&projection, shape, &std);

        // 逐字段比对（不含随机 EntityId）：kind / layer / points / radius / 样式
        let signature = |es: &[Entity2D]| -> Vec<_> {
            es.iter()
                .map(|e| {
                    (
                        e.kind,
                        e.layer,
                        e.points.clone(),
                        e.radius,
                        e.style.width_mm,
                        e.style.line_type,
                        e.source.clone(),
                    )
                })
                .collect()
        };
        assert_eq!(
            signature(&first),
            signature(&second),
            "同一投影须逐字段可复现"
        );
        assert!(!first.is_empty());
    }

    /// **真实内核腿**：带圆孔板经 `project_view` 落到 `core_doc` 实体。
    ///
    /// 断言：图元非空、含 `EntityKind::Circle`、全部带 `source`、`view.geometry_keys()` 非空。
    /// 内核必须先断言非 mock——**"跳过"与"通过"可区分**，内核不可用即 fail。
    ///
    /// debug 腿下标记为 **ignored**（wasmtime 编译过慢），不再静默 `return` 造"假绿"。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn real_kernel_projection_reaches_core_doc_entities() {
        let mut kernel = Kernel::new()
            .expect("启用 occt feature 后 Kernel::new() 必须成功（occt 腿固定 --release）");
        assert!(
            kernel.is_real(),
            "本测试必须跑真实内核，不许在 mock 上假绿；实得后端 {}",
            kernel.backend_name()
        );

        // 带圆孔板：box - cylinder（沿 Z 俯视应得解析圆）
        let plate = kernel.make_box(100.0, 60.0, 10.0).unwrap();
        let hole = kernel.make_cylinder(15.0, 40.0).unwrap();
        let holed = kernel.cut(plate, hole).unwrap();
        let brep = kernel.export_brep(holed).unwrap();

        let shape = ShapeId::new();
        let mut doc = Document::new("带圆孔板");
        doc.insert_geometry(core_model::GeometryRecord::brep(shape, brep))
            .unwrap();
        doc.insert_element(
            core_model::Element::new(core_model::Category::generic(), "板").with_geometry(shape),
        )
        .unwrap();

        let std = DraftingStandard::gb_industrial();
        let mut view = View::plan("俯视平面", core_model::ElementId::new(), 0.0);

        let (entities, _warnings) = project_view(&mut kernel, &doc, &view, &std).unwrap();
        assert!(!entities.is_empty(), "带圆孔板俯视投影不应为空");
        assert!(
            entities.iter().any(|e| e.kind == EntityKind::Circle),
            "带圆孔板的俯视图必须含解析圆；实得 kinds={:?}",
            entities.iter().map(|e| e.kind).collect::<Vec<_>>()
        );
        assert!(
            entities.iter().all(|e| e.source.is_some()),
            "每条图元都必须带 shape 级溯源"
        );

        let (written, _ws) = reproject_into(&mut kernel, &doc, &mut view, &std).unwrap();
        assert_eq!(written, view.entities.len());
        assert!(!view.geometry_keys().is_empty(), "视图的关联键集合不应为空");
    }

    /// 由内核网格求形状包围盒（min, max）。
    #[cfg(feature = "occt")]
    fn bbox_of(kernel: &mut Kernel, h: core_kernel::ShapeHandle) -> ([f64; 3], [f64; 3]) {
        let mesh = kernel.tessellate(h, 0.5, 0.5).unwrap();
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for p in &mesh.positions {
            for i in 0..3 {
                let v = p[i];
                lo[i] = lo[i].min(v);
                hi[i] = hi[i].max(v);
            }
        }
        (lo, hi)
    }

    /// 统计落在 `OUTLINE`（可见）层的解析圆条数。
    #[cfg(feature = "occt")]
    fn visible_circles(entities: &[Entity2D], std: &DraftingStandard) -> usize {
        let outline = std.layer_by_name(LAYER_OUTLINE).unwrap();
        entities
            .iter()
            .filter(|e| e.kind == EntityKind::Circle && e.layer == outline.id)
            .count()
    }

    /// **方向敏感钉子（P1-1）**：非对称的 +Z 凸台。
    ///
    /// 从 **+Z** 俯视：凸台顶圆可见 → 可见圆数 > 0；
    /// 从 **-Z** 仰视：凸台被底板遮住 → 可见圆数 == 0。
    ///
    /// 两视图都必须非空（底板轮廓仍可见）——**"方向敏感"且"有锚"**，
    /// 防"两边都空"式假绿。若抹掉 [`view_def_of`] 的唯一取反，本用例必红。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn real_kernel_projection_is_direction_sensitive() {
        let mut kernel = Kernel::new()
            .expect("启用 occt feature 后 Kernel::new() 必须成功（occt 腿固定 --release）");
        assert!(
            kernel.is_real(),
            "本测试必须跑真实内核，不许在 mock 上假绿；实得后端 {}",
            kernel.backend_name()
        );

        // 底板 + 顶面凸台：沿 +Z 明显不对称。
        let plate = kernel.make_box(100.0, 60.0, 10.0).unwrap();
        let (p_lo, p_hi) = bbox_of(&mut kernel, plate);
        let cx = (p_lo[0] + p_hi[0]) / 2.0;
        let cy = (p_lo[1] + p_hi[1]) / 2.0;
        let base = kernel.make_cylinder(15.0, 40.0).unwrap();
        // 圆柱底面装配到板顶面 p_hi[2]，并居中于底板之上。
        let (b_lo, _b_hi) = bbox_of(&mut kernel, base);
        let boss = kernel.translate(base, cx, cy, p_hi[2] - b_lo[2]).unwrap();
        let solid = kernel.fuse(plate, boss).unwrap();

        // 几何自检：融合体确在 +Z 方向伸长（凸台高于板顶），否则测试前提不成立。
        let (s_lo, s_hi) = bbox_of(&mut kernel, solid);
        assert!(
            s_hi[2] > p_hi[2] + 1.0,
            "凸台必须明显伸出板顶（板顶 {}，融合体顶 {}）——否则非对称前提不成立",
            p_hi[2],
            s_hi[2]
        );
        assert!(
            (s_lo[2] - p_lo[2]).abs() < 1e-6,
            "融合体底面应仍为板底（{} vs {}）",
            s_lo[2],
            p_lo[2]
        );

        let brep = kernel.export_brep(solid).unwrap();
        let shape = ShapeId::new();
        let mut doc = Document::new("带凸台板");
        doc.insert_geometry(core_model::GeometryRecord::brep(shape, brep))
            .unwrap();
        doc.insert_element(
            core_model::Element::new(core_model::Category::generic(), "板").with_geometry(shape),
        )
        .unwrap();

        let std = DraftingStandard::gb_industrial();

        // 俯视：ViewOrientation::top()（direction = -Z）——view_def_of 取反得 +Z。
        let top_view = View::plan("俯视", core_model::ElementId::new(), 0.0);
        let (top_es, _tw) = project_view(&mut kernel, &doc, &top_view, &std).unwrap();

        // 仰视：显式构造 direction = +Z——view_def_of 取反得 -Z。
        let mut bottom_view = View::new("仰视", core_doc::ViewKind::Elevation);
        bottom_view.orientation =
            ViewOrientation::new(DVec3::new(0.0, 0.0, -1.0), DVec3::Z, DVec3::X);
        let (bottom_es, _bw) = project_view(&mut kernel, &doc, &bottom_view, &std).unwrap();

        let top_visible = visible_circles(&top_es, &std);
        let bottom_visible = visible_circles(&bottom_es, &std);

        assert!(!top_es.is_empty(), "俯视投影不应为空（底板轮廓必须是锚）");
        assert!(
            !bottom_es.is_empty(),
            "仰视投影不应为空（底板轮廓必须是锚）"
        );
        assert!(
            top_visible > 0,
            "从 +Z 俯视必须看到凸台顶圆（可见圆数 {top_visible}）；\
             若为 0 说明 view_def_of 的方向取反被抹掉或方向判反"
        );
        assert_eq!(
            bottom_visible, 0,
            "从 -Z 仰视凸台被底板遮挡，可见圆数必须为 0（实得 {bottom_visible}）；\
             若 > 0 说明方向不敏感 / 取反被抹掉"
        );
    }
}

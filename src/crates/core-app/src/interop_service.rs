//! 互操作服务：**把 `core-doc` 的图纸翻译成 `core-interop` 认的中性结构**，
//! 以及把导入产物并回文档。
//!
//! ## 为什么这一步只能在这里做
//!
//! `core-interop` **不能依赖 `core-doc`**（设计文档 2.2 依赖方向表），
//! 它只认 [`core_interop::Drawing2D`] 那套中性的"线 + 字"结构。
//! 而把 `core_doc::Drawing` 转成它的代码，必须由一个**同时看得见两边**的层来写——
//! 就是 `core-app`：
//!
//! ```text
//! core_doc::Drawing ──(本模块)──▶ Drawing2D ──▶ SvgExporter / DxfExporter
//!        ▲
//!        └── 制图标准在这里"落地"：LayerId → 具体图层名 / 颜色 / 线型 / 线宽
//! ```
//!
//! ## 三个关键动作
//!
//! 1. **ByLayer 落地**：图元存的是 `LayerId`，出口处必须换成**具体值**，
//!    否则 DXF 里会带着一堆悬空图层引用（设计文档 7.7）。
//! 2. **布置求值**：视图坐标 → 图纸坐标（按 [`Placement`] 的比例与位置）。
//! 3. **标注绘制**：把 [`Annotation`] 摊成线 + 文字。
//!
//! ## 已知简化（务必知悉，不要当成 bug）
//!
//! | 项 | 当前做法 | 何时补齐 |
//! |---|---|---|
//! | 圆 / 圆弧 | **输出解析实体**（`Curve2D.analytic` → DXF `CIRCLE` / `ARC`）；离散点列**仍保留**作兜底（SVG 等需要点列）| 已可用（M4） |
//! | 椭圆 | **输出解析实体**（`Curve2D.analytic` → DXF `ELLIPSE`，短长半轴比走组码 40）；离散点列按椭圆参数采样作兜底 | 已可用（M5 B5.1） |
//! | 标注 | 尺寸线 + 箭头（闭合三角**轮廓**）+ 尺寸界线（限线性）+ 自动量测值文字；公差 / 实心填充未画 | 箭头与界线已可用（M5 B5.2） |
//! | 标题栏 / 会签栏 | 画**图框**（GB/T 14689 留边）+ 标题栏**外框/简化格线/字段**（图名/图号/图幅/比例/修订）；**非** GB/T 10609.1 完整栏、无会签栏/对中符号 | 已可用（M5 B5.4） |
//! | 线宽 | 取图层或图元覆盖值（mm，图纸实际宽度）| 已可用 |
//!
//! [`Placement`]: core_doc::Placement

use core_doc::standard::{LAYER_DIM, LAYER_OUTLINE, LAYER_TEXT};
use core_doc::{
    Annotation, AnnotationKind, DraftingStandard, Drawing, Entity2D, EntityKind, LayerDef, Sheet,
    View,
};
use core_interop::{
    Analytic2D, Curve2D, Drawing2D, ExportOutput, Format, FormatRouter, ImportReport, Text2D,
};
use core_model::LayerId;
use glam::DVec2;

use crate::document_service::DocumentService;
use crate::error::AppResult;

/// 圆 / 圆弧离散段数（整圆）。
///
/// 64 段在 1:50 的图上，直径 1 m 的圆的弦高误差约 0.03 mm——肉眼看不出。
/// **M4 起它只作兜底点列**：出口要 `CIRCLE` / `ARC` / `ELLIPSE` 的地方走 [`entity_analytic`]
/// 的解析描述（见模块文档的已知简化表）；点列仍供 SVG 等需要折线的出口使用，
/// 并作为"解析图元缺失时"的最后一道可用性保证。椭圆按长短半轴**参数采样**，
/// 不再是同半径圆近似。
const CIRCLE_SEGMENTS: usize = 64;

/// 单独导出"一个视图"时留的四周白边（mm）。
pub const VIEW_MARGIN_MM: f64 = 10.0;

/// 兜底字高（mm）——制图标准里找不到指定文字样式时用。
const FALLBACK_TEXT_HEIGHT_MM: f64 = 3.5;

/// 标题栏字段文字距格线内边的留白（mm）。
const TITLE_BLOCK_PAD_MM: f64 = 2.0;

/// 视图标题标签距内容底边的间距（mm）。
const VIEW_LABEL_GAP_MM: f64 = 4.0;

// ============================================================================
// 转换：core-doc → Drawing2D
// ============================================================================

/// 把**一张图纸**摊平成中性二维结构（含布置求值）。
///
/// 视点中心对准 [`Placement::at`]，按 `1/scale_denominator` 缩放；
/// 每个布置的内容包围盒下方再写一条「视图名 1:N」标签（G6-M4 出图深化）。
pub fn sheet_to_2d(drawing: &Drawing, sheet: &Sheet) -> AppResult<Drawing2D> {
    sheet.validate()?;
    let (width_mm, height_mm) = sheet.size_mm();
    let mut out = Drawing2D::new(width_mm, height_mm);

    // 标签的图层与字高照标题栏的既有口径解析一次（循环外，不重复查表）。
    let text_layer = drawing.standard.require_layer(LAYER_TEXT)?.name.clone();
    let normal_height = drawing
        .standard
        .text_style("标准")
        .map_or(FALLBACK_TEXT_HEIGHT_MM, |t| t.height_mm);

    for placement in &sheet.placements {
        let view = drawing
            .view(placement.view)
            .ok_or(core_doc::DocError::ViewNotFound(placement.view))?;
        let scale = placement.drawing_scale();
        let bbox = content_bbox(view);
        let center = bbox.map_or(DVec2::ZERO, |(min, max)| (min + max) * 0.5);
        let offset = placement.at - center * scale;
        append_view_content(&drawing.standard, view, scale, offset, &mut out)?;

        // 视图标题标签：内容包围盒底边下方 GAP 处，左端对齐内容左边缘。
        // 空视图（bbox = None）half_w/half_h 取 0，标签落在布置点正下方。
        let (half_w, half_h) = bbox.map_or((0.0, 0.0), |(min, max)| {
            ((max.x - min.x) * scale * 0.5, (max.y - min.y) * scale * 0.5)
        });
        out.push_text(
            Text2D::new(
                [
                    placement.at.x - half_w,
                    placement.at.y - half_h - VIEW_LABEL_GAP_MM,
                ],
                format!("{} 1:{}", view.name, placement.scale_denominator),
                normal_height,
            )
            .on_layer(text_layer.clone()),
        );
    }

    // 图框 + 标题栏**必须在所有 placement 之后**追加：
    // 保证 `curves[0]` 仍是第一条视图实体（既有钉子依赖此顺序）。
    append_title_block(&drawing.standard, sheet, &mut out)?;

    Ok(out)
}

/// 把**单个视图**摊平成中性二维结构（1:1，按内容自适应图幅）。
///
/// 用途：快速预览 / 单元测试 / 不需要图纸排版时的直接导出。
pub fn view_to_2d(view: &View, standard: &DraftingStandard) -> AppResult<Drawing2D> {
    let Some((min, max)) = content_bbox(view) else {
        return Ok(Drawing2D::new(2.0 * VIEW_MARGIN_MM, 2.0 * VIEW_MARGIN_MM));
    };
    let size = max - min;
    let mut out = Drawing2D::new(size.x + 2.0 * VIEW_MARGIN_MM, size.y + 2.0 * VIEW_MARGIN_MM);
    let offset = DVec2::splat(VIEW_MARGIN_MM) - min;
    append_view_content(standard, view, 1.0, offset, &mut out)?;
    Ok(out)
}

/// 视图内容（图元 + 标注）在**视图坐标系**里的包围盒 `(min, max)`。
fn content_bbox(view: &View) -> Option<(DVec2, DVec2)> {
    let mut min = DVec2::splat(f64::INFINITY);
    let mut max = DVec2::splat(f64::NEG_INFINITY);
    let mut any = false;

    let mut extend = |p: DVec2| {
        min = min.min(p);
        max = max.max(p);
        any = true;
    };

    for entity in &view.entities {
        for p in &entity.points {
            extend(*p);
        }
        // 圆的包围盒半径取 `radius`；椭圆的长短半轴已不复用 `radius`，
        // 取 `max(semi_major, semi_minor)` 保守放盒（否则椭圆会被排除出 bbox）。
        let bounds_radius = match entity.kind {
            EntityKind::Circle => entity.radius,
            EntityKind::Ellipse => match (entity.semi_major, entity.semi_minor) {
                (Some(a), Some(b)) => Some(a.max(b)),
                _ => entity.radius,
            },
            _ => None,
        };
        if let (Some(center), Some(r)) = (entity.points.first(), bounds_radius) {
            extend(*center - DVec2::splat(r));
            extend(*center + DVec2::splat(r));
        }
    }
    for annotation in &view.annotations {
        for p in &annotation.points {
            extend(*p);
        }
    }

    any.then_some((min, max))
}

/// 把一个视图的全部内容按 `p ↦ offset + p * scale` 追加出去。
fn append_view_content(
    standard: &DraftingStandard,
    view: &View,
    scale: f64,
    offset: DVec2,
    out: &mut Drawing2D,
) -> AppResult<()> {
    for entity in &view.entities {
        if let Some((points, closed)) = entity_path(entity) {
            let (layer, line_type, width_mm, color) = resolved_style(standard, entity)?;
            let mut curve = Curve2D::new(transform(&points, scale, offset), layer)
                .with_line_type(line_type)
                .with_style(width_mm, color);
            if closed {
                curve = curve.closed();
            }
            // M4: 为圆/圆弧填充解析图元（同时保留 circle_points 兜底）
            curve.analytic = entity_analytic(entity);
            out.push_curve(curve);
        }
        if let Some(text) = entity_text(standard, entity, scale, offset)? {
            out.push_text(text);
        }
    }
    for annotation in &view.annotations {
        append_annotation(standard, annotation, scale, offset, out)?;
    }
    Ok(())
}

/// **ByLayer 落地**：把"图层 id + 图元覆盖"解成出口处要的具体值。
///
/// 返回 `(图层名, 线型名, 线宽 mm, RGB)`。
fn resolved_style(
    standard: &DraftingStandard,
    entity: &Entity2D,
) -> AppResult<(String, String, f64, [u8; 3])> {
    let layer: &LayerDef = standard.require_layer_by_id(entity.layer)?;
    let line_type = entity.style.line_type.unwrap_or(layer.line_type);
    let width_mm = entity.style.width_mm.unwrap_or(layer.width_mm);
    let color = entity.style.color.unwrap_or(layer.color);
    Ok((
        layer.name.clone(),
        line_type.name().to_string(),
        width_mm,
        [color.r, color.g, color.b],
    ))
}

/// 图元的点位路径与闭合性；`None` 表示该图元不是"线"（如文字、符号）。
fn entity_path(entity: &Entity2D) -> Option<(Vec<[f64; 2]>, bool)> {
    let raw = |points: &[DVec2]| -> Vec<[f64; 2]> { points.iter().map(|p| [p.x, p.y]).collect() };

    match entity.kind {
        EntityKind::Line | EntityKind::Polyline | EntityKind::Axis => {
            Some((raw(&entity.points), false))
        }
        EntityKind::Dimension | EntityKind::Leader => Some((raw(&entity.points), false)),
        EntityKind::Hatch => Some((raw(&entity.points), true)),
        EntityKind::Circle => {
            let center = entity.points.first()?;
            Some((circle_points(*center, entity.radius.unwrap_or(0.0)), true))
        }
        EntityKind::Arc => {
            let center = entity.points.first()?;
            let start = entity.points.get(1)?;
            let end = entity.points.get(2)?;
            Some((
                arc_points(*center, entity.radius.unwrap_or(0.0), *start, *end),
                false,
            ))
        }
        // 椭圆按长短半轴参数采样（缺字段时回退到 `radius` 或 0，绝不 panic）
        EntityKind::Ellipse => {
            let center = entity.points.first()?;
            let rx = entity.semi_major.or(entity.radius).unwrap_or(0.0);
            let ry = entity.semi_minor.or(entity.radius).unwrap_or(0.0);
            let rot = entity.rotation_rad.unwrap_or(0.0);
            Some((ellipse_points(*center, rx, ry, rot, CIRCLE_SEGMENTS), true))
        }
        EntityKind::Text | EntityKind::Symbol => None,
    }
}

/// 从图元中提取解析二维图元（圆 / 弧 / 椭圆）。
///
/// 圆 / 弧**既保留离散点列**（`circle_points` / `arc_points` 兜底），
/// 也提供解析描述，供导出器写真正的 `CIRCLE` / `ARC` 实体。
/// 椭圆由长短半轴精确表达 → `ELLIPSE`；三字段任一缺失即返回 `None`
/// （走折线兜底，**绝不静默当圆**）。
fn entity_analytic(entity: &Entity2D) -> Option<Analytic2D> {
    let center = entity.points.first().copied()?;
    match entity.kind {
        EntityKind::Circle => Some(Analytic2D::Circle {
            center: [center.x, center.y],
            radius: entity.radius?,
        }),
        EntityKind::Arc => {
            let start = entity.points.get(1).copied()?;
            let end = entity.points.get(2).copied()?;
            // 从离散点反算角度
            let a0 = (start - center).y.atan2((start - center).x);
            let a1 = (end - center).y.atan2((end - center).x);
            Some(Analytic2D::Arc {
                center: [center.x, center.y],
                radius: entity.radius?,
                start_angle_rad: a0,
                end_angle_rad: a1,
            })
        }
        EntityKind::Ellipse => Some(Analytic2D::Ellipse {
            center: [center.x, center.y],
            rx: entity.semi_major?,
            ry: entity.semi_minor?,
            rotation_rad: entity.rotation_rad?,
        }),
        _ => None,
    }
}

/// 文字 / 符号类图元 → [`Text2D`]。
fn entity_text(
    standard: &DraftingStandard,
    entity: &Entity2D,
    scale: f64,
    offset: DVec2,
) -> AppResult<Option<Text2D>> {
    let Some(content) = entity.text.clone() else {
        return Ok(None);
    };
    let Some(at) = entity.points.first() else {
        return Ok(None);
    };
    let layer = standard.require_layer_by_id(entity.layer)?.name.clone();
    let height = standard
        .text_style("标准")
        .map_or(FALLBACK_TEXT_HEIGHT_MM, |t| t.height_mm);
    Ok(Some(
        Text2D::new(
            [offset.x + at.x * scale, offset.y + at.y * scale],
            content,
            height,
        )
        .on_layer(layer),
    ))
}

/// 标注 → 线 + 文字（简化实现，见模块文档的已知简化表）。
fn append_annotation(
    standard: &DraftingStandard,
    annotation: &Annotation,
    scale: f64,
    offset: DVec2,
    out: &mut Drawing2D,
) -> AppResult<()> {
    let dim_layer = standard.require_layer(LAYER_DIM)?;
    let layer = dim_layer.name.clone();
    let width_mm = dim_layer.width_mm;
    let color = [dim_layer.color.r, dim_layer.color.g, dim_layer.color.b];

    // 尺寸线 / 引线：直接把标注点连起来（箭头与尺寸界线见下方 helper）
    let draws_line = matches!(
        annotation.kind,
        AnnotationKind::Linear
            | AnnotationKind::Radial
            | AnnotationKind::Diametric
            | AnnotationKind::Leader
    );
    if draws_line && annotation.points.len() >= 2 {
        let points: Vec<[f64; 2]> = annotation.points.iter().map(|p| [p.x, p.y]).collect();
        out.push_curve(
            Curve2D::new(transform(&points, scale, offset), layer.clone())
                .with_style(width_mm, color),
        );
    }

    // 箭头 + 尺寸界线：先把量测点变换到**图纸坐标**，再用 `AnnotationStyle` 的
    // `arrow_size_mm` / `extension_offset_mm` **原值**构造（二者是图纸 mm，绝不乘 scale）。
    if draws_line && annotation.points.len() >= 2 {
        let p1 = offset + annotation.points[0] * scale;
        let p2 = offset + annotation.points[1] * scale;
        push_dimension_arrows_and_extensions(
            out,
            annotation.kind,
            p1,
            p2,
            &layer,
            width_mm,
            color,
            f64::from(annotation.style.arrow_size_mm),
            f64::from(annotation.style.extension_offset_mm),
        );
    }

    // 文字：优先用覆盖文字，否则用自动量测值；都没有就不出文字
    let Some(content) = annotation
        .text
        .clone()
        .or_else(|| annotation.measured_value().map(|v| format!("{v:.0}")))
    else {
        return Ok(());
    };

    let at = if annotation.points.len() >= 2 {
        (annotation.points[0] + annotation.points[1]) * 0.5
    } else {
        annotation.points.first().copied().unwrap_or(DVec2::ZERO)
    };
    let height = standard
        .text_style(&annotation.style.text_style)
        .or_else(|| standard.text_style("标准"))
        .map_or(FALLBACK_TEXT_HEIGHT_MM, |t| t.height_mm);

    out.push_text(
        Text2D::new(
            [offset.x + at.x * scale, offset.y + at.y * scale],
            content,
            height,
        )
        .on_layer(layer),
    );
    Ok(())
}

/// 画**图框**（GB/T 14689-2008 留装订边格式）+ **标题栏**（外框 / 简化格线 / 字段文字）。
///
/// # 画什么 / 不画什么
///
/// - **画**：图框（闭合矩形）、标题栏外框（闭合矩形）、一条上下分行横线 + 下行三条竖向
///   分格线、图名 / 图号 / 图幅 / 比例 / 修订五个字段文字。
/// - **不画**（**不宣称**已实现）：GB/T 10609.1-2008 的多区多栏标题栏、会签栏、对中符号、
///   附加符号、装订边以外的其它格式。
///
/// 全部坐标为**图纸坐标**（mm，原点左下），**不随 placement scale 缩放**。
/// 标题栏底边与图框下边线重合、右边与图框右边线重合（GB 规定）。
///
/// # 跳过（显式，不静默画错位置）
///
/// - `title_block.at_bottom_right == false`：本模型仅实现右下角 → **只画图框**。
/// - 标题栏尺寸放不进图框内区（`tw > 内宽 || th > 内高`）→ **只画图框**。
///
/// # 文字锚点
///
/// ⚠ [`Text2D`] **没有对齐锚点**（`at` 即插入点），故字段文字**不宣称居中**，
/// 一律以左内边距 [`TITLE_BLOCK_PAD_MM`] mm 起排。
pub fn append_title_block(
    standard: &DraftingStandard,
    sheet: &Sheet,
    out: &mut Drawing2D,
) -> AppResult<()> {
    let (w, h) = sheet.size_mm();
    let (a, c) = sheet.format.margins_mm();

    let outline = standard.require_layer(LAYER_OUTLINE)?;
    let outline_layer = outline.name.clone();
    let outline_width = outline.width_mm;
    let outline_color = [outline.color.r, outline.color.g, outline.color.b];

    // ---- 图框：闭合 4 点矩形（内缩 a 左 / c 其余三边）----
    out.push_curve(
        Curve2D::new(
            vec![[a, c], [w - c, c], [w - c, h - c], [a, h - c]],
            outline_layer.clone(),
        )
        .closed()
        .with_style(outline_width, outline_color),
    );

    // ---- 标题栏：只实现右下角 ----
    if !sheet.title_block.at_bottom_right {
        // 仅实现右下角格式；其余位置不画，绝不静默挪到别处
        return Ok(());
    }
    let tw = f64::from(sheet.title_block.width_mm);
    let th = f64::from(sheet.title_block.height_mm);
    let inner_w = (w - c) - a;
    let inner_h = (h - c) - c;
    if tw > inner_w || th > inner_h {
        // 标题栏放不进图框内区 → 只画图框（避免画出框外）
        return Ok(());
    }

    let tx1 = w - c; // 右边与图框右边线重合
    let ty0 = c; // 底边与图框下边线重合
    let tx0 = tx1 - tw;
    let ty1 = ty0 + th;

    // 标题栏外框
    out.push_curve(
        Curve2D::new(
            vec![[tx0, ty0], [tx1, ty0], [tx1, ty1], [tx0, ty1]],
            outline_layer.clone(),
        )
        .closed()
        .with_style(outline_width, outline_color),
    );

    // 简化格线：一条横线分上下两行；三条竖线**只在下行**分 4 格
    let ty_mid = ty0 + th / 2.0;
    out.push_curve(
        Curve2D::new(vec![[tx0, ty_mid], [tx1, ty_mid]], outline_layer.clone())
            .with_style(outline_width, outline_color),
    );
    for k in 1..4 {
        let x = tx0 + tw * (k as f64) / 4.0;
        out.push_curve(
            Curve2D::new(vec![[x, ty0], [x, ty_mid]], outline_layer.clone())
                .with_style(outline_width, outline_color),
        );
    }

    // ---- 字段文字 ----
    let text_layer = standard.require_layer(LAYER_TEXT)?.name.clone();
    let big_height = standard
        .text_style("大字")
        .map_or(FALLBACK_TEXT_HEIGHT_MM, |t| t.height_mm);
    let normal_height = standard
        .text_style("标准")
        .map_or(FALLBACK_TEXT_HEIGHT_MM, |t| t.height_mm);

    // 上行：图名（大字），左内边距起排（无居中锚点）
    out.push_text(
        Text2D::new(
            [tx0 + TITLE_BLOCK_PAD_MM, ty_mid + TITLE_BLOCK_PAD_MM],
            sheet.name.clone(),
            big_height,
        )
        .on_layer(text_layer.clone()),
    );

    // 下行 4 格：图号 / 图幅 / 比例 / 修订
    let cell_w = tw / 4.0;
    let first_scale = sheet.placements.first().map_or(1, |p| p.scale_denominator);
    let fields = [
        format!("图号:{}", sheet.number),
        format!("图幅:{}", sheet.format.code()),
        format!("比例:1:{}", first_scale),
        format!("修订:{}", sheet.revision),
    ];
    for (i, content) in fields.into_iter().enumerate() {
        out.push_text(
            Text2D::new(
                [
                    tx0 + (i as f64) * cell_w + TITLE_BLOCK_PAD_MM,
                    ty0 + TITLE_BLOCK_PAD_MM,
                ],
                content,
                normal_height,
            )
            .on_layer(text_layer.clone()),
        );
    }

    Ok(())
}

/// 追一条闭合三角**轮廓**作箭头：尖端 `tip`、底边中心 `base_center`、半宽 `half`。
///
/// ⚠ `Curve2D` **没有填充属性**，故只画三角轮廓，**不宣称"实心 / 填充"**。
#[allow(clippy::too_many_arguments)]
fn push_arrow_triangle(
    out: &mut Drawing2D,
    tip: DVec2,
    base_center: DVec2,
    normal: DVec2,
    half: f64,
    layer: &str,
    width_mm: f64,
    color: [u8; 3],
) {
    out.push_curve(
        Curve2D::new(
            vec![
                [tip.x, tip.y],
                [
                    base_center.x + normal.x * half,
                    base_center.y + normal.y * half,
                ],
                [
                    base_center.x - normal.x * half,
                    base_center.y - normal.y * half,
                ],
            ],
            layer,
        )
        .closed()
        .with_style(width_mm, color),
    );
}

/// 尺寸箭头 + 尺寸界线，全部在**图纸坐标**里按 `arrow_size_mm` / `extension_offset_mm`
/// 的**原值**构造。
///
/// # 两条关键口径
///
/// - **不随 placement scale 缩放**：`arrow_size_mm` 与 `extension_offset_mm` 是图纸上的
///   mm，调用方已把端点变换到图纸坐标，此处**直接使用原值**。注意方向：`drawing_scale()`
///   对 1:100 返回 **0.01**（`core-doc/src/sheet.rs:49-55`），若误乘它箭头会被**缩成
///   0.03 mm**（而非放大）——放大或缩小都是错的，钉子 `dimension_arrows_are_not_scaled_by_placement`
///   会精确打红。
/// - **只画轮廓**：`Curve2D` 无填充属性，箭头是**闭合三角轮廓**，不宣称实心。
///
/// # 几何约定
///
/// 令 `d = p2 - p1`、`u = d/|d|`、`n = (-u.y, u.x)`（`d` 逆时针 90° 法向）、`s = arrow_size_mm`。
/// 箭头在端点处"尖端朝外"、底边半宽 `s/6`（长:宽 ≈ 3:1，合国标观感）。
/// 尺寸界线（**仅 [`AnnotationKind::Linear`]**）取确定性的 `+n` 侧、每端一条，长 `gap .. gap+s`：
/// 本模型里尺寸线与量测点重合、无"偏移 / 侧向"信息，故只能取定一侧。
#[allow(clippy::too_many_arguments)]
fn push_dimension_arrows_and_extensions(
    out: &mut Drawing2D,
    kind: AnnotationKind,
    p1: DVec2,
    p2: DVec2,
    layer: &str,
    width_mm: f64,
    color: [u8; 3],
    arrow_size_mm: f64,
    extension_offset_mm: f64,
) {
    let d = p2 - p1;
    let len = d.length();
    // 退化（端点重合）或箭头尺寸非正 → 不出箭头 / 界线（文字仍由调用方处理）。
    if len < 1e-9 || arrow_size_mm <= 0.0 {
        return;
    }
    let u = d / len;
    let n = DVec2::new(-u.y, u.x);
    let s = arrow_size_mm;
    let half = s / 6.0;

    // 出箭头的端点：Linear / Diametric → 两端；Radial / Leader → 仅自由端 P2；其余 → 不出。
    let (arrow_at_p1, arrow_at_p2) = match kind {
        AnnotationKind::Linear | AnnotationKind::Diametric => (true, true),
        AnnotationKind::Radial | AnnotationKind::Leader => (false, true),
        _ => (false, false),
    };
    if arrow_at_p1 {
        // 尖端 P1 朝外，底边落在 P1 内侧（P1 + u*s）
        push_arrow_triangle(out, p1, p1 + u * s, n, half, layer, width_mm, color);
    }
    if arrow_at_p2 {
        // 尖端 P2 朝外，底边落在 P2 内侧（P2 - u*s）
        push_arrow_triangle(out, p2, p2 - u * s, n, half, layer, width_mm, color);
    }

    // 尺寸界线：仅线性，两端各一条，画在 +n 侧。
    if kind == AnnotationKind::Linear {
        for p in [p1, p2] {
            let a = p + n * extension_offset_mm;
            let b = p + n * (extension_offset_mm + s);
            out.push_curve(
                Curve2D::new(vec![[a.x, a.y], [b.x, b.y]], layer).with_style(width_mm, color),
            );
        }
    }
}

/// 点列变换 `p ↦ offset + p * scale`。
fn transform(points: &[[f64; 2]], scale: f64, offset: DVec2) -> Vec<[f64; 2]> {
    points
        .iter()
        .map(|p| [offset.x + p[0] * scale, offset.y + p[1] * scale])
        .collect()
}

/// 整圆离散（**不重复首点**，闭合性由 `Curve2D::closed` 表达）。
fn circle_points(center: DVec2, radius: f64) -> Vec<[f64; 2]> {
    let step = std::f64::consts::TAU / CIRCLE_SEGMENTS as f64;
    (0..CIRCLE_SEGMENTS)
        .map(|i| {
            let a = step * i as f64;
            [center.x + radius * a.cos(), center.y + radius * a.sin()]
        })
        .collect()
}

/// 椭圆离散（`rx·cos t`、`ry·sin t`，绕中心旋转 `rot` 后再平移；
/// **不重复首点**，闭合性由 `Curve2D::closed` 表达）。
///
/// 这是解析图元缺失时的几何兜底；`rx`/`ry` 为长短半轴，`rot` 为弧度。
fn ellipse_points(center: DVec2, rx: f64, ry: f64, rot: f64, n: usize) -> Vec<[f64; 2]> {
    let (sr, cr) = rot.sin_cos();
    (0..n)
        .map(|i| {
            let t = std::f64::consts::TAU * (i as f64) / (n as f64);
            let (st, ct) = t.sin_cos();
            let x = rx * ct;
            let y = ry * st;
            [center.x + cr * x - sr * y, center.y + sr * x + cr * y]
        })
        .collect()
}

/// 圆弧离散（**逆时针**扫掠，含首含尾）。
fn arc_points(center: DVec2, radius: f64, start: DVec2, end: DVec2) -> Vec<[f64; 2]> {
    let a0 = (start - center).y.atan2((start - center).x);
    let a1 = (end - center).y.atan2((end - center).x);

    let mut sweep = a1 - a0;
    while sweep <= 0.0 {
        sweep += std::f64::consts::TAU;
    }

    let steps = ((sweep / std::f64::consts::TAU) * CIRCLE_SEGMENTS as f64)
        .ceil()
        .max(2.0) as usize;
    (0..=steps)
        .map(|i| {
            let a = a0 + sweep * (i as f64) / (steps as f64);
            [center.x + radius * a.cos(), center.y + radius * a.sin()]
        })
        .collect()
}

/// 图层名解析（诊断 / 测试用）：某个 `LayerId` 在标准里叫什么。
pub fn layer_name_of(standard: &DraftingStandard, id: LayerId) -> Option<String> {
    standard.layer_by_id(id).map(|l| l.name.clone())
}

// ============================================================================
// 互操作服务
// ============================================================================

/// 导入 / 导出的总入口（内部持有 [`FormatRouter`]）。
///
/// 它本身**不含任何合规判断逻辑**——DWG 的隔离红线在
/// [`FormatRouter::import`] 里，这里只是转发与"并回文档"。
pub struct InteropService {
    router: FormatRouter,
}

impl InteropService {
    /// 内置实现全注册（STEP / glTF / IFC / DWG 导入 + SVG / DXF 导出）。
    #[must_use]
    pub fn new() -> Self {
        Self {
            router: FormatRouter::with_builtin(),
        }
    }

    /// 用给定的路由器构造（测试 / 定制实现）。
    #[must_use]
    pub fn with_router(router: FormatRouter) -> Self {
        Self { router }
    }

    /// 只读路由器。
    #[must_use]
    pub fn router(&self) -> &FormatRouter {
        &self.router
    }

    /// 可导入的格式。
    #[must_use]
    pub fn import_formats(&self) -> Vec<Format> {
        self.router.import_formats()
    }

    /// 可导出的格式。
    #[must_use]
    pub fn export_formats(&self) -> Vec<Format> {
        self.router.export_formats()
    }

    /// 某格式当前是否可用（受合规红线约束）。
    #[must_use]
    pub fn is_format_usable(&self, format: Format) -> bool {
        self.router.is_format_usable(format)
    }

    // ---------- 导入 ----------

    /// 按内容嗅探格式并导入（**不落盘到文档**）。
    pub fn import(&self, filename: &str, source: &[u8]) -> AppResult<ImportReport> {
        Ok(self.router.import(filename, source)?)
    }

    /// 导入并**并入文档**——整批插入是**一个撤销步**。
    ///
    /// 任一条目冲突（重复 id 等）→ 文档回到导入前（靠 [`DocumentService::edit`] 的快照回滚）。
    pub fn import_into(
        &self,
        filename: &str,
        source: &[u8],
        service: &mut DocumentService,
    ) -> AppResult<ImportReport> {
        let report = self.import(filename, source)?;
        service.edit(|doc| {
            report.apply_to(doc)?;
            Ok(())
        })?;
        Ok(report)
    }

    // ---------- 导出 ----------

    /// 导出一张图纸（含布置求值）。
    pub fn export_sheet(
        &self,
        format: Format,
        drawing: &Drawing,
        sheet_id: core_model::SheetId,
    ) -> AppResult<ExportOutput> {
        let sheet = drawing
            .sheet(sheet_id)
            .ok_or(core_doc::DocError::SheetNotFound(sheet_id))?;
        let flat = sheet_to_2d(drawing, sheet)?;
        Ok(self.router.export(format, &flat)?)
    }

    /// 导出一个视图（1:1，自适应图幅）。
    pub fn export_view(
        &self,
        format: Format,
        view: &View,
        standard: &DraftingStandard,
    ) -> AppResult<ExportOutput> {
        let flat = view_to_2d(view, standard)?;
        Ok(self.router.export(format, &flat)?)
    }
}

impl Default for InteropService {
    fn default() -> Self {
        Self::new()
    }
}

/// 文字图层名（供调用方核对导出结果，避免与 `core-doc` 的常量走散）。
pub const EXPORT_TEXT_LAYER: &str = LAYER_TEXT;

#[cfg(test)]
mod tests {
    use super::*;
    use core_doc::{Annotation, Entity2D, Placement, Sheet, SheetFormat, View, ViewKind};
    use core_model::{ElementId, GeometryKey, LayerId, ShapeId};

    use crate::error::AppError;

    fn standard() -> DraftingStandard {
        DraftingStandard::gb_industrial()
    }

    fn outline_layer(s: &DraftingStandard) -> LayerId {
        s.layer_by_name(core_doc::standard::LAYER_OUTLINE)
            .unwrap()
            .id
    }

    #[test]
    fn an_empty_view_flattens_to_an_empty_drawing() {
        let v = View::new("空", ViewKind::Drafting);
        let flat = view_to_2d(&v, &standard()).unwrap();
        assert!(flat.is_empty());
        assert!(flat.width_mm > 0.0 && flat.height_mm > 0.0);
    }

    #[test]
    fn by_layer_style_is_resolved_at_the_exit() {
        let s = standard();
        let v = View::new("手绘", ViewKind::Drafting);
        let mut v = v;
        v.push_entity(Entity2D::line(
            outline_layer(&s),
            DVec2::ZERO,
            DVec2::new(100.0, 0.0),
        ));

        let flat = view_to_2d(&v, &s).unwrap();
        let curve = &flat.curves[0];
        assert_eq!(curve.layer, core_doc::standard::LAYER_OUTLINE);
        assert_eq!(curve.line_type, "Continuous");
        assert!((curve.width_mm - 0.5).abs() < 1e-12, "取图层线宽");
        assert_eq!(curve.color, [0, 0, 0], "取图层颜色");
    }

    #[test]
    fn entity_style_overrides_the_layer() {
        let s = standard();
        let mut v = View::new("手绘", ViewKind::Drafting);
        let mut line = Entity2D::line(outline_layer(&s), DVec2::ZERO, DVec2::new(10.0, 0.0));
        line.style = core_doc::EntityStyle::by_layer()
            .with_line_type(core_doc::LineType::Dashed)
            .with_color(core_doc::Rgb::RED);
        v.push_entity(line);

        let flat = view_to_2d(&v, &s).unwrap();
        assert_eq!(flat.curves[0].line_type, "Dashed");
        assert_eq!(flat.curves[0].color, [255, 0, 0]);
    }

    #[test]
    fn a_circle_is_exported_as_an_analytic_circle_entity() {
        let s = standard();
        let mut v = View::new("手绘", ViewKind::Drafting);
        v.push_entity(Entity2D::circle(
            outline_layer(&s),
            DVec2::new(50.0, 50.0),
            25.0,
        ));

        // 通过 DXF 出口验证圆不再拆成 64 段 LINE，而是写出解析 CIRCLE 实体
        let svc = InteropService::new();
        let dxf = svc.export_view(Format::Dxf, &v, &s).unwrap();
        let text = dxf.as_text().unwrap();
        assert!(text.contains("CIRCLE"), "圆应被写成 CIRCLE 实体");
        let line_count = text.matches("\nLINE\n").count();
        assert!(line_count < 64, "不应拆成 64 段 LINE，实得 {line_count}");
    }

    #[test]
    fn ellipse_not_downgraded_to_circle_in_dxf() {
        let s = standard();
        let mut v = View::new("手绘", ViewKind::Drafting);
        v.push_entity(Entity2D::ellipse(
            outline_layer(&s),
            DVec2::new(10.0, 20.0),
            50.0,
            20.0,
            0.0,
        ));

        // ① 解析描述必须是 Ellipse 变体，且半轴与输入一致（绝不当圆）
        let analytic = entity_analytic(&v.entities[0]).expect("椭圆应有解析描述");
        match analytic {
            Analytic2D::Ellipse { center, rx, ry, .. } => {
                assert_eq!(center, [10.0, 20.0]);
                assert_eq!(rx, 50.0);
                assert_eq!(ry, 20.0);
            }
            other => panic!("椭圆不得降级为圆 / 其它：{other:?}"),
        }

        // ② 端到端：DXF 出口写 ELLIPSE 而非 CIRCLE
        let svc = InteropService::new();
        let dxf = svc.export_view(Format::Dxf, &v, &s).unwrap();
        let text = dxf.as_text().unwrap();
        assert!(
            text.contains("\nELLIPSE\n"),
            "应写 ELLIPSE 实体，实文：\n{text}"
        );
        assert!(!text.contains("\nCIRCLE\n"), "椭圆不得被写成 CIRCLE");
        assert!(text.contains("40\n0.400000"), "短长半轴比应为 0.4");
    }

    /// P1 钉子：椭圆包围盒必须用 `max(semi_major, semi_minor)` 扩展，
    /// 而非只看 `radius`——椭圆 `radius == None`，退回旧行为会把椭圆漏出 bbox。
    #[test]
    fn content_bbox_covers_ellipse_semi_major_extent() {
        let s = standard();
        let mut v = View::new("手绘", ViewKind::Drafting);
        v.push_entity(Entity2D::ellipse(
            outline_layer(&s),
            DVec2::ZERO,
            50.0,
            20.0,
            0.0,
        ));

        let (min, max) = content_bbox(&v).expect("椭圆必须产生包围盒");
        assert!(
            (min.x + 50.0).abs() < 1e-9 && (min.y + 50.0).abs() < 1e-9,
            "min 应为 (-50,-50)（用长半轴 50 扩展），实得 {min:?}"
        );
        assert!(
            (max.x - 50.0).abs() < 1e-9 && (max.y - 50.0).abs() < 1e-9,
            "max 应为 (50,50)，实得 {max:?}"
        );
    }

    /// P2-b 钉子：椭圆兜底采样的每个点都落在椭圆周上（参数化正确）。
    #[test]
    fn ellipse_points_fallback_lies_on_the_ellipse() {
        let pts = ellipse_points(DVec2::ZERO, 10.0, 4.0, 0.0, 64);
        // 与 `circle_points` 同风格：不重复首点，闭合性由 `Curve2D.closed` 表达，
        // 故 64 段 → 64 个点（若写成 65 会多出一个与首点重合的退化段）。
        assert_eq!(pts.len(), 64, "64 段应为 64 个不重复点");
        for p in &pts {
            let v = (p[0] / 10.0).powi(2) + (p[1] / 4.0).powi(2);
            assert!(
                (v - 1.0).abs() < 1e-9,
                "点 {p:?} 不在椭圆上：(x/10)²+(y/4)²={v}"
            );
        }
        assert_ne!(pts.first(), pts.last(), "首尾不得重合（避免退化段）");
    }

    #[test]
    fn arc_points_start_at_the_start_and_end_at_the_end() {
        let center = DVec2::ZERO;
        let pts = arc_points(center, 10.0, DVec2::new(10.0, 0.0), DVec2::new(0.0, 10.0));
        assert!(pts.len() >= 3);
        assert!((pts[0][0] - 10.0).abs() < 1e-9 && pts[0][1].abs() < 1e-9);
        assert!(pts.last().unwrap()[0].abs() < 1e-9);
        assert!((pts.last().unwrap()[1] - 10.0).abs() < 1e-9);
    }

    #[test]
    fn text_entities_become_text2d_on_their_layer() {
        let s = standard();
        let text_layer = s.layer_by_name(LAYER_TEXT).unwrap().id;
        let mut v = View::new("手绘", ViewKind::Drafting);
        v.push_entity(Entity2D::text(
            text_layer,
            DVec2::new(5.0, 5.0),
            "一层平面图",
        ));

        let flat = view_to_2d(&v, &s).unwrap();
        assert_eq!(flat.texts.len(), 1);
        assert_eq!(flat.texts[0].content, "一层平面图");
        assert_eq!(flat.texts[0].layer, LAYER_TEXT);
    }

    #[test]
    fn annotations_draw_a_dimension_line_and_a_measured_label() {
        let s = standard();
        let mut v = View::plan("一层", ElementId::new(), 0.0);
        v.push_entity(Entity2D::line(
            outline_layer(&s),
            DVec2::ZERO,
            DVec2::new(600.0, 0.0),
        ));
        v.push_annotation(Annotation::linear(DVec2::ZERO, DVec2::new(600.0, 0.0)));

        let flat = view_to_2d(&v, &s).unwrap();
        assert!(
            flat.curves.iter().any(|c| c.layer == LAYER_DIM),
            "标注要出尺寸线"
        );
        assert!(
            flat.texts
                .iter()
                .any(|t| t.content == "600" && t.layer == LAYER_DIM),
            "自动量测值应被格式化后出图"
        );
    }

    #[test]
    fn overridden_annotation_text_wins_over_the_measurement() {
        let s = standard();
        let mut v = View::new("手绘", ViewKind::Drafting);
        v.push_annotation(
            Annotation::linear(DVec2::ZERO, DVec2::new(600.0, 0.0)).with_text("600(净宽)"),
        );
        let flat = view_to_2d(&v, &s).unwrap();
        assert!(flat.texts.iter().any(|t| t.content == "600(净宽)"));
    }

    // ---------- B5.2：标注箭头 / 尺寸界线 ----------

    /// 直接调 `append_annotation`（scale=1、offset=0），使图纸坐标 == 视图坐标，
    /// 便于精确断言箭头 / 界线的几何（避开 `view_to_2d` 的图幅白边偏移）。
    fn flat_of(annotations: &[Annotation]) -> Drawing2D {
        let s = standard();
        let mut out = Drawing2D::new(1000.0, 1000.0);
        for a in annotations {
            append_annotation(&s, a, 1.0, DVec2::ZERO, &mut out).unwrap();
        }
        out
    }

    fn approx2(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
    }

    /// 存在任一顶点 ≈ 目标点（容差 1e-9）。
    fn has_point(flat: &Drawing2D, p: [f64; 2]) -> bool {
        flat.curves
            .iter()
            .any(|c| c.points.iter().any(|q| approx2(*q, p)))
    }

    /// 存在一条端点恰为 `a`、`b`（顺序无关）的线段。
    fn has_segment(flat: &Drawing2D, a: [f64; 2], b: [f64; 2]) -> bool {
        flat.curves.iter().any(|c| {
            c.points.len() == 2
                && ((approx2(c.points[0], a) && approx2(c.points[1], b))
                    || (approx2(c.points[0], b) && approx2(c.points[1], a)))
        })
    }

    /// 全部"闭合三角轮廓"（箭头）。
    fn triangles(flat: &Drawing2D) -> Vec<&Curve2D> {
        flat.curves
            .iter()
            .filter(|c| c.closed && c.points.len() == 3)
            .collect()
    }

    #[test]
    fn annotation_draws_arrows_and_extension_lines() {
        let flat = flat_of(&[Annotation::linear(DVec2::ZERO, DVec2::new(100.0, 0.0))]);

        // 两个箭头：闭合三角轮廓，各 3 点
        assert_eq!(triangles(&flat).len(), 2, "Linear 两端各一个箭头");

        // 尖端在两量测点；底边半宽 s/6=0.5、偏离端点 3（s=3）
        for tip in [[0.0, 0.0], [100.0, 0.0]] {
            assert!(has_point(&flat, tip), "缺少尖端 {tip:?}");
        }
        assert!(has_point(&flat, [3.0, 0.5]) && has_point(&flat, [3.0, -0.5]));
        assert!(has_point(&flat, [97.0, 0.5]) && has_point(&flat, [97.0, -0.5]));

        // 尺寸界线（仅 Linear）：两端各一条，画在 +n 侧，长 gap..gap+s = 2..5
        assert!(has_segment(&flat, [0.0, 2.0], [0.0, 5.0]), "左尺寸界线缺失");
        assert!(
            has_segment(&flat, [100.0, 2.0], [100.0, 5.0]),
            "右尺寸界线缺失"
        );
    }

    #[test]
    fn annotation_arrow_size_and_offset_come_from_style() {
        let mut ann = Annotation::linear(DVec2::ZERO, DVec2::new(100.0, 0.0));
        ann.style.arrow_size_mm = 10;
        ann.style.extension_offset_mm = 5;
        let flat = flat_of(&[ann]);

        let tris = triangles(&flat);
        assert_eq!(tris.len(), 2);
        // 左箭头（尖端在 (0,0)）：底边两点 x≈s=10、y≈±s/6 → 基宽 = 2*(s/6) = 3.3333…
        let tri = tris
            .iter()
            .find(|c| c.points.iter().any(|q| approx2(*q, [0.0, 0.0])))
            .expect("应有尖端在 (0,0) 的箭头");
        let base_pts: Vec<[f64; 2]> = tri.points.iter().copied().filter(|q| q[0] > 1.0).collect();
        assert_eq!(base_pts.len(), 2);
        let width = ((base_pts[0][0] - base_pts[1][0]).powi(2)
            + (base_pts[0][1] - base_pts[1][1]).powi(2))
        .sqrt();
        assert!(
            (width - 2.0 * (10.0 / 6.0)).abs() < 1e-9,
            "箭头基宽应为 2*(s/6)=3.3333…，实得 {width}"
        );

        // 界线用 style 的 offset=5、长 gap..gap+s = 5..15
        assert!(
            has_segment(&flat, [0.0, 5.0], [0.0, 15.0]),
            "界线未用 style.extension_offset_mm"
        );
        assert!(has_segment(&flat, [100.0, 5.0], [100.0, 15.0]));
    }

    #[test]
    fn zero_arrow_size_yields_no_arrows_or_extensions() {
        let mut ann = Annotation::linear(DVec2::ZERO, DVec2::new(100.0, 0.0));
        ann.style.arrow_size_mm = 0; // 退化：s<=0 → 跳过箭头与界线（但文字照旧）
        let flat = flat_of(&[ann]);

        assert!(triangles(&flat).is_empty(), "s=0 不得出箭头");
        assert!(
            !has_segment(&flat, [0.0, 2.0], [0.0, 5.0]),
            "s=0 不得出尺寸界线"
        );
        assert_eq!(flat.texts.len(), 1, "文字仍应写出");
        assert_eq!(flat.texts[0].content, "100", "线性量测值仍应出图");
    }

    #[test]
    fn dimension_arrows_are_not_scaled_by_placement() {
        let s = standard();
        let mut drawing = Drawing::with_standard(s.clone());
        let mut v = View::plan("一层", ElementId::new(), 0.0);
        // 10 m 的线性尺寸，1:100 出图
        v.push_annotation(Annotation::linear(DVec2::ZERO, DVec2::new(10000.0, 0.0)));
        let view_id = drawing.add_view(v);

        let mut sheet = Sheet::new("一层", "建施-01", SheetFormat::A2);
        sheet.place(Placement::new(view_id, DVec2::new(300.0, 200.0)).with_scale(100));
        let flat = sheet_to_2d(&drawing, &sheet).unwrap();

        let tri = triangles(&flat)
            .into_iter()
            .next()
            .expect("1:100 图上箭头仍须存在");
        let mut max_edge = 0.0_f64;
        for (i, a) in tri.points.iter().enumerate() {
            let b = tri.points[(i + 1) % 3];
            max_edge = max_edge.max(((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt());
        }
        // 几何：s=3、半宽 s/6=0.5 → 最长边 = sqrt(3² + 0.5²) = sqrt(9.25)
        let want = (3.0_f64 * 3.0 + 0.5 * 0.5).sqrt();
        assert!(
            (max_edge - want).abs() < 1e-9,
            "箭头应为图纸 3 mm 量级，实得最长边 {max_edge}"
        );
        assert!(
            max_edge < 10.0,
            "箭头尺寸被 placement scale 放大了（应 ≈{want:.4}，实测 {max_edge}）"
        );
    }

    #[test]
    fn radial_annotation_gets_one_arrow_and_no_extension() {
        let flat = flat_of(&[Annotation::new(
            AnnotationKind::Radial,
            vec![DVec2::ZERO, DVec2::new(50.0, 0.0)],
        )]);

        assert_eq!(triangles(&flat).len(), 1, "Radial 仅自由端一个箭头");
        assert!(has_point(&flat, [50.0, 0.0]), "箭头尖端应在自由端 (50,0)");
        // 无尺寸界线：整体只有"1 条尺寸线(2 点开曲线) + 1 个三角"共 2 条曲线
        assert_eq!(flat.curves.len(), 2, "Radial 不得出尺寸界线");
    }

    #[test]
    fn degenerate_annotation_gets_no_arrow_or_extension() {
        // ① 单点标高：既无箭头也无界线（文字仍可出）
        let flat = flat_of(&[
            Annotation::new(AnnotationKind::Elevation, vec![DVec2::ZERO]).with_text("±0.000"),
        ]);
        assert!(triangles(&flat).is_empty(), "单点标注不得出箭头");
        assert!(!has_segment(&flat, [0.0, 2.0], [0.0, 5.0]));
        assert_eq!(flat.curves.len(), 0, "单点标注不得出任何线");
        assert_eq!(flat.texts.len(), 1, "文字仍应写出");

        // ② 端点重合的线性尺寸（L<1e-9）：箭头 / 界线都跳过，仅保留既有尺寸线
        let flat = flat_of(&[Annotation::linear(
            DVec2::new(7.0, 7.0),
            DVec2::new(7.0, 7.0),
        )]);
        assert!(triangles(&flat).is_empty(), "退化线段不得出箭头");
        assert!(
            !has_segment(&flat, [7.0, 9.0], [7.0, 12.0]),
            "退化线段不得出尺寸界线"
        );
        assert_eq!(flat.curves.len(), 1, "只保留既有尺寸线（2 点）");
        assert_eq!(flat.texts.len(), 1, "退化线段文字仍应写出");
    }

    #[test]
    fn unbounded_geometry_references_do_not_break_export() {
        // 图元带溯源（关联更新锚点）不应影响导出
        let s = standard();
        let mut v = View::plan("一层", ElementId::new(), 0.0);
        v.push_entity(
            Entity2D::line(outline_layer(&s), DVec2::ZERO, DVec2::new(1.0, 0.0))
                .from_geometry(GeometryKey::edge(ShapeId::new(), 0)),
        );
        assert!(view_to_2d(&v, &s).is_ok());
    }

    #[test]
    fn an_unknown_layer_id_is_reported_rather_than_silently_dropped() {
        let s = standard();
        let mut v = View::new("手绘", ViewKind::Drafting);
        v.push_entity(Entity2D::line(
            LayerId::new(),
            DVec2::ZERO,
            DVec2::new(1.0, 0.0),
        ));
        assert!(matches!(view_to_2d(&v, &s), Err(AppError::Doc(_))));
    }

    #[test]
    fn sheet_export_applies_placement_scale_and_offset() {
        let s = standard();
        let mut drawing = Drawing::with_standard(s.clone());
        let mut v = View::plan("一层平面图", ElementId::new(), 0.0);
        // 一条 1 m 长的线，模型坐标
        v.push_entity(Entity2D::line(
            outline_layer(&s),
            DVec2::ZERO,
            DVec2::new(1000.0, 0.0),
        ));
        let view_id = drawing.add_view(v);

        let mut sheet = Sheet::new("一层平面", "建施-01", SheetFormat::A2);
        sheet.place(Placement::new(view_id, DVec2::new(300.0, 200.0)).with_scale(100));
        let (w, h) = sheet.size_mm();
        let flat = sheet_to_2d(&drawing, &sheet).unwrap();
        assert_eq!((flat.width_mm, flat.height_mm), (w, h));

        let curve = &flat.curves[0];
        // 1:100 → 1000 mm 的线在图上长 10 mm
        let dx = (curve.points[1][0] - curve.points[0][0]).abs();
        assert!((dx - 10.0).abs() < 1e-9, "实际 {dx}");
    }

    #[test]
    fn sheet_export_centres_the_view_on_its_placement() {
        let s = standard();
        let mut drawing = Drawing::with_standard(s.clone());
        let mut v = View::plan("一层", ElementId::new(), 0.0);
        v.push_entity(Entity2D::line(
            outline_layer(&s),
            DVec2::new(-500.0, 0.0),
            DVec2::new(500.0, 0.0),
        ));
        let view_id = drawing.add_view(v);

        let mut sheet = Sheet::new("一层", "建施-01", SheetFormat::A2);
        sheet.place(Placement::new(view_id, DVec2::new(300.0, 200.0)).with_scale(10));

        let flat = sheet_to_2d(&drawing, &sheet).unwrap();
        let mid = (flat.curves[0].points[0][0] + flat.curves[0].points[1][0]) * 0.5;
        assert!(
            (mid - 300.0).abs() < 1e-9,
            "视图中心应对准布置点，实际 {mid}"
        );
    }

    #[test]
    fn sheet_export_writes_a_view_label_under_each_placement() {
        let s = standard();
        let mut drawing = Drawing::with_standard(s.clone());
        let mut v = View::plan("一层平面图", ElementId::new(), 0.0);
        // L 形两条线：模型坐标包围盒 x∈[-500,500]、y∈[0,1000]
        v.push_entity(Entity2D::line(
            outline_layer(&s),
            DVec2::new(-500.0, 0.0),
            DVec2::new(500.0, 0.0),
        ));
        v.push_entity(Entity2D::line(
            outline_layer(&s),
            DVec2::new(500.0, 0.0),
            DVec2::new(500.0, 1000.0),
        ));
        let view_id = drawing.add_view(v);

        let mut sheet = Sheet::new("一层平面", "建施-01", SheetFormat::A2);
        sheet.place(Placement::new(view_id, DVec2::new(300.0, 200.0)).with_scale(100));
        let flat = sheet_to_2d(&drawing, &sheet).unwrap();

        // 几何：1:100 → half_w = half_h = 5 mm → 标签插入点 (300-5, 200-5-4) = (295, 191)
        let label = flat
            .texts
            .iter()
            .find(|t| t.content == "一层平面图 1:100")
            .expect("每个布置下方应有「视图名 1:N」标签");
        assert!(
            (label.at[0] - 295.0).abs() < 1e-9,
            "左端应对齐内容左边缘，实际 {}",
            label.at[0]
        );
        assert!(
            (label.at[1] - 191.0).abs() < 1e-9,
            "应在内容底边(195)下方 4 mm，实际 {}",
            label.at[1]
        );
        assert_eq!(
            label.layer,
            core_doc::standard::LAYER_TEXT,
            "标签走文字图层"
        );
        assert!(
            (label.height_mm - 3.5).abs() < 1e-12,
            "字高取「标准」样式，实际 {}",
            label.height_mm
        );

        // 标签是 texts、不产生 curves：curves[0] 仍是第一条视图实体（1 m 线 → 图上 10 mm）
        let curve = &flat.curves[0];
        let dx = (curve.points[1][0] - curve.points[0][0]).abs();
        assert!(
            (dx - 10.0).abs() < 1e-9,
            "curves[0] 仍是 1:100 的水平线，实际 dx={dx}"
        );
    }

    #[test]
    fn sheet_export_labels_each_placement_individually() {
        let s = standard();
        let mut drawing = Drawing::with_standard(s.clone());
        let mut v = View::plan("一层平面图", ElementId::new(), 0.0);
        v.push_entity(Entity2D::line(
            outline_layer(&s),
            DVec2::new(-500.0, 0.0),
            DVec2::new(500.0, 0.0),
        ));
        v.push_entity(Entity2D::line(
            outline_layer(&s),
            DVec2::new(500.0, 0.0),
            DVec2::new(500.0, 1000.0),
        ));
        let view_id = drawing.add_view(v);

        // 同一视图布置两次：比例与位置都不同
        let mut sheet = Sheet::new("一层平面", "建施-01", SheetFormat::A2);
        sheet.place(Placement::new(view_id, DVec2::new(150.0, 150.0)).with_scale(100));
        sheet.place(Placement::new(view_id, DVec2::new(450.0, 150.0)).with_scale(50));
        let flat = sheet_to_2d(&drawing, &sheet).unwrap();

        // 几何：1:100 → half=5 → 标签 (145,141)；1:50 → half=10 → 标签 (440,136)
        let label100 = flat
            .texts
            .iter()
            .find(|t| t.content == "一层平面图 1:100")
            .expect("1:100 布置应有自己的标签");
        let label50 = flat
            .texts
            .iter()
            .find(|t| t.content == "一层平面图 1:50")
            .expect("1:50 布置应有自己的标签");
        assert!(
            (label100.at[0] - 145.0).abs() < 1e-9 && (label100.at[1] - 141.0).abs() < 1e-9,
            "1:100 标签位置不对，实际 {:?}",
            label100.at
        );
        assert!(
            (label50.at[0] - 440.0).abs() < 1e-9 && (label50.at[1] - 136.0).abs() < 1e-9,
            "1:50 标签位置不对，实际 {:?}",
            label50.at
        );
        assert!(
            label50.at[1] < label100.at[1],
            "1:50 画得更大 → 标签应更低，实际 {} vs {}",
            label50.at[1],
            label100.at[1]
        );
    }

    #[test]
    fn sheet_export_labels_empty_views_below_the_placement() {
        let s = standard();
        let mut drawing = Drawing::with_standard(s.clone());
        let v = View::new("空视图", ViewKind::Drafting);
        let view_id = drawing.add_view(v);

        let mut sheet = Sheet::new("一层平面", "建施-01", SheetFormat::A2);
        sheet.place(Placement::new(view_id, DVec2::new(200.0, 150.0)).with_scale(100));
        let flat = sheet_to_2d(&drawing, &sheet).unwrap();

        // 空视图没有内容包围盒 → half 取 0，标签落在布置点正下方 4 mm
        let label = flat
            .texts
            .iter()
            .find(|t| t.content == "空视图 1:100")
            .expect("空视图也应有标签");
        assert!((label.at[0] - 200.0).abs() < 1e-9, "实际 {}", label.at[0]);
        assert!(
            (label.at[1] - 146.0).abs() < 1e-9,
            "应在布置点(150)正下方 4 mm，实际 {}",
            label.at[1]
        );
    }

    #[test]
    fn exporting_a_missing_sheet_is_reported() {
        let drawing = Drawing::new();
        let svc = InteropService::new();
        let err = svc
            .export_sheet(Format::Svg, &drawing, core_model::SheetId::new())
            .unwrap_err();
        assert!(matches!(
            err,
            AppError::Doc(core_doc::DocError::SheetNotFound(_))
        ));
    }

    #[test]
    fn sheet_export_rejects_an_invalid_placement() {
        let s = standard();
        let mut drawing = Drawing::with_standard(s.clone());
        let view_id = drawing.add_view(View::plan("一层", ElementId::new(), 0.0));
        let mut sheet = Sheet::new("一层", "建施-01", SheetFormat::A4);
        // A4 横放 297×210 → (1000,1000) 越界
        sheet.place(Placement::new(view_id, DVec2::new(1000.0, 1000.0)));
        assert!(sheet_to_2d(&drawing, &sheet).is_err());
    }

    #[test]
    fn the_service_reports_the_builtin_format_set() {
        let svc = InteropService::new();
        assert_eq!(svc.import_formats().len(), 5);
        assert_eq!(svc.export_formats().len(), 2);
        assert!(svc.is_format_usable(Format::Svg));
        assert!(!svc.is_format_usable(Format::Dwg), "DWG 受隔离红线约束");
    }

    #[test]
    fn exporting_a_view_end_to_end_produces_svg_and_dxf() {
        let s = standard();
        let mut v = View::plan("一层平面图", ElementId::new(), 0.0);
        v.push_entity(Entity2D::line(
            outline_layer(&s),
            DVec2::ZERO,
            DVec2::new(6000.0, 0.0),
        ));
        v.push_annotation(Annotation::linear(DVec2::ZERO, DVec2::new(6000.0, 0.0)));

        let svc = InteropService::new();
        let svg = svc.export_view(Format::Svg, &v, &s).unwrap();
        assert!(svg.as_text().unwrap().contains("<svg"));

        let dxf = svc.export_view(Format::Dxf, &v, &s).unwrap();
        let text = dxf.as_text().unwrap();
        assert!(text.contains("ENTITIES"));
        assert!(
            text.contains(LAYER_DIM),
            "标注图层必须出现在 DXF 的 LAYER 表里"
        );
    }

    #[test]
    fn importing_a_step_file_into_a_document_is_refused_without_touching_it() {
        const STEP: &[u8] = b"ISO-10303-21;\nHEADER;\nFILE_NAME('a.step','',(''),(''),'','','');\nFILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n";
        let svc = InteropService::new();
        let mut doc = DocumentService::new("导入测试");
        let depth_before = doc.undo_depth();

        // STEP 通道推迟至 OCCT sidecar → 必须显式报错；
        // 且失败的导入**不得**产生撤销步、不得改动文档（M4 §9 Top-1 假绿防线）。
        let err = svc.import_into("a.step", STEP, &mut doc).unwrap_err();
        assert!(matches!(err, AppError::Interop(_)));
        assert_eq!(doc.undo_depth(), depth_before, "失败的导入不得产生撤销步");
        assert_eq!(doc.element_count(), 0, "失败的导入不得改动文档");
    }

    #[test]
    fn importing_an_ifc_file_into_a_document_is_one_undo_step() {
        const IFC: &[u8] = b"ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('ViewDefinition [CoordinationView]'),'2;1');\nFILE_NAME('tower.ifc','2024-01-01T00:00:00',(''),(''),'','','');\nFILE_SCHEMA(('IFC4X3'));\nENDSEC;\nDATA;\n#100=IFCSPACE('g',$,'ROOM',$,$,$,$,$,$);\n#200=IFCWALL('w',$,$,$,$,$,$,$);\nENDSEC;\n";
        let svc = InteropService::new();
        let mut doc = DocumentService::new("导入测试");
        let depth_before = doc.undo_depth();

        let report = svc.import_into("tower.ifc", IFC, &mut doc).unwrap();
        assert_eq!(report.format, Format::Ifc4x3);
        assert!(report.element_count() >= 2, "应导入构件元素");
        assert_eq!(doc.undo_depth(), depth_before + 1, "整批导入是一个撤销步");

        assert!(doc.undo());
        assert_eq!(doc.element_count(), 0);
    }

    #[test]
    fn importing_a_dwg_is_refused_by_the_compliance_guard() {
        let svc = InteropService::new();
        let mut dwg = b"AC1015".to_vec();
        dwg.extend_from_slice(&[0, 0, 0, 1]);
        let err = svc.import("legacy.dwg", &dwg).unwrap_err();
        assert!(matches!(err, AppError::Interop(_)));
    }

    #[test]
    fn layer_name_lookup_agrees_with_the_standard() {
        let s = standard();
        let outline = outline_layer(&s);
        assert_eq!(
            layer_name_of(&s, outline).as_deref(),
            Some(core_doc::standard::LAYER_OUTLINE)
        );
        assert_eq!(layer_name_of(&s, LayerId::new()), None);
    }

    #[test]
    fn export_text_layer_constant_matches_core_doc() {
        assert_eq!(EXPORT_TEXT_LAYER, core_doc::standard::LAYER_TEXT);
    }

    /// B5.3 钉子 ⑯（缺 4）：`GB_LINE_TYPES.name` 必须与 `core_doc::LineType::name()` 逐字一致。
    ///
    /// DXF/SVG 都拿这个名字当查表键；一旦漂移，`gb_line_type` 返回 `None`，
    /// 该图层会**静默退化成实线**——这正是"看着成功、线型却不对"。此测试跨 crate
    /// 钉住这条名称契约（`core-app` 同时依赖 `core-doc` 与 `core-interop`）。
    #[test]
    fn gb_line_type_names_match_core_doc_line_type_names() {
        for name in core_doc::LineType::all_names() {
            assert!(
                core_interop::gb_line_type(name).is_some(),
                "线型名 {name} 在 core-doc 与 core-interop 之间漂移，会导致该图层静默变实线"
            );
        }
    }

    // ---------- B5.4：图框 + 标题栏 ----------

    /// A2 横放（594×420）图纸 + 一个 1:100 视图布置的固件。
    fn a2_sheet_with_one_view(scale_denominator: u32) -> (Drawing, Sheet) {
        let s = standard();
        let mut drawing = Drawing::with_standard(s.clone());
        let mut v = View::plan("一层", ElementId::new(), 0.0);
        v.push_entity(Entity2D::line(
            outline_layer(&s),
            DVec2::ZERO,
            DVec2::new(1000.0, 0.0),
        ));
        let view_id = drawing.add_view(v);
        let mut sheet = Sheet::new("一层平面", "建施-01", SheetFormat::A2);
        sheet
            .place(Placement::new(view_id, DVec2::new(300.0, 200.0)).with_scale(scale_denominator));
        (drawing, sheet)
    }

    /// 存在闭合 4 点矩形，其顶点集恰为 `corners`（顺序无关，容差 1e-9）。
    fn has_closed_quad(flat: &Drawing2D, corners: [[f64; 2]; 4]) -> bool {
        flat.curves.iter().any(|c| {
            c.closed
                && c.points.len() == 4
                && corners
                    .iter()
                    .all(|k| c.points.iter().any(|q| approx2(*q, *k)))
                && c.points
                    .iter()
                    .all(|q| corners.iter().any(|k| approx2(*q, *k)))
        })
    }

    /// 全部闭合 4 点矩形（图框 / 标题栏）。
    fn quads(flat: &Drawing2D) -> Vec<&Curve2D> {
        flat.curves
            .iter()
            .filter(|c| c.closed && c.points.len() == 4)
            .collect()
    }

    /// B5.4 钉子 ②：图框在留边内——A2 → (a,c)=(25,10) → (25,10)…(584,410)。
    #[test]
    fn frame_is_drawn_inside_the_sheet_margins() {
        let (drawing, sheet) = a2_sheet_with_one_view(100);
        let flat = sheet_to_2d(&drawing, &sheet).unwrap();
        assert!(
            has_closed_quad(
                &flat,
                [[25.0, 10.0], [584.0, 10.0], [584.0, 410.0], [25.0, 410.0]]
            ),
            "图框应按 GB/T 14689 留边 (25,10) 内缩（594-10=584、420-10=410），实得矩形：{:?}",
            quads(&flat)
        );
    }

    /// B5.4 钉子 ③：标题栏贴合图框**右下角**（右边 584、底边 10），宽 180、高 56 → (404,10)…(584,66)。
    #[test]
    fn title_block_sits_at_the_frame_bottom_right() {
        let (drawing, sheet) = a2_sheet_with_one_view(100);
        let flat = sheet_to_2d(&drawing, &sheet).unwrap();
        assert!(
            has_closed_quad(
                &flat,
                [[404.0, 10.0], [584.0, 10.0], [584.0, 66.0], [404.0, 66.0]]
            ),
            "标题栏应贴合图框右下角（584-180=404、10+56=66），实得矩形：{:?}",
            quads(&flat)
        );
    }

    /// B5.4 钉子 ④：标题栏写全图名 / 图号 / 图幅 / 比例 / 修订五个字段。
    #[test]
    fn title_block_carries_sheet_fields() {
        let (drawing, sheet) = a2_sheet_with_one_view(100);
        let flat = sheet_to_2d(&drawing, &sheet).unwrap();
        let texts: Vec<&str> = flat.texts.iter().map(|t| t.content.as_str()).collect();
        for want in ["一层平面", "图号:建施-01", "图幅:A2", "比例:1:100", "修订:"] {
            assert!(
                texts.contains(&want),
                "标题栏字段缺 {want:?}，实得 {texts:?}"
            );
        }
    }

    /// B5.4 钉子 ⑤：修订非空时出「修订:A」。
    #[test]
    fn title_block_revision_is_emitted_when_set() {
        let (drawing, mut sheet) = a2_sheet_with_one_view(100);
        sheet.revision = "A".to_string();
        let flat = sheet_to_2d(&drawing, &sheet).unwrap();
        assert!(
            flat.texts.iter().any(|t| t.content == "修订:A"),
            "修订非空时应出「修订:A」，实得 {:?}",
            flat.texts.iter().map(|t| &t.content).collect::<Vec<_>>()
        );
    }

    /// B5.4 钉子 ⑥：标题栏放不进图框内区 → 只画图框（显式跳过）。
    #[test]
    fn oversized_title_block_is_skipped_but_the_frame_is_still_drawn() {
        let (drawing, mut sheet) = a2_sheet_with_one_view(100);
        sheet.title_block.width_mm = 1000; // A2 图框内宽仅 559 → 放不进
        let flat = sheet_to_2d(&drawing, &sheet).unwrap();
        assert!(
            has_closed_quad(
                &flat,
                [[25.0, 10.0], [584.0, 10.0], [584.0, 410.0], [25.0, 410.0]]
            ),
            "图框仍应画出，实得矩形：{:?}",
            quads(&flat)
        );
        assert_eq!(
            quads(&flat).len(),
            1,
            "超尺寸标题栏应跳过：只应剩图框这一个矩形"
        );
        // 视图标签属于布置输出（G6-M4），跳过标题栏只影响字段文字——标签照旧
        assert!(
            flat.texts.iter().all(|t| t.content == "一层 1:100"),
            "跳过标题栏时不应有字段文字（视图标签照旧），实得 {:?}",
            flat.texts.iter().map(|t| &t.content).collect::<Vec<_>>()
        );
    }

    /// B5.4 钉子 ⑦：非右下角标题栏 → 只画图框（本模型仅实现右下角）。
    #[test]
    fn title_block_is_skipped_when_not_bottom_right() {
        let (drawing, mut sheet) = a2_sheet_with_one_view(100);
        sheet.title_block.at_bottom_right = false;
        let flat = sheet_to_2d(&drawing, &sheet).unwrap();
        assert!(
            has_closed_quad(
                &flat,
                [[25.0, 10.0], [584.0, 10.0], [584.0, 410.0], [25.0, 410.0]]
            ),
            "图框仍应画出，实得矩形：{:?}",
            quads(&flat)
        );
        assert_eq!(quads(&flat).len(), 1, "非右下角标题栏应跳过：只应剩图框");
        // 视图标签属于布置输出（G6-M4），跳过标题栏只影响字段文字——标签照旧
        assert!(
            flat.texts.iter().all(|t| t.content == "一层 1:100"),
            "跳过标题栏时不应有字段文字（视图标签照旧），实得 {:?}",
            flat.texts.iter().map(|t| &t.content).collect::<Vec<_>>()
        );
    }

    /// B5.4 钉子 ⑧：图框只属于**图纸**——单视图导出不得带图框 / 标题栏。
    #[test]
    fn view_to_2d_has_no_frame_or_title_block() {
        let s = standard();
        let mut v = View::plan("一层", ElementId::new(), 0.0);
        v.push_entity(Entity2D::line(
            outline_layer(&s),
            DVec2::ZERO,
            DVec2::new(1000.0, 0.0),
        ));
        let flat = view_to_2d(&v, &s).unwrap();
        assert!(
            quads(&flat).is_empty(),
            "单视图导出不应带图框 / 标题栏矩形，实得：{:?}",
            quads(&flat)
        );
    }

    /// B5.4 钉子 ⑨：图框与标题栏都落在轮廓图层。
    #[test]
    fn frame_and_title_block_share_the_outline_layer() {
        let (drawing, sheet) = a2_sheet_with_one_view(100);
        let flat = sheet_to_2d(&drawing, &sheet).unwrap();
        let qs = quads(&flat);
        assert_eq!(qs.len(), 2, "应为图框 + 标题栏两个矩形");
        for c in qs {
            assert_eq!(c.layer, LAYER_OUTLINE, "图框与标题栏都应落在轮廓图层");
        }
    }

    /// 全部**开放** 2 点线段（仅用于断言失败时的排障输出）。
    fn open_segments(flat: &Drawing2D) -> Vec<[[f64; 2]; 2]> {
        flat.curves
            .iter()
            .filter(|c| !c.closed && c.points.len() == 2)
            .map(|c| [c.points[0], c.points[1]])
            .collect()
    }

    /// B5.4 钉子 ⑩（QA E1）：简化格线真的画出来了——
    /// 横线 `[404,38]→[584,38]`（`ty_mid = 10 + 56/2 = 38`）+ 三条**只在下行**的竖线
    /// `x = 404 + 180*k/4`（k=1,2,3 → 449/494/539），每条 `[x,10]→[x,38]`，**不得**画到 `ty1=66`。
    ///
    /// QA 此前把横线删掉、竖线 `for k in 1..4` 改 `1..2` 时全绿零打红——本钉子专门封住该盲区。
    #[test]
    fn title_block_graphics_have_grid_lines() {
        let (drawing, sheet) = a2_sheet_with_one_view(100);
        let flat = sheet_to_2d(&drawing, &sheet).unwrap();

        // 上下分行横线：跑满标题栏宽，位于 ty_mid=38
        assert!(
            has_segment(&flat, [404.0, 38.0], [584.0, 38.0]),
            "标题栏上下分行横线缺失（应 [404,38]→[584,38]），实得开放 2 点线：{:?}",
            open_segments(&flat)
        );

        // 三条下行分格竖线：止于 ty_mid=38，不到 ty1=66
        for x in [449.0, 494.0, 539.0] {
            assert!(
                has_segment(&flat, [x, 10.0], [x, 38.0]),
                "下行分格竖线缺失（应 [{x},10]→[{x},38]，止于 y=38），实得开放 2 点线：{:?}",
                open_segments(&flat)
            );
            assert!(
                !has_segment(&flat, [x, 10.0], [x, 66.0]),
                "竖线 {x} 不得越过 ty_mid 画到 ty1=66（格线只在下行）"
            );
        }
    }

    /// B5.4 钉子 ⑪（QA E2）：字段文字插入点真的带上了左内边距 `TITLE_BLOCK_PAD_MM = 2.0`。
    ///
    /// QA 此前把 `TITLE_BLOCK_PAD_MM` 改成 0.0 时全绿零打红——本钉子专门封住该盲区。
    /// 插入点为 `Text2D.at`（`core-interop/src/drawing2d.rs:161`）。
    #[test]
    fn title_block_text_uses_the_left_padding() {
        let (drawing, sheet) = a2_sheet_with_one_view(100);
        let flat = sheet_to_2d(&drawing, &sheet).unwrap();

        // 图名（上行）：[tx0 + PAD, ty_mid + PAD] = [404+2, 38+2] = [406, 40]
        let name = flat
            .texts
            .iter()
            .find(|t| t.content == sheet.name)
            .unwrap_or_else(|| {
                panic!(
                    "缺图名文字 {:?}，实得 {:?}",
                    sheet.name,
                    flat.texts.iter().map(|t| &t.content).collect::<Vec<_>>()
                )
            });
        assert!(
            approx2(name.at, [406.0, 40.0]),
            "图名插入点应为 [406,40]（tx0=404 + PAD=2），实得 {:?}",
            name.at
        );

        // 下行 4 格：cell_w = 180/4 = 45，插入点 [tx0 + i*45 + PAD, ty0 + PAD] = [406/451/496/541, 12]
        let fields = [
            ("图号:建施-01", [406.0, 12.0]),
            ("图幅:A2", [451.0, 12.0]),
            ("比例:1:100", [496.0, 12.0]),
            ("修订:", [541.0, 12.0]),
        ];
        for (want, at) in fields {
            let t = flat
                .texts
                .iter()
                .find(|t| t.content == want)
                .unwrap_or_else(|| {
                    panic!(
                        "缺字段文字 {want:?}，实得 {:?}",
                        flat.texts.iter().map(|t| &t.content).collect::<Vec<_>>()
                    )
                });
            assert!(
                approx2(t.at, at),
                "字段 {want:?} 插入点应为 {at:?}（含 PAD=2），实得 {:?}",
                t.at
            );
        }
    }
}

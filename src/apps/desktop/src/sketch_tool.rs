//! 草图工具纯逻辑（G4-M15 骨架 + G4-M16 十工具补全）：工具状态机 + 捕捉 /
//! 正交 / 倒角纯函数。
//!
//! 本模块**不依赖 egui、不门控 feature**（默认腿可单测）——画布渲染与指针
//! 交互（`egui::Window` / painter / 输入事件）在门控模块 `sketch_editor`，
//! 二者通过 [`SketchDraft`] 状态机 + 纯函数衔接。
//!
//! 单位与坐标：mm、数学坐标（y 向上、角度 ccw 为正）；屏幕换算（y 向下）
//! 只在编辑器层做，本模块收发的全是数学坐标。
//!
//! bin crate 的默认腿（无 gui feature）下唯一非测试调用方 `sketch_editor`
//! 不参与编译，pub 项以单测先行驱动——整模块抑制 dead_code 告警（与
//! `sketch_geom` 同款口径，接线后保留亦无害）。

#![allow(dead_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

use std::f64::consts::TAU;

use glam::DVec2;

use core_app::SketchEntity;

use crate::sketch_geom::{
    arc_midpoint, catmull_rom_to_bezier, chamfer_corner, circle_through_three, fillet_corner,
    point_segment_distance, segment_intersection,
};

/// 草图画图工具（G4-M15 骨架五种 + G4-M16 补全十种）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SketchTool {
    /// 折线：连续点击加点，回车 / 双击结束。
    Polyline,
    /// 矩形：两击（对角点）→ 闭合折线。
    Rect,
    /// 圆：两击（圆心 → 半径）。
    Circle,
    /// 椭圆：三击（中心 → 长轴端点 → 短轴半距点）。
    Ellipse,
    /// 圆弧：三击（圆心 → 起点定半径与起角 → 终点定终角，ccw）。
    Arc,
    /// 正多边形·内接：两击（中心 → 顶点定外接圆半径与方向，边数参数行定）。
    PolygonIn,
    /// 正多边形·外切：两击（中心 → 边中点定内切圆半径，边数参数行定）。
    PolygonOut,
    /// 三点弧：三击（起点 → 弧上中间点 → 终点，方向由中间点定）。
    Arc3,
    /// 贝塞尔：连续点击控制点，回车 / 双击结束 → 单条 n 阶贝塞尔。
    Bezier,
    /// 样条：连续点击拟合点，回车 / 双击结束 → Catmull-Rom 分段三次贝塞尔。
    Spline,
    /// 辅助点：单击放置（不进 wire，只在画布可见）。
    Point,
    /// 构造线：两击（无限延长两点线，不进 wire）。
    Construction,
    /// 射线：两击（起点 → 方向点，不进 wire）。
    Ray,
    /// 倒圆角：单击两条线段图元 + 参数行半径。
    Fillet,
    /// 倒直线角：单击两条线段图元 + 参数行距离。
    Chamfer,
}

/// 正多边形默认边数（编辑器新建状态与草稿默认值）。
pub const DEFAULT_POLYGON_SIDES: usize = 6;

impl SketchTool {
    /// 工具全集（编辑器工具栏的枚举序）。
    pub const ALL: [SketchTool; 15] = [
        Self::Polyline,
        Self::Rect,
        Self::Circle,
        Self::Ellipse,
        Self::Arc,
        Self::PolygonIn,
        Self::PolygonOut,
        Self::Arc3,
        Self::Bezier,
        Self::Spline,
        Self::Point,
        Self::Construction,
        Self::Ray,
        Self::Fillet,
        Self::Chamfer,
    ];

    /// 工具名（按钮标签）。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Polyline => "折线",
            Self::Rect => "矩形",
            Self::Circle => "圆",
            Self::Ellipse => "椭圆",
            Self::Arc => "圆弧",
            Self::PolygonIn => "内接多边形",
            Self::PolygonOut => "外切多边形",
            Self::Arc3 => "三点弧",
            Self::Bezier => "贝塞尔",
            Self::Spline => "样条",
            Self::Point => "点",
            Self::Construction => "构造线",
            Self::Ray => "射线",
            Self::Fillet => "圆角",
            Self::Chamfer => "倒角",
        }
    }
}

/// 进行中的图元（工具交互状态机）。
///
/// 交互约定（骨架口径 + G4-M16 补全）：
/// - 矩形两击 → [`SketchEntity::Polyline`]（5 点闭合）；
/// - 圆两击（圆心 → 半径）；椭圆三击（中心 → 长轴端点定 major/angle → 短轴点
///   取到主轴线**垂距**）；圆弧三击（圆心 → 起点 → 终点，ccw 归一 +2π）；
/// - 正多边形两击（中心 → 顶点 / 边中点定大小，边数为草稿参数副本）；
/// - 三点弧三击（起点 → 弧上中间点 → 终点，外接圆 + 中间点定方向）；
/// - 贝塞尔 / 样条连续点击，回车 / 双击收尾（[`SketchDraft::finish_open`]）；
/// - 点单击即放置；构造线 / 射线两击（辅助图元，不进 wire）；
/// - 圆角 / 倒角不走点状态机——单击拾取线段图元（[`SketchDraft::pick_corner`]）；
/// - 半径 / 半轴为零（重合点击）不推进——继续等下一击，绝不产出退化图元。
#[derive(Debug, Clone, PartialEq, Default)]
pub enum SketchDraft {
    /// 无进行中图元（等待选工具 / 已完成）。
    #[default]
    None,
    /// 折线（已点击的顶点）。
    Polyline { points: Vec<DVec2> },
    /// 矩形（第一击对角点）。
    Rect { first: Option<DVec2> },
    /// 圆（第一击圆心）。
    Circle { center: Option<DVec2> },
    /// 椭圆（中心 + 长轴端点）。
    Ellipse {
        center: Option<DVec2>,
        axis_end: Option<DVec2>,
    },
    /// 圆弧（圆心 + 起点）。
    Arc {
        center: Option<DVec2>,
        start: Option<DVec2>,
    },
    /// 正多边形（中心 + 参数副本：边数 / 内外切——编辑器参数行修改后同步）。
    Polygon {
        center: Option<DVec2>,
        sides: usize,
        circumscribed: bool,
    },
    /// 三点弧（起点 + 弧上中间点）。
    Arc3 {
        start: Option<DVec2>,
        mid: Option<DVec2>,
    },
    /// 贝塞尔（已点击的控制点）。
    Bezier { points: Vec<DVec2> },
    /// 样条（已点击的拟合点）。
    Spline { points: Vec<DVec2> },
    /// 辅助点（单击即放置，无中间状态）。
    Point,
    /// 构造线（第一击起点）。
    Construction { first: Option<DVec2> },
    /// 射线（第一击起点）。
    Ray { start: Option<DVec2> },
    /// 倒圆角（选两条线段图元下标）。
    Fillet {
        first: Option<usize>,
        second: Option<usize>,
    },
    /// 倒直线角（选两条线段图元下标）。
    Chamfer {
        first: Option<usize>,
        second: Option<usize>,
    },
}

/// 一次画布点击的推进结果。
#[derive(Debug, Clone, PartialEq)]
pub enum ClickOutcome {
    /// 状态推进（图元未完成）。
    Continued,
    /// 图元完成（已按最终口径构造）。
    Finished(SketchEntity),
}

impl SketchDraft {
    /// 新建一个工具的初始草稿（正多边形用默认边数）。
    #[must_use]
    pub fn new(tool: SketchTool) -> Self {
        Self::with_params(tool, DEFAULT_POLYGON_SIDES)
    }

    /// 新建草稿（正多边形带边数参数副本——编辑器参数行可调，建草稿时复制）。
    #[must_use]
    pub fn with_params(tool: SketchTool, polygon_sides: usize) -> Self {
        match tool {
            SketchTool::Polyline => Self::Polyline { points: Vec::new() },
            SketchTool::Rect => Self::Rect { first: None },
            SketchTool::Circle => Self::Circle { center: None },
            SketchTool::Ellipse => Self::Ellipse {
                center: None,
                axis_end: None,
            },
            SketchTool::Arc => Self::Arc {
                center: None,
                start: None,
            },
            SketchTool::PolygonIn => Self::Polygon {
                center: None,
                sides: polygon_sides.max(3),
                circumscribed: false,
            },
            SketchTool::PolygonOut => Self::Polygon {
                center: None,
                sides: polygon_sides.max(3),
                circumscribed: true,
            },
            SketchTool::Arc3 => Self::Arc3 {
                start: None,
                mid: None,
            },
            SketchTool::Bezier => Self::Bezier { points: Vec::new() },
            SketchTool::Spline => Self::Spline { points: Vec::new() },
            SketchTool::Point => Self::Point,
            SketchTool::Construction => Self::Construction { first: None },
            SketchTool::Ray => Self::Ray { start: None },
            SketchTool::Fillet => Self::Fillet {
                first: None,
                second: None,
            },
            SketchTool::Chamfer => Self::Chamfer {
                first: None,
                second: None,
            },
        }
    }

    /// 状态栏步骤提示（按当前交互阶段，全量文案）。
    #[must_use]
    pub fn hint(&self) -> &'static str {
        match self {
            Self::None => "选择工具开始绘制",
            Self::Polyline { points } => {
                if points.is_empty() {
                    "折线：单击放置第一个顶点"
                } else {
                    "折线：单击加点，回车/双击结束"
                }
            }
            Self::Rect { first: None } => "矩形：单击第一个对角点",
            Self::Rect { first: Some(_) } => "矩形：单击第二个对角点",
            Self::Circle { center: None } => "圆：单击圆心",
            Self::Circle { center: Some(_) } => "圆：单击确定半径",
            Self::Ellipse { center: None, .. } => "椭圆：单击中心",
            Self::Ellipse {
                center: Some(_),
                axis_end: None,
            } => "椭圆：单击长轴端点（定长半轴与方向）",
            Self::Ellipse {
                axis_end: Some(_), ..
            } => "椭圆：单击短轴半距点（定短半轴）",
            Self::Arc { center: None, .. } => "圆弧：单击圆心",
            Self::Arc {
                center: Some(_),
                start: None,
            } => "圆弧：单击起点（定半径与起始角）",
            Self::Arc { start: Some(_), .. } => "圆弧：单击终点（逆时针扫到终止角）",
            Self::Polygon { center: None, .. } => "正多边形：单击中心",
            Self::Polygon {
                circumscribed: false,
                ..
            } => "正多边形（内接）：单击顶点定外接圆大小",
            Self::Polygon {
                circumscribed: true,
                ..
            } => "正多边形（外切）：单击边中点定内切圆大小",
            Self::Arc3 { start: None, .. } => "三点弧：单击起点",
            Self::Arc3 {
                start: Some(_),
                mid: None,
            } => "三点弧：单击弧上中间点",
            Self::Arc3 { mid: Some(_), .. } => "三点弧：单击终点",
            Self::Bezier { points } => {
                if points.is_empty() {
                    "贝塞尔：单击第一个控制点"
                } else {
                    "贝塞尔：单击加点，回车/双击结束"
                }
            }
            Self::Spline { points } => {
                if points.is_empty() {
                    "样条：单击第一个拟合点"
                } else {
                    "样条：单击加点，回车/双击结束"
                }
            }
            Self::Point => "点：单击放置辅助点（不进 wire）",
            Self::Construction { first: None } => "构造线：单击起点",
            Self::Construction { first: Some(_) } => "构造线：单击终点",
            Self::Ray { start: None } => "射线：单击起点",
            Self::Ray { start: Some(_) } => "射线：单击方向点",
            Self::Fillet { first: None, .. } => "圆角：单击第一条线段",
            Self::Fillet {
                first: Some(_),
                second: None,
            } => "圆角：单击第二条线段",
            Self::Fillet {
                second: Some(_), ..
            } => "圆角：应用中…",
            Self::Chamfer { first: None, .. } => "倒角：单击第一条线段",
            Self::Chamfer {
                first: Some(_),
                second: None,
            } => "倒角：单击第二条线段",
            Self::Chamfer {
                second: Some(_), ..
            } => "倒角：应用中…",
        }
    }

    /// 单击推进；图元完成时返回 [`ClickOutcome::Finished`] 并复位为
    /// [`SketchDraft::None`]（下一帧选工具继续）。
    pub fn click(&mut self, p: DVec2) -> ClickOutcome {
        match self {
            Self::Polyline { points } => {
                points.push(p);
                ClickOutcome::Continued
            }
            Self::Rect { first } => match *first {
                None => {
                    *first = Some(p);
                    ClickOutcome::Continued
                }
                Some(f) => {
                    let entity = SketchEntity::Polyline(vec![
                        f,
                        DVec2::new(p.x, f.y),
                        p,
                        DVec2::new(f.x, p.y),
                        f,
                    ]);
                    *self = Self::None;
                    ClickOutcome::Finished(entity)
                }
            },
            Self::Circle { center } => match *center {
                None => {
                    *center = Some(p);
                    ClickOutcome::Continued
                }
                Some(c) => {
                    let r = (p - c).length();
                    if r <= f64::EPSILON {
                        return ClickOutcome::Continued; // 重合点击：等下一击
                    }
                    *self = Self::None;
                    ClickOutcome::Finished(SketchEntity::Circle {
                        center: c,
                        radius: r,
                    })
                }
            },
            Self::Ellipse { center, axis_end } => match (*center, *axis_end) {
                (None, _) => {
                    *center = Some(p);
                    ClickOutcome::Continued
                }
                (Some(_), None) => {
                    *axis_end = Some(p);
                    ClickOutcome::Continued
                }
                (Some(c), Some(a)) => {
                    let axis = a - c;
                    let major = axis.length();
                    if major <= f64::EPSILON {
                        return ClickOutcome::Continued; // 长轴为零：等下一击
                    }
                    // 短半轴 = 第三击到主轴线的垂距（|v|·sinθ）；CAD 惯例裁剪
                    // 短 ≤ 长（否则随手一点就被内核响亮拒绝，UX 不可用）。
                    let v = p - c;
                    let dir = axis / major;
                    let minor = (dir.x * v.y - dir.y * v.x).abs().min(major);
                    if minor <= f64::EPSILON {
                        return ClickOutcome::Continued; // 短轴为零：等下一击
                    }
                    *self = Self::None;
                    ClickOutcome::Finished(SketchEntity::Ellipse {
                        center: c,
                        major,
                        minor,
                        major_angle: axis.y.atan2(axis.x),
                    })
                }
            },
            Self::Arc { center, start } => match (*center, *start) {
                (None, _) => {
                    *center = Some(p);
                    ClickOutcome::Continued
                }
                (Some(_), None) => {
                    *start = Some(p);
                    ClickOutcome::Continued
                }
                (Some(c), Some(s)) => {
                    let r = (s - c).length();
                    if r <= f64::EPSILON {
                        return ClickOutcome::Continued; // 起点重合：等下一击
                    }
                    let end_vec = p - c;
                    if end_vec.length() <= f64::EPSILON {
                        return ClickOutcome::Continued; // 终点重合：等下一击
                    }
                    let start_angle = (s - c).y.atan2((s - c).x);
                    let end_angle = end_vec.y.atan2(end_vec.x);
                    // ccw 归一（终角 < 起角 +2π）；0 扫角（终点重合于起点角）
                    // 不推进——绝不产出退化弧。
                    let (sa, ea) = core_app::normalize_arc_angles(start_angle, end_angle);
                    if (ea - sa).abs() <= f64::EPSILON {
                        return ClickOutcome::Continued;
                    }
                    *self = Self::None;
                    ClickOutcome::Finished(SketchEntity::Arc {
                        center: c,
                        radius: r,
                        start_angle: sa,
                        end_angle: ea,
                    })
                }
            },
            Self::Polygon {
                center,
                sides,
                circumscribed,
            } => match *center {
                None => {
                    *center = Some(p);
                    ClickOutcome::Continued
                }
                Some(c) => {
                    let verts = polygon_vertices(c, p, *sides, *circumscribed);
                    if verts.is_empty() {
                        return ClickOutcome::Continued; // 半径为零：等下一击
                    }
                    *self = Self::None;
                    ClickOutcome::Finished(SketchEntity::Polyline(verts))
                }
            },
            Self::Arc3 { start, mid } => match (*start, *mid) {
                (None, _) => {
                    *start = Some(p);
                    ClickOutcome::Continued
                }
                (Some(_), None) => {
                    *mid = Some(p);
                    ClickOutcome::Continued
                }
                (Some(s), Some(m)) => {
                    let Some((center, radius)) = circle_through_three(s, m, p) else {
                        // 三点共线：等下一击（用户重选终点）。
                        return ClickOutcome::Continued;
                    };
                    if radius <= f64::EPSILON {
                        return ClickOutcome::Continued;
                    }
                    let a_s = (s.y - center.y).atan2(s.x - center.x);
                    let a_m = (m.y - center.y).atan2(m.x - center.x);
                    let a_e = (p.y - center.y).atan2(p.x - center.x);
                    let span = (a_e - a_s).rem_euclid(TAU);
                    if span <= f64::EPSILON {
                        return ClickOutcome::Continued; // 起终点重合：等下一击
                    }
                    // 弧方向由中间点定：ccw 扫掠（s→e）含中间点则 s→e 存储，
                    // 否则反向存储（e→s ccw 扫同一几何弧，边方向反转）。
                    let mid_off = (a_m - a_s).rem_euclid(TAU);
                    let (from_angle, to_angle) = if mid_off <= span {
                        (a_s, a_e)
                    } else {
                        (a_e, a_s)
                    };
                    let (sa, ea) = core_app::normalize_arc_angles(from_angle, to_angle);
                    if (ea - sa).abs() <= f64::EPSILON {
                        return ClickOutcome::Continued;
                    }
                    *self = Self::None;
                    ClickOutcome::Finished(SketchEntity::Arc {
                        center,
                        radius,
                        start_angle: sa,
                        end_angle: ea,
                    })
                }
            },
            Self::Bezier { points } => {
                points.push(p);
                ClickOutcome::Continued
            }
            Self::Spline { points } => {
                points.push(p);
                ClickOutcome::Continued
            }
            Self::Point => {
                *self = Self::None;
                ClickOutcome::Finished(SketchEntity::Point(p))
            }
            Self::Construction { first } => match *first {
                None => {
                    *first = Some(p);
                    ClickOutcome::Continued
                }
                Some(f) => {
                    if (p - f).length() <= f64::EPSILON {
                        return ClickOutcome::Continued; // 重合点击：等下一击
                    }
                    *self = Self::None;
                    ClickOutcome::Finished(SketchEntity::AuxLine {
                        from: f,
                        to: p,
                        ray: false,
                    })
                }
            },
            Self::Ray { start } => match *start {
                None => {
                    *start = Some(p);
                    ClickOutcome::Continued
                }
                Some(s) => {
                    if (p - s).length() <= f64::EPSILON {
                        return ClickOutcome::Continued; // 重合点击：等下一击
                    }
                    *self = Self::None;
                    ClickOutcome::Finished(SketchEntity::AuxLine {
                        from: s,
                        to: p,
                        ray: true,
                    })
                }
            },
            // 圆角 / 倒角：不走点状态机（单击拾取线段图元，见 `pick_corner`）。
            Self::Fillet { .. } | Self::Chamfer { .. } => ClickOutcome::Continued,
            Self::None => ClickOutcome::Continued, // 无草稿：点击交给工具选择
        }
    }

    /// 开放曲线收尾（回车 / 双击）：折线 ≥2 顶点 / 贝塞尔 ≥2 控制点 /
    /// 样条 ≥2 拟合点时返回图元并复位。
    ///
    /// 贝塞尔 → [`SketchEntity::Bezier`] 单条 n 阶段（全部控制点一段直通）；
    /// 样条 → Catmull-Rom 拟合为分段三次贝塞尔（n 点 → n-1 段）。
    pub fn finish_open(&mut self) -> Option<SketchEntity> {
        match self {
            Self::Polyline { points } if points.len() >= 2 => {
                let entity = SketchEntity::Polyline(points.clone());
                *self = Self::None;
                Some(entity)
            }
            Self::Bezier { points } if points.len() >= 2 => {
                let entity = SketchEntity::Bezier(vec![points.clone()]);
                *self = Self::None;
                Some(entity)
            }
            Self::Spline { points } if points.len() >= 2 => {
                let segments = catmull_rom_to_bezier(points);
                if segments.is_empty() {
                    return None;
                }
                let entity = SketchEntity::Bezier(segments.into_iter().map(Vec::from).collect());
                *self = Self::None;
                Some(entity)
            }
            _ => None,
        }
    }

    /// Esc 取消进行中：返回是否取消了一个非空草稿。
    pub fn abort(&mut self) -> bool {
        let had = !matches!(self, Self::None);
        if had {
            *self = Self::None;
        }
        had
    }

    /// Backspace 撤点：折线 / 贝塞尔 / 样条撤最后一点，三点弧退回中间点，
    /// 两击类（矩形 / 圆 / 多边形 / 构造线 / 射线）退回第一击，倒角退回
    /// 上一次选定；撤无可撤返回 false。
    pub fn backtrack(&mut self) -> bool {
        match self {
            Self::Polyline { points } | Self::Bezier { points } | Self::Spline { points } => {
                points.pop().is_some()
            }
            Self::Arc3 { start, mid } => mid.take().is_some() || start.take().is_some(),
            Self::Rect { first }
            | Self::Circle { center: first }
            | Self::Polygon { center: first, .. }
            | Self::Construction { first }
            | Self::Ray { start: first } => first.take().is_some(),
            Self::Fillet { first, second } | Self::Chamfer { first, second } => {
                second.take().is_some() || first.take().is_some()
            }
            Self::Ellipse { center, axis_end } => {
                axis_end.take().is_some() || center.take().is_some()
            }
            Self::Arc { center, start } => start.take().is_some() || center.take().is_some(),
            Self::Point | Self::None => false, // 单击即完成 / 无草稿：无撤点
        }
    }
}

/// 网格捕捉：mm 坐标 round 到 `step` 格点。
#[must_use]
pub fn snap_grid(p: DVec2, step: f64) -> DVec2 {
    DVec2::new((p.x / step).round() * step, (p.y / step).round() * step)
}

/// 对象捕捉：`tol`（mm）内**最近**的已有图元锚点——折线顶点 / 段中点、
/// 圆/椭圆/弧中心与圆的 4 个轴向极值点。
///
/// 无命中时返回 `None`（调用方退回网格捕捉 / 原始坐标）。
#[must_use]
pub fn snap_entities(entities: &[SketchEntity], p: DVec2, tol: f64) -> Option<DVec2> {
    let mut best: Option<(f64, DVec2)> = None;
    let mut consider = |q: DVec2| {
        let d = (q - p).length();
        if d <= tol && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, q));
        }
    };
    for e in entities {
        match e {
            SketchEntity::Polyline(points) => {
                for v in points {
                    consider(*v);
                }
                for w in points.windows(2) {
                    consider((w[0] + w[1]) / 2.0);
                }
            }
            SketchEntity::Bezier(segments) => {
                // 控制点全量（含段端点——端点即锚点，吸附价值最高）。
                for seg in segments {
                    for p in seg {
                        consider(*p);
                    }
                }
            }
            SketchEntity::Point(p) => consider(*p),
            SketchEntity::AuxLine { from, to, .. } => {
                consider(*from);
                consider(*to);
            }
            SketchEntity::Circle { center, radius } => {
                consider(*center);
                consider(*center + DVec2::new(*radius, 0.0));
                consider(*center - DVec2::new(*radius, 0.0));
                consider(*center + DVec2::new(0.0, *radius));
                consider(*center - DVec2::new(0.0, *radius));
            }
            SketchEntity::Ellipse { center, .. } | SketchEntity::Arc { center, .. } => {
                consider(*center);
            }
        }
    }
    best.map(|(_, q)| q)
}

/// 正交约束（Shift 临时启用）：相对 `prev` 锁定 0°/90°——取 dx/dy 绝对值
/// 较大者保留另一轴归 `prev`。
#[must_use]
pub fn orthogonalize(prev: DVec2, p: DVec2) -> DVec2 {
    let d = p - prev;
    if d.x.abs() >= d.y.abs() {
        DVec2::new(p.x, prev.y)
    } else {
        DVec2::new(prev.x, p.y)
    }
}

/// 正多边形顶点序列（闭合：首末点重合，与矩形 5 点闭合同款口径）。
///
/// 内接（circumscribed=false）：单击点半径 = 外接圆半径，顶点在圆上，
/// 首顶点方向取单击方向；外切（true）：单击点半径 = 内切圆半径（边心距），
/// 单击点为第一条边的中点，顶点半径放大 r/cos(π/n)、相位偏半步。
/// 边数 < 3 视为 3（三角形）；半径为零返回空表（调用方等下一击）。
#[must_use]
pub fn polygon_vertices(
    center: DVec2,
    radius_point: DVec2,
    sides: usize,
    circumscribed: bool,
) -> Vec<DVec2> {
    let n = sides.max(3);
    let r = (radius_point - center).length();
    if r <= f64::EPSILON {
        return Vec::new();
    }
    let theta0 = (radius_point.y - center.y).atan2(radius_point.x - center.x);
    let step = TAU / n as f64;
    let (vertex_radius, phase) = if circumscribed {
        (r / (step * 0.5).cos(), step * 0.5)
    } else {
        (r, 0.0)
    };
    // 闭合点显式取首点副本（cos(TAU) ≠ 1.0 的浮点误差会让首末点差 ~1e-14）。
    let first = center
        + DVec2::new(
            vertex_radius * (theta0 + phase).cos(),
            vertex_radius * (theta0 + phase).sin(),
        );
    let mut verts: Vec<DVec2> = (0..n)
        .map(|i| {
            let a = theta0 + phase + step * i as f64;
            center + DVec2::new(vertex_radius * a.cos(), vertex_radius * a.sin())
        })
        .collect();
    verts.push(first);
    verts
}

/// 倒角 / 圆角拾取：点附近（tol）最近的**单段折线**图元下标——倒角只作用
/// 于线段图元，多段折线 / 曲线 / 辅助图元不参与。
#[must_use]
pub fn pick_corner_segment(entities: &[SketchEntity], p: DVec2, tol: f64) -> Option<usize> {
    let mut best: Option<(f64, usize)> = None;
    for (i, e) in entities.iter().enumerate() {
        let SketchEntity::Polyline(points) = e else {
            continue;
        };
        if points.len() != 2 {
            continue;
        }
        let d = point_segment_distance(p, points[0], points[1]);
        if d <= tol && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, i));
        }
    }
    best.map(|(_, i)| i)
}

/// 倒角 / 圆角拾取推进结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CornerPick {
    /// 点击落空（容差内无单段折线）或与已选图元相同。
    Ignored,
    /// 第一条线段已选定。
    First(usize),
    /// 两条都选定（调用方 apply 后复位草稿）。
    Both(usize, usize),
}

impl SketchDraft {
    /// 倒角 / 圆角选择推进：拾取到单段折线下标时推进（不走 click 状态机——
    /// 倒角交互**选图元不选点**）；两次选定后返回 [`CornerPick::Both`]，
    /// 由调用方 apply（`apply_fillet` / `apply_chamfer`）后复位草稿。
    pub fn pick_corner(&mut self, picked: Option<usize>) -> CornerPick {
        match self {
            Self::Fillet { first, second } | Self::Chamfer { first, second } => {
                let Some(p) = picked else {
                    return CornerPick::Ignored;
                };
                match *first {
                    None => {
                        *first = Some(p);
                        CornerPick::First(p)
                    }
                    Some(f) if f == p => CornerPick::Ignored, // 同一图元：不能自己倒自己
                    Some(f) => {
                        *second = Some(p);
                        CornerPick::Both(f, p)
                    }
                }
            }
            _ => CornerPick::Ignored,
        }
    }
}

/// 倒圆角应用：对两条单段折线（图元下标 first / second）在其交点处倒圆角，
/// 替换为 [修剪段 P, 圆角弧, 修剪段 Q]（修剪段退化时跳过，≤3 个图元）。
///
/// 圆角弧存储方向：ccw 扫掠凸向交点侧（`arc_midpoint` 距交点近者）——若该
/// 弧从切点 2 起扫，则存储为反向（起点 = 切点 2），几何曲线不变。
/// 半径过大（切点越出原线段）/ 平行或共线无交点 / 下标无效返回 `Err`
/// （图元列表不变）。
pub fn apply_fillet(
    entities: &mut Vec<SketchEntity>,
    first: usize,
    second: usize,
    radius: f64,
) -> Result<(), String> {
    apply_corner(entities, first, second, CornerKind::Fillet(radius))
}

/// 倒直线角应用：同 [`apply_fillet`]，连接段为直线（倒角距离 distance），
/// 替换为 [修剪段 P, 倒角线, 修剪段 Q]。
pub fn apply_chamfer(
    entities: &mut Vec<SketchEntity>,
    first: usize,
    second: usize,
    distance: f64,
) -> Result<(), String> {
    apply_corner(entities, first, second, CornerKind::Chamfer(distance))
}

/// 倒角种类（内部）：圆角（半径）或直线角（距离）。
#[derive(Debug, Clone, Copy)]
enum CornerKind {
    Fillet(f64),
    Chamfer(f64),
}

/// 距交点较远的线段端点（角点侧端点被修剪掉，保留远端）。
fn far_end(seg: [DVec2; 2], v: DVec2) -> DVec2 {
    if v.distance(seg[0]) >= v.distance(seg[1]) {
        seg[0]
    } else {
        seg[1]
    }
}

/// 倒角 / 圆角统一实现（见 `apply_fillet` / `apply_chamfer`）。
fn apply_corner(
    entities: &mut Vec<SketchEntity>,
    first: usize,
    second: usize,
    kind: CornerKind,
) -> Result<(), String> {
    if first == second || first >= entities.len() || second >= entities.len() {
        return Err("下标无效（同一图元或越界）".to_string());
    }
    // 两条都必须是单段折线（倒角只作用于线段图元）。
    let (pa, pb) = match (&entities[first], &entities[second]) {
        (SketchEntity::Polyline(a), SketchEntity::Polyline(b)) if a.len() == 2 && b.len() == 2 => {
            ([a[0], a[1]], [b[0], b[1]])
        }
        _ => return Err("只支持两条单段折线图元".to_string()),
    };
    // 交点 v（两线段所在直线的交点；平行 / 共线 / 不相交 → None）。
    let Some(v) = segment_intersection(pa[0], pa[1], pb[0], pb[1]) else {
        return Err("两线段平行或共线，无交点可倒".to_string());
    };
    // 保留端：各取距交点较远者（角点侧端点被修剪掉）。
    let (a, b) = (far_end(pa, v), far_end(pb, v));
    // 切点与（圆角时）圆心。
    let (t1, t2, connector) = match kind {
        CornerKind::Fillet(radius) => {
            let Some((t1, t2, center)) = fillet_corner(a, v, b, radius) else {
                return Err("圆角失败：半径非正或两边共线".to_string());
            };
            (t1, t2, Some((center, radius)))
        }
        CornerKind::Chamfer(distance) => {
            let Some((t1, t2)) = chamfer_corner(a, v, b, distance) else {
                return Err("倒角失败：距离非正或边长不足".to_string());
            };
            (t1, t2, None)
        }
    };
    // 切点必须落在原线段上（交点在延长线上时切点会越出线段）。
    if point_segment_distance(t1, pa[0], pa[1]) > 1e-6
        || point_segment_distance(t2, pb[0], pb[1]) > 1e-6
    {
        return Err("切点越出原线段：参数过大".to_string());
    }
    // 连接图元：圆角为弧（方向凸向交点侧），倒角为直线段。
    let connector_entity = match connector {
        Some((center, radius)) => {
            let mid_ccw = arc_midpoint(center, t1, t2, true);
            let mid_cw = arc_midpoint(center, t1, t2, false);
            let (from, to) = match (mid_ccw, mid_cw) {
                (Some(m1), Some(m2)) if m1.distance(v) <= m2.distance(v) => (t1, t2),
                (Some(_), Some(_)) => (t2, t1), // cw 弧更近：反向存储同一几何弧
                _ => (t1, t2),                  // 中点退化（不该发生）：按 ccw
            };
            let sa = (from.y - center.y).atan2(from.x - center.x);
            let ea = (to.y - center.y).atan2(to.x - center.x);
            let (sa, ea) = core_app::normalize_arc_angles(sa, ea);
            Some(SketchEntity::Arc {
                center,
                radius,
                start_angle: sa,
                end_angle: ea,
            })
        }
        None => Some(SketchEntity::Polyline(vec![t1, t2])),
    };
    // 组装新图元：修剪段退化（切点贴保留端）时跳过。
    let mut fresh: Vec<SketchEntity> = Vec::with_capacity(3);
    if a.distance(t1) > 1e-9 {
        fresh.push(SketchEntity::Polyline(vec![a, t1]));
    }
    fresh.extend(connector_entity);
    if t2.distance(b) > 1e-9 {
        fresh.push(SketchEntity::Polyline(vec![t2, b]));
    }
    // 替换：先删大下标再删小下标，再在小下标处插入新图元（其余顺序不变）。
    let (i, j) = (first.min(second), first.max(second));
    entities.remove(j);
    entities.remove(i);
    for (k, e) in fresh.into_iter().enumerate() {
        entities.insert(i + k, e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 向量近似相等断言（1e-6，带信息量的失败消息）。
    fn assert_v(actual: DVec2, expected: [f64; 2]) {
        assert!(
            (actual.x - expected[0]).abs() < 1e-6 && (actual.y - expected[1]).abs() < 1e-6,
            "期望 ({}, {})，实得 ({}, {})",
            expected[0],
            expected[1],
            actual.x,
            actual.y
        );
    }

    /// 网格捕捉：round 到格。
    #[test]
    fn grid_snap_rounds_to_step() {
        let p = snap_grid(DVec2::new(103.0, -97.0), 10.0);
        assert_eq!(p, DVec2::new(100.0, -100.0));
    }

    /// 对象捕捉：取容差内**最近**锚点；容差外返回 None。
    #[test]
    fn entity_snap_takes_nearest_within_tol() {
        let entities = vec![SketchEntity::Polyline(vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(50.0, 0.0),
        ])];
        // 端点附近（最近者胜）。
        let q = snap_entities(&entities, DVec2::new(2.0, 1.0), 5.0).expect("端点应在容差内");
        assert_eq!(q, DVec2::new(0.0, 0.0));
        // 段中点。
        let q = snap_entities(&entities, DVec2::new(24.0, 2.0), 5.0).expect("中点应在容差内");
        assert_eq!(q, DVec2::new(25.0, 0.0));
        // 容差外。
        assert!(snap_entities(&entities, DVec2::new(50.0, 20.0), 5.0).is_none());
    }

    /// 正交：dx 较大锁 y、dy 较大锁 x。
    #[test]
    fn orthogonal_locks_the_larger_axis() {
        let prev = DVec2::new(10.0, 10.0);
        assert_eq!(
            orthogonalize(prev, DVec2::new(40.0, 14.0)),
            DVec2::new(40.0, 10.0)
        );
        assert_eq!(
            orthogonalize(prev, DVec2::new(12.0, 40.0)),
            DVec2::new(10.0, 40.0)
        );
    }

    /// 矩形两击 → 5 点闭合折线（首末重合）。
    #[test]
    fn rect_two_clicks_finish_closed_polyline() {
        let mut d = SketchDraft::new(SketchTool::Rect);
        assert_eq!(d.click(DVec2::new(0.0, 0.0)), ClickOutcome::Continued);
        let out = d.click(DVec2::new(50.0, 40.0));
        match out {
            ClickOutcome::Finished(SketchEntity::Polyline(pts)) => {
                assert_eq!(pts.len(), 5);
                assert_eq!(pts[0], pts[4], "首末点应重合（闭合）");
                assert_eq!(pts[2], DVec2::new(50.0, 40.0));
            }
            other => panic!("矩形两击应完成闭合折线，实得 {other:?}"),
        }
        assert_eq!(d, SketchDraft::None, "完成后复位");
    }

    /// 圆两击 → 圆；重合点击不产出退化图元。
    #[test]
    fn circle_two_clicks_finish_and_zero_radius_waits() {
        let mut d = SketchDraft::new(SketchTool::Circle);
        assert_eq!(d.click(DVec2::new(100.0, 100.0)), ClickOutcome::Continued);
        match d.click(DVec2::new(130.0, 100.0)) {
            ClickOutcome::Finished(SketchEntity::Circle { center, radius }) => {
                assert_eq!(center, DVec2::new(100.0, 100.0));
                assert!((radius - 30.0).abs() < 1e-9);
            }
            other => panic!("圆两击应完成，实得 {other:?}"),
        }
        // 重合点击：不推进、不产出。
        let mut d = SketchDraft::new(SketchTool::Circle);
        d.click(DVec2::new(0.0, 0.0));
        assert_eq!(d.click(DVec2::new(0.0, 0.0)), ClickOutcome::Continued);
        assert!(matches!(d, SketchDraft::Circle { center: Some(_) }));
    }

    /// 椭圆三击 → 椭圆（短轴取垂距并裁剪 ≤ 长）。
    #[test]
    fn ellipse_three_clicks_finish_with_minor_clamped() {
        let mut d = SketchDraft::new(SketchTool::Ellipse);
        d.click(DVec2::new(0.0, 0.0)); // 中心
        d.click(DVec2::new(80.0, 0.0)); // 长轴端点（沿 +x）
        match d.click(DVec2::new(0.0, 45.0)) {
            ClickOutcome::Finished(SketchEntity::Ellipse {
                center,
                major,
                minor,
                major_angle,
            }) => {
                assert_eq!(center, DVec2::new(0.0, 0.0));
                assert!((major - 80.0).abs() < 1e-9);
                assert!((minor - 45.0).abs() < 1e-9, "短半轴应为主轴垂距");
                assert!(major_angle.abs() < 1e-9, "主轴沿 +x");
            }
            other => panic!("椭圆三击应完成，实得 {other:?}"),
        }
        // 第三击垂距超过长半轴：裁剪到 major（CAD 惯例）。
        let mut d = SketchDraft::new(SketchTool::Ellipse);
        d.click(DVec2::new(0.0, 0.0));
        d.click(DVec2::new(50.0, 0.0));
        match d.click(DVec2::new(0.0, 500.0)) {
            ClickOutcome::Finished(SketchEntity::Ellipse { minor, major, .. }) => {
                assert!((minor - major).abs() < 1e-9, "短半轴应裁剪到长半轴");
            }
            other => panic!("应完成（裁剪后），实得 {other:?}"),
        }
    }

    /// 圆弧三击 → 弧（ccw 归一；终点重合不产出退化弧）。
    #[test]
    fn arc_three_clicks_finish_with_ccw_normalize() {
        let mut d = SketchDraft::new(SketchTool::Arc);
        d.click(DVec2::new(50.0, 50.0)); // 圆心
        d.click(DVec2::new(100.0, 50.0)); // 起点（+x 方向，起角 0）
        match d.click(DVec2::new(50.0, 0.0)) {
            ClickOutcome::Finished(SketchEntity::Arc {
                center,
                radius,
                start_angle,
                end_angle,
            }) => {
                assert_eq!(center, DVec2::new(50.0, 50.0));
                assert!((radius - 50.0).abs() < 1e-9);
                assert!(start_angle.abs() < 1e-9);
                // 终点在 −y 方向（−π/2）：归一后应为 3π/2（ccw 正扫）。
                assert!((end_angle - 3.0 * std::f64::consts::FRAC_PI_2).abs() < 1e-9);
            }
            other => panic!("圆弧三击应完成，实得 {other:?}"),
        }
        // 终点重合于起点角：0 扫角不推进。
        let mut d = SketchDraft::new(SketchTool::Arc);
        d.click(DVec2::new(0.0, 0.0));
        d.click(DVec2::new(50.0, 0.0));
        assert_eq!(d.click(DVec2::new(50.0, 0.0)), ClickOutcome::Continued);
    }

    /// 折线：加点 → 收尾（≥2 点）；Backspace 撤点；Esc 取消。
    #[test]
    fn polyline_finish_backtrack_and_abort() {
        let mut d = SketchDraft::new(SketchTool::Polyline);
        assert!(d.abort(), "非空草稿应可取消");
        assert_eq!(d, SketchDraft::None);

        let mut d = SketchDraft::new(SketchTool::Polyline);
        d.click(DVec2::new(0.0, 0.0));
        d.click(DVec2::new(30.0, 0.0));
        d.click(DVec2::new(30.0, 40.0));
        // 单点时收尾失败。
        let mut one = SketchDraft::new(SketchTool::Polyline);
        one.click(DVec2::new(0.0, 0.0));
        assert!(one.finish_open().is_none(), "单点折线不应完成");
        // 3 点收尾。
        match d.finish_open() {
            Some(SketchEntity::Polyline(pts)) => assert_eq!(pts.len(), 3),
            other => panic!("三点折线应完成，实得 {other:?}"),
        }
        // Backspace 撤点：2 → 1 → 0 → 空（pop 成功即 true，空折线再撤无事发生）。
        let mut d = SketchDraft::new(SketchTool::Polyline);
        d.click(DVec2::new(0.0, 0.0));
        d.click(DVec2::new(30.0, 0.0));
        assert!(d.backtrack(), "两点撤点应成功");
        assert!(matches!(d, SketchDraft::Polyline { ref points } if points.len() == 1));
        assert!(d.backtrack(), "单点撤点应成功（撤掉最后一个点）");
        assert!(matches!(d, SketchDraft::Polyline { ref points } if points.is_empty()));
        assert!(!d.backtrack(), "空折线再撤无事发生");
        // Esc 取消：空折线也是非 None 草稿（abort 清回 None）。
        assert!(d.abort(), "非 None 草稿（空折线）应可取消");
        assert_eq!(d, SketchDraft::None);
        assert!(!d.abort(), "二次取消无事发生");
    }

    /// 工具全集与提示文案完整（编辑器工具栏枚举序）。
    #[test]
    fn tool_set_and_hints_are_complete() {
        assert_eq!(SketchTool::ALL.len(), 15);
        assert!(!SketchTool::ALL.is_empty());
        for tool in SketchTool::ALL {
            assert!(!tool.label().is_empty());
            assert!(!SketchDraft::new(tool).hint().is_empty());
        }
    }

    /// 内接多边形：单击中心 → 顶点（首顶点在单击方向，闭合 5 点）。
    #[test]
    fn polygon_inscribes_vertices_on_click_radius() {
        let mut d = SketchDraft::with_params(SketchTool::PolygonIn, 4);
        assert_eq!(d.click(DVec2::new(0.0, 0.0)), ClickOutcome::Continued);
        match d.click(DVec2::new(50.0, 0.0)) {
            ClickOutcome::Finished(SketchEntity::Polyline(pts)) => {
                assert_eq!(pts.len(), 5, "4 边形闭合 = 5 点");
                assert_eq!(pts[0], pts[4], "首末点应重合（闭合）");
                assert_v(pts[0], [50.0, 0.0]);
                assert_v(pts[1], [0.0, 50.0]);
                assert_v(pts[2], [-50.0, 0.0]);
                assert_v(pts[3], [0.0, -50.0]);
            }
            other => panic!("内接多边形两击应完成，实得 {other:?}"),
        }
        assert_eq!(d, SketchDraft::None, "完成后复位");
    }

    /// 外切多边形：单击点为第一条边的中点（顶点半径放大 r/cos(π/n)）。
    #[test]
    fn polygon_out_click_is_edge_midpoint() {
        let mut d = SketchDraft::with_params(SketchTool::PolygonOut, 4);
        d.click(DVec2::new(0.0, 0.0));
        match d.click(DVec2::new(50.0, 0.0)) {
            ClickOutcome::Finished(SketchEntity::Polyline(pts)) => {
                assert_eq!(pts.len(), 5);
                // 顶点在 45°/135°/225°/315°，半径 50/cos45°。
                let vr = 50.0 / std::f64::consts::FRAC_1_SQRT_2;
                assert!(
                    (pts[0].length() - vr).abs() < 1e-6,
                    "顶点半径应放大 r/cos(π/n)"
                );
                assert_v(pts[0], [50.0, 50.0]);
                assert_v(pts[1], [-50.0, 50.0]);
                // 单击点 (50,0) 应为边 (50,50)-(50,-50) 的中点。
                assert_v((pts[0] + pts[3]) * 0.5, [50.0, 0.0]);
            }
            other => panic!("外切多边形两击应完成，实得 {other:?}"),
        }
    }

    /// 多边形零半径（重合点击）不推进；边数 < 3 视为 3。
    #[test]
    fn polygon_zero_radius_waits_and_min_sides_is_three() {
        let mut d = SketchDraft::with_params(SketchTool::PolygonIn, 4);
        d.click(DVec2::new(10.0, 10.0));
        assert_eq!(d.click(DVec2::new(10.0, 10.0)), ClickOutcome::Continued);
        assert!(matches!(
            d,
            SketchDraft::Polygon {
                center: Some(_),
                ..
            }
        ));

        // 边数 1 → clamp 到 3（三角形闭合 4 点）。
        let mut d = SketchDraft::with_params(SketchTool::PolygonIn, 1);
        d.click(DVec2::new(0.0, 0.0));
        match d.click(DVec2::new(30.0, 0.0)) {
            ClickOutcome::Finished(SketchEntity::Polyline(pts)) => assert_eq!(pts.len(), 4),
            other => panic!("边数 clamp 到 3 应完成，实得 {other:?}"),
        }
    }

    /// 三点弧：ccw 扫掠含中间点则 s→e 存储，否则反向存储（同一几何弧）。
    #[test]
    fn arc3_through_three_points_both_directions() {
        // 中间点在 ccw 扫掠内（上半圆）：s=(1,0) → m=(0,1) → e=(-1,0)。
        let mut d = SketchDraft::new(SketchTool::Arc3);
        d.click(DVec2::new(1.0, 0.0));
        d.click(DVec2::new(0.0, 1.0));
        match d.click(DVec2::new(-1.0, 0.0)) {
            ClickOutcome::Finished(SketchEntity::Arc {
                center,
                radius,
                start_angle,
                end_angle,
            }) => {
                assert_v(center, [0.0, 0.0]);
                assert!((radius - 1.0).abs() < 1e-9);
                assert!(start_angle.abs() < 1e-9, "起角应为 0");
                assert!(
                    (end_angle - std::f64::consts::PI).abs() < 1e-9,
                    "终角应为 π"
                );
                // 弧上中点应落在中间点 (0,1)（ccw 扫掠）。
                let sp = center + radius * DVec2::new(start_angle.cos(), start_angle.sin());
                let ep = center + radius * DVec2::new(end_angle.cos(), end_angle.sin());
                let mid = arc_midpoint(center, sp, ep, true).expect("有效弧");
                assert_v(mid, [0.0, 1.0]);
            }
            other => panic!("三点弧应完成，实得 {other:?}"),
        }
        // 中间点在 ccw 扫掠外（下半圆）：s=(1,0) → m=(0,-1) → e=(-1,0)，
        // 反向存储（e→s ccw 扫同一几何弧）。
        let mut d = SketchDraft::new(SketchTool::Arc3);
        d.click(DVec2::new(1.0, 0.0));
        d.click(DVec2::new(0.0, -1.0));
        match d.click(DVec2::new(-1.0, 0.0)) {
            ClickOutcome::Finished(SketchEntity::Arc {
                center,
                start_angle,
                end_angle,
                ..
            }) => {
                // 反向存储：起角 π（e），终角 2π（s），弧仍经过 m=(0,-1)。
                assert!((start_angle - std::f64::consts::PI).abs() < 1e-9);
                assert!((end_angle - std::f64::consts::TAU).abs() < 1e-9);
                let sp = center + DVec2::new(start_angle.cos(), start_angle.sin());
                let ep = center + DVec2::new(end_angle.cos(), end_angle.sin());
                let mid = arc_midpoint(center, sp, ep, true).expect("有效弧");
                assert_v(mid, [0.0, -1.0]);
            }
            other => panic!("三点弧（反向）应完成，实得 {other:?}"),
        }
    }

    /// 三点弧共线：等下一击（重选终点），草稿保持阶段 2。
    #[test]
    fn arc3_collinear_waits_for_new_end() {
        let mut d = SketchDraft::new(SketchTool::Arc3);
        d.click(DVec2::new(0.0, 0.0));
        d.click(DVec2::new(1.0, 1.0));
        assert_eq!(d.click(DVec2::new(2.0, 2.0)), ClickOutcome::Continued);
        assert!(matches!(
            d,
            SketchDraft::Arc3 {
                start: Some(_),
                mid: Some(_)
            }
        ));
    }

    /// 贝塞尔：连续控制点 → 收尾为单条 n 阶段（一段直通）。
    #[test]
    fn bezier_finish_is_single_n_order_segment() {
        let mut d = SketchDraft::new(SketchTool::Bezier);
        d.click(DVec2::new(0.0, 0.0));
        assert!(d.finish_open().is_none(), "单控制点不应完成");
        d.click(DVec2::new(30.0, 60.0));
        d.click(DVec2::new(60.0, 60.0));
        d.click(DVec2::new(90.0, 0.0));
        match d.finish_open() {
            Some(SketchEntity::Bezier(segments)) => {
                assert_eq!(segments.len(), 1, "贝塞尔全部控制点一段直通");
                assert_eq!(segments[0].len(), 4, "4 控制点 = 三次");
                assert_eq!(segments[0][0], DVec2::new(0.0, 0.0));
                assert_eq!(segments[0][3], DVec2::new(90.0, 0.0));
            }
            other => panic!("贝塞尔应收尾为单条 n 阶段，实得 {other:?}"),
        }
        assert_eq!(d, SketchDraft::None, "完成后复位");
    }

    /// 样条：拟合点 → Catmull-Rom 分段三次贝塞尔（n 点 → n-1 段）。
    #[test]
    fn spline_finish_is_catmull_rom_multi_segment() {
        let mut d = SketchDraft::new(SketchTool::Spline);
        d.click(DVec2::new(0.0, 0.0));
        assert!(d.finish_open().is_none(), "单拟合点不应完成");
        d.click(DVec2::new(30.0, 40.0));
        d.click(DVec2::new(60.0, 0.0));
        match d.finish_open() {
            Some(SketchEntity::Bezier(segments)) => {
                assert_eq!(segments.len(), 2, "3 拟合点 → 2 段三次贝塞尔");
                // 每段 4 控制点，段间首尾衔接。
                assert_eq!(segments[0].len(), 4);
                assert_eq!(segments[1].len(), 4);
                assert_eq!(segments[0][3], segments[1][0], "段间首尾应衔接");
                assert_eq!(segments[0][0], DVec2::new(0.0, 0.0));
                assert_eq!(segments[1][3], DVec2::new(60.0, 0.0));
            }
            other => panic!("样条应收尾为分段贝塞尔，实得 {other:?}"),
        }
    }

    /// 辅助点：单击即放置；构造线 / 射线：两击（重合不推进）。
    #[test]
    fn point_construction_and_ray_place_aux_entities() {
        // 点：单击完成。
        let mut d = SketchDraft::new(SketchTool::Point);
        assert_eq!(
            d.click(DVec2::new(5.0, 7.0)),
            ClickOutcome::Finished(SketchEntity::Point(DVec2::new(5.0, 7.0)))
        );
        assert_eq!(d, SketchDraft::None);
        // 构造线：两击 → AuxLine(ray=false)。
        let mut d = SketchDraft::new(SketchTool::Construction);
        assert_eq!(d.click(DVec2::new(0.0, 0.0)), ClickOutcome::Continued);
        match d.click(DVec2::new(40.0, 0.0)) {
            ClickOutcome::Finished(SketchEntity::AuxLine { from, to, ray }) => {
                assert_eq!(from, DVec2::new(0.0, 0.0));
                assert_eq!(to, DVec2::new(40.0, 0.0));
                assert!(!ray);
            }
            other => panic!("构造线两击应完成，实得 {other:?}"),
        }
        // 射线：两击 → AuxLine(ray=true)。
        let mut d = SketchDraft::new(SketchTool::Ray);
        d.click(DVec2::new(0.0, 0.0));
        match d.click(DVec2::new(0.0, 40.0)) {
            ClickOutcome::Finished(SketchEntity::AuxLine { from, to, ray }) => {
                assert_eq!(from, DVec2::new(0.0, 0.0));
                assert_eq!(to, DVec2::new(0.0, 40.0));
                assert!(ray);
            }
            other => panic!("射线两击应完成，实得 {other:?}"),
        }
        // 重合点击不推进（构造线）。
        let mut d = SketchDraft::new(SketchTool::Construction);
        d.click(DVec2::new(3.0, 3.0));
        assert_eq!(d.click(DVec2::new(3.0, 3.0)), ClickOutcome::Continued);
        assert!(matches!(d, SketchDraft::Construction { first: Some(_) }));
    }

    /// Backspace 泛化：贝塞尔撤控制点、三点弧退回中间点。
    #[test]
    fn backtrack_works_on_open_curves_and_arc3() {
        // 贝塞尔：2 控制点 → 撤到 1 → 撤到空。
        let mut d = SketchDraft::new(SketchTool::Bezier);
        d.click(DVec2::new(0.0, 0.0));
        d.click(DVec2::new(30.0, 0.0));
        assert!(d.backtrack());
        assert!(matches!(d, SketchDraft::Bezier { ref points } if points.len() == 1));
        assert!(d.backtrack());
        assert!(matches!(d, SketchDraft::Bezier { ref points } if points.is_empty()));
        assert!(!d.backtrack(), "空控制点序列再撤无事发生");
        // 三点弧：start+mid → 撤 mid → 撤 start → 撤无可撤。
        let mut d = SketchDraft::new(SketchTool::Arc3);
        d.click(DVec2::new(0.0, 0.0));
        d.click(DVec2::new(10.0, 0.0));
        assert!(d.backtrack(), "撤中间点应成功");
        assert!(matches!(
            d,
            SketchDraft::Arc3 {
                start: Some(_),
                mid: None
            }
        ));
        assert!(d.backtrack(), "撤起点应成功");
        assert!(matches!(
            d,
            SketchDraft::Arc3 {
                start: None,
                mid: None
            }
        ));
        assert!(!d.backtrack(), "退无可退");
    }

    /// 圆角应用：直角两段 → 修剪段 + 圆角弧 + 修剪段（凸向交点侧）。
    #[test]
    fn fillet_apply_replaces_with_arc_and_trims() {
        let mut entities = vec![
            SketchEntity::Polyline(vec![DVec2::new(0.0, 0.0), DVec2::new(100.0, 0.0)]),
            SketchEntity::Polyline(vec![DVec2::new(100.0, 0.0), DVec2::new(100.0, 100.0)]),
        ];
        apply_fillet(&mut entities, 0, 1, 10.0).expect("直角可倒圆角");
        assert_eq!(entities.len(), 3, "两段替换为 修剪段+弧+修剪段");
        // 修剪段 P：[(0,0),(90,0)]。
        match &entities[0] {
            SketchEntity::Polyline(pts) => {
                assert_eq!(pts.len(), 2);
                assert_v(pts[1], [90.0, 0.0]);
            }
            other => panic!("第一段应为修剪折线，实得 {other:?}"),
        }
        // 圆角弧：圆心 (90,10) 半径 10，凸向交点 (100,0)。
        match &entities[1] {
            SketchEntity::Arc {
                center,
                radius,
                start_angle,
                end_angle,
            } => {
                assert_v(*center, [90.0, 10.0]);
                assert!((*radius - 10.0).abs() < 1e-9);
                // 弧上中点（ccw 扫掠）应靠近交点 (100,0)（凸向角点）。
                let sp = *center + *radius * DVec2::new(start_angle.cos(), start_angle.sin());
                let ep = *center + *radius * DVec2::new(end_angle.cos(), end_angle.sin());
                let mid = arc_midpoint(*center, sp, ep, true).expect("有效弧");
                assert!(
                    mid.distance(DVec2::new(100.0, 0.0)) < 10.0,
                    "圆角弧应凸向交点侧"
                );
            }
            other => panic!("中间应为圆角弧，实得 {other:?}"),
        }
        // 修剪段 Q：[(100,10),(100,100)]。
        match &entities[2] {
            SketchEntity::Polyline(pts) => {
                assert_eq!(pts.len(), 2);
                assert_v(pts[0], [100.0, 10.0]);
            }
            other => panic!("第二段应为修剪折线，实得 {other:?}"),
        }
    }

    /// 倒角应用：直角两段 → 修剪段 + 倒角直线 + 修剪段（乱序下标）。
    #[test]
    fn chamfer_apply_replaces_with_line_and_trims() {
        let mut entities = vec![
            SketchEntity::Polyline(vec![DVec2::new(0.0, 0.0), DVec2::new(100.0, 0.0)]),
            SketchEntity::Polyline(vec![DVec2::new(100.0, 0.0), DVec2::new(100.0, 100.0)]),
        ];
        // 乱序下标 (1,0)：pa = 原 1（竖段）、pb = 原 0（横段），新图元插在 min=0。
        apply_chamfer(&mut entities, 1, 0, 10.0).expect("边长充足（下标可乱序）");
        assert_eq!(entities.len(), 3);
        // entities[0] = 竖段修剪 [(100,100),(100,10)]。
        match &entities[0] {
            SketchEntity::Polyline(pts) => {
                assert_eq!(pts.len(), 2);
                assert_v(pts[0], [100.0, 100.0]);
                assert_v(pts[1], [100.0, 10.0]);
            }
            other => panic!("第一段应为竖段修剪折线，实得 {other:?}"),
        }
        // entities[1] = 倒角连接线 [(100,10),(90,0)]。
        match &entities[1] {
            SketchEntity::Polyline(pts) => {
                assert_eq!(pts.len(), 2, "倒角连接段为直线");
                assert_v(pts[0], [100.0, 10.0]);
                assert_v(pts[1], [90.0, 0.0]);
            }
            other => panic!("中间应为倒角直线，实得 {other:?}"),
        }
        // entities[2] = 横段修剪 [(90,0),(0,0)]。
        match &entities[2] {
            SketchEntity::Polyline(pts) => {
                assert_v(pts[0], [90.0, 0.0]);
                assert_v(pts[1], [0.0, 0.0]);
            }
            other => panic!("第二段应为横段修剪折线，实得 {other:?}"),
        }
    }

    /// 倒角几何拒绝：半径过大（切点越界）/ 平行无交点 / 同一下标 / 多段折线。
    #[test]
    fn corner_apply_rejects_bad_geometry() {
        // 半径 200 超出 100 长线段：切点越界。
        let mut entities = vec![
            SketchEntity::Polyline(vec![DVec2::new(0.0, 0.0), DVec2::new(100.0, 0.0)]),
            SketchEntity::Polyline(vec![DVec2::new(100.0, 0.0), DVec2::new(100.0, 100.0)]),
        ];
        let err = apply_fillet(&mut entities, 0, 1, 200.0).expect_err("半径过大应拒绝");
        assert!(err.contains("越出"), "应提示切点越界，实得 {err}");
        assert_eq!(entities.len(), 2, "失败时图元不变");
        // 平行无交点。
        let mut parallel = vec![
            SketchEntity::Polyline(vec![DVec2::new(0.0, 0.0), DVec2::new(50.0, 0.0)]),
            SketchEntity::Polyline(vec![DVec2::new(0.0, 30.0), DVec2::new(50.0, 30.0)]),
        ];
        let err = apply_fillet(&mut parallel, 0, 1, 5.0).expect_err("平行应拒绝");
        assert!(err.contains("平行"), "应提示无交点，实得 {err}");
        // 同一下标。
        let mut one = vec![SketchEntity::Polyline(vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(50.0, 0.0),
        ])];
        assert!(apply_fillet(&mut one, 0, 0, 5.0).is_err());
        // 多段折线不参与倒角。
        let mut multi = vec![
            SketchEntity::Polyline(vec![DVec2::new(0.0, 0.0), DVec2::new(50.0, 0.0)]),
            SketchEntity::Polyline(vec![
                DVec2::new(50.0, 0.0),
                DVec2::new(50.0, 50.0),
                DVec2::new(80.0, 50.0),
            ]),
        ];
        let err = apply_fillet(&mut multi, 0, 1, 5.0).expect_err("多段折线应拒绝");
        assert!(err.contains("单段"), "应提示只支持单段，实得 {err}");
    }

    /// 倒角拾取：只认单段折线、取最近、容差外 None。
    #[test]
    fn pick_corner_segment_filters_and_takes_nearest() {
        let entities = vec![
            SketchEntity::Circle {
                center: DVec2::new(0.0, 0.0),
                radius: 10.0,
            }, // 非折线：跳过
            SketchEntity::Polyline(vec![DVec2::new(0.0, 0.0), DVec2::new(50.0, 0.0)]),
            SketchEntity::Polyline(vec![
                DVec2::new(0.0, 40.0),
                DVec2::new(0.0, 60.0),
                DVec2::new(30.0, 60.0),
            ]), // 多段：跳过
            SketchEntity::Polyline(vec![DVec2::new(0.0, 20.0), DVec2::new(50.0, 20.0)]),
        ];
        // 靠近第一条（y=0）。
        assert_eq!(
            pick_corner_segment(&entities, DVec2::new(25.0, 3.0), 5.0),
            Some(1)
        );
        // 靠近第二条（y=20）。
        assert_eq!(
            pick_corner_segment(&entities, DVec2::new(25.0, 22.0), 5.0),
            Some(3)
        );
        // 容差外。
        assert_eq!(
            pick_corner_segment(&entities, DVec2::new(25.0, 31.0), 5.0),
            None
        );
    }

    /// 倒角选择推进：两次选定 → Both；同图元忽略；落空忽略。
    #[test]
    fn pick_corner_advances_two_picks() {
        let mut d = SketchDraft::new(SketchTool::Fillet);
        assert_eq!(d.pick_corner(None), CornerPick::Ignored);
        assert_eq!(d.pick_corner(Some(3)), CornerPick::First(3));
        assert_eq!(d.pick_corner(None), CornerPick::Ignored);
        assert_eq!(d.pick_corner(Some(3)), CornerPick::Ignored, "同一图元忽略");
        assert_eq!(d.pick_corner(Some(7)), CornerPick::Both(3, 7));
        // 倒角工具同款推进。
        let mut d = SketchDraft::new(SketchTool::Chamfer);
        assert_eq!(d.pick_corner(Some(1)), CornerPick::First(1));
        assert_eq!(d.pick_corner(Some(4)), CornerPick::Both(1, 4));
        // 非倒角草稿：忽略。
        let mut d = SketchDraft::new(SketchTool::Polyline);
        assert_eq!(d.pick_corner(Some(0)), CornerPick::Ignored);
    }
}

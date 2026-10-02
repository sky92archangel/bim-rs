//! 草图工具纯逻辑（G4-M15 骨架 + G4-M16 绘制补全 + G4-M17 编辑 / 标注）：
//! 工具状态机 + 捕捉 / 正交 / 倒角 / 几何变换纯函数。
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
    mirror_point, offset_circle, offset_segment, point_segment_distance, rotate_point, scale_point,
    segment_intersection, split_segment, trim_segment,
};

/// 草图画图工具（G4-M15 骨架五种 + G4-M16 补全十种 + G4-M17 编辑/标注十二种）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SketchTool {
    // —— 绘制类（15）——
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
    // —— 编辑类（8，G4-M17，全部顶点级纯几何、与内核无关）——
    /// 移动：选图元 → 基点 → 目标点（顶点平移）。
    Move,
    /// 复制：选图元 → 基点 → 目标点（克隆 + 平移，可连续多份）。
    Copy,
    /// 旋转：选图元 → 基点 → 参考点 → 目标点（顶点绕基点旋转）。
    Rotate,
    /// 缩放：选图元 → 基点 → 参考点 → 目标点（顶点绕基点等比缩放）。
    Scale,
    /// 镜像：选图元 → 镜像线两点（顶点关于直线反射，弧方向反转）。
    Mirror,
    /// 偏移：单击线段 / 圆 → 按参数行距离生成平行线 / 同心圆（原图元不动）。
    Offset,
    /// 修剪：单击边界线段 → 单击被剪线段（保留单击侧）。
    Trim,
    /// 打断：单击线段内部一点 → 断成两段。
    Break,
    // —— 标注类（4，G4-M17，显示型：不进 wire，标注数字）——
    /// 线性标注：三击（两点 → 放置位置），测轴对齐增量。
    DimLinear,
    /// 对齐标注：三击（两点 → 放置位置），测真实距离。
    DimAlign,
    /// 半径标注：单击圆 → 单击标注位置（引线 + R 数字）。
    DimRadius,
    /// 角度标注：单击两条线段（交点定角顶点，角弧 + 度数）。
    DimAngle,
}

/// 正多边形默认边数（编辑器新建状态与草稿默认值）。
pub const DEFAULT_POLYGON_SIDES: usize = 6;

/// 绘制类工具全集（工具栏第一行）。
pub const DRAWING_TOOLS: [SketchTool; 15] = [
    SketchTool::Polyline,
    SketchTool::Rect,
    SketchTool::Circle,
    SketchTool::Ellipse,
    SketchTool::Arc,
    SketchTool::PolygonIn,
    SketchTool::PolygonOut,
    SketchTool::Arc3,
    SketchTool::Bezier,
    SketchTool::Spline,
    SketchTool::Point,
    SketchTool::Construction,
    SketchTool::Ray,
    SketchTool::Fillet,
    SketchTool::Chamfer,
];

/// 编辑 + 标注类工具全集（工具栏第二行）。
pub const EDITING_TOOLS: [SketchTool; 12] = [
    SketchTool::Move,
    SketchTool::Copy,
    SketchTool::Rotate,
    SketchTool::Scale,
    SketchTool::Mirror,
    SketchTool::Offset,
    SketchTool::Trim,
    SketchTool::Break,
    SketchTool::DimLinear,
    SketchTool::DimAlign,
    SketchTool::DimRadius,
    SketchTool::DimAngle,
];

impl SketchTool {
    /// 工具全集（编辑器工具栏的枚举序 = 绘制 + 编辑标注）。
    pub const ALL: [SketchTool; 27] = [
        SketchTool::Polyline,
        SketchTool::Rect,
        SketchTool::Circle,
        SketchTool::Ellipse,
        SketchTool::Arc,
        SketchTool::PolygonIn,
        SketchTool::PolygonOut,
        SketchTool::Arc3,
        SketchTool::Bezier,
        SketchTool::Spline,
        SketchTool::Point,
        SketchTool::Construction,
        SketchTool::Ray,
        SketchTool::Fillet,
        SketchTool::Chamfer,
        SketchTool::Move,
        SketchTool::Copy,
        SketchTool::Rotate,
        SketchTool::Scale,
        SketchTool::Mirror,
        SketchTool::Offset,
        SketchTool::Trim,
        SketchTool::Break,
        SketchTool::DimLinear,
        SketchTool::DimAlign,
        SketchTool::DimRadius,
        SketchTool::DimAngle,
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
            Self::Move => "移动",
            Self::Copy => "复制",
            Self::Rotate => "旋转",
            Self::Scale => "缩放",
            Self::Mirror => "镜像",
            Self::Offset => "偏移",
            Self::Trim => "修剪",
            Self::Break => "打断",
            Self::DimLinear => "线性标注",
            Self::DimAlign => "对齐标注",
            Self::DimRadius => "半径标注",
            Self::DimAngle => "角度标注",
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
/// - 编辑 / 标注类（G4-M17）走 [`SketchDraft::edit_click`]——拾取阶段选图元、
///   点阶段定基点 / 目标点 / 标注位置；偏移单击即成（编辑器侧直接应用）；
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
    /// 移动（G4-M17：图元下标 → 基点）。
    Move {
        target: Option<usize>,
        base: Option<DVec2>,
    },
    /// 复制（图元下标 → 基点；目标点可连续点击多份）。
    Copy {
        target: Option<usize>,
        base: Option<DVec2>,
    },
    /// 旋转（图元下标 → 基点 → 参考点；目标点定转角）。
    Rotate {
        target: Option<usize>,
        base: Option<DVec2>,
        reference: Option<DVec2>,
    },
    /// 缩放（图元下标 → 基点 → 参考点；目标点定比例）。
    Scale {
        target: Option<usize>,
        base: Option<DVec2>,
        reference: Option<DVec2>,
    },
    /// 镜像（图元下标 → 镜像线第一点）。
    Mirror {
        target: Option<usize>,
        first: Option<DVec2>,
    },
    /// 偏移（无状态：单击拾取即按参数行距离成新图元）。
    Offset,
    /// 修剪（边界线段图元下标）。
    Trim { boundary: Option<usize> },
    /// 打断（无状态：单击拾取 + 段内点即断）。
    Break,
    /// 线性标注（两点 → 放置位置）。
    DimLinear {
        first: Option<DVec2>,
        second: Option<DVec2>,
    },
    /// 对齐标注（两点 → 放置位置）。
    DimAlign {
        first: Option<DVec2>,
        second: Option<DVec2>,
    },
    /// 半径标注（拾取圆捕获 (center, radius) → 放置位置）。
    DimRadius { circle: Option<(DVec2, f64)> },
    /// 角度标注（拾取两条线段图元下标）。
    DimAngle {
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

/// 一次编辑 / 标注点击的推进结果（G4-M17 编辑类统一入口）。
#[derive(Debug, Clone, PartialEq)]
pub enum EditOutcome {
    /// 交互推进（拾取 / 基点 / 参考点等已记录，草稿保持）。
    Continued,
    /// 单击落空或类型不符（容差内无图元 / 图元类型不支持该阶段）——草稿保持，
    /// 等下一击。
    Ignored,
    /// 应用成功：`replace` = 原地替换（下标, 新图元）；`add` = 追加图元
    /// （由调用方落地到实体列表）。
    Applied {
        replace: Option<(usize, SketchEntity)>,
        add: Vec<SketchEntity>,
    },
    /// 几何校验失败（无交点 / 参数退化等）——调用方记日志并复位草稿。
    Failed(String),
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
            SketchTool::Move => Self::Move {
                target: None,
                base: None,
            },
            SketchTool::Copy => Self::Copy {
                target: None,
                base: None,
            },
            SketchTool::Rotate => Self::Rotate {
                target: None,
                base: None,
                reference: None,
            },
            SketchTool::Scale => Self::Scale {
                target: None,
                base: None,
                reference: None,
            },
            SketchTool::Mirror => Self::Mirror {
                target: None,
                first: None,
            },
            SketchTool::Offset => Self::Offset,
            SketchTool::Trim => Self::Trim { boundary: None },
            SketchTool::Break => Self::Break,
            SketchTool::DimLinear => Self::DimLinear {
                first: None,
                second: None,
            },
            SketchTool::DimAlign => Self::DimAlign {
                first: None,
                second: None,
            },
            SketchTool::DimRadius => Self::DimRadius { circle: None },
            SketchTool::DimAngle => Self::DimAngle {
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
            Self::Move { target: None, .. } => "移动：单击选择图元",
            Self::Move {
                target: Some(_),
                base: None,
            } => "移动：单击基点",
            Self::Move { .. } => "移动：单击目标点",
            Self::Copy { target: None, .. } => "复制：单击选择图元",
            Self::Copy {
                target: Some(_),
                base: None,
            } => "复制：单击基点",
            Self::Copy { .. } => "复制：单击目标点（可连续复制，Esc 结束）",
            Self::Rotate { target: None, .. } => "旋转：单击选择图元",
            Self::Rotate {
                target: Some(_),
                base: None,
                ..
            } => "旋转：单击基点",
            Self::Rotate {
                base: Some(_),
                reference: None,
                ..
            } => "旋转：单击参考点",
            Self::Rotate { .. } => "旋转：单击目标点（定转角）",
            Self::Scale { target: None, .. } => "缩放：单击选择图元",
            Self::Scale {
                target: Some(_),
                base: None,
                ..
            } => "缩放：单击基点",
            Self::Scale {
                base: Some(_),
                reference: None,
                ..
            } => "缩放：单击参考点",
            Self::Scale { .. } => "缩放：单击目标点（定比例）",
            Self::Mirror { target: None, .. } => "镜像：单击选择图元",
            Self::Mirror {
                target: Some(_),
                first: None,
            } => "镜像：单击镜像线第一点",
            Self::Mirror { .. } => "镜像：单击镜像线第二点",
            Self::Offset => "偏移：单击线段或圆（距离见参数行，正 = 左侧 / 向外）",
            Self::Trim { boundary: None } => "修剪：单击边界线段",
            Self::Trim { boundary: Some(_) } => "修剪：单击被剪线段（保留单击侧）",
            Self::Break => "打断：单击线段内部一点",
            Self::DimLinear { first: None, .. } => "线性标注：单击尺寸起点",
            Self::DimLinear {
                first: Some(_),
                second: None,
            } => "线性标注：单击尺寸终点",
            Self::DimLinear { .. } => "线性标注：单击尺寸线位置",
            Self::DimAlign { first: None, .. } => "对齐标注：单击尺寸起点",
            Self::DimAlign {
                first: Some(_),
                second: None,
            } => "对齐标注：单击尺寸终点",
            Self::DimAlign { .. } => "对齐标注：单击尺寸线位置",
            Self::DimRadius { circle: None } => "半径标注：单击圆（捕获圆心半径）",
            Self::DimRadius { .. } => "半径标注：单击引线位置",
            Self::DimAngle { first: None, .. } => "角度标注：单击第一条线段",
            Self::DimAngle {
                first: Some(_),
                second: None,
            } => "角度标注：单击第二条线段",
            Self::DimAngle { .. } => "角度标注：单击角弧位置",
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
            // 编辑 / 标注类（G4-M17）：不走绘制点状态机（编辑器按 is_editing
            // 分派到 `edit_click` / 偏移直接应用）。
            Self::Move { .. }
            | Self::Copy { .. }
            | Self::Rotate { .. }
            | Self::Scale { .. }
            | Self::Mirror { .. }
            | Self::Offset
            | Self::Trim { .. }
            | Self::Break
            | Self::DimLinear { .. }
            | Self::DimAlign { .. }
            | Self::DimRadius { .. }
            | Self::DimAngle { .. } => ClickOutcome::Continued,
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
    /// 两击类（矩形 / 圆 / 多边形 / 构造线 / 射线）退回第一击，圆角 / 倒角
    /// 退回上一次选定，编辑类退回参考点 / 基点 / 图元，标注类退回上一点；
    /// 撤无可撤返回 false。
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
            Self::Move { target, base } | Self::Copy { target, base } => {
                base.take().is_some() || target.take().is_some()
            }
            Self::Rotate {
                target,
                base,
                reference,
            }
            | Self::Scale {
                target,
                base,
                reference,
            } => reference.take().is_some() || base.take().is_some() || target.take().is_some(),
            Self::Mirror { target, first } => first.take().is_some() || target.take().is_some(),
            Self::Trim { boundary } => boundary.take().is_some(),
            Self::DimLinear { first, second } | Self::DimAlign { first, second } => {
                second.take().is_some() || first.take().is_some()
            }
            Self::DimAngle { first, second } => second.take().is_some() || first.take().is_some(),
            Self::Point | Self::Offset | Self::Break | Self::DimRadius { .. } | Self::None => {
                false // 单击即完成 / 无状态 / 无草稿：无撤点
            }
        }
    }

    /// 编辑 / 标注草稿当前应高亮的已选图元下标（画布拾取反馈）：
    /// 圆角 / 倒角高亮第一条线段，Move/Copy/Rotate/Scale/Mirror 高亮 target、
    /// Trim 高亮 boundary、DimAngle 高亮 first；其余（无下标状态）返回 `None`。
    #[must_use]
    pub fn edit_highlight(&self) -> Option<usize> {
        match self {
            Self::Fillet { first, .. } | Self::Chamfer { first, .. } => *first,
            Self::Move { target, .. }
            | Self::Copy { target, .. }
            | Self::Rotate { target, .. }
            | Self::Scale { target, .. }
            | Self::Mirror { target, .. } => *target,
            Self::Trim { boundary } => *boundary,
            Self::DimAngle { first, .. } => *first,
            _ => None,
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
            // 标注（显示型）：无锚点，不参与捕捉。
            SketchEntity::DimLinear { .. }
            | SketchEntity::DimRadius { .. }
            | SketchEntity::DimAngle { .. } => {}
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

    /// 当前草稿是否为编辑 / 标注类（点击走 [`SketchDraft::edit_click`] 或偏移
    /// 直接应用，不走绘制状态机 [`SketchDraft::click`]）。
    #[must_use]
    pub fn is_editing(&self) -> bool {
        matches!(
            self,
            Self::Move { .. }
                | Self::Copy { .. }
                | Self::Rotate { .. }
                | Self::Scale { .. }
                | Self::Mirror { .. }
                | Self::Offset
                | Self::Trim { .. }
                | Self::Break
                | Self::DimLinear { .. }
                | Self::DimAlign { .. }
                | Self::DimRadius { .. }
                | Self::DimAngle { .. }
        )
    }

    /// 编辑 / 标注类工具统一点击推进（G4-M17 十一项；偏移无中间状态，在编辑器
    /// 侧直接应用 [`offset_entity`]，不走本函数）。
    ///
    /// 拾取阶段（选图元）用 `picked`（容差内无命中或图元类型不符 →
    /// [`EditOutcome::Ignored`]，草稿保持等下一击），点阶段（基点 / 目标点 /
    /// 标注位置）用 `p`；应用阶段从 `entities` 读原图元，返回
    /// [`EditOutcome::Applied`]（替换 / 追加由调用方落地）。交互完成时草稿
    /// 复位为 [`SketchDraft::None`]——**复制除外**（同一基点可连续放置多份，
    /// Esc / 换工具结束）；打断为无状态单击（可连续断多段）。
    pub fn edit_click(
        &mut self,
        p: DVec2,
        picked: Option<usize>,
        entities: &[SketchEntity],
    ) -> EditOutcome {
        match self {
            Self::Move { target, base } => match (*target, *base) {
                (None, _) => match picked {
                    None => EditOutcome::Ignored,
                    Some(i) => {
                        *target = Some(i);
                        EditOutcome::Continued
                    }
                },
                (Some(_), None) => {
                    *base = Some(p);
                    EditOutcome::Continued
                }
                (Some(i), Some(b)) => {
                    let Some(e) = entities.get(i) else {
                        return EditOutcome::Failed("图元下标越界".to_string());
                    };
                    let moved = translate_entity(e, p - b);
                    *self = Self::None;
                    EditOutcome::Applied {
                        replace: Some((i, moved)),
                        add: Vec::new(),
                    }
                }
            },
            Self::Copy { target, base } => match (*target, *base) {
                (None, _) => match picked {
                    None => EditOutcome::Ignored,
                    Some(i) => {
                        *target = Some(i);
                        EditOutcome::Continued
                    }
                },
                (Some(_), None) => {
                    *base = Some(p);
                    EditOutcome::Continued
                }
                (Some(i), Some(b)) => {
                    let Some(e) = entities.get(i) else {
                        return EditOutcome::Failed("图元下标越界".to_string());
                    };
                    // 草稿不复位：同一基点连续放置多份（Esc / 换工具结束）。
                    EditOutcome::Applied {
                        replace: None,
                        add: vec![translate_entity(e, p - b)],
                    }
                }
            },
            Self::Rotate {
                target,
                base,
                reference,
            } => match (*target, *base, *reference) {
                (None, _, _) => match picked {
                    None => EditOutcome::Ignored,
                    Some(i) => {
                        *target = Some(i);
                        EditOutcome::Continued
                    }
                },
                (Some(_), None, _) => {
                    *base = Some(p);
                    EditOutcome::Continued
                }
                (Some(_), Some(b), None) => {
                    if (p - b).length() <= f64::EPSILON {
                        return EditOutcome::Ignored; // 参考点与基点重合：等下一击
                    }
                    *reference = Some(p);
                    EditOutcome::Continued
                }
                (Some(i), Some(b), Some(r)) => {
                    let Some(e) = entities.get(i) else {
                        return EditOutcome::Failed("图元下标越界".to_string());
                    };
                    // 转角 = 目标方向角 − 参考方向角（都相对基点）。
                    let angle = (p - b).y.atan2((p - b).x) - (r - b).y.atan2((r - b).x);
                    let rotated = rotate_entity(e, b, angle);
                    *self = Self::None;
                    EditOutcome::Applied {
                        replace: Some((i, rotated)),
                        add: Vec::new(),
                    }
                }
            },
            Self::Scale {
                target,
                base,
                reference,
            } => match (*target, *base, *reference) {
                (None, _, _) => match picked {
                    None => EditOutcome::Ignored,
                    Some(i) => {
                        *target = Some(i);
                        EditOutcome::Continued
                    }
                },
                (Some(_), None, _) => {
                    *base = Some(p);
                    EditOutcome::Continued
                }
                (Some(_), Some(b), None) => {
                    if (p - b).length() <= f64::EPSILON {
                        return EditOutcome::Ignored; // 参考点与基点重合：等下一击
                    }
                    *reference = Some(p);
                    EditOutcome::Continued
                }
                (Some(i), Some(b), Some(r)) => {
                    let Some(e) = entities.get(i) else {
                        return EditOutcome::Failed("图元下标越界".to_string());
                    };
                    let d_ref = (r - b).length();
                    if d_ref <= f64::EPSILON || (p - b).length() <= f64::EPSILON {
                        return EditOutcome::Ignored; // 比例退化：等下一击
                    }
                    // 比例 = 目标距 / 参考距（都相对基点）。
                    let factor = (p - b).length() / d_ref;
                    let scaled = scale_entity(e, b, factor);
                    *self = Self::None;
                    EditOutcome::Applied {
                        replace: Some((i, scaled)),
                        add: Vec::new(),
                    }
                }
            },
            Self::Mirror { target, first } => match (*target, *first) {
                (None, _) => match picked {
                    None => EditOutcome::Ignored,
                    Some(i) => {
                        *target = Some(i);
                        EditOutcome::Continued
                    }
                },
                (Some(_), None) => {
                    *first = Some(p);
                    EditOutcome::Continued
                }
                (Some(i), Some(f)) => {
                    let Some(e) = entities.get(i) else {
                        return EditOutcome::Failed("图元下标越界".to_string());
                    };
                    if (p - f).length() <= f64::EPSILON {
                        return EditOutcome::Ignored; // 镜像线退化：等下一击
                    }
                    let mirrored = mirror_entity(e, f, p);
                    *self = Self::None;
                    EditOutcome::Applied {
                        replace: Some((i, mirrored)),
                        add: Vec::new(),
                    }
                }
            },
            Self::Trim { boundary } => match *boundary {
                None => match picked {
                    None => EditOutcome::Ignored,
                    Some(i) if single_segment(entities.get(i)).is_some() => {
                        *boundary = Some(i);
                        EditOutcome::Continued
                    }
                    Some(_) => EditOutcome::Ignored, // 边界必须是单段折线
                },
                Some(bi) => match picked {
                    None => EditOutcome::Ignored,
                    Some(i) => {
                        let (Some(b_seg), Some(t_seg)) = (
                            single_segment(entities.get(bi)),
                            single_segment(entities.get(i)),
                        ) else {
                            return EditOutcome::Ignored;
                        };
                        if segment_intersection(b_seg[0], b_seg[1], t_seg[0], t_seg[1]).is_none() {
                            return EditOutcome::Failed("被剪线段与边界无交点".to_string());
                        }
                        let Some((a, b)) = trim_segment(t_seg[0], t_seg[1], b_seg[0], b_seg[1], p)
                        else {
                            return EditOutcome::Failed("修剪失败：被剪线段退化".to_string());
                        };
                        *self = Self::None;
                        EditOutcome::Applied {
                            replace: Some((i, SketchEntity::Polyline(vec![a, b]))),
                            add: Vec::new(),
                        }
                    }
                },
            },
            Self::Break => {
                let Some(i) = picked else {
                    return EditOutcome::Ignored;
                };
                let Some(seg) = single_segment(entities.get(i)) else {
                    return EditOutcome::Ignored; // 打断只作用于单段折线
                };
                // 打断点取单击点在线段上的投影（端点 / 段外投影到端点 → None）。
                let Some(([a, mid], [_, b])) = split_segment(seg[0], seg[1], p) else {
                    return EditOutcome::Failed("打断点须落在线段内部".to_string());
                };
                EditOutcome::Applied {
                    replace: Some((i, SketchEntity::Polyline(vec![a, mid]))),
                    add: vec![SketchEntity::Polyline(vec![mid, b])],
                }
            }
            Self::DimLinear { first, second } => match (*first, *second) {
                (None, _) => {
                    *first = Some(p);
                    EditOutcome::Continued
                }
                (Some(f), None) => {
                    if (p - f).length() <= f64::EPSILON {
                        return EditOutcome::Ignored; // 零长度尺寸：等下一击
                    }
                    *second = Some(p);
                    EditOutcome::Continued
                }
                (Some(f), Some(s)) => {
                    let entity = SketchEntity::DimLinear {
                        from: f,
                        to: s,
                        pos: p,
                        linear: true,
                    };
                    *self = Self::None;
                    EditOutcome::Applied {
                        replace: None,
                        add: vec![entity],
                    }
                }
            },
            Self::DimAlign { first, second } => match (*first, *second) {
                (None, _) => {
                    *first = Some(p);
                    EditOutcome::Continued
                }
                (Some(f), None) => {
                    if (p - f).length() <= f64::EPSILON {
                        return EditOutcome::Ignored; // 零长度尺寸：等下一击
                    }
                    *second = Some(p);
                    EditOutcome::Continued
                }
                (Some(f), Some(s)) => {
                    let entity = SketchEntity::DimLinear {
                        from: f,
                        to: s,
                        pos: p,
                        linear: false,
                    };
                    *self = Self::None;
                    EditOutcome::Applied {
                        replace: None,
                        add: vec![entity],
                    }
                }
            },
            Self::DimRadius { circle } => match *circle {
                None => match picked.and_then(|i| entities.get(i)) {
                    Some(SketchEntity::Circle { center, radius }) => {
                        *circle = Some((*center, *radius));
                        EditOutcome::Continued
                    }
                    _ => EditOutcome::Ignored, // 半径标注只挂在圆上
                },
                Some((c, r)) => {
                    let entity = SketchEntity::DimRadius {
                        center: c,
                        radius: r,
                        pos: p,
                    };
                    *self = Self::None;
                    EditOutcome::Applied {
                        replace: None,
                        add: vec![entity],
                    }
                }
            },
            Self::DimAngle { first, second } => match (*first, *second) {
                (None, _) => match picked {
                    None => EditOutcome::Ignored,
                    Some(i) if single_segment(entities.get(i)).is_some() => {
                        *first = Some(i);
                        EditOutcome::Continued
                    }
                    Some(_) => EditOutcome::Ignored, // 角度标注只挂线段
                },
                (Some(f), None) => match picked {
                    None => EditOutcome::Ignored,
                    Some(i) if i != f && single_segment(entities.get(i)).is_some() => {
                        *second = Some(i);
                        EditOutcome::Continued
                    }
                    _ => EditOutcome::Ignored, // 同一图元 / 非线段
                },
                (Some(f), Some(s)) => {
                    let (Some(seg1), Some(seg2)) = (
                        single_segment(entities.get(f)),
                        single_segment(entities.get(s)),
                    ) else {
                        return EditOutcome::Failed("线段图元已失效".to_string());
                    };
                    // 角顶点 = 两线段交点；两臂取距交点较远的端点（方向点）。
                    let Some(v) = segment_intersection(seg1[0], seg1[1], seg2[0], seg2[1]) else {
                        return EditOutcome::Failed("两线段平行或共线，无角顶点".to_string());
                    };
                    let entity = SketchEntity::DimAngle {
                        vertex: v,
                        arm1: far_end(seg1, v),
                        arm2: far_end(seg2, v),
                        pos: p,
                    };
                    *self = Self::None;
                    EditOutcome::Applied {
                        replace: None,
                        add: vec![entity],
                    }
                }
            },
            // 绘制类草稿不经本函数（编辑器按 is_editing 分派）。
            _ => EditOutcome::Ignored,
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

/// 单段折线提取（修剪 / 打断 / 角度标注只作用于线段图元；多段折线 / 曲线 /
/// 辅助图元返回 `None`）。
fn single_segment(e: Option<&SketchEntity>) -> Option<[DVec2; 2]> {
    match e? {
        SketchEntity::Polyline(points) if points.len() == 2 => Some([points[0], points[1]]),
        _ => None,
    }
}

/// 图元几何变换（G4-M17 编辑类底座）：所有顶点 / 位置点经 `f` 映射。
///
/// - 折线 / 贝塞尔 / 点 / 构造线：全点映射；
/// - 圆：圆心映射，半径用探针点（圆心 + (r, 0)）映射还原——平移 / 旋转 /
///   镜像半径不变，均匀缩放按比例；
/// - 椭圆：长轴端点探针映射还原新主轴方向与缩放系数（major / minor 同乘 k）；
/// - 弧：起终点探针映射还原新半径与起终角，`flip_handedness`（镜像）时
///   起终点互换后 ccw 归一——镜像翻手性，几何弧不变、扫向反转；
/// - 标注：位置点全映射（显示型，数值由渲染层按映射后几何重算）。
#[must_use]
pub fn transform_entity(
    e: &SketchEntity,
    f: &dyn Fn(DVec2) -> DVec2,
    flip_handedness: bool,
) -> SketchEntity {
    match e {
        SketchEntity::Polyline(points) => {
            SketchEntity::Polyline(points.iter().map(|&v| f(v)).collect())
        }
        SketchEntity::Bezier(segments) => SketchEntity::Bezier(
            segments
                .iter()
                .map(|seg| seg.iter().map(|&v| f(v)).collect())
                .collect(),
        ),
        SketchEntity::Point(p) => SketchEntity::Point(f(*p)),
        SketchEntity::AuxLine { from, to, ray } => SketchEntity::AuxLine {
            from: f(*from),
            to: f(*to),
            ray: *ray,
        },
        SketchEntity::Circle { center, radius } => {
            let probe = *center + DVec2::new(*radius, 0.0);
            SketchEntity::Circle {
                center: f(*center),
                radius: (f(probe) - f(*center)).length(),
            }
        }
        SketchEntity::Ellipse {
            center,
            major,
            minor,
            major_angle,
        } => {
            let d = DVec2::new(major * major_angle.cos(), major * major_angle.sin());
            let new_center = f(*center);
            let new_d = f(*center + d) - new_center;
            // 探针距离 |new_d| = 变换比例 × major = 新长半轴；短半轴同比例。
            let k = new_d.length();
            let ratio = if *major > f64::EPSILON {
                k / *major
            } else {
                0.0
            };
            SketchEntity::Ellipse {
                center: new_center,
                major: k,
                minor: minor * ratio,
                major_angle: new_d.y.atan2(new_d.x),
            }
        }
        SketchEntity::Arc {
            center,
            radius,
            start_angle,
            end_angle,
        } => {
            let new_center = f(*center);
            let sp = DVec2::new(radius * start_angle.cos(), radius * start_angle.sin());
            let ep = DVec2::new(radius * end_angle.cos(), radius * end_angle.sin());
            let new_sp = f(*center + sp) - new_center;
            let new_ep = f(*center + ep) - new_center;
            let (mut sa, mut ea) = (new_sp.y.atan2(new_sp.x), new_ep.y.atan2(new_ep.x));
            if flip_handedness {
                std::mem::swap(&mut sa, &mut ea); // 镜像翻手性：起终点互换
            }
            let (sa, ea) = core_app::normalize_arc_angles(sa, ea);
            SketchEntity::Arc {
                center: new_center,
                radius: new_sp.length(),
                start_angle: sa,
                end_angle: ea,
            }
        }
        SketchEntity::DimLinear {
            from,
            to,
            pos,
            linear,
        } => SketchEntity::DimLinear {
            from: f(*from),
            to: f(*to),
            pos: f(*pos),
            linear: *linear,
        },
        SketchEntity::DimRadius {
            center,
            radius,
            pos,
        } => {
            let probe = *center + DVec2::new(*radius, 0.0);
            SketchEntity::DimRadius {
                center: f(*center),
                radius: (f(probe) - f(*center)).length(),
                pos: f(*pos),
            }
        }
        SketchEntity::DimAngle {
            vertex,
            arm1,
            arm2,
            pos,
        } => SketchEntity::DimAngle {
            vertex: f(*vertex),
            arm1: f(*arm1),
            arm2: f(*arm2),
            pos: f(*pos),
        },
    }
}

/// 平移图元（移动 / 复制共用）。
#[must_use]
pub fn translate_entity(e: &SketchEntity, d: DVec2) -> SketchEntity {
    transform_entity(e, &|p| p + d, false)
}

/// 图元绕 pivot 旋转 angle_rad（ccw 为正）。
#[must_use]
pub fn rotate_entity(e: &SketchEntity, pivot: DVec2, angle_rad: f64) -> SketchEntity {
    transform_entity(e, &|p| rotate_point(p, pivot, angle_rad), false)
}

/// 图元绕 pivot 均匀缩放 factor 倍（负比例 = 点反射，手性保持）。
#[must_use]
pub fn scale_entity(e: &SketchEntity, pivot: DVec2, factor: f64) -> SketchEntity {
    transform_entity(e, &|p| scale_point(p, pivot, factor), false)
}

/// 图元关于直线（line_a → line_b）镜像（翻手性：弧起终点互换）。
#[must_use]
pub fn mirror_entity(e: &SketchEntity, line_a: DVec2, line_b: DVec2) -> SketchEntity {
    transform_entity(e, &|p| mirror_point(p, line_a, line_b), true)
}

/// 偏移图元（G4-M17）：单段折线平行偏移（正 = a→b 左侧）/ 圆同心偏移
/// （正 = 向外）；其余图元 / 线段退化 / 向内偏移超出半径返回 `Err`
/// （原图元不动，调用方按 fillet 同款生成新图元）。
pub fn offset_entity(e: &SketchEntity, distance: f64) -> Result<SketchEntity, String> {
    match e {
        SketchEntity::Polyline(points) if points.len() == 2 => {
            let Some((a, b)) = offset_segment(points[0], points[1], distance) else {
                return Err("偏移失败：线段退化".to_string());
            };
            Ok(SketchEntity::Polyline(vec![a, b]))
        }
        SketchEntity::Circle { center, radius } => {
            let Some((c, r)) = offset_circle(*center, *radius, distance) else {
                return Err("偏移失败：半径不足（向内偏移超出半径）".to_string());
            };
            Ok(SketchEntity::Circle {
                center: c,
                radius: r,
            })
        }
        _ => Err("偏移只支持单段折线或圆".to_string()),
    }
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

    /// 工具全集与提示文案完整（编辑器工具栏枚举序 = 绘制 15 + 编辑标注 12）。
    #[test]
    fn tool_set_and_hints_are_complete() {
        assert_eq!(SketchTool::ALL.len(), 27);
        assert_eq!(DRAWING_TOOLS.len(), 15);
        assert_eq!(EDITING_TOOLS.len(), 12);
        assert!(!SketchTool::ALL.is_empty());
        for tool in SketchTool::ALL {
            assert!(!tool.label().is_empty());
            assert!(!SketchDraft::new(tool).hint().is_empty());
            assert!(
                !SketchDraft::new(tool).is_editing() || EDITING_TOOLS.contains(&tool),
                "is_editing 与编辑行一致"
            );
        }
        for tool in EDITING_TOOLS {
            assert!(
                SketchDraft::new(tool).is_editing(),
                "编辑行工具应为编辑草稿"
            );
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

    // —— G4-M17 编辑 / 标注类（edit_click + 变换 / 偏移）——

    /// 移动：三击（拾取 → 基点 → 目标点）→ 顶点平移，草稿复位。
    #[test]
    fn edit_move_translates_and_resets() {
        let entities = vec![SketchEntity::Polyline(vec![
            DVec2::ZERO,
            DVec2::new(10.0, 0.0),
        ])];
        let mut draft = SketchDraft::new(SketchTool::Move);
        assert_eq!(
            draft.edit_click(DVec2::ZERO, Some(0), &entities),
            EditOutcome::Continued
        );
        assert_eq!(
            draft.edit_click(DVec2::new(10.0, 0.0), None, &entities),
            EditOutcome::Continued
        );
        match draft.edit_click(DVec2::new(15.0, 5.0), None, &entities) {
            EditOutcome::Applied {
                replace: Some((0, SketchEntity::Polyline(pts))),
                add,
            } => {
                assert!(add.is_empty());
                assert_eq!(pts.len(), 2);
                assert_v(pts[0], [5.0, 5.0]);
                assert_v(pts[1], [15.0, 5.0]);
            }
            other => panic!("应为替换后的平移折线，实得 {other:?}"),
        }
        // 交互完成草稿复位（编辑器据以重启同工具）。
        assert_eq!(draft, SketchDraft::None);
        // 拾取落空 → Ignored。
        let mut draft = SketchDraft::new(SketchTool::Move);
        assert_eq!(
            draft.edit_click(DVec2::ZERO, None, &entities),
            EditOutcome::Ignored
        );
    }

    /// 复制：三击后草稿不复位（同一基点连续放置多份）。
    #[test]
    fn edit_copy_places_repeated_copies() {
        let entities = vec![SketchEntity::Polyline(vec![
            DVec2::ZERO,
            DVec2::new(10.0, 0.0),
        ])];
        let mut draft = SketchDraft::new(SketchTool::Copy);
        draft.edit_click(DVec2::ZERO, Some(0), &entities);
        draft.edit_click(DVec2::ZERO, None, &entities); // 基点 = 原点
        // 第一份：目标点 (15, 0) → 副本 [(15,0),(25,0)]（基点平移到目标点）。
        match draft.edit_click(DVec2::new(15.0, 0.0), None, &entities) {
            EditOutcome::Applied { replace, add } => {
                assert!(replace.is_none(), "复制是追加不是替换");
                assert_eq!(add.len(), 1);
                match &add[0] {
                    SketchEntity::Polyline(pts) => {
                        assert_v(pts[0], [15.0, 0.0]);
                        assert_v(pts[1], [25.0, 0.0]);
                    }
                    other => panic!("应为折线副本，实得 {other:?}"),
                }
            }
            other => panic!("应为 Applied，实得 {other:?}"),
        }
        // 草稿不复位：同一基点直接放第二份。
        assert!(matches!(
            draft,
            SketchDraft::Copy {
                target: Some(0),
                base: Some(_),
            }
        ));
        match draft.edit_click(DVec2::new(15.0, 10.0), None, &entities) {
            EditOutcome::Applied { add, .. } => match &add[0] {
                SketchEntity::Polyline(pts) => {
                    assert_v(pts[0], [15.0, 10.0]);
                    assert_v(pts[1], [25.0, 10.0]);
                }
                other => panic!("第二份应为折线副本，实得 {other:?}"),
            },
            other => panic!("第二份应为 Applied，实得 {other:?}"),
        }
    }

    /// 旋转：四击（拾取 → 基点 → 参考点 → 目标点）→ 绕基点旋转参考差角。
    #[test]
    fn edit_rotate_by_reference_angle() {
        let entities = vec![SketchEntity::Polyline(vec![
            DVec2::new(10.0, 0.0),
            DVec2::new(20.0, 0.0),
        ])];
        let mut draft = SketchDraft::new(SketchTool::Rotate);
        draft.edit_click(DVec2::ZERO, Some(0), &entities);
        draft.edit_click(DVec2::ZERO, None, &entities); // 基点 = 原点
        draft.edit_click(DVec2::new(10.0, 0.0), None, &entities); // 参考点
        // 目标点 (0,10)：转角 = atan2(10,0) − 0 = 90°。
        match draft.edit_click(DVec2::new(0.0, 10.0), None, &entities) {
            EditOutcome::Applied {
                replace: Some((0, SketchEntity::Polyline(pts))),
                ..
            } => {
                assert_v(pts[0], [0.0, 10.0]);
                assert_v(pts[1], [0.0, 20.0]);
            }
            other => panic!("应为替换后的旋转折线，实得 {other:?}"),
        }
        // 参考点与基点重合 → Ignored（草稿保持等下一击）。
        let mut draft = SketchDraft::new(SketchTool::Rotate);
        draft.edit_click(DVec2::ZERO, Some(0), &entities);
        draft.edit_click(DVec2::ZERO, None, &entities);
        assert_eq!(
            draft.edit_click(DVec2::ZERO, None, &entities),
            EditOutcome::Ignored
        );
    }

    /// 缩放：四击 → 绕基点等比缩放（圆半径同乘）。
    #[test]
    fn edit_scale_doubles_geometry() {
        let entities = vec![SketchEntity::Circle {
            center: DVec2::new(10.0, 0.0),
            radius: 5.0,
        }];
        let mut draft = SketchDraft::new(SketchTool::Scale);
        draft.edit_click(DVec2::ZERO, Some(0), &entities);
        draft.edit_click(DVec2::ZERO, None, &entities); // 基点 = 原点
        draft.edit_click(DVec2::new(10.0, 0.0), None, &entities); // 参考距 10
        match draft.edit_click(DVec2::new(20.0, 0.0), None, &entities) {
            EditOutcome::Applied {
                replace: Some((0, SketchEntity::Circle { center, radius, .. })),
                ..
            } => {
                assert_v(center, [20.0, 0.0]);
                assert!((radius - 10.0).abs() < 1e-9, "半径应同乘 2×");
            }
            other => panic!("应为替换后的缩放圆，实得 {other:?}"),
        }
    }

    /// 镜像：三击 → 顶点关于直线反射，弧起终点互换（翻手性）后 ccw 归一。
    #[test]
    fn edit_mirror_flips_arc_handedness() {
        use std::f64::consts::{FRAC_PI_2, PI};
        let entities = vec![SketchEntity::Arc {
            center: DVec2::ZERO,
            radius: 10.0,
            start_angle: 0.0,
            end_angle: FRAC_PI_2,
        }];
        let mut draft = SketchDraft::new(SketchTool::Mirror);
        draft.edit_click(DVec2::ZERO, Some(0), &entities);
        draft.edit_click(DVec2::ZERO, None, &entities); // 镜像线第一点
        // 镜像线 = y 轴（第二点 (0,10)）：起角 0 → π、终角 π/2 不变，
        // 翻手性互换后 ccw 归一 → (π/2, π)。
        match draft.edit_click(DVec2::new(0.0, 10.0), None, &entities) {
            EditOutcome::Applied {
                replace:
                    Some((
                        0,
                        SketchEntity::Arc {
                            center,
                            radius,
                            start_angle,
                            end_angle,
                        },
                    )),
                add,
            } => {
                assert!(add.is_empty());
                assert_v(center, [0.0, 0.0]);
                assert!((radius - 10.0).abs() < 1e-9);
                assert!((start_angle - FRAC_PI_2).abs() < 1e-9, "起终点应互换");
                assert!((end_angle - PI).abs() < 1e-9);
            }
            other => panic!("应为替换后的反向弧，实得 {other:?}"),
        }
        assert_eq!(draft, SketchDraft::None);
        // 镜像线退化（两点重合）→ Ignored。
        let mut draft = SketchDraft::new(SketchTool::Mirror);
        draft.edit_click(DVec2::ZERO, Some(0), &entities);
        draft.edit_click(DVec2::ZERO, None, &entities);
        assert_eq!(
            draft.edit_click(DVec2::ZERO, None, &entities),
            EditOutcome::Ignored
        );
    }

    /// 椭圆变换探针：旋转随主轴角度、缩放同乘长短轴（探针点还原）。
    #[test]
    fn transform_ellipse_probe_keeps_shape() {
        use std::f64::consts::FRAC_PI_2;
        let e = SketchEntity::Ellipse {
            center: DVec2::new(10.0, 0.0),
            major: 10.0,
            minor: 4.0,
            major_angle: 0.0,
        };
        // 旋转 90°：主轴角度随转，长短轴不变。
        match rotate_entity(&e, DVec2::ZERO, FRAC_PI_2) {
            SketchEntity::Ellipse {
                center,
                major,
                minor,
                major_angle,
            } => {
                assert_v(center, [0.0, 10.0]);
                assert!((major - 10.0).abs() < 1e-9);
                assert!((minor - 4.0).abs() < 1e-9);
                assert!((major_angle - FRAC_PI_2).abs() < 1e-9);
            }
            other => panic!("应为旋转后椭圆，实得 {other:?}"),
        }
        // 缩放 2×：长短轴同乘，角度不变。
        match scale_entity(&e, DVec2::ZERO, 2.0) {
            SketchEntity::Ellipse {
                center,
                major,
                minor,
                major_angle,
            } => {
                assert_v(center, [20.0, 0.0]);
                assert!((major - 20.0).abs() < 1e-9);
                assert!((minor - 8.0).abs() < 1e-9);
                assert!(major_angle.abs() < 1e-9);
            }
            other => panic!("应为缩放后椭圆，实得 {other:?}"),
        }
    }

    /// 偏移三态：线段左侧 / 圆向内 / 半径不足与不支持图元拒绝。
    #[test]
    fn edit_offset_segment_and_circle() {
        // 线段：正 = a→b 左侧（y+）。
        let seg = SketchEntity::Polyline(vec![DVec2::ZERO, DVec2::new(10.0, 0.0)]);
        match offset_entity(&seg, 5.0) {
            Ok(SketchEntity::Polyline(pts)) => {
                assert_v(pts[0], [0.0, 5.0]);
                assert_v(pts[1], [10.0, 5.0]);
            }
            other => panic!("应为左偏线段，实得 {other:?}"),
        }
        // 圆：负 = 向内偏。
        let circle = SketchEntity::Circle {
            center: DVec2::ZERO,
            radius: 10.0,
        };
        match offset_entity(&circle, -3.0) {
            Ok(SketchEntity::Circle { center, radius }) => {
                assert_v(center, [0.0, 0.0]);
                assert!((radius - 7.0).abs() < 1e-9);
            }
            other => panic!("应为同心内偏圆，实得 {other:?}"),
        }
        // 圆：向内偏超出半径 → Err。
        assert!(offset_entity(&circle, -12.0).is_err());
        // 曲线 / 多段折线 → Err。
        let curve = SketchEntity::Bezier(vec![vec![DVec2::ZERO, DVec2::new(10.0, 0.0)]]);
        assert!(offset_entity(&curve, 1.0).is_err());
        let multi = SketchEntity::Polyline(vec![
            DVec2::ZERO,
            DVec2::new(10.0, 0.0),
            DVec2::new(10.0, 10.0),
        ]);
        assert!(offset_entity(&multi, 1.0).is_err());
    }

    /// 修剪：两击 → 保留单击侧（左 / 右两态）。
    #[test]
    fn edit_trim_keeps_clicked_side() {
        let entities = vec![
            SketchEntity::Polyline(vec![DVec2::new(5.0, -10.0), DVec2::new(5.0, 10.0)]),
            SketchEntity::Polyline(vec![DVec2::ZERO, DVec2::new(10.0, 0.0)]),
        ];
        // 保留左段：被剪线段单击 (2,0)。
        let mut draft = SketchDraft::new(SketchTool::Trim);
        assert_eq!(
            draft.edit_click(DVec2::new(5.0, 0.0), Some(0), &entities),
            EditOutcome::Continued
        );
        match draft.edit_click(DVec2::new(2.0, 0.0), Some(1), &entities) {
            EditOutcome::Applied {
                replace: Some((1, SketchEntity::Polyline(pts))),
                add,
            } => {
                assert!(add.is_empty());
                assert_eq!(pts.len(), 2);
                assert_v(pts[0], [0.0, 0.0]);
                assert_v(pts[1], [5.0, 0.0]);
            }
            other => panic!("应为左段修剪结果，实得 {other:?}"),
        }
        assert_eq!(draft, SketchDraft::None);
        // 保留右段：单击 (8,0)。
        let mut draft = SketchDraft::new(SketchTool::Trim);
        draft.edit_click(DVec2::new(5.0, 0.0), Some(0), &entities);
        match draft.edit_click(DVec2::new(8.0, 0.0), Some(1), &entities) {
            EditOutcome::Applied {
                replace: Some((1, SketchEntity::Polyline(pts))),
                ..
            } => {
                assert_v(pts[0], [5.0, 0.0]);
                assert_v(pts[1], [10.0, 0.0]);
            }
            other => panic!("应为右段修剪结果，实得 {other:?}"),
        }
        // 无交点 → Failed。
        let parallel = vec![
            SketchEntity::Polyline(vec![DVec2::new(5.0, -10.0), DVec2::new(5.0, 10.0)]),
            SketchEntity::Polyline(vec![DVec2::new(20.0, 0.0), DVec2::new(30.0, 0.0)]),
        ];
        let mut draft = SketchDraft::new(SketchTool::Trim);
        draft.edit_click(DVec2::new(5.0, 0.0), Some(0), &parallel);
        assert!(matches!(
            draft.edit_click(DVec2::new(25.0, 0.0), Some(1), &parallel),
            EditOutcome::Failed(_)
        ));
    }

    /// 打断：段内点断两段（replace + add）；端点 Failed；落空 Ignored。
    #[test]
    fn edit_break_splits_at_inner_point() {
        let entities = vec![SketchEntity::Polyline(vec![
            DVec2::ZERO,
            DVec2::new(10.0, 0.0),
        ])];
        let mut draft = SketchDraft::new(SketchTool::Break);
        match draft.edit_click(DVec2::new(4.0, 0.0), Some(0), &entities) {
            EditOutcome::Applied {
                replace: Some((0, SketchEntity::Polyline(first))),
                add,
            } => {
                assert_eq!(add.len(), 1);
                assert_v(first[0], [0.0, 0.0]);
                assert_v(first[1], [4.0, 0.0]);
                match &add[0] {
                    SketchEntity::Polyline(second) => {
                        assert_v(second[0], [4.0, 0.0]);
                        assert_v(second[1], [10.0, 0.0]);
                    }
                    other => panic!("第二段应为折线，实得 {other:?}"),
                }
            }
            other => panic!("应为打断两段结果，实得 {other:?}"),
        }
        // 端点（含段外投影到端点）→ Failed。
        let mut draft = SketchDraft::new(SketchTool::Break);
        assert!(matches!(
            draft.edit_click(DVec2::ZERO, Some(0), &entities),
            EditOutcome::Failed(_)
        ));
        // 落空拾取 / 非单段折线 → Ignored。
        assert_eq!(
            draft.edit_click(DVec2::new(4.0, 0.0), None, &entities),
            EditOutcome::Ignored
        );
    }

    /// 线性 / 对齐标注：三击 → DimLinear（linear 真 / 假）；重合点击 Ignored。
    #[test]
    fn edit_dim_linear_and_align() {
        let entities: Vec<SketchEntity> = Vec::new();
        // 线性标注（linear = true）。
        let mut draft = SketchDraft::new(SketchTool::DimLinear);
        assert_eq!(
            draft.edit_click(DVec2::ZERO, None, &entities),
            EditOutcome::Continued
        );
        assert_eq!(
            draft.edit_click(DVec2::new(10.0, 0.0), None, &entities),
            EditOutcome::Continued
        );
        match draft.edit_click(DVec2::new(5.0, 8.0), None, &entities) {
            EditOutcome::Applied { replace, add } => {
                assert!(replace.is_none());
                assert_eq!(add.len(), 1);
                match &add[0] {
                    SketchEntity::DimLinear {
                        from,
                        to,
                        pos,
                        linear,
                    } => {
                        assert_v(*from, [0.0, 0.0]);
                        assert_v(*to, [10.0, 0.0]);
                        assert_v(*pos, [5.0, 8.0]);
                        assert!(*linear, "线性标注 linear = true");
                    }
                    other => panic!("应为线性标注，实得 {other:?}"),
                }
            }
            other => panic!("应为 Applied，实得 {other:?}"),
        }
        assert_eq!(draft, SketchDraft::None);
        // 对齐标注（linear = false）。
        let mut draft = SketchDraft::new(SketchTool::DimAlign);
        draft.edit_click(DVec2::ZERO, None, &entities);
        draft.edit_click(DVec2::new(10.0, 0.0), None, &entities);
        match draft.edit_click(DVec2::new(5.0, 8.0), None, &entities) {
            EditOutcome::Applied { add, .. } => match &add[0] {
                SketchEntity::DimLinear { linear, .. } => assert!(!*linear),
                other => panic!("应为对齐标注，实得 {other:?}"),
            },
            other => panic!("对齐标注应为 Applied，实得 {other:?}"),
        }
        // 零长度尺寸（重合点击）→ Ignored。
        let mut draft = SketchDraft::new(SketchTool::DimLinear);
        draft.edit_click(DVec2::ZERO, None, &entities);
        assert_eq!(
            draft.edit_click(DVec2::ZERO, None, &entities),
            EditOutcome::Ignored
        );
    }

    /// 半径标注：拾圆捕获 (center, radius) → 位置定标注点；非圆 Ignored。
    #[test]
    fn edit_dim_radius_captures_circle() {
        let entities = vec![SketchEntity::Circle {
            center: DVec2::ZERO,
            radius: 7.0,
        }];
        let mut draft = SketchDraft::new(SketchTool::DimRadius);
        assert_eq!(
            draft.edit_click(DVec2::new(7.0, 0.0), Some(0), &entities),
            EditOutcome::Continued
        );
        assert!(matches!(
            draft,
            SketchDraft::DimRadius {
                circle: Some((_, r)),
            } if (r - 7.0).abs() < 1e-9
        ));
        match draft.edit_click(DVec2::new(10.0, 5.0), None, &entities) {
            EditOutcome::Applied { replace, add } => {
                assert!(replace.is_none());
                match &add[0] {
                    SketchEntity::DimRadius {
                        center,
                        radius,
                        pos,
                    } => {
                        assert_v(*center, [0.0, 0.0]);
                        assert!((radius - 7.0).abs() < 1e-9);
                        assert_v(*pos, [10.0, 5.0]);
                    }
                    other => panic!("应为半径标注，实得 {other:?}"),
                }
            }
            other => panic!("应为 Applied，实得 {other:?}"),
        }
        // 非圆拾取 → Ignored（草稿保持等下一击）。
        let lines = vec![SketchEntity::Polyline(vec![
            DVec2::ZERO,
            DVec2::new(10.0, 0.0),
        ])];
        let mut draft = SketchDraft::new(SketchTool::DimRadius);
        assert_eq!(
            draft.edit_click(DVec2::ZERO, Some(0), &lines),
            EditOutcome::Ignored
        );
    }

    /// 角度标注：两线段交点定角顶点、远端定臂方向 → 位置定角弧；平行 Failed。
    #[test]
    fn edit_dim_angle_needs_intersection() {
        let entities = vec![
            SketchEntity::Polyline(vec![DVec2::ZERO, DVec2::new(20.0, 0.0)]),
            SketchEntity::Polyline(vec![DVec2::new(5.0, -5.0), DVec2::new(5.0, 10.0)]),
        ];
        let mut draft = SketchDraft::new(SketchTool::DimAngle);
        assert_eq!(
            draft.edit_click(DVec2::new(10.0, 0.0), Some(0), &entities),
            EditOutcome::Continued
        );
        assert_eq!(
            draft.edit_click(DVec2::new(5.0, 5.0), Some(1), &entities),
            EditOutcome::Continued
        );
        match draft.edit_click(DVec2::new(12.0, 4.0), None, &entities) {
            EditOutcome::Applied { replace, add } => {
                assert!(replace.is_none());
                assert_eq!(add.len(), 1);
                match &add[0] {
                    SketchEntity::DimAngle {
                        vertex,
                        arm1,
                        arm2,
                        pos,
                    } => {
                        assert_v(*vertex, [5.0, 0.0]);
                        assert_v(*arm1, [20.0, 0.0]);
                        assert_v(*arm2, [5.0, 10.0]);
                        assert_v(*pos, [12.0, 4.0]);
                    }
                    other => panic!("应为角度标注，实得 {other:?}"),
                }
            }
            other => panic!("应为 Applied，实得 {other:?}"),
        }
        // 平行线段 → Failed。
        let parallel = vec![
            SketchEntity::Polyline(vec![DVec2::ZERO, DVec2::new(10.0, 0.0)]),
            SketchEntity::Polyline(vec![DVec2::new(0.0, 5.0), DVec2::new(10.0, 5.0)]),
        ];
        let mut draft = SketchDraft::new(SketchTool::DimAngle);
        draft.edit_click(DVec2::ZERO, Some(0), &parallel);
        draft.edit_click(DVec2::new(5.0, 5.0), Some(1), &parallel);
        assert!(matches!(
            draft.edit_click(DVec2::new(5.0, 2.0), None, &parallel),
            EditOutcome::Failed(_)
        ));
    }

    /// 高亮跟踪：编辑草稿按阶段返回已选图元下标（画布拾取反馈复用）。
    #[test]
    fn edit_highlight_tracks_pick_stage() {
        let entities = vec![
            SketchEntity::Polyline(vec![DVec2::ZERO, DVec2::new(10.0, 0.0)]),
            SketchEntity::Circle {
                center: DVec2::ZERO,
                radius: 5.0,
            },
        ];
        // Move：未拾取 → None；拾取后 → 下标。
        let mut draft = SketchDraft::new(SketchTool::Move);
        assert_eq!(draft.edit_highlight(), None);
        draft.edit_click(DVec2::ZERO, Some(0), &entities);
        assert_eq!(draft.edit_highlight(), Some(0));
        // Trim：边界必须是单段折线——拾取圆 → Ignored、不高亮。
        let mut draft = SketchDraft::new(SketchTool::Trim);
        assert_eq!(
            draft.edit_click(DVec2::ZERO, Some(1), &entities),
            EditOutcome::Ignored
        );
        assert_eq!(draft.edit_highlight(), None);
        // DimAngle：第一条线段拾取后高亮。
        let mut draft = SketchDraft::new(SketchTool::DimAngle);
        assert_eq!(
            draft.edit_click(DVec2::ZERO, Some(0), &entities),
            EditOutcome::Continued
        );
        assert_eq!(draft.edit_highlight(), Some(0));
        // 圆角草稿：高亮第一条线段（画布侧统一走 edit_highlight）。
        let mut draft = SketchDraft::new(SketchTool::Fillet);
        draft.pick_corner(Some(0));
        assert_eq!(draft.edit_highlight(), Some(0));
    }
}

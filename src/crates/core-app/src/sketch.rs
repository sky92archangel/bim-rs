//! 草图数据模型（G4-M15）：2D 图元与圆弧角度归一。
//!
//! 本模块是**纯数据 + 纯函数**层：[`SketchEntity`] 描述用户在草图编辑器里画的
//! 2D 图元（mm 单位、数学坐标 ccw 约定——y 向上、角度逆时针为正），画布层的
//! 屏幕坐标换算（y 向下）在 GUI 侧完成，此处只收数学坐标。
//!
//! 「图元 → 内核解析边 → wire」的序列化链路在 [`crate::geometry_service`]
//! （`build_sketch_wire`）；「造 wire + 建元素 + 落盘」的门面在
//! [`crate::session`]（`build_sketch_profile`）。

use std::f64::consts::TAU;

use glam::DVec2;
use serde::{Deserialize, Serialize};

/// 草图 2D 图元（mm；数学坐标：y 向上、角度 ccw 为正）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SketchEntity {
    /// 折线：≥ 2 个顶点，相邻顶点连直段；封闭与否由首末点是否重合决定，
    /// **不强制封闭**（封闭检查是特征链路（拉伸 / 旋转扫掠）的事，见 G4-M18）。
    Polyline(Vec<DVec2>),
    /// 整圆：圆心 + 半径。
    Circle {
        /// 圆心
        center: DVec2,
        /// 半径（mm，有限正数；非法输入由内核层响亮拒绝）
        radius: f64,
    },
    /// 整椭圆：中心 + 长、短半轴 + 主轴方向。
    ///
    /// `major_angle` 是主轴相对 +x 的转角（rad，ccw）。内核 `make_ellipse_edge`
    /// **无主轴方向参数**（主轴沿法向局部 +x），非零朝向由上层
    /// 「造边 + `rotate`」组合实现（见 `build_sketch_wire`）。
    Ellipse {
        /// 中心
        center: DVec2,
        /// 长半轴（mm，有限正数）
        major: f64,
        /// 短半轴（mm，有限正数，≤ major）
        minor: f64,
        /// 主轴方向（rad，ccw，相对 +x）
        major_angle: f64,
    },
    /// 圆弧：圆心 + 半径 + 起终角（rad，ccw）。
    ///
    /// 终角 < 起角时建边前先归一（`normalize_arc_angles`，终角 +2π）。
    Arc {
        /// 圆心
        center: DVec2,
        /// 半径（mm，有限正数）
        radius: f64,
        /// 起始角（rad，ccw，相对 +x）
        start_angle: f64,
        /// 终止角（rad，ccw，相对 +x）
        end_angle: f64,
    },
    /// 分段贝塞尔曲线（G4-M16）：每段是一个控制点序列（≥ 2 点，一条
    /// `make_bezier_edge` 直通——vendor 的贝塞尔边接受任意 ≥2 控制点的
    /// n 阶曲线；多段各产一条边）。
    ///
    /// - **贝塞尔多段线工具**（用户点控制点）：一条 n 阶曲线；
    /// - **样条拟合点工具**（用户点锚点）：Catmull-Rom → 多段三次
    ///   （每段 [起点, 控制1, 控制2, 终点]）。
    Bezier(
        /// 每段控制点序列（≥ 2 点，mm；相邻段共享端点与否由工具决定）
        Vec<Vec<DVec2>>,
    ),
    /// 辅助顶点标记（G4-M16）：**不进 wire、不进封闭检查**——只在画布可见。
    Point(
        /// 标记位置
        DVec2,
    ),
    /// 辅助定位线（G4-M16）：**不进 wire、不进封闭检查**，画布虚线显示。
    ///
    /// - `ray = false`：构造线——`from`/`to` 是无限延长直线上的两点；
    /// - `ray = true`：射线——`from` 是起点，`to` 是方向点。
    AuxLine {
        /// 构造线两点之一 / 射线起点
        from: DVec2,
        /// 构造线另一点 / 射线方向点
        to: DVec2,
        /// true = 射线（from 起点 → to 方向），false = 构造线（无限延长）
        ray: bool,
    },
    /// 显示型线性 / 对齐标注（G4-M17）：**不进 wire、不进封闭检查**。
    ///
    /// - `linear = true`：线性——测 `from`→`to` 的轴对齐增量（|dx| ≥ |dy| 取 dx
    ///   否则 dy），尺寸线过 `pos` 轴对齐；
    /// - `linear = false`：对齐——测真实距离，尺寸线平行 `from`→`to` 且过 `pos`。
    DimLinear {
        /// 测量起点
        from: DVec2,
        /// 测量终点
        to: DVec2,
        /// 标注放置位置（尺寸线经过）
        pos: DVec2,
        /// true = 线性（轴对齐增量），false = 对齐（真实距离）
        linear: bool,
    },
    /// 显示型半径标注（G4-M17）：**不进 wire**；`center`/`radius` 完成时从圆
    /// 图元捕获（此后独立于圆——圆被删/改标注不随动，P0 显示型口径）。
    DimRadius {
        /// 被标圆圆心
        center: DVec2,
        /// 被标圆半径
        radius: f64,
        /// 标注放置位置（引线终点 + 文字位）
        pos: DVec2,
    },
    /// 显示型角度标注（G4-M17）：**不进 wire**；`vertex` 为两线段交点，
    /// `arm1`/`arm2` 是两臂方向点（交点两侧保留端），角弧半径由 `pos` 定。
    DimAngle {
        /// 角顶点（两线段交点）
        vertex: DVec2,
        /// 第一臂方向点
        arm1: DVec2,
        /// 第二臂方向点
        arm2: DVec2,
        /// 标注放置位置（角弧经过 + 文字位）
        pos: DVec2,
    },
}

/// 圆弧角度归一：终角 < 起角 → 终角 + 2π（ccw 正向扫掠区间）。
///
/// 起终角相等时原样返回（0 扫角——退化弧交由内核裁决）；角度非有限数时
/// 不做修改（交由内核层 `finite` 校验响亮拒绝）。
#[must_use]
pub fn normalize_arc_angles(start: f64, end: f64) -> (f64, f64) {
    if end < start {
        (start, end + TAU)
    } else {
        (start, end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 终角 < 起角：+2π 归一到正向区间。
    #[test]
    fn end_before_start_normalizes_up() {
        let (s, e) = normalize_arc_angles(0.0, -std::f64::consts::FRAC_PI_2);
        assert_eq!(s, 0.0);
        assert!((e - (std::f64::consts::TAU - std::f64::consts::FRAC_PI_2)).abs() < 1e-12);
    }

    /// 正常区间（终角 > 起角）原样返回。
    #[test]
    fn forward_interval_is_untouched() {
        let (s, e) = normalize_arc_angles(-std::f64::consts::FRAC_PI_2, 0.0);
        assert_eq!(s, -std::f64::consts::FRAC_PI_2);
        assert_eq!(e, 0.0);
    }

    /// 起终角相等：原样返回（0 扫角，交由内核裁决）。
    #[test]
    fn equal_angles_are_untouched() {
        let (s, e) = normalize_arc_angles(1.25, 1.25);
        assert_eq!(s, 1.25);
        assert_eq!(e, 1.25);
    }

    /// 非有限角度：不修改，交由内核层响亮拒绝。
    #[test]
    fn non_finite_angles_are_untouched() {
        let (s, e) = normalize_arc_angles(f64::NEG_INFINITY, 0.0);
        assert_eq!(s, f64::NEG_INFINITY);
        assert_eq!(e, 0.0);
    }

    /// SketchEntity 可序列化往返（serde，DVec2 已带 serde）。
    #[test]
    fn entity_serde_roundtrip() {
        let entities = vec![
            SketchEntity::Polyline(vec![DVec2::new(0.0, 0.0), DVec2::new(50.0, 0.0)]),
            SketchEntity::Circle {
                center: DVec2::new(100.0, 100.0),
                radius: 25.0,
            },
            SketchEntity::Ellipse {
                center: DVec2::new(0.0, 0.0),
                major: 80.0,
                minor: 30.0,
                major_angle: 0.0,
            },
            SketchEntity::Arc {
                center: DVec2::new(50.0, 50.0),
                radius: 50.0,
                start_angle: -std::f64::consts::FRAC_PI_2,
                end_angle: 0.0,
            },
        ];
        let json = serde_json::to_string(&entities).expect("序列化成功");
        let back: Vec<SketchEntity> = serde_json::from_str(&json).expect("反序列化成功");
        assert_eq!(back, entities);
    }
}

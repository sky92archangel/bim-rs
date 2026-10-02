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

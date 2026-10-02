//! 草图纯几何算法库（G4-M14，apps/desktop 内，零新依赖）。
//!
//! 全部为纯函数：输入 [`glam::DVec2`] 与标量，输出计算结果或 `Option`，
//! 不依赖内核与 GUI 状态；坐标单位与内核一致（毫米）。
//! 方向约定：标准数学坐标（x 右、y 上），"逆时针（ccw）"指该坐标系下的正方向；
//! 草图画布层（y 向下）在 M15 接线时自行换算。
//!
//! bin crate 下 M15 草图编辑器接线前部分函数暂无调用方，以单测先行驱动，
//! 故整模块抑制 dead_code 告警（M15 接线后保留亦无害）。

#![allow(dead_code)]

use std::f64::consts::TAU;

use glam::DVec2;

/// 线段求交：参数法求 a1→a2 与 b1→b2 的交点，t/u 均落在 [0,1] 才算相交；
/// 平行/共线或交点在延长线上返回 `None`。
pub fn segment_intersection(a1: DVec2, a2: DVec2, b1: DVec2, b2: DVec2) -> Option<DVec2> {
    let d1 = a2 - a1;
    let d2 = b2 - b1;
    let denom = d1.perp_dot(d2);
    if denom.abs() < 1e-9 {
        return None; // 平行或共线
    }
    let t = (b1 - a1).perp_dot(d2) / denom;
    let u = (b1 - a1).perp_dot(d1) / denom;
    if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
        Some(a1 + d1 * t)
    } else {
        None
    }
}

/// 点到线段的最近点：投影参数 clamp 到 [0,1]；退化线段（a==b）返回 a。
pub fn closest_point_on_segment(p: DVec2, a: DVec2, b: DVec2) -> DVec2 {
    let ab = b - a;
    let len2 = ab.length_squared();
    if len2 < 1e-12 {
        return a;
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    a + ab * t
}

/// 点到线段的最短距离（毫米）。
pub fn point_segment_distance(p: DVec2, a: DVec2, b: DVec2) -> f64 {
    p.distance(closest_point_on_segment(p, a, b))
}

/// 三点外接圆：返回 (圆心, 半径)；三点共线或重合返回 `None`。
pub fn circle_through_three(a: DVec2, b: DVec2, c: DVec2) -> Option<(DVec2, f64)> {
    let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
    if d.abs() < 1e-9 {
        return None; // 三点共线
    }
    let a2 = a.length_squared();
    let b2 = b.length_squared();
    let c2 = c.length_squared();
    let ux = (a2 * (b.y - c.y) + b2 * (c.y - a.y) + c2 * (a.y - b.y)) / d;
    let uy = (a2 * (c.x - b.x) + b2 * (a.x - c.x) + c2 * (b.x - a.x)) / d;
    let center = DVec2::new(ux, uy);
    Some((center, center.distance(a)))
}

/// 圆弧中点：center 圆心，start/end 弧端点；ccw=true 沿逆时针扫掠取角平分点，
/// 否则顺时针。start/end 与 center 重合返回 `None`；start 与 end 同角视为整圆，取对径点。
pub fn arc_midpoint(center: DVec2, start: DVec2, end: DVec2, ccw: bool) -> Option<DVec2> {
    let v0 = start - center;
    let v1 = end - center;
    if v0.length_squared() < 1e-24 || v1.length_squared() < 1e-24 {
        return None;
    }
    let a0 = v0.y.atan2(v0.x);
    let a1 = v1.y.atan2(v1.x);
    let mut sweep = a1 - a0;
    if ccw {
        while sweep <= 0.0 {
            sweep += TAU;
        }
    } else {
        while sweep >= 0.0 {
            sweep -= TAU;
        }
    }
    let mid_angle = a0 + sweep * 0.5;
    Some(center + DVec2::new(mid_angle.cos(), mid_angle.sin()) * v0.length())
}

/// 点关于直线（过 line_a、line_b）的镜像；两点重合（直线退化）原样返回。
pub fn mirror_point(p: DVec2, line_a: DVec2, line_b: DVec2) -> DVec2 {
    let d = line_b - line_a;
    let len2 = d.length_squared();
    if len2 < 1e-24 {
        return p;
    }
    let unit = d / len2.sqrt();
    let v = p - line_a;
    line_a + (2.0 * v.dot(unit) * unit - v)
}

/// 点绕 pivot 旋转 angle_rad 弧度（数学坐标下逆时针为正）。
pub fn rotate_point(p: DVec2, pivot: DVec2, angle_rad: f64) -> DVec2 {
    let (sin, cos) = angle_rad.sin_cos();
    let v = p - pivot;
    pivot + DVec2::new(v.x * cos - v.y * sin, v.x * sin + v.y * cos)
}

/// 点绕 pivot 均匀缩放 factor 倍（factor=1 不动，0.5 向 pivot 收拢一半）。
pub fn scale_point(p: DVec2, pivot: DVec2, factor: f64) -> DVec2 {
    pivot + (p - pivot) * factor
}

/// 线段平行偏移：向 a→b 方向的左侧（数学坐标逆时针一侧）偏移 distance，<0 即向右；
/// a==b 返回 `None`。
pub fn offset_segment(a: DVec2, b: DVec2, distance: f64) -> Option<(DVec2, DVec2)> {
    let d = b - a;
    let len = d.length();
    if len < 1e-9 {
        return None;
    }
    let normal = DVec2::new(-d.y, d.x) / len; // 左法向
    Some((a + normal * distance, b + normal * distance))
}

/// 圆偏移：distance>0 向外放大、<0 向内收缩；半径收缩到 ≤0 返回 `None`。
pub fn offset_circle(center: DVec2, radius: f64, distance: f64) -> Option<(DVec2, f64)> {
    let r = radius + distance;
    (r > 1e-9).then_some((center, r))
}

/// 轮廓封闭性：首末点距离 ≤ tolerance 视为封闭；点数 < 2 不封闭。
pub fn is_closed_loop(points: &[DVec2], tolerance: f64) -> bool {
    if points.len() < 2 {
        return false;
    }
    points[0].distance(points[points.len() - 1]) <= tolerance
}

/// Catmull-Rom 拟合点转三次贝塞尔段：返回每段 [起点, 控制1, 控制2, 终点]。
/// 切线取相邻点差的一半（均匀 Catmull-Rom），首末端按重复点法（p[-1]=p[0]、p[n]=p[n-1]）；
/// 点数 < 2 返回空表。
pub fn catmull_rom_to_bezier(points: &[DVec2]) -> Vec<[DVec2; 4]> {
    if points.len() < 2 {
        return Vec::new();
    }
    let n = points.len();
    let tangent = |i: usize| -> DVec2 {
        let prev = if i == 0 { points[0] } else { points[i - 1] };
        let next = if i + 1 >= n {
            points[n - 1]
        } else {
            points[i + 1]
        };
        (next - prev) * 0.5
    };
    (0..n - 1)
        .map(|i| {
            let p0 = points[i];
            let p1 = points[i + 1];
            [p0, p0 + tangent(i) / 3.0, p1 - tangent(i + 1) / 3.0, p1]
        })
        .collect()
}

/// n 阶贝塞尔取值（de Casteljau 逐层线性插值折叠）：controls 为控制点序列，
/// t ∈ [0,1]。空序列返回原点、单点原样返回（调用方保证 ≥2 点，此处只为
/// 不 panic）。
#[must_use]
pub fn bezier_sample(controls: &[DVec2], t: f64) -> DVec2 {
    if controls.is_empty() {
        return DVec2::ZERO;
    }
    let mut pts: Vec<DVec2> = controls.to_vec();
    while pts.len() > 1 {
        let mut next = Vec::with_capacity(pts.len() - 1);
        for w in pts.windows(2) {
            next.push(w[0] + (w[1] - w[0]) * t);
        }
        pts = next;
    }
    pts[0]
}

/// n 阶贝塞尔折线采样：0..=segments 均匀参数采样（segments+1 个点）——
/// 画布无贝塞尔 primitive，渲染用折线近似；控制点 <2 原样返回。
#[must_use]
pub fn bezier_sample_polyline(controls: &[DVec2], segments: usize) -> Vec<DVec2> {
    if controls.len() < 2 {
        return controls.to_vec();
    }
    let n = segments.max(1);
    (0..=n)
        .map(|i| bezier_sample(controls, i as f64 / n as f64))
        .collect()
}

/// 倒圆角：顶点 v，两邻边为 a→v 与 v→b，圆角半径 radius。
/// 返回 (切点1, 切点2, 圆心)：切点1 在 v→a 上、切点2 在 v→b 上，距 v 均为 r/tan(φ/2)；
/// 圆心在角平分线上距 v r/sin(φ/2)，φ 为 v→a 与 v→b 的夹角。
/// 半径非正/邻边退化/两边共线（无角可倒）返回 `None`。
pub fn fillet_corner(a: DVec2, v: DVec2, b: DVec2, radius: f64) -> Option<(DVec2, DVec2, DVec2)> {
    if !radius.is_finite() || radius <= 0.0 {
        return None;
    }
    let to_a = a - v;
    let to_b = b - v;
    if to_a.length_squared() < 1e-24 || to_b.length_squared() < 1e-24 {
        return None;
    }
    let dir_a = to_a / to_a.length();
    let dir_b = to_b / to_b.length();
    if dir_a.perp_dot(dir_b).abs() < 1e-9 {
        return None; // 两边共线，无角可倒
    }
    let cos_theta = dir_a.dot(dir_b);
    let cos_half = ((1.0 + cos_theta) * 0.5).max(0.0).sqrt();
    let sin_half = ((1.0 - cos_theta) * 0.5).max(0.0).sqrt();
    if sin_half < 1e-12 {
        return None; // 夹角退化为 0（针尖形）
    }
    let offset = radius * cos_half / sin_half; // 切点距顶点 r/tan(φ/2)
    let center_dist = radius / sin_half; // 圆心距顶点 r/sin(φ/2)
    let bisector = (dir_a + dir_b).normalize_or_zero();
    Some((
        v + dir_a * offset,
        v + dir_b * offset,
        v + bisector * center_dist,
    ))
}

/// 倒直线角：顶点 v，两邻边为 a→v 与 v→b，两切点距顶点均为 distance。
/// distance 非正或任一邻边长度不足返回 `None`。
pub fn chamfer_corner(a: DVec2, v: DVec2, b: DVec2, distance: f64) -> Option<(DVec2, DVec2)> {
    if !distance.is_finite() || distance <= 0.0 {
        return None;
    }
    let to_a = a - v;
    let to_b = b - v;
    if to_a.length() < distance || to_b.length() < distance {
        return None; // 边长不足，切点会越过端点
    }
    Some((
        v + to_a.normalize_or_zero() * distance,
        v + to_b.normalize_or_zero() * distance,
    ))
}

/// 修剪线段：用裁剪边 (cutter1, cutter2) 切断 a→b，保留靠近 keep_near 一侧；
/// 无交点（裁剪边未命中）原样返回；a==b 返回 `None`。
pub fn trim_segment(
    a: DVec2,
    b: DVec2,
    cutter1: DVec2,
    cutter2: DVec2,
    keep_near: DVec2,
) -> Option<(DVec2, DVec2)> {
    let ab = b - a;
    let len2 = ab.length_squared();
    if len2 < 1e-24 {
        return None;
    }
    let Some(p) = segment_intersection(a, b, cutter1, cutter2) else {
        return Some((a, b)); // 无交点，无需修剪
    };
    let t_keep = ((keep_near - a).dot(ab) / len2).clamp(0.0, 1.0);
    let t_cut = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    if t_keep <= t_cut {
        Some((a, p)) // 保留 a 侧
    } else {
        Some((p, b))
    }
}

/// 在 at 处打断线段 a→b：打断点取 at 在线段上的投影；
/// 投影落在端点上或之外、或 a==b 返回 `None`（不打断）。
pub fn split_segment(a: DVec2, b: DVec2, at: DVec2) -> Option<([DVec2; 2], [DVec2; 2])> {
    let ab = b - a;
    let len2 = ab.length_squared();
    if len2 < 1e-24 {
        return None;
    }
    let t = ((at - a).dot(ab) / len2).clamp(0.0, 1.0);
    if t <= 0.0 || t >= 1.0 {
        return None; // 打断点落在线段端点或之外
    }
    let mid = a + ab * t;
    Some(([a, mid], [mid, b]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_1_SQRT_2, FRAC_PI_2};

    /// 浮点向量近似相等断言（带信息量的失败消息）。
    fn assert_near(actual: DVec2, expected: [f64; 2], eps: f64) {
        assert!(
            (actual.x - expected[0]).abs() <= eps && (actual.y - expected[1]).abs() <= eps,
            "期望 ({}, {})，实得 ({}, {})",
            expected[0],
            expected[1],
            actual.x,
            actual.y
        );
    }

    #[test]
    fn segments_cross_at_expected_point() {
        let p = segment_intersection(
            DVec2::new(-10.0, 0.0),
            DVec2::new(10.0, 0.0),
            DVec2::new(0.0, -10.0),
            DVec2::new(0.0, 10.0),
        );
        assert_near(p.expect("十字相交应得交点"), [0.0, 0.0], 1e-9);
    }

    #[test]
    fn segments_parallel_or_out_of_range_have_no_intersection() {
        // 平行：denom = 0
        assert!(
            segment_intersection(
                DVec2::ZERO,
                DVec2::new(10.0, 0.0),
                DVec2::new(0.0, 1.0),
                DVec2::new(10.0, 1.0)
            )
            .is_none()
        );
        // 延长线上相交（t=5 超出 [0,1]）
        assert!(
            segment_intersection(
                DVec2::ZERO,
                DVec2::new(1.0, 0.0),
                DVec2::new(5.0, -10.0),
                DVec2::new(5.0, -1.0)
            )
            .is_none()
        );
    }

    #[test]
    fn closest_point_clamps_to_segment() {
        // 中垂线上投影落段内
        assert_near(
            closest_point_on_segment(DVec2::new(5.0, 3.0), DVec2::ZERO, DVec2::new(10.0, 0.0)),
            [5.0, 0.0],
            1e-9,
        );
        // 端点外投影被 clamp 到 a
        assert_near(
            closest_point_on_segment(DVec2::new(-4.0, 2.0), DVec2::ZERO, DVec2::new(10.0, 0.0)),
            [0.0, 0.0],
            1e-9,
        );
    }

    #[test]
    fn point_segment_distance_matches_geometry() {
        // 垂足落段内：距离 = 高
        assert!(
            (point_segment_distance(DVec2::new(5.0, 3.0), DVec2::ZERO, DVec2::new(10.0, 0.0))
                - 3.0)
                .abs()
                < 1e-9
        );
        // 垂足落段外：距离 = 到端点
        assert!(
            (point_segment_distance(DVec2::new(15.0, 0.0), DVec2::ZERO, DVec2::new(10.0, 0.0))
                - 5.0)
                .abs()
                < 1e-9
        );
    }

    #[test]
    fn circumscribed_circle_of_three_points() {
        let (center, radius) = circle_through_three(
            DVec2::new(1.0, 0.0),
            DVec2::new(0.0, 1.0),
            DVec2::new(-1.0, 0.0),
        )
        .expect("不共线三点必有外接圆");
        assert_near(center, [0.0, 0.0], 1e-9);
        assert!((radius - 1.0).abs() < 1e-9);
    }

    #[test]
    fn collinear_points_have_no_circumcircle() {
        assert!(
            circle_through_three(DVec2::ZERO, DVec2::new(1.0, 1.0), DVec2::new(2.0, 2.0)).is_none()
        );
    }

    #[test]
    fn arc_midpoint_follows_sweep_direction() {
        let center = DVec2::ZERO;
        let start = DVec2::new(1.0, 0.0);
        let end = DVec2::new(0.0, 1.0);
        // 逆时针 90°：中点在 45° 方向
        let ccw = arc_midpoint(center, start, end, true).expect("有效圆弧");
        assert!((ccw.x - FRAC_1_SQRT_2).abs() < 1e-9);
        assert!((ccw.y - FRAC_1_SQRT_2).abs() < 1e-9);
        // 顺时针 270°：中点在 -135° 方向
        let cw = arc_midpoint(center, start, end, false).expect("有效圆弧");
        assert!((cw.x + FRAC_1_SQRT_2).abs() < 1e-9);
        assert!((cw.y + FRAC_1_SQRT_2).abs() < 1e-9);
    }

    #[test]
    fn arc_midpoint_rejects_degenerate_radius() {
        let c = DVec2::new(1.0, 1.0);
        // start 与 center 重合（半径为零）
        assert!(arc_midpoint(c, c, DVec2::new(2.0, 1.0), true).is_none());
    }

    #[test]
    fn mirror_point_across_axis_and_diagonal() {
        // 关于 y 轴镜像
        let p = mirror_point(DVec2::new(3.0, 4.0), DVec2::ZERO, DVec2::new(0.0, 5.0));
        assert_near(p, [-3.0, 4.0], 1e-9);
        // 关于 y=x 对角线镜像
        let q = mirror_point(DVec2::new(1.0, 3.0), DVec2::ZERO, DVec2::new(2.0, 2.0));
        assert_near(q, [3.0, 1.0], 1e-9);
    }

    #[test]
    fn rotate_point_quarter_turn() {
        // 绕原点转 90°
        let p = rotate_point(DVec2::new(1.0, 0.0), DVec2::ZERO, FRAC_PI_2);
        assert_near(p, [0.0, 1.0], 1e-9);
        // 绕非原点 pivot
        let q = rotate_point(DVec2::new(2.0, 1.0), DVec2::new(1.0, 1.0), FRAC_PI_2);
        assert_near(q, [1.0, 2.0], 1e-9);
    }

    #[test]
    fn scale_point_about_pivot() {
        // 绕 (10,10) 收拢一半
        let p = scale_point(DVec2::ZERO, DVec2::new(10.0, 10.0), 0.5);
        assert_near(p, [5.0, 5.0], 1e-9);
        // 绕原点放大 3 倍
        assert_near(
            scale_point(DVec2::new(2.0, 2.0), DVec2::ZERO, 3.0),
            [6.0, 6.0],
            1e-9,
        );
    }

    #[test]
    fn offset_segment_goes_left_of_direction() {
        let (a, b) = offset_segment(DVec2::ZERO, DVec2::new(10.0, 0.0), 5.0).expect("有效线段");
        assert_near(a, [0.0, 5.0], 1e-9);
        assert_near(b, [10.0, 5.0], 1e-9);
        // 退化线段
        assert!(offset_segment(DVec2::ZERO, DVec2::ZERO, 5.0).is_none());
    }

    #[test]
    fn offset_circle_grows_and_shrinks() {
        let (c, r) = offset_circle(DVec2::new(3.0, 4.0), 10.0, 2.0).expect("外扩有效");
        assert_near(c, [3.0, 4.0], 1e-9);
        assert!((r - 12.0).abs() < 1e-9);
        // 半径收缩为负
        assert!(offset_circle(DVec2::ZERO, 10.0, -15.0).is_none());
    }

    #[test]
    fn closed_loop_checks_first_last_distance() {
        let pts = [
            DVec2::ZERO,
            DVec2::new(10.0, 0.0),
            DVec2::new(10.0, 10.0),
            DVec2::ZERO,
        ];
        assert!(is_closed_loop(&pts, 1e-6));
        let open = [DVec2::ZERO, DVec2::new(10.0, 0.0)];
        assert!(!is_closed_loop(&open, 1e-6));
        // 单点不封闭
        let single = [DVec2::ZERO];
        assert!(!is_closed_loop(&single, 1e-6));
    }

    #[test]
    fn catmull_rom_keeps_straight_line_straight() {
        let pts = [DVec2::ZERO, DVec2::new(5.0, 0.0), DVec2::new(10.0, 0.0)];
        let segments = catmull_rom_to_bezier(&pts);
        assert_eq!(segments.len(), 2);
        for seg in &segments {
            for p in seg {
                assert!(p.y.abs() < 1e-9, "直线拟合点生成的贝塞尔控制点应共线");
            }
        }
        // 每段首尾与原始点对齐
        assert_near(segments[0][0], [0.0, 0.0], 1e-9);
        assert_near(segments[0][3], [5.0, 0.0], 1e-9);
        assert_near(segments[1][0], [5.0, 0.0], 1e-9);
        assert_near(segments[1][3], [10.0, 0.0], 1e-9);
    }

    #[test]
    fn catmull_rom_requires_two_points() {
        assert!(catmull_rom_to_bezier(&[DVec2::ZERO]).is_empty());
        assert!(catmull_rom_to_bezier(&[]).is_empty());
    }

    #[test]
    fn bezier_sample_quadratic_midpoint() {
        // 二次贝塞尔 t=0.5：(P0 + 2·P1 + P2) / 4
        let b = bezier_sample(
            &[DVec2::ZERO, DVec2::new(50.0, 100.0), DVec2::new(100.0, 0.0)],
            0.5,
        );
        assert_near(b, [50.0, 50.0], 1e-9);
    }

    #[test]
    fn bezier_sample_endpoints_match_controls() {
        let controls = [
            DVec2::new(1.0, 2.0),
            DVec2::new(3.0, 8.0),
            DVec2::new(9.0, 4.0),
            DVec2::new(15.0, 20.0),
        ];
        // t=0 / t=1 落在首末控制点。
        assert_near(bezier_sample(&controls, 0.0), [1.0, 2.0], 1e-9);
        assert_near(bezier_sample(&controls, 1.0), [15.0, 20.0], 1e-9);
        // 线性（2 控制点）：中点 = 平均。
        assert_near(bezier_sample(&controls[..2], 0.5), [2.0, 5.0], 1e-9);
    }

    #[test]
    fn bezier_sample_polyline_count_and_endpoints() {
        let controls = [DVec2::ZERO, DVec2::new(10.0, 0.0)];
        let pts = bezier_sample_polyline(&controls, 8);
        assert_eq!(pts.len(), 9);
        assert_near(pts[0], [0.0, 0.0], 1e-9);
        assert_near(pts[8], [10.0, 0.0], 1e-9);
        // 均匀参数下线性曲线采样等距：t=2/8 → (2.5, 0)。
        assert_near(pts[2], [2.5, 0.0], 1e-9);
        // 控制点不足 2：原样返回。
        assert_eq!(bezier_sample_polyline(&controls[..1], 8).len(), 1);
    }

    #[test]
    fn fillet_right_angle_has_expected_geometry() {
        // 直角：a=(-100,0) → v=(0,0) → b=(0,100)，r=10
        let (t1, t2, center) = fillet_corner(
            DVec2::new(-100.0, 0.0),
            DVec2::ZERO,
            DVec2::new(0.0, 100.0),
            10.0,
        )
        .expect("直角可倒圆角");
        assert_near(t1, [-10.0, 0.0], 1e-9);
        assert_near(t2, [0.0, 10.0], 1e-9);
        assert_near(center, [-10.0, 10.0], 1e-9);
    }

    #[test]
    fn fillet_rejects_collinear_and_bad_radius() {
        // 两边共线（180° 直线），无角可倒
        assert!(
            fillet_corner(
                DVec2::new(-10.0, 0.0),
                DVec2::ZERO,
                DVec2::new(10.0, 0.0),
                5.0
            )
            .is_none()
        );
        // 半径非正
        assert!(
            fillet_corner(
                DVec2::new(-10.0, 0.0),
                DVec2::ZERO,
                DVec2::new(0.0, 10.0),
                0.0
            )
            .is_none()
        );
    }

    #[test]
    fn chamfer_cuts_at_fixed_distance() {
        let (t1, t2) = chamfer_corner(
            DVec2::new(-100.0, 0.0),
            DVec2::ZERO,
            DVec2::new(0.0, 100.0),
            20.0,
        )
        .expect("边长充足");
        assert_near(t1, [-20.0, 0.0], 1e-9);
        assert_near(t2, [0.0, 20.0], 1e-9);
        // 边长不足
        assert!(
            chamfer_corner(
                DVec2::new(1.0, 0.0),
                DVec2::ZERO,
                DVec2::new(0.0, 100.0),
                20.0
            )
            .is_none()
        );
    }

    #[test]
    fn trim_keeps_side_near_anchor() {
        // 保留 a 侧
        let (a, b) = trim_segment(
            DVec2::ZERO,
            DVec2::new(10.0, 0.0),
            DVec2::new(5.0, -5.0),
            DVec2::new(5.0, 5.0),
            DVec2::new(0.0, 1.0),
        )
        .expect("裁剪边命中应可修剪");
        assert_near(a, [0.0, 0.0], 1e-9);
        assert_near(b, [5.0, 0.0], 1e-9);
        // 保留 b 侧
        let (a, b) = trim_segment(
            DVec2::ZERO,
            DVec2::new(10.0, 0.0),
            DVec2::new(5.0, -5.0),
            DVec2::new(5.0, 5.0),
            DVec2::new(10.0, 1.0),
        )
        .expect("裁剪边命中应可修剪");
        assert_near(a, [5.0, 0.0], 1e-9);
        assert_near(b, [10.0, 0.0], 1e-9);
    }

    #[test]
    fn trim_without_hit_returns_original() {
        // 裁剪边与目标线段平行，无交点 → 原样返回
        let (a, b) = trim_segment(
            DVec2::ZERO,
            DVec2::new(10.0, 0.0),
            DVec2::new(5.0, 5.0),
            DVec2::new(15.0, 5.0),
            DVec2::ZERO,
        )
        .expect("无交点应原样返回");
        assert_near(a, [0.0, 0.0], 1e-9);
        assert_near(b, [10.0, 0.0], 1e-9);
    }

    #[test]
    fn split_breaks_segment_at_projection() {
        let (first, second) =
            split_segment(DVec2::ZERO, DVec2::new(10.0, 0.0), DVec2::new(3.0, 2.0))
                .expect("打断点投影在段内");
        assert_near(first[0], [0.0, 0.0], 1e-9);
        assert_near(first[1], [3.0, 0.0], 1e-9);
        assert_near(second[0], [3.0, 0.0], 1e-9);
        assert_near(second[1], [10.0, 0.0], 1e-9);
        // 投影落在线段外（clamp 后 t=1）不打断
        assert!(split_segment(DVec2::ZERO, DVec2::new(10.0, 0.0), DVec2::new(15.0, 0.0)).is_none());
    }
}

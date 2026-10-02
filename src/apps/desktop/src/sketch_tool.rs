//! 草图工具纯逻辑（G4-M15）：工具状态机 + 捕捉 / 正交纯函数。
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

use glam::DVec2;

use core_app::SketchEntity;

/// 草图画图工具（骨架五种；M16 再补正多边形 / 三点弧 / 样条等）。
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
}

impl SketchTool {
    /// 工具全集（编辑器工具栏的枚举序）。
    pub const ALL: [SketchTool; 5] = [
        Self::Polyline,
        Self::Rect,
        Self::Circle,
        Self::Ellipse,
        Self::Arc,
    ];

    /// 工具名（按钮标签）。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Polyline => "折线",
            Self::Rect => "矩形",
            Self::Circle => "圆",
            Self::Ellipse => "椭圆",
            Self::Arc => "圆弧",
        }
    }
}

/// 进行中的图元（工具交互状态机）。
///
/// 交互约定（骨架口径）：
/// - 矩形两击 → [`SketchEntity::Polyline`]（5 点闭合）；
/// - 圆两击（圆心 → 半径）；
/// - 椭圆三击（中心 → 长轴端点定 major/angle → 短轴点取到主轴线**垂距**）；
/// - 圆弧三击（圆心 → 起点定半径与起角 → 终点，ccw 归一 +2π）；
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
    /// 新建一个工具的初始草稿。
    #[must_use]
    pub fn new(tool: SketchTool) -> Self {
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
        }
    }

    /// 状态栏步骤提示（按当前交互阶段，骨架口径全量文案）。
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
            Self::None => ClickOutcome::Continued, // 无草稿：点击交给工具选择
        }
    }

    /// 折线收尾（回车 / 双击）：≥ 2 点时返回图元并复位。
    pub fn finish_polyline(&mut self) -> Option<SketchEntity> {
        match self {
            Self::Polyline { points } if points.len() >= 2 => {
                let entity = SketchEntity::Polyline(points.clone());
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

    /// 折线 Backspace：撤掉最后一个点。
    pub fn backtrack(&mut self) -> bool {
        match self {
            Self::Polyline { points } => points.pop().is_some(),
            _ => false,
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(one.finish_polyline().is_none(), "单点折线不应完成");
        // 3 点收尾。
        match d.finish_polyline() {
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
        assert_eq!(SketchTool::ALL.len(), 5);
        assert!(!SketchTool::ALL.is_empty());
        for tool in SketchTool::ALL {
            assert!(!tool.label().is_empty());
            assert!(!SketchDraft::new(tool).hint().is_empty());
        }
    }
}

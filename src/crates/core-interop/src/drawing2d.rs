//! **中性二维交换结构** —— 让 `core-doc` 与格式序列化器解耦。
//!
//! ## 为什么需要它
//!
//! 导出 DXF / SVG 需要"一堆二维线 + 文字"。这些数据在 `core-doc` 里
//! （`View` / `Entity2D` / `Annotation`），但 `core-interop` **不能依赖 `core-doc`**
//! （设计文档 2.2 依赖方向表）。
//!
//! 解法：`core-interop` 只认这套**中性结构**，谁都能来喂它——
//! `core-app` 负责把 `core_doc::Drawing` 转成它。
//!
//! ```text
//! core_doc::Drawing ──(core-app 转换)──▶ Drawing2D ──▶ SvgExporter / DxfExporter
//! ```
//!
//! ## 为什么样式是"已解析"的
//!
//! [`Curve2D`] 里带的是**具体的**图层名、线型名、线宽、颜色，而不是
//! `LayerId` 引用。原因：导出时制图标准（`DraftingStandard`）必须已经**落地**
//! 生效——"ByLayer 跟随图层"这种间接在出口处必须一次性解开，
//! 否则 DXF 里会带着一堆悬空图层引用。

use serde::{Deserialize, Serialize};

/// 解析二维图元（§7.4：圆、弧必须是解析实体）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Analytic2D {
    /// 圆（圆心 + 半径）
    Circle { center: [f64; 2], radius: f64 },
    /// 弧（弧度——与全局约定一致）
    Arc {
        center: [f64; 2],
        radius: f64,
        start_angle_rad: f64,
        end_angle_rad: f64,
    },
    /// 椭圆（中心 + 长短半轴 + 主轴方位角，弧度）
    Ellipse {
        center: [f64; 2],
        /// 长半轴（`rx >= ry` 由生产方保证）
        rx: f64,
        /// 短半轴
        ry: f64,
        /// 主轴方位角（弧度）
        rotation_rad: f64,
    },
}

/// 一条待导出的二维曲线。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Curve2D {
    /// 点列（图纸坐标，**单位 mm，Y 轴朝上**）
    pub points: Vec<[f64; 2]>,
    /// 是否闭合（填充 / 圆 / 闭合多段线）
    pub closed: bool,
    /// 图层名（已解析的具体名，如 `"OUTLINE-可见轮廓"`）
    pub layer: String,
    /// 线型名（已解析，如 `"Continuous"` / `"Dashed"`）
    pub line_type: String,
    /// 线宽（mm，图纸上的实际宽度；`0.0` 表示按"最细线"）
    pub width_mm: f64,
    /// 颜色（已解析，RGB）
    pub color: [u8; 3],
    /// 解析图元（圆/弧；`None` 表示折线；反序列化兼容旧 JSON）
    #[serde(default)]
    pub analytic: Option<Analytic2D>,
}

impl Curve2D {
    /// 一条折线。
    #[must_use]
    pub fn new(points: Vec<[f64; 2]>, layer: impl Into<String>) -> Self {
        Self {
            points,
            closed: false,
            layer: layer.into(),
            line_type: "Continuous".to_string(),
            width_mm: 0.0,
            color: [0, 0, 0],
            analytic: None,
        }
    }

    /// 闭合（链式）。
    #[must_use]
    pub fn closed(mut self) -> Self {
        self.closed = true;
        self
    }

    /// 指定线型（链式）。
    #[must_use]
    pub fn with_line_type(mut self, line_type: impl Into<String>) -> Self {
        self.line_type = line_type.into();
        self
    }

    /// 指定线宽与颜色（链式）。
    #[must_use]
    pub fn with_style(mut self, width_mm: f64, color: [u8; 3]) -> Self {
        self.width_mm = width_mm;
        self.color = color;
        self
    }

    /// 段数（点数 - 1，闭合时等于点数）。
    #[must_use]
    pub fn segment_count(&self) -> usize {
        if self.points.len() < 2 {
            return 0;
        }
        if self.closed {
            self.points.len()
        } else {
            self.points.len() - 1
        }
    }

    /// 若为解析圆，返回 `(center, radius)`。
    #[must_use]
    pub fn as_circle(&self) -> Option<(&[f64; 2], f64)> {
        self.analytic.as_ref().and_then(|a| match a {
            Analytic2D::Circle { center, radius } => Some((center, *radius)),
            _ => None,
        })
    }

    /// 若为解析弧，返回 `(center, radius, start_angle_rad, end_angle_rad)`。
    #[must_use]
    pub fn as_arc(&self) -> Option<(&[f64; 2], f64, f64, f64)> {
        self.analytic.as_ref().and_then(|a| match a {
            Analytic2D::Arc {
                center,
                radius,
                start_angle_rad,
                end_angle_rad,
            } => Some((center, *radius, *start_angle_rad, *end_angle_rad)),
            _ => None,
        })
    }

    /// 若为解析椭圆，返回 `(center, rx, ry, rotation_rad)`。
    #[must_use]
    pub fn as_ellipse(&self) -> Option<(&[f64; 2], f64, f64, f64)> {
        self.analytic.as_ref().and_then(|a| match a {
            Analytic2D::Ellipse {
                center,
                rx,
                ry,
                rotation_rad,
            } => Some((center, *rx, *ry, *rotation_rad)),
            _ => None,
        })
    }
}

/// 一段待导出的文字。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Text2D {
    /// 插入点（图纸坐标，mm，Y 轴朝上）
    pub at: [f64; 2],
    /// 内容
    pub content: String,
    /// 字高（mm）
    pub height_mm: f64,
    /// 旋转角（度，逆时针）
    pub rotation_deg: f64,
    /// 图层名
    pub layer: String,
    /// 颜色
    pub color: [u8; 3],
}

impl Text2D {
    /// 构造一段文字（不旋转）。
    #[must_use]
    pub fn new(at: [f64; 2], content: impl Into<String>, height_mm: f64) -> Self {
        Self {
            at,
            content: content.into(),
            height_mm,
            rotation_deg: 0.0,
            layer: crate::drawing2d::DEFAULT_TEXT_LAYER.to_string(),
            color: [0, 0, 0],
        }
    }

    /// 指定图层（链式）。
    #[must_use]
    pub fn on_layer(mut self, layer: impl Into<String>) -> Self {
        self.layer = layer.into();
        self
    }

    /// 指定旋转角（链式）。
    #[must_use]
    pub fn rotated(mut self, degrees: f64) -> Self {
        self.rotation_deg = degrees;
        self
    }
}

/// 默认文字图层名（与 `core_doc::standard::LAYER_TEXT` 同值）。
pub const DEFAULT_TEXT_LAYER: &str = "TEXT-文字";

/// 一幅**已解析**的二维图纸，可直接喂给格式序列化器。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Drawing2D {
    /// 图幅宽（mm）
    pub width_mm: f64,
    /// 图幅高（mm）
    pub height_mm: f64,
    /// 曲线
    pub curves: Vec<Curve2D>,
    /// 文字
    pub texts: Vec<Text2D>,
}

impl Drawing2D {
    /// 空图纸。
    #[must_use]
    pub fn new(width_mm: f64, height_mm: f64) -> Self {
        Self {
            width_mm,
            height_mm,
            curves: Vec::new(),
            texts: Vec::new(),
        }
    }

    /// 加一条曲线。
    pub fn push_curve(&mut self, curve: Curve2D) {
        self.curves.push(curve);
    }

    /// 加一段文字。
    pub fn push_text(&mut self, text: Text2D) {
        self.texts.push(text);
    }

    /// 是否有内容。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.curves.is_empty() && self.texts.is_empty()
    }

    /// 曲线 + 文字总数。
    #[must_use]
    pub fn entity_count(&self) -> usize {
        self.curves.len() + self.texts.len()
    }

    /// 用到的全部图层名（去重、排序）。
    #[must_use]
    pub fn layer_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .curves
            .iter()
            .map(|c| c.layer.clone())
            .chain(self.texts.iter().map(|t| t.layer.clone()))
            .collect();
        names.sort();
        names.dedup();
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_count_accounts_for_closing() {
        let open = Curve2D::new(vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]], "L");
        assert_eq!(open.segment_count(), 2);

        let closed = Curve2D::new(vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]], "L").closed();
        assert_eq!(closed.segment_count(), 3, "闭合后要补最后一段");
    }

    #[test]
    fn single_point_curve_has_no_segments() {
        let c = Curve2D::new(vec![[0.0, 0.0]], "L");
        assert_eq!(c.segment_count(), 0);
    }

    #[test]
    fn layer_names_are_deduplicated() {
        let mut d = Drawing2D::new(420.0, 297.0);
        d.push_curve(Curve2D::new(vec![[0.0, 0.0], [1.0, 0.0]], "OUTLINE"));
        d.push_curve(Curve2D::new(vec![[0.0, 0.0], [0.0, 1.0]], "OUTLINE"));
        d.push_text(Text2D::new([0.0, 0.0], "一层平面图", 7.0).on_layer("TEXT"));
        assert_eq!(
            d.layer_names(),
            vec!["OUTLINE".to_string(), "TEXT".to_string()]
        );
    }

    #[test]
    fn empty_drawing_is_reported() {
        let d = Drawing2D::new(297.0, 210.0);
        assert!(d.is_empty());
        assert_eq!(d.entity_count(), 0);
    }

    #[test]
    fn drawing_roundtrips_through_json() {
        let mut d = Drawing2D::new(420.0, 297.0);
        d.push_curve(
            Curve2D::new(vec![[0.0, 0.0], [10.0, 0.0]], "OUTLINE").with_style(0.5, [0, 0, 0]),
        );
        d.push_text(Text2D::new([5.0, 5.0], "600", 3.5));
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(serde_json::from_str::<Drawing2D>(&json).unwrap(), d);
    }

    #[test]
    fn as_ellipse_reads_center_and_semi_axes() {
        let mut c = Curve2D::new(vec![[1.0, 2.0]], "L");
        c.analytic = Some(Analytic2D::Ellipse {
            center: [1.0, 2.0],
            rx: 5.0,
            ry: 3.0,
            rotation_rad: 0.5,
        });
        let (center, rx, ry, rot) = c.as_ellipse().expect("应识别为解析椭圆");
        assert_eq!(*center, [1.0, 2.0]);
        assert_eq!(rx, 5.0);
        assert_eq!(ry, 3.0);
        assert_eq!(rot, 0.5);

        // 圆不得被误认为椭圆
        let mut cir = Curve2D::new(vec![[0.0, 0.0]], "L");
        cir.analytic = Some(Analytic2D::Circle {
            center: [0.0, 0.0],
            radius: 1.0,
        });
        assert!(cir.as_ellipse().is_none(), "圆不得被 as_ellipse 误认");
    }
}

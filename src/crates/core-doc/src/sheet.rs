//! 图纸与视图布置（设计文档 5.2）。
//!
//! **视图 ≠ 图纸**：一张视图是"一个投影结果"，一张图纸是"把若干视图按比例
//! 摆到一张图幅上，再套上标题栏"。所以 [`Sheet`] 里存的是 [`Placement`]
//! （视图 id + 位置 + 比例），**不是视图本体**——同一张视图可以摆到多张图纸上，
//! 甚至在同一张图纸上摆两次（如详图索引）。

use core_model::{SheetId, ViewId};
use glam::DVec2;
use serde::{Deserialize, Serialize};

use crate::error::{DocError, DocResult};
use crate::standard::{SheetFormat, TitleBlockDef};

/// 一个视图在图纸上的布置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    /// 被布置的视图
    pub view: ViewId,
    /// 视图中心在图纸上的位置（mm，图纸坐标系原点在左下角）
    pub at: DVec2,
    /// 该视图在这张图纸上的比例分母（可与视图自身比例不同）
    pub scale_denominator: u32,
}

impl Placement {
    /// 构造一个布置（默认 1:100）。
    #[must_use]
    pub fn new(view: ViewId, at: DVec2) -> Self {
        Self {
            view,
            at,
            scale_denominator: 100,
        }
    }

    /// 指定比例（链式）。
    #[must_use]
    pub fn with_scale(mut self, denominator: u32) -> Self {
        self.scale_denominator = denominator;
        self
    }

    /// 模型尺寸 → 图纸尺寸的缩放系数（如 1:100 → 0.01）。
    ///
    /// ⚠ 分母为 0 视为无效，返回 `1.0`（调用方应先用
    /// [`Sheet::validate`] 拦掉）。
    #[must_use]
    pub fn drawing_scale(&self) -> f64 {
        if self.scale_denominator == 0 {
            1.0
        } else {
            1.0 / f64::from(self.scale_denominator)
        }
    }
}

/// 一张图纸。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sheet {
    /// 持久 id（同时登记在 `Document::sheets`）
    pub id: SheetId,
    /// 图纸名
    pub name: String,
    /// 图号（如 `"建施-01"`）
    pub number: String,
    /// 图幅
    pub format: SheetFormat,
    /// 标题栏（**规则**，出图时由绘制层画出来）
    pub title_block: TitleBlockDef,
    /// 视图布置表
    pub placements: Vec<Placement>,
    /// 当前修订代号（如 `"A"`；空串表示未修订）
    pub revision: String,
}

impl Sheet {
    /// 构造一张空图纸。
    #[must_use]
    pub fn new(name: impl Into<String>, number: impl Into<String>, format: SheetFormat) -> Self {
        Self {
            id: SheetId::new(),
            name: name.into(),
            number: number.into(),
            format,
            title_block: TitleBlockDef::default(),
            placements: Vec::new(),
            revision: String::new(),
        }
    }

    /// 尺寸（宽 mm, 高 mm）。
    #[must_use]
    pub fn size_mm(&self) -> (f64, f64) {
        self.format.size_mm()
    }

    /// 加一个视图布置。
    pub fn place(&mut self, placement: Placement) {
        self.placements.push(placement);
    }

    /// 是否有尚未布置任何视图。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.placements.is_empty()
    }

    /// 布置数量。
    #[must_use]
    pub fn placement_count(&self) -> usize {
        self.placements.len()
    }

    /// 该图纸引用的全部视图 id。
    #[must_use]
    pub fn view_ids(&self) -> Vec<ViewId> {
        self.placements.iter().map(|p| p.view).collect()
    }

    /// 校验图纸自身的一致性。
    ///
    /// 拦两类问题：① 比例分母为 0；② 视图位置超出图幅可布置区域
    /// （已扣除右下角标题栏）。
    pub fn validate(&self) -> DocResult<()> {
        let (w, h) = self.size_mm();
        for p in &self.placements {
            if p.scale_denominator == 0 {
                return Err(DocError::Standard(format!(
                    "图纸 {} 的比例分母为 0",
                    self.number
                )));
            }
            if p.at.x < 0.0 || p.at.y < 0.0 || p.at.x > w || p.at.y > h {
                return Err(DocError::Standard(format!(
                    "图纸 {} 上视图布置越界：({:.1}, {:.1}) 超出 {:.0}×{:.0}",
                    self.number, p.at.x, p.at.y, w, h
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_converts_scale_to_factor() {
        let p = Placement::new(ViewId::new(), DVec2::new(100.0, 100.0)).with_scale(100);
        assert!((p.drawing_scale() - 0.01).abs() < 1e-12);
        let p50 = Placement::new(ViewId::new(), DVec2::ZERO).with_scale(50);
        assert!((p50.drawing_scale() - 0.02).abs() < 1e-12);
    }

    #[test]
    fn zero_denominator_is_rejected_by_validate() {
        let mut s = Sheet::new("一层平面", "建施-01", SheetFormat::A2);
        s.place(Placement::new(ViewId::new(), DVec2::new(200.0, 200.0)).with_scale(0));
        assert!(matches!(s.validate(), Err(DocError::Standard(_))));
    }

    #[test]
    fn out_of_bounds_placement_is_rejected() {
        let mut s = Sheet::new("一层平面", "建施-01", SheetFormat::A4);
        // A4 横放 297×210，放 (1000, 1000) 明显越界
        s.place(Placement::new(ViewId::new(), DVec2::new(1000.0, 1000.0)));
        assert!(matches!(s.validate(), Err(DocError::Standard(_))));
    }

    #[test]
    fn valid_sheet_passes_validation() {
        let mut s = Sheet::new("一层平面", "建施-01", SheetFormat::A2);
        s.place(Placement::new(ViewId::new(), DVec2::new(250.0, 300.0)).with_scale(100));
        s.place(Placement::new(ViewId::new(), DVec2::new(400.0, 300.0)).with_scale(100));
        assert!(s.validate().is_ok());
        assert_eq!(s.placement_count(), 2);
        assert_eq!(s.view_ids().len(), 2);
        assert!(!s.is_empty());
    }

    #[test]
    fn same_view_can_be_placed_twice() {
        let view = ViewId::new();
        let mut s = Sheet::new("标准图", "G-01", SheetFormat::A1);
        s.place(Placement::new(view, DVec2::new(200.0, 400.0)));
        s.place(Placement::new(view, DVec2::new(600.0, 400.0)).with_scale(50));
        assert_eq!(s.placement_count(), 2, "同一视图可以摆多处（如详图索引）");
    }

    #[test]
    fn sheet_roundtrips_through_json() {
        let mut s = Sheet::new("一层平面", "建施-01", SheetFormat::A2);
        s.place(Placement::new(ViewId::new(), DVec2::new(250.0, 300.0)));
        s.revision = "A".to_string();
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Sheet>(&json).unwrap(), s);
    }
}

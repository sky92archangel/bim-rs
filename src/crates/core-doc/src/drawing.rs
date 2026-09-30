//! 图纸库（设计文档 5.2）—— 与 `core-model::Document` **配对**的那一半。
//!
//! ## 为什么拆成两半
//!
//! `Document` 是**模型**：构件、几何、基准元素、关系。
//! [`Drawing`] 是**图纸**：视图、图纸、制图标准、关联更新索引。
//!
//! 分家的理由（[`crate::entity`] 也说过）：一个构件可以在三张视图里各有一条投影线，
//! 这三条线是**图纸的对象**不是**模型的对象**。混在一起会让"删构件"顺手删掉图纸内容。
//!
//! `Document` 里只留 `views: BTreeSet<ViewId>` / `sheets: BTreeSet<SheetId>` 两行
//! **引用**；本体在这里。二者由 `core-app` 的用例同时持有并保持一致
//! （[`Drawing::sync_registry`] 就是干这个的）。

use std::collections::{BTreeMap, BTreeSet};

use core_model::Document;
use serde::{Deserialize, Serialize};

use crate::dependency::DependencyGraph;
use crate::error::{DocError, DocResult};
use crate::sheet::Sheet;
use crate::standard::DraftingStandard;
use crate::view::View;
use core_model::{ShapeId, SheetId, ViewId};

/// 一个文档的图纸库。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Drawing {
    /// 制图标准（规则引擎）
    pub standard: DraftingStandard,
    /// 视图表
    pub views: BTreeMap<ViewId, View>,
    /// 图纸表
    pub sheets: BTreeMap<SheetId, Sheet>,
    /// 关联更新索引
    pub dependencies: DependencyGraph,
}

impl Drawing {
    /// 用一套制图标准新建空图纸库。
    #[must_use]
    pub fn with_standard(standard: DraftingStandard) -> Self {
        Self {
            standard,
            views: BTreeMap::new(),
            sheets: BTreeMap::new(),
            dependencies: DependencyGraph::new(),
        }
    }

    /// 出厂默认（国标工业厂房标准）。
    #[must_use]
    pub fn new() -> Self {
        Self::with_standard(DraftingStandard::gb_industrial())
    }

    /// 加入一个视图。
    pub fn add_view(&mut self, view: View) -> ViewId {
        let id = view.id;
        self.views.insert(id, view);
        id
    }

    /// 加入一张图纸。
    pub fn add_sheet(&mut self, sheet: Sheet) -> SheetId {
        let id = sheet.id;
        self.sheets.insert(id, sheet);
        id
    }

    /// 取视图。
    #[must_use]
    pub fn view(&self, id: ViewId) -> Option<&View> {
        self.views.get(&id)
    }

    /// 改视图。
    pub fn view_mut(&mut self, id: ViewId) -> DocResult<&mut View> {
        self.views.get_mut(&id).ok_or(DocError::ViewNotFound(id))
    }

    /// 取图纸。
    #[must_use]
    pub fn sheet(&self, id: SheetId) -> Option<&Sheet> {
        self.sheets.get(&id)
    }

    /// 删视图，并**清掉所有引用它的布置与绑定**（防悬空）。
    ///
    /// 返回被删的视图与被连带移除的布置数量。
    pub fn remove_view(&mut self, id: ViewId) -> DocResult<(View, usize)> {
        let view = self.views.remove(&id).ok_or(DocError::ViewNotFound(id))?;

        for entity in &view.entities {
            self.dependencies.remove_entity(entity.id);
        }
        for ann in &view.annotations {
            self.dependencies.remove_entity(ann.id);
        }

        let mut dropped = 0;
        for sheet in self.sheets.values_mut() {
            let before = sheet.placements.len();
            sheet.placements.retain(|p| p.view != id);
            dropped += before - sheet.placements.len();
        }
        Ok((view, dropped))
    }

    /// 视图数量。
    #[must_use]
    pub fn view_count(&self) -> usize {
        self.views.len()
    }

    /// 图纸数量。
    #[must_use]
    pub fn sheet_count(&self) -> usize {
        self.sheets.len()
    }

    /// 重建关联更新索引（所有视图扫一遍）。
    pub fn rebuild_index(&mut self) {
        let views: Vec<View> = self.views.values().cloned().collect();
        self.dependencies.rebuild_from_views(&views);
    }

    /// 把 `Document` 里的视图 / 图纸 id 登记与本库对齐。
    ///
    /// `Document` 只留引用（见 crate 文档），这里把两边**保持同步**：
    /// 库里多出来的登记进文档，文档里多出来的从库里删掉。
    /// 返回 `(新增登记数, 清理数)`。
    pub fn sync_registry(&self, doc: &mut Document) -> (usize, usize) {
        let mut added = 0;
        let mut removed = 0;

        for id in self.views.keys() {
            if doc.register_view(*id) {
                added += 1;
            }
        }
        for id in self.sheets.keys() {
            if doc.register_sheet(*id) {
                added += 1;
            }
        }

        let stale_views: Vec<ViewId> = doc
            .views()
            .iter()
            .filter(|id| !self.views.contains_key(id))
            .copied()
            .collect();
        for id in stale_views {
            doc.unregister_view(id);
            removed += 1;
        }
        let stale_sheets: Vec<SheetId> = doc
            .sheets()
            .iter()
            .filter(|id| !self.sheets.contains_key(id))
            .copied()
            .collect();
        for id in stale_sheets {
            doc.unregister_sheet(id);
            removed += 1;
        }

        (added, removed)
    }

    /// 哪些视图的图元 / 标注**绑定了给定形状**。
    ///
    /// **只看 [`core_model::GeometryKey::shape`]，不看内部子形状键**（规划 §2.3(e)）：
    /// 关联更新以 `ShapeId` 为单位圈定影响范围，面 / 边 / 顶点级的细化由后续查询完成。
    #[must_use]
    pub fn views_bound_to_shapes(&self, shapes: &[ShapeId]) -> BTreeSet<ViewId> {
        let mut out = BTreeSet::new();
        for (id, view) in &self.views {
            let touches = view
                .entities
                .iter()
                .filter_map(|e| e.source.as_ref())
                .chain(view.annotations.iter().filter_map(|a| a.bound_to.as_ref()))
                .any(|key| shapes.contains(&key.shape));
            if touches {
                out.insert(*id);
            }
        }
        out
    }

    /// 把这些视图标记为"需重新生成"（[`View::up_to_date`] 置 `false`）。
    ///
    /// 返回**实际被标脏的视图数**（集合里存在于本库的视图）；集合里不在库中的
    /// id 被忽略，不计入。
    pub fn mark_views_stale(&mut self, views: &BTreeSet<ViewId>) -> usize {
        let mut marked = 0;
        for id in views {
            if let Some(view) = self.views.get_mut(id) {
                view.mark_stale();
                marked += 1;
            }
        }
        marked
    }

    /// 全库体检：每个视图都能生成 + 每张图纸都合法 + 索引自洽。
    pub fn validate(&self) -> DocResult<()> {
        for view in self.views.values() {
            view.ensure_generatable()?;
        }
        for sheet in self.sheets.values() {
            sheet.validate()?;
        }
        if !self.dependencies.is_consistent() {
            return Err(DocError::Annotation("关联更新索引不自洽".to_string()));
        }
        Ok(())
    }
}

impl Default for Drawing {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::{ElementId, LayerId};
    use glam::DVec2;

    use crate::annotation::Annotation;
    use crate::entity::Entity2D;
    use crate::sheet::Placement;
    use crate::standard::SheetFormat;
    use crate::view::ViewKind;

    fn plan_with_content() -> View {
        let mut v = View::plan("一层平面图", ElementId::new(), 0.0);
        v.push_entity(Entity2D::line(
            LayerId::new(),
            DVec2::ZERO,
            DVec2::new(6000.0, 0.0),
        ));
        v.push_annotation(Annotation::linear(DVec2::ZERO, DVec2::new(6000.0, 0.0)));
        v
    }

    #[test]
    fn default_drawing_uses_the_gb_standard() {
        let d = Drawing::new();
        assert_eq!(d.standard.name, "GB-工业厂房");
        assert_eq!(d.view_count(), 0);
        assert_eq!(d.sheet_count(), 0);
    }

    #[test]
    fn removing_a_view_cleans_sheet_placements() {
        let mut d = Drawing::new();
        let view_id = d.add_view(plan_with_content());

        let mut sheet = Sheet::new("一层平面", "建施-01", SheetFormat::A2);
        sheet.place(Placement::new(view_id, DVec2::new(250.0, 300.0)));
        d.add_sheet(sheet);
        assert_eq!(d.sheet_count(), 1);

        let (_, dropped) = d.remove_view(view_id).unwrap();
        assert_eq!(dropped, 1, "图纸上的布置必须一起清掉，否则悬空");
        let sheet_id = *d.sheets.keys().next().unwrap();
        assert!(d.sheet(sheet_id).unwrap().is_empty());
    }

    #[test]
    fn sync_registry_keeps_document_and_drawing_in_step() {
        let mut d = Drawing::new();
        let view_id = d.add_view(plan_with_content());
        let sheet = Sheet::new("一层平面", "建施-01", SheetFormat::A2);
        let sheet_id = d.add_sheet(sheet);

        let mut doc = Document::new("d");
        let (added, removed) = d.sync_registry(&mut doc);
        assert_eq!(added, 2, "1 个视图 + 1 张图纸");
        assert_eq!(removed, 0);
        assert!(doc.views().contains(&view_id));
        assert!(doc.sheets().contains(&sheet_id));

        // 库里删掉视图后再同步，文档里的登记应被清掉
        d.remove_view(view_id).unwrap();
        let (added2, removed2) = d.sync_registry(&mut doc);
        assert_eq!(added2, 0);
        assert_eq!(removed2, 1);
        assert!(!doc.views().contains(&view_id));
    }

    #[test]
    fn rebuild_index_picks_up_bindings_from_all_views() {
        use core_model::{GeometryKey, ShapeId};

        let shape = ShapeId::new();
        let mut d = Drawing::new();
        let mut v = View::plan("一层", ElementId::new(), 0.0);
        v.push_entity(
            Entity2D::line(LayerId::new(), DVec2::ZERO, DVec2::new(1.0, 0.0))
                .from_geometry(GeometryKey::edge(shape, 0)),
        );
        d.add_view(v);

        d.rebuild_index();
        assert_eq!(d.dependencies.entity_count(), 1);
        assert_eq!(d.dependencies.key_count(), 1);
    }

    #[test]
    fn validate_catches_a_view_that_lost_its_datum() {
        let mut d = Drawing::new();
        let mut v = View::new("坏视图", ViewKind::Plan); // 平面图但没挂标高
        v.up_to_date = true;
        d.add_view(v);
        assert!(matches!(
            d.validate(),
            Err(DocError::UnsupportedViewKind(_))
        ));
    }

    #[test]
    fn healthy_drawing_validates() {
        let mut d = Drawing::new();
        let view_id = d.add_view(plan_with_content());
        let mut sheet = Sheet::new("一层平面", "建施-01", SheetFormat::A2);
        sheet.place(Placement::new(view_id, DVec2::new(250.0, 300.0)));
        d.add_sheet(sheet);
        d.rebuild_index();
        assert!(d.validate().is_ok());
    }

    // ---------- M5 / B3-c：视图绑定查询 + 标脏 ----------

    #[test]
    fn views_bound_to_shapes_finds_only_views_touching_those_shapes() {
        use core_model::{GeometryKey, ShapeId};

        let shape_a = ShapeId::new();
        let shape_b = ShapeId::new();

        let mut d = Drawing::new();

        // v1：图元溯源到 shape_a 的**一个面**（内部键 index=7 是旁证：只看 shape）。
        let mut v1 = View::plan("v1", ElementId::new(), 0.0);
        v1.push_entity(
            Entity2D::line(LayerId::new(), DVec2::ZERO, DVec2::new(1.0, 0.0))
                .from_geometry(GeometryKey::face(shape_a, 7)),
        );
        let id1 = d.add_view(v1);

        // v2：标注**绑到** shape_b 的边。
        let mut v2 = View::plan("v2", ElementId::new(), 0.0);
        v2.push_annotation(
            Annotation::linear(DVec2::ZERO, DVec2::new(1.0, 0.0))
                .bound_to(GeometryKey::edge(shape_b, 0)),
        );
        let id2 = d.add_view(v2);

        // v3：内容没有任何溯源。
        let mut v3 = View::plan("v3", ElementId::new(), 0.0);
        v3.push_entity(Entity2D::line(
            LayerId::new(),
            DVec2::ZERO,
            DVec2::new(1.0, 0.0),
        ));
        let id3 = d.add_view(v3);

        assert_eq!(
            d.views_bound_to_shapes(&[shape_a]),
            BTreeSet::from([id1]),
            "只找绑定了 shape_a 的视图"
        );
        assert_eq!(
            d.views_bound_to_shapes(&[shape_a, shape_b]),
            BTreeSet::from([id1, id2]),
            "一次查询多个形状取并集"
        );
        assert!(
            !d.views_bound_to_shapes(&[shape_a]).contains(&id3),
            "没有溯源的视图不该被牵连"
        );
        assert!(d.views_bound_to_shapes(&[]).is_empty(), "空形状集无绑定");
    }

    #[test]
    fn mark_views_stale_only_touches_bound_views() {
        use core_model::{GeometryKey, ShapeId};

        let shape_a = ShapeId::new();
        let shape_other = ShapeId::new();

        let mut d = Drawing::new();

        let mut v1 = View::plan("v1", ElementId::new(), 0.0);
        v1.push_entity(
            Entity2D::line(LayerId::new(), DVec2::ZERO, DVec2::new(1.0, 0.0))
                .from_geometry(GeometryKey::face(shape_a, 0)),
        );
        let id1 = d.add_view(v1);

        let mut v2 = View::plan("v2", ElementId::new(), 0.0);
        v2.push_entity(
            Entity2D::line(LayerId::new(), DVec2::ZERO, DVec2::new(1.0, 0.0))
                .from_geometry(GeometryKey::face(shape_other, 0)),
        );
        let id2 = d.add_view(v2);

        // 先把两张视图都当成"已随模型更新过"。
        d.view_mut(id1).unwrap().up_to_date = true;
        d.view_mut(id2).unwrap().up_to_date = true;

        let bound = d.views_bound_to_shapes(&[shape_a]);
        let n = d.mark_views_stale(&bound);

        assert_eq!(n, 1, "只有绑定 shape_a 的那张视图被标脏");
        assert!(!d.view(id1).unwrap().up_to_date, "绑定视图被标脏");
        assert!(d.view(id2).unwrap().up_to_date, "无关视图保持最新");
    }

    #[test]
    fn drawing_roundtrips_through_json() {
        let mut d = Drawing::new();
        d.add_view(plan_with_content());
        d.rebuild_index();
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(serde_json::from_str::<Drawing>(&json).unwrap(), d);
    }
}

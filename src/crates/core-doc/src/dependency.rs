//! 关联更新索引（设计文档 6.4）。
//!
//! ## 要解决的问题
//!
//! 模型里挪了一根柱。文档里有 5 张视图、30 条标注 quoting 它。
//! **全量重算**当然对，但慢得不能用；**不重算**又会让图纸和模型对不上。
//!
//! 正确做法是**只重算受影响的**：靠的就是这张双向索引。
//!
//! ```text
//! GeometryKey(shape#7, face#3)  ──forward──▶  { EntityId(a), EntityId(b), Annotation(c) }
//!            ▲
//!            └──reverse── { EntityId(a): { key1, key2 } }
//! ```
//!
//! ## 素材来自哪
//!
//! - **前向**：扫描视图里每个 [`crate::Entity2D::source`] 与
//!   [`crate::Annotation::bound_to`]（[`DependencyGraph::rebuild_from_views`]）；
//! - **"改了哪些键"**：来自内核布尔运算返回的 `EvolutionData`（设计文档 6.4）。
//!
//! ## 与设计文档的一处说明
//!
//! [`core_model::GeometryKey`] 里的 `SubShapeRef` 目前是"拓扑序号"占位。
//! 稳定的拓扑绑定（能跨会话、跨重建）是 M5 的课题——见设计文档 6.6 与
//! [`core_model::SubShapeRef`] 的文档。**本模块的接口不因此改变**：
//! 换掉 `SubShapeRef` 的内部表示，这里一行不用动。

use std::collections::{BTreeMap, BTreeSet};

use core_model::GeometryKey;
use serde::{Deserialize, Serialize};

use crate::ids::EntityId;
use crate::view::View;

/// 模型拓扑 ↔ 二维图元 / 标注 的双向索引。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DependencyGraph {
    /// 模型键 → 依赖它的图元 / 标注
    forward: BTreeMap<GeometryKey, BTreeSet<EntityId>>,
    /// 图元 / 标注 → 它依赖的模型键
    reverse: BTreeMap<EntityId, BTreeSet<GeometryKey>>,
}

impl DependencyGraph {
    /// 空索引。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 建立一条绑定。返回 `true` 表示这是**新**绑定（之前没有）。
    pub fn bind(&mut self, entity: EntityId, key: GeometryKey) -> bool {
        let added = self.forward.entry(key.clone()).or_default().insert(entity);
        self.reverse.entry(entity).or_default().insert(key);
        added
    }

    /// 解除一条绑定。返回 `true` 表示确实删掉了一条。
    pub fn unbind(&mut self, entity: EntityId, key: &GeometryKey) -> bool {
        let removed_f = self
            .forward
            .get_mut(key)
            .is_some_and(|set| set.remove(&entity));
        if self.forward.get(key).is_some_and(|set| set.is_empty()) {
            self.forward.remove(key);
        }
        let removed_r = self
            .reverse
            .get_mut(&entity)
            .is_some_and(|set| set.remove(key));
        if self.reverse.get(&entity).is_some_and(|set| set.is_empty()) {
            self.reverse.remove(&entity);
        }
        removed_f || removed_r
    }

    /// 移除某个图元 / 标注的全部绑定（删图元时调）。
    pub fn remove_entity(&mut self, entity: EntityId) {
        if let Some(keys) = self.reverse.remove(&entity) {
            for key in keys {
                if let Some(set) = self.forward.get_mut(&key) {
                    set.remove(&entity);
                    if set.is_empty() {
                        self.forward.remove(&key);
                    }
                }
            }
        }
    }

    /// 某个模型键影响了哪些图元 / 标注。
    #[must_use]
    pub fn dependents_of(&self, key: &GeometryKey) -> BTreeSet<EntityId> {
        self.forward.get(key).cloned().unwrap_or_default()
    }

    /// 某个图元 / 标注依赖了哪些模型键。
    #[must_use]
    pub fn keys_of(&self, entity: EntityId) -> BTreeSet<GeometryKey> {
        self.reverse.get(&entity).cloned().unwrap_or_default()
    }

    /// **关联更新的主查询**：这一批变了的模型键，一共牵连多少图元 / 标注。
    ///
    /// 调用方拿这个集合去重投 / 重标，其余图纸内容不动。
    #[must_use]
    pub fn affected_by<'k, I>(&self, keys: I) -> BTreeSet<EntityId>
    where
        I: IntoIterator<Item = &'k GeometryKey>,
    {
        let mut out = BTreeSet::new();
        for key in keys {
            if let Some(set) = self.forward.get(key) {
                out.extend(set.iter().copied());
            }
        }
        out
    }

    /// **关联更新主查询（带重解析）**：给定"旧键 → 重解析后的新键"映射，
    /// 返回受影响图元 / 标注集合。
    ///
    /// 与 [`DependencyGraph::affected_by`] 的区别：稳定名重解析后，同一份绑定可能
    /// **旧键还没改绑、新键已经被引用**，故**旧键与新键的绑定都算**——
    /// 漏掉任一方向都会让"该重投的图元"漏网。
    #[must_use]
    pub fn affected_after_resolve(
        &self,
        resolved: &BTreeMap<GeometryKey, GeometryKey>,
    ) -> BTreeSet<EntityId> {
        let mut out = BTreeSet::new();
        for (old, new) in resolved {
            if let Some(set) = self.forward.get(old) {
                out.extend(set.iter().copied());
            }
            if let Some(set) = self.forward.get(new) {
                out.extend(set.iter().copied());
            }
        }
        out
    }

    /// 把索引里所有 `from` 键**就地改绑**到 `to` 键（稳定名重解析后调）。
    ///
    /// 返回**被改绑的旧键条数**：只统计在 `forward` 里确实有绑定的旧键
    /// （没有绑定的旧键无可改绑，不计入），自映射（`from == to`）亦不计入。
    ///
    /// `forward` / `reverse` **两个方向同时维护一致**：旧键从正表摘除、并入新键，
    /// 反向表里每个实体的键集合也把旧键换成新键。调用后
    /// [`DependencyGraph::is_consistent`] 必须仍为 `true`。
    pub fn remap(&mut self, resolved: &BTreeMap<GeometryKey, GeometryKey>) -> usize {
        let mut remapped = 0;
        for (old, new) in resolved {
            if old == new {
                continue;
            }
            let Some(entities) = self.forward.remove(old) else {
                continue;
            };
            self.forward
                .entry(new.clone())
                .or_default()
                .extend(entities.iter().copied());
            for entity in entities {
                if let Some(keys) = self.reverse.get_mut(&entity) {
                    keys.remove(old);
                    keys.insert(new.clone());
                }
            }
            remapped += 1;
        }
        remapped
    }

    /// 从若干视图重建索引（覆盖旧内容）。
    ///
    /// 素材：每条 [`crate::Entity2D::source`] 与每个 [`crate::Annotation::bound_to`]。
    pub fn rebuild_from_views(&mut self, views: &[View]) {
        self.clear();
        for view in views {
            for entity in &view.entities {
                if let Some(key) = &entity.source {
                    self.bind(entity.id, key.clone());
                }
            }
            for ann in &view.annotations {
                if let Some(key) = &ann.bound_to {
                    self.bind(ann.id, key.clone());
                }
            }
        }
    }

    /// 清空。
    pub fn clear(&mut self) {
        self.forward.clear();
        self.reverse.clear();
    }

    /// 被引用的模型键数量。
    #[must_use]
    pub fn key_count(&self) -> usize {
        self.forward.len()
    }

    /// 参与绑定的图元 / 标注数量。
    #[must_use]
    pub fn entity_count(&self) -> usize {
        self.reverse.len()
    }

    /// 是否还没有任何绑定。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.forward.is_empty()
    }

    /// 自检：正向与反向两张表必须互为镜像。
    ///
    /// 这是 `core-testkit` 用来体检不变式的一条断言。
    #[must_use]
    pub fn is_consistent(&self) -> bool {
        let forward_ok = self.forward.iter().all(|(key, entities)| {
            entities
                .iter()
                .all(|e| self.reverse.get(e).is_some_and(|keys| keys.contains(key)))
        });
        let reverse_ok = self.reverse.iter().all(|(entity, keys)| {
            keys.iter()
                .all(|k| self.forward.get(k).is_some_and(|set| set.contains(entity)))
        });
        forward_ok && reverse_ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::{LayerId, ShapeId};
    use glam::DVec2;

    use crate::annotation::Annotation;
    use crate::entity::Entity2D;
    use crate::view::{View, ViewKind};

    fn view_with_two_bound_entities(shape: ShapeId) -> View {
        let mut v = View::new("v", ViewKind::Drafting);
        v.push_entity(
            Entity2D::line(LayerId::new(), DVec2::ZERO, DVec2::new(10.0, 0.0))
                .from_geometry(GeometryKey::edge(shape, 0)),
        );
        v.push_entity(
            Entity2D::line(
                LayerId::new(),
                DVec2::new(10.0, 0.0),
                DVec2::new(10.0, 10.0),
            )
            .from_geometry(GeometryKey::edge(shape, 1)),
        );
        v.push_annotation(
            Annotation::linear(DVec2::ZERO, DVec2::new(10.0, 0.0))
                .bound_to(GeometryKey::edge(shape, 0)),
        );
        v
    }

    #[test]
    fn binding_is_bidirectional() {
        let shape = ShapeId::new();
        let mut g = DependencyGraph::new();
        let e = EntityId::new();
        let key = GeometryKey::face(shape, 0);

        assert!(g.bind(e, key.clone()));
        assert_eq!(g.dependents_of(&key).len(), 1);
        assert!(g.keys_of(e).contains(&key));
        assert!(g.is_consistent());
    }

    #[test]
    fn binding_twice_is_idempotent() {
        let shape = ShapeId::new();
        let mut g = DependencyGraph::new();
        let e = EntityId::new();
        let key = GeometryKey::face(shape, 0);
        assert!(g.bind(e, key.clone()));
        assert!(!g.bind(e, key.clone()), "重复绑定不算新增");
        assert_eq!(g.dependents_of(&key).len(), 1);
    }

    #[test]
    fn unbinding_removes_from_both_tables() {
        let shape = ShapeId::new();
        let mut g = DependencyGraph::new();
        let e = EntityId::new();
        let key = GeometryKey::face(shape, 0);
        g.bind(e, key.clone());

        assert!(g.unbind(e, &key));
        assert!(g.is_empty(), "两张表都应清空");
        assert!(g.is_consistent());
    }

    #[test]
    fn affected_by_returns_the_union_for_a_batch_of_keys() {
        let shape = ShapeId::new();
        let mut g = DependencyGraph::new();
        let v = view_with_two_bound_entities(shape);
        g.rebuild_from_views(&[v]);

        let edge0 = GeometryKey::edge(shape, 0);
        let edge1 = GeometryKey::edge(shape, 1);

        assert_eq!(
            g.affected_by([&edge0]).len(),
            2,
            "edge0 上有 1 条线 + 1 个标注"
        );
        assert_eq!(g.affected_by([&edge1]).len(), 1);
        assert_eq!(
            g.affected_by([&edge0, &edge1]).len(),
            3,
            "两条边一起变，涉及全部 3 个对象"
        );
    }

    // ---------- M5 / B3-c：带重解析的查询与改绑 ----------

    #[test]
    fn affected_after_resolve_covers_old_and_new_keys() {
        let shape = ShapeId::new();
        let mut g = DependencyGraph::new();

        let old = GeometryKey::face(shape, 1);
        let new = GeometryKey::face(shape, 2);
        let e_old = EntityId::new();
        let e_new = EntityId::new();
        let e_other = EntityId::new();
        g.bind(e_old, old.clone());
        g.bind(e_new, new.clone());
        g.bind(e_other, GeometryKey::face(shape, 9));

        let resolved = BTreeMap::from([(old.clone(), new.clone())]);
        let affected = g.affected_after_resolve(&resolved);

        assert!(affected.contains(&e_old), "旧键的绑定必须被列入");
        assert!(affected.contains(&e_new), "新键的绑定也必须被列入");
        assert_eq!(affected.len(), 2);
        assert!(!affected.contains(&e_other), "未被重解析的键不受牵连");
    }

    #[test]
    fn remap_rebinds_and_reports_count() {
        let shape = ShapeId::new();
        let mut g = DependencyGraph::new();
        let old = GeometryKey::face(shape, 0);
        let new = GeometryKey::face(shape, 1);
        let e = EntityId::new();
        g.bind(e, old.clone());

        let resolved = BTreeMap::from([(old.clone(), new.clone())]);
        assert_eq!(g.remap(&resolved), 1, "一个 from 键被改绑");

        assert!(g.dependents_of(&old).is_empty(), "旧键查询应为空");
        assert!(g.keys_of(e).contains(&new), "keys_of 必须指向新键");
        assert!(!g.keys_of(e).contains(&old));
        assert_eq!(g.dependents_of(&new), BTreeSet::from([e]));
        assert!(g.is_consistent(), "改绑后正反两表仍须互为镜像");

        // 幂等：旧键已不在索引里，再跑一次无可改绑。
        assert_eq!(g.remap(&resolved), 0);
    }

    #[test]
    fn remap_merges_two_old_keys_into_one_new_key() {
        let shape = ShapeId::new();
        let mut g = DependencyGraph::new();
        let old_a = GeometryKey::face(shape, 0);
        let old_b = GeometryKey::face(shape, 1);
        let new = GeometryKey::face(shape, 7);
        let ea = EntityId::new();
        let eb = EntityId::new();
        g.bind(ea, old_a.clone());
        g.bind(eb, old_b.clone());

        let resolved = BTreeMap::from([(old_a.clone(), new.clone()), (old_b.clone(), new.clone())]);
        assert_eq!(g.remap(&resolved), 2, "两个 from 键各算一条");

        assert_eq!(g.dependents_of(&new), BTreeSet::from([ea, eb]));
        assert!(g.dependents_of(&old_a).is_empty());
        assert!(g.dependents_of(&old_b).is_empty());
        assert!(g.is_consistent());
    }

    #[test]
    fn removing_an_entity_cleans_up_every_reference() {
        let shape = ShapeId::new();
        let mut g = DependencyGraph::new();
        g.bind(EntityId::new(), GeometryKey::face(shape, 0));
        let target = *g
            .forward
            .get(&GeometryKey::face(shape, 0))
            .and_then(|s| s.iter().next())
            .unwrap();

        g.remove_entity(target);
        assert!(g.is_empty());
        assert!(g.is_consistent());
    }

    #[test]
    fn rebuild_from_views_scans_entities_and_annotations() {
        let shape = ShapeId::new();
        let mut g = DependencyGraph::new();
        let v = view_with_two_bound_entities(shape);
        g.rebuild_from_views(&[v]);

        assert_eq!(g.key_count(), 2, "两条不同的边");
        assert_eq!(g.entity_count(), 3, "2 条线 + 1 个标注");
        assert!(g.is_consistent());
    }

    #[test]
    fn graph_roundtrips_through_json() {
        let shape = ShapeId::new();
        let mut g = DependencyGraph::new();
        g.rebuild_from_views(&[view_with_two_bound_entities(shape)]);
        let json = serde_json::to_string(&g).unwrap();
        assert_eq!(serde_json::from_str::<DependencyGraph>(&json).unwrap(), g);
    }
}

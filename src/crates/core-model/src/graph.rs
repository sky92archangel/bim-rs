//! 关系图查询原语（设计文档 4.1 `Relations`、4.6 关系层、6.4 关联更新的**骨架**）。
//!
//! ## 职责边界（为什么只放"纯遍历"）
//!
//! 本模块**只做零依赖的有界图遍历**：把 [`Document`] 里的关系表当作一张
//! **无向图**，回答"从某个端点出发、N 跳内能到达哪些端点 / 视图 / 图纸"。
//! 它不建索引、不碰几何、不依赖任何上层 crate——因此归 M2 数据层。
//!
//! 而"脏标记 → 增量重绘"的**关联更新引擎**（依赖内核几何演化历史与 `core-doc`
//! 的二维图元）属 M5，**不在本模块**。这里只提供它将要使用的遍历原语。
//!
//! ## 关键性质
//!
//! - **把关系视为无向边**：用 [`Relation::touches`] 判定，A→B 与 B→A 等价。
//! - **必须防环**：用 `visited` 集合，环状关系 + 有限 `max_depth` 也一定会终止。
//! - **确定性**：结果按 `(跳数, 端点)` 排序（[`Endpoint`] 实现了 `Ord`），便于测试与 diff。

use std::collections::{BTreeSet, VecDeque};

use crate::document::Document;
use crate::element::Element;
use crate::ids::{ElementId, GeometryKey, SheetId, ViewId};
use crate::relation::{Endpoint, RelationKind};

impl Document {
    /// 有界广度优先遍历关系图（**把关系视为无向边**）。
    ///
    /// 返回 `(跳数, 端点)` 列表，**含起点自身（跳数 0）**；`max_depth == 0` 时
    /// 只返回起点。结果按 `(跳数, 端点)` 升序排列并去重。
    ///
    /// 实现用 [`VecDeque`] + [`BTreeSet`] 已访问集合——**环状关系下也能终止**。
    #[must_use]
    pub fn relations_walk(&self, start: Endpoint, max_depth: usize) -> Vec<(usize, Endpoint)> {
        let mut visited: BTreeSet<Endpoint> = BTreeSet::new();
        let mut reached: Vec<(usize, Endpoint)> = Vec::new();
        let mut queue: VecDeque<(usize, Endpoint)> = VecDeque::new();

        visited.insert(start);
        reached.push((0, start));
        queue.push_back((0, start));

        while let Some((depth, node)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            for relation in self.relations().values() {
                // 无向边：任一端等于 node，另一端即为邻居。
                let neighbour = if relation.from == node {
                    Some(relation.to)
                } else if relation.to == node {
                    Some(relation.from)
                } else {
                    None
                };
                if let Some(next) = neighbour
                    && visited.insert(next)
                {
                    let next_depth = depth + 1;
                    reached.push((next_depth, next));
                    queue.push_back((next_depth, next));
                }
            }
        }

        reached.sort();
        reached
    }

    /// 从某元素出发，经 `<= max_depth` 跳可达的**视图** id 集合
    /// （供关联更新定位"要重绘的视图"）。
    #[must_use]
    pub fn dependent_views(&self, start: ElementId, max_depth: usize) -> BTreeSet<ViewId> {
        self.relations_walk(Endpoint::Element(start), max_depth)
            .into_iter()
            .filter_map(|(_, endpoint)| match endpoint {
                Endpoint::View(view) => Some(view),
                _ => None,
            })
            .collect()
    }

    /// 从某元素出发，经 `<= max_depth` 跳可达的**图纸** id 集合。
    #[must_use]
    pub fn dependent_sheets(&self, start: ElementId, max_depth: usize) -> BTreeSet<SheetId> {
        self.relations_walk(Endpoint::Element(start), max_depth)
            .into_iter()
            .filter_map(|(_, endpoint)| match endpoint {
                Endpoint::Sheet(sheet) => Some(sheet),
                _ => None,
            })
            .collect()
    }

    /// 反向查：哪些元素通过 [`RelationKind::BoundToGeometry`] 绑定了给定几何单元
    /// （§6.4 的 geom → 元素一跳）。
    ///
    /// **精确匹配**给定的 [`GeometryKey`]——只返回绑定了该键的元素。
    #[must_use]
    pub fn elements_bound_to_geometry(&self, key: &GeometryKey) -> Vec<&Element> {
        self.relations()
            .values()
            .filter(
                |relation| matches!(&relation.kind, RelationKind::BoundToGeometry(k) if k == key),
            )
            .filter_map(|relation| match relation.from {
                Endpoint::Element(id) => self.element(id),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datum::DatumElement;
    use crate::element::Category;
    use crate::geometry::GeometryRecord;
    use crate::ids::ShapeId;
    use crate::relation::Relation;

    fn element(name: &str) -> Element {
        Element::new(Category::generic(), name)
    }

    #[test]
    fn walk_finds_a_two_hop_view_but_depth_one_does_not() {
        // A --PositionedOn--> D（基准）；V --ReferencedByDatum--> D。
        // 于是 A 到 V 需要 2 跳（经 D）。
        let mut doc = Document::new("d");
        let a = element("柱");
        let a_id = a.id;
        doc.insert_element(a).unwrap();

        let level = DatumElement::level("F2", 3600.0);
        let level_id = level.id;
        doc.insert_datum(level).unwrap();

        let view = ViewId::new();

        doc.insert_relation(Relation::new(
            RelationKind::PositionedOn,
            a_id.into(),
            level_id.into(),
        ))
        .unwrap();
        doc.insert_relation(Relation::new(
            RelationKind::ReferencedByDatum,
            Endpoint::View(view),
            level_id.into(),
        ))
        .unwrap();

        let two = doc.dependent_views(a_id, 2);
        assert!(two.contains(&view), "2 跳应能找到该视图，实得 {two:?}");

        let one = doc.dependent_views(a_id, 1);
        assert!(
            !one.contains(&view),
            "1 跳不应找到该视图（否则说明 max_depth 被忽略或退化成单跳）"
        );

        // 起点自身在 0 跳，且 walk 至少返回起点
        let zero = doc.relations_walk(Endpoint::Element(a_id), 0);
        assert_eq!(zero, vec![(0, Endpoint::Element(a_id))]);
    }

    #[test]
    fn walk_terminates_on_cycles() {
        // A 与 B 互指成环；有限 max_depth 下必须终止（漏 visited 会死循环超时）。
        let mut doc = Document::new("d");
        let a = element("A");
        let a_id = a.id;
        doc.insert_element(a).unwrap();
        let b = element("B");
        let b_id = b.id;
        doc.insert_element(b).unwrap();

        doc.insert_relation(Relation::new(
            RelationKind::Contains,
            a_id.into(),
            b_id.into(),
        ))
        .unwrap();
        doc.insert_relation(Relation::new(
            RelationKind::Contains,
            b_id.into(),
            a_id.into(),
        ))
        .unwrap();

        let reached = doc.relations_walk(Endpoint::Element(a_id), 3);
        // 只有 A、B 两个端点，且各出现一次（去重）
        assert_eq!(
            reached.len(),
            2,
            "环状关系应终止且只含两个端点：{reached:?}"
        );
        assert!(reached.contains(&(0, Endpoint::Element(a_id))));
        assert!(reached.contains(&(1, Endpoint::Element(b_id))));
    }

    #[test]
    fn elements_bound_to_geometry_is_exact() {
        // 两条不同 key 的 BoundToGeometry：只应精确命中对应元素。
        let mut doc = Document::new("d");
        let a = element("绑定-1");
        let a_id = a.id;
        doc.insert_element(a).unwrap();
        let b = element("绑定-2");
        let b_id = b.id;
        doc.insert_element(b).unwrap();

        let k1 = GeometryKey::face(ShapeId::new(), 0);
        let k2 = GeometryKey::whole(ShapeId::new());

        doc.insert_relation(Relation::new(
            RelationKind::BoundToGeometry(k1.clone()),
            a_id.into(),
            a_id.into(),
        ))
        .unwrap();
        doc.insert_relation(Relation::new(
            RelationKind::BoundToGeometry(k2.clone()),
            b_id.into(),
            b_id.into(),
        ))
        .unwrap();

        let hits = doc.elements_bound_to_geometry(&k1);
        assert_eq!(hits.len(), 1, "只应命中绑定了 k1 的那一个元素");
        assert_eq!(hits[0].id, a_id);

        assert!(
            doc.elements_bound_to_geometry(&GeometryKey::edge(ShapeId::new(), 9))
                .is_empty(),
            "没有任何绑定的 key 应返回空（否则说明忽略了 key 匹配）"
        );
    }

    #[test]
    fn dependent_sheets_walk_reaches_sheets() {
        let mut doc = Document::new("d");
        let a = element("A");
        let a_id = a.id;
        doc.insert_element(a).unwrap();
        let b = element("B");
        let b_id = b.id;
        doc.insert_element(b).unwrap();

        let sheet = SheetId::new();
        doc.insert_relation(Relation::new(
            RelationKind::Contains,
            a_id.into(),
            b_id.into(),
        ))
        .unwrap();
        doc.insert_relation(Relation::new(
            RelationKind::Contains,
            b_id.into(),
            Endpoint::Sheet(sheet),
        ))
        .unwrap();

        let sheets = doc.dependent_sheets(a_id, 2);
        assert!(sheets.contains(&sheet));
        assert!(doc.dependent_sheets(a_id, 1).is_empty());
    }

    #[test]
    fn geometry_records_do_not_participate_in_the_walk() {
        // 关系图只由 relations 决定；几何记录的存在与否不影响遍历结果。
        let mut doc = Document::new("d");
        let a = element("A");
        let a_id = a.id;
        doc.insert_element(a).unwrap();
        doc.insert_geometry(GeometryRecord::brep(ShapeId::new(), "t"))
            .unwrap();

        let reached = doc.relations_walk(Endpoint::Element(a_id), 5);
        assert_eq!(reached, vec![(0, Endpoint::Element(a_id))]);
    }

    #[test]
    fn walk_terminates_on_a_self_loop() {
        // 自环：from == to。无向邻接里 node 的"另一端点"就是它自己，
        // 已 visited → 不再入队；必须只返回起点且不死循环。
        let mut doc = Document::new("d");
        let a = element("A");
        let a_id = a.id;
        doc.insert_element(a).unwrap();
        doc.insert_relation(Relation::new(
            RelationKind::Contains,
            a_id.into(),
            a_id.into(),
        ))
        .unwrap();

        let reached = doc.relations_walk(Endpoint::Element(a_id), 5);
        assert_eq!(
            reached,
            vec![(0, Endpoint::Element(a_id))],
            "自环下应只返回起点"
        );
    }

    #[test]
    fn duplicate_bidirectional_edges_are_deduplicated() {
        // 同时存在 A→B 与 B→A 两条关系：端点必须去重，每个端点只出现一次。
        let mut doc = Document::new("d");
        let a = element("A");
        let a_id = a.id;
        doc.insert_element(a).unwrap();
        let b = element("B");
        let b_id = b.id;
        doc.insert_element(b).unwrap();

        doc.insert_relation(Relation::new(
            RelationKind::Contains,
            a_id.into(),
            b_id.into(),
        ))
        .unwrap();
        doc.insert_relation(Relation::new(
            RelationKind::Contains,
            b_id.into(),
            a_id.into(),
        ))
        .unwrap();

        let reached = doc.relations_walk(Endpoint::Element(a_id), 3);
        assert_eq!(reached.len(), 2, "重复双向边应去重：{reached:?}");
        assert_eq!(reached.iter().filter(|(_, e)| *e == a_id.into()).count(), 1);
        assert_eq!(reached.iter().filter(|(_, e)| *e == b_id.into()).count(), 1);
    }

    #[test]
    fn disconnected_component_is_not_reached() {
        // 另有一对无关端点 C↔D；从 A 出发（其分量仅含 A、B）不得含 C / D。
        let mut doc = Document::new("d");
        let a = element("A");
        let a_id = a.id;
        doc.insert_element(a).unwrap();
        let b = element("B");
        let b_id = b.id;
        doc.insert_element(b).unwrap();
        let c = element("C");
        let c_id = c.id;
        doc.insert_element(c).unwrap();
        let d = element("D");
        let d_id = d.id;
        doc.insert_element(d).unwrap();

        doc.insert_relation(Relation::new(
            RelationKind::Contains,
            a_id.into(),
            b_id.into(),
        ))
        .unwrap();
        doc.insert_relation(Relation::new(
            RelationKind::Contains,
            c_id.into(),
            d_id.into(),
        ))
        .unwrap();

        let reached = doc.relations_walk(Endpoint::Element(a_id), 100);
        assert!(reached.contains(&(0, Endpoint::Element(a_id))));
        assert!(reached.contains(&(1, Endpoint::Element(b_id))));
        let endpoints: Vec<Endpoint> = reached.iter().map(|(_, e)| *e).collect();
        assert!(
            !endpoints.contains(&Endpoint::Element(c_id))
                && !endpoints.contains(&Endpoint::Element(d_id)),
            "不得跨越连通分量：{reached:?}"
        );
    }

    #[test]
    fn same_shape_different_subshape_keys_do_not_cross_match() {
        // 同一 ShapeId 下 face(s,0) / edge(s,0) / whole(s) 三个键互不串味。
        let mut doc = Document::new("d");
        let a = element("A");
        let a_id = a.id;
        doc.insert_element(a).unwrap();
        let b = element("B");
        let b_id = b.id;
        doc.insert_element(b).unwrap();
        let c = element("C");
        let c_id = c.id;
        doc.insert_element(c).unwrap();

        let shape = ShapeId::new();
        let k_face = GeometryKey::face(shape, 0);
        let k_edge = GeometryKey::edge(shape, 0);
        let k_whole = GeometryKey::whole(shape);

        doc.insert_relation(Relation::new(
            RelationKind::BoundToGeometry(k_face.clone()),
            a_id.into(),
            a_id.into(),
        ))
        .unwrap();
        doc.insert_relation(Relation::new(
            RelationKind::BoundToGeometry(k_edge.clone()),
            b_id.into(),
            b_id.into(),
        ))
        .unwrap();
        doc.insert_relation(Relation::new(
            RelationKind::BoundToGeometry(k_whole.clone()),
            c_id.into(),
            c_id.into(),
        ))
        .unwrap();

        let hit_face = doc.elements_bound_to_geometry(&k_face);
        assert_eq!(hit_face.len(), 1);
        assert_eq!(hit_face[0].id, a_id);

        let hit_edge = doc.elements_bound_to_geometry(&k_edge);
        assert_eq!(hit_edge.len(), 1);
        assert_eq!(hit_edge[0].id, b_id);

        let hit_whole = doc.elements_bound_to_geometry(&k_whole);
        assert_eq!(hit_whole.len(), 1);
        assert_eq!(hit_whole[0].id, c_id);
    }
}

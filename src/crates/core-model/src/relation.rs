//! 关系模型（设计文档 4.1 的 `Relations`、4.6 的关系层、6.4 的关联更新）。
//!
//! 关系图是**关联更新的骨架**：改一个构件，沿关系图就能找到"哪些视图、哪些标注
//! 要跟着变"，而不必全量重算。

use serde::{Deserialize, Serialize};

use crate::ids::{ElementId, GeometryKey, RelationId, SheetId, ViewId};

/// 关系端点：关系可以连接元素、视图或图纸。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Endpoint {
    /// 元素
    Element(ElementId),
    /// 视图（本体在 `core-doc`）
    View(ViewId),
    /// 图纸（本体在 `core-doc`）
    Sheet(SheetId),
}

impl From<ElementId> for Endpoint {
    fn from(v: ElementId) -> Self {
        Self::Element(v)
    }
}

impl From<ViewId> for Endpoint {
    fn from(v: ViewId) -> Self {
        Self::View(v)
    }
}

impl From<SheetId> for Endpoint {
    fn from(v: SheetId) -> Self {
        Self::Sheet(v)
    }
}

/// 关系种类。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RelationKind {
    /// 构件**定位在**某基准元素上（如"标高 X 上的 Y 轴"，设计文档 4.6.1）。
    PositionedOn,
    /// 视图 / 图纸**引用了**某基准元素（设计文档 4.6 关系层）。
    ReferencedByDatum,
    /// 二维图元 / 标注**绑定到**模型几何（关联更新用，设计文档 6.4）。
    BoundToGeometry(GeometryKey),
    /// 通用父子包含。
    Contains,
}

/// 一条关系。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relation {
    /// 关系 id
    pub id: RelationId,
    /// 种类
    pub kind: RelationKind,
    /// 起端
    pub from: Endpoint,
    /// 终端
    pub to: Endpoint,
    /// 备注（可选）
    pub note: String,
}

impl Relation {
    /// 构造一条关系（自动生成 id）。
    #[must_use]
    pub fn new(kind: RelationKind, from: Endpoint, to: Endpoint) -> Self {
        Self {
            id: RelationId::new(),
            kind,
            from,
            to,
            note: String::new(),
        }
    }

    /// 附一句备注。
    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = note.into();
        self
    }

    /// 该关系是否触及给定端点。
    #[must_use]
    pub fn touches(&self, endpoint: Endpoint) -> bool {
        self.from == endpoint || self.to == endpoint
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positioned_on_relation_links_element_to_datum() {
        let column = ElementId::new();
        let level = ElementId::new();
        let r = Relation::new(RelationKind::PositionedOn, column.into(), level.into());
        assert!(r.touches(column.into()));
        assert!(r.touches(level.into()));
        assert!(!r.touches(ElementId::new().into()));
    }

    #[test]
    fn relation_endpoints_support_views() {
        let view = ViewId::new();
        let grid = ElementId::new();
        let r = Relation::new(
            RelationKind::ReferencedByDatum,
            Endpoint::View(view),
            grid.into(),
        );
        assert_eq!(r.from, Endpoint::View(view));
    }

    #[test]
    fn bound_to_geometry_carries_the_geometry_key() {
        let shape = crate::ids::ShapeId::new();
        let key = GeometryKey::face(shape, 3);
        let r = Relation::new(
            RelationKind::BoundToGeometry(key.clone()),
            ElementId::new().into(),
            ElementId::new().into(),
        );
        match r.kind {
            RelationKind::BoundToGeometry(k) => assert_eq!(k, key),
            _ => panic!("种类应为 BoundToGeometry"),
        }
    }
}

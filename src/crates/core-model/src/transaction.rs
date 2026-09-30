//! 事务（设计文档 4.3）。
//!
//! ## 为什么要有事务
//!
//! 一次用户操作（"画一段桥架"）往往要改好几处：建元素、建几何、建关系、登记视图。
//! **要么全成，要么全不成**——否则会留下半个构件、悬空关系这类脏数据。
//!
//! ## 两条硬约束（来自设计文档 4.3）
//!
//! 1. **提交是原子的**：中途任何一步失败，文档回到事务开始前的样子。
//! 2. **一次提交 = 一个撤销步**：`version` 只 +1，撤销栈只需存一个 [`Delta`] 批次。
//!
//! ## 实现方式
//!
//! 本实现采用**影子副本 + 一次性换入**：提交时先克隆一份文档，把变更逐条应用到副本上；
//! 全部成功才换回。代价是一次 `Document` 深拷贝（文档级、低频，可接受），
//! 换来的是"不可能出现半提交"这一强保证。
//!
//! ⚠ **待优化**：文档很大时可改为"记录反向补丁 + 失败回放"以避免克隆。
//! 这属于 M2 的性能优化项，当前正确性优先。

use serde::{Deserialize, Serialize};

use crate::datum::DatumElement;
use crate::document::Document;
use crate::element::Element;
use crate::error::ModelResult;
use crate::geometry::GeometryRecord;
use crate::ids::{ElementId, RelationId, ShapeId, SheetId, ViewId};
use crate::relation::Relation;

/// 一次事务内的**一条变更**。
///
/// 可序列化：撤销 / 重做栈直接存它，也能进日志做审计。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Delta {
    /// 新建元素
    InsertElement(Box<Element>),
    /// 删除元素（级联删关系）
    RemoveElement(ElementId),
    /// 新建基准元素
    InsertDatum(Box<DatumElement>),
    /// 删除基准元素（级联删关系）
    RemoveDatum(ElementId),
    /// 新增几何记录
    InsertGeometry(Box<GeometryRecord>),
    /// 删除几何记录
    RemoveGeometry(ShapeId),
    /// 新增关系
    InsertRelation(Box<Relation>),
    /// 删除关系
    RemoveRelation(RelationId),
    /// 登记视图 id
    RegisterView(ViewId),
    /// 注销视图 id
    UnregisterView(ViewId),
    /// 登记图纸 id
    RegisterSheet(SheetId),
    /// 注销图纸 id
    UnregisterSheet(SheetId),
}

/// 一次事务：**变更先攒着，`commit` 才落地**。
///
/// ```no_run
/// use core_model::{Transaction, Element, Category, Document};
///
/// let mut doc = Document::new("demo");
/// let mut tx = Transaction::begin(&mut doc);
/// tx.add_element(Element::new(Category::generic(), "柱"));
/// tx.commit().unwrap();
/// assert_eq!(doc.element_count(), 1);
/// ```
pub struct Transaction<'d> {
    doc: &'d mut Document,
    pending: Vec<Delta>,
    committed: bool,
}

impl<'d> Transaction<'d> {
    /// 开启一个事务。
    #[must_use]
    pub fn begin(doc: &'d mut Document) -> Self {
        Self {
            doc,
            pending: Vec::new(),
            committed: false,
        }
    }

    /// 已攒下的变更条数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// 是否还没有任何变更。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// 只读地看一眼攒下的变更（测试 / 调试用）。
    #[must_use]
    pub fn pending(&self) -> &[Delta] {
        &self.pending
    }

    /// 新建元素，返回其持久 id。
    pub fn add_element(&mut self, element: Element) -> ElementId {
        let id = element.id;
        self.pending.push(Delta::InsertElement(Box::new(element)));
        id
    }

    /// 删除元素。
    pub fn remove_element(&mut self, id: ElementId) {
        self.pending.push(Delta::RemoveElement(id));
    }

    /// 新建基准元素，返回其持久 id。
    pub fn add_datum(&mut self, datum: DatumElement) -> ElementId {
        let id = datum.id;
        self.pending.push(Delta::InsertDatum(Box::new(datum)));
        id
    }

    /// 删除基准元素。
    pub fn remove_datum(&mut self, id: ElementId) {
        self.pending.push(Delta::RemoveDatum(id));
    }

    /// 新增几何记录，返回其 id。
    pub fn add_geometry(&mut self, record: GeometryRecord) -> ShapeId {
        let id = record.shape_id;
        self.pending.push(Delta::InsertGeometry(Box::new(record)));
        id
    }

    /// 删除几何记录。
    pub fn remove_geometry(&mut self, id: ShapeId) {
        self.pending.push(Delta::RemoveGeometry(id));
    }

    /// 新增关系，返回其 id。
    pub fn add_relation(&mut self, relation: Relation) -> RelationId {
        let id = relation.id;
        self.pending.push(Delta::InsertRelation(Box::new(relation)));
        id
    }

    /// 删除关系。
    pub fn remove_relation(&mut self, id: RelationId) {
        self.pending.push(Delta::RemoveRelation(id));
    }

    /// 登记视图 id。
    pub fn register_view(&mut self, id: ViewId) {
        self.pending.push(Delta::RegisterView(id));
    }

    /// 登记图纸 id。
    pub fn register_sheet(&mut self, id: SheetId) {
        self.pending.push(Delta::RegisterSheet(id));
    }

    /// 直接推入一条自定义变更（模块 / 将来的命令层用）。
    pub fn push(&mut self, delta: Delta) {
        self.pending.push(delta);
    }

    /// 提交：把本轮变更**原子地**应用到文档上。
    ///
    /// 成功且**确实有新变更**时，`version` +1 并把这次提交的 [`Delta`] 批次返回
    /// （调用方可压入撤销栈）。空事务返回空 `Vec` 且不动版本号。
    pub fn commit(self) -> ModelResult<Vec<Delta>> {
        let Self {
            doc,
            pending,
            committed,
        } = self;
        if committed {
            return Ok(Vec::new());
        }
        if pending.is_empty() {
            return Ok(Vec::new());
        }

        // 影子副本：先在副本上跑，全绿才换回，保证"不可能半提交"。
        let mut staged = doc.clone();
        for delta in &pending {
            apply(&mut staged, delta)?;
        }
        staged.bump_version();

        *doc = staged;
        Ok(pending)
    }

    /// 放弃事务（无副作用；文档从未被改动过）。
    pub fn rollback(self) -> Vec<Delta> {
        self.pending
    }
}

/// 把一条变更应用到文档上（提交路径的唯一入口）。
fn apply(doc: &mut Document, delta: &Delta) -> ModelResult<()> {
    match delta {
        Delta::InsertElement(e) => {
            doc.insert_element((**e).clone())?;
        }
        Delta::RemoveElement(id) => {
            doc.remove_element(*id)?;
        }
        Delta::InsertDatum(d) => {
            doc.insert_datum((**d).clone())?;
        }
        Delta::RemoveDatum(id) => {
            doc.remove_datum(*id)?;
        }
        Delta::InsertGeometry(g) => {
            doc.insert_geometry((**g).clone())?;
        }
        Delta::RemoveGeometry(id) => {
            doc.remove_geometry(*id);
        }
        Delta::InsertRelation(r) => {
            doc.insert_relation((**r).clone())?;
        }
        Delta::RemoveRelation(id) => {
            doc.remove_relation(*id)?;
        }
        Delta::RegisterView(id) => {
            doc.register_view(*id);
        }
        Delta::UnregisterView(id) => {
            doc.unregister_view(*id);
        }
        Delta::RegisterSheet(id) => {
            doc.register_sheet(*id);
        }
        Delta::UnregisterSheet(id) => {
            doc.unregister_sheet(*id);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datum::DatumElement;
    use crate::element::Category;
    use crate::error::ModelError;
    use crate::ids::ShapeId;
    use crate::relation::{Relation, RelationKind};

    #[test]
    fn commit_applies_all_changes_and_bumps_version_once() {
        let mut doc = Document::new("d");
        let mut tx = Transaction::begin(&mut doc);

        let level = tx.add_datum(DatumElement::level("F2", 3600.0));
        let column = tx.add_element(Element::new(Category::generic(), "柱"));
        tx.add_relation(Relation::new(
            RelationKind::PositionedOn,
            column.into(),
            level.into(),
        ));

        let applied = tx.commit().unwrap();
        assert_eq!(applied.len(), 3);
        assert_eq!(doc.element_count(), 1);
        assert_eq!(doc.datums().len(), 1);
        assert_eq!(doc.relations().len(), 1);
        assert_eq!(doc.version(), 1, "一次提交只 +1 个版本");
    }

    #[test]
    fn empty_commit_is_a_noop() {
        let mut doc = Document::new("d");
        let tx = Transaction::begin(&mut doc);
        assert!(tx.is_empty());
        let applied = tx.commit().unwrap();
        assert!(applied.is_empty());
        assert_eq!(doc.version(), 0);
    }

    #[test]
    fn rollback_leaves_document_untouched() {
        let mut doc = Document::new("d");
        let mut tx = Transaction::begin(&mut doc);
        tx.add_element(Element::new(Category::generic(), "柱"));
        let dropped = tx.rollback();
        assert_eq!(dropped.len(), 1);
        assert_eq!(doc.element_count(), 0);
        assert_eq!(doc.version(), 0);
    }

    #[test]
    fn failing_commit_is_atomic() {
        let mut doc = Document::new("d");
        let existing = Element::new(Category::generic(), "已存在");
        let dup_id = existing.id;
        doc.insert_element(existing).unwrap();
        let version_before = doc.version();

        let mut tx = Transaction::begin(&mut doc);
        // 第一条合法
        tx.add_element(Element::new(Category::generic(), "新柱"));
        // 第二条 id 冲突 → 整个事务失败
        tx.add_element(Element {
            id: dup_id,
            ..Element::new(Category::generic(), "撞 id")
        });

        let err = tx.commit().unwrap_err();
        assert!(matches!(err, ModelError::DuplicateId(_)));
        assert_eq!(doc.element_count(), 1, "失败事务不得留下半成品");
        assert_eq!(doc.version(), version_before, "失败事务不推进版本");
    }

    #[test]
    fn geometry_and_view_registration_go_through_the_transaction() {
        let mut doc = Document::new("d");
        let view = ViewId::new();
        let shape = ShapeId::new();

        let mut tx = Transaction::begin(&mut doc);
        tx.add_geometry(GeometryRecord::brep(shape, "CASCADE"));
        tx.register_view(view);
        tx.commit().unwrap();

        assert!(doc.geometry_record(shape).is_some());
        assert!(doc.views().contains(&view));
    }

    #[test]
    fn delta_is_serializable_for_the_undo_stack() {
        let delta = Delta::InsertElement(Box::new(Element::new(Category::generic(), "柱")));
        let s = serde_json::to_string(&delta).unwrap();
        assert_eq!(serde_json::from_str::<Delta>(&s).unwrap(), delta);
    }
}

//! 文档服务：**同时持有文档与图纸**，并提供撤销 / 重做。
//!
//! ## 为什么这个服务必须同时持有两样东西
//!
//! 设计文档 4.1 把图纸本体（`View` / `Sheet`）从 `Document` 里挪了出去
//! （否则 `core-model` 就要依赖 `core-doc`）。代价是**两边必须有人配对管理**——
//! 这个"人"就是 [`DocumentService`]：
//!
//! ```text
//! DocumentService
//!   ├─ doc:     Document     模型：元素 / 几何 / 基准元素 / 关系
//!   └─ drawing: Drawing      图纸：视图 / 图纸 / 制图标准 / 关联索引
//!        ▲
//!        └── sync_drawing_registry() 保证 doc.views / doc.sheets 与 drawing 一致
//! ```
//!
//! ## 撤销的实现（与设计文档的差异，务必知悉）
//!
//! 设计文档 4.3 说"一次提交 = 一个撤销步"，出发点是 `Delta` 可序列化、省内存。
//! 但**只存 `Delta` 无法撤销**——撤销需要**反向补丁**（`Delta` 只有正向语义，
//! 且 `RemoveElement` 没有携带被删的元素内容）。
//!
//! 因此本实现采用**快照撤销**：每次成功编辑把编辑前的 `Document` **与 `Drawing`**
//! 各存一份（见 [`HistoryEntry`]）。
//!
//! | | 快照撤销（当前） | 反向补丁（将来） |
//! |---|---|---|
//! | 正确性 | 天然正确 | 需要为每个 `Delta` 写逆运算，**容易错** |
//! | 内存 | 大（文档级复制）| 小 |
//! | 适用规模 | 一阶段（文档通常 < 10 万元素）| 大文档 |
//!
//! **这是刻意的取舍**：先把正确性做对。反向补丁列为性能优化项，
//! 接口（[`DocumentService::undo`] / [`DocumentService::redo`]）不变；
//! 更细的 `EditScope`（只快照被改动的范围）也是后续优化——**先正确性，后性能**。
//!
//! ## M5：`Drawing` 已纳入历史（修复 §0.6）
//!
//! 此前（M5 之前）撤销**只回滚 `Document`、不回滚 `Drawing`**，导致"撤销后模型与图纸
//! 不一致"（图纸里留下一个已经不存在的元素对应的视图 / 图元）。M5 出图后 `Drawing`
//! 会频繁变更（视图重投、标注重标、关联索引重建），故历史项改为**同时保存两侧快照**：
//! `undo` 时文档与图纸一起回滚，`redo` 时一起前进。

use core_doc::Drawing;
use core_model::{Document, Element, ElementId, ShapeId};
use serde_json::Value as JsonValue;

use crate::error::{AppError, AppResult};

/// 默认的撤销层数上限。
pub const DEFAULT_UNDO_LIMIT: usize = 100;

/// 一次历史项：**同时**保存文档与图纸快照。
///
/// 取代原先的 `undo_stack: Vec<Document>`（修复 §0.6：撤销只回滚 `Document`
/// 导致模型与图纸不一致）。内存代价见文件末尾 TODO 讨论；
/// `EditScope`（只快照被改范围）是后续优化——先正确性，后性能。
#[derive(Debug, Clone)]
struct HistoryEntry {
    doc: Document,
    drawing: Drawing,
}

/// 文档服务。
pub struct DocumentService {
    doc: Document,
    drawing: Drawing,
    /// 撤销栈：每项是"编辑前"的文档 + 图纸快照
    undo_stack: Vec<HistoryEntry>,
    /// 重做栈
    redo_stack: Vec<HistoryEntry>,
    /// 撤销层数上限
    undo_limit: usize,
}

impl DocumentService {
    /// 新建一个空文档服务（图纸用国标工业厂房标准）。
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            doc: Document::new(name),
            drawing: Drawing::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            undo_limit: DEFAULT_UNDO_LIMIT,
        }
    }

    /// 新建一个空文档服务，并**注入既有图纸**（便于测试 / 从既有出图状态起步）。
    #[must_use]
    pub fn new_with_drawing(name: impl Into<String>, drawing: Drawing) -> Self {
        Self {
            doc: Document::new(name),
            drawing,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            undo_limit: DEFAULT_UNDO_LIMIT,
        }
    }

    /// 用指定的文档与图纸构造。
    #[must_use]
    pub fn with_parts(doc: Document, drawing: Drawing) -> Self {
        Self {
            doc,
            drawing,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            undo_limit: DEFAULT_UNDO_LIMIT,
        }
    }

    /// 设置撤销层数上限（链式）。
    #[must_use]
    pub fn with_undo_limit(mut self, limit: usize) -> Self {
        self.undo_limit = limit.max(1);
        if self.undo_stack.len() > self.undo_limit {
            let excess = self.undo_stack.len() - self.undo_limit;
            self.undo_stack.drain(0..excess);
        }
        self
    }

    // ---------- 只读访问 ----------

    /// 文档（只读）。
    #[must_use]
    pub fn doc(&self) -> &Document {
        &self.doc
    }

    /// 图纸（只读）。
    #[must_use]
    pub fn drawing(&self) -> &Drawing {
        &self.drawing
    }

    /// 文档版本号。
    #[must_use]
    pub fn version(&self) -> u64 {
        self.doc.version()
    }

    /// 元素数量。
    #[must_use]
    pub fn element_count(&self) -> usize {
        self.doc.element_count()
    }

    // ---------- 编辑（带撤销） ----------

    /// **在一个可撤销的编辑里改文档**。
    ///
    /// 语义：
    /// - `f` 返回 `Ok` → 记为**一个撤销步**，清空重做栈；
    /// - `f` 返回 `Err` → **文档恢复到编辑前**（快照回滚），错误原样返回。
    ///
    /// ## 用法
    ///
    /// 需要"改多处、要么全成"时，在 `f` 内部开一个事务：
    ///
    /// ```no_run
    /// # use core_app::DocumentService;
    /// # use core_sdk::{Element, Category, Transaction};
    /// let mut svc = DocumentService::new("demo");
    /// svc.edit(|doc| {
    ///     let mut tx = Transaction::begin(doc);
    ///     tx.add_element(Element::new(Category::generic(), "柱"));
    ///     tx.add_element(Element::new(Category::generic(), "梁"));
    ///     tx.commit()?;
    ///     Ok(())
    /// })
    /// .unwrap();
    /// assert_eq!(svc.element_count(), 2);
    /// ```
    pub fn edit<F>(&mut self, f: F) -> AppResult<()>
    where
        F: FnOnce(&mut Document) -> AppResult<()>,
    {
        let snapshot = HistoryEntry {
            doc: self.doc.clone(),
            drawing: self.drawing.clone(),
        };
        if let Err(e) = f(&mut self.doc) {
            // 回滚（含版本号）：文档与图纸一并恢复
            self.doc = snapshot.doc;
            self.drawing = snapshot.drawing;
            return Err(e);
        }
        self.push_undo(snapshot);
        Ok(())
    }

    /// 便捷：加一个元素（自动成为一个撤销步）。
    pub fn add_element(&mut self, element: Element) -> AppResult<ElementId> {
        let mut id_slot: Option<ElementId> = None;
        self.edit(|doc| {
            id_slot = Some(doc.insert_element(element)?);
            Ok(())
        })?;
        id_slot.ok_or_else(|| {
            AppError::Model(core_model::ModelError::ElementNotFound(ElementId::new()))
        })
    }

    /// 便捷：删一个元素（自动成为一个撤销步，并级联清关系）。
    pub fn remove_element(&mut self, id: ElementId) -> AppResult<()> {
        self.edit(|doc| {
            doc.remove_element(id)?;
            Ok(())
        })
    }

    /// 便捷：登记 / 对齐视图与图纸的 id 集合。
    pub fn sync_drawing_registry(&mut self) -> AppResult<(usize, usize)> {
        let mut counts = (0usize, 0usize);
        let drawing = self.drawing.clone();
        self.edit(|doc| {
            counts = drawing.sync_registry(doc);
            Ok(())
        })?;
        Ok(counts)
    }

    /// 图纸可变引用（**本身不产生撤销步**）。
    ///
    /// 改动图纸不会立刻压入历史；但 `DocumentService` 的历史项**同时**保存图纸快照
    /// （见模块文档 §M5），故随后一次 `undo` 会把图纸**一并回滚**到对应快照。
    ///
    /// 改完图纸后请调 [`DocumentService::rebuild_dependency_index`]，
    /// 否则关联更新索引会与图纸内容不一致。
    pub fn drawing_mut(&mut self) -> &mut Drawing {
        &mut self.drawing
    }

    /// **同时**读文档（只读）+ 取图纸（可变）——投影写回的借用形状。
    ///
    /// [`DocumentService::doc`]（`&Document`）与 [`DocumentService::drawing_mut`]
    /// （`&mut Drawing`）不能同时借（同一服务的 `&self` / `&mut self` 两条路）；
    /// 投影写回恰恰需要"读模型 + 改图纸"两件事一起做（投影要从 `Document`
    /// 装载 BREP、结果要写回 `Drawing`），故给一个组合访问器：两个字段
    /// **同一次**分离借用交给闭包。
    pub fn with_doc_and_drawing_mut<R>(
        &mut self,
        f: impl FnOnce(&Document, &mut Drawing) -> R,
    ) -> R {
        f(&self.doc, &mut self.drawing)
    }

    /// 重建关联更新索引（图纸内容改动后必须调）。
    pub fn rebuild_dependency_index(&mut self) {
        self.drawing.rebuild_index();
    }

    // ---------- 撤销 / 重做 ----------

    fn push_undo(&mut self, snapshot: HistoryEntry) {
        self.undo_stack.push(snapshot);
        if self.undo_stack.len() > self.undo_limit {
            let excess = self.undo_stack.len() - self.undo_limit;
            self.undo_stack.drain(0..excess);
        }
        self.redo_stack.clear();
    }

    /// 能否撤销。
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    /// 能否重做。
    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// 撤销层数。
    #[must_use]
    pub fn undo_depth(&self) -> usize {
        self.undo_stack.len()
    }

    /// 重做层数。
    #[must_use]
    pub fn redo_depth(&self) -> usize {
        self.redo_stack.len()
    }

    /// 撤销一步（**文档与图纸一起回滚**）。返回是否真的撤销了。
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo_stack.pop() else {
            return false;
        };
        let current = HistoryEntry {
            doc: std::mem::replace(&mut self.doc, previous.doc),
            drawing: std::mem::replace(&mut self.drawing, previous.drawing),
        };
        self.redo_stack.push(current);
        true
    }

    /// 重做一步（**文档与图纸一起前进**）。返回是否真的重做了。
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo_stack.pop() else {
            return false;
        };
        let current = HistoryEntry {
            doc: std::mem::replace(&mut self.doc, next.doc),
            drawing: std::mem::replace(&mut self.drawing, next.drawing),
        };
        self.undo_stack.push(current);
        true
    }

    /// 清空历史（存盘后调）。
    pub fn clear_history(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    // ---------- 持久化（.bimdoc 的语义部分） ----------

    /// 把**语义部分**序列化成 JSON（= `.bimdoc` 里的 `doc.json`）。
    ///
    /// 几何正文不在这里——它按设计文档 4.2 分文件存在
    /// `geom/<shapeid>.brep`。打包成 zip 是 I/O 层的活（M2）。
    pub fn to_json(&self) -> AppResult<String> {
        serde_json::to_string_pretty(&self.doc)
            .map_err(|e| AppError::Model(core_model::ModelError::Serde(e.to_string())))
    }

    /// 从 `doc.json` 内容恢复。
    pub fn from_json(json: &str) -> AppResult<Self> {
        let doc: Document = serde_json::from_str(json)
            .map_err(|e| AppError::Model(core_model::ModelError::Serde(e.to_string())))?;
        Ok(Self::new_from_doc(doc))
    }

    fn new_from_doc(doc: Document) -> Self {
        Self {
            doc,
            drawing: Drawing::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            undo_limit: DEFAULT_UNDO_LIMIT,
        }
    }

    /// 文档摘要（界面状态栏 / 诊断用）。
    #[must_use]
    pub fn summary(&self) -> JsonValue {
        serde_json::json!({
            "name": self.doc.meta().name,
            "version": self.doc.version(),
            "elements": self.doc.element_count(),
            "datums": self.doc.datums().len(),
            "geometry": self.doc.geometry_count(),
            "relations": self.doc.relations().len(),
            "views": self.drawing.view_count(),
            "sheets": self.drawing.sheet_count(),
            "undo_depth": self.undo_stack.len(),
        })
    }

    /// 某元素引用的几何记录是否存在（数据完整性自查）。
    #[must_use]
    pub fn has_dangling_geometry(&self, id: ElementId) -> bool {
        match self.doc.element(id) {
            Some(e) => e.geometry_ref.is_some_and(|s| !self.has_geometry(s)),
            None => false,
        }
    }

    fn has_geometry(&self, shape: ShapeId) -> bool {
        self.doc.geometry_record(shape).is_some()
    }
}

// TODO(M5+)：把"全量两侧快照"细化为按范围的增量。
//   已做：历史项 `HistoryEntry { doc, drawing }` 同时存两侧快照，撤销 / 重做两栈同步
//         （修复 §0.6 的"撤销后模型与图纸不一致"）。
//   待做（性能）：`enum EditScope { Doc, Drawing, Both }`——只快照本次编辑实际改动的范围，
//         避免每次编辑都整份复制 `Drawing`（出图后图纸可能很大）。
//   原则：先正确性，后性能。

#[cfg(test)]
mod tests {
    use super::*;
    use core_doc::{Entity2D, Placement, Sheet, SheetFormat, View, ViewKind};
    use core_model::{Category, DatumElement, LayerId, Value};
    use core_sdk::Transaction;
    use glam::DVec2;

    #[test]
    fn a_new_service_is_empty() {
        let svc = DocumentService::new("变电站-A");
        assert_eq!(svc.element_count(), 0);
        assert_eq!(svc.version(), 0);
        assert!(!svc.can_undo());
        assert!(!svc.can_redo());
    }

    #[test]
    fn an_edit_becomes_one_undo_step() {
        let mut svc = DocumentService::new("d");
        svc.edit(|doc| {
            let mut tx = Transaction::begin(doc);
            tx.add_element(Element::new(Category::generic(), "柱"));
            tx.commit()?;
            Ok(())
        })
        .unwrap();

        assert_eq!(svc.element_count(), 1);
        assert_eq!(svc.undo_depth(), 1);
        assert!(svc.can_undo());
    }

    #[test]
    fn adding_and_removing_via_convenience_helpers() {
        let mut svc = DocumentService::new("d");
        let id = svc
            .add_element(Element::new(Category::generic(), "柱"))
            .unwrap();
        assert_eq!(svc.element_count(), 1);

        svc.remove_element(id).unwrap();
        assert_eq!(svc.element_count(), 0);
        assert_eq!(svc.undo_depth(), 2);
    }

    #[test]
    fn undo_and_redo_round_trip() {
        let mut svc = DocumentService::new("d");
        svc.add_element(Element::new(Category::generic(), "柱"))
            .unwrap();
        assert_eq!(svc.element_count(), 1);

        assert!(svc.undo());
        assert_eq!(svc.element_count(), 0);
        assert!(svc.can_redo());

        assert!(svc.redo());
        assert_eq!(svc.element_count(), 1);
        assert!(!svc.can_redo());
    }

    #[test]
    fn undo_restores_drawing_changes_too() {
        let mut svc = DocumentService::new("d");
        // 先有一个撤销步（其快照包含"空图纸"）
        svc.add_element(Element::new(Category::generic(), "柱"))
            .unwrap();
        // 该步之后改动图纸：加一个带图元的视图
        let mut view = View::new("手绘", ViewKind::Drafting);
        view.push_entity(Entity2D::circle(LayerId::new(), DVec2::ZERO, 5.0));
        let view_id = svc.drawing_mut().add_view(view);
        assert_eq!(svc.drawing().view_count(), 1);
        assert!(svc.drawing().view(view_id).is_some());

        // 撤销：模型与图纸必须**一起**回滚（修复 §0.6 的已知边界）
        assert!(svc.undo());
        assert_eq!(svc.element_count(), 0, "文档回滚");
        assert_eq!(svc.drawing().view_count(), 0, "图纸也要回滚");
        assert!(
            svc.drawing().view(view_id).is_none(),
            "视图连同其图元一起消失"
        );
    }

    #[test]
    fn redo_restores_drawing_changes_too() {
        let mut svc = DocumentService::new("d");
        svc.add_element(Element::new(Category::generic(), "柱"))
            .unwrap();
        let mut view = View::new("手绘", ViewKind::Drafting);
        view.push_entity(Entity2D::circle(LayerId::new(), DVec2::ZERO, 5.0));
        let view_id = svc.drawing_mut().add_view(view);

        assert!(svc.undo());
        assert_eq!(svc.element_count(), 0);
        assert_eq!(svc.drawing().view_count(), 0);

        // 重做：模型与图纸必须**一起**前进
        assert!(svc.redo());
        assert_eq!(svc.element_count(), 1, "文档前进");
        assert_eq!(svc.drawing().view_count(), 1, "图纸也前进");
        assert!(svc.drawing().view(view_id).is_some(), "视图重新出现");
    }

    #[test]
    fn undo_on_empty_history_is_a_noop() {
        let mut svc = DocumentService::new("d");
        assert!(!svc.undo());
        assert!(!svc.redo());
        assert_eq!(svc.element_count(), 0);
    }

    #[test]
    fn a_failing_edit_rolls_the_document_back() {
        let mut svc = DocumentService::new("d");
        svc.add_element(Element::new(Category::generic(), "已存在"))
            .unwrap();
        let version_before = svc.version();
        let depth_before = svc.undo_depth();

        let err = svc
            .edit(|doc| {
                // 先改一处（成功）
                doc.insert_datum(DatumElement::level("F2", 3600.0))?;
                // 再制造一个失败
                Err(AppError::RecipeNotExecutable("故意的失败".to_string()))
            })
            .unwrap_err();

        assert!(matches!(err, AppError::RecipeNotExecutable(_)));
        assert_eq!(svc.element_count(), 1, "失败编辑不得留下半成品");
        assert!(svc.doc().datums().is_empty(), "已改的部分也要回滚");
        assert_eq!(svc.version(), version_before, "版本号也要回滚");
        assert_eq!(svc.undo_depth(), depth_before, "失败编辑不产生撤销步");
    }

    #[test]
    fn a_new_edit_clears_the_redo_stack() {
        let mut svc = DocumentService::new("d");
        svc.add_element(Element::new(Category::generic(), "A"))
            .unwrap();
        svc.undo();
        assert!(svc.can_redo());

        svc.add_element(Element::new(Category::generic(), "B"))
            .unwrap();
        assert!(!svc.can_redo(), "新编辑后旧的重做分支必须丢弃");
    }

    #[test]
    fn undo_limit_drops_the_oldest_steps() {
        let mut svc = DocumentService::new("d").with_undo_limit(3);
        for i in 0..6 {
            svc.add_element(Element::new(Category::generic(), format!("柱-{i}")))
                .unwrap();
        }
        assert_eq!(svc.undo_depth(), 3, "超出上限的最老步骤应被丢弃");
        assert_eq!(svc.element_count(), 6);
    }

    #[test]
    fn clear_history_empties_both_stacks() {
        let mut svc = DocumentService::new("d");
        svc.add_element(Element::new(Category::generic(), "A"))
            .unwrap();
        svc.undo();
        svc.clear_history();
        assert!(!svc.can_undo() && !svc.can_redo());
    }

    #[test]
    fn sync_drawing_registry_aligns_both_sides() {
        let mut svc = DocumentService::new("d");
        let view = View::plan("一层平面图", core_model::ElementId::new(), 0.0);
        let view_id = view.id;
        svc.drawing_mut().add_view(view);
        let sheet = Sheet::new("一层平面", "建施-01", SheetFormat::A2);
        let sheet_id = sheet.id;
        svc.drawing_mut().add_sheet(sheet);

        let (added, removed) = svc.sync_drawing_registry().unwrap();
        assert_eq!((added, removed), (2, 0));
        assert!(svc.doc().views().contains(&view_id));
        assert!(svc.doc().sheets().contains(&sheet_id));
    }

    #[test]
    fn rebuild_dependency_index_picks_up_view_content() {
        let mut svc = DocumentService::new("d");
        let mut view = View::new("手绘", ViewKind::Drafting);
        view.push_entity(Entity2D::circle(LayerId::new(), DVec2::ZERO, 5.0));
        svc.drawing_mut().add_view(view);
        svc.rebuild_dependency_index();
        assert_eq!(
            svc.drawing().dependencies.entity_count(),
            0,
            "圆未绑定模型几何"
        );
    }

    #[test]
    fn json_round_trip_preserves_the_document() {
        let mut svc = DocumentService::new("往返");
        svc.add_element(Element::new(Category::generic(), "柱"))
            .unwrap();
        svc.edit(|doc| {
            doc.insert_datum(DatumElement::level_with_slab("F1", 0.0, 30.0))?;
            Ok(())
        })
        .unwrap();

        let json = svc.to_json().unwrap();
        let back = DocumentService::from_json(&json).unwrap();
        assert_eq!(back.doc(), svc.doc());
        assert_eq!(back.element_count(), 1);
        assert!(!back.can_undo(), "从文件恢复后不应有历史");
    }

    #[test]
    fn from_json_reports_bad_input() {
        assert!(DocumentService::from_json("{ not json").is_err());
    }

    #[test]
    fn summary_reports_the_important_counts() {
        let mut svc = DocumentService::new("变电站");
        svc.add_element(Element::new(Category::generic(), "柱"))
            .unwrap();
        let s = svc.summary();
        assert_eq!(s["name"], "变电站");
        assert_eq!(s["elements"], 1);
        assert_eq!(s["undo_depth"], 1);
    }

    #[test]
    fn dangling_geometry_can_be_detected() {
        let mut svc = DocumentService::new("d");
        let phantom = ShapeId::new();
        let id = svc
            .add_element(Element::new(Category::generic(), "悬空几何").with_geometry(phantom))
            .unwrap();
        assert!(svc.has_dangling_geometry(id), "几何记录不存在 → 悬空");

        svc.edit(|doc| {
            doc.insert_geometry(core_model::GeometryRecord::brep(phantom, "t"))?;
            Ok(())
        })
        .unwrap();
        assert!(!svc.has_dangling_geometry(id));
    }

    #[test]
    fn unknown_element_is_not_reported_as_dangling() {
        let svc = DocumentService::new("d");
        assert!(!svc.has_dangling_geometry(ElementId::new()));
    }

    #[test]
    fn sheet_placement_survives_the_service_layer() {
        let mut svc = DocumentService::new("d");
        let view = View::plan("一层", core_model::ElementId::new(), 0.0);
        let view_id = view.id;
        svc.drawing_mut().add_view(view);
        let mut sheet = Sheet::new("平面", "建施-01", SheetFormat::A2);
        sheet.place(Placement::new(view_id, DVec2::new(250.0, 300.0)));
        svc.drawing_mut().add_sheet(sheet);
        svc.sync_drawing_registry().unwrap();

        assert_eq!(svc.drawing().view_count(), 1);
        assert_eq!(svc.drawing().sheet_count(), 1);
        assert!(svc.drawing().validate().is_ok());
    }

    #[test]
    fn edit_can_store_properties_on_an_element() {
        let mut svc = DocumentService::new("d");
        let mut element = Element::new(Category::new("TRAY"), "桥架-1");
        element.properties.set("width", Value::Float(600.0));
        let id = svc.add_element(element).unwrap();
        assert_eq!(
            svc.doc().element(id).unwrap().properties.get_float("width"),
            Some(600.0)
        );
    }
}

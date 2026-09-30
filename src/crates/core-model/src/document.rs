//! 文档模型（设计文档 3.3 / 4.1 / 4.2）。
//!
//! [`Document`] 是**能力①的落点**：一个 `.bimdoc` 打开后内存里的那个对象。
//! 它**只有数据、没有算法**，也**不持有任何内核句柄**。
//!
//! ## 内存布局（与 `.bimdoc` 文件布局一一对应）
//!
//! | 内存字段 | `.bimdoc` 中的位置 | 说明 |
//! |---|---|---|
//! | [`Document::meta`] / `elements` / `datums` / `relations` | `doc.json` | 语义层，可读可 diff |
//! | [`Document::geometry`] | `geom/<shapeid>.brep` | 几何正文分文件存（设计文档 4.2）|
//! | [`Document::views`] / [`Document::sheets`] | `views/*.json` | **只有 id**；本体在 `core-doc` |
//!
//! ## 为什么 `views` / `sheets` 只有 id
//!
//! 视图与图纸的本体（`View` / `Sheet` / `Entity2D` / `Annotation`）属于第 3 层
//! `core-doc`。若 `Document` 直接持有它们，`core-model` 就得依赖 `core-doc`，
//! 破坏"数据层零依赖"。所以这里只留**引用**，由 `core-app` 的图纸库配对管理。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::datum::{DatumElement, DatumKind};
use crate::element::Element;
use crate::error::{ModelError, ModelResult};
use crate::geometry::GeometryRecord;
use crate::ids::{ElementId, RelationId, ShapeId, SheetId, ViewId};
use crate::relation::{Endpoint, Relation};

/// 约定单位：长度一律毫米（设计文档 2.5）。
pub const UNIT_MM: &str = "mm";

/// 约定坐标系：Z 轴朝上右手系（设计文档 2.5）。
pub const COORDINATE_SYSTEM: &str = "Z-up right-handed";

/// 文档元信息（写进 `doc.json` 的 `meta` 段）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    /// 文档名
    pub name: String,
    /// 长度单位（约定 [`UNIT_MM`]）
    pub unit: String,
    /// 坐标系（约定 [`COORDINATE_SYSTEM`]）
    pub coordinate_system: String,
    /// 作者
    pub author: String,
    /// 写入本文件的应用版本
    ///
    /// ⚠ **占位**：正式版应由 `core-app` 传入自身版本号，这里先用本 crate 的版本兜底。
    pub app_version: String,
    /// 创建时间（ISO 8601 / UTC）
    pub created_utc: String,
    /// 最近修改时间（ISO 8601 / UTC）
    pub modified_utc: String,
}

impl Meta {
    /// 以文档名构造一份默认元信息。
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            unit: UNIT_MM.to_string(),
            coordinate_system: COORDINATE_SYSTEM.to_string(),
            author: String::new(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            created_utc: String::new(),
            modified_utc: String::new(),
        }
    }

    /// 指定作者。
    #[must_use]
    pub fn with_author(mut self, author: impl Into<String>) -> Self {
        self.author = author.into();
        self
    }
}

/// 一份 BIM 文档。
///
/// 所有集合都用 `BTreeMap` / `BTreeSet` 而非 `HashMap`：**序列化结果稳定**，
/// 同样的文档每次存盘字节一致，`git diff` 才有意义（设计文档 4.2 的"可 diff"目标）。
///
/// ## 封装纪律（「变更必须经由事务」的编译期保证）
///
/// **全部字段均私有**：外部只能经只读访问器（[`meta`](Self::meta) /
/// [`elements`](Self::elements) / …）读取，任何改动一律走命名方法
/// （`insert_*` / `remove_*` / `register_*` / [`remove_geometry`](Self::remove_geometry)）。
/// 这样便**从编译期**堵死了"绕过事务直接改文档字段"这条脏路径。
///
/// `Serialize` / `Deserialize` / `Clone` / `Debug` / `PartialEq` 的 derive 宏在**本模块内**
/// 展开、可直接读写私有字段，故字段私有化**不影响** .bimdoc 的持久化与往返。
///
/// 回归钉子见 [`tests::document_fields_stay_private_source_guard`]（**源码级守卫**）：
/// 一旦有人把任一字段改回 `pub`（含 `pub(crate)` / `pub(super)` 等变形），该守卫立即变红。
/// 之所以不用 compile_fail doctest，是因为本沙箱无法运行它（根因详见该测试的注释）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    /// 元信息
    meta: Meta,
    /// 元素表
    elements: BTreeMap<ElementId, Element>,
    /// 基准元素表（轴网 / 标高 / 参考面 / 参考线，设计文档 4.6）
    datums: BTreeMap<ElementId, DatumElement>,
    /// 几何记录表（对应 `geom/*.brep`）
    geometry: BTreeMap<ShapeId, GeometryRecord>,
    /// 关系表
    relations: BTreeMap<RelationId, Relation>,
    /// 视图 id 集合（本体在 `core-doc`）
    views: BTreeSet<ViewId>,
    /// 图纸 id 集合（本体在 `core-doc`）
    sheets: BTreeSet<SheetId>,
    /// 单调递增的版本号（每次提交 +1，供关联更新判定"模型变了"）
    version: u64,
}

impl Document {
    /// 新建一份空文档。
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            meta: Meta::new(name),
            elements: BTreeMap::new(),
            datums: BTreeMap::new(),
            geometry: BTreeMap::new(),
            relations: BTreeMap::new(),
            views: BTreeSet::new(),
            sheets: BTreeSet::new(),
            version: 0,
        }
    }

    // ---------- 只读访问器（字段私有化后的唯一读取通道） ----------
    //
    // 纪律：这里**只提供 `&self` 只读访问器**，一个 `*_mut` 都不给。任何修改
    // 都必须走命名方法（`insert_*` / `remove_*` / `register_*` …），从而把"改动
    // 文档"收口到事务路径上。`element_mut` 返回的是"元素实体"的可变借用、并非
    // 文档字段，故不违反本纪律，予以保留。

    /// 元信息。
    #[must_use]
    pub fn meta(&self) -> &Meta {
        &self.meta
    }

    /// 元素表（只读）。
    #[must_use]
    pub fn elements(&self) -> &BTreeMap<ElementId, Element> {
        &self.elements
    }

    /// 基准元素表（只读）。
    #[must_use]
    pub fn datums(&self) -> &BTreeMap<ElementId, DatumElement> {
        &self.datums
    }

    /// 几何记录表（只读）。
    #[must_use]
    pub fn geometry(&self) -> &BTreeMap<ShapeId, GeometryRecord> {
        &self.geometry
    }

    /// 关系表（只读）。
    #[must_use]
    pub fn relations(&self) -> &BTreeMap<RelationId, Relation> {
        &self.relations
    }

    /// 视图 id 集合（只读）。
    #[must_use]
    pub fn views(&self) -> &BTreeSet<ViewId> {
        &self.views
    }

    /// 图纸 id 集合（只读）。
    #[must_use]
    pub fn sheets(&self) -> &BTreeSet<SheetId> {
        &self.sheets
    }

    /// 版本号。
    #[must_use]
    pub fn version(&self) -> u64 {
        self.version
    }

    /// 几何记录数量（等价 `self.geometry().len()`，语义更清晰）。
    #[must_use]
    pub fn geometry_count(&self) -> usize {
        self.geometry.len()
    }

    /// 版本号 +1（由事务提交调用）。
    pub(crate) fn bump_version(&mut self) {
        self.version = self.version.saturating_add(1);
    }

    // ---------- 元素 ----------

    /// 插入一个元素。**持久 id 冲突直接报错**（设计文档 3.3：id 永不复用）。
    pub fn insert_element(&mut self, element: Element) -> ModelResult<ElementId> {
        let id = element.id;
        if self.elements.contains_key(&id) {
            return Err(ModelError::DuplicateId(id.to_string()));
        }
        self.elements.insert(id, element);
        Ok(id)
    }

    /// 查元素。
    #[must_use]
    pub fn element(&self, id: ElementId) -> Option<&Element> {
        self.elements.get(&id)
    }

    /// 改元素（不存在则报错）。
    pub fn element_mut(&mut self, id: ElementId) -> ModelResult<&mut Element> {
        self.elements
            .get_mut(&id)
            .ok_or(ModelError::ElementNotFound(id))
    }

    /// 删元素，并**级联清掉所有触及它的关系**（防悬空引用）。
    pub fn remove_element(&mut self, id: ElementId) -> ModelResult<Element> {
        let element = self
            .elements
            .remove(&id)
            .ok_or(ModelError::ElementNotFound(id))?;
        let endpoint = Endpoint::Element(id);
        self.relations.retain(|_, r| !r.touches(endpoint));
        Ok(element)
    }

    /// 元素数量。
    #[must_use]
    pub fn element_count(&self) -> usize {
        self.elements.len()
    }

    // ---------- 几何 ----------

    /// 插入一份几何记录。
    pub fn insert_geometry(&mut self, record: GeometryRecord) -> ModelResult<ShapeId> {
        let id = record.shape_id;
        if self.geometry.contains_key(&id) {
            return Err(ModelError::DuplicateId(id.to_string()));
        }
        self.geometry.insert(id, record);
        Ok(id)
    }

    /// 取几何记录。
    #[must_use]
    pub fn geometry_record(&self, id: ShapeId) -> Option<&GeometryRecord> {
        self.geometry.get(&id)
    }

    /// 删除一份几何记录；不存在返回 `None`（**兼容既有语义，不报错**）。
    ///
    /// 这是"受控修改"通道：补上原先被外部直接 `doc.geometry.remove(...)` 绕过
    /// 事务的那条路径（那个绕过点位于 `core-app` 的几何服务）。
    pub fn remove_geometry(&mut self, id: ShapeId) -> Option<GeometryRecord> {
        self.geometry.remove(&id)
    }

    /// 取元素所引用的几何记录（无几何或引用悬空时为 `None`）。
    #[must_use]
    pub fn geometry_of_element(&self, id: ElementId) -> Option<&GeometryRecord> {
        let shape = self.elements.get(&id)?.geometry_ref?;
        self.geometry.get(&shape)
    }

    /// 删除无人引用的几何记录，返回被删的数量（垃圾回收用）。
    pub fn collect_unreferenced_geometry(&mut self) -> usize {
        let referenced: BTreeSet<ShapeId> = self
            .elements
            .values()
            .filter_map(|e| e.geometry_ref)
            .chain(self.datums.values().filter_map(|d| d.geom_ref))
            .collect();
        let before = self.geometry.len();
        self.geometry.retain(|id, _| referenced.contains(id));
        before - self.geometry.len()
    }

    // ---------- 基准元素 ----------

    /// 插入一个基准元素。
    pub fn insert_datum(&mut self, datum: DatumElement) -> ModelResult<ElementId> {
        let id = datum.id;
        if self.datums.contains_key(&id) {
            return Err(ModelError::DuplicateId(id.to_string()));
        }
        self.datums.insert(id, datum);
        Ok(id)
    }

    /// 查基准元素。
    #[must_use]
    pub fn datum(&self, id: ElementId) -> Option<&DatumElement> {
        self.datums.get(&id)
    }

    /// 删基准元素，并级联清掉触及它的关系。
    pub fn remove_datum(&mut self, id: ElementId) -> ModelResult<DatumElement> {
        let datum = self
            .datums
            .remove(&id)
            .ok_or(ModelError::ElementNotFound(id))?;
        let endpoint = Endpoint::Element(id);
        self.relations.retain(|_, r| !r.touches(endpoint));
        Ok(datum)
    }

    /// 某一种类的全部基准元素（按 id 排序）。
    pub fn datums_of_kind(&self, kind: DatumKind) -> Vec<&DatumElement> {
        self.datums.values().filter(|d| d.is_kind(kind)).collect()
    }

    /// **需要写盘的**基准元素（剔除 [`DatumScope::Session`]）。
    ///
    /// 设计文档 4.6.3：像"设为当前工作平面"这类纯会话状态**不进 `.bimdoc`**。
    pub fn persistent_datums(&self) -> impl Iterator<Item = &DatumElement> {
        self.datums.values().filter(|d| !d.is_session_scoped())
    }

    /// **不写盘的**基准元素（仅会话态，打开新会话即丢弃）。
    pub fn session_datums(&self) -> Vec<&DatumElement> {
        self.datums
            .values()
            .filter(|d| d.is_session_scoped())
            .collect()
    }

    /// **可持久化的副本**：剔除全部 [`DatumScope::Session`] 基准元素（设计文档 4.6.3）。
    ///
    /// 像"设为当前工作平面"这类**纯会话状态不进 `.bimdoc`**。容器写盘
    /// （`doc.json`）之前必须先过这一道，否则会话态会随文件泄漏出去——
    /// 下次打开时会凭空多出一个本不存在的基准元素。
    ///
    /// 与 [`persistent_datums`](Self::persistent_datums) 的关系：后者给出**只读
    /// 视图**，本方法给出**可写盘的整份文档**（其余字段——元素 / 几何 / 关系 /
    /// 视图 id / 版本号——原样保留）。
    #[must_use]
    pub fn to_persistable(&self) -> Self {
        let mut out = self.clone();
        out.datums.retain(|_, d| !d.is_session_scoped());
        out
    }

    // ---------- 关系 ----------

    /// 插入一条关系。
    pub fn insert_relation(&mut self, relation: Relation) -> ModelResult<RelationId> {
        let id = relation.id;
        if self.relations.contains_key(&id) {
            return Err(ModelError::DuplicateId(id.to_string()));
        }
        self.relations.insert(id, relation);
        Ok(id)
    }

    /// 查关系。
    #[must_use]
    pub fn relation(&self, id: RelationId) -> Option<&Relation> {
        self.relations.get(&id)
    }

    /// 删关系。
    pub fn remove_relation(&mut self, id: RelationId) -> ModelResult<Relation> {
        self.relations
            .remove(&id)
            .ok_or_else(|| ModelError::Relation(format!("关系不存在：{id}")))
    }

    /// 所有触及某端点的关系（关联更新的起点，设计文档 6.4）。
    #[must_use]
    pub fn relations_touching(&self, endpoint: Endpoint) -> Vec<&Relation> {
        self.relations
            .values()
            .filter(|r| r.touches(endpoint))
            .collect()
    }

    /// "定位在"某基准元素上的元素（反向查 [`crate::RelationKind::PositionedOn`]）。
    #[must_use]
    pub fn elements_positioned_on(&self, datum: ElementId) -> Vec<&Element> {
        let target = Endpoint::Element(datum);
        self.relations
            .values()
            .filter(|r| r.to == target)
            .filter_map(|r| match r.from {
                Endpoint::Element(id) => self.elements.get(&id),
                _ => None,
            })
            .collect()
    }

    // ---------- 视图 / 图纸（只登记 id） ----------

    /// 登记一个视图 id。
    pub fn register_view(&mut self, id: ViewId) -> bool {
        self.views.insert(id)
    }

    /// 登记一个图纸 id。
    pub fn register_sheet(&mut self, id: SheetId) -> bool {
        self.sheets.insert(id)
    }

    /// 注销一个视图 id。
    pub fn unregister_view(&mut self, id: ViewId) -> bool {
        self.views.remove(&id)
    }

    /// 注销一个图纸 id。
    pub fn unregister_sheet(&mut self, id: SheetId) -> bool {
        self.sheets.remove(&id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datum::DatumScope;
    use crate::element::Category;
    use crate::relation::RelationKind;

    #[test]
    fn new_document_is_empty_and_versioned_zero() {
        let doc = Document::new("变电站-A");
        assert_eq!(doc.meta.name, "变电站-A");
        assert_eq!(doc.meta.unit, UNIT_MM);
        assert_eq!(doc.element_count(), 0);
        assert_eq!(doc.version, 0);
    }

    #[test]
    fn duplicate_persistent_id_is_rejected() {
        let mut doc = Document::new("d");
        let e = Element::new(Category::generic(), "柱");
        let id = e.id;
        doc.insert_element(e).unwrap();
        let dup = Element {
            id,
            ..Element::new(Category::generic(), "柱-复制")
        };
        assert!(matches!(
            doc.insert_element(dup),
            Err(ModelError::DuplicateId(_))
        ));
    }

    #[test]
    fn removing_element_cascades_its_relations() {
        let mut doc = Document::new("d");
        let column = Element::new(Category::generic(), "柱");
        let column_id = column.id;
        doc.insert_element(column).unwrap();

        let level = DatumElement::level("F2", 3600.0);
        let level_id = level.id;
        doc.insert_datum(level).unwrap();

        doc.insert_relation(Relation::new(
            RelationKind::PositionedOn,
            column_id.into(),
            level_id.into(),
        ))
        .unwrap();
        assert_eq!(doc.relations.len(), 1);

        doc.remove_element(column_id).unwrap();
        assert!(doc.relations.is_empty(), "删元素必须级联清关系");
    }

    #[test]
    fn session_datums_are_excluded_from_persistence() {
        let mut doc = Document::new("d");
        doc.insert_datum(DatumElement::level("F1", 0.0)).unwrap();
        let mut work_plane = DatumElement::ref_plane("当前工作平面");
        work_plane.scope = DatumScope::Session;
        doc.insert_datum(work_plane).unwrap();

        assert_eq!(doc.persistent_datums().count(), 1, "只应剩文档级标高");
        assert_eq!(doc.session_datums().len(), 1, "工作平面属会话态");
    }

    #[test]
    fn to_persistable_drops_session_scoped_datums_but_keeps_the_rest() {
        let mut doc = Document::new("d");
        doc.insert_datum(DatumElement::level("F1", 0.0)).unwrap();
        let mut work_plane = DatumElement::ref_plane("当前工作平面");
        work_plane.scope = DatumScope::Session;
        doc.insert_datum(work_plane).unwrap();

        let shape = ShapeId::new();
        doc.insert_geometry(GeometryRecord::brep(shape, "t"))
            .unwrap();
        doc.insert_element(Element::new(Category::generic(), "柱").with_geometry(shape))
            .unwrap();

        let persistable = doc.to_persistable();
        assert_eq!(persistable.datums().len(), doc.persistent_datums().count());
        assert_eq!(persistable.datums().len(), 1, "只应剩文档级标高");
        assert!(persistable.session_datums().is_empty());
        // 其余字段原样保留（会话态只影响 datums 一张表）
        assert_eq!(persistable.elements(), doc.elements());
        assert_eq!(persistable.geometry(), doc.geometry());
        assert_eq!(persistable.relations(), doc.relations());
        assert_eq!(persistable.meta(), doc.meta());
        assert_eq!(persistable.version(), doc.version());
    }

    #[test]
    fn to_persistable_is_identity_without_session_datums() {
        let mut doc = Document::new("d");
        doc.insert_datum(DatumElement::level("F1", 0.0)).unwrap();
        assert_eq!(doc.to_persistable(), doc);
    }

    #[test]
    fn datums_can_be_queried_by_kind() {
        let mut doc = Document::new("d");
        doc.insert_datum(DatumElement::grid("1")).unwrap();
        doc.insert_datum(DatumElement::grid("2")).unwrap();
        doc.insert_datum(DatumElement::level("F1", 0.0)).unwrap();
        assert_eq!(doc.datums_of_kind(DatumKind::Grid).len(), 2);
        assert_eq!(doc.datums_of_kind(DatumKind::Level).len(), 1);
    }

    #[test]
    fn unreferenced_geometry_is_collected() {
        let mut doc = Document::new("d");
        let orphan = doc
            .insert_geometry(GeometryRecord::brep(ShapeId::new(), "orphan"))
            .unwrap();

        let shape = ShapeId::new();
        doc.insert_geometry(GeometryRecord::brep(shape, "used"))
            .unwrap();
        let e = Element::new(Category::generic(), "柱").with_geometry(shape);
        doc.insert_element(e).unwrap();

        assert_eq!(doc.collect_unreferenced_geometry(), 1);
        assert!(doc.geometry_record(orphan).is_none());
        assert!(doc.geometry_record(shape).is_some());
    }

    #[test]
    fn reverse_lookup_finds_elements_positioned_on_a_datum() {
        let mut doc = Document::new("d");
        let level = DatumElement::level("F2", 3600.0);
        let level_id = level.id;
        doc.insert_datum(level).unwrap();

        let a = Element::new(Category::generic(), "柱-1");
        let a_id = a.id;
        doc.insert_element(a).unwrap();
        let b = Element::new(Category::generic(), "柱-2");
        doc.insert_element(b).unwrap();

        doc.insert_relation(Relation::new(
            RelationKind::PositionedOn,
            a_id.into(),
            level_id.into(),
        ))
        .unwrap();

        let found = doc.elements_positioned_on(level_id);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, a_id);
    }

    #[test]
    fn document_roundtrips_through_json() {
        let mut doc = Document::new("往返");
        doc.insert_datum(DatumElement::level("F1", 0.0)).unwrap();
        doc.insert_element(Element::new(Category::generic(), "柱"))
            .unwrap();

        let s = serde_json::to_string(&doc).unwrap();
        let back: Document = serde_json::from_str(&s).unwrap();
        assert_eq!(back, doc);
        // BTreeMap 保证确定性：两次序列化字节一致
        assert_eq!(serde_json::to_string(&back).unwrap(), s);
    }

    #[test]
    fn remove_geometry_replaces_the_direct_field_mutation() {
        // 钉子：字段私有化后，删除几何只能走受控方法 `remove_geometry`。
        // 语义与 `BTreeMap::remove` 一致——删已存在的返回 `Some`，不存在返回 `None`（不报错）。
        let mut doc = Document::new("d");
        let shape = ShapeId::new();
        doc.insert_geometry(GeometryRecord::brep(shape, "t"))
            .unwrap();
        assert_eq!(doc.geometry_count(), 1);

        assert!(
            doc.remove_geometry(shape).is_some(),
            "删除已存在的几何记录应返回 Some"
        );
        assert_eq!(doc.geometry_count(), 0);
        assert!(
            doc.remove_geometry(shape).is_none(),
            "删除不存在的几何记录应静默返回 None（兼容既有语义，不报错）"
        );
    }

    #[test]
    fn document_fields_stay_private_source_guard() {
        // 保证「Document 字段必须私有」的**回归钉子**，取代沙箱内无法运行的
        // compile_fail doctest。
        //
        // ## 为什么不用 compile_fail doctest（根因）
        //
        // rustdoc 编译 doctest 有两条路径：
        //   - `no_run` 等可走**合并**路径（把所有 doctest 合成一个 crate 一次编译）；
        //   - `compile_fail` **不能合并**，必须**逐条单独**调用 rustc 编译。
        // 该「非合并」路径会给子进程接**管道 stdin**；而本机沙箱无法创建管道 stdin
        // （`CreateProcess` 报 `ERROR_PIPE_BUSY` / `os error 231`），故 compile_fail
        // doctest 在沙箱内**必然失败**（与本文档内容无关；仓库内任何 compile_fail
        // doctest 都如此）。`no_run` 走合并路径、不接管道 stdin，故可正常编译。
        // 因此改以本源码守卫兜底。
        //
        // ## 真机 / CI 上可把下面这条 doctest 加回 `Document` 的文档注释
        // （在沙箱内会显示 1 failed，属环境原因，非代码问题）：
        //
        //     /// ```compile_fail
        //     /// let mut doc = core_model::Document::new("d");
        //     /// doc.elements.clear(); // 字段私有 → 编译失败；改回 pub 则本 doctest 变红
        //     /// ```
        //
        // 注意：上面那段刻意写成普通 `//` 注释、且**不用 ``` 围栏包成真 doctest**，
        // 否则它自身会在沙箱里变红。
        let src = include_str!("document.rs");
        let header = "pub struct Document {";
        let start = src.find(header).expect("应能找到 Document 定义") + header.len();
        let end = src[start..]
            .find("\n}")
            .expect("应能找到 Document 定义体结尾")
            + start;
        let body = &src[start..end];

        // 1) 剥掉 `//` 行注释（按行取 "//" 之前的部分），避免注释里的字面量误触发。
        let mut without_comments = String::new();
        for line in body.lines() {
            without_comments.push_str(line.split("//").next().unwrap_or(""));
        }
        // 2) 去掉所有空白字符做归一化——同时自动覆盖「跨行 pub\nmeta」与「多空格 pub  meta」。
        let normalized: String = without_comments
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        // 3) 归一化后的 struct 体内**根本不得出现 `pub` 这个词**：`Document` 字段类型是
        //    Meta / BTreeMap<..> / BTreeSet<..> / u64，体内本不含 `pub`，故本断言对正确
        //    代码恒真；而 `pub` / `pub(crate)` / `pub(super)` / 跨行 / 多空格等**任意**
        //    可见性变形一律触发红。
        assert!(
            !normalized.contains("pub"),
            "Document 的字段一旦获得任何可见性修饰符（pub / pub(crate) / pub(super) / \
             跨行或多余空格的变体）本守卫即变红。归一化后的 struct 体为：\n{normalized}"
        );
    }
}

//! 模块注册表（设计文档 9.3）—— **Core 与专业模块之间唯一的接触面**。
//!
//! ## 设计原则：Core 永远不需要为"加了一个专业"而改代码
//!
//! 强电电气的桥架字段、暖通的送风量、电缆隧道的支架间距——这些字段名
//! Core **一个都不认识**。Core 只知道"有个模式叫 X、它有哪些字段"。
//! 那些模式就是通过这里注册进来的。
//!
//! ## 两处被代码钉死的边界
//!
//! 1. **[`Registry::try_define_datum_kind`]** —— 永远返回错误。
//!    基准元素种类（轴网 / 标高 / 参考面 / 参考线）由 Core 固定，
//!    模块不许新增（设计文档 4.6.5）。把它写成"一调用就报错"的方法，
//!    比写在文档里有效得多。
//! 2. **[`Registry::install`]** —— 先验模块清单（含 SDK API 版本），
//!    验不过就**一个字段都不写进注册表**。

use std::collections::BTreeMap;

use core_model::{Category, Element, PropertySchema};
use serde::{Deserialize, Serialize};

use crate::annotation::AnnotationRule;
use crate::error::{SdkError, SdkResult};
use crate::factory::ElementFactory;
use crate::geometry_op::GeometryOp;
use crate::module::{Module, ModuleManifest};

/// 模块声明的图层（专业图层，如 `"TRAY-电缆桥架"`）。
///
/// **为什么 SDK 侧不带 `LayerId`**：id 的分配是 Core 的活
/// （`core-doc` 的 `DraftingStandard::upsert_layer` 在装载时分配）。
/// 模块只声明"我要一个叫这个名字、这个颜色、这个线宽的图层"。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerSpec {
    /// 图层名（唯一键）
    pub name: String,
    /// 颜色 RGB
    pub color: [u8; 3],
    /// 线型名（须是 `core-doc` 已知的线型名）
    pub line_type: String,
    /// 线宽 mm
    pub width_mm: f64,
    /// 说明
    pub description: String,
}

impl LayerSpec {
    /// 构造一个实线图层。
    #[must_use]
    pub fn new(name: impl Into<String>, color: [u8; 3], width_mm: f64) -> Self {
        Self {
            name: name.into(),
            color,
            line_type: "Continuous".to_string(),
            width_mm,
            description: String::new(),
        }
    }

    /// 指定线型（链式）。
    #[must_use]
    pub fn with_line_type(mut self, line_type: impl Into<String>) -> Self {
        self.line_type = line_type.into();
        self
    }

    /// 说明（链式）。
    #[must_use]
    pub fn with_description(mut self, text: impl Into<String>) -> Self {
        self.description = text.into();
        self
    }
}

/// 品类的显示信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CategoryInfo {
    /// 品类名（= `Category` 的值）
    pub name: String,
    /// 界面显示名
    pub display_name: String,
    /// 声明它的模块
    pub module: String,
}

/// 注册表内容的**只读概览**（可序列化，便于诊断与测试）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RegistrySummary {
    /// 模块数
    pub modules: usize,
    /// 品类数
    pub categories: usize,
    /// 属性模式数
    pub schemas: usize,
    /// 元素工厂数
    pub factories: usize,
    /// 标注规则数
    pub annotation_rules: usize,
    /// 几何算子数
    pub geometry_ops: usize,
    /// 专业图层数
    pub layers: usize,
}

/// 模块注册表。
#[derive(Default)]
pub struct Registry {
    modules: BTreeMap<String, ModuleManifest>,
    categories: BTreeMap<String, CategoryInfo>,
    schemas: BTreeMap<String, PropertySchema>,
    factories: Vec<Box<dyn ElementFactory>>,
    annotation_rules: Vec<Box<dyn AnnotationRule>>,
    geometry_ops: Vec<Box<dyn GeometryOp>>,
    layers: BTreeMap<String, LayerSpec>,
}

impl Registry {
    /// 空注册表。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    // ---------- 模块 ----------

    /// 装载一个模块：**先验清单，再让它注册**。
    ///
    /// ⚠ **已知限制**：若 `register` 执行到一半失败，已写入的条目**不会回滚**
    /// （`Box<dyn Trait>` 无法安全撤销）。宿主（`core-app::ModuleHost`）在
    /// 收到错误时应**丢弃整个注册表重建**，而不是继续用半成品。
    /// 这是刻意的取舍：注册是启动期一次性动作，重建的代价可以接受；
    /// 而给注册表加事务会显著复杂化接口。
    pub fn install(&mut self, module: &dyn Module) -> SdkResult<()> {
        let manifest = module.manifest();
        manifest.validate()?;

        let id = manifest.id.as_str().to_string();
        if self.modules.contains_key(&id) {
            return Err(SdkError::DuplicateModule(id));
        }

        module.register(self)?;

        // 模块自己登记清单（`register` 里通常会调 `register_module`）；
        // 若它没登记，这里补上——保证装了就有记录。
        self.modules.entry(id).or_insert(manifest);
        Ok(())
    }

    /// 登记模块清单。
    pub fn register_module(&mut self, manifest: &ModuleManifest) -> SdkResult<()> {
        manifest.validate()?;
        let id = manifest.id.as_str().to_string();
        if self.modules.contains_key(&id) {
            return Err(SdkError::DuplicateModule(id));
        }
        self.modules.insert(id, manifest.clone());
        Ok(())
    }

    /// 查模块。
    #[must_use]
    pub fn module(&self, id: &str) -> Option<&ModuleManifest> {
        self.modules.get(id)
    }

    /// 模块数。
    #[must_use]
    pub fn module_count(&self) -> usize {
        self.modules.len()
    }

    // ---------- 品类 ----------

    /// 登记一个专业品类。
    pub fn register_category(
        &mut self,
        name: impl Into<String>,
        display_name: impl Into<String>,
        module: impl Into<String>,
    ) -> SdkResult<()> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(SdkError::InvalidManifest("品类名不能为空".to_string()));
        }
        if self.categories.contains_key(&name) {
            return Err(SdkError::DuplicateCategory(name));
        }
        self.categories.insert(
            name.clone(),
            CategoryInfo {
                name,
                display_name: display_name.into(),
                module: module.into(),
            },
        );
        Ok(())
    }

    /// 是否已登记该品类。
    #[must_use]
    pub fn has_category(&self, name: &str) -> bool {
        self.categories.contains_key(name)
    }

    /// 品类信息。
    #[must_use]
    pub fn category(&self, name: &str) -> Option<&CategoryInfo> {
        self.categories.get(name)
    }

    /// 品类数。
    #[must_use]
    pub fn category_count(&self) -> usize {
        self.categories.len()
    }

    /// 全部品类（按名排序）。
    #[must_use]
    pub fn categories(&self) -> Vec<&CategoryInfo> {
        self.categories.values().collect()
    }

    // ---------- 属性模式 ----------

    /// 登记属性模式。
    pub fn register_schema(&mut self, schema: PropertySchema) -> SdkResult<()> {
        if schema.type_name.trim().is_empty() {
            return Err(SdkError::InvalidManifest("模式名不能为空".to_string()));
        }
        // 同名模式若内容完全一致，视为幂等（模块被重复装载时的容错）
        if let Some(existing) = self.schemas.get(&schema.type_name) {
            if *existing == schema {
                return Ok(());
            }
            return Err(SdkError::DuplicateSchema(schema.type_name));
        }
        self.schemas.insert(schema.type_name.clone(), schema);
        Ok(())
    }

    /// 查模式。
    #[must_use]
    pub fn schema(&self, name: &str) -> Option<&PropertySchema> {
        self.schemas.get(name)
    }

    /// 模式数。
    #[must_use]
    pub fn schema_count(&self) -> usize {
        self.schemas.len()
    }

    /// 全部属性模式（按模式名排序）。
    ///
    /// G5 类型管理器的列表数据源（照 [`Registry::categories`] 先例）。
    #[must_use]
    pub fn schemas(&self) -> Vec<&PropertySchema> {
        self.schemas.values().collect()
    }

    /// **覆盖式登记属性模式**（GUI 类型管理器编辑语义：存在则覆盖，不存在则插入）。
    ///
    /// 与 [`Registry::register_schema`] 的差异：register_schema 对同名异内容
    /// **响亮拒绝**（模块装载语义——两份不同定义撞名须人工裁决，模块被重复
    /// 装载靠内容幂等容错）；本方法**直接覆盖**（类型管理器编辑语义——用户
    /// 改完字段就该落库，不需要先删再建）。
    ///
    /// ⚠ 覆盖**只影响新实例**：既有实例的 `PropertyMap` 是元素落成时刻的
    /// 快照，不回写（设计文档 3.3——句柄可丢、文本为真相）。
    pub fn replace_schema(&mut self, schema: PropertySchema) -> SdkResult<()> {
        if schema.type_name.trim().is_empty() {
            return Err(SdkError::InvalidManifest("模式名不能为空".to_string()));
        }
        self.schemas.insert(schema.type_name.clone(), schema);
        Ok(())
    }

    /// **批量恢复属性模式**（G6-M2：schema 持久化——容器 schemas.json 段
    /// 打开时装回；同名逐个覆盖，空名响应拒绝）。
    ///
    /// 恢复语义与 [`Self::replace_schema`] 相同（覆盖式）：容器里的定义
    /// 就是真相来源，撞名以容器为准。
    pub fn restore_schemas(
        &mut self,
        schemas: impl IntoIterator<Item = PropertySchema>,
    ) -> SdkResult<()> {
        for schema in schemas {
            self.replace_schema(schema)?;
        }
        Ok(())
    }

    /// 用已注册的模式校验一个元素的属性。
    pub fn validate_element(&self, element: &Element) -> SdkResult<()> {
        let name = element.properties.schema.clone();
        let schema = self
            .schemas
            .get(&name)
            .ok_or(SdkError::UnknownSchema(name))?;
        schema.validate(&element.properties)?;
        Ok(())
    }

    // ---------- 元素工厂 ----------

    /// 登记元素工厂。同一品类只允许一个工厂。
    pub fn register_factory(&mut self, factory: Box<dyn ElementFactory>) -> SdkResult<()> {
        let cat = factory.category().to_string();
        if cat.trim().is_empty() {
            return Err(SdkError::Factory("工厂未声明品类".to_string()));
        }
        if self.factories.iter().any(|f| f.category() == cat) {
            return Err(SdkError::DuplicateCategory(cat));
        }
        self.factories.push(factory);
        Ok(())
    }

    /// 按品类找工厂。
    #[must_use]
    pub fn factory_for(&self, category: &str) -> Option<&dyn ElementFactory> {
        self.factories
            .iter()
            .find(|f| f.category() == category)
            .map(AsRef::as_ref)
    }

    /// 工厂数。
    #[must_use]
    pub fn factory_count(&self) -> usize {
        self.factories.len()
    }

    // ---------- 标注规则 ----------

    /// 登记标注规则。同名规则只允许一个。
    pub fn register_annotation_rule(&mut self, rule: Box<dyn AnnotationRule>) -> SdkResult<()> {
        let id = rule.id().to_string();
        if self.annotation_rules.iter().any(|r| r.id() == id) {
            return Err(SdkError::Annotation(format!("标注规则重复：{id}")));
        }
        self.annotation_rules.push(rule);
        Ok(())
    }

    /// 全部标注规则 id。
    #[must_use]
    pub fn annotation_rule_ids(&self) -> Vec<&str> {
        self.annotation_rules.iter().map(|r| r.id()).collect()
    }

    /// 标注规则数。
    #[must_use]
    pub fn annotation_rule_count(&self) -> usize {
        self.annotation_rules.len()
    }

    // ---------- 几何算子 ----------

    /// 登记几何算子。同名算子只允许一个。
    pub fn register_geometry_op(&mut self, op: Box<dyn GeometryOp>) -> SdkResult<()> {
        let id = op.id().to_string();
        if self.geometry_ops.iter().any(|o| o.id() == id) {
            return Err(SdkError::GeometryOp(format!("几何算子重复：{id}")));
        }
        self.geometry_ops.push(op);
        Ok(())
    }

    /// 按 id 找算子。
    #[must_use]
    pub fn geometry_op(&self, id: &str) -> Option<&dyn GeometryOp> {
        self.geometry_ops
            .iter()
            .find(|o| o.id() == id)
            .map(AsRef::as_ref)
    }

    /// 几何算子 id 列表。
    #[must_use]
    pub fn geometry_op_ids(&self) -> Vec<&str> {
        self.geometry_ops.iter().map(|o| o.id()).collect()
    }

    /// 几何算子数。
    #[must_use]
    pub fn geometry_op_count(&self) -> usize {
        self.geometry_ops.len()
    }

    // ---------- 专业图层 ----------

    /// 登记专业图层。
    pub fn register_layer(&mut self, layer: LayerSpec) -> SdkResult<()> {
        if layer.name.trim().is_empty() {
            return Err(SdkError::InvalidManifest("图层名不能为空".to_string()));
        }
        if layer.width_mm < 0.0 {
            return Err(SdkError::InvalidManifest(format!(
                "图层 {} 线宽为负",
                layer.name
            )));
        }
        if self.layers.contains_key(&layer.name) {
            return Err(SdkError::DuplicateCategory(layer.name));
        }
        self.layers.insert(layer.name.clone(), layer);
        Ok(())
    }

    /// 全部专业图层。
    #[must_use]
    pub fn layers(&self) -> Vec<&LayerSpec> {
        self.layers.values().collect()
    }

    /// 图层数。
    #[must_use]
    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }

    // ---------- 基准元素边界（设计文档 4.6.5） ----------

    /// Core 保留的基准元素种类（**模块不得重定义**）。
    #[must_use]
    pub const fn reserved_datum_kinds() -> [&'static str; 4] {
        ["Grid", "Level", "RefPlane", "RefLine"]
    }

    /// **这个函数永远返回错误。**
    ///
    /// 它存在的唯一目的是：当模块作者想"给 Core 加一种新的基准元素"时，
    /// 能从编译器/运行时拿到一句明确的话，而不是自己另起一套坐标系。
    ///
    /// 合法做法：给 [`core_model::DatumElement::extra`] 加属性、
    /// 注册标注规则、在模块内部保留专业定位规则。
    pub fn try_define_datum_kind(&mut self, name: impl Into<String>) -> SdkResult<()> {
        Err(SdkError::DatumKindReserved(name.into()))
    }

    // ---------- 概览 ----------

    /// 只读概览。
    #[must_use]
    pub fn summary(&self) -> RegistrySummary {
        RegistrySummary {
            modules: self.modules.len(),
            categories: self.categories.len(),
            schemas: self.schemas.len(),
            factories: self.factories.len(),
            annotation_rules: self.annotation_rules.len(),
            geometry_ops: self.geometry_ops.len(),
            layers: self.layers.len(),
        }
    }

    /// 把一个品类包成 `Category`。
    ///
    /// 便于模块侧：`registry.category_of("POWER_CABLE_TRAY")`。
    #[must_use]
    pub fn category_of(&self, name: &str) -> Category {
        Category::new(name)
    }

    /// 是否为空注册表。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
            && self.categories.is_empty()
            && self.schemas.is_empty()
            && self.factories.is_empty()
            && self.annotation_rules.is_empty()
            && self.geometry_ops.is_empty()
            && self.layers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::{Element, FieldDef, FieldType, PropertyMap, Value};

    use crate::annotation::{AnnotationContext, AnnotationNeed};
    use crate::factory::CreateRequest;
    use crate::geometry_op::{BuildOp, BuildStep, OpInput, OpOutput};

    struct DummyFactory;
    impl ElementFactory for DummyFactory {
        fn category(&self) -> &'static str {
            "DEMO"
        }
        fn display_name(&self) -> &'static str {
            "示例构件"
        }
        fn create(&self, req: &CreateRequest) -> SdkResult<Element> {
            Ok(Element::new(
                core_model::Category::new(self.category()),
                req.name.clone(),
            ))
        }
    }

    struct DummyRule;
    impl AnnotationRule for DummyRule {
        fn id(&self) -> &'static str {
            "demo.linear"
        }
        fn display_name(&self) -> &'static str {
            "示例线性标注"
        }
        fn need(&self) -> AnnotationNeed {
            AnnotationNeed::LinearDimension
        }
        fn context(&self) -> AnnotationContext {
            AnnotationContext::new(AnnotationNeed::LinearDimension)
        }
    }

    struct DummyOp;
    impl GeometryOp for DummyOp {
        fn id(&self) -> &'static str {
            "demo.box"
        }
        fn display_name(&self) -> &'static str {
            "示例方盒"
        }
        fn apply(&self, _input: &OpInput) -> SdkResult<OpOutput> {
            Ok(OpOutput::of(vec![BuildStep::new(BuildOp::MakeBox)]))
        }
    }

    fn demo_schema() -> PropertySchema {
        PropertySchema::new("DEMO").with_field(
            FieldDef::new("width", FieldType::Float, "宽")
                .with_unit("mm")
                .required(),
        )
    }

    #[test]
    fn a_fresh_registry_is_empty() {
        let r = Registry::new();
        assert!(r.is_empty());
        assert_eq!(r.summary(), RegistrySummary::default());
    }

    #[test]
    fn categories_can_be_registered_and_queried() {
        let mut r = Registry::new();
        r.register_category("POWER_CABLE_TRAY", "动力电缆桥架", "ceec.power")
            .unwrap();
        assert!(r.has_category("POWER_CABLE_TRAY"));
        assert_eq!(
            r.category("POWER_CABLE_TRAY")
                .map(|c| c.display_name.as_str()),
            Some("动力电缆桥架")
        );
        assert_eq!(r.category_count(), 1);
    }

    #[test]
    fn duplicate_category_is_rejected() {
        let mut r = Registry::new();
        r.register_category("A", "A", "m").unwrap();
        assert!(matches!(
            r.register_category("A", "A2", "m2"),
            Err(SdkError::DuplicateCategory(_))
        ));
    }

    #[test]
    fn empty_category_name_is_rejected() {
        let mut r = Registry::new();
        assert!(r.register_category("  ", "x", "m").is_err());
    }

    #[test]
    fn identical_schema_registration_is_idempotent() {
        let mut r = Registry::new();
        r.register_schema(demo_schema()).unwrap();
        r.register_schema(demo_schema()).unwrap();
        assert_eq!(r.schema_count(), 1, "重复装载同一模块不应报错");
    }

    #[test]
    fn conflicting_schema_registration_is_rejected() {
        let mut r = Registry::new();
        r.register_schema(demo_schema()).unwrap();
        let different =
            PropertySchema::new("DEMO").with_field(FieldDef::new("depth", FieldType::Float, "深"));
        assert!(matches!(
            r.register_schema(different),
            Err(SdkError::DuplicateSchema(_))
        ));
    }

    #[test]
    fn schemas_list_in_name_order() {
        let mut r = Registry::new();
        r.register_schema(PropertySchema::new("B_SCHEMA").with_field(FieldDef::new(
            "w",
            FieldType::Float,
            "宽",
        )))
        .unwrap();
        r.register_schema(PropertySchema::new("A_SCHEMA").with_field(FieldDef::new(
            "h",
            FieldType::Float,
            "高",
        )))
        .unwrap();
        let names: Vec<&str> = r.schemas().iter().map(|s| s.type_name.as_str()).collect();
        assert_eq!(
            names,
            vec!["A_SCHEMA", "B_SCHEMA"],
            "BTreeMap 天然按模式名排序"
        );
    }

    #[test]
    fn replace_schema_overrides_existing_content() {
        let mut r = Registry::new();
        r.register_schema(demo_schema()).unwrap();
        let edited = PropertySchema::new("DEMO").with_field(
            FieldDef::new("depth", FieldType::Float, "深")
                .with_unit("mm")
                .required(),
        );
        r.replace_schema(edited).unwrap();
        assert_eq!(r.schema_count(), 1, "覆盖不新增条目");
        let current = r.schema("DEMO").unwrap();
        assert!(current.field("width").is_none(), "旧字段已被覆盖");
        assert!(current.field("depth").is_some(), "新字段已生效");
        // 覆盖后旧内容再走 register_schema → 异内容拒绝（模块装载语义不变）
        assert!(matches!(
            r.register_schema(demo_schema()),
            Err(SdkError::DuplicateSchema(_))
        ));
    }

    #[test]
    fn replace_schema_inserts_when_absent() {
        let mut r = Registry::new();
        assert_eq!(r.schema_count(), 0);
        r.replace_schema(demo_schema()).unwrap();
        assert_eq!(r.schema_count(), 1);
        assert!(r.schema("DEMO").is_some());
    }

    #[test]
    fn replace_schema_rejects_empty_name() {
        let mut r = Registry::new();
        assert!(matches!(
            r.replace_schema(PropertySchema::new("  ")),
            Err(SdkError::InvalidManifest(_))
        ));
    }

    #[test]
    fn validate_element_uses_the_registered_schema() {
        let mut r = Registry::new();
        r.register_schema(demo_schema()).unwrap();

        let mut ok = Element::new(core_model::Category::new("DEMO"), "桥架");
        ok.properties = PropertyMap::new("DEMO").with("width", Value::Float(600.0));
        assert!(r.validate_element(&ok).is_ok());

        let mut missing = Element::new(core_model::Category::new("DEMO"), "桥架");
        missing.properties = PropertyMap::new("DEMO");
        assert!(r.validate_element(&missing).is_err());
    }

    #[test]
    fn validate_element_reports_unknown_schema() {
        let r = Registry::new();
        let mut e = Element::new(core_model::Category::new("X"), "x");
        e.properties = PropertyMap::new("NOT_REGISTERED");
        assert!(matches!(
            r.validate_element(&e),
            Err(SdkError::UnknownSchema(_))
        ));
    }

    #[test]
    fn only_one_factory_per_category() {
        let mut r = Registry::new();
        r.register_factory(Box::new(DummyFactory)).unwrap();
        assert!(r.factory_for("DEMO").is_some());
        assert!(matches!(
            r.register_factory(Box::new(DummyFactory)),
            Err(SdkError::DuplicateCategory(_))
        ));
    }

    #[test]
    fn annotation_rules_and_geometry_ops_register_with_unique_ids() {
        let mut r = Registry::new();
        r.register_annotation_rule(Box::new(DummyRule)).unwrap();
        r.register_geometry_op(Box::new(DummyOp)).unwrap();

        assert_eq!(r.annotation_rule_ids(), vec!["demo.linear"]);
        assert_eq!(r.geometry_op_ids(), vec!["demo.box"]);
        assert!(r.geometry_op("demo.box").is_some());
        assert!(r.geometry_op("nope").is_none());

        assert!(r.register_annotation_rule(Box::new(DummyRule)).is_err());
        assert!(r.register_geometry_op(Box::new(DummyOp)).is_err());
    }

    #[test]
    fn professional_layers_can_be_registered() {
        let mut r = Registry::new();
        r.register_layer(
            LayerSpec::new("TRAY-电缆桥架", [255, 255, 0], 0.35)
                .with_description("动力电缆桥架轮廓"),
        )
        .unwrap();
        assert_eq!(r.layer_count(), 1);
        assert_eq!(r.layers()[0].name, "TRAY-电缆桥架");
    }

    // ---- G6-M2：schema 持久化（容器 schemas.json 段打开时装回） ----

    /// G6-M2 钉 3：restore_schemas 批量装回——空 registry 恢复两个模式；
    /// **同名覆盖**（容器里的定义就是真相来源）；空名响应拒绝。
    #[test]
    fn restore_schemas_batch_loads_and_overrides_by_name() {
        let mut r = Registry::new();
        assert_eq!(r.schema_count(), 0);

        // 批量恢复两个模式
        let second = PropertySchema::new("SECOND").with_field(
            core_model::FieldDef::new("h", core_model::FieldType::Float, "高").required(),
        );
        r.restore_schemas(vec![demo_schema(), second]).unwrap();
        assert_eq!(r.schema_count(), 2);
        assert!(r.schema("DEMO").is_some());
        assert!(r.schema("SECOND").is_some());

        // 同名覆盖：SECOND 换掉字段后恢复，旧字段消失新字段生效
        let replaced = PropertySchema::new("SECOND").with_field(
            core_model::FieldDef::new("len", core_model::FieldType::Float, "长").required(),
        );
        r.restore_schemas(vec![replaced]).unwrap();
        assert_eq!(r.schema_count(), 2, "同名覆盖不增加条目");
        let s = r.schema("SECOND").unwrap();
        assert!(s.field("h").is_none(), "旧字段消失");
        assert!(s.field("len").is_some(), "新字段生效");

        // 空名响应拒绝
        assert!(matches!(
            r.restore_schemas(vec![PropertySchema::new("  ")]),
            Err(SdkError::InvalidManifest(_))
        ));
    }

    #[test]
    fn negative_layer_width_is_rejected() {
        let mut r = Registry::new();
        assert!(
            r.register_layer(LayerSpec::new("BAD", [0, 0, 0], -1.0))
                .is_err()
        );
    }

    #[test]
    fn datum_kinds_cannot_be_redefined() {
        let mut r = Registry::new();
        let err = r.try_define_datum_kind("CableAxis").unwrap_err();
        assert!(matches!(err, SdkError::DatumKindReserved(_)));
        assert_eq!(
            Registry::reserved_datum_kinds(),
            ["Grid", "Level", "RefPlane", "RefLine"]
        );
    }

    #[test]
    fn summary_tracks_every_bucket() {
        let mut r = Registry::new();
        r.register_category("A", "A", "m").unwrap();
        r.register_schema(demo_schema()).unwrap();
        r.register_factory(Box::new(DummyFactory)).unwrap();
        r.register_annotation_rule(Box::new(DummyRule)).unwrap();
        r.register_geometry_op(Box::new(DummyOp)).unwrap();
        r.register_layer(LayerSpec::new("L", [0, 0, 0], 0.2))
            .unwrap();

        let s = r.summary();
        assert_eq!(s.categories, 1);
        assert_eq!(s.schemas, 1);
        assert_eq!(s.factories, 1);
        assert_eq!(s.annotation_rules, 1);
        assert_eq!(s.geometry_ops, 1);
        assert_eq!(s.layers, 1);
        assert!(!r.is_empty());
    }

    #[test]
    fn summary_roundtrips_through_json() {
        let s = RegistrySummary {
            modules: 1,
            categories: 2,
            schemas: 3,
            factories: 4,
            annotation_rules: 5,
            geometry_ops: 6,
            layers: 7,
        };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<RegistrySummary>(&json).unwrap(), s);
    }

    #[test]
    fn category_of_wraps_a_name() {
        let r = Registry::new();
        assert_eq!(r.category_of("TRAY").as_str(), "TRAY");
    }
}

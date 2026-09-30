//! 模块宿主：**装载专业模块，并把它们的声明落到 Core 上**。
//!
//! ## 三层职责，别混
//!
//! | 层 | 干什么 |
//! |---|---|
//! | [`core_sdk::Module`] | 模块自己**声明**要什么（品类 / 模式 / 工厂 / 标注 / 算子 / 图层）|
//! | [`core_sdk::Registry`] | 把声明**收下来**（纯数据，不认识图纸）|
//! | **本模块** | 把注册表里的东西**翻译成 Core 的东西**——目前就是把专业图层装进 [`DraftingStandard`] |
//!
//! 为什么"图层"要单独翻译：SDK 侧的 [`core_sdk::LayerSpec`] **故意不带 `LayerId`**
//! （id 的分配是 Core 的活），只声明"我要一个叫这个名字、这个颜色、这个线宽的图层"。
//! 分配 id 的那一步就在这里。
//!
//! ## 装载失败的回滚策略（与 `Registry` 的已知限制配套）
//!
//! [`core_sdk::Registry::install`] 的文档里写明：`register` 跑到一半失败时，
//! 已写入的条目**不会回滚**——`Box<dyn Trait>` 无法安全撤销。
//! 官方给的处置是"**丢弃整个注册表重建**"。
//!
//! 本模块就是这么做的，而且做得更省：**先在候选注册表上重放全部已装载模块**，
//! 新模块在候选表上装成功才**整体换上去**。
//!
//! ```text
//! install(new_module)
//!   ├─ 候选表 = 重放(已装载模块…) + new_module
//!   ├─ new_module.self_check(候选表)      ← 不通过就整个丢弃
//!   └─ 通过 → self.registry = 候选表       ← 换上去的动作是原子的
//! ```
//!
//! 代价是每次装载要重放已有模块（O(n)），但装载是**启动期一次性动作**，
//! 这点代价换来"注册表永远不会是半成品"是划算的。

use core_doc::{DocError, DocResult, DraftingStandard, LayerDef, LineType, Rgb};
use core_model::Element;
use core_sdk::{
    CreateRequest, ElementFactory, LayerSpec, Module, ModuleManifest, Registry, RegistrySummary,
    SdkError, SdkResult,
};

use crate::error::AppResult;

/// **线型名 → 线型**。
///
/// 这个名字是 `core-doc`（[`LineType::name`]）与模块之间的**契约字符串**；
/// 认不出来必须报错，不能悄悄退回实线——否则"模块声明的虚线图层"
/// 到了图纸上会变成实线，而且没人会发现。
#[must_use]
pub fn parse_line_type(name: &str) -> Option<LineType> {
    match name {
        "Continuous" => Some(LineType::Continuous),
        "Dashed" => Some(LineType::Dashed),
        "DashDot" => Some(LineType::DashDot),
        "DoubleDashDot" => Some(LineType::DoubleDashDot),
        "Dotted" => Some(LineType::Dotted),
        _ => None,
    }
}

/// 把模块声明的图层转成 Core 的图层定义（**id 在这里分配**）。
fn layer_def_from_spec(spec: &LayerSpec) -> DocResult<LayerDef> {
    let line_type = parse_line_type(&spec.line_type).ok_or_else(|| {
        DocError::Standard(format!(
            "模块图层 `{}` 声明了未知线型 `{}`，已知线型为 {:?}",
            spec.name,
            spec.line_type,
            LineType::all_names()
        ))
    })?;
    let [r, g, b] = spec.color;
    Ok(LayerDef::new(
        spec.name.clone(),
        Rgb::new(r, g, b),
        line_type,
        spec.width_mm,
    ))
}

/// 模块宿主。
#[derive(Default)]
pub struct ModuleHost {
    registry: Registry,
    /// 已成功装载的模块（用于失败时在候选表上重放）。
    installed: Vec<Box<dyn Module>>,
    /// 已装载模块的清单（诊断 / 界面"已加载模块"列表用）。
    manifests: Vec<ModuleManifest>,
}

impl ModuleHost {
    /// 空宿主。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 一次装载若干模块（**全部成功才生效**）。
    ///
    /// 语义上等价于依次 [`ModuleHost::install`]，但任一失败时宿主保持原样。
    pub fn with_modules(modules: Vec<Box<dyn Module>>) -> SdkResult<Self> {
        let mut host = Self::new();
        for module in modules {
            host.install(module)?;
        }
        Ok(host)
    }

    // ---------- 装载 ----------

    /// 装载一个模块。
    ///
    /// 流程见模块文档的流程图：候选表重放 → 自检 → 原子替换。
    pub fn install(&mut self, module: Box<dyn Module>) -> SdkResult<()> {
        let mut candidate = Registry::new();
        for existing in &self.installed {
            candidate.install(existing.as_ref())?;
        }
        candidate.install(module.as_ref())?;
        module.self_check(&candidate)?;

        self.manifests.push(module.manifest());
        self.installed.push(module);
        self.registry = candidate;
        Ok(())
    }

    /// 依次装载，收集每个模块的错误但**不中断**（界面要一次列全"哪些模块坏了"）。
    ///
    /// 成功的模块照常生效；失败的模块不进宿主。
    pub fn install_all_lenient(&mut self, modules: Vec<Box<dyn Module>>) -> Vec<SdkError> {
        let mut errors = Vec::new();
        for module in modules {
            if let Err(e) = self.install(module) {
                errors.push(e);
            }
        }
        errors
    }

    // ---------- 只读访问 ----------

    /// 只读注册表（界面按品类建菜单、按模式建属性面板都读它）。
    #[must_use]
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// 可变注册表（**仅供测试与 Core 内部扩展**；模块请走 `Module::register`）。
    pub fn registry_mut(&mut self) -> &mut Registry {
        &mut self.registry
    }

    /// 已装载模块的清单。
    #[must_use]
    pub fn manifests(&self) -> &[ModuleManifest] {
        &self.manifests
    }

    /// 已装载模块数。
    #[must_use]
    pub fn module_count(&self) -> usize {
        self.manifests.len()
    }

    /// 某模块是否已装载。
    #[must_use]
    pub fn is_installed(&self, id: &str) -> bool {
        self.manifests.iter().any(|m| m.id.as_str() == id)
    }

    /// 注册表概览。
    #[must_use]
    pub fn summary(&self) -> RegistrySummary {
        self.registry.summary()
    }

    // ---------- 落地到 Core ----------

    /// **把模块声明的专业图层装进制图标准**。
    ///
    /// 同名图层**覆盖**（`upsert_layer`）：院标 / 项目可以覆盖模块给的默认值，
    /// 而模块不必知道院标长什么样。
    ///
    /// 返回装入的图层数。任一线型未知 → 整批报错（不留半套图层）。
    pub fn apply_layers_to(&self, standard: &mut DraftingStandard) -> DocResult<usize> {
        let specs: Vec<LayerSpec> = self.registry.layers().into_iter().cloned().collect();
        // 先全部解析成功再落地，避免"装了一半才发现线型写错"
        let defs: Vec<LayerDef> = specs
            .iter()
            .map(layer_def_from_spec)
            .collect::<DocResult<Vec<_>>>()?;
        for def in defs {
            standard.upsert_layer(def);
        }
        Ok(specs.len())
    }

    /// 用模块注册的元素工厂造一个构件。
    ///
    /// 没登记该品类的工厂时报错（而不是造一个"裸元素"）——
    /// 裸元素会让"桥架"和"柱子"在数据上长得一模一样。
    pub fn create(&self, category: &str, request: &CreateRequest) -> AppResult<Element> {
        let factory: &dyn ElementFactory = self
            .registry
            .factory_for(category)
            .ok_or_else(|| SdkError::Factory(format!("品类 `{category}` 没有登记工厂")))?;
        let element = factory.create(request)?;
        Ok(element)
    }

    /// 用某工厂的默认参数造元素（界面"新建"按钮的预填路径）。
    pub fn create_with_defaults(&self, category: &str, name: &str) -> AppResult<Element> {
        let factory = self
            .registry
            .factory_for(category)
            .ok_or_else(|| SdkError::Factory(format!("品类 `{category}` 没有登记工厂")))?;
        let request = CreateRequest::new(name).with_params(factory.default_params());
        let element = factory.create(&request)?;
        Ok(element)
    }

    /// 便捷：按品类造一个带默认名的元素（等价于 [`build_element`](core_sdk::build_element) + 工厂覆盖）。
    pub fn create_named(&self, category: &str, name: &str) -> AppResult<Element> {
        self.create(category, &CreateRequest::new(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_doc::standard::LAYER_CABLE_TRAY;
    use core_model::{Category, FieldDef, FieldType, PropertyMap, PropertySchema, Value};
    use core_sdk::{
        AnnotationContext, AnnotationNeed, AnnotationRule, BuildOp, BuildStep, GeometryOp, OpInput,
        OpOutput, build_element,
    };

    use crate::error::AppError;

    // ---------- 测试用模块 ----------

    struct TrayFactory;

    impl ElementFactory for TrayFactory {
        fn category(&self) -> &'static str {
            "POWER_CABLE_TRAY"
        }
        fn display_name(&self) -> &'static str {
            "动力电缆桥架"
        }
        fn schema(&self) -> Option<PropertySchema> {
            Some(
                PropertySchema::new("POWER_CABLE_TRAY")
                    .with_field(FieldDef::new("width", FieldType::Float, "宽").with_unit("mm")),
            )
        }
        fn create(&self, request: &CreateRequest) -> SdkResult<Element> {
            let mut e = build_element(self.category(), request, "动力电缆桥架");
            e.properties.set(
                "width",
                Value::Float(request.param_f64("width").unwrap_or(600.0)),
            );
            Ok(e)
        }
        fn default_params(&self) -> PropertyMap {
            PropertyMap::new("POWER_CABLE_TRAY").with("width", Value::Float(600.0))
        }
    }

    struct TrayDimRule;

    impl AnnotationRule for TrayDimRule {
        fn id(&self) -> &'static str {
            "ceec.power.tray.width"
        }
        fn display_name(&self) -> &'static str {
            "桥架宽度标注"
        }
        fn need(&self) -> AnnotationNeed {
            AnnotationNeed::LinearDimension
        }
        fn context(&self) -> AnnotationContext {
            AnnotationContext::new(AnnotationNeed::LinearDimension)
        }
    }

    struct TrayBoxOp;

    impl GeometryOp for TrayBoxOp {
        fn id(&self) -> &'static str {
            "ceec.power.tray.box"
        }
        fn display_name(&self) -> &'static str {
            "桥架方盒"
        }
        fn apply(&self, input: &OpInput) -> SdkResult<OpOutput> {
            Ok(OpOutput::of(vec![
                BuildStep::new(BuildOp::MakeBox)
                    .with_f64("dx", input.f64_or("width", 600.0))
                    .with_f64("dy", 150.0)
                    .with_f64("dz", 6000.0),
            ]))
        }
    }

    /// 一个功能完整的示例模块。
    struct PowerModule;

    impl Module for PowerModule {
        fn manifest(&self) -> ModuleManifest {
            ModuleManifest::new("ceec.power", "强电电气", "0.1.0")
                .with_description("示例：动力电缆桥架")
                .with_vendor("电网工程院")
        }

        fn register(&self, registry: &mut Registry) -> SdkResult<()> {
            registry.register_category("POWER_CABLE_TRAY", "动力电缆桥架", "ceec.power")?;
            if let Some(schema) = TrayFactory.schema() {
                registry.register_schema(schema)?;
            }
            registry.register_factory(Box::new(TrayFactory))?;
            registry.register_annotation_rule(Box::new(TrayDimRule))?;
            registry.register_geometry_op(Box::new(TrayBoxOp))?;
            registry.register_layer(
                LayerSpec::new(LAYER_CABLE_TRAY, [255, 255, 0], 0.35)
                    .with_description("动力电缆桥架轮廓"),
            )?;
            Ok(())
        }

        fn self_check(&self, registry: &Registry) -> SdkResult<()> {
            if !registry.has_category("POWER_CABLE_TRAY") {
                return Err(SdkError::Factory("自检失败：品类没登记上".to_string()));
            }
            Ok(())
        }
    }

    /// 一个"注册到一半就失败"的模块——用来验证宿主不会留下半成品。
    struct BrokenModule;

    impl Module for BrokenModule {
        fn manifest(&self) -> ModuleManifest {
            ModuleManifest::new("demo.broken", "半成品模块", "0.1.0")
        }
        fn register(&self, registry: &mut Registry) -> SdkResult<()> {
            registry.register_category("HALF_BAKED", "半成品", "demo.broken")?;
            Err(SdkError::InvalidManifest("注册到一半故意失败".to_string()))
        }
    }

    /// 一个自检永远不过的模块。
    struct FailingSelfCheck;

    impl Module for FailingSelfCheck {
        fn manifest(&self) -> ModuleManifest {
            ModuleManifest::new("demo.selfcheck", "自检不过", "0.1.0")
        }
        fn register(&self, registry: &mut Registry) -> SdkResult<()> {
            registry.register_category("SELF_CHECK", "自检不过", "demo.selfcheck")
        }
        fn self_check(&self, _registry: &Registry) -> SdkResult<()> {
            Err(SdkError::Factory("自检故意失败".to_string()))
        }
    }

    // ---------- 线型解析 ----------

    #[test]
    fn line_type_names_round_trip() {
        for name in LineType::all_names() {
            assert!(parse_line_type(name).is_some(), "{name} 应能解析");
        }
        assert_eq!(parse_line_type("DashDot"), Some(LineType::DashDot));
    }

    #[test]
    fn an_unknown_line_type_name_is_none() {
        assert_eq!(parse_line_type("Wavy"), None);
        assert_eq!(parse_line_type(""), None);
    }

    // ---------- 装载 ----------

    #[test]
    fn a_fresh_host_is_empty() {
        let host = ModuleHost::new();
        assert_eq!(host.module_count(), 0);
        assert!(host.registry().is_empty());
        assert_eq!(host.summary(), RegistrySummary::default());
    }

    #[test]
    fn installing_a_module_populates_the_registry() {
        let mut host = ModuleHost::new();
        host.install(Box::new(PowerModule)).unwrap();

        assert_eq!(host.module_count(), 1);
        assert!(host.is_installed("ceec.power"));
        assert!(host.registry().has_category("POWER_CABLE_TRAY"));

        let s = host.summary();
        assert_eq!(s.modules, 1);
        assert_eq!(s.categories, 1);
        assert_eq!(s.schemas, 1);
        assert_eq!(s.factories, 1);
        assert_eq!(s.annotation_rules, 1);
        assert_eq!(s.geometry_ops, 1);
        assert_eq!(s.layers, 1);
    }

    #[test]
    fn a_module_that_fails_midway_leaves_no_trace() {
        let mut host = ModuleHost::new();
        host.install(Box::new(PowerModule)).unwrap();
        let before = host.summary();

        let err = host.install(Box::new(BrokenModule)).unwrap_err();
        assert!(matches!(err, SdkError::InvalidManifest(_)));
        assert_eq!(host.summary(), before, "候选表被丢弃，宿主必须原样");
        assert!(!host.is_installed("demo.broken"));
        assert!(
            !host.registry().has_category("HALF_BAKED"),
            "半成品品类不得残留"
        );
    }

    #[test]
    fn a_module_failing_self_check_is_not_kept() {
        let mut host = ModuleHost::new();
        let err = host.install(Box::new(FailingSelfCheck)).unwrap_err();
        assert!(matches!(err, SdkError::Factory(_)));
        assert_eq!(host.module_count(), 0);
        assert!(host.registry().is_empty());
    }

    #[test]
    fn previously_installed_modules_survive_a_failed_install() {
        let mut host = ModuleHost::new();
        host.install(Box::new(PowerModule)).unwrap();
        assert!(host.install(Box::new(FailingSelfCheck)).is_err());

        // 重放机制必须把老模块重新装回去
        assert!(host.is_installed("ceec.power"));
        assert!(host.registry().has_category("POWER_CABLE_TRAY"));
        assert_eq!(host.registry().module_count(), 1);
    }

    #[test]
    fn installing_the_same_module_twice_is_refused() {
        let mut host = ModuleHost::new();
        host.install(Box::new(PowerModule)).unwrap();
        assert!(matches!(
            host.install(Box::new(PowerModule)),
            Err(SdkError::DuplicateModule(_))
        ));
    }

    #[test]
    fn a_module_with_a_wrong_api_version_never_enters() {
        struct Legacy;
        impl Module for Legacy {
            fn manifest(&self) -> ModuleManifest {
                ModuleManifest::new("demo.legacy", "老模块", "0.0.1").requiring_api(0)
            }
            fn register(&self, _r: &mut Registry) -> SdkResult<()> {
                Ok(())
            }
        }
        let mut host = ModuleHost::new();
        assert!(matches!(
            host.install(Box::new(Legacy)),
            Err(SdkError::ApiVersionMismatch { .. })
        ));
        assert_eq!(host.module_count(), 0);
    }

    #[test]
    fn lenient_install_reports_each_failure_but_keeps_the_good_ones() {
        let mut host = ModuleHost::new();
        let errors = host.install_all_lenient(vec![
            Box::new(PowerModule),
            Box::new(BrokenModule),
            Box::new(FailingSelfCheck),
        ]);
        assert_eq!(errors.len(), 2);
        assert_eq!(host.module_count(), 1);
        assert!(host.is_installed("ceec.power"));
    }

    #[test]
    fn with_modules_builds_a_host_in_one_call() {
        let host = ModuleHost::with_modules(vec![Box::new(PowerModule)]).unwrap();
        assert_eq!(host.module_count(), 1);
    }

    #[test]
    fn manifests_are_listed_for_the_ui() {
        let mut host = ModuleHost::new();
        host.install(Box::new(PowerModule)).unwrap();
        let names: Vec<&str> = host.manifests().iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["强电电气"]);
    }

    // ---------- 图层落地 ----------

    #[test]
    fn professional_layers_are_written_into_the_drafting_standard() {
        let host = ModuleHost::with_modules(vec![Box::new(PowerModule)]).unwrap();
        let mut standard = DraftingStandard::gb_industrial();
        let before = standard.layers.len();

        let added = host.apply_layers_to(&mut standard).unwrap();
        assert_eq!(added, 1);
        assert_eq!(standard.layers.len(), before + 1);

        let layer = standard.require_layer(LAYER_CABLE_TRAY).unwrap();
        assert_eq!(layer.line_type, LineType::Continuous);
        assert!((layer.width_mm - 0.35).abs() < 1e-12);
        assert_eq!(layer.color, Rgb::new(255, 255, 0));
        assert!(layer.id != core_model::LayerId::new(), "id 必须被分配");
    }

    #[test]
    fn a_module_layer_overrides_a_same_named_gb_layer() {
        struct Override;
        impl Module for Override {
            fn manifest(&self) -> ModuleManifest {
                ModuleManifest::new("demo.override", "院标覆盖", "0.1.0")
            }
            fn register(&self, registry: &mut Registry) -> SdkResult<()> {
                registry.register_layer(LayerSpec::new(
                    core_doc::standard::LAYER_AXIS,
                    [0, 0, 0],
                    0.25,
                ))
            }
        }

        let host = ModuleHost::with_modules(vec![Box::new(Override)]).unwrap();
        let mut standard = DraftingStandard::gb_industrial();
        let before = standard.layers.len();
        host.apply_layers_to(&mut standard).unwrap();

        assert_eq!(standard.layers.len(), before, "同名应覆盖而非追加");
        let axis = standard
            .require_layer(core_doc::standard::LAYER_AXIS)
            .unwrap();
        assert_eq!(axis.color, Rgb::BLACK, "院标覆盖了国标的红色");
    }

    #[test]
    fn an_unknown_line_type_in_a_layer_is_refused_atomically() {
        struct BadLineType;
        impl Module for BadLineType {
            fn manifest(&self) -> ModuleManifest {
                ModuleManifest::new("demo.badline", "坏线型", "0.1.0")
            }
            fn register(&self, registry: &mut Registry) -> SdkResult<()> {
                // SDK 侧只校验"线宽不为负"，线型名的合法性得等 Core 落地时才知道
                registry.register_layer(
                    LayerSpec::new("BAD-坏图层", [1, 2, 3], 0.2).with_line_type("Wavy"),
                )
            }
        }

        let host = ModuleHost::with_modules(vec![Box::new(BadLineType)]).unwrap();
        let mut standard = DraftingStandard::gb_industrial();
        let before = standard.layers.len();

        assert!(matches!(
            host.apply_layers_to(&mut standard),
            Err(DocError::Standard(_))
        ));
        assert_eq!(standard.layers.len(), before, "整批失败，不留半套图层");
    }

    #[test]
    fn applying_layers_twice_is_idempotent() {
        let host = ModuleHost::with_modules(vec![Box::new(PowerModule)]).unwrap();
        let mut standard = DraftingStandard::gb_industrial();
        host.apply_layers_to(&mut standard).unwrap();
        let count = standard.layers.len();
        host.apply_layers_to(&mut standard).unwrap();
        assert_eq!(standard.layers.len(), count);
    }

    // ---------- 造构件 ----------

    #[test]
    fn create_routes_through_the_registered_factory() {
        let host = ModuleHost::with_modules(vec![Box::new(PowerModule)]).unwrap();
        let element = host
            .create(
                "POWER_CABLE_TRAY",
                &CreateRequest::new("桥架-1").with_param("width", Value::Float(400.0)),
            )
            .unwrap();

        assert_eq!(element.name, "桥架-1");
        assert_eq!(element.category, Category::new("POWER_CABLE_TRAY"));
        assert_eq!(element.properties.get_float("width"), Some(400.0));
        assert!(!element.has_geometry(), "工厂不造几何");
    }

    #[test]
    fn create_with_defaults_uses_the_factory_prefill() {
        let host = ModuleHost::with_modules(vec![Box::new(PowerModule)]).unwrap();
        let element = host.create_with_defaults("POWER_CABLE_TRAY", "").unwrap();
        assert_eq!(element.name, "动力电缆桥架");
        assert_eq!(element.properties.get_float("width"), Some(600.0));
    }

    #[test]
    fn creating_an_unregistered_category_is_refused() {
        let host = ModuleHost::new();
        let err = host.create_named("NOT_THERE", "x").unwrap_err();
        assert!(matches!(err, AppError::Sdk(SdkError::Factory(_))));
    }

    #[test]
    fn created_elements_validate_against_the_registered_schema() {
        let host = ModuleHost::with_modules(vec![Box::new(PowerModule)]).unwrap();
        let element = host.create_with_defaults("POWER_CABLE_TRAY", "").unwrap();
        assert!(host.registry().validate_element(&element).is_ok());
    }

    #[test]
    fn geometry_ops_from_modules_produce_recipes() {
        let host = ModuleHost::with_modules(vec![Box::new(PowerModule)]).unwrap();
        let op = host.registry().geometry_op("ceec.power.tray.box").unwrap();
        let input = OpInput::new(PropertyMap::new("X").with("width", Value::Float(800.0)));
        let output = op.apply(&input).unwrap();

        assert_eq!(output.step_count(), 1);
        assert!(output.is_well_ordered());
        assert_eq!(output.steps[0].arg_f64("dx"), Some(800.0));
    }

    #[test]
    fn datum_kinds_still_cannot_be_defined_by_the_host_side() {
        let mut host = ModuleHost::new();
        assert!(
            host.registry_mut()
                .try_define_datum_kind("CableAxis")
                .is_err()
        );
    }
}

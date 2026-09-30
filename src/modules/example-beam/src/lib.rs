//! # example-beam —— 示例专业模块（SDK 体检用）
//!
//! 设计文档第 9 章要求：**一阶段必须用一个最小示例模块跑通
//! "加载 → 建模 → 出图 → 存盘"**，否则二阶段的 SDK 生态是空中楼阁。
//! 本 crate 就是那个示例模块。
//!
//! ## 它刻意只依赖 `core-sdk` 一个 crate
//!
//! ```toml
//! [dependencies]
//! core-sdk.workspace = true      # ← 只有这一行（铁律 3）
//! ```
//!
//! 因此它**看不到内核、也看不到图纸层**。如果将来发现"这个示例模块做不到某件事"，
//! 那不是模块的问题，而是 **SDK 的缺口**——这正是把示例模块当"体检"用的意义。
//!
//! ## 它注册了什么
//!
//! | 注册项 | 具体内容 |
//! |---|---|
//! | 品类 | `BEAM`（混凝土梁） |
//! | 属性模式 | 宽 / 高 / 长 / 强度等级 |
//! | 元素工厂 | [`BeamFactory`]（只造语义元素，不建几何） |
//! | 几何算子 | [`BeamGeometryOp`]（长方体 + 平移配方）、[`BeamHoleOp`]（毛坯 → 圆柱 → Cut → 平移配方，M7b） |
//! | 标注规则 | [`BeamLengthRule`]（线性尺寸，0 位小数） |
//! | 专业图层 | `BEAM-梁` / `BEAM-钢筋` |

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

use core_sdk::{
    AnnotationContext, AnnotationNeed, AnnotationRule, BuildOp, BuildStep, CreateRequest, Element,
    ElementFactory, FieldDef, FieldType, GeometryOp, LayerSpec, Module, ModuleManifest, OpInput,
    OpOutput, PropertyMap, PropertySchema, Registry, SdkError, SdkResult, Value, build_element,
};

/// 模块 id（反向域名风格）。
pub const MODULE_ID: &str = "example.beam";

/// 品类名（= 属性模式名，Core 用字符串作键）。
pub const CATEGORY: &str = "BEAM";

/// 几何算子 id。
pub const GEOMETRY_OP_ID: &str = "example.beam.rect";

/// 开孔梁几何算子 id（M7b：模块从 SDK 增量生长的第二算子）。
pub const GEOMETRY_OP_ID_HOLE: &str = "example.beam.hole";

/// 标注规则 id。
pub const ANNOTATION_RULE_ID: &str = "example.beam.length";

/// 默认截面宽（mm）。
pub const DEFAULT_WIDTH_MM: f64 = 300.0;
/// 默认截面高（mm）。
pub const DEFAULT_HEIGHT_MM: f64 = 600.0;
/// 默认梁长（mm）。
pub const DEFAULT_LENGTH_MM: f64 = 6000.0;
/// 默认孔半径（mm）。
pub const DEFAULT_HOLE_RADIUS_MM: f64 = 50.0;
/// 默认孔长（mm，圆柱刀具的高）。
pub const DEFAULT_HOLE_LENGTH_MM: f64 = 200.0;

// =============================================================================
// 属性模式
// =============================================================================

/// 梁的属性模式。
///
/// 注意字段名全是**专业词汇**（宽/高/强度等级）——Core 一个都不认识，
/// 这正是模块存在的意义（设计文档 9.3：Core 永远不为加专业而改）。
#[must_use]
pub fn beam_schema() -> PropertySchema {
    PropertySchema::new(CATEGORY)
        .with_field(
            FieldDef::new("width", FieldType::Float, "截面宽")
                .with_unit("mm")
                .required()
                .with_default(Value::Float(DEFAULT_WIDTH_MM)),
        )
        .with_field(
            FieldDef::new("height", FieldType::Float, "截面高")
                .with_unit("mm")
                .required()
                .with_default(Value::Float(DEFAULT_HEIGHT_MM)),
        )
        .with_field(
            FieldDef::new("length", FieldType::Float, "梁长")
                .with_unit("mm")
                .required()
                .with_default(Value::Float(DEFAULT_LENGTH_MM)),
        )
        .with_field(
            FieldDef::new(
                "material",
                FieldType::Enum(vec!["C30".into(), "C35".into(), "C40".into()]),
                "混凝土强度等级",
            )
            .with_default(Value::Text("C30".to_string())),
        )
}

// =============================================================================
// 元素工厂
// =============================================================================

/// 梁的元素工厂。
///
/// **只造语义元素**：返回的 [`Element`] 没有几何，`geometry_ref` 由
/// [`BeamGeometryOp`] 的配方执行完再回填（设计文档 9.2）。
pub struct BeamFactory;

impl ElementFactory for BeamFactory {
    fn category(&self) -> &str {
        CATEGORY
    }

    fn display_name(&self) -> &str {
        "混凝土梁"
    }

    fn schema(&self) -> Option<PropertySchema> {
        Some(beam_schema())
    }

    fn create(&self, request: &CreateRequest) -> SdkResult<Element> {
        let mut e = build_element(self.category(), request, "混凝土梁");
        e.properties.set(
            "width",
            Value::Float(request.param_f64("width").unwrap_or(DEFAULT_WIDTH_MM)),
        );
        e.properties.set(
            "height",
            Value::Float(request.param_f64("height").unwrap_or(DEFAULT_HEIGHT_MM)),
        );
        e.properties.set(
            "length",
            Value::Float(request.param_f64("length").unwrap_or(DEFAULT_LENGTH_MM)),
        );
        e.properties.set(
            "material",
            Value::Text(request.param_text("material").unwrap_or("C30").to_string()),
        );
        Ok(e)
    }

    fn default_params(&self) -> PropertyMap {
        PropertyMap::new(CATEGORY)
            .with("width", Value::Float(DEFAULT_WIDTH_MM))
            .with("height", Value::Float(DEFAULT_HEIGHT_MM))
            .with("length", Value::Float(DEFAULT_LENGTH_MM))
            .with("material", Value::Text("C30".to_string()))
    }
}

// =============================================================================
// 几何算子（返回配方，不直接建几何）
// =============================================================================

/// 矩形混凝土梁的几何算子。
///
/// 它返回的是**两步行之有效的配方**（`MakeBox` → `Translate`），不是内核调用。
/// 执行配方的是 `core-app`（模块看不到内核）。
pub struct BeamGeometryOp;

impl GeometryOp for BeamGeometryOp {
    fn id(&self) -> &str {
        GEOMETRY_OP_ID
    }

    fn display_name(&self) -> &str {
        "矩形混凝土梁"
    }

    fn description(&self) -> &str {
        "由 宽×高×长 生成梁体，并按定位点放置"
    }

    fn apply(&self, input: &OpInput) -> SdkResult<OpOutput> {
        let width = input.f64_or("width", DEFAULT_WIDTH_MM);
        let height = input.f64_or("height", DEFAULT_HEIGHT_MM);
        let length = input.f64_or("length", DEFAULT_LENGTH_MM);

        if width <= 0.0 || height <= 0.0 || length <= 0.0 {
            return Err(SdkError::BadParameter(
                "梁的截面尺寸与长度必须为正数（单位 mm）".to_string(),
            ));
        }

        let [x, y, z] = input.anchor;
        let body = BuildStep::new(BuildOp::MakeBox)
            .with_f64("dx", width)
            .with_f64("dy", height)
            .with_f64("dz", length)
            .with_note("梁毛坯");
        let placed = BuildStep::new(BuildOp::Translate)
            .from_steps(vec![0])
            .with_f64("x", x)
            .with_f64("y", y)
            .with_f64("z", z)
            .with_note("按定位点放置");

        let mut out = OpOutput::of(vec![body, placed]);
        if length > 12_000.0 {
            out = out.with_warning("梁长超过 12m，请确认是否需要设置施工缝或后浇带");
        }
        Ok(out)
    }
}

/// 带孔混凝土梁的几何算子（M7b：example.beam.hole）。
///
/// 它返回的是**四步行之有效的配方**（`MakeBox` → `MakeCylinder` → `Cut` →
/// `Translate`），不是内核调用。执行配方的是 `core-app`（模块看不到内核）。
///
/// 与 [`BeamGeometryOp`] 的差异：多了「开孔」两步——孔是**圆柱刀具**（半径
/// `hole_radius`、长 `hole_length`），沿梁毛坯原点处钻入。注意本配方**自身含
/// `Cut` 步**（毛坯 − 孔），是「毛坯 → 结果」的完整构件算子，**不适合**再被
/// 当作「刀具配方」喂给布尔切削（刀具应当是无布尔历史的实心毛坯）。
pub struct BeamHoleOp;

impl GeometryOp for BeamHoleOp {
    fn id(&self) -> &str {
        GEOMETRY_OP_ID_HOLE
    }

    fn display_name(&self) -> &str {
        "开孔混凝土梁"
    }

    fn description(&self) -> &str {
        "由 宽×高×长 生成梁体并按孔径/孔长开孔，再按定位点放置"
    }

    fn apply(&self, input: &OpInput) -> SdkResult<OpOutput> {
        let width = input.f64_or("width", DEFAULT_WIDTH_MM);
        let height = input.f64_or("height", DEFAULT_HEIGHT_MM);
        let length = input.f64_or("length", DEFAULT_LENGTH_MM);
        let hole_radius = input.f64_or("hole_radius", DEFAULT_HOLE_RADIUS_MM);
        let hole_length = input.f64_or("hole_length", DEFAULT_HOLE_LENGTH_MM);

        if width <= 0.0
            || height <= 0.0
            || length <= 0.0
            || hole_radius <= 0.0
            || hole_length <= 0.0
        {
            return Err(SdkError::BadParameter(
                "梁的截面尺寸、长度与孔半径/孔长必须为正数（单位 mm）".to_string(),
            ));
        }

        let [x, y, z] = input.anchor;
        let body = BuildStep::new(BuildOp::MakeBox)
            .with_f64("dx", width)
            .with_f64("dy", height)
            .with_f64("dz", length)
            .with_note("梁毛坯");
        let hole = BuildStep::new(BuildOp::MakeCylinder)
            .with_f64("radius", hole_radius)
            .with_f64("height", hole_length)
            .with_note("孔刀具");
        let opened = BuildStep::new(BuildOp::Cut)
            .from_steps(vec![0, 1])
            .with_note("开孔");
        let placed = BuildStep::new(BuildOp::Translate)
            .from_steps(vec![2])
            .with_f64("x", x)
            .with_f64("y", y)
            .with_f64("z", z)
            .with_note("按定位点放置");

        let mut out = OpOutput::of(vec![body, hole, opened, placed]);
        if length > 12_000.0 {
            out = out.with_warning("梁长超过 12m，请确认是否需要设置施工缝或后浇带");
        }
        Ok(out)
    }
}

// =============================================================================
// 标注规则
// =============================================================================

/// 梁长标注规则。
///
/// 模块只说"要一个线性尺寸、0 位小数"；**怎么画**（尺寸界线、箭头、文字样式）
/// 由 `core-doc` 依 `DraftingStandard` 决定。
pub struct BeamLengthRule;

impl AnnotationRule for BeamLengthRule {
    fn id(&self) -> &str {
        ANNOTATION_RULE_ID
    }

    fn display_name(&self) -> &str {
        "梁长标注"
    }

    fn need(&self) -> AnnotationNeed {
        AnnotationNeed::LinearDimension
    }

    fn context(&self) -> AnnotationContext {
        AnnotationContext::new(AnnotationNeed::LinearDimension)
            .with_decimals(0)
            .with_description("梁的净跨长度（mm）")
    }
}

// =============================================================================
// 专业图层
// =============================================================================

/// 本模块声明的专业图层（`LayerId` 由 Core 在装载时分配）。
#[must_use]
pub fn beam_layers() -> Vec<LayerSpec> {
    vec![
        LayerSpec::new("BEAM-梁", [255, 0, 0], 0.50).with_description("混凝土梁轮廓"),
        LayerSpec::new("BEAM-钢筋", [0, 0, 255], 0.25)
            .with_line_type("Dashed")
            .with_description("梁内钢筋（示意）"),
    ]
}

// =============================================================================
// 模块本体
// =============================================================================

/// 示例梁模块。
///
/// `register` 是**纯粹的注册动作**（设计文档 9.2：不 I/O、不读数、不启线程）。
pub struct ExampleBeam;

impl Module for ExampleBeam {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(MODULE_ID, "示例梁模块", env!("CARGO_PKG_VERSION"))
            .with_description("SDK 体检模块：注册品类 / 属性模式 / 工厂 / 算子 / 标注 / 图层")
            .with_vendor("轻量化 BIM 内核平台")
    }

    fn register(&self, registry: &mut Registry) -> SdkResult<()> {
        registry.register_module(&self.manifest())?;
        registry.register_category(CATEGORY, "混凝土梁", MODULE_ID)?;
        registry.register_schema(beam_schema())?;
        registry.register_factory(Box::new(BeamFactory))?;
        registry.register_geometry_op(Box::new(BeamGeometryOp))?;
        registry.register_geometry_op(Box::new(BeamHoleOp))?;
        registry.register_annotation_rule(Box::new(BeamLengthRule))?;
        for layer in beam_layers() {
            registry.register_layer(layer)?;
        }
        Ok(())
    }

    /// 把刚注册的东西逐项回读一遍。
    ///
    /// 这是"体检"的核心：注册时报了错名字、或某一项根本没进注册表，
    /// 都会在这里暴露，而不是等到用户点"新建梁"才发现。
    fn self_check(&self, registry: &Registry) -> SdkResult<()> {
        let mut missing: Vec<String> = Vec::new();

        if !registry.has_category(CATEGORY) {
            missing.push(format!("品类 {CATEGORY}"));
        }
        if registry.schema(CATEGORY).is_none() {
            missing.push(format!("属性模式 {CATEGORY}"));
        }
        if registry.factory_for(CATEGORY).is_none() {
            missing.push(format!("元素工厂 {CATEGORY}"));
        }
        if registry.geometry_op(GEOMETRY_OP_ID).is_none() {
            missing.push(format!("几何算子 {GEOMETRY_OP_ID}"));
        }
        if registry.geometry_op(GEOMETRY_OP_ID_HOLE).is_none() {
            missing.push(format!("几何算子 {GEOMETRY_OP_ID_HOLE}"));
        }
        if !registry.annotation_rule_ids().contains(&ANNOTATION_RULE_ID) {
            missing.push(format!("标注规则 {ANNOTATION_RULE_ID}"));
        }
        let registered = registry.layers();
        for layer in beam_layers() {
            if !registered.iter().any(|l| l.name == layer.name) {
                missing.push(format!("图层 {}", layer.name));
            }
        }

        if missing.is_empty() {
            Ok(())
        } else {
            Err(SdkError::Factory(format!(
                "示例梁模块自检失败，注册表缺少：{}",
                missing.join("、")
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_sdk::ModuleId;

    fn installed() -> Registry {
        let mut reg = Registry::new();
        reg.install(&ExampleBeam).expect("示例模块应当能装载");
        reg
    }

    #[test]
    fn manifest_is_valid_and_namespaced() {
        let m = ExampleBeam.manifest();
        assert_eq!(m.id, ModuleId::new(MODULE_ID));
        assert!(m.id.looks_namespaced());
        assert!(m.validate().is_ok());
    }

    #[test]
    fn installing_registers_every_bucket() {
        let s = installed().summary();
        assert_eq!(s.modules, 1);
        assert_eq!(s.categories, 1);
        assert_eq!(s.schemas, 1);
        assert_eq!(s.factories, 1);
        // M7b 起模块注册 2 个几何算子（BeamGeometryOp + BeamHoleOp）——
        // 与注册行为同步的期望值更新（非弱化：断言仍逐桶精确匹配）。
        assert_eq!(s.geometry_ops, 2);
        assert_eq!(s.annotation_rules, 1);
        assert_eq!(s.layers, 2);
    }

    #[test]
    fn self_check_passes_after_a_clean_install() {
        let reg = installed();
        assert!(ExampleBeam.self_check(&reg).is_ok());
    }

    #[test]
    fn self_check_reports_a_missing_category() {
        // 往注册表里"少装一样东西"来验证自检真的会红。
        let mut reg = Registry::new();
        reg.register_module(&ExampleBeam.manifest()).unwrap();
        let err = ExampleBeam.self_check(&reg).unwrap_err();
        assert!(matches!(err, SdkError::Factory(_)));
    }

    #[test]
    fn factory_produces_a_geometryless_element_with_defaults() {
        let e = BeamFactory.create(&CreateRequest::new("")).unwrap();
        assert_eq!(e.category.as_str(), CATEGORY);
        assert_eq!(e.name, "混凝土梁", "空名应回落到显示名");
        assert!(!e.has_geometry(), "工厂不建几何");
        assert_eq!(e.properties.schema, CATEGORY);
        assert_eq!(e.properties.get_float("width"), Some(DEFAULT_WIDTH_MM));
        assert_eq!(e.properties.get_text("material"), Some("C30"));
    }

    #[test]
    fn factory_honours_user_parameters() {
        let e = BeamFactory
            .create(&CreateRequest::new("KL-1").with_param("width", Value::Float(400.0)))
            .unwrap();
        assert_eq!(e.name, "KL-1");
        assert_eq!(e.properties.get_float("width"), Some(400.0));
    }

    #[test]
    fn geometry_op_emits_a_well_ordered_recipe() {
        let input = OpInput::new(PropertyMap::new(CATEGORY).with("length", Value::Float(9000.0)))
            .at([1000.0, 0.0, 3000.0]);
        let out = BeamGeometryOp.apply(&input).unwrap();

        assert_eq!(out.step_count(), 2);
        assert_eq!(out.steps[0].op, BuildOp::MakeBox);
        assert_eq!(out.steps[1].op, BuildOp::Translate);
        assert_eq!(out.steps[0].arg_f64("dz"), Some(9000.0));
        assert!(out.is_well_ordered());
        for step in &out.steps {
            assert!(step.validate_arity().is_ok());
        }
    }

    #[test]
    fn geometry_op_rejects_non_positive_dimensions() {
        let input = OpInput::new(PropertyMap::new(CATEGORY).with("width", Value::Float(0.0)));
        assert!(matches!(
            BeamGeometryOp.apply(&input),
            Err(SdkError::BadParameter(_))
        ));
    }

    #[test]
    fn very_long_beams_warn_but_still_build() {
        let input = OpInput::new(PropertyMap::new(CATEGORY).with("length", Value::Float(15_000.0)));
        let out = BeamGeometryOp.apply(&input).unwrap();
        assert_eq!(out.step_count(), 2);
        assert_eq!(out.warnings.len(), 1);
    }

    #[test]
    fn annotation_rule_binds_geometry_with_zero_decimals() {
        let c = BeamLengthRule.context();
        assert_eq!(c.need, AnnotationNeed::LinearDimension);
        assert!(c.binds_geometry());
        assert!(!c.requires_datum);
        assert_eq!(c.decimals, Some(0));
    }

    #[test]
    fn declared_layers_use_known_line_types() {
        // "Continuous" / "Dashed" 是 core-doc 的契约线型名——
        // 模块侧认错名字会在装载时被 core-app 的图层套用逻辑拒绝。
        for layer in beam_layers() {
            assert!(
                layer.line_type == "Continuous" || layer.line_type == "Dashed",
                "线型名必须是 core-doc 已知的契约字符串，实得 {}",
                layer.line_type
            );
            assert!(layer.width_mm > 0.0);
        }
    }

    #[test]
    fn the_module_cannot_redefine_datum_kinds() {
        // 顺手验证边界：模块想给 Core 加一种基准元素，必须被拒。
        let mut reg = Registry::new();
        assert!(matches!(
            reg.try_define_datum_kind("BeamAxis"),
            Err(SdkError::DatumKindReserved(_))
        ));
    }

    // ---------- M7b-1：BeamHoleOp 第二算子（example.beam.hole）----------

    #[test]
    fn hole_op_emits_a_well_ordered_recipe() {
        let input = OpInput::new(PropertyMap::new(CATEGORY).with("length", Value::Float(9000.0)))
            .at([1000.0, 0.0, 3000.0]);
        let out = BeamHoleOp.apply(&input).unwrap();

        // 4 步：MakeBox → MakeCylinder → Cut → Translate
        assert_eq!(out.step_count(), 4);
        assert_eq!(out.steps[0].op, BuildOp::MakeBox);
        assert_eq!(out.steps[1].op, BuildOp::MakeCylinder);
        assert_eq!(out.steps[2].op, BuildOp::Cut);
        assert_eq!(out.steps[3].op, BuildOp::Translate);

        // 缺省沿用常量体系 + M7b 新常量
        assert_eq!(out.steps[0].arg_f64("dx"), Some(DEFAULT_WIDTH_MM));
        assert_eq!(out.steps[0].arg_f64("dy"), Some(DEFAULT_HEIGHT_MM));
        assert_eq!(out.steps[0].arg_f64("dz"), Some(9000.0));
        assert_eq!(out.steps[1].arg_f64("radius"), Some(DEFAULT_HOLE_RADIUS_MM));
        assert_eq!(out.steps[1].arg_f64("height"), Some(DEFAULT_HOLE_LENGTH_MM));

        // Cut 步元数 2 的 inputs 正确性：吃毛坯 + 孔刀具
        assert_eq!(out.steps[2].inputs, vec![0, 1]);
        assert!(out.steps[2].validate_arity().is_ok());
        // Translate 吃 Cut 的结果
        assert_eq!(out.steps[3].inputs, vec![2]);

        assert!(out.is_well_ordered());
        for step in &out.steps {
            assert!(step.validate_arity().is_ok());
        }
    }

    #[test]
    fn hole_op_rejects_non_positive_parameters() {
        for (key, value) in [
            ("width", 0.0),
            ("height", -600.0),
            ("length", 0.0),
            ("hole_radius", 0.0),
            ("hole_length", -200.0),
        ] {
            let input = OpInput::new(PropertyMap::new(CATEGORY).with(key, Value::Float(value)));
            assert!(
                matches!(BeamHoleOp.apply(&input), Err(SdkError::BadParameter(_))),
                "参数 {key}={value} 必须被响亮拒绝（BadParameter）"
            );
        }
    }

    #[test]
    fn hole_op_honours_user_parameters() {
        let input = OpInput::new(
            PropertyMap::new(CATEGORY)
                .with("width", Value::Float(400.0))
                .with("hole_radius", Value::Float(60.0))
                .with("hole_length", Value::Float(250.0)),
        );
        let out = BeamHoleOp.apply(&input).unwrap();
        assert_eq!(out.steps[0].arg_f64("dx"), Some(400.0));
        assert_eq!(out.steps[1].arg_f64("radius"), Some(60.0));
        assert_eq!(out.steps[1].arg_f64("height"), Some(250.0));
    }

    #[test]
    fn hole_op_recipe_roundtrips_through_json() {
        let input = OpInput::new(
            PropertyMap::new(CATEGORY)
                .with("hole_radius", Value::Float(60.0))
                .with("hole_length", Value::Float(250.0)),
        )
        .at([10.0, 20.0, 30.0]);
        let out = BeamHoleOp.apply(&input).unwrap();

        let text = serde_json::to_string(&out).expect("配方应可序列化");
        let back: OpOutput = serde_json::from_str(&text).expect("配方应可反序列化");
        assert_eq!(back, out, "JSON 往返后配方应逐字段相等");
    }
}

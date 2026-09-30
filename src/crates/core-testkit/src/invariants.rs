//! 跨层不变式：把"必须恒成立"的规则写成一个函数，各处调用。
//!
//! 这些断言的价值不在于当下通过，而在于**改动几何/文档/注册表时有东西替你盯着**。
//! 例如"配方必须依赖有序"这条，一旦哪天 `GeometryOp` 实现里出现前向引用，
//! 调用过本函数的测试会立刻变红。

use core_model::Document;
use core_sdk::{OpOutput, Registry};

/// 断言一份配方**可以安全送到 `core-app` 去执行**。
///
/// 两项检查：
/// 1. 依赖下标都指向更早的步骤（有向无环）；
/// 2. 每步的输入个数与算子声明的 arity 一致。
#[track_caller]
pub fn assert_recipe_is_executable(output: &OpOutput) {
    assert!(
        output.is_well_ordered(),
        "配方依赖无序（存在前向引用或 arity 不符），不可执行：{:?}",
        output.steps
    );
    for (i, step) in output.steps.iter().enumerate() {
        assert!(
            step.validate_arity().is_ok(),
            "第 {i} 步 `{}` 的输入个数与算子 arity 不符",
            step.op.label()
        );
    }
}

/// 断言文档经 JSON 往返后**完全相等**。
///
/// `.bimdoc` 的存盘基于序列化，若往返不相等，说明存盘会丢信息。
#[track_caller]
pub fn assert_document_roundtrips_json(doc: &Document) {
    let json = serde_json::to_string(doc).expect("文档应可序列化为 JSON");
    let back: Document = serde_json::from_str(&json).expect("文档应可从 JSON 反序列化");
    assert_eq!(&back, doc, "文档 JSON 往返后不相等，存盘会丢信息");
}

/// 断言注册表的概览计数与逐项计数**自洽**。
#[track_caller]
pub fn assert_registry_summary_consistent(registry: &Registry) {
    let s = registry.summary();
    assert_eq!(s.modules, registry.module_count(), "模块计数不一致");
    assert_eq!(s.categories, registry.category_count(), "品类计数不一致");
    assert_eq!(s.schemas, registry.schema_count(), "模式计数不一致");
    assert_eq!(s.factories, registry.factory_count(), "工厂计数不一致");
    assert_eq!(
        s.annotation_rules,
        registry.annotation_rule_count(),
        "标注规则计数不一致"
    );
    assert_eq!(
        s.geometry_ops,
        registry.geometry_op_count(),
        "几何算子计数不一致"
    );
    assert_eq!(s.layers, registry.layer_count(), "图层计数不一致");
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_sdk::{
        BuildOp, BuildStep, GeometryOp, LayerSpec, OpInput, PropertyMap, Registry, SdkResult,
    };

    use crate::fixtures::{demo_document, demo_schema};

    struct BoxOp;
    impl GeometryOp for BoxOp {
        fn id(&self) -> &str {
            "testkit.box"
        }
        fn display_name(&self) -> &str {
            "夹具方盒"
        }
        fn apply(&self, _input: &OpInput) -> SdkResult<OpOutput> {
            Ok(OpOutput::of(vec![
                BuildStep::new(BuildOp::MakeBox),
                BuildStep::new(BuildOp::Translate).from_steps(vec![0]),
            ]))
        }
    }

    #[test]
    fn a_well_formed_recipe_passes() {
        let out = BoxOp.apply(&OpInput::new(PropertyMap::new("X"))).unwrap();
        assert_recipe_is_executable(&out);
    }

    #[test]
    #[should_panic(expected = "配方依赖无序")]
    fn a_forward_reference_is_caught() {
        let bad = OpOutput::of(vec![BuildStep::new(BuildOp::Translate).from_steps(vec![1])]);
        assert_recipe_is_executable(&bad);
    }

    #[test]
    fn a_fresh_document_roundtrips_through_json() {
        assert_document_roundtrips_json(&demo_document());
    }

    #[test]
    fn an_empty_registry_is_self_consistent() {
        assert_registry_summary_consistent(&Registry::new());
    }

    #[test]
    fn a_populated_registry_stays_self_consistent() {
        let mut reg = Registry::new();
        reg.register_category("A", "A", "testkit").unwrap();
        reg.register_schema(demo_schema()).unwrap();
        reg.register_layer(LayerSpec::new("L", [0, 0, 0], 0.2))
            .unwrap();
        reg.register_geometry_op(Box::new(BoxOp)).unwrap();
        assert_registry_summary_consistent(&reg);
    }

    #[test]
    fn counts_actually_react_to_registration() {
        let mut reg = Registry::new();
        reg.register_category("A", "A", "t").unwrap();
        assert_eq!(reg.summary().categories, 1);
    }
}

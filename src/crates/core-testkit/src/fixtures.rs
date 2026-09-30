//! 固定夹具：多处复用的测试输入。
//!
//! 这里的东西**一律用确定值**（不取随机、不取当前时间），
//! 否则黄金样本比对就失去意义。

use core_model::{
    Category, Document, Element, FieldDef, FieldType, PropertyMap, PropertySchema, Value,
};

/// 夹具专用品类名。刻意与任何真实模块不同，避免误以为是产品数据。
pub const DEMO_CATEGORY: &str = "TESTKIT_DEMO";

/// 一个示例属性模式：宽度（mm）+ 材质（文本）。
///
/// `required` 的宽度用来验证"缺字段要被拒"这类校验路径。
#[must_use]
pub fn demo_schema() -> PropertySchema {
    PropertySchema::new(DEMO_CATEGORY)
        .with_field(
            FieldDef::new("width", FieldType::Float, "宽度")
                .with_unit("mm")
                .required()
                .with_default(Value::Float(600.0)),
        )
        .with_field(FieldDef::new("material", FieldType::Text, "材质"))
}

/// 造一个夹具元素（**不带几何**——几何要真实内核，属 M1）。
///
/// 名字为空时回落为 `"夹具构件"`。
#[must_use]
pub fn demo_element(name: &str) -> Element {
    let display = if name.trim().is_empty() {
        "夹具构件"
    } else {
        name
    };
    let mut element = Element::new(Category::new(DEMO_CATEGORY), display);
    element.properties = PropertyMap::new(DEMO_CATEGORY)
        .with("width", Value::Float(600.0))
        .with("material", Value::Text("热镀锌钢".to_string()));
    element
}

/// 造一份含两个元素的夹具文档。
///
/// 元素的 `ElementId` 由 `core-model` 自动生成（UUID v7），
/// 因此**每次调用 id 不同**——需要固定 id 的场景请自备构造。
#[must_use]
pub fn demo_document() -> Document {
    let mut doc = Document::new("测试文档");
    // 夹具里的元素一定插得进去（空文档、id 唯一），失败说明 core-model 有 bug；
    // 这里选择"安静地忽略"以避免 toolbox 自身 panic，由调用方的断言去发现数量不对。
    let _ = doc.insert_element(demo_element("夹具构件-1"));
    let _ = doc.insert_element(demo_element("夹具构件-2"));
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_element_falls_back_to_a_display_name() {
        assert_eq!(demo_element("").name, "夹具构件");
        assert_eq!(demo_element("  ").name, "夹具构件");
        assert_eq!(demo_element("X").name, "X");
    }

    #[test]
    fn demo_element_carries_no_geometry() {
        assert!(
            !demo_element("X").has_geometry(),
            "夹具的几何属 M1，此处不该有"
        );
    }

    #[test]
    fn demo_document_has_two_elements() {
        assert_eq!(demo_document().element_count(), 2);
    }

    #[test]
    fn demo_schema_declares_its_fields() {
        let s = demo_schema();
        assert_eq!(s.type_name, DEMO_CATEGORY);
        assert!(s.field("width").is_some());
        assert_eq!(s.field("width").map(|f| f.required), Some(true));
    }

    #[test]
    fn demo_element_matches_the_demo_schema() {
        let s = demo_schema();
        assert!(s.validate(&demo_element("X").properties).is_ok());
    }
}

//! 元素模型（设计文档 4.1）。

use serde::{Deserialize, Serialize};

use crate::ids::{ElementId, RelationId, ShapeId};
use crate::property::PropertyMap;

/// 元素品类。
///
/// **Core 不内置任何专业品类**：模块经 SDK 注册自己的品类名（如
/// `"POWER_CABLE_TRAY"`）。这里只提供一个通用包装，避免用裸 `String`
/// 带来的"哪里都能塞错值"问题。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Category(String);

impl Category {
    /// 通用构件（未指定专业品类时的兜底）。
    pub const GENERIC: &'static str = "GENERIC";

    /// 由名字构造。
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// 通用构件。
    #[must_use]
    pub fn generic() -> Self {
        Self(Self::GENERIC.to_string())
    }

    /// 取名字。
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Category {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 文档中的一个元素（语义对象）。
///
/// **关键区分**（设计文档 4.1）：`Element` 是"语义对象"（一根柱、一段桥架），
/// 几何是另一回事——通过 [`Element::geometry_ref`] 指向 [`crate::GeometryRecord`]。
/// 一个元素可以**没有几何**（纯标注），也可以多个元素**共享**同一段几何。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Element {
    /// 持久 id
    pub id: ElementId,
    /// 品类
    pub category: Category,
    /// 显示名
    pub name: String,
    /// 几何记录 id；`None` 表示该元素无几何
    pub geometry_ref: Option<ShapeId>,
    /// 属性
    pub properties: PropertyMap,
    /// 关系 id
    pub relations: Vec<RelationId>,
    /// 可见性（G6-M2：元素树眼睛——文档级显隐，随 .bimdoc 持久化）。
    ///
    /// `false` 时视口渲染跳过该元素引用的形状（某形状的**全部**引用元素
    /// 都隐藏时才不画——多元素共享同一段几何按"任一可见即画"）。
    /// `#[serde(default)]` 让旧容器缺字段时默认可见——双向兼容：
    /// 旧程序读新文件（serde 忽略未知字段）也不受影响。
    #[serde(default = "default_visible")]
    pub visible: bool,
}

fn default_visible() -> bool {
    true
}

impl Element {
    /// 构造一个元素（自动生成持久 id）。
    #[must_use]
    pub fn new(category: Category, name: impl Into<String>) -> Self {
        Self {
            id: ElementId::new(),
            category,
            name: name.into(),
            geometry_ref: None,
            properties: PropertyMap::new(Category::GENERIC),
            relations: Vec::new(),
            visible: true,
        }
    }

    /// 指定几何引用。
    #[must_use]
    pub fn with_geometry(mut self, shape: ShapeId) -> Self {
        self.geometry_ref = Some(shape);
        self
    }

    /// 指定属性。
    #[must_use]
    pub fn with_properties(mut self, properties: PropertyMap) -> Self {
        self.properties = properties;
        self
    }

    /// 指定可见性（链式；默认可见）。
    #[must_use]
    pub fn with_visible(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }

    /// 是否有几何。
    #[must_use]
    pub fn has_geometry(&self) -> bool {
        self.geometry_ref.is_some()
    }
}

impl Default for Element {
    fn default() -> Self {
        Self::new(Category::generic(), "")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::property::Value;

    #[test]
    fn new_element_has_generated_id_and_no_geometry() {
        let e = Element::new(Category::new("WALL"), "外墙-1");
        assert!(!e.has_geometry());
        assert_eq!(e.category.as_str(), "WALL");
        assert!(e.relations.is_empty());
    }

    #[test]
    fn builders_attach_geometry_and_properties() {
        let shape = ShapeId::new();
        let e = Element::new(Category::generic(), "柱")
            .with_geometry(shape)
            .with_properties(PropertyMap::new("COLUMN").with("h", Value::Float(3000.0)));
        assert!(e.has_geometry());
        assert_eq!(e.geometry_ref, Some(shape));
        assert_eq!(e.properties.get_float("h"), Some(3000.0));
    }

    #[test]
    fn category_has_a_stable_generic_name() {
        assert_eq!(Category::generic().as_str(), Category::GENERIC);
    }

    // ---- G6-M2：元素可见性（元素树眼睛的数据层） ----

    /// G6-M2 钉 1：新元素默认可见；with_visible 链式生效。
    #[test]
    fn new_element_is_visible_by_default() {
        let e = Element::new(Category::generic(), "柱");
        assert!(e.visible, "新元素默认可见");

        let hidden = Element::new(Category::generic(), "柱").with_visible(false);
        assert!(!hidden.visible, "with_visible(false) 生效");
    }

    /// G6-M2 钉 2：visible 随 JSON 往返；**旧容器缺字段时默认可见**（serde
    /// default 向后兼容——旧 .bimdoc 反序列化不受影响）。
    #[test]
    fn visible_persists_and_old_json_without_the_field_defaults_to_visible() {
        // 新格式：visible=true 与 false 都完整往返
        let shown = Element::new(Category::generic(), "显");
        let hidden = Element::new(Category::generic(), "隐").with_visible(false);
        for e in [shown, hidden] {
            let json = serde_json::to_string(&e).unwrap();
            let back: Element = serde_json::from_str(&json).unwrap();
            assert_eq!(back, e, "visible 随 JSON 往返");
        }

        // 旧格式：手工造一份**没有 visible 字段**的 JSON（旧版本程序写出的容器）
        let e = Element::new(Category::generic(), "旧格式");
        let mut value = serde_json::to_value(&e).unwrap();
        value.as_object_mut().unwrap().remove("visible");
        let old_json = serde_json::to_string(&value).unwrap();
        let back: Element = serde_json::from_str(&old_json).unwrap();
        assert!(
            back.visible,
            "旧容器缺 visible 字段时必须默认可见（serde default 向后兼容）"
        );
    }
}

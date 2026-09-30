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
}

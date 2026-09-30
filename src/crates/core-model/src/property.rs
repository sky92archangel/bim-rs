//! 属性体系（设计文档 4.4）—— 专业扩展的入口。
//!
//! 用**模式（Schema）+ 实例**两级：
//! - [`PropertySchema`] 由专业模块注册（见 `core-sdk`）；
//! - [`PropertyMap`] 挂在元素实例上。
//!
//! **核心原则**：`core-model` **不认识任何专业字段**。它只知道"有个模式叫 X、
//! 它有哪些字段"。强电电气的字段、暖通的风量、电缆隧道的支架间距，**全部由模块注册**。
//! 这样 Core 永远不会因为"加了个专业"而要改。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{ModelError, ModelResult};
use crate::ids::ElementId;

/// 字段的类型。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldType {
    /// 布尔
    Bool,
    /// 整数
    Int,
    /// 浮点数（注意单位，见 [`FieldDef::unit`]）
    Float,
    /// 文本
    Text,
    /// 枚举（取值限于给定集合）
    Enum(Vec<String>),
    /// 引用另一个元素
    Ref,
}

/// 一个字段的定义。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldDef {
    /// 字段名（模块内唯一）
    pub name: String,
    /// 类型
    pub field_type: FieldType,
    /// 单位（如 `"mm"` / `"kW"` / `"m3/h"`）；无量纲留空
    pub unit: Option<String>,
    /// 是否必填
    pub required: bool,
    /// 默认值
    pub default: Option<Value>,
    /// 说明（会展示给用户）
    pub doc: String,
}

impl FieldDef {
    /// 构造一个非必填、无单位的字段。
    #[must_use]
    pub fn new(name: impl Into<String>, field_type: FieldType, doc: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            field_type,
            unit: None,
            required: false,
            default: None,
            doc: doc.into(),
        }
    }

    /// 指定单位。
    #[must_use]
    pub fn with_unit(mut self, unit: impl Into<String>) -> Self {
        self.unit = Some(unit.into());
        self
    }

    /// 标记为必填。
    #[must_use]
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    /// 指定默认值。
    #[must_use]
    pub fn with_default(mut self, value: Value) -> Self {
        self.default = Some(value);
        self
    }
}

/// 属性模式：由专业模块注册。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PropertySchema {
    /// 模式名（如 `"POWER_CABLE_TRAY"`）
    pub type_name: String,
    /// 字段定义
    pub fields: Vec<FieldDef>,
}

impl PropertySchema {
    /// 构造一个空模式。
    #[must_use]
    pub fn new(type_name: impl Into<String>) -> Self {
        Self {
            type_name: type_name.into(),
            fields: Vec::new(),
        }
    }

    /// 追加一个字段。
    #[must_use]
    pub fn with_field(mut self, field: FieldDef) -> Self {
        self.fields.push(field);
        self
    }

    /// 按名查字段。
    #[must_use]
    pub fn field(&self, name: &str) -> Option<&FieldDef> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// 校验一份属性值是否满足本模式。
    pub fn validate(&self, map: &PropertyMap) -> ModelResult<()> {
        if map.schema != self.type_name {
            return Err(ModelError::PropertyField(format!(
                "模式不匹配：期望 {}，实得 {}",
                self.type_name, map.schema
            )));
        }
        for field in &self.fields {
            match (map.values.get(&field.name), field.required) {
                (None, true) => {
                    return Err(ModelError::PropertyField(format!(
                        "缺少必填字段：{}",
                        field.name
                    )));
                }
                (Some(value), _) => {
                    if !value.type_matches(&field.field_type) {
                        return Err(ModelError::PropertyField(format!(
                            "字段 {} 类型不匹配",
                            field.name
                        )));
                    }
                }
                (None, false) => {}
            }
        }
        for key in map.values.keys() {
            if self.field(key).is_none() {
                return Err(ModelError::PropertyField(format!("未知字段：{key}")));
            }
        }
        Ok(())
    }
}

/// 属性值。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    /// 布尔
    Bool(bool),
    /// 整数
    Int(i64),
    /// 浮点数
    Float(f64),
    /// 文本
    Text(String),
    /// 引用另一个元素
    Ref(ElementId),
}

impl Value {
    /// 值的类型是否匹配字段类型。
    #[must_use]
    pub fn type_matches(&self, field_type: &FieldType) -> bool {
        match (self, field_type) {
            (Self::Bool(_), FieldType::Bool)
            | (Self::Int(_), FieldType::Int)
            | (Self::Float(_), FieldType::Float)
            | (Self::Text(_), FieldType::Text)
            | (Self::Ref(_), FieldType::Ref) => true,
            (Self::Text(s), FieldType::Enum(options)) => options.contains(s),
            _ => false,
        }
    }
}

/// 挂在元素实例上的属性值集合。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PropertyMap {
    /// 所遵循的模式名
    pub schema: String,
    /// 字段 → 值
    pub values: BTreeMap<String, Value>,
}

impl PropertyMap {
    /// 构造一份空属性，指定所遵循的模式。
    #[must_use]
    pub fn new(schema: impl Into<String>) -> Self {
        Self {
            schema: schema.into(),
            values: BTreeMap::new(),
        }
    }

    /// 置入一个值（链式）。
    #[must_use]
    pub fn with(mut self, key: impl Into<String>, value: Value) -> Self {
        self.values.insert(key.into(), value);
        self
    }

    /// 置入一个值。
    pub fn set(&mut self, key: impl Into<String>, value: Value) {
        self.values.insert(key.into(), value);
    }

    /// 取值。
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    /// 取浮点值（若类型不符则返回 `None`）。
    #[must_use]
    pub fn get_float(&self, key: &str) -> Option<f64> {
        match self.values.get(key) {
            Some(Value::Float(v)) => Some(*v),
            _ => None,
        }
    }

    /// 取文本值。
    #[must_use]
    pub fn get_text(&self, key: &str) -> Option<&str> {
        match self.values.get(key) {
            Some(Value::Text(v)) => Some(v.as_str()),
            _ => None,
        }
    }

    /// 字段数量。
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// 是否无属性。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tray_schema() -> PropertySchema {
        PropertySchema::new("POWER_CABLE_TRAY")
            .with_field(
                FieldDef::new("width", FieldType::Float, "桥架宽度")
                    .with_unit("mm")
                    .required(),
            )
            .with_field(FieldDef::new("material", FieldType::Text, "材质"))
            .with_field(FieldDef::new(
                "install",
                FieldType::Enum(vec!["架空".into(), "埋地".into()]),
                "敷设方式",
            ))
    }

    #[test]
    fn valid_map_passes_validation() {
        let schema = tray_schema();
        let map = PropertyMap::new("POWER_CABLE_TRAY")
            .with("width", Value::Float(600.0))
            .with("material", Value::Text("热镀锌钢".into()));
        assert!(schema.validate(&map).is_ok());
    }

    #[test]
    fn missing_required_field_is_rejected() {
        let schema = tray_schema();
        let map = PropertyMap::new("POWER_CABLE_TRAY");
        assert!(matches!(
            schema.validate(&map),
            Err(ModelError::PropertyField(_))
        ));
    }

    #[test]
    fn wrong_type_is_rejected() {
        let schema = tray_schema();
        let map = PropertyMap::new("POWER_CABLE_TRAY").with("width", Value::Text("600".into()));
        assert!(schema.validate(&map).is_err());
    }

    #[test]
    fn enum_value_must_be_in_options() {
        let schema = tray_schema();
        let ok = PropertyMap::new("POWER_CABLE_TRAY")
            .with("width", Value::Float(600.0))
            .with("install", Value::Text("架空".into()));
        assert!(schema.validate(&ok).is_ok());

        let bad = PropertyMap::new("POWER_CABLE_TRAY")
            .with("width", Value::Float(600.0))
            .with("install", Value::Text("悬空".into()));
        assert!(schema.validate(&bad).is_err());
    }

    #[test]
    fn unknown_field_is_rejected() {
        let schema = tray_schema();
        let map = PropertyMap::new("POWER_CABLE_TRAY")
            .with("width", Value::Float(600.0))
            .with("colour", Value::Text("灰".into()));
        assert!(schema.validate(&map).is_err());
    }

    #[test]
    fn schema_name_must_match() {
        let schema = tray_schema();
        let map = PropertyMap::new("SOMETHING_ELSE").with("width", Value::Float(600.0));
        assert!(schema.validate(&map).is_err());
    }
}

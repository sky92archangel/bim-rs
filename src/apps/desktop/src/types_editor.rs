//! 类型编辑器纯逻辑（G5-M20）：Types 页的状态 + 快照 / 校验 / 构造纯函数。
//!
//! 本模块**不依赖 egui、不门控 feature**（默认腿可单测）——Types 页的渲染与
//! 交互在门控模块 `gui`，二者通过 [`TypesEdit`] 状态 + 纯函数衔接（照
//! `sketch_tool` / `sketch_editor` 同款分层）。
//!
//! ## 编辑语义（G5-M19 确立的覆盖语义）
//!
//! - **新建模式**：type_name 可填；保存前重名校验（与已注册同名 → 拒绝——
//!   新建不该静默覆盖既有类型）。
//! - **编辑模式**：type_name 只读（改名会插入新条目、旧条目残留——不做改名
//!   语义）；保存走 `upsert_schema` 直接覆盖（`Registry::replace_schema`）。
//! - ⚠ 保存只影响**新实例**：既有实例的 `PropertyMap` 是元素落成时刻的快照，
//!   不回写（设计文档 3.3）——GUI 面板上如实说明。
//! - ⚠ schema 是**会话态**：`.bimdoc` 不持久化 registry，自定义类型随会话
//!   消失——重开文档后自定义类型丢失（诚实提示）。
//!
//! bin crate 的默认腿（无 gui feature）下唯一非测试调用方 `gui` 不参与编译，
//! pub 项以单测先行驱动——整模块抑制 dead_code 告警（与 `sketch_tool` 同款
//! 口径，接线后保留亦无害）。

#![allow(dead_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

use core_model::{FieldDef, FieldType, PropertySchema, Value};

/// FieldType 六种（下拉遍历用；Enum 带空选项占位——选中后选项由
/// [`FieldRow::enum_options`] 编辑）。
pub const FIELD_TYPES: [FieldType; 6] = [
    FieldType::Bool,
    FieldType::Int,
    FieldType::Float,
    FieldType::Text,
    FieldType::Enum(Vec::new()),
    FieldType::Ref,
];

/// [`FieldType`] 的下拉显示文本。
#[must_use]
pub fn type_label(ft: &FieldType) -> &'static str {
    match ft {
        FieldType::Bool => "布尔",
        FieldType::Int => "整数",
        FieldType::Float => "浮点数",
        FieldType::Text => "文本",
        FieldType::Enum(_) => "枚举",
        FieldType::Ref => "引用",
    }
}

/// 一行字段的编辑态（GUI 表单 ↔ [`FieldDef`] 的桥）。
#[derive(Debug, Clone, PartialEq)]
pub struct FieldRow {
    /// 字段名
    pub name: String,
    /// 类型
    pub field_type: FieldType,
    /// 单位文本（空串 = 无单位）
    pub unit: String,
    /// 是否必填
    pub required: bool,
    /// 是否有默认值
    pub has_default: bool,
    /// 默认值文本（提交时按类型解析；解析失败红字拦截）
    pub default_text: String,
    /// 枚举选项（逗号分隔；仅 Enum 类型用）
    pub enum_options: String,
    /// 说明（字段名 tooltip 展示给用户）
    pub doc: String,
}

impl FieldRow {
    /// 一行空白字段（浮点 / 无单位 / 非必填 / 无默认）。
    #[must_use]
    pub fn blank() -> Self {
        Self {
            name: String::new(),
            field_type: FieldType::Float,
            unit: String::new(),
            required: false,
            has_default: false,
            default_text: String::new(),
            enum_options: String::new(),
            doc: String::new(),
        }
    }

    /// [`FieldDef`] → 编辑行（快照：默认值反序列化为文本、枚举选项逗号拼接）。
    #[must_use]
    pub fn from_def(def: &FieldDef) -> Self {
        let enum_options = match &def.field_type {
            FieldType::Enum(options) => options.join(","),
            _ => String::new(),
        };
        Self {
            name: def.name.clone(),
            field_type: def.field_type.clone(),
            unit: def.unit.clone().unwrap_or_default(),
            required: def.required,
            has_default: def.default.is_some(),
            default_text: def
                .default
                .as_ref()
                .map(default_to_text)
                .unwrap_or_default(),
            enum_options,
            doc: def.doc.clone(),
        }
    }

    /// 编辑行 → [`FieldDef`]（默认值按类型解析；错误文案直接可显示）。
    ///
    /// # Errors
    /// 字段名为空 / 枚举无选项 / 默认值解析失败 / 枚举默认值不在选项内。
    pub fn to_def(&self) -> Result<FieldDef, String> {
        if self.name.trim().is_empty() {
            return Err("存在未命名的字段行".to_string());
        }
        let options = if matches!(self.field_type, FieldType::Enum(_)) {
            let opts: Vec<String> = self
                .enum_options
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            if opts.is_empty() {
                return Err(format!(
                    "枚举字段 {} 需要至少一个选项（逗号分隔）",
                    self.name.trim()
                ));
            }
            opts
        } else {
            Vec::new()
        };
        let field_type = match &self.field_type {
            FieldType::Enum(_) => FieldType::Enum(options),
            other => other.clone(),
        };
        let mut def = FieldDef::new(self.name.trim(), field_type, self.doc.clone());
        if !self.unit.trim().is_empty() {
            def = def.with_unit(self.unit.trim());
        }
        if self.required {
            def = def.required();
        }
        if self.has_default {
            let value = parse_default(&self.default_text, &def.field_type).ok_or_else(|| {
                format!(
                    "字段 {} 默认值不是合法的 {}",
                    self.name.trim(),
                    type_label(&def.field_type)
                )
            })?;
            if let (FieldType::Enum(opts), Value::Text(s)) = (&def.field_type, &value)
                && !opts.contains(s)
            {
                return Err(format!("字段 {} 默认值不在枚举选项内", self.name.trim()));
            }
            def = def.with_default(value);
        }
        Ok(def)
    }
}

/// 默认值文本 → [`Value`]（按字段类型解析；失败 `None`）。
///
/// Ref 不支持默认值（需要元素 id，GUI 首版不做）。
fn parse_default(text: &str, field_type: &FieldType) -> Option<Value> {
    match field_type {
        FieldType::Bool => match text.trim() {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => None,
        },
        FieldType::Int => text.trim().parse::<i64>().ok().map(Value::Int),
        FieldType::Float => text.trim().parse::<f64>().ok().map(Value::Float),
        FieldType::Text => Some(Value::Text(text.to_string())),
        FieldType::Enum(_) => Some(Value::Text(text.trim().to_string())),
        FieldType::Ref => None,
    }
}

/// 默认值 → 文本（快照回读用）。
fn default_to_text(v: &Value) -> String {
    match v {
        Value::Bool(b) => format!("{b}"),
        Value::Int(i) => format!("{i}"),
        Value::Float(f) => format!("{f}"),
        Value::Text(s) => s.clone(),
        Value::Ref(r) => format!("{r}"),
    }
}

/// 类型编辑器状态（G5-M20：Types 页——新建 / 编辑 [`PropertySchema`]）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TypesEdit {
    /// 编辑中的模式名（新建模式可填；编辑模式只读——不做改名语义）
    pub type_name: String,
    /// 编辑中的字段行
    pub fields: Vec<FieldRow>,
    /// 新建（`true`）/ 编辑已有（`false`）
    pub is_new: bool,
    /// 最近一次保存 / 校验失败的原因（红字显示；成功即关闭编辑器）
    pub message: Option<String>,
}

impl TypesEdit {
    /// 新建空白类型（一行空白字段，方便直接改）。
    #[must_use]
    pub fn new_blank() -> Self {
        Self {
            type_name: String::new(),
            fields: vec![FieldRow::blank()],
            is_new: true,
            message: None,
        }
    }

    /// 编辑已有类型（快照；type_name 锁死只读）。
    #[must_use]
    pub fn from_schema(schema: &PropertySchema) -> Self {
        Self {
            type_name: schema.type_name.clone(),
            fields: schema.fields.iter().map(FieldRow::from_def).collect(),
            is_new: false,
            message: None,
        }
    }

    /// 校验并构造 [`PropertySchema`]（保存前调用；错误文案直接可显示）。
    ///
    /// # Errors
    /// 类型名为空 / 无字段行 / 字段行不合法 / 字段名重复。
    pub fn build_schema(&self) -> Result<PropertySchema, String> {
        if self.type_name.trim().is_empty() {
            return Err("类型名不能为空".to_string());
        }
        if self.fields.is_empty() {
            return Err("类型至少需要一个字段".to_string());
        }
        let mut fields = Vec::with_capacity(self.fields.len());
        for row in &self.fields {
            let def = row.to_def()?;
            if fields.iter().any(|f: &FieldDef| f.name == def.name) {
                return Err(format!("字段名重复：{}", def.name));
            }
            fields.push(def);
        }
        Ok(PropertySchema {
            type_name: self.type_name.trim().to_string(),
            fields,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn float_field() -> FieldDef {
        FieldDef::new("width", FieldType::Float, "宽")
            .with_unit("mm")
            .required()
            .with_default(Value::Float(600.0))
    }

    #[test]
    fn blank_row_has_no_name_and_fails() {
        let err = FieldRow::blank().to_def().unwrap_err();
        assert!(err.contains("未命名"));
    }

    #[test]
    fn schema_snapshot_roundtrip() {
        let schema = PropertySchema::new("DEMO")
            .with_field(float_field())
            .with_field(FieldDef::new("material", FieldType::Text, "材质"))
            .with_field(FieldDef::new(
                "install",
                FieldType::Enum(vec!["架空".to_string(), "埋地".to_string()]),
                "敷设方式",
            ));
        let edit = TypesEdit::from_schema(&schema);
        let rebuilt = edit.build_schema().unwrap();
        assert_eq!(rebuilt, schema, "快照 → 构造应无损往返");
    }

    #[test]
    fn default_text_parses_by_type() {
        assert_eq!(
            parse_default("1.5", &FieldType::Float),
            Some(Value::Float(1.5))
        );
        assert_eq!(parse_default("42", &FieldType::Int), Some(Value::Int(42)));
        assert_eq!(parse_default("4.5", &FieldType::Int), None);
        assert_eq!(
            parse_default("true", &FieldType::Bool),
            Some(Value::Bool(true))
        );
        assert_eq!(
            parse_default("false", &FieldType::Bool),
            Some(Value::Bool(false))
        );
        assert_eq!(parse_default("yes", &FieldType::Bool), None);
        assert_eq!(
            parse_default("任意文本", &FieldType::Text),
            Some(Value::Text("任意文本".to_string()))
        );
        assert_eq!(
            parse_default("x", &FieldType::Ref),
            None,
            "Ref 不支持默认值"
        );
    }

    #[test]
    fn enum_field_options_and_default_checked() {
        // 空选项 → 拒绝
        let mut row = FieldRow::blank();
        row.name = "install".to_string();
        row.field_type = FieldType::Enum(Vec::new());
        row.enum_options = String::new();
        assert!(row.to_def().unwrap_err().contains("至少一个选项"));

        // 默认值在选项内 → Ok（选项按逗号切分并去空白）
        row.enum_options = "架空, 埋地".to_string();
        row.has_default = true;
        row.default_text = "埋地".to_string();
        let def = row.to_def().unwrap();
        assert_eq!(
            def.field_type,
            FieldType::Enum(vec!["架空".to_string(), "埋地".to_string()])
        );
        assert_eq!(def.default, Some(Value::Text("埋地".to_string())));

        // 默认值不在选项内 → 拒绝
        row.default_text = "悬空".to_string();
        assert!(row.to_def().unwrap_err().contains("不在枚举选项内"));
    }

    #[test]
    fn duplicate_field_names_rejected() {
        let mut row = FieldRow::blank();
        row.name = "width".to_string();
        let edit = TypesEdit {
            type_name: "DEMO".to_string(),
            fields: vec![row.clone(), row],
            is_new: true,
            message: None,
        };
        assert!(edit.build_schema().unwrap_err().contains("字段名重复"));
    }

    #[test]
    fn type_name_and_fields_required() {
        let mut row = FieldRow::blank();
        row.name = "w".to_string();
        // 空名
        let edit = TypesEdit {
            type_name: "  ".to_string(),
            fields: vec![row],
            is_new: true,
            message: None,
        };
        assert!(edit.build_schema().unwrap_err().contains("类型名不能为空"));
        // 空字段表
        let edit = TypesEdit {
            type_name: "DEMO".to_string(),
            fields: Vec::new(),
            is_new: true,
            message: None,
        };
        assert!(
            edit.build_schema()
                .unwrap_err()
                .contains("至少需要一个字段")
        );
    }

    #[test]
    fn ref_field_cannot_have_default() {
        let mut row = FieldRow::blank();
        row.name = "host".to_string();
        row.field_type = FieldType::Ref;
        row.has_default = true;
        row.default_text = "E1".to_string();
        assert!(row.to_def().unwrap_err().contains("默认值"));
    }

    #[test]
    fn field_types_const_covers_six_kinds() {
        assert_eq!(FIELD_TYPES.len(), 6);
        assert_eq!(type_label(&FieldType::Enum(vec![])), "枚举");
    }
}

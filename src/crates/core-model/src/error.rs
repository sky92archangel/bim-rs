//! 数据层的错误类型。

use crate::ids::{ElementId, ShapeId};
use thiserror::Error;

/// 数据层错误。
#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum ModelError {
    /// 元素不存在。
    #[error("元素不存在：{0}")]
    ElementNotFound(ElementId),

    /// 几何记录不存在。
    #[error("几何记录不存在：{0}")]
    GeometryNotFound(ShapeId),

    /// 重复的持久 id（持久 id 永不复用，见设计文档 3.3）。
    #[error("持久 id 冲突（永不复用）：{0}")]
    DuplicateId(String),

    /// 属性模式未注册。
    #[error("属性模式未注册：{0}")]
    UnknownSchema(String),

    /// 属性字段不存在或类型不匹配。
    #[error("属性字段错误：{0}")]
    PropertyField(String),

    /// 基准元素错误（如重定义 DatumKind 被禁止，见设计文档 4.6.5）。
    #[error("基准元素错误：{0}")]
    Datum(String),

    /// 关系错误（如悬空引用）。
    #[error("关系错误：{0}")]
    Relation(String),

    /// 序列化 / 反序列化失败。
    #[error("序列化失败：{0}")]
    Serde(String),
}

/// 数据层结果别名。
pub type ModelResult<T> = Result<T, ModelError>;

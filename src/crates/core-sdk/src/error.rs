//! SDK 错误类型。

use thiserror::Error;

/// SDK / 模块注册错误。
#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum SdkError {
    /// 模块声明的 SDK API 版本与当前 Core 不符——**必须拒绝加载**，
    /// 否则会在运行时以难查的方式崩。
    #[error("SDK API 版本不匹配：模块要求 v{required}，当前 Core 为 v{current}")]
    ApiVersionMismatch {
        /// 模块要求的版本
        required: u32,
        /// 当前 Core 的版本
        current: u32,
    },

    /// 模块清单非法（缺 id / 版本号格式不对等）。
    #[error("模块清单非法：{0}")]
    InvalidManifest(String),

    /// 模块 id 重复。
    #[error("模块已注册：{0}")]
    DuplicateModule(String),

    /// 品类重复注册。
    #[error("品类已注册：{0}")]
    DuplicateCategory(String),

    /// 属性模式重复注册。
    #[error("属性模式已注册：{0}")]
    DuplicateSchema(String),

    /// 找不到属性模式（校验元素属性时）。
    #[error("属性模式未注册：{0}")]
    UnknownSchema(String),

    /// **禁止重定义基准元素种类**（设计文档 4.6.5）。
    ///
    /// `DatumKind` 是 Core 的**基础设施枚举**（轴网 / 标高 / 参考面 / 参考线）。
    /// 模块可以给基准元素**加属性**、加标注规则、加专业定位规则，
    /// 但**不许新增/改名 DatumKind**——否则跨专业坐标系立刻分裂。
    #[error("禁止重定义基准元素种类 `{0}`：DatumKind 由 Core 固定（设计文档 4.6.5）")]
    DatumKindReserved(String),

    /// 元素工厂错误。
    #[error("元素工厂错误：{0}")]
    Factory(String),

    /// 标注规则错误。
    #[error("标注规则错误：{0}")]
    Annotation(String),

    /// 几何算子错误。
    #[error("几何算子错误：{0}")]
    GeometryOp(String),

    /// 参数缺失或类型不符。
    #[error("参数错误：{0}")]
    BadParameter(String),

    /// 数据层错误透传。
    #[error("数据层错误：{0}")]
    Model(#[from] core_model::ModelError),
}

/// SDK 结果别名。
pub type SdkResult<T> = Result<T, SdkError>;

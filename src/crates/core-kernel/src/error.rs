//! 内核适配层的错误类型。
//!
//! 分层约定（设计文档 2.5）：`kernel` 抛 [`KernelError`]，`model` 抛 `ModelError`，
//! `app` 汇聚为 `AppError`（携带**可展示给用户的中文消息 + 可上报的错误码**）。

use crate::handle::ShapeHandle;
use thiserror::Error;

/// 内核适配层错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum KernelError {
    /// 内核尚未初始化（例如线程本地池尚未装入内核实例）。
    #[error("内核未初始化：{0}")]
    NotInitialized(String),

    /// 形状句柄无效，或已被 [`crate::Kernel::release`] 释放。
    #[error("形状句柄无效或已释放：{0}")]
    InvalidHandle(ShapeHandle),

    /// 几何运算失败。`op` 为算子名，便于定位。
    #[error("几何运算失败[{op}]：{msg}")]
    Geometry {
        /// 算子名（如 `"fuse"`、`"project_edges"`）。
        op: &'static str,
        /// 失败原因（底层消息）。
        msg: String,
    },

    /// 后端不支持该操作（例如骨架阶段的占位后端）。
    #[error("后端不支持该操作：{0}")]
    Unsupported(&'static str),

    /// 参数非法（如非正的尺寸、非单位向量等）。
    #[error("参数非法：{0}")]
    InvalidArgument(String),

    /// 运算结果为空（设计文档 §5.3）：例如要求"恰好一个实体"却一个都没有。
    ///
    /// **与 [`KernelError::NotSingleSolid`] 配对**：空结果与多体分别报不同错，
    /// 绝不用 `.first().unwrap_or(default)` 静默取第一个。
    #[error("运算结果为空：{0}")]
    EmptyResult(String),

    /// 期望**恰好一个**实体，实际得到多个（设计文档 §5.3）。
    ///
    /// 携带 `count`：调用方据此能给出"这里有 N 个不相连体，请先拆分"的可读信息。
    #[error("期望恰好一个实体，实际得到 {count} 个")]
    NotSingleSolid {
        /// 实际的实体个数。
        count: usize,
    },

    /// 后端内部错误（透传自底层库）。
    #[error("内核内部错误：{0}")]
    Backend(String),
}

/// 内核适配层的结果别名。
pub type KernelResult<T> = Result<T, KernelError>;

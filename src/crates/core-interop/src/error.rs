//! 互操作层的错误类型。

use thiserror::Error;

use crate::capability::Format;

/// 互操作层错误。
///
/// ⚠ 不派生 `Clone`：`KernelError` 未实现 `Clone`（它的 `Unsupported(&'static str)`
/// 与 `Geometry { op: &'static str, .. }` 都不必为可克隆付出代价）。
/// 本错误类型只需可比较、可展示。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum InteropError {
    /// 认不出格式。
    #[error("无法识别文件格式（文件名：{0}）")]
    UnknownFormat(String),

    /// 该格式在本阶段不支持该方向（导入 / 导出）。
    #[error("{format:?} 不支持{direction}")]
    Unsupported {
        /// 格式
        format: Format,
        /// 方向（"导入" / "导出"）
        direction: &'static str,
    },

    /// **合规红线**：该格式的库（许可证）不允许在本进程内使用，必须走独立转换服务。
    #[error(
        "{format:?} 必须经独立转换服务调用（{library}，{license}）；\
         绝不可链接进主程序，见设计文档 7.8"
    )]
    RequiresIsolation {
        /// 格式
        format: Format,
        /// 提供该能力的库
        library: &'static str,
        /// 该库的许可证
        license: &'static str,
    },

    /// 解析失败。
    #[error("解析失败：{0}")]
    Parse(String),

    /// 写出失败。
    #[error("写出失败：{0}")]
    Write(String),

    /// 数据层错误透传。
    #[error("数据层错误：{0}")]
    Model(#[from] core_model::ModelError),

    /// 内核层错误透传。
    #[error("内核层错误：{0}")]
    Kernel(#[from] core_kernel::KernelError),
}

/// 互操作层结果别名。
pub type InteropResult<T> = Result<T, InteropError>;

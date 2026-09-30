//! 应用层错误：把下面四层的错误**收敛成一个类型**。
//!
//! 为什么要收敛：UI 只应该看到一个错误类型。否则每个按钮的回调里都要
//! `match` 四种错误，界面代码立刻失控。

use core_model::{GeometryKey, ShapeId};
use thiserror::Error;

/// 应用层错误。
#[derive(Debug, Error)]
pub enum AppError {
    /// 数据层错误。
    #[error("数据层：{0}")]
    Model(#[from] core_model::ModelError),

    /// 内核层错误。
    #[error("内核层：{0}")]
    Kernel(#[from] core_kernel::KernelError),

    /// 图纸层错误。
    #[error("图纸层：{0}")]
    Doc(#[from] core_doc::DocError),

    /// 互操作层错误。
    #[error("互操作层：{0}")]
    Interop(#[from] core_interop::InteropError),

    /// SDK 层错误。
    #[error("SDK：{0}")]
    Sdk(#[from] core_sdk::SdkError),

    /// **缺少几何装载器**：文档里的 BREP 文本无法变回内核句柄。
    ///
    /// 这是 M1 的交付物（真实 `occt-wasm` 后端接线后，`ShapeLoader` 才有实现）。
    /// 现在明确报错，而不是假装成功。
    #[error("几何装载器未安装：无法把几何记录 {0} 重建为内核句柄（M1 交付物）")]
    LoaderMissing(ShapeId),

    /// 配方不可执行（步骤顺序错 / 输入个数不对 / 引用了不存在的中间结果）。
    #[error("配方不可执行：{0}")]
    RecipeNotExecutable(String),

    /// 找不到与某几何键对应的子形状（关联更新时）。
    #[error("找不到几何键对应的子形状：{0:?}")]
    MissingSubShape(Box<GeometryKey>),

    /// `.bimdoc` 容器读写错误（zip 形态与目录形态共用同一套语义）。
    ///
    /// 覆盖：缺文件、魔数 / 版本不认识、JSON 坏、zip 坏、几何副本不一致。
    /// 设计文档 §1 的精神：**宁可报错不可猜**——绝不静默返回一个空文档。
    ///
    /// 底层 `std::io::Error` 一律在此被收敛成带上下文的文字（含出错文件名），
    /// 因此**不再单独设一个 `Io` 变体**——那会是一个无人构造的死变体。
    #[error("文档容器：{0}")]
    Container(String),

    /// **该能力尚未实现**——**响亮拒绝**，绝不假装成功 / 静默返回空结果。
    ///
    /// 与 [`LoaderMissing`](AppError::LoaderMissing) 同一精神：未落地的能力必须
    /// 报错，而不是让调用方以为"操作合法但恰好没命中"。
    ///
    /// 现有调用者：三维拾取 [`KernelPicker`](crate::KernelPicker) 的**边 / 顶点**
    /// 选择（首版 B4.1b-2 只做面）。因此本变体**有真实构造点，不是死变体**。
    #[error("不支持：{0}")]
    Unsupported(String),
}

/// 应用层结果别名。
pub type AppResult<T> = Result<T, AppError>;

//! 图纸层的错误类型。

use core_model::{DatumKind, ElementId, ViewId};
use thiserror::Error;

use crate::ids::EntityId;

/// 图纸层错误。
#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum DocError {
    /// 视图不存在。
    #[error("视图不存在：{0}")]
    ViewNotFound(ViewId),

    /// 图纸不存在。
    #[error("图纸不存在：{0}")]
    SheetNotFound(core_model::SheetId),

    /// 图元不存在。
    #[error("图元不存在：{0}")]
    EntityNotFound(EntityId),

    /// 标注错误（如找不到宿主图元）。
    #[error("标注错误：{0}")]
    Annotation(String),

    /// 制图标准错误（如引用了未定义的图层）。
    #[error("制图标准错误：{0}")]
    Standard(String),

    /// 缺少生成该视图所需的基准元素（设计文档 4.6.1：视图生成依赖轴网 / 标高）。
    #[error("缺少基准元素：{0}")]
    MissingDatum(ElementId),

    /// 该视图类型不支持此操作。
    #[error("视图类型不支持：{0}")]
    UnsupportedViewKind(String),

    /// 基准元素类型不符：`from_level` 要求 `DatumKind::Level`、`from_grid` 要求
    /// `DatumKind::Grid`，传入了别种基准（如把轴网当标高用）时报此错。
    ///
    /// 这是把设计文档 4.6.1 理由③"没有基准元素就没有平立剖体系"变成**构造期硬校验**
    /// 的地方——宁可在建视图时就拒绝，也不放一张"挂错基准"的图出去。
    #[error("基准元素类型不符：{id}（期望 {expected:?}，实为 {found:?}）")]
    WrongDatumKind {
        /// 出错的基准元素 id
        id: ElementId,
        /// 期望的基准种类
        expected: DatumKind,
        /// 实际的基准种类
        found: DatumKind,
    },
}

/// 图纸层结果别名。
pub type DocResult<T> = Result<T, DocError>;

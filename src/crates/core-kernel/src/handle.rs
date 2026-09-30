//! 会话态形状句柄。

use std::fmt;

/// **会话态**形状句柄：内核 arena 内的临时编号。
///
/// ## 生命周期（设计文档 3.3，最重要的设计点之一）
///
/// - 生命周期 = **进程会话**；程序一关即失效；
/// - **绝不可存盘**——文档里存的是 `core-model` 的 `ShapeId`（UUID v7，持久态）；
/// - 句柄可丢：丢了不影响文档，下次用时从持久几何（BREP/STEP 文本）重建。
///
/// ## 与 `ShapeId` 的分工
///
/// | 概念 | 归属 | 生命周期 | 用途 |
/// |---|---|---|---|
/// | `ShapeHandle`（本类型） | `core-kernel`（occt-wasm arena） | 会话 | 当前会话里算几何 |
/// | `ShapeId`（UUID v7） | `core-model` | 与文档同寿、可存盘 | 构件在文档里的"身份证号" |
///
/// 二者的映射表由 `core-app` 维护（见 crate 文档"差异 1"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShapeHandle(pub u32);

impl ShapeHandle {
    /// 取出底层编号。
    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

impl fmt::Display for ShapeHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

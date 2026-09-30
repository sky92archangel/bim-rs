//! # core-kernel —— 内核适配层（五层架构第 1 层）
//!
//! **唯一直接接触 `occt-wasm` 的地方。** 上层只看到本 crate 定义的类型
//! （[`ShapeHandle`]、[`MeshData`]、[`Projection`]、[`ViewDef`]、[`KernelError`]），
//! 将来若更换几何内核，只需改本 crate 一个地方（设计文档 1.2 铁律 1、目标 G3）。
//!
//! ## 与设计文档的两处差异（务必知悉）
//!
//! 1. **门面返回 `ShapeHandle` 而非 `ShapeId`。**
//!    设计文档 3.1 的示例把 `Kernel` 方法的返回值写成 `ShapeId`，但 3.3 已明确：
//!    `ShapeId` 是 **`core-model` 管的持久态**（UUID v7、可存盘），`ShapeHandle`
//!    才是**内核会话态**（arena 临时编号）。若门面使用 `ShapeId`，`core-kernel`
//!    就要依赖 `core-model`，直接违反 1.2 铁律 2。因此本 crate 的门面**只使用
//!    [`ShapeHandle`]**；`ShapeId ↔ ShapeHandle` 的映射由 `core-app` 的几何解析器维护。
//!
//! 2. **真实 occt-wasm 后端已接线（M0 接线，M1 补齐）。**
//!    默认构建提供纯 Rust 的 [`MockBackend`]（[`BackendKind::Mock`]）；用
//!    `--features occt` 启用 [`occt::OcctBackend`]（真实 `occt-wasm` v4 + wasmtime，
//!    [`BackendKind::Real`]）。见 [`Kernel::new`]。
//!    ⚠ 真实后端**必须 `--release`**：debug 下 wasmtime 编译 21 MB WASM 慢约 100 倍。
//!
//! 3. **两个后端共用同一份槽位 arena。**
//!    内核的 `ShapeHandle` 无法从裸 `u32` 重建（字段 `pub(crate)`），且内核**没有**
//!    `is_alive`。所以适配层自持一张槽位表（[`arena::SlotArena`]），把"对外编号 ↔
//!    内核句柄"的映射、存活判定与重复释放检测都收在一处。两个后端共用同一实现，
//!    避免"换后端才暴露"的语义差异。
//!
//! ## 单位与坐标约定（设计文档 2.5）
//!
//! - 长度单位 **毫米（mm）**；
//! - 角度单位 **弧度（rad）**；
//! - 坐标系统一 **Z 轴朝上、右手系**。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

pub mod arena;
pub mod backend;
pub mod error;
pub mod handle;
pub mod kernel;
pub mod mock;
#[cfg(feature = "occt")]
pub mod occt;
pub mod pool;
pub mod tolerance;
pub mod types;

/// **已知上游缺陷钉子**（§5.5 第 2 层）——源码级物理隔离：只在测试 + `occt` 下编译。
///
/// 它钉的是 `occt-wasm`（本 fork）的上游缺陷，**不是**本平台的 bug。详见文件头注释。
///
/// # ⚠ 为什么模块名以 `zz_` 打头、并用 `#[path]` 指向规范文件
///
/// libtest **按测试全名（模块路径）字典序**执行用例。wasm trap 会让被调用的内核
/// 线性内存长大且不回收；若本模块（原名 `known_upstream_defects`，首字母 `k`）
/// 排在 `occt::tests`（首字母 `o`）**之前**，trap 的污染会波及紧随其后的
/// `occt::tests::a_facade_new_returns_the_real_backend`——该用例会**再建一具**内核，
/// 于是在分配数 MB 时 OOM，进程以 `0xc0000409` 中止（日志里**没有** FAILED 行）。
///
/// 故让本模块**排到最后**执行：`zz_` 前缀保证其字典序在 `types` 之后。文件名仍为
/// 规范要求的 `known_upstream_defects.rs`（由 `#[path]` 指过来），门禁 6 的过滤词
/// `known_upstream_defects` 也仍能匹配（是测试全名的子串）。
#[cfg(all(test, feature = "occt"))]
#[path = "known_upstream_defects.rs"]
mod zz_known_upstream_defects;

pub use arena::SlotArena;
pub use backend::{BackendKind, KernelBackend};
pub use error::{KernelError, KernelResult};
pub use handle::ShapeHandle;
pub use kernel::Kernel;
pub use mock::MockBackend;
#[cfg(feature = "occt")]
pub use occt::{FACE_HASH_UPPER_BOUND, OcctBackend};
pub use types::{
    BoundingBox, CurveGeometry, CurveKind, CurveRange, Evolution, FaceEvolution, FaceGroup,
    LineClass, MeshData, PointOnEdge, PointOnFace, ProjectedCurve, Projection, SubShapeKind,
    ViewDef,
};

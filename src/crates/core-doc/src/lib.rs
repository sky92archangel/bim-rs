//! # core-doc —— 二维图纸层（五层架构第 3 层）
//!
//! 承载 **能力②（HLR 显示，消费侧）** 与 **能力③（CAD 平面图纸编辑）**。
//!
//! ## 本层负责什么（设计文档 6.2 的分工表）
//!
//! **HLR 算法由 `occt-wasm` 全权负责，本层不自研。** 本层要做的是
//! **HLR 之外**的三件事：
//!
//! | # | 事项 | 在哪个文件 |
//! |---|---|---|
//! | ① | 把 HLR 结果转成**可编辑的二维实体**（接线，不是算法） | [`entity`] |
//! | ② | 标注与**关联更新**（出图层的本职） | [`annotation`] / [`dependency`] |
//! | ③ | 图纸语义层：图层 / 线型 / 图幅 / 比例（制图国标） | [`standard`] |
//!
//! ## 依赖边界
//!
//! 允许依赖：`core-model`、`glam`（+ 将来的 `kurbo` / `lyon`）。
//! **禁止依赖 `occt-wasm`**——几何算法一律经 `core-kernel` 的抽象走
//! （设计文档 1.2 铁律 2）。
//!
//! 因此 [`view::ViewOrientation`] **故意不复用** `core_kernel::ViewDef`，
//! 二者由 `core-app` 负责转换。
//!
//! ## 与 `capability` 编号的对应
//!
//! ```text
//! core-kernel.project_edges ──▶ Projection { visible: [ProjectedCurve], hidden: [...] }
//!                                        │
//!                       ┌────────────────┴────────────────┐
//!                       ▼                                 ▼
//!            Entity2D{kind: Line/Circle/…}        按 LineClass 分图层
//!            + source: GeometryKey               （可见→OUTLINE，不可见→HIDDEN）
//!                       │
//!                       ▼
//!               Annotation::bound_to(GeometryKey)
//!                       │
//!                       ▼
//!               DependencyGraph  ← 模型变了，只重投这些
//! ```

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

pub mod annotation;
pub mod dependency;
pub mod drawing;
pub mod entity;
pub mod error;
pub mod ids;
pub mod picker;
pub mod sheet;
pub mod standard;
pub mod view;

pub use annotation::{Annotation, AnnotationKind, AnnotationStyle};
pub use dependency::DependencyGraph;
pub use drawing::Drawing;
pub use entity::{Entity2D, EntityKind, EntityStyle};
pub use error::{DocError, DocResult};
pub use ids::EntityId;
pub use picker::{
    Camera, PickFilter, PickKind, PickParam, PickResult, Ray, TriangleHit, ray_hits_mesh,
    ray_triangle_intersect,
};
pub use sheet::{Placement, Sheet};
pub use standard::{
    DraftingStandard, LayerDef, LineType, Rgb, SheetFormat, TextStyle, TitleBlockDef,
};
pub use view::{View, ViewKind, ViewOrientation};

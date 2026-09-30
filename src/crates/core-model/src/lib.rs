//! # core-model —— 数据层（五层架构第 2 层）
//!
//! 承载**能力①（B-Rep 信息存储与管理）**：文档、元素、属性、事务、基准元素。
//!
//! ## 铁律（设计文档 1.2 / 2.2）
//!
//! 本 crate **完全不 `import` `occt_wasm`**，也**不依赖任何其它 `core-*`**。
//! 几何在这里只是"一段 BREP/STEP 文本 + 一个持久 id"（见 [`GeometryRecord`]），
//! 需要算的时候由上层经 `core-kernel` 现场重建句柄。
//!
//! ## 与设计文档 4.1 的一处调整
//!
//! 4.1 的元素树把 `Views` / `Sheets` 画在 `Document` 里面，但视图与图纸的本体
//! （`Entity2D` / `Annotation`）属于第 3 层 `core-doc`。若 `Document` 直接持有它们，
//! `core-model` 就要依赖 `core-doc`，与本层"零依赖"矛盾。
//!
//! 因此：**`Document` 只持有 [`ViewId`] / [`SheetId`] 集合**，`View` / `Sheet` 本体在
//! `core-doc`，二者由 `core-app` 的图纸库配对管理。
//!
//! ## 单位与坐标约定（设计文档 2.5）
//!
//! 长度 **毫米（mm）**；角度 **弧度（rad）**；坐标系 **Z 轴朝上、右手系**。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

pub mod datum;
pub mod document;
pub mod element;
pub mod error;
pub mod geometry;
pub mod graph;
pub mod ids;
pub mod property;
pub mod relation;
pub mod transaction;

pub use datum::{
    DatumConvention, DatumElement, DatumKind, DatumScope, GridDirection, GridNumbering,
};
pub use document::{Document, Meta};
pub use element::{Category, Element};
pub use error::{ModelError, ModelResult};
pub use geometry::{GeomFormat, GeometryRecord};
pub use ids::{
    ElementId, GeometryKey, LayerId, RelationId, ShapeId, SheetId, SubShapeRef, TopoKind, TopoName,
    TopoRef, ViewId,
};
pub use property::{FieldDef, FieldType, PropertyMap, PropertySchema, Value};
pub use relation::{Relation, RelationKind};
pub use transaction::{Delta, Transaction};

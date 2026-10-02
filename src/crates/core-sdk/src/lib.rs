//! # core-sdk —— 二次开发 SDK（五层架构第 4 层）
//!
//! 承载**能力⑥（模块二次开发 SDK）**：**只定义契约，不实现任何东西**。
//!
//! ## 本 crate 是"一堵墙"，也是一扇"门"
//!
//! ```text
//! modules/*  ──只允许依赖──▶  core-sdk  ──▶  core-model
//!                                ╳   （模块不许碰 core-kernel / core-doc / core-app）
//! ```
//!
//! 模块因此**看不到内核**（铁律 1 的威力在这里兑现：换几何内核，模块一行不改），
//! 也看不到图纸层（标注的实现细节被挡在外面）。
//!
//! ## 为了方便，SDK 同时是 `core-model` 的门面
//!
//! 模块里写：
//!
//! ```ignore
//! use core_sdk::model::{Element, PropertyMap, Value};   // ✅ 推荐
//! use core_model::Element;                              // ❌ 模块不该直接依赖 core-model
//! ```
//!
//! 二者等价（[`model`] 就是 `core_model` 的原样再导出），
//! 但前者让 `modules/*` 的 `Cargo.toml` 里**只有一行依赖**，
//! `cargo xtask check-deps` 也就好判了。
//!
//! ## 模块能做什么 / 不能做什么
//!
//! | 能做 | 不能做 |
//! |---|---|
//! | 注册品类、属性模式、元素工厂 | 新增 / 改名**基准元素种类**（[`Registry::try_define_datum_kind`] 会拒）|
//! | 注册标注规则（声明"标什么"）| 直接生成二维图元（那是 `core-doc`）|
//! | 注册几何算子（返回**配方**）| 直接调内核（模块看不到内核）|
//! | 注册专业图层 | 改制图国标的基础图层定义 |
//!
//! ## 版本纪律
//!
//! 任何 trait 签名变动都要把 [`SDK_API_VERSION`] +1。
//! 每个模块在 [`ModuleManifest::api_version`] 里声明自己要哪一版，
//! 不匹配就**拒绝装载**——这是唯一能防住"模块悄悄不兼容"的机制。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

pub mod annotation;
pub mod error;
pub mod factory;
pub mod geometry_op;
pub mod module;
pub mod registry;

/// **数据模型门面**：模块请统一写 `core_sdk::model::Xxx`。
pub use core_model as model;

pub use annotation::{AnnotationContext, AnnotationNeed, AnnotationRule};
pub use error::{SdkError, SdkResult};
pub use factory::{CreateRequest, ElementFactory, build_element};
pub use geometry_op::{
    BuildOp, BuildStep, GeometryOp, OpInput, OpOutput, ParamDef, ParamDefault, ParamKind,
};
pub use module::{Module, ModuleId, ModuleManifest, SDK_API_VERSION};
pub use registry::{CategoryInfo, LayerSpec, Registry, RegistrySummary};

// 常用数据模型类型在 SDK 根上再导出一层，模块侧少写 `model::` 前缀。
pub use core_model::{
    Category, DatumConvention, DatumElement, DatumKind, DatumScope, Delta, Document, Element,
    ElementId, FieldDef, FieldType, GeometryKey, GeometryRecord, LayerId, Meta, ModelError,
    PropertyMap, PropertySchema, Relation, RelationId, RelationKind, ShapeId, SheetId, Transaction,
    Value, ViewId,
};

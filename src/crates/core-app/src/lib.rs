//! # core-app —— 应用服务层（五层架构第 4.5 层）
//!
//! 本层是**用例编排层**，也是**唯一能同时看见所有下层**的地方。
//! 它本身**不做算法、不做图纸语义、不做格式解析**——它只做三件事：
//!
//! 1. **架桥**：把依赖方向不兼容的两层对接起来
//!    （`core-doc` 的图纸 → `core-interop` 的中性结构）；
//! 2. **持有跨层状态**：`ShapeId ↔ ShapeHandle` 映射、文档与图纸的配对、
//!    撤销栈；
//! 3. **收敛错误**：把下四层的错误统一成 [`AppError`]，让 UI 只处理一种。
//!
//! ## 三个服务，各自解决一个"没人管就会出事"的问题
//!
//! | 服务 | 解决什么 |
//! |---|---|
//! | [`GeometryService`] | 持久 `ShapeId` 与临时 `ShapeHandle` 的映射（**内核看不见数据层，只能在这里对账**）|
//! | [`DocumentService`] | `Document` 与 `Drawing` 分家后**必须有人配对**，顺带管撤销 / 重做 |
//! | [`InteropService`] | `core-interop` 看不见 `core-doc`，**转换只能在这里写** |
//! | [`ModuleHost`] | 模块声明与 Core 落地之间的最后一公里（专业图层的 id 分配等）|
//!
//! ## 依赖边界（设计文档 2.2）
//!
//! 允许依赖**全部** `core-kernel` / `core-model` / `core-doc` / `core-interop` / `core-sdk`。
//! **禁止**被上述任何一层反向依赖——依赖方向严格向上，`core-app` 是终点之一
//! （另一个终点是 `apps/*`）。
//!
//! ```text
//! apps/*  ──▶  core-app  ──▶  core-doc ──▶ core-model
//!                 │                ▲
//!                 ├──▶ core-interop┘（只依赖 core-kernel / core-model）
//!                 ├──▶ core-kernel
//!                 └──▶ core-sdk  ──▶ core-model
//! ```
//!
//! ## 顶层门面：[`CoreSession`]
//!
//! [`CoreSession`] 把下面四个服务串成"一个会话对象"，是全平台**唯一能同时看见
//! 五层**的装配口；它只编排、不实现算法：
//!
//! ```text
//! CoreSession ──┬─ DocumentService（文档 + 撤销栈）
//!               ├─ GeometryService（ShapeId ↔ ShapeHandle）
//!               ├─ InteropService （图纸 ⇄ 中性二维结构）
//!               └─ ModuleHost     （专业模块 + 图层落地）
//! ```
//!
//! ## 本层刻意不做的事
//!
//! - **不做几何算法**：几何一律经 [`GeometryService::kernel`] 转交内核。
//! - **不做合规判断**：DWG 的隔离红线在 `core-interop::FormatRouter` 里，
//!   本层只是转发（少一处判断，就少一处判断漏掉的可能）。
//! - **持久化只做"容器"、不做"格式语义"**：[`store`] 负责 `.bimdoc`
//!   的 zip / 目录打包解包（设计文档 4.2），"语义部分 ↔ JSON"仍由
//!   [`DocumentService::to_json`] 提供；`doc.json` 是权威副本，
//!   `geom/*.brep` 是标准导出副本，二者不一致即报错。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

pub mod document_service;
pub mod error;
pub mod evolution_align;
pub mod geometry_service;
pub mod interop_service;
pub mod loader;
pub mod module_host;
pub mod picker_service;
pub mod projection_service;
pub mod selector;
pub mod session;
pub mod sketch;
pub mod store;
pub mod topo_naming;
pub mod update_service;

pub use document_service::{DEFAULT_UNDO_LIMIT, DocumentService};
pub use error::{AppError, AppResult};
pub use evolution_align::{HistoryRecord, ResolvedDelta, resolve_delta};
pub use geometry_service::{ExecutedRecipe, GeometryService, ShapeLoader, UnavailableLoader};
pub use interop_service::{
    EXPORT_TEXT_LAYER, InteropService, VIEW_MARGIN_MM, append_title_block, layer_name_of,
    sheet_to_2d, view_to_2d,
};
pub use loader::{BrepShapeLoader, SUPPORTED_LOAD_FORMATS, can_load};
pub use module_host::{ModuleHost, parse_line_type};
pub use picker_service::KernelPicker;
pub use projection_service::{
    curve_to_entity, project_shape, project_view, projectable_shapes, reproject_into, view_def_of,
};
pub use selector::{EdgeSelector, FaceSelector};
pub use session::{CoreSession, RevolveAxis, SessionSummary};
pub use sketch::{SKETCH_CLOSED_TOL_MM, SketchEntity, is_closed_profile, normalize_arc_angles};
pub use store::{
    CONTAINER_MANIFEST, FORMAT_VERSION, open_document, open_from_dir, pack_to_dir, save_document,
};
pub use topo_naming::{
    EdgeNameTable, VertexNameTable, edge_names, edge_names_from_handle,
    face_hash_names_from_handle, face_names, face_names_from_handle, face_names_from_mesh,
    resolve_by_name, resolve_key, vertex_names, vertex_names_from_handle,
};
pub use update_service::{UpdateReport, update_after_commit};

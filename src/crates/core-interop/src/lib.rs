//! # core-interop —— 通用数据导入导出（五层架构第 1.5 层）
//!
//! 承载**能力④（通用数据导入导出）**：IFC / STEP / DXF / glTF 四格式互操作基线，
//! 外加 SVG / DWG / GIM 三个扩展格式。
//!
//! ## 本 crate 的第一性事实：**只有两项要"新买库"**
//!
//! [`capability::CAPABILITY_ITEMS`] 就是《立项方案》§9.3.1 那张表。
//! 记住这个结论，能省掉大量无谓的选型调研：
//!
//! | | 工作量项 | 要新买库吗 | 结论 |
//! |---|---|---|---|
//! | ① | 二维投影 | **不要** | 复用 `occt-wasm::project_edges` + 自写视图矩阵 |
//! | ② | 制图国标 | **不要** | 是"规则"不是"格式"，没有库可买 → 自建规则引擎 |
//! | ③ | STEP / DXF / SVG | **不要** | STEP 走内核；DXF/SVG 是序列化，手写 |
//! | ④ | IFC4X3 | **要** | `ifc-lite-core` / `bimifc-parser`（MPL-2.0）|
//! | ⑤ | DWG | **要** | `LibreDWG`（GPLv3+）→ **必须独立进程** |
//! | ⑥ | GIM | **不要**（无库可用）| 自建解析器，单列电网侧模块 |
//!
//! ## 合规红线（不可越）
//!
//! GPLv3+ 的 LibreDWG **绝不可链接进主程序**。本 crate 用三重防线钉死它：
//!
//! 1. [`Format::requires_isolation`] → [`FormatRouter::import`] **在分发前拦截**；
//! 2. [`readers::DwgImporter`] 自身**永远返回错误**；
//! 3. 根目录 `deny.toml` **禁止 `libredwg` 出现在依赖图里**。
//!
//! ## 依赖边界
//!
//! 允许：`core-kernel`、`core-model`、`dxf` / `gltf` / ifc 库。
//! **禁止依赖 `core-doc`**——二维图纸数据经中性的
//! [`drawing2d::Drawing2D`] 传入，由 `core-app` 负责转换
//! （否则 `core-doc` → 互操作 的方向会形成环）。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

pub mod capability;
pub mod detect;
pub mod drawing2d;
pub mod dxf;
pub mod dxf_reader;
pub mod error;
pub mod flow;
pub mod linetype;
pub mod readers;
pub mod router;
pub mod svg;
pub mod writers;

pub use capability::{
    CAPABILITY_ITEMS, CapabilityItem, FORMATS, Format, FormatSpec, Isolation, Support, WorkItem,
    capability_item, items_needing_new_libraries, workload_hotspots,
};
pub use detect::detect;
pub use drawing2d::{Analytic2D, Curve2D, Drawing2D, Text2D};
pub use dxf::DxfOutput;
pub use error::{InteropError, InteropResult};
pub use flow::{ExportOutput, Exporter, ImportContext, ImportReport, ImportedShape, Importer};
pub use linetype::{
    GB_BASE_WIDTH_MM, GB_LINE_TYPES, GbLineType, gb_dash_pattern_mm, gb_line_type,
    gb_total_length_mm,
};
pub use readers::{DwgImporter, DxfImporter, GltfImporter, IfcImporter, StepImporter};
pub use router::FormatRouter;
pub use svg::SvgOutput;
pub use writers::{DxfExporter, SvgExporter};

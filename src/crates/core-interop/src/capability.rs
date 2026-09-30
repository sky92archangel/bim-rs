//! **能力④（通用数据导入导出）的可核算工作量清单** —— 本文件是
//! 《轻量化BIM内核平台_立项方案》§9.3.1 那张表的**代码副本**。
//!
//! ## 一句话结论（立项方案 §9.3.1）
//!
//! > "通用数据导入导出"听上去是一句话，落到实处只有**两项**真正"要新买库"：
//! > **IFC** 与 **DWG**。其余四项要么复用内核，要么只是"规则"（没有库可买），
//! > 要么压根没有可用的开源实现（GIM）。
//!
//! ## 为什么把这张表写成代码
//!
//! 表格写在文档里会被忘掉；写在代码里就成了**可断言的约束**：
//!
//! - [`FORMATS`] 里 `isolation: SeparateProcess` 的项，[`crate::FormatRouter`]
//!   会**拒绝在进程内调用**（见 [`Format::requires_isolation`]）；
//! - `deny.toml` 里同样禁掉了 `libredwg` 这类会被误链进来的依赖；
//! - 测试 [`tests::dwg_must_not_run_in_process`] 把这条红线钉死。
//!
//! ## 许可强度次序（合规红线，不可颠倒）
//!
//! ```text
//! occt-wasm (LGPL-2.1-only, WASM 动态形态)  <  ifc-lite / bimifc (MPL-2.0)  <  LibreDWG (GPLv3+)
//!        可静态/动态链接                      文件级 copyleft，可静态链接       不可链接，只能独立进程
//! ```

use serde::{Deserialize, Serialize};

// =============================================================================
// 一、六项工作量（对应立项方案 §9.3.1 表）
// =============================================================================

/// 能力④下的六个工作量项。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WorkItem {
    /// ① 二维投影
    Projection,
    /// ② 制图国标与图纸语义层
    DraftingStandard,
    /// ③ STEP / DXF / SVG
    StepDxfSvg,
    /// ④ IFC4X3
    Ifc,
    /// ⑤ DWG
    Dwg,
    /// ⑥ GIM
    Gim,
}

impl WorkItem {
    /// 编号（文档里的 ①②③…）
    #[must_use]
    pub const fn ordinal(self) -> char {
        match self {
            Self::Projection => '①',
            Self::DraftingStandard => '②',
            Self::StepDxfSvg => '③',
            Self::Ifc => '④',
            Self::Dwg => '⑤',
            Self::Gim => '⑥',
        }
    }

    /// 是否**需要新引入第三方库**（这是本表唯一真正要回答的问题）。
    #[must_use]
    pub const fn needs_new_library(self) -> bool {
        matches!(self, Self::Ifc | Self::Dwg)
    }

    /// 是否是本能力里的**工作量热点**（决定排期与人力）。
    #[must_use]
    pub const fn is_workload_hotspot(self) -> bool {
        matches!(self, Self::DraftingStandard | Self::Gim)
    }
}

/// 部署隔离形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Isolation {
    /// 进程内：可直接链接进主程序。
    InProcess,
    /// **必须独立进程 / 微服务**：因许可证传染性，绝不可链接进主程序。
    SeparateProcess,
}

/// 一项工作量的完整说明。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityItem {
    /// 对应哪一项
    pub item: WorkItem,
    /// 标题（与文档一致）
    pub title: &'static str,
    /// 选用方案 / 库
    pub approach: &'static str,
    /// 许可证（`"—"` 表示自写，无第三方许可）
    pub license: &'static str,
    /// 部署形态
    pub isolation: Isolation,
    /// 工作量估计（人月口径）
    pub workload: &'static str,
    /// 备注 / 风险
    pub note: &'static str,
}

/// **能力④工作量清单**（六项，与立项方案 §9.3.1 表一一对应）。
pub const CAPABILITY_ITEMS: [CapabilityItem; 6] = [
    CapabilityItem {
        item: WorkItem::Projection,
        title: "二维投影",
        approach: "复用 occt-wasm::project_edges 的二维曲线 + 自写视图变换矩阵 + kurbo 绘制",
        license: "MIT / Apache-2.0（kurbo）",
        isolation: Isolation::InProcess,
        workload: "≈0.5 人月",
        note: "「投影与消隐是同一件事的两半」——不存在也不需要\"二维投影库\"",
    },
    CapabilityItem {
        item: WorkItem::DraftingStandard,
        title: "制图国标与图纸语义层",
        approach: "自建可配置规则引擎 DraftingStandard（图幅/比例/线型线宽/图层/标注样式/会签栏/图纸集）",
        license: "—（自写）",
        isolation: Isolation::InProcess,
        workload: "≈3–4 人月（**本能力最重的一段**）",
        note: "它是\"规则\"不是\"格式\"，没有库可买；且与能力③出图强耦合",
    },
    CapabilityItem {
        item: WorkItem::StepDxfSvg,
        title: "STEP / DXF / SVG",
        approach: "STEP 走 occt-wasm 原生读写；DXF / SVG 自写读写，绘制用 kurbo / lyon",
        license: "MIT（kurbo / lyon）",
        isolation: Isolation::InProcess,
        workload: "≈1.5–2 人月",
        note: "DXF 只做\"兼容读写\"子集，不做全量 AutoCAD 语义",
    },
    CapabilityItem {
        item: WorkItem::Ifc,
        title: "IFC4X3",
        approach: "ifc-lite-core 或 bimifc-parser（**二选一，实测后定**）",
        license: "MPL-2.0",
        isolation: Isolation::InProcess,
        workload: "≈2 人月（含覆盖度实测）",
        note: "MPL-2.0 为文件级 copyleft，可静态链接不传染自研代码；**先导入后写回**，覆盖度须实测",
    },
    CapabilityItem {
        item: WorkItem::Dwg,
        title: "DWG",
        approach: "LibreDWG（C 库），**包成独立转换服务**，经 IPC / HTTP 调用",
        license: "GPLv3+",
        isolation: Isolation::SeparateProcess,
        workload: "≈1.5 人月（含服务化与协议）",
        note: "**只导入不导出**；绝不可静态链接进主程序。ODA 一阶段不引入，将来仅替换转换服务内部实现",
    },
    CapabilityItem {
        item: WorkItem::Gim,
        title: "GIM",
        approach: "自建解析器（国网非公开标准，**无可用开源实现**）",
        license: "—（自写）",
        isolation: Isolation::InProcess,
        workload: "2–3 人月（单列\"电网侧模块\"）",
        note: "不进本期 Core，作为电网侧独立模块；parser 之外还要维护国网版本适配",
    },
];

/// 按项查表。
#[must_use]
pub fn capability_item(item: WorkItem) -> &'static CapabilityItem {
    // 六项常量表，必有一项匹配；用 `unwrap_or` 落到首项以满足「无 panic」门禁。
    CAPABILITY_ITEMS
        .iter()
        .find(|c| c.item == item)
        .unwrap_or(&CAPABILITY_ITEMS[0])
}

/// 真正需要新引入第三方库的项（本表的核心结论）。
#[must_use]
pub fn items_needing_new_libraries() -> Vec<&'static CapabilityItem> {
    CAPABILITY_ITEMS
        .iter()
        .filter(|c| c.item.needs_new_library())
        .collect()
}

/// 工作量热点项。
#[must_use]
pub fn workload_hotspots() -> Vec<&'static CapabilityItem> {
    CAPABILITY_ITEMS
        .iter()
        .filter(|c| c.item.is_workload_hotspot())
        .collect()
}

// =============================================================================
// 二、格式矩阵（四格式互操作基线 + 扩展格式）
// =============================================================================

/// 受支持的格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Format {
    /// STEP（AP203 / AP214）—— 原计划走内核，现推迟（上游 occt-wasm 4.x 构建形态缺陷）
    Step,
    /// DXF —— 兼容读写
    Dxf,
    /// SVG —— 出图出口
    Svg,
    /// glTF —— 三维预览
    Gltf,
    /// IFC4X3 —— 建筑行业互操作基线
    Ifc4x3,
    /// DWG —— 存量资源，只导入
    Dwg,
    /// GIM —— 国网电网工程数据
    Gim,
}

impl Format {
    /// 人类可读名。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Step => "STEP",
            Self::Dxf => "DXF",
            Self::Svg => "SVG",
            Self::Gltf => "glTF",
            Self::Ifc4x3 => "IFC4X3",
            Self::Dwg => "DWG",
            Self::Gim => "GIM",
        }
    }

    /// 常见扩展名（小写，不含点）。
    #[must_use]
    pub const fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Step => &["step", "stp"],
            Self::Dxf => &["dxf"],
            Self::Svg => &["svg"],
            Self::Gltf => &["gltf", "glb"],
            Self::Ifc4x3 => &["ifc", "ifczip", "ifcxml"],
            Self::Dwg => &["dwg"],
            Self::Gim => &["gim"],
        }
    }

    /// 按扩展名猜格式。
    #[must_use]
    pub fn from_extension(ext: &str) -> Option<Self> {
        let ext = ext.trim_start_matches('.').to_ascii_lowercase();
        // 顺序敏感：ifc 与 step 扩展名不同，无歧义；glb 归 glTF。
        const ALL: [Format; 7] = [
            Format::Step,
            Format::Dxf,
            Format::Svg,
            Format::Gltf,
            Format::Ifc4x3,
            Format::Dwg,
            Format::Gim,
        ];
        ALL.into_iter()
            .find(|f| f.extensions().contains(&ext.as_str()))
    }

    /// 从文件名取扩展名并猜格式。
    #[must_use]
    pub fn from_filename(filename: &str) -> Option<Self> {
        let ext = std::path::Path::new(filename)
            .extension()
            .and_then(|e| e.to_str())?;
        Self::from_extension(ext)
    }

    /// 该格式的表格项。
    #[must_use]
    pub fn spec(self) -> &'static FormatSpec {
        FORMATS
            .iter()
            .find(|f| f.format == self)
            .unwrap_or(&FORMATS[0])
    }

    /// **是否必须在独立进程中处理**（合规红线）。
    #[must_use]
    pub const fn requires_isolation(self) -> bool {
        matches!(self, Self::Dwg)
    }

    /// 对应的工作量项（SVG / glTF 不单列，归 ③ 与四格式基线）。
    #[must_use]
    pub const fn work_item(self) -> Option<WorkItem> {
        match self {
            Self::Step | Self::Dxf | Self::Svg => Some(WorkItem::StepDxfSvg),
            Self::Ifc4x3 => Some(WorkItem::Ifc),
            Self::Dwg => Some(WorkItem::Dwg),
            Self::Gim => Some(WorkItem::Gim),
            Self::Gltf => None,
        }
    }
}

/// 本阶段的实现状态。
///
/// ## `Native` 的准确读法（§8.1，**必须**按此理解）
///
/// `Native` 一律读作「**进程内已实现、可直接调用**」——它**不等于**"由 `occt-wasm`
/// 内核实现"。历史上 [`Support::label`] 曾把它叫"内核原生"，那是遗留措辞：
/// 本阶段四格式基线里，**只有 STEP 原本走内核**（且已证伪、退回 `NotPlanned`），
/// 而 **IFC / DXF / glTF 三者全是第三方库承载**（`ifc-lite-core` / `dxf` / `gltf`）。
/// 因此凡用 `Native` 的格式，其 [`FormatSpec::note`] **必须写明实现来源**
/// （哪个库 / 是否自写），不能让读者误以为"内核原生"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Support {
    /// **进程内已实现可用**（≠"由 occt-wasm 内核实现"；第三方库或自写实现也算）。
    Native,
    /// 计划实现（M4）。⚠ **M4 收工后此变体不应再出现在 [`FORMATS`] 中**——
    /// 否则就是一个"悬空承诺"（它的 [`Support::is_usable_in_process`] 口径为真，
    /// 会让人误以为已交付）。由 `no_format_is_left_with_a_dangling_planned_promise` 钉住。
    Planned,
    /// **必须经独立转换服务**，主程序内不可用。
    ViaSeparateProcess,
    /// 本阶段不做。
    NotPlanned,
}

impl Support {
    /// 是否"本进程内就能用"。
    #[must_use]
    pub const fn is_usable_in_process(self) -> bool {
        matches!(self, Self::Native | Self::Planned)
    }

    /// 中文标签。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Native => "进程内可用",
            Self::Planned => "计划实现",
            Self::ViaSeparateProcess => "独立进程",
            Self::NotPlanned => "本阶段不做",
        }
    }
}

/// 一个格式的支持矩阵行。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatSpec {
    /// 格式
    pub format: Format,
    /// 导入支持
    pub import: Support,
    /// 导出支持
    pub export: Support,
    /// 依赖库
    pub library: &'static str,
    /// 许可证
    pub license: &'static str,
    /// 部署形态
    pub isolation: Isolation,
    /// 备注
    pub note: &'static str,
}

/// **格式支持矩阵**（设计文档 7.6 的能力矩阵）。
pub const FORMATS: [FormatSpec; 7] = [
    FormatSpec {
        format: Format::Step,
        import: Support::NotPlanned,
        export: Support::NotPlanned,
        library: "待定（推迟至 OCCT sidecar 里程碑）",
        license: "待定（推迟至 OCCT sidecar 里程碑）",
        isolation: Isolation::InProcess,
        note: "内核原生 STEP 读写已被实测证伪：本 fork 的 STEP 出口触发上游 occt-wasm 的 \
               wasm trap（uninitialized element），故推迟至 OCCT sidecar 里程碑",
    },
    FormatSpec {
        format: Format::Dxf,
        import: Support::Native,
        export: Support::Native,
        library: "自写导出 + dxf crate 导入",
        license: "MIT（dxf crate）",
        isolation: Isolation::InProcess,
        note: "进程内实现来源：自写序列化器导出 + dxf crate 读取（第三方库，MIT），非 occt-wasm 内核实现；兼容子集读写，不做全量 AutoCAD 语义",
    },
    FormatSpec {
        format: Format::Svg,
        import: Support::NotPlanned,
        export: Support::Native,
        library: "自写（+ kurbo / lyon）",
        license: "MIT",
        isolation: Isolation::InProcess,
        note: "进程内实现来源：自写 to_svg 序列化器（零新依赖），非 occt-wasm 内核实现；曲线细分待 M5 接 kurbo",
    },
    FormatSpec {
        format: Format::Gltf,
        import: Support::Native,
        export: Support::NotPlanned,
        library: "gltf (gltf-rs)",
        license: "MIT / Apache-2.0",
        isolation: Isolation::InProcess,
        note: "进程内实现来源：gltf crate（第三方库，MIT / Apache-2.0），非 occt-wasm 内核实现；只读结构、不进 B-Rep，只作三维预览、不作工程图依据；导出本阶段不做（occt-wasm 的 glTF 导出上游缺陷 trap）",
    },
    FormatSpec {
        format: Format::Ifc4x3,
        import: Support::Native,
        export: Support::NotPlanned,
        library: "ifc-lite-core",
        license: "MPL-2.0",
        isolation: Isolation::InProcess,
        note: "进程内实现来源：ifc-lite-core（第三方库，MPL-2.0），非 occt-wasm 内核实现；先导入后写回，覆盖度须实测",
    },
    FormatSpec {
        format: Format::Dwg,
        import: Support::ViaSeparateProcess,
        export: Support::NotPlanned,
        library: "LibreDWG",
        license: "GPLv3+",
        isolation: Isolation::SeparateProcess,
        note: "**只导入**；必须独立转换服务，绝不链接进主程序",
    },
    FormatSpec {
        format: Format::Gim,
        import: Support::NotPlanned,
        export: Support::NotPlanned,
        library: "自写解析器",
        license: "—",
        isolation: Isolation::InProcess,
        note: "国网非公开标准，无可用开源实现；单列电网侧模块，不进本期 Core",
    },
];

/// 四格式互操作基线（设计文档 7.1）。
#[must_use]
pub const fn baseline_formats() -> [Format; 4] {
    [Format::Ifc4x3, Format::Step, Format::Dxf, Format::Gltf]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_ifc_and_dwg_need_new_libraries() {
        let needs = items_needing_new_libraries();
        assert_eq!(needs.len(), 2, "全表只有两项要新买库");
        let items: Vec<WorkItem> = needs.iter().map(|c| c.item).collect();
        assert!(items.contains(&WorkItem::Ifc));
        assert!(items.contains(&WorkItem::Dwg));
    }

    #[test]
    fn projection_and_standard_do_not_need_libraries() {
        assert!(!WorkItem::Projection.needs_new_library());
        assert!(!WorkItem::DraftingStandard.needs_new_library());
        assert!(!WorkItem::StepDxfSvg.needs_new_library());
        assert!(
            !WorkItem::Gim.needs_new_library(),
            "GIM 是没库可用，不是要买库"
        );
    }

    #[test]
    fn workload_hotspots_are_standard_and_gim() {
        let hot: Vec<WorkItem> = workload_hotspots().iter().map(|c| c.item).collect();
        assert_eq!(hot, vec![WorkItem::DraftingStandard, WorkItem::Gim]);
    }

    #[test]
    fn every_work_item_has_exactly_one_row() {
        for item in [
            WorkItem::Projection,
            WorkItem::DraftingStandard,
            WorkItem::StepDxfSvg,
            WorkItem::Ifc,
            WorkItem::Dwg,
            WorkItem::Gim,
        ] {
            assert_eq!(capability_item(item).item, item);
        }
        assert_eq!(CAPABILITY_ITEMS.len(), 6);
    }

    #[test]
    fn ordinals_match_the_document_table() {
        assert_eq!(WorkItem::Projection.ordinal(), '①');
        assert_eq!(WorkItem::Gim.ordinal(), '⑥');
    }

    #[test]
    fn dwg_must_not_run_in_process() {
        // 合规红线：GPLv3+ 只允许独立进程
        assert!(Format::Dwg.requires_isolation());
        assert_eq!(Format::Dwg.spec().isolation, Isolation::SeparateProcess);
        assert_eq!(Format::Dwg.spec().import, Support::ViaSeparateProcess);
        assert_eq!(
            Format::Dwg.spec().export,
            Support::NotPlanned,
            "只导入不导出"
        );
    }

    #[test]
    fn license_order_is_respected() {
        // occt-wasm (LGPL) 与 IFC (MPL) 都在进程内；只有 GPL 家族被隔离
        assert_eq!(Format::Step.spec().isolation, Isolation::InProcess);
        assert_eq!(Format::Ifc4x3.spec().isolation, Isolation::InProcess);
        assert_eq!(Format::Ifc4x3.spec().license, "MPL-2.0");
        assert_eq!(Format::Dwg.spec().license, "GPLv3+");
    }

    #[test]
    fn ifc_is_import_only_in_phase_one() {
        assert!(!Format::Ifc4x3.requires_isolation());
        assert_eq!(Format::Ifc4x3.spec().import, Support::Native);
        assert_eq!(Format::Ifc4x3.spec().export, Support::NotPlanned);
    }

    #[test]
    fn extension_guessing_works() {
        assert_eq!(Format::from_extension("STEP"), Some(Format::Step));
        assert_eq!(Format::from_extension("stp"), Some(Format::Step));
        assert_eq!(Format::from_extension(".ifc"), Some(Format::Ifc4x3));
        assert_eq!(Format::from_extension("glb"), Some(Format::Gltf));
        assert_eq!(Format::from_extension("txt"), None);
    }

    #[test]
    fn filename_guessing_works() {
        assert_eq!(
            Format::from_filename(r"C:\dwg\1F平面.dwg"),
            Some(Format::Dwg)
        );
        assert_eq!(Format::from_filename("model.IFC"), Some(Format::Ifc4x3));
        assert_eq!(Format::from_filename("noext"), None);
    }

    #[test]
    fn work_item_mapping_is_complete_for_the_baseline() {
        for f in baseline_formats() {
            assert!(f.spec().format == f);
        }
        assert_eq!(Format::Ifc4x3.work_item(), Some(WorkItem::Ifc));
        assert_eq!(Format::Step.work_item(), Some(WorkItem::StepDxfSvg));
        assert_eq!(
            Format::Gltf.work_item(),
            None,
            "glTF 属四格式基线，不单列工作量"
        );
    }

    #[test]
    fn capability_table_serializes_with_the_expected_shape() {
        // 注意：`CapabilityItem` 的文本字段是 `&'static str`，其 `Deserialize`
        // 只在 `'de = 'static` 时成立，因此**无法**从一个临时 JSON 字符串反序列化。
        // 这里只验证序列化方向；反序列化由各层自己的 owned 类型承担。
        let value = serde_json::to_value(CAPABILITY_ITEMS).unwrap();
        let arr = value.as_array().expect("能力清单应序列化为数组");
        assert_eq!(arr.len(), 6);
        assert_eq!(
            arr[3]["item"],
            serde_json::to_value(WorkItem::Ifc).unwrap(),
            "第 4 项应是 IFC"
        );
    }

    // =========================================================================
    // 声明层守卫（M4 §5.5 第 1 层）：STEP / glTF 出口绝不能声称 Native
    // =========================================================================

    /// **判定"STEP 的声明是否诚实"**。
    ///
    /// 事实：本 fork 的 STEP 出口与 `xcaf_export_gltf` **不可用**（上游 wasm trap）。
    /// 因此诚实声明必须满足：
    /// - `import != Native`（不许自称内核原生进口）；
    /// - `export == NotPlanned`（**严格**——不许用 `Planned` / `ViaSeparateProcess`
    ///   这类"看起来可用"的变体蒙混，因为 `is_usable_in_process()` 会把它们当可用）；
    /// - 且 `note` **点名上游缺陷**（含"上游"或"trap"字样，把读者引向 OCCT sidecar）。
    ///
    /// 抽成函数是为了**让守卫本身可被攻击**（见下一条测试）——否则"永远不会红"的
    /// 空断言等于没守卫。
    fn step_declaration_is_honest(spec: &FormatSpec) -> bool {
        spec.import != Support::Native
            && spec.export == Support::NotPlanned
            && (spec.note.contains("上游") || spec.note.contains("trap"))
    }

    /// 真实声明必须诚实：STEP 既非 Native，且 note 点名上游缺陷；
    /// glTF 出口同样受 `xcaf_export_gltf` 的上游 trap 牵连，不得声明为 Native。
    #[test]
    fn step_and_gltf_export_must_not_be_declared_native_while_upstream_is_broken() {
        let spec = Format::Step.spec();
        assert_ne!(
            spec.import,
            Support::Native,
            "上游 STEP 入口不可用，import 绝不许声明为进程内可用"
        );
        assert_eq!(
            spec.export,
            Support::NotPlanned,
            "上游 STEP 出口不可用（wasm trap），export 必须是 NotPlanned"
        );
        assert!(
            spec.note.contains("上游") || spec.note.contains("trap"),
            "note 必须点名上游缺陷（含'上游'或'trap'），实得：{}",
            spec.note
        );
        assert!(
            step_declaration_is_honest(spec),
            "当前 STEP 声明本身必须是诚实的"
        );

        // glTF 出口走的是同一条坏掉的 XCAF 路径（`xcaf_export_gltf` trap），
        // glTF 在本项目只作"三维预览导入"，导出不在计划内 → 必须是 NotPlanned。
        let gltf = Format::Gltf.spec();
        assert_ne!(
            gltf.export,
            Support::Native,
            "glTF 导出不可用（上游缺陷）；不得标成 Native"
        );
        assert_ne!(
            gltf.export,
            Support::Planned,
            "glTF 导出已从 M4 移除；Planned 在 M4 结束后是悬空承诺，不得标 Planned"
        );
        assert_eq!(
            gltf.export,
            Support::NotPlanned,
            "glTF 导出必须为 NotPlanned（本阶段不做）"
        );
        assert!(
            gltf.note.contains("本阶段不做") || gltf.note.contains("trap"),
            "glTF 导出的 note 必须说明原因（本阶段不做 / 上游缺陷），实得：{}",
            gltf.note
        );
        assert!(
            gltf_export_declaration_is_honest(gltf),
            "当前 glTF 导出声明本身必须是诚实的"
        );

        // glTF 导入：M4 真实交付 → Native 认可（§8.1 读作"进程内已实现可用"）
        assert_eq!(
            gltf.import,
            Support::Native,
            "glTF 结构解析是 M4 真实交付，应为 Native（§8.1 读法）"
        );
    }

    /// **判定"glTF 导出的声明是否诚实"**（与 [`step_declaration_is_honest`] 同构）。
    ///
    /// glTF 导出必须 `NotPlanned`（既非"冒充已交付"的 `Native`，也非"悬空承诺"的
    /// `Planned`），且 `note` 说明原因（含"本阶段不做"或"trap"）。
    fn gltf_export_declaration_is_honest(spec: &FormatSpec) -> bool {
        spec.export == Support::NotPlanned
            && (spec.note.contains("本阶段不做") || spec.note.contains("trap"))
    }

    /// **守卫必须被攻击过（M2 教训）**：glTF 导出守卫同样不能是空断言。
    #[test]
    fn gltf_export_guard_detects_native_and_planned_disguise() {
        let honest = *Format::Gltf.spec();
        assert!(
            gltf_export_declaration_is_honest(&honest),
            "基线：当前 glTF 导出声明必须被判为诚实"
        );

        // 攻击 1：冒充已交付（Native）
        let mut attack_native = honest;
        attack_native.export = Support::Native;
        assert!(
            !gltf_export_declaration_is_honest(&attack_native),
            "守卫必须识破 export=Native 的冒充"
        );

        // 攻击 2：悬空承诺（Planned）
        let mut attack_planned = honest;
        attack_planned.export = Support::Planned;
        assert!(
            !gltf_export_declaration_is_honest(&attack_planned),
            "守卫必须识破 export=Planned 的悬空承诺"
        );

        // 攻击 3：把 note 洗成"看起来没问题"（去掉'本阶段不做'/'trap'字样）
        let mut attack_note = honest;
        attack_note.note = "glTF 导出已就绪";
        assert!(
            !gltf_export_declaration_is_honest(&attack_note),
            "守卫必须识破 note 洗白"
        );
    }

    // =========================================================================
    // 声明层守卫（M4 §5.5 第 2 层）：M4 收工后不得残留悬空的 `Planned` 承诺
    // =========================================================================

    /// **判定"该格式声明里有没有悬空的 `Planned` 承诺"**。
    ///
    /// `Support::Planned` 的 [`Support::is_usable_in_process`] 口径为**真**——
    /// 也就是说，一个写着 `Planned` 的格式，在上层看来"能用"，实际却还没交付。
    /// M4 一旦收工，任何一个残留在 [`FORMATS`] 里的 `Planned` 都是**悬空承诺**。
    ///
    /// 这条守卫与上一条是"同一把锁的两面"：
    /// - 上一条防 **`Native` 冒充已交付**（明明坏的却声称可用）；
    /// - 这一条防 **`Planned` 掩盖未交付**（明明没做却挂着"计划实现"）。
    ///
    /// 抽成函数是为了让它**可被攻击**——空断言等于没守卫。
    fn has_no_dangling_planned_promise(spec: &FormatSpec) -> bool {
        spec.import != Support::Planned && spec.export != Support::Planned
    }

    /// 当前格式矩阵里**不得**残留任何 `Planned`（M4 已收口）。
    #[test]
    fn no_format_is_left_with_a_dangling_planned_promise() {
        for spec in &FORMATS {
            assert!(
                has_no_dangling_planned_promise(spec),
                "{} 仍挂着 Planned（import={:?} / export={:?}）——M4 收工后这是悬空承诺",
                spec.format.label(),
                spec.import,
                spec.export
            );
        }
    }

    /// **守卫必须被攻击过（M2 教训）**：故意制造带 `Planned` 的声明，
    /// 断言守卫两侧（import / export）都能识破。
    #[test]
    fn planned_guard_detects_dangling_promise_attack() {
        let honest = *Format::Svg.spec();
        assert!(
            has_no_dangling_planned_promise(&honest),
            "基线：当前 SVG 声明不得带 Planned"
        );

        // 攻击 1：import 侧挂 Planned
        let mut attack_import = honest;
        attack_import.import = Support::Planned;
        assert!(
            !has_no_dangling_planned_promise(&attack_import),
            "守卫必须识破 import=Planned"
        );

        // 攻击 2：export 侧挂 Planned（这正是 SVG 收口前的状态）
        let mut attack_export = honest;
        attack_export.export = Support::Planned;
        assert!(
            !has_no_dangling_planned_promise(&attack_export),
            "守卫必须识破 export=Planned"
        );
    }

    /// **守卫必须被攻击过（M2 教训）**：故意把 STEP 声明改回不诚实的几种写法，
    /// 断言守卫函数**都能识破**——否则第 1 层就是"永远不会红"的空断言。
    #[test]
    fn capability_guard_detects_native_regression_attack() {
        let honest = *Format::Step.spec();
        assert!(
            step_declaration_is_honest(&honest),
            "基线：当前声明必须被判为诚实"
        );

        // 攻击 1：把 import 悄悄改回 `Native`（经典回归）
        let mut attack_import = honest;
        attack_import.import = Support::Native;
        assert!(
            !step_declaration_is_honest(&attack_import),
            "守卫必须识破 import=Native 的回归"
        );

        // 攻击 2：把 export 悄悄改回 `Native`
        let mut attack_export = honest;
        attack_export.export = Support::Native;
        assert!(
            !step_declaration_is_honest(&attack_export),
            "守卫必须识破 export=Native 的回归"
        );

        // 攻击 3：换一种"看起来有理"的变体写法——用 `Planned` / `ViaSeparateProcess`
        // 冒充可用（这些在 `is_usable_in_process()` 口径下也会被当成"可用"，故必须拦）
        for variant in [Support::Planned, Support::ViaSeparateProcess] {
            let mut attack_variant = honest;
            attack_variant.export = variant;
            assert!(
                !step_declaration_is_honest(&attack_variant),
                "守卫必须识破 export={variant:?} 的变体冒充"
            );
        }

        // 攻击 4：把 note 洗成"看起来没问题"（去掉'上游'/'trap'字样）
        let mut attack_note = honest;
        attack_note.note = "内核原生 STEP 读写，无需新库";
        assert!(
            !step_declaration_is_honest(&attack_note),
            "守卫必须识破 note 洗白（不再点名上游缺陷）"
        );
    }
}

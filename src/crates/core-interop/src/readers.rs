//! 具体导入器实现。
//!
//! ## 当前状态（诚实说明）
//!
//! | 实现 | 头部校验 | 几何导入 | 说明 |
//! |---|---|---|---|
//! | [`StepImporter`] | ✅ 真做 | ❌ **不交付** | 校验"是不是合法 STEP 头"后**显式报 `Unsupported`**（几何通道推迟至 OCCT sidecar 里程碑，见下文）；**绝不返回空成功** |
//! | [`GltfImporter`] | ✅ 真做 | ✅ 可用 | JSON 结构 + `gltf` crate 读网格；只作三维预览，网格不进 B-Rep |
//! | [`IfcImporter`] | ✅ 真做 | ✅ 可用 | `ifc-lite-core`（MPL-2.0）解析构件 / 属性集 / 空间层级 → `Element` |
//! | [`DxfImporter`] | ✅ 真做 | ✅ 可用 | 兼容子集（LINE / CIRCLE / ARC / LWPOLYLINE / TEXT） |
//! | [`DwgImporter`] | ❌ 拒绝 | ❌ 拒绝 | **合规红线**：必须独立进程，见下 |
//!
//! "几何待接入"这种状态**一律以显式错误（`Unsupported`）暴露，而不是假装成功**：
//! 上层必须能分辨"这个文件我认出来了但几何没进来"与"导入成功"。这正是 M4 §9
//! Top-1 那条"库半可用不得被整体标绿"的假绿防线——**返回空成功是最坏的一种骗法**。

use core_model::Category;

use crate::capability::Format;
use crate::error::{InteropError, InteropResult};
use crate::flow::{ImportContext, ImportReport, Importer};

// =============================================================================
// 共享小工具
// =============================================================================

/// 从 STEP 风格的 `KEYWORD(...)` 里抽出所有单引号字符串。
///
/// STEP / IFC 的头部就是这样一串 `FILE_NAME('a.ifc','2024-...',...)`。
/// 这里只做**括号配对 + 引号提取**，不做完整词法分析——头部解析不需要。
fn step_quoted_strings(input: &str, keyword: &str) -> Vec<String> {
    let Some(start) = input.find(keyword) else {
        return Vec::new();
    };
    let rest = &input[start..];
    let Some(open) = rest.find('(') else {
        return Vec::new();
    };

    // 往后扫，做括号配对，遇到顶层分号就停
    let bytes = rest.as_bytes();
    let mut depth = 0i32;
    let mut end = rest.len();
    for (i, b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            b';' if depth == 0 => {
                end = i;
                break;
            }
            _ => {}
        }
    }

    let body = &rest[open..end.min(rest.len())];
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    let mut chars = body.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\'' if !in_quote => {
                in_quote = true;
                cur.clear();
            }
            '\'' if in_quote => {
                // STEP 里 '' 表示一个转义的单引号
                if chars.peek() == Some(&'\'') {
                    cur.push('\'');
                    let _ = chars.next();
                } else {
                    in_quote = false;
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ if in_quote => cur.push(ch),
            _ => {}
        }
    }
    out
}

/// 从 STEP / IFC 头部抽出通用元信息。
fn parse_iso10303_header(source: &[u8], report: &mut ImportReport) {
    let text = String::from_utf8_lossy(source);

    let schema = step_quoted_strings(&text, "FILE_SCHEMA");
    if let Some(first) = schema.first() {
        report.note("file_schema", first.clone());
    }

    let name = step_quoted_strings(&text, "FILE_NAME");
    if let Some(v) = name.first() {
        report.note("file_name", v.clone());
    }
    if let Some(v) = name.get(1) {
        report.note("timestamp", v.clone());
    }
    // 第 3 个参数是作者列表，形如 ('张三','李四')
    if let Some(v) = name.get(2)
        && !v.is_empty()
    {
        report.note("author", v.clone());
    }
    // 第 4 个是组织
    if let Some(v) = name.get(3)
        && !v.is_empty()
    {
        report.note("organization", v.clone());
    }

    let desc = step_quoted_strings(&text, "FILE_DESCRIPTION");
    if let Some(v) = desc.first() {
        report.note("file_description", v.clone());
    }
}

// =============================================================================
// STEP
// =============================================================================

/// STEP（AP203 / AP214 / AP242）导入器。
///
/// **几何导入已推迟至「OCCT sidecar 里程碑」**（不是本项目未实现，是上游缺陷）：
/// 原计划把 `DATA` 段的实体解析交给 `occt-wasm` 的 `xcaf_*` 接口（XCAF 保留颜色 /
/// 层 / 名称等装配语义，正是 STEP 需要的）。但**实测证伪**——本 fork 的
/// `xcaf_import_step` / `import_step` / `xcaf_export_step` / `export_step` 全部
/// 触发上游 wasm trap（`uninitialized element` 等），根因是 **4.x 构建形态缺陷：
/// 文件系统未编入 WASM 产物，host 侧不可修**（STEP 读写必须走临时文件）。
/// 故本实现现在能做的是：**认出格式、抽出头部元信息、明确告知几何已推迟**。
///
/// 详尽的证据链与"为什么不引入第二套几何内核"的裁决见
/// `core-kernel/src/known_upstream_defects.rs` 的文件头。
///
/// ## 为什么 `import` 返回 `Err` 而不是"只带头部元信息的成功"
///
/// 本实现**保留**是为了：(a) 让 `Format::Step` 仍出现在路由器"认识的格式"里，
/// 从而给出**明确的推迟说明**而不是笼统的"没有导入器"；(b) 区分**垃圾输入**
/// （非 STEP → `Err(Parse)`）与**合法 STEP 但通道不可用**（→ `Err(Unsupported)`）。
///
/// **绝不返回空成功**（M4 §9 Top-1）：一份"`Ok` + 只有头部元信息、零几何"的报告，
/// 会让上层与用户误以为 STEP 导入成功——那正是"库半可用被整体标绿"的翻版。
pub struct StepImporter;

impl Importer for StepImporter {
    fn format(&self) -> Format {
        Format::Step
    }

    fn can_import(&self, head: &[u8]) -> bool {
        crate::detect::detect("x.step", head) == Some(Format::Step)
    }

    fn import(&self, source: &[u8], ctx: &mut ImportContext) -> InteropResult<ImportReport> {
        let mut header = ImportReport::new(Format::Step);
        parse_iso10303_header(source, &mut header);

        // 先区分两类失败（§5.2：垃圾输入测的是"垃圾"，与"通道缺陷"无关）：
        // ① 连 FILE_SCHEMA 都没有 → 这压根不是 STEP → 解析错；
        // ② 是合法 STEP，但几何通道不可用 → 显式 Unsupported（下方）。
        if !header.metadata.contains_key("file_schema") {
            return Err(InteropError::Parse(format!(
                "{}：未找到 FILE_SCHEMA，可能不是合法 STEP 文件",
                ctx.source_name
            )));
        }

        // ② 合法 STEP：M4 不交付读写 → **显式报错**（R3 定稿，§2.5/:413）。
        Err(InteropError::Unsupported {
            format: Format::Step,
            direction: "导入（几何通道推迟至 OCCT sidecar 里程碑：\
                        上游 occt-wasm 4.x 构建形态缺陷，文件系统未编入 WASM 产物，host 侧不可修）",
        })
    }
}

// =============================================================================
// glTF
// =============================================================================

/// glTF 2.0 导入器（`.gltf` JSON 与 `.glb` 二进制容器）。
///
/// glTF 在本项目里的定位是**三维预览**，不是工程图依据——它只有三角网格，
/// 没有精确 B-Rep。**绝不可拿 glTF 当出图来源**（这正是设计文档 6.2 反复强调的
/// "预览级消隐 vs 工程图级消隐"那条分界线）。
pub struct GltfImporter;

impl Importer for GltfImporter {
    fn format(&self) -> Format {
        Format::Gltf
    }

    fn can_import(&self, head: &[u8]) -> bool {
        crate::detect::detect("x.gltf", head) == Some(Format::Gltf)
    }

    fn import(&self, source: &[u8], ctx: &mut ImportContext) -> InteropResult<ImportReport> {
        let mut report = ImportReport::new(Format::Gltf);

        // .glb：跳过 12 字节头 + 8 字节块头，拿到 JSON 块
        let json_slice: &[u8] = if source.starts_with(b"glTF") {
            if source.len() < 20 {
                return Err(InteropError::Parse(format!(
                    "{}：glb 文件过短",
                    ctx.source_name
                )));
            }
            let len = u32::from_le_bytes([source[12], source[13], source[14], source[15]]) as usize;
            let start = 20;
            let end = (start + len).min(source.len());
            &source[start..end]
        } else {
            source
        };

        let value: serde_json::Value = serde_json::from_slice(json_slice).map_err(|e| {
            InteropError::Parse(format!("{}：glTF JSON 解析失败：{e}", ctx.source_name))
        })?;

        if let Some(v) = value
            .get("asset")
            .and_then(|a| a.get("version"))
            .and_then(serde_json::Value::as_str)
        {
            report.note("gltf_version", v);
        }
        if let Some(g) = value
            .get("asset")
            .and_then(|a| a.get("generator"))
            .and_then(serde_json::Value::as_str)
        {
            report.note("generator", g);
        }

        let mesh_count = value
            .get("meshes")
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len);
        let node_count = value
            .get("nodes")
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len);
        report.note("mesh_count", mesh_count.to_string());
        report.note("node_count", node_count.to_string());

        // 用 gltf crate 真实解析——仅用于统计顶点数，不进入 B-Rep
        if mesh_count > 0 {
            let gltf_result = if source.starts_with(b"glTF") {
                // .glb：直接用 import_slice（它内部处理容器头）
                gltf::import_slice(source)
            } else {
                gltf::import_slice(source)
            };
            if let Ok(gltf) = gltf_result {
                let vertex_count: usize = gltf
                    .0
                    .meshes()
                    .flat_map(|m| m.primitives())
                    .filter_map(|p| {
                        let reader = p.reader(|_| None::<&[u8]>);
                        reader.read_positions().map(|iter| iter.count())
                    })
                    .sum();
                report.note("vertex_count", vertex_count.to_string());
            }
            // 解析失败不影响已有 metadata——仍可展示
        }

        // 一次导入建一个占位元素，让上层能看到"导进来了一坨东西"
        if mesh_count > 0 {
            let mut element = core_model::Element::new(Category::new("GLTF_IMPORT"), "glTF 几何");
            element.properties = core_model::PropertyMap::new("GLTF_IMPORT")
                .with("mesh_count", core_model::Value::Int(mesh_count as i64));
            report.elements.push(element);
        }

        report.warn(format!(
            "已读出 {mesh_count} 个网格 / {node_count} 个节点；网格→B-Rep 的转换不在计划内\
             （glTF 只作三维预览，不作工程图依据）"
        ));
        Ok(report)
    }
}

// =============================================================================
// IFC
// =============================================================================

/// IFC4X3 导入器（ifc-lite-core 后端）。
///
/// ## 库已选定：ifc-lite-core（MPL-2.0）
///
/// 用 `ifc_lite_core::EntityScanner` **两遍**扫描：
/// 1. 分类实体——构件（墙/梁/柱/板…）、空间层级（Project/Site/Building/Storey/Space）
///    落成 `Element`；同时收集 `IfcRelDefinesByProperties` / `IfcRelAggregates` /
///    `IfcRelContainedInSpatialStructure` 三条关系；
/// 2. 用 `ifc_lite_core::EntityDecoder::decode_by_id` 解出**属性集**
///    （`IfcPropertySet` → `IfcPropertySingleValue`）与**空间层级**，写进构件的
///    `PropertyMap`。
///
/// ## 达标线（设计文档 §7.5 / plan §4.4）
///
/// 必须能读**构件 + 属性集 + 空间层级**三者，缺一不算"IFC 可解析"。
/// 空间层级**先用属性表达**（`contained_in` / `contained_in_name`），
/// **不新造 `core-model` 结构**（一阶段约定）。
pub struct IfcImporter;

/// 把 IFC 实体类型映射到本项目品类名；非"构件/空间"类返回 `None`。
fn ifc_element_category(ifc_type: &ifc_lite_core::IfcType) -> Option<&'static str> {
    use ifc_lite_core::IfcType as T;
    Some(match ifc_type {
        T::IfcSpace => "IFC_SPACE",
        T::IfcWall | T::IfcWallStandardCase => "IFC_WALL",
        T::IfcDoor => "IFC_DOOR",
        T::IfcWindow => "IFC_WINDOW",
        T::IfcBeam => "IFC_BEAM",
        T::IfcColumn => "IFC_COLUMN",
        T::IfcSlab => "IFC_SLAB",
        T::IfcRoof => "IFC_ROOF",
        T::IfcStair => "IFC_STAIR",
        T::IfcRamp => "IFC_RAMP",
        T::IfcPipeSegment | T::IfcPipeFitting => "IFC_PIPE",
        T::IfcDuctSegment | T::IfcDuctFitting => "IFC_DUCT",
        T::IfcCableCarrierSegment | T::IfcCableCarrierFitting => "IFC_CABLE_CARRIER",
        T::IfcBuilding => "IFC_BUILDING",
        T::IfcBuildingStorey => "IFC_STOREY",
        T::IfcProject => "IFC_PROJECT",
        T::IfcSite => "IFC_SITE",
        _ => return None,
    })
}

/// 把一个 IFC 属性值转成本项目的 [`core_model::Value`]。
///
/// 返回 `None` 表示"本轮不落库"（空值 / 派生值 / 实体引用——引用未建映射，
/// 不硬塞成文本以免误导）。
fn ifc_attr_to_value(attr: &ifc_lite_core::AttributeValue) -> Option<core_model::Value> {
    use ifc_lite_core::AttributeValue as A;
    match attr {
        A::String(s) => Some(core_model::Value::Text(s.clone())),
        A::Integer(i) => Some(core_model::Value::Int(*i)),
        A::Float(f) => Some(core_model::Value::Float(*f)),
        A::Enum(e) => Some(core_model::Value::Text(e.clone())),
        // 类型化值（如 `IFCLABEL('2HR')` / `IFCLENGTHMEASURE(1.5)`）在库里存成
        // `List([String("TYPENAME"), 值])` → 取末位真实值。
        A::List(items) => items.last().and_then(ifc_attr_to_value),
        A::EntityRef(_) | A::Null | A::Derived => None,
    }
}

impl Importer for IfcImporter {
    fn format(&self) -> Format {
        Format::Ifc4x3
    }

    fn can_import(&self, head: &[u8]) -> bool {
        crate::detect::detect("x.ifc", head) == Some(Format::Ifc4x3)
    }

    fn import(&self, source: &[u8], ctx: &mut ImportContext) -> InteropResult<ImportReport> {
        use ifc_lite_core::IfcType;

        let mut report = ImportReport::new(Format::Ifc4x3);
        parse_iso10303_header(source, &mut report);

        if !report.metadata.contains_key("file_schema") {
            return Err(InteropError::Parse(format!(
                "{}：未找到 FILE_SCHEMA（IFC 文件必须以 ISO-10303-21 头开始）",
                ctx.source_name
            )));
        }

        let total_count = ifc_lite_core::entity_count(source);
        report.note("entity_total", total_count.to_string());

        let mut decoder = ifc_lite_core::EntityDecoder::new(source);

        // 实体 id → report.elements 下标（把属性/层级挂到对应构件上）
        let mut element_index: std::collections::BTreeMap<u32, usize> =
            std::collections::BTreeMap::new();
        // 待处理属性关系：(被描述对象 ids, 属性集 id)
        let mut prop_relations: Vec<(Vec<u32>, u32)> = Vec::new();
        // 空间层级：子 id → 父 id
        let mut parent_of: std::collections::BTreeMap<u32, u32> = std::collections::BTreeMap::new();
        // 实体 id → 原始类型名（把父 id 翻译成可读名）
        let mut type_name_of: std::collections::BTreeMap<u32, String> =
            std::collections::BTreeMap::new();

        let mut supported = 0usize;
        let mut unsupported = 0usize;

        // ---- 第一遍：分类构件 + 收集三条关系 ----
        let mut scanner = ifc_lite_core::EntityScanner::new(source);
        while let Some((id, type_name, _start, _end)) = scanner.next_entity() {
            let ifc_type = ifc_lite_core::ifc_type_from_keyword(type_name);
            type_name_of.insert(id, type_name.to_string());

            if let Some(category) = ifc_element_category(&ifc_type) {
                supported += 1;
                let mut element = core_model::Element::new(
                    core_model::Category::new(category),
                    category.to_string(),
                );
                element.properties = core_model::PropertyMap::new(category)
                    .with("ifc_type", core_model::Value::Text(type_name.to_string()))
                    .with("ifc_id", core_model::Value::Text(format!("#{id}")));
                element_index.insert(id, report.elements.len());
                report.elements.push(element);
                continue;
            }

            // 关系实体：不产出构件，但承载属性集 / 空间层级
            match ifc_type {
                IfcType::IfcRelDefinesByProperties => {
                    // [4]=RelatedObjects [5]=RelatingPropertyDefinition
                    if let Ok(rel) = decoder.decode_by_id(id)
                        && let (Some(objects), Some(pset)) = (rel.get_refs(4), rel.get_ref(5))
                    {
                        prop_relations.push((objects, pset));
                    }
                }
                IfcType::IfcRelAggregates => {
                    // [4]=RelatingObject(父) [5]=RelatedObjects(子)
                    if let Ok(rel) = decoder.decode_by_id(id)
                        && let (Some(parent), Some(children)) = (rel.get_ref(4), rel.get_refs(5))
                    {
                        for child in children {
                            parent_of.insert(child, parent);
                        }
                    }
                }
                IfcType::IfcRelContainedInSpatialStructure => {
                    // [4]=RelatedElements [5]=RelatingStructure(空间容器)
                    if let Ok(rel) = decoder.decode_by_id(id)
                        && let (Some(elements), Some(container)) = (rel.get_refs(4), rel.get_ref(5))
                    {
                        for e in elements {
                            parent_of.insert(e, container);
                        }
                    }
                }
                _ => unsupported += 1,
            }
        }

        // ---- 第二遍：属性集（IfcRelDefinesByProperties → IfcPropertySet → IfcPropertySingleValue）----
        let mut property_count = 0usize;
        for (objects, pset_id) in &prop_relations {
            let Ok(pset) = decoder.decode_by_id(*pset_id) else {
                continue;
            };
            let pset_name = pset.get_string(2).unwrap_or("PropertySet").to_string();
            // [4]=HasProperties（引用列表）
            let Some(prop_ids) = pset.get_refs(4) else {
                continue;
            };
            for prop_id in prop_ids {
                let Ok(prop) = decoder.decode_by_id(prop_id) else {
                    continue;
                };
                // IfcPropertySingleValue：[0]=Name [2]=NominalValue
                let Some(name) = prop.get_string(0) else {
                    continue;
                };
                let Some(value) = prop.get(2).and_then(ifc_attr_to_value) else {
                    continue;
                };
                let key = format!("{pset_name}.{name}");
                for obj in objects {
                    if let Some(&idx) = element_index.get(obj) {
                        report.elements[idx]
                            .properties
                            .set(key.clone(), value.clone());
                        property_count += 1;
                    }
                }
            }
        }

        // ---- 第三遍：空间层级（IfcRelAggregates / IfcRelContainedInSpatialStructure）----
        let mut hierarchy_count = 0usize;
        for (child, parent) in &parent_of {
            let Some(&idx) = element_index.get(child) else {
                continue;
            };
            let parent_desc = match type_name_of.get(parent) {
                Some(n) => format!("#{parent} ({n})"),
                None => format!("#{parent}"),
            };
            report.elements[idx]
                .properties
                .set("contained_in", core_model::Value::Text(parent_desc));
            if let Some(&pidx) = element_index.get(parent) {
                let parent_name = report.elements[pidx].name.clone();
                report.elements[idx]
                    .properties
                    .set("contained_in_name", core_model::Value::Text(parent_name));
            }
            hierarchy_count += 1;
        }

        report.note("entity_supported", supported.to_string());
        report.note("entity_unsupported", unsupported.to_string());
        report.note("property_count", property_count.to_string());
        report.note("hierarchy_count", hierarchy_count.to_string());

        if unsupported > 0 {
            report.warn(format!(
                "IFC 实体解析：{supported} 类已识别，{unsupported} 类未支持跳过"
            ));
        } else {
            report.warn(format!("IFC 实体解析：{supported} 类已识别，0 跳过"));
        }

        Ok(report)
    }
}

// =============================================================================
// DXF
// =============================================================================

/// DXF 导入器：用 `dxf` crate 读入，产出中性 Drawing2D。
///
/// 兼容子集：LINE / CIRCLE / ARC / LWPOLYLINE / TEXT。
/// 不做全量 AutoCAD 语义（无动态块、无 XREF、无 annotative scaling）。
pub struct DxfImporter;

impl Importer for DxfImporter {
    fn format(&self) -> Format {
        Format::Dxf
    }

    fn can_import(&self, head: &[u8]) -> bool {
        crate::detect::detect("x.dxf", head) == Some(Format::Dxf)
    }

    fn import(&self, source: &[u8], _ctx: &mut ImportContext) -> InteropResult<ImportReport> {
        let mut report = ImportReport::new(Format::Dxf);
        let (drawing, warnings) = crate::dxf_reader::read_dxf(source)?;

        // 统计：解析实体（圆/弧）与折线分开计数
        let mut line_count = 0usize;
        let mut circle_count = 0usize;
        let mut arc_count = 0usize;
        for curve in &drawing.curves {
            if curve.as_circle().is_some() {
                circle_count += 1;
            } else if curve.as_arc().is_some() {
                arc_count += 1;
            } else {
                line_count += 1;
            }
        }
        report.note("line_count", line_count.to_string());
        report.note("circle_count", circle_count.to_string());
        report.note("arc_count", arc_count.to_string());
        report.note("text_count", drawing.texts.len().to_string());

        // 把 Drawing2D 的 curves/texts 转成 Element；解析信息写进 properties
        for curve in &drawing.curves {
            let mut element =
                core_model::Element::new(core_model::Category::new("DXF_IMPORT"), "DXF 曲线");
            let mut props = core_model::PropertyMap::new("DXF_IMPORT")
                .with("layer", core_model::Value::Text(curve.layer.clone()));
            if let Some((center, radius)) = curve.as_circle() {
                props = props
                    .with("kind", core_model::Value::Text("circle".to_string()))
                    .with("center_x", core_model::Value::Float(center[0]))
                    .with("center_y", core_model::Value::Float(center[1]))
                    .with("radius", core_model::Value::Float(radius));
            } else if let Some((center, radius, start, end)) = curve.as_arc() {
                props = props
                    .with("kind", core_model::Value::Text("arc".to_string()))
                    .with("center_x", core_model::Value::Float(center[0]))
                    .with("center_y", core_model::Value::Float(center[1]))
                    .with("radius", core_model::Value::Float(radius))
                    .with("start_angle_rad", core_model::Value::Float(start))
                    .with("end_angle_rad", core_model::Value::Float(end));
            } else {
                props = props
                    .with("kind", core_model::Value::Text("polyline".to_string()))
                    .with(
                        "point_count",
                        core_model::Value::Int(curve.points.len() as i64),
                    );
            }
            element.properties = props;
            report.elements.push(element);
        }
        for text in &drawing.texts {
            let mut element =
                core_model::Element::new(core_model::Category::new("DXF_IMPORT"), "DXF 文字");
            element.properties = core_model::PropertyMap::new("DXF_IMPORT")
                .with("kind", core_model::Value::Text("text".to_string()))
                .with("content", core_model::Value::Text(text.content.clone()))
                .with("layer", core_model::Value::Text(text.layer.clone()));
            report.elements.push(element);
        }

        for w in warnings {
            report.warn(w);
        }
        report.warn("DXF 兼容子集，不做全量 AutoCAD 语义".to_string());
        Ok(report)
    }
}

// =============================================================================
// DWG —— 合规红线
// =============================================================================

/// DWG 导入器：**永远返回 [`InteropError::RequiresIsolation`]**。
///
/// ## 为什么这个实现"什么都不做"反而是对的
///
/// LibreDWG 是 **GPLv3+**。把它链接进主程序会传染整个平台层的许可证
/// （设计文档 7.8）。所以：
///
/// - 主程序里**不存在**能真正读 DWG 的代码路径；
/// - 想读 DWG 只能起一个**独立转换服务**（自带 LibreDWG），主程序通过
///   IPC / HTTP 把文件丢过去，拿回来的是 STEP 或 BREP 文本；
/// - 这个实现的存在意义，是让"误用"**在编译期就无法发生**——
///   只要有人写了 `DwgImporter.import(...)`，就会拿到一个明确的错误，
///   而不是悄悄链进 GPL 代码。
///
/// 测试 [`tests::dwg_import_always_refuses`] 把这条红线钉死。
pub struct DwgImporter;

impl Importer for DwgImporter {
    fn format(&self) -> Format {
        Format::Dwg
    }

    fn import(&self, _source: &[u8], _ctx: &mut ImportContext) -> InteropResult<ImportReport> {
        Err(InteropError::RequiresIsolation {
            format: Format::Dwg,
            library: "LibreDWG",
            license: "GPLv3+",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEP: &[u8] = b"ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('CAD output'),'2;1');\nFILE_NAME('shaft.step','2024-05-06T08:00:00',('Zhang San'),('CEEC'),'','','');\nFILE_SCHEMA(('AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }'));\nENDSEC;\nDATA;\n#1=SHAPE_REPRESENTATION();\nENDSEC;\nEND-ISO-10303-21;\n";
    const IFC: &[u8] = b"ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('ViewDefinition [CoordinationView]'),'2;1');\nFILE_NAME('tower.ifc','2024-01-01T00:00:00',(''),(''),'','','');\nFILE_SCHEMA(('IFC4X3'));\nENDSEC;\nDATA;\n#100=IFCSPACE('g',$,'ROOM',$,$,$,$,$,$);\n#200=IFCWALL('w',$,$,$,$,$,$,$);\n#201=IFCWALL('w2',$,$,$,$,$,$,$);\nENDSEC;\n";

    #[test]
    fn step_import_is_explicitly_unsupported_never_a_silent_success() {
        // R3 定稿：M4 不交付 STEP 读写。合法 STEP 必须返回**显式 Unsupported**，
        // 绝不能是"Ok + 只带头部元信息"的空成功（M4 §9 Top-1 假绿防线）。
        let mut ctx = ImportContext::new("shaft.step");
        let err = StepImporter.import(STEP, &mut ctx).unwrap_err();
        match err {
            InteropError::Unsupported { format, direction } => {
                assert_eq!(format, Format::Step);
                assert!(
                    direction.contains("OCCT sidecar"),
                    "错误信息必须指向 OCCT sidecar 里程碑，实得：{direction}"
                );
            }
            other => panic!("合法 STEP 必须显式 Unsupported，实得：{other:?}"),
        }
    }

    #[test]
    fn step_without_schema_is_a_parse_error() {
        let mut ctx = ImportContext::new("bad.step");
        let err = StepImporter
            .import(b"ISO-10303-21;\nHEADER;\n", &mut ctx)
            .unwrap_err();
        assert!(matches!(err, InteropError::Parse(_)));
    }

    /// §5.2 明文钉：**垃圾输入**测的是"垃圾"，与"通道缺陷"无关——
    /// **不得**因为通道坏掉就把它改成"永远 Err(Unsupported)"。
    #[test]
    fn step_garbage_is_parse_error() {
        let mut ctx = ImportContext::new("garbage.step");
        let err = StepImporter
            .import(b"not a step at all", &mut ctx)
            .unwrap_err();
        assert!(
            matches!(err, InteropError::Parse(_)),
            "垃圾输入必须是解析错（区分于合法 STEP 的 Unsupported），实得：{err:?}"
        );
    }

    #[test]
    fn ifc_header_is_extracted_and_entities_are_counted() {
        let mut ctx = ImportContext::new("tower.ifc");
        let r = IfcImporter.import(IFC, &mut ctx).unwrap();
        assert_eq!(
            r.metadata.get("file_schema").map(String::as_str),
            Some("IFC4X3")
        );
        // 不再检查 entity_ifcspace/entity_ifcwall 元信息——改由 ifc-lite-core
        // 实体级分类，对应元素现直接产出为 Element
        assert!(
            r.element_count() >= 3,
            "应解析出 IFCSPACE(1) + IFCWALL(2) 共 3 个元素"
        );
        assert!(!r.is_empty(), "库接入后应产出几何元素");
    }

    #[test]
    fn ifc_importer_honestly_warns_parsing_result() {
        let mut ctx = ImportContext::new("tower.ifc");
        let r = IfcImporter.import(IFC, &mut ctx).unwrap();
        assert!(
            r.warnings.iter().any(|w| w.contains("已识别")),
            "warn 应包含解析结果说明"
        );
    }

    /// 一个**含空间层级 + 属性集**的最小 IFC4 样本（内联，不落盘）。
    ///
    /// 结构：Project →(聚合)→ Site →(聚合)→ Building →(聚合)→ Storey；
    /// Wall 被 Storey **包含**；Wall 挂一个 `Pset_WallCommon` 属性集。
    const IFC_RICH: &[u8] = b"ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('ViewDefinition [CoordinationView]'),'2;1');\nFILE_NAME('tower.ifc','2024-01-01T00:00:00',(''),(''),'','','');\nFILE_SCHEMA(('IFC4X3'));\nENDSEC;\nDATA;\n#1=IFCPROJECT('prj',$,'Tower',$,$,$,$,$,$);\n#2=IFCSITE('site',$,'Site',$,$,$,$,$,$);\n#3=IFCBUILDING('bld',$,'Building',$,$,$,$,$,$);\n#4=IFCBUILDINGSTOREY('sty',$,'Storey 1',$,$,$,$,$,$);\n#10=IFCWALL('wall',$,'Wall A',$,$,$,$,$);\n#20=IFCRELAGGREGATES('ra1',$,$,$,#1,(#2));\n#21=IFCRELAGGREGATES('ra2',$,$,$,#2,(#3));\n#22=IFCRELAGGREGATES('ra3',$,$,$,#3,(#4));\n#23=IFCRELCONTAINEDINSPATIALSTRUCTURE('rc1',$,$,$,(#10),#4);\n#30=IFCPROPERTYSET('ps1',$,'Pset_WallCommon',$,(#31));\n#31=IFCPROPERTYSINGLEVALUE('FireRating',$,IFCLABEL('2HR'),$);\n#32=IFCRELDEFINESBYPROPERTIES('rd1',$,$,$,(#10),#30);\nENDSEC;\nEND-ISO-10303-21;\n";

    /// §5.2 明文钉 + §9 Top-3 反例：**只数实体名不算达标**——
    /// 构件必须落成 `Element` 且 `PropertyMap` 非空。
    #[test]
    fn ifc_sample_reads_walls_with_non_empty_properties() {
        let mut ctx = ImportContext::new("tower.ifc");
        let r = IfcImporter.import(IFC_RICH, &mut ctx).unwrap();

        assert!(r.element_count() >= 1, "至少要读出一个构件元素");
        let wall = r
            .elements
            .iter()
            .find(|e| e.category.as_str() == "IFC_WALL")
            .expect("应读出 IFC_WALL 构件");
        // 只记类型名不算数：属性集里必须有真读出来的键
        assert!(
            !wall.properties.values.is_empty(),
            "构件 PropertyMap 必须非空（只数实体名不算达标）"
        );
        assert_eq!(
            wall.properties.get_text("ifc_type"),
            Some("IFCWALL"),
            "应记录 IFC 类型"
        );
    }

    /// 达标线三要素之"属性集"：`IfcRelDefinesByProperties → IfcPropertySet →
    /// IfcPropertySingleValue` 必须真读进构件的 `PropertyMap`。
    #[test]
    fn ifc_reads_property_sets_into_properties() {
        let mut ctx = ImportContext::new("tower.ifc");
        let r = IfcImporter.import(IFC_RICH, &mut ctx).unwrap();

        let wall = r
            .elements
            .iter()
            .find(|e| e.category.as_str() == "IFC_WALL")
            .expect("应读出 IFC_WALL 构件");
        assert_eq!(
            wall.properties.get_text("Pset_WallCommon.FireRating"),
            Some("2HR"),
            "属性集必须读进构件属性；实得键：{:?}",
            wall.properties.values.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            r.metadata.get("property_count").map(String::as_str),
            Some("1"),
            "应统计到 1 条属性"
        );
    }

    /// 达标线三要素之"空间层级"：Project→Site→Building→Storey 的聚合链
    /// 与"构件被楼层包含"必须表达出来（一阶段用属性，不新造 core-model 结构）。
    #[test]
    fn ifc_reads_spatial_hierarchy_into_properties() {
        let mut ctx = ImportContext::new("tower.ifc");
        let r = IfcImporter.import(IFC_RICH, &mut ctx).unwrap();

        // 四个空间层级元素都应在
        for cat in ["IFC_PROJECT", "IFC_SITE", "IFC_BUILDING", "IFC_STOREY"] {
            assert!(
                r.elements.iter().any(|e| e.category.as_str() == cat),
                "应读出空间层级元素 {cat}"
            );
        }
        // storey 的父应是 building（聚合链）
        let storey = r
            .elements
            .iter()
            .find(|e| e.category.as_str() == "IFC_STOREY")
            .expect("应读出 IFC_STOREY");
        let contained = storey
            .properties
            .get_text("contained_in")
            .expect("storey 应记录其父（building）");
        assert!(
            contained.contains("IFCBUILDING"),
            "storey 的父应是 building，实得：{contained}"
        );

        // wall 被 storey「包含」，且能给出可读父名
        let wall = r
            .elements
            .iter()
            .find(|e| e.category.as_str() == "IFC_WALL")
            .expect("应读出 IFC_WALL");
        assert!(
            wall.properties.get_text("contained_in").is_some(),
            "wall 应记录所属空间结构"
        );
        assert_eq!(
            wall.properties.get_text("contained_in_name"),
            Some("IFC_STOREY"),
            "wall 的容器应是 IFC_STOREY"
        );
    }

    /// §5.2：未支持实体 warn **但不整包失败**。
    #[test]
    fn ifc_unknown_entity_warns_not_fails() {
        let src = b"ISO-10303-21;\nHEADER;\nFILE_NAME('u.ifc','',(''),(''),'','','');\nFILE_SCHEMA(('IFC4X3'));\nENDSEC;\nDATA;\n#1=IFCWALL('w',$,$,$,$,$,$,$);\n#2=IFCTOTALLYMADEUPENTITY('x',$);\nENDSEC;\n";
        let mut ctx = ImportContext::new("unknown.ifc");
        let r = IfcImporter.import(src, &mut ctx).unwrap();
        assert!(r.element_count() >= 1, "已知构件照常导入");
        assert!(
            r.metadata
                .get("entity_unsupported")
                .is_some_and(|v| v != "0"),
            "未知实体应计入 unsupported"
        );
    }

    #[test]
    fn gltf_json_metadata_is_extracted() {
        let src = br#"{"asset":{"version":"2.0","generator":"Blender"},"meshes":[{"name":"m1"},{"name":"m2"}],"nodes":[{},{}]}"#;
        let mut ctx = ImportContext::new("m.gltf");
        let r = GltfImporter.import(src, &mut ctx).unwrap();
        assert_eq!(
            r.metadata.get("gltf_version").map(String::as_str),
            Some("2.0")
        );
        assert_eq!(
            r.metadata.get("generator").map(String::as_str),
            Some("Blender")
        );
        assert_eq!(r.metadata.get("mesh_count").map(String::as_str), Some("2"));
        assert_eq!(r.element_count(), 1);
    }

    #[test]
    fn glb_container_json_chunk_is_read() {
        let json = br#"{"asset":{"version":"2.0"},"meshes":[{"name":"m"}]}"#;
        let mut glb = b"glTF".to_vec();
        glb.extend_from_slice(&2u32.to_le_bytes()); // version
        glb.extend_from_slice(&((20 + json.len()) as u32).to_le_bytes()); // total length
        glb.extend_from_slice(&(json.len() as u32).to_le_bytes()); // chunk length
        glb.extend_from_slice(b"JSON");
        glb.extend_from_slice(json);

        let mut ctx = ImportContext::new("m.glb");
        let r = GltfImporter.import(&glb, &mut ctx).unwrap();
        assert_eq!(r.metadata.get("mesh_count").map(String::as_str), Some("1"));
    }

    #[test]
    fn gltf_without_meshes_produces_no_elements() {
        let src = br#"{"asset":{"version":"2.0"},"meshes":[]}"#;
        let mut ctx = ImportContext::new("empty.gltf");
        let r = GltfImporter.import(src, &mut ctx).unwrap();
        assert_eq!(r.element_count(), 0);
    }

    #[test]
    fn gltf_garbage_is_a_parse_error() {
        let mut ctx = ImportContext::new("bad.gltf");
        assert!(GltfImporter.import(b"not json at all", &mut ctx).is_err());
    }

    #[test]
    fn dwg_import_always_refuses() {
        let mut ctx = ImportContext::new("legacy.dwg");
        let err = DwgImporter.import(b"AC1015\x00\x00", &mut ctx).unwrap_err();
        match err {
            InteropError::RequiresIsolation {
                format,
                library,
                license,
            } => {
                assert_eq!(format, Format::Dwg);
                assert_eq!(library, "LibreDWG");
                assert_eq!(license, "GPLv3+");
            }
            other => panic!("DWG 必须走隔离错误，实得：{other:?}"),
        }
    }

    #[test]
    fn can_import_dispatches_on_real_headers() {
        assert!(StepImporter.can_import(STEP));
        assert!(!StepImporter.can_import(IFC));
        assert!(IfcImporter.can_import(IFC));
        assert!(!IfcImporter.can_import(STEP));
    }

    #[test]
    fn step_quoted_string_parser_handles_brackets_inside_quotes() {
        let s = "FILE_NAME('weird(name).step','',(''),(''),'','','');";
        let out = step_quoted_strings(s, "FILE_NAME");
        assert_eq!(out[0], "weird(name).step", "引号内的括号不应影响配对");
    }

    #[test]
    fn step_quoted_string_parser_handles_escaped_quotes() {
        let s = "FILE_NAME('it''s.step','');";
        let out = step_quoted_strings(s, "FILE_NAME");
        assert_eq!(out[0], "it's.step");
    }
}

//! 关联更新编排（M5 B3-f / §2.3(g) / §4）。
//!
//! ## 一次改动之后，图纸要跟着动哪些东西
//!
//! 模型改了（构件尺寸变了 / 布尔重算），图纸不会自己变。本模块把"改了什么"
//! 编排成一条**可观测**的更新流水线：
//!
//! ```text
//! changed_shapes + histories
//!         │
//!   ① resolve_delta  ── 旧键 → 新键（演化优先、指纹退避）
//!         │
//!   ② affected_after_resolve ── 受影响图元 / 标注（旧键与新键的绑定都算）
//!         │
//!   ③ views_bound_to_shapes ── 只有"绑定了被改形状"的视图才动
//!         │
//!   ④ 定向重投 ── 只重投受影响的形状；**无关图元逐字节不变**
//!         │
//!   ⑤ remeasure_from_entities ── 相关线性标注自动重算（不手工改图）
//!         │
//!   ⑥ dependencies.remap ── 把索引里的旧键改绑到新键
//!         │
//!   ⑦ mark_views_stale ── 标脏需要重新生成的视图
//! ```
//!
//! ## 为什么"定向重投"要按形状、且**原地保留 `EntityId`**
//!
//! `project_shape` 每次都会造出全新的 [`core_doc::Entity2D`]（新 `EntityId`）。
//! 若整视图清空重投，会有两个后果：
//!
//! 1. **无关图元也变了**——违反"只重投受影响的形状"（§4 场景 B：改 A 时
//!    构件 B 的图元必须逐字节不变）；
//! 2. **标注脱钩**——[`core_doc::Annotation::associates`] 存的是图元的 `EntityId`，
//!    一旦图元换了 id，[`core_doc::Annotation::remeasure_from_entities`] 就再也
//!    找不到它该量的几何，尺寸标注会静默失效。
//!
//! 因此本模块**逐形状原地替换**：只摘掉属于该形状的旧图元，再把新投影按**位置
//! 复用**旧 `EntityId` 放回原位。结果：其它形状的图元一字不动，标注关联继续有效。
//!
//! ## 红线
//!
//! `changed_shapes` 为空时**报告全 0、图纸逐个字节不变**（可观测的"无事发生"）。

use std::collections::BTreeSet;

use core_doc::{DraftingStandard, Drawing, Entity2D, View};
use core_kernel::Kernel;
use core_model::{Document, ShapeId, ViewId};

use crate::error::AppResult;
use crate::evolution_align::{HistoryRecord, resolve_delta};
use crate::projection_service::project_shape;

/// 一次关联更新的可观测结果。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UpdateReport {
    /// 本次被改的形状（回显入参，便于调用方记账）。
    pub changed_shapes: Vec<ShapeId>,
    /// 经**演化**（精确直连）解析出的键数。
    pub evolution_resolved: usize,
    /// 经**几何指纹**（最近匹配）解析出的键数。
    pub fingerprint_resolved: usize,
    /// 定向重投写回的新图元数。
    pub reprojected_entities: usize,
    /// 自动重算且**量测值确实变化**的标注数。
    pub remeasured_annotations: usize,
    /// 被标脏（需重新生成）的视图数。
    pub stale_views: usize,
    /// 定向重投过程中收集到的警告（曲线降级 / 丢弃 / 投影为空等），**不再被吞**（P1-2）。
    pub warnings: Vec<String>,
}

/// **关联更新编排**：模型提交之后，让图纸跟着动。
///
/// # 参数
///
/// - `changed_shapes`：本次几何发生变化的持久形状 id；
/// - `histories`：这些形状的**带历史记录**（[`HistoryRecord`]，来自
///   [`GeometryService`](crate::geometry_service::GeometryService) 带历史布尔在
///   **布尔当刻**的采集，调用方负责把步骤号对到 `ShapeId`）。为空则退避到几何指纹。
///
/// # 返回
///
/// [`UpdateReport`]：见各字段。
///
/// # 边界
///
/// - `changed_shapes` 为空 → 返回全 0 报告，**不改图纸任何字节**。
/// - 只处理"绑定了被改形状"的视图；无关视图不进 `stale_views`、不被重投。
pub fn update_after_commit(
    kernel: &mut Kernel,
    doc: &Document,
    drawing: &mut Drawing,
    changed_shapes: &[ShapeId],
    histories: &[(ShapeId, HistoryRecord)],
) -> AppResult<UpdateReport> {
    if changed_shapes.is_empty() {
        return Ok(UpdateReport::default());
    }

    // ① 旧键 → 新键：演化优先、指纹退避。
    let delta = resolve_delta(kernel, doc, drawing, changed_shapes, histories)?;

    // 分路径计数：**由 resolve_delta 如实标注**哪些旧键经演化解析（P1-3）。
    // 注：不再"按该形状有没有历史"计数——那会把指纹兜底也误算成演化。
    let evolution_resolved = delta.via_evolution.len();
    let fingerprint_resolved = delta.map.len() - evolution_resolved;
    let resolved = &delta.map;

    // ② 受影响图元 / 标注（旧键与新键的绑定都算——重解析后两个方向都可能被引用）。
    let affected = drawing.dependencies.affected_after_resolve(resolved);

    // ③ 受影响视图：绑定了被改形状的视图（其余视图一概不动）。
    let affected_views: BTreeSet<ViewId> = drawing.views_bound_to_shapes(changed_shapes);

    // 重投要用制图标准，先从 drawing 里取一份（避免与 views 的可变借用冲突）。
    let standard: DraftingStandard = drawing.standard.clone();

    // ④ 定向重投：只重投受影响的形状；无关图元逐字节不变。
    let mut reprojected_entities = 0usize;
    let mut warnings: Vec<String> = Vec::new();
    for &view_id in &affected_views {
        let Some(view) = drawing.views.get_mut(&view_id) else {
            continue;
        };
        for &shape in changed_shapes {
            if view_binds_shape(view, shape) {
                let (n, mut ws) = reproject_shape_in_place(kernel, doc, view, shape, &standard)?;
                reprojected_entities += n;
                warnings.append(&mut ws);
            }
        }
    }

    // ⑤ 重算相关标注（在重投之后；associates 因 id 原地复用而仍然有效）。
    let mut remeasured_annotations = 0usize;
    for &view_id in &affected_views {
        let Some(view) = drawing.views.get_mut(&view_id) else {
            continue;
        };
        // 图元只读借用与标注可变借用是**不同字段**，可并存。
        let entity_refs: Vec<&Entity2D> = view.entities.iter().collect();
        for ann in &mut view.annotations {
            // 先按 delta 改绑宿主编号，保持与索引一致。
            if let Some(key) = ann.bound_to.clone()
                && let Some(new_key) = resolved.get(&key)
            {
                ann.bound_to = Some(new_key.clone());
            }
            if !ann.needs_model_tracking() {
                continue;
            }
            let bound_to_changed = ann
                .bound_to
                .as_ref()
                .is_some_and(|key| changed_shapes.contains(&key.shape));
            if (bound_to_changed || affected.contains(&ann.id))
                && ann.remeasure_from_entities(&entity_refs)
            {
                remeasured_annotations += 1;
            }
        }
    }

    // ⑥ 把索引里的旧键改绑到新键（标注的 `bound_to` 已在 ⑤ 同步，二者一致）。
    drawing.dependencies.remap(resolved);

    // ⑦ 标脏需要重新生成的视图。
    let stale_views = drawing.mark_views_stale(&affected_views);

    Ok(UpdateReport {
        changed_shapes: changed_shapes.to_vec(),
        evolution_resolved,
        fingerprint_resolved,
        reprojected_entities,
        remeasured_annotations,
        stale_views,
        warnings,
    })
}

/// 视图是否绑定了某形状（图元 `source` 或标注 `bound_to` 的 `ShapeId` 命中）。
fn view_binds_shape(view: &View, shape: ShapeId) -> bool {
    view.entities
        .iter()
        .filter_map(|entity| entity.source.as_ref())
        .any(|key| key.shape == shape)
        || view
            .annotations
            .iter()
            .filter_map(|ann| ann.bound_to.as_ref())
            .any(|key| key.shape == shape)
}

/// **定向重投单个形状**到一个视图，返回 `(写回的新图元数, 警告)`。
///
/// 保持"其余形状的图元逐字节不变"：只摘除属于 `shape` 的旧图元，把新投影按位置
/// **复用旧 `EntityId`** 放回（首个旧图元处，稳定顺序）。
///
/// 投影警告（曲线降级 / 丢弃等）**不再被吞**（P1-2）。
fn reproject_shape_in_place(
    kernel: &mut Kernel,
    doc: &Document,
    view: &mut View,
    shape: ShapeId,
    standard: &DraftingStandard,
) -> AppResult<(usize, Vec<String>)> {
    let (fresh, warnings) = project_shape(kernel, doc, shape, view, standard)?;

    let belongs = |entity: &Entity2D| entity.source.as_ref().is_some_and(|key| key.shape == shape);

    let old_ids: Vec<core_doc::EntityId> = view
        .entities
        .iter()
        .filter(|entity| belongs(entity))
        .map(|entity| entity.id)
        .collect();
    let insert_at = view
        .entities
        .iter()
        .position(&belongs)
        .unwrap_or(view.entities.len());

    // 摘掉该形状的旧图元（其它形状原地保留）。
    view.entities.retain(|entity| !belongs(entity));

    // 位置复用旧 id：标注的 associates 在重投后依旧指向这些图元。
    let written = fresh.len();
    let insert_at = insert_at.min(view.entities.len());
    for (offset, mut entity) in fresh.into_iter().enumerate() {
        if let Some(id) = old_ids.get(offset) {
            entity.id = *id;
        }
        view.entities.insert(insert_at + offset, entity);
    }

    view.up_to_date = true;
    Ok((written, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **可观测的"无事发生"**：`changed_shapes` 为空 → 报告全 0、图纸逐字节不变。
    #[test]
    fn update_report_zero_when_nothing_changed() {
        let mut kernel = Kernel::mock();
        let doc = Document::new("空文档");
        let mut drawing = Drawing::new();
        let before = drawing.clone();

        let report = update_after_commit(&mut kernel, &doc, &mut drawing, &[], &[]).unwrap();

        assert_eq!(report, UpdateReport::default(), "无改动必须给出全 0 报告");
        assert!(report.changed_shapes.is_empty());
        assert_eq!(report.stale_views, 0);
        assert_eq!(report.reprojected_entities, 0);
        assert_eq!(report.remeasured_annotations, 0);
        assert_eq!(report.evolution_resolved, 0);
        assert_eq!(report.fingerprint_resolved, 0);
        assert_eq!(drawing, before, "无改动时图纸必须逐字节不变（可观测）");
    }

    // ======================= 真实内核腿（--features occt --release） =======================

    #[cfg(feature = "occt")]
    use crate::projection_service::reproject_into;
    #[cfg(feature = "occt")]
    use crate::topo_naming::{face_hash_names_from_handle, face_names};
    #[cfg(feature = "occt")]
    use core_doc::{Annotation, DraftingStandard, ViewKind, ViewOrientation};
    #[cfg(feature = "occt")]
    use core_kernel::{FaceEvolution, tolerance};
    #[cfg(feature = "occt")]
    use core_model::{Category, Element, GeometryKey, GeometryRecord};
    #[cfg(feature = "occt")]
    use glam::DVec2;

    /// 拿一具**真实**内核；release 腿里**先断言非 mock**——绝不在占位后端上假绿
    /// （"跳过"≠"通过"）。
    ///
    /// debug 腿下由各调用方 `#[test]` 自行标记为 **ignored**，此处不再静默跳过。
    #[cfg(feature = "occt")]
    fn real_kernel(test: &str) -> Kernel {
        let kernel = Kernel::new()
            .expect("启用 occt feature 后 Kernel::new() 必须成功（occt 腿固定 --release）");
        assert!(
            kernel.is_real(),
            "{test} 必须跑真实内核，不许在占位后端上假绿；实得后端 {}",
            kernel.backend_name()
        );
        kernel
    }

    /// 某形状在视图里的图元 JSON（按 `source.shape` 过滤）——用于"逐字节不变"判等。
    #[cfg(feature = "occt")]
    fn entities_json(view: &View, shape: ShapeId) -> Vec<String> {
        view.entities
            .iter()
            .filter(|entity| entity.source.as_ref().is_some_and(|key| key.shape == shape))
            .map(|entity| serde_json::to_string(entity).unwrap())
            .collect()
    }

    /// **带历史布尔端到端贯通真实内核**：两个相交 box 走 `cut_with_history`，
    /// 结果可继续用、面级演化可解、`deleted` 是输入哈希子集。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn boolean_history_wires_end_to_end_through_core_kernel() {
        let mut kernel = real_kernel("boolean_history_wires_end_to_end_through_core_kernel");

        let a = kernel.make_box(100.0, 100.0, 100.0).unwrap();
        let b = kernel.make_box(100.0, 100.0, 100.0).unwrap();
        let b = kernel.translate(b, 50.0, 50.0, 50.0).unwrap();

        let mut inputs = kernel.face_hashes(a).unwrap();
        assert!(!inputs.is_empty(), "盒必须有面哈希");
        inputs.extend(kernel.face_hashes(b).unwrap());
        let input_set: BTreeSet<u64> = inputs.iter().copied().collect();

        let evo = kernel.cut_with_history(a, b, &inputs).unwrap();

        // result 可继续用
        let volume = kernel.volume(evo.result).unwrap();
        assert!(
            (volume - 875_000.0).abs() / 875_000.0 < 1e-6,
            "两立方体（错位 50）相减体积应约 875000，实得 {volume}"
        );
        let mesh = kernel
            .tessellate(
                evo.result,
                tolerance::DEFAULT_LINEAR_DEFLECTION,
                tolerance::DEFAULT_ANGULAR_DEFLECTION,
            )
            .unwrap();
        assert!(
            mesh.vertex_count() > 0 && mesh.triangle_count() > 0,
            "带历史布尔的结果必须可网格化"
        );

        // modified / generated 解出非空分组
        let groups: Vec<&FaceEvolution> = evo.modified.iter().chain(evo.generated.iter()).collect();
        assert!(
            !groups.is_empty(),
            "必须有面级演化分组（modified ∪ generated 不能同时为空）"
        );
        assert!(
            groups.iter().all(|group| !group.outputs.is_empty()),
            "每个演化分组的输出面不得为空（k ≥ 1）"
        );

        // deleted 是输入哈希子集
        for hash in &evo.deleted {
            assert!(
                input_set.contains(hash),
                "deleted 必须落在输入面哈希集合内；实得 {hash} 不在输入"
            );
        }
    }

    /// **P0-1 收口证据（真实内核）**：跨文档重装域下，绑在**输入面**上的键经
    /// [`update_after_commit`] 的**演化主路径**改绑到**输出面**。
    ///
    /// 关键：`history` 里的"面哈希 → 稳定名"表是**布尔当刻从活句柄采集**的
    /// （[`face_hash_names_from_handle`]），故 [`Evolution`](core_kernel::Evolution)
    /// 的实例级哈希域与采集表同域；而文档里的旧键（`old_names`）取自 **BREP 重装域**
    /// ——两域不同，靠 [`TopoName`](core_model::TopoName) 精确匹配缝合。
    /// 这正是修复前 `evolution_resolved` 恒 0 的那条路径。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn evolution_history_realigns_bound_key_across_document_reload() {
        let mut kernel = real_kernel("evolution_history_realigns_bound_key_across_document_reload");
        let shape = ShapeId::new();

        // ① 真实内核：造 a，导出 BREP（旧几何）。
        let a = kernel.make_box(100.0, 100.0, 100.0).unwrap();
        let brep_old = kernel.export_brep(a).unwrap();

        // ② **活句柄域**：a 的"面哈希 → 稳定名"。
        let mut input_names = face_hash_names_from_handle(&mut kernel, a).unwrap();
        assert!(!input_names.is_empty(), "盒必须采集到面名");

        // ③ **文档重装域**：把旧几何收进文档，取该域的面名表。
        let mut doc = Document::new("跨文档演化");
        doc.insert_geometry(GeometryRecord::brep(shape, brep_old))
            .unwrap();
        let old_names = face_names(&mut kernel, &doc, shape).unwrap();
        assert!(!old_names.is_empty(), "重装域必须能算出面名");

        // ④ 刀具体 b：尺寸与 a **不同**（避免逐位同名导致名→哈希歧义）。
        let b = kernel.make_box(60.0, 60.0, 60.0).unwrap();
        let b = kernel.translate(b, 70.0, 70.0, 70.0).unwrap();
        input_names.extend(face_hash_names_from_handle(&mut kernel, b).unwrap());

        // ⑤ 布尔当刻：面哈希并集 → cut_with_history → 采集结果面名。
        let mut hashes = kernel.face_hashes(a).unwrap();
        hashes.extend(kernel.face_hashes(b).unwrap());
        let evo = kernel.cut_with_history(a, b, &hashes).unwrap();
        let output_names = face_hash_names_from_handle(&mut kernel, evo.result).unwrap();
        let history = HistoryRecord::new(evo, input_names, output_names);

        // ⑥ 选一个确实被改到、且稳定名在文档重装域可见的输入面。
        let chosen = history.input_names.iter().find_map(|(hash, name)| {
            if history.evolution.outputs_of(*hash).is_empty() {
                return None;
            }
            old_names
                .iter()
                .find(|(_, n)| n.exact_match(name))
                .map(|(idx, _)| (*idx, *name))
        });
        let (old_index, old_name) = chosen
            .expect("cut 必须改到某输入面，且其稳定名应在 doc 重装域可见（TopoName 跨会话稳定）");

        // ⑦ 图纸：一条绑在**输入面**上的标注承载旧键。
        let mut drawing = Drawing::new();
        let mut view =
            View::new("正立面", ViewKind::Elevation).with_orientation(ViewOrientation::front());
        view.push_annotation(
            Annotation::linear(DVec2::ZERO, DVec2::new(10.0, 0.0))
                .bound_to(GeometryKey::face_named(shape, old_index, old_name)),
        );
        drawing.add_view(view);
        drawing.rebuild_index();

        // ⑧ 文档换成**新几何**（布尔结果，落在与活句柄不同的域）。
        let brep_new = kernel.export_brep(history.evolution.result).unwrap();
        doc.remove_geometry(shape);
        doc.insert_geometry(GeometryRecord::brep(shape, brep_new))
            .unwrap();

        // ⑨ 关联更新：跨文档重装域的演化主路径必须命中。
        let report = update_after_commit(
            &mut kernel,
            &doc,
            &mut drawing,
            &[shape],
            &[(shape, history)],
        )
        .unwrap();

        assert!(
            report.evolution_resolved >= 1,
            "演化主路径必须真的命中（跨 doc 重装域）；实得 evolution_resolved={}",
            report.evolution_resolved
        );

        // ⑩ 该标注的绑定已改到**输出面**（不再是旧输入面名）。
        let rebound = drawing.views.values().next().unwrap().annotations[0]
            .bound_to
            .clone();
        assert!(
            rebound.as_ref().and_then(GeometryKey::topo_name) != Some(&old_name),
            "标注必须改绑到输出面，而非停留在旧输入面名；实得 {rebound:?}"
        );
    }

    /// **§4 场景 B 主验收**：改构件 A 尺寸 → A 的图元变、构件 B 的图元**逐字节不变**。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn associative_update_reprojects_only_the_changed_shape() {
        let mut kernel = real_kernel("associative_update_reprojects_only_the_changed_shape");
        let standard = DraftingStandard::gb_industrial();
        let shape_a = ShapeId::new();
        let shape_b = ShapeId::new();

        // 两根柱（A 与 B）同在一张正立面视图里。
        let mut doc = Document::new("两柱");
        let ha = kernel.make_box(400.0, 400.0, 3000.0).unwrap();
        let hb = kernel.make_box(400.0, 400.0, 3000.0).unwrap();
        let hb = kernel.translate(hb, 3000.0, 0.0, 0.0).unwrap();
        doc.insert_geometry(GeometryRecord::brep(
            shape_a,
            kernel.export_brep(ha).unwrap(),
        ))
        .unwrap();
        doc.insert_geometry(GeometryRecord::brep(
            shape_b,
            kernel.export_brep(hb).unwrap(),
        ))
        .unwrap();
        doc.insert_element(Element::new(Category::generic(), "柱A").with_geometry(shape_a))
            .unwrap();
        doc.insert_element(Element::new(Category::generic(), "柱B").with_geometry(shape_b))
            .unwrap();

        let mut drawing = Drawing::new();
        let mut view =
            View::new("正立面", ViewKind::Elevation).with_orientation(ViewOrientation::front());
        reproject_into(&mut kernel, &doc, &mut view, &standard).unwrap();
        drawing.add_view(view);
        drawing.rebuild_index();

        let view_id = *drawing.views.keys().next().unwrap();
        let before_a = entities_json(&drawing.views[&view_id], shape_a);
        let before_b = entities_json(&drawing.views[&view_id], shape_b);
        assert!(
            !before_a.is_empty() && !before_b.is_empty(),
            "A、B 两个构件都必须有投影图元（否则判等无意义）"
        );

        // 改 A：宽 400 → 600。
        let ha2 = kernel.make_box(600.0, 400.0, 3000.0).unwrap();
        doc.remove_geometry(shape_a);
        doc.insert_geometry(GeometryRecord::brep(
            shape_a,
            kernel.export_brep(ha2).unwrap(),
        ))
        .unwrap();

        let report = update_after_commit(&mut kernel, &doc, &mut drawing, &[shape_a], &[]).unwrap();

        let after_a = entities_json(&drawing.views[&view_id], shape_a);
        let after_b = entities_json(&drawing.views[&view_id], shape_b);

        assert!(report.reprojected_entities > 0, "定向重投必须有产出");
        assert_eq!(report.stale_views, 1, "只有绑定 A 的那张视图被标脏");
        assert_ne!(after_a, before_a, "构件 A 的图元必须随尺寸改变");
        assert_eq!(
            after_b, before_b,
            "构件 B 的图元必须逐字节不变（只重投受影响形状）"
        );
    }

    /// **场景 B 尺寸联动**：柱宽 400 → 600 → 尺寸标注量测值自动 400 → 600（不手工改图）。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn associative_update_remeasures_dimension_automatically() {
        let mut kernel = real_kernel("associative_update_remeasures_dimension_automatically");
        let standard = DraftingStandard::gb_industrial();
        let shape = ShapeId::new();

        let mut doc = Document::new("柱");
        let handle = kernel.make_box(400.0, 400.0, 3000.0).unwrap();
        doc.insert_geometry(GeometryRecord::brep(
            shape,
            kernel.export_brep(handle).unwrap(),
        ))
        .unwrap();
        doc.insert_element(Element::new(Category::generic(), "柱").with_geometry(shape))
            .unwrap();

        let mut drawing = Drawing::new();
        let mut view =
            View::new("正立面", ViewKind::Elevation).with_orientation(ViewOrientation::front());
        reproject_into(&mut kernel, &doc, &mut view, &standard).unwrap();
        assert!(!view.entities.is_empty(), "柱的正立面必须投影出图元");

        // 尺寸标注：只关联**左右两条竖直棱线**（最邻近点对 = 柱宽）。
        // 不能关联全部图元——同一角点被多条边重复引用，最近点对会退化成 0。
        let verticals: Vec<&Entity2D> = view
            .entities
            .iter()
            .filter(|e| e.points.len() == 2 && (e.points[0].x - e.points[1].x).abs() < 1e-9)
            .collect();
        assert!(verticals.len() >= 2, "正立面必须至少有两条竖直棱线");
        let left = verticals
            .iter()
            .min_by(|a, b| a.points[0].x.partial_cmp(&b.points[0].x).unwrap())
            .unwrap();
        let right = verticals
            .iter()
            .max_by(|a, b| a.points[0].x.partial_cmp(&b.points[0].x).unwrap())
            .unwrap();
        let mut annotation = Annotation::linear(DVec2::ZERO, DVec2::new(400.0, 0.0))
            .bound_to(GeometryKey::whole(shape));
        annotation.associates = vec![left.id, right.id];
        let before = annotation.measured_value().unwrap();
        assert!(
            (before - 400.0).abs() < 1e-6,
            "初始量测应为柱宽 400，实得 {before}"
        );
        view.push_annotation(annotation);
        drawing.add_view(view);
        drawing.rebuild_index();

        // 改宽 400 → 600。
        let handle2 = kernel.make_box(600.0, 400.0, 3000.0).unwrap();
        doc.remove_geometry(shape);
        doc.insert_geometry(GeometryRecord::brep(
            shape,
            kernel.export_brep(handle2).unwrap(),
        ))
        .unwrap();

        let report = update_after_commit(&mut kernel, &doc, &mut drawing, &[shape], &[]).unwrap();

        let view_id = *drawing.views.keys().next().unwrap();
        let after = drawing.views[&view_id].annotations[0]
            .measured_value()
            .unwrap();
        assert!(
            (after - 600.0).abs() < 1e-6,
            "尺寸标注必须自动从 400 变到 600（不手工改图），实得 {after}"
        );
        assert_eq!(report.remeasured_annotations, 1, "应恰好自动重算 1 条标注");
    }
}

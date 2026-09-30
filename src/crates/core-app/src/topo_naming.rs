//! 稳定拓扑命名：几何指纹计算 + 跨会话重解析（M5 规划 §2.3(f) / §3.4）。
//!
//! ## 解决的问题
//!
//! `GeometryKey` 里的子形状定位（[`core_model::SubShapeRef`]）原先只是一个
//! **会话内拓扑序号**——它假设"第 3 个面"跨会话 / 跨重建不变。**事实并非如此**：
//! OCCT 的子形状顺序由其内部遍历决定，跨会话不稳定（M5 规划 §3.1）。
//!
//! 本模块把"一个面的身份"从"内核给它的序号"改由**它自身的几何**决定：
//!
//! - **代表点**：面网格三角形的**面积加权质心**；
//! - **法向**：面网格三角形法向的**面积加权平均**（归一化）；
//! - **尺度**：面网格**三角形面积之和**。
//!
//! 这三者都是**形状的内蕴几何量**，与内核内部排序无关；且代表点用
//! **相对形状包围盒 min** 的局部坐标（[`core_model::POS_STEP_MM`] 量化）→
//! **对整体平移不变**（M5 规划 §3.5）。故由此构造的 [`TopoName`] 跨会话稳定。
//!
//! ## 红线（M5 规划附录 A#2/#3）
//!
//! - 面哈希是**会话内**量，**只作会话内回连**（`face_hash` ↔ 面序号），
//!   **绝不进持久键**；
//! - 面哈希**只经 [`Kernel::face_hashes`] 取得**（域由内核定死），
//!   **禁止自取 `upper_bound`**。
//!
//! ## 为什么在 core-app
//!
//! 指纹计算要 `tessellate` / `face_hashes`（内核），而 `ShapeId` → `ShapeHandle`
//! 的装载必须读 `Document` 里的 `GeometryRecord`——只有 `core-app` 同时看得见两者
//! （与 `projection_service` 一致）。

use std::collections::BTreeMap;

use core_kernel::{Kernel, KernelError, KernelResult, MeshData, ShapeHandle, SubShapeKind};
use core_model::ids::MAX_TOPO_DISTANCE;
use core_model::{Document, GeometryKey, ModelError, ShapeId, SubShapeRef, TopoName};
use glam::DVec3;

use crate::error::AppResult;
use crate::geometry_service::ShapeLoader;
use crate::loader::BrepShapeLoader;

/// 网格细分的线性挠度（mm）。越细，面积 / 质心估计越稳。
const TESS_LINEAR_MM: f64 = 0.5;
/// 网格细分的角度挠度（弧度）。
const TESS_ANGULAR_RAD: f64 = 0.5;
/// 判定"零面积组 / 退化三角形"的阈值。
const AREA_EPS: f64 = 1e-12;
/// 判定"最近匹配并列"的容差：两候选距离差在此以内即视为并列 → 拒绝解析
/// （歧义时**宁可不解析，也不乱指**）。
const AMBIGUITY_EPS: f64 = 1e-9;

/// 面指纹表：`(面序号, 稳定名)`，面序号与 [`Kernel::face_hashes`] 同序。
pub type FaceNameTable = Vec<(u32, TopoName)>;

/// 面指纹表 + 同序的会话面哈希（`hashes[i]` 对应 `names[i].0`）。
pub type FaceNamesAndHashes = (FaceNameTable, Vec<u64>);

/// 由网格算出一个形状的**面**指纹表：`(面序号, 稳定名)`。
///
/// - 面序号 = 该组在 `mesh.face_groups` 中的位置，且 **`face_groups` 与
///   [`Kernel::face_hashes`] 同序同域**（M0 钉子 `face_group_hashes_map_back_to_face_handles`
///   先验通过）——故此处位置即内核面序号，`face_hash` 仅作会话内回连凭证；
/// - 代表点 = 该组三角形的**面积加权质心**；
/// - 法向 = 该组三角形法向的**面积加权平均**并归一化；
/// - 尺度 = 该组**三角形面积之和**；
/// - `bbox_min` = `mesh` 全体顶点逐轴最小值（作局部坐标原点）。
///
/// **纯函数**：不碰内核，便于默认腿用合成 [`MeshData`] 直接测试。
/// 面积为零的退化组被**跳过**（无法定义质心 / 法向），不产出假指纹。
#[must_use]
pub fn face_names_from_mesh(mesh: &MeshData) -> FaceNameTable {
    let bbox_min = bbox_min(mesh);
    let mut out = Vec::new();

    for (ordinal, group) in mesh.face_groups.iter().enumerate() {
        let start = group.tri_start as usize;
        let count = group.tri_count as usize;

        let mut total_area = 0.0_f64;
        let mut centroid = DVec3::ZERO;
        let mut normal = DVec3::ZERO;

        for t in 0..count {
            let base = (start + t) * 3;
            let (Some(&ia), Some(&ib), Some(&ic)) = (
                mesh.indices.get(base),
                mesh.indices.get(base + 1),
                mesh.indices.get(base + 2),
            ) else {
                continue;
            };
            let (Some(&pa), Some(&pb), Some(&pc)) = (
                mesh.positions.get(ia as usize),
                mesh.positions.get(ib as usize),
                mesh.positions.get(ic as usize),
            ) else {
                continue;
            };
            let pa = DVec3::from_array(pa);
            let pb = DVec3::from_array(pb);
            let pc = DVec3::from_array(pc);

            let cross = (pb - pa).cross(pc - pa);
            let area = 0.5 * cross.length();
            if area <= AREA_EPS {
                continue;
            }
            total_area += area;
            centroid += area * ((pa + pb + pc) / 3.0);
            normal += area * cross.normalize_or_zero();
        }

        if total_area <= AREA_EPS {
            // 退化组：无面积 → 无法定义身份，跳过（绝不产出假指纹）。
            continue;
        }

        centroid /= total_area;
        let mean_normal = normal.normalize_or_zero();
        out.push((
            ordinal as u32,
            TopoName::of_face(
                centroid.to_array(),
                mean_normal.to_array(),
                total_area,
                bbox_min,
            ),
        ));
    }

    out
}

/// 一个形状的**面**指纹表 + 同序的会话面哈希（`face_names` 的带哈希版）。
///
/// 返回 `(Vec<(面序号, TopoName)>, Vec<u64>)`，两者**同序**：
/// `hashes[i]` 是面序号 `names[i].0` 的会话哈希。
///
/// 供 `evolution_align`（演化对齐）一次装载即取得"名字 ↔ 哈希"两侧信息，
/// 避免重复装载同一形状。
pub fn face_names_and_hashes(
    kernel: &mut Kernel,
    doc: &Document,
    shape: ShapeId,
) -> AppResult<FaceNamesAndHashes> {
    load_topology(kernel, doc, shape, |kernel, handle| {
        let mesh = kernel.tessellate(handle, TESS_LINEAR_MM, TESS_ANGULAR_RAD)?;
        let hashes = kernel.face_hashes(handle)?;
        Ok((face_names_from_mesh(&mesh), hashes))
    })
}

/// 从**活句柄**采集"会话面哈希 → 稳定名"表（不读文档、不装载）。
///
/// 会话面哈希是**实例级**的，跨会话不稳；本函数在句柄活着的当刻把哈希与
/// 跨会话稳定的 [`TopoName`] 绑在一起，供 `evolution_align` 透传对齐。
///
/// 面哈希不可用（后端不支持 / 句柄已释放）时返回 `Err`，由调用方决定是否
/// 退避到几何指纹（绝不静默造一张假表）。
pub fn face_hash_names_from_handle(
    kernel: &mut Kernel,
    handle: ShapeHandle,
) -> AppResult<BTreeMap<u64, TopoName>> {
    let mesh = kernel.tessellate(handle, TESS_LINEAR_MM, TESS_ANGULAR_RAD)?;
    let names = face_names_from_mesh(&mesh);
    let hashes = kernel.face_hashes(handle)?;
    let mut out = BTreeMap::new();
    for (index, name) in names {
        if let Some(&hash) = hashes.get(index as usize) {
            out.insert(hash, name);
        }
    }
    Ok(out)
}

/// 一个形状的**面**指纹表（装载 → 网格化 → 算指纹 → **释放句柄**）。
///
/// # 与规划的一处偏差（据实）
///
/// M5 §2.3(f) 的原签名是 `face_names(kernel, shape, bbox_min)`。但 `ShapeId`
/// （持久 UUID）→ `ShapeHandle`（会话句柄）必须读 [`Document`] 的
/// `GeometryRecord` 才能装载（见 `GeometryService::resolve`），且 `bbox_min`
/// 由网格自算更稳（不必外部先给）。故此处签名改为
/// `face_names(kernel, doc, shape)`（照 B1 `project_shape` 已确立的做法）。
pub fn face_names(kernel: &mut Kernel, doc: &Document, shape: ShapeId) -> AppResult<FaceNameTable> {
    Ok(face_names_and_hashes(kernel, doc, shape)?.0)
}

/// **边**指纹表：`(边序号, 稳定名)`，边序号 = [`Kernel::sub_shapes`]`(_, Edge)` 的位置。
pub type EdgeNameTable = Vec<(u32, TopoName)>;

/// **顶点**指纹表：`(顶点序号, 稳定名)`，顶点序号 = [`Kernel::sub_shapes`]`(_, Vertex)` 的位置。
pub type VertexNameTable = Vec<(u32, TopoName)>;

/// 从**活句柄**算出一个形状的**面**指纹表（`face_names` 的活句柄版）。
///
/// 网格化 → [`face_names_from_mesh`]。序号 = `face_groups` 位置，与
/// `sub_shapes(Face)` **同序**（R16 钉子 `sub_faces_are_same_order_as_face_hashes` 已证）。
/// 是 [`EdgeSelector::ByTopoName`](crate::EdgeSelector::ByTopoName) 与
/// [`FaceSelector::ByTopoName`](crate::FaceSelector::ByTopoName) 的**活句柄名表来源**
/// （`selector::resolve` 只拿到 `&mut Kernel` + 活 `ShapeHandle`，**不接触 `ShapeId`/`Document`**）。
pub fn face_names_from_handle(
    kernel: &mut Kernel,
    handle: ShapeHandle,
) -> AppResult<FaceNameTable> {
    let mesh = kernel.tessellate(handle, TESS_LINEAR_MM, TESS_ANGULAR_RAD)?;
    Ok(face_names_from_mesh(&mesh))
}

/// 单条边的稳定名（从活句柄 · 抽取以便复用）。
///
/// # 硬约束（**实测依据，违反即错**；M5 §2.7.13 定向实测）
///
/// - **代表点 = 参数域中点**：`mid = (first + last) / 2`（`curve_parameters`）→
///   `curve_point_at_param(edge, mid)`。**禁止**用 `project_point_on_edge` 的最近点：
///   后者**依赖外部查询点、不是边的内在量**——同一条圆弧换个查询点就得到另一个代表点
///   （同一弧上两点可相距 **16.64 mm**），且查询点取**圆弧圆心**等退化位置时最近点
///   不唯一 / 不存在 → `Err`；而参数域中点 `t` 是**内在唯一量**（往返位移 ≤ **3e-14**）。
///   注：最近点投影**跨往返是稳定的**（≤ 5.9e-14），故其被弃用**不是**因为"跨往返漂移"。
/// - **切向必须归一化**：`project_point_on_edge` 的 `tangent` 实为 `dP/dparam`，
///   圆边模长 = 半径（实测 15，**非单位**）→ `normalize_or_zero()`。
/// - **长度 = `curve_length(edge)`**（真几何长度），**不是** `curve_parameters` 的
///   `last − first`（角度参数化圆边 span = π/2 ≈ 1.571，而弧长 = 15·π/2 ≈ 23.562）。
fn edge_name_of(
    kernel: &mut Kernel,
    edge: ShapeHandle,
    bbox_min: [f64; 3],
) -> KernelResult<TopoName> {
    let range = kernel.curve_parameters(edge)?;
    let mid = (range.first + range.last) / 2.0;
    let rep = kernel.curve_point_at_param(edge, mid)?;
    let length = kernel.curve_length(edge)?;
    let tangent = kernel
        .project_point_on_edge(edge, rep)?
        .tangent
        .normalize_or_zero();
    Ok(TopoName::of_edge(
        rep.to_array(),
        tangent.to_array(),
        length,
        bbox_min,
    ))
}

/// 从**活句柄**算出一个形状的**边**指纹表（有序，按 `sub_shapes(Edge)` 序号）。
///
/// # 错误口径（与 [`face_hash_names_from_handle`] 的"**绝不静默造表**"一致）
///
/// - `tessellate` / `sub_shapes` 失败 → `?` 透传；
/// - **任一条边**的 `curve_parameters` / `curve_point_at_param` / `curve_length` /
///   `project_point_on_edge` 失败 → `Err(Geometry{op:"edge_names"})`（**响亮报错，
///   绝不静默跳过该边**——跳边会让名表与 `sub_shapes(Edge)` 序号**错位**，
///   是比报错更隐蔽的假绿）；
/// - 空子形状 → `Ok(vec![])`（与 `face_names_from_mesh` 一致，不造假条目）。
pub fn edge_names_from_handle(
    kernel: &mut Kernel,
    handle: ShapeHandle,
) -> AppResult<EdgeNameTable> {
    let mesh = kernel.tessellate(handle, TESS_LINEAR_MM, TESS_ANGULAR_RAD)?;
    let bbox_min = bbox_min(&mesh);
    let edges = kernel.sub_shapes(handle, SubShapeKind::Edge)?;
    let mut out = Vec::with_capacity(edges.len());
    for (i, &edge) in edges.iter().enumerate() {
        let name = edge_name_of(kernel, edge, bbox_min).map_err(|e| KernelError::Geometry {
            op: "edge_names",
            msg: format!("第 {i} 条边指纹计算失败：{e}"),
        })?;
        out.push((i as u32, name));
    }
    Ok(out)
}

/// 从**活句柄**算出一个形状的**顶点**指纹表（有序，按 `sub_shapes(Vertex)` 序号）。
///
/// 顶点坐标用 `bounding_box(vertex)` 的**退化盒**取中心（实测 cube 顶点返回
/// `size=[0,0,0]`，盒中心即精确坐标）→ [`TopoName::of_vertex`]。
/// **顶点指纹不需新增内核门面**（与边不同）。
///
/// 错误口径同 [`edge_names_from_handle`]：`bounding_box` 失败 → `Err(Geometry{op:"vertex_names"})`。
pub fn vertex_names_from_handle(
    kernel: &mut Kernel,
    handle: ShapeHandle,
) -> AppResult<VertexNameTable> {
    let mesh = kernel.tessellate(handle, TESS_LINEAR_MM, TESS_ANGULAR_RAD)?;
    let bbox_min = bbox_min(&mesh);
    let verts = kernel.sub_shapes(handle, SubShapeKind::Vertex)?;
    let mut out = Vec::with_capacity(verts.len());
    for (i, &v) in verts.iter().enumerate() {
        let bb = kernel.bounding_box(v).map_err(|e| KernelError::Geometry {
            op: "vertex_names",
            msg: format!("第 {i} 个顶点坐标取失败：{e}"),
        })?;
        let coord = [
            (bb.min[0] + bb.max[0]) / 2.0,
            (bb.min[1] + bb.max[1]) / 2.0,
            (bb.min[2] + bb.max[2]) / 2.0,
        ];
        out.push((i as u32, TopoName::of_vertex(coord, bbox_min)));
    }
    Ok(out)
}

/// 一个形状的**边**指纹表（装载 → 算指纹 → **释放句柄**；供 [`resolve_key`] 用）。
pub fn edge_names(kernel: &mut Kernel, doc: &Document, shape: ShapeId) -> AppResult<EdgeNameTable> {
    load_topology(kernel, doc, shape, edge_names_from_handle)
}

/// 一个形状的**顶点**指纹表（装载 → 算指纹 → **释放句柄**；供 [`resolve_key`] 用）。
pub fn vertex_names(
    kernel: &mut Kernel,
    doc: &Document,
    shape: ShapeId,
) -> AppResult<VertexNameTable> {
    load_topology(kernel, doc, shape, vertex_names_from_handle)
}

/// 在候选名里为旧名找**最佳匹配**。
///
/// 1. 有 `exact_match` 者 → 取该候选序号；**多个精确匹配 ⇒ 歧义，拒绝**；
/// 2. 否则取 `distance` 最小且 `< MAX_TOPO_DISTANCE` 者；**最近者并列
///    （差 `< AMBIGUITY_EPS`）⇒ 歧义，拒绝**；
/// 3. 都不满足 → `None`（**宁可不解析，也不乱指**）。
#[must_use]
pub fn resolve_by_name(old: &TopoName, candidates: &[(u32, TopoName)]) -> Option<u32> {
    // ① 精确匹配优先；但不止一个精确匹配时无法唯一确定身份 → 拒绝。
    let mut exact: Option<u32> = None;
    for (index, name) in candidates {
        if name.exact_match(old) {
            if exact.is_some() {
                // 第二个精确匹配：歧义，绝不随便挑一个。
                return None;
            }
            exact = Some(*index);
        }
    }
    if exact.is_some() {
        return exact;
    }

    // ② 最近匹配，且必须落在可信阈值内；若最近距离"并列" → 歧义，拒绝。
    let mut best: Option<(f64, u32)> = None;
    let mut ambiguous = false;
    for (index, name) in candidates {
        let d = old.distance(name);
        if d >= MAX_TOPO_DISTANCE {
            continue;
        }
        match best {
            Some((best_d, _)) => {
                if d < best_d - AMBIGUITY_EPS {
                    // 明显更近：替换最优并清除此前的并列标记。
                    best = Some((d, *index));
                    ambiguous = false;
                } else if (d - best_d).abs() <= AMBIGUITY_EPS {
                    // 与当前最优并列 → 无法唯一确定身份。
                    ambiguous = true;
                }
            }
            None => {
                best = Some((d, *index));
                ambiguous = false;
            }
        }
    }
    if ambiguous {
        return None;
    }
    best.map(|(_, index)| index)
}

/// 把一个旧键重解析到新键（**B6.1b 起覆盖 `Whole` / `Face` / `Edge` / `Vertex`**）。
///
/// - `Whole`：`ShapeId` 是持久 UUID，天然稳定 → 直接原样返回；
/// - `Face`（带名）：重算该形状的**面**指纹（[`face_names`]）→ [`resolve_by_name`] → 新序号；
///   无名旧键无法几何重解析 → `None`；
/// - `Edge`（带名）：重算该形状的**边**指纹（[`edge_names`]）→ 新序号；
///   无名旧键 → `None`；
/// - `Vertex`（带名）：重算该形状的**顶点**指纹（[`vertex_names`]）→ 新序号；
///   无名旧键 → `None`。
///
/// **口径（四条分支完全一致）**：`Ok(None)` = **拒绝解析**，交调用方兜底
/// （关联更新内部路径有"整视图重投"兜底，属合法域路径）。
pub fn resolve_key(
    kernel: &mut Kernel,
    doc: &Document,
    shape: ShapeId,
    old: &GeometryKey,
) -> AppResult<Option<GeometryKey>> {
    match &old.sub {
        SubShapeRef::Whole => Ok(Some(GeometryKey::whole(shape))),
        SubShapeRef::Face(r) => {
            let Some(name) = r.name else {
                // 无名旧键：没有几何身份可比对，拒绝解析（交给整视图重投兜底）。
                return Ok(None);
            };
            let candidates = face_names(kernel, doc, shape)?;
            match resolve_by_name(&name, &candidates) {
                Some(index) => Ok(Some(GeometryKey::face_named(shape, index, name))),
                None => Ok(None),
            }
        }
        SubShapeRef::Edge(r) => {
            let Some(name) = r.name else {
                // 无名旧键：无法几何重解析（交整视图重投兜底）。
                return Ok(None);
            };
            let candidates = edge_names(kernel, doc, shape)?;
            match resolve_by_name(&name, &candidates) {
                Some(index) => Ok(Some(GeometryKey::edge_named(shape, index, name))),
                None => Ok(None),
            }
        }
        SubShapeRef::Vertex(r) => {
            let Some(name) = r.name else {
                return Ok(None);
            };
            let candidates = vertex_names(kernel, doc, shape)?;
            match resolve_by_name(&name, &candidates) {
                Some(index) => Ok(Some(GeometryKey::vertex_named(shape, index, name))),
                None => Ok(None),
            }
        }
    }
}

/// 一次"装载形状 → 执行 `f` → **无论成败都释放句柄**"的统一入口。
fn load_topology<T>(
    kernel: &mut Kernel,
    doc: &Document,
    shape: ShapeId,
    f: impl FnOnce(&mut Kernel, core_kernel::ShapeHandle) -> AppResult<T>,
) -> AppResult<T> {
    let record = doc
        .geometry_record(shape)
        .ok_or(ModelError::GeometryNotFound(shape))?;
    let handle = BrepShapeLoader::new().load(kernel, record)?;
    let out = f(kernel, handle);
    // 释放句柄（避免 arena 泄漏）；释放失败不覆盖主结果。
    let _ = kernel.release(handle);
    out
}

/// `mesh` 全体顶点逐轴最小值（空网格退化为原点）。
fn bbox_min(mesh: &MeshData) -> [f64; 3] {
    let mut min = DVec3::splat(f64::INFINITY);
    for p in &mesh.positions {
        min = min.min(DVec3::from_array(*p));
    }
    if min.is_finite() {
        min.to_array()
    } else {
        [0.0; 3]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_kernel::FaceGroup;
    use core_model::TopoName;

    /// 两个互不共面的四边形面（各 2 个三角形），用于指纹测试。
    ///
    /// 面 0 在 `z=0`，面 1 在 `z=dz`；两者法向都是 `+Z`，但质心不同。
    fn two_quad_mesh(dz: f64) -> MeshData {
        let positions = vec![
            // 面 0（z=0）
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [10.0, 10.0, 0.0],
            [0.0, 10.0, 0.0],
            // 面 1（z=dz）
            [0.0, 0.0, dz],
            [10.0, 0.0, dz],
            [10.0, 10.0, dz],
            [0.0, 10.0, dz],
        ];
        let indices = vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7];
        let face_groups = vec![
            FaceGroup {
                tri_start: 0,
                tri_count: 2,
                face_hash: 1,
            },
            FaceGroup {
                tri_start: 2,
                tri_count: 2,
                face_hash: 2,
            },
        ];
        MeshData {
            positions,
            indices,
            normals: Vec::new(),
            face_groups,
        }
    }

    fn translate(mesh: &MeshData, t: [f64; 3]) -> MeshData {
        MeshData {
            positions: mesh
                .positions
                .iter()
                .map(|p| [p[0] + t[0], p[1] + t[1], p[2] + t[2]])
                .collect(),
            indices: mesh.indices.clone(),
            normals: mesh.normals.clone(),
            face_groups: mesh.face_groups.clone(),
        }
    }

    #[test]
    fn face_fingerprint_is_stable_across_repeated_computation() {
        let mesh = two_quad_mesh(5.0);
        let first = face_names_from_mesh(&mesh);
        let second = face_names_from_mesh(&mesh);
        assert_eq!(first, second, "同一网格两次算指纹必须完全相等");
        assert_eq!(first.len(), 2);
    }

    /// **"注定会失败"钉子**：把一个面整体搬走，它的指纹**必须变**——
    /// 防"指纹恒等"（把常量当指纹）式假绿。
    #[test]
    fn face_fingerprint_changes_when_face_moves() {
        let before = two_quad_mesh(5.0);
        // 只把**面 1** 沿 +Z 搬 10 mm（面 0 不动 → 包围盒 min 不变）。
        let after = two_quad_mesh(15.0);

        let b = face_names_from_mesh(&before);
        let a = face_names_from_mesh(&after);
        assert_eq!(b.len(), 2);
        assert_eq!(a.len(), 2);

        // 面 0 未动 → 指纹不变。
        assert_eq!(b[0], a[0], "未移动的面指纹不该变");

        // 面 1 动了 → 指纹必须变。
        assert_ne!(b[1].1, a[1].1, "被移动的面指纹必须变化（防指纹恒等式假绿）");
        // 且变化的是位置分量（法向 / 面积不变）。
        assert_ne!(b[1].1.pos_q, a[1].1.pos_q);
        assert_eq!(b[1].1.dir_q, a[1].1.dir_q);
        assert_eq!(b[1].1.size_q, a[1].1.size_q);
    }

    #[test]
    fn face_fingerprint_is_translation_invariant() {
        let mesh = two_quad_mesh(5.0);
        let moved = translate(&mesh, [100.0, 200.0, 300.0]);

        let a = face_names_from_mesh(&mesh);
        let b = face_names_from_mesh(&moved);
        assert_eq!(
            a, b,
            "整体平移后各面指纹必须不变（局部坐标相对包围盒 min 归一化的效果）"
        );
    }

    /// 一个"远"面指纹（质心距参照点 300 mm > 100 mm 参照尺度）。
    fn far_name() -> TopoName {
        TopoName::of_face([300.0, 0.0, 0.0], [0.0, 0.0, 1.0], 1000.0, [0.0; 3])
    }

    #[test]
    fn resolve_by_name_prefers_exact_match() {
        let old = TopoName::of_face([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], 1000.0, [0.0; 3]);
        // 候选 0 很远，候选 1 与 old 精确相等 → 必须取 1。
        let candidates = vec![(0u32, far_name()), (1u32, old)];
        assert_eq!(resolve_by_name(&old, &candidates), Some(1));
    }

    #[test]
    fn resolve_by_name_falls_back_to_nearest_within_threshold() {
        let old = TopoName::of_face([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], 1000.0, [0.0; 3]);
        // 无精确匹配；候选 0 处 5 mm 内、同向同面积 → 距离远小于阈值。
        let near = TopoName::of_face([5.0, 0.0, 0.0], [0.0, 0.0, 1.0], 1000.0, [0.0; 3]);
        let candidates = vec![(7u32, near)];
        assert_eq!(resolve_by_name(&old, &candidates), Some(7));
    }

    /// **"注定会失败"钉子**：全部候选都超阈值 → 必须 `None`——
    /// 防"随便指一个"式假绿。
    #[test]
    fn resolve_by_name_refuses_when_too_far() {
        let old = TopoName::of_face([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], 1000.0, [0.0; 3]);
        let turned = TopoName::of_face([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], 1000.0, [0.0; 3]);
        let candidates = vec![(0u32, far_name()), (1u32, turned)];
        assert_eq!(
            resolve_by_name(&old, &candidates),
            None,
            "全部超阈值时必须拒绝解析（宁可不解析，也不乱指）"
        );
    }

    /// **歧义钉子（精确匹配）**：两个候选都与旧名精确相等 → 无法唯一确定身份，
    /// 必须 `None`（防"挑第一个"式假绿）。
    #[test]
    fn resolve_by_name_refuses_on_ambiguous_exact_match() {
        let old = TopoName::of_face([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], 1000.0, [0.0; 3]);
        // 两个候选逐位相同，且都与 old 精确匹配 → 歧义。
        let candidates = vec![(3u32, old), (9u32, old)];
        assert_eq!(
            resolve_by_name(&old, &candidates),
            None,
            "多个精确匹配构成歧义，必须拒绝解析"
        );
    }

    /// **歧义钉子（最近并列）**：无精确匹配，两个候选到旧名距离完全相等 →
    /// 无法唯一确定身份，必须 `None`。
    #[test]
    fn resolve_by_name_refuses_on_tied_nearest() {
        let old = TopoName::of_face([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], 1000.0, [0.0; 3]);
        // 沿 +X / +Y 各偏 5 mm（对称）→ 两者距离相等，且都远超 exact_match 容差。
        let along_x = TopoName::of_face([5.0, 0.0, 0.0], [0.0, 0.0, 1.0], 1000.0, [0.0; 3]);
        let along_y = TopoName::of_face([0.0, 5.0, 0.0], [0.0, 0.0, 1.0], 1000.0, [0.0; 3]);
        assert_eq!(
            old.distance(&along_x),
            old.distance(&along_y),
            "两候选到旧名的距离必须严格相等（否则本用例不构成并列）"
        );
        let candidates = vec![(1u32, along_x), (2u32, along_y)];
        assert_eq!(
            resolve_by_name(&old, &candidates),
            None,
            "最近距离并列构成歧义，必须拒绝解析"
        );
    }

    // ---- 真实内核腿：跨会话稳定性硬证明 ----

    /// 建形状 → 算面指纹 → **释放会话**（drop → 重开内核）→ 从 BREP 重解析
    /// → 指纹**完全相等**（M5 规划 §3.5 的硬证明）。
    ///
    /// 真实内核测试**必须先断言内核非 mock**；内核不可用即 fail（"跳过"≠"通过"）。
    ///
    /// debug 腿下本用例被标记为 **ignored**（wasmtime 编译过慢），而非静默 `return`
    /// 造出"假绿"；release 腿正视执行。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn face_fingerprint_survives_session_reload() {
        let mut kernel = Kernel::new()
            .expect("启用 occt feature 后 Kernel::new() 必须成功（occt 腿固定 --release）");
        assert!(
            kernel.is_real(),
            "本测试必须跑真实内核，不许在 mock 上假绿；实得后端 {}",
            kernel.backend_name()
        );

        let plate = kernel.make_box(100.0, 60.0, 10.0).unwrap();
        let hole = kernel.make_cylinder(15.0, 40.0).unwrap();
        let holed = kernel.cut(plate, hole).unwrap();
        let brep = kernel.export_brep(holed).unwrap();

        let shape = ShapeId::new();
        let mut doc = Document::new("带圆孔板");
        doc.insert_geometry(core_model::GeometryRecord::brep(shape, brep))
            .unwrap();

        let first = face_names(&mut kernel, &doc, shape).unwrap();
        assert!(!first.is_empty(), "带圆孔板应至少有一个面");

        // **释放会话**：丢弃当前内核，重开一个全新内核（模拟跨会话 / 懒惰重建）。
        drop(kernel);
        let mut kernel2 = Kernel::new().expect("重开会话的内核必须成功");
        assert!(kernel2.is_real(), "重开的会话也必须是真实内核");

        let second = face_names(&mut kernel2, &doc, shape).unwrap();
        assert_eq!(
            first, second,
            "跨会话（从 BREP 重解析）后各面指纹必须逐位相等；不相等即方案不成立"
        );
    }

    // ---- B6.1b：边 / 顶点名表 + resolve_key 边/顶点分支 ----

    #[cfg(feature = "occt")]
    use core_kernel::SubShapeKind;
    #[cfg(feature = "occt")]
    use core_model::TopoKind;
    #[cfg(feature = "occt")]
    use core_model::ids::{DIR_QUANT_SCALE, LENGTH_STEP_MM, POS_STEP_MM};

    /// 建一个真实内核并断言确为真后端（"跳过"≠"通过"）。
    #[cfg(feature = "occt")]
    fn real_kernel() -> Kernel {
        let k = Kernel::new().expect("启用 occt feature 后 Kernel::new() 必须成功");
        assert!(
            k.is_real(),
            "本测试必须跑真实内核，不许在 mock 上假绿；实得后端 {}",
            k.backend_name()
        );
        k
    }

    /// 活句柄版的"带整圆通孔板"：盒 100×60×10 挖 Φ30 通孔。
    #[cfg(feature = "occt")]
    fn holed_handle(kernel: &mut Kernel) -> core_kernel::ShapeHandle {
        let plate = kernel.make_box(100.0, 60.0, 10.0).unwrap();
        let hole = kernel.make_cylinder(15.0, 40.0).unwrap();
        kernel.cut(plate, hole).unwrap()
    }

    /// 文档版的"带整圆通孔板"（供 `edge_names` / `vertex_names` / `resolve_key` 用）。
    #[cfg(feature = "occt")]
    fn holed_plate(kernel: &mut Kernel) -> (ShapeId, Document) {
        let holed = holed_handle(kernel);
        let brep = kernel.export_brep(holed).unwrap();
        let shape = ShapeId::new();
        let mut doc = Document::new("带圆孔板");
        doc.insert_geometry(core_model::GeometryRecord::brep(shape, brep))
            .unwrap();
        (shape, doc)
    }

    /// 默认腿钉子：mock `sub_shapes(Edge/Vertex)` 返回**空** → 名表为**空 `Ok(vec![])`**
    /// （**不造假条目、不报错**——空集是合法几何事实）。
    #[test]
    fn edge_and_vertex_name_tables_are_empty_on_mock() {
        let mut k = Kernel::mock();
        let h = k.make_box(10.0, 10.0, 10.0).expect("mock 建模应成功");
        assert!(
            edge_names_from_handle(&mut k, h).unwrap().is_empty(),
            "mock 无边拓扑 → 边名表必须为空（且为 Ok）"
        );
        assert!(
            vertex_names_from_handle(&mut k, h).unwrap().is_empty(),
            "mock 无顶点拓扑 → 顶点名表必须为空（且为 Ok）"
        );
    }

    /// 默认腿钉子：无名旧键（旧文档）的 **Edge / Vertex** 分支 → `Ok(None)`，
    /// 且在读文档 / 触内核前即返回（文档里根本没有该形状也不影响）。
    #[test]
    fn resolve_key_refuses_unnamed_edge_and_vertex_keys() {
        let mut k = Kernel::mock();
        let doc = Document::new("空文档");
        let shape = ShapeId::new();

        let ek = GeometryKey::edge(shape, 3);
        assert_eq!(
            resolve_key(&mut k, &doc, shape, &ek).unwrap(),
            None,
            "无名边键无法几何重解析 → 拒绝解析 Ok(None)"
        );
        let vk = GeometryKey::vertex(shape, 7);
        assert_eq!(
            resolve_key(&mut k, &doc, shape, &vk).unwrap(),
            None,
            "无名顶点键无法几何重解析 → 拒绝解析 Ok(None)"
        );
    }

    /// occt 钉子：`edge_names_from_handle` 的序号**按 `sub_shapes(Edge)` 顺序连续**、
    /// 每条都是 `TopoKind::Edge`——防"名表序号 off-by-one / 乱序"。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn edge_names_from_handle_orders_and_counts() {
        let mut k = real_kernel();
        let h = holed_handle(&mut k);

        let edges = k.sub_shapes(h, SubShapeKind::Edge).unwrap();
        let names = edge_names_from_handle(&mut k, h).unwrap();
        assert_eq!(
            names.len(),
            edges.len(),
            "边名表长度须等于 sub_shapes(Edge)"
        );
        assert!(
            names.len() >= 12,
            "带孔板应至少 12 条边，实得 {}",
            names.len()
        );
        for (pos, (idx, name)) in names.iter().enumerate() {
            assert_eq!(
                *idx, pos as u32,
                "边名表序号须按 sub_shapes 顺序连续，无跳号/乱序"
            );
            assert_eq!(name.kind, TopoKind::Edge, "边名种类须为 Edge");
        }
    }

    /// occt 钉子：`edge_name_of` 的三条**硬约束**逐条封死（`occt.rs`/topo 规格 §2.7.13）：
    /// ① 代表点 = **参数域中点**（非最近点 / 非起点）；② 切向**单位化**；③ 长度 = `curve_length`
    /// 的**真弧长**（非 `curve_parameters` 的 `last−first`）。
    ///
    /// 用**角参数化的圆弧孔边**当试金石（实测该板孔边为 1/4 圆弧：弧长 = 半径 × 参数跨度，
    /// `len/span = 半径 15 ≫ 1`；直边则 `len == span`，比值恒 1）。弧边**按几何判据自动识别**
    /// （不硬编码序号 / 角度），故对内核把圆弧拆成几段也鲁棒。
    /// 用量化 `size_q` 反推长度：真弧长与参数跨度量化值截然不同，若误用跨度即错。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn edge_name_of_uses_param_midpoint_arc_length_and_unit_tangent() {
        let mut k = real_kernel();
        let h = holed_handle(&mut k);

        let mesh = k.tessellate(h, TESS_LINEAR_MM, TESS_ANGULAR_RAD).unwrap();
        let bmin = bbox_min(&mesh);
        let edges = k.sub_shapes(h, SubShapeKind::Edge).unwrap();

        let mut found = 0;
        for &e in &edges {
            let len = k.curve_length(e).unwrap();
            let range = k.curve_parameters(e).unwrap();
            let span = range.last - range.first;
            // 角参数化圆弧边：真弧长 = 半径 × 参数跨度 → `len/span = 半径 15`；
            // 直边 `len == span`（比值 1）。据此识别圆弧边，对内核拆分圆弧不敏感。
            if span <= 0.0 || len <= span * 1.5 {
                continue;
            }
            found += 1;
            assert!(
                (len / span - 15.0).abs() < 1e-3,
                "圆弧边 len/span 应≈孔半径 15（len={len}, span={span}）"
            );
            let name = edge_name_of(&mut k, e, bmin).unwrap();

            // ③ 长度 = 真弧长（若写成 last-first 的跨度，量化值会截然不同）。
            let expect_q = (len / LENGTH_STEP_MM).round() as i64;
            let wrong_q = (span / LENGTH_STEP_MM).round() as i64;
            assert_ne!(
                expect_q, wrong_q,
                "（本用例前提）弧长与参数跨度量化值须不同"
            );
            assert_eq!(
                name.size_q, expect_q,
                "弧边长度须用 curve_length 的真弧长（{len}），不得用参数域跨度（{span}）"
            );

            // ② 切向必须单位化：`|dir_q/10000| ≈ 1`（不归一化时 |dP/dparam|=15 → 饱和到 i16 上限）。
            let mag = (f64::from(name.dir_q[0]).powi(2)
                + f64::from(name.dir_q[1]).powi(2)
                + f64::from(name.dir_q[2]).powi(2))
            .sqrt();
            assert!(
                (mag - DIR_QUANT_SCALE).abs() < 500.0,
                "边切向须归一化（|dir_q|≈{DIR_QUANT_SCALE}），实得 {mag}"
            );

            // ① 代表点 = 参数域中点：中点须远离参数起点（四分之一弧的弦长级），
            //    且 name 的 rep 重建须≈中点。
            let mid = (range.first + range.last) / 2.0;
            let p_mid = k.curve_point_at_param(e, mid).unwrap();
            let p_start = k.curve_point_at_param(e, range.first).unwrap();
            assert!(
                (p_mid - p_start).length() > 1.0,
                "圆弧中点须远离参数起点，实得 {}",
                (p_mid - p_start).length()
            );
            let rep = [
                bmin[0] + f64::from(name.pos_q[0]) * POS_STEP_MM,
                bmin[1] + f64::from(name.pos_q[1]) * POS_STEP_MM,
                bmin[2] + f64::from(name.pos_q[2]) * POS_STEP_MM,
            ];
            let rep_err = (DVec3::from_array(rep) - p_mid).length();
            assert!(
                rep_err < POS_STEP_MM,
                "边代表点须是参数域中点（重建误差 {rep_err} 超一个量化格），不得取最近点/起点"
            );
        }
        assert!(
            found >= 1,
            "带通孔板应至少有一条角参数化圆弧孔边（否则本用例空转）"
        );
    }

    /// occt 钉子：`vertex_names_from_handle` 逐顶点序号 → 名字的 rep **确是该顶点坐标**
    /// （用 `bounding_box` 中心当独立 oracle），且数量与 `sub_shapes(Vertex)` 一致。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn vertex_names_from_handle_match_corner_points() {
        let mut k = real_kernel();
        let h = k.make_box(10.0, 10.0, 10.0).unwrap();

        let mesh = k.tessellate(h, TESS_LINEAR_MM, TESS_ANGULAR_RAD).unwrap();
        let bmin = bbox_min(&mesh);
        let verts = k.sub_shapes(h, SubShapeKind::Vertex).unwrap();
        let names = vertex_names_from_handle(&mut k, h).unwrap();
        assert_eq!(
            names.len(),
            verts.len(),
            "顶点名表长度须等于 sub_shapes(Vertex)"
        );
        assert_eq!(names.len(), 8, "立方体应有 8 个顶点，实得 {}", names.len());

        for (i, (idx, name)) in names.iter().enumerate() {
            assert_eq!(*idx, i as u32, "顶点名表序号须按 sub_shapes 顺序连续");
            assert_eq!(name.kind, TopoKind::Vertex, "顶点名种类须为 Vertex");
            // 独立 oracle：该顶点的真实坐标 = 其退化包围盒中心。
            let bb = k.bounding_box(verts[i]).unwrap();
            let coord = [
                (bb.min[0] + bb.max[0]) / 2.0,
                (bb.min[1] + bb.max[1]) / 2.0,
                (bb.min[2] + bb.max[2]) / 2.0,
            ];
            let rep = [
                bmin[0] + f64::from(name.pos_q[0]) * POS_STEP_MM,
                bmin[1] + f64::from(name.pos_q[1]) * POS_STEP_MM,
                bmin[2] + f64::from(name.pos_q[2]) * POS_STEP_MM,
            ];
            let err = (DVec3::from_array(rep) - DVec3::from_array(coord)).length();
            assert!(
                err < POS_STEP_MM,
                "第 {i} 个顶点名 rep {rep:?} 须≈顶点真坐标 {coord:?}（误差 {err}）"
            );
        }
    }

    /// **跨会话硬证明**（边 + 顶点）：会话 1 算出的**带名键** → `drop` 内核、重开会话 →
    /// `resolve_key` 在同一文档上**重解析成功**，且重解析出的序号在**新名表**同序号处
    /// `exact_match` 原稳定名 → 序号指向**同一几何**的子形状。
    ///
    /// 这是 B6.1b 的"跨会话可用性"终验：`curve_point_at_param` / `curve_length` 门面 +
    /// 边/顶点名表 + `resolve_key` 分支 + 装载链路，一整条打通。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn edge_and_vertex_keys_survive_session_reload() {
        let mut k1 = real_kernel();
        let (shape, doc) = holed_plate(&mut k1);

        let edges1 = edge_names(&mut k1, &doc, shape).unwrap();
        assert!(
            edges1.len() >= 12,
            "带孔板应至少 12 条边，实得 {}",
            edges1.len()
        );
        let (ei, en) = edges1[edges1.len() / 2];
        let edge_key = GeometryKey::edge_named(shape, ei, en);

        let verts1 = vertex_names(&mut k1, &doc, shape).unwrap();
        assert!(
            verts1.len() >= 8,
            "带孔板应至少 8 个顶点，实得 {}",
            verts1.len()
        );
        let (vi, vn) = verts1[verts1.len() / 2];
        let vertex_key = GeometryKey::vertex_named(shape, vi, vn);

        // **释放会话**：丢弃内核，重开全新会话。
        drop(k1);
        let mut k2 = real_kernel();

        // ---- 边键跨会话重解析 ----
        let rk = resolve_key(&mut k2, &doc, shape, &edge_key)
            .unwrap()
            .expect("跨会话边键应能重解析（名字由内蕴几何构出，跨会话稳定）");
        assert_eq!(rk.shape, shape, "shape 须不变");
        let SubShapeRef::Edge(r) = rk.sub else {
            panic!("应仍是 Edge 子形状键，实得 {:?}", rk.sub);
        };
        assert_eq!(r.name, Some(en), "重解析后须保留原稳定名");
        let edges2 = edge_names(&mut k2, &doc, shape).unwrap();
        let name_at = edges2
            .iter()
            .find(|(i, _)| *i == r.index)
            .expect("重解析出的序号须存在于新名表");
        assert!(
            name_at.1.exact_match(&en),
            "重解析出的序号 {} 须指向与原名同一几何的边",
            r.index
        );

        // ---- 顶点键跨会话重解析 ----
        let rk = resolve_key(&mut k2, &doc, shape, &vertex_key)
            .unwrap()
            .expect("跨会话顶点键应能重解析");
        let SubShapeRef::Vertex(r) = rk.sub else {
            panic!("应仍是 Vertex 子形状键，实得 {:?}", rk.sub);
        };
        assert_eq!(r.name, Some(vn), "重解析后须保留原稳定名");
        let verts2 = vertex_names(&mut k2, &doc, shape).unwrap();
        let name_at = verts2
            .iter()
            .find(|(i, _)| *i == r.index)
            .expect("重解析出的序号须存在于新名表");
        assert!(
            name_at.1.exact_match(&vn),
            "重解析出的序号 {} 须指向与原名同一几何的顶点",
            r.index
        );
    }

    /// occt 钉子：远离真实几何的**边 / 顶点名** → `resolve_by_name` 超阈值 → `resolve_key`
    /// 拒绝解析 `Ok(None)`（**绝不乱指**）。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn resolve_key_refuses_absent_edge_and_vertex_names() {
        let mut k = real_kernel();
        let (shape, doc) = holed_plate(&mut k);

        let bogus_e = TopoName::of_edge([1.0e6, 1.0e6, 1.0e6], [1.0, 0.0, 0.0], 1.0, [0.0; 3]);
        let key = GeometryKey::edge_named(shape, 0, bogus_e);
        assert!(
            resolve_key(&mut k, &doc, shape, &key).unwrap().is_none(),
            "远离真实几何的边名须拒绝解析"
        );

        let bogus_v = TopoName::of_vertex([1.0e6, 1.0e6, 1.0e6], [0.0; 3]);
        let key = GeometryKey::vertex_named(shape, 0, bogus_v);
        assert!(
            resolve_key(&mut k, &doc, shape, &key).unwrap().is_none(),
            "远离真实几何的顶点名须拒绝解析"
        );
    }
}

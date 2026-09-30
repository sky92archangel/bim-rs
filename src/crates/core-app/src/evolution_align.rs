//! 演化历史 ↔ 持久键的对齐（M5 规划 §2.3(f′) / §3.2 / §3.7）。
//!
//! ## 这是"主路径"与"持久化路径"的缝合点
//!
//! - **主路径**：[`Evolution`] 给的是**会话内面哈希**（与
//!   [`Kernel::face_hashes`] 同域），精确回答"**这次操作**改了哪些面"；
//! - **持久化路径**：[`TopoName`] 是**跨会话稳定名**（由几何指纹构造），
//!   回答"**跨会话哪个键还是那个面**"。
//!
//! 本模块做两件事：① 把输入面哈希映射回绑定在其上的旧键；② 把输出面哈希
//! 映射成新的绑定键。二者结合，才能既精确又持久（M5 §3.2 的"主从配合"）。
//!
//! ## 为什么必须先采集 [`HistoryRecord`]（P0-1）
//!
//! 会话面哈希是**实例级**的：同一几何经 `export_brep` → 重导入 / 文档重装后，
//! [`Kernel::face_hashes`] 会给出**不同的值**。而 [`Evolution`] 的输入 / 输出面
//! 哈希属于"执行该布尔的活句柄实例"域。若等
//! [`update_after_commit`](crate::update_service::update_after_commit) 再拿
//! `ShapeId` 重装形状去算哈希，必然落进**另一个哈希域**、恒不匹配——这正是
//! 演化主路径跨文档恒空的根因。故由 `geometry_service` **在布尔当刻、句柄还活着
//! 时**把每个面哈希的稳定名摘下，随 [`HistoryRecord`] 透传；此后对齐**只走稳定名**。
//!
//! ## `resolve_delta` 的四档退路（M5 §3.6）
//!
//! 1. **有 [`HistoryRecord`]** → 精确直连（输入稳定名 → 输出面稳定名）；
//! 2. 无 [`HistoryRecord`] / 演化覆盖不到 → 几何指纹**最近匹配**（[`resolve_by_name`]）；
//! 3. 指纹也**拒绝解析**（超阈值）→ 该键不进 delta，由调用方**整视图重投**兜底；
//! 4. `ShapeId` 级键恒为最终兜底（`ShapeId` 是持久 UUID）。
//!
//! ## 红线
//!
//! 面哈希**只作会话内对齐**，**绝不进持久键**（M5 附录 A#2/#12）；
//! 面哈希**只经 [`Kernel::face_hashes`] 取得**，禁止自取 `upper_bound`。

use std::collections::{BTreeMap, BTreeSet};

use core_doc::Drawing;
use core_kernel::{Evolution, Kernel};
use core_model::ids::MAX_TOPO_DISTANCE;
use core_model::{Document, GeometryKey, ShapeId, TopoName};

use crate::error::AppResult;
use crate::topo_naming::{face_names, resolve_by_name};

/// 收集图纸里**绑定到某形状**的全部几何键（去重）。
///
/// 只看 `GeometryKey.shape`（不看内部子形状），素材来自每条
/// [`core_doc::Entity2D::source`] 与每个 [`core_doc::Annotation::bound_to`]。
fn bound_keys(drawing: &Drawing, shape: ShapeId) -> BTreeSet<GeometryKey> {
    let mut out = BTreeSet::new();
    for view in drawing.views.values() {
        for entity in &view.entities {
            if let Some(key) = &entity.source
                && key.shape == shape
            {
                out.insert(key.clone());
            }
        }
        for ann in &view.annotations {
            if let Some(key) = &ann.bound_to
                && key.shape == shape
            {
                out.insert(key.clone());
            }
        }
    }
    out
}

/// 一次带历史布尔的**完整记录**：内核级 [`Evolution`] + **布尔当刻从活句柄采集**的
/// 两张"会话面哈希 → 稳定名"表。
///
/// ## 为什么必须由 `geometry_service` 在布尔当刻采集
///
/// 会话面哈希是**实例级**的（同一几何经 export → 重导入 / 文档重装后 `face_hashes`
/// 值会变）。[`Evolution`] 里的输入 / 输出面哈希属于"执行该布尔的活句柄实例"域。等
/// [`update_after_commit`](crate::update_service::update_after_commit) 再拿 `ShapeId`
/// 重装，必然落进另一域、恒不匹配（P0-1）。故在句柄还活着时把每个面哈希的稳定名摘下
/// 随记录透传，此后对齐**只走稳定名**。
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryRecord {
    /// 内核级面级演化（哈希域 = 本次布尔的活句柄实例）。
    pub evolution: Evolution,
    /// 输入侧（两操作数合并）：会话面哈希 → 稳定名。
    pub input_names: BTreeMap<u64, TopoName>,
    /// 输出侧（结果形状）：会话面哈希 → 稳定名。
    pub output_names: BTreeMap<u64, TopoName>,
}

impl HistoryRecord {
    /// 组装一条记录（三部分通常来自 `geometry_service` 在布尔当刻的采集）。
    #[must_use]
    pub fn new(
        evolution: Evolution,
        input_names: BTreeMap<u64, TopoName>,
        output_names: BTreeMap<u64, TopoName>,
    ) -> Self {
        Self {
            evolution,
            input_names,
            output_names,
        }
    }

    /// 稳定名 → 输入侧会话面哈希（精确优先；否则最近匹配且 `< MAX_TOPO_DISTANCE`）。
    #[must_use]
    pub fn input_hash_for_name(&self, name: &TopoName) -> Option<u64> {
        // ① 精确匹配优先。
        for (hash, candidate) in &self.input_names {
            if candidate.exact_match(name) {
                return Some(*hash);
            }
        }

        // ② 最近匹配，且必须落在可信阈值内。
        let mut best: Option<(f64, u64)> = None;
        for (hash, candidate) in &self.input_names {
            let d = name.distance(candidate);
            if d >= MAX_TOPO_DISTANCE {
                continue;
            }
            if best.is_none_or(|(best_d, _)| d < best_d) {
                best = Some((d, *hash));
            }
        }
        best.map(|(_, hash)| hash)
    }

    /// 输出面哈希 → 新键（身份由稳定名承载，序号占位 0）。
    #[must_use]
    pub fn key_for_output(&self, shape: ShapeId, output_hash: u64) -> Option<GeometryKey> {
        let name = *self.output_names.get(&output_hash)?;
        Some(GeometryKey::face_named(shape, 0, name))
    }
}

/// `resolve_delta` 的结果：`旧键 → 新键` + **哪些旧键是经演化解析的**。
///
/// 分离 `via_evolution` 是为了让 `UpdateReport` 能如实区分"演化命中"与"指纹兜底"
/// （修 P1-3：原先只按"该形状有没有历史"计数，会把指纹兜底也算成演化）。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ResolvedDelta {
    /// 旧键 → 新键。
    pub map: BTreeMap<GeometryKey, GeometryKey>,
    /// 其中经【演化】精确解析出的旧键（其余为指纹）。
    pub via_evolution: BTreeSet<GeometryKey>,
}

/// **一次改动的子形状 delta**：`旧键 → 新键`（供 `DependencyGraph::affected_after_resolve`）。
///
/// **优先走 [`HistoryRecord`]（精确）；覆盖不到时回退到几何指纹最近匹配**：
///
/// - 对每个 `shape ∈ changed_shapes`：若 `histories` 里有它的记录 →
///   **演化分支**（按"输入稳定名 → 输出面稳定名"精确对齐；指纹被绕过）；
/// - 否则 → **指纹分支**（`resolve_by_name` 最近匹配）。
///
/// 返回的 [`ResolvedDelta::map`] **只含能确定对齐的键**；无法解析的键不进映射，
/// 由调用方整视图重投兜底。
pub fn resolve_delta(
    kernel: &mut Kernel,
    doc: &Document,
    drawing: &Drawing,
    changed_shapes: &[ShapeId],
    histories: &[(ShapeId, HistoryRecord)],
) -> AppResult<ResolvedDelta> {
    let mut map = BTreeMap::new();
    let mut via_evolution = BTreeSet::new();

    for &shape in changed_shapes {
        match histories.iter().find(|(s, _)| *s == shape) {
            Some((_, rec)) => {
                // 指纹候选仍需（演化覆盖不到的面要兜底）。
                let candidates = face_names(kernel, doc, shape)?;
                for old in bound_keys(drawing, shape) {
                    let Some(name) = old.topo_name().copied() else {
                        continue;
                    };
                    let mut new_key: Option<GeometryKey> = None;
                    if let Some(input_hash) = rec.input_hash_for_name(&name) {
                        for out_hash in rec.evolution.outputs_of(input_hash) {
                            if let Some(k) = rec.key_for_output(shape, out_hash) {
                                new_key = Some(k);
                                break;
                            }
                        }
                    }
                    match new_key {
                        Some(k) => {
                            via_evolution.insert(old.clone());
                            map.insert(old, k);
                        }
                        None => {
                            // 演化覆盖不到（未受影响面 / 名表缺失）→ 指纹兜底。
                            if let Some(index) = resolve_by_name(&name, &candidates) {
                                map.insert(old, GeometryKey::face_named(shape, index, name));
                            }
                        }
                    }
                }
            }
            None => {
                let candidates = face_names(kernel, doc, shape)?;
                for old in bound_keys(drawing, shape) {
                    let Some(name) = old.topo_name().copied() else {
                        continue;
                    };
                    if let Some(index) = resolve_by_name(&name, &candidates) {
                        map.insert(old, GeometryKey::face_named(shape, index, name));
                    }
                }
            }
        }
    }

    Ok(ResolvedDelta { map, via_evolution })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topo_naming::face_names_and_hashes;
    use core_doc::{Entity2D, View};
    use core_kernel::{FaceEvolution, ShapeHandle};
    use core_model::{ElementId, LayerId};
    use glam::DVec2;

    /// 造一个"某形状的面 0 绑了一条图元"的图纸。
    fn drawing_with_bound_key(key: GeometryKey) -> Drawing {
        let mut drawing = Drawing::new();
        let mut view = View::plan("测试视图", ElementId::new(), 0.0);
        view.push_entity(
            Entity2D::line(LayerId::new(), DVec2::ZERO, DVec2::new(1.0, 0.0)).from_geometry(key),
        );
        drawing.add_view(view);
        drawing
    }

    /// 建一个 mock 内核 + 一个盒形状的文档，返回 `(kernel, doc, shape, 候选名, 哈希)`。
    fn mock_box_scene() -> (Kernel, Document, ShapeId, Vec<(u32, TopoName)>, Vec<u64>) {
        let mut kernel = Kernel::mock();
        let handle = kernel.make_box(100.0, 60.0, 10.0).unwrap();
        let brep = kernel.export_brep(handle).unwrap();

        let shape = ShapeId::new();
        let mut doc = Document::new("盒");
        doc.insert_geometry(core_model::GeometryRecord::brep(shape, brep))
            .unwrap();

        let (candidates, hashes) = face_names_and_hashes(&mut kernel, &doc, shape).unwrap();
        (kernel, doc, shape, candidates, hashes)
    }

    /// **演化优先**：构造 [`HistoryRecord`]（面 0 → 面 1）→ delta 走演化分支，
    /// 新键的稳定名落到**输出面**（面 1），而非指纹的"原地不动"。
    #[test]
    fn resolve_delta_prefers_evolution_over_fingerprint() {
        let (mut kernel, doc, shape, candidates, hashes) = mock_box_scene();
        assert!(candidates.len() >= 2, "盒至少有 2 个面");
        let n0 = candidates[0].1;
        let n1 = candidates[1].1;

        let old_key = GeometryKey::face(shape, candidates[0].0).with_topo_name(n0);
        let drawing = drawing_with_bound_key(old_key.clone());

        // 合成演化：面 0 的会话哈希 → 面 1 的会话哈希。
        let evolution = Evolution {
            result: ShapeHandle(0),
            modified: vec![FaceEvolution {
                input_hash: hashes[0],
                outputs: vec![hashes[1]],
            }],
            generated: Vec::new(),
            deleted: Vec::new(),
        };
        let rec = HistoryRecord::new(
            evolution,
            BTreeMap::from([(hashes[0], n0), (hashes[1], n1)]),
            BTreeMap::from([(hashes[1], n1)]),
        );

        let delta = resolve_delta(&mut kernel, &doc, &drawing, &[shape], &[(shape, rec)]).unwrap();

        let new_key = delta
            .map
            .get(&old_key)
            .expect("演化分支必须产出 面0 → 面1 的 delta");
        assert_eq!(
            new_key.topo_name(),
            Some(&n1),
            "演化分支的新键稳定名必须落到**输出面**（面 1）"
        );
        assert_ne!(
            new_key, &old_key,
            "演化分支必须改绑到输出面，而非指纹的原地不动"
        );
        assert!(
            delta.via_evolution.contains(&old_key),
            "该旧键必须被标记为经【演化】解析（而非指纹）"
        );
    }

    /// **指纹退避钉子**：`histories` 空 → 必须走指纹分支并**产出映射**，
    /// 且**不得**被误记为演化（防"没历史就静默不动"式假绿，也防 P1-3 式误计）。
    #[test]
    fn resolve_delta_falls_back_to_fingerprint_when_no_evolution() {
        let (mut kernel, doc, shape, candidates, hashes) = mock_box_scene();
        let _ = hashes;
        let n0 = candidates[0].1;

        let old_key = GeometryKey::face(shape, candidates[0].0).with_topo_name(n0);
        let drawing = drawing_with_bound_key(old_key.clone());

        let delta = resolve_delta(&mut kernel, &doc, &drawing, &[shape], &[]).unwrap();

        assert!(
            !delta.map.is_empty(),
            "无演化历史时必须退回指纹分支并产出映射（绝不静默不动）"
        );
        assert_eq!(
            delta.map.get(&old_key),
            Some(&GeometryKey::face_named(shape, candidates[0].0, n0)),
            "指纹分支应把旧键最近匹配回它自己（精确匹配）"
        );
        assert!(
            delta.via_evolution.is_empty(),
            "无历史时不得有任何键被标记为演化解析（指纹 ≠ 演化）"
        );
    }
}

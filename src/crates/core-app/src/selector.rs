//! 子形状选择器（设计文档 §2.6）：把"用户意图"解析成**当前形状的子形状句柄列表**喂内核。
//!
//! ## 链路
//!
//! ```text
//! EdgeSelector / FaceSelector ──resolve(&mut Kernel, shape)──▶ KernelResult<Vec<ShapeHandle>>
//! ```
//!
//! ## B6.1b 起：`ByTopoName` 已落地
//!
//! [`ByTopoName`](FaceSelector::ByTopoName) 由 B6.1b 接通：解析路径为
//! **活句柄 → 当前名表（[`face_names_from_handle`] / [`edge_names_from_handle`]）→
//! [`resolve_by_name`] → 子形状序号 → [`Kernel::sub_shapes`] 取句柄**。
//! 未命中（名字不存在 / 超出容差 / 歧义）**一律** [`KernelError::Geometry`]，
//! **绝不**返回空列表（"响亮拒绝"精神不变，只是拒绝的理由从"未实现"变为"无匹配"）。
//! [`ByIndex`](FaceSelector::ByIndex) 与 [`NearPoint`](FaceSelector::NearPoint) 语义不变。
//!
//! ## 语义要点
//!
//! - `NearPoint` 用 `project_point_on_face` / `project_point_on_edge` 的**最近点投影距离**，
//!   取**最小**者返回单元素 `Vec`。⚠ 两者均为**距离最近**语义、**非射线命中**：
//!   面投影是**无界平面距离**（查询点须确实最靠近目标面，否则可能被相邻面的无限延展
//!   平面"抢走"）；边投影对**形状内 / 边界**点把点**钳到边上最近点**（参数亦钳进
//!   `[first,last]`），但对**形状外远点则可能 `Err`**（实测：盒外对角点 0/12 成功），
//!   **并非对任意查询点恒成功**。
//! - 越界 / 空集 / 全部投影失败**一律 `Err`**（[`KernelError::Geometry`]），**绝不**返回空列表。

use core_kernel::{Kernel, KernelError, KernelResult, ShapeHandle, SubShapeKind};
use core_model::TopoName;
use glam::DVec3;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::topo_naming::{edge_names_from_handle, face_names_from_handle, resolve_by_name};

/// 选择器算子名（进 [`KernelError::Geometry::op`]）。
const OP: &str = "selector";

/// **活句柄名表计算**函数签名：`face_names_from_handle` / `edge_names_from_handle` 共用
/// （两者都返回 `AppResult<Vec<(序号, TopoName)>>`）。
type NameTableFn = fn(&mut Kernel, ShapeHandle) -> AppResult<Vec<(u32, TopoName)>>;

/// **边**选择器：解析为当前形状的一条边句柄。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EdgeSelector {
    /// 按**跨会话稳定拓扑名**选择（B6.1b 已落地）。
    ///
    /// 解析路径：**活句柄 → 当前边名表（[`edge_names_from_handle`]）→
    /// [`resolve_by_name`] → 边序号 → [`Kernel::sub_shapes`]`(_, Edge)` 取句柄**。
    /// 未命中（名字不存在 / 超出容差 / 歧义）→ [`KernelError::Geometry`]。
    ByTopoName(TopoName),
    /// 按内核 `sub_shapes(_, Edge)` 的**序号**选择。
    ByIndex(u32),
    /// 选择到给定点**投影距离最近**的边。
    NearPoint(DVec3),
}

/// **面**选择器：解析为当前形状的一个面句柄。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FaceSelector {
    /// 按**跨会话稳定拓扑名**选择（B6.1b 已落地）。
    ///
    /// 解析路径：**活句柄 → 当前面名表（[`face_names_from_handle`]）→
    /// [`resolve_by_name`] → 面序号 → [`Kernel::sub_shapes`]`(_, Face)` 取句柄**。
    /// 未命中（名字不存在 / 超出容差 / 歧义）→ [`KernelError::Geometry`]。
    ByTopoName(TopoName),
    /// 按内核 `sub_shapes(_, Face)` 的**序号**选择（与 R16 同序一致）。
    ByIndex(u32),
    /// 选择到给定点**投影距离最近**的面。
    NearPoint(DVec3),
}

impl EdgeSelector {
    /// 解析为当前形状的边句柄列表（**恒为 1 元素**或 `Err`，绝不返回空列表）。
    ///
    /// - [`ByTopoName(name)`](EdgeSelector::ByTopoName) → 活句柄边名表 + [`resolve_by_name`]
    ///   解析出序号 → `sub_shapes(_, Edge)[i]`；未命中 → `Err(Geometry)`。
    /// - [`ByIndex(i)`](EdgeSelector::ByIndex) → `sub_shapes(_, Edge)[i]`；越界 → `Err(Geometry)`。
    /// - [`NearPoint(p)`](EdgeSelector::NearPoint) → 投影距离最小的边；空集 / 全失败 → `Err(Geometry)`。
    pub fn resolve(
        &self,
        kernel: &mut Kernel,
        shape: ShapeHandle,
    ) -> KernelResult<Vec<ShapeHandle>> {
        match self {
            EdgeSelector::ByTopoName(name) => by_topo_name(
                kernel,
                shape,
                SubShapeKind::Edge,
                name,
                edge_names_from_handle,
            ),
            EdgeSelector::ByIndex(i) => by_index(kernel, shape, SubShapeKind::Edge, *i),
            EdgeSelector::NearPoint(p) => near_point_edge(kernel, shape, *p),
        }
    }
}

impl FaceSelector {
    /// 解析为当前形状的面句柄列表（**恒为 1 元素**或 `Err`，绝不返回空列表）。
    ///
    /// - [`ByTopoName(name)`](FaceSelector::ByTopoName) → 活句柄面名表 + [`resolve_by_name`]
    ///   解析出序号 → `sub_shapes(_, Face)[i]`；未命中 → `Err(Geometry)`。
    /// - [`ByIndex(i)`](FaceSelector::ByIndex) → `sub_shapes(_, Face)[i]`；越界 → `Err(Geometry)`。
    /// - [`NearPoint(p)`](FaceSelector::NearPoint) → 投影距离最小的面；空集 / 全失败 → `Err(Geometry)`。
    pub fn resolve(
        &self,
        kernel: &mut Kernel,
        shape: ShapeHandle,
    ) -> KernelResult<Vec<ShapeHandle>> {
        match self {
            FaceSelector::ByTopoName(name) => by_topo_name(
                kernel,
                shape,
                SubShapeKind::Face,
                name,
                face_names_from_handle,
            ),
            FaceSelector::ByIndex(i) => by_index(kernel, shape, SubShapeKind::Face, *i),
            FaceSelector::NearPoint(p) => near_point_face(kernel, shape, *p),
        }
    }
}

// ---- 编解码 helper（M5-B6.2b）：选择器 ↔ `Value::Text` 的**单点收敛** -------
//
// 设计问题①（规格 §2.7.16.1）的落地：`Value` 无列表类型，且选择器类型住在 `core-app`
// （不能下移到 `core-model` / `core-sdk` 而破坏单向依赖），故 `BuildStep` 里以
// **`Value::Text` 装一段 JSON 数组**表达"若干选择器"。线格式 = 选择器 serde
// **外部标签**表示：`{"ByIndex":3}` / `{"NearPoint":[x,y,z]}` / `{"ByTopoName":{…}}`。
//
// **硬约束**：内部调用方与测试**禁止手搓 JSON**——一律走这四个 helper（单点收敛于
// 选择器类型的归属层），否则 schema 一变就有多处漂移。
//
// **响亮失败**：`parse_*` 失败一律 [`AppError::RecipeNotExecutable`]，**绝不静默空表**
// ——空 `edge_ids` 会让 `fillet` / `chamfer` 静默无效（B6.1b 已封的假绿）。

/// 把一组边选择器编码成 **JSON 文本**（进 `BuildStep::with_text("edge_selectors", …)`）。
///
/// **不 panic**：`EdgeSelector` 只含 serde 可序列化字段，正常不会失败；万一失败
/// （类型被改坏）→ 响亮 [`AppError::RecipeNotExecutable`]，**绝不**回退成 `"[]"`
/// （空表会让 `fillet` 静默无效）。
pub fn encode_edge_selectors(selectors: &[EdgeSelector]) -> AppResult<String> {
    serde_json::to_string(selectors).map_err(|err| {
        AppError::RecipeNotExecutable(format!(
            "edge_selectors 编码失败（选择器含不可序列化内容）：{err}"
        ))
    })
}

/// 解析 [`encode_edge_selectors`] 产出的 JSON 文本；**失败响亮 `Err`**（绝不静默空表）。
pub fn parse_edge_selectors(text: &str) -> AppResult<Vec<EdgeSelector>> {
    serde_json::from_str(text).map_err(|err| {
        AppError::RecipeNotExecutable(format!(
            "edge_selectors 解析失败（非法 JSON / 选择器 schema 不符）：{err}"
        ))
    })
}

/// 把一组面选择器编码成 **JSON 文本**（进 `BuildStep::with_text("face_selectors", …)`）。
///
/// **不 panic**：同 [`encode_edge_selectors`]——失败响亮 `Err`，绝不回退空表。
pub fn encode_face_selectors(selectors: &[FaceSelector]) -> AppResult<String> {
    serde_json::to_string(selectors).map_err(|err| {
        AppError::RecipeNotExecutable(format!(
            "face_selectors 编码失败（选择器含不可序列化内容）：{err}"
        ))
    })
}

/// 解析 [`encode_face_selectors`] 产出的 JSON 文本；**失败响亮 `Err`**（绝不静默空表）。
pub fn parse_face_selectors(text: &str) -> AppResult<Vec<FaceSelector>> {
    serde_json::from_str(text).map_err(|err| {
        AppError::RecipeNotExecutable(format!(
            "face_selectors 解析失败（非法 JSON / 选择器 schema 不符）：{err}"
        ))
    })
}

/// **释放 `sub_shapes` 枚举出、但未被选中的句柄**（防 arena 泄漏）。
///
/// `sub_shapes` 返回的是**新会话句柄**，只有被选中的那一条会被调用方带走；其余若不在此
/// 释放，就永久滞留在 arena（`on_single_solid` 只看得见"选中的"那条，看不见被丢弃的）。
fn release_others(kernel: &mut Kernel, subs: &[ShapeHandle], keep: ShapeHandle) {
    let rest: Vec<ShapeHandle> = subs.iter().copied().filter(|h| *h != keep).collect();
    kernel.release_many(&rest);
}

/// `ByIndex` 公共实现：枚举指定种类子形状 → 取第 `i` 个。
///
/// 越界（**含后端返回空集的"整体空"情形**）→ [`KernelError::Geometry`]，绝不静默返回空列表。
/// **被丢弃的其余子形状句柄一律释放**（`sub_shapes` 分配的会话句柄）。
fn by_index(
    kernel: &mut Kernel,
    shape: ShapeHandle,
    kind: SubShapeKind,
    i: u32,
) -> KernelResult<Vec<ShapeHandle>> {
    let subs = kernel.sub_shapes(shape, kind)?;
    match subs.get(i as usize).copied() {
        Some(h) => {
            release_others(kernel, &subs, h);
            Ok(vec![h])
        }
        None => {
            kernel.release_many(&subs);
            Err(KernelError::Geometry {
                op: OP,
                msg: format!("ByIndex({i}) 越界：共 {} 个", subs.len()),
            })
        }
    }
}

/// `ByTopoName` 公共实现：**活句柄名表 → 名称匹配 → 序号 → 子形状句柄**。
///
/// # 借用顺序（**硬约束**）
///
/// `table_fn` 需要 `&mut Kernel`，而 `sub_shapes` 也要 `&mut Kernel`——故必须**先让
/// 名表落成拥有所有权的 `Vec`**（`table_fn` 返回即释放其借用），**再**借 kernel 取
/// `sub_shapes`。绝不可以在名表借用未结束时调 `sub_shapes`（借用冲突）。
///
/// # 错误口径（与 [`by_index`] 的"绝不静默返回空列表"一致）
///
/// - `table_fn` 失败（`tessellate` / 边指纹计算失败等）→ 还原为 [`KernelError`]（若原错
///   已是内核错则原样透传，否则包成 [`KernelError::Geometry`]，`op` 恒为 [`OP`]）；
/// - `resolve_by_name` 返回 `None`（名字不存在 / 超容差 / 歧义）→ [`KernelError::Geometry`]；
/// - 解析出的序号理论上必在名表内，但 `sub_shapes` 是**另一趟**枚举——若其长度意外更短
///   （后端不一致），取不到即 [`KernelError::Geometry`]（**绝不**返回空列表）。
fn by_topo_name(
    kernel: &mut Kernel,
    shape: ShapeHandle,
    kind: SubShapeKind,
    name: &TopoName,
    table_fn: NameTableFn,
) -> KernelResult<Vec<ShapeHandle>> {
    let table = table_fn(kernel, shape).map_err(flatten_app_error)?;
    let Some(index) = resolve_by_name(name, &table) else {
        return Err(KernelError::Geometry {
            op: OP,
            msg: format!(
                "ByTopoName：无匹配子形状（名字不存在 / 超出容差 / 歧义），候选 {} 个",
                table.len()
            ),
        });
    };
    let subs = kernel.sub_shapes(shape, kind)?;
    match subs.get(index as usize).copied() {
        Some(h) => {
            release_others(kernel, &subs, h);
            Ok(vec![h])
        }
        None => {
            kernel.release_many(&subs);
            Err(KernelError::Geometry {
                op: OP,
                msg: format!(
                    "ByTopoName：解析出序号 {index} 越界（sub_shapes 共 {} 个）",
                    subs.len()
                ),
            })
        }
    }
}

/// 把 [`AppError`] 还原成 [`KernelError`]：内核错原样透传，其余包成
/// [`KernelError::Geometry`]（`op` 恒为 [`OP`]）——名表计算未预期失败须**响亮报错**，
/// 绝不静默吞掉。
fn flatten_app_error(err: AppError) -> KernelError {
    match err {
        AppError::Kernel(e) => e,
        other => KernelError::Geometry {
            op: OP,
            msg: format!("活句柄名表计算失败：{other}"),
        },
    }
}

/// `NearPoint`（面）公共实现：枚举面 → 取到 `p` 投影距离**最小**者。
fn near_point_face(
    kernel: &mut Kernel,
    shape: ShapeHandle,
    p: DVec3,
) -> KernelResult<Vec<ShapeHandle>> {
    let subs = kernel.sub_shapes(shape, SubShapeKind::Face)?;
    if subs.is_empty() {
        return Err(KernelError::Geometry {
            op: OP,
            msg: "NearPoint：形状没有可选的面子形状".to_string(),
        });
    }
    let mut best: Option<(f64, ShapeHandle)> = None;
    for f in &subs {
        // 该面投影失败（未命中）→ **跳过**，不当作候选。
        let Ok(pof) = kernel.project_point_on_face(*f, p) else {
            continue;
        };
        let closer = match best {
            Some((d, _)) => pof.distance < d,
            None => true,
        };
        if closer {
            best = Some((pof.distance, *f));
        }
    }
    match best {
        Some((_, h)) => {
            release_others(kernel, &subs, h);
            Ok(vec![h])
        }
        None => {
            kernel.release_many(&subs);
            Err(KernelError::Geometry {
                op: OP,
                msg: "NearPoint：所有面的投影均失败，无法确定最近面".to_string(),
            })
        }
    }
}

/// `NearPoint`（边）公共实现：枚举边 → 取到 `p` **最近点距离最小**者。
fn near_point_edge(
    kernel: &mut Kernel,
    shape: ShapeHandle,
    p: DVec3,
) -> KernelResult<Vec<ShapeHandle>> {
    let subs = kernel.sub_shapes(shape, SubShapeKind::Edge)?;
    if subs.is_empty() {
        return Err(KernelError::Geometry {
            op: OP,
            msg: "NearPoint：形状没有可选的边子形状".to_string(),
        });
    }
    let mut best: Option<(f64, ShapeHandle)> = None;
    for e in &subs {
        // 该边投影失败（未命中）→ **跳过**，不当作候选。
        let Ok(poe) = kernel.project_point_on_edge(*e, p) else {
            continue;
        };
        let d = poe.point.distance(p);
        let closer = match best {
            Some((bd, _)) => d < bd,
            None => true,
        };
        if closer {
            best = Some((d, *e));
        }
    }
    match best {
        Some((_, h)) => {
            release_others(kernel, &subs, h);
            Ok(vec![h])
        }
        None => {
            kernel.release_many(&subs);
            Err(KernelError::Geometry {
                op: OP,
                msg: "NearPoint：所有边的投影均失败，无法确定最近边".to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "occt")]
    use crate::geometry_service::GeometryService;

    /// mock 内核 + 一个立方体句柄。
    fn mock_box() -> (Kernel, ShapeHandle) {
        let mut k = Kernel::mock();
        let h = k.make_box(10.0, 10.0, 10.0).expect("mock 建模应成功");
        (k, h)
    }

    /// 默认腿探针用的 `TopoName`（mock 无面 / 边拓扑可解析 → 无论值如何都应 `Err`）。
    fn sample_toponame() -> TopoName {
        TopoName::of_face([0.0, 0.0, 10.0], [0.0, 0.0, 1.0], 100.0, [0.0, 0.0, 0.0])
    }

    /// 默认腿钉子 ①：mock 下 `ByTopoName` **响亮拒绝**（mock `sub_shapes(Face/Edge)`
    /// 返回空 → 名表为空 / 命中后取句柄越界 → `Err(Geometry)`），**绝不**假 `Ok`（含空 Vec）。
    ///
    /// B6.1b 已把 `ByTopoName` 从"未实现拒绝"改为"真实解析"：故本钉的期望错误从
    /// [`KernelError::Unsupported`] 改为 [`KernelError::Geometry`]（mock 无名表 / 无子形状可解析）。
    #[test]
    fn selector_by_top_name_is_refused_loudly() {
        let (mut k, h) = mock_box();
        let tn = sample_toponame();

        let got = EdgeSelector::ByTopoName(tn).resolve(&mut k, h);
        assert!(
            got.is_err(),
            "mock 下 Edge ByTopoName 绝不返回 Ok（含空 Vec），实得 {got:?}"
        );
        match got {
            Err(KernelError::Geometry { op, .. }) => assert_eq!(op, "selector"),
            other => panic!("mock 下 Edge ByTopoName 应 Err(Geometry)，实得 {other:?}"),
        }

        let got = FaceSelector::ByTopoName(tn).resolve(&mut k, h);
        assert!(
            got.is_err(),
            "mock 下 Face ByTopoName 绝不返回 Ok（含空 Vec），实得 {got:?}"
        );
        match got {
            Err(KernelError::Geometry { op, .. }) => assert_eq!(op, "selector"),
            other => panic!("mock 下 Face ByTopoName 应 Err(Geometry)，实得 {other:?}"),
        }
    }

    /// 默认腿钉子 ②：mock 下 `NearPoint` **响亮拒绝**（mock `sub_shapes(Face/Edge)` 返回空
    /// → 走"空子形状"守卫 → `Err(Geometry)`），**绝不**返回空列表 / 假句柄。
    #[test]
    fn selector_near_point_is_refused_loudly_on_mock() {
        let (mut k, h) = mock_box();
        let p = DVec3::new(5.0, 5.0, 5.0);

        let got = FaceSelector::NearPoint(p).resolve(&mut k, h);
        assert!(
            got.is_err(),
            "mock 下 Face NearPoint 绝不返回 Ok（含空 Vec），实得 {got:?}"
        );
        match got {
            Err(KernelError::Geometry { op, ref msg }) => {
                assert_eq!(op, "selector", "算子名应为 selector");
                assert!(
                    msg.contains("没有可选"),
                    "应落在『空子形状』守卫（mock 无面拓扑），实得 msg={msg}"
                );
            }
            other => panic!("mock 下 Face NearPoint 应 Err(Geometry)，实得 {other:?}"),
        }

        let got = EdgeSelector::NearPoint(p).resolve(&mut k, h);
        assert!(
            got.is_err(),
            "mock 下 Edge NearPoint 绝不返回 Ok（含空 Vec），实得 {got:?}"
        );
        match got {
            Err(KernelError::Geometry { op, ref msg }) => {
                assert_eq!(op, "selector", "算子名应为 selector");
                assert!(
                    msg.contains("没有可选"),
                    "应落在『空子形状』守卫（mock 无边拓扑），实得 msg={msg}"
                );
            }
            other => panic!("mock 下 Edge NearPoint 应 Err(Geometry)，实得 {other:?}"),
        }
    }

    /// 默认腿钉子 ③：**mock `sub_shapes(Face/Edge)` 实测返回空** → `ByIndex`（含 0 号）
    /// 越界 → `Err(Geometry)`；**绝不返回空列表**。大索引（越界）同样 `Err`。
    ///
    /// （若将来 mock 改为返回非空，本钉会立刻红——届时须据实改断言为"解析出对应序号句柄"。）
    #[test]
    fn selector_by_index_is_refused_or_resolves_consistently_on_mock() {
        let (mut k, h) = mock_box();

        for i in [0u32, 1, 999] {
            let got = FaceSelector::ByIndex(i).resolve(&mut k, h);
            assert!(
                got.is_err(),
                "mock 下 Face ByIndex({i}) 绝不返回 Ok（含空 Vec），实得 {got:?}"
            );
            match got {
                Err(KernelError::Geometry { op, .. }) => assert_eq!(op, "selector"),
                other => panic!("mock 下 Face ByIndex({i}) 应 Err(Geometry)，实得 {other:?}"),
            }
        }

        for i in [0u32, 999] {
            let got = EdgeSelector::ByIndex(i).resolve(&mut k, h);
            assert!(
                got.is_err(),
                "mock 下 Edge ByIndex({i}) 绝不返回 Ok（含空 Vec），实得 {got:?}"
            );
            match got {
                Err(KernelError::Geometry { op, .. }) => assert_eq!(op, "selector"),
                other => panic!("mock 下 Edge ByIndex({i}) 应 Err(Geometry)，实得 {other:?}"),
            }
        }
    }

    /// occt 腿钉子：真内核下 `ByIndex` / `NearPoint` **确实解析出正确的面**。
    ///
    /// 造立方体（多个面）：`ByIndex(i)` 解析出的面须与 `sub_shapes(Face)` 第 i 面
    /// **几何同一**（用"固定种子点的面投影点 + 单位法向"作**几何签名**比对，**不依赖
    /// 句柄数值**——两次 `sub_shapes` 返回不同会话句柄）。越界 `Err` 由默认腿钉子 ③ 独占守卫。
    /// `NearPoint` 对**多个互不相同**的查询点（含精确命中与**沿目标面法向偏移的非精确命中**）→
    /// 须解析出**各自对应的**面（距离最小者，且以独立 oracle 自证 argmin），返回值**随查询点
    /// 变化**；从而"恒取首个 / 末个"、"忽略距离只按序号 / 位置返回"、"只搜固定序号集"、
    /// "只认精确命中"等退化变异必红。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn selector_resolves_real_sub_shapes_by_index_and_near_point() {
        let mut g = GeometryService::new().expect("真实内核应可用");
        assert_eq!(
            g.backend_kind(),
            core_kernel::BackendKind::Real,
            "钉子必须跑在真实内核上，不许在 mock 上假绿"
        );
        let h = g
            .kernel_mut()
            .make_box(10.0, 10.0, 10.0)
            .expect("真实内核建模应成功");

        // 几何签名：把固定种子点投影到面 → (最近点, 单位法向)，唯一标识一个面。
        let seed = DVec3::new(5.0, 5.0, 5.0);
        let sig = |g: &mut GeometryService, f: ShapeHandle| -> (DVec3, DVec3) {
            let pof = g
                .kernel_mut()
                .project_point_on_face(f, seed)
                .expect("面投影应成功");
            let n = g
                .kernel_mut()
                .surface_normal(f, pof.u, pof.v)
                .expect("面法向应成功");
            (pof.point, n)
        };
        let close = |a: DVec3, b: DVec3| (a - b).length() < 1e-6;

        let faces = g
            .kernel_mut()
            .sub_shapes(h, SubShapeKind::Face)
            .expect("枚举面应成功");
        assert!(faces.len() >= 2, "立方体应有多个面，实得 {}", faces.len());
        let golden: Vec<(DVec3, DVec3)> = faces.iter().map(|f| sig(&mut g, *f)).collect();

        // ---- ByIndex(i) 须几何同一于 golden[i] ----
        for (i, gld) in golden.iter().enumerate() {
            let resolved = FaceSelector::ByIndex(i as u32)
                .resolve(g.kernel_mut(), h)
                .expect("ByIndex 应解析成功");
            assert_eq!(resolved.len(), 1, "ByIndex 应返回恰 1 个面");
            let s = sig(&mut g, resolved[0]);
            assert!(
                close(s.0, gld.0) && close(s.1, gld.1),
                "ByIndex({i}) 解析的面应与第 {i} 面几何同一：got {:?} vs golden {:?}",
                s,
                gld
            );
        }

        // ---- 越界 ByIndex 的 Err 保障由**默认腿钉子 ③**独占（与后端无关的包装层逻辑），
        //      本 occt 钉只验证真内核下**在范围内**的解析正确性，避免两钉重复守卫同一条分支。

        // ---- NearPoint 须取**距离最小**面（一般语义，非单点、非精确命中）----
        // 三个查询点：(1, len-2) 为**精确命中**（距离 0）；序号 3 沿目标面法向**偏移 1e-3**
        // （距离 >0，但仍是全局距离最小者）。断言返回**各自对应**的面，且返回值**随查询点变化**；
        // 从而"恒取首个 / 恒取末个"、"忽略距离只按序号 / 位置返回"（逃逸 A：只搜 {1,len-2}）、
        // "只认精确命中 dist<1e-9"（逃逸 B）等退化变异必红。
        let specs: [(usize, bool); 3] = [(1, false), (faces.len() - 2, false), (3, true)];
        for &(t, _) in &specs {
            assert!(
                t != 0 && t != faces.len() - 1,
                "查询目标序号 {t} 不得为首 / 末（faces.len()={}）",
                faces.len()
            );
        }
        assert!(
            specs[0].0 != specs[1].0 && specs[1].0 != specs[2].0 && specs[0].0 != specs[2].0,
            "三个查询点的期望序号须两两不同（实得 {:?}），否则无法证『随查询点变化』",
            specs.map(|s| s.0)
        );
        let mut seen_idx: Vec<usize> = Vec::new();
        for &(t, offset) in &specs {
            // 目标面上一点（恰落在面上）：种子点到第 t 面的最近点。
            let pof = g
                .kernel_mut()
                .project_point_on_face(faces[t], seed)
                .expect("面投影应成功");
            let q = if offset {
                // 沿目标面法向偏移 1e-3：离开该面 → 距离 >0，但仍是全局距离最小者。
                let n = g
                    .kernel_mut()
                    .surface_normal(faces[t], pof.u, pof.v)
                    .expect("面法向应成功");
                pof.point + n * 1e-3
            } else {
                pof.point
            };
            // 独立 oracle：对全部面独立求 q 的投影距离，自证第 t 面确为**严格全局最近**
            // （防"偏移量过大导致目标并非 argmin"造成的假红 / 假绿）。
            let d_t = g
                .kernel_mut()
                .project_point_on_face(faces[t], q)
                .expect("目标面投影必须成功")
                .distance;
            for (j, &f) in faces.iter().enumerate() {
                if j == t {
                    continue;
                }
                if let Ok(p) = g.kernel_mut().project_point_on_face(f, q) {
                    assert!(
                        d_t < p.distance,
                        "oracle 自证失败：第 {j} 面距 {} 不大于目标第 {t} 面距 {d_t}（偏移量过大？）",
                        p.distance
                    );
                }
            }
            if offset {
                assert!(
                    d_t > 1e-6,
                    "偏移查询点应距目标第 {t} 面 >1e-6（非精确命中，封死『只认精确命中』逃逸），实得 {d_t}"
                );
            }
            // 选择器结果须几何同一于第 t 面。
            let resolved = FaceSelector::NearPoint(q)
                .resolve(g.kernel_mut(), h)
                .expect("NearPoint 应解析成功");
            assert_eq!(resolved.len(), 1, "NearPoint 应返回恰 1 个面");
            let s = sig(&mut g, resolved[0]);
            assert!(
                close(s.0, golden[t].0) && close(s.1, golden[t].1),
                "q 最近的应是第 {t} 面，实得 {:?} vs golden {:?}",
                s,
                golden[t]
            );
            let idx = golden
                .iter()
                .position(|gld| close(s.0, gld.0) && close(s.1, gld.1))
                .expect("解析出的面签名必在 golden 中");
            seen_idx.push(idx);
        }
        // 退化守卫：若实现忽略距离只按固定序号 / 位置返回，多个不同 q 会返回同一子形状。
        for (a, &ia) in seen_idx.iter().enumerate() {
            for (b, &ib) in seen_idx.iter().enumerate().skip(a + 1) {
                assert_ne!(
                    ia, ib,
                    "第 {a} 与第 {b} 个查询点返回同一面——NearPoint 退化（未按距离取最小者）"
                );
            }
        }
    }

    /// 盒 `[0,10]³` 的 **12 条边中点**（解析给出，与内核无关）。
    #[cfg(feature = "occt")]
    fn box_edge_mids() -> [DVec3; 12] {
        [
            DVec3::new(5.0, 0.0, 0.0),
            DVec3::new(5.0, 10.0, 0.0),
            DVec3::new(0.0, 5.0, 0.0),
            DVec3::new(10.0, 5.0, 0.0),
            DVec3::new(5.0, 0.0, 10.0),
            DVec3::new(5.0, 10.0, 10.0),
            DVec3::new(0.0, 5.0, 10.0),
            DVec3::new(10.0, 5.0, 10.0),
            DVec3::new(0.0, 0.0, 5.0),
            DVec3::new(10.0, 0.0, 5.0),
            DVec3::new(10.0, 10.0, 5.0),
            DVec3::new(0.0, 10.0, 5.0),
        ]
    }

    /// 边的**几何签名** `(命中点, 单位切向, 参数)`：用盒 12 条边中点逐一作查询点，
    /// **只接受真正落在该边上的命中**（`命中点 ≈ 查询点`，容差 1e-6）。
    ///
    /// ⚠ 实测（已用临时探针核实，盒 10³）：`project_point_on_edge` 对**形状内 / 边界**
    /// 查询点成功并把点钳到边上最近点（参数亦钳进 `[first,last]`）；但对**形状外远点**
    /// 则可能**全部或部分 `Err`**（如盒外对角点 (10.3,10.3,10.3) 0/12 成功），
    /// **并非对任意查询点恒成功**。故必须靠"命中点≈查询点"筛出真正的边内点，否则会
    /// 拿到别的边的钳制投影点。
    #[cfg(feature = "occt")]
    fn edge_sig(kernel: &mut Kernel, e: ShapeHandle) -> Option<(DVec3, DVec3, f64)> {
        for m in box_edge_mids() {
            if let Ok(poe) = kernel.project_point_on_edge(e, m)
                && (poe.point - m).length() < 1e-6
            {
                return Some((poe.point, poe.tangent, poe.param));
            }
        }
        None
    }

    /// 两个边签名是否同一（逐分量容差）。
    #[cfg(feature = "occt")]
    fn sig_eq(a: (DVec3, DVec3, f64), b: (DVec3, DVec3, f64)) -> bool {
        (a.0 - b.0).length() < 1e-6 && (a.1 - b.1).length() < 1e-6 && (a.2 - b.2).abs() < 1e-6
    }

    /// occt 腿钉子②：真内核下 `EdgeSelector` 的 `ByIndex` / `NearPoint` **确实解析出正确的边**。
    ///
    /// 造立方体（12 条边）：`ByIndex(i)` 解析出的边须与 `sub_shapes(Edge)` 第 i 条边
    /// **几何同一**（用"边中点 + 单位切向 + 参数"的**几何签名**比对，**不依赖句柄数值**——
    /// 两次 `sub_shapes` 返回不同会话句柄；签名还须**两两不同**以证边身份可区分）。
    /// `NearPoint` 对**多个互不相同**的查询点（含精确命中与**沿垂直于边切向偏移的非精确命中**）→
    /// 须解析出**各自对应的**边（距离最小者，且以独立 oracle 自证 argmin），返回值**随查询点
    /// 变化**；从而"恒取首个 / 末个"、"忽略距离只按序号 / 位置返回"、"只搜固定序号集"、
    /// "只认精确命中"等退化变异必红。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn edge_selector_resolves_real_sub_shapes_by_index_and_near_point() {
        let mut k = Kernel::new().expect("启用 occt feature 后 Kernel::new() 必须成功");
        assert!(
            k.is_real(),
            "本测试必须跑真实内核，不许在 mock 上假绿；实得后端 {}",
            k.backend_name()
        );
        let h = k.make_box(10.0, 10.0, 10.0).expect("建模应成功");

        let edges = k.sub_shapes(h, SubShapeKind::Edge).expect("枚举边应成功");
        assert_eq!(edges.len(), 12, "立方体应有 12 条边，实得 {}", edges.len());

        // 每条边的几何签名；并断言签名**两两不同**（否则边身份不可区分 → 钉子失效）。
        let golden: Vec<(DVec3, DVec3, f64)> = edges
            .iter()
            .map(|e| edge_sig(&mut k, *e).expect("12 条边都应能算出几何签名"))
            .collect();
        for i in 0..golden.len() {
            for j in (i + 1)..golden.len() {
                assert!(
                    !sig_eq(golden[i], golden[j]),
                    "第 {i} 与第 {j} 条边签名相同——边身份不可区分，钉子失效"
                );
            }
        }

        // ---- ByIndex(i) 须几何同一于 golden[i]（逐条，防 off-by-one）----
        for (i, gld) in golden.iter().enumerate() {
            let resolved = EdgeSelector::ByIndex(i as u32)
                .resolve(&mut k, h)
                .expect("ByIndex 应解析成功");
            assert_eq!(resolved.len(), 1, "ByIndex 应返回恰 1 条边");
            let s = edge_sig(&mut k, resolved[0]).expect("解析出的边应能算几何签名");
            assert!(
                sig_eq(s, *gld),
                "ByIndex({i}) 解析的边应与第 {i} 条边几何同一：got {:?} vs golden {:?}",
                s,
                gld
            );
        }

        // ---- NearPoint 须取**距离最小**边（一般语义，非单点、非精确命中）----
        // 三个查询点：(1, len-2) 为**精确命中**（距离 0）；序号 5 沿**垂直于该边切向**的方向
        // **偏移 1e-3**（距离 >0，但仍是全局距离最小者）。断言返回**各自对应**的边，且返回值
        // **随查询点变化**；从而"恒取首个 / 恒取末个"、"忽略距离只按序号 / 位置返回"（逃逸 A）、
        // "只认精确命中 dist<1e-9"（逃逸 B）等退化变异必红。
        let specs: [(usize, bool); 3] = [(1, false), (golden.len() - 2, false), (5, true)];
        for &(t, _) in &specs {
            assert!(
                t != 0 && t != golden.len() - 1,
                "查询目标序号 {t} 不得为首 / 末（golden.len()={}）",
                golden.len()
            );
        }
        assert!(
            specs[0].0 != specs[1].0 && specs[1].0 != specs[2].0 && specs[0].0 != specs[2].0,
            "三个查询点的期望序号须两两不同（实得 {:?}），否则无法证『随查询点变化』",
            specs.map(|s| s.0)
        );
        let mut seen_idx: Vec<usize> = Vec::new();
        for &(t, offset) in &specs {
            let q = if offset {
                // 沿**垂直于该边切向**的方向偏移 1e-3：离开该边 → 距离 >0，但仍是全局最近边。
                let tan = golden[t].1;
                let axis = if tan.x.abs() < 0.9 {
                    DVec3::X
                } else {
                    DVec3::Y
                };
                golden[t].0 + tan.cross(axis).normalize() * 1e-3
            } else {
                golden[t].0 // 恰落在第 t 条边上（该边的精确中点）
            };
            // 独立 oracle：对全部边独立求 q 的最近点距离，自证第 t 条边确为**严格全局最近**。
            let d_t = k
                .project_point_on_edge(edges[t], q)
                .expect("目标边投影必须成功")
                .point
                .distance(q);
            for (j, &e) in edges.iter().enumerate() {
                if j == t {
                    continue;
                }
                if let Ok(poe) = k.project_point_on_edge(e, q) {
                    let dj = poe.point.distance(q);
                    assert!(
                        d_t < dj,
                        "oracle 自证失败：第 {j} 条边距 {dj} 不大于目标第 {t} 条边距 {d_t}（偏移量过大？）"
                    );
                }
            }
            if offset {
                assert!(
                    d_t > 1e-6,
                    "偏移查询点应距目标第 {t} 条边 >1e-6（非精确命中，封死『只认精确命中』逃逸），实得 {d_t}"
                );
            }
            // 选择器结果须几何同一于第 t 条边。
            let resolved = EdgeSelector::NearPoint(q)
                .resolve(&mut k, h)
                .expect("NearPoint 应解析成功");
            assert_eq!(resolved.len(), 1, "NearPoint 应返回恰 1 条边");
            let s = edge_sig(&mut k, resolved[0]).expect("解析出的边应能算几何签名");
            assert!(
                sig_eq(s, golden[t]),
                "q 最近的应是第 {t} 条边，实得 {:?} vs golden {:?}",
                s,
                golden[t]
            );
            let idx = golden
                .iter()
                .position(|gld| sig_eq(s, *gld))
                .expect("解析出的边签名必在 golden 中");
            seen_idx.push(idx);
        }
        // 退化守卫：若实现忽略距离只按固定序号 / 位置返回，多个不同 q 会返回同一子形状。
        for (a, &ia) in seen_idx.iter().enumerate() {
            for (b, &ib) in seen_idx.iter().enumerate().skip(a + 1) {
                assert_ne!(
                    ia, ib,
                    "第 {a} 与第 {b} 个查询点返回同一边——NearPoint 退化（未按距离取最小者）"
                );
            }
        }
    }

    /// 面的**几何签名** `(最近点, 单位法向)`：把固定种子点投影到面。唯一标识一个面。
    #[cfg(feature = "occt")]
    fn face_sig(kernel: &mut Kernel, f: ShapeHandle) -> (DVec3, DVec3) {
        let seed = DVec3::new(5.0, 5.0, 5.0);
        let pof = kernel.project_point_on_face(f, seed).expect("面投影应成功");
        let n = kernel
            .surface_normal(f, pof.u, pof.v)
            .expect("面法向应成功");
        (pof.point, n)
    }

    /// 三维点是否同一（容差）。
    #[cfg(feature = "occt")]
    fn v3_eq(a: DVec3, b: DVec3) -> bool {
        (a - b).length() < 1e-6
    }

    /// occt 腿钉子：真内核下 `ByTopoName`（B6.1b 接通）**确实解析出正确的子形状**。
    ///
    /// - 面 / 边：以**活句柄名表**（`face_names_from_handle` / `edge_names_from_handle`）里
    ///   的每个已知名反查 → 须解析回**同序号**的子形状（用几何签名比对，**不依赖句柄数值**）
    ///   ——从而"序号 off-by-one / 全局偏移 / 恒取首末"等退化变异必红；
    /// - **不存在的名字** → 须响亮 `Err(Geometry{op:"selector"})`，**绝不**返回空列表。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn selector_by_top_name_resolves_real_sub_shapes() {
        let mut k = Kernel::new().expect("启用 occt feature 后 Kernel::new() 必须成功");
        assert!(
            k.is_real(),
            "本测试必须跑真实内核，不许在 mock 上假绿；实得后端 {}",
            k.backend_name()
        );
        let h = k.make_box(10.0, 10.0, 10.0).expect("建模应成功");

        // ---- 面：每个已知名须解析回同序号的面 ----
        let fnames = crate::topo_naming::face_names_from_handle(&mut k, h).expect("面名表应成功");
        assert!(fnames.len() >= 2, "立方体应有多个面，实得 {}", fnames.len());
        let faces = k.sub_shapes(h, SubShapeKind::Face).expect("枚举面应成功");
        assert_eq!(
            fnames.len(),
            faces.len(),
            "面名表长度须等于 sub_shapes(Face)"
        );
        let fgolden: Vec<(DVec3, DVec3)> = faces.iter().map(|f| face_sig(&mut k, *f)).collect();
        for (fi, fname) in &fnames {
            let resolved = FaceSelector::ByTopoName(*fname)
                .resolve(&mut k, h)
                .expect("面名解析应成功");
            assert_eq!(resolved.len(), 1, "ByTopoName 应返回恰 1 个面");
            let s = face_sig(&mut k, resolved[0]);
            let gld = fgolden[*fi as usize];
            assert!(
                v3_eq(s.0, gld.0) && v3_eq(s.1, gld.1),
                "面名(序号 {fi})应解析回同一面：got {s:?} vs golden {gld:?}"
            );
        }

        // ---- 边：每个已知名须解析回同序号的边 ----
        let enames = crate::topo_naming::edge_names_from_handle(&mut k, h).expect("边名表应成功");
        assert_eq!(
            enames.len(),
            12,
            "立方体应有 12 条边，实得 {}",
            enames.len()
        );
        let edges = k.sub_shapes(h, SubShapeKind::Edge).expect("枚举边应成功");
        let egolden: Vec<(DVec3, DVec3, f64)> = edges
            .iter()
            .map(|e| edge_sig(&mut k, *e).expect("12 条边都应能算出几何签名"))
            .collect();
        for (ei, ename) in &enames {
            let resolved = EdgeSelector::ByTopoName(*ename)
                .resolve(&mut k, h)
                .expect("边名解析应成功");
            assert_eq!(resolved.len(), 1, "ByTopoName 应返回恰 1 条边");
            let s = edge_sig(&mut k, resolved[0]).expect("解析出的边应能算几何签名");
            assert!(
                sig_eq(s, egolden[*ei as usize]),
                "边名(序号 {ei})应解析回同一条边：got {s:?} vs golden {:?}",
                egolden[*ei as usize]
            );
        }

        // ---- 不存在的名字 → 响亮 Err(Geometry)，绝不返回空列表 ----
        let bogus = TopoName::of_face([1.0e6, 1.0e6, 1.0e6], [0.0, 0.0, 1.0], 100.0, [0.0; 3]);
        let got = FaceSelector::ByTopoName(bogus).resolve(&mut k, h);
        match got {
            Err(KernelError::Geometry { op, .. }) => assert_eq!(op, "selector"),
            other => panic!("不存在的面名应 Err(Geometry)，实得 {other:?}"),
        }
        let got = EdgeSelector::ByTopoName(bogus).resolve(&mut k, h);
        match got {
            Err(KernelError::Geometry { op, .. }) => assert_eq!(op, "selector"),
            other => panic!("不存在的边名应 Err(Geometry)，实得 {other:?}"),
        }
    }

    /// occt 腿钉子（一般语义 · **曲边**）：`ByTopoName` 对**角参数化圆弧孔边**也能正确解析。
    ///
    /// 防"只对直边可用"的退化实现：用带通孔板，按几何判据（`len/span ≈ 半径 15 ≫ 1`）挑一条
    /// 圆弧边名反查 → 解析出的边须仍是圆弧边（`len/span` 仍 ≈ 半径，而非某条直边的比值 1）。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn selector_by_top_name_resolves_arc_edge() {
        let mut k = Kernel::new().expect("启用 occt feature 后 Kernel::new() 必须成功");
        assert!(k.is_real(), "本测试必须跑真实内核");
        let plate = k.make_box(100.0, 60.0, 10.0).unwrap();
        let hole = k.make_cylinder(15.0, 40.0).unwrap();
        let h = k.cut(plate, hole).unwrap();

        let enames = crate::topo_naming::edge_names_from_handle(&mut k, h).expect("边名表应成功");
        let edges = k.sub_shapes(h, SubShapeKind::Edge).expect("枚举边应成功");

        // 按几何判据挑圆弧边：`len/span ≈ 半径 15 ≫ 1`（直边比值恒 1）。
        let mut arc_name = None;
        for (ei, nm) in &enames {
            let e = edges[*ei as usize];
            let len = k.curve_length(e).unwrap();
            let span = {
                let r = k.curve_parameters(e).unwrap();
                r.last - r.first
            };
            if span > 0.0 && len > span * 1.5 {
                arc_name = Some(*nm);
                break;
            }
        }
        let arc_name = arc_name.expect("带通孔板应存在角参数化圆弧孔边");

        let resolved = EdgeSelector::ByTopoName(arc_name)
            .resolve(&mut k, h)
            .expect("弧边名应解析成功");
        assert_eq!(resolved.len(), 1, "ByTopoName 应返回恰 1 条边");
        let e = resolved[0];
        let len = k.curve_length(e).expect("解析出的边应能取长度");
        let span = {
            let r = k.curve_parameters(e).expect("解析出的边应能取参数");
            r.last - r.first
        };
        assert!(
            (len / span - 15.0).abs() < 1e-3,
            "解析出的应是圆弧边（len/span≈半径 15），实得 len={len} span={span}"
        );
    }

    /// **钉 12（M5-B6.2b）**：`edge_selectors` JSON 编解码 **往返相等**；
    /// **非法 JSON → 响亮 `Err(RecipeNotExecutable)`**（绝不静默空表）。
    ///
    /// 线格式 = 选择器 serde 外部标签表示（规格 §2.7.16.1 权威）——故逐字断言
    /// `[{"ByIndex":3}]`，防"改成内部标签 / 对象包裹"之类的 schema 漂移。
    #[test]
    fn edge_selectors_json_roundtrip_and_loud_failure() {
        let tn = sample_toponame();
        let originals = vec![
            EdgeSelector::ByIndex(3),
            EdgeSelector::NearPoint(DVec3::new(1.0, 2.0, 3.0)),
            EdgeSelector::ByTopoName(tn),
        ];
        let text = encode_edge_selectors(&originals).expect("编码不应失败");
        let parsed = parse_edge_selectors(&text).expect("合法 JSON 应解析成功");
        assert_eq!(parsed, originals, "encode∘parse 往返必须逐元素相等");

        // 外部标签线格式（权威）：单元素即 `[{"ByIndex":3}]`
        assert_eq!(
            encode_edge_selectors(&[EdgeSelector::ByIndex(3)]).unwrap(),
            r#"[{"ByIndex":3}]"#,
            "线格式须为 serde 外部标签表示"
        );

        // 非法 JSON / schema 不符 → 一律响亮 Err（绝不静默返回空表）
        for bad in [
            "",
            "not json",
            "[{\"Nope\":1}]",
            "{\"ByIndex\":3}",
            "[3]",
            "null",
        ] {
            match parse_edge_selectors(bad) {
                Err(AppError::RecipeNotExecutable(_)) => {}
                other => panic!(
                    "非法 edge_selectors {bad:?} 必须响亮 Err(RecipeNotExecutable)，实得 {other:?}"
                ),
            }
        }
    }

    /// **钉 12 的面对偶**：`face_selectors` JSON 编解码往返 + 非法 JSON 响亮失败。
    #[test]
    fn face_selectors_json_roundtrip_and_loud_failure() {
        let tn = sample_toponame();
        let originals = vec![
            FaceSelector::ByIndex(0),
            FaceSelector::NearPoint(DVec3::new(-1.5, 0.0, 2.5)),
            FaceSelector::ByTopoName(tn),
        ];
        let text = encode_face_selectors(&originals).expect("编码不应失败");
        let parsed = parse_face_selectors(&text).expect("合法 JSON 应解析成功");
        assert_eq!(parsed, originals, "encode∘parse 往返必须逐元素相等");

        assert_eq!(
            encode_face_selectors(&[FaceSelector::ByIndex(0)]).unwrap(),
            r#"[{"ByIndex":0}]"#,
            "线格式须为 serde 外部标签表示"
        );

        for bad in ["", "not json", "[{\"Nope\":1}]", "{}", "[true]"] {
            match parse_face_selectors(bad) {
                Err(AppError::RecipeNotExecutable(_)) => {}
                other => panic!(
                    "非法 face_selectors {bad:?} 必须响亮 Err(RecipeNotExecutable)，实得 {other:?}"
                ),
            }
        }
    }
}

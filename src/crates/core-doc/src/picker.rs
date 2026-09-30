//! 三维拾取：注入接口 + **纯逻辑**射线-三角粗筛（B4.0 接口；B4.1a 粗筛数学）。
//!
//! ## 为什么相机只给 trait、不给具体实现
//!
//! 依据方案 §2.4：`core-doc` **不应定义具体相机**——相机是 UI / 渲染层的东西，
//! 若 `core-doc` 定义它，就会反向依赖渲染层。正确做法是把"屏幕坐标 → 世界射线"
//! 这一件事抽象成 [`Camera`] **trait 注入**：UI / 渲染层提供实现，`core-doc`
//! （及将来的拾取器）只消费它。
//!
//! ## 本模块的边界（架构铁律③）
//!
//! `core-doc` **只依赖 `core-model`**，**不得**依赖 `core-kernel`（`xtask check-deps`
//! 硬校验）。因此这里的数学**零内核依赖**：只吃 `glam::DVec3` / `[f64; 3]` / `&[u32]`，
//! **不出现** `core_kernel::MeshData` / `ShapeHandle`。网格语义与内核 `MeshData`
//! 一致（`positions` + 三角 `indices`），但**不共用类型**。
//!
//! B4.1a 交付：[`ray_triangle_intersect`]（Möller–Trumbore）与 [`ray_hits_mesh`]
//! （粗筛取最近命中）。**KernelPicker 服务本体、内核 `surface_normal` 门面属 B4.1b**，
//! 不在本模块。

use glam::DVec3;
use serde::{Deserialize, Serialize};

/// 世界空间中的一条射线（起点 + **单位**方向）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    /// 射线起点（世界坐标，mm）
    pub origin: DVec3,
    /// 射线方向（**单位向量**）
    pub direction: DVec3,
}

/// 相机：把**屏幕像素坐标**映射为世界空间射线（最小注入接口）。
///
/// `screen` 约定：视口**像素**坐标，原点在**左上角**，x 向右、y 向下。
/// 由 UI / 渲染层实现本 trait，`core-doc` 不定义具体相机。
pub trait Camera {
    /// 求过屏幕像素点 `screen` 的世界空间射线。
    fn ray_through(&self, screen: (f32, f32)) -> Ray;
}

// =============================================================================
// 拾取数据类型（B4.1a）
// =============================================================================

/// 拾取到的子形状种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PickKind {
    /// 面。
    Face,
    /// 边。
    Edge,
    /// 顶点。
    Vertex,
}

/// 拾取参数（面局部参数 / 曲线参数，含义由实现与对应 [`PickKind`] 决定）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PickParam {
    /// 面局部参数 `(u, v)`。
    Face {
        /// 面局部参数 u。
        u: f64,
        /// 面局部参数 v。
        v: f64,
    },
    /// 曲线参数 `t`（**非归一化**）。
    Edge {
        /// 曲线参数。
        t: f64,
    },
    /// 顶点（无参数）。
    Vertex,
}

/// 一次拾取的结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PickResult {
    /// 命中的持久元素 id。
    pub element: core_model::ElementId,
    /// 命中子形状种类。
    pub kind: PickKind,
    /// **会话内**用于高亮的面/边哈希（**不可持久化**：跨会话重建后失效）。
    pub topo_hash: u64,
    /// 世界空间命中点（mm）。
    pub world_point: DVec3,
    /// 拾取参数。
    pub param: PickParam,
    /// 命中处法向（mm，**单位**向量）；边/顶点或不可得时为 `None`。
    pub normal: Option<DVec3>,
}

/// 拾取过滤（只选面 / 只选边 / …）。默认**全 false**（显式开启）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct PickFilter {
    /// 是否可选面。
    pub faces: bool,
    /// 是否可选边。
    pub edges: bool,
    /// 是否可选顶点。
    pub vertices: bool,
}

impl PickFilter {
    /// 只选面（首版面拾取用）。
    #[must_use]
    pub const fn faces_only() -> Self {
        Self {
            faces: true,
            edges: false,
            vertices: false,
        }
    }
}

// =============================================================================
// 纯逻辑：射线-三角求交 + 网格粗筛（B4.1a；零内核依赖）
// =============================================================================

/// 求交/重心坐标判定容差（无量纲，配合单位方向与 mm 级几何）。
const RAY_EPS: f64 = 1e-9;

/// 射线与单个三角形求交（**Möller–Trumbore**）。
///
/// 返回 `(t, u, v)`：`t` 为沿 `dir` 的距离参数（`dir` 必须为**单位**向量，
/// 否则 `t` 不是距离）；`u`/`v` 为重心坐标（对应顶点 `b`/`c`，`a` 的权重为 `1-u-v`）。
///
/// 仅命中满足 `t >= 0` 且 `u, v >= -EPS`、`u + v <= 1 + EPS` 的射线；
/// 否则 `None`（含背面/反方向 `t < 0`、与三角形平面**平行**、退化三角形等）。
#[must_use]
pub fn ray_triangle_intersect(
    origin: DVec3,
    dir: DVec3,
    a: DVec3,
    b: DVec3,
    c: DVec3,
) -> Option<(f64, f64, f64)> {
    let e1 = b - a;
    let e2 = c - a;
    let pvec = dir.cross(e2);
    let det = e1.dot(pvec);
    // 行列式趋零 → 射线与三角形平面平行（或三角形退化）。
    if det.abs() < RAY_EPS {
        return None;
    }
    let inv_det = 1.0 / det;
    let tvec = origin - a;
    let u = tvec.dot(pvec) * inv_det;
    if u < -RAY_EPS || u > 1.0 + RAY_EPS {
        return None;
    }
    let qvec = tvec.cross(e1);
    let v = dir.dot(qvec) * inv_det;
    if v < -RAY_EPS || u + v > 1.0 + RAY_EPS {
        return None;
    }
    let t = e2.dot(qvec) * inv_det;
    if t < 0.0 {
        return None;
    }
    Some((t, u, v))
}

/// 一次网格粗筛命中。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TriangleHit {
    /// 命中的三角形序号（`indices` 里第几个**完整**三角形，0 基）。
    pub tri_index: usize,
    /// 沿射线距离参数（`ray.direction` 为单位向量时即距离，mm）。
    pub distance: f64,
    /// 世界交点（mm）。
    pub point: DVec3,
    /// 三角形**单位**法向（`(b-a)×(c-a)` 归一化；退化三角形已跳过）。
    pub normal: DVec3,
    /// 重心坐标 `(u, v)`（对顶点 `b`/`c`）。
    pub bary: (f64, f64),
}

/// 在整张三角网格上求**最近**命中（扫 `indices` 每 3 个一三角形，取 `t` 最小者）。
///
/// `positions` / `indices` 的语义同内核 `MeshData`（**但不依赖它**）：
/// `indices` 每 3 个一组构成一个三角形，值为 `positions` 的下标。
///
/// # 不完整/非法输入的口径（**不静默错位**）
///
/// - `indices.len()` **不是 3 的倍数**：只遍历 `indices.len() / 3` 个**完整**三角形，
///   尾随的残缺索引**直接忽略**（不会与前面的索引错位配对成伪三角形）。
/// - 某三角形含**越界索引**（`>= positions.len()`）：**跳过该三角形**（不 panic）。
/// - **退化三角形**（三点共线 / 法向无法归一化）：**跳过**（不 panic）。
#[must_use]
pub fn ray_hits_mesh(ray: &Ray, positions: &[[f64; 3]], indices: &[u32]) -> Option<TriangleHit> {
    let mut best: Option<TriangleHit> = None;

    // `as_chunks::<3>()`：尾随残缺索引被忽略（不静默错位）。
    for (tri_index, tri) in indices.as_chunks::<3>().0.iter().enumerate() {
        // 越界索引 → 跳过该三角形（不 panic）。
        let [i0, i1, i2] = *tri;
        if i0 as usize >= positions.len()
            || i1 as usize >= positions.len()
            || i2 as usize >= positions.len()
        {
            continue;
        }
        let a = DVec3::from(positions[i0 as usize]);
        let b = DVec3::from(positions[i1 as usize]);
        let c = DVec3::from(positions[i2 as usize]);

        // 退化三角形 → 跳过（法向无法归一化）。
        let n = (b - a).cross(c - a);
        let n_len = n.length();
        if n_len < RAY_EPS {
            continue;
        }

        let Some((t, u, v)) = ray_triangle_intersect(ray.origin, ray.direction, a, b, c) else {
            continue;
        };
        // 取 t 最小者（最近命中）。
        if best.as_ref().is_some_and(|h| h.distance <= t) {
            continue;
        }
        best = Some(TriangleHit {
            tri_index,
            distance: t,
            point: ray.origin + ray.direction * t,
            normal: n / n_len,
            bary: (u, v),
        });
    }

    best
}

#[cfg(test)]
mod tests {
    use super::*;

    /// B4.0 钉子 ③：一个最小测试相机——证明 [`Camera`] 接口 **object-safe 且可注入**。
    struct TestCamera {
        ray: Ray,
    }

    impl Camera for TestCamera {
        fn ray_through(&self, _screen: (f32, f32)) -> Ray {
            self.ray
        }
    }

    #[test]
    fn camera_is_object_safe_and_injectable() {
        let expected = Ray {
            origin: DVec3::new(1.0, 2.0, 3.0),
            direction: DVec3::NEG_Z,
        };
        let cam = TestCamera { ray: expected };

        // 作为 trait 对象注入（object-safe 的证明）
        let c: &dyn Camera = &cam;
        assert_eq!(c.ray_through((0.0, 0.0)), expected);
        assert_eq!(c.ray_through((123.0, 456.0)), expected);
    }

    // ---- B4.1a 钉子：射线-三角求交 + 网格粗筛（纯逻辑，零内核依赖）-------------

    /// 单位正方形（XY 平面，z=0）拆成两个三角形：`(0,1,2)` 与 `(0,2,3)`。
    fn unit_square() -> (Vec<[f64; 3]>, Vec<u32>) {
        let positions = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ];
        let indices = vec![0, 1, 2, 0, 2, 3];
        (positions, indices)
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9
    }

    /// B4.1a 钉子 ①：求交基本正确——`t/u/v` 与解析值逐位吻合（容差 1e-9），交点正确。
    #[test]
    fn ray_triangle_intersect_hits_with_exact_barycentrics() {
        let a = DVec3::new(0.0, 0.0, 0.0);
        let b = DVec3::new(1.0, 0.0, 0.0);
        let c = DVec3::new(1.0, 1.0, 0.0);
        // 从 (0.75,0.25,1) 沿 -Z 打下去：解析命中 (t,u,v) = (1.0, 0.5, 0.25)。
        let (t, u, v) = ray_triangle_intersect(DVec3::new(0.75, 0.25, 1.0), DVec3::NEG_Z, a, b, c)
            .expect("应命中三角形");
        assert!(approx(t, 1.0), "t 应为 1.0，实得 {t}");
        assert!(approx(u, 0.5), "u 应为 0.5，实得 {u}");
        assert!(approx(v, 0.25), "v 应为 0.25，实得 {v}");

        // 走整网格接口一并验证 point/normal/bary/tri_index。
        let (positions, indices) = unit_square();
        let ray = Ray {
            origin: DVec3::new(0.75, 0.25, 1.0),
            direction: DVec3::NEG_Z,
        };
        let mh = ray_hits_mesh(&ray, &positions, &indices).expect("网格应命中");
        assert_eq!(mh.tri_index, 0, "应命中第一个三角形");
        assert!(
            approx(mh.distance, 1.0),
            "distance 应为 1.0，实得 {}",
            mh.distance
        );
        assert!(
            (mh.point - DVec3::new(0.75, 0.25, 0.0)).length() <= 1e-9,
            "交点应为 (0.75,0.25,0)，实得 {:?}",
            mh.point
        );
        assert!(
            (mh.normal - DVec3::Z).length() <= 1e-9,
            "法向应为 +Z，实得 {:?}",
            mh.normal
        );
        assert!(
            approx(mh.bary.0, 0.5) && approx(mh.bary.1, 0.25),
            "重心坐标应 (0.5,0.25)，实得 {:?}",
            mh.bary
        );
    }

    /// B4.1a 钉子 ②：未命中 / 背面（`t<0`）/ 平行 → `None`。
    #[test]
    fn ray_triangle_intersect_rejects_miss_backface_and_parallel() {
        let a = DVec3::new(0.0, 0.0, 0.0);
        let b = DVec3::new(1.0, 0.0, 0.0);
        let c = DVec3::new(1.0, 1.0, 0.0);

        // 偏离：射线落在三角形之外。
        assert!(
            ray_triangle_intersect(DVec3::new(2.0, 2.0, 1.0), DVec3::NEG_Z, a, b, c).is_none(),
            "三角形外的射线应未命中"
        );
        // 反方向（三角形在射线背后）：origin 在 z=-1、dir 仍 -Z → t<0。
        assert!(
            ray_triangle_intersect(DVec3::new(0.75, 0.25, -1.0), DVec3::NEG_Z, a, b, c).is_none(),
            "t<0（三角形在射线背后）应判未命中"
        );
        // 平行：dir 落在三角形平面内（+X）→ det≈0。
        assert!(
            ray_triangle_intersect(DVec3::new(0.75, 0.25, 1.0), DVec3::X, a, b, c).is_none(),
            "与三角形平面平行的射线应未命中"
        );
    }

    /// B4.1a 钉子 ③：网格**最近**命中——两块不同 z 的三角形，射线穿过两者取 `t` 最小者。
    #[test]
    fn ray_hits_mesh_returns_the_nearest_triangle() {
        let positions = vec![
            [0.0, 0.0, 2.0],
            [1.0, 0.0, 2.0],
            [1.0, 1.0, 2.0], // tri0 @ z=2（较远）
            [0.0, 0.0, 5.0],
            [1.0, 0.0, 5.0],
            [1.0, 1.0, 5.0], // tri1 @ z=5（较近）
        ];
        let indices = vec![0, 1, 2, 3, 4, 5];
        let ray = Ray {
            origin: DVec3::new(0.75, 0.25, 10.0),
            direction: DVec3::NEG_Z,
        };
        let hit = ray_hits_mesh(&ray, &positions, &indices).expect("应命中");
        assert_eq!(
            hit.tri_index, 1,
            "应命中更近的 tri1(z=5)，实得 tri_index={}",
            hit.tri_index
        );
        assert!(
            approx(hit.distance, 5.0),
            "最近距离应为 5.0，实得 {}",
            hit.distance
        );
        assert!(
            (hit.normal.length() - 1.0).abs() <= 1e-9,
            "法向应为单位向量，实得 |n|={}",
            hit.normal.length()
        );
        assert!(
            (hit.point - DVec3::new(0.75, 0.25, 5.0)).length() <= 1e-9,
            "交点应为 (0.75,0.25,5)，实得 {:?}",
            hit.point
        );
    }

    /// B4.1a 钉子 ④：守卫——退化三角形 / 越界索引 / `len % 3 != 0` 均不 panic、不静默错位。
    #[test]
    fn ray_hits_mesh_guards_degenerate_out_of_range_and_ragged_indices() {
        let ray = Ray {
            origin: DVec3::new(0.75, 0.25, 1.0),
            direction: DVec3::NEG_Z,
        };

        // (a) 退化三角形（三点共线）→ 跳过，不 panic，无命中。
        let degenerate = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]];
        assert!(
            ray_hits_mesh(&ray, &degenerate, &[0, 1, 2]).is_none(),
            "退化三角形应被跳过（无命中），且不得 panic"
        );

        // (b) 越界索引 → 跳过该三角形，不 panic。
        let (positions, _) = unit_square();
        assert!(
            ray_hits_mesh(&ray, &positions, &[0, 1, 9]).is_none(),
            "越界索引三角形应被跳过（无命中），且不得 panic"
        );
        // 合法 + 越界混合：只算合法那条，且序号按“完整三角形”计数。
        let mixed = ray_hits_mesh(&ray, &positions, &[0, 1, 9, 0, 1, 2]).expect("合法三角形应命中");
        assert_eq!(mixed.tri_index, 1, "混合输入下应命中第 2 个（合法）三角形");

        // (c) `indices.len() % 3 != 0`：尾随残缺索引被忽略，**不得**与前面错位配对。
        //     结果必须与“只取前 len/3*3 个索引”逐位一致。
        let (positions2, full) = unit_square();
        let mut ragged = full.clone();
        ragged.push(7); // 尾随残缺索引（len=7 → 7%3==1）
        assert_eq!(ragged.len() % 3, 1, "本用例要求 len%3==1");
        let got = ray_hits_mesh(&ray, &positions2, &ragged).expect("残缺输入仍应命中完整三角形");
        let baseline = ray_hits_mesh(&ray, &positions2, &full).expect("完整输入应命中");
        assert_eq!(got, baseline, "残缺尾索引不得改变结果（不静默错位）");
    }

    /// B4.1a 钉子 ⑤：拾取类型——过滤器默认/only，结果字段齐全（含 serde 往返）。
    #[test]
    fn pick_types_defaults_and_field_construction() {
        assert_eq!(
            PickFilter::default(),
            PickFilter {
                faces: false,
                edges: false,
                vertices: false
            },
            "默认过滤器应全 false（显式开启）"
        );
        let fo = PickFilter::faces_only();
        assert!(
            fo.faces && !fo.edges && !fo.vertices,
            "faces_only 应只开 faces，实得 {fo:?}"
        );

        // 面拾取结果字段齐全。
        let id = core_model::ElementId::new();
        let r = PickResult {
            element: id,
            kind: PickKind::Face,
            topo_hash: 0xDEAD_BEEF,
            world_point: DVec3::new(1.0, 2.0, 3.0),
            param: PickParam::Face { u: 0.25, v: 0.5 },
            normal: Some(DVec3::Z),
        };
        assert_eq!(r.element, id);
        assert_eq!(r.kind, PickKind::Face);
        assert_eq!(r.topo_hash, 0xDEAD_BEEF);
        assert_eq!(r.param, PickParam::Face { u: 0.25, v: 0.5 });
        assert_eq!(r.normal, Some(DVec3::Z));

        // 边 / 顶点参数形状可构造。
        assert_eq!(PickParam::Edge { t: 1.5 }, PickParam::Edge { t: 1.5 });
        assert_eq!(PickParam::Vertex, PickParam::Vertex);

        // serde：`PickResult` 含 `ElementId`（持久 id，已 serde）——应能 JSON 往返。
        let json = serde_json::to_string(&r).expect("PickResult 应可序列化");
        let back: PickResult = serde_json::from_str(&json).expect("PickResult 应可反序列化");
        assert_eq!(back, r, "PickResult JSON 往返应保真");
    }
}

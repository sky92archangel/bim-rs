//! 三维拾取服务（[`KernelPicker`]）：屏幕坐标 → 世界射线 → 网格粗筛 → 内核精算 → 文档元素。
//!
//! ## 链路（设计文档 §6.6 "拾取粗筛"）
//!
//! ```text
//! screen ──Camera──▶ Ray ──ray_hits_mesh──▶ TriangleHit（粗筛，取全局最近）
//!                                              │
//!                          tri_index ──face_groups──▶ g（面组索引）
//!                                              │
//!                        sub_shapes(Face)[g] ──R16 同序──▶ face 句柄
//!                                              │
//!                    project_point_on_face / surface_normal ──▶ (u,v) / 法向
//!                                              │
//!                       handle ──element.geometry_ref 反查──▶ ElementId
//! ```
//!
//! ## 为什么是"具体服务"而不是 `core_doc::Picker` trait（据实校正，**非遗漏**）
//!
//! 方案 §2.4 里的 `fn pick(&self, …) -> Option<PickResult>` 只是**示例签名**，本批
//! **不落**该 trait，理由有三：
//!
//! 1. 当前**无 UI 消费者**——没有第二个实现，抽象零收益；
//! 2. `pick` 必须 `&mut` 访问内核（`tessellate` / `project_point_on_face` 都要 `&mut`），
//!    `&self` 签名**不成立**；
//! 3. `-> Option<PickResult>` 会强迫把内核错误**吞成 `None`**，违反本项目
//!    "宁可响亮、绝不静默"的铁律。
//!
//! 故先落具体服务 [`KernelPicker`]，返回 `AppResult<Option<PickResult>>`：
//! **未命中是 `Ok(None)`、错误是 `Err`**（两者可区分）。将来真有第二个实现再抽 trait。
//!
//! ## 本批范围（B4.1b-2）
//!
//! 首版**只做面拾取**。边 / 顶点过滤器 → [`AppError::Unsupported`]（**响亮拒绝**，
//! 绝不 `Ok(None)` 假装"没选中"）。

use core_doc::{Camera, PickFilter, PickKind, PickParam, PickResult, ray_hits_mesh};
use core_kernel::{KernelError, ShapeHandle, SubShapeKind};
use core_model::{Document, ShapeId};
use glam::DVec3;

use crate::error::{AppError, AppResult};
use crate::geometry_service::GeometryService;

/// 拾取粗筛用的**线性挠度**（mm）。
///
/// 取 0.1 mm：远细于常见构件尺寸，保证面边界处的三角**不跨越两个面**
/// （否则粗筛会把命中归到错误的面组），又不至于把网格撑爆。真实内核也接受该量级。
const PICK_LINEAR: f64 = 0.1;

/// 拾取粗筛用的**角度挠度**（rad）。
///
/// 取 0.5 rad ≈ 28.6°：对平面 / 圆柱面足以保住面分组边界，同时控制三角数量。
const PICK_ANGULAR: f64 = 0.5;

/// 三维拾取服务：借用几何服务（句柄映射 + 内核）与文档（元素反查）。
pub struct KernelPicker<'a> {
    /// 几何服务：`ShapeId ↔ ShapeHandle` 映射 + 内核（`&mut`，粗筛 / 精算都要）。
    geometry: &'a mut GeometryService,
    /// 文档：由命中的 `ShapeId` 反查所属 `Element`。
    document: &'a Document,
}

impl<'a> KernelPicker<'a> {
    /// 新建拾取器（借用几何服务与文档）。
    #[must_use]
    pub fn new(geometry: &'a mut GeometryService, document: &'a Document) -> Self {
        Self { geometry, document }
    }

    /// 拾取屏幕 `screen` 处的**面**。
    ///
    /// 返回语义（**未命中与错误严格区分**）：
    /// - `Ok(Some(r))`：命中某个元素的某个面；
    /// - `Ok(None)`：未请求面、缓存为空、或全部未命中（**正常情况，不是错误**）；
    /// - `Err(AppError::Unsupported)`：请求了**边 / 顶点**（首版未实现）——**响亮拒绝**；
    /// - `Err(AppError::Kernel(…))`：内核侧真错误（占位后端无面拓扑 / 网格分组不完整 /
    ///   面组越界等），**绝不吞成 `None`**。
    ///
    /// 本方法**绝不 panic**。
    pub fn pick(
        &mut self,
        screen: (f32, f32),
        camera: &dyn Camera,
        filter: PickFilter,
    ) -> AppResult<Option<PickResult>> {
        // 边 / 顶点：首版未实现 → 响亮拒绝（绝不 `Ok(None)` 装作"没选中"）。
        if filter.edges || filter.vertices {
            return Err(AppError::Unsupported(
                "边/顶点拾取尚未实现（B4.1b-2 首版只做面）".to_string(),
            ));
        }
        // 未请求面 → 无事可做。
        if !filter.faces {
            return Ok(None);
        }

        let ray = camera.ray_through(screen);

        // 粗筛胜者：只留最小必要信息，**不长期持有 `mesh`**。
        struct Coarse {
            shape: ShapeId,
            handle: ShapeHandle,
            distance: f64,
            group_index: usize,
            face_hash: u64,
            point: DVec3,
        }

        let mut best: Option<Coarse> = None;
        for shape in self.geometry.cached_ids() {
            let Some(handle) = self.geometry.handle_of(shape) else {
                continue;
            };
            let mesh = self
                .geometry
                .kernel_mut()
                .tessellate(handle, PICK_LINEAR, PICK_ANGULAR)?;
            let Some(hit) = ray_hits_mesh(&ray, &mesh.positions, &mesh.indices) else {
                continue;
            };
            // `tri_index` → `face_group` 索引；找不到 → 响亮报错（**不得**静默落到 0 号面）。
            let Some(group_index) = mesh.face_groups.iter().position(|fg| {
                let start = fg.tri_start as usize;
                hit.tri_index >= start && hit.tri_index < start + fg.tri_count as usize
            }) else {
                return Err(KernelError::Geometry {
                    op: "pick",
                    msg: format!(
                        "粗筛命中的三角形 {} 不属于任何 face_group（网格分组不完整）",
                        hit.tri_index
                    ),
                }
                .into());
            };
            // 跨形状取**全局最近**。
            if best.as_ref().is_some_and(|b| b.distance <= hit.distance) {
                continue;
            }
            best = Some(Coarse {
                shape,
                handle,
                distance: hit.distance,
                group_index,
                face_hash: mesh.face_groups[group_index].face_hash,
                point: hit.point,
            });
        }

        let Some(best) = best else {
            return Ok(None);
        };

        // 元素反查：几何孤儿（无对应元素）**不可**作为"元素"被拾取。
        let Some(element) = self
            .document
            .elements()
            .values()
            .find(|e| e.geometry_ref == Some(best.shape))
            .map(|e| e.id)
        else {
            return Ok(None);
        };

        // ---- 精算：R16 同序 → `faces[g]` 即命中面 ----
        let faces = self
            .geometry
            .kernel_mut()
            .sub_shapes(best.handle, SubShapeKind::Face)?;
        if faces.is_empty() {
            // 粗筛命中一个有网格的形状，内核却**枚举不出任何面** → 这是数据不一致
            // （网格与拓扑互相矛盾），**不是**"能力缺失"；与紧邻的 face_group 越界分支
            // 同语义，如实报 `Geometry`（占位后端 mock 亦走此分支）。
            return Err(KernelError::Geometry {
                op: "pick",
                msg: "粗筛命中但形状枚举不出面（内核返回空子形状）——网格与拓扑不一致".to_string(),
            }
            .into());
        }
        let Some(&face) = faces.get(best.group_index) else {
            return Err(KernelError::Geometry {
                op: "pick",
                msg: format!(
                    "命中面组索引 {} 越界（内核面数 {}）——R16 同序被破坏",
                    best.group_index,
                    faces.len()
                ),
            }
            .into());
        };
        let pof = self
            .geometry
            .kernel_mut()
            .project_point_on_face(face, best.point)?;
        // 法向取不到**不致命**（内核不提供时）→ `None`，不 panic。
        let normal = self
            .geometry
            .kernel_mut()
            .surface_normal(face, pof.u, pof.v)
            .ok();

        Ok(Some(PickResult {
            element,
            kind: PickKind::Face,
            topo_hash: best.face_hash,
            world_point: best.point,
            param: PickParam::Face { u: pof.u, v: pof.v },
            normal,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_doc::Ray;
    use core_model::{Category, Element};

    /// 固定射线的测试相机（证明 [`Camera`] object-safe 且可注入）。
    struct FixedCamera(Ray);

    impl Camera for FixedCamera {
        fn ray_through(&self, _screen: (f32, f32)) -> Ray {
            self.0
        }
    }

    /// 默认腿（mock）钉子 ①：**边 / 顶点过滤器必须响亮拒绝**（`Err`），绝不静默 `Ok(None)`。
    ///
    /// 守"跳过与通过可区分"：首版不支持边 / 顶点，若返回 `Ok(None)`，调用方会以为
    /// "用户没点到"，从而**永远发现不了**这个能力缺失。
    #[test]
    fn picker_rejects_edge_and_vertex_filters_loudly() {
        let mut g = GeometryService::with_mock_kernel();
        let doc = Document::new("d");
        let cam = FixedCamera(Ray {
            origin: DVec3::ZERO,
            direction: DVec3::NEG_Z,
        });
        let mut p = KernelPicker::new(&mut g, &doc);

        for filter in [
            PickFilter {
                faces: true,
                edges: true,
                vertices: false,
            },
            PickFilter {
                faces: true,
                edges: false,
                vertices: true,
            },
            // 连面都不请求、只请求边 / 顶点 → 仍须响亮拒绝（不能被"faces=false → Ok(None)"短路）。
            PickFilter {
                faces: false,
                edges: true,
                vertices: true,
            },
        ] {
            let got = p.pick((0.0, 0.0), &cam, filter);
            assert!(
                matches!(got, Err(AppError::Unsupported(_))),
                "边/顶点过滤器应响亮拒绝（Err(Unsupported)），实得 {got:?}（filter={filter:?}）"
            );
        }

        // 对照：完全未请求任何种类 → `Ok(None)`（不是 Err）。
        let got = p.pick((0.0, 0.0), &cam, PickFilter::default());
        assert!(
            matches!(got, Ok(None)),
            "空过滤器应 Ok(None)（无事可做），实得 {got:?}"
        );
    }

    /// 默认腿（mock）钉子 ②：**mock 绝不允许假命中**。
    ///
    /// 先取 mock 网格、选一个三角形、构造一条**确实打中它**的射线（并独立用
    /// [`ray_hits_mesh`] 确认粗筛命中），再走完整 `pick`。断言结果：
    /// - **绝不**是 `Ok(Some(_))`（mock 没有精算能力，任何"命中结果"都是编造的）；
    /// - 粗筛命中后必须得到 **`Err(Kernel(Geometry{op:"pick",..}))`**——mock 的
    ///   `sub_shapes(Face)` 返回空，即"网格与拓扑不一致"，`pick` 如实报 `Geometry`
    ///   （而非 `Ok(None)` 蒙混、也非 `Unsupported`）。
    #[test]
    fn picker_never_fakes_a_hit_on_mock() {
        // 造一个 mock 形状并缓存 + 装元素（走真实的反查路径，否则会短路成 Ok(None)）。
        let mut g = GeometryService::with_mock_kernel();
        let handle = g
            .kernel_mut()
            .make_box(10.0, 10.0, 10.0)
            .expect("mock 建模应成功");
        let shape = ShapeId::new();
        g.cache(shape, handle);
        let mut doc = Document::new("d");
        let element = Element::new(Category::generic(), "box").with_geometry(shape);
        doc.insert_element(element).expect("插入元素应成功");

        // 取 mock 网格，选第一个三角形，构造一条确实打中它的射线。
        let mesh = g
            .kernel_mut()
            .tessellate(handle, PICK_LINEAR, PICK_ANGULAR)
            .expect("mock 网格应可用");
        let a = DVec3::from(mesh.positions[mesh.indices[0] as usize]);
        let b = DVec3::from(mesh.positions[mesh.indices[1] as usize]);
        let c = DVec3::from(mesh.positions[mesh.indices[2] as usize]);
        let centroid = (a + b + c) / 3.0;
        let n = (b - a).cross(c - a).normalize();
        let ray = Ray {
            origin: centroid + n * 10.0,
            direction: -n,
        };

        // 独立确认：这条射线**确实**命中粗筛（否则本用例的"命中后须报错"前提不成立）。
        let coarse = ray_hits_mesh(&ray, &mesh.positions, &mesh.indices)
            .expect("构造的射线应命中某个三角形");
        assert!(coarse.distance > 0.0, "粗筛命中距离应为正");

        let cam = FixedCamera(ray);
        let mut p = KernelPicker::new(&mut g, &doc);
        let got = p.pick((0.0, 0.0), &cam, PickFilter::faces_only());

        // ① 绝不假命中。
        assert!(
            !matches!(got, Ok(Some(_))),
            "mock 腿绝不允许假命中（不得返回 Ok(Some(_))），实得 {got:?}"
        );
        // ② 粗筛命中后 → 精算处**响亮拒绝**（mock 无面拓扑 → 网格与拓扑不一致 → Kernel::Geometry）。
        match got {
            Err(AppError::Kernel(KernelError::Geometry { op: "pick", .. })) => {}
            other => {
                panic!(
                    "粗筛命中后应得到 Err(Kernel(Geometry{{op:\"pick\",..}}))（数据不一致：枚举不出面），实得 {other:?}"
                )
            }
        }
    }

    /// occt 腿钉子 ①：**命中预期面，且与独立粗筛法向 / 面哈希一致**。
    ///
    /// 造一个立方体，从顶面中心正上方（由 mesh AABB 推出，**不写死原点假设**）
    /// 打一条竖直向下的射线。断言：
    /// - `Ok(Some(r))`、`r.kind==Face`、`r.element==期望元素`、`r.topo_hash != 0`；
    /// - `r.world_point ≈ 顶面中心`、`r.param` 为 `Face{u,v}` 且有限、`r.normal ≈ +Z`；
    /// - **独立正确性 oracle**：同一射线 / 同一 mesh 跑 [`ray_hits_mesh`]，`r.normal`
    ///   必须 ≈ `coarse.normal`（立方体不同面法向不同 → R16 / 映射错位必红）；
    ///   再由 `coarse.tri_index` 独立重算 `g`，断言 `r.topo_hash == face_hashes[g]`
    ///   （钉住 `tri → group → handle` 全链）。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn pick_hits_the_expected_face_with_matching_normal() {
        let mut g = GeometryService::new().expect("真实内核应可用");
        assert_eq!(
            g.backend_kind(),
            core_kernel::BackendKind::Real,
            "钉子必须跑在真实内核上，不许在 mock 上假绿"
        );

        let handle = g
            .kernel_mut()
            .make_box(10.0, 10.0, 10.0)
            .expect("真实内核建模应成功");
        let shape = ShapeId::new();
        g.cache(shape, handle);
        let mut doc = Document::new("d");
        let element = Element::new(Category::generic(), "box").with_geometry(shape);
        let element_id = element.id;
        doc.insert_element(element).expect("插入元素应成功");

        // 由 mesh AABB 求顶面中心（**不写死原点 / 尺寸假设**）。
        let mesh = g
            .kernel_mut()
            .tessellate(handle, PICK_LINEAR, PICK_ANGULAR)
            .expect("真实内核网格应可用");
        let mut mn = [f64::INFINITY; 3];
        let mut mx = [f64::NEG_INFINITY; 3];
        for p in &mesh.positions {
            for k in 0..3 {
                mn[k] = mn[k].min(p[k]);
                mx[k] = mx[k].max(p[k]);
            }
        }
        let cx = 0.5 * (mn[0] + mx[0]);
        let cy = 0.5 * (mn[1] + mx[1]);
        let ztop = mx[2];

        let ray = Ray {
            origin: DVec3::new(cx, cy, ztop + 100.0),
            direction: DVec3::NEG_Z,
        };
        let cam = FixedCamera(ray);
        // 拾取器整段 `&mut` 借用 `g`；用块把它圈住，出块即释放（后续要再用 `g`）。
        let r = {
            let mut p = KernelPicker::new(&mut g, &doc);
            p.pick((0.0, 0.0), &cam, PickFilter::faces_only())
                .expect("拾取不应报错")
        };
        let Some(r) = r else {
            panic!("顶面中心正上方的射线应命中，实得 None");
        };

        assert_eq!(r.kind, PickKind::Face, "应拾取到面");
        assert_eq!(r.element, element_id, "应命中持有的那个元素");
        assert_ne!(r.topo_hash, 0, "面哈希不应为 0");
        assert!(
            (r.world_point - DVec3::new(cx, cy, ztop)).length() < 1e-6,
            "命中点应≈顶面中心 ({cx},{cy},{ztop})，实得 {:?}",
            r.world_point
        );
        match r.param {
            PickParam::Face { u, v } => assert!(
                u.is_finite() && v.is_finite(),
                "面局部参数应有限，实得 u={u}, v={v}"
            ),
            other => panic!("面拾取的 param 应为 Face{{u,v}}，实得 {other:?}"),
        }
        let n = r
            .normal
            .expect("面法向应可得（真实内核 surface_normal 应 Ok）");
        assert!(
            (n - DVec3::Z).length() < 1e-6,
            "顶面**外**法向应为 +Z（射线自上而下打顶面），实得 {n:?}"
        );

        // ---- 独立 oracle：粗筛法向 + tri→group→handle 链 ----
        let coarse = ray_hits_mesh(&ray, &mesh.positions, &mesh.indices).expect("粗筛应命中");
        assert!(
            (n - coarse.normal).length() < 1e-6,
            "精算法向应≈粗筛三角形法向（防『精算到别的面』假绿）：n={n:?} vs coarse={:?}",
            coarse.normal
        );
        let gidx = mesh
            .face_groups
            .iter()
            .position(|fg| {
                let s = fg.tri_start as usize;
                coarse.tri_index >= s && coarse.tri_index < s + fg.tri_count as usize
            })
            .expect("粗筛三角形应属于某个面组");

        // 拾取器已出块释放，可再次 `&mut` 借内核取面哈希。
        let hashes = g.kernel_mut().face_hashes(handle).expect("面哈希应可得");
        assert!(gidx < hashes.len(), "重算的面组索引应在面哈希范围内");
        assert_eq!(
            r.topo_hash, hashes[gidx],
            "topo_hash 应等于按 g 独立重算的面哈希（钉住 tri→group→handle 同序链）"
        );
    }

    /// occt 腿钉子 ②：**远离模型的射线未命中 → `Ok(None)`，且不 panic**。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn pick_misses_outside_the_model_returns_none() {
        let mut g = GeometryService::new().expect("真实内核应可用");
        assert_eq!(g.backend_kind(), core_kernel::BackendKind::Real);
        let handle = g
            .kernel_mut()
            .make_box(10.0, 10.0, 10.0)
            .expect("建模应成功");
        let shape = ShapeId::new();
        g.cache(shape, handle);
        let doc = Document::new("d");
        let ray = Ray {
            origin: DVec3::new(1.0e6, 1.0e6, 1.0e6),
            direction: DVec3::NEG_Z,
        };
        let cam = FixedCamera(ray);
        let mut p = KernelPicker::new(&mut g, &doc);
        let got = p.pick((0.0, 0.0), &cam, PickFilter::faces_only());
        assert!(
            matches!(got, Ok(None)),
            "远离模型的射线应 Ok(None)（不 panic），实得 {got:?}"
        );
    }

    /// occt 腿钉子 ③（加强）：**空缓存 → `Ok(None)`**（不 panic、不报错）。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn pick_on_empty_cache_returns_none() {
        let mut g = GeometryService::new().expect("真实内核应可用");
        assert_eq!(g.backend_kind(), core_kernel::BackendKind::Real);
        let doc = Document::new("d");
        let ray = Ray {
            origin: DVec3::ZERO,
            direction: DVec3::NEG_Z,
        };
        let cam = FixedCamera(ray);
        let mut p = KernelPicker::new(&mut g, &doc);
        let got = p.pick((0.0, 0.0), &cam, PickFilter::faces_only());
        assert!(matches!(got, Ok(None)), "空缓存应 Ok(None)，实得 {got:?}");
    }

    /// occt 腿钉子 ④（加强，超出方案示例钉表）：**跨形状取全局最近 + 元素按命中形状反查**。
    ///
    /// 为让两处"顺序类"缺陷**确定性**打红，显式控制迭代顺序：
    /// 同一竖直射线上叠放两个立方体（矮的 `z∈[0,10]`、高的 `z∈[0,20]`，共享 x,y 足迹），
    /// 相机在正上方 → **高的更近**。再据 `cached_ids()`（`BTreeMap<ShapeId>` 升序）与
    /// `elements()`（`BTreeMap<ElementId>` 升序）把**较远的形状**与**错误的元素**安排到
    /// **较小 id**（=先被遍历）。于是：
    /// - 变异"取首个形状" → 稳定选中较远形状 → 期望元素不符 → 红；
    /// - 变异"元素反查返回首个" → 稳定返回错误元素 → 红。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn pick_returns_the_nearest_shape_and_its_element() {
        let mut g = GeometryService::new().expect("真实内核应可用");
        assert_eq!(g.backend_kind(), core_kernel::BackendKind::Real);

        // 两个形状：矮(z∈[0,10]，远) 与 高(z∈[0,20]，近)。
        let b_far = g
            .kernel_mut()
            .make_box(10.0, 10.0, 10.0)
            .expect("建模应成功");
        let b_near = g
            .kernel_mut()
            .make_box(10.0, 10.0, 20.0)
            .expect("建模应成功");
        // 形状 id 升序 → 先遍历者须是**较远**的矮盒。
        let (id_far, id_near) = {
            let a = ShapeId::new();
            let b = ShapeId::new();
            if a < b { (a, b) } else { (b, a) }
        };
        g.cache(id_far, b_far);
        g.cache(id_near, b_near);

        // 两个元素：先建后按 id 排序，把**错误**（绑远）那个安排成较小 id。
        let e1 = Element::new(Category::generic(), "far");
        let e2 = Element::new(Category::generic(), "near");
        let (e_low, e_high) = if e1.id < e2.id { (e1, e2) } else { (e2, e1) };
        let e_wrong = e_low.with_geometry(id_far); // 较小 id → 先遍历 → 错
        let e_right = e_high.with_geometry(id_near); // 较大 id → 后遍历 → 对
        let expected = e_right.id;
        let mut doc = Document::new("d");
        doc.insert_element(e_wrong).expect("插入元素应成功");
        doc.insert_element(e_right).expect("插入元素应成功");

        // 正上方竖直向下：应命中**高盒**顶面 (5,5,20)。
        let ray = Ray {
            origin: DVec3::new(5.0, 5.0, 200.0),
            direction: DVec3::NEG_Z,
        };
        let cam = FixedCamera(ray);
        let mut p = KernelPicker::new(&mut g, &doc);
        let got = p.pick((0.0, 0.0), &cam, PickFilter::faces_only());
        let Some(r) = got.expect("拾取不应报错") else {
            panic!("正上方射线应命中高盒，实得 None");
        };

        assert_eq!(
            r.element, expected,
            "应命中**较近**（高）盒所绑元素；取首个形状 / 元素反查取首个都会打红此断言"
        );
        assert!(
            (r.world_point - DVec3::new(5.0, 5.0, 20.0)).length() < 1e-6,
            "命中点应≈高盒顶面 (5,5,20)，实得 {:?}（若为 z≈10 则说明选中了较远矮盒）",
            r.world_point
        );
        assert_eq!(r.kind, PickKind::Face);
        assert_ne!(r.topo_hash, 0);
    }

    /// occt 腿钉子 ⑤：**粗筛命中但无元素引用该形状 → `Ok(None)`**（几何孤儿不可假装成元素）。
    ///
    /// 覆盖 `pick` 的**元素孤儿分支**（`elements().find(geometry_ref==shape)` 落空）。
    /// 真内核造一个被射线命中的盒子（**登记缓存但不装元素**），并往文档塞一个**绑到别的
    /// shape**的元素（该 shape **未登记缓存** → 不在粗筛路径上、也不会被射线看到）。
    /// 于是粗筛确实命中，但反查不到引用该形状的元素 → 必须 `Ok(None)`；**绝不**退化为
    /// "返回文档里第一个元素"（那会把几何孤儿错当成某个真实元素返回，属假命中）。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn pick_on_a_shape_without_an_element_returns_none() {
        let mut g = GeometryService::new().expect("真实内核应可用");
        assert_eq!(
            g.backend_kind(),
            core_kernel::BackendKind::Real,
            "钉子必须跑在真实内核上，不许在 mock 上假绿"
        );

        // 被命中的形状：登记缓存，但**不**装任何元素。
        let hit_handle = g
            .kernel_mut()
            .make_box(10.0, 10.0, 10.0)
            .expect("建模应成功");
        let hit_shape = ShapeId::new();
        g.cache(hit_shape, hit_handle);

        // 诱饵：文档里有一个元素，但其 `geometry_ref` 指向**另一个未登记缓存**的形状
        // → 反查 `hit_shape` 必然落空；同时"返回首个元素"型变异会错误返回这个诱饵。
        let decoy_shape = ShapeId::new();
        let mut doc = Document::new("d");
        let decoy = Element::new(Category::generic(), "decoy").with_geometry(decoy_shape);
        doc.insert_element(decoy).expect("插入元素应成功");

        // 由**被命中形状**的 mesh AABB 推顶面中心（不写死原点 / 尺寸），向上打竖直射线。
        let mesh = g
            .kernel_mut()
            .tessellate(hit_handle, PICK_LINEAR, PICK_ANGULAR)
            .expect("真实内核网格应可用");
        let mut mn = [f64::INFINITY; 3];
        let mut mx = [f64::NEG_INFINITY; 3];
        for p in &mesh.positions {
            for k in 0..3 {
                mn[k] = mn[k].min(p[k]);
                mx[k] = mx[k].max(p[k]);
            }
        }
        let cx = 0.5 * (mn[0] + mx[0]);
        let cy = 0.5 * (mn[1] + mx[1]);
        let ztop = mx[2];
        let ray = Ray {
            origin: DVec3::new(cx, cy, ztop + 100.0),
            direction: DVec3::NEG_Z,
        };
        // 独立确认这条射线**确实**命中该形状的粗筛网格（否则"命中后须 Ok(None)"前提不成立）。
        assert!(
            ray_hits_mesh(&ray, &mesh.positions, &mesh.indices).is_some(),
            "构造的射线应命中该形状的粗筛网格"
        );

        let cam = FixedCamera(ray);
        let got = {
            let mut p = KernelPicker::new(&mut g, &doc);
            p.pick((0.0, 0.0), &cam, PickFilter::faces_only())
        };

        assert!(
            !matches!(got, Ok(Some(_))),
            "命中的形状无元素引用（几何孤儿）→ 绝不可假装命中某个元素，实得 {got:?}"
        );
        assert!(
            matches!(got, Ok(None)),
            "粗筛命中但无对应元素应 Ok(None)，实得 {got:?}"
        );
    }
}

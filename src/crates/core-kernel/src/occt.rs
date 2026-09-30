//! 真实几何后端：`occt-wasm` v4（M0 交付物）。
//!
//! 本模块**只有在本文件内**才允许出现 `occt_wasm::` 类型（设计文档 1.2 铁律 1）。
//! 编译门控：`--features occt`（见 crate 根 `Cargo.toml` 的 `[features]`）。
//!
//! ## 源码已核实的事实（2026-09，`occt-wasm 4.0.0` 实测）
//!
//! | 事实 | 说明 |
//! |---|---|
//! | 运行时 | 内嵌 brotli 压缩的 **21 MB WASM**，经 **wasmtime 48** 原生 JIT 执行；**不需要浏览器** |
//! | 创建开销 | `OcctKernel::new()` 解压 + 编译，**约 100–500 ms**；单次调用很快 → 必须每线程复用（[`crate::pool`]） |
//! | debug 惩罚 | wasmtime 编译在 debug 下**慢约 100 倍** → M0 验证必须 `--release` |
//! | 句柄 | `occt_wasm::ShapeHandle(pub(crate) u32)`：**字段是 crate 私有**，外部**无法**从一个裸 `u32` 重建 |
//! | 无 `is_alive` | 官方不提供"句柄是否有效"查询（只有 `is_null`）→ 见下方"自有 arena" |
//! | 许可 | crate 元数据声明 **`MIT OR Apache-2.0`**（⚠ 与设计文档 1.4 记的 LGPL-2.1 不一致，见 README"许可待复核"） |
//!
//! ## 自有 arena（为什么需要）
//!
//! 因为 `occt_wasm::ShapeHandle` 无法从裸 `u32` 重建，本后端**不能**把内核句柄直接
//! 当作 [`ShapeHandle`] 往外传（那样 `fuse(a, b)` 就没有办法把它还原成内核句柄）。
//! 因此本后端维护一张槽位表：
//!
//! ```text
//! 对外 ShapeHandle(idx)  ──►  slots[idx] = Some(occt_wasm::ShapeHandle)
//! ```
//!
//! 顺带收益：`is_alive` / `live_count` / 重复释放检测都能在槽位表上实现
//! （内核自己不提供这三件事）。
//!
//! ## HLR 投影的展开
//!
//! 内核 `project_edges` 返回 6 个 **B-Rep compound**（可见/隐藏 × 轮廓/光顺/尖锐）。
//! 本后端把每个 compound 展开成边，逐边问 `curve_type` 得到**解析类型**
//! （这正是"M0 精确圆判定"要守的能力），再尽力提取几何参数（圆 → 圆心/半径）。

use occt_wasm::OcctKernel;

use crate::arena::SlotArena;
use crate::backend::{BackendKind, KernelBackend};
use crate::error::{KernelError, KernelResult};
use crate::handle::ShapeHandle;
use crate::types::{
    BoundingBox, CurveGeometry, CurveKind, CurveRange, DVec2, DVec3, Evolution, FaceGroup,
    LineClass, MeshData, PointOnEdge, PointOnFace, ProjectedCurve, Projection, SubShapeKind,
    ViewDef, decode_groups, dvec3_from_kernel_vec, dvec3_from_kernel_vec_op,
};

/// 面哈希的 `hash_upper_bound`：必须与**网格化器产出 `face_groups[].face_hash`
/// 时用的 bound 一致**，否则两边的哈希值集完全不相交，拾取粗筛永远不中。
///
/// 这不是猜的，是**实测**出来的：对 `make_box(10,10,10)` 的 6 个面，
/// `hash_code(face, i32::MAX)` 恰好等于 `mesh.face_groups` 里的 6 个 `face_hash`
/// （`[1652326887, 666922992, 1428387292, 620608512, 1116169211, 44387034]`，逐个相等且同序）；
/// 而用 `1_000_000` 会得到另一组值（`[326887, 922992, …]`）——两组**没有一个数相同**。
/// OCCT 的 `HashCode(upper)` 语义就是"结果落在 `[1, upper]`"，所以 bound 不同结果就不同。
pub const FACE_HASH_UPPER_BOUND: i32 = i32::MAX;

/// 把内核错误转成适配层错误（保留算子名，便于定位）。
fn op_err(op: &'static str) -> impl FnOnce(occt_wasm::OcctError) -> KernelError {
    move |e| KernelError::Geometry {
        op,
        msg: e.to_string(),
    }
}

/// 校验"有限正数"参数（与 [`crate::MockBackend`] 保持同一契约）。
fn positive(op: &'static str, name: &str, v: f64) -> KernelResult<f64> {
    if !v.is_finite() || v <= 0.0 {
        return Err(KernelError::InvalidArgument(format!(
            "{op}: {name} 必须为有限正数，实得 {v}"
        )));
    }
    Ok(v)
}

/// 校验"有限非负数"（允许 0，与占位后端同契约，如圆锥尖顶 `r2 = 0`）。
fn non_negative(op: &'static str, name: &str, v: f64) -> KernelResult<f64> {
    if !v.is_finite() || v < 0.0 {
        return Err(KernelError::InvalidArgument(format!(
            "{op}: {name} 必须为有限非负数，实得 {v}"
        )));
    }
    Ok(v)
}

/// 校验"有限数"（可正可负、可为 0），与占位后端同契约。
fn finite_arg(op: &'static str, name: &str, v: f64) -> KernelResult<f64> {
    if !v.is_finite() {
        return Err(KernelError::InvalidArgument(format!(
            "{op}: {name} 必须为有限数，实得 {v}"
        )));
    }
    Ok(v)
}

/// 校验旋转/回转轴：三分量有限且非零向量。
fn check_axis(op: &'static str, dx: f64, dy: f64, dz: f64) -> KernelResult<()> {
    let (dx, dy, dz) = (
        finite_arg(op, "dx", dx)?,
        finite_arg(op, "dy", dy)?,
        finite_arg(op, "dz", dz)?,
    );
    if dx * dx + dy * dy + dz * dz == 0.0 {
        return Err(KernelError::InvalidArgument(format!(
            "{op}: 旋转轴不能为零向量（dx=dy=dz=0）"
        )));
    }
    Ok(())
}

/// 真实 `occt-wasm` 后端。
pub struct OcctBackend {
    /// wasmtime 里的 OCCT 实例（**昂贵**，每线程只建一个）。
    kernel: OcctKernel,
    /// 槽位表：对外 `ShapeHandle.raw()` 是这里的下标，槽里存内核真实句柄。
    ///
    /// 用共享的 [`SlotArena`] 而不是自己写一份：两个后端的槽位语义必须一致，
    /// 否则上层缓存会在换后端时出现只在一边复现的怪 bug。
    arena: SlotArena<occt_wasm::ShapeHandle>,
}

impl OcctBackend {
    /// 创建真实后端。**耗时约 100–500 ms**（解压 + JIT 编译 21 MB WASM）。
    pub fn new() -> KernelResult<Self> {
        let kernel = OcctKernel::new().map_err(op_err("OcctKernel::new"))?;
        Ok(Self {
            kernel,
            arena: SlotArena::new(),
        })
    }

    /// 把一个内核句柄收进槽位表，返回对外句柄。
    fn alloc(&mut self, h: occt_wasm::ShapeHandle) -> ShapeHandle {
        self.arena.alloc(h)
    }

    /// 把对外句柄还原成内核句柄。
    fn resolve(&self, h: ShapeHandle) -> KernelResult<occt_wasm::ShapeHandle> {
        self.arena.get(h).copied()
    }

    /// 内核实例的只读访问（供本 crate 内的 M0 探针测试使用）。
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn kernel_mut(&mut self) -> &mut OcctKernel {
        &mut self.kernel
    }

    /// `occt_wasm::EvolutionData` → [`Evolution`]（**唯一的类型翻译点**）。
    ///
    /// 铁律①：`occt_wasm::` 类型只许出现在本文件，绝不外泄。要点：
    /// - `result` 用 B0 补好的 `occt_wasm::ShapeHandle::from_raw(evo.result_id)`
    ///   重建内核句柄，再收进自有 arena 得到对外 `ShapeHandle`；
    /// - `modified` / `generated` 是扁平 `Vec<i32>`，走 [`decode_groups`] 解成
    ///   `[输入面哈希, k, 输出面哈希×k]*` 分组（越界 / 负 k 即 `Err`，不静默截断）；
    /// - `deleted` 是"被删输入面哈希"的扁平表，逐个抬升为 `u64`（与 `face_hashes`
    ///   同一抬升口径）。
    fn convert_evolution(&mut self, evo: occt_wasm::EvolutionData) -> KernelResult<Evolution> {
        let result = self.alloc(occt_wasm::ShapeHandle::from_raw(evo.result_id));
        Ok(Evolution {
            result,
            modified: decode_groups(&evo.modified)?,
            generated: decode_groups(&evo.generated)?,
            deleted: evo.deleted.iter().map(|&v| u64::from(v as u32)).collect(),
        })
    }
}

/// `curve_type` 返回的类型名 → [`CurveKind`]。
///
/// ⚠ **顺序有讲究**：必须先判 `spline` 再判 `line`——因为 `"spline"` 里含有子串
/// `"line"`（`s-p-line`），顺序反了会把 B 样条误判成直线。
fn classify_curve(type_name: &str) -> CurveKind {
    let t = type_name.to_ascii_lowercase();
    if t.contains("circle") {
        CurveKind::Circle
    } else if t.contains("ellipse") {
        CurveKind::Ellipse
    } else if t.contains("spline") || t.contains("bezier") {
        CurveKind::BSpline
    } else if t.contains("arc") {
        CurveKind::Arc
    } else if t.contains("line") {
        CurveKind::Line
    } else {
        CurveKind::Other
    }
}

/// 取曲线上某参数处的二维点（丢 Z——投影结果落在投影平面内）。
fn point2(kernel: &mut OcctKernel, edge: occt_wasm::ShapeHandle, t: f64) -> Option<DVec2> {
    let v = kernel.curve_point_at_param(edge, t).ok()?;
    if v.len() >= 2 {
        Some(DVec2::new(v[0], v[1]))
    } else {
        None
    }
}

/// 三点定圆心（共线则返回 `None`）。
fn circumcenter(p1: DVec2, p2: DVec2, p3: DVec2) -> Option<(DVec2, f64)> {
    let d = 2.0 * (p1.x * (p2.y - p3.y) + p2.x * (p3.y - p1.y) + p3.x * (p1.y - p2.y));
    if d.abs() < 1e-12 {
        return None;
    }
    let (a, b, c) = (
        p1.length_squared(),
        p2.length_squared(),
        p3.length_squared(),
    );
    let ux = (a * (p2.y - p3.y) + b * (p3.y - p1.y) + c * (p1.y - p2.y)) / d;
    let uy = (a * (p3.x - p2.x) + b * (p1.x - p3.x) + c * (p2.x - p1.x)) / d;
    let center = DVec2::new(ux, uy);
    Some((center, (p1 - center).length()))
}

/// 尽力从一条边上提取解析几何；失败返回 `None`（外层退化为 [`CurveGeometry::Other`]）。
///
/// **注意**：这是 best-effort——`kind` 来自内核的 `curve_type`（可靠），
/// 几何参数靠采样拟合（圆/弧用三点定心）。M0 只强依赖 `kind`。
fn extract_geometry(
    kernel: &mut OcctKernel,
    edge: occt_wasm::ShapeHandle,
    kind: CurveKind,
) -> Option<CurveGeometry> {
    let range = kernel.curve_parameters(edge).ok()?;
    if range.len() < 2 {
        return None;
    }
    let (t0, t1) = (range[0], range[1]);
    match kind {
        CurveKind::Line => {
            let a = point2(kernel, edge, t0)?;
            let b = point2(kernel, edge, t1)?;
            Some(CurveGeometry::Line { a, b })
        }
        CurveKind::Circle | CurveKind::Arc => {
            let closed = kernel.curve_is_closed(edge).unwrap_or(false);
            let span = t1 - t0;
            let p1 = point2(kernel, edge, t0)?;
            let p2 = point2(
                kernel,
                edge,
                if closed {
                    t0 + span / 3.0
                } else {
                    t0 + span * 0.5
                },
            )?;
            let p3 = point2(
                kernel,
                edge,
                if closed { t0 + span * 2.0 / 3.0 } else { t1 },
            )?;
            let (center, radius) = circumcenter(p1, p2, p3)?;
            if kind == CurveKind::Circle || closed {
                Some(CurveGeometry::Circle { center, radius })
            } else {
                Some(CurveGeometry::Arc {
                    center,
                    radius,
                    start: (p1 - center).y.atan2((p1 - center).x),
                    end: (p3 - center).y.atan2((p3 - center).x),
                })
            }
        }
        // 椭圆 / B 样条：M0 不做参数反演，交给后续里程碑
        CurveKind::Ellipse | CurveKind::BSpline | CurveKind::Other => None,
    }
}

/// 展开一个投影 compound 的所有边，追加进 `out`。
///
/// **空分支是合法的**：HLR 返回的 6 个分支（visible/hidden × outline/smooth/sharp）
/// 大多为空，此时内核给的是 **id = 0 的占位句柄**。这类句柄必须先挡掉再查子形状，
/// 否则整条投影链路会被带崩。
///
/// ⚠ 判空只能看原始 `id`：内核的 arena 从 `1` 开始编号，`0` 是"无形状"哨兵，
/// 而它**连自己的 `is_null` 都不接受 0**——`is_null(ShapeHandle(0))` 会报
/// `Invalid shape ID: 0`（实测）。所以这里用 `id() == 0`，不用 `is_null`。
fn collect_branch(
    kernel: &mut OcctKernel,
    compound: occt_wasm::ShapeHandle,
    class: LineClass,
    out: &mut Vec<ProjectedCurve>,
) -> KernelResult<()> {
    if compound.id() == 0 {
        return Ok(());
    }
    let edges = kernel
        .get_sub_shapes(compound, "edge")
        .map_err(op_err("get_sub_shapes"))?;
    for e in edges {
        // 单条边取类型失败不应整体失败：退化为 Other，把其它边照常收进来
        let name = kernel.curve_type(e).unwrap_or_else(|_| "Other".to_string());
        let kind = classify_curve(&name);
        let data = extract_geometry(kernel, e, kind).unwrap_or(CurveGeometry::Other);
        out.push(ProjectedCurve {
            kind,
            data,
            layer_hint: class,
        });
    }
    Ok(())
}

/// 内核 `Mesh`（f32 交错缓冲）→ 适配层 [`MeshData`]（f64 / 分组结构）。
///
/// ⚠ **单位换算**：内核 `face_groups` 的三元组 `[tri_start, tri_count, face_hash]`
/// 里，`tri_start` / `tri_count` 是**索引**单位（实测长方体 6 个面各为
/// `[0,6,·] [6,6,·] … [30,6,·]`，跨度 36 = `indices.len()`），而本适配层的
/// [`FaceGroup`] 定义是**三角形**单位。这里必须 `/3`，换算不整除就报错——
/// 宁可响亮失败，也不要静默少算三角形。
fn convert_mesh(mesh: occt_wasm::Mesh) -> KernelResult<MeshData> {
    let positions = mesh
        .positions
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| [f64::from(c[0]), f64::from(c[1]), f64::from(c[2])])
        .collect();
    let normals = mesh
        .normals
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| [f64::from(c[0]), f64::from(c[1]), f64::from(c[2])])
        .collect();
    // face_groups = [tri_start, tri_count, face_hash] × n（**索引**单位，见函数文档）
    let mut face_groups = Vec::with_capacity(mesh.face_groups.len() / 3);
    for g in mesh.face_groups.as_chunks::<3>().0 {
        let raw_start = g[0].max(0) as u32;
        let raw_count = g[1].max(0) as u32;
        if !raw_start.is_multiple_of(3) || !raw_count.is_multiple_of(3) {
            return Err(KernelError::Geometry {
                op: "tessellate",
                msg: format!(
                    "面分组的索引区间无法无损换算成三角形单位：\
                     tri_start={raw_start}, tri_count={raw_count}（应为 3 的倍数）"
                ),
            });
        }
        face_groups.push(FaceGroup {
            tri_start: raw_start / 3,
            tri_count: raw_count / 3,
            // 面哈希按原始 i32 位型无损抬升；⚠ 跨会话不稳定，不可持久化
            face_hash: u64::from(g[2] as u32),
        });
    }
    Ok(MeshData {
        positions,
        indices: mesh.indices,
        normals,
        face_groups,
    })
}

impl KernelBackend for OcctBackend {
    fn name(&self) -> &'static str {
        "occt-wasm"
    }

    fn kind(&self) -> BackendKind {
        BackendKind::Real
    }

    fn make_box(&mut self, dx: f64, dy: f64, dz: f64) -> KernelResult<ShapeHandle> {
        let (dx, dy, dz) = (
            positive("make_box", "dx", dx)?,
            positive("make_box", "dy", dy)?,
            positive("make_box", "dz", dz)?,
        );
        let h = self
            .kernel
            .make_box(dx, dy, dz)
            .map_err(op_err("make_box"))?;
        Ok(self.alloc(h))
    }

    fn make_sphere(&mut self, r: f64) -> KernelResult<ShapeHandle> {
        let r = positive("make_sphere", "r", r)?;
        let h = self.kernel.make_sphere(r).map_err(op_err("make_sphere"))?;
        Ok(self.alloc(h))
    }

    fn make_cylinder(&mut self, r: f64, h: f64) -> KernelResult<ShapeHandle> {
        let (r, h) = (
            positive("make_cylinder", "r", r)?,
            positive("make_cylinder", "h", h)?,
        );
        let handle = self
            .kernel
            .make_cylinder(r, h)
            .map_err(op_err("make_cylinder"))?;
        Ok(self.alloc(handle))
    }

    fn make_cone(&mut self, r1: f64, r2: f64, h: f64) -> KernelResult<ShapeHandle> {
        let r1 = non_negative("make_cone", "r1", r1)?;
        let r2 = non_negative("make_cone", "r2", r2)?;
        let h = positive("make_cone", "h", h)?;
        let handle = self
            .kernel
            .make_cone(r1, r2, h)
            .map_err(op_err("make_cone"))?;
        Ok(self.alloc(handle))
    }

    fn make_torus(&mut self, major: f64, minor: f64) -> KernelResult<ShapeHandle> {
        let major = positive("make_torus", "major", major)?;
        let minor = positive("make_torus", "minor", minor)?;
        let handle = self
            .kernel
            .make_torus(major, minor)
            .map_err(op_err("make_torus"))?;
        Ok(self.alloc(handle))
    }

    fn extrude(
        &mut self,
        shape: ShapeHandle,
        dx: f64,
        dy: f64,
        dz: f64,
    ) -> KernelResult<ShapeHandle> {
        let (dx, dy, dz) = (
            finite_arg("extrude", "dx", dx)?,
            finite_arg("extrude", "dy", dy)?,
            finite_arg("extrude", "dz", dz)?,
        );
        let k = self.resolve(shape)?;
        let h = self
            .kernel
            .extrude(k, dx, dy, dz)
            .map_err(op_err("extrude"))?;
        Ok(self.alloc(h))
    }

    #[allow(clippy::too_many_arguments)]
    fn revolve(
        &mut self,
        shape: ShapeHandle,
        px: f64,
        py: f64,
        pz: f64,
        dx: f64,
        dy: f64,
        dz: f64,
        angle_rad: f64,
    ) -> KernelResult<ShapeHandle> {
        for (name, v) in [("px", px), ("py", py), ("pz", pz)] {
            finite_arg("revolve", name, v)?;
        }
        check_axis("revolve", dx, dy, dz)?;
        finite_arg("revolve", "angle_rad", angle_rad)?;
        let k = self.resolve(shape)?;
        let h = self
            .kernel
            .revolve(k, px, py, pz, dx, dy, dz, angle_rad)
            .map_err(op_err("revolve"))?;
        Ok(self.alloc(h))
    }

    #[allow(clippy::too_many_arguments)]
    fn rotate(
        &mut self,
        shape: ShapeHandle,
        px: f64,
        py: f64,
        pz: f64,
        dx: f64,
        dy: f64,
        dz: f64,
        angle_rad: f64,
    ) -> KernelResult<ShapeHandle> {
        for (name, v) in [("px", px), ("py", py), ("pz", pz)] {
            finite_arg("rotate", name, v)?;
        }
        check_axis("rotate", dx, dy, dz)?;
        finite_arg("rotate", "angle_rad", angle_rad)?;
        let k = self.resolve(shape)?;
        let h = self
            .kernel
            .rotate(k, px, py, pz, dx, dy, dz, angle_rad)
            .map_err(op_err("rotate"))?;
        Ok(self.alloc(h))
    }

    fn bounding_box(&mut self, shape: ShapeHandle) -> KernelResult<BoundingBox> {
        let k = self.resolve(shape)?;
        // `use_triangulation = false`：用精确 B-Rep 包围盒，不靠三角网格近似。
        let bb = self
            .kernel
            .get_bounding_box(k, false)
            .map_err(op_err("get_bounding_box"))?;
        // 铁律 1：`occt_wasm::BoundingBox` / `Vec3` **不得**出本文件——在这里转成适配层类型。
        Ok(BoundingBox::new(
            [bb.min.x, bb.min.y, bb.min.z],
            [bb.max.x, bb.max.y, bb.max.z],
        ))
    }

    fn surface_area(&mut self, shape: ShapeHandle) -> KernelResult<f64> {
        let k = self.resolve(shape)?;
        self.kernel
            .get_surface_area(k)
            .map_err(op_err("get_surface_area"))
    }

    fn sub_shapes(
        &mut self,
        shape: ShapeHandle,
        kind: SubShapeKind,
    ) -> KernelResult<Vec<ShapeHandle>> {
        let k = self.resolve(shape)?;
        let raw = self
            .kernel
            .get_sub_shapes(k, kind.tag())
            .map_err(op_err("get_sub_shapes"))?;
        Ok(raw.into_iter().map(|h| self.alloc(h)).collect())
    }

    fn sub_shape_count(&mut self, shape: ShapeHandle, kind: SubShapeKind) -> KernelResult<usize> {
        let k = self.resolve(shape)?;
        let n = self
            .kernel
            .sub_shape_count(k, kind.tag())
            .map_err(op_err("sub_shape_count"))?;
        Ok(n.max(0) as usize)
    }

    fn fuse(&mut self, a: ShapeHandle, b: ShapeHandle) -> KernelResult<ShapeHandle> {
        let (ka, kb) = (self.resolve(a)?, self.resolve(b)?);
        let h = self.kernel.fuse(ka, kb).map_err(op_err("fuse"))?;
        Ok(self.alloc(h))
    }

    fn cut(&mut self, a: ShapeHandle, b: ShapeHandle) -> KernelResult<ShapeHandle> {
        let (ka, kb) = (self.resolve(a)?, self.resolve(b)?);
        let h = self.kernel.cut(ka, kb).map_err(op_err("cut"))?;
        Ok(self.alloc(h))
    }

    fn common(&mut self, a: ShapeHandle, b: ShapeHandle) -> KernelResult<ShapeHandle> {
        let (ka, kb) = (self.resolve(a)?, self.resolve(b)?);
        let h = self.kernel.common(ka, kb).map_err(op_err("common"))?;
        Ok(self.alloc(h))
    }

    fn fuse_with_history(
        &mut self,
        a: ShapeHandle,
        b: ShapeHandle,
        input_face_hashes: &[u64],
    ) -> KernelResult<Evolution> {
        let (ka, kb) = (self.resolve(a)?, self.resolve(b)?);
        // 域已在 face_hashes 里定死；这里仅做**无损窄化**
        // （哈希 ∈ [1, i32::MAX]，正数，故 `as i32` 位型无损）。
        let hashes: Vec<i32> = input_face_hashes.iter().map(|&h| h as i32).collect();
        let evo = self
            .kernel
            .fuse_with_history(ka, kb, &hashes, FACE_HASH_UPPER_BOUND)
            .map_err(op_err("fuse_with_history"))?;
        self.convert_evolution(evo)
    }

    fn cut_with_history(
        &mut self,
        a: ShapeHandle,
        b: ShapeHandle,
        input_face_hashes: &[u64],
    ) -> KernelResult<Evolution> {
        let (ka, kb) = (self.resolve(a)?, self.resolve(b)?);
        let hashes: Vec<i32> = input_face_hashes.iter().map(|&h| h as i32).collect();
        let evo = self
            .kernel
            .cut_with_history(ka, kb, &hashes, FACE_HASH_UPPER_BOUND)
            .map_err(op_err("cut_with_history"))?;
        self.convert_evolution(evo)
    }

    // ---- 修复类算子（M4 heal ops）-------------------------------------------

    /// **缝合（成壳）**。vendor `sew` 吃一个形状数组，这里只传单元素。
    ///
    /// ⚠ 实测语义：产出的是**壳 / 复合体**（`solid_count == 0`，体积 = 各输入之和）。
    /// 要拿单一实体请走 [`sew_and_solidify`](KernelBackend::sew_and_solidify)。
    fn sew(&mut self, shape: ShapeHandle, tolerance: f64) -> KernelResult<ShapeHandle> {
        let _tol = non_negative("sew", "tolerance", tolerance)?;
        let k = self.resolve(shape)?;
        let h = self.kernel.sew(&[k], tolerance).map_err(op_err("sew"))?;
        Ok(self.alloc(h))
    }

    /// **缝合并实体化**。vendor `sew_and_solidify` 吃一个面数组，这里只传单元素。
    fn sew_and_solidify(
        &mut self,
        shape: ShapeHandle,
        tolerance: f64,
    ) -> KernelResult<ShapeHandle> {
        let _tol = non_negative("sew_and_solidify", "tolerance", tolerance)?;
        let k = self.resolve(shape)?;
        let h = self
            .kernel
            .sew_and_solidify(&[k], tolerance)
            .map_err(op_err("sew_and_solidify"))?;
        Ok(self.alloc(h))
    }

    fn fix_shape(&mut self, shape: ShapeHandle) -> KernelResult<ShapeHandle> {
        let k = self.resolve(shape)?;
        let h = self.kernel.fix_shape(k).map_err(op_err("fix_shape"))?;
        Ok(self.alloc(h))
    }

    fn unify_same_domain(&mut self, shape: ShapeHandle) -> KernelResult<ShapeHandle> {
        let k = self.resolve(shape)?;
        let h = self
            .kernel
            .unify_same_domain(k)
            .map_err(op_err("unify_same_domain"))?;
        Ok(self.alloc(h))
    }

    // ---- 八算子门面（M5-B6.2a）-----------------------------------------------

    /// **圆角**：`radius` 须有限非负；解析实体 + 各边句柄后交 vendor `fillet`
    /// （`kernel_generated.rs:877`）。返回新句柄，输入句柄仍有效。
    fn fillet(
        &mut self,
        solid: ShapeHandle,
        edges: &[ShapeHandle],
        radius: f64,
    ) -> KernelResult<ShapeHandle> {
        let _r = non_negative("fillet", "radius", radius)?;
        let k = self.resolve(solid)?;
        let kedges = edges
            .iter()
            .map(|&e| self.resolve(e))
            .collect::<KernelResult<Vec<_>>>()?;
        let h = self
            .kernel
            .fillet(k, &kedges, radius)
            .map_err(op_err("fillet"))?;
        Ok(self.alloc(h))
    }

    /// **倒角**：`distance` 须有限非负；vendor `chamfer`（`:899`）。
    fn chamfer(
        &mut self,
        solid: ShapeHandle,
        edges: &[ShapeHandle],
        distance: f64,
    ) -> KernelResult<ShapeHandle> {
        let _d = non_negative("chamfer", "distance", distance)?;
        let k = self.resolve(solid)?;
        let kedges = edges
            .iter()
            .map(|&e| self.resolve(e))
            .collect::<KernelResult<Vec<_>>>()?;
        let h = self
            .kernel
            .chamfer(k, &kedges, distance)
            .map_err(op_err("chamfer"))?;
        Ok(self.alloc(h))
    }

    /// **抽壳**：`thickness` / `tolerance` 须有限非负；vendor `shell`（`:980`）。
    fn shell(
        &mut self,
        solid: ShapeHandle,
        faces: &[ShapeHandle],
        thickness: f64,
        tolerance: f64,
    ) -> KernelResult<ShapeHandle> {
        let _t = non_negative("shell", "thickness", thickness)?;
        let _tol = non_negative("shell", "tolerance", tolerance)?;
        let k = self.resolve(solid)?;
        let kfaces = faces
            .iter()
            .map(|&f| self.resolve(f))
            .collect::<KernelResult<Vec<_>>>()?;
        let h = self
            .kernel
            .shell(k, &kfaces, thickness, tolerance)
            .map_err(op_err("shell"))?;
        Ok(self.alloc(h))
    }

    /// **偏置**：`distance` / `tolerance` 须有限非负；vendor `offset`（`:1009`）。
    fn offset(
        &mut self,
        shape: ShapeHandle,
        distance: f64,
        tolerance: f64,
    ) -> KernelResult<ShapeHandle> {
        let _d = non_negative("offset", "distance", distance)?;
        let _tol = non_negative("offset", "tolerance", tolerance)?;
        let k = self.resolve(shape)?;
        let h = self
            .kernel
            .offset(k, distance, tolerance)
            .map_err(op_err("offset"))?;
        Ok(self.alloc(h))
    }

    /// **加厚**：`thickness` / `tolerance` 须有限非负；vendor `thicken`（`:1046`）。
    ///
    /// ⚠ 输入是**面 / 壳**（不是实体）。
    fn thicken(
        &mut self,
        shape: ShapeHandle,
        thickness: f64,
        tolerance: f64,
    ) -> KernelResult<ShapeHandle> {
        let _t = non_negative("thicken", "thickness", thickness)?;
        let _tol = non_negative("thicken", "tolerance", tolerance)?;
        let k = self.resolve(shape)?;
        let h = self
            .kernel
            .thicken(k, thickness, tolerance)
            .map_err(op_err("thicken"))?;
        Ok(self.alloc(h))
    }

    /// **放样**：`wires.len()` 须 **≥ 2**（否则 [`KernelError::InvalidArgument`]）；
    /// vendor `loft`（`:3042`）。
    fn loft(
        &mut self,
        wires: &[ShapeHandle],
        is_solid: bool,
        ruled: bool,
    ) -> KernelResult<ShapeHandle> {
        if wires.len() < 2 {
            return Err(KernelError::InvalidArgument(format!(
                "loft: 放样至少需要 2 个截面 wire，实得 {}",
                wires.len()
            )));
        }
        let kwires = wires
            .iter()
            .map(|&w| self.resolve(w))
            .collect::<KernelResult<Vec<_>>>()?;
        let h = self
            .kernel
            .loft(&kwires, is_solid, ruled)
            .map_err(op_err("loft"))?;
        Ok(self.alloc(h))
    }

    /// **扫掠**：vendor `sweep`（`:3100`）。`transition_mode` 直通 i32。
    fn sweep(
        &mut self,
        profile: ShapeHandle,
        spine: ShapeHandle,
        transition_mode: i32,
    ) -> KernelResult<ShapeHandle> {
        let kp = self.resolve(profile)?;
        let ks = self.resolve(spine)?;
        let h = self
            .kernel
            .sweep(kp, ks, transition_mode)
            .map_err(op_err("sweep"))?;
        Ok(self.alloc(h))
    }

    /// **管道**：vendor `pipe`（`:2988`）。
    fn pipe(&mut self, profile: ShapeHandle, spine: ShapeHandle) -> KernelResult<ShapeHandle> {
        let kp = self.resolve(profile)?;
        let ks = self.resolve(spine)?;
        let h = self.kernel.pipe(kp, ks).map_err(op_err("pipe"))?;
        Ok(self.alloc(h))
    }

    /// **直边**：vendor `make_line_edge`（`:1716`）直通；6 坐标 `finite_arg` 校验
    /// （照 `translate` 模式）。
    fn make_line_edge(
        &mut self,
        x1: f64,
        y1: f64,
        z1: f64,
        x2: f64,
        y2: f64,
        z2: f64,
    ) -> KernelResult<ShapeHandle> {
        for (name, v) in [
            ("x1", x1),
            ("y1", y1),
            ("z1", z1),
            ("x2", x2),
            ("y2", y2),
            ("z2", z2),
        ] {
            finite_arg("make_line_edge", name, v)?;
        }
        let h = self
            .kernel
            .make_line_edge(x1, y1, z1, x2, y2, z2)
            .map_err(op_err("make_line_edge"))?;
        Ok(self.alloc(h))
    }

    /// **三点弧**：vendor `make_arc_edge`（`:1780`）直通；9 坐标 `finite_arg` 校验；
    /// 三点共线时内核报错，**原样透传，不做预判**。
    #[allow(clippy::too_many_arguments)]
    fn make_arc_edge(
        &mut self,
        x1: f64,
        y1: f64,
        z1: f64,
        x2: f64,
        y2: f64,
        z2: f64,
        x3: f64,
        y3: f64,
        z3: f64,
    ) -> KernelResult<ShapeHandle> {
        for (name, v) in [
            ("x1", x1),
            ("y1", y1),
            ("z1", z1),
            ("x2", x2),
            ("y2", y2),
            ("z2", z2),
            ("x3", x3),
            ("y3", y3),
            ("z3", z3),
        ] {
            finite_arg("make_arc_edge", name, v)?;
        }
        let h = self
            .kernel
            .make_arc_edge(x1, y1, z1, x2, y2, z2, x3, y3, z3)
            .map_err(op_err("make_arc_edge"))?;
        Ok(self.alloc(h))
    }

    /// **线框**：vendor `make_wire`（`:1625`）直通；`edges` 空表 →
    /// [`KernelError::InvalidArgument`] **响亮拒绝**（照 `loft` 空截面模式）。
    fn make_wire(&mut self, edges: &[ShapeHandle]) -> KernelResult<ShapeHandle> {
        if edges.is_empty() {
            return Err(KernelError::InvalidArgument(
                "make_wire: 边集为空，至少需要 1 条边".to_string(),
            ));
        }
        let kedges = edges
            .iter()
            .map(|&e| self.resolve(e))
            .collect::<KernelResult<Vec<_>>>()?;
        let h = self
            .kernel
            .make_wire(&kedges)
            .map_err(op_err("make_wire"))?;
        Ok(self.alloc(h))
    }

    /// **由 wire 造面**：vendor `make_face`（`:1642`）直通。
    fn make_face(&mut self, wire: ShapeHandle) -> KernelResult<ShapeHandle> {
        let kw = self.resolve(wire)?;
        let h = self.kernel.make_face(kw).map_err(op_err("make_face"))?;
        Ok(self.alloc(h))
    }

    /// **提取外环**：vendor `outer_wire`（`:2547`）直通。
    fn outer_wire(&mut self, face: ShapeHandle) -> KernelResult<ShapeHandle> {
        let kf = self.resolve(face)?;
        let h = self.kernel.outer_wire(kf).map_err(op_err("outer_wire"))?;
        Ok(self.alloc(h))
    }

    fn translate(
        &mut self,
        shape: ShapeHandle,
        dx: f64,
        dy: f64,
        dz: f64,
    ) -> KernelResult<ShapeHandle> {
        for (name, v) in [("dx", dx), ("dy", dy), ("dz", dz)] {
            if !v.is_finite() {
                return Err(KernelError::InvalidArgument(format!(
                    "translate: {name} 必须为有限数，实得 {v}"
                )));
            }
        }
        let k = self.resolve(shape)?;
        let h = self
            .kernel
            .translate(k, dx, dy, dz)
            .map_err(op_err("translate"))?;
        Ok(self.alloc(h))
    }

    fn tessellate(
        &mut self,
        shape: ShapeHandle,
        linear: f64,
        angular: f64,
    ) -> KernelResult<MeshData> {
        let k = self.resolve(shape)?;
        let mesh = self
            .kernel
            .tessellate(k, linear, angular)
            .map_err(op_err("tessellate"))?;
        convert_mesh(mesh)
    }

    fn face_hashes(&mut self, shape: ShapeHandle) -> KernelResult<Vec<u64>> {
        let k = self.resolve(shape)?;
        // 与 convert_mesh 里同一个域、同一个抬升口径（i32 位型无损 → u64）。
        let hashes = self
            .kernel
            .sub_shape_hashes(k, "face", FACE_HASH_UPPER_BOUND)
            .map_err(op_err("sub_shape_hashes"))?;
        Ok(hashes.into_iter().map(|v| u64::from(v as u32)).collect())
    }

    // ---- 拾取：点投影（M5-B4）------------------------------------------------

    /// 真实内核的点到面投影：门面 handle → vendor handle → 6 元校验映射。
    fn project_point_on_face(&mut self, face: ShapeHandle, p: DVec3) -> KernelResult<PointOnFace> {
        let k = self.resolve(face)?;
        let v = self
            .kernel
            .project_point_on_face(k, p.x, p.y, p.z)
            .map_err(op_err("project_point_on_face"))?;
        PointOnFace::from_kernel_vec(&v)
    }

    /// 真实内核的点到边投影：门面 handle → vendor handle → 7 元校验映射。
    fn project_point_on_edge(&mut self, edge: ShapeHandle, p: DVec3) -> KernelResult<PointOnEdge> {
        let k = self.resolve(edge)?;
        let v = self
            .kernel
            .project_point_on_edge(k, p.x, p.y, p.z)
            .map_err(op_err("project_point_on_edge"))?;
        PointOnEdge::from_kernel_vec(&v)
    }

    /// 真实内核的面法向：门面 handle → vendor handle → 3 元校验映射。
    fn surface_normal(&mut self, face: ShapeHandle, u: f64, v: f64) -> KernelResult<DVec3> {
        let k = self.resolve(face)?;
        let n = self
            .kernel
            .surface_normal(k, u, v)
            .map_err(op_err("surface_normal"))?;
        dvec3_from_kernel_vec(&n)
    }

    /// 真实内核的曲线参数域：门面 handle → vendor handle → 2 元校验映射。
    fn curve_parameters(&mut self, curve: ShapeHandle) -> KernelResult<CurveRange> {
        let k = self.resolve(curve)?;
        let r = self
            .kernel
            .curve_parameters(k)
            .map_err(op_err("curve_parameters"))?;
        CurveRange::from_kernel_vec(&r)
    }

    /// 真实内核的按参数取点：门面 handle → vendor handle → 3 元校验映射。
    ///
    /// 包 vendor `curve_point_at_param`（`kernel_generated.rs:2681`）。用**带 `op`
    /// 的 3 元守卫**（[`dvec3_from_kernel_vec_op`]），使错误标签为
    /// `curve_point_at_param` 而非误报成 `surface_normal`。
    fn curve_point_at_param(&mut self, edge: ShapeHandle, param: f64) -> KernelResult<DVec3> {
        let k = self.resolve(edge)?;
        let v = self
            .kernel
            .curve_point_at_param(k, param)
            .map_err(op_err("curve_point_at_param"))?;
        dvec3_from_kernel_vec_op(&v, "curve_point_at_param")
    }

    /// 真实内核的曲线几何长度：门面 handle → vendor handle。
    ///
    /// 包 vendor `curve_length`（`kernel_generated.rs:2725`，标量，无元数守卫）。
    fn curve_length(&mut self, edge: ShapeHandle) -> KernelResult<f64> {
        let k = self.resolve(edge)?;
        self.kernel.curve_length(k).map_err(op_err("curve_length"))
    }

    fn project_edges(&mut self, shape: ShapeHandle, view: ViewDef) -> KernelResult<Projection> {
        let k = self.resolve(shape)?;
        // 未给屏幕右轴时传零向量，并置 has_x_axis=false 让内核自选
        let axis = view.x_axis.unwrap_or(DVec3::ZERO);
        let has_x_axis = view.x_axis.is_some();

        // 投影过程中内核会新建 compound + 边；用 checkpoint/release_since 圈住它们，
        // 提取完即回收，避免内核 arena 无限增长。
        let mark = self.kernel.checkpoint().ok();

        let proj = self
            .kernel
            .project_edges(
                k,
                view.origin.x,
                view.origin.y,
                view.origin.z,
                view.direction.x,
                view.direction.y,
                view.direction.z,
                axis.x,
                axis.y,
                axis.z,
                has_x_axis,
            )
            .map_err(op_err("project_edges"))?;

        let mut visible = Vec::new();
        let mut hidden = Vec::new();
        collect_branch(
            &mut self.kernel,
            proj.visible_outline,
            LineClass::Outline,
            &mut visible,
        )?;
        collect_branch(
            &mut self.kernel,
            proj.visible_smooth,
            LineClass::Smooth,
            &mut visible,
        )?;
        collect_branch(
            &mut self.kernel,
            proj.visible_sharp,
            LineClass::Sharp,
            &mut visible,
        )?;
        collect_branch(
            &mut self.kernel,
            proj.hidden_outline,
            LineClass::Outline,
            &mut hidden,
        )?;
        collect_branch(
            &mut self.kernel,
            proj.hidden_smooth,
            LineClass::Smooth,
            &mut hidden,
        )?;
        collect_branch(
            &mut self.kernel,
            proj.hidden_sharp,
            LineClass::Sharp,
            &mut hidden,
        )?;

        if let Some(m) = mark {
            // 尽力回收投影临时物；失败不影响结果
            let _ = self.kernel.release_since(m);
        }

        Ok(Projection { visible, hidden })
    }

    fn import_brep(&mut self, text: &str) -> KernelResult<ShapeHandle> {
        let h = self.kernel.from_brep(text).map_err(op_err("from_brep"))?;
        Ok(self.alloc(h))
    }

    fn export_brep(&mut self, shape: ShapeHandle) -> KernelResult<String> {
        let k = self.resolve(shape)?;
        self.kernel.to_brep(k).map_err(op_err("to_brep"))
    }

    fn volume(&mut self, shape: ShapeHandle) -> KernelResult<f64> {
        let k = self.resolve(shape)?;
        self.kernel.get_volume(k).map_err(op_err("get_volume"))
    }

    fn is_alive(&self, shape: ShapeHandle) -> bool {
        self.arena.is_alive(shape)
    }

    fn release(&mut self, shape: ShapeHandle) -> KernelResult<()> {
        // 先让自己在**对外语义上**失效（take 会归还槽位并检测重复释放），
        // 再通知内核。这样即使内核侧释放失败，也不会出现"内核已释放、
        // 但对外仍可解析"的错位状态。
        let k = self.arena.take(shape)?;
        self.kernel.release(k).map_err(op_err("release"))
    }

    fn live_count(&self) -> usize {
        self.arena.live_count()
    }

    fn live_handles(&self) -> Vec<ShapeHandle> {
        self.arena.live_handles()
    }
}

/// **仅供测试**：把 arena 清回空状态（内核侧 `release_all` + 清空槽位表）。
///
/// M0 探针用例共享一个内核实例（理由见 `tests::SHARED`）。但不少探针用例
/// **故意不释放**句柄（要留着观察拓扑、面分组、剖面），若不清空，
/// `live_count()` 与 `arena_reuses_freed_slots` 这类断言就会随用例执行顺序漂移。
/// 这里让每个用例都从"干净 arena"出发，等价于旧版"每个用例各建一个内核"的语义。
#[cfg(test)]
impl OcctBackend {
    fn reset_for_test(&mut self) {
        self.arena.clear();
        let _ = self.kernel.release_all();
    }
}

// =============================================================================
// 测试专用：全 crate 唯一的内核后端（occt 探针与 known_upstream_defects 共用）
// =============================================================================
//
// **为什么必须唯一**：`OcctKernel::new()` 要 JIT 编译 ~21 MB WASM。每个测试模块
// 各建一份会把进程撑爆——实测报 `memory allocation ... failed`，进程以
// `0xc0000409 (STATUS_STACK_BUFFER_OVERRUN)` 中止（日志里**没有** FAILED 行，
// 极易误判成代码回归）。故本 crate 的**所有** `#[cfg(test)]` 模块一律经由
// [`shared_test_backend`] 取得同一实例。

/// 全 crate 唯一的内核后端实例。
#[cfg(test)]
static SHARED_BACKEND: std::sync::OnceLock<Option<std::sync::Mutex<OcctBackend>>> =
    std::sync::OnceLock::new();

/// 首次加载失败的原因（供门禁用例给出可读信息）。
#[cfg(test)]
static BACKEND_LOAD_ERROR: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// 借用**全 crate 唯一**的共享内核后端（测试专用）。
///
/// **不做 debug 跳过**（那是 [`tests::try_backend`] 的职责）：本函数始终真正
/// 初始化并返回内核，供"必须真跑真断言"的钉子（如 `known_upstream_defects`）使用。
/// 内核不可用时返回 `None`，原因记入 [`backend_load_error`]。
///
/// 每次取得后都 `reset_for_test`，使每个用例从干净 arena 出发。
#[cfg(test)]
pub(crate) fn shared_test_backend() -> Option<std::sync::MutexGuard<'static, OcctBackend>> {
    use std::sync::PoisonError;
    let cell = SHARED_BACKEND.get_or_init(|| match OcctBackend::new() {
        Ok(b) => Some(std::sync::Mutex::new(b)),
        Err(e) => {
            let _ = BACKEND_LOAD_ERROR.set(e.to_string());
            eprintln!("⚠ 跳过真实内核用例：内核不可用（{e}）");
            None
        }
    });
    let mutex = cell.as_ref()?;
    // 用例 panic 会毒化锁。**故意吃下毒化**：真实内核是外部依赖，一个用例失败
    // 不该让其余用例集体报 PoisonError，把真实原因盖掉。
    let mut guard = mutex.lock().unwrap_or_else(PoisonError::into_inner);
    guard.reset_for_test();
    Some(guard)
}

/// 内核为何加载不了（没有失败过则为 `None`）。
#[cfg(test)]
pub(crate) fn backend_load_error() -> Option<String> {
    BACKEND_LOAD_ERROR.get().cloned()
}

#[cfg(test)]
mod tests {
    //! M0 探针测试（对应设计文档 10.1 的 M0 五项判据）。
    //!
    //! | 判据 | 本模块测试 |
    //! |---|---|
    //! | ① `project_edges` 返回**精确圆** | `projection_of_holed_plate_yields_analytic_circle` |
    //! | ② 剖面投影可行 | `section_plane_of_holed_plate_contains_a_circle` |
    //! | ③ WASM 在桌面正确加载 | `wasm_loads_and_reports_backend_name` / `a_facade_new_returns_the_real_backend` |
    //! | ④ IFC 组件可读子集 | **未覆盖**（属 `core-interop`，见 M4） |
    //! | ⑤ 拾取链路可用 | `face_group_hashes_map_back_to_face_handles` |
    //!
    //! ⚠ **必须用 `cargo test -p core-kernel --features occt --release` 运行**：
    //! debug 下 wasmtime 编译 21 MB WASM 慢约 100 倍，故此处按 occt-wasm 官方
    //! 集成测试的做法在 debug 下把这些用例标记为 **ignored**（不再静默 `return`
    //! 造"假绿"；`cargo test` 会显式列出 `ignored`）。
    //!
    //! ⚠ **本模块所有用例共用一份内核实例**（见下方 `SHARED`），因为并行测试各建
    //! 一个内核会把进程撑到 OOM。代价是用例实际串行执行，因此它们**不得**
    //! 依赖"上一步留下的 arena 状态"——`try_backend` 每次都会先清空 arena。
    //!
    //! 真实内核是**外部依赖**，测试只钉住"能力存在且数值合理"，不钉住内核内部实现。

    use super::*;
    use crate::kernel::Kernel;
    use std::sync::MutexGuard;

    /// 借用**全 crate 唯一**的共享内核（见模块级 [`shared_test_backend`]）。
    ///
    /// debug 构建或内核不可用时返回 `None`（跳过而非失败）——debug 下 wasmtime
    /// 编译 21 MB WASM 慢约 100 倍，按全仓库共识跳过。
    fn try_backend() -> Option<MutexGuard<'static, OcctBackend>> {
        if cfg!(debug_assertions) {
            eprintln!(
                "⚠ 跳过 M0 真实内核测试：debug 模式下 wasmtime 编译过慢，\
                 请用 `cargo test -p core-kernel --features occt --release`。"
            );
            return None;
        }
        shared_test_backend()
    }

    /// 内核为何加载不了（没有失败过则为 `None`）。
    fn load_error() -> Option<String> {
        backend_load_error()
    }

    /// **门禁：启用 `occt` feature 后，真实内核必须真的能实例化。**
    ///
    /// ## 为什么必须有这一条（2026-09-23 踩过的坑）
    ///
    /// 上面 14 条 occt 用例在"内核不可用"时都是 `let Some(..) = try_backend()
    /// else { return }`——这样套件仍可跑，但**整套 `--features occt` 会显示全绿，
    /// 而内核其实从未加载成功**。实测：`occt-wasm` 4.0.0 的
    /// `OcctKernel::new()` 用 `Linker::new()` 建了个**空 linker**，
    /// 内嵌 WASM 声明的 `env::emscripten_get_preloaded_image_data` 无人提供，
    /// wasmtime 48 于是在 `instantiate` 阶段报 `unknown import`。
    /// 当时"全绿"是 14 条用例集体静默跳过的结果，不是后端可用。
    ///
    /// 所以把"能不能加载"单独钉成一条**会失败**的用例：跳过可以用，
    /// **但不能让整条链路看起来是通的**。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn occt_feature_must_actually_instantiate_the_kernel() {
        let loaded = try_backend().is_some();
        assert!(
            loaded,
            "启用 `occt` feature 后真实内核必须能实例化，但它加载失败了。\n\
             实际原因：{}\n\
             排查方向：`occt-wasm` 的 `OcctKernel::new()` 是否用了\
             `Linker::define_unknown_imports_as_traps(&module)` 或显式定义了\
             全部 host import（Emscripten 产物会声明 `env::*` 系列导入）。",
            load_error().unwrap_or_else(|| "<未记录>".to_owned())
        );
    }

    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn wasm_loads_and_reports_backend_name() {
        let Some(mut b) = try_backend() else { return };
        assert_eq!(b.name(), "occt-wasm");
        let h = b.make_box(10.0, 20.0, 30.0).unwrap();
        assert!(b.is_alive(h));
        assert_eq!(b.live_count(), 1);
        b.release(h).unwrap();
        assert_eq!(b.live_count(), 0);
    }

    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn handle_lifecycle_roundtrips_and_rejects_double_release() {
        let Some(mut b) = try_backend() else { return };
        let h = b.make_box(1.0, 1.0, 1.0).unwrap();
        b.release(h).unwrap();
        assert!(!b.is_alive(h));
        assert!(b.release(h).is_err(), "二次释放应报错");
        assert_eq!(
            b.tessellate(h, 0.1, 0.5),
            Err(KernelError::InvalidHandle(h)),
            "已释放的句柄不得再运算"
        );
    }

    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn non_positive_dimensions_are_rejected_before_touching_the_kernel() {
        let Some(mut b) = try_backend() else { return };
        assert!(matches!(
            b.make_box(0.0, 1.0, 1.0),
            Err(KernelError::InvalidArgument(_))
        ));
        assert!(matches!(
            b.make_cylinder(1.0, -1.0),
            Err(KernelError::InvalidArgument(_))
        ));
    }

    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn arena_reuses_freed_slots() {
        let Some(mut b) = try_backend() else { return };
        let h1 = b.make_box(1.0, 1.0, 1.0).unwrap();
        b.release(h1).unwrap();
        let h2 = b.make_box(2.0, 2.0, 2.0).unwrap();
        assert_eq!(h1, h2, "释放的槽位应被复用（arena 语义）");
    }

    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn tessellation_reports_face_groups_for_picking() {
        let Some(mut b) = try_backend() else { return };
        let h = b.make_box(10.0, 10.0, 10.0).unwrap();
        let m = b.tessellate(h, 0.1, 0.5).unwrap();
        assert_eq!(m.triangle_count(), 12, "长方体应三角化为 12 个三角形");
        assert!(m.vertex_count() >= 8);
        assert_eq!(
            m.face_groups.len(),
            6,
            "长方体应有 6 个面分组（拾取粗筛的输入）"
        );
        let sum: u32 = m.face_groups.iter().map(|g| g.tri_count).sum();
        assert_eq!(sum, 12, "面分组的三角形总数应等于总三角形数");

        // **单位必须是三角形、不是索引**：内核给的原始区间是索引单位
        // （每面 6），不换算就会得到 36。这里用"首尾相接 + 恰好铺满"把它钉死，
        // 将来谁再把单位搞错，这两条断言会立刻报红。
        let mut cursor = 0u32;
        for g in &m.face_groups {
            assert_eq!(
                g.tri_start, cursor,
                "面分组应首尾相接（三角形单位）：期望 tri_start={cursor}，实得 {}",
                g.tri_start
            );
            cursor += g.tri_count;
        }
        assert_eq!(
            cursor as usize,
            m.triangle_count(),
            "面分组区间应恰好铺满全部三角形"
        );
    }

    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn boolean_cut_keeps_shapes_alive_and_produces_more_topology() {
        let Some(mut b) = try_backend() else { return };
        let plate = b.make_box(100.0, 60.0, 10.0).unwrap();
        let hole = b.make_cylinder(15.0, 40.0).unwrap();
        let holed = b.cut(plate, hole).unwrap();
        assert!(b.is_alive(holed));
        let m = b.tessellate(holed, 0.1, 0.5).unwrap();
        assert!(
            m.face_groups.len() > 6,
            "带圆孔的长方体面数应多于 6，实得 {}",
            m.face_groups.len()
        );
    }

    /// **M0 头号判据**：圆孔板做工程图级消隐投影，能否判出**解析圆**。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn projection_of_holed_plate_yields_analytic_circle() {
        let Some(mut b) = try_backend() else { return };
        let plate = b.make_box(100.0, 60.0, 10.0).unwrap();
        let hole = b.make_cylinder(15.0, 40.0).unwrap();
        let holed = b.cut(plate, hole).unwrap();

        // 沿 Z 俯视（孔轴方向）
        let view = ViewDef::top(DVec3::new(0.0, 0.0, 1000.0));
        let proj = b.project_edges(holed, view).unwrap();

        let kinds_visible: Vec<CurveKind> = proj.visible.iter().map(|c| c.kind).collect();
        let kinds_hidden: Vec<CurveKind> = proj.hidden.iter().map(|c| c.kind).collect();

        assert!(
            !proj.is_empty(),
            "圆孔板沿孔轴投影不应为空（曲线数={}）",
            proj.curve_count()
        );

        assert!(
            kinds_visible.contains(&CurveKind::Circle) || kinds_hidden.contains(&CurveKind::Circle),
            "M0 失败：未判出解析圆。共 {} 条曲线，可见{kinds_visible:?} 隐藏{kinds_hidden:?}",
            proj.curve_count()
        );

        // 半径若提取成功，应约等于孔半径 15
        for c in proj.visible.iter().chain(proj.hidden.iter()) {
            if c.kind == CurveKind::Circle
                && let CurveGeometry::Circle { radius, .. } = c.data
            {
                assert!(
                    (radius - 15.0).abs() < 0.05,
                    "圆孔半径应约 15，实得 {radius}"
                );
            }
        }
    }

    /// **M0 第三判据**：剖面投影。用 `section_plane` 在中面剖开，截面是带圆洞的矩形。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn section_plane_of_holed_plate_contains_a_circle() {
        let Some(mut b) = try_backend() else { return };
        let mk = b.kernel_mut();
        let plate = mk.make_box(100.0, 60.0, 10.0).unwrap();
        let hole = mk.make_cylinder(15.0, 40.0).unwrap();
        let holed = mk.cut(plate, hole).unwrap();

        let section = mk
            .section_plane(holed, 0.0, 0.0, 5.0, 0.0, 0.0, 1.0)
            .unwrap();
        let edges = mk.get_sub_shapes(section, "edge").unwrap();
        assert!(
            edges.len() >= 5,
            "带圆洞的矩形截面至少应有 4 直边 + 1 圆边，实得 {}",
            edges.len()
        );
        // 截面上应能识别出圆（洞壁被剖开后的轮廓）
        let has_circle = edges
            .iter()
            .any(|e| classify_curve(&mk.curve_type(*e).unwrap_or_default()) == CurveKind::Circle);
        assert!(has_circle, "剖面应包含一条解析圆边");
    }

    /// **M0 第五判据**：拾取链路。
    ///
    /// 设计文档 6.6 的拾取方案是「网格面哈希**粗筛** → `project_point_on_face`
    /// **精算**」。粗筛要成立，前提是**网格面分组的哈希**与**内核面的哈希**同属
    /// 一个哈希域、且顺序一致。
    ///
    /// > 这条用例一度被误判为"判据不成立"。真实原因很隐蔽：它早先直接调
    /// > `sub_shape_hashes(kh, "face", 1_000_000)`，而网格化器用的是 `i32::MAX`。
    /// > OCCT 的 `HashCode(upper)` 结果落在 `[1, upper]`，**bound 不同则值集完全不相交**
    /// > （同一个面：`326887` vs `1652326887`）——两个集合没有一个数相同，
    /// > 看起来像"两边根本不是一个面"，其实只是"尺子不一样长"。
    /// > 现在域由 [`FACE_HASH_UPPER_BOUND`] 固定在后端内部，并通过
    /// > [`KernelBackend::face_hashes`] 暴露成正式能力，不再泄漏给调用方。
    ///
    /// 本测试走**公开契约**（不碰 `kernel_mut`），因此 mock 后端也要一并满足。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn face_group_hashes_map_back_to_face_handles() {
        let Some(mut b) = try_backend() else { return };
        let h = b.make_box(10.0, 10.0, 10.0).unwrap();
        let mesh = b.tessellate(h, 0.1, 0.5).unwrap();
        let from_mesh: Vec<u64> = mesh.face_groups.iter().map(|g| g.face_hash).collect();
        let from_kernel = b.face_hashes(h).unwrap();

        assert_eq!(from_kernel.len(), 6, "长方体应有 6 个面哈希");
        // 用 `Vec` 而不是集合：**顺序也必须一致**——粗筛命中后要能按序号回查面句柄。
        assert_eq!(
            from_mesh, from_kernel,
            "网格面分组的哈希必须与内核面哈希**同域同序**，否则拾取粗筛会选错面"
        );
    }

    /// BREP 文本往返——`.bimdoc` 持久化要用的能力（设计文档 4.2 / 3.3）。
    ///
    /// 走**公开契约**（`export_brep` / `import_brep` / `volume`），不碰 `kernel_mut`：
    /// 这条链路是 `core-app::GeometryService` 的"存盘 / 惰性重建"直接依赖的接口。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn brep_text_roundtrip_preserves_volume() {
        let Some(mut b) = try_backend() else { return };
        let h = b.make_box(10.0, 20.0, 30.0).unwrap();
        let v1 = b.volume(h).unwrap();
        let text = b.export_brep(h).unwrap();
        let back = b.import_brep(&text).unwrap();

        assert!(b.is_alive(back), "导入的几何应是一个可用的新句柄");
        assert_ne!(back, h, "导入应造出独立句柄，而不是复用原句柄");
        assert!((v1 - 6000.0).abs() < 1.0, "长方体体积应约 6000，实得 {v1}");
        let v2 = b.volume(back).unwrap();
        assert!((v1 - v2).abs() < 1e-6, "BREP 往返体积应无损：{v1} vs {v2}");

        // 导出的必须是真 OCCT BREP 文本（首行是 CASCADE 头），不是随意字符串
        assert!(
            text.contains("CASCADE"),
            "导出的应是 OCCT BREP 文本，实得前 40 字符：{:?}",
            text.chars().take(40).collect::<String>()
        );
    }

    /// 平移算子（`occt_translate`）已接线：平移是刚体运动，体积不变。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn translate_moves_the_shape_without_changing_volume() {
        let Some(mut b) = try_backend() else { return };
        let h = b.make_box(10.0, 20.0, 30.0).unwrap();
        let v = b.volume(h).unwrap();
        let moved = b.translate(h, 1000.0, 0.0, 3000.0).unwrap();
        assert!(b.is_alive(h), "平移产生新形状，原句柄仍有效");
        assert_ne!(moved, h);
        let v2 = b.volume(moved).unwrap();
        assert!((v - v2).abs() / v < 1e-6, "平移不改变体积：{v} vs {v2}");
    }

    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn real_backend_declares_itself_as_real() {
        // 用共享内核而不是再 new 一个：并行测试里多一个实例就多一份 21 MB WASM
        // （M0 踩过的 OOM 就是这么来的）。
        let Some(b) = try_backend() else { return };
        assert_eq!(b.kind(), BackendKind::Real);
        assert!(b.kind().is_real());
    }

    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn live_handles_exposes_the_arena_contents() {
        let Some(mut b) = try_backend() else { return };
        let a = b.make_box(1.0, 1.0, 1.0).unwrap();
        let c = b.make_sphere(1.0).unwrap();
        assert_eq!(b.live_handles(), vec![a, c]);
        b.release(a).unwrap();
        assert_eq!(b.live_handles(), vec![c]);
    }

    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn malformed_brep_text_is_rejected() {
        let Some(mut b) = try_backend() else { return };
        assert!(
            matches!(
                b.import_brep("这不是 BREP"),
                Err(KernelError::Geometry { .. })
            ),
            "非法 BREP 文本应报 Geometry 错误，不得静默成功"
        );
        assert_eq!(b.live_count(), 0, "失败的导入不应留下句柄");
    }

    /// 走门面：证明 `Kernel::new()` 真的接上了真实后端（否则等于没接线）。
    ///
    /// # ⚠ 为什么函数名以 `a_` 打头（**必须排在 `occt::tests` 的最前**）
    ///
    /// libtest **按测试全名字典序**执行；本用例会通过 `Kernel::new()` **再建一具**
    /// 内核（vendor 每具内核各建一个 wasmtime `Engine`，JIT 21 MB WASM，内存开销巨大）。
    /// 若它在共享探针内核（[`shared_test_backend`]）**建好之后**才跑，进程里就**同时**
    /// 有两具内核——在内存受限的机器上直接 OOM（实测报 `memory allocation ... failed`，
    /// 进程以 `0xc0000409` 中止，日志里**没有** FAILED 行）。
    ///
    /// 本用例**先于**任何其它 occt 用例运行，故此刻共享内核尚未创建，全进程仅此一具内核；
    /// 本用例结束、该内核 `Drop` 后，后续 occt 用例才创建共享内核。于是任意时刻
    /// **至多一具**内核。`a_` 前缀保证它排在 `occt::tests::arena_reuses_freed_slots`
    /// （同为 `a` 开头，第二位 `_`(0x5f) < `r`(0x72)）之前。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn a_facade_new_returns_the_real_backend() {
        let Ok(mut k) = Kernel::new() else {
            eprintln!("⚠ 跳过：内核不可用");
            return;
        };
        assert_eq!(k.backend_name(), "occt-wasm");
        let h = k.make_box(10.0, 20.0, 30.0).unwrap();
        assert!(k.is_alive(h));
    }

    // ---- M3：新构造算子（锥 / 环 / 拉伸 / 旋转体 / 刚体旋转）-----------------

    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn m3_cone_and_torus_construct_with_sane_volumes() {
        let Some(mut b) = try_backend() else { return };
        let cone = b.make_cone(3.0, 0.0, 10.0).unwrap();
        let v = b.volume(cone).unwrap();
        let want = std::f64::consts::PI * 10.0 / 3.0 * 9.0;
        assert!(
            (v - want).abs() / want < 1e-6,
            "圆锥体积应约 {want}，实得 {v}"
        );

        let torus = b.make_torus(10.0, 2.0).unwrap();
        let vt = b.volume(torus).unwrap();
        let want_t = 2.0 * std::f64::consts::PI * std::f64::consts::PI * 10.0 * 4.0;
        assert!(
            (vt - want_t).abs() / want_t < 1e-6,
            "圆环体积应约 {want_t}，实得 {vt}"
        );
    }

    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn m3_extrude_and_revolve_build_from_a_profile() {
        let Some(mut b) = try_backend() else { return };
        // OCCT 的 `extrude`/`revolve` 需要**面/线**轮廓（实体不可直接扫掠）。
        // 轮廓来源：内核 `make_rectangle` 造 10×10 平面矩形（位于 XY 平面）。
        // —— 拉伸：XY 面沿 +Z 拉高 50（面垂直于拉伸向），体积显著大于 0。
        let rect = b.kernel_mut().make_rectangle(10.0, 10.0).unwrap();
        let profile_e = b.alloc(rect);
        let solid = b.extrude(profile_e, 0.0, 0.0, 50.0).unwrap();
        assert!(
            b.volume(solid).unwrap() > 0.0,
            "拉伸后应得到有体积的实体，实得 {}",
            b.volume(solid).unwrap()
        );

        // —— 旋转体：绕 Z 轴扫掠要求轮廓位于**含轴平面**内。先把矩形绕 X 轴
        // 转 90°（XY→XZ 平面），再沿 +X 平移到离轴 20 处，然后绕 Z 整圈扫掠。
        let rect2 = b.kernel_mut().make_rectangle(10.0, 10.0).unwrap();
        let rect2 = b.alloc(rect2);
        let upright = b
            .rotate(
                rect2,
                0.0,
                0.0,
                0.0,
                1.0,
                0.0,
                0.0,
                std::f64::consts::FRAC_PI_2,
            )
            .unwrap();
        let profile_r = b.translate(upright, 20.0, 0.0, 0.0).unwrap();
        let rev = b
            .revolve(
                profile_r,
                0.0,
                0.0,
                0.0,
                0.0,
                0.0,
                1.0,
                std::f64::consts::TAU,
            )
            .unwrap();
        assert!(
            b.volume(rev).unwrap() > 0.0,
            "绕 Z 轴整圈扫掠应得到有体积的环体，实得 {}",
            b.volume(rev).unwrap()
        );
    }

    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn m3_rotate_preserves_volume_and_moves_the_bbox() {
        let Some(mut b) = try_backend() else { return };
        let h = b.make_box(10.0, 20.0, 30.0).unwrap();
        let v = b.volume(h).unwrap();
        let rot = b
            .rotate(h, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, std::f64::consts::FRAC_PI_2)
            .unwrap();
        assert!(b.is_alive(h), "旋转产生新形状，原句柄仍有效");
        let v2 = b.volume(rot).unwrap();
        assert!((v - v2).abs() / v < 1e-6, "旋转不改变体积：{v} vs {v2}");

        // 绕 Z 转 90°：x/y 方向的包围盒尺寸应互换（±容差）
        let bb = b.bounding_box(rot).unwrap();
        let [sx, sy, _] = bb.size();
        assert!(
            (sx - 20.0).abs() < 0.05 && (sy - 10.0).abs() < 0.05,
            "旋转 90° 后 bbox 的 x/y 应互换，实得 [{sx}, {sy}]"
        );
    }

    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn m3_bounding_box_and_surface_area_are_reported() {
        let Some(mut b) = try_backend() else { return };
        let h = b.make_box(10.0, 20.0, 30.0).unwrap();
        let bb = b.bounding_box(h).unwrap();
        let [sx, sy, sz] = bb.size();
        assert!(
            (sx - 10.0).abs() < 1e-6 && (sy - 20.0).abs() < 1e-6 && (sz - 30.0).abs() < 1e-6,
            "盒的 bbox 尺寸应精确，实得 [{sx}, {sy}, {sz}]"
        );
        // 表面积 = 2·(10·20 + 20·30 + 10·30) = 2200
        let a = b.surface_area(h).unwrap();
        assert!((a - 2200.0).abs() < 1.0, "表面积应约 2200，实得 {a}");
    }

    /// **M3 复合体钉子 A/B（occt 腿）**：远离两盒 fuse → 2 个 solid；多体必须响亮拒绝。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn m3_disconnected_fuse_yields_two_solids_and_multi_solid_is_refused() {
        let Some(mut b) = try_backend() else { return };
        let a = b.make_box(10.0, 10.0, 10.0).unwrap();
        let far_box = b.make_box(10.0, 10.0, 10.0).unwrap();
        let far = b.translate(far_box, 100.0, 0.0, 0.0).unwrap();
        let fused = b.fuse(a, far).unwrap();
        let solids = b.sub_shapes(fused, SubShapeKind::Solid).unwrap();
        assert_eq!(
            solids.len(),
            2,
            "一次布尔产出了两个互不相连体，必须能拆出 2 个"
        );
        assert_eq!(b.sub_shape_count(fused, SubShapeKind::Solid).unwrap(), 2);

        // 多体必须响亮拒绝（此处直接以 face 腿口径校验枚举数，门面 `single_solid`
        // 的 NotSingleSolid 语义由 core-app 探针在 mock 腿钉死）。
        assert!(
            solids.len() > 1,
            "多于一个实体时，上层必须走 single_solid 而不能取 first"
        );
    }

    // =========================================================================
    // M5-B0：常驻钉子（由 M4 一次性探针转正，§0.8）
    // =========================================================================
    //
    // 这三条钉子钉住 B3/B4/B5 关键路径所依赖的三项内核能力——
    //   ① `*_with_history` / `EvolutionData`（B3 关联更新主路径）；
    //   ② 拾取精算 `project_point_on_edge` / `project_point_on_face`（B4）；
    //   ③ NURBS 提取 `get_nurbs_curve_data`（B5"圆→NURBS 保真"退路）。
    //
    // **与旧探针的区别（M0 铁律：跳过与通过必须可区分）**：
    // 旧探针用 `try_backend() else { return }`，在内核不可用时**静默跳过**；
    // 现改为经 [`require_real_backend`] 取后端——**release 下内核不可用即 FAIL**。
    // 断言也由宽松的 `>= 3` 收紧为**精确长度 / 精确字段位置**。
    // 本项目已踩过坑：`export_step` 在 runtime trap（WASM 产物没编进文件系统）。
    // 故钉子**绝不静默跳过**：凡是 trap / 报错，一律用 `panic!` 把完整错误链打出来。

    /// 把错误的**完整 `source()` 链**（逐层的 `Display` 与 `Debug`）拼成一段文本。
    ///
    /// 与 `known_upstream_defects::error_chain_text` 同口径：`OcctError::Runtime`
    /// 的顶层 `Display` 只打上下文，真正的 wasm trap 文本在 `source()` 链更深层，
    /// 必须逐层拼出来才看得见。
    fn error_chain_text(err: &(dyn std::error::Error + 'static)) -> String {
        let mut out = format!("{err}\n{err:?}");
        let mut cur = err.source();
        while let Some(e) = cur {
            out.push_str(&format!("\n{e}\n{e:?}"));
            cur = e.source();
        }
        out
    }

    /// 取真实内核后端，供**常驻钉子**使用（M5 §0.8 探针转正）。
    ///
    /// - **debug 下返回 `None`**（全仓库共识：wasmtime 编译 21 MB WASM 慢约 100 倍，
    ///   occt 门禁只在 `--release` 跑）——调用方据此 `return` 跳过。
    /// - **release 下内核不可用则 `panic!`**：这是 M0 铁律"跳过与通过可区分"的落地，
    ///   取代旧探针的 `try_backend() else { return }` 静默跳过。
    ///
    /// 用法：`let Some(mut b) = require_real_backend("测试名") else { return; };`
    fn require_real_backend(test: &str) -> Option<MutexGuard<'static, OcctBackend>> {
        match try_backend() {
            Some(b) => Some(b),
            None => {
                if cfg!(debug_assertions) {
                    return None;
                }
                panic!(
                    "`{test}` 是常驻钉子，必须真跑真断言，但真实内核不可用。\n\
                     实际原因：{}\n\
                     请用 `cargo test -p core-kernel --features occt --release -- --test-threads=1` 运行。",
                    load_error().unwrap_or_else(|| "<未记录>".to_owned())
                );
            }
        }
    }

    /// 造一块「带圆孔的长方板」（复用既有投影/剖面用例的造形）——内核句柄口径。
    fn holed_plate(mk: &mut OcctKernel) -> occt_wasm::ShapeHandle {
        let plate = mk.make_box(100.0, 60.0, 10.0).unwrap();
        let hole = mk.make_cylinder(15.0, 40.0).unwrap();
        mk.cut(plate, hole).unwrap()
    }

    /// 分组解码的中间表示：`(输入面哈希, 输出面哈希组)` 列表。
    type FaceGroups = Vec<(i32, Vec<i32>)>;

    /// 把 `[输入面哈希, k, 输出面哈希×k]*` 扁平表解成 `(输入哈希, 输出组)`。
    ///
    /// 返回 `(组, 已消耗元素个数)`；**越界 / 负数 k 一律 `Err`**（不许静默截断）。
    /// 分组编码见 M5 规划 §0.3（实测：`modified` 例 `k=1`，`generated` 例 `k=3`）。
    fn decode_face_groups(flat: &[i32]) -> Result<(FaceGroups, usize), String> {
        let mut groups = Vec::new();
        let mut i = 0usize;
        while i < flat.len() {
            if i + 2 > flat.len() {
                return Err(format!(
                    "组头不完整：i={i} 起剩余 {} 个元素，不足 [in,k] 两元",
                    flat.len() - i
                ));
            }
            let input = flat[i];
            let k = flat[i + 1];
            if k < 0 {
                return Err(format!("i={i} 处组头含负的输出个数 k={k}"));
            }
            let k = k as usize;
            if i + 2 + k > flat.len() {
                return Err(format!(
                    "组边界外溢：i={i} 声明 k={k}，需 {} 个元素，实际剩余 {}",
                    i + 2 + k,
                    flat.len() - i
                ));
            }
            groups.push((input, flat[i + 2..i + 2 + k].to_vec()));
            i += 2 + k;
        }
        Ok((groups, i))
    }

    /// **钉子 ①（B3 主路径能力护栏）**：`fuse_with_history` / `EvolutionData` 可用且分组自洽。
    ///
    /// 断言：布尔 history **不 trap**；`modified`/`generated` 能按 `[in,k,out×k]`
    /// 解成组（组边界不越界、恰好铺满扁平表）；`result_id` 非 0。
    /// 两个相交 box 做带历史布尔；输入面哈希用 `sub_shape_hashes(·, "face", FACE_HASH_UPPER_BOUND)`
    /// 取（与网格面哈希同域）。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn evolution_history_data_is_available_and_grouped() {
        let Some(mut b) = require_real_backend("evolution_history_data_is_available_and_grouped")
        else {
            return;
        };
        assert_eq!(
            b.kind(),
            BackendKind::Real,
            "钉子必须跑在真实内核上，不许在 mock 上假绿"
        );
        let mk = b.kernel_mut();

        // 两个相交的 box（第二个平移后与第一个重叠，布尔会产生真实的新生面）
        let a = mk.make_box(10.0, 10.0, 10.0).unwrap();
        let braw = mk.make_box(10.0, 10.0, 10.0).unwrap();
        let bb = mk.translate(braw, 5.0, 5.0, 5.0).unwrap();

        // 输入面哈希：两个操作数的面哈希按同一 bound 口径拼接
        let mut hashes = mk
            .sub_shape_hashes(a, "face", FACE_HASH_UPPER_BOUND)
            .unwrap();
        hashes.extend(
            mk.sub_shape_hashes(bb, "face", FACE_HASH_UPPER_BOUND)
                .unwrap(),
        );

        let evo = mk
            .fuse_with_history(a, bb, &hashes, FACE_HASH_UPPER_BOUND)
            .unwrap_or_else(|e| {
                panic!(
                    "布尔 history **不可用（trap/失败）**——B3 关联更新主路径受阻。\
                     完整错误链：\n{}",
                    error_chain_text(&e)
                )
            });

        assert_ne!(evo.result_id, 0, "带历史布尔的结果 id 不应为 0");

        // modified / generated 是扁平 Vec<i32>，按 `[输入面哈希, k, 输出面哈希×k]` 分组。
        let (mod_groups, mod_consumed) = decode_face_groups(&evo.modified)
            .unwrap_or_else(|m| panic!("modified 分组解码失败：{m}；原表={:?}", evo.modified));
        let (gen_groups, gen_consumed) = decode_face_groups(&evo.generated)
            .unwrap_or_else(|m| panic!("generated 分组解码失败：{m}；原表={:?}", evo.generated));
        assert_eq!(
            mod_consumed,
            evo.modified.len(),
            "modified 分组应恰好铺满扁平表"
        );
        assert_eq!(
            gen_consumed,
            evo.generated.len(),
            "generated 分组应恰好铺满扁平表"
        );
        assert!(
            !mod_groups.is_empty() || !gen_groups.is_empty() || !evo.deleted.is_empty(),
            "EvolutionData 可读但 modified/generated/deleted 全为空（result_id={}）",
            evo.result_id
        );
        // 组内自洽：每组 k 必须 ≥ 1（空组会让"变了哪些面"判不出来）
        for (input, outputs) in mod_groups.iter().chain(gen_groups.iter()) {
            assert!(
                !outputs.is_empty(),
                "输入面哈希 {input} 对应的输出面数不应为 0（k≥1）"
            );
        }
        println!(
            "evolution: result_id={}, modified 组数={}, generated 组数={}, deleted(len)={}",
            evo.result_id,
            mod_groups.len(),
            gen_groups.len(),
            evo.deleted.len()
        );
    }

    /// **钉子 ②（B4 能力护栏）**：拾取精算返回**已知元数**，未命中返回 `Err`。
    ///
    /// 断言：edge **精确 7 元** `[px,py,pz, tx,ty,tz, param]`；face **精确 6 元**
    /// `[px,py,pz, u, v, distance]`；**未命中返回 `Err`（领域错误，非 panic/trap）**。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn point_projection_returns_known_arities_and_errs_on_miss() {
        let Some(mut b) =
            require_real_backend("point_projection_returns_known_arities_and_errs_on_miss")
        else {
            return;
        };
        assert_eq!(
            b.kind(),
            BackendKind::Real,
            "钉子必须跑在真实内核上，不许在 mock 上假绿"
        );
        let mk = b.kernel_mut();

        let holed = holed_plate(mk);

        // ---- 边：命中（用边上中点查询）→ 精确 7 元 ----
        let edges = mk.get_sub_shapes(holed, "edge").unwrap();
        assert!(!edges.is_empty(), "带孔板应至少有一条边");
        let mut edge_hit: Option<(occt_wasm::ShapeHandle, Vec<f64>)> = None;
        for e in &edges {
            let r = mk.curve_parameters(*e).unwrap_or_default();
            if r.len() < 2 {
                continue;
            }
            let p = mk
                .curve_point_at_param(*e, (r[0] + r[1]) * 0.5)
                .unwrap_or_default();
            if p.len() < 3 {
                continue;
            }
            if let Ok(v) = mk.project_point_on_edge(*e, p[0], p[1], p[2]) {
                edge_hit = Some((*e, v));
                break;
            }
        }
        let (edge, v_edge) = edge_hit.unwrap_or_else(|| {
            panic!(
                "project_point_on_edge 对所有 {} 条边（用边上中点查询）都失败——该 API 不可用",
                edges.len()
            )
        });
        assert_eq!(
            v_edge.len(),
            7,
            "edge 精算必须**精确 7 元** [p,t,param]，实得 len={} 值={:?}",
            v_edge.len(),
            v_edge
        );

        // ---- 边：**未命中 → Err（领域错误，非 panic）** ----
        let edge_miss = mk.project_point_on_edge(edge, 1.0e6, 1.0e6, 1.0e6);
        assert!(
            edge_miss.is_err(),
            "远离边的查询点应返回领域错误（Err），实得 {:?}——B4 必须处理\"未命中即 Err\"",
            edge_miss
        );
        println!(
            "edge 未命中错误（领域错误、非 trap）：{}",
            error_chain_text(&edge_miss.unwrap_err())
        );

        // ---- 面：命中（用面质心查询）→ 精确 6 元 ----
        let faces = mk.get_sub_shapes(holed, "face").unwrap();
        assert!(!faces.is_empty(), "带孔板应至少有一个面");
        let mut face_hit: Option<(occt_wasm::ShapeHandle, Vec<f64>)> = None;
        for f in &faces {
            let c = mk.get_surface_center_of_mass(*f).unwrap_or_default();
            if c.len() < 3 {
                continue;
            }
            if let Ok(v) = mk.project_point_on_face(*f, c[0], c[1], c[2]) {
                face_hit = Some((*f, v));
                break;
            }
        }
        let (_face, v_face) = face_hit.unwrap_or_else(|| {
            panic!(
                "project_point_on_face 对所有 {} 个面（用面质心查询）都失败——该 API 不可用",
                faces.len()
            )
        });
        assert_eq!(
            v_face.len(),
            6,
            "face 精算必须**精确 6 元** [p,uv,dist]，实得 len={} 值={:?}",
            v_face.len(),
            v_face
        );

        // ---- 面：**未命中 → Err（领域错误，非 panic）** ----
        // 实测：平面投影是**无界**的（任意远点都得 6 元结果），故平面点不构成"未命中"。
        // 真正返回领域错误的是**圆柱面 + 轴上点**（最近点不唯一/不存在）——
        // 孔壁圆柱的轴为 Z 轴（x=y=0），取轴上一点即触发 `projectPointOnFace: no projection found`。
        let cylinder_face = faces
            .iter()
            .copied()
            .find(|f| mk.surface_type(*f).unwrap_or_default() == "cylinder")
            .expect("带孔板应含一条圆柱面（孔壁）");
        let face_miss = mk.project_point_on_face(cylinder_face, 0.0, 0.0, 1.0e6);
        assert!(
            face_miss.is_err(),
            "圆柱面 + 轴上点应返回领域错误（Err），实得 {:?}",
            face_miss
        );
        println!(
            "face 未命中错误（领域错误、非 trap）：{}",
            error_chain_text(&face_miss.unwrap_err())
        );
    }

    /// **钉子 ④（B4 门面接线护栏）**：点投影经**门面句柄**返回**具名类型**，未命中即 `Err`。
    ///
    /// 钉子②只钉住 **raw vendor 内核**（`mk.project_point_on_face/_edge` 返回裸 `Vec<f64>`）。
    /// 本钉子补上**门面这一环**：`sub_shapes` 产出的门面 [`ShapeHandle`] 必须能直接喂给
    /// `project_point_on_face/_edge`，且内核 6/7 元向量被 [`PointOnFace`]/[`PointOnEdge`]
    /// 正确映射（`distance` / `tangent` / `param`）。**不碰 `kernel_mut()`、不碰 raw `mk.`**。
    ///
    /// # ⚠ 为什么走 `OcctBackend` 公开契约而不是 `Kernel::new()`
    ///
    /// 一具内核 = 一个 wasmtime `Engine` + JIT 21 MB WASM；与共享探针内核**并存**会 OOM
    /// （见模块级说明与 `a_facade_new_returns_the_real_backend` 的 `a_` 前缀注释）。
    /// 故走**共享内核后端的公开契约**（`OcctBackend` 的 `KernelBackend` 方法）——
    /// 与 `face_group_hashes_map_back_to_face_handles`（"走公开契约，不碰 kernel_mut"）
    /// 同一套路；`Kernel::project_point_on_face` 只是对它的**一行转发**，另有
    /// `core-kernel` 的 mock 单测（`mock_rejects_point_projection_loudly`）覆盖蓝图。
    ///
    /// 造形（全走门面）：长方体 `100×60×10` 与圆柱 `r=15,h=40`（轴为 Z、过原点）相减
    /// → 角部 `(0,0)` 处留一个 1/4 圆柱缺口。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn facade_point_projection_is_typed_and_errs_on_miss() {
        let Some(mut b) = require_real_backend("facade_point_projection_is_typed_and_errs_on_miss")
        else {
            return;
        };
        assert_eq!(
            b.kind(),
            BackendKind::Real,
            "钉子必须跑在真实内核上，不许在 mock 上假绿"
        );

        // 造形**全走门面**（不碰 kernel_mut / raw mk.）：
        let plate = b.make_box(100.0, 60.0, 10.0).unwrap();
        let hole = b.make_cylinder(15.0, 40.0).unwrap();
        let holed = b.cut(plate, hole).unwrap();

        let faces = b.sub_shapes(holed, SubShapeKind::Face).unwrap();
        let edges = b.sub_shapes(holed, SubShapeKind::Edge).unwrap();
        assert!(!faces.is_empty(), "带孔板应至少有一个面");
        assert!(!edges.is_empty(), "带孔板应至少有一条边");

        let near = |p: DVec3, q: DVec3, tol: f64| (p - q).length() <= tol;

        // ---- 面：命中（已知顶面上的点）→ `PointOnFace`，distance≈0，u/v 有限 ----
        // 顶面 z=10、x∈[0,100]、y∈[0,60]；查询点 (50,30,10) 远离角部缺口。
        let face_query = DVec3::new(50.0, 30.0, 10.0);
        let mut best_face: Option<(ShapeHandle, PointOnFace)> = None;
        for f in &faces {
            if let Ok(p) = b.project_point_on_face(*f, face_query)
                && best_face.is_none_or(|(_, q)| p.distance < q.distance)
            {
                best_face = Some((*f, p));
            }
        }
        let (_, on_face) = best_face.expect("顶面应能被投影命中（至少一个面返回 Ok）");
        assert!(
            on_face.distance <= 1e-6,
            "在面上查询应得 distance≈0，实得 {}",
            on_face.distance
        );
        assert!(
            near(on_face.point, face_query, 1e-6),
            "命中点应≈查询点：{:?} vs {:?}",
            on_face.point,
            face_query
        );
        assert!(
            on_face.u.is_finite() && on_face.v.is_finite(),
            "面局部参数 u/v 应有限，实得 u={}, v={}",
            on_face.u,
            on_face.v
        );

        // ---- 边：命中（已知立方体竖边中点）→ `PointOnEdge`，切向单位，param 有限 ----
        // 竖边在 (100,0)、z∈[0,10]；查询点 (100,0,5) 即其≈中点，且远离角部缺口。
        let edge_query = DVec3::new(100.0, 0.0, 5.0);
        let mut best_edge: Option<(ShapeHandle, PointOnEdge)> = None;
        for e in &edges {
            if let Ok(p) = b.project_point_on_edge(*e, edge_query) {
                let better = best_edge.is_none_or(|(_, q)| {
                    (p.point - edge_query).length() < (q.point - edge_query).length()
                });
                if better {
                    best_edge = Some((*e, p));
                }
            }
        }
        let (hit_edge, on_edge) = best_edge.expect("竖边应能被投影命中（至少一条边返回 Ok）");
        assert!(
            near(on_edge.point, edge_query, 1e-6),
            "边命中点应≈查询点：{:?} vs {:?}",
            on_edge.point,
            edge_query
        );
        let tn = on_edge.tangent.length();
        assert!(
            (tn - 1.0).abs() <= 1e-6,
            "切向应为单位向量（|t|≈1），实得 |t|={tn}"
        );
        assert!(
            on_edge.param.is_finite(),
            "曲线参数应有限，实得 param={}",
            on_edge.param
        );

        // ---- 边：**未命中 → Err（领域错误，非 panic）** ----
        let edge_err = match b.project_point_on_edge(hit_edge, DVec3::new(1.0e6, 1.0e6, 1.0e6)) {
            Err(e @ KernelError::Geometry { .. }) => e,
            other => panic!(
                "远离边的查询点应返回领域错误 KernelError::Geometry，实得 {other:?}——\
                 若上游改为返回 Ok/他类错误，须同步更新 B4 的\"未命中即 Err\"契约"
            ),
        };

        // ---- 面：圆柱面 + 轴上点 → **Err**；平面 → Ok（区分"无界平面"与"圆柱无解"） ----
        // 先靠**纯几何**认出孔壁圆柱面：取柱面上一点 (r/√2, r/√2, 5)（避开全部平面），
        // 对每个面投影，**最小距离≈0** 的那个必是圆柱面（平面在该点距离 ≥5）。
        let r = 15.0_f64;
        let wall = DVec3::new(r / 2.0_f64.sqrt(), r / 2.0_f64.sqrt(), 5.0);
        let mut cyl: Option<(f64, ShapeHandle)> = None;
        for f in &faces {
            if let Ok(p) = b.project_point_on_face(*f, wall)
                && cyl.is_none_or(|(d, _)| p.distance < d)
            {
                cyl = Some((p.distance, *f));
            }
        }
        let (cyl_dist, cyl_face) = cyl.expect("柱面点应能被某个面投影命中");
        assert!(
            cyl_dist <= 1e-6,
            "孔壁柱面点应落在某个面上（distance≈0），实得最小距离 {cyl_dist}"
        );

        let axis_point = DVec3::new(0.0, 0.0, 1.0e6);
        let face_err = match b.project_point_on_face(cyl_face, axis_point) {
            Err(e @ KernelError::Geometry { .. }) => e,
            other => panic!("圆柱面 + 轴上点应返回领域错误 KernelError::Geometry，实得 {other:?}"),
        };
        // 对照：其余（平面）面投影同一远点 → **仍 Ok**（平面投影无界，正是"命中判定"
        // 不能靠它、须自建射线-三角粗筛的原因，见 B4.1）。
        let planar_ok = faces
            .iter()
            .any(|&f| f != cyl_face && b.project_point_on_face(f, axis_point).is_ok());
        assert!(
            planar_ok,
            "平面上投射任意远点应 Ok（无界）；这里连一个 Ok 都没有，说明面枚举或投影连线出了问题"
        );

        println!(
            "门面点投影：面命中 distance={:.3e}（u={}, v={}）；边命中 |t|={tn}、param={}；\
             圆柱面上轴点 → {}；边远点 → {}",
            on_face.distance,
            on_face.u,
            on_face.v,
            on_edge.param,
            error_chain_text(&face_err),
            error_chain_text(&edge_err),
        );
    }

    /// **钉子 ③（B5 能力护栏）**：`get_nurbs_curve_data` **只吃 BSpline/Bezier 边**。
    ///
    /// 断言：解析圆边 → `Err`（领域错误，非 trap）；Bezier 边 → `degree=3` 且 `poles` 非空。
    /// 含义：解析圆**不能**靠它转 NURBS；"圆→NURBS 保真"须**先自建 BSpline 边**。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn nurbs_extraction_requires_bspline_or_bezier_edges() {
        let Some(mut b) = require_real_backend("nurbs_extraction_requires_bspline_or_bezier_edges")
        else {
            return;
        };
        assert_eq!(
            b.kind(),
            BackendKind::Real,
            "钉子必须跑在真实内核上，不许在 mock 上假绿"
        );
        let mk = b.kernel_mut();

        let holed = holed_plate(mk);
        let edges = mk.get_sub_shapes(holed, "edge").unwrap();

        // （a）解析圆边 → **必须 Err**（领域错误，非 trap）
        let circle = edges
            .iter()
            .copied()
            .find(|e| classify_curve(&mk.curve_type(*e).unwrap_or_default()) == CurveKind::Circle);
        let circle = circle.expect("带孔板应含一条解析圆边");
        match mk.get_nurbs_curve_data(circle) {
            Ok(n) => panic!(
                "解析圆边不应能直接提取 NURBS，却返回 degree={}, poles.len={}——\
                 若上游改为支持，须同步更新 B5 退路方案",
                n.degree,
                n.poles.len()
            ),
            Err(e) => println!(
                "解析圆边被拒（**领域错误、非 trap**）：{}",
                error_chain_text(&e)
            ),
        }

        // （b）真正的 NURBS 曲线：cubic Bezier 边（`get_nurbs_curve_data` 的合法输入）
        let bez = mk
            .make_bezier_edge(&[
                0.0, 0.0, 0.0, // p0
                10.0, 0.0, 0.0, // p1
                10.0, 10.0, 0.0, // p2
                0.0, 10.0, 0.0, // p3
            ])
            .unwrap_or_else(|e| {
                panic!(
                    "make_bezier_edge 失败，无法构造 NURBS 输入。完整错误链：\n{}",
                    error_chain_text(&e)
                )
            });

        let n = mk.get_nurbs_curve_data(bez).unwrap_or_else(|e| {
            panic!(
                "get_nurbs_curve_data(Bezier 边) 失败——B5 退路受阻。完整错误链：\n{}",
                error_chain_text(&e)
            )
        });

        assert_eq!(
            n.degree, 3,
            "cubic Bezier 边的 NURBS degree 应精确为 3，实得 {}",
            n.degree
        );
        assert!(!n.poles.is_empty(), "NURBS 提取应返回非空 poles");
        println!(
            "Bezier 边 NURBS: degree={}, poles.len={}, knots.len={}",
            n.degree,
            n.poles.len(),
            n.knots.len()
        );
    }

    /// **B4.1b-1 钉子（occt 腿）：`surface_normal` 门面返回单位法向且轴对齐**。
    ///
    /// 走 `OcctBackend` 公开契约（不碰 `kernel_mut()` / raw `mk.`）——与
    /// `facade_point_projection_is_typed_and_errs_on_miss` 同一套路。
    ///
    /// 断言链：对立方体 6 个面，各自投影一个内部点取 `(u,v)`，再取该处法向：
    /// ① `|n| ≈ 1`（单位）；② 恰好一个分量 ±1、其余 ≈0（轴对齐）；
    /// ③ 6 个法向**恰好组成 `{±X, ±Y, ±Z}` 各一次**——这条同时钉住
    /// `dvec3_from_kernel_vec` 的 **3 元算术映射**（若把轴序或符号映射错，集合即不符）。
    /// ④ **径向 oracle**：法向必须等于"由体心指向该面最近点"的单位向量
    /// （`n == (p − c).normalize()`）——这条**独立**于多重集断言，能钉住
    /// **循环轴置换**逃逸（如 `&[n1,n2,n0]`：分量仍单位、仍轴对齐、多重集仍
    /// `{±X,±Y,±Z}`，故只有径向 oracle 能打红）。
    ///
    /// # ⚠ 适用范围（禁止外推）
    ///
    /// 径向 oracle 仅在 **"体心到面的最近点方向 == 该面外法向"** 时成立。本钉只用于
    /// `make_box` 的**凸平面**（盒心在体内、最近点在边界，且面外法向即径向）。
    /// **对非凸 / 带孔 / 偏心曲线面不成立**——在该类形体上 `p − center` 未必沿面法向，
    /// 会**假红**。**禁止**把该 oracle 抄到任何非立方体用例。
    ///
    /// 若上游把法向改成"归一化的面朝向任意向量"，②③ 会红——须同步 B4 拾取高亮契约。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn facade_surface_normal_is_unit_and_axis_aligned() {
        let Some(mut b) = require_real_backend("facade_surface_normal_is_unit_and_axis_aligned")
        else {
            return;
        };
        assert_eq!(
            b.kind(),
            BackendKind::Real,
            "钉子必须跑在真实内核上，不许在 mock 上假绿"
        );

        let h = b.make_box(10.0, 10.0, 10.0).unwrap();
        let bb = b.bounding_box(h).unwrap();
        let center = DVec3::new(
            (bb.min[0] + bb.max[0]) / 2.0,
            (bb.min[1] + bb.max[1]) / 2.0,
            (bb.min[2] + bb.max[2]) / 2.0,
        );
        let faces = b.sub_shapes(h, SubShapeKind::Face).unwrap();
        assert_eq!(faces.len(), 6, "立方体应有 6 个面，实得 {}", faces.len());

        // 每个面：投影内部点取 (u,v) → 取法向 → 校验单位 + 轴对齐 + 径向 oracle。
        let mut seen: Vec<(usize, i8)> = Vec::new(); // (轴 0/1/2, 符号 ±1)
        for f in &faces {
            let pof = b
                .project_point_on_face(*f, center)
                .unwrap_or_else(|e| panic!("面投影应命中（面 {f:?}）：{}", error_chain_text(&e)));
            let n = b.surface_normal(*f, pof.u, pof.v).unwrap_or_else(|e| {
                panic!("面 {f:?} 的 surface_normal 应 Ok：{}", error_chain_text(&e))
            });
            // ④ 独立径向 oracle：法向 == 由体心指向面上最近点的单位向量。
            // 体心在盒内、最近点在边界，故该向量朝外，与面法向同向同号。
            let radial = (pof.point - center).normalize();
            assert!(
                (n - radial).length() < 1e-9,
                "面法向应等于'由体心指向面上最近点'的单位向量（径向 oracle，钉方向+符号）：\
                 n={n:?} vs radial={radial:?}，差 {}",
                (n - radial).length()
            );
            let ln = n.length();
            assert!(
                (ln - 1.0).abs() <= 1e-6,
                "面法向应为单位向量（|n|≈1），实得 |n|={ln}（n={n:?}）"
            );
            let comps = [n.x, n.y, n.z];
            let nonzero: Vec<usize> = (0..3).filter(|&i| comps[i].abs() > 1e-9).collect();
            assert_eq!(
                nonzero.len(),
                1,
                "面法向应轴对齐（恰好一个分量非零），实得 n={n:?}"
            );
            let axis = nonzero[0];
            assert!(
                (comps[axis].abs() - 1.0).abs() <= 1e-6,
                "轴对齐法向的唯一非零分量应为 ±1，实得 {:?}（n={n:?}）",
                comps[axis]
            );
            seen.push((axis, if comps[axis] > 0.0 { 1 } else { -1 }));
        }

        // 6 个法向恰好 = {±X, ±Y, ±Z} 各一次（多重集相等）。
        seen.sort_unstable();
        let expected = vec![(0, -1), (0, 1), (1, -1), (1, 1), (2, -1), (2, 1)];
        assert_eq!(
            seen, expected,
            "立方体 6 面法向应恰好是 {{±X,±Y,±Z}} 各一次，实得 {seen:?}"
        );
        println!("立方体 6 面法向（轴,符号）：{seen:?}");
    }

    /// **B4.1b-1 钉子（occt 腿）：`curve_parameters` 门面返回 2 元区间且 `last > first`**。
    ///
    /// 走 `OcctBackend` 公开契约（不碰 `kernel_mut()`）。断言：
    /// ① 立方体每条边都能取到有限参数域且 `last > first`（证明 `CurveRange::from_kernel_vec`
    /// 的 **2 元映射**正确、且区间有序）；
    /// ② **弧长 oracle**：对每条边用 `bounding_box(edge)` 求其 AABB，边是直线段，AABB 退化方向
    /// 长度为 0、**非退化分量长度即该边几何长度 `L`**；断言 `last − first == L`
    /// （QA 已独立证实该内核对直线边按**弧长参数化**，故真域长度必等于边长）。
    ///
    /// ② 是**独立**于 ① 的 oracle：若门面被改成返回**恒定假域**（如 `[0,1]`），
    /// ① 仍绿（`1 > 0`、有限）而 ② 必红（`1 ≠ 10`）——故必须两者并存。
    /// 若上游改变参数化口径（如归一化到 `[0,1]`），② 会红——须同步 `PointOnEdge.param`
    /// 的归一化约定（B4.1b 消费者）。
    ///
    /// # ⚠ 适用范围（禁止外推）
    ///
    /// 弧长 oracle（`|span − L|`，`L` 取 `bounding_box` 的 AABB **非退化延展**）仅在
    /// **"直线且轴对齐"** 的边上成立：
    /// - **圆弧边**：参数域跨度为**角度值**（如 π/2），而 AABB 延展为**弦长量级** → **假红**；
    /// - **非轴对齐直线边**：AABB 延展（各轴投影）**小于**真长 → **假红**。
    ///
    /// 本钉只用于 `make_box` 的 12 条**轴对齐直边**；**禁止**复用到曲线 / 斜置边。
    /// 若需扩到任意直线边，应改用**两端点距离**（或曲线弧长）替代 AABB 延展。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn facade_curve_parameters_returns_two_element_range() {
        let Some(mut b) = require_real_backend("facade_curve_parameters_returns_two_element_range")
        else {
            return;
        };
        assert_eq!(
            b.kind(),
            BackendKind::Real,
            "钉子必须跑在真实内核上，不许在 mock 上假绿"
        );

        let h = b.make_box(10.0, 10.0, 10.0).unwrap();
        let edges = b.sub_shapes(h, SubShapeKind::Edge).unwrap();
        assert!(!edges.is_empty(), "立方体应至少有一条边");
        let mut checked = 0usize;
        for e in &edges {
            let r = b.curve_parameters(*e).unwrap_or_else(|err| {
                panic!(
                    "边 {e:?} 的 curve_parameters 应 Ok：{}",
                    error_chain_text(&err)
                )
            });
            assert!(
                r.first.is_finite() && r.last.is_finite(),
                "曲线参数域应有限，实得 [{}, {}]",
                r.first,
                r.last
            );
            assert!(
                r.last > r.first,
                "曲线参数域应满足 last > first（可用于归一化 param），实得 [{}, {}]",
                r.first,
                r.last
            );
            // ② 独立弧长 oracle：边长 L 由该边 AABB 的非退化分量导出（不写死尺寸）。
            let ebb = b.bounding_box(*e).unwrap_or_else(|err| {
                panic!("边 {e:?} 的 bounding_box 应 Ok：{}", error_chain_text(&err))
            });
            let ext = [
                ebb.max[0] - ebb.min[0],
                ebb.max[1] - ebb.min[1],
                ebb.max[2] - ebb.min[2],
            ];
            let l = ext.iter().copied().fold(0.0_f64, f64::max);
            assert!(
                l > 0.0,
                "直线边的 AABB 应有一个非退化方向（L>0），实得 ext={ext:?}"
            );
            assert!(
                (r.last - r.first - l).abs() < 1e-6,
                "曲线参数域长度应等于边几何长度 L（弧长参数化）：\
                 (last − first)={} vs L={l}（域 [{}, {}]）",
                r.last - r.first,
                r.first,
                r.last
            );
            checked += 1;
        }
        assert!(checked > 0, "应至少校验到一条边的参数域");
        println!("立方体 {checked} 条边的参数域均满足 last > first 且 last−first == 边长");
    }

    /// **B6.1a 钉子（occt 腿）：`curve_point_at_param` 沿直线边按弧长走点**。
    ///
    /// 走 `OcctBackend` 公开契约（不碰 `kernel_mut()`）。断言：
    /// ① 立方体每条边 `curve_parameters = [0, 10]`（实测），`point@first / @last / @mid`
    /// 均 `Ok` —— 证明 3 元映射正确（`dvec3_from_kernel_vec_op`）；
    /// ② **独立 oracle（算术平均）**：直线边参数域中点的像 == 两端点算术平均
    /// （`curve_point_at_param` 若被改成返回恒定假点、或把参数当**归一化** `[0,1]`
    /// 处理，② 必红）；
    /// ③ **独立 oracle（AABB 角点）**：两端点各落在该边 `bounding_box` 的 AABB 角点上
    /// （每轴坐标 == min 或 max）；
    /// ④ 实测 golden：**存在**一条恰为 `(0,0,0) ⇄ (0,0,10)` 的直边，且其中点
    /// `point@5 == (0,0,5)`（不假定该边在 `sub_shapes` 中的下标）。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn facade_curve_point_at_param_walks_a_straight_edge_by_arc_length() {
        let Some(mut b) =
            require_real_backend("facade_curve_point_at_param_walks_a_straight_edge_by_arc_length")
        else {
            return;
        };
        assert_eq!(
            b.kind(),
            BackendKind::Real,
            "钉子必须跑在真实内核上，不许在 mock 上假绿"
        );

        let h = b.make_box(10.0, 10.0, 10.0).unwrap();
        let edges = b.sub_shapes(h, SubShapeKind::Edge).unwrap();
        assert!(!edges.is_empty(), "立方体应至少有一条边");

        let mut saw_origin_z_edge = false;
        let mut checked = 0usize;
        for e in &edges {
            let r = b.curve_parameters(*e).unwrap_or_else(|err| {
                panic!(
                    "边 {e:?} 的 curve_parameters 应 Ok：{}",
                    error_chain_text(&err)
                )
            });
            // ① 实测：立方体每条直边按弧长参数化为 [0, 10]。
            assert!(
                (r.first - 0.0).abs() < 1e-9 && (r.last - 10.0).abs() < 1e-9,
                "立方体直边参数域实测应为 [0, 10]，实得 [{}, {}]",
                r.first,
                r.last
            );
            let p0 = b.curve_point_at_param(*e, r.first).unwrap_or_else(|err| {
                panic!("边 {e:?} 的 point@first 应 Ok：{}", error_chain_text(&err))
            });
            let p1 = b.curve_point_at_param(*e, r.last).unwrap_or_else(|err| {
                panic!("边 {e:?} 的 point@last 应 Ok：{}", error_chain_text(&err))
            });
            let pm = b
                .curve_point_at_param(*e, r.first + (r.last - r.first) * 0.5)
                .unwrap_or_else(|err| {
                    panic!("边 {e:?} 的 point@mid 应 Ok：{}", error_chain_text(&err))
                });

            // ② 独立 oracle：参数域中点的像 == 两端点算术平均（直线边按弧长）。
            assert!(
                (pm - (p0 + p1) * 0.5).length() < 1e-9,
                "直线边参数域中点的像应等于两端点算术平均：pm={pm:?} vs avg={:?}",
                (p0 + p1) * 0.5
            );

            // ③ 独立 oracle：两端点各落在该边 AABB 角点上（每轴 == min 或 max）。
            let bb = b.bounding_box(*e).unwrap_or_else(|err| {
                panic!("边 {e:?} 的 bounding_box 应 Ok：{}", error_chain_text(&err))
            });
            let on_corner =
                |c: f64, lo: f64, hi: f64| (c - lo).abs() < 1e-9 || (c - hi).abs() < 1e-9;
            for (label, p) in [("first", p0), ("last", p1)] {
                let pa = p.to_array();
                assert!(
                    on_corner(pa[0], bb.min[0], bb.max[0])
                        && on_corner(pa[1], bb.min[1], bb.max[1])
                        && on_corner(pa[2], bb.min[2], bb.max[2]),
                    "边 {e:?} 的 point@{label}={pa:?} 应落在 AABB 角点上：AABB=[{:?}, {:?}]",
                    bb.min,
                    bb.max
                );
            }

            // ④ 实测 golden：恰为 (0,0,0) ⇄ (0,0,10) 的直边，其中点应为 (0,0,5)。
            let o = DVec3::new(0.0, 0.0, 0.0);
            let z10 = DVec3::new(0.0, 0.0, 10.0);
            if (p0 - o).length() < 1e-9 && (p1 - z10).length() < 1e-9 {
                assert!(
                    (pm - DVec3::new(0.0, 0.0, 5.0)).length() < 1e-9,
                    "边 (0,0,0)→(0,0,10) 的 point@5 应为 (0,0,5)，实得 {pm:?}"
                );
                saw_origin_z_edge = true;
            }
            checked += 1;
        }
        assert!(checked > 0, "应至少校验到一条边");
        assert!(
            saw_origin_z_edge,
            "应存在一条恰为 (0,0,0)→(0,0,10) 的立方体直边（实测 golden）"
        );
        println!(
            "立方体 {checked} 条直边：curve_point_at_param 按弧长走点、中点=AABB 两端算术平均"
        );
    }

    /// **B6.1a 补钉（occt 腿）：`curve_point_at_param` 在**圆弧边**上按角度参数走点**。
    ///
    /// # 为什么必须补（QA P3）
    ///
    /// `facade_curve_point_at_param_walks_a_straight_edge_by_arc_length` 只覆盖**立方体
    /// 直边**——对直边，"按参数取点"与"两端点线性插值"**结果逐位相同**，故该钉**无法
    /// 区分**二者：QA 构造变异 B1（把 `occt.rs:717` 实现改为两端点线性插值）后直边钉
    /// **全绿逃脱**。本钉用**带孔板孔边（φ15 圆弧边）**补上圆弧覆盖，令"直线假设"式
    /// 退化实现**必红**。
    ///
    /// # 断言（全部独立 oracle，不抄被测实现）
    ///
    /// 对每条**圆弧边**（判定与 `facade_curve_length_matches_geometry` 同口径：
    /// `curve_length > 参数域跨度`）：
    /// ① 取 `[first,last]`、`mid=(first+last)/2`，采样 `p_first/p_mid/p_last`；
    /// ② **弧高 oracle**：`|p_mid − (p_first+p_last)/2| == R·(1 − cos(span/2))`
    ///    （`R=15`、`span` 由 `curve_parameters` **独立**取），容差 1e-6——
    ///    **线性插值令 `p_mid` 落在弦上（弧高 = 0），必红**；并直白断言弧高 `> 1.0mm`
    ///    （封死"弦中点"式退化）；
    /// ③ **半径 oracle**：以三点**外接圆心**（通用几何反解 `circumcenter`，非被测实现）
    ///    求圆心/半径，断言**半径 ≈ 15** 且 **`|p_mid − 圆心| ≈ 15`**，容差 1e-6——
    ///    线性插值三点共线 → 外接圆心退化 → 必红；
    /// ④ 平面性：孔边为**水平**圆弧（采样点 z 恒定）。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn facade_curve_point_at_param_stays_on_arc_edge() {
        let Some(mut b) = require_real_backend("facade_curve_point_at_param_stays_on_arc_edge")
        else {
            return;
        };
        assert_eq!(
            b.kind(),
            BackendKind::Real,
            "钉子必须跑在真实内核上，不许在 mock 上假绿"
        );

        const HOLE_R: f64 = 15.0;
        let plate = b.make_box(100.0, 60.0, 10.0).unwrap();
        let hole = b.make_cylinder(HOLE_R, 40.0).unwrap();
        let holed = b.cut(plate, hole).unwrap();
        let edges = b.sub_shapes(holed, SubShapeKind::Edge).unwrap();
        assert!(!edges.is_empty(), "带孔板应至少有一条边");

        let mut arc_checked = 0usize;
        for e in &edges {
            let r = b.curve_parameters(*e).unwrap_or_else(|err| {
                panic!(
                    "孔板边 {e:?} 的 curve_parameters 应 Ok：{}",
                    error_chain_text(&err)
                )
            });
            let span = r.last - r.first;
            let l = b.curve_length(*e).unwrap_or_else(|err| {
                panic!(
                    "孔板边 {e:?} 的 curve_length 应 Ok：{}",
                    error_chain_text(&err)
                )
            });
            // 圆弧边判定（与 curve_length 钉同口径）：几何长度 > 参数域跨度（角度值）。
            if l <= span + 1e-6 {
                continue;
            }
            // 闭合边（param 域近似整周）另有退化风险，本钉不覆盖、显式跳过。
            if span >= std::f64::consts::TAU - 1e-6 {
                continue;
            }
            let mid = r.first + span * 0.5;
            let p_first = b.curve_point_at_param(*e, r.first).unwrap_or_else(|err| {
                panic!(
                    "孔边 {e:?} 的 point@first 应 Ok：{}",
                    error_chain_text(&err)
                )
            });
            let p_last = b.curve_point_at_param(*e, r.last).unwrap_or_else(|err| {
                panic!("孔边 {e:?} 的 point@last 应 Ok：{}", error_chain_text(&err))
            });
            let p_mid = b.curve_point_at_param(*e, mid).unwrap_or_else(|err| {
                panic!("孔边 {e:?} 的 point@mid 应 Ok：{}", error_chain_text(&err))
            });

            // ② 弧高 oracle：|p_mid − 弦中点| == R·(1 − cos(span/2))。
            let chord_mid = (p_first + p_last) * 0.5;
            let sag = (p_mid - chord_mid).length();
            let sag_expected = HOLE_R * (1.0 - (span * 0.5).cos());
            assert!(
                (sag - sag_expected).abs() < 1e-6,
                "圆弧边中点的弧高应 == R·(1−cos(span/2))：sag={sag} vs 解析={sag_expected}\
                 （span={span}）——线性插值退化会令 sag=0"
            );
            assert!(
                sag > 1.0,
                "圆弧边中点必须显著偏离弦中点：sag={sag} 应 > 1.0（弦中点距圆心仅 ≈10.6，弧中点=15）"
            );

            // ③ 半径 oracle：三点外接圆心 → 半径 ≈ 15 且 |p_mid − 圆心| ≈ 15。
            let (center, radius) = circumcenter(
                DVec2::new(p_first.x, p_first.y),
                DVec2::new(p_mid.x, p_mid.y),
                DVec2::new(p_last.x, p_last.y),
            )
            .unwrap_or_else(|| {
                panic!("圆弧边 {e:?} 的三采样点应不共线（外接圆心存在）——线性插值退化会令其共线")
            });
            assert!(
                (radius - HOLE_R).abs() < 1e-6,
                "孔边外接圆半径应 ≈ {HOLE_R}，实得 {radius}"
            );
            let p_mid_xy = DVec2::new(p_mid.x, p_mid.y);
            assert!(
                ((p_mid_xy - center).length() - HOLE_R).abs() < 1e-6,
                "孔边中点应落在半径 {HOLE_R} 的圆弧上：|p_mid − 圆心|={} vs {HOLE_R}",
                (p_mid_xy - center).length()
            );

            // ④ 平面性：孔边为水平圆弧（采样点 z 恒定）。
            assert!(
                (p_first.z - p_mid.z).abs() < 1e-6 && (p_last.z - p_mid.z).abs() < 1e-6,
                "孔边应为水平圆弧（采样点 z 恒定）：{:?}/{:?}/{:?}",
                p_first.z,
                p_mid.z,
                p_last.z
            );
            arc_checked += 1;
        }
        assert!(
            arc_checked > 0,
            "带孔板应至少有一条圆弧边——否则本钉未覆盖圆弧（正是 B1 逃脱的前提）"
        );
        println!(
            "curve_point_at_param 圆弧覆盖：{arc_checked} 条孔边中点弧高 == 解析值、半径 == {HOLE_R}"
        );
    }

    /// **B6.1a 钉子（occt 腿）：`curve_length` 返回真几何长度（非参数域跨度）**。
    ///
    /// 走 `OcctBackend` 公开契约。断言：
    /// ① **立方体直边**：`curve_length ≈ 10`（容差 1e-9），独立 oracle = 该边 AABB
    /// 的非退化延展（轴对齐直边，AABB 最大延展即边长）；
    /// ② **带孔板孔边（圆弧边）**：`curve_length ≈ R·span`（`R=15`、`span = last − first`
    /// 由 `curve_parameters` **独立**取，实测 `≈ π/2`），容差 1e-6。
    /// 这是本钉的**核心**：圆弧边的参数域跨度是**角度值**（1.571），而几何长度是
    /// **弧长**（23.562）——若 `curve_length` 被误实现为返回 `last − first`，② 必红。
    /// 同时断言 `length > span`，证明二者**不是**同一个量（防"量纲混入"）。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn facade_curve_length_matches_geometry() {
        let Some(mut b) = require_real_backend("facade_curve_length_matches_geometry") else {
            return;
        };
        assert_eq!(
            b.kind(),
            BackendKind::Real,
            "钉子必须跑在真实内核上，不许在 mock 上假绿"
        );

        // ① 立方体直边：length == AABB 非退化延展。
        let h = b.make_box(10.0, 10.0, 10.0).unwrap();
        let edges = b.sub_shapes(h, SubShapeKind::Edge).unwrap();
        assert!(!edges.is_empty(), "立方体应至少有一条边");
        let mut straight_checked = 0usize;
        for e in &edges {
            let r = b.curve_parameters(*e).unwrap();
            let l = b.curve_length(*e).unwrap_or_else(|err| {
                panic!("边 {e:?} 的 curve_length 应 Ok：{}", error_chain_text(&err))
            });
            assert!(
                (l - 10.0).abs() < 1e-9,
                "立方体直边 curve_length 应 ≈ 10，实得 {l}（域 [{}, {}]）",
                r.first,
                r.last
            );
            // 独立 oracle：轴对齐直边的 AABB 最大延展即其几何长度。
            let ebb = b.bounding_box(*e).unwrap();
            let ext = [
                ebb.max[0] - ebb.min[0],
                ebb.max[1] - ebb.min[1],
                ebb.max[2] - ebb.min[2],
            ];
            let l_oracle = ext.iter().copied().fold(0.0_f64, f64::max);
            assert!(
                (l - l_oracle).abs() < 1e-9,
                "curve_length 应与 AABB 非退化延展（边长）一致：l={l} vs oracle={l_oracle}"
            );
            straight_checked += 1;
        }
        assert!(straight_checked > 0, "应至少校验到一条直边");

        // ② 带孔板孔边：length == R·span（span 由 curve_parameters 独立取）。
        const HOLE_R: f64 = 15.0;
        let plate = b.make_box(100.0, 60.0, 10.0).unwrap();
        let hole = b.make_cylinder(HOLE_R, 40.0).unwrap();
        let holed = b.cut(plate, hole).unwrap();
        let hedges = b.sub_shapes(holed, SubShapeKind::Edge).unwrap();
        assert!(!hedges.is_empty(), "带孔板应至少有一条边");
        let mut arc_checked = 0usize;
        for e in &hedges {
            let r = b.curve_parameters(*e).unwrap();
            let span = r.last - r.first;
            let l = b.curve_length(*e).unwrap_or_else(|err| {
                panic!(
                    "孔板边 {e:?} 的 curve_length 应 Ok：{}",
                    error_chain_text(&err)
                )
            });
            // 圆弧边判定：几何长度显著大于参数域跨度（角度值）。
            if l > span + 1e-6 {
                assert!(
                    (l - HOLE_R * span).abs() < 1e-6,
                    "孔边弧长应 = R·span（R=15）：l={l} vs R·span={}（span={span}）",
                    HOLE_R * span
                );
                // 实测：φ15 弧长 ≈ 15·π/2 ≈ 23.5619。
                assert!(
                    (l - 23.561_944_902).abs() < 1e-6,
                    "孔边弧长应 ≈ 23.5619，实得 {l}"
                );
                arc_checked += 1;
            }
        }
        assert!(
            arc_checked > 0,
            "带孔板应至少有一条圆弧边（curve_length 显著大于参数域跨度）——\
             否则 curve_length 未真正返回几何长度"
        );
        println!(
            "curve_length：直边 {straight_checked} 条 == AABB 延展；孔边 {arc_checked} 条 == R·span（弧长）"
        );
    }

    /// **R16 同序钉子（occt 腿，B4.1b-1 硬前置）**：`sub_shapes(Face)` 的顺序与
    /// `face_hashes(shape)` 的顺序**逐位一致**。
    ///
    /// # 为什么必须钉
    ///
    /// 拾取粗筛（B4.1）用 `mesh.face_groups[].face_hash` 反查面；若 `sub_shapes(Face)`
    /// 的枚举顺序与 `face_hashes` 的顺序**不同**，哈希→句柄的映射就会**静默错位**
    /// （看着成功、实则捡错面）。故须显式证明两者同序。
    ///
    /// 断言：对每个 `i`，单独对 `faces[i]` 取哈希应得 `vec![hashes[i]]`。
    /// **反向对照**（仅在文档中说明，不进断言）：若顺序被错开一位，
    /// `faces[i]` 的哈希会等于 `hashes[(i+1)%6]` 而非 `hashes[i]`，本用例即红。
    /// **若真的红了 = 真实发现（R16 不成立），不得放宽断言、不得删用例**——
    /// 应如实上报实测顺序，由 B4.1 粗筛按实测顺序适配。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn sub_faces_are_same_order_as_face_hashes() {
        let Some(mut b) = require_real_backend("sub_faces_are_same_order_as_face_hashes") else {
            return;
        };
        assert_eq!(
            b.kind(),
            BackendKind::Real,
            "钉子必须跑在真实内核上，不许在 mock 上假绿"
        );

        let h = b.make_box(10.0, 10.0, 10.0).unwrap();
        let hashes = b.face_hashes(h).unwrap();
        let faces = b.sub_shapes(h, SubShapeKind::Face).unwrap();
        assert_eq!(
            hashes.len(),
            faces.len(),
            "face_hashes 与 sub_shapes(Face) 数量应一致（{}/{})",
            hashes.len(),
            faces.len()
        );

        for (i, f) in faces.iter().enumerate() {
            let own = b.face_hashes(*f).unwrap_or_else(|e| {
                panic!("单面 {f:?} 的 face_hashes 应 Ok：{}", error_chain_text(&e))
            });
            assert_eq!(
                own,
                vec![hashes[i]],
                "faces[{i}] 的哈希应与 hashes[{i}] 一致（R16 同序）；\
                 若不符 = 真实发现，须按实测顺序适配粗筛，不得放宽本条"
            );
        }
        println!("R16 同序成立：{n} 个面逐一匹配", n = faces.len());
    }

    // =========================================================================
    // B6.2a 八算子真实内核钉（每算子 ≥1；独立 oracle + 落盘 BREP 变 + ≥2 参数组）
    // =========================================================================
    //
    // **独立 oracle**：不用「被测算子自己算出的量」当断言依据，而是**解析式**算出
    // 期望（体积 / 包围盒跨度），再与内核结果比对——否则「只对某一特例成立的退化
    // 实现」会逃逸（B6.0 五层复核教训）。每组用 **≥2 组不同参数**。
    //
    // ⚠ 夹具用 `kernel_mut()` 直接构造 wire（放样截面 / 扫掠脊线），再 `alloc` 成
    // 门面句柄——这是**测试内**合法用法（`alloc` 在本模块私有，子模块可访问）。

    /// 包围盒三轴跨度。
    fn bbox_span(b: &mut OcctBackend, h: ShapeHandle) -> [f64; 3] {
        let bb = b.bounding_box(h).unwrap();
        [
            bb.max[0] - bb.min[0],
            bb.max[1] - bb.min[1],
            bb.max[2] - bb.min[2],
        ]
    }

    /// 在 `sx×sy×sz` 盒上取 `z=0` 与 `z=sz` 两面的 outer_wire（**放样截面**）。
    fn horizontal_section_wires(
        b: &mut OcctBackend,
        sx: f64,
        sy: f64,
        sz: f64,
    ) -> (ShapeHandle, ShapeHandle) {
        let (w0, w1) = {
            let mk = b.kernel_mut();
            let bx = mk.make_box(sx, sy, sz).unwrap();
            let fs = mk.get_sub_shapes(bx, "face").unwrap();
            let mut bottom = None;
            let mut top = None;
            for f in fs {
                let bb = mk.get_bounding_box(f, false).unwrap();
                if bb.min.z.abs() < 1e-9 && bb.max.z.abs() < 1e-9 {
                    bottom = Some(f);
                }
                if (bb.min.z - sz).abs() < 1e-9 && (bb.max.z - sz).abs() < 1e-9 {
                    top = Some(f);
                }
            }
            (
                mk.outer_wire(bottom.expect("应找到底面 wire")).unwrap(),
                mk.outer_wire(top.expect("应找到顶面 wire")).unwrap(),
            )
        };
        (b.alloc(w0), b.alloc(w1))
    }

    /// `profile` = 固定 10×10 截面（盒 `x=0` 面的 outer_wire，位于 YZ 平面）；
    /// `spine` = 沿 +X、起点在原点、长 `len` 的直线 wire。
    /// 返回的门面句柄面积 oracle = 100，长度 = `len`。
    fn profile_and_spine(b: &mut OcctBackend, len: f64) -> (ShapeHandle, ShapeHandle) {
        let (pw, sw) = {
            let mk = b.kernel_mut();
            let pb = mk.make_box(10.0, 10.0, 10.0).unwrap();
            let pfs = mk.get_sub_shapes(pb, "face").unwrap();
            let mut xf = None;
            for f in pfs {
                let bb = mk.get_bounding_box(f, false).unwrap();
                if bb.min.x.abs() < 1e-9 && bb.max.x.abs() < 1e-9 {
                    xf = Some(f);
                }
            }
            let pw = mk.outer_wire(xf.expect("应找到 x=0 面 wire")).unwrap();

            let sb = mk.make_box(len, len, len).unwrap();
            let es = mk.get_sub_shapes(sb, "edge").unwrap();
            let mut se = None;
            for e in es {
                let bb = mk.get_bounding_box(e, false).unwrap();
                if bb.min.y.abs() < 1e-9
                    && bb.max.y.abs() < 1e-9
                    && bb.min.z.abs() < 1e-9
                    && bb.max.z.abs() < 1e-9
                    && (bb.max.x - len).abs() < 1e-9
                {
                    se = Some(e);
                }
            }
            let sw = mk.make_wire(&[se.expect("应找到沿 X 的直边")]).unwrap();
            (pw, sw)
        };
        (b.alloc(pw), b.alloc(sw))
    }

    /// **B6.2a occt 钉：`fillet` 真把一条棱倒圆**。
    ///
    /// 独立 oracle：`10³` 盒在一条长 `L=10` 的棱上倒 `r` 圆 → 去除体积
    /// `(1 − π/4)·r²·L`；面数由 6 增至 >6；BREP 文本必变。**2 组参数**（边 0/r=1、边 1/r=2）。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn fillet_actually_rounds_an_edge() {
        let Some(mut b) = try_backend() else { return };
        let bx = b.make_box(10.0, 10.0, 10.0).unwrap();
        let edges = b.sub_shapes(bx, SubShapeKind::Edge).unwrap();
        assert_eq!(edges.len(), 12, "立方体应有 12 条棱");
        let v0 = b.volume(bx).unwrap();
        let brep0 = b.export_brep(bx).unwrap();

        for (idx, r) in [(0usize, 1.0f64), (1, 2.0)] {
            let out = b.fillet(bx, &[edges[idx]], r).unwrap();
            let v = b.volume(out).unwrap();
            // 独立 oracle：被削去的体积 = (1 - π/4)·r²·L
            let removed = (1.0 - std::f64::consts::FRAC_PI_4) * r * r * 10.0;
            assert!(
                v < v0,
                "倒圆应减小体积：倒圆前 {v0:.4}，倒圆后 {v:.4}（边{idx} r={r}）"
            );
            assert!(
                (v0 - v - removed).abs() < 0.01,
                "倒圆去除体积应≈(1−π/4)r²L={removed:.4}，实得 {:.4}（边{idx} r={r}）",
                v0 - v
            );
            assert!(
                b.sub_shape_count(out, SubShapeKind::Face).unwrap() > 6,
                "倒圆后应新增面（>6），实得 {}（边{idx} r={r}）",
                b.sub_shape_count(out, SubShapeKind::Face).unwrap()
            );
            assert_ne!(
                b.export_brep(out).unwrap(),
                brep0,
                "倒圆后落盘 BREP 文本必须变化（边{idx} r={r}）"
            );
        }
    }

    /// **B6.2a occt 钉：`chamfer` 真切掉一个角**。
    ///
    /// 独立 oracle：`10³` 盒在一条长 `L=10` 的棱上切 `d` 倒角 → 去除体积
    /// `0.5·d²·L`；面数 >6；BREP 变。**2 组参数**（边 0/d=1、边 1/d=2）。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn chamfer_actually_cuts_a_corner() {
        let Some(mut b) = try_backend() else { return };
        let bx = b.make_box(10.0, 10.0, 10.0).unwrap();
        let edges = b.sub_shapes(bx, SubShapeKind::Edge).unwrap();
        let v0 = b.volume(bx).unwrap();
        let brep0 = b.export_brep(bx).unwrap();

        for (idx, d) in [(0usize, 1.0f64), (1, 2.0)] {
            let out = b.chamfer(bx, &[edges[idx]], d).unwrap();
            let v = b.volume(out).unwrap();
            let removed = 0.5 * d * d * 10.0; // 三角棱柱：½·d·d·L
            assert!(v < v0, "倒角应减小体积（边{idx} d={d}）");
            assert!(
                (v0 - v - removed).abs() < 0.01,
                "倒角去除体积应≈½d²L={removed:.4}，实得 {:.4}（边{idx} d={d}）",
                v0 - v
            );
            assert!(
                b.sub_shape_count(out, SubShapeKind::Face).unwrap() > 6,
                "倒角后应新增面（>6），实得 {}（边{idx} d={d}）",
                b.sub_shape_count(out, SubShapeKind::Face).unwrap()
            );
            assert_ne!(
                b.export_brep(out).unwrap(),
                brep0,
                "倒角后落盘 BREP 文本必须变化（边{idx} d={d}）"
            );
        }
    }

    /// **B6.2a occt 钉：`shell` 真把盒挖空**。
    ///
    /// 独立 oracle：`10³` 盒开一 `10×10` 面、壁厚 `t` 抽壳 → 体积
    /// `1000 − (10−2t)²·(10−t)`（t=1 → 424；t=2 → 712）；产出恰 1 个实体；BREP 变。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn shell_actually_hollows_a_box() {
        let Some(mut b) = try_backend() else { return };
        let bx = b.make_box(10.0, 10.0, 10.0).unwrap();
        let faces = b.sub_shapes(bx, SubShapeKind::Face).unwrap();
        assert!(!faces.is_empty());
        let brep0 = b.export_brep(bx).unwrap();

        for t in [1.0f64, 2.0] {
            let out = b.shell(bx, &[faces[0]], t, 1e-6).unwrap();
            let v = b.volume(out).unwrap();
            let inner = (10.0 - 2.0 * t) * (10.0 - 2.0 * t) * (10.0 - t);
            let expected = 1000.0 - inner;
            assert!(
                (v - expected).abs() < 1.0,
                "抽壳体积应≈1000−(10−2t)²(10−t)={expected:.2}，实得 {v:.4}（t={t}）"
            );
            assert_eq!(
                b.sub_shape_count(out, SubShapeKind::Solid).unwrap(),
                1,
                "抽壳应产出恰 1 个实体（t={t}）"
            );
            assert_ne!(
                b.export_brep(out).unwrap(),
                brep0,
                "抽壳后落盘 BREP 文本必须变化（t={t}）"
            );
        }
    }

    /// **B6.2a occt 钉：`offset` 真把盒外扩**。
    ///
    /// 独立 oracle：`10³` 盒外扩 `d` → 包围盒各轴跨度 `10 + 2d`（t=1 → 12；t=2 → 14）；
    /// 体积增大；BREP 变。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn offset_actually_grows_the_box() {
        let Some(mut b) = try_backend() else { return };
        let bx = b.make_box(10.0, 10.0, 10.0).unwrap();
        let v0 = b.volume(bx).unwrap();
        let brep0 = b.export_brep(bx).unwrap();

        for d in [1.0f64, 2.0] {
            let out = b.offset(bx, d, 1e-6).unwrap();
            let span = bbox_span(&mut b, out);
            let want = 10.0 + 2.0 * d;
            for (i, s) in span.iter().enumerate() {
                assert!(
                    (s - want).abs() < 1e-6,
                    "外扩 d={d} 后第{i}轴包围盒跨度应≈{want}，实得 {s}"
                );
            }
            assert!(b.volume(out).unwrap() > v0, "外扩应增大体积（d={d}）");
            assert_ne!(
                b.export_brep(out).unwrap(),
                brep0,
                "外扩后落盘 BREP 文本必须变化（d={d}）"
            );
        }
    }

    /// **B6.2a occt 钉：`thicken` 真把面加厚成实体**。
    ///
    /// 独立 oracle：`10×10` 面加厚 `t` → 产实体、体积≈`100·t`（t=1 → 100；t=2 → 200）；
    /// BREP 变。
    ///
    /// ⚠ 本钉输入取**平面面**（最简单的「面 / 壳」输入；闭合壳路径**不宣称**已钉住）。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn thicken_turns_a_shell_into_a_solid() {
        let Some(mut b) = try_backend() else { return };
        let bx = b.make_box(10.0, 10.0, 10.0).unwrap();
        let faces = b.sub_shapes(bx, SubShapeKind::Face).unwrap();
        let f = faces[0];
        let brep_face = b.export_brep(f).unwrap();

        for t in [1.0f64, 2.0] {
            let out = b.thicken(f, t, 1e-6).unwrap();
            let v = b.volume(out).unwrap();
            let expected = 100.0 * t;
            assert!(
                (v - expected).abs() < 1.0,
                "加厚体积应≈面积·t=100·t={expected:.2}，实得 {v:.4}（t={t}）"
            );
            assert_eq!(
                b.sub_shape_count(out, SubShapeKind::Solid).unwrap(),
                1,
                "加厚应产出恰 1 个实体（t={t}）"
            );
            assert_ne!(
                b.export_brep(out).unwrap(),
                brep_face,
                "加厚后落盘 BREP 文本必须变化（t={t}）"
            );
        }
    }

    /// **B6.2a occt 钉：`loft` 真由两个截面放样成体**。
    ///
    /// 独立 oracle：`10×10` 方截面在 `z=0` 与 `z=H` 之间放样 → 体积≈`100·H`、
    /// 包围盒 z 跨度≈`H`；BREP 非空。**2 组高度**（H=10 → 1000；H=20 → 2000）。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn loft_actually_builds_a_solid() {
        let Some(mut b) = try_backend() else { return };
        for h in [10.0f64, 20.0] {
            let (w0, w1) = horizontal_section_wires(&mut b, 10.0, 10.0, h);
            let out = b.loft(&[w0, w1], true, false).unwrap();
            let v = b.volume(out).unwrap();
            let expected = 100.0 * h;
            assert!(
                (v - expected).abs() < 1.0,
                "放样体积应≈截面·高=100·H={expected:.2}，实得 {v:.4}（H={h}）"
            );
            let span = bbox_span(&mut b, out);
            assert!(
                (span[2] - h).abs() < 1e-6,
                "放样包围盒 z 跨度应≈H={h}，实得 {}",
                span[2]
            );
            assert!(
                !b.export_brep(out).unwrap().is_empty(),
                "放样结果 BREP 不应为空"
            );
        }
    }

    /// **B6.2a occt 钉：`sweep` 真沿脊线扫掠**。
    ///
    /// 独立 oracle：`10×10` 截面沿长 `L` 的直线脊线扫掠 → 体积≈`100·L`；
    /// 包围盒沿脊线（+X）延展跨度≈`L`；BREP 变（≠ 脊线文本）。**2 组长度**（10 / 20）。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn sweep_actually_sweeps_along_a_spine() {
        let Some(mut b) = try_backend() else { return };
        for len in [10.0f64, 20.0] {
            let (prof, spine) = profile_and_spine(&mut b, len);
            let spine_brep = b.export_brep(spine).unwrap();
            let out = b.sweep(prof, spine, 0).unwrap();
            let v = b.volume(out).unwrap();
            let expected = 100.0 * len;
            assert!(
                (v - expected).abs() < 1.0,
                "扫掠体积应≈截面·脊长=100·L={expected:.2}，实得 {v:.4}（L={len}）"
            );
            let span = bbox_span(&mut b, out);
            assert!(
                (span[0] - len).abs() < 1e-6,
                "扫掠包围盒应沿脊线(+X)延展到 {len}，实得 {}",
                span[0]
            );
            assert_ne!(
                b.export_brep(out).unwrap(),
                spine_brep,
                "扫掠结果 BREP 必须区别于脊线（L={len}）"
            );
        }
    }

    /// **B6.2a occt 钉：`pipe` 真沿脊线扫成管**。
    ///
    /// 独立 oracle：`10×10` 截面沿长 `L` 直线脊线成管 → 体积 `>0` 且 `<100·L`（非实心块）；
    /// 包围盒沿 +X 延展跨度≈`L`；BREP 变。**2 组长度**（10 / 20）。
    ///
    /// ⚠ 实测 `pipe` 的 `sub_shape_count(Solid)` 为 0（非实体记账），故本钉只断言
    /// 体积 / 包围盒，**不宣称**其实体性（见报告「不宣称」）。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn pipe_actually_pipes_profile_along_spine() {
        let Some(mut b) = try_backend() else { return };
        for len in [10.0f64, 20.0] {
            let (prof, spine) = profile_and_spine(&mut b, len);
            let spine_brep = b.export_brep(spine).unwrap();
            let out = b.pipe(prof, spine).unwrap();
            let v = b.volume(out).unwrap();
            assert!(v > 0.0, "管道体积应 >0（L={len}）");
            assert!(
                v < 100.0 * len,
                "管道应为空心管、体积应 <实心块 100·L={:.1}，实得 {v:.4}（L={len}）",
                100.0 * len
            );
            let span = bbox_span(&mut b, out);
            assert!(
                (span[0] - len).abs() < 1e-6,
                "管道包围盒应沿脊线(+X)延展到 {len}，实得 {}",
                span[0]
            );
            assert_ne!(
                b.export_brep(out).unwrap(),
                spine_brep,
                "管道结果 BREP 必须区别于脊线（L={len}）"
            );
        }
    }

    // =========================================================================
    // M7a 线框生产 trait 契约钉（occt 腿；断言 trait 层产出**真实**线/面几何）
    // =========================================================================

    /// **M7a occt 钉 1：线框生产经 trait 产出真实几何**。
    ///
    /// 4 条直边 → `make_wire` → `sub_shape_count(Wire) ≥ 1` + 包围盒跨度
    /// `[10,10,0]`（独立 oracle）+ `export_brep` 非空；wire → `make_face` →
    /// `sub_shape_count(Face) ≥ 1` + `surface_area ≈ 矩形面积`（独立 oracle：
    /// 10×10 = 100）；face → `outer_wire` → BREP 可往返且包围盒与原 wire 一致。
    ///
    /// ⚠ **实测适配**：真实内核上 **wire 的 `surface_area` = 0**（线无面积，
    /// GProp 面属性对 1 维形状恒 0）——wire 的数值校验用**包围盒跨度**与
    /// `sub_shape_count`，不用面积（规格「surface_area>0」判据与实测不符，
    /// 见偏差清单）。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn wire_ops_build_true_geometry_through_trait() {
        let Some(mut b) = try_backend() else { return };
        assert_eq!(
            b.kind(),
            BackendKind::Real,
            "钉子必须跑在真实内核上，不许在 mock 上假绿"
        );

        // 10×10 矩形四条直边（XY 平面、z=0，逆时针序）
        let e0 = b.make_line_edge(0.0, 0.0, 0.0, 10.0, 0.0, 0.0).unwrap();
        let e1 = b.make_line_edge(10.0, 0.0, 0.0, 10.0, 10.0, 0.0).unwrap();
        let e2 = b.make_line_edge(10.0, 10.0, 0.0, 0.0, 10.0, 0.0).unwrap();
        let e3 = b.make_line_edge(0.0, 10.0, 0.0, 0.0, 0.0, 0.0).unwrap();

        // wire：真实几何 —— sub_shape_count(Wire) ≥ 1、包围盒 [10,10,0]、BREP 非空
        let wire = b.make_wire(&[e0, e1, e2, e3]).unwrap();
        assert!(
            b.sub_shape_count(wire, SubShapeKind::Wire).unwrap() >= 1,
            "wire 的 sub_shape_count(Wire) 应 ≥ 1，实得 {}",
            b.sub_shape_count(wire, SubShapeKind::Wire).unwrap()
        );
        let wire_span = bbox_span(&mut b, wire);
        assert!(
            (wire_span[0] - 10.0).abs() < 1e-6
                && (wire_span[1] - 10.0).abs() < 1e-6
                && wire_span[2].abs() < 1e-6,
            "wire 包围盒跨度应为 [10,10,0]，实得 {wire_span:?}"
        );
        let wire_brep = b.export_brep(wire).unwrap();
        assert!(!wire_brep.is_empty(), "wire 的 BREP 不应为空");

        // face：独立 oracle —— 10×10 矩形面积 = 100
        let face = b.make_face(wire).unwrap();
        let face_area = b.surface_area(face).unwrap();
        assert!(
            (face_area - 100.0).abs() < 1e-6,
            "make_face 面积应≈10×10=100，实得 {face_area:.4}"
        );
        assert!(
            b.sub_shape_count(face, SubShapeKind::Face).unwrap() >= 1,
            "face 的 sub_shape_count(Face) 应 ≥ 1，实得 {}",
            b.sub_shape_count(face, SubShapeKind::Face).unwrap()
        );

        // outer_wire：BREP 可往返（相容性），且包围盒与原 wire 一致
        let outer = b.outer_wire(face).unwrap();
        let outer_brep = b.export_brep(outer).unwrap();
        assert!(!outer_brep.is_empty(), "外环 BREP 不应为空");
        let back = b.import_brep(&outer_brep).unwrap();
        assert_eq!(
            b.export_brep(back).unwrap(),
            outer_brep,
            "外环 BREP 应可往返"
        );
        let outer_span = bbox_span(&mut b, outer);
        assert!(
            (outer_span[0] - wire_span[0]).abs() < 1e-6
                && (outer_span[1] - wire_span[1]).abs() < 1e-6,
            "外环与原 wire 包围盒应一致（{outer_span:?} vs {wire_span:?}）"
        );
    }

    /// **M7a occt 钉 2：三点弧是真实的圆弧边**。
    ///
    /// 独立 oracle：半圆（r=10，圆心原点，XZ 平面，三点 (−10,0,0) / (0,0,10) /
    /// (10,0,0)——中间点在圆上）→ `curve_length ≈ π·10`（若内核退化成弦边则
    /// 长度为 20，本钉精确打红）。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn arc_edge_is_a_true_three_point_arc() {
        let Some(mut b) = try_backend() else { return };
        let arc = b
            .make_arc_edge(-10.0, 0.0, 0.0, 0.0, 0.0, 10.0, 10.0, 0.0, 0.0)
            .unwrap();
        let len = b.curve_length(arc).unwrap();
        let want = std::f64::consts::PI * 10.0;
        assert!(
            (len - want).abs() < 1e-6,
            "半圆弧长应≈π·10={want:.6}，实得 {len:.6}"
        );
        assert!(
            (len - 20.0).abs() > 1.0,
            "弧长必须区别于弦长 20（真弧判定，实得 {len:.6}）"
        );
    }

    /// **M7a occt 钉 3：空边集响亮拒绝**（与 `loft` 空截面同款
    /// [`KernelError::InvalidArgument`]，绝不静默成功）。
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn make_wire_refuses_empty_edges() {
        let Some(mut b) = try_backend() else { return };
        assert!(
            matches!(b.make_wire(&[]), Err(KernelError::InvalidArgument(_))),
            "空边集应返回 InvalidArgument（响亮拒绝，绝不静默成功）"
        );
    }
}

//! 内核门面。
//!
//! [`Kernel`] 是上层唯一接触的几何入口，内部持有一个 [`KernelBackend`]。
//! 上层**永远不会**看到 `occt_wasm::` 的任何类型（铁律 1）。

use std::collections::BTreeSet;
use std::fmt;

use crate::backend::{BackendKind, KernelBackend};
use crate::error::{KernelError, KernelResult};
use crate::handle::ShapeHandle;
use crate::mock::MockBackend;
use crate::types::{
    BoundingBox, CurveRange, DVec3, Evolution, MeshData, PointOnEdge, PointOnFace, Projection,
    SubShapeKind, ViewDef,
};

/// 几何内核门面。
pub struct Kernel {
    backend: Box<dyn KernelBackend>,
}

impl fmt::Debug for Kernel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Kernel")
            .field("backend", &self.backend.name())
            .field("kind", &self.backend.kind())
            .field("live_shapes", &self.backend.live_count())
            .finish()
    }
}

impl Kernel {
    /// 创建**真实后端**的内核实例。
    ///
    /// # 状态（M0 已接线，2026-09）
    ///
    /// 启用 `--features occt` 时返回 [`OcctBackend`](crate::occt::OcctBackend)
    /// （真实 `occt-wasm` v4 + wasmtime）；未启用该 feature 时返回
    /// [`KernelError::Unsupported`]，提示改用 [`Kernel::mock`]。
    ///
    /// # ⚠ 性能
    ///
    /// 真实后端首次创建要解压并 JIT 编译约 21 MB WASM，**耗时 100–500 ms**，
    /// 且在 **debug 构建下慢约 100 倍**。因此：
    ///
    /// - 业务代码请用 [`crate::pool::with_kernel`]（**每线程只建一次**）；
    /// - 验证真实后端请用 `cargo test -p core-kernel --features occt --release`。
    pub fn new() -> KernelResult<Self> {
        #[cfg(feature = "occt")]
        {
            Ok(Self::with_backend(crate::occt::OcctBackend::new()?))
        }
        #[cfg(not(feature = "occt"))]
        {
            Err(crate::error::KernelError::Unsupported(
                "未启用 occt 后端：请用 `--features occt` 构建，或用 Kernel::mock()",
            ))
        }
    }

    /// 用指定后端构造内核。
    pub fn with_backend(backend: impl KernelBackend + 'static) -> Self {
        Self {
            backend: Box::new(backend),
        }
    }

    /// 构造一个占位后端的内核（供 M0 之前的开发与测试）。
    #[must_use]
    pub fn mock() -> Self {
        Self::with_backend(MockBackend::new())
    }

    /// 后端名称。
    #[must_use]
    pub fn backend_name(&self) -> &'static str {
        self.backend.name()
    }

    /// 后端**可信度**：真实几何内核，还是简化的占位后端。
    ///
    /// 上层（尤其 [`crate::pool`]）必须用它来分辨"这话能不能当真"，
    /// 而不是靠后端名字符串猜。见 [`BackendKind`]。
    #[must_use]
    pub fn kind(&self) -> BackendKind {
        self.backend.kind()
    }

    /// 是否真实几何内核（`kind() == Real` 的便捷写法）。
    #[must_use]
    pub fn is_real(&self) -> bool {
        self.kind().is_real()
    }

    // ---- 基元 ----------------------------------------------------------------

    /// 长方体，尺寸 `dx × dy × dz`（mm）。
    pub fn make_box(&mut self, dx: f64, dy: f64, dz: f64) -> KernelResult<ShapeHandle> {
        self.backend.make_box(dx, dy, dz)
    }

    /// 球，半径 `r`（mm）。
    pub fn make_sphere(&mut self, r: f64) -> KernelResult<ShapeHandle> {
        self.backend.make_sphere(r)
    }

    /// 圆柱，半径 `r`、高 `h`（mm）。
    pub fn make_cylinder(&mut self, r: f64, h: f64) -> KernelResult<ShapeHandle> {
        self.backend.make_cylinder(r, h)
    }

    /// 圆锥 / 圆台，下底半径 `r1`、上底半径 `r2`、高 `h`（mm）。
    pub fn make_cone(&mut self, r1: f64, r2: f64, h: f64) -> KernelResult<ShapeHandle> {
        self.backend.make_cone(r1, r2, h)
    }

    /// 圆环，主半径 `major`、次半径 `minor`（mm）。
    pub fn make_torus(&mut self, major: f64, minor: f64) -> KernelResult<ShapeHandle> {
        self.backend.make_torus(major, minor)
    }

    // ---- 轮廓 → 体 -----------------------------------------------------------

    /// 拉伸：把轮廓沿 `(dx, dy, dz)`（mm）拉成体。
    pub fn extrude(
        &mut self,
        shape: ShapeHandle,
        dx: f64,
        dy: f64,
        dz: f64,
    ) -> KernelResult<ShapeHandle> {
        self.backend.extrude(shape, dx, dy, dz)
    }

    /// 旋转（轮廓 → 体）：绕点、轴、角（rad）扫掠。
    #[allow(clippy::too_many_arguments)]
    pub fn revolve(
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
        self.backend
            .revolve(shape, px, py, pz, dx, dy, dz, angle_rad)
    }

    // ---- 布尔 ----------------------------------------------------------------

    /// 融合。
    pub fn fuse(&mut self, a: ShapeHandle, b: ShapeHandle) -> KernelResult<ShapeHandle> {
        self.backend.fuse(a, b)
    }

    /// 切除。
    pub fn cut(&mut self, a: ShapeHandle, b: ShapeHandle) -> KernelResult<ShapeHandle> {
        self.backend.cut(a, b)
    }

    /// 求交。
    pub fn common(&mut self, a: ShapeHandle, b: ShapeHandle) -> KernelResult<ShapeHandle> {
        self.backend.common(a, b)
    }

    // ---- 带历史布尔（M5-B3 关联更新主路径）-----------------------------------

    /// **带历史的融合**：返回结果形状 + 面级演化（会话内）。
    ///
    /// `input_face_hashes` 必须取自 [`Kernel::face_hashes`]（**同域**）。
    /// 内部固定用 `FACE_HASH_UPPER_BOUND` 口径，**不暴露 bound 参数**。
    /// 返回 [`Evolution`] 为适配层自有类型（铁律①：不外泄 `occt_wasm::` 类型）。
    pub fn fuse_with_history(
        &mut self,
        a: ShapeHandle,
        b: ShapeHandle,
        input_face_hashes: &[u64],
    ) -> KernelResult<Evolution> {
        self.backend.fuse_with_history(a, b, input_face_hashes)
    }

    /// **带历史的切除**（语义同 [`Kernel::fuse_with_history`]）。
    pub fn cut_with_history(
        &mut self,
        a: ShapeHandle,
        b: ShapeHandle,
        input_face_hashes: &[u64],
    ) -> KernelResult<Evolution> {
        self.backend.cut_with_history(a, b, input_face_hashes)
    }

    // ---- 修复类算子（设计文档 3.3 / M4 heal ops）-----------------------------

    /// **缝合（成壳）**：把 `shape` 的面按 `tolerance`（mm）缝成壳。
    ///
    /// ⚠ 产出的是**壳 / 复合体**，不是实体（`sub_shape_count(_, Solid)` 为 0，
    /// 体积 = 各输入之和）。要单一实体请用 [`sew_and_solidify`](Kernel::sew_and_solidify)。
    pub fn sew(&mut self, shape: ShapeHandle, tolerance: f64) -> KernelResult<ShapeHandle> {
        self.backend.sew(shape, tolerance)
    }

    /// **缝合并实体化**：缝合后再合成一个实体（`sub_shape_count(_, Solid)` 为 1）。
    pub fn sew_and_solidify(
        &mut self,
        shape: ShapeHandle,
        tolerance: f64,
    ) -> KernelResult<ShapeHandle> {
        self.backend.sew_and_solidify(shape, tolerance)
    }

    /// **修复形状**：跑一遍内核形状修复器。对合法形状应为**保体积**的恒等（到容差内）。
    pub fn fix_shape(&mut self, shape: ShapeHandle) -> KernelResult<ShapeHandle> {
        self.backend.fix_shape(shape)
    }

    /// **同域合并**：合并同一几何域上的相邻面（去掉多余分界线），**体积守恒**。
    pub fn unify_same_domain(&mut self, shape: ShapeHandle) -> KernelResult<ShapeHandle> {
        self.backend.unify_same_domain(shape)
    }

    // ---- 八算子门面（M5-B6.2a）-----------------------------------------------
    //
    // 一行转发到后端（同 `fuse` / `sew`）。**返回新句柄，输入句柄仍有效**；
    // 8 算子不产出 `Evolution`。

    /// **圆角**：在 `solid` 的 `edges` 上按 `radius`（mm）倒圆。返回新句柄。
    pub fn fillet(
        &mut self,
        solid: ShapeHandle,
        edges: &[ShapeHandle],
        radius: f64,
    ) -> KernelResult<ShapeHandle> {
        self.backend.fillet(solid, edges, radius)
    }

    /// **倒角**：在 `solid` 的 `edges` 上按 `distance`（mm）切角。返回新句柄。
    pub fn chamfer(
        &mut self,
        solid: ShapeHandle,
        edges: &[ShapeHandle],
        distance: f64,
    ) -> KernelResult<ShapeHandle> {
        self.backend.chamfer(solid, edges, distance)
    }

    /// **抽壳**：把 `solid` 按 `thickness`（mm）挖空，在 `faces` 上开口。返回新句柄。
    pub fn shell(
        &mut self,
        solid: ShapeHandle,
        faces: &[ShapeHandle],
        thickness: f64,
        tolerance: f64,
    ) -> KernelResult<ShapeHandle> {
        self.backend.shell(solid, faces, thickness, tolerance)
    }

    /// **偏置**：把 `shape` 按 `distance`（mm）外扩 / 内缩。返回新句柄。
    pub fn offset(
        &mut self,
        shape: ShapeHandle,
        distance: f64,
        tolerance: f64,
    ) -> KernelResult<ShapeHandle> {
        self.backend.offset(shape, distance, tolerance)
    }

    /// **加厚**：把**面 / 壳**按 `thickness`（mm）加厚成实体。返回新句柄。
    pub fn thicken(
        &mut self,
        shape: ShapeHandle,
        thickness: f64,
        tolerance: f64,
    ) -> KernelResult<ShapeHandle> {
        self.backend.thicken(shape, thickness, tolerance)
    }

    /// **放样**：由 `wires`（**N ≥ 2**）放样成体 / 面。返回新句柄。
    pub fn loft(
        &mut self,
        wires: &[ShapeHandle],
        is_solid: bool,
        ruled: bool,
    ) -> KernelResult<ShapeHandle> {
        self.backend.loft(wires, is_solid, ruled)
    }

    /// **扫掠**：把 `profile` 沿 `spine` 扫掠（`transition_mode` 直通）。返回新句柄。
    pub fn sweep(
        &mut self,
        profile: ShapeHandle,
        spine: ShapeHandle,
        transition_mode: i32,
    ) -> KernelResult<ShapeHandle> {
        self.backend.sweep(profile, spine, transition_mode)
    }

    /// **管道**：把 `profile` 沿 `spine` 扫成管。返回新句柄。
    pub fn pipe(&mut self, profile: ShapeHandle, spine: ShapeHandle) -> KernelResult<ShapeHandle> {
        self.backend.pipe(profile, spine)
    }

    /// **直边**：由两端点定一条线段边。返回新句柄。
    pub fn make_line_edge(
        &mut self,
        x1: f64,
        y1: f64,
        z1: f64,
        x2: f64,
        y2: f64,
        z2: f64,
    ) -> KernelResult<ShapeHandle> {
        self.backend.make_line_edge(x1, y1, z1, x2, y2, z2)
    }

    /// **三点弧**：由起点 / 中间点 / 终点定一段圆弧边。返回新句柄。
    #[allow(clippy::too_many_arguments)]
    pub fn make_arc_edge(
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
        self.backend
            .make_arc_edge(x1, y1, z1, x2, y2, z2, x3, y3, z3)
    }

    /// **贝塞尔样条边**：由 N（≥ 2）个控制点定一条 Bézier 样条边。返回新句柄。
    pub fn make_bezier_edge(&mut self, points: &[[f64; 3]]) -> KernelResult<ShapeHandle> {
        self.backend.make_bezier_edge(points)
    }

    /// **整圆边**（G4-M15）：由圆心 / 法向 / 半径定一个整圆边。返回新句柄。
    #[allow(clippy::too_many_arguments)]
    pub fn make_circle_edge(
        &mut self,
        cx: f64,
        cy: f64,
        cz: f64,
        nx: f64,
        ny: f64,
        nz: f64,
        radius: f64,
    ) -> KernelResult<ShapeHandle> {
        self.backend
            .make_circle_edge(cx, cy, cz, nx, ny, nz, radius)
    }

    /// **圆弧边**（G4-M15）：由圆心 / 法向 / 半径 / 起终角定一段圆弧边。返回新句柄。
    #[allow(clippy::too_many_arguments)]
    pub fn make_circle_arc(
        &mut self,
        cx: f64,
        cy: f64,
        cz: f64,
        nx: f64,
        ny: f64,
        nz: f64,
        radius: f64,
        start_angle: f64,
        end_angle: f64,
    ) -> KernelResult<ShapeHandle> {
        self.backend
            .make_circle_arc(cx, cy, cz, nx, ny, nz, radius, start_angle, end_angle)
    }

    /// **整椭圆边**（G4-M15）：由中心 / 法向 / 长短半轴定一个整椭圆边。返回新句柄。
    #[allow(clippy::too_many_arguments)]
    pub fn make_ellipse_edge(
        &mut self,
        cx: f64,
        cy: f64,
        cz: f64,
        nx: f64,
        ny: f64,
        nz: f64,
        major_radius: f64,
        minor_radius: f64,
    ) -> KernelResult<ShapeHandle> {
        self.backend
            .make_ellipse_edge(cx, cy, cz, nx, ny, nz, major_radius, minor_radius)
    }

    /// **线框**：把一组边按序连成 wire（N ≥ 1，空表响亮拒绝）。返回新句柄。
    pub fn make_wire(&mut self, edges: &[ShapeHandle]) -> KernelResult<ShapeHandle> {
        self.backend.make_wire(edges)
    }

    /// **由 wire 造面**：把闭合 wire 生成一个面。返回新句柄。
    pub fn make_face(&mut self, wire: ShapeHandle) -> KernelResult<ShapeHandle> {
        self.backend.make_face(wire)
    }

    /// **提取外环**：提取 face 的外环 wire。返回新句柄。
    pub fn outer_wire(&mut self, face: ShapeHandle) -> KernelResult<ShapeHandle> {
        self.backend.outer_wire(face)
    }

    // ---- 变换 ----------------------------------------------------------------

    /// 平移：把 `shape` 沿 `(dx, dy, dz)`（mm）平移，返回新句柄。
    ///
    /// 平移是刚体运动，不改变体积；原句柄仍然有效。
    pub fn translate(
        &mut self,
        shape: ShapeHandle,
        dx: f64,
        dy: f64,
        dz: f64,
    ) -> KernelResult<ShapeHandle> {
        self.backend.translate(shape, dx, dy, dz)
    }

    /// 旋转（刚体）：把 `shape` 绕点 `(px, py, pz)`、轴 `(dx, dy, dz)`、
    /// 角度 `angle_rad`（rad）旋转，返回新句柄。原句柄仍有效。
    #[allow(clippy::too_many_arguments)]
    pub fn rotate(
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
        self.backend
            .rotate(shape, px, py, pz, dx, dy, dz, angle_rad)
    }

    // ---- 几何查询 ------------------------------------------------------------

    /// 轴对齐包围盒（mm）。
    pub fn bounding_box(&mut self, shape: ShapeHandle) -> KernelResult<BoundingBox> {
        self.backend.bounding_box(shape)
    }

    /// 表面积（mm²）。
    pub fn surface_area(&mut self, shape: ShapeHandle) -> KernelResult<f64> {
        self.backend.surface_area(shape)
    }

    /// **点到面的最近点投影**（mm）。⚠ 非"射线命中判定"（平面投影无界），见 trait 文档。
    pub fn project_point_on_face(
        &mut self,
        face: ShapeHandle,
        point: DVec3,
    ) -> KernelResult<PointOnFace> {
        self.backend.project_point_on_face(face, point)
    }

    /// **点到边的最近点投影**（mm）。返回最近点 + 单位切向 + 曲线参数（非归一化）。
    pub fn project_point_on_edge(
        &mut self,
        edge: ShapeHandle,
        point: DVec3,
    ) -> KernelResult<PointOnEdge> {
        self.backend.project_point_on_edge(edge, point)
    }

    /// **面在 `(u, v)` 处的单位法向**（mm，单位向量）；`(u,v)` 取自 `PointOnFace`。
    pub fn surface_normal(&mut self, face: ShapeHandle, u: f64, v: f64) -> KernelResult<DVec3> {
        self.backend.surface_normal(face, u, v)
    }

    /// **曲线的参数域 `[first, last]`**（非归一化），用于归一化 `PointOnEdge.param`。
    pub fn curve_parameters(&mut self, curve: ShapeHandle) -> KernelResult<CurveRange> {
        self.backend.curve_parameters(curve)
    }

    /// **曲线在参数 `param` 处的点**（mm）；`param` 取曲线的**原始**参数域
    /// （如参数域中点）。这是跨会话稳定的边代表点来源。
    pub fn curve_point_at_param(&mut self, edge: ShapeHandle, param: f64) -> KernelResult<DVec3> {
        self.backend.curve_point_at_param(edge, param)
    }

    /// **曲线的几何长度**（mm）；`TopoName::of_edge` 的 `length` 唯一来源
    /// （`curve_parameters` 的 `last − first` 对圆弧边**不是长度**）。
    pub fn curve_length(&mut self, edge: ShapeHandle) -> KernelResult<f64> {
        self.backend.curve_length(edge)
    }

    // ---- 子形状 / 复合体（设计文档 §5.3）-------------------------------------

    /// 枚举某形状的指定种类子形状（返回**新会话句柄**）。
    pub fn sub_shapes(
        &mut self,
        shape: ShapeHandle,
        kind: SubShapeKind,
    ) -> KernelResult<Vec<ShapeHandle>> {
        self.backend.sub_shapes(shape, kind)
    }

    /// 某形状某类子形状的数量。
    pub fn sub_shape_count(
        &mut self,
        shape: ShapeHandle,
        kind: SubShapeKind,
    ) -> KernelResult<usize> {
        self.backend.sub_shape_count(shape, kind)
    }

    /// **拆出全部实体**（设计文档 §5.3）——即 `sub_shapes(_, Solid)` 的一次调用化。
    ///
    /// 布尔结果恒为 compound：要把它当实体用，先在这里拆。返回的是**新会话句柄**
    /// （需调用方释放或由 [`Kernel::scope`] 圈住）。
    pub fn split_solids(&mut self, shape: ShapeHandle) -> KernelResult<Vec<ShapeHandle>> {
        self.backend.sub_shapes(shape, SubShapeKind::Solid)
    }

    /// 要求**恰好一个**实体：返回它；否则**响亮报错，绝不取 first**。
    ///
    /// - 0 个 → [`KernelError::EmptyResult`]；
    /// - \>1 个 → [`KernelError::NotSingleSolid { count }`]（`count` 为实际个数）。
    ///
    /// 这是设计文档 §5.3「任何布尔之后、任何"要求单实体"的算子之前都要显式处理
    /// 复合体」的**一次调用化**——将来 `fillet` 接线（M5）会先 `single_solid`
    /// 再 `fillet`，多体时响亮失败而非静默改坏模型。
    ///
    /// 注意：本方法**会**分配句柄来枚举子形状，但 `Kernel::scope` 可把它们圈掉；
    /// 返回的那个实体句柄需调用方负责释放。
    pub fn single_solid(&mut self, shape: ShapeHandle) -> KernelResult<ShapeHandle> {
        let mut solids = self.backend.sub_shapes(shape, SubShapeKind::Solid)?;
        match solids.len() {
            0 => Err(KernelError::EmptyResult(format!(
                "要求恰好一个实体，但该形状不含任何实体（shape={shape:?}）"
            ))),
            1 => Ok(solids.remove(0)),
            count => {
                // 顺手回收刚枚举出来的句柄，避免调用方拿到错误后还要自己清理。
                self.release_many(&solids);
                Err(KernelError::NotSingleSolid { count })
            }
        }
    }

    // ---- 网格化 / HLR --------------------------------------------------------

    /// 三角化（`linear` mm，`angular` rad）。
    pub fn tessellate(
        &mut self,
        shape: ShapeHandle,
        linear: f64,
        angular: f64,
    ) -> KernelResult<MeshData> {
        self.backend.tessellate(shape, linear, angular)
    }

    /// 该形状**各面的会话哈希**（顺序同内核面序）。
    ///
    /// 与 [`Kernel::tessellate`] 产出的 [`MeshData::face_groups`] 里的
    /// `face_hash` **同域**，可直接用于三维拾取的粗筛（设计文档 6.6）。
    /// ⚠ 会话哈希不可持久化。
    pub fn face_hashes(&mut self, shape: ShapeHandle) -> KernelResult<Vec<u64>> {
        self.backend.face_hashes(shape)
    }

    /// 工程图级消隐投影（设计文档 3.4）。
    pub fn project_edges(&mut self, shape: ShapeHandle, view: ViewDef) -> KernelResult<Projection> {
        self.backend.project_edges(shape, view)
    }

    // ---- 持久化互操作（设计文档 3.3 / 4.2）------------------------------------

    /// 把 BREP 文本装回成一个会话句柄（**惰性重建**的一半）。
    ///
    /// 与 [`Kernel::release`] 配对，构成"句柄可丢"的完整闭环：
    /// 文档里存 BREP 文本 → 用时 [`import_brep`](Kernel::import_brep) 重建 →
    /// 不用了 [`release`](Kernel::release) 丢掉。丢了不影响文档。
    pub fn import_brep(&mut self, text: &str) -> KernelResult<ShapeHandle> {
        self.backend.import_brep(text)
    }

    /// 把会话句柄导出成 BREP 文本（**存盘**的一半）。
    pub fn export_brep(&mut self, shape: ShapeHandle) -> KernelResult<String> {
        self.backend.export_brep(shape)
    }

    /// 实体体积（mm³）。非实体返回 `0.0`。
    pub fn volume(&mut self, shape: ShapeHandle) -> KernelResult<f64> {
        self.backend.volume(shape)
    }

    // ---- 生命周期 ------------------------------------------------------------

    /// 句柄是否有效。
    #[must_use]
    pub fn is_alive(&self, shape: ShapeHandle) -> bool {
        self.backend.is_alive(shape)
    }

    /// 释放句柄。
    pub fn release(&mut self, shape: ShapeHandle) -> KernelResult<()> {
        self.backend.release(shape)
    }

    /// 当前存活形状数（泄漏自查用）。
    #[must_use]
    pub fn live_count(&self) -> usize {
        self.backend.live_count()
    }

    /// 当前所有存活句柄（升序）。
    #[must_use]
    pub fn live_handles(&self) -> Vec<ShapeHandle> {
        self.backend.live_handles()
    }

    /// 批量释放；**已经释放过的会被跳过**（不报错）。
    ///
    /// 返回实际释放成功的数量。
    ///
    /// 为什么需要"跳过已释放"：批量清场时，调用方往往无法（也不该）逐个记住
    /// 哪个句柄已经在中间步骤里被消费掉了。逐个 `release` 会让第一个陈旧句柄
    /// 中断整轮清理——那才是真正的泄漏来源。
    pub fn release_many(&mut self, handles: &[ShapeHandle]) -> usize {
        let mut released = 0;
        for h in handles {
            if self.release(*h).is_ok() {
                released += 1;
            }
        }
        released
    }

    /// **以"临时物自动回收"的方式执行 `f`**（设计文档 3.7 的"统一清理"）。
    ///
    /// 进入时记下存活句柄，执行 `f`，离开时释放**期间新产生**的句柄——
    /// 但 `keep` 里列出的句柄会被保留（用来把结果带出去）。
    ///
    /// 返回 `(f 的返回值, 实际回收的临时句柄数)`。
    ///
    /// # 为什么这是一个门面级能力
    ///
    /// 一次 HLR 或布尔运算会造出多个中间形状（投影的 6 个 compound、布尔的
    /// 临时实体）。内核 arena **不自动回收**，漏一个就少一点内存，跑一个大装配
    /// 就爆。把"圈临时物"做成一次性 API，比在每个算子旁边手写 release 靠谱得多：
    ///
    /// ```no_run
    /// # use core_kernel::{Kernel, KernelResult, ShapeHandle, ViewDef};
    /// # fn demo(k: &mut Kernel, solid: ShapeHandle, view: ViewDef) -> KernelResult<()> {
    /// // 投影：只带走最终要用的结果，中间 6 个 compound 自动回收
    /// let (projection, _recycled) = k.scope(&[], |k| k.project_edges(solid, view))?;
    /// # let _ = projection;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # 注意
    ///
    /// - `keep` 里的句柄即使不存在也不会报错（与 [`Kernel::release_many`] 同规）；
    /// - 若 `f` 已经把某个临时句柄释放了，这里会**跳过**它；
    /// - **不做嵌套特殊处理**：内层 `scope` 回收过的句柄，外层再看到时已经不存在了。
    pub fn scope<R>(
        &mut self,
        keep: &[ShapeHandle],
        f: impl FnOnce(&mut Self) -> R,
    ) -> KernelResult<(R, usize)> {
        let before: BTreeSet<u32> = self.live_handles().into_iter().map(|h| h.raw()).collect();
        let kept: BTreeSet<u32> = keep.iter().map(|h| h.raw()).collect();

        let out = f(self);

        let temps: Vec<ShapeHandle> = self
            .live_handles()
            .into_iter()
            .filter(|h| !before.contains(&h.raw()) && !kept.contains(&h.raw()))
            .collect();
        let recycled = self.release_many(&temps);
        Ok((out, recycled))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(feature = "occt"))]
    fn new_backend_is_not_wired_without_feature() {
        // 未开 feature 时应明确报错，而不是偷偷退回 mock（避免"以为在用真内核"）
        assert!(matches!(
            Kernel::new(),
            Err(crate::error::KernelError::Unsupported(_))
        ));
    }

    #[test]
    fn mock_kernel_forwards_operations() {
        let mut k = Kernel::mock();
        assert_eq!(k.backend_name(), "mock");
        let h = k.make_box(10.0, 20.0, 30.0).unwrap();
        assert!(k.is_alive(h));
        assert_eq!(k.live_count(), 1);
        let m = k.tessellate(h, 0.1, 0.5).unwrap();
        assert_eq!(m.triangle_count(), 12);
        k.release(h).unwrap();
        assert_eq!(k.live_count(), 0);
    }

    #[test]
    fn mock_kernel_forwards_translate() {
        let mut k = Kernel::mock();
        let h = k.make_box(10.0, 20.0, 30.0).unwrap();
        let moved = k.translate(h, 1000.0, 0.0, 3000.0).unwrap();
        assert!(k.is_alive(h), "平移产生新形状，原句柄仍有效");
        assert_ne!(moved, h);
        assert!((k.volume(moved).unwrap() - 6000.0).abs() < 1e-9);
        assert_eq!(k.live_count(), 2);
    }

    #[test]
    fn debug_format_includes_backend() {
        let k = Kernel::mock();
        let s = format!("{k:?}");
        assert!(s.contains("mock"));
    }

    #[test]
    fn kind_distinguishes_mock_from_real() {
        assert_eq!(Kernel::mock().kind(), BackendKind::Mock);
        assert!(!Kernel::mock().is_real());
    }

    #[test]
    fn brep_export_import_roundtrips_through_the_facade() {
        let mut k = Kernel::mock();
        let h = k.make_box(2.0, 3.0, 4.0).unwrap();
        let text = k.export_brep(h).unwrap();
        let back = k.import_brep(&text).unwrap();
        assert_eq!(k.live_count(), 2);
        assert!((k.volume(h).unwrap() - k.volume(back).unwrap()).abs() < 1e-9);
    }

    #[test]
    fn release_many_skips_handles_that_are_already_gone() {
        let mut k = Kernel::mock();
        let a = k.make_box(1.0, 1.0, 1.0).unwrap();
        let b = k.make_box(1.0, 1.0, 1.0).unwrap();
        k.release(a).unwrap();
        // a 已经没了，b 还在；批量清理不应被 a 打断
        assert_eq!(k.release_many(&[a, b]), 1);
        assert_eq!(k.live_count(), 0);
    }

    /// `scope` 的核心语义：**期间新造的句柄会被回收，`keep` 里的不会被回收**。
    #[test]
    fn scope_recycles_temporaries_and_keeps_the_named_result() {
        let mut k = Kernel::mock();
        let seed = k.make_box(1.0, 1.0, 1.0).unwrap();

        let (result, recycled) = k
            .scope(&[], |k| {
                let tmp = k.make_sphere(5.0).unwrap();
                k.fuse(seed, tmp).unwrap()
            })
            .unwrap();

        // 造了 tmp 与 fuse 结果两个；都被回收
        assert_eq!(recycled, 2);
        assert_eq!(k.live_count(), 1, "只剩进入前的 seed");
        assert!(k.is_alive(seed));
        assert!(
            !k.is_alive(result),
            "未列入 keep 的结果会被回收（符合'临时物'语义）"
        );
    }

    #[test]
    fn scope_honours_the_keep_list() {
        let mut k = Kernel::mock();
        // 先造一个句柄，再把它列进 keep——它必须活下来
        let keeper = k.make_box(1.0, 1.0, 1.0).unwrap();

        let (temp, recycled) = k.scope(&[keeper], |k| k.make_sphere(1.0).unwrap()).unwrap();

        assert_eq!(recycled, 1, "只有临时造出来的那个球被回收");
        assert!(k.is_alive(keeper), "keep 列表里的句柄必须留下");
        assert!(!k.is_alive(temp), "未列入 keep 的新句柄应被回收");
        assert_eq!(k.live_count(), 1);
    }

    #[test]
    fn scope_is_a_noop_when_nothing_is_allocated() {
        let mut k = Kernel::mock();
        let seed = k.make_box(1.0, 1.0, 1.0).unwrap();
        let (value, recycled) = k.scope(&[], |_| 42).unwrap();
        assert_eq!((value, recycled), (42, 0));
        assert_eq!(k.live_count(), 1);
        assert!(k.is_alive(seed));
    }

    #[test]
    fn mock_kernel_forwards_m3_construction_ops() {
        let mut k = Kernel::mock();
        let cone = k.make_cone(3.0, 0.0, 10.0).unwrap();
        let torus = k.make_torus(10.0, 2.0).unwrap();
        assert!(k.volume(cone).unwrap() > 0.0);
        assert!(k.volume(torus).unwrap() > 0.0);

        let base = k.make_box(10.0, 10.0, 10.0).unwrap();
        let ext = k.extrude(base, 5.0, 0.0, 0.0).unwrap();
        assert_eq!(k.bounding_box(ext).unwrap().size(), [15.0, 10.0, 10.0]);
        let rev = k
            .revolve(base, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, std::f64::consts::TAU)
            .unwrap();
        assert!(k.volume(rev).unwrap() > 0.0);
    }

    #[test]
    fn mock_kernel_rotate_and_queries_are_forwarded() {
        let mut k = Kernel::mock();
        let h = k.make_box(10.0, 20.0, 30.0).unwrap();
        let rot = k
            .rotate(h, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, std::f64::consts::FRAC_PI_2)
            .unwrap();
        assert_ne!(k.export_brep(rot).unwrap(), k.export_brep(h).unwrap());
        assert!((k.volume(rot).unwrap() - 6000.0).abs() < 1e-9);
        assert!((k.surface_area(h).unwrap() - 2.0 * (200.0 + 600.0 + 300.0)).abs() < 1e-9);
        assert_eq!(k.bounding_box(h).unwrap().max, [10.0, 20.0, 30.0]);
    }

    /// `split_solids` = `sub_shapes(Solid)`；`single_solid` 多体必须响亮拒绝。
    #[test]
    fn split_solids_and_single_solid_enforce_the_compound_ritual() {
        let mut k = Kernel::mock();
        let a = k.make_box(10.0, 10.0, 10.0).unwrap();
        let far_box = k.make_box(10.0, 10.0, 10.0).unwrap();
        let far = k.translate(far_box, 100.0, 0.0, 0.0).unwrap();
        let fused = k.fuse(a, far).unwrap();

        let solids = k.split_solids(fused).unwrap();
        assert_eq!(solids.len(), 2, "远离两盒 fuse 应能拆出 2 个实体");

        match k.single_solid(fused) {
            Err(KernelError::NotSingleSolid { count }) => assert_eq!(count, 2),
            other => panic!("多体必须响亮拒绝，实得 {other:?}"),
        }
    }

    /// 单实体走 `single_solid` 应正常返回；`split_solids` 单盒应得 1 个。
    #[test]
    fn single_solid_returns_the_only_solid() {
        let mut k = Kernel::mock();
        let h = k.make_box(1.0, 1.0, 1.0).unwrap();
        assert_eq!(k.split_solids(h).unwrap().len(), 1);
        let one = k.single_solid(h).unwrap();
        assert!(k.is_alive(one));
    }
}

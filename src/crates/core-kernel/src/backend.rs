//! 内核后端抽象。
//!
//! [`Kernel`](crate::Kernel) 门面只依赖本 trait，不依赖任何具体后端。
//! 这样"换内核"= 换一个 [`KernelBackend`] 实现（设计文档目标 G3）。
//!
//! **实现者须知**：所有以长度为参数的算子，单位一律 **毫米（mm）**；
//! 角度一律 **弧度（rad）**；坐标系 **Z 轴朝上、右手系**。

use crate::error::KernelResult;
use crate::handle::ShapeHandle;
use crate::types::{
    BoundingBox, CurveRange, DVec3, Evolution, MeshData, PointOnEdge, PointOnFace, Projection,
    SubShapeKind, ViewDef,
};

/// 后端种类 —— 决定"这个内核的话能不能当真"。
///
/// 存在的理由很具体：占位后端与真实后端**接口一样但可信度完全不同**。
/// 内核池（[`crate::pool`]）必须能分辨两者，否则会出现
/// "以为在用真几何内核、其实拿到的是占位后端"的静默降级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackendKind {
    /// 真实几何内核（`occt-wasm`）：结果可用于出图、存盘与交付。
    Real,
    /// 纯 Rust 占位后端：几何是**简化的**，只能用于跑通链路与单测。
    Mock,
}

impl BackendKind {
    /// 是否真实几何内核。
    #[must_use]
    pub const fn is_real(self) -> bool {
        matches!(self, Self::Real)
    }

    /// 中文短名（日志 / 错误信息用）。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Real => "真实内核",
            Self::Mock => "占位后端",
        }
    }
}

/// 几何内核后端。
///
/// 一阶段有两个实现：
/// - [`MockBackend`](crate::MockBackend)：纯 Rust 占位（[`BackendKind::Mock`]），
///   供无 WASM 环境的开发与测试；
/// - [`OcctBackend`](crate::OcctBackend)：真实 `occt-wasm` v4（[`BackendKind::Real`]，
///   M0 接入、M1 补齐持久化互操作）。
///
/// **Send supertrait（G6-M7）**：`Kernel`（`Box<dyn KernelBackend>`）随
/// `CoreSession` 进 HTTP 桥的共享会话表（`Mutex<HashMap<…>>`）——axum 的
/// `State` 要求 `Send + Sync`，`Mutex<T>: Sync` 要求 `T: Send`，Send 断在此。
/// 两个实现的 Send 已被现有代码实证：MockBackend 纯数据结构；OcctBackend
/// 在 occt 腿的静态共享 `Mutex<OcctBackend>`（`occt.rs` SHARED_BACKEND）编译通过。
pub trait KernelBackend: Send {
    /// 后端名称（用于日志与错误信息）。
    fn name(&self) -> &'static str;

    /// 后端的**可信度**。默认 [`BackendKind::Real`]：新增真实后端时不必显式声明，
    /// 但**占位后端必须覆盖**为 [`BackendKind::Mock`]。
    fn kind(&self) -> BackendKind {
        BackendKind::Real
    }

    // ---- 基元 ----------------------------------------------------------------

    /// 长方体，尺寸 `dx × dy × dz`（mm），一角在原点。
    fn make_box(&mut self, dx: f64, dy: f64, dz: f64) -> KernelResult<ShapeHandle>;

    /// 球，半径 `r`（mm），球心在原点。
    fn make_sphere(&mut self, r: f64) -> KernelResult<ShapeHandle>;

    /// 圆柱，半径 `r`、高 `h`（mm），轴线沿 +Z。
    fn make_cylinder(&mut self, r: f64, h: f64) -> KernelResult<ShapeHandle>;

    /// 圆锥 / 圆台，下底半径 `r1`、上底半径 `r2`、高 `h`（mm），轴线沿 +Z。
    ///
    /// `r2 == 0` 即普通圆锥（尖顶）；`r1 == r2` 退化为圆柱。
    fn make_cone(&mut self, r1: f64, r2: f64, h: f64) -> KernelResult<ShapeHandle>;

    /// 圆环（环面），主半径 `major`（环心到管心）、次半径 `minor`（管子半径），单位 mm。
    fn make_torus(&mut self, major: f64, minor: f64) -> KernelResult<ShapeHandle>;

    // ---- 轮廓 → 体 -----------------------------------------------------------

    /// 拉伸：把 `shape`（轮廓）沿 `(dx, dy, dz)`（mm）拉伸成一个体。
    fn extrude(
        &mut self,
        shape: ShapeHandle,
        dx: f64,
        dy: f64,
        dz: f64,
    ) -> KernelResult<ShapeHandle>;

    /// 旋转（轮廓 → 体）：绕点 `(px, py, pz)`、轴 `(dx, dy, dz)`、角度 `angle_rad`（rad）旋转扫掠。
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
    ) -> KernelResult<ShapeHandle>;

    // ---- 布尔 ----------------------------------------------------------------

    /// 融合（并集）。
    fn fuse(&mut self, a: ShapeHandle, b: ShapeHandle) -> KernelResult<ShapeHandle>;

    /// 切除（差集）。
    fn cut(&mut self, a: ShapeHandle, b: ShapeHandle) -> KernelResult<ShapeHandle>;

    /// 求交。
    fn common(&mut self, a: ShapeHandle, b: ShapeHandle) -> KernelResult<ShapeHandle>;

    // ---- 带历史布尔（M5-B3 关联更新主路径）-----------------------------------

    /// **带历史的融合**：返回结果形状 + 面级演化（会话内）。
    ///
    /// `input_face_hashes` 必须取自 [`face_hashes`](KernelBackend::face_hashes)
    /// （**同域**；禁止自取 bound）。实现内部固定用 `FACE_HASH_UPPER_BOUND` 口径，
    /// 对外**不暴露 bound 参数**。返回的 [`Evolution`] 是**适配层自有类型**，
    /// 绝不外泄 `occt_wasm::` 类型（铁律①）。
    fn fuse_with_history(
        &mut self,
        a: ShapeHandle,
        b: ShapeHandle,
        input_face_hashes: &[u64],
    ) -> KernelResult<Evolution>;

    /// **带历史的切除**（语义同 [`fuse_with_history`](KernelBackend::fuse_with_history)）。
    fn cut_with_history(
        &mut self,
        a: ShapeHandle,
        b: ShapeHandle,
        input_face_hashes: &[u64],
    ) -> KernelResult<Evolution>;

    // ---- 修复类算子（设计文档 3.3 / M4 heal ops）-----------------------------
    //
    // 这四个算子是"几何体检 / 修复"的最小可用集：缝合（把一组面缝成壳）、
    // 缝合并实体化（缝完再合成一个实体）、修复形状（跑一遍内核的 shape fixer）、
    // 同域合并（把落在同一几何域上的相邻面合并，去掉多余的分界线）。
    //
    // ⚠ 它们与布尔类算子的**返回语义不同**：布尔返回 compound（可能含多个实体），
    // 而 [`sew`](KernelBackend::sew) 返回的是**壳 / 复合体**（`solid_count == 0`）。

    /// **缝合（成壳）**：把 `shape` 里的面按 `tolerance`（mm）缝合成一个壳。
    ///
    /// # ⚠ 实测事实（2026-09，必须牢记）
    ///
    /// `sew` 产出的是**壳 / 复合体**，**不是**实体：缝好之后
    /// [`sub_shape_count`](KernelBackend::sub_shape_count)`(_, Solid)` 恒为 **0**，
    /// 而 [`volume`](KernelBackend::volume) 等于**各输入体积之和**（缝合不改变体积）。
    ///
    /// > 要拿到**单一实体**，必须用
    /// > [`sew_and_solidify`](KernelBackend::sew_and_solidify)，**不要**指望
    /// > `sew` 之后再 `single_solid` —— 那会得到
    /// > [`KernelError::EmptyResult`](crate::KernelError::EmptyResult)。
    ///
    /// `tolerance` 是缝合容差（mm），需为有限非负数。
    fn sew(&mut self, shape: ShapeHandle, tolerance: f64) -> KernelResult<ShapeHandle>;

    /// **缝合并实体化**：把 `shape` 里的面按 `tolerance`（mm）缝合，**再合成一个实体**。
    ///
    /// 与 [`sew`](KernelBackend::sew) 的唯一区别就在最后一步：本方法产出一个
    /// **实心体**，因此 [`sub_shape_count`](KernelBackend::sub_shape_count)`(_, Solid)`
    /// 为 **1**（前提是输入面组确实围成了封闭壳）。
    ///
    /// 什么时候用哪个：
    /// - 只想把散面拼起来看看拓扑是否闭合 → [`sew`](KernelBackend::sew)；
    /// - 要拿这个结果继续做布尔 / 出图 / 量体积（即需要一个实体）→ 本方法。
    ///
    /// `tolerance` 是缝合容差（mm），需为有限非负数。
    fn sew_and_solidify(&mut self, shape: ShapeHandle, tolerance: f64)
    -> KernelResult<ShapeHandle>;

    /// **修复形状**：对 `shape` 跑一遍内核的形状修复器（`ShapeFix_Shape`），
    /// 修补小的拓扑 / 几何缺陷（缺失的面朝向、微小缝隙、退化边等）。
    ///
    /// 对**本已合法**的形状，修复应是**保体积的恒等（到容差内）**——
    /// 因此正确性校验的写法是"修复前后体积相等"，而不是"形状变成了别的样子"。
    /// 返回一个新句柄；原句柄仍有效。
    fn fix_shape(&mut self, shape: ShapeHandle) -> KernelResult<ShapeHandle>;

    /// **同域合并**：把 `shape` 中落在**同一几何域**上的相邻面（共面 / 同柱面等）
    /// 合并成一个面，去掉它们之间那条"多余"的分界线（`ShapeUpgrade_UnifySameDomain`）。
    ///
    /// 这是清理布尔 / 缝合残留切痕的标准手段，**不改变几何本身**——
    /// 体积守恒，返回一个新句柄，原句柄仍有效。
    fn unify_same_domain(&mut self, shape: ShapeHandle) -> KernelResult<ShapeHandle>;

    // ---- 八算子门面（M5-B6.2a）-----------------------------------------------
    //
    // fillet / chamfer / shell / offset / thicken / loft / sweep / pipe 的**门面层**。
    // 逐个照 `fuse` / `sew` 范本：**签名只用适配层 `ShapeHandle`**（铁律①：绝不
    // 外泄 `occt_wasm::` 类型）。本批只补门面与 mock 桩，派发接线属 B6.2b。
    //
    // ⚠ **返回语义**：与 `translate` / `sew` 一致——返回**新句柄**，输入的
    // `solid` / `edges` / `faces` / `wires` 等**原句柄仍有效**（不消费输入）。
    //
    // ⚠ 8 算子**不产出 `Evolution`**（vendor `*_with_history` 五条不接线）。

    /// **圆角**：在 `solid` 的 `edges` 上按 `radius`（mm）倒圆。
    ///
    /// `edges` 由上层在该实体上解析选中边得到（**空边集**对多数内核是无声无效 →
    /// 上层须响亮拒绝空表）。`radius` 须为有限非负数。
    fn fillet(
        &mut self,
        solid: ShapeHandle,
        edges: &[ShapeHandle],
        radius: f64,
    ) -> KernelResult<ShapeHandle>;

    /// **倒角**：在 `solid` 的 `edges` 上按 `distance`（mm）切角。
    ///
    /// `distance` 须为有限非负数。
    fn chamfer(
        &mut self,
        solid: ShapeHandle,
        edges: &[ShapeHandle],
        distance: f64,
    ) -> KernelResult<ShapeHandle>;

    /// **抽壳**：把 `solid` 按 `thickness`（mm）挖空，在 `faces` 上开口。
    ///
    /// `thickness` / `tolerance` 须为有限非负数。
    fn shell(
        &mut self,
        solid: ShapeHandle,
        faces: &[ShapeHandle],
        thickness: f64,
        tolerance: f64,
    ) -> KernelResult<ShapeHandle>;

    /// **偏置**：把 `shape` 按 `distance`（mm）外扩 / 内缩。
    ///
    /// `distance` / `tolerance` 须为有限非负数。
    fn offset(
        &mut self,
        shape: ShapeHandle,
        distance: f64,
        tolerance: f64,
    ) -> KernelResult<ShapeHandle>;

    /// **加厚**：把**面 / 壳** `shape` 按 `thickness`（mm）加厚成实体。
    ///
    /// ⚠ 输入是**面 / 壳**（不是实体）；`thickness` / `tolerance` 须为有限非负数。
    fn thicken(
        &mut self,
        shape: ShapeHandle,
        thickness: f64,
        tolerance: f64,
    ) -> KernelResult<ShapeHandle>;

    /// **放样**：由 `wires`（**N ≥ 2**，顺序即放样顺序）放样；`is_solid` 决定产实体 /
    /// 面，`ruled` 决定是否直纹。
    ///
    /// `wires.len() < 2` 时返回
    /// [`KernelError::InvalidArgument`](crate::KernelError::InvalidArgument)。
    fn loft(
        &mut self,
        wires: &[ShapeHandle],
        is_solid: bool,
        ruled: bool,
    ) -> KernelResult<ShapeHandle>;

    /// **扫掠**：把 `profile` 沿 `spine` 扫掠，`transition_mode` 为拐角过渡方式
    /// （`i32` 直通 vendor；各值语义未辨，见「不宣称」）。
    fn sweep(
        &mut self,
        profile: ShapeHandle,
        spine: ShapeHandle,
        transition_mode: i32,
    ) -> KernelResult<ShapeHandle>;

    /// **管道**：把 `profile` 沿 `spine` 扫成管。
    fn pipe(&mut self, profile: ShapeHandle, spine: ShapeHandle) -> KernelResult<ShapeHandle>;

    // ---- 线框生产（M7a）------------------------------------------------------
    //
    // 配方世界「从零造线框/面」的最小集。签名照 loft 范本：只用适配层
    // `ShapeHandle`（铁律①：绝不外泄 `occt_wasm::` 类型）。返回语义与 `translate`
    // 一致——返回**新句柄**，输入原句柄仍有效（不消费输入）。
    //
    // ⚠ wire/face 是**线/面**，`volume()` 返 0.0（见 `volume` 文档）——数值校验用
    //   `surface_area` / `sub_shape_count`，不得用体积。

    /// **直边**：由两端点 `(x1, y1, z1)` → `(x2, y2, z2)`（mm）定一条线段边。
    ///
    /// 六个坐标须为有限数，否则返回
    /// [`KernelError::InvalidArgument`](crate::KernelError::InvalidArgument)；
    /// 两端点重合时内核报错（原样透传）。
    fn make_line_edge(
        &mut self,
        x1: f64,
        y1: f64,
        z1: f64,
        x2: f64,
        y2: f64,
        z2: f64,
    ) -> KernelResult<ShapeHandle>;

    /// **三点弧**：由起点 / 中间点 / 终点三个坐标（mm）定一段圆弧边
    /// （vendor `make_arc_edge` 为三点式，**无角度参数**）。
    ///
    /// 九个坐标须为有限数；三点共线时内核报错（原样透传，不做预判）。
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
    ) -> KernelResult<ShapeHandle>;

    /// **贝塞尔样条边**（G4 前置）：由 N（≥ 2）个**控制点**（mm）定一条
    /// Bézier 样条边（vendor `make_bezier_edge` 扁平点直通；N−1 阶）。
    ///
    /// `points` 为空表或单点时返回
    /// [`KernelError::InvalidArgument`](crate::KernelError::InvalidArgument)；
    /// 任一坐标非有限数同样拒绝（`finite` 逐点校验，照 `make_line_edge` 口径）。
    fn make_bezier_edge(&mut self, points: &[[f64; 3]]) -> KernelResult<ShapeHandle>;

    /// **整圆边**（G4-M15）：由圆心 `(cx, cy, cz)`（mm）、所在平面法向
    /// `(nx, ny, nz)` 与半径（mm）定一个整圆边。
    ///
    /// 七个标量须为有限数、法向须为**非零向量**、半径须为**有限正数**，否则返回
    /// [`KernelError::InvalidArgument`](crate::KernelError::InvalidArgument)。
    #[allow(clippy::too_many_arguments)]
    fn make_circle_edge(
        &mut self,
        cx: f64,
        cy: f64,
        cz: f64,
        nx: f64,
        ny: f64,
        nz: f64,
        radius: f64,
    ) -> KernelResult<ShapeHandle>;

    /// **圆弧边**（G4-M15）：圆心 / 法向 / 半径同 [`Self::make_circle_edge`]，
    /// 另有起终角（rad，数学坐标 ccw 为正）。
    ///
    /// 角度语义以内核为准（occt-wasm 内部按 ccw 扫掠），适配层不做预判；
    /// 「终角 < 起角」的归一由**上层**（core-app `sketch` 模块）在造边前完成。
    #[allow(clippy::too_many_arguments)]
    fn make_circle_arc(
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
    ) -> KernelResult<ShapeHandle>;

    /// **整椭圆边**（G4-M15）：由中心 / 法向与长、短半轴（mm）定一个整椭圆边。
    ///
    /// 校验同 [`Self::make_circle_edge`]（长短半轴均为有限正数）。
    /// ⚠ vendor `make_ellipse_edge` **无主轴方向参数**——主轴沿法向局部 +x，
    /// 任意朝向由上层「造边 + [`Self::rotate`]」组合实现（见 core-app
    /// `sketch` 模块）。
    #[allow(clippy::too_many_arguments)]
    fn make_ellipse_edge(
        &mut self,
        cx: f64,
        cy: f64,
        cz: f64,
        nx: f64,
        ny: f64,
        nz: f64,
        major_radius: f64,
        minor_radius: f64,
    ) -> KernelResult<ShapeHandle>;

    /// **线框**：把一组边按序连成 wire（**变长**，N ≥ 1，边顺序即连接序）。
    ///
    /// `edges` 为**空表**时返回
    /// [`KernelError::InvalidArgument`](crate::KernelError::InvalidArgument)
    /// （与 `loft` 空截面同款响亮拒绝）。
    fn make_wire(&mut self, edges: &[ShapeHandle]) -> KernelResult<ShapeHandle>;

    /// **由 wire 造面**：把闭合 `wire` 生成一个面。
    fn make_face(&mut self, wire: ShapeHandle) -> KernelResult<ShapeHandle>;

    /// **提取外环**：提取 `face` 的外环 wire。
    fn outer_wire(&mut self, face: ShapeHandle) -> KernelResult<ShapeHandle>;

    // ---- 变换 ----------------------------------------------------------------

    /// 平移：把 `shape` 沿 `(dx, dy, dz)`（mm）平移，返回一个**新的**句柄。
    ///
    /// 平移是刚体运动：**不改变拓扑与体积**，原句柄仍然有效（不消费输入）。
    /// 三个分量必须为有限数（可正可负、可为 0），否则返回
    /// [`KernelError::InvalidArgument`](crate::KernelError::InvalidArgument)。
    fn translate(
        &mut self,
        shape: ShapeHandle,
        dx: f64,
        dy: f64,
        dz: f64,
    ) -> KernelResult<ShapeHandle>;

    /// 旋转（刚体）：把 `shape` 绕**点 `(px, py, pz)`**、**轴 `(dx, dy, dz)`**、
    /// 角度 `angle_rad`（rad）旋转，返回一个新句柄。
    ///
    /// 与 [`translate`](KernelBackend::translate) 同为刚体运动：**不改变体积与拓扑**，
    /// 原句柄仍有效。轴向量需为非零有限数，角度需为有限数。
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
    ) -> KernelResult<ShapeHandle>;

    // ---- 几何查询 ------------------------------------------------------------

    /// 轴对齐包围盒（mm）。返回**适配层**的 [`BoundingBox`]，不是内核类型。
    fn bounding_box(&mut self, shape: ShapeHandle) -> KernelResult<BoundingBox>;

    /// 表面积（mm²）。非实体（面 / 壳 / 线）按其自身面积返回。
    fn surface_area(&mut self, shape: ShapeHandle) -> KernelResult<f64>;

    // ---- 子形状（设计文档 §5.3 复合体处理）----------------------------------

    /// 枚举某形状的**子形状句柄**（按 `kind`）。
    ///
    /// **这是 §5.3 复合体拆分的直接前置**：布尔结果恒为 compound，要先
    /// `sub_shapes(_, Solid)` 拆出实体，才能喂给"要求单实体"的算子。
    ///
    /// ⚠ 返回的是**新分配的会话句柄**（需调用方释放或由 [`crate::Kernel::scope`]
    /// 圈住）；子形状的**序号 / 会话哈希跨会话不稳定，不可持久化**。
    fn sub_shapes(
        &mut self,
        shape: ShapeHandle,
        kind: SubShapeKind,
    ) -> KernelResult<Vec<ShapeHandle>>;

    /// 某形状某类子形状的**数量**（不分配句柄，便宜）。
    fn sub_shape_count(&mut self, shape: ShapeHandle, kind: SubShapeKind) -> KernelResult<usize>;

    // ---- 网格化 --------------------------------------------------------------

    /// 三角化。
    ///
    /// `linear` 为线性挠度（mm），`angular` 为角度挠度（rad）。
    fn tessellate(
        &mut self,
        shape: ShapeHandle,
        linear: f64,
        angular: f64,
    ) -> KernelResult<MeshData>;

    /// 该形状**各面的会话哈希**，顺序与内核的面序一致。
    ///
    /// **为什么必须有这个方法**：三维拾取走"网格粗筛 → 内核精算"两步
    /// （设计文档 6.6）。粗筛要能用 [`MeshData::face_groups`] 里的
    /// [`FaceGroup::face_hash`](crate::FaceGroup::face_hash) 命中内核的某个面，
    /// 两边就必须**同属一个哈希域**；而"域"由内核的 `hash_upper_bound` 决定——
    /// 同一个面在不同 bound 下会得到**完全不相交**的值集（实测：`1_000_000` 下是
    /// `326887`，`i32::MAX` 下是 `1652326887`）。把这个 bound 泄漏给上层，
    /// 等于埋一个"怎么看都对、一比就不中"的坑；所以约定由**后端自己**保证
    /// 本方法的返回值与它 `tessellate` 产出的 `face_hash` **同域**。
    ///
    /// 与 [`live_handles`](KernelBackend::live_handles) 同理：**不提供默认实现**。
    /// 静默返回空集会让人以为"这个形状没有面"，而不是"这个后端没实现"。
    ///
    /// ⚠ 返回的是**会话哈希**，跨会话不稳定，**不可持久化**；持久绑定必须落到
    /// `core-model` 的 `ShapeId` + 拓扑绑定上。
    fn face_hashes(&mut self, shape: ShapeHandle) -> KernelResult<Vec<u64>>;

    // ---- 拾取：点投影（M5-B4）------------------------------------------------

    /// **点到面的最近点投影**（mm）。返回最近点 + 面局部 uv + 距离。
    ///
    /// ⚠ **不是"射线命中判定"**：平面投影**无界**（任意远点都有结果）；
    /// 只有最近点不存在/不唯一（如圆柱面 + 轴上点）才 `Err`。
    /// 因此**不能**靠它判断"射线是否打中面"——命中判定须自建（射线-三角粗筛，B4.1）。
    fn project_point_on_face(
        &mut self,
        face: ShapeHandle,
        point: DVec3,
    ) -> KernelResult<PointOnFace>;

    /// **点到边的最近点投影**（mm）。返回最近点 + 单位切向 + 曲线参数（**非归一化**）。
    /// 查询点远离该边时返回 `Err`（领域错误）。
    fn project_point_on_edge(
        &mut self,
        edge: ShapeHandle,
        point: DVec3,
    ) -> KernelResult<PointOnEdge>;

    // ---- 拾取：法向 / 参数域（M5-B4.1b）--------------------------------------

    /// **面在 `(u, v)` 处的单位法向**（mm，单位向量）。
    ///
    /// `(u, v)` 取自 [`PointOnFace`]（同一次拾取的面局部参数）。用于确定拾取面的
    /// 朝向（如高亮、拉伸方向）。外露 [`DVec3`] 而非 vendor 类型（铁律①）。
    fn surface_normal(&mut self, face: ShapeHandle, u: f64, v: f64) -> KernelResult<DVec3>;

    /// **曲线的参数域 `[first, last]`**（**非归一化**）。
    ///
    /// 用于把 [`PointOnEdge::param`] 归一化到 `[0, 1]`。仿真实内核的
    /// `curve_parameters`（元数 **2**）。
    fn curve_parameters(&mut self, curve: ShapeHandle) -> KernelResult<CurveRange>;

    /// **曲线在参数 `param` 处的点**（mm）。仿真实内核的 `curve_point_at_param`。
    ///
    /// 与 [`Self::curve_parameters`] 配套：`param` 取该曲线的**原始**参数域
    /// （如取参数域中点）。这是**跨会话稳定的代表点**来源——实测对圆弧边，
    /// 参数域中点往返位移 ≤ 3.2e-14。
    ///
    /// **为何不用最近点投影当代表点**（B6.1 护栏；**与往返稳定性无关**）：
    /// 1. 它**依赖外部查询点、不是边的内在量**——同一条圆弧换个查询点就得到另一个
    ///    "代表点"（同一弧上两点可相距 16.64 mm）；而参数域中点
    ///    `t = (first + last) / 2` 是**内在唯一量**。
    /// 2. 查询点取**圆弧圆心**（原点 / 形状中心）等退化位置时最近点不唯一 / 不存在
    ///    → 投影失败。
    ///
    /// 最近点投影本身**跨 BREP 往返是稳定的**（实测 ≤ 5.9e-14），故其被弃用
    /// **不是**因为"跨往返漂移"。
    ///
    /// 外露 [`DVec3`] 而非 vendor 类型（铁律①）。
    fn curve_point_at_param(&mut self, edge: ShapeHandle, param: f64) -> KernelResult<DVec3>;

    /// **曲线的几何长度**（mm）。仿真实内核的 `curve_length`。
    ///
    /// 这是 `TopoName::of_edge` 的 `length` **唯一来源**：`curve_parameters` 的
    /// `last − first` 是**参数域跨度**，对**角度参数化**的圆弧边**不是长度**
    /// （实测 φ15 孔边 span = π/2 ≈ 1.571，而弧长 = 15·π/2 ≈ 23.562）；
    /// `bounding_box(edge)` 延展与 `|tangent|` 也都**不可**当长度。
    fn curve_length(&mut self, edge: ShapeHandle) -> KernelResult<f64>;

    // ---- HLR -----------------------------------------------------------------

    /// 计算工程图级消隐投影（设计文档 3.4）。
    ///
    /// 返回值必须包含**解析曲线**（圆仍是圆），否则无法用于出图。
    fn project_edges(&mut self, shape: ShapeHandle, view: ViewDef) -> KernelResult<Projection>;

    // ---- 持久化互操作（设计文档 3.3 / 4.2）------------------------------------

    /// 把持久化的几何正文**装回**成会话句柄。
    ///
    /// 这是"句柄可丢、按需重建"的一半：文档里存的是 `core-model` 的
    /// `GeometryRecord`（BREP 文本），打开文档时用它把句柄重建出来。
    ///
    /// `format` 决定正文的解析方式（一阶段实现 BREP 文本；STEP 属 M4 互操作）。
    fn import_brep(&mut self, text: &str) -> KernelResult<ShapeHandle>;

    /// 把会话句柄**导出**成持久化正文（BREP 文本）。这是"存盘"的一半。
    fn export_brep(&mut self, shape: ShapeHandle) -> KernelResult<String>;

    /// 实体体积（mm³）。用于布尔正确性的数值校验与存盘前后一致性检查。
    ///
    /// 非实体（面 / 壳 / 线）返回 `0.0`，不报错。
    fn volume(&mut self, shape: ShapeHandle) -> KernelResult<f64>;

    // ---- 生命周期 ------------------------------------------------------------

    /// 句柄是否仍然有效。
    fn is_alive(&self, shape: ShapeHandle) -> bool;

    /// 释放句柄。重复释放应返回错误。
    fn release(&mut self, shape: ShapeHandle) -> KernelResult<()>;

    /// 当前存活形状数量（用于调试与泄漏自查）。
    fn live_count(&self) -> usize;

    /// 当前所有存活句柄（升序）。
    ///
    /// 用于**临时物统一回收**：在 `before = live_handles()` 之后跑一段代码，
    /// 再用 `after` 与 `before` 的差集找出这段代码新造的句柄并释放它们
    /// （见 [`Kernel::scope`](crate::Kernel::scope)）。
    ///
    /// 之所以做成必需方法而不是给个默认空实现：**静默返回空集会让
    /// "回收临时物"变成一句空话**，而调用方看不出任何异常。
    fn live_handles(&self) -> Vec<ShapeHandle>;
}

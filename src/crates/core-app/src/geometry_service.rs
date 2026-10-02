//! 几何服务：**持久态 `ShapeId` ↔ 会话态 `ShapeHandle` 的唯一映射持有者**。
//!
//! ## 这个映射为什么必须存在，且必须在这里
//!
//! 设计文档 3.3 把标识符分成了两套：
//!
//! | | `ShapeId` | `ShapeHandle` |
//! |---|---|---|
//! | 是什么 | UUID v7，**可存盘** | arena `u32`，**会话内有效** |
//! | 谁持有 | `core-model`（数据层）| `core-kernel`（内核层）|
//! | 生命周期 | 与文档同生共死 | 与进程同生共死 |
//!
//! 两层的**依赖方向铁律**（`core-kernel` 不许依赖 `core-model`）意味着
//! 内核**无法自己**知道"这个句柄对应文档里的哪个构件"。所以映射只能落在
//! **同时看得见两边的层**——`core-app`。
//!
//! ## 懒重建（设计文档 3.3）
//!
//! 打开一个 5000 构件的文档，不需要 5000 个内核句柄——内核创建开销是
//! 100–500 ms/实例，而句柄本身也吃内存。所以：
//!
//! ```text
//! 视图第一次要显示某构件 → resolve(ShapeId) → 查缓存
//!                                        ├─ 命中 → 直接用
//!                                        └─ 未命中 → 从 GeometryRecord 装载 → 缓存
//! ```
//!
//! 装载动作本身由 [`ShapeLoader`] 提供——因为"BREP 文本 → 内核句柄"要用真实后端。
//! **M1 已交付**：[`crate::loader::BrepShapeLoader`] 按 `GeomFormat` 分派到
//! [`Kernel::import_brep`]，并作为 [`GeometryService::new`] 的默认装载器。
//!
//! ## 存盘方向（M1 补齐）
//!
//! 反向的"句柄 → BREP 文本"由 [`GeometryService::snapshot`] 提供：
//!
//! ```text
//! 配方执行 → 句柄 ──snapshot──▶ GeometryRecord ──insert_geometry──▶ 文档
//! ```
//!
//! 一来一回就构成了设计文档 3.3 所说的"**句柄可丢**"：
//! 句柄只是会话缓存，随时可以丢掉再 [`resolve`](GeometryService::resolve) 重建。

use std::collections::BTreeMap;

use core_kernel::{Kernel, KernelError, KernelResult, MeshData, ShapeHandle};
use core_model::{Document, GeometryRecord, ShapeId, Value};
use core_sdk::{BuildOp, OpOutput};

use crate::error::{AppError, AppResult};
use crate::evolution_align::HistoryRecord;
use crate::selector::{EdgeSelector, FaceSelector, parse_edge_selectors, parse_face_selectors};
use crate::sketch::{SketchEntity, normalize_arc_angles};
use crate::topo_naming::face_hash_names_from_handle;

/// 把持久化的几何记录**装回**成内核句柄。
///
/// 默认实现是 [`BrepShapeLoader`](crate::loader::BrepShapeLoader)：按 `format`
/// 分派到内核门面的 BREP 读写接口（`GeomFormat::BrepText` 可用，
/// `BrepBinary` / `Step` 明确报错并指向后续里程碑）。
///
/// 之所以定义成 trait 而不是让 `GeometryService` 直接调内核：
/// 1. 测试可以注入 `FakeLoader` 来单独验证缓存 / 回收逻辑，不依赖真实后端；
/// 2. 将来可能要按格式分派（BREP 走一条路、STEP 走另一条路）；
/// 3. [`UnavailableLoader`] 可以显式表达"这一刻不具备装载能力"。
pub trait ShapeLoader: Send + Sync {
    /// 装载一份几何记录。
    fn load(&mut self, kernel: &mut Kernel, record: &GeometryRecord) -> KernelResult<ShapeHandle>;
}

/// 一个会**拒绝一切**的装载器：用于显式表达"当前策略下不装载任何几何"。
///
/// 默认装载器是 [`BrepShapeLoader`](crate::loader::BrepShapeLoader)；
/// 这个类型留给"必须确认装载从未发生"的测试与只读 / 审阅场景。
///
/// 它比 `None` 好的地方在于：错误信息能说清"是哪一份几何没能装载"。
pub struct UnavailableLoader;

impl ShapeLoader for UnavailableLoader {
    fn load(&mut self, _kernel: &mut Kernel, record: &GeometryRecord) -> KernelResult<ShapeHandle> {
        Err(KernelError::Geometry {
            op: "ShapeLoader::load",
            msg: format!(
                "几何装载被显式禁用：拒绝把 {} 重建为内核句柄（当前安装的是 UnavailableLoader）",
                record.shape_id
            ),
        })
    }
}

/// 一次配方执行的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutedRecipe {
    /// 每一步产出的句柄（与配方步骤一一对应）
    pub results: Vec<ShapeHandle>,
    /// 配方带来的警告（透传）+ 本服务补充的**可观测**回退说明
    pub warnings: Vec<String>,
    /// 本次执行中**带历史算子**产出的记录（`步骤序号 → HistoryRecord`）。
    ///
    /// [`HistoryRecord`] = 内核级 [`Evolution`](core_kernel::Evolution) + **布尔当刻
    /// 从活句柄采集**的"会话面哈希 → 稳定名"表（输入 / 输出两侧）。只有真正调用了
    /// 带历史布尔（[`Kernel::fuse_with_history`] / [`Kernel::cut_with_history`]）的
    /// 步骤才会出现在这里；后端不具备该能力（如占位后端）时**回退为无历史算子并置空**，
    /// 同时往 [`warnings`] 写一条可观测说明（绝不静默装作有历史）。
    pub evolutions: Vec<(usize, HistoryRecord)>,
}

impl ExecutedRecipe {
    /// 最终结果句柄（配方为空时为 `None`）。
    #[must_use]
    pub fn final_handle(&self) -> Option<ShapeHandle> {
        self.results.last().copied()
    }

    /// 执行了多少步。
    #[must_use]
    pub fn step_count(&self) -> usize {
        self.results.len()
    }

    /// 中间结果全部句柄（含最终结果）——**由调用方负责释放**。
    #[must_use]
    pub fn handles(&self) -> &[ShapeHandle] {
        &self.results
    }

    /// 本次执行收集到的带历史记录（`步骤序号 → HistoryRecord`）。
    ///
    /// 调用方据步骤号把记录对到自己的 `ShapeId` 上（例如第 k 步的结果句柄
    /// 被收进文档时用的 `ShapeId`），从而构造
    /// [`update_after_commit`](crate::update_service::update_after_commit) 的
    /// `histories` 入参。空 = 本次执行没有任何带历史算子（或后端不支持）。
    #[must_use]
    pub fn evolutions(&self) -> &[(usize, HistoryRecord)] {
        &self.evolutions
    }

    /// 第 `step` 步产出的带历史记录（若有）。
    #[must_use]
    pub fn evolution_for_step(&self, step: usize) -> Option<&HistoryRecord> {
        self.evolutions
            .iter()
            .find(|(index, _)| *index == step)
            .map(|(_, evo)| evo)
    }
}

/// 几何服务。
pub struct GeometryService {
    kernel: Kernel,
    /// 持久 → 会话
    handles: BTreeMap<ShapeId, ShapeHandle>,
    /// 会话 → 持久（反向查询，拾取时要用：拿到一个句柄，反查是哪个构件）
    reverse: BTreeMap<u32, ShapeId>,
    loader: Box<dyn ShapeLoader>,
    /// 每形状最近一次的带历史记录（**会话态，M6b 路径 B**：`GeometryRecord`
    /// 不携带 history，历史由调用方在 snapshot 收几何进文档时经
    /// [`GeometryService::remember_history`] 登记，供 [`GeometryService::history_of`]
    /// 回读——M6b 会话态，不落盘容器）。
    histories: BTreeMap<ShapeId, HistoryRecord>,
    /// 拾取网格化缓存（ShapeId → 固定挠度 tessellate 结果，**会话态**，M12）。
    ///
    /// 拾取链路每次都用固定挠度重跑内核 `tessellate`——真实内核（wasm JIT）
    /// 代价可观，同一形状反复拾取时白跑。按 `ShapeId` 缓存；
    /// [`GeometryService::cache`] / [`GeometryService::forget`] 同步失效
    /// （句柄替换 / 解除 → 旧网格必错），开新会话是全新服务（天然空缓存）。
    pick_meshes: BTreeMap<ShapeId, MeshData>,
}

impl GeometryService {
    /// 占位内核 + 默认（BREP）装载器的服务。
    ///
    /// ⚠ 这是刻意的：默认状态下 `Kernel::new()` 会失败（未启用 `occt` feature），
    /// 若这里偷偷退回 mock，用户会以为"几何真的算出来了"。
    /// 想用占位内核请显式调本方法，**不要**让 [`GeometryService::new`] 静默降级。
    ///
    /// 占位内核配 [`BrepShapeLoader`](crate::loader::BrepShapeLoader) 不矛盾：
    /// 装载器只负责"按格式选接口"，占位后端读写的是自带的自描述文本格式。
    #[must_use]
    pub fn with_mock_kernel() -> Self {
        Self::with_kernel(Kernel::mock())
    }

    /// 尝试用**真实内核**构造（M0 起可用，需 `--features occt`）。
    ///
    /// 未启用 `occt` feature 时返回 [`AppError::Kernel`]（`Unsupported`），
    /// **不会**退回占位内核。默认装载器为 [`BrepShapeLoader`](crate::loader::BrepShapeLoader)。
    pub fn new() -> AppResult<Self> {
        Ok(Self::with_kernel(Kernel::new()?))
    }

    /// 用指定内核构造（测试 / 专门后端）；装载器取默认的 [`BrepShapeLoader`](crate::loader::BrepShapeLoader)。
    #[must_use]
    pub fn with_kernel(kernel: Kernel) -> Self {
        Self {
            kernel,
            handles: BTreeMap::new(),
            reverse: BTreeMap::new(),
            loader: Box::new(crate::loader::BrepShapeLoader::new()),
            histories: BTreeMap::new(),
            pick_meshes: BTreeMap::new(),
        }
    }

    /// 替换几何装载器（测试注入 `FakeLoader`，或换成 [`UnavailableLoader`] 显式禁用）。
    pub fn install_loader(&mut self, loader: Box<dyn ShapeLoader>) {
        self.loader = loader;
    }

    // ---------- 几何落盘（句柄 → 记录）----------

    /// 把一个会话句柄**导出成持久记录**（设计文档 3.3 的"存盘"方向）。
    ///
    /// 与 [`resolve`](GeometryService::resolve) 互为逆运算：
    ///
    /// ```text
    /// snapshot:  ShapeHandle ──▶ GeometryRecord（BREP 文本，可进 .bimdoc）
    /// resolve:   GeometryRecord ──▶ ShapeHandle
    /// ```
    ///
    /// **不写文档**：本方法只产出记录。要不要收进文档、以哪个 `ShapeId` 收，
    /// 是调用方（用例层）的决定——`core-app` 不做这类业务判断。
    pub fn snapshot(&mut self, shape: ShapeId, handle: ShapeHandle) -> AppResult<GeometryRecord> {
        let text = self.kernel.export_brep(handle)?;
        Ok(GeometryRecord::brep(shape, text))
    }

    /// **快照并收进文档**：`snapshot` + `Document::insert_geometry`。
    ///
    /// 已存在同一 `ShapeId` 的记录时返回 [`AppError::Model`]
    /// （`DuplicateId`）——不覆盖，避免"悄悄把别人的几何替换掉"。
    pub fn store(
        &mut self,
        doc: &mut Document,
        shape: ShapeId,
        handle: ShapeHandle,
    ) -> AppResult<GeometryRecord> {
        let record = self.snapshot(shape, handle)?;
        doc.insert_geometry(record.clone())?;
        Ok(record)
    }

    // ---------- 带历史演化（M6b 路径 B：会话态，不落盘容器）----------

    /// 登记某形状最近一次的带历史记录（**会话态**）。
    ///
    /// 由调用方在"snapshot 收几何进文档"的时机调用：`execute()` 产出的
    /// [`ExecutedRecipe::evolutions`]（步骤序号 → [`HistoryRecord`]）里，
    /// **结果句柄即该形状句柄**的那条演化，就是该形状的构造历史。同形状重复
    /// 登记会**覆盖**旧记录（只保留最近一次）。
    ///
    /// ⚠ 记录里的 [`Evolution`](core_kernel::Evolution) 携带的会话句柄在句柄释放后
    /// **悬垂**——[`HistoryRecord`] 的消费方（[`resolve_delta`](crate::evolution_align::resolve_delta)）
    /// 只读面级演化分组与两张名字表，不解引用句柄字段。
    pub fn remember_history(&mut self, shape: ShapeId, record: HistoryRecord) {
        self.histories.insert(shape, record);
    }

    /// 某形状最近一次的带历史记录（若有；会话态，重开即丢）。
    #[must_use]
    pub fn history_of(&self, shape: ShapeId) -> Option<&HistoryRecord> {
        self.histories.get(&shape)
    }

    /// 后端名（诊断用）。
    #[must_use]
    pub fn backend_name(&self) -> &'static str {
        self.kernel.backend_name()
    }

    /// 后端可信度（真实几何内核 vs 占位后端）。
    #[must_use]
    pub fn backend_kind(&self) -> core_kernel::BackendKind {
        self.kernel.kind()
    }

    /// 只读内核（网格化 / 投影等只读操作）。
    #[must_use]
    pub fn kernel(&self) -> &Kernel {
        &self.kernel
    }

    /// 可变内核。
    pub fn kernel_mut(&mut self) -> &mut Kernel {
        &mut self.kernel
    }

    // ---------- 映射管理 ----------

    /// 登记一条映射。若该 `ShapeId` 已有映射，**旧句柄的反向项会被清掉**。
    pub fn cache(&mut self, shape: ShapeId, handle: ShapeHandle) {
        if let Some(old) = self.handles.insert(shape, handle)
            && old != handle
        {
            self.reverse.remove(&old.raw());
        }
        self.reverse.insert(handle.raw(), shape);
        // 句柄替换 → 旧网格缓存必错，同步失效（M12）。
        self.pick_meshes.remove(&shape);
    }

    /// 持久 → 会话。
    #[must_use]
    pub fn handle_of(&self, shape: ShapeId) -> Option<ShapeHandle> {
        self.handles.get(&shape).copied()
    }

    /// 会话 → 持久（拾取路径用）。
    #[must_use]
    pub fn shape_of(&self, handle: ShapeHandle) -> Option<ShapeId> {
        self.reverse.get(&handle.raw()).copied()
    }

    /// 是否已缓存。
    #[must_use]
    pub fn is_cached(&self, shape: ShapeId) -> bool {
        self.handles.contains_key(&shape)
    }

    /// 缓存条数。
    #[must_use]
    pub fn cached_count(&self) -> usize {
        self.handles.len()
    }

    /// 是否尚未缓存任何几何。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }

    /// 解除映射（**不释放内核句柄**）。
    pub fn forget(&mut self, shape: ShapeId) -> Option<ShapeHandle> {
        let handle = self.handles.remove(&shape)?;
        self.reverse.remove(&handle.raw());
        // 映射解除 → 网格缓存同步失效（M12）。
        self.pick_meshes.remove(&shape);
        Some(handle)
    }

    /// 释放内核句柄并解除映射。
    pub fn release(&mut self, shape: ShapeId) -> AppResult<()> {
        if let Some(handle) = self.forget(shape) {
            self.kernel.release(handle)?;
        }
        Ok(())
    }

    /// **惰性重建**：拿到 `ShapeId` 对应的内核句柄，必要时装载。
    pub fn resolve(&mut self, doc: &Document, shape: ShapeId) -> AppResult<ShapeHandle> {
        if let Some(handle) = self.handle_of(shape) {
            return Ok(handle);
        }
        let record = doc
            .geometry_record(shape)
            .ok_or(core_model::ModelError::GeometryNotFound(shape))?;
        let handle = self.loader.load(&mut self.kernel, record)?;
        // 借用结束后再写缓存
        self.cache(shape, handle);
        Ok(handle)
    }

    /// **垃圾回收**：丢掉"文档里已经没有对应几何记录"的映射。
    ///
    /// 返回被回收的条数。用途：执行了一批删除操作之后调用一次，
    /// 防止缓存里攒着一堆指向已删构件的句柄（内存泄漏的典型来源）。
    pub fn collect_garbage(&mut self, doc: &Document) -> usize {
        let stale: Vec<ShapeId> = self
            .handles
            .keys()
            .filter(|id| doc.geometry_record(**id).is_none())
            .copied()
            .collect();
        for id in &stale {
            self.forget(*id);
        }
        stale.len()
    }

    /// 全部已缓存的持久 id（测试 / 诊断用）。
    #[must_use]
    pub fn cached_ids(&self) -> Vec<ShapeId> {
        self.handles.keys().copied().collect()
    }

    /// **拾取网格化（带缓存，M12）**：同 `ShapeId` 重复拾取不再重跑内核 tessellate。
    ///
    /// 拾取链路用固定挠度（PICK_LINEAR / PICK_ANGULAR）网格化做粗筛——真实内核
    /// （wasm JIT）代价可观。缓存按 `ShapeId` 记（`cache` / `forget` 同步失效，
    /// 见字段文档）；**不同挠度不共享缓存**——本方法只服务拾取这一固定挠度，
    /// 渲染层另有自己的挠度（GUI 的 `VIEW_LINEAR`），互不混用。
    pub fn tessellate_for_pick(
        &mut self,
        shape: ShapeId,
        linear: f64,
        angular: f64,
    ) -> AppResult<MeshData> {
        if let Some(mesh) = self.pick_meshes.get(&shape) {
            return Ok(mesh.clone());
        }
        let Some(handle) = self.handle_of(shape) else {
            return Err(AppError::Model(core_model::ModelError::GeometryNotFound(
                shape,
            )));
        };
        let mesh = self.kernel.tessellate(handle, linear, angular)?;
        self.pick_meshes.insert(shape, mesh.clone());
        Ok(mesh)
    }

    // ---------- 配方执行 ----------

    /// **执行配方**（设计文档 3.5）：把 SDK 给的 [`OpOutput`] 变成内核句柄。
    ///
    /// ## 当前能执行哪些算子
    ///
    /// | 算子 | 状态 |
    /// |---|---|
    /// | `MakeBox` / `MakeCylinder` / `MakeSphere` / `MakeCone` / `MakeTorus` | ✅ |
    /// | `Fuse` / `Cut` / `Common` | ✅ |
    /// | `Translate` / `Rotate` | ✅ |
    /// | `Extrude` / `Revolve` | ✅（轮廓 → 体） |
    /// | `Fillet` / `Chamfer` / `Shell` / `Offset` / `Thicken` | ✅（M5-B6.2b 接线） |
    /// | `Loft` / `Sweep` / `Pipe` | ✅（操作数经 MakeWire/MakeFace 可表达） |
    /// | `MakeLineEdge` / `MakeArcEdge` / `MakeWire` / `MakeFace` / `ExtractWire` | ✅（M7a 线框生产：从零造线框/面） |
    ///
    /// M3 起**全部 `BuildOp` 都已接线**；任何**非法参数**都会经真实内核错误路径
    /// （`KernelError::InvalidArgument` / `Geometry`）被**明确报错**，绝不静默跳过
    /// ——静默跳过会让"桥架少了一段"这种问题在几百步之后才被发现。
    ///
    /// ## 调用方须知
    ///
    /// 返回的句柄**尚未**写进文档。要把句柄落成 `GeometryRecord`（BREP 文本）
    /// 请接着调 [`snapshot`](GeometryService::snapshot) / [`store`](GeometryService::store)
    /// ——即"执行 → 落盘"是一条两段式链路，中间由调用方决定收哪些结果。
    ///
    /// ## 失败即回滚
    ///
    /// 任一步失败时，本方法会**先释放这次执行已经造出的全部句柄**，再返回原错误
    /// （不改写错误信息）。否则一个中途失败的配方会在内核里留下无人认领的
    /// "半成品几何"，跑几次废配方就把 arena 撑爆——这正是本方法的泄漏防线。
    pub fn execute(&mut self, recipe: &OpOutput) -> AppResult<ExecutedRecipe> {
        if !recipe.is_well_ordered() {
            return Err(AppError::RecipeNotExecutable(
                "步骤顺序或输入个数不正确（依赖必须指向更早的步骤）".to_string(),
            ));
        }

        let mut results: Vec<ShapeHandle> = Vec::with_capacity(recipe.steps.len());
        let mut evolutions: Vec<(usize, HistoryRecord)> = Vec::new();
        let mut notices: Vec<String> = Vec::new();
        for (index, step) in recipe.steps.iter().enumerate() {
            match self.execute_step(index, step, &results, &mut notices) {
                Ok((handle, evolution)) => {
                    results.push(handle);
                    if let Some(evo) = evolution {
                        evolutions.push((index, evo));
                    }
                }
                Err(err) => {
                    // 回滚：释放本次已创建的全部句柄，再原样抛出错误。
                    // `release_many` 会跳过已被消费的句柄，不会打断整轮清理。
                    self.kernel.release_many(&results);
                    return Err(err);
                }
            }
        }

        let mut warnings = recipe.warnings.clone();
        warnings.extend(notices);

        Ok(ExecutedRecipe {
            results,
            warnings,
            evolutions,
        })
    }

    /// **草图 wire 构建**（G4-M15）：把草图 2D 图元序列化成内核解析边并连成 wire。
    ///
    /// 逐实体造边（z 平面 = 世界 z = 0，法向 (0, 0, 1)）：折线 → 相邻顶点直段
    /// （[`Kernel::make_line_edge`]）；整圆 → [`Kernel::make_circle_edge`]；
    /// 圆弧 → 角度归一（[`normalize_arc_angles`]，终角 < 起角 +2π）后
    /// [`Kernel::make_circle_arc`]；椭圆 → [`Kernel::make_ellipse_edge`]
    /// （主轴沿 +x），`major_angle` 非零时再「造边 + `rotate`」组合
    /// （vendor 无主轴方向参数，见 [`crate::sketch::SketchEntity::Ellipse`]）。
    ///
    /// **回滚纪律**（照 [`Self::execute`]）：任一图元失败即释放全部已建边，
    /// 再原样抛错——绝不留半成品；`make_wire` 失败同样回滚。成功时中间边
    /// 句柄全部释放（句柄可丢，文本为真相——设计文档 3.3），只返回 wire。
    ///
    /// 辅助图元（点 / 构造线 / 射线）**不进 wire**——组装时跳过；跳过后
    /// 无任何有效边（全辅助草图）→ [`AppError::RecipeNotExecutable`]。
    ///
    /// # Errors
    /// 空图元表 / 折线顶点不足 2 / 全辅助无有效边 →
    /// [`AppError::RecipeNotExecutable`]；
    /// 图元非法（非有限坐标、非正半径、零法向、短半轴 > 长半轴、贝塞尔段
    /// 控制点不足 2）或内核构造失败 → [`AppError::Kernel`]。
    pub fn build_sketch_wire(&mut self, entities: &[SketchEntity]) -> AppResult<ShapeHandle> {
        if entities.is_empty() {
            return Err(AppError::RecipeNotExecutable(
                "草图图元为空：至少需要 1 个图元才能建 wire".to_string(),
            ));
        }
        let edges = self.assemble_sketch_edges(entities)?;
        if edges.is_empty() {
            return Err(AppError::RecipeNotExecutable(
                "草图中无进 wire 的图元：点 / 构造线 / 射线是辅助图元（不进 wire），\
                 至少需要 1 个几何图元"
                    .to_string(),
            ));
        }
        let result = self.kernel.make_wire(&edges);
        self.kernel.release_many(&edges);
        result.map_err(Into::into)
    }

    /// 逐实体造草图解析边（[`Self::build_sketch_wire`] 的组装底座）。
    ///
    /// 折线 1 个实体可产出多条直段（相邻顶点 windows(2)）；贝塞尔每段产 1 条边
    /// （每段控制点序列 ≥ 2，一条 `make_bezier_edge` 直通）；其余图元各产 1 条边。
    /// **辅助图元（点 / 构造线 / 射线）产 0 条边**——不进 wire。
    /// **任一图元失败即释放全部已建边**再原样抛错（回滚纪律：谁分配谁回收单点化）。
    fn assemble_sketch_edges(&mut self, entities: &[SketchEntity]) -> AppResult<Vec<ShapeHandle>> {
        let mut edges: Vec<ShapeHandle> = Vec::new();
        for (i, entity) in entities.iter().enumerate() {
            let built: AppResult<Vec<ShapeHandle>> = match entity {
                SketchEntity::Polyline(points) => {
                    if points.len() < 2 {
                        Err(AppError::RecipeNotExecutable(format!(
                            "草图第 {i} 个图元（折线）至少需要 2 个顶点，实得 {}",
                            points.len()
                        )))
                    } else {
                        points
                            .windows(2)
                            .map(|w| {
                                self.kernel
                                    .make_line_edge(w[0].x, w[0].y, 0.0, w[1].x, w[1].y, 0.0)
                                    .map_err(Into::into)
                            })
                            .collect()
                    }
                }
                SketchEntity::Bezier(segments) => {
                    if segments.is_empty() {
                        Err(AppError::RecipeNotExecutable(format!(
                            "草图第 {i} 个图元（贝塞尔）至少需要 1 段，实得 0 段"
                        )))
                    } else {
                        segments
                            .iter()
                            .enumerate()
                            .map(|(j, seg)| {
                                if seg.len() < 2 {
                                    Err(AppError::RecipeNotExecutable(format!(
                                        "草图第 {i} 个图元的第 {j} 段贝塞尔至少需要 \
                                         2 个控制点，实得 {}",
                                        seg.len()
                                    )))
                                } else {
                                    let flat: Vec<[f64; 3]> =
                                        seg.iter().map(|p| [p.x, p.y, 0.0]).collect();
                                    self.kernel.make_bezier_edge(&flat).map_err(Into::into)
                                }
                            })
                            .collect()
                    }
                }
                // 辅助图元：不进 wire（组装跳过，0 条边）。
                SketchEntity::Point(_) | SketchEntity::AuxLine { .. } => Ok(vec![]),
                SketchEntity::Circle { center, radius } => self
                    .kernel
                    .make_circle_edge(center.x, center.y, 0.0, 0.0, 0.0, 1.0, *radius)
                    .map(|h| vec![h])
                    .map_err(Into::into),
                SketchEntity::Arc {
                    center,
                    radius,
                    start_angle,
                    end_angle,
                } => {
                    let (s, e) = normalize_arc_angles(*start_angle, *end_angle);
                    self.kernel
                        .make_circle_arc(center.x, center.y, 0.0, 0.0, 0.0, 1.0, *radius, s, e)
                        .map(|h| vec![h])
                        .map_err(Into::into)
                }
                SketchEntity::Ellipse {
                    center,
                    major,
                    minor,
                    major_angle,
                } => {
                    let edge = self.kernel.make_ellipse_edge(
                        center.x, center.y, 0.0, 0.0, 0.0, 1.0, *major, *minor,
                    )?;
                    if major_angle.abs() > 1e-9 {
                        // 任意朝向：造边（主轴沿 +x）+ 绕过圆心、沿 z 轴旋转。
                        // rotate 返回新句柄、输入仍有效（不消费输入）——旧边照旧释放。
                        let rotated = self.kernel.rotate(
                            edge,
                            center.x,
                            center.y,
                            0.0,
                            0.0,
                            0.0,
                            1.0,
                            *major_angle,
                        );
                        let _ = self.kernel.release(edge);
                        rotated.map(|h| vec![h]).map_err(Into::into)
                    } else {
                        Ok(vec![edge])
                    }
                }
            };
            match built {
                Ok(mut list) => edges.append(&mut list),
                Err(err) => {
                    self.kernel.release_many(&edges);
                    return Err(err);
                }
            }
        }
        Ok(edges)
    }

    /// **对既有句柄直接执行单算子**（G3 原生算子门面的执行底座）。
    ///
    /// 与 [`Self::execute`] 的差异：输入不是配方步骤，而是**调用方解析好的句柄**
    /// （文档既有几何经 [`Self::resolve`] 懒重建）；配方由本方法构造——单步
    /// `BuildStep::new(op)` + `args` + `inputs` 顺序引用（第 k 个输入即
    /// `inputs[k]`）。
    ///
    /// 复用 [`Self::execute_step`] 的**全部算子分派**（含 `Fillet`/`Chamfer`/`Shell`
    /// 的 `single_solid` 子选择链路与 `Fuse`/`Cut` 的带历史布尔）与回滚纪律：
    /// 本步失败无产出可回滚；**输入句柄是调用方的**（文档既有几何），不在此释放
    /// ——`execute_step` 内部的临时句柄（`single_solid` / 选择器解析）照旧自管。
    ///
    /// # Errors
    /// 元数不匹配（[`BuildOp::arity_ok`] 单一口径）或算子执行失败时返回
    /// [`AppError::RecipeNotExecutable`] / [`AppError::Kernel`]。
    pub fn execute_native(
        &mut self,
        op: BuildOp,
        args: &BTreeMap<String, Value>,
        inputs: &[ShapeHandle],
    ) -> AppResult<ExecutedRecipe> {
        if !op.arity_ok(inputs.len()) {
            return Err(AppError::RecipeNotExecutable(format!(
                "{} 需要 {} 个输入形状，实得 {}",
                op.label(),
                op.arity(),
                inputs.len()
            )));
        }
        let mut step = core_sdk::BuildStep::new(op);
        step.args = args.clone();
        step.inputs = (0..inputs.len()).collect();

        let mut results: Vec<ShapeHandle> = Vec::new();
        let mut notices: Vec<String> = Vec::new();
        let mut evolutions: Vec<(usize, HistoryRecord)> = Vec::new();
        match self.execute_step(0, &step, inputs, &mut notices) {
            Ok((handle, evolution)) => {
                results.push(handle);
                if let Some(evo) = evolution {
                    evolutions.push((0, evo));
                }
            }
            // 失败即净：本步无产出；输入句柄归调用方，不在此释放。
            Err(err) => return Err(err),
        }
        Ok(ExecutedRecipe {
            results,
            warnings: notices,
            evolutions,
        })
    }

    /// 执行一步。`previous` 是前面各步的结果。
    ///
    /// 返回 `(句柄, 可选带历史记录)`：`Fuse` / `Cut` 走**带历史**路径，后端不具备该
    /// 能力时回退普通算子并置空记录（同时往 `notices` 写一条可观测说明）。
    fn execute_step(
        &mut self,
        index: usize,
        step: &core_sdk::BuildStep,
        previous: &[ShapeHandle],
        notices: &mut Vec<String>,
    ) -> AppResult<(ShapeHandle, Option<HistoryRecord>)> {
        // 输入解析：必须先校验，避免越界
        let input = |slot: usize| -> AppResult<ShapeHandle> {
            let target = step.inputs.get(slot).copied().ok_or_else(|| {
                AppError::RecipeNotExecutable(format!(
                    "第 {index} 步（{}）缺少第 {slot} 个输入",
                    step.op.label()
                ))
            })?;
            previous.get(target).copied().ok_or_else(|| {
                AppError::RecipeNotExecutable(format!(
                    "第 {index} 步（{}）引用了不存在的第 {target} 步结果",
                    step.op.label()
                ))
            })
        };

        let need = |key: &str| -> AppResult<f64> {
            step.arg_f64(key).ok_or_else(|| {
                AppError::RecipeNotExecutable(format!(
                    "第 {index} 步（{}）缺少参数 `{key}`",
                    step.op.label()
                ))
            })
        };

        // 修复类算子（`Sew` / `SewAndSolidify`）的缝合容差：键名固定 `tolerance`（mm），
        // 缺省落到与 `core_kernel::tolerance::POINT_COINCIDENT` 对齐的常量上
        // （`core-sdk` 不能依赖 `core-kernel`，故常量在 `core-sdk` 侧定义，见其文档）。
        let sew_tolerance = step
            .arg_f64("tolerance")
            .unwrap_or(core_sdk::geometry_op::DEFAULT_SEW_TOLERANCE_MM);

        let (handle, evolution) = match step.op {
            BuildOp::MakeBox => (
                self.kernel
                    .make_box(need("dx")?, need("dy")?, need("dz")?)?,
                None,
            ),
            BuildOp::MakeCylinder => (
                self.kernel
                    .make_cylinder(need("radius")?, need("height")?)?,
                None,
            ),
            BuildOp::MakeSphere => (self.kernel.make_sphere(need("radius")?)?, None),
            BuildOp::MakeCone => (
                self.kernel
                    .make_cone(need("r1")?, need("r2")?, need("height")?)?,
                None,
            ),
            BuildOp::MakeTorus => (
                self.kernel
                    .make_torus(need("major_radius")?, need("minor_radius")?)?,
                None,
            ),
            // ---- 带历史布尔（M5-B3 关联更新主路径）----
            // 优先 `*_with_history`；仅当后端明确不具备该能力（`Unsupported`，
            // 如占位后端）时回退普通算子并置空演化——真实内核路径**只调一次**。
            BuildOp::Fuse => {
                self.boolean_with_history(input(0)?, input(1)?, false, index, notices)?
            }
            BuildOp::Cut => {
                self.boolean_with_history(input(0)?, input(1)?, true, index, notices)?
            }
            BuildOp::Common => (self.kernel.common(input(0)?, input(1)?)?, None),
            BuildOp::Translate => (
                self.kernel
                    .translate(input(0)?, need("x")?, need("y")?, need("z")?)?,
                None,
            ),
            BuildOp::Rotate => (
                self.kernel.rotate(
                    input(0)?,
                    need("px")?,
                    need("py")?,
                    need("pz")?,
                    need("dx")?,
                    need("dy")?,
                    need("dz")?,
                    need("angle_rad")?,
                )?,
                None,
            ),
            BuildOp::Extrude => (
                self.kernel
                    .extrude(input(0)?, need("dx")?, need("dy")?, need("dz")?)?,
                None,
            ),
            BuildOp::Revolve => (
                self.kernel.revolve(
                    input(0)?,
                    need("px")?,
                    need("py")?,
                    need("pz")?,
                    need("dx")?,
                    need("dy")?,
                    need("dz")?,
                    need("angle_rad")?,
                )?,
                None,
            ),
            // ---- M4 修复类算子（一元；`Sew`/`SewAndSolidify` 走 `tolerance` 缺省）----
            BuildOp::Sew => (self.kernel.sew(input(0)?, sew_tolerance)?, None),
            BuildOp::SewAndSolidify => (
                self.kernel.sew_and_solidify(input(0)?, sew_tolerance)?,
                None,
            ),
            BuildOp::FixShape => (self.kernel.fix_shape(input(0)?)?, None),
            BuildOp::UnifySameDomain => (self.kernel.unify_same_domain(input(0)?)?, None),
            // ---- 八算子（M5-B6.2b：真实派发）--------------------------------------
            // 子形状选择器以 **JSON-in-`Value::Text`** 传入（`edge_selectors` / `face_selectors`），
            // 经 `crate::selector` 的编解码 helper 解析——**解析失败 / 缺 key 一律响亮 `Err`**，
            // 绝不静默当空选择器（空 `edge_ids` 会让 `fillet` 静默无效）。
            //
            // `Fillet` / `Chamfer` / `Shell` / `Offset` 先 `single_solid`（多体 → 响亮
            // `NotSingleSolid`，**绝不取 first**）；选择器**针对 `single_solid` 后的实体**解析
            // （子形状序号 / 身份以实体为准，不是 compound）。`single_solid` 与选择器 `resolve`
            // 都会分配新会话句柄 → 由 [`GeometryService::on_single_solid`] 在**成功与失败两条
            // 路径**都回收（照 `execute` 的 `release_many` 回滚风格）。`Thicken` **不** `single_solid`
            // （输入是面 / 壳，`single_solid` 会 `EmptyResult`）。
            BuildOp::Fillet => {
                let radius = need("radius")?;
                let selectors = require_edge_selectors(step, index)?;
                let operand = input(0)?;
                let out = self.on_single_solid(
                    operand,
                    &SubSelect::Edges(&selectors),
                    |k, solid, subs| k.fillet(solid, subs, radius),
                )?;
                (out, None)
            }
            BuildOp::Chamfer => {
                let distance = need("distance")?;
                let selectors = require_edge_selectors(step, index)?;
                let operand = input(0)?;
                let out = self.on_single_solid(
                    operand,
                    &SubSelect::Edges(&selectors),
                    |k, solid, subs| k.chamfer(solid, subs, distance),
                )?;
                (out, None)
            }
            BuildOp::Shell => {
                let thickness = need("thickness")?;
                let tolerance = model_tolerance(step);
                let selectors = require_face_selectors(step, index)?;
                let operand = input(0)?;
                let out = self.on_single_solid(
                    operand,
                    &SubSelect::Faces(&selectors),
                    |k, solid, subs| k.shell(solid, subs, thickness, tolerance),
                )?;
                (out, None)
            }
            BuildOp::Offset => {
                let distance = need("distance")?;
                let tolerance = model_tolerance(step);
                let operand = input(0)?;
                // 规格 §2.7.16.3：vendor `offset(solid_id, …)` → **建议**同 `Fillet` 走
                // `single_solid`（是否必须由 occt 腿实测裁定；见 B6.2b 报告）。
                let solid = self.kernel.single_solid(operand)?;
                let out = self.kernel.offset(solid, distance, tolerance);
                self.kernel.release(solid).ok();
                (out?, None)
            }
            BuildOp::Thicken => {
                // **不** `single_solid`：输入是**面 / 壳**（vendor `thicken(shape_id, …)`），
                // `single_solid` 会 `EmptyResult`（规格 §2.7.16.3 逐字）。
                let thickness = need("thickness")?;
                let tolerance = model_tolerance(step);
                let operand = input(0)?;
                (self.kernel.thicken(operand, thickness, tolerance)?, None)
            }
            BuildOp::Loft => {
                // 变长：`inputs` 顺序即放样顺序（N ≥ 2，由 `arity_ok` 前置把关）。
                let is_solid = step.arg_bool("is_solid").unwrap_or(true);
                let ruled = step.arg_bool("ruled").unwrap_or(false);
                let mut wires = Vec::with_capacity(step.inputs.len());
                for slot in 0..step.inputs.len() {
                    wires.push(input(slot)?);
                }
                (self.kernel.loft(&wires, is_solid, ruled)?, None)
            }
            BuildOp::Sweep => {
                // `inputs` 顺序恒为 `[profile, spine]`（规格 §2.7.16.2）。
                let mode = step.arg_i64("transition_mode").unwrap_or(0) as i32;
                (self.kernel.sweep(input(0)?, input(1)?, mode)?, None)
            }
            BuildOp::Pipe => (self.kernel.pipe(input(0)?, input(1)?)?, None),
            // ---- 线框生产（M7a：配方世界「从零造线框/面」的最小集）--------------
            // `MakeLineEdge` / `MakeArcEdge` 无输入（坐标即参数，键名照 M7a-1
            // 权威表，**零 angle 键**：弧走 vendor 三点式）；`MakeWire` 变长
            // （N ≥ 1，`inputs` 顺序即连接序，`arity_ok` 前置把关）；
            // `MakeFace` / `ExtractWire` 取 `input(0)`。
            BuildOp::MakeLineEdge => (
                self.kernel.make_line_edge(
                    need("x1")?,
                    need("y1")?,
                    need("z1")?,
                    need("x2")?,
                    need("y2")?,
                    need("z2")?,
                )?,
                None,
            ),
            BuildOp::MakeArcEdge => (
                self.kernel.make_arc_edge(
                    need("x1")?,
                    need("y1")?,
                    need("z1")?,
                    need("x2")?,
                    need("y2")?,
                    need("z2")?,
                    need("x3")?,
                    need("y3")?,
                    need("z3")?,
                )?,
                None,
            ),
            BuildOp::MakeWire => {
                // 变长：inputs 顺序即连接序（N ≥ 1，由 arity_ok 前置把关）。
                let mut edges = Vec::with_capacity(step.inputs.len());
                for slot in 0..step.inputs.len() {
                    edges.push(input(slot)?);
                }
                (self.kernel.make_wire(&edges)?, None)
            }
            // 贝塞尔样条边（G4 前置）：`points` 键 JSON 坐标数组（必填），
            // 解析见 `require_spline_points`（缺 key / 格式错一律响亮报错）。
            BuildOp::MakeSplineEdge => (
                self.kernel
                    .make_bezier_edge(&require_spline_points(step, index)?)?,
                None,
            ),
            BuildOp::MakeFace => (self.kernel.make_face(input(0)?)?, None),
            BuildOp::ExtractWire => (self.kernel.outer_wire(input(0)?)?, None),
        };
        Ok((handle, evolution))
    }

    /// **在 `single_solid` 后的实体上解析子形状选择器并调用算子**（M5-B6.2b 接线核心）。
    ///
    /// ## 为什么需要它
    ///
    /// `Fillet` / `Chamfer` / `Shell` 的输入是一个 **compound**（布尔 / 放样的结果恒为
    /// compound），而 `fillet` / `chamfer` / `shell` 要求**单实体**。故链路是：
    /// `single_solid` → **在实体上**解析选择器 → 调算子。`single_solid` 与选择器
    /// `resolve` 都分配**新会话句柄**，内核 arena 不自动回收——故**成功与失败两条路径**
    /// 都必须释放（否则跑几个废配方就把 arena 撑爆）。
    ///
    /// ## 回收口径
    ///
    /// - `single_solid` 失败（0 / >1 实体）：它**自己**已清理枚举出的句柄 → 直接透传错误；
    /// - 选择器解析中途失败：回收**已解析出的全部子形状** + `solid`，再抛原错；
    /// - 算子调用（成功或失败）：一律回收 `subs` + `solid`——vendor 语义为
    ///   "**返回新句柄、原句柄仍有效**"（同 `translate` / `sew`）。
    fn on_single_solid<F>(
        &mut self,
        operand: ShapeHandle,
        selectors: &SubSelect<'_>,
        op: F,
    ) -> AppResult<ShapeHandle>
    where
        F: FnOnce(&mut Kernel, ShapeHandle, &[ShapeHandle]) -> KernelResult<ShapeHandle>,
    {
        let solid = self.kernel.single_solid(operand)?;

        let mut subs: Vec<ShapeHandle> = Vec::new();
        let resolve_result: KernelResult<()> = match selectors {
            SubSelect::Edges(list) => conscribe(&mut self.kernel, solid, list, &mut subs),
            SubSelect::Faces(list) => conscribe(&mut self.kernel, solid, list, &mut subs),
        };
        if let Err(err) = resolve_result {
            self.kernel.release_many(&subs);
            self.kernel.release(solid).ok();
            return Err(err.into());
        }

        let result = op(&mut self.kernel, solid, &subs);
        // 成功与失败两条路径都回收临时句柄。
        self.kernel.release_many(&subs);
        self.kernel.release(solid).ok();
        Ok(result?)
    }

    /// **带历史布尔 + 布尔当刻采集面名 + 显式回退**。
    ///
    /// 行为契约（M5 §2.3(g)）：
    /// - 优先调 [`Kernel::fuse_with_history`] / [`Kernel::cut_with_history`]，
    ///   输入面哈希取**两个操作数的并集**（与 `core-kernel` 面哈希同域）；
    /// - ★ **在布尔当刻、句柄还活着**时，从两侧操作数与结果句柄采集
    ///   "会话面哈希 → 稳定名"表，随 [`HistoryRecord`] 透传——修 P0-1：
    ///   会话面哈希是实例级的，等 `update_after_commit` 重装文档即落进另一哈希域；
    /// - 后端返回 [`KernelError::Unsupported`]（占位后端 / 无此能力）时，
    ///   **回退**到普通 [`Kernel::fuse`] / [`Kernel::cut`]（置空记录）；
    /// - 其它错误照常抛出（不吞）。
    ///
    /// 回退**绝不静默**：往 `notices` 写一条可观测说明，最终进入
    /// [`ExecutedRecipe::warnings`]。真实内核路径**只调** `*_with_history` 一次，
    /// 不违反"M0 附录 A#13 不许两次布尔"。
    fn boolean_with_history(
        &mut self,
        a: ShapeHandle,
        b: ShapeHandle,
        is_cut: bool,
        index: usize,
        notices: &mut Vec<String>,
    ) -> AppResult<(ShapeHandle, Option<HistoryRecord>)> {
        // 面哈希不可用时（不应发生在真实后端）取空表：仍会走 with_history，
        // 由后端自己决定是报 Unsupported 还是给出（可能为空的）演化。
        //
        // 口径与 `core-kernel` 的演化史钉子一致：**两个操作数的面哈希并集**
        // （演化会同时牵涉被切体与刀具体两侧的面）。
        let mut hashes = self.kernel.face_hashes(a).unwrap_or_default();
        hashes.extend(self.kernel.face_hashes(b).unwrap_or_default());

        // ★ 布尔当刻、句柄还活着——采集两侧的"面哈希 → 稳定名"。
        //   会话面哈希是实例级的，跨文档重装即失效；此处必须趁句柄存活摘名。
        let mut input_names = match face_hash_names_from_handle(&mut self.kernel, a) {
            Ok(names) => names,
            Err(err) => {
                notices.push(format!(
                    "第 {index} 步：无法采集操作数 A 的面名（{err}），本步演化对齐将退避到几何指纹"
                ));
                BTreeMap::new()
            }
        };
        match face_hash_names_from_handle(&mut self.kernel, b) {
            Ok(names) => input_names.extend(names),
            Err(err) => notices.push(format!(
                "第 {index} 步：无法采集操作数 B 的面名（{err}），本步演化对齐将退避到几何指纹"
            )),
        }

        let attempt = if is_cut {
            self.kernel.cut_with_history(a, b, &hashes)
        } else {
            self.kernel.fuse_with_history(a, b, &hashes)
        };

        match attempt {
            Ok(evo) => {
                let output_names = match face_hash_names_from_handle(&mut self.kernel, evo.result) {
                    Ok(names) => names,
                    Err(err) => {
                        notices.push(format!(
                            "第 {index} 步：无法采集结果面名（{err}），本步演化对齐将退避到几何指纹"
                        ));
                        BTreeMap::new()
                    }
                };
                let handle = evo.result;
                Ok((
                    handle,
                    Some(HistoryRecord::new(evo, input_names, output_names)),
                ))
            }
            Err(KernelError::Unsupported(reason)) => {
                let label = if is_cut { "Cut" } else { "Fuse" };
                notices.push(format!(
                    "第 {index} 步（{label}）：后端不支持带历史布尔（{reason}），\
                     已回退为无历史算子，本步骤不产出 Evolution\
                     （关联更新对本形状将退避到几何指纹）"
                ));
                let handle = if is_cut {
                    self.kernel.cut(a, b)?
                } else {
                    self.kernel.fuse(a, b)?
                };
                Ok((handle, None))
            }
            Err(err) => Err(err.into()),
        }
    }
}

/// 取 `shell` / `offset` / `thicken` 的建模容差：`tolerance` 键（可选）缺省落到
/// [`core_sdk::geometry_op::DEFAULT_SEW_TOLERANCE_MM`]（**复用**，不另立重复常量）。
///
/// **实测裁定（B6.2b）**：该缺省 `1e-7`（与 `core_kernel::tolerance::POINT_COINCIDENT`
/// 对齐）在 occt 腿上**可行**——`shell`（t=1/2）、`offset`（d=1/2）、`thicken`（t=1/2）
/// 各 2 组参数经真实 `execute` / dispatch 全绿（体积 / 包围盒 oracle 均满足），故**不**
/// 另立 1e-6 常量。
fn model_tolerance(step: &core_sdk::BuildStep) -> f64 {
    step.arg_f64("tolerance")
        .unwrap_or(core_sdk::geometry_op::DEFAULT_SEW_TOLERANCE_MM)
}

/// 取并解析**必填**的 `edge_selectors`（`Value::Text` JSON 数组）。
///
/// 缺 key / 解析失败一律 [`AppError::RecipeNotExecutable`]（**绝不静默空表**）。
fn require_edge_selectors(
    step: &core_sdk::BuildStep,
    index: usize,
) -> AppResult<Vec<EdgeSelector>> {
    let text = step.arg_text("edge_selectors").ok_or_else(|| {
        AppError::RecipeNotExecutable(format!(
            "第 {index} 步（{}）缺少参数 `edge_selectors`",
            step.op.label()
        ))
    })?;
    parse_edge_selectors(text).map_err(|err| {
        AppError::RecipeNotExecutable(format!("第 {index} 步（{}）：{err}", step.op.label()))
    })
}

/// 取并解析**必填**的 `face_selectors`（`Value::Text` JSON 数组）。
///
/// 缺 key / 解析失败一律 [`AppError::RecipeNotExecutable`]（**绝不静默空表**）。
fn require_face_selectors(
    step: &core_sdk::BuildStep,
    index: usize,
) -> AppResult<Vec<FaceSelector>> {
    let text = step.arg_text("face_selectors").ok_or_else(|| {
        AppError::RecipeNotExecutable(format!(
            "第 {index} 步（{}）缺少参数 `face_selectors`",
            step.op.label()
        ))
    })?;
    parse_face_selectors(text).map_err(|err| {
        AppError::RecipeNotExecutable(format!("第 {index} 步（{}）：{err}", step.op.label()))
    })
}

/// 取并解析**必填**的 `points`（贝塞尔样条控制点，`Value::Text` JSON
/// `[[x,y,z],...]`，N ≥ 2）——`MakeSplineEdge` 专用（G4 前置）。
///
/// 缺 key / 解析失败 / 点数不足一律 [`AppError::RecipeNotExecutable`]
///（**绝不静默**）。
fn require_spline_points(step: &core_sdk::BuildStep, index: usize) -> AppResult<Vec<[f64; 3]>> {
    let text = step.arg_text("points").ok_or_else(|| {
        AppError::RecipeNotExecutable(format!(
            "第 {index} 步（{}）缺少参数 `points`",
            step.op.label()
        ))
    })?;
    let raw: Vec<[f64; 3]> = serde_json::from_str(text).map_err(|err| {
        AppError::RecipeNotExecutable(format!(
            "第 {index} 步（{}）：`points` JSON 解析失败：{err}",
            step.op.label()
        ))
    })?;
    if raw.len() < 2 {
        return Err(AppError::RecipeNotExecutable(format!(
            "第 {index} 步（{}）：`points` 至少需要 2 个控制点，实得 {}",
            step.op.label(),
            raw.len()
        )));
    }
    Ok(raw)
}

/// [`GeometryService::on_single_solid`] 的选择器种类（边 / 面）。
enum SubSelect<'a> {
    Edges(&'a [EdgeSelector]),
    Faces(&'a [FaceSelector]),
}

/// 选择器解析的共用抽象——让 [`conscribe`] 对边 / 面两种选择器**一份代码**。
trait Resolvable {
    fn resolve_on(&self, kernel: &mut Kernel, solid: ShapeHandle)
    -> KernelResult<Vec<ShapeHandle>>;
}

impl Resolvable for EdgeSelector {
    fn resolve_on(
        &self,
        kernel: &mut Kernel,
        solid: ShapeHandle,
    ) -> KernelResult<Vec<ShapeHandle>> {
        self.resolve(kernel, solid)
    }
}

impl Resolvable for FaceSelector {
    fn resolve_on(
        &self,
        kernel: &mut Kernel,
        solid: ShapeHandle,
    ) -> KernelResult<Vec<ShapeHandle>> {
        self.resolve(kernel, solid)
    }
}

/// 把 `list` 里每个选择器（针对 `solid`）解析并累积进 `out`；**任一失败即响亮返回原错**
/// （已累积的句柄由调用方回收，不在此处 release——保证"谁分配谁回收"单点）。
fn conscribe<R: Resolvable>(
    kernel: &mut Kernel,
    solid: ShapeHandle,
    list: &[R],
    out: &mut Vec<ShapeHandle>,
) -> KernelResult<()> {
    for sel in list {
        let mut resolved = sel.resolve_on(kernel, solid)?;
        out.append(&mut resolved);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "occt")]
    use core_kernel::SubShapeKind;
    use core_kernel::tolerance;
    use core_model::{Category, Element, GeomFormat};

    /// 一个"假装能装载"的装载器：无论内容如何都造一个 10×10×10 的盒子。
    ///
    /// 用途：测通缓存与回收逻辑，而不依赖真实后端。
    struct FakeLoader {
        loads: usize,
    }

    impl ShapeLoader for FakeLoader {
        fn load(
            &mut self,
            kernel: &mut Kernel,
            _record: &GeometryRecord,
        ) -> KernelResult<ShapeHandle> {
            self.loads += 1;
            kernel.make_box(10.0, 10.0, 10.0)
        }
    }

    fn doc_with_geometry(shape: ShapeId) -> Document {
        let mut doc = Document::new("d");
        doc.insert_geometry(GeometryRecord {
            shape_id: shape,
            format: GeomFormat::BrepText,
            text: "CASCADE Topology V3".to_string(),
        })
        .unwrap();
        doc
    }

    #[test]
    fn mock_service_reports_its_backend() {
        let g = GeometryService::with_mock_kernel();
        assert_eq!(g.backend_name(), "mock");
        assert_eq!(g.backend_kind(), core_kernel::BackendKind::Mock);
        assert!(g.is_empty());
    }

    #[test]
    #[cfg(not(feature = "occt"))]
    fn real_kernel_needs_the_occt_feature_so_new_reports_unsupported() {
        // 未启用 occt feature 时构不出真实内核——必须报错，不许静默降级成 mock
        assert!(matches!(GeometryService::new(), Err(AppError::Kernel(_))));
    }

    /// M1 验收门禁：**真实后端**下 `GeometryService::new()` 必须真的可用。
    ///
    /// 这条**会失败**（而不是跳过）——理由见 `core-kernel::occt` 里
    /// `occt_feature_must_actually_instantiate_the_kernel` 的说明：
    /// 内核加载失败时若所有用例都静默跳过，整套 `--features occt` 会假绿。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn with_the_occt_feature_new_returns_a_real_kernel() {
        let g = GeometryService::new().unwrap_or_else(|e| {
            panic!(
                "启用 `occt` feature 后 `GeometryService::new()` 必须成功，实得：{e}\n\
                 注意：`core-kernel --features occt` 里同名门禁用例会给出根因提示。"
            )
        });
        assert_eq!(g.backend_name(), "occt-wasm");
        assert_eq!(g.backend_kind(), core_kernel::BackendKind::Real);
        assert!(g.is_empty());
    }

    /// M1 验收（端到端）：真实内核跑配方 → 落盘 BREP 文本 → **换一个会话**重新装载 → 体量一致。
    ///
    /// 这是设计文档 3.3「句柄可丢、文本为真相」在真实后端上的最终证明。
    ///
    /// 内核不可用时**报告后跳过**：门禁由上面那条 `new_returns_a_real_kernel` 负责，
    /// 这里不必再制造第二个失败信号。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn real_kernel_execute_snapshot_resolve_keeps_the_volume() {
        let Ok(mut g1) = GeometryService::new() else {
            eprintln!("⚠ 跳过端到端用例：真实内核不可用（根因见门禁用例）。");
            return;
        };

        let recipe = OpOutput::of(vec![
            core_sdk::BuildStep::new(BuildOp::MakeBox)
                .with_f64("dx", 600.0)
                .with_f64("dy", 150.0)
                .with_f64("dz", 6000.0),
        ]);
        let shape = ShapeId::new();
        let mut doc = Document::new("d");

        // ① 第一会话：真内核执行 + 导出 BREP 文本
        let executed = g1.execute(&recipe).unwrap();
        let handle = executed.final_handle().unwrap();
        let original = g1.kernel_mut().volume(handle).unwrap();
        assert!(
            (original - 600.0 * 150.0 * 6000.0).abs() / original < 1e-6,
            "体积应接近 5.4e8 mm³，实得 {original}"
        );

        let record = g1.store(&mut doc, shape, handle).unwrap();
        assert!(
            record.text.contains("CASCADE"),
            "真实后端导出的必须是 OCCT BREP 文本，实得前 80 字符：{}",
            record.text.chars().take(80).collect::<String>()
        );
        drop(g1); // 会话结束，句柄全部作废

        // ② 第二会话：只用文档里的文本重建
        let mut g2 = GeometryService::new().unwrap();
        let reloaded = g2.resolve(&doc, shape).unwrap();
        let reloaded_volume = g2.kernel_mut().volume(reloaded).unwrap();
        assert!(
            (reloaded_volume - original).abs() / original < 1e-9,
            "重建后体积必须与原始一致：{original} vs {reloaded_volume}"
        );
    }

    /// **采集侧 + 缝合的端到端钉子（真实内核）**：配方走 `Cut` 的**带历史成功路径**，
    /// `boolean_with_history` 必须在**布尔当刻**采集到非空的面名表并装入
    /// [`HistoryRecord`]；再由
    /// [`update_after_commit`](crate::update_service::update_after_commit) 经
    /// `ShapeId ↔ HistoryRecord` 缝合，跨文档重装域驱动演化对齐。
    ///
    /// 这是 QA 指出的覆盖缺口的收口：把采集到的 `input_names` / `output_names`
    /// 置空（M1 变异），本用例必须在采集侧断言处变红——**"跳过"与"通过"可区分**。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn boolean_records_face_names_and_feeds_associative_update_end_to_end() {
        let mut g = GeometryService::new().expect("occt 腿必须能造真内核");
        assert!(
            g.backend_kind() == core_kernel::BackendKind::Real,
            "本用例必须跑真实内核，不许在占位后端上假绿；实得 {}",
            g.backend_name()
        );

        // 配方：100³ 盒 − r15 h200 圆柱（沿 Z 贯通）→ 带历史 Cut。
        let recipe = OpOutput::of(vec![
            core_sdk::BuildStep::new(BuildOp::MakeBox)
                .with_f64("dx", 100.0)
                .with_f64("dy", 100.0)
                .with_f64("dz", 100.0),
            core_sdk::BuildStep::new(BuildOp::MakeCylinder)
                .with_f64("radius", 15.0)
                .with_f64("height", 200.0),
            core_sdk::BuildStep::new(BuildOp::Cut).from_steps(vec![0, 1]),
        ]);
        let executed = g.execute(&recipe).unwrap();

        // ---- ③ 采集侧钉子（M1 变异在此变红）----
        let hrec = executed
            .evolution_for_step(2)
            .expect("Cut 必须产出带历史记录（成功路径）");
        assert!(
            !hrec.input_names.is_empty(),
            "布尔当刻必须采集到非空输入面名表（空表=采集侧失效）"
        );
        assert!(
            !hrec.output_names.is_empty(),
            "布尔当刻必须采集到非空输出面名表"
        );
        assert!(
            !hrec.evolution.modified.is_empty() || !hrec.evolution.generated.is_empty(),
            "Cut 的面级演化分组不得同时为空"
        );

        // ---- ④ 缝合钉子：ShapeId ↔ HistoryRecord → 演化对齐 ----
        let shape = ShapeId::new();
        let result_handle = executed.final_handle().unwrap();
        let mut doc = Document::new("布尔端到端");
        // `store` 只导出 BREP 文本、**不释放**句柄；重投时从 doc 重新装载。
        g.store(&mut doc, shape, result_handle).unwrap();

        // 选一个"确实被改到"的输入面（`outputs_of` 非空）。
        let chosen = hrec
            .input_names
            .iter()
            .find(|(h, _)| !hrec.evolution.outputs_of(**h).is_empty())
            .map(|(_, n)| *n)
            .expect("cut 必改到某输入面，否则本用例无意义");
        let old_key = core_model::GeometryKey::face_named(shape, 0, chosen);

        let mut drawing = core_doc::Drawing::new();
        let mut view = core_doc::View::new("正立面", core_doc::ViewKind::Elevation)
            .with_orientation(core_doc::ViewOrientation::front());
        view.push_annotation(
            core_doc::Annotation::linear(glam::DVec2::ZERO, glam::DVec2::new(10.0, 0.0))
                .bound_to(old_key.clone()),
        );
        drawing.add_view(view);
        drawing.rebuild_index();

        let histories = [(shape, hrec.clone())];
        let report = crate::update_service::update_after_commit(
            g.kernel_mut(),
            &doc,
            &mut drawing,
            &[shape],
            &histories,
        )
        .unwrap();

        assert!(
            report.evolution_resolved >= 1,
            "带历史布尔的成功路径必须真的驱动演化对齐（跨 ShapeId 缝合）；\
             实得 evolution_resolved={}",
            report.evolution_resolved
        );
        let rebound = drawing.views.values().next().unwrap().annotations[0]
            .bound_to
            .clone();
        assert!(
            rebound.as_ref().and_then(|k| k.topo_name()) != Some(&chosen),
            "绑定必须改到演化输出面（不再是输入面）；实得 {rebound:?}"
        );
    }

    #[test]
    fn caching_is_bidirectional() {
        let mut g = GeometryService::with_mock_kernel();
        let shape = ShapeId::new();
        let handle = g.kernel_mut().make_box(1.0, 1.0, 1.0).unwrap();

        g.cache(shape, handle);
        assert_eq!(g.handle_of(shape), Some(handle));
        assert_eq!(g.shape_of(handle), Some(shape));
        assert!(g.is_cached(shape));
        assert_eq!(g.cached_count(), 1);
    }

    #[test]
    fn recaching_a_shape_clears_the_stale_reverse_entry() {
        let mut g = GeometryService::with_mock_kernel();
        let shape = ShapeId::new();
        let old = g.kernel_mut().make_box(1.0, 1.0, 1.0).unwrap();
        let new = g.kernel_mut().make_box(2.0, 2.0, 2.0).unwrap();

        g.cache(shape, old);
        g.cache(shape, new);

        assert_eq!(g.handle_of(shape), Some(new));
        assert_eq!(g.shape_of(old), None, "旧句柄的反向项必须被清掉");
        assert_eq!(g.shape_of(new), Some(shape));
        assert_eq!(g.cached_count(), 1);
    }

    #[test]
    fn resolve_uses_the_cache_on_the_second_call() {
        let shape = ShapeId::new();
        let doc = doc_with_geometry(shape);
        let mut g = GeometryService::with_mock_kernel();
        g.install_loader(Box::new(FakeLoader { loads: 0 }));

        let a = g.resolve(&doc, shape).unwrap();
        let b = g.resolve(&doc, shape).unwrap();
        assert_eq!(a, b, "第二次应命中缓存");
        assert_eq!(g.cached_count(), 1);
    }

    #[test]
    fn resolve_reports_missing_geometry_record() {
        let doc = Document::new("d");
        let mut g = GeometryService::with_mock_kernel();
        g.install_loader(Box::new(FakeLoader { loads: 0 }));
        assert!(matches!(
            g.resolve(&doc, ShapeId::new()),
            Err(AppError::Model(core_model::ModelError::GeometryNotFound(_)))
        ));
    }

    #[test]
    fn default_loader_refuses_rather_than_pretending() {
        let shape = ShapeId::new();
        let doc = doc_with_geometry(shape);
        let mut g = GeometryService::with_mock_kernel();
        // 显式换成"拒绝一切"的装载器 → 必须报错，不能悄悄返回一个假句柄
        g.install_loader(Box::new(UnavailableLoader));
        assert!(matches!(g.resolve(&doc, shape), Err(AppError::Kernel(_))));
        assert!(g.is_empty());
    }

    #[test]
    fn the_default_loader_is_the_brep_loader_not_a_refusing_stub() {
        // 反向断言：**不**装任何东西时，默认装载器是能用的那个。
        // 这条测试防止将来有人把默认值改回 UnavailableLoader 而无人察觉。
        let mut g = GeometryService::with_mock_kernel();
        let handle = g.kernel_mut().make_box(2.0, 2.0, 2.0).unwrap();
        let text = g.kernel_mut().export_brep(handle).unwrap();

        let shape = ShapeId::new();
        let mut doc = Document::new("d");
        doc.insert_geometry(GeometryRecord::brep(shape, text))
            .unwrap();

        let resolved = g.resolve(&doc, shape).expect("默认装载器应能读 BREP 文本");
        assert!(g.kernel().is_alive(resolved));
    }

    /// **M1 关键闭环**：配方执行 → 落盘成记录 → 换一个服务重新装载 → 几何等价。
    ///
    /// 这验证了"句柄可丢"：句柄只是会话缓存，文档里那份 BREP 文本才是真相。
    #[test]
    fn execute_then_snapshot_then_resolve_reproduces_the_same_volume() {
        let recipe = OpOutput::of(vec![
            core_sdk::BuildStep::new(BuildOp::MakeBox)
                .with_f64("dx", 10.0)
                .with_f64("dy", 20.0)
                .with_f64("dz", 30.0),
        ]);

        // ① 第一个会话：执行配方，量体积
        let shape = ShapeId::new();
        let mut doc = Document::new("d");
        let mut g1 = GeometryService::with_mock_kernel();
        let executed = g1.execute(&recipe).unwrap();
        let handle = executed.final_handle().unwrap();
        let original_volume = g1.kernel_mut().volume(handle).unwrap();
        assert!((original_volume - 6000.0).abs() < 1e-9);

        // ② 落盘
        let record = g1.store(&mut doc, shape, handle).unwrap();
        assert_eq!(record.format, core_model::GeomFormat::BrepText);
        assert!(!record.text.is_empty(), "落盘记录必须有正文");
        drop(g1); // 会话结束：句柄全部作废

        // ③ 第二个会话：从文档重新装载
        let mut g2 = GeometryService::with_mock_kernel();
        assert!(!g2.is_cached(shape));
        let reloaded = g2.resolve(&doc, shape).unwrap();
        let reloaded_volume = g2.kernel_mut().volume(reloaded).unwrap();
        assert!(
            (reloaded_volume - original_volume).abs() < 1e-9,
            "体量必须一致"
        );
    }

    #[test]
    fn store_refuses_to_overwrite_an_existing_record() {
        let shape = ShapeId::new();
        let mut doc = doc_with_geometry(shape);
        let mut g = GeometryService::with_mock_kernel();
        let handle = g.kernel_mut().make_box(1.0, 1.0, 1.0).unwrap();

        // doc 里已经有 shape 这条记录 → 不许覆盖
        assert!(matches!(
            g.store(&mut doc, shape, handle),
            Err(AppError::Model(core_model::ModelError::DuplicateId(_)))
        ));
    }

    #[test]
    fn snapshot_of_a_released_handle_is_an_error_not_empty_text() {
        let shape = ShapeId::new();
        let mut g = GeometryService::with_mock_kernel();
        let handle = g.kernel_mut().make_box(1.0, 1.0, 1.0).unwrap();
        g.kernel_mut().release(handle).unwrap();

        assert!(matches!(
            g.snapshot(shape, handle),
            Err(AppError::Kernel(core_kernel::KernelError::InvalidHandle(_)))
        ));
    }

    #[test]
    fn garbage_collection_drops_mappings_for_deleted_geometry() {
        let kept = ShapeId::new();
        let removed = ShapeId::new();
        let mut doc = doc_with_geometry(kept);
        doc.insert_geometry(GeometryRecord::brep(removed, "x"))
            .unwrap();

        let mut g = GeometryService::with_mock_kernel();
        g.install_loader(Box::new(FakeLoader { loads: 0 }));
        g.resolve(&doc, kept).unwrap();
        g.resolve(&doc, removed).unwrap();
        assert_eq!(g.cached_count(), 2);

        doc.remove_geometry(removed);
        assert_eq!(g.collect_garbage(&doc), 1);
        assert_eq!(g.cached_count(), 1);

        // 顺带确认：被回收后仍能重新装载
        assert_eq!(g.collect_garbage(&doc), 0);
    }

    #[test]
    fn forget_and_release_keep_the_kernel_consistent() {
        let shape = ShapeId::new();
        let mut g = GeometryService::with_mock_kernel();
        let handle = g.kernel_mut().make_box(1.0, 1.0, 1.0).unwrap();
        g.cache(shape, handle);

        assert_eq!(g.forget(shape), Some(handle));
        assert_eq!(g.cached_count(), 0);
        assert!(g.kernel().is_alive(handle), "forget 不释放内核句柄");

        g.release(shape).unwrap();
        assert!(
            g.kernel().is_alive(handle),
            "已解除映射，release 找不到句柄"
        );
    }

    #[test]
    fn release_actually_frees_a_cached_handle() {
        let shape = ShapeId::new();
        let mut g = GeometryService::with_mock_kernel();
        let handle = g.kernel_mut().make_box(1.0, 1.0, 1.0).unwrap();
        g.cache(shape, handle);
        assert_eq!(g.kernel().live_count(), 1);

        g.release(shape).unwrap();
        assert_eq!(g.kernel().live_count(), 0);
    }

    #[test]
    fn executing_a_box_recipe_produces_a_handle() {
        let mut g = GeometryService::with_mock_kernel();
        let recipe = OpOutput::of(vec![
            core_sdk::BuildStep::new(BuildOp::MakeBox)
                .with_f64("dx", 600.0)
                .with_f64("dy", 150.0)
                .with_f64("dz", 6000.0),
        ]);

        let executed = g.execute(&recipe).unwrap();
        assert_eq!(executed.step_count(), 1);
        let h = executed.final_handle().unwrap();
        assert!(g.kernel().is_alive(h));

        let mesh = g
            .kernel_mut()
            .tessellate(
                h,
                tolerance::DEFAULT_LINEAR_DEFLECTION,
                tolerance::DEFAULT_ANGULAR_DEFLECTION,
            )
            .unwrap();
        assert_eq!(mesh.vertex_count(), 8);
        assert_eq!(mesh.triangle_count(), 12);
    }

    #[test]
    fn a_multi_step_boolean_recipe_executes() {
        let mut g = GeometryService::with_mock_kernel();
        let recipe = OpOutput::of(vec![
            core_sdk::BuildStep::new(BuildOp::MakeBox)
                .with_f64("dx", 100.0)
                .with_f64("dy", 100.0)
                .with_f64("dz", 100.0),
            core_sdk::BuildStep::new(BuildOp::MakeCylinder)
                .with_f64("radius", 20.0)
                .with_f64("height", 200.0),
            core_sdk::BuildStep::new(BuildOp::Cut).from_steps(vec![0, 1]),
        ]);

        let executed = g.execute(&recipe).unwrap();
        assert_eq!(executed.step_count(), 3);
        assert!(executed.final_handle().is_some());
        assert!(recipe.is_well_ordered());
    }

    #[test]
    fn recipe_warnings_are_passed_through() {
        let mut g = GeometryService::with_mock_kernel();
        let recipe = OpOutput::of(vec![
            core_sdk::BuildStep::new(BuildOp::MakeSphere).with_f64("radius", 5.0),
        ])
        .with_warning("球体半径偏小");
        let executed = g.execute(&recipe).unwrap();
        assert_eq!(executed.warnings.len(), 1);
    }

    /// **回退必须可察觉**：占位后端不具备带历史布尔能力 → 回退普通算子、置空演化，
    /// 并往 `warnings` 写一条说明（绝不静默装作"有历史"）。
    #[test]
    fn boolean_fallback_without_history_is_observable_not_silent() {
        let mut g = GeometryService::with_mock_kernel();
        let recipe = OpOutput::of(vec![
            core_sdk::BuildStep::new(BuildOp::MakeBox)
                .with_f64("dx", 100.0)
                .with_f64("dy", 100.0)
                .with_f64("dz", 100.0),
            core_sdk::BuildStep::new(BuildOp::MakeCylinder)
                .with_f64("radius", 20.0)
                .with_f64("height", 200.0),
            core_sdk::BuildStep::new(BuildOp::Cut).from_steps(vec![0, 1]),
        ]);

        let executed = g.execute(&recipe).unwrap();

        // 形状照旧产出（回退不改变布尔结果）
        let handle = executed.final_handle().unwrap();
        assert!(g.kernel().is_alive(handle));
        // 没有演化，且**可观测**
        assert!(executed.evolutions().is_empty(), "无历史后端不得产出演化");
        assert!(executed.evolution_for_step(2).is_none());
        assert!(
            executed
                .warnings
                .iter()
                .any(|w| w.contains("不支持带历史布尔")),
            "回退必须写一条可观测警告，绝不能静默；实得 warnings={:?}",
            executed.warnings
        );
    }

    #[test]
    fn missing_parameters_are_reported_before_touching_the_kernel() {
        let mut g = GeometryService::with_mock_kernel();
        let recipe = OpOutput::of(vec![
            core_sdk::BuildStep::new(BuildOp::MakeBox).with_f64("dx", 1.0),
        ]);
        assert!(matches!(
            g.execute(&recipe),
            Err(AppError::RecipeNotExecutable(_))
        ));
        assert_eq!(g.kernel().live_count(), 0, "失败不应留下半成品几何");
    }

    #[test]
    fn invalid_operator_arguments_are_refused_loudly() {
        // M3 起所有 BuildOp 都已接线，"未实现算子"这个前提不复存在；
        // 但"**非法参数必须经真实内核错误路径响亮失败**"这一契约仍然有效，
        // 只是驱动源从"未实现"换成了"真正非法的参数"。
        let mut g = GeometryService::with_mock_kernel();
        // 第 1 步造盒成功；第 2 步圆锥下底半径 r1 = -1 会被
        // `MockBackend::non_negative` 拒绝 → `KernelError::InvalidArgument`。
        let recipe = OpOutput::of(vec![
            core_sdk::BuildStep::new(BuildOp::MakeBox)
                .with_f64("dx", 1.0)
                .with_f64("dy", 1.0)
                .with_f64("dz", 1.0),
            core_sdk::BuildStep::new(BuildOp::MakeCone)
                .with_f64("r1", -1.0)
                .with_f64("r2", 0.0)
                .with_f64("height", 10.0),
        ]);
        let r = g.execute(&recipe);
        assert!(
            matches!(r, Err(AppError::Kernel(_))),
            "非法参数必须响亮失败（真实内核错误路径）：{r:?}"
        );
        assert_eq!(g.kernel().live_count(), 0, "失败不得留下半成品几何");
    }

    /// 中途失败必须**回滚**：已造出的句柄要全部释放，不留"半成品几何"。
    #[test]
    fn a_failing_middle_step_rolls_back_everything_created_so_far() {
        let mut g = GeometryService::with_mock_kernel();
        assert_eq!(g.kernel().live_count(), 0);
        let before = g.kernel().live_count();

        // 3 步：MakeBox（成功）→ MakeCone(r1 = -1，非法参数，失败）→ MakeBox（永不执行）
        let recipe = OpOutput::of(vec![
            core_sdk::BuildStep::new(BuildOp::MakeBox)
                .with_f64("dx", 1.0)
                .with_f64("dy", 1.0)
                .with_f64("dz", 1.0),
            core_sdk::BuildStep::new(BuildOp::MakeCone)
                .with_f64("r1", -1.0)
                .with_f64("r2", 0.0)
                .with_f64("height", 10.0),
            core_sdk::BuildStep::new(BuildOp::MakeBox)
                .with_f64("dx", 2.0)
                .with_f64("dy", 2.0)
                .with_f64("dz", 2.0),
        ]);

        let r = g.execute(&recipe);
        assert!(
            matches!(r, Err(AppError::Kernel(_))),
            "第 2 步（MakeCone，r1 = -1）应经真实内核错误路径失败：{r:?}"
        );
        assert_eq!(
            g.kernel().live_count(),
            before,
            "失败必须回滚——第 1 步造出的句柄不能被遗留"
        );

        // 回滚之后，一条正确配方仍应照常工作（句柄从干净状态重新计）
        let good = OpOutput::of(vec![
            core_sdk::BuildStep::new(BuildOp::MakeBox)
                .with_f64("dx", 3.0)
                .with_f64("dy", 4.0)
                .with_f64("dz", 5.0),
        ]);
        let executed = g.execute(&good).unwrap();
        assert_eq!(executed.step_count(), 1);
        assert!(g.kernel().is_alive(executed.final_handle().unwrap()));
        assert_eq!(g.kernel().live_count(), 1);
    }

    /// M3 接线后的**正向**用例：`Rotate` 现在真的会旋转（体积不变、姿态改变）。
    #[test]
    fn rotate_now_actually_rotates() {
        let mut g = GeometryService::with_mock_kernel();
        let recipe = OpOutput::of(vec![
            core_sdk::BuildStep::new(BuildOp::MakeBox)
                .with_f64("dx", 10.0)
                .with_f64("dy", 20.0)
                .with_f64("dz", 30.0),
            core_sdk::BuildStep::new(BuildOp::Rotate)
                .from_steps(vec![0])
                .with_f64("px", 0.0)
                .with_f64("py", 0.0)
                .with_f64("pz", 0.0)
                .with_f64("dx", 0.0)
                .with_f64("dy", 0.0)
                .with_f64("dz", 1.0)
                .with_f64("angle_rad", std::f64::consts::FRAC_PI_2),
        ]);
        let executed = g.execute(&recipe).unwrap();
        let h = executed.final_handle().unwrap();
        assert!(g.kernel().is_alive(h), "旋转产生新句柄，且应存活");

        // 刚体旋转不改变体积。
        assert!((g.kernel_mut().volume(h).unwrap() - 6000.0).abs() < 1e-9);
        // 绕 Z 轴 90°：世界包围盒的 x/y 尺寸互换 → 3D 姿态确实变了。
        let [sx, sy, _] = g.kernel_mut().bounding_box(h).unwrap().size();
        assert!(
            (sx - 20.0).abs() < 1e-6 && (sy - 10.0).abs() < 1e-6,
            "绕 Z 转 90° 后 x/y 应互换，实得 [{sx}, {sy}]"
        );
    }

    #[test]
    fn translate_operator_now_moves_the_shape() {
        // M1 补齐：Translate 已在内核门面接线，配方里的平移步骤会真正执行。
        let mut g = GeometryService::with_mock_kernel();
        let recipe = OpOutput::of(vec![
            core_sdk::BuildStep::new(BuildOp::MakeBox)
                .with_f64("dx", 10.0)
                .with_f64("dy", 20.0)
                .with_f64("dz", 30.0),
            core_sdk::BuildStep::new(BuildOp::Translate)
                .from_steps(vec![0])
                .with_f64("x", 1000.0)
                .with_f64("y", 0.0)
                .with_f64("z", 3000.0),
        ]);
        let executed = g.execute(&recipe).unwrap();
        assert_eq!(executed.step_count(), 2);
        let h = executed.final_handle().unwrap();
        assert!(g.kernel().is_alive(h));
        // 平移是刚体运动，体积不变
        assert!((g.kernel_mut().volume(h).unwrap() - 6000.0).abs() < 1e-9);
    }

    #[test]
    fn badly_ordered_recipes_are_refused() {
        let mut g = GeometryService::with_mock_kernel();
        let recipe = OpOutput::of(vec![
            core_sdk::BuildStep::new(BuildOp::Translate).from_steps(vec![5]),
        ]);
        assert!(matches!(
            g.execute(&recipe),
            Err(AppError::RecipeNotExecutable(_))
        ));
    }

    #[test]
    fn empty_recipe_executes_trivially() {
        let mut g = GeometryService::with_mock_kernel();
        let executed = g.execute(&OpOutput::empty()).unwrap();
        assert_eq!(executed.step_count(), 0);
        assert_eq!(executed.final_handle(), None);
    }

    #[test]
    fn picking_path_works_via_reverse_lookup() {
        // 拾取流程：渲染器给出句柄 → 反查持久 id → 拿到元素
        let mut doc = Document::new("d");
        let shape = ShapeId::new();
        doc.insert_geometry(GeometryRecord::brep(shape, "t"))
            .unwrap();
        let element = Element::new(Category::generic(), "柱").with_geometry(shape);
        let element_id = element.id;
        doc.insert_element(element).unwrap();

        let mut g = GeometryService::with_mock_kernel();
        g.install_loader(Box::new(FakeLoader { loads: 0 }));
        let handle = g.resolve(&doc, shape).unwrap();

        let picked_shape = g.shape_of(handle).unwrap();
        assert_eq!(picked_shape, shape);
        let picked_element = doc
            .elements()
            .values()
            .find(|e| e.geometry_ref == Some(picked_shape))
            .unwrap();
        assert_eq!(picked_element.id, element_id);
    }

    #[test]
    fn fake_loader_counts_its_loads() {
        // 确保"懒重建只发生一次"这条性质是可观测的
        let shape = ShapeId::new();
        let doc = doc_with_geometry(shape);
        let mut g = GeometryService::with_mock_kernel();
        g.install_loader(Box::new(FakeLoader { loads: 0 }));
        g.resolve(&doc, shape).unwrap();
        g.resolve(&doc, shape).unwrap();
        g.resolve(&doc, shape).unwrap();
        assert_eq!(g.cached_count(), 1);
    }

    // ===== M5-B6.2b：八算子接线钉子 =========================================

    /// 用 `crate::selector` 的 helper 编码选择器（**测试禁手搓 JSON**）。
    fn edge_json(selectors: &[EdgeSelector]) -> String {
        crate::selector::encode_edge_selectors(selectors).expect("编码 edge_selectors 不应失败")
    }

    fn face_json(selectors: &[FaceSelector]) -> String {
        crate::selector::encode_face_selectors(selectors).expect("编码 face_selectors 不应失败")
    }

    /// 一个 10³ 立方体步骤。
    fn a_cube() -> core_sdk::BuildStep {
        core_sdk::BuildStep::new(BuildOp::MakeBox)
            .with_f64("dx", 10.0)
            .with_f64("dy", 10.0)
            .with_f64("dz", 10.0)
    }

    /// **钉 11（M5-B6.2b）**：8 条 recipe（含合法参数与**已解析的选择器 JSON**）在 mock 后端
    /// 经**真实 `execute`** → `Err`，且 `live_count()` 回到 0（**回滚**）。
    ///
    /// 证明 8 个 dispatch 臂**真被消费**（而非编译进来却不可达）；mock 一律 `Unsupported` /
    /// `Geometry`，**绝不返假几何**（默认腿只验"响亮拒绝"）。
    #[test]
    fn new_ops_refuse_on_mock_through_execute() {
        let e0 = edge_json(&[EdgeSelector::ByIndex(0)]);
        let f0 = face_json(&[FaceSelector::ByIndex(0)]);

        let cases: Vec<(&str, OpOutput)> = vec![
            (
                "Fillet",
                OpOutput::of(vec![
                    a_cube(),
                    core_sdk::BuildStep::new(BuildOp::Fillet)
                        .with_f64("radius", 1.0)
                        .with_text("edge_selectors", e0.clone())
                        .from_steps(vec![0]),
                ]),
            ),
            (
                "Chamfer",
                OpOutput::of(vec![
                    a_cube(),
                    core_sdk::BuildStep::new(BuildOp::Chamfer)
                        .with_f64("distance", 1.0)
                        .with_text("edge_selectors", e0.clone())
                        .from_steps(vec![0]),
                ]),
            ),
            (
                "Shell",
                OpOutput::of(vec![
                    a_cube(),
                    core_sdk::BuildStep::new(BuildOp::Shell)
                        .with_f64("thickness", 1.0)
                        .with_text("face_selectors", f0.clone())
                        .from_steps(vec![0]),
                ]),
            ),
            (
                "Offset",
                OpOutput::of(vec![
                    a_cube(),
                    core_sdk::BuildStep::new(BuildOp::Offset)
                        .with_f64("distance", 1.0)
                        .from_steps(vec![0]),
                ]),
            ),
            (
                "Thicken",
                OpOutput::of(vec![
                    a_cube(),
                    core_sdk::BuildStep::new(BuildOp::Thicken)
                        .with_f64("thickness", 1.0)
                        .from_steps(vec![0]),
                ]),
            ),
            (
                "Loft",
                OpOutput::of(vec![
                    a_cube(),
                    a_cube(),
                    core_sdk::BuildStep::new(BuildOp::Loft).from_steps(vec![0, 1]),
                ]),
            ),
            (
                "Sweep",
                OpOutput::of(vec![
                    a_cube(),
                    a_cube(),
                    core_sdk::BuildStep::new(BuildOp::Sweep).from_steps(vec![0, 1]),
                ]),
            ),
            (
                "Pipe",
                OpOutput::of(vec![
                    a_cube(),
                    a_cube(),
                    core_sdk::BuildStep::new(BuildOp::Pipe).from_steps(vec![0, 1]),
                ]),
            ),
        ];

        for (name, recipe) in cases {
            let mut g = GeometryService::with_mock_kernel();
            assert!(recipe.is_well_ordered(), "{name} recipe 应结构合法");
            let got = g.execute(&recipe);
            assert!(
                got.is_err(),
                "{name} 在 mock 后端必须响亮 Err（绝不返假几何），实得 {got:?}"
            );
            assert_eq!(
                g.kernel().live_count(),
                0,
                "{name} 失败后句柄必须全部回滚（live_count 应为 0）"
            );
        }
    }

    // ===== M7a：线框生产接线钉（mock 腿）=====================================

    /// 一条 M7a 直边步骤（两端点，mm；键名照 M7a-1 权威表，零 angle 键）。
    fn line_step(x1: f64, y1: f64, z1: f64, x2: f64, y2: f64, z2: f64) -> core_sdk::BuildStep {
        core_sdk::BuildStep::new(BuildOp::MakeLineEdge)
            .with_f64("x1", x1)
            .with_f64("y1", y1)
            .with_f64("z1", z1)
            .with_f64("x2", x2)
            .with_f64("y2", y2)
            .with_f64("z2", z2)
    }

    /// **M7a mock 接线钉 1**：wire/face 配方在 mock 后端走**真实 `execute`** 成功并落盘。
    ///
    /// MakeLineEdge×4 → MakeWire → MakeFace（6 步）经真实 `execute` →
    /// `step_count()==6` + 每步句柄 `is_alive` + snapshot 落盘 `record.text` 非空，
    /// 且与「只有 wire 无 face」配方的落盘文本**不同**（「落盘文本真变了」钉）。
    /// 证明 M7a 派发臂**真被消费**（「能编译≠被消费」）。
    #[test]
    fn wire_ops_execute_on_mock_and_persist() {
        let mut g = GeometryService::with_mock_kernel();

        // 6 步配方：4 直边 → wire → face（单位正方形，XY 平面）
        let recipe = OpOutput::of(vec![
            line_step(0.0, 0.0, 0.0, 10.0, 0.0, 0.0),
            line_step(10.0, 0.0, 0.0, 10.0, 10.0, 0.0),
            line_step(10.0, 10.0, 0.0, 0.0, 10.0, 0.0),
            line_step(0.0, 10.0, 0.0, 0.0, 0.0, 0.0),
            core_sdk::BuildStep::new(BuildOp::MakeWire).from_steps(vec![0, 1, 2, 3]),
            core_sdk::BuildStep::new(BuildOp::MakeFace).from_steps(vec![4]),
        ]);
        assert!(recipe.is_well_ordered(), "wire/face recipe 应结构合法");
        let executed = g
            .execute(&recipe)
            .expect("mock 腿 wire/face 配方应执行成功");
        assert_eq!(executed.step_count(), 6, "6 步配方应产出 6 个结果");
        assert!(
            executed.results.iter().all(|&h| g.kernel().is_alive(h)),
            "每步句柄都应存活（成功路径不回滚）"
        );

        // 落盘：face 记录非空且带 face 标签
        let face_h = executed.final_handle().unwrap();
        let shape = ShapeId::new();
        let record = g.snapshot(shape, face_h).expect("落盘应成功");
        assert!(!record.text.is_empty(), "落盘记录必须有正文");
        assert!(
            record.text.contains(" face "),
            "face 落盘文本应带 face 标签：{}",
            record.text
        );

        // 对照配方「只有 wire 无 face」：落盘文本必须与 face 版**不同**
        let wire_recipe = OpOutput::of(vec![
            line_step(0.0, 0.0, 0.0, 10.0, 0.0, 0.0),
            line_step(10.0, 0.0, 0.0, 10.0, 10.0, 0.0),
            line_step(10.0, 10.0, 0.0, 0.0, 10.0, 0.0),
            line_step(0.0, 10.0, 0.0, 0.0, 0.0, 0.0),
            core_sdk::BuildStep::new(BuildOp::MakeWire).from_steps(vec![0, 1, 2, 3]),
        ]);
        let wire_executed = g.execute(&wire_recipe).expect("wire-only 配方应执行成功");
        let wire_h = wire_executed.final_handle().unwrap();
        let wire_text = g.kernel_mut().export_brep(wire_h).unwrap();
        assert!(!wire_text.is_empty());
        // M7b P3 补强：wire 落盘文本的**种类**断言——含 wire 标记，且不含
        // box / edge 标记（wire 是线框，不是实体毛坯也不是离散边）。
        assert!(
            wire_text.contains(" wire "),
            "wire 落盘文本应带 wire 标签：{wire_text}"
        );
        assert!(
            !wire_text.contains(" box ") && !wire_text.contains(" edge"),
            "wire 落盘文本不得带 box / edge 标签：{wire_text}"
        );
        assert_ne!(
            record.text, wire_text,
            "加 MakeFace 后落盘文本必须真变（否则 face 是静默成功）"
        );

        // 占位面诚实记账：线/面体积 = 0（backend `volume` 文档逐字）
        assert_eq!(
            g.kernel_mut().volume(face_h).unwrap(),
            0.0,
            "线/面体积应为 0.0，不报错"
        );
    }

    /// **M7a mock 接线钉 2**：mock 腿 MakeWire 之后接 Sweep / Pipe / Loft →
    /// execute **Err**（既有 mock 拒绝语义在派发层透传，前两步的 wire 真造出来了）
    /// + `live_count()==0`（**回滚红线**：失败时连成功造出的直边与 wire 一起回收）。
    #[test]
    fn wire_ops_refuse_on_mock_for_loft_sweep_pipe() {
        let build = |final_op: BuildOp| -> OpOutput {
            OpOutput::of(vec![
                line_step(0.0, 0.0, 0.0, 100.0, 0.0, 0.0),
                core_sdk::BuildStep::new(BuildOp::MakeWire).from_steps(vec![0]),
                core_sdk::BuildStep::new(final_op).from_steps(vec![0, 1]),
            ])
        };
        for (name, final_op) in [
            ("Sweep", BuildOp::Sweep),
            ("Pipe", BuildOp::Pipe),
            ("Loft", BuildOp::Loft),
        ] {
            let mut g = GeometryService::with_mock_kernel();
            let recipe = build(final_op);
            assert!(recipe.is_well_ordered(), "{name} recipe 应结构合法");
            let got = g.execute(&recipe);
            assert!(
                got.is_err(),
                "{name} 在 mock 后端必须响亮 Err（前两步 wire 成功，第三步拒绝），实得 {got:?}"
            );
            assert_eq!(
                g.kernel().live_count(),
                0,
                "{name} 失败后句柄必须全部回滚（含成功造出的直边与 wire）"
            );
        }
    }

    /// **M7a mock 接线钉 3**：MakeWire 0 输入被配方前置门拒绝
    /// （`execute` 的 `is_well_ordered` 门 + SDK 层 `validate_arity` 同口径）——
    /// 变长下限 1 的可观测行为。
    #[test]
    fn make_wire_zero_inputs_is_refused() {
        let recipe = OpOutput::of(vec![core_sdk::BuildStep::new(BuildOp::MakeWire)]);
        assert!(
            !recipe.is_well_ordered(),
            "0 输入的 MakeWire 不应通过有序判定"
        );

        let mut g = GeometryService::with_mock_kernel();
        let got = g.execute(&recipe);
        assert!(
            got.is_err(),
            "MakeWire 0 输入必须被前置门拒绝（变长下限 1），实得 {got:?}"
        );
        assert_eq!(g.kernel().live_count(), 0, "失败后不应留下句柄");

        // SDK 层同口径：validate_arity 对 0 输入报错
        assert!(
            core_sdk::BuildStep::new(BuildOp::MakeWire)
                .validate_arity()
                .is_err(),
            "SDK 层 validate_arity 对 MakeWire 0 输入应报错"
        );
    }

    /// **缺 `edge_selectors` 必须响亮 `Err`**（默认腿）：`fillet` 缺选择器 → 经真实 `execute`
    /// 在碰内核之前就报 `RecipeNotExecutable`，**绝不静默当空选择器**（空 `edge_ids` 会让
    /// `fillet` 静默无效）。
    #[test]
    fn fillet_missing_edge_selectors_is_refused_loudly() {
        let mut g = GeometryService::with_mock_kernel();
        let recipe = OpOutput::of(vec![
            a_cube(),
            core_sdk::BuildStep::new(BuildOp::Fillet)
                .with_f64("radius", 1.0)
                .from_steps(vec![0]),
        ]);
        match g.execute(&recipe) {
            Err(AppError::RecipeNotExecutable(msg)) => assert!(
                msg.contains("edge_selectors"),
                "错误信息应指明缺 `edge_selectors`，实得 {msg}"
            ),
            other => panic!("缺 edge_selectors 必须响亮 Err(RecipeNotExecutable)，实得 {other:?}"),
        }
        assert_eq!(g.kernel().live_count(), 0, "失败后必须回滚");
    }

    /// **F3③ 钉**：经**真实 `execute`** 喂**非法 `edge_selectors` JSON** → 必须 `Err`。
    ///
    /// 与 helper 直调 `parse_*` 分开：本钉覆盖**派发路径**（防"派发吞解析错 / 静默当空选择器"）。
    #[test]
    fn illegal_edge_selectors_json_is_refused_loudly_through_execute() {
        let mut g = GeometryService::with_mock_kernel();
        let recipe = OpOutput::of(vec![
            a_cube(),
            core_sdk::BuildStep::new(BuildOp::Fillet)
                .with_f64("radius", 1.0)
                .with_text("edge_selectors", "not-json")
                .from_steps(vec![0]),
        ]);
        match g.execute(&recipe) {
            Err(AppError::RecipeNotExecutable(msg)) => assert!(
                msg.contains("edge_selectors") && msg.contains("解析失败"),
                "非法 edge_selectors 应经派发响亮报解析失败，实得 {msg}"
            ),
            other => {
                panic!("非法 edge_selectors 必须响亮 Err(RecipeNotExecutable)，实得 {other:?}")
            }
        }
        assert_eq!(g.kernel().live_count(), 0, "失败后必须回滚");
    }

    /// **F3②′ 钉**：喂**多体（compound）**输入 → 必须响亮 `Err(NotSingleSolid{count:2})`，
    /// **绝不静默取首体**（钉"弃用 `single_solid` 结果"变异）。
    ///
    /// 构造：两盒 → 平移其一 → `Fuse` 得含 2 实体的 compound → `Fillet`。
    /// 若接线**弃用 `single_solid`**（或取 first 实体）继续做 `fillet`，则错误类型 / 时机将不同 → 本钉红。
    #[test]
    fn fillet_on_multi_solid_is_refused_loudly() {
        let mut g = GeometryService::with_mock_kernel();
        let recipe = OpOutput::of(vec![
            a_cube(),
            a_cube(),
            core_sdk::BuildStep::new(BuildOp::Translate)
                .with_f64("x", 100.0)
                .with_f64("y", 0.0)
                .with_f64("z", 0.0)
                .from_steps(vec![1]),
            core_sdk::BuildStep::new(BuildOp::Fuse).from_steps(vec![0, 2]),
            core_sdk::BuildStep::new(BuildOp::Fillet)
                .with_f64("radius", 1.0)
                .with_text("edge_selectors", edge_json(&[EdgeSelector::ByIndex(0)]))
                .from_steps(vec![3]),
        ]);
        match g.execute(&recipe) {
            Err(AppError::Kernel(KernelError::NotSingleSolid { count })) => {
                assert_eq!(count, 2, "应识别出 2 个实体");
            }
            other => panic!("多体 fillet 必须响亮 Err(NotSingleSolid)，实得 {other:?}"),
        }
        assert_eq!(g.kernel().live_count(), 0, "失败后必须回滚");
    }

    /// 真实后端守卫：occt 腿的钉子必须跑在真实内核上。
    #[cfg(feature = "occt")]
    fn real_service_or_panic(what: &str) -> GeometryService {
        let g = GeometryService::new().unwrap_or_else(|e| {
            panic!("{what} 必须跑在真实内核上，但 GeometryService::new() 失败：{e}")
        });
        assert_eq!(
            g.backend_kind(),
            core_kernel::BackendKind::Real,
            "{what} 必须在真实内核上，不许在 mock 上假绿"
        );
        g
    }

    /// **集成钉（M5-B6.2b）**：`fillet` **走 `GeometryService::execute`**（含 `edge_selectors`
    /// JSON）→ 几何真变——**证明 dispatch + 选择器 + 门面三段真接通**。
    ///
    /// 独立 oracle：10³ 盒在一条长 `L=10` 的棱倒 `r` 圆 → 去除体积 `(1−π/4)·r²·L`；
    /// 面数 >6；落盘 BREP ≠ 倒圆前的盒。**2 组参数**（边 0/r=1、边 1/r=2）。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn fillet_uses_the_selected_edge_end_to_end() {
        let mut g = real_service_or_panic("fillet 集成钉");

        for (edge_idx, r) in [(0u32, 1.0f64), (1, 2.0)] {
            let recipe = OpOutput::of(vec![
                a_cube(),
                core_sdk::BuildStep::new(BuildOp::Fillet)
                    .with_f64("radius", r)
                    .with_text(
                        "edge_selectors",
                        edge_json(&[EdgeSelector::ByIndex(edge_idx)]),
                    )
                    .from_steps(vec![0]),
            ]);

            let executed = g.execute(&recipe).expect("fillet recipe 应执行成功");
            assert_eq!(executed.step_count(), 2);
            let box_h = executed.results[0];
            let out = executed.final_handle().unwrap();

            // 倒圆前的盒（同一次 execute 的第 0 步结果）——用作独立基线。
            let v0 = g.kernel_mut().volume(box_h).unwrap();
            assert!(
                (v0 - 1000.0).abs() < 1e-6,
                "第 0 步应是 10³ 盒（体积 1000），实得 {v0}"
            );
            let brep0 = g.kernel_mut().export_brep(box_h).unwrap();

            let v = g.kernel_mut().volume(out).unwrap();
            let removed = (1.0 - std::f64::consts::FRAC_PI_4) * r * r * 10.0;
            assert!(v < v0, "倒圆应减小体积（边{edge_idx} r={r}）");
            assert!(
                (v0 - v - removed).abs() < 0.01,
                "倒圆去除体积应≈(1−π/4)r²L={removed:.4}，实得 {:.4}（边{edge_idx} r={r}）",
                v0 - v
            );
            assert!(
                g.kernel_mut()
                    .sub_shape_count(out, SubShapeKind::Face)
                    .unwrap()
                    > 6,
                "倒圆后应新增面（>6）（边{edge_idx} r={r}）"
            );
            assert_ne!(
                g.kernel_mut().export_brep(out).unwrap(),
                brep0,
                "倒圆后落盘 BREP 文本必须变化（边{edge_idx} r={r}）"
            );
        }
    }

    /// **occt 钉（execute 级）**：`chamfer` 经真实 `execute` 真切角。
    ///
    /// 独立 oracle：去除体积 `0.5·d²·L`；面数 >6；BREP 变。**2 组参数**。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn chamfer_executes_through_recipe() {
        let mut g = real_service_or_panic("chamfer occt 钉");
        for (edge_idx, d) in [(0u32, 1.0f64), (1, 2.0)] {
            let recipe = OpOutput::of(vec![
                a_cube(),
                core_sdk::BuildStep::new(BuildOp::Chamfer)
                    .with_f64("distance", d)
                    .with_text(
                        "edge_selectors",
                        edge_json(&[EdgeSelector::ByIndex(edge_idx)]),
                    )
                    .from_steps(vec![0]),
            ]);
            let executed = g.execute(&recipe).expect("chamfer recipe 应执行成功");
            let box_h = executed.results[0];
            let out = executed.final_handle().unwrap();
            let brep0 = g.kernel_mut().export_brep(box_h).unwrap();
            let v = g.kernel_mut().volume(out).unwrap();
            let removed = 0.5 * d * d * 10.0;
            assert!(v < 1000.0, "倒角应减小体积（边{edge_idx} d={d}）");
            assert!(
                (1000.0 - v - removed).abs() < 0.01,
                "倒角去除体积应≈½d²L={removed:.4}，实得 {:.4}（边{edge_idx} d={d}）",
                1000.0 - v
            );
            assert!(
                g.kernel_mut()
                    .sub_shape_count(out, SubShapeKind::Face)
                    .unwrap()
                    > 6,
                "倒角后应新增面（>6）（边{edge_idx} d={d}）"
            );
            assert_ne!(
                g.kernel_mut().export_brep(out).unwrap(),
                brep0,
                "倒角后 BREP 必须变化（边{edge_idx} d={d}）"
            );
        }
    }

    /// **occt 钉（execute 级）**：`shell` 经真实 `execute` 真挖空。
    ///
    /// 独立 oracle：开一个 10×10 面、壁厚 `t` → 体积 `1000−(10−2t)²(10−t)`；恰 1 实体；BREP 变。
    /// **2 组参数**（t=1、2）。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn shell_executes_through_recipe() {
        let mut g = real_service_or_panic("shell occt 钉");
        for t in [1.0f64, 2.0] {
            let recipe = OpOutput::of(vec![
                a_cube(),
                core_sdk::BuildStep::new(BuildOp::Shell)
                    .with_f64("thickness", t)
                    .with_text("face_selectors", face_json(&[FaceSelector::ByIndex(0)]))
                    .from_steps(vec![0]),
            ]);
            let executed = g.execute(&recipe).expect("shell recipe 应执行成功");
            let box_h = executed.results[0];
            let out = executed.final_handle().unwrap();
            let brep0 = g.kernel_mut().export_brep(box_h).unwrap();
            let v = g.kernel_mut().volume(out).unwrap();
            let inner = (10.0 - 2.0 * t) * (10.0 - 2.0 * t) * (10.0 - t);
            let expected = 1000.0 - inner;
            assert!(
                (v - expected).abs() < 1.0,
                "抽壳体积应≈1000−(10−2t)²(10−t)={expected:.2}，实得 {v:.4}（t={t}）"
            );
            assert_eq!(
                g.kernel_mut()
                    .sub_shape_count(out, SubShapeKind::Solid)
                    .unwrap(),
                1,
                "抽壳应产出恰 1 个实体（t={t}）"
            );
            assert_ne!(
                g.kernel_mut().export_brep(out).unwrap(),
                brep0,
                "抽壳后 BREP 必须变化（t={t}）"
            );
        }
    }

    /// **occt 钉（execute 级）**：`offset` 经真实 `execute` 真外扩。
    ///
    /// 独立 oracle：包围盒各轴跨度 ≈ `10+2d`；体积增大；BREP 变。**2 组参数**（d=1、2）。
    /// （本钉顺带裁定 `Offset` 是否需 `single_solid`——见实现报告。）
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn offset_executes_through_recipe() {
        let mut g = real_service_or_panic("offset occt 钉");
        for d in [1.0f64, 2.0] {
            let recipe = OpOutput::of(vec![
                a_cube(),
                core_sdk::BuildStep::new(BuildOp::Offset)
                    .with_f64("distance", d)
                    .from_steps(vec![0]),
            ]);
            let executed = g.execute(&recipe).expect("offset recipe 应执行成功");
            let box_h = executed.results[0];
            let out = executed.final_handle().unwrap();
            let brep0 = g.kernel_mut().export_brep(box_h).unwrap();
            let bb = g.kernel_mut().bounding_box(out).unwrap();
            let span = [
                bb.max[0] - bb.min[0],
                bb.max[1] - bb.min[1],
                bb.max[2] - bb.min[2],
            ];
            let want = 10.0 + 2.0 * d;
            for (i, s) in span.iter().enumerate() {
                assert!(
                    (s - want).abs() < 1e-6,
                    "外扩 d={d} 后第{i}轴包围盒跨度应≈{want}，实得 {s}"
                );
            }
            assert!(
                g.kernel_mut().volume(out).unwrap() > 1000.0,
                "外扩应增大体积（d={d}）"
            );
            assert_ne!(
                g.kernel_mut().export_brep(out).unwrap(),
                brep0,
                "外扩后 BREP 必须变化（d={d}）"
            );
        }
    }

    /// **occt 钉（execute_step 级）**：`thicken` 经 dispatch 真把面加厚成实体。
    ///
    /// ⚠ 为何走 `execute_step` 而非 `execute`：`thicken` 的输入是**面 / 壳**，而当前
    /// **没有任何 `BuildOp` 能产出面 / 壳**（配方的 `inputs` 只能引用前序步骤结果）——
    /// 故在 `core-app` 内无法把面装进 recipe 的 `inputs`。本钉用 `execute_step` 直喂
    /// 面句柄，**仍走真实 dispatch 臂**（`single_solid` 之外的 `kernel.thicken` 路径）。
    ///
    /// 独立 oracle：10×10 面加厚 `t` → 恰 1 实体、体积 ≈ 100·t；BREP 变。**2 组参数**。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn thicken_executes_through_dispatch() {
        let mut g = real_service_or_panic("thicken occt 钉");
        for t in [1.0f64, 2.0] {
            let bx = g.kernel_mut().make_box(10.0, 10.0, 10.0).unwrap();
            let faces = g.kernel_mut().sub_shapes(bx, SubShapeKind::Face).unwrap();
            let f = faces[0];
            let brep_face = g.kernel_mut().export_brep(f).unwrap();

            let step = core_sdk::BuildStep::new(BuildOp::Thicken)
                .with_f64("thickness", t)
                .from_steps(vec![0]);
            let mut notices = Vec::new();
            let (out, evo) = g
                .execute_step(1, &step, &[f], &mut notices)
                .expect("thicken dispatch 应成功");
            assert!(evo.is_none(), "8 算子不产出 Evolution");

            let v = g.kernel_mut().volume(out).unwrap();
            assert!(
                (v - 100.0 * t).abs() < 1.0,
                "加厚体积应≈面积·t=100·t={:.2}，实得 {v:.4}（t={t}）",
                100.0 * t
            );
            assert_eq!(
                g.kernel_mut()
                    .sub_shape_count(out, SubShapeKind::Solid)
                    .unwrap(),
                1,
                "加厚应产出恰 1 个实体（t={t}）"
            );
            assert_ne!(
                g.kernel_mut().export_brep(out).unwrap(),
                brep_face,
                "加厚后 BREP 必须变化（t={t}）"
            );
            g.kernel_mut().release(out).ok();
        }
    }

    /// **F1 钉（occt）**：成功 `fillet` 经**真实 `execute`** 后 `live_count()` 回到基线+2——
    /// 证明 `ByIndex` 解析时**未选中的 11 条边句柄已被释放**（防 "success 后 live_count=13" 泄漏）。
    ///
    /// 现有 mock 路径钉（钉 11）只走失败路径，**永不触发**成功路径泄漏；本钉补上。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn successful_fillet_execute_returns_live_count_to_baseline() {
        let mut g = real_service_or_panic("fillet live_count 钉");
        let base = g.kernel().live_count();
        let recipe = OpOutput::of(vec![
            a_cube(),
            core_sdk::BuildStep::new(BuildOp::Fillet)
                .with_f64("radius", 1.0)
                .with_text("edge_selectors", edge_json(&[EdgeSelector::ByIndex(0)]))
                .from_steps(vec![0]),
        ]);
        let executed = g.execute(&recipe).expect("fillet recipe 应成功");
        assert_eq!(executed.step_count(), 2);
        assert_eq!(
            g.kernel().live_count(),
            base + 2,
            "成功 fillet 后应只剩配方 2 个结果句柄（选择器未选中的边句柄必须已释放，无泄漏）"
        );
    }

    /// **F1 钉（occt）**：成功 `shell` 经**真实 `execute`** 后 `live_count()` 回到基线+2——
    /// 面选择器 `ByIndex` 同样不得泄漏未选中的面句柄。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn successful_shell_execute_returns_live_count_to_baseline() {
        let mut g = real_service_or_panic("shell live_count 钉");
        let base = g.kernel().live_count();
        let recipe = OpOutput::of(vec![
            a_cube(),
            core_sdk::BuildStep::new(BuildOp::Shell)
                .with_f64("thickness", 1.0)
                .with_text("face_selectors", face_json(&[FaceSelector::ByIndex(0)]))
                .from_steps(vec![0]),
        ]);
        let executed = g.execute(&recipe).expect("shell recipe 应成功");
        assert_eq!(executed.step_count(), 2);
        assert_eq!(
            g.kernel().live_count(),
            base + 2,
            "成功 shell 后应只剩配方 2 个结果句柄（选择器未选中的面句柄必须已释放，无泄漏）"
        );
    }

    /// 从 `shape` 枚举 wire，返回**首个**满足 `pred(min, max)` 者（`min`/`max` 为 3 分量包围盒角点）。
    ///
    /// 用**几何谓词**（而非 `sub_shapes` 序号）挑 wire——序号可能随后端版本漂移，几何谓词稳。
    #[cfg(feature = "occt")]
    fn wire_where(
        g: &mut GeometryService,
        shape: ShapeHandle,
        pred: impl Fn([f64; 3], [f64; 3]) -> bool,
    ) -> ShapeHandle {
        let ws = g
            .kernel_mut()
            .sub_shapes(shape, SubShapeKind::Wire)
            .expect("枚举 wire 应成功");
        let mut chosen = None;
        for w in &ws {
            let bb = g.kernel_mut().bounding_box(*w).expect("wire 包围盒应成功");
            if pred(bb.min, bb.max) {
                chosen = Some(*w);
                break;
            }
        }
        match chosen {
            Some(h) => {
                // `sub_shapes` 分配的会话句柄：未选中的一律释放（与生产期选择器同口径）。
                let rest: Vec<ShapeHandle> = ws.iter().copied().filter(|x| *x != h).collect();
                g.kernel_mut().release_many(&rest);
                h
            }
            None => {
                g.kernel_mut().release_many(&ws);
                panic!("未找到满足几何谓词的 wire");
            }
        }
    }

    /// 经 `execute_step` 派发 `Loft`（`inputs` 顺序即放样顺序）。
    #[cfg(feature = "occt")]
    fn loft_via_dispatch(
        g: &mut GeometryService,
        wires: &[ShapeHandle],
        is_solid: bool,
        ruled: bool,
    ) -> ShapeHandle {
        let step = core_sdk::BuildStep::new(BuildOp::Loft)
            .with_bool("is_solid", is_solid)
            .with_bool("ruled", ruled)
            .from_steps((0..wires.len()).collect());
        let mut notices = Vec::new();
        let (out, evo) = g
            .execute_step(1, &step, wires, &mut notices)
            .expect("loft dispatch 应成功");
        assert!(evo.is_none(), "8 算子不产出 Evolution");
        out
    }

    /// **F2 钉（occt）**：`loft` 经 `execute_step` 派发——dispatch 结果与**直调内核**几何一致
    /// （体积 + 包围盒 + 实体数），且 **≥2 组参数**（h=10 / h=20）的几何与 BREP **真变**。
    ///
    /// ⚠ 不比对 BREP 文本相等：两次独立 `loft` 调用的 `TopoDS` 子形状共享方式不同，BREP 序列化
    /// （`Surfaces` 段）会不同——故 dispatch↔直调用**几何量**比对，BREP 只用于**跨参数组**判"真变"。
    ///
    /// 打消"派发臂不可达 / 参数被丢弃"变异：`is_solid=true` 须产 1 实体、体积 = 10·10·h。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn loft_executes_through_dispatch() {
        let mut g = real_service_or_panic("loft occt 钉");
        let mut breps: Vec<String> = Vec::new();
        for (h, want_vol) in [(10.0f64, 1000.0f64), (20.0, 2000.0)] {
            let bx = g.kernel_mut().make_box(10.0, 10.0, h).unwrap();
            let z0 = wire_where(&mut g, bx, |min, max| {
                min[2].abs() < 1e-9 && max[2].abs() < 1e-9
            });
            let zh = wire_where(&mut g, bx, |min, max| {
                (min[2] - h).abs() < 1e-9 && (max[2] - h).abs() < 1e-9
            });

            let oracle = g.kernel_mut().loft(&[z0, zh], true, false).unwrap();
            let vo = g.kernel_mut().volume(oracle).unwrap();
            let bb_o = g.kernel_mut().bounding_box(oracle).unwrap();
            assert!(
                (vo - want_vol).abs() < 1e-3,
                "oracle loft h={h} 体积应≈{want_vol}，实得 {vo}"
            );
            assert!(
                !g.kernel_mut().export_brep(oracle).unwrap().is_empty(),
                "loft h={h} BREP 不应为空"
            );

            let out = loft_via_dispatch(&mut g, &[z0, zh], true, false);
            let v = g.kernel_mut().volume(out).unwrap();
            assert!(
                (v - want_vol).abs() < 1e-3,
                "dispatch loft h={h} 体积应≈{want_vol}，实得 {v}"
            );
            assert_eq!(
                g.kernel_mut()
                    .sub_shape_count(out, SubShapeKind::Solid)
                    .unwrap(),
                1,
                "is_solid=true 应产出恰 1 实体（h={h}）"
            );
            let bb = g.kernel_mut().bounding_box(out).unwrap();
            assert!(
                bb.min == bb_o.min && bb.max == bb_o.max,
                "dispatch loft 几何须与直调内核一致（h={h}）：dispatch {:?} vs oracle {:?}",
                (bb.min, bb.max),
                (bb_o.min, bb_o.max)
            );
            breps.push(g.kernel_mut().export_brep(out).unwrap());
            g.kernel_mut().release(out).ok();
            g.kernel_mut().release(oracle).ok();
        }
        assert_ne!(
            breps[0], breps[1],
            "两组参数（h=10 / h=20）的 BREP 必须不同（几何真变）"
        );
    }

    /// **F2 钉（occt）**：`sweep` 经 `execute_step` 派发——**profile/spine 顺序恒为 `[input(0), input(1)]`**。
    ///
    /// 打消 **F3⑥**（输入顺序互换）：同一对 wire **两个顺序**各作一组参数，其直调几何
    /// **显著不同**（体积 12077 vs 63565）；dispatch 在 `[profile, spine]` / `[spine, profile]`
    /// 两输入下须分别复现**各自**直调结果（若派发把两参数对调，必红）。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn sweep_executes_through_dispatch() {
        let mut g = real_service_or_panic("sweep occt 钉");
        let bx = g.kernel_mut().make_box(30.0, 10.0, 10.0).unwrap();
        // profile：x=0 面上的方形 wire；spine：y=10 面上的长方形 wire。
        let p = wire_where(&mut g, bx, |_, max| max[0].abs() < 1e-9);
        let s = wire_where(&mut g, bx, |min, max| {
            (min[1] - 10.0).abs() < 1e-9 && (max[0] - 30.0).abs() < 1e-9
        });

        // 直调内核取两组 oracle（顺序互换 → 几何显著不同）。
        let o_ps = g.kernel_mut().sweep(p, s, 1).unwrap();
        let o_sp = g.kernel_mut().sweep(s, p, 1).unwrap();
        let v_ps = g.kernel_mut().volume(o_ps).unwrap();
        let v_sp = g.kernel_mut().volume(o_sp).unwrap();
        let bb_ps = g.kernel_mut().bounding_box(o_ps).unwrap();
        let bb_sp = g.kernel_mut().bounding_box(o_sp).unwrap();
        assert!(
            v_ps > 1.0 && v_sp > 1.0,
            "两组 sweep 都应是实体（v_ps={v_ps}, v_sp={v_sp}）"
        );
        assert!(
            (v_ps - v_sp).abs() > 1.0,
            "profile/spine 顺序互换必须改变几何（v_ps={v_ps}, v_sp={v_sp}）"
        );
        g.kernel_mut().release(o_ps).ok();
        g.kernel_mut().release(o_sp).ok();

        let mut outs: Vec<String> = Vec::new();
        for (a, b_, want_v, want_bb, tag) in [
            (p, s, v_ps, (bb_ps.min, bb_ps.max), "profile,spine"),
            (s, p, v_sp, (bb_sp.min, bb_sp.max), "spine,profile"),
        ] {
            let step = core_sdk::BuildStep::new(BuildOp::Sweep)
                .with_i64("transition_mode", 1)
                .from_steps(vec![0, 1]);
            let mut notices = Vec::new();
            let (out, _) = g
                .execute_step(1, &step, &[a, b_], &mut notices)
                .expect("sweep dispatch 应成功");
            let v = g.kernel_mut().volume(out).unwrap();
            assert!(
                (v - want_v).abs() < 0.01,
                "sweep[{tag}] dispatch 体积应≈{want_v}，实得 {v}（顺序不得互换 / `transition_mode` 不得丢弃）"
            );
            let bb = g.kernel_mut().bounding_box(out).unwrap();
            assert!(
                bb.min == want_bb.0 && bb.max == want_bb.1,
                "sweep[{tag}] dispatch 包围盒须与直调内核一致"
            );
            outs.push(g.kernel_mut().export_brep(out).unwrap());
            g.kernel_mut().release(out).ok();
        }
        assert_ne!(
            outs[0], outs[1],
            "profile/spine 顺序互换必须产出**不同 BREP**"
        );
    }

    /// **F2 钉（occt）**：`pipe` 经 `execute_step` 派发——**≥2 组参数**几何真变 + BREP 真变，
    /// 且 **profile/spine 顺序不得互换**（F3⑥）。
    ///
    /// ⚠ `pipe` 对盒面 wire 产**开口壳**（体积≈0），故几何真变以**包围盒**判定。
    /// 顺序互换用**互换敏感的包围盒 oracle**（`(w0,w1)` 与 `(w1,w0)` 的 bb 显著不同）——
    /// **不依赖** dispatch 与直调的 BREP 文本相等（两次独立调用子形状共享方式不同，序列化会漂）。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn pipe_executes_through_dispatch() {
        let mut g = real_service_or_panic("pipe occt 钉");
        let bx = g.kernel_mut().make_box(30.0, 10.0, 10.0).unwrap();
        let w0 = wire_where(&mut g, bx, |_, max| max[0].abs() < 1e-9); // x=0 方形
        let w1 = wire_where(&mut g, bx, |min, _| (min[0] - 30.0).abs() < 1e-9); // x=30 方形
        let w2 = wire_where(&mut g, bx, |_, max| max[1].abs() < 1e-9); // y=0 长方形
        let w4 = wire_where(&mut g, bx, |_, max| max[2].abs() < 1e-9); // z=0 长方形

        // 经 dispatch 调 `Pipe`（`inputs` 顺序恒 `[profile, spine]`）。
        let pipe_dispatch = |g: &mut GeometryService, a: ShapeHandle, b_: ShapeHandle| {
            let step = core_sdk::BuildStep::new(BuildOp::Pipe).from_steps(vec![0, 1]);
            let mut notices = Vec::new();
            g.execute_step(1, &step, &[a, b_], &mut notices)
                .expect("pipe dispatch 应成功")
                .0
        };
        let bb_of = |g: &mut GeometryService, h: ShapeHandle| {
            let bb = g.kernel_mut().bounding_box(h).unwrap();
            (bb.min, bb.max)
        };
        let bb_eq = |a: ([f64; 3], [f64; 3]), b: ([f64; 3], [f64; 3])| {
            a.0.iter().zip(b.0).all(|(x, y)| (x - y).abs() < 1e-6)
                && a.1.iter().zip(b.1).all(|(x, y)| (x - y).abs() < 1e-6)
        };

        // ---- ≥2 组参数：包围盒 & BREP 真变 ----
        let o_a = pipe_dispatch(&mut g, w0, w2);
        let o_b = pipe_dispatch(&mut g, w2, w4);
        let bb_a = bb_of(&mut g, o_a);
        let bb_b = bb_of(&mut g, o_b);
        let brep_a = g.kernel_mut().export_brep(o_a).unwrap();
        let brep_b = g.kernel_mut().export_brep(o_b).unwrap();
        assert!(
            !brep_a.is_empty() && !brep_b.is_empty(),
            "pipe BREP 不应为空"
        );
        assert!(
            !bb_eq(bb_a, bb_b),
            "两组 pipe 参数的包围盒必须不同（几何真变）：{bb_a:?} vs {bb_b:?}"
        );
        assert_ne!(brep_a, brep_b, "两组 pipe 参数的 BREP 必须不同");
        for h in [o_a, o_b] {
            g.kernel_mut().release(h).ok();
        }

        // ---- F3⑥：profile/spine 顺序互换 → 包围盒真变（**互换敏感**，不依赖 BREP 文本相等）----
        // 实测：pipe(w0,w1) bb=[0,0,-10]/[0,20,10]；pipe(w1,w0) bb=[30,0,0]/[30,20,20]。
        let o_ps = pipe_dispatch(&mut g, w0, w1);
        let o_sp = pipe_dispatch(&mut g, w1, w0);
        let bb_ps = bb_of(&mut g, o_ps);
        let bb_sp = bb_of(&mut g, o_sp);
        assert!(
            bb_eq(bb_ps, ([0.0, 0.0, -10.0], [0.0, 20.0, 10.0])),
            "pipe([profile,spine]) 包围盒应为 [0,0,-10]/[0,20,10]，实得 {bb_ps:?}（顺序不得互换）"
        );
        assert!(
            bb_eq(bb_sp, ([30.0, 0.0, 0.0], [30.0, 20.0, 20.0])),
            "pipe([spine,profile]) 包围盒应为 [30,0,0]/[30,20,20]，实得 {bb_sp:?}（顺序不得互换）"
        );
        assert!(
            !bb_eq(bb_ps, bb_sp),
            "pipe 顺序互换必须改变几何（包围盒）：{bb_ps:?} vs {bb_sp:?}"
        );
        assert_ne!(
            g.kernel_mut().export_brep(o_ps).unwrap(),
            g.kernel_mut().export_brep(o_sp).unwrap(),
            "pipe 顺序互换必须产出**不同 BREP**"
        );
        for h in [o_ps, o_sp] {
            g.kernel_mut().release(h).ok();
        }
    }

    /// **F3⑤ 钉（occt）**：`loft` 的 `is_solid` 与 `ruled` **旗标真被消费**（不得写死）。
    ///
    /// - `is_solid`：真 → 恰 1 实体；假 → 0 实体（开口壳）；且实体体积 **>** 壳体积。
    /// - `ruled`：同输入下 `true` / `false` 的曲面类型不同 → **BREP 必须不同**（体积可相同，
    ///   故只断言 BREP 差异——这正是"旗标写死"变异的可观测信号）。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn loft_flags_is_solid_and_ruled_are_honored() {
        let mut g = real_service_or_panic("loft 旗标钉");
        // 平行面、**不同尺寸**的两个方形 wire（10×10 @ z=10 与 20×20 @ z=0）——`ruled` 才能区分。
        let b1 = g.kernel_mut().make_box(10.0, 10.0, 10.0).unwrap();
        let b2 = g.kernel_mut().make_box(20.0, 20.0, 10.0).unwrap();
        let t1 = wire_where(&mut g, b1, |min, max| {
            (min[2] - 10.0).abs() < 1e-9 && (max[2] - 10.0).abs() < 1e-9
        });
        let btm2 = wire_where(&mut g, b2, |min, max| {
            min[2].abs() < 1e-9 && max[2].abs() < 1e-9
        });
        let bb1 = g.kernel_mut().bounding_box(t1).unwrap();
        let bb2 = g.kernel_mut().bounding_box(btm2).unwrap();
        assert!(
            bb1.max[0] < 11.0 && bb1.max[1] < 11.0,
            "t1 应为 10×10 方形，实得 {:?}",
            (bb1.min, bb1.max)
        );
        assert!(
            bb2.max[0] > 19.0 && bb2.max[1] > 19.0,
            "btm2 应为 20×20 方形，实得 {:?}",
            (bb2.min, bb2.max)
        );

        let solid = loft_via_dispatch(&mut g, &[t1, btm2], true, false);
        let shell = loft_via_dispatch(&mut g, &[t1, btm2], false, false);
        let ruled = loft_via_dispatch(&mut g, &[t1, btm2], true, true);

        assert_eq!(
            g.kernel_mut()
                .sub_shape_count(solid, SubShapeKind::Solid)
                .unwrap(),
            1,
            "is_solid=true → 恰 1 实体"
        );
        assert_eq!(
            g.kernel_mut()
                .sub_shape_count(shell, SubShapeKind::Solid)
                .unwrap(),
            0,
            "is_solid=false → 无实体（开口壳）"
        );
        let v_solid = g.kernel_mut().volume(solid).unwrap();
        let v_shell = g.kernel_mut().volume(shell).unwrap();
        // 开口壳的 `volume` 为**带符号**量（实测 -1500），故比较**绝对值**。
        assert!(
            v_solid.abs() > v_shell.abs() && v_shell.abs() > 0.0,
            "实体放样体积应大于开口壳体积绝对值（v_solid={v_solid}, v_shell={v_shell}）"
        );

        let b_solid = g.kernel_mut().export_brep(solid).unwrap();
        let b_ruled = g.kernel_mut().export_brep(ruled).unwrap();
        assert!(!b_solid.is_empty() && !b_ruled.is_empty());
        assert_ne!(
            b_solid, b_ruled,
            "ruled=true 与 ruled=false 的 BREP 必须不同（旗标不得写死）"
        );

        for h in [solid, shell, ruled] {
            g.kernel_mut().release(h).ok();
        }
    }

    // =========================================================================
    // M7a：线框生产 execute 级全链钉（occt 腿）
    // =========================================================================
    //
    // 此前 `Loft` / `Sweep` / `Pipe` / `Thicken` 的操作数（截面 wire / 面输入）在
    // Recipe 世界**造不出来**——输入只能来自既有形状。M7a 起 `MakeLineEdge` /
    // `MakeArcEdge` / `MakeWire` / `MakeFace` 使「从零造操作数」成为配方可表达的
    // 操作，以下钉子证明相关算子在**真实 execute 路径**上端到端走通。

    /// 一条 M7a 三点弧步骤（起点 / 中间点 / 终点，mm）。
    #[cfg(feature = "occt")]
    #[allow(clippy::too_many_arguments)]
    fn arc_step(
        x1: f64,
        y1: f64,
        z1: f64,
        x2: f64,
        y2: f64,
        z2: f64,
        x3: f64,
        y3: f64,
        z3: f64,
    ) -> core_sdk::BuildStep {
        core_sdk::BuildStep::new(BuildOp::MakeArcEdge)
            .with_f64("x1", x1)
            .with_f64("y1", y1)
            .with_f64("z1", z1)
            .with_f64("x2", x2)
            .with_f64("y2", y2)
            .with_f64("z2", z2)
            .with_f64("x3", x3)
            .with_f64("y3", y3)
            .with_f64("z3", z3)
    }

    /// **M7a execute 级钉：`Sweep` 用配方造的 wire 端到端走通**。
    ///
    /// 配方：YZ 平面 10×10 矩形 4 直边 → MakeWire(profile) → 沿 X 直边 →
    /// MakeWire(spine) → Sweep，**走真实 `execute`** 成功；`sub_shape_count(Solid)==1`
    /// + 独立 oracle：体积 ≈ 10×10×100 = 10000 + 落盘 BREP ≠ spine/profile 文本。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn sweep_executes_end_to_end_with_real_wire() {
        let mut g = real_service_or_panic("sweep execute 级钉");

        // profile：YZ 平面 10×10 矩形（x=0，与脊线垂直）；spine：沿 +X 长 100
        let recipe = OpOutput::of(vec![
            line_step(0.0, 0.0, 0.0, 0.0, 10.0, 0.0),   // 0
            line_step(0.0, 10.0, 0.0, 0.0, 10.0, 10.0), // 1
            line_step(0.0, 10.0, 10.0, 0.0, 0.0, 10.0), // 2
            line_step(0.0, 0.0, 10.0, 0.0, 0.0, 0.0),   // 3
            core_sdk::BuildStep::new(BuildOp::MakeWire).from_steps(vec![0, 1, 2, 3]), // 4 profile
            line_step(0.0, 0.0, 0.0, 100.0, 0.0, 0.0),  // 5
            core_sdk::BuildStep::new(BuildOp::MakeWire).from_steps(vec![5]), // 6 spine
            core_sdk::BuildStep::new(BuildOp::Sweep).from_steps(vec![4, 6]), // 7
        ]);
        assert!(recipe.is_well_ordered(), "sweep recipe 应结构合法");
        let executed = g.execute(&recipe).expect("sweep recipe 应执行成功");
        assert_eq!(executed.step_count(), 8);
        let profile_h = executed.results[4];
        let spine_h = executed.results[6];
        let out = executed.final_handle().unwrap();

        // 落盘断言：产物 BREP 与 profile / spine 文本均不同
        let profile_brep = g.kernel_mut().export_brep(profile_h).unwrap();
        let spine_brep = g.kernel_mut().export_brep(spine_h).unwrap();
        let out_brep = g.kernel_mut().export_brep(out).unwrap();
        assert!(!out_brep.is_empty(), "扫掠产物 BREP 不应为空");
        assert_ne!(out_brep, profile_brep, "扫掠产物必须区别于 profile");
        assert_ne!(out_brep, spine_brep, "扫掠产物必须区别于 spine");

        // 独立 oracle：10×10 截面沿 X 长 100 → 体积 ≈ 10000、包围盒沿脊线延展
        assert_eq!(
            g.kernel_mut()
                .sub_shape_count(out, SubShapeKind::Solid)
                .unwrap(),
            1,
            "扫掠产物应是实体"
        );
        let v = g.kernel_mut().volume(out).unwrap();
        assert!(
            (v - 10_000.0).abs() < 1.0,
            "扫掠体积应≈10×10×100=10000，实得 {v:.4}"
        );
        let bb = g.kernel_mut().bounding_box(out).unwrap();
        assert!(
            (bb.max[0] - 100.0).abs() < 1e-6,
            "包围盒应沿脊线(+X)延展到 100，实得 {:?}",
            (bb.min, bb.max)
        );
        assert!(
            (bb.max[1] - 10.0).abs() < 1e-6 && (bb.max[2] - 10.0).abs() < 1e-6,
            "截面跨度应为 10×10，实得 {:?}",
            (bb.min, bb.max)
        );
    }

    /// **M7a execute 级钉：`Pipe` 用配方造的圆 wire 端到端走通**。
    ///
    /// 配方：两段半圆三点弧拼成 r=10 圆（YZ 平面）→ MakeWire(profile) →
    /// 沿 X 直边 → MakeWire(spine) → Pipe，走真实 `execute` 成功。
    ///
    /// ⚠ **实测适配（B6.2b F2 钉已证）**：`pipe` 对 wire profile 产**开口壳**
    /// （无端盖，`sub_shape_count(Solid)==0`、体积远小于实心棒 π r² L），故
    /// oracle 用**包围盒**（直径 20 × 脊长 100）+ 体积 `|v| < π r² L`（壳语义），
    /// **不宣称**「体积 ≈ π r² L」（规格该条与实测不符，见偏差清单）。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn pipe_executes_end_to_end_with_real_wire() {
        let mut g = real_service_or_panic("pipe execute 级钉");

        // r=10 圆：上半圆 (0,10,0)→(0,0,10)→(0,-10,0)，下半圆 (0,-10,0)→(0,0,-10)→(0,10,0)
        let recipe = OpOutput::of(vec![
            arc_step(0.0, 10.0, 0.0, 0.0, 0.0, 10.0, 0.0, -10.0, 0.0), // 0 上半圆
            arc_step(0.0, -10.0, 0.0, 0.0, 0.0, -10.0, 0.0, 10.0, 0.0), // 1 下半圆
            core_sdk::BuildStep::new(BuildOp::MakeWire).from_steps(vec![0, 1]), // 2 圆 profile
            line_step(0.0, 0.0, 0.0, 100.0, 0.0, 0.0),                 // 3 spine
            core_sdk::BuildStep::new(BuildOp::MakeWire).from_steps(vec![3]), // 4 spine wire
            core_sdk::BuildStep::new(BuildOp::Pipe).from_steps(vec![2, 4]), // 5
        ]);
        assert!(recipe.is_well_ordered(), "pipe recipe 应结构合法");
        let executed = g.execute(&recipe).expect("pipe recipe 应执行成功");
        assert_eq!(executed.step_count(), 6);
        let profile_h = executed.results[2];
        let spine_h = executed.results[4];
        let out = executed.final_handle().unwrap();

        // M7b P3 补强：**种类计数**断言（实测口径，探针已证：Solid=0、Shell=1、
        // Face=2、Wire=2）。开口壳语义：无端盖 → `Solid==0`、`Shell>=1`——
        // 与体积断言互补，且**不依赖体积**（体积口径已在下方 oracle 覆盖）。
        assert_eq!(
            g.kernel_mut()
                .sub_shape_count(out, SubShapeKind::Solid)
                .unwrap(),
            0,
            "pipe 对 wire profile 产开口壳（无端盖），sub_shape_count(Solid) 必须为 0"
        );
        assert!(
            g.kernel_mut()
                .sub_shape_count(out, SubShapeKind::Shell)
                .unwrap()
                >= 1,
            "pipe 产物必须含开口壳（sub_shape_count(Shell)>=1）"
        );

        // 落盘断言：产物 BREP 与 profile / spine 文本均不同
        let profile_brep = g.kernel_mut().export_brep(profile_h).unwrap();
        let spine_brep = g.kernel_mut().export_brep(spine_h).unwrap();
        let out_brep = g.kernel_mut().export_brep(out).unwrap();
        assert!(!out_brep.is_empty(), "管道产物 BREP 不应为空");
        assert_ne!(out_brep, profile_brep, "管道产物必须区别于 profile");
        assert_ne!(out_brep, spine_brep, "管道产物必须区别于 spine");

        // 独立 oracle：r=10 圆沿 X 长 100 → 包围盒直径 20 × 脊长 100；
        // 开口壳体积 |v| 远小于实心棒 π r² L ≈ 31416
        let v = g.kernel_mut().volume(out).unwrap();
        assert!(
            v.abs() < std::f64::consts::PI * 100.0 * 100.0,
            "开口壳体积应远小于实心棒 π r² L≈31416，实得 {v:.4}"
        );
        let bb = g.kernel_mut().bounding_box(out).unwrap();
        assert!(
            (bb.max[0] - 100.0).abs() < 1e-6,
            "包围盒应沿脊线(+X)延展到 100，实得 {:?}",
            (bb.min, bb.max)
        );
        assert!(
            (bb.max[1] - 10.0).abs() < 1e-6 && (bb.max[2] - 10.0).abs() < 1e-6,
            "圆截面直径应为 20，实得 {:?}",
            (bb.min, bb.max)
        );
    }

    /// **M7a execute 级钉：`Loft` 用两条配方造的矩形 wire 端到端走通**。
    ///
    /// 两个同心、不同尺寸的矩形 wire（10×10 @ z=0、20×20 @ z=50，同起点逆时针）→
    /// Loft(is_solid=true, ruled=true) → **台体公式**独立 oracle：
    /// `V = h/3·(A1+A2+√(A1·A2)) = 50/3·(100+400+200) ≈ 11666.67`；BREP 变化。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn loft_executes_end_to_end_with_two_rect_wires() {
        let mut g = real_service_or_panic("loft execute 级钉");

        // 一个 9 步配方：8 直边 → 2 wire → Loft
        let recipe = OpOutput::of(vec![
            line_step(0.0, 0.0, 0.0, 10.0, 0.0, 0.0), // 0  10×10 @ z=0（逆时针）
            line_step(10.0, 0.0, 0.0, 10.0, 10.0, 0.0), // 1
            line_step(10.0, 10.0, 0.0, 0.0, 10.0, 0.0), // 2
            line_step(0.0, 10.0, 0.0, 0.0, 0.0, 0.0), // 3
            line_step(-5.0, -5.0, 50.0, 15.0, -5.0, 50.0), // 4  20×20 @ z=50（同起点逆时针）
            line_step(15.0, -5.0, 50.0, 15.0, 15.0, 50.0), // 5
            line_step(15.0, 15.0, 50.0, -5.0, 15.0, 50.0), // 6
            line_step(-5.0, 15.0, 50.0, -5.0, -5.0, 50.0), // 7
            core_sdk::BuildStep::new(BuildOp::MakeWire).from_steps(vec![0, 1, 2, 3]), // 8
            core_sdk::BuildStep::new(BuildOp::MakeWire).from_steps(vec![4, 5, 6, 7]), // 9
            core_sdk::BuildStep::new(BuildOp::Loft)
                .with_bool("is_solid", true)
                .with_bool("ruled", true)
                .from_steps(vec![8, 9]), // 10
        ]);
        assert!(recipe.is_well_ordered(), "loft recipe 应结构合法");
        let executed = g.execute(&recipe).expect("loft recipe 应执行成功");
        assert_eq!(executed.step_count(), 11);
        let w1 = executed.results[8];
        let w2 = executed.results[9];
        let out = executed.final_handle().unwrap();

        // 落盘断言：产物 BREP 与两个截面 wire 文本均不同
        assert_ne!(
            g.kernel_mut().export_brep(out).unwrap(),
            g.kernel_mut().export_brep(w1).unwrap(),
            "放样产物必须区别于截面 1"
        );
        assert_ne!(
            g.kernel_mut().export_brep(out).unwrap(),
            g.kernel_mut().export_brep(w2).unwrap(),
            "放样产物必须区别于截面 2"
        );

        // 独立 oracle：台体公式 V = h/3·(A1+A2+√(A1·A2))
        let want = 50.0 / 3.0 * (100.0 + 400.0 + 200.0);
        assert_eq!(
            g.kernel_mut()
                .sub_shape_count(out, SubShapeKind::Solid)
                .unwrap(),
            1,
            "is_solid=true → 恰 1 实体"
        );
        let v = g.kernel_mut().volume(out).unwrap();
        assert!(
            (v.abs() - want).abs() < 1.0,
            "台体体积应≈h/3·(A1+A2+√(A1·A2))={want:.2}，实得 {v:.4}（符号为朝向，不宣断）"
        );
        let bb = g.kernel_mut().bounding_box(out).unwrap();
        assert!(
            (bb.max[2] - 50.0).abs() < 1e-6,
            "放样包围盒 z 跨度应≈50，实得 {:?}",
            (bb.min, bb.max)
        );
    }

    /// **M7a execute 级钉：`MakeFace` → `Thicken` 全链首次走通**。
    ///
    /// 矩形 10×10 wire → MakeFace → Thicken(t=2) → 体积 ≈ 面积×t = 200
    /// （独立 oracle）。**Thicken 的面输入首次在 Recipe 世界造出来**——
    /// 此前该输入只能来自既有形状的子形状选择（派发臂内部临时路径）。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn make_face_then_thicken_executes_end_to_end() {
        let mut g = real_service_or_panic("thicken execute 级钉");

        let recipe = OpOutput::of(vec![
            line_step(0.0, 0.0, 0.0, 10.0, 0.0, 0.0),   // 0
            line_step(10.0, 0.0, 0.0, 10.0, 10.0, 0.0), // 1
            line_step(10.0, 10.0, 0.0, 0.0, 10.0, 0.0), // 2
            line_step(0.0, 10.0, 0.0, 0.0, 0.0, 0.0),   // 3
            core_sdk::BuildStep::new(BuildOp::MakeWire).from_steps(vec![0, 1, 2, 3]), // 4 wire
            core_sdk::BuildStep::new(BuildOp::MakeFace).from_steps(vec![4]), // 5 face
            core_sdk::BuildStep::new(BuildOp::Thicken)
                .with_f64("thickness", 2.0)
                .from_steps(vec![5]), // 6
        ]);
        assert!(recipe.is_well_ordered(), "thicken recipe 应结构合法");
        let executed = g.execute(&recipe).expect("thicken 全链配方应执行成功");
        assert_eq!(executed.step_count(), 7);
        let face_h = executed.results[5];
        let out = executed.final_handle().unwrap();

        // 面面积独立 oracle：10×10 = 100
        let area = g.kernel_mut().surface_area(face_h).unwrap();
        assert!(
            (area - 100.0).abs() < 1e-6,
            "MakeFace 面积应≈100，实得 {area:.4}"
        );

        // Thicken(t=2) → 体积 ≈ 面积×t = 200（独立 oracle；符号为朝向，不宣断）
        let v = g.kernel_mut().volume(out).unwrap();
        assert!(
            (v.abs() - 200.0).abs() < 1.0,
            "加厚体积应≈面积×t=200，实得 {v:.4}"
        );
        assert_eq!(
            g.kernel_mut()
                .sub_shape_count(out, SubShapeKind::Solid)
                .unwrap(),
            1,
            "加厚应产出恰 1 个实体"
        );
    }

    /// **M7a execute 级钉：`ExtractWire` 往返 BREP 相容**。
    ///
    /// ⚠ 本钉只验 **face 路径**（MakeWire → MakeFace → ExtractWire）：从 solid
    /// 提 face **不在本批**（face_selectors 是派发臂内部临时路径，不产出步骤结果；
    /// `ExtractFace` 属 M8 候选，见规格 §9 #4）。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn extract_wire_from_box_face_executes() {
        let mut g = real_service_or_panic("extract_wire execute 级钉");

        let recipe = OpOutput::of(vec![
            line_step(0.0, 0.0, 0.0, 10.0, 0.0, 0.0),   // 0
            line_step(10.0, 0.0, 0.0, 10.0, 10.0, 0.0), // 1
            line_step(10.0, 10.0, 0.0, 0.0, 10.0, 0.0), // 2
            line_step(0.0, 10.0, 0.0, 0.0, 0.0, 0.0),   // 3
            core_sdk::BuildStep::new(BuildOp::MakeWire).from_steps(vec![0, 1, 2, 3]), // 4 wire
            core_sdk::BuildStep::new(BuildOp::MakeFace).from_steps(vec![4]), // 5 face
            core_sdk::BuildStep::new(BuildOp::ExtractWire).from_steps(vec![5]), // 6 外环
        ]);
        assert!(recipe.is_well_ordered(), "extract_wire recipe 应结构合法");
        let executed = g.execute(&recipe).expect("extract_wire 全链配方应执行成功");
        assert_eq!(executed.step_count(), 7);
        let wire_h = executed.results[4];
        let outer_h = executed.final_handle().unwrap();

        // 外环 BREP 可往返（相容性），且与原 wire 面积一致
        let outer_brep = g.kernel_mut().export_brep(outer_h).unwrap();
        assert!(!outer_brep.is_empty(), "外环 BREP 不应为空");
        let back = g.kernel_mut().import_brep(&outer_brep).unwrap();
        assert_eq!(
            g.kernel_mut().export_brep(back).unwrap(),
            outer_brep,
            "外环 BREP 应可往返"
        );
        let a_outer = g.kernel_mut().surface_area(outer_h).unwrap();
        let a_wire = g.kernel_mut().surface_area(wire_h).unwrap();
        assert!(
            (a_outer - a_wire).abs() < 1e-6,
            "外环与原 wire 面积应一致（{a_outer:.4} vs {a_wire:.4}）"
        );
    }

    // ===== G3：param_defs 键 ↔ execute 缺参报错对齐（计划 §8 风险 1）========

    /// 全部 30 算子（与 core-sdk 清点钉同款清单；exhaustive match 保证同步）。
    fn all_ops() -> Vec<BuildOp> {
        let all: [BuildOp; 30] = [
            BuildOp::MakeBox,
            BuildOp::MakeCylinder,
            BuildOp::MakeSphere,
            BuildOp::MakeCone,
            BuildOp::MakeTorus,
            BuildOp::Extrude,
            BuildOp::Revolve,
            BuildOp::Fuse,
            BuildOp::Cut,
            BuildOp::Common,
            BuildOp::Translate,
            BuildOp::Rotate,
            BuildOp::Sew,
            BuildOp::SewAndSolidify,
            BuildOp::FixShape,
            BuildOp::UnifySameDomain,
            BuildOp::Fillet,
            BuildOp::Chamfer,
            BuildOp::Shell,
            BuildOp::Offset,
            BuildOp::Thicken,
            BuildOp::Loft,
            BuildOp::Sweep,
            BuildOp::Pipe,
            BuildOp::MakeLineEdge,
            BuildOp::MakeArcEdge,
            BuildOp::MakeSplineEdge,
            BuildOp::MakeWire,
            BuildOp::MakeFace,
            BuildOp::ExtractWire,
        ];
        all.into_iter().collect()
    }

    /// 占位参数值（钉子补齐"其它必填键"用；选择器经 encode helper、控制点经
    /// 常量 JSON，禁手搓；三点不共线，occt 腿贝塞尔直通也能过）。
    fn placeholder_value(d: &core_sdk::ParamDef) -> Value {
        use core_sdk::ParamKind;
        match d.kind {
            ParamKind::Length | ParamKind::Angle => Value::Float(1.0),
            ParamKind::Int => Value::Int(1),
            ParamKind::Bool => Value::Bool(true),
            ParamKind::Points => Value::Text(SPLINE_POINTS_JSON.to_string()),
            ParamKind::Selectors => {
                if d.key == "face_selectors" {
                    Value::Text(face_json(&[FaceSelector::ByIndex(0)]))
                } else {
                    Value::Text(edge_json(&[EdgeSelector::ByIndex(0)]))
                }
            }
        }
    }

    /// **G3 钉 3（计划 §8 风险 1，正向）**：遍历全部算子，`param_defs` 的每个
    /// **required** 键在「只缺它」时，`execute` 必须报**含该键名**的错误——
    /// 防「表单改了、execute 不认」（`need()` 键名与 `param_defs` 漂移）。
    ///
    /// 构造：前置喂足输入（`a_cube`）+ 补齐该键**之前**的必填键（placeholder）——
    /// `execute` 臂内参数从左到右求值、先报靠前缺键，所以「只缺第 i 个」须补齐
    /// 前 i 个；这条顺序约定同时钉住了 `param_defs` 的**键序 = 臂内求值顺序**。
    #[test]
    fn param_defs_required_keys_align_with_execute_errors() {
        for op in all_ops() {
            let required: Vec<core_sdk::ParamDef> =
                op.param_defs().into_iter().filter(|d| d.required).collect();
            for (i, missing) in required.iter().enumerate() {
                let mut step = core_sdk::BuildStep::new(op).from_steps((0..op.arity()).collect());
                for d in &required[..i] {
                    step.args.insert(d.key.to_string(), placeholder_value(d));
                }
                let mut steps: Vec<core_sdk::BuildStep> =
                    (0..op.arity()).map(|_| a_cube()).collect();
                steps.push(step);
                let recipe = OpOutput::of(steps);

                let mut g = GeometryService::with_mock_kernel();
                match g.execute(&recipe) {
                    Err(AppError::RecipeNotExecutable(msg)) => assert!(
                        msg.contains(missing.key),
                        "{op:?} 缺 `{}` 应报含键名错误，实得 {msg}",
                        missing.key
                    ),
                    other => panic!(
                        "{op:?} 只缺 `{}`（已补齐前 {i} 个必填键）应报缺参，实得 {other:?}",
                        missing.key
                    ),
                }
                assert_eq!(g.kernel().live_count(), 0, "缺参失败应回滚");
            }
        }
    }

    /// **G3 钉 4（反向）**：全部 `param_defs` 声明键补齐（required 用 placeholder、
    /// 可选用各自 default）+ 喂足输入 → `execute` 的 Err（若有）**不含「缺少参数」**
    /// ——证明 execute 需要的键都被 `param_defs` 覆盖（漏声明的键会报
    /// 「缺少参数 X」；mock 后端对算子本身的 Unsupported / Geometry 照常透传）。
    #[test]
    fn all_declared_keys_satisfy_execute_so_no_missing_param_error() {
        for op in all_ops() {
            let mut step = core_sdk::BuildStep::new(op).from_steps((0..op.arity()).collect());
            for d in op.param_defs() {
                let v = match d.default {
                    Some(core_sdk::ParamDefault::Float(f)) => Value::Float(f),
                    Some(core_sdk::ParamDefault::Int(i)) => Value::Int(i),
                    Some(core_sdk::ParamDefault::Bool(b)) => Value::Bool(b),
                    Some(core_sdk::ParamDefault::Text(t)) => Value::Text(t.to_string()),
                    // required 无预填 → placeholder
                    None => placeholder_value(&d),
                };
                step.args.insert(d.key.to_string(), v);
            }
            let mut steps: Vec<core_sdk::BuildStep> = (0..op.arity()).map(|_| a_cube()).collect();
            steps.push(step);
            let recipe = OpOutput::of(steps);

            let mut g = GeometryService::with_mock_kernel();
            if let Err(AppError::RecipeNotExecutable(msg)) = g.execute(&recipe) {
                assert!(
                    !msg.contains("缺少参数") && !msg.contains("缺少第"),
                    "{op:?} 全声明键补齐后仍报缺参（param_defs 漏声明），实得 {msg}"
                );
            }
        }
    }

    /// **G3 钉 5：`execute_native` 元数校验**——0/1/2 输入算子喂错个数响亮报错；
    /// 正确个数 + 输入句柄执行成功（mock 基本体链路），输入句柄归调用方
    /// （`execute_native` 失败不释放输入）。
    #[test]
    fn execute_native_validates_arity_and_keeps_caller_inputs() {
        let mut g = GeometryService::with_mock_kernel();
        let box_h = g.kernel_mut().make_box(10.0, 10.0, 10.0).unwrap();

        // 0 输入算子喂 1 个输入 → 报错；输入句柄仍存活（归调用方）。
        let got = g.execute_native(BuildOp::MakeBox, &BTreeMap::new(), &[box_h]);
        assert!(got.is_err(), "MakeBox 喂 1 输入应报错");
        assert!(g.kernel().is_alive(box_h), "输入句柄归调用方，失败不释放");

        // 1 输入算子喂 0 个输入 → 报错。
        let mut args = BTreeMap::new();
        args.insert("x".to_string(), Value::Float(100.0));
        args.insert("y".to_string(), Value::Float(0.0));
        args.insert("z".to_string(), Value::Float(0.0));
        let got = g.execute_native(BuildOp::Translate, &args, &[]);
        assert!(got.is_err(), "Translate 喂 0 输入应报错");

        // 正确链路：Translate(box, (100,0,0)) → 新句柄存活、体积不变。
        let executed = g
            .execute_native(BuildOp::Translate, &args, &[box_h])
            .expect("Translate 单步应执行成功");
        assert_eq!(executed.step_count(), 1);
        let moved = executed.final_handle().unwrap();
        assert!(g.kernel().is_alive(moved));
        assert!((g.kernel_mut().volume(moved).unwrap() - 1000.0).abs() < 1e-9);
        // vendor 语义：返回新句柄、原句柄仍有效。
        assert!(g.kernel().is_alive(box_h), "平移后原句柄仍有效");
    }

    // ===== G4-M13：MakeSplineEdge（贝塞尔样条边）接线钉子 ====================

    /// 占位样条控制点（钉子补齐 `points` 键 / 构造样条步骤用；三点不共线）。
    const SPLINE_POINTS_JSON: &str = "[[0.0,0.0,0.0],[100.0,0.0,0.0],[100.0,100.0,0.0]]";

    /// 一个样条边步骤（3 控制点）。
    fn a_spline_edge() -> core_sdk::BuildStep {
        let mut step = core_sdk::BuildStep::new(BuildOp::MakeSplineEdge);
        step.args.insert(
            "points".to_string(),
            Value::Text(SPLINE_POINTS_JSON.to_string()),
        );
        step
    }

    /// **M13 钉 1（mock 记录型 + wire 聚合）**：3 控制点样条边经 `execute` 成功；
    /// `MakeWire` 能把它当成员边聚合——extents（首末控制点）折进 wire 包围盒。
    #[test]
    fn make_spline_edge_records_and_wires_in_mock() {
        let recipe = OpOutput::of(vec![a_spline_edge()]);
        let mut g = GeometryService::with_mock_kernel();
        let executed = g.execute(&recipe).expect("样条边应执行成功");
        let edge = executed.final_handle().unwrap();

        // 聚合成 wire：首末控制点 (0,0,0)/(100,100,0) 折进包围盒。
        let wire = g
            .execute_native(BuildOp::MakeWire, &BTreeMap::new(), &[edge])
            .expect("单边 wire 应成功")
            .final_handle()
            .unwrap();
        let bb = g.kernel_mut().bounding_box(wire).unwrap();
        let (min, max) = (bb.min, bb.max);
        assert!(
            min.iter().all(|v| v.abs() < 1e-9),
            "wire 包围盒 min 应为首控制点 (0,0,0)，实得 {min:?}"
        );
        assert!(
            max.iter()
                .zip([100.0, 100.0, 0.0])
                .all(|(a, b)| (a - b).abs() < 1e-9),
            "wire 包围盒 max 应为末控制点 (100,100,0)，实得 {max:?}"
        );
    }

    /// **M13 钉 2（解析响亮拒绝）**：`points` 点数 < 2、JSON 格式错一律
    /// [`AppError::RecipeNotExecutable`]——绝不静默放行。
    #[test]
    fn make_spline_edge_refuses_bad_points_json() {
        let mut g = GeometryService::with_mock_kernel();

        // 点数 < 2。
        let mut step = core_sdk::BuildStep::new(BuildOp::MakeSplineEdge);
        step.args
            .insert("points".to_string(), Value::Text("[[1.0,2.0,3.0]]".into()));
        let got = g.execute(&OpOutput::of(vec![step]));
        assert!(
            matches!(&got, Err(AppError::RecipeNotExecutable(msg))
                if msg.contains("至少需要 2 个控制点")),
            "单点样条应响亮拒绝，实得 {got:?}"
        );

        // JSON 格式错。
        let mut step = core_sdk::BuildStep::new(BuildOp::MakeSplineEdge);
        step.args
            .insert("points".to_string(), Value::Text("不是 JSON".into()));
        let got = g.execute(&OpOutput::of(vec![step]));
        assert!(
            matches!(&got, Err(AppError::RecipeNotExecutable(msg))
                if msg.contains("解析失败")),
            "坏 JSON 应响亮拒绝，实得 {got:?}"
        );

        // 两条失败路径都不得留下句柄（回滚干净）。
        assert_eq!(g.kernel().live_count(), 0, "解析失败应回滚");
    }

    /// **M13 钉 3（execute_native 直通）**：`execute_native` 同样吃
    /// `MakeSplineEdge`（G3 原生门面自动复用新算子臂）。
    #[test]
    fn execute_native_dispatches_make_spline_edge() {
        let mut args = BTreeMap::new();
        args.insert(
            "points".to_string(),
            Value::Text(SPLINE_POINTS_JSON.to_string()),
        );
        let mut g = GeometryService::with_mock_kernel();
        let executed = g
            .execute_native(BuildOp::MakeSplineEdge, &args, &[])
            .expect("原生门面应直通样条边");
        assert_eq!(executed.step_count(), 1);
        assert!(executed.final_handle().is_some());
    }

    // ===== G4-M15：草图 wire 构建（build_sketch_wire）钉子 ====================

    use glam::DVec2;
    use std::f64::consts::FRAC_PI_2;

    /// **M15 钉 1（矩形折线 wire）**：4 顶点折线 → 4 条直段 → wire，
    /// extents 折进包围盒 (0,0,0) ~ (50,40,0)；中间边句柄全部回收
    /// （live_count 回到 0——句柄可丢，文本为真相）。
    #[test]
    fn sketch_rect_polyline_builds_a_wire() {
        let rect = SketchEntity::Polyline(vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(50.0, 0.0),
            DVec2::new(50.0, 40.0),
            DVec2::new(0.0, 40.0),
            DVec2::new(0.0, 0.0),
        ]);
        let mut g = GeometryService::with_mock_kernel();
        let wire = g.build_sketch_wire(&[rect]).expect("矩形折线应建线成功");

        let bb = g.kernel_mut().bounding_box(wire).unwrap();
        assert!(
            bb.min.iter().all(|v| v.abs() < 1e-9),
            "wire min 应为 (0,0,0)，实得 {bb:?}"
        );
        assert!(
            bb.max
                .iter()
                .zip([50.0, 40.0, 0.0])
                .all(|(a, b)| (a - b).abs() < 1e-9),
            "wire max 应为 (50,40,0)，实得 {bb:?}"
        );
        // 中间边句柄已全部回收（成功路径释放边，只留 wire 一个句柄）。
        assert_eq!(g.kernel().live_count(), 1, "只留 wire 一个句柄");
    }

    /// **M15 钉 2（整圆 wire）**：圆图元 → `make_circle_edge`（记录型）→
    /// wire 包围盒 = 圆心 ± 半径。
    #[test]
    fn sketch_circle_wire_bbox_matches_center_and_radius() {
        let circle = SketchEntity::Circle {
            center: DVec2::new(100.0, 100.0),
            radius: 25.0,
        };
        let mut g = GeometryService::with_mock_kernel();
        let wire = g.build_sketch_wire(&[circle]).expect("圆应建线成功");

        let bb = g.kernel_mut().bounding_box(wire).unwrap();
        assert!(
            bb.min
                .iter()
                .zip([75.0, 75.0, 0.0])
                .all(|(a, b)| (a - b).abs() < 1e-9),
            "wire min 应为圆心−半径，实得 {bb:?}"
        );
        assert!(
            bb.max
                .iter()
                .zip([125.0, 125.0, 0.0])
                .all(|(a, b)| (a - b).abs() < 1e-9),
            "wire max 应为圆心+半径，实得 {bb:?}"
        );
    }

    /// **M15 钉 3（圆弧角度归一）**：终角 < 起角（3π/2 → π）经
    /// `normalize_arc_angles` +2π 后转三点弧——中间点应落在**正向**扫掠区间
    /// （π/4 方向），包围盒据此判定；不归一的话中间点会在反向 (5π/4)。
    #[test]
    fn sketch_arc_normalizes_end_before_start() {
        // 起角 3π/2（朝 −y），终角 π（朝 −x）：正向扫掠途经 2π/0/π/2 …，
        // 归一后中间点在 9π/4 = π/4 方向（+x +y 象限）。
        let arc = SketchEntity::Arc {
            center: DVec2::new(0.0, 0.0),
            radius: 50.0,
            start_angle: 3.0 * FRAC_PI_2,
            end_angle: std::f64::consts::PI,
        };
        let mut g = GeometryService::with_mock_kernel();
        let wire = g.build_sketch_wire(&[arc]).expect("圆弧应建线成功");

        let bb = g.kernel_mut().bounding_box(wire).unwrap();
        // 三点：起点 (0,−50)、中间点 50·(cos 9π/4, sin 9π/4) = 50·cos(π/4) ≈ 35.36、
        // 终点 (−50, 0)。
        let mid = 50.0 * std::f64::consts::FRAC_1_SQRT_2; // 50·cos(π/4)
        assert!(
            bb.max
                .iter()
                .zip([mid, mid, 0.0])
                .all(|(a, b)| (a - b).abs() < 1e-9),
            "归一后中间点应在 π/4 方向（+x+y 象限），实得 {bb:?}"
        );
        assert!(
            bb.min
                .iter()
                .zip([-50.0, -50.0, 0.0])
                .all(|(a, b)| (a - b).abs() < 1e-9),
            "wire min 应为起终点极值，实得 {bb:?}"
        );
    }

    /// **M15 钉 4（椭圆双角）**：`major_angle = 0` 直接记录（包围盒 ±a/±b）；
    /// `major_angle = π/2` 走「造边 + rotate」组合——mock 把 AABB 真转 90°，
    /// 包围盒尺寸应互换（x 范围 ±b、y 范围 ±a），且**记录点（圆心）跟着转**
    /// （rotation 特判修复，z 不被角度污染）。
    #[test]
    fn sketch_ellipse_major_angle_rotates_the_edge() {
        let mut g = GeometryService::with_mock_kernel();

        // 主轴沿 +x：轴对齐。
        let aligned = SketchEntity::Ellipse {
            center: DVec2::new(0.0, 0.0),
            major: 80.0,
            minor: 30.0,
            major_angle: 0.0,
        };
        let w0 = g.build_sketch_wire(&[aligned]).expect("椭圆应建线成功");
        let bb0 = g.kernel_mut().bounding_box(w0).unwrap();
        assert!(
            bb0.min
                .iter()
                .zip([-80.0, -30.0, 0.0])
                .all(|(a, b)| (a - b).abs() < 1e-9)
                && bb0
                    .max
                    .iter()
                    .zip([80.0, 30.0, 0.0])
                    .all(|(a, b)| (a - b).abs() < 1e-9),
            "轴对齐椭圆包围盒应为 ±80/±30，实得 {bb0:?}"
        );

        // 主轴转 90°（ccw）：造边（沿 +x）+ rotate 组合 → 包围盒尺寸互换。
        let rotated = SketchEntity::Ellipse {
            center: DVec2::new(0.0, 0.0),
            major: 80.0,
            minor: 30.0,
            major_angle: FRAC_PI_2,
        };
        let w1 = g.build_sketch_wire(&[rotated]).expect("旋转椭圆应建线成功");
        let bb1 = g.kernel_mut().bounding_box(w1).unwrap();
        assert!(
            bb1.min
                .iter()
                .zip([-30.0, -80.0, 0.0])
                .all(|(a, b)| (a - b).abs() < 1e-9)
                && bb1
                    .max
                    .iter()
                    .zip([30.0, 80.0, 0.0])
                    .all(|(a, b)| (a - b).abs() < 1e-9),
            "π/2 旋转后包围盒应为 ±30/±80（尺寸互换），实得 {bb1:?}"
        );
    }

    /// **M15 钉 5（混合图元边数）**：折线（3 顶点 = 2 段）+ 圆（1 边）+
    /// 圆弧（1 边）→ wire 的边数 = 4（mock 把边数记在 `rotation[0]`）。
    #[test]
    fn sketch_mixed_entities_count_all_edges() {
        let entities = vec![
            SketchEntity::Polyline(vec![
                DVec2::new(0.0, 0.0),
                DVec2::new(50.0, 0.0),
                DVec2::new(50.0, 50.0),
            ]),
            SketchEntity::Circle {
                center: DVec2::new(100.0, 25.0),
                radius: 10.0,
            },
            SketchEntity::Arc {
                center: DVec2::new(0.0, 100.0),
                radius: 20.0,
                start_angle: 0.0,
                end_angle: FRAC_PI_2,
            },
        ];
        let mut g = GeometryService::with_mock_kernel();
        let wire = g.build_sketch_wire(&entities).expect("混合图元应建线成功");

        let text = g.kernel_mut().export_brep(wire).unwrap();
        // mock 的 wire 把边数记在 `rotation[0]`。v3 文本 token 布局：
        // magic(0) version(1) tag(2) size[3](3..6) volume(6) origin[3](7..10)
        // rotation[3](10..13) solid_count(13) → rotation[0] = tokens[10]。
        let tokens: Vec<&str> = text.split_whitespace().collect();
        let edge_count: f64 = tokens[10].parse().expect("rotation[0] 应为可解析数");
        assert!(
            (edge_count - 4.0).abs() < 1e-9,
            "wire 应含 4 条边，实得 {tokens:?}"
        );
    }

    /// **M15 钉 6（非法输入响亮拒绝 + 回滚）**：负半径 → [`AppError::Kernel`]；
    /// 单点折线 → [`AppError::RecipeNotExecutable`]；空图元表 →
    /// [`AppError::RecipeNotExecutable`]。三条失败路径都**不留句柄**
    /// （live_count 回 0——回滚纪律）。
    #[test]
    fn sketch_wire_refuses_bad_input_and_rolls_back() {
        let mut g = GeometryService::with_mock_kernel();

        // 负半径（内核层 InvalidArgument）。
        let bad = SketchEntity::Circle {
            center: DVec2::new(0.0, 0.0),
            radius: -5.0,
        };
        let got = g.build_sketch_wire(&[bad]);
        assert!(
            matches!(&got, Err(AppError::Kernel(core_kernel::KernelError::InvalidArgument(m)))
                if m.contains("radius")),
            "负半径应报 Kernel::InvalidArgument，实得 {got:?}"
        );

        // 单点折线（组装层 RecipeNotExecutable，先于内核）。
        let thin = SketchEntity::Polyline(vec![DVec2::new(0.0, 0.0)]);
        let got = g.build_sketch_wire(&[thin]);
        assert!(
            matches!(&got, Err(AppError::RecipeNotExecutable(m)) if m.contains("至少需要 2 个顶点")),
            "单点折线应响亮拒绝，实得 {got:?}"
        );

        // 空图元表。
        let got = g.build_sketch_wire(&[]);
        assert!(
            matches!(got, Err(AppError::RecipeNotExecutable(_))),
            "空表应拒绝"
        );

        // 三条失败路径都不留句柄（回滚干净）。
        assert_eq!(g.kernel().live_count(), 0, "失败应回滚全部已建边");
    }

    /// **M16 钉 1（贝塞尔分段直通）**：每段控制点序列各产一条
    /// `make_bezier_edge`（mock 腿 EdgeSpline 记录首末控制点）——两段
    /// （3 点 n 阶 + 4 点三次）合成 wire 后 bbox 覆盖两段端点、边数 = 2。
    #[test]
    fn sketch_bezier_builds_edges_per_segment() {
        let bezier = SketchEntity::Bezier(vec![
            vec![
                DVec2::new(-50.0, 0.0),
                DVec2::new(0.0, 0.0),
                DVec2::new(50.0, 0.0),
            ],
            vec![
                DVec2::new(50.0, 0.0),
                DVec2::new(50.0, 50.0),
                DVec2::new(0.0, 50.0),
                DVec2::new(-50.0, 50.0),
            ],
        ]);
        let mut g = GeometryService::with_mock_kernel();
        let wire = g.build_sketch_wire(&[bezier]).expect("贝塞尔应建线成功");

        let bb = g.kernel_mut().bounding_box(wire).unwrap();
        assert!(
            bb.min
                .iter()
                .zip([-50.0, 0.0, 0.0])
                .all(|(a, b)| (a - b).abs() < 1e-9),
            "wire min 应为两段端点包络，实得 {bb:?}"
        );
        assert!(
            bb.max
                .iter()
                .zip([50.0, 50.0, 0.0])
                .all(|(a, b)| (a - b).abs() < 1e-9),
            "wire max 应为两段端点包络，实得 {bb:?}"
        );
        let text = g.kernel_mut().export_brep(wire).unwrap();
        let tokens: Vec<&str> = text.split_whitespace().collect();
        let edge_count: f64 = tokens[10].parse().expect("rotation[0] 应为可解析数");
        assert!(
            (edge_count - 2.0).abs() < 1e-9,
            "两段贝塞尔应产 2 条边，实得 {tokens:?}"
        );
    }

    /// **M16 钉 2（辅助图元不进 wire）**：点 / 构造线 / 射线组装时跳过
    /// （0 条边）——wire 只含折线的直段，边数与 bbox 不受辅助图元影响。
    #[test]
    fn sketch_aux_entities_are_skipped_in_wire() {
        let entities = vec![
            SketchEntity::Polyline(vec![DVec2::new(0.0, 0.0), DVec2::new(100.0, 0.0)]),
            SketchEntity::Point(DVec2::new(200.0, 200.0)),
            SketchEntity::AuxLine {
                from: DVec2::new(0.0, 0.0),
                to: DVec2::new(100.0, 100.0),
                ray: false,
            },
        ];
        let mut g = GeometryService::with_mock_kernel();
        let wire = g.build_sketch_wire(&entities).expect("混合图元应建线成功");

        let bb = g.kernel_mut().bounding_box(wire).unwrap();
        let min_ok = bb
            .min
            .iter()
            .zip([0.0, 0.0, 0.0])
            .all(|(a, b)| (a - b).abs() < 1e-9);
        let max_ok = bb
            .max
            .iter()
            .zip([100.0, 0.0, 0.0])
            .all(|(a, b)| (a - b).abs() < 1e-9);
        assert!(
            min_ok && max_ok,
            "wire bbox 应只含折线直段（辅助图元不影响），实得 {bb:?}"
        );
        let text = g.kernel_mut().export_brep(wire).unwrap();
        let tokens: Vec<&str> = text.split_whitespace().collect();
        let edge_count: f64 = tokens[10].parse().expect("rotation[0] 应为可解析数");
        assert!(
            (edge_count - 1.0).abs() < 1e-9,
            "辅助图元应跳过：wire 只含折线 1 条直段，实得 {tokens:?}"
        );
    }

    /// **M16 钉 3（全辅助草图响亮拒绝）**：只有点 / 构造线 / 射线时组装出
    /// 0 条边——`build_sketch_wire` 前置检查拒绝（绝不把空 wire 交内核）。
    #[test]
    fn sketch_aux_only_wire_is_rejected() {
        let entities = vec![
            SketchEntity::Point(DVec2::new(10.0, 10.0)),
            SketchEntity::AuxLine {
                from: DVec2::ZERO,
                to: DVec2::new(50.0, 0.0),
                ray: true,
            },
        ];
        let mut g = GeometryService::with_mock_kernel();
        let got = g.build_sketch_wire(&entities);
        assert!(
            matches!(&got, Err(AppError::RecipeNotExecutable(m)) if m.contains("辅助")),
            "全辅助草图应响亮拒绝，实得 {got:?}"
        );
        assert_eq!(g.kernel().live_count(), 0, "拒绝路径不留句柄");
    }

    /// **M16 钉 4（贝塞尔段控制点不足 2 响亮拒绝）**：单控制点段不是合法
    /// 贝塞尔边——组装层拒绝并回滚（内核层 N≥2 校验的前置兜底）。
    #[test]
    fn sketch_bezier_rejects_segment_with_fewer_than_two_controls() {
        let bezier = SketchEntity::Bezier(vec![vec![DVec2::new(0.0, 0.0)]]);
        let mut g = GeometryService::with_mock_kernel();
        let got = g.build_sketch_wire(&[bezier]);
        assert!(
            matches!(&got, Err(AppError::RecipeNotExecutable(m)) if m.contains("2 个控制点")),
            "不足 2 控制点的贝塞尔段应响亮拒绝，实得 {got:?}"
        );
        assert_eq!(g.kernel().live_count(), 0, "拒绝路径不留句柄");
    }
}

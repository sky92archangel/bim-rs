//! 会话门面：**把四个服务串成一个"会话对象"**（设计文档 §9.2 / §9.3）。
//!
//! ## 本层是什么
//!
//! `core-app` 此前有四个各自独立的服务，但**没有"一个会话"把它们串起来**：
//!
//! ```text
//! CoreSession                     ← 唯一能同时看见五层的装配口
//!   ├─ DocumentService  文档 + 撤销栈（能力①）
//!   ├─ GeometryService  ShapeId ↔ ShapeHandle（能力②⑤）
//!   ├─ InteropService   图纸 ⇄ 中性二维结构（能力③④）
//!   └─ ModuleHost       专业模块装载 + 图层落地（能力⑥）
//! ```
//!
//! **它只编排，不实现算法**：几何一律转交 [`GeometryService`]，
//! 文档编辑一律走 [`DocumentService`]，模块落库一律走 [`ModuleHost`]。
//! 把它当成"积木的拼装说明书"，而不是"又一层实现"。
//!
//! ## 端到端的一条链
//!
//! ```text
//! install_module → create_element → build_geometry → create_view → save / open
//!     (ModuleHost)    (工厂 + 文档)    (算子 + 句柄 + 落盘)   (出图 + 存图)
//! ```
//!
//! ## 默认内核
//!
//! [`CoreSession::new`] 默认用**占位内核**（[`GeometryService::with_mock_kernel`]）
//! 配默认的 BREP 装载器——与 [`GeometryService`] 的既有约定一致：
//! **显式**选择占位后端，绝不静默把 `GeometryService::new()` 的失败吞掉。
//! 真实内核待 `occt-wasm` 可实例化后再切（见仓库 README 的「M1」节）。

use core_doc::{
    Camera, DraftingStandard, Drawing, Entity2D, PickFilter, PickResult, View, ViewKind,
    ViewOrientation,
};
use core_kernel::{KernelError, MeshData};
use core_model::{Document, ElementId, PropertyMap, ShapeId, ViewId};
use core_sdk::{Module, OpInput, SdkError};
use std::collections::BTreeMap;
use std::path::Path;

use crate::document_service::DocumentService;
use crate::error::{AppError, AppResult};
use crate::evolution_align::HistoryRecord;
use crate::geometry_service::GeometryService;
use crate::interop_service::InteropService;
use crate::module_host::ModuleHost;
use crate::picker_service::KernelPicker;
use crate::projection_service::{project_shape, reproject_into};
use crate::store;
use crate::topo_naming::face_hash_names_from_handle;
use crate::update_service::{UpdateReport, update_after_commit};

/// 会话概览（供桌面端自检打印、状态栏显示）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    /// 已装载模块数
    pub modules: usize,
    /// 元素数
    pub elements: usize,
    /// 几何记录数
    pub geometry: usize,
    /// 专业图层数（模块声明的）
    pub layers: usize,
}

/// 一个建模 / 出图会话。
pub struct CoreSession {
    /// 文档 + 图纸 + 撤销栈
    document: DocumentService,
    /// 持久 `ShapeId` ↔ 会话 `ShapeHandle`
    geometry: GeometryService,
    /// 图纸 ⇄ 中性二维结构
    interop: InteropService,
    /// 专业模块宿主
    modules: ModuleHost,
}

impl CoreSession {
    /// 新建一个空会话（占位内核 + 默认 BREP 装载器 + 国标工业厂房图纸）。
    ///
    /// 真实几何内核经 feature `occt` 提供；此处的占位后端是**显式**选择，
    /// 不是"构造失败后偷偷降级"。
    pub fn new() -> AppResult<Self> {
        Ok(Self {
            document: DocumentService::new("未命名"),
            geometry: GeometryService::with_mock_kernel(),
            interop: InteropService::new(),
            modules: ModuleHost::new(),
        })
    }

    /// 打开一个 `.bimdoc`（zip 或目录形态）并包成会话。
    ///
    /// v2 容器自带图纸段（`drawing.json` + `views/`），`Drawing` 随容器恢复；
    /// 装载器 / 模块需要调用方在打开后自行装回（模块是启动期动作，
    /// 文档本身不记录"用了哪些模块"，见设计文档 8.4）。
    pub fn open(path: &Path) -> AppResult<Self> {
        let (doc, drawing) = store::open_document(path)?;
        Ok(Self::from_parts(doc, drawing))
    }

    /// 用**真实内核**新建一个空会话（occt feature 门控；M6b 真腿装配）。
    ///
    /// 与 [`CoreSession::new`] 的占位内核相对：**显式**要求真实后端，
    /// 未启用 `occt` / 内核不可用时返回 [`AppError::Kernel`]，
    /// **绝不**静默降级成占位内核（与 [`GeometryService::new`] 同一纪律）。
    #[cfg(feature = "occt")]
    pub fn with_real_kernel() -> AppResult<Self> {
        Ok(Self {
            document: DocumentService::new("未命名"),
            geometry: GeometryService::new()?,
            interop: InteropService::new(),
            modules: ModuleHost::new(),
        })
    }

    /// 打开 `.bimdoc` 并配**真实内核**（occt feature 门控；M6b 真腿装配）。
    ///
    /// [`CoreSession::open`] 的真腿变体：容器装配相同（图纸随容器恢复），
    /// 几何服务用 [`GeometryService::new`]（真实内核）——重开后的 rebuild /
    /// 投影 / 指纹计算走真实几何，而 [`CoreSession::open`] 配占位内核。
    #[cfg(feature = "occt")]
    pub fn open_real(path: &Path) -> AppResult<Self> {
        let (doc, drawing) = store::open_document(path)?;
        Ok(Self {
            document: DocumentService::with_parts(doc, drawing),
            geometry: GeometryService::new()?,
            interop: InteropService::new(),
            modules: ModuleHost::new(),
        })
    }

    /// 用一份已恢复的文档构造会话（几何缓存 / 历史皆为空）。
    ///
    /// ⚠ **重开请走 [`CoreSession::open`]**：本函数的 `Drawing` 是**空壳**
    /// （占位默认），不是文档里保存过的图纸——直接用它重开会丢图纸。
    /// 本函数保留给"从纯文档骨架起步"的测试与迁移场景。
    #[must_use]
    pub fn from_document(doc: Document) -> Self {
        Self::from_parts(doc, Drawing::new())
    }

    /// 用指定的文档与图纸构造会话（几何缓存 / 历史皆为空）。
    ///
    /// [`CoreSession::open`] 的装配底层；也便于测试注入既有出图状态。
    #[must_use]
    pub fn from_parts(doc: Document, drawing: Drawing) -> Self {
        Self {
            document: DocumentService::with_parts(doc, drawing),
            geometry: GeometryService::with_mock_kernel(),
            interop: InteropService::new(),
            modules: ModuleHost::new(),
        }
    }

    // ---------- 模块 ----------

    /// 装载一个专业模块（失败则宿主保持原样，见 [`ModuleHost::install`]）。
    pub fn install_module(&mut self, module: Box<dyn Module>) -> AppResult<()> {
        self.modules.install(module)?;
        Ok(())
    }

    // ---------- 建模用例 ----------

    /// **新建构件**：按品类走模块工厂造语义元素，再收进文档。
    ///
    /// 工厂**不建几何**（设计文档 9.2）；几何由 [`CoreSession::build_geometry`]
    /// 随后回填。返回新元素的持久 id。
    pub fn create_element(&mut self, category: &str, name: &str) -> AppResult<ElementId> {
        let element = self.modules.create_named(category, name)?;
        self.document.add_element(element)
    }

    /// **造几何**：走 SDK 的几何算子 → 执行配方 → 落盘 → 回填元素。
    ///
    /// 步骤：
    /// 1. 读元素的属性当作算子输入；
    /// 2. 从模块注册表取几何算子 [`core_sdk::GeometryOp`] 并生成配方；
    /// 3. [`GeometryService::execute`] 把配方式变成内核句柄；
    /// 4. [`GeometryService::snapshot`] 导出 BREP 文本，收进文档，
    ///    并把 `element.geometry_ref` 指向新几何。
    ///
    /// 执行完**释放全部中间与最终句柄**——句柄只是会话缓存，
    /// 文档里的 BREP 文本才是真相（设计文档 3.3：句柄可丢）。
    pub fn build_geometry(&mut self, element: ElementId, op_id: &str) -> AppResult<ShapeId> {
        // ① 元素必须存在，且尚未有几何
        let (params, already) = {
            let e = self.document.doc().element(element).ok_or(AppError::Model(
                core_model::ModelError::ElementNotFound(element),
            ))?;
            (e.properties.clone(), e.has_geometry())
        };
        if already {
            return Err(AppError::RecipeNotExecutable(format!(
                "元素 {element} 已有几何，拒绝重复构造（先删旧几何再重建）"
            )));
        }

        // ② 取算子 → 配方（借用在这里结束，下面才能可变借 geometry）
        let recipe = {
            let op = self
                .modules
                .registry()
                .geometry_op(op_id)
                .ok_or_else(|| SdkError::GeometryOp(format!("未登记几何算子 `{op_id}`")))?;
            op.apply(&OpInput::new(params))?
        };

        // ③ 执行配方
        let executed = self.geometry.execute(&recipe)?;
        let handles: Vec<core_kernel::ShapeHandle> = executed.handles().to_vec();
        let final_handle = *handles.last().ok_or_else(|| {
            AppError::RecipeNotExecutable(format!(
                "几何算子 `{op_id}` 的配方为空，没有产生任何形状"
            ))
        })?;

        // ④ 导出 + 落盘 + 回填
        let shape = ShapeId::new();
        let record = self.geometry.snapshot(shape, final_handle)?;
        self.document.edit(|doc| {
            doc.insert_geometry(record)?;
            let e = doc.element_mut(element)?;
            e.geometry_ref = Some(shape);
            Ok(())
        })?;

        // ④′ 记录带历史演化（M6b 路径 B，会话态）：本次执行中**结果句柄即最终句柄**
        //     的那条演化就是该形状的构造历史，绑定新 `ShapeId` 登记——
        //     [`GeometryService::history_of`] 据此回读。配方无布尔演化（或布尔后
        //     还有后续步骤）时不登记，关联更新对该形状退避到几何指纹。
        if let Some((_, hrec)) = executed
            .evolutions()
            .iter()
            .find(|(_, rec)| rec.evolution.result == final_handle)
        {
            self.geometry.remember_history(shape, hrec.clone());
        }

        // ⑤ 释放本次执行产生的全部句柄（含最终结果）：句柄可丢，文本为真相
        for handle in &handles {
            let _ = self.geometry.kernel_mut().release(*handle);
        }
        Ok(shape)
    }

    /// **修改既有元素几何并触发关联更新**（[`update_after_commit`] 的首个生产调用方）。
    ///
    /// 与 [`CoreSession::build_geometry`] 的差异：元素**已有**几何——新配方的结果
    /// **复用原 `ShapeId`** 收进文档（remove + insert 同 id，构件身份不变），图纸里
    /// 绑定在该形状上的图元 / 标注经键解析（演化优先、指纹退避）与**定向重投**跟着
    /// 更新，需要重新生成的视图被标脏（`up_to_date = false`）。
    ///
    /// 流程：
    /// 1. 读元素 `geometry_ref` 得旧形状 id（元素不存在 / 尚无几何 → 明确报错，
    ///    不静默造新——无几何请走 [`CoreSession::build_geometry`]）；
    /// 2. 执行新配方（参数由调用方给，**不经**元素属性）→ snapshot；
    /// 3. `document.edit` 复用原 `ShapeId` 换几何记录；
    /// 4. 缝合 histories：本次执行 `evolutions()` 中**结果句柄即最终句柄**的那条
    ///    [`HistoryRecord`] 绑定该 `ShapeId`（**最小闭环**：中间 step 的句柄已释放、
    ///    无 `ShapeId`，不强行缝合中间态——不宣称清单 #3）；配方无布尔演化则
    ///    `histories` 为空，键解析退避到几何指纹（安全，不 panic）；
    /// 5. 调 [`update_after_commit`]（定向重投 + 标注重算 + 索引 remap + 标脏），
    ///    返回 [`UpdateReport`] 供调用方决定 regenerate——本门面**不自动重投**
    ///    stale 视图。
    ///
    /// # 红线（继承自 [`update_after_commit`]，原样保持不绕过）
    ///
    /// 本方法构造的 `changed_shapes` 恒为该形状（非空）；"空 `changed_shapes` →
    /// 全 0 报告、图纸逐字节不变"的既有红线在内层原样继承，本门面不改写。
    pub fn rebuild_geometry(
        &mut self,
        element_id: ElementId,
        op_id: &str,
        params: PropertyMap,
    ) -> AppResult<UpdateReport> {
        // ① 元素必须存在，且已有几何（rebuild 的前提）
        let old_shape = {
            let e = self
                .document
                .doc()
                .element(element_id)
                .ok_or(AppError::Model(core_model::ModelError::ElementNotFound(
                    element_id,
                )))?;
            e.geometry_ref.ok_or(AppError::RecipeNotExecutable(format!(
                "元素 {element_id} 尚无几何，无从重建（请先走 build_geometry 构造）"
            )))?
        };

        // ② 取算子 → 配方（借用随语句结束）→ 执行新配方
        let recipe = {
            let op = self
                .modules
                .registry()
                .geometry_op(op_id)
                .ok_or_else(|| SdkError::GeometryOp(format!("未登记几何算子 `{op_id}`")))?;
            op.apply(&OpInput::new(params))?
        };
        let executed = self.geometry.execute(&recipe)?;
        let handles: Vec<core_kernel::ShapeHandle> = executed.handles().to_vec();
        let final_handle = *handles.last().ok_or_else(|| {
            AppError::RecipeNotExecutable(format!(
                "几何算子 `{op_id}` 的配方为空，没有产生任何形状"
            ))
        })?;

        // ③ snapshot 新几何 → 复用原 ShapeId 收进文档（构件身份不变：
        //    几何变了，构件还是那个构件；视图绑定键因此继续指向该形状）
        let record = self.geometry.snapshot(old_shape, final_handle)?;
        self.document.edit(|doc| {
            doc.remove_geometry(old_shape);
            doc.insert_geometry(record)?;
            Ok(())
        })?;

        // ④ 缝合 histories（最小闭环）：结果句柄即最终句柄的那条演化绑定该 ShapeId；
        //    无（配方无布尔 / 布尔后还有后续步骤）则空 → 键解析退避到几何指纹。
        //    中间 step 的句柄已释放、无 ShapeId——不强行缝合中间态（不宣称清单 #3）。
        let histories: Vec<(ShapeId, HistoryRecord)> = executed
            .evolutions()
            .iter()
            .filter(|(_, rec)| rec.evolution.result == final_handle)
            .map(|(_, rec)| (old_shape, rec.clone()))
            .collect();

        // ⑤ 释放本次执行产生的全部句柄（句柄可丢，文本为真相——与 build_geometry 一致）
        for handle in &handles {
            let _ = self.geometry.kernel_mut().release(*handle);
        }

        // ⑥ 关联更新：定向重投 + 标注重算 + 索引 remap + 标脏。
        //    changed_shapes 恒非空 → update_after_commit 既有红线在内层原样继承；
        //    本门面不自动重投 stale 视图（调用方据 UpdateReport 决定）。
        let kernel = self.geometry.kernel_mut();
        let report = self.document.with_doc_and_drawing_mut(|doc, drawing| {
            update_after_commit(kernel, doc, drawing, &[old_shape], &histories)
        })?;
        Ok(report)
    }

    /// **对既有元素做布尔切削并触发关联更新**（第二条生产路径：旧几何参与布尔）。
    ///
    /// 与 [`CoreSession::rebuild_geometry`] 的本质差异：rebuild 是「毛坯 → 结果」
    /// **从零重造**——其演化记录的 `input_names` 是**新配方操作数（毛坯）的面名**，
    /// 而文档里的旧键是**旧几何**的面名，两者对不上，演化优先分支在 rebuild 场景
    /// 数学上无法真实命中、必指纹退避；carve 是**旧几何句柄直接参与布尔**——
    /// 演化记录的 `input_names` 含**旧几何面名**，旧键经「输入稳定名 → 输出面
    /// 稳定名」精确直连，演化优先分支**真实命中**（M6b-3 降级理由的消除路径）。
    ///
    /// 流程：
    /// 1. 读元素 `geometry_ref` 得旧形状 id（元素不存在 / 尚无几何 → 明确报错，
    ///    不静默造新——无几何请走 [`CoreSession::build_geometry`]）；
    /// 2. [`GeometryService::resolve`] 懒重建旧句柄 + `execute` 刀具配方
    ///    （`tool_op_id` 经算子注册表产刀具毛坯，参数由调用方给、不经元素属性）；
    /// 3. [`core_kernel::Kernel::cut_with_history`]（既有 API，后端零改动）：hashes
    ///    取 `face_hashes(旧) ∪ face_hashes(刀具)` 并集（照 `boolean_with_history`
    ///    口径）；布尔当刻采集两侧「面哈希 → 稳定名」→ **input_names 含旧几何面名**；
    /// 4. 复用旧 `ShapeId` 收新几何（remove + insert 同 id，构件身份不变）；
    ///    缝合 histories：`(旧形状, 本次演化记录)`；
    /// 5. 调 [`update_after_commit`]（update_service 本体零改动），返回
    ///    [`UpdateReport`] 供调用方决定 regenerate——本门面不自动重投 stale 视图；
    /// 6. 释放本次全部句柄（旧句柄 + 刀具句柄 + 新句柄；句柄可丢，文本为真相）。
    ///
    /// # 回退（与 `boolean_with_history` 同一纪律）
    ///
    /// 后端不具备带历史布尔（占位内核返回 `Unsupported`）时，**回退**为普通
    /// [`core_kernel::Kernel::cut`] 并置空演化记录，回退说明并入返回报告的
    /// `warnings`（**绝不静默**）——键解析对该形状退避到几何指纹（安全，不 panic）。
    ///
    /// # 红线（继承自 [`update_after_commit`]，原样保持不绕过）
    ///
    /// 本方法构造的 `changed_shapes` 恒为该形状（非空）；"空 `changed_shapes` →
    /// 全 0 报告、图纸逐字节不变"的既有红线在内层原样继承，本门面不改写。
    pub fn carve_geometry(
        &mut self,
        element_id: ElementId,
        tool_op_id: &str,
        tool_params: PropertyMap,
    ) -> AppResult<UpdateReport> {
        // ① 元素必须存在，且已有几何（carve 的前提，照 rebuild_geometry ①）
        let old_shape = {
            let e = self
                .document
                .doc()
                .element(element_id)
                .ok_or(AppError::Model(core_model::ModelError::ElementNotFound(
                    element_id,
                )))?;
            e.geometry_ref.ok_or(AppError::RecipeNotExecutable(format!(
                "元素 {element_id} 尚无几何，无从开孔切削（请先走 build_geometry 构造）"
            )))?
        };

        // ② 懒重建旧句柄（既有路径）+ 取刀具算子 → 配方 → 执行
        let old_handle = self.geometry.resolve(self.document.doc(), old_shape)?;
        let recipe = {
            let op = self
                .modules
                .registry()
                .geometry_op(tool_op_id)
                .ok_or_else(|| SdkError::GeometryOp(format!("未登记刀具算子 `{tool_op_id}`")))?;
            op.apply(&OpInput::new(tool_params))?
        };
        let executed = self.geometry.execute(&recipe)?;
        let tool_handle = executed.final_handle().ok_or_else(|| {
            AppError::RecipeNotExecutable(format!(
                "刀具算子 `{tool_op_id}` 的配方为空，没有产生任何刀具"
            ))
        })?;

        let mut notices: Vec<String> = Vec::new();

        // ③ 带历史布尔：hashes 取两侧面哈希并集（照 boolean_with_history 口径）；
        //    布尔当刻、句柄还活着——采集两侧「面哈希 → 稳定名」，
        //    **input_names 含旧几何面名**（演化优先分支真实命中的关键）。
        let mut hashes = self
            .geometry
            .kernel_mut()
            .face_hashes(old_handle)
            .unwrap_or_default();
        hashes.extend(
            self.geometry
                .kernel_mut()
                .face_hashes(tool_handle)
                .unwrap_or_default(),
        );
        let mut input_names =
            match face_hash_names_from_handle(self.geometry.kernel_mut(), old_handle) {
                Ok(names) => names,
                Err(err) => {
                    notices.push(format!(
                        "carve_geometry：无法采集旧几何面名（{err}），本步演化对齐将退避到几何指纹"
                    ));
                    BTreeMap::new()
                }
            };
        match face_hash_names_from_handle(self.geometry.kernel_mut(), tool_handle) {
            Ok(names) => input_names.extend(names),
            Err(err) => notices.push(format!(
                "carve_geometry：无法采集刀具体面名（{err}），本步演化对齐将退避到几何指纹"
            )),
        }

        let attempt = self
            .geometry
            .kernel_mut()
            .cut_with_history(old_handle, tool_handle, &hashes);
        let (new_handle, history) = match attempt {
            Ok(evo) => {
                let output_names = match face_hash_names_from_handle(
                    self.geometry.kernel_mut(),
                    evo.result,
                ) {
                    Ok(names) => names,
                    Err(err) => {
                        notices.push(format!(
                            "carve_geometry：无法采集结果面名（{err}），本步演化对齐将退避到几何指纹"
                        ));
                        BTreeMap::new()
                    }
                };
                let handle = evo.result;
                (
                    handle,
                    Some(HistoryRecord::new(evo, input_names, output_names)),
                )
            }
            Err(KernelError::Unsupported(reason)) => {
                // 回退绝不静默：普通 cut + 置空演化记录 + 可观测说明。
                let handle = self.geometry.kernel_mut().cut(old_handle, tool_handle)?;
                notices.push(format!(
                    "carve_geometry：后端不支持带历史布尔（{reason}），已回退为无历史 cut，\
                     本次切削不产出演化记录（关联更新对该形状将退避到几何指纹）"
                ));
                (handle, None)
            }
            Err(err) => return Err(err.into()),
        };

        // ④ 复用旧 ShapeId 收新几何（remove + insert 同 id，构件身份不变：
        //    几何变了，构件还是那个构件；视图绑定键因此继续指向该形状）
        let record = self.geometry.snapshot(old_shape, new_handle)?;
        self.document.edit(|doc| {
            doc.remove_geometry(old_shape);
            doc.insert_geometry(record)?;
            Ok(())
        })?;

        // 缝合 histories（本次演化记录绑旧形状）；回退路径无记录 → 空 → 键解析退避指纹。
        let histories: Vec<(ShapeId, HistoryRecord)> = history
            .map(|rec| vec![(old_shape, rec)])
            .unwrap_or_default();

        // ⑤ 释放本次全部句柄（句柄可丢，文本为真相——与 build/rebuild 一致）：
        //    先解除旧形状的句柄映射（旧几何已被布尔替换，映射不得悬垂），
        //    再释放旧句柄 + 刀具配方全部句柄 + 新结果句柄。
        if let Some(cached) = self.geometry.forget(old_shape) {
            let _ = self.geometry.kernel_mut().release(cached);
        }
        for handle in executed.handles() {
            let _ = self.geometry.kernel_mut().release(*handle);
        }
        let _ = self.geometry.kernel_mut().release(new_handle);

        // ⑥ 关联更新（update_after_commit 本体零改动）+ 回退说明并入报告（绝不静默）。
        let kernel = self.geometry.kernel_mut();
        let mut report = self.document.with_doc_and_drawing_mut(|doc, drawing| {
            update_after_commit(kernel, doc, drawing, &[old_shape], &histories)
        })?;
        report.warnings.extend(notices);
        Ok(report)
    }

    // ---------- 出图用例（五环"出图"环的门面入口，设计文档 §8.5）----------

    /// **新建视图并做 P0 投影初生成**（五环"出图"入口）。
    ///
    /// 流程：建视图（登记进图纸）→ [`projection_service::reproject_into`]
    /// 全量重投并写回 → [`Drawing::rebuild_index`] → [`DocumentService::sync_drawing_registry`]
    /// 把视图 id 登记进 `Document`。
    ///
    /// - 视图 `kind` 取 [`ViewKind::Drafting`]：P0 出图不挑基准（`from_level` /
    ///   `from_grid` 的基准挂接属后续用例），`Drafting` 不要求基准元素，
    ///   [`Drawing::validate`] 保持合法。
    /// - 投影产生的**警告不作为错误**（"投影为空 / 曲线降级"都是合法状态）；
    ///   需要逐条观测警告请走 [`CoreSession::regenerate_view`] 或
    ///   [`CoreSession::project_shape`]——二者把 `(数量, warnings)` 原样上抛。
    pub fn create_view(&mut self, name: &str, orientation: ViewOrientation) -> AppResult<ViewId> {
        // ① 建视图并登记进图纸（借用随语句结束）
        let view = View::new(name, ViewKind::Drafting).with_orientation(orientation);
        let view_id = view.id;
        self.document.drawing_mut().add_view(view);

        // ② P0 投影初生成 + 写回（reproject_into 内部置 up_to_date = true）+ 重建索引
        //    字段级分离借用：standard（不可变）与单个视图（可变）同取。
        let kernel = self.geometry.kernel_mut();
        self.document.with_doc_and_drawing_mut(|doc, drawing| {
            let standard = &drawing.standard;
            let view = drawing
                .views
                .get_mut(&view_id)
                .ok_or(core_doc::DocError::ViewNotFound(view_id))?;
            reproject_into(kernel, doc, view, standard)?;
            drawing.rebuild_index();
            Ok::<(), AppError>(())
        })?;

        // ③ 把视图 id 登记 / 对齐进 Document（引用集合两侧一致）
        self.document.sync_drawing_registry()?;
        Ok(view_id)
    }

    /// **全量重投一个视图**（关联更新的兜底入口；写回 + 重建索引）。
    ///
    /// [`projection_service::reproject_into`] 的门面包装：写回后置
    /// `up_to_date = true`，返回 `(写入图元数, 警告)`——警告**原样上抛**
    /// （修 P1-2 的延续：降级 / 丢弃 / 投影为空都如实可见，不被吞）。
    pub fn regenerate_view(
        &mut self,
        view_id: ViewId,
        standard: &DraftingStandard,
    ) -> AppResult<(usize, Vec<String>)> {
        let kernel = self.geometry.kernel_mut();
        self.document.with_doc_and_drawing_mut(|doc, drawing| {
            let view = drawing.view_mut(view_id)?;
            let out = reproject_into(kernel, doc, view, standard)?;
            drawing.rebuild_index();
            Ok(out)
        })
    }

    /// **单形状投影**（诊断用，不写回任何视图）。
    ///
    /// [`projection_service::project_shape`] 的门面转发：返回 `(图元, 警告)`，
    /// 警告原样上抛。
    ///
    /// 签名取 `&mut self`（与规划初稿的 `&self` 不同）：投影要从 `Document`
    /// 装载 BREP 并调内核 [`Kernel::project_edges`]——两者都需要可变内核
    /// （`projection_service::project_shape` 的 kernel 参数是 `&mut Kernel`）。
    pub fn project_shape(
        &mut self,
        shape: ShapeId,
        view: &View,
        standard: &DraftingStandard,
    ) -> AppResult<(Vec<Entity2D>, Vec<String>)> {
        project_shape(
            self.geometry.kernel_mut(),
            self.document.doc(),
            shape,
            view,
            standard,
        )
    }

    // ---------- 渲染 / 拾取（GUI 消费，M8 起点） ----------

    /// 网格化某形状（渲染层消费；内部先 `resolve` 惰性重建句柄）。
    ///
    /// 为什么放会话层：网格化需要可变内核（`Kernel::tessellate` 是 `&mut self`），
    /// 而 [`CoreSession::geometry`] 只给了只读访问——把 "resolve → tessellate"
    /// 两步合成一个门面，调用方（桌面端视口）不再需要自己拆借用。
    pub fn tessellate(&mut self, shape: ShapeId, linear: f64, angular: f64) -> AppResult<MeshData> {
        let handle = self.geometry.resolve(self.document.doc(), shape)?;
        Ok(self
            .geometry
            .kernel_mut()
            .tessellate(handle, linear, angular)?)
    }

    /// 三维拾取（[`KernelPicker`] 的门面转发）：屏幕坐标 → 世界射线 → 面组粗筛 →
    /// 内核精算 → 元素反查。
    ///
    /// 返回语义与 [`KernelPicker::pick`] 一致：**未命中是 `Ok(None)`、错误是 `Err`**（可区分）。
    /// `&mut geometry` 与 `&document` 的借用拆分在门面内部完成（不相交字段），调用方不用自己拆。
    pub fn pick_at(
        &mut self,
        screen: (f32, f32),
        camera: &dyn Camera,
        filter: PickFilter,
    ) -> AppResult<Option<PickResult>> {
        KernelPicker::new(&mut self.geometry, self.document.doc()).pick(screen, camera, filter)
    }

    // ---------- 持久化 ----------

    /// 存盘（= [`store::save_document`] 的 zip 形态）。
    ///
    /// v2 容器：文档 + **图纸**一并落盘（`drawing.json` + `views/*.json`）。
    pub fn save(&self, path: &Path) -> AppResult<()> {
        store::save_document(path, self.document.doc(), self.document.drawing())
    }

    // ---------- 只读访问器 ----------

    /// 文档服务（含文档本体与图纸、撤销栈）。
    #[must_use]
    pub fn document(&self) -> &DocumentService {
        &self.document
    }

    /// 几何服务（`ShapeId ↔ ShapeHandle` 映射、内核门面）。
    #[must_use]
    pub fn geometry(&self) -> &GeometryService {
        &self.geometry
    }

    /// 互操作服务（图纸 ⇄ 中性二维结构）。
    #[must_use]
    pub fn interop(&self) -> &InteropService {
        &self.interop
    }

    /// 模块宿主。
    #[must_use]
    pub fn modules(&self) -> &ModuleHost {
        &self.modules
    }

    /// 会话概览。
    #[must_use]
    pub fn summary(&self) -> SessionSummary {
        SessionSummary {
            modules: self.modules.module_count(),
            elements: self.document.element_count(),
            geometry: self.document.doc().geometry_count(),
            layers: self.modules.summary().layers,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::{PropertyMap, Value};
    use core_sdk::{
        CreateRequest, Element, ElementFactory, GeometryOp, ModuleManifest, OpOutput, Registry,
        SdkResult, build_element,
    };
    use std::collections::BTreeSet;

    // M7b occt 腿钉子专用导入（默认腿不用 → 一并 cfg 门控，避免 unused 告警）。
    #[cfg(feature = "occt")]
    use crate::topo_naming::face_names;
    #[cfg(feature = "occt")]
    use core_doc::Annotation;
    #[cfg(feature = "occt")]
    use core_model::GeometryKey;
    #[cfg(feature = "occt")]
    use glam::DVec2;

    // ---------- 测试用最小模块（只依赖 core-sdk，不碰 example-beam）----------

    struct BoxFactory;

    impl ElementFactory for BoxFactory {
        fn category(&self) -> &str {
            "DEMO_BOX"
        }
        fn display_name(&self) -> &str {
            "示例盒"
        }
        fn create(&self, request: &CreateRequest) -> SdkResult<Element> {
            let mut e = build_element(self.category(), request, "示例盒");
            e.properties
                .set("dx", Value::Float(request.param_f64("dx").unwrap_or(10.0)));
            e.properties
                .set("dy", Value::Float(request.param_f64("dy").unwrap_or(20.0)));
            e.properties
                .set("dz", Value::Float(request.param_f64("dz").unwrap_or(30.0)));
            Ok(e)
        }
        fn default_params(&self) -> PropertyMap {
            PropertyMap::new("DEMO_BOX")
                .with("dx", Value::Float(10.0))
                .with("dy", Value::Float(20.0))
                .with("dz", Value::Float(30.0))
        }
    }

    /// 只发一步 `MakeBox` 的算子——内核门面已支持，端到端能跑通。
    struct BoxOp;

    impl GeometryOp for BoxOp {
        fn id(&self) -> &str {
            "test.demo.box"
        }
        fn display_name(&self) -> &str {
            "示例方盒"
        }
        fn apply(&self, input: &OpInput) -> SdkResult<OpOutput> {
            use core_sdk::{BuildOp, BuildStep};
            Ok(OpOutput::of(vec![
                BuildStep::new(BuildOp::MakeBox)
                    .with_f64("dx", input.f64_or("dx", 10.0))
                    .with_f64("dy", input.f64_or("dy", 20.0))
                    .with_f64("dz", input.f64_or("dz", 30.0)),
            ]))
        }
    }

    struct DemoModule;

    impl Module for DemoModule {
        fn manifest(&self) -> ModuleManifest {
            ModuleManifest::new("test.demo", "示例模块", "0.1.0")
        }
        fn register(&self, registry: &mut Registry) -> SdkResult<()> {
            registry.register_module(&self.manifest())?;
            registry.register_category("DEMO_BOX", "示例盒", "test.demo")?;
            registry.register_factory(Box::new(BoxFactory))?;
            registry.register_geometry_op(Box::new(BoxOp))?;
            Ok(())
        }
    }

    #[test]
    fn a_fresh_session_is_empty() {
        let s = CoreSession::new().unwrap();
        let summary = s.summary();
        assert_eq!(summary.modules, 0);
        assert_eq!(summary.elements, 0);
        assert_eq!(summary.geometry, 0);
        assert_eq!(s.document().element_count(), 0);
    }

    #[test]
    fn install_create_and_build_is_the_happy_path() {
        let mut s = CoreSession::new().unwrap();
        s.install_module(Box::new(DemoModule)).unwrap();

        let id = s.create_element("DEMO_BOX", "盒-1").unwrap();
        assert_eq!(s.document().element_count(), 1);

        let shape = s.build_geometry(id, "test.demo.box").unwrap();

        // 元素回填了几何引用
        let e = s.document().doc().element(id).unwrap();
        assert_eq!(e.geometry_ref, Some(shape));
        // 几何落进了文档
        assert!(s.document().doc().geometry_record(shape).is_some());

        let summary = s.summary();
        assert_eq!(summary.modules, 1);
        assert_eq!(summary.elements, 1);
        assert_eq!(summary.geometry, 1);
    }

    #[test]
    fn building_geometry_releases_its_handles() {
        let mut s = CoreSession::new().unwrap();
        s.install_module(Box::new(DemoModule)).unwrap();
        let id = s.create_element("DEMO_BOX", "").unwrap();
        s.build_geometry(id, "test.demo.box").unwrap();
        // 句柄可丢：执行完不留任何存活句柄
        assert_eq!(s.geometry().kernel().live_count(), 0);
    }

    #[test]
    fn creating_an_unregistered_category_is_refused() {
        let mut s = CoreSession::new().unwrap();
        assert!(s.create_element("NOPE", "x").is_err());
    }

    #[test]
    fn building_geometry_with_an_unknown_op_is_refused() {
        let mut s = CoreSession::new().unwrap();
        s.install_module(Box::new(DemoModule)).unwrap();
        let id = s.create_element("DEMO_BOX", "").unwrap();
        assert!(matches!(
            s.build_geometry(id, "no.such.op"),
            Err(AppError::Sdk(SdkError::GeometryOp(_)))
        ));
    }

    #[test]
    fn build_geometry_refuses_an_unknown_element() {
        let mut s = CoreSession::new().unwrap();
        s.install_module(Box::new(DemoModule)).unwrap();
        assert!(matches!(
            s.build_geometry(ElementId::new(), "test.demo.box"),
            Err(AppError::Model(core_model::ModelError::ElementNotFound(_)))
        ));
    }

    #[test]
    fn saving_and_reopening_a_session_keeps_the_document() {
        let mut s = CoreSession::new().unwrap();
        s.install_module(Box::new(DemoModule)).unwrap();
        let id = s.create_element("DEMO_BOX", "盒-A").unwrap();
        s.build_geometry(id, "test.demo.box").unwrap();

        let mut path = std::env::temp_dir();
        path.push(format!("session-roundtrip-{}.bimdoc", std::process::id()));
        s.save(&path).unwrap();

        let reopened = CoreSession::open(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(reopened.document().element_count(), 1);
        assert_eq!(reopened.document().doc().geometry_count(), 1);
        assert_eq!(reopened.document().doc(), s.document().doc());
    }

    // ---------- M6a-1/2：出图门面与容器 v2 钉子 ----------

    #[test]
    fn session_reopen_preserves_drawing() {
        // 直接打 session.rs from_document 时代 `with_parts(doc, Drawing::new())` 的根源：
        // save → open 后 Drawing 必须随容器恢复（含视图与实体）。
        let mut s = CoreSession::new().unwrap();
        s.install_module(Box::new(DemoModule)).unwrap();
        let id = s.create_element("DEMO_BOX", "盒-B").unwrap();
        s.build_geometry(id, "test.demo.box").unwrap();
        s.create_view("俯视", ViewOrientation::top()).unwrap();

        let before = s.document().drawing().clone();
        assert_eq!(before.view_count(), 1, "探针前提：存盘前有 1 个视图");
        assert!(
            !before
                .view(before.views.keys().next().copied().unwrap())
                .unwrap()
                .entities
                .is_empty()
        );

        let mut path = std::env::temp_dir();
        path.push(format!("session-drawing-{}.bimdoc", std::process::id()));
        s.save(&path).unwrap();

        let reopened = CoreSession::open(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(
            reopened.document().drawing(),
            &before,
            "重开后 Drawing 必须与存盘前相等（不是空壳）"
        );
        // 引用集合两侧一致（sync_drawing_registry 在 create_view 已对齐，open 后仍对齐）
        assert_eq!(
            reopened.document().doc().views(),
            s.document().doc().views()
        );
    }

    #[test]
    fn session_save_embeds_drawing_entry() {
        // save 后容器（zip 形态）必须含 drawing.json + views/ 段，
        // 且 open 后实体数与存盘前 Drawing 一致。
        let mut s = CoreSession::new().unwrap();
        s.install_module(Box::new(DemoModule)).unwrap();
        let id = s.create_element("DEMO_BOX", "盒-C").unwrap();
        s.build_geometry(id, "test.demo.box").unwrap();
        let view_id = s.create_view("俯视", ViewOrientation::top()).unwrap();
        let entity_count = s.document().drawing().view(view_id).unwrap().entities.len();
        assert!(entity_count > 0, "探针前提：mock 腿出图应有图元");

        let mut path = std::env::temp_dir();
        path.push(format!("session-embed-{}.bimdoc", std::process::id()));
        s.save(&path).unwrap();

        let file = std::fs::File::open(&path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        assert!(
            archive.by_name(store::DRAWING_ENTRY).is_ok(),
            "容器必须含 {}",
            store::DRAWING_ENTRY
        );
        let mut view_entries = 0;
        for i in 0..archive.len() {
            let e = archive.by_index(i).unwrap();
            if e.name().starts_with(store::VIEWS_DIR) && e.name().ends_with(".json") {
                view_entries += 1;
            }
        }
        assert_eq!(view_entries, 1, "1 个视图应落 1 个 views/*.json");
        drop(archive);

        let reopened = CoreSession::open(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let back_view = reopened.document().drawing().view(view_id).unwrap();
        assert_eq!(
            back_view.entities.len(),
            entity_count,
            "open 后实体数必须与存盘前 Drawing 一致"
        );
    }

    #[test]
    fn mock_create_view_yields_entities() {
        // mock 腿出图：俯视 + ViewOrientation::top() → entities 非空、视图最新、id 已登记；
        // 空投影（无形状可投）不报错——合法状态，不吞成 Err。
        let mut s = CoreSession::new().unwrap();
        s.install_module(Box::new(DemoModule)).unwrap();
        let id = s.create_element("DEMO_BOX", "盒-D").unwrap();
        let shape = s.build_geometry(id, "test.demo.box").unwrap();

        let view_id = s.create_view("俯视", ViewOrientation::top()).unwrap();
        let view = s.document().drawing().view(view_id).unwrap();
        assert!(
            !view.entities.is_empty(),
            "mock 腿 create_view 必须产出图元（理想化正面视）"
        );
        assert!(view.up_to_date, "初生成后视图应为最新");
        assert!(
            s.document().doc().views().contains(&view_id),
            "视图 id 必须登记进 Document"
        );

        // 诊断门面：单形状投影返回 (图元, 警告)，不写回
        let view_copy = view.clone();
        let standard = s.document().drawing().standard.clone();
        let (entities, warnings) = s.project_shape(shape, &view_copy, &standard).unwrap();
        assert!(!entities.is_empty(), "单形状投影同样产出图元");
        let before = s.document().drawing().view(view_id).unwrap().entities.len();
        let _ = warnings;
        assert_eq!(
            s.document().drawing().view(view_id).unwrap().entities.len(),
            before,
            "project_shape 是诊断口，不得写回视图"
        );

        // 空文档出图：0 个可投形状 → Ok（空视图），不报错
        let mut empty = CoreSession::new().unwrap();
        let empty_id = empty.create_view("空投影", ViewOrientation::top()).unwrap();
        assert!(
            empty
                .document()
                .drawing()
                .view(empty_id)
                .unwrap()
                .entities
                .is_empty(),
            "空文档出图得到空视图（合法状态，不报错）"
        );
    }

    #[test]
    fn regenerate_view_flips_up_to_date() {
        // 标脏 → regenerate → up_to_date 翻回 true，written 与警告如实返回。
        let mut s = CoreSession::new().unwrap();
        s.install_module(Box::new(DemoModule)).unwrap();
        let id = s.create_element("DEMO_BOX", "盒-E").unwrap();
        s.build_geometry(id, "test.demo.box").unwrap();
        let view_id = s.create_view("俯视", ViewOrientation::top()).unwrap();

        let mut stale = BTreeSet::new();
        stale.insert(view_id);
        let marked = s.document.drawing_mut().mark_views_stale(&stale);
        assert_eq!(marked, 1, "标脏应命中 1 个视图");
        assert!(
            !s.document().drawing().view(view_id).unwrap().up_to_date,
            "标脏后 up_to_date 应为 false"
        );

        let standard = s.document().drawing().standard.clone();
        let (written, _warnings) = s.regenerate_view(view_id, &standard).unwrap();
        assert!(written > 0, "重投应写回图元，实得 {written}");
        assert!(
            s.document().drawing().view(view_id).unwrap().up_to_date,
            "重投后 up_to_date 应翻回 true"
        );

        // 未登记的视图重投必须报错（DocError::ViewNotFound），不许静默
        assert!(s.regenerate_view(ViewId::new(), &standard).is_err());
    }

    // ---------- M6b-1：真腿装配（occt feature 门控，默认腿零影响）----------

    /// M6b-1 验收：occt feature 下 `with_real_kernel` 可用且**真是真实内核**；
    /// `open_real` 与 mock 版 `open` 同一容器装配，几何服务走真实后端。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn with_real_kernel_wires_a_real_backend() {
        let mut s = CoreSession::with_real_kernel().expect("occt 腿下 with_real_kernel 必须成功");
        assert_eq!(s.geometry().backend_name(), "occt-wasm");
        assert!(
            s.geometry().backend_kind().is_real(),
            "真腿会话必须跑真实内核，不许在占位后端上假绿"
        );

        // open 真腿变体：建模 → 存盘 → open_real → 文档往返 + 仍是真实内核
        s.install_module(Box::new(DemoModule)).unwrap();
        let id = s.create_element("DEMO_BOX", "盒-真").unwrap();
        s.build_geometry(id, "test.demo.box").unwrap();

        let mut path = std::env::temp_dir();
        path.push(format!("session-real-{}.bimdoc", std::process::id()));
        s.save(&path).unwrap();
        let reopened = CoreSession::open_real(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(reopened.geometry().backend_name(), "occt-wasm");
        assert_eq!(reopened.document().doc(), s.document().doc());
    }

    // ---------- M6b-2：rebuild_geometry 关联更新闭环（update_after_commit 首个生产调用方）----------

    #[test]
    fn rebuild_geometry_empty_input_is_noop() {
        // update_service 既有红线的门面级复验：会话里没有任何视图绑定该形状
        // （affected 为空）→ 报告除 changed_shapes 回显外全 0、Drawing 逐字节不变。
        let mut s = CoreSession::new().unwrap();
        s.install_module(Box::new(DemoModule)).unwrap();
        let id = s.create_element("DEMO_BOX", "盒-F").unwrap();
        let shape = s.build_geometry(id, "test.demo.box").unwrap();
        let old_text = s
            .document()
            .doc()
            .geometry_record(shape)
            .unwrap()
            .text
            .clone();
        let params = PropertyMap::new("DEMO_BOX")
            .with("dx", Value::Float(60.0))
            .with("dy", Value::Float(20.0))
            .with("dz", Value::Float(30.0));

        let before = s.document().drawing().clone();
        let report = s.rebuild_geometry(id, "test.demo.box", params).unwrap();

        assert_eq!(
            report.changed_shapes,
            vec![shape],
            "changed_shapes 应回显该形状"
        );
        assert_eq!(report.evolution_resolved, 0, "无绑定键 → 无演化解析");
        assert_eq!(report.fingerprint_resolved, 0, "无绑定键 → 无指纹解析");
        assert_eq!(report.reprojected_entities, 0, "无绑定视图 → 无重投");
        assert_eq!(report.remeasured_annotations, 0);
        assert_eq!(report.stale_views, 0, "无绑定视图 → 无 stale");
        assert_eq!(
            s.document().drawing(),
            &before,
            "无绑定视图时 rebuild 不得改图纸任何字节"
        );
        // 构件身份不变：geometry_ref 复用原 ShapeId，几何记录已换成新配方结果
        let e = s.document().doc().element(id).unwrap();
        assert_eq!(
            e.geometry_ref,
            Some(shape),
            "geometry_ref 必须复用原 ShapeId"
        );
        let new_text = s
            .document()
            .doc()
            .geometry_record(shape)
            .unwrap()
            .text
            .clone();
        assert_ne!(new_text, old_text, "rebuild 后几何记录必须是新配方结果");
    }

    #[test]
    fn rebuild_geometry_marks_stale_views() {
        // mark_views_stale 接线钉：改参数 rebuild → 绑定该形状的视图
        // up_to_date=false（等待 regenerate），定向重投照常产出。
        let mut s = CoreSession::new().unwrap();
        s.install_module(Box::new(DemoModule)).unwrap();
        let id = s.create_element("DEMO_BOX", "盒-G").unwrap();
        let shape = s.build_geometry(id, "test.demo.box").unwrap();
        let view_id = s.create_view("俯视", ViewOrientation::top()).unwrap();
        assert!(
            s.document().drawing().view(view_id).unwrap().up_to_date,
            "探针前提：初生成后视图应最新"
        );

        let params = PropertyMap::new("DEMO_BOX")
            .with("dx", Value::Float(60.0))
            .with("dy", Value::Float(20.0))
            .with("dz", Value::Float(30.0));
        let report = s.rebuild_geometry(id, "test.demo.box", params).unwrap();

        assert_eq!(
            report.stale_views, 1,
            "绑定该形状的视图必须被标脏（mark_views_stale 接线）"
        );
        assert!(
            !s.document().drawing().view(view_id).unwrap().up_to_date,
            "rebuild 后视图应 up_to_date=false，等待 regenerate"
        );
        assert!(report.reprojected_entities > 0, "定向重投必须有产出");
        assert_eq!(report.changed_shapes, vec![shape]);
        let e = s.document().doc().element(id).unwrap();
        assert_eq!(e.geometry_ref, Some(shape), "geometry_ref 复用原 ShapeId");
    }

    #[test]
    fn rebuild_geometry_no_history_is_safe() {
        // evolution_align.rs 指纹退避分支的复验：DemoModule 配方是单步 MakeBox
        //（无布尔演化）→ history_of 为 None → 键解析走指纹退避——必须不 panic、
        // 报告合法、标脏与重投照常。
        let mut s = CoreSession::new().unwrap();
        s.install_module(Box::new(DemoModule)).unwrap();
        let id = s.create_element("DEMO_BOX", "盒-H").unwrap();
        let shape = s.build_geometry(id, "test.demo.box").unwrap();
        assert!(
            s.geometry().history_of(shape).is_none(),
            "探针前提：无布尔配方不得登记带历史记录"
        );
        let view_id = s.create_view("俯视", ViewOrientation::top()).unwrap();

        let params = PropertyMap::new("DEMO_BOX")
            .with("dx", Value::Float(40.0))
            .with("dy", Value::Float(20.0))
            .with("dz", Value::Float(30.0));
        let report = s.rebuild_geometry(id, "test.demo.box", params).unwrap();

        assert_eq!(report.stale_views, 1, "指纹退避分支下标脏照常工作");
        assert!(
            report.reprojected_entities > 0,
            "指纹退避分支下重投照常工作"
        );
        assert!(
            !s.document().drawing().view(view_id).unwrap().up_to_date,
            "标脏照常（不 panic 是本钉核心判据）"
        );
    }

    // ---------- M7b：carve_geometry 演化优先分支替代验证路径（M6b-3 降级项）----------

    /// M7b occt 腿钉子公共装配：真实内核 + DemoModule + 造盒 +
    /// **绑全部旧面**的标注（每面一条 `face_named` 键——只有 face_named 键
    /// 参与演化 / 指纹解析；投影图元的 `whole` 键在 resolve_delta 里被跳过，
    /// 不承担可观测）。返回 `(会话, 元素, 形状, 初始绑定键序表, 视图 id)`，
    /// 键序表与视图标注同序（remap 是原地改绑，顺序不变）。
    #[cfg(feature = "occt")]
    fn carved_session(what: &str) -> (CoreSession, ElementId, ShapeId, Vec<GeometryKey>, ViewId) {
        let mut s = CoreSession::with_real_kernel().expect("occt 腿下 with_real_kernel 必须成功");
        assert!(
            s.geometry().backend_kind().is_real(),
            "{what} 必须跑真实内核，不许在占位后端上假绿"
        );
        s.install_module(Box::new(DemoModule)).unwrap();
        let id = s.create_element("DEMO_BOX", what).unwrap();
        let shape = s.build_geometry(id, "test.demo.box").unwrap();

        // 旧几何面名表（从文档装载，跨会话稳定名）→ 每面绑一条标注进正立面视图。
        let table = {
            let kernel = s.geometry.kernel_mut();
            face_names(kernel, s.document.doc(), shape).expect("旧几何面名表必须可采集（occt 腿）")
        };
        assert!(!table.is_empty(), "探针前提：长方体至少有一个面");
        let mut view =
            View::new("正立面", ViewKind::Elevation).with_orientation(ViewOrientation::front());
        let mut keys = Vec::with_capacity(table.len());
        for (index, name) in table {
            let key = GeometryKey::face_named(shape, index, name);
            view.push_annotation(
                Annotation::linear(DVec2::ZERO, DVec2::new(10.0, 0.0)).bound_to(key.clone()),
            );
            keys.push(key);
        }
        let view_id = s.document.drawing_mut().add_view(view);
        s.document.drawing_mut().rebuild_index();
        (s, id, shape, keys, view_id)
    }

    /// 开孔刀具参数：5×5×15 小盒（原点对齐，从 10×20×30 毛坯切掉一个角——
    /// 3 个旧面被改、3 个旧面保持）。carve / mock 钉子共用。
    fn corner_cut_tool_params() -> PropertyMap {
        PropertyMap::new("DEMO_BOX")
            .with("dx", Value::Float(5.0))
            .with("dy", Value::Float(5.0))
            .with("dz", Value::Float(15.0))
    }

    /// M7b 钉 1（occt 腿 + 对照）：carve 让**旧几何句柄直接参与布尔** → 演化记录的
    /// input_names 含旧几何面名 → 演化优先分支**真实命中**；对照场景：同装配走
    /// rebuild（毛坯从零重造——单步配方无演化记录，布尔配方的 input_names 也是
    /// 新配方毛坯面名，与旧键对不上）→ `evolution_resolved==0`、指纹退避接管。
    /// 两钉并排，把「为什么必须 carve 才能消除 M6b-3 降级」钉死。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn carve_geometry_hits_evolution_branch_for_real() {
        // ---- carve 场景：旧几何参与布尔 → 演化分支真实命中 ----
        let (mut s, id, shape, keys, _view) = carved_session("盒-carve");
        let report = s
            .carve_geometry(id, "test.demo.box", corner_cut_tool_params())
            .unwrap();

        assert_eq!(
            report.changed_shapes,
            vec![shape],
            "changed_shapes 回显入参"
        );
        assert!(
            report.evolution_resolved >= 1,
            "carve 必须真实命中演化优先分支；实得 evolution_resolved={}",
            report.evolution_resolved
        );
        assert!(
            report.evolution_resolved + report.fingerprint_resolved <= keys.len(),
            "演化 + 指纹解析的键数不得超过绑定键总数（{}）；实得 {}+{}",
            keys.len(),
            report.evolution_resolved,
            report.fingerprint_resolved
        );
        assert_eq!(
            s.geometry().kernel().live_count(),
            0,
            "carve 后不得残留任何存活句柄（旧 / 刀具 / 新句柄全部释放）"
        );

        // ---- 对照场景：同装配走 rebuild → 演化分支不命中、指纹退避接管 ----
        let (mut s2, id2, _shape2, _keys2, _view2) = carved_session("盒-rebuild");
        let params = PropertyMap::new("DEMO_BOX")
            .with("dx", Value::Float(10.0))
            .with("dy", Value::Float(20.0))
            .with("dz", Value::Float(30.0));
        let report2 = s2.rebuild_geometry(id2, "test.demo.box", params).unwrap();
        assert_eq!(
            report2.evolution_resolved, 0,
            "rebuild（毛坯从零重造）的演化记录 input_names 与旧键对不上，\
             演化分支不得命中——这正是 M6b-3 降级的根因"
        );
        assert!(
            report2.fingerprint_resolved >= 1,
            "rebuild 必须由指纹退避接管"
        );
    }

    /// M7b 钉 2（occt 腿）：绑定旧面的标注在 carve 后**必须 remap 到演化输出面**。
    /// 逐键可观测判据：演化命中的键经 `key_for_output` remap 到**输出面稳定名**
    /// （恒 `face_named(shape, 0, 输出名)`，被改面的输出名必异于输入名）→
    /// 「topo_name 已改变的标注数 == evolution_resolved」严格对账；指纹兜底
    /// remap 到同名面（名字不变）；remap 后键的 shape 仍是原 ShapeId（构件身份不变）。
    #[test]
    #[cfg(feature = "occt")]
    #[cfg_attr(
        debug_assertions,
        ignore = "真实内核用例在 debug 下 wasmtime 编译过慢，请用 --release 运行"
    )]
    fn carve_geometry_updates_bound_annotations() {
        let (mut s, id, shape, keys, view_id) = carved_session("盒-标注");
        let report = s
            .carve_geometry(id, "test.demo.box", corner_cut_tool_params())
            .unwrap();
        let evolution = report.evolution_resolved;
        assert!(
            evolution >= 1,
            "探针前提：演化分支必须真实命中；实得 {evolution}"
        );

        // 逐键对账：topo_name 改变 ⟺ 演化命中（指纹兜底 remap 到同名面，名字不变）。
        let mut remapped = 0usize;
        {
            let view = s.document.drawing().view(view_id).unwrap();
            assert_eq!(
                view.annotations.len(),
                keys.len(),
                "标注数不得增减（remap 是原地改绑）"
            );
            for (old, ann) in keys.iter().zip(view.annotations.iter()) {
                let cur = ann.bound_to.as_ref().expect("绑定标注的 bound_to 不得丢");
                assert_eq!(cur.shape, shape, "remap 后键必须仍指向原 ShapeId");
                if cur.topo_name() != old.topo_name() {
                    remapped += 1;
                }
            }
        }
        assert!(
            remapped >= 1,
            "演化命中的旧面必须 remap 到输出面稳定名（topo_name 改变）"
        );
        assert_eq!(
            remapped, evolution,
            "topo_name 改变的标注数必须与 evolution_resolved 严格对账；\
             实得 remapped={remapped}, evolution_resolved={evolution}"
        );
    }

    /// M7b 钉 3（默认腿 mock）：无绑定键时 carve **安全**——占位内核不支持带历史
    /// 布尔（`Unsupported`）→ 回退普通 cut，回退说明并入 warnings（绝不静默）；
    /// 报告合法全 0 类、不 panic；geometry_ref 复用 ShapeId、几何记录换成切削
    /// 结果、句柄全部释放。
    #[test]
    fn carve_geometry_without_bound_keys_is_safe() {
        let mut s = CoreSession::new().unwrap();
        s.install_module(Box::new(DemoModule)).unwrap();
        let id = s.create_element("DEMO_BOX", "盒-无绑定").unwrap();
        let shape = s.build_geometry(id, "test.demo.box").unwrap();
        let old_text = s
            .document()
            .doc()
            .geometry_record(shape)
            .unwrap()
            .text
            .clone();

        let report = s
            .carve_geometry(id, "test.demo.box", corner_cut_tool_params())
            .unwrap();

        // 全 0 类：无绑定键 → 无解析、无重投、无标脏（update_after_commit 红线语义）
        assert_eq!(report.evolution_resolved, 0, "无绑定键不得有演化解析");
        assert_eq!(report.fingerprint_resolved, 0, "无绑定键不得有指纹解析");
        assert_eq!(report.reprojected_entities, 0, "无绑定视图不得重投");
        assert_eq!(report.remeasured_annotations, 0);
        assert_eq!(report.stale_views, 0);
        assert_eq!(
            report.changed_shapes,
            vec![shape],
            "changed_shapes 回显入参"
        );
        // mock 回退可观测：warnings 必须含回退说明（绝不静默）
        assert!(
            !report.warnings.is_empty(),
            "mock 回退说明必须并入 warnings（绝不静默）"
        );
        // 构件身份不变：geometry_ref 复用 ShapeId，几何记录已换成切削结果
        let e = s.document().doc().element(id).unwrap();
        assert_eq!(e.geometry_ref, Some(shape), "geometry_ref 复用原 ShapeId");
        let new_text = s
            .document()
            .doc()
            .geometry_record(shape)
            .unwrap()
            .text
            .clone();
        assert_ne!(new_text, old_text, "carve 后几何记录必须是切削结果");
        // 句柄全部释放（旧缓存句柄 + 刀具句柄 + 新句柄）
        assert_eq!(s.geometry().kernel().live_count(), 0);
    }
}

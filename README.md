# 轻量化 BIM 内核平台（Core）

本仓库是**轻量化 BIM 内核平台一阶段（Core）**的 Rust 工程骨架。产品定位、范围与验收标准见同目录上层的《轻量化 BIM 内核平台_立项方案》，工程结构、接口与落地节奏见《轻量化 BIM 内核平台_软件设计文档》。**本 README 只讲"怎么把它跑起来、改哪里"。**

> 一句话定位：交付一个具备**六大基础能力**的通用 Core —— ① B-Rep 信息存储管理、② HLR 显示、③ CAD 平面图纸编辑、④ 通用数据导入导出、⑤ 几何构造、⑥ 二次开发 SDK；专业功能（强电电气 / 暖通 / 给排水 / 弱电桥架 / 电缆隧道 / 电缆排管 / 电缆层支架 …）全部以 SDK 模块方式在**第二阶段**挂接。

---

## 快速开始

```bash
cargo build                                  # 常规构建（当前无需 C++ 工具链）
cargo fmt --all --check                      # 格式门禁（设计文档 2.6 / ci.yml lint）
cargo test                                   # 全部单元测试
cargo clippy --workspace --all-targets -- -D warnings
cargo xtask check-deps                       # 校验依赖方向（架构门禁）

# M0 真实内核验证（加载 occt-wasm 的 WASM，慢；务必加 --release）
cargo test -p core-kernel --features occt --release
```

依赖走本机已配置的 crates.io 镜像源，无需额外设置。

> **`--release` 不是可选项**：`occt-wasm` 启动要解压一个 brotli 压缩的 ~21 MB WASM 并交给 wasmtime 48 JIT，
> debug 构建下该路径有 **~100× 惩罚**（同为几百毫秒的调用会变成几十秒）。默认 workspace 门禁（`cargo test`）
> **不加载 WASM**，因为真实后端是 feature `occt` 门控的。

---

## 门禁清单（合并前必过，与 `.github/workflows/ci.yml` 一一对应）

| 门禁 | 命令 | CI job |
|---|---|---|
| 格式 | `cargo fmt --all --check` | `lint` |
| 静态检查 | `cargo clippy --workspace --all-targets -- -D warnings` | `lint` |
| 依赖方向 | `cargo xtask check-deps` | `deps` |
| 许可证 | `cargo deny check licenses bans sources`（`deny.toml`） | `deps` |
| 单元测试 | `cargo test --workspace` | `unit` |

`golden` / `bench` / `license` 三个 job 在 CI 里标了 `continue-on-error`，随 M5 / M3 / 发布启用。

---

## 目录结构

```
.
├─ Cargo.toml                  # workspace 根：成员 + 依赖目录 + lint 门禁
├─ rust-toolchain.toml         # 钉死工具链
├─ deny.toml                   # cargo-deny：许可证白名单 + 禁用依赖
├─ .github/workflows/ci.yml    # 门禁（见设计文档 2.6）
├─ xtask/                      # 构建/校验脚本（Rust 写脚本）
├─ crates/
│  ├─ core-kernel/             # 第 1 层：内核适配（唯一碰 occt-wasm 的地方）
│  ├─ core-model/              # 第 2 层：数据模型（纯数据，零几何知识）
│  ├─ core-doc/                # 第 3 层：二维图纸 + 关联更新
│  ├─ core-interop/            # 第 1.5 层：四格式互操作
│  ├─ core-sdk/                # 第 4 层：SDK 契约（只定义，不实现）
│  ├─ core-app/                # 第 4 层：应用服务（用例编排）
│  └─ core-testkit/            # 测试夹具与黄金样本
├─ modules/
│  ├─ example-beam/            # 最小示例模块（用于给 SDK 接口"体检"）
│  ├─ cable-tunnel/            # 电力侧标杆模块（M7c：矩形隧道段 / 电缆排管段）
│  └─ hvac-duct/               # 建筑侧暖通模块（M7：矩形风管段 / 变径管 Loft）
├─ apps/
│  ├─ desktop/                 # 桌面端壳（egui + wgpu，见 1.6）
│  ├─ server/                  # 服务端壳（无头）
│  └─ web/                     # 浏览器端（TS + npm，非 cargo 成员）
├─ vendor/
│  └─ occt-wasm/               # occt-wasm 4.0.0 本地 fork（P0 修复；见「M1」节，非 workspace 成员）
└─ testdata/                   # STEP / DXF / IFC / glTF 测试样本
```

> **`vendor/occt-wasm` 是什么**：上游 4.0.0 的 linker 缺陷（见「M1」节）导致内核无法实例化，
> 我们把该 crate 源码放进工程内、补一行 `define_unknown_imports_as_traps`，再用根 `Cargo.toml`
> 的 `[patch.crates-io]` 指向它。**升级 `occt-wasm` 时须重新评估是否可移除本补丁。**

---

## 依赖方向（架构铁律，`cargo xtask check-deps` 会拦）

| crate | 职责 | 允许依赖 | **禁止依赖** |
|---|---|---|---|
| `core-kernel` | 包装 occt-wasm；句柄、错误、HLR、网格化 | `occt-wasm`、`thiserror`、`glam` | 其它 `core-*` |
| `core-model` | 文档 / 元素 / 属性 / 事务 / 基准元素（纯数据） | `serde`、`uuid`、`thiserror` | `occt-wasm`、其它 `core-*` |
| `core-doc` | 二维实体、标注、图纸、关联更新 | `core-model`、`kurbo`、`glam` | `occt-wasm` |
| `core-interop` | 四格式读写 | `core-kernel`、`core-model`、dxf/gltf/ifc 库 | `core-doc` |
| `core-sdk` | trait 定义 + 注册表（**只定义，不实现**） | `core-model` | 具体模块 |
| `core-app` | 用例编排 | 以上全部 | UI |
| `apps/*` | 界面 / 进程入口 | `core-app` | — |
| `modules/*` | 专业功能 | `core-sdk`（**只允许这一个**） | 其它 `core-*` |

**自检法**：画箭头图，若既有向上又有向下的箭头，就是循环依赖。

---

## 与设计文档的差异（务必知悉）

1. **`occt-wasm` 版本**：设计文档 2.3 曾写作 `v3`，实测最新为 **`v4.0.0`**，根 `Cargo.toml` 取 `"4"`（设计文档 2.3 已同步订正）。
   **M0 已核对** v4 的实际 API 与设计文档 3.4 / 附录 C 的假设：`project_edges` 的参数与返回、`ProjectionData`、
   `make_*`、`xcaf_*`、`get_sub_shapes` / `sub_shape_hashes` / `curve_type` **与文档逐条一致，无需订正**。
2. **内核接入方式**：`core-kernel` 同时提供两套 `KernelBackend` 实现——
   `MockBackend`（纯 Rust，默认开，供下游与测试推进）与 **`OcctBackend`（真实 `occt-wasm` 后端）**。
   真实后端由 feature 门控：`cargo ... --features occt` 才编译；默认 `cargo build` **不碰 WASM，保持轻量无 C++ 工具链依赖**。
   M0 曾称"已完成接线"，该结论**已于 2026-09-23 订正**（原 M0 报告的绿是**假绿**：接线代码在，
   但运行时内核装不起来）。**当晚 P0 已修复**——`occt-wasm` 4.0.0 的 linker 缺陷由
   **vendor + `[patch.crates-io]`** 解决，真实后端现已可实例化（见「M1」节的「✅ 已修复」）。
3. **重依赖（wgpu / egui / eframe / dxf / gltf / ifc-lite-core）**：登记在 `[workspace.dependencies]` 版本目录中。
   **M4 已启用三个互操作库**：`core-interop` 以 `xxx.workspace = true` 引用 `dxf` / `gltf` / `ifc-lite-core`
   （版本只在根 `Cargo.toml` 维护一处，避免走散）。
   `apps/desktop` 仍把 `egui` / `egui-wgpu` / `eframe` 声明为 **optional**（默认 feature 不开），
   因此 `cargo build --workspace` 仍**不编译**它们，骨架构建保持轻量。
   ⚠ **订正（M7 时点，M8 已解决）**：M7 时点 `--features gui` 只负责**编译**
   eframe/egui/egui-wgpu 依赖，`main.rs` 里没有任何窗口代码（开了也不弹窗）。
   **M8 起点已实现第一版真实 GUI**：`cargo run -p bim-desktop --features gui`
   弹出图形窗口——三维视口（轨道相机 + 面着色 + 地面网格/坐标轴）、单击拾取
   （复用 §6.6 `KernelPicker` 链路，命中面高亮）、属性面板、构件列表（三个专业
   模块的算子按钮）、操作日志。实现见 `apps/desktop/src/gui.rs`（egui 0.29，
   CPU 画家算法，零自定义 shader；`--features occt` 时真内核）。控制台端到端
   演示仍保留（不带 `gui` feature 的默认形态）。
4. **apps/web**：非 cargo 成员，仅占位（见 `apps/web/README.md`）。

---

## 仓库自动化（`cargo xtask`）

| 命令 | 作用 |
|---|---|
| `cargo xtask check-deps` | **架构门禁**：逐条对照上表校验内部依赖方向，并禁止 `occt-wasm` 泄漏到内核适配层之外。违规即非零退出 |
| `cargo xtask fmt` | 格式门禁（等价 `cargo fmt --all --check`） |
| `cargo xtask test` | 全量测试 |
| `cargo xtask clippy` | clippy（`-D warnings`） |
| `cargo xtask gen-samples` | 生成黄金样本（**依赖 M1 真实内核，当前未启用**） |
| `cargo xtask golden` / `bench` / `license-check` | 出图黄金样本 / 性能回归 / LGPL 声明随附（**分别排 M5 / M3 / 发布前，当前未启用**） |

> `cargo xtask` 由根目录 `.cargo/config.toml` 的别名提供（等价于 `cargo run -q -p xtask -- <task>`）。
> 替身写法：`cargo run -q -p xtask -- check-deps`。
>
> **"未启用"的任务不会静默成功**：它们会打印"该做什么、何时启用"再以 0 退出。
> 这样 CI 上看到的红/绿始终有信息量——静默成功会让人误以为那一项已经过了。

---

## M0 技术验证结论（⚠️ 曾订正：原结论是假绿；2026-09-23 晚已修复 P0 并复验）

> **订正说明（2026-09-23）**：本节原先写"M0 已完成"，那个结论**不成立**。
> 原 `cargo test -p core-kernel --features occt --release` 报 `ok. 29 passed`，
> 但那 29 条里有 **14 条 occt 用例是静默跳过的**——用例开头写的是
> `let Some(b) = try_backend() else { return }`，内核建不起来就直接 `return`，
> 计数上仍算 **passed**。即**内核从未真正跑起来过**，判据 ①②③⑤ 均未被真正验证。
> 教训一句话：**"跳过"必须与"通过"在报告里可区分。**

> **P0 修复与复验（2026-09-23 晚）**：上面那个"内核完全装不起来"的阻塞**已解决**——
> 方案是 **vendor + `[patch.crates-io]`**（把 `occt-wasm` 4.0.0 源码放进
> `src/vendor/occt-wasm/`，仅在其 `kernel.rs` 补一行
> `Linker::define_unknown_imports_as_traps(&module)`）。详见下一节「M1」的
> **「✅ 已修复」**小节。
>
> **随后适配层三条残留红也一并修掉（2026-09-23 深夜）**：判据 ①②⑤ 的根因都不在内核，
> 而在适配层的"口径"（空分支判定 / 长度单位 / 哈希域），现已修正并各自加上了会报红的钉子。
> 至此 **M0 五项判据中可验的 ①②③⑤ 全部通过**（④ 属 M4）。详见文末「M0 判据复验」。

设计文档 10.1 给 M0 排了五项判据。**原始**实测命令与（现已作废的）结论如下，
保留在此仅作对照——真实状态见下一节：

```bash
cargo test -p core-kernel --features occt --release
# 报告: ok. 29 passed; 0 failed   ← 但其中 14 条 occt 用例是静默跳过，非真通过
```

| # | M0 判据 | 原报告结论 | 订正后（P0 修复前） | P0 修复后复验 | 对应用例 |
|---|---|---|---|---|---|
| ① | **精确圆判定**：圆孔板投影后能判出 `Circle` 而非折线 | ✅ | ❌ 未验证 | ✅ **真通过**（内核本就判出半径 15 的解析圆；红的是适配层对空分支的处理） | `projection_of_holed_plate_yields_analytic_circle`（板 100×60×10 钻 ⌀30 通孔，俯视投影，断言 `CurveKind::Circle` 且半径 ≈15±0.05） |
| ② | **剖面投影**：能取到截面而非空集 | ✅ | ❌ 未验证 | ✅ **真通过**（同一空分支缺陷，一并修正） | `section_plane_of_holed_plate_contains_a_circle` |
| ③ | **WASM 加载**：wasmtime 能原生加载并报告后端名 | ✅ | ❌ 未验证（实测加载失败） | ✅ **真通过** | `wasm_loads_and_reports_backend_name`、`a_facade_new_returns_the_real_backend` |
| ④ | **IFC 组件可用** | ⊘ 未覆盖 | ⊘ 未覆盖 | ⊘ 未覆盖 | 归属 `core-interop`，排在 **M4**（`occt-wasm` 不覆盖 IFC，需 ifc-lite / bimifc） |
| ⑤ | **拾取哈希映射**：`mesh_shape` 的 `face_hash` 能映回面句柄 | ✅ | ❌ 未验证 | ✅ **真通过**（"两个域"是误判：两边**同域同序**，只是早先用错了 `hash_upper_bound`） | `face_group_hashes_map_back_to_face_handles` |

**→ 结论（P0 修复前）**：M0 的"真实内核可用"这一前置条件实际未满足，M0 不能算完成，M1 因此受阻。
**→ 结论（现状）**：真实内核**确实装起来了**（三维佐证：探针实测几何 + 两道守护门禁转绿 + 全量 457 绿）；
适配层三处口径缺陷修正后，**M0 可验判据 ①②③⑤ 全部通过**（④ 属 M4）。

**验证中确认的四条硬事实（已回写设计文档）**：

1. **内核句柄无法从裸 `u32` 重建**：`occt_wasm::ShapeHandle` 的字段是 `pub(crate)`。因此 `OcctBackend` 自带一个
   **槽位 arena**（`slots: Vec<Option<occt_wasm::ShapeHandle>>` + `free` 空闲栈），对外只暴露自有 `ShapeHandle(idx)`。
   副产品：内核本没有 `is_alive`，arena **替它补上了** `is_alive` / `live_count` / 重复释放检测（重复释放返回 `InvalidHandle`）。
2. **内核 arena 不会自动回收**：`project_edges` 会产生一批临时 compound。实现用 `checkpoint()` → 投影 → `release_since(mark)`
   把临时对象圈住，避免长跑泄漏。
3. **许可证口径**：crate 元数据是 `MIT OR Apache-2.0`，但**内嵌的 WASM 是 LGPL-2.1-only**。
   设计文档 356 / 368 行记录的正是这个组合（不是矛盾）。**动态加载 WASM 形态满足 LGPL 的"可替换"要求**。
4. **一个内核实例很重，测试里必须共享**：`OcctKernel::new()` 要解压 ~21 MB WASM 再 JIT。
   起初每个用例各建一个内核，默认并行测试下直接 **OOM**——
   报的是 `memory allocation of 1048576 bytes failed`，进程以 `0xc0000409` 中止，
   **日志里没有任何 `FAILED` 行**，极易被误读成代码回归。改为进程内单实例（`OnceLock<Mutex<_>>`）
   后：不再 OOM，且耗时 `91.72s → 18.67s`（内核只装一次）。
   → **M1 的"每线程复用一个内核"要真落地，别退化成每调用一次建一个。**

**尚未做（属 M1 及其后，不是 M0 欠账）**：真实后端替换默认的 `MockBackend`、线程本地内核池硬化、
`is_alive` 语义与池的协作、HLR 结果 → 可编辑二维实体的接线。

---

## M1：内核适配（✅ 代码已完成 / ✅ 真实后端已可实例化）

M1 的定义（设计文档 10.1）：`core-kernel` 完成 —— 能建基元、布尔、网格化、HLR；错误 / 句柄 / 容差就绪。
落地口径（README 上表）：用真实后端替换默认的 `MockBackend`；落地句柄生命周期与线程本地内核池。

### 已完成（与内核是否可用无关的工程硬化）

| # | 事项 | 产出 |
|---|---|---|
| 1 | **共享槽位 arena** | `core-kernel/src/arena.rs`：通用 `SlotArena<T>`，两后端共用槽位语义（alloc / reuse / `is_alive` / `live_count` / 重复释放检测）。5 条单测 |
| 2 | **后端可信度** | `BackendKind::{Real, Mock}` + `KernelBackend::kind()`（默认 `Real`，mock **必须**覆写）。杜绝"mock 冒充真内核"的静默降级 |
| 3 | **BREP 导入导出** | trait 增 `import_brep` / `export_brep` / `volume` / `live_handles`（`live_handles` 是**必填**，不给默认空实现——否则"回收临时物"会变成谎言） |
| 4 | **临时物统一清理** | `Kernel::scope(keep, f)` + `release_many`（设计文档 3.7）：跑一段闭包，自动回收"新产生的、且不在 keep 名单里"的句柄 |
| 5 | **内核池硬化** | `core-kernel/src/pool.rs`：修掉一个**真实 bug**——`with_kernel` 原先只查 `slot.is_none()`，某线程先跑过 mock 后会**永远拿 mock**；现检查 `is_real()` 非真则丢弃重建。重入改用 `try_borrow` → `KernelError::NotInitialized`，不再 panic |
| 6 | **装载器接线** | `core-app/src/loader.rs`：`BrepShapeLoader` 按 `GeomFormat` 分派（`BrepText → import_brep`，其余指向后续里程碑），取代原先"一律拒绝"的占位 stub |
| 7 | **落盘闭环** | `GeometryService::snapshot` / `store`：`export_brep` → `GeometryRecord::brep` → 文档；重名不覆盖（`DuplicateId`）。新增"执行→落盘→重载"往返测试（占位后端） |

### ✅ 已修复：`occt-wasm` 4.0.0 内核无法实例化（P0，vendor + patch）

**症状**（修复前，`--features occt --release` 亦复现）：

```
Kernel(Geometry { op: "OcctKernel::new",
  msg: "WASM runtime error: unknown import:
        `env::emscripten_get_preloaded_image_data` has not been defined" })
```

**根因（源码级确认，属上游 4.0.0 缺陷、非本项目代码问题）**：`occt-wasm` 4.0.0 的
`src/kernel.rs:148-161` 用 `let linker = Linker::new(&engine);` 建了一个**空 linker**，
随后直接 `linker.instantiate(...)`。内嵌的 WASM 是 **Emscripten 产物**，声明了 `env::*` 系列
host import；wasmtime 48 在 `instantiate` 阶段要求全部 import 有名有主 → 实例化直接失败。

**为何"等上游修"与"降级"都不可行（已实证）**：crates.io 上 4.0.0 即**最新**（2026-09-21 发布，无 4.0.1）；
3.3.3 与 4.0.0 代码几乎相同（4711 vs 4738 行），降级不改命；根 `Cargo.toml` 锁 `version = "4"`，
`--precise 3.3.3` 被 cargo 拒绝。**故只有第三条路：本地 fork 修复。**

**采用的方案（= 把库"下载到工程内"）**：

1. 把 `occt-wasm` 4.0.0 源码完整复制进 **`src/vendor/occt-wasm/`**（15 个文件；WASM 本体
   `src/occt-wasm.wasm.br` 的 **SHA256 与上游逐字节一致，未被替换**）。
2. 在其 `src/kernel.rs` 的 `OcctKernel::new()` 里，`instantiate` 之前补一行：

   ```rust
   let mut linker = Linker::new(&engine);              // 原为 let linker
   linker.define_unknown_imports_as_traps(&module)?;   // ← 新增：未实现的 import 一律定义成 trap
   let instance = linker.instantiate(&mut store, &module)?;
   ```

3. 根 `Cargo.toml` 加
   ```toml
   [patch.crates-io]
   occt-wasm = { path = "vendor/occt-wasm" }
   ```
   并把 `vendor/occt-wasm` 放进 `[workspace] exclude`（**不作为 member**，避免它自己的
   lint/deny 规则污染本仓门禁）。
4. `vendor/occt-wasm/PATCHES.md` 记录了基线、改了什么、为什么、import 全表、
   **traps vs default-values 的 A/B 结论**、以及上游修复后如何移除本补丁。

**为何选 `traps` 而非 `default values`**：`define_unknown_imports_as_traps` 下，若真调到
未被实现的导入会**显式 trap**（响亮失败）；`define_unknown_imports_as_default_values` 则会
**静默返回 0**——那会把几何算错而不报警，正是我们最不能接受的失败模式。A/B 实测两版结果
逐条完全一致，证明**本项目已验证的几何路径未调用任何未实现导入**。

**修复后复验（真实内核独立实测，非"跑一遍绿"）**：

| 实测项 | 结果 |
|---|---|
| `make_box(10,20,30)` 体积（QA 独立计算） | **6000.0**（精确） |
| BREP 导出/导入往返 | 含 `CASCADE` 头、**几何守恒** |
| 布尔 `cut` | 58232.854（差 1767.146 = π·15²·10/4，圆柱在板角只切 1/4，**正确**） |
| `translate(1000,0,3000)` | bbox 精确位移、体积守恒 |
| HLR 圆孔板俯视 | outline/smooth 层全 null；sharp 层各 5 条（4×line + 1×circle），采样半径 **15.0000**（±0.05 内）→ **内核确实判出了解析圆** |
| `cargo metadata` | 确认 `occt-wasm` 来源是 **`vendor/` path 包**（非 registry） |

**未引入任何新假绿（QA 证伪式复核确认）**：全量门禁仍硬失败可拦（见下表）、无新增静默跳过、
`check-deps` 与 `deny` 仍在真拦、WASM 本体未变。

**防复发措施（本次 M1 的核心价值之一）**：把"能不能加载"钉成**会失败的用例**——
**"跳过"与"通过"在报告里必须可区分**：

| 用例 | 位置 | 行为 |
|---|---|---|
| `occt_feature_must_actually_instantiate_the_kernel` | `core-kernel/src/occt.rs` | 启用 `occt` 后内核**必须**真能实例化；失败即 `assert!` 失败并打印根因与排查方向 |
| `with_the_occt_feature_new_returns_a_real_kernel` | `core-app/src/geometry_service.rs` | 同上，端到端视角的门禁 |
| `real_kernel_execute_snapshot_resolve_keeps_the_volume` | `core-app/src/geometry_service.rs` | 内核不可用时**报告原因并跳过**（避免与上面两条造成重复噪音） |

另记录：`LOAD_ERROR: OnceLock<String>` 会把首次加载失败原因留住，供门禁用例打印（`load_error()`）。

### 门禁状态（2026-09-23 实测）

| 门禁 | 命令 | 结果 |
|---|---|---|
| 格式 | `cargo fmt --all --check` | ✅ 0 |
| clippy（默认） | `cargo clippy --workspace --all-targets -- -D warnings` | ✅ 0 |
| clippy（occt 腿） | `cargo clippy -p core-kernel -p core-app -p core-testkit --features occt --all-targets -- -D warnings` | ✅ 0 |
| 测试（默认） | `cargo test --workspace --no-fail-fast` | ✅ **474 passed / 0 failed / 2 ignored**（M1 时点 457；M2 收口 +17） |
| 架构 | `cargo xtask check-deps` | ✅ 0 |
| 许可 | `cargo deny check licenses bans sources` | ✅ `bans ok, licenses ok, sources ok` |
| 端到端 | `cargo run -q -p bim-desktop` | ✅ 退出码 0 |
| 测试（occt 腿） | `cargo test -p core-kernel --features occt --release` | ✅ **62 passed / 0 failed / 0 ignored** |
| 测试（occt 腿 / core-app） | `cargo test -p core-app --features occt --release` | ✅ 142 passed / 0 failed（108 lib + 10 + 16 + 7 + 1） |

**三个独立回归探针**（`core-app/tests/`，确定性、无外部依赖，均长期保留）：

| 探针 | 条数 | 结果 |
|---|---|---|
| `container_probe.rs` | 16 | ✅ 16 passed / 0 ignored |
| `translate_probe.rs` | 7 | ✅ 7 passed / 0 ignored |
| `brep_parse_probe.rs` | 10 | ✅ 10 passed / 0 ignored |

> 记一句：occt 腿的"红"是**有信息量的红**。P0 修复前它精确指出"真实内核没装起来"；
> P0 修复后剩下的 3 条红**同样有信息量**——它精确指出"适配层口径错了"，而不是"内核不行"。

### M0 判据复验：适配层三处口径缺陷（**已全部修正**）

P0 修复后重跑 occt 腿，内核实例化门禁转绿，但余 3 条红。源码级定位后确认：
**三条全部与 linker 补丁无关，是适配层（`core-kernel`）自己的口径问题**。现已修正：

| # | 现象 | 根因 | 修法 |
|---|---|---|---|
| 1 | `projection_of_holed_plate_yields_analytic_circle` / `section_plane_…` 报错而非判出圆 | `collect_branch` 对**空分支**也调 `get_sub_shapes` → 冒泡 `Invalid shape ID: 0`。内核侧其实**已判出半径恰为 15 的解析圆**（HLR 实测佐证） | HLR 的空分支用 **id = 0** 占位；`collect_branch` 先挡 `id() == 0` 再查子形状。⚠ 判空**只能看 `id()`**：内核连自己的 `is_null(ShapeHandle(0))` 都报 `Invalid shape ID: 0` |
| 2 | `tessellation_reports_face_groups_for_picking` 断言 `sum == 12`，实得 **36** | 内核 `face_groups` 的 `tri_start/tri_count` 是**索引**单位（实测长方体 6 面为 `[0,6,·] … [30,6,·]`，跨度 36 = `indices.len()`），而适配层 `FaceGroup` 定义是**三角形**单位 | `convert_mesh` 换算 `/3`；**不整除就报错**（不静默少算三角形）。用例加"首尾相接 + 恰好铺满"两条钉子 |
| 3 | `face_group_hashes_map_back_to_face_handles`（判据⑤）两集合不相交 | **不是"两个域"，是两把尺子**：OCCT `HashCode(upper)` 结果落在 `[1, upper]`，用例传 `1_000_000`、而网格化器用 `i32::MAX` → 同一个面得到 `326887` vs `1652326887`，看似"根本不是一个面" | 域由后端内部常量 **`FACE_HASH_UPPER_BOUND = i32::MAX`** 固定（实测与网格哈希**逐个相等且同序**），并新增正式能力 **`KernelBackend::face_hashes`** 把它暴露出来，不再让调用方猜 bound |

**判据⑤的落地口径（设计文档 6.6 的拾取链路）**：粗筛 = `MeshData.face_groups[].face_hash`
与 `Kernel::face_hashes(shape)` **同域同序**比对；精算 = `project_point_on_face`。
新能力对**两个后端都成立**（mock 由构造保证，真实后端由 bound 对齐保证），并已写成用例。

---

## 框架补齐（2026-09-23）：`.bimdoc` 容器 / `CoreSession` / 端到端闭环

设计文档 §4.2 要求 `.bimdoc` 为自建容器、§9 要求端到端"加载 → 建模 → 出图 → 存盘"跑通。
在此之前工程只到"各服务各自可测"，缺三块结构性拼图，本次补齐：

| # | 缺块 | 落地 |
|---|---|---|
| 1 | **`.bimdoc` 容器（§4.2）** | `core-app/src/store.rs`：`save_document` / `open_document`（zip）+ `pack_to_dir` / `open_from_dir`（目录形态）。布局 = `manifest.json`（版本识别）+ `doc.json`（**语义权威副本**）+ `geom/<shapeid>.brep`（**必须与 doc.json 一致的导出副本**）+ `views/*.json` + `assets/`。打开时逐条 `verify_geometry`，不一致 / 缺失**明确报错**，不静默降级。本机无 `tempfile` 依赖 → 自建 `TempDir`（`process::id()` + 原子计数器 + `Drop` 递归删） |
| 2 | **`CoreSession` 顶层门面** | `core-app/src/session.rs`：把 `DocumentService` / `GeometryService` / `InteropService` / `ModuleHost` 装配成一个会话对象，提供 `new` / `open` / `from_document` / `install_module` / `create_element` / `build_geometry` / `save` / `summary`。默认**显式**选择占位内核 + BREP 装载器（不静默降级） |
| 3 | **端到端闭环** | `apps/desktop/src/main.rs`：`CoreSession` → 装载 `example-beam` → `create_element` → `build_geometry`（算子 → 配方 → execute → snapshot → 落盘 → 回填 `geometry_ref`）→ `save` 临时 `.bimdoc` → `open` → 断言元素 / 品类一致；退出码 0 |

**为什么端到端用例必须放 `apps/desktop`**：`core-app` 不许依赖 `example-beam`（否则 `core-app → modules/* → core-sdk → …` 形成反向依赖，违反铁律②）。`apps/*` 是依赖终点，把"示例模块"的集成验证放这里，是唯一不破架子的位置。

**`GeometryOp` 现在能真正执行 `Translate`**：
- trait 增 `KernelBackend::translate`（`core-kernel/src/backend.rs`）；`occt.rs` 包装 `OcctKernel::translate`；`geometry_service.rs` 的 `BuildOp::Translate` 由 `Unsupported` 改为**真调用**。
- `Rotate` / `Extrude` / `Revolve` 仍显式返回 `KernelError::Unsupported`——**不假装支持**。
- `GeometryService::execute` 任一步失败时，先 `release_many` 已产生的中间句柄再**原样抛出原错误**（回滚，防不可达句柄泄漏）。

**一个必须记住的坑（本轮最大教训）**：占位后端的 `translate` 最初是**"静默成功"**——校验完 `dx/dy/dz` 就把它们丢掉，导出文本与未平移时**逐字节相同**。
于是"平移"能过门禁、能过演示，**却会在存盘后凭空消失**。修法：`Shape` 增加 `origin: [f64; 3]`，`translate` 累加偏移；
MOCK-BREP 文本升 **v2**（末尾三段 origin），且 `from_text` **必须兼容 v1**（v1 一律视为 origin 归零）、**解析改严**（非数字 token / token 数不符 / 版本未知一律 `Err`，不再 `filter_map` 静默丢弃）；
`tessellate` / `project_edges` 把 origin 纳入计算；`volume` 明确注明是**平移不变量**（有意不读 origin）。

> **教训：全绿 + 演示通过 ≠ 没丢数据。** 凡是"改了什么、存下来还在不在"的路径，必须有一条**独立探针**专门攻击它。

**"改严"要一次改彻底（同一个坑的第二跳）**：把 `filter_map` 的静默丢弃改成 `Err` 之后，`from_text` 仍**漏掉了非有限数**——
`NaN` / `inf` / `-inf` 都是合法的 `f64` token，于是 `MOCK-BREP v2 box 100 200 300 6e6 NaN 0 0` 会被**静默收下**，把非有限几何注入文档。
修法：`from_text`（`mock.rs:159-179`）对 `size` / `volume` / `origin` **逐分量 `is_finite()`** 校验；`translate`（`mock.rs:455-461`）的同名校验同样用 `is_finite()`。
**注意不能靠 `> 0` 之类的比较来兜底——`NaN` 参与任何比较恒为 `false`，只有显式 `is_finite()` 才拦得住。**
反向边界也要钉：`-0.0` 是**有限**值，必须**照常接受**（`brep_parse_probe` 的 p9 专门钉这个，防止矫枉过正）。

**会话态基准元素不落盘**（§4.6.3）：`Document::to_persistable()` 在写盘前剔除 `DatumScope::Session` 的基准元素，`store.rs` 走它——
规则放在**数据层**，不在 app 层复制一份。

**独立回归探针**（`core-app/tests/`）：

| 探针 | 条数 | 覆盖 |
|---|---|---|
| `container_probe.rs` | 16 | 空文档 / 三几何往返 / zip 篡改 / 缺 geom / 坏版本 / 非 UTF-8 / 目录当 zip / 非法 JSON / 中文名 / **会话态泄漏** … |
| `translate_probe.rs` | 7 | mock translate 语义 / 已释放句柄 / 多步配方句柄数 / 端到端最小模块 / `Rotate` 仍 `Err` / **中途失败必须回滚** / **平移必须存得住** |
| `brep_parse_probe.rs` | 10 | v1 旧文本可读且 origin 归零 / 17 条畸形输入全 `Err` 且不 panic / **v1/v2 无段数歧义**（靠版本 token 判别）/ **非有限数必须拒**（`NaN` `±inf` × size/volume/origin × v1/v2）/ **`-0.0` 必须接受**（防矫枉过正） |

---

## M2：数据层收口（✅ 已完成，2026-09-23）

M2 的定义（设计文档 10.1）是"`core-model` 完成：文档 CRUD、事务、存盘、再打开一致"；§4.6.6 进一步要求
"四类内置基准元素 + Relations + 事务支持"。

**先说实话：M2 的大部分内容此前已随骨架落地**（`Document` 幂等 CRUD 与级联删关系、`Transaction` 影子副本原子提交、
四类 `DatumKind` 与 `DatumScope`、`.bimdoc` 往返、会话态不落盘）。所以本轮**不是"从零做 M2"，而是逐条对照设计
文档收口**：干掉两条"文档明文要求、代码却没做到"的纪律，并补上关系图的遍历能力。

### 本轮补的三块

| # | 缺的是什么 | 文档依据 | 落地 |
|---|---|---|---|
| **A** | **事务强制**：`Document` 的 8 个字段（`meta`/`elements`/`datums`/`geometry`/`relations`/`views`/`sheets`/`version`）原先**全是 `pub`**，任何地方都能直接改——`core-app/src/geometry_service.rs` 里就实打实地 `doc.geometry.remove(...)` 绕过事务 | §4.3「所有对文档的修改**必须**走事务（CI 里检查 `Document` 的字段私有化即可强制）」 | 字段全部私有化；只提供 9 个 **`&self` 只读访问器**（`meta()/elements()/datums()/geometry()/relations()/views()/sheets()/version()/geometry_count()`），**一个 `*_mut` 都不给**；补唯一受控修改口 `remove_geometry()`；迁移 12 个文件的调用点 |
| **B** | **轴号编号不可配**：`GridNumbering` 只有 `Numeric/Alphabetic/Mixed`，既没有"是否跳过易混字母 I/O/Z"，也没有"序号 → 轴号文本"的能力 | §4.6.4「轴号命名规则（…是否跳过 I/O/Z）必须可配置」 | 新增 `DatumConvention.skip_ambiguous_grid_letters`（`#[serde(default)]`，默认 true）+ `GridDirection` + 纯函数 `grid_label(index, direction)`，字母用**双射 base-K**（不跳过 K=26：`1→A…26→Z,27→AA`；跳过 I/O/Z 时 K=23：`9→J,21→W,23→Y,24→AA`） |
| **C** | **Relations 只能单跳**：只有 `relations_touching` / `elements_positioned_on`，无法"沿关系图找"（§6.4 关联更新要用） | §4.6.2 关系层、§6.4 | 新增 `core-model/src/graph.rs`：`relations_walk(start, max_depth)`（无向 BFS + `visited` 防环 + 按 `(跳数,端点)` 排序保证确定性）、`dependent_views` / `dependent_sheets` / `elements_bound_to_geometry`。**零新依赖**（仅 std） |

> **C 的归属划清**：这里只放"纯数据、零几何"的**遍历原语**。§6.4 的 `DependencyGraph` 与"脏标记 → 增量重绘"
> 依赖内核几何演化历史（M3）与 `core-doc` 的二维图元（M5），**推给 M5**，不在本模块。

### 怎么证明"字段真的私有化了"（要守卫，不要口头纪律）

`compile_fail` doctest 是这类纪律最自然的写法，但**本沙箱跑不了它**——根因已实测定位（见下）。
于是改用**源码级守卫** `document_fields_stay_private_source_guard`：`include_str!("document.rs")` 取出
`Document` 定义体 → 剥掉 `//` 注释 → **去掉所有空白做归一化** → 断言体内**根本不出现 `pub`**。

**为什么是"断言不含 `pub`"而不是逐字段匹配 `pub {field}:`**：第一版就是后者，被 QA **实测打穿**——
把 `meta` 改成 `pub(crate) meta: Meta,` 时它**仍然绿**（字面匹配 `"pub meta:"` 匹配不上 `pub(crate) meta:`）。
这类"看起来在守、其实漏报"的守卫比没有守卫更危险。现在的写法对 `pub` / `pub(crate)` / `pub(super)` /
跨行 / 多空格**一律报红**，并已由**三次攻击实测**确认（`pub meta:` 红、`pub(crate) meta:` 红、跨行红）。

### 一个必须记住的环境事实：`compile_fail` doctest 在本沙箱必失败

| 路径 | 是否接**管道 stdin** | 本沙箱 |
|---|---|---|
| rustdoc **合并**路径（`no_run` 等，全部 doctest 合成一个 crate 一次编译） | 否 | ✅ 可跑 |
| rustdoc **非合并**路径（`compile_fail` 必须**逐条单独**调 rustc） | **是** | ❌ `CreateProcess` 报 `ERROR_PIPE_BUSY` / `os error 231` |

失败发生在 **spawn 阶段、编译器还没介入**，与用例内容无关；**仓库里任何 `compile_fail` doctest 都会如此**。
所以：本仓以源码守卫兜底；**真机 / CI 上可把 `compile_fail` doctest 加回**（片段与说明写在
`document.rs` 该守卫的注释里，刻意用普通 `//` 注释以免它自己变成真 doctest）。

### M2 收口门禁（2026-09-23 实测）

| 门禁 | 命令 | 结果 |
|---|---|---|
| 格式 | `cargo fmt --all --check` | ✅ 0 |
| clippy | `cargo clippy --workspace --all-targets -- -D warnings` | ✅ 0 |
| 依赖方向 | `cargo xtask check-deps` | ✅ 合规（`core-model` 内部依赖 `[]`；`graph.rs` 未引入新依赖） |
| 许可 | `cargo deny check licenses bans sources` | ✅ `bans ok, licenses ok, sources ok` |
| 测试 | `cargo test --workspace --no-fail-fast` | ✅ **474 passed / 0 failed / 2 ignored**（收口前 457；**+17 全部为新增用例，无一条既有断言被弱化**） |
| 测试（occt 腿） | `cargo test -p core-kernel --features occt --release` | ✅ 62 passed / 0 failed / 0 ignored |

### 复核方式（三层，不是"跑一遍绿"）

1. **架构师**（只读）先出影响面清单——核实时**多挖出一处**我方清单遗漏的迁移点（`core-doc/src/drawing.rs`
   直读 `doc.views`/`doc.sheets`），并指出关键坑：`transaction.rs` 与 `document` 是**兄弟模块**，
   私有化后其 `apply()` 里的 `doc.geometry.remove(id)` 必编译红。
2. **工程师**实施并自证门禁；报告了 2 处与计划不符（测试模块同样直读私有字段；`compile_fail` doctest 环境不可用）。
3. **QA 对抗式复核**（fresh eyes）：逐项证伪——把字段改回 `pub` 打穿守卫（由此发现 P2-1）、独立推演双射
   进制边界值表并与实现逐个核对、自写 13 项图遍历对抗用例（自环 / 双向重复边 / 多连通分量 / 同 shape 不同 sub）、
   抽查被迁移用例确认**只改取值方式、断言语义未放松**，并给出 `compile_fail` 的最小复现与根因。

> **本轮最值钱的一条经验**：**守卫本身也要被攻击**。一个"只在字面形态上匹配"的守卫会给团队**假的保证**，
> 比不写守卫更坏——它让"跳过"看起来像"通过"（与 M0 那次假绿同源）。

---

## M4：互操作基线（✅ 已完成 / **STEP 明确宣告未达成**，2026-09-24）

M4 的定义（设计文档 10.1）：**四格式基线** —— IFC 可解析、STEP 可读写、DXF 可兼容、glTF 可解析（依赖 M1）；
外加 M3 判给 M4 的三个内核算子 `sew` / `fix_shape` / `unify_same_domain`。

### 一句话结论（诚实口径，务必据此对外表述）

> **四格式里三格式达标**（IFC 可解析 / DXF 可兼容 / glTF 可解析），
> **STEP 明确宣告"未达成 §10.1 的可读写"** —— 原因是**上游 `occt-wasm` 4.x 的构建形态缺陷**（见下），
> **不是本项目未实现**。M4 **不引入第二套几何内核**来绕过它。**禁用"部分支持"这类含糊措辞。**

### 交付矩阵（实测）

| 格式 | 导入 | 导出 | 承载实现（`note` 已写明来源） | 实测 |
|---|---|---|---|---|
| **DXF** | ✅ 可用 | ✅ 可用 | 自写序列化器**导出** + `dxf` crate **读取** | CIRCLE / ARC 以**真解析实体**往返，圆数守恒、角 rad↔deg 已核 |
| **glTF** | ✅ 可用 | ⊘ 不做 | `gltf` crate（只读结构 + 顶点统计） | 内嵌 buffer 可读；**网格不进 B-Rep**，只作三维预览 |
| **IFC4X3** | ✅ 可用 | ⊘ 不做 | `ifc-lite-core`（MPL-2.0） | 达标线三要素齐备：**构件** + **属性集**（`IfcRelDefinesByProperties`→`IfcPropertySet`→`IfcPropertySingleValue`）+ **空间层级**（`IfcRelAggregates` / `IfcRelContainedInSpatialStructure`），落 `Element` 的 `PropertyMap`；未支持实体 warn 跳过、**不整包失败** |
| **STEP** | ⊘ **不交付** | ⊘ **不交付** | 原计划走内核（已证伪） | 见「STEP 判死」；`StepImporter::import` **显式返回 `Err(Unsupported)`**（绝不返回"只带头部元信息"的空成功），垃圾输入仍 `Err(Parse)` |

三个内核算子（M3 判给 M4）：`sew`（成壳）/ `sew_and_solidify`（成实体）/ `fix_shape` / `unify_same_domain`
—— 真实内核腿用例在 `core-app/tests/heal_ops.rs`，**4 条全绿**（`sew` 断言成壳 `Solid==0`、`sew_and_solidify` 断言 `Solid==1`）。

### 为什么 STEP 推迟（三层证据链，不是"试了下不行"）

1. **本 fork 的 STEP 出入口全 trap**：`export_step` / `import_step` / `xcaf_import_step` / `xcaf_export_step` / `xcaf_export_gltf` 全部触发 wasm trap。
2. **补 `_initialize()` 只把故障"搬家"**：确实能修掉 `uninitialized element`，但故障随即迁移到 `Quantity_Date invalid parameters` → `exportStep: write failed` / `cannot create temp file`。
3. **决定性铁证 —— 文件系统根本没编进 WASM**：对解压产物（24,290,435 B）扫字符串，`openat` / `path_open` / `MEMFS` / `fopen` / `mkstemp` / `__syscall_openat` **全部 0 命中**（`__syscall_*` 只有 chmod / faccessat / getdents64 / rmdir / unlinkat）。而 STEP 读写**必须**走临时文件 → **host 侧没有任何入口可补**。

> **为什么不采用"补 `_initialize()`"这个一行补丁**：它只是把**硬 trap 降级为 `write failed`** —— 换不来 STEP，
> 却丢掉了"显式失败"的安全性。**不修它，比修它更安全。**

### 缺陷钉子：为绿 = 缺陷仍在（`core-kernel/src/known_upstream_defects.rs`）

把上游 trap **主动钉成"注定会失败"的用例** —— 这是 M0 那条铁律的延续：「外部依赖可用性必须有'注定会失败'的用例钉住」。
三条钉子：`export_step_traps_upstream` / `import_step_traps_upstream` / `xcaf_export_gltf_traps_upstream`。

> ⚠ **这三个钉子"为绿"是正常的**（= 上游缺陷仍存在）；**将来变红才是喜讯** —— 那意味着上游修好了，可以把 STEP 接回来。

**两个必须记住的技术坑（本轮踩实）**：

1. **trap 以 `Err` 暴露，不是 panic** → **绝不能用 `#[should_panic]`**，必须 `expect_err` + 断言错误链。
2. **`Display` 只打顶层上下文**：`OcctError::Runtime(wasmtime::Error)` 的 `Display` 停在
   `error while executing at wasm backtrace: ...`，**真正的 trap 文本挂在 `source()` 链更深一层**。
   曾因只看顶层 `Display` 误判"错误文本漂移"，实为**观察位置不对**。钉子工具 `error_chain_text` 逐层拼 `source()` 后才拿到稳定子串。

### 声明层守卫：一把锁的两面（`capability.rs`）

`capability::FORMATS` 是"能力表写进代码"的落点。本轮把它做成**可被攻击的守卫**（承 M2 教训：守卫本身也要被攻击）：

| 面 | 防的是什么 | 攻击用例 |
|---|---|---|
| **`Native` 面** | **冒充已交付**（明明坏的却声称可用） | `capability_guard_detects_native_regression_attack`：把 STEP 的 import / export 改回 `Native`、或换成 `Planned` / `ViaSeparateProcess`，守卫必须**全部识破** |
| **`Planned` 面** | **掩盖未交付**（明明没做却挂着"计划实现"——`Planned` 在 `is_usable_in_process()` 口径下也为真） | `planned_guard_detects_dangling_promise_attack`：给某格式挂 `Planned`，守卫必须识破 |
| **glTF 导出面** | glTF 导出走同一条坏掉的 XCAF 路径（`xcaf_export_gltf` trap），且 M4 已把它移出范围 | `gltf_export_guard_detects_native_and_planned_disguise`：把 glTF `export` 改回 `Native` / `Planned` / 洗白 note，守卫必须**全部识破** |

> **`Support::Native` 的准确读法**（§8.1）：一律读作「**进程内已实现、可直接调用**」，
> **不等于**"由 `occt-wasm` 内核实现" —— IFC / DXF / glTF 三者全是**第三方库承载**。
> 故凡用 `Native` 的格式，`note` **必须写明实现来源**。历史 label「内核原生」已改为「进程内可用」。

### 一个走空的退路（记录在案）：`truck-stepio`

实测 `truck-stepio 0.3.0` **确实能用**（真 Cube 往返无损、B-Rep 级、112 依赖全 Apache-2.0 无 GPL），
但它是**第二套几何内核** —— 几何进不了 OCCT 域，M4 内**无上层可消费**（孤岛）。
故**不在 M4 引入**，仅作为「OCCT sidecar 里程碑」的**选型输入**记录在案。`ruststep 0.4.0` 更弱（只结构级、无 writer）→ 弃。

### 收口复核：独立 QA 抓出的 3 条 P1（已全部修复）

M4 落地后由**独立对抗式复核**（fresh-eyes，非实现者）打穿，抓出 3 条均为真的 P1，已逐条对齐方案原文修复：

| # | 问题 | 修复 |
|---|---|---|
| **P1-1** | `StepImporter::import` 仍返回 `Ok` + 只带头部元信息（与方案「必须 `Err(Unsupported)`」冲突）；且 `is_format_usable(Step)==true` 与声明层 `NotPlanned` 打架 | `import` 改为**显式 `Err(Unsupported)`**（垃圾输入仍 `Err(Parse)`）；`FormatRouter::is_format_usable` **同时查注册表与能力声明**（注册 ≠ 可用），STEP 判为不可用 |
| **P1-2** | `IfcImporter` 只到"构件类型识别"（属性集/空间层级缺失，达标线三要素只做了 1/3） | 用 `EntityDecoder::decode_by_id` 两遍扫描，补 **属性集**与**空间层级**，落 `Element` 的 `PropertyMap`（不新造 `core-model` 结构）；补 3 条钉子 |
| **P1-3** | `svg.rs` 对"只有 1 个圆心兜底点"的解析圆/弧 `continue` 跳过 → `read_dxf → to_svg` **静默丢几何** | `to_svg` 对点列不足但带 `analytic` 的曲线**现场离散化**（圆 64 段 / 弧按扫角），不再静默丢；补 2 条钉子 |

> 教训延续：**"能编译"≠"被消费"**（M3）、**"注册在案"≠"可用"**（本轮 P1-1）、**"看着成功"≠"没丢东西"**（本轮 P1-3）。

### M4 门禁（2026-09-24 实测）

| 门禁 | 命令 | 结果 |
|---|---|---|
| 格式 | `cargo fmt --all --check` | ✅ 0 |
| clippy（默认） | `cargo clippy --workspace --all-targets -- -D warnings` | ✅ 0 |
| clippy（occt 腿） | `cargo clippy -p core-kernel -p core-app -p core-testkit --features occt --all-targets -- -D warnings` | ✅ 0 |
| 单元测试（默认） | `cargo test --workspace --no-fail-fast` | ✅ **533 passed / 0 failed / 2 ignored**（M3 509 → +24：内核算子 + DXF 解析 + IFC 属性集/层级 + 声明层守卫 + P1 收口钉子；**无一条既有断言被弱化**） |
| 架构 | `cargo xtask check-deps` | ✅ 合规（`core-interop` 内部依赖 `[core-model, core-kernel]`，未越 `core-doc`） |
| 许可 | `cargo deny check licenses bans sources` | ✅ `bans ok, licenses ok, sources ok`（新增 dxf / gltf / ifc-lite-core 均过白名单）。⚠ `advisories` 腿需联网拉 RustSec 库，**本机网络受限未能复核**（见下注） |
| 测试（occt 腿 / core-kernel） | `cargo test -p core-kernel --features occt --release -- --test-threads=1` | ✅ **92 passed / 0 failed / 0 ignored**（含 3 条缺陷钉子） |
| 缺陷钉子（定向） | `cargo test -p core-kernel --features occt --release known_upstream_defects -- --test-threads=1` | ✅ **3 passed / 0 failed** |
| 测试（occt 腿 / core-app） | `cargo test -p core-app --features occt --release -- --test-threads=1` | ✅ **159 passed / 0 failed**（110 lib + 10 + 16 + 7 + `heal_ops` 4 + 11 + 1） |

> **`cargo deny check advisories` 为何未复核**：该腿要 `git fetch` `https://github.com/RustSec/advisory-db`，本机网络受限 → `failed to fetch advisory database`（超时）。
> 这是**环境限制、与 M4 代码无关**；licenses / bans / sources 三条（本里程碑真正关心的许可白名单与 `libredwg` 禁令）**已本地复核通过**。CI（有网）应能补上 advisories 腿。

> **occt 腿为什么必须 `--release` + `-- --test-threads=1`**：`--release` 是因为内核要解压 ~21 MB WASM 并 JIT（debug 下约 100× 惩罚）；
> 单线程是因为**并行会让内核线性内存爆掉**（表现为 `memory allocation of N bytes failed` + 进程 `0xc0000409` 中止，**日志里没有任何 `FAILED` 行**，极易误读成代码回归）。

---

## M7：专业模块（✅ 已完成，2026-09-30 —— 电力侧与建筑侧各至少一个可用模块）

M7 的定义（设计文档 10.1）：**专业模块** —— 电力侧与建筑侧各至少一个可用模块（依赖 M6）。
落地口径：模块以 Recipe 模式挂接（`GeometryOp::apply` 只产出 `BuildStep` 序列，由 `core-app` 执行），
**零 SDK 改动、不碰内核**——继 `example-beam`、`cable-tunnel` 之后，`hvac-duct` 不改一行 SDK
就能挂上，即立项 P0 风险（"SDK 接口若贴合单一专业就废了"）的持续证伪。

### 交付

| # | 事项 | 说明 |
|---|---|---|
| M7a | **变长算子** | `Loft`（截面 N≥2）/ `MakeWire`（边 N≥1），`arity_ok` 单一事实源；`core-sdk/src/geometry_op.rs` 有专门的 M7a 用例（变长标注、零输入被配方门禁拒绝） |
| M7b | **example-beam 第二算子** | `example.beam.hole`：MakeBox→MakeCylinder→Cut→Translate（圆孔板，布尔 Cut 真派发）——M6b-3 候选的 M7 落地 |
| M7c | **电力侧标杆模块 `cable-tunnel`** | `power.tunnel.rect`（矩形隧道段：MakeBox→MakeCylinder→Cut→Translate）+ `power.tunnel.ductbank`（电缆排管段：MakeLineEdge×4→MakeWire→**Pipe**）；16 条单测 |
| M7d | **建筑侧暖通模块 `hvac-duct`** | `hvac.duct.rect`（矩形风管段：MakeBox→Translate）+ `hvac.duct.transition`（变径管：两端截面→MakeWire→**Loft**(is_solid+ruled)→Translate，11 步配方）+ `hvac.duct.width` 宽度标注规则；17 条单测。默认 500×320×1200 mm 镀锌钢板，宽 >2500 落 warning 不 fail |

### 为什么变径管是 M7 的关键一仗

`hvac.duct.transition` 是**第一个使用变长算子 Loft 的模块**：

- 起端 400×400、末端 800×800、延米 1000，两截面绕同一 anchor 居中、`ruled=true` → **精确棱台**；
- occt 腿体积 oracle 用解析式 `V = h/3 × (S1 + S2 + √(S1·S2))` = **373,333,333.33 mm³（±0.5%）**、
  bbox 800×800×1000（±1e-6），实测全部命中；
- mock 腿不硬凑：占位后端不支持 Loft，`hvac.duct.rect`（MakeBox/Translate）走 mock 五环，
  变径管的真实 execute 断言放 occt 腿——**不假装支持**。

### e2e（依赖终点铁律：模块的集成验证只放 `apps/desktop`）

| 腿 | 文件 | 用例 | 结果 |
|---|---|---|---|
| mock（默认 CI） | `apps/desktop/tests/e2e_hvac.rs` | `hvac_module_passes_five_loop_minimal_check`：矩形风管五环（加载→建模→出图→存盘→再打开），含宽/高/延米属性往返与 Drawing 深相等 | ✅ 1 passed / 0.17s |
| occt（`--features occt --release -- --test-threads=1`） | `apps/desktop/tests/e2e_hvac_occt.rs` | `duct_transition_loft_executes_with_exact_frustum_volume`（真内核 11 步 → Solid==1 → 棱台体积/bbox 双 oracle）+ `hvac_module_passes_five_loop_minimal_check_real`（真内核五环） | ✅ 2 passed / 19.74s |

电力侧同构先例（M7c 时点已落）：`apps/desktop/tests/e2e_power.rs`（occt 腿 5 条：配方有序性 /
配方可表达性 / rect 真执行落盘 / ductbank Pipe 端到端 / 五环）。

### M7 门禁（2026-09-30 实测）

| 门禁 | 命令 | 结果 |
|---|---|---|
| 格式 | `cargo fmt --all --check` | ✅ 0 |
| clippy（默认腿） | `cargo clippy --workspace --all-targets -- -D warnings` | ✅ 0 |
| clippy（occt 腿） | `cargo clippy -p bim-desktop --features occt --all-targets -- -D warnings` | ✅ 0 |
| 架构 | `cargo xtask check-deps` | ✅ 合规（`hvac-duct` 内部依赖仅 `[core-sdk]`，铁律 3） |
| 单元测试（默认腿全量） | `cargo test --workspace --no-fail-fast` | ✅ **720 passed / 0 failed / 2 ignored**（M6 收口基线 674 → +46：cable-tunnel 16 + hvac-duct 17 + e2e_hvac 1 + M7a/M7b 算子用例） |
| 三模块单测 | `cargo test -p hvac-duct -p cable-tunnel -p example-beam` | ✅ **49 passed**（hvac-duct 17 + cable-tunnel 16 + example-beam 16） |
| occt 腿（bim-desktop） | `cargo test -p bim-desktop --features occt --release -- --test-threads=1` | ✅ **14 passed / 0 failed**（e2e_mock 5 + e2e_occt 1 + e2e_power 5 + e2e_hvac 1 + e2e_hvac_occt 2） |

> e2e_hvac.rs 踩过的两个小坑：`geometry_op_ids()` 迭代项是 `&&str`，`id == &str` 直接编译错，
> 且 clippy 要求用 `.contains(&ID)` 而非 `iter().any()`；`ModuleHost` 没有直接的
> `geometry_op_ids()` 方法，要走 `session.modules().registry().geometry_op_ids()`。

---

## M8：桌面端 GUI 第一版（✅ 已完成，2026-10-01 —— egui 真实窗口：视口 / 拾取 / 属性面板）

**交付（`src/apps/desktop/src/gui.rs`，~900 行，`gui` feature 门控；egui 0.29 + eframe/wgpu）**：

| 能力 | 说明 |
|---|---|
| 三维视口 | 轨道相机（Z 朝上，mm 单位）+ CPU 画家算法（三角形按相机空间深度降序组装进单个 `epaint::Mesh`，零自定义 shader、零 wgpu 生命周期代码）+ 双面光照平面着色（\|n·l\|）+ 地面网格（z=0，±6 m）与 RGB 坐标轴 |
| 相机控制（实测校准） | 左键拖动旋转（**拖右＝物体右转、拖下＝相机降低**，两轴方向均按实机反馈校准）；右键或 Shift+左键平移；滚轮缩放（0.5 m ~ 500 m）；**无限俯仰**——越过极点由 `(pitch, yaw) → (±π−pitch, yaw+π)` 参数化翻转（同一眼点的等价参数化，画面无跳变），伴随 `pitch_drag_sign` 翻转保持拖动方向自洽 |
| 拾取 | 视口相机实现 `core_doc::Camera::ray_through`（视口几何每帧喂给相机），单击走 `CoreSession::pick_at` → KernelPicker（网格粗筛 → 内核精算 → 元素反查）全链；命中面橙色高亮、选中构件 35% 混色。mock 腿下命中后精算必然 `Err`（无真面拓扑，"绝不假命中"铁律），日志响亮并**引导用真内核运行**；点击空白处不再静默（补"未命中"日志） |
| 面板 | 顶栏（内核名 / 构件·几何·模块计数 / 适配视图 / 清空选择）；左栏构件列表 + 六条算子按钮（三模块；mock 不支持的算子点击后日志响亮报 `Unsupported`）；右栏属性面板（参数表 / 拾取信息 / 网格统计）；底栏日志（stick_to_bottom，cap 200） |
| CJK 字体 | egui 默认字体无中文字形；运行时从 `C:/Windows/Fonts` 加载 msyh.ttc → simhei.ttf → simsun.ttc → Deng.ttf 兜底，push 到 families 末尾作 fallback |
| `CoreSession` 两个门面 | `tessellate(shape, linear, angular)`（resolve→tessellate 合一，调用方不再拆借用）、`pick_at(screen, camera, filter)`（KernelPicker 转发，`&mut geometry` 与 `&document` 不相交字段借用拆分在门面内完成） |
| 失败口径 | 全程不 panic 不 unwrap（workspace lint `unwrap_used`/`panic` = deny）；装载 / 建几何 / 网格化 / 拾取失败一律写底栏日志 |

**启动**：`cargo run -p bim-desktop --features gui`（mock 内核，秒开）；`--features gui,occt`（**真内核**，首帧 JIT 编译 occt-wasm 慢，属正常）。控制台演示形态不变（无 `gui` feature 时仍走 `run()`）。

**门禁（2026-10-01 实测）**：

| 门禁 | 结果 |
|---|---|
| fmt | ✅ 0 |
| clippy（default / gui / gui,occt 三腿，`-D warnings`） | ✅ 全 RC=0 |
| 默认腿全量 | ✅ **720 passed / 0 failed / 2 ignored**（session 门面未破坏任何既有用例） |
| GUI 实机（mock 腿） | ✅ 窗口持续 32 分 48 秒，退出码 0 |
| GUI 实机（**occt 真内核腿，首次**） | ✅ 窗口持续 **1 小时 51 分 57 秒**，退出码 0（无 panic、无错误输出） |
| occt 腿全量（**M7 遗留拆环收口**） | ✅ **21 passed / 0 failed**（e2e_mock 5 + **e2e_occt 8**〔8 环拆为独立用例，104.56s〕+ e2e_power 5 + e2e_hvac 1 + e2e_hvac_occt 2） |

> egui 0.29 三个版本事实（升级时需复核）：① epaint `TextureId` 只有 `Managed(u64)` / `User(u64)`，字体纹理是 `Managed(0)`——**没有 `Egui` 变体**；② `FontDefinitions.font_data` 直接收 `FontData`（不收 `Arc<FontData>`）；③ 浮点字面量传 f32 参数须写 `1.0_f32`（rustc 淘汰 f32 回退，未来成硬错误）。

---

## 下一步（对应设计文档 10.1 里程碑）

| 里程碑 | 本仓库起点 |
|---|---|
| **M0 技术验证** | ✅ **可验判据 ①②③⑤ 全部通过**（④ 属 M4）。原结论曾作废（`29 passed` 中 14 条 occt 用例静默跳过）；经 **P0 修复（vendor + patch）** 与**适配层三处口径修正**后，`cargo test -p core-kernel --features occt --release` = **62 passed / 0 failed / 0 ignored** |
| **M1 内核适配** | ✅ **代码层已完成**（arena / 后端可信度 / BREP 导入导出 / `scope` 清理 / 内核池硬化 / 装载器 / 落盘闭环）；✅ **真实后端已可实例化**（linker 缺陷由 **vendor + `[patch.crates-io]`** 修复）；✅ **拾取链路已打通**（新增 `KernelBackend::face_hashes`） |
| **M2 数据层** | ✅ **已完成**（2026-09-23 收口）：四类内置基准元素 + Relations + 事务；并补上 §4.3 的**事务强制**（`Document` 字段私有化 + 只读访问器）、§4.6.4 的**轴号编号可配置**（跳 I/O/Z）、以及 §6.4 要用的**关系图遍历原语**（`graph.rs`）。门禁 **474 passed / 0 failed / 2 ignored**，详见上一节 |
| **M3 几何构造** | ✅ **已完成**：新增 `MakeCone`/`MakeTorus`，并接线 `Rotate`/`Extrude`/`Revolve`（**不再**显式 `Unsupported`）；容差对齐设计文档 §5.2（`TOL_POINT`/`TOL_FACE`/`TOL_ANGLE`/`TOL_SELF_INTERSECTION` + 角度容差独立于长度）；布尔复合体按 §5.3 提供 `split_solids`/`single_solid`（多体**响亮报错**、空结果报 `EmptyResult`，绝不静默取 first）；补齐几何查询 `bounding_box`/`surface_area`。门禁 **509 passed / 0 failed / 2 ignored**，occt 腿（`core-kernel`）**89 passed / 0 failed** |
| **M4 互操作基线** | ✅ **已完成（STEP 明确宣告未达成）**：**DXF / glTF / IFC4X3 三格式达标**（DXF 解析实体读写、glTF 真解析结构、IFC 达标线三要素「构件+属性集+空间层级」齐备），**STEP 不交付并显式报 `Err(Unsupported)`**（上游 `occt-wasm` 4.x 构建形态缺陷：文件系统未编入 WASM，host 侧不可修）；三个内核算子 `sew`/`sew_and_solidify`/`fix_shape`/`unify_same_domain` 接线完成；上游 trap 钉成 3 条"注定会失败"的缺陷钉子 + 声明层守卫"一把锁的两面"（含 glTF 导出）。经独立 QA 抓出并修复 3 条 P1。门禁 **533 passed / 0 failed / 2 ignored**，occt 腿（`core-kernel`）**92 passed / 0 failed**、`core-app` **159 passed / 0 failed**，详见上一节 |
| **M5 出图 P0 子集** | ✅ **已完成**：真实视向投影（平面视图绑 `Level`、立面/剖面绑 `Grid`）；HLR 六类曲线 → 可编辑二维实体接线；`BuildOp` 八算子（Fillet/Chamfer/Shell/Offset/Thicken/Loft/Sweep/Pipe）随 M5-B6.2b 接线真实派发（24 变体全接线，后端不具备的能力仍显式 `Unsupported`）；"模型→视图→标注→图纸"闭环 + 关联更新（`rebuild_index`/`sync_drawing_registry`）；DXF 导出圆是真解析实体 |
| **M6 SDK + 示例模块** | ✅ **已完成（2026-09-30 收口）**：五环接口体检全部落为自动化断言（mock 腿 5 条 + occt 腿 8 环 20 断言，真内核全链 17.17s）；容器 v2（`FORMAT_VERSION` 1→2，`drawing.json` + `views/<viewid>.json`，修复「重开丢图纸」）；CoreSession 出图门面（`create_view`/`regenerate_view`/`project_shape`）；`rebuild_geometry` 为 `update_after_commit` 首个生产调用方（复用原 `ShapeId`、演化优先/指纹退避、定向重投+标脏）。门禁 **674 passed / 0 failed / 2 ignored**，bim-desktop occt 腿 6 passed |
| **M7 专业模块** | ✅ **已完成（2026-09-30）**：电力侧 `cable-tunnel`（矩形隧道段 + 电缆排管段 MakeWire→Pipe）+ 建筑侧 `hvac-duct`（矩形风管段 + 变径管 Loft，首个变长算子模块）；变长算子 `Loft`/`MakeWire`（`arity_ok` 单一事实源）；example-beam 第二算子（布尔 Cut 圆孔板）。全部**零 SDK 改动**挂接。门禁 **720 passed / 0 failed / 2 ignored**，三模块 49 单测，bim-desktop occt 腿 **14 passed**（含棱台体积 oracle），详见「M7」节 |
| **M8 桌面 GUI（新增起点）** | ✅ **已完成（2026-10-01）**：egui 0.29 + eframe/wgpu 真实窗口（`src/apps/desktop/src/gui.rs`，`gui` feature 门控）；三维视口（轨道相机 + CPU 画家算法 + 双面光照）、相机控制（实测校准两轴方向 + 无限俯仰过极点翻转）、拾取全链（`ray_through` → KernelPicker）、CJK 字体、`CoreSession` 两个门面（`tessellate`/`pick_at`）。门禁 fmt/clippy 三腿全绿、默认腿 720 passed；GUI 实机：mock 腿 32m48s / **occt 真内核腿 1h51m57s** 均退出码 0，详见「M8」节 |
| **M9 GUI 文件链路（二阶段）** | ⏳ **规划中（2026-10-01 立项）**：`rfd` 原生文件对话框；保存走 `CoreSession::save`、打开按 feature 路由（occt → `open_real`，否则 `open`）；标题栏显示当前文件路径。**验收**：GUI 里保存 → 关闭 → 重开，元素 / 几何 / 图纸全链往返一致；双腿（mock / occt）均可跑。依赖 M8 ✅ |
| **M10 GUI 关联更新（二阶段）** | ⏳ **规划中**：右栏属性面板可编辑（构件参数）→ `rebuild_geometry`（M6 关联更新首链在 GUI 生效）→ 网格重建 + 拾取即时反映新几何 + stale 视图徽章；"重新出图"走 `regenerate_view`。**验收**：GUI 改梁宽 300→500，网格 / 拾取 / stale 提示全部正确。依赖 M9 |
| **M11 出图视图面板（二阶段）** | ⏳ **规划中**：新增"图纸"侧面板：视图列表（真实视向：平面绑 `Level`、立面 / 剖面绑 `Grid`）+ `Entity2D` 投影渲染（线段 / **解析圆** / 弧 / 文字）+ DXF 导出按钮（`core-interop` 既有能力）。**验收**：GUI 建俯视 / 立面视图，2D 图元正确显示且圆是真圆；导出 DXF 外部可打开。依赖 M9 M10 |
| **M12 拾取增强 + 性能（二阶段）** | ⏳ **规划中**：`KernelPicker` 扩展边 / 顶点拾取（§6.6 全链收尾）；网格**增量**重建（现为 dirty 全量重建）+ 拾取 tessellate 结果缓存；性能基线入门禁。**验收**：边 / 顶点拾取命中且高亮与面区分；大装配（千级构件）交互有明确性能边界记录。依赖 M8 M11 |

> **二阶段不规划项**：STEP（上游 `occt-wasm` 4.x 构建形态缺陷，host 侧不可修，见 M4）；DWG（LibreDWG GPLv3+ 绝不可链接主程序，只可独立转换服务，见设计文档 §2 依赖注记与 §7.8）。

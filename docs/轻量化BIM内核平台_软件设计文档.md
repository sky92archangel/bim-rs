# 轻量化 BIM 内核平台 —— 软件设计文档（工程实现篇）

[TOC]

> **文件性质**：面向研发团队（做架构、写代码的人）的工程实现说明书；同时前 3 章写得足够"大白话"，让需要把关技术的领导也能看懂"这东西到底怎么搭起来、我们的钱花在哪"。
> **上游文档**：《轻量化 BIM 内核平台\_立项方案.md》（v4，呈报稿）——那份回答"**该不该做、做多大**"；这一份回答"**到底怎么做、第一行代码从哪写**"。
> **撰写日期**：2026-09-22　|　版本：v1（初稿）
> **参考源码**：`e:/WORKSPACE/CODE_RUST/occt-wasm`（OCCT 的 WASM 封装，几何与出图主力）、`e:/WORKSPACE/CODE_RUST/cadrum`（静态链接 OCCT 的 Rust crate，作为预览级消隐的第二实现与对照）。**本文所有"源码事实"均可在附录 B 的索引里逐条核对。**

---

## 〇、给领导的五分钟导读

### 0.1 这份文档解决什么问题

立项方案决定了"**做一个轻量化 BIM 内核平台（Core），专业模块用 SDK 挂**"。但立项方案不回答下面这些**工程问题**：

| 领导的疑问 | 本文档的哪一章回答 |
|---|---|
| 这软件到底由哪几块拼起来？ | 第一章 总体设计 |
| 代码放在哪、分成几个包、谁依赖谁？ | 第二章 仓库与工程结构 |
| 我们从网上拿来的开源库，怎么用、怎么包？ | 第三章 内核适配层 |
| 数据存哪、怎么存、改了图纸模型会不会自动更新？ | 第四、六章 |
| 四个格式（IFC/STEP/DXF/glTF）怎么落地？ | 第七章 |
| 第三方怎么给我们写模块？ | 第八章 SDK |
| 第一步先干什么、验收看什么？ | 第十章 工程落地指导 |

### 0.2 一句话技术路线（大白话版）

> **我们不从零造"几何计算"这台发动机，而是买一台现成的、很成熟的发动机（OCCT，用 WebAssembly 装进我们的程序），然后自己造"车身、仪表盘、方向盘和改装接口"——也就是数据管理、出图、界面和 SDK。**
>
> 发动机 = 几何内核（算体积、做布尔、算消隐）。车身 = 我们的平台层。改装接口 = SDK，让第二阶段的"强电电气""暖通""电缆隧道"等专业模块能挂上来。

### 0.3 一页架构图（先用文字，后面有详细图）

```
┌─────────────────────────────────────────────────────────────┐
│  第 5 层  UI 层（桌面端 / 浏览器端 / 服务端各自的壳）          │
│   桌面窗口、Web 页面、后台任务面板                            │
├─────────────────────────────────────────────────────────────┤
│  第 4 层  应用服务层（业务动作：新建文档、出图、导入导出）      │
│   文档服务 / 视图服务 / 图纸服务 / 导入导出服务 / 模块运行时    │
├─────────────────────────────────────────────────────────────┤
│  第 3 层  文档与视图层（能力③CAD平面图纸 + 能力②HLR出图）     │
│   二维实体、标注、图纸集、关联更新引擎                        │
├─────────────────────────────────────────────────────────────┤
│  第 2 层  数据层（能力①B-Rep存储管理）                        │
│   文档模型、属性、事务、持久化                                │
├─────────────────────────────────────────────────────────────┤
│  第 1 层  内核适配层（能力⑤几何构造 + 能力④互操作）           │
│   把 occt-wasm 包成我们自己的稳定接口；句柄管理、错误映射       │
└─────────────────────────────────────────────────────────────┘
              ▲ 所有专业模块（能力⑥ SDK）从第 1~4 层暴露的
                接口"挂"进来，不直接碰内核
```

**依赖方向只有一条：上层可以调下层，下层永远不知道上层存在。** 这条铁律决定了 SDK 接口对不对、二次开发能不能低成本长出来。

### 0.4 关键名词对照表（说人话）

| 专业词 | 大白话 | 在我们的代码里叫什么 |
|---|---|---|
| B-Rep | 用"面片、边、角点"精确描述一个立体的数学表示（不是糊在一起的三角网格） | `ShapeHandle` 指向的东西 |
| 几何内核 | 算立体的那台发动机 | occt-wasm 里的 `OcctKernel` |
| HLR（消隐） | 把立体拍扁成一张二维线框图，挡住的部分不画（或画虚线） | `project_edges` |
| 句柄 / Handle | 一张"取货单"，凭它去仓库里取那个立体，不直接拿着立体跑 | `ShapeHandle(u32)` |
| 拓扑 | 立体之间的"谁连着谁"关系（面挨着边、边连着点） | 内核里 OCCT 自己管 |
| 容差 / eps | 判断"两个点算不算同一个点"的尺子 | `1e-6 ~ 1e-8` |
| 事务 | 一批改动要么全成、要么全不动，像银行转账 | `Transaction` |
| 关联更新 | 改了模型，图上的线、尺寸、标注自己跟着变 | `update dependents` |
| XCAF | OCCT 里管"装配树 + 颜色 + 名字"的模块 | occt-wasm 的 `xcaf_*` |
| WASM | 一种能在很多平台跑的"通用可执行格式"，把 C++ 编成它就能到处跑 | `.wasm` 文件 |
| SDK | 给外人用的"改装接口" | `core-sdk` crate |

### 0.5 我们要"用"什么、"造"什么（投资视角）

| 类别 | 用现成的（省钱省时间） | 自己造（真正的壁垒） |
|---|---|---|
| 几何计算 | ✅ `occt-wasm`（OCCT）：布尔、拉伸、倒角、抽壳、求交 | ❌ 不自研 |
| 消隐出图 | ✅ `occt-wasm` 的 `project_edges`（**精确算法**） | 我们要造的是"**把消隐结果变成可编辑的二维图纸**"这一层 |
| 三维渲染 | ✅ `wgpu`（自选） | ❌ |
| 数据存储管理 | ❌ | ✅ **自研**（能力①） |
| 二维图纸与标注 | ❌ | ✅ **自研**（能力③，最难） |
| 四格式互操作 | STEP/glTF 用 `occt-wasm`；DXF 用现成 Rust 库 | **IFC 解析要自研或外购**（唯一缺口） |
| SDK 与模块运行时 | ❌ | ✅ **自研**（能力⑥，决定复利） |

> **给领导的一句话结论**：钱主要花在"数据管理 + 出图层 + SDK"这三块**开源拿不到、也买不到**的地方；几何计算这种"十年才能追上的"部分，我们用集成的方式绕过去。
>
> **另一个省心的结论**：**只用一个内核库（`occt-wasm`）就够了**——几何、HLR、STEP/glTF、装配它全有。**不需要再引入 `cadrum` 之类的第二个内核库**；多引一个，只会多一份依赖与 LGPL 合规评审，却换不来能力。（`cadrum` 详见 1.4 决策二。）

---

## 一、总体设计

### 1.1 设计目标（先立靶子，再看怎么打）

| # | 目标 | 可验收的样子（一句话） |
|---|---|---|
| G1 | 六大基础能力齐备 | 能建模、能存、能出图、能互导、能被别人挂模块 |
| G2 | 出图达到 Revit 级的 **P0 子集** | 改模型 → 平面/立面/剖面/详图 → 标尺寸/标高/标记 → 放进图纸框，全流程能跑通且联动 |
| G3 | 内核可替换 | 将来若换掉 occt-wasm，第 1 层以上代码不用大改 |
| G4 | 专业模块零侵入 | 新专业模块不改 Core 一行代码，只实现 SDK 接口 |
| G5 | 三端同源 | 桌面、浏览器、服务端共用同一套 Core 代码 |

### 1.2 五层架构与依赖倒置（核心）

```
        ┌──────────────┐
        │  UI 层       │  桌面(egui/tauri) / Web(React+WASM) / 服务端(HTTP)
        └──────┬───────┘
               │ 只调"应用服务"
        ┌──────▼───────┐
        │ 应用服务层   │  DocumentService / ViewService / SheetService /
        │ (app)        │  InteropService / ModuleHost
        └──────┬───────┘
               │
   ┌───────────▼───────────┐        ┌────────────────────┐
   │ 文档与视图层 (doc)     │◄──────►│ SDK (sdk)          │
   │ 二维实体/标注/图纸/关联 │  被双方 │ trait + 注册表      │
   └───────────┬───────────┘  依赖  └─────────┬──────────┘
               │                              │
        ┌──────▼───────┐              ┌───────▼────────┐
        │ 数据层(model)│              │ 专业模块(modules)│
        │ 文档/属性/事务│              │ 二阶段才出现     │
        └──────┬───────┘              └────────────────┘
               │
        ┌──────▼───────────────────────────┐
        │ 内核适配层 (kernel)                │  ← 唯一直接碰 occt-wasm 的地方
        │ KernelHandle / 句柄 / 错误 / HLR   │
        └──────────────────────────────────┘
```

**三条必须写进团队公约的规则**：

1. **`kernel` 层往上不暴露任何 `occt_wasm::` 类型。** 上层只看到我们自己定义的 `ShapeId`、`MeshData`、`Projection`。这样将来换内核，只改 `kernel` 一个 crate。
2. **`kernel` 层不允许依赖 `model` 及以上任何东西。** 依赖方向单向向上，杜绝循环依赖。
3. **专业模块只能通过 `sdk` 暴露的 trait 干活。** 模块里出现 `occt_wasm::` 直接打回。

### 1.3 六大基础能力 → 代码模块映射

| 能力 | 承载 crate | 关键接口 | 主要依赖 |
|---|---|---|---|
| ① B-Rep 存储管理 | `core-model` | `Document`、`Element`、`Transaction`、`PropertyMap` | 无 |
| ② HLR 显示 | `core-kernel`（算）+ `core-doc`（用） | `Kernel::project_edges` → `Projection` | occt-wasm |
| ③ CAD 平面图纸编辑 | `core-doc` | `View`、`Annotation`、`Sheet`、`DependencyGraph` | core-model |
| ④ 通用数据导入导出 | `core-interop` | `import_*` / `export_*` | occt-wasm、dxf、gltf、ifc 组件 |
| ⑤ 几何构造 | `core-kernel` | `Kernel::*`（拉伸/布尔/倒角…） | occt-wasm |
| ⑥ 模块二次开发 SDK | `core-sdk` | `GeometryOp`、`PropertySchema`、`AnnotationRule`、`Module` | core-model |

> 注意：能力②的"算"在 `kernel`，"用"在 `doc`。这个拆分是为了让 **出图逻辑不绑死在内核上**——万一将来要换消隐引擎，`doc` 不用动。

### 1.4 两项关键技术决策（已定，附理由）

**决策一：几何与出图主力用 `occt-wasm`（Rust crate 版本）。**

理由（源码级）：
- 它是**唯一同时提供精确 HLR + 完整 B-Rep 算子 + STEP/glTF/XCAF** 的现成方案；
- 它的 HLR 走的是 OCCT 真正的 `HLRBRep_Algo` 算法（见 3.4），**返回的是 B-Rep 曲线，不是折线**——这是"工程图级出图"成立的前提；
- Rust crate 形态嵌入 brotli 压缩的 WASM（约 4.7 MB → 解压约 21 MB），`cargo add` 即可，**不需要 C++ 工具链**。

**决策二：一阶段 `cadrum` 默认不引入——没有它也能把 BIM 做完。**

> **结论先行**：本项目**不依赖 `cadrum`**。它的角色只是"可选对照 / 观察位"，**不是任何能力的必要件**。工程上默认**不加这个依赖**。

理由（源码级）：
- **能力上不缺**：occt-wasm 已覆盖几何构造、**工程图级 HLR**、STEP/BREP/STL/glTF 读写与导出、XCAF 装配、网格化、曲线/曲面查询、演化历史——六项基础能力里凡涉及"内核"的部分它全包，**没有任何一项需要 cadrum 来补**；
- **它的 HLR 不合格**：消隐在 Rust 侧对**三角网格**做（`detect_silhouette_edges` + `classify_edges` 中点采样），输出 `Vec<DVec2>` 折线，**无轮廓/光顺/棱线分类、无可编辑曲线、无溯源**（见 3.6、6.6.5），做不了出图；
- **它也无拾取**：和 occt-wasm 一样要自建拾取，**不带来额外收益**；
- **它反而增加合规负担**：**静态链接 OCCT**，LGPL 2.1 要求"终端用户能重新链接 OCCT"，比 occt-wasm 的 WASM 动态加载更重；
- **它唯一的相对优势**是"原生静态链接、免 WASM 开销"，仅对**模型快照（PNG/SVG）**这类轻用途有意义——**属可选优化，不进一阶段关键路径**。

| 如不确定要不要 cadrum | 做法 |
|---|---|
| 一阶段 | **不引入**。专注 occt-wasm 一条线，少一个依赖、少一份合规评审 |
| 若将来确有"原生性能快照"需求 | 以 `feature = "preview-cadrum"` 的可选开关单独加，**不进默认构建** |
| 若将来 occt-wasm 维护断档（极端情形） | cadrum 可作为"几何能力"的应急备选——但**也需先补出图与拾取**，不是拿来就用 |

> 详见立项方案附录 A / A.4。本设计文档不再重复论证，只在第三章给出 occt-wasm 的接入方式（cadrum 接入示例仅作可选附录，见 3.6）。

### 1.5 三端形态与部署

| 端 | 技术选择 | 内核接入方式 | 说明 |
|---|---|---|---|
| 桌面 | **Rust + egui + wgpu（eframe）**（见 1.6） | `occt-wasm` **Rust crate**（wasmtime） | 主战场，大装配与出图 |
| 浏览器 | React/Vue + TS + wgpu(WebGPU) | `occt-wasm` **npm 包**（Embind） | 轻量协同、评审 |
| 服务端 | Rust（axum 等）+ 无头 | `occt-wasm` **Rust crate** | 批量转换、批量出图 |

> **重要提醒**：桌面/服务端用 **crate**，浏览器用 **npm 包**，两者的 API 名字不同（Rust 是 `snake_case`，TS 是 `camelCase`），但**底层是同一个 WASM、同一套方法**。我们的 Core 逻辑若想三端复用，必须把"业务逻辑"写在**与语言无关的抽象之上**，或者接受桌/服务端共用 Rust、浏览器端维护一层 TS 壳。

### 1.6 桌面端 GUI 选型：egui / Tauri / Slint 怎么选

**先厘清一件事**：这三者**不在同一层**。`Tauri` 是"应用外壳 + 系统 WebView（HTML/JS）"；`egui` 与 `Slint` 是 **Rust 原生 GUI 工具包**。放在"**3D 视口 + 鼠标转/移/缩放/点选**"这个场景里，真正的分水岭只有一条：**谁能让你干净地拥有 3D 渲染面与输入循环。**

| 方案 | 是什么 | 3D 视口怎么放 | 鼠标交互 | 性能 | 适配结论 |
|---|---|---|---|---|---|
| **egui + wgpu（eframe）** | Rust 原生即时模式 GUI | **一等公民**：`egui_wgpu` 的 paint callback 让你在 egui 面板里**直接跑自己的 wgpu 渲染通道** | 走 `winit`，鼠标位置/拖拽/滚轮在**同一事件循环**，最直接 | **原生 wgpu，延迟最低、内存最小** | ✅ **推荐：CAD 视口为主** |
| **Tauri + Web 前端** | 应用外壳 + 系统 WebView | 视口在 WebView 里（WebGPU/WebGL + three.js 等） | DOM 事件，成熟好用 | WebView 引擎 + IPC + 浏览器内 WASM，开销更高 | 若要与浏览器端**共用同一套前端** |
| **Slint** | Rust 声明式 GUI | 经 `set_rendering_notifier` 拿底层图形上下文（**主要 OpenGL 路径**；wgpu 需桥接） | 有事件，但 **3D 集成要自己搭** | 原生、轻量、省内存 | UI 为主、**3D 需求轻** |

**结论：桌面主客户端用 `egui + wgpu`（eframe）。** 四条理由：

1. **与架构同源**：Core 是 Rust、内核是 occt-wasm 的 **Rust crate**（wasmtime 原生跑）→ **全在一个原生进程里**，没有 WebView、没有 IPC、没有"浏览器里再跑一层 WASM"的**双重开销**；
2. **视口是一等公民**：`egui_wgpu` 的 paint callback 让我们在自己的 wgpu 管线里画视口——**拾取的 ID Buffer 回读、高亮、尺寸预览都在这条通道里完成**（呼应 6.6）；
3. **输入最直接**：鼠标转/移/缩放/框选/点选全在同一事件循环，实现最省事、延迟最低；
4. **即时模式天然适合 CAD**："工具 + 视口 + 悬浮操作"（拖尺寸、拉手柄、吸附提示）正是 immediate-mode 的强项。

**什么时候换：**

- **要和浏览器端复用同一套前端** → 选 **Tauri**。代价：**WebGPU 在各系统 WebView 的支持不均**（Windows WebView2 好；macOS WKWebView 需较新系统；Linux WebKitGTK 弱），必须按目标系统实测并准备 **WebGL 回退**；
- **只做轻量查看器 / 嵌入式** → 选 **Slint**（省内存），但 3D 视口要用 **OpenGL notifier** 自己搭，且与我们的 wgpu 渲染栈**不同源**，等于维护两套渲染。

**性能说人话：**

- 三者"UI 框架本身"的开销差别，**通常远小于你自己的几何/渲染开销**。真正的重活是 **occt-wasm 几何计算 + 你的 wgpu 渲染 + 拾取回读**——这块在 egui 和 Slint 下**完全一样**（都得你自己写）；
- egui 即时模式在交互时**每帧重绘整个 UI**；对 CAD 面板规模无压力，若要极致可做事件驱动重绘 + 视口离屏缓存；
- Tauri 的额外开销来自 **WebView 引擎 + 前后端 IPC + 浏览器内的 WASM**，**大装配时更明显**。

> **一句话选型**：**桌面 = egui + wgpu**；浏览器 = TS + WebGPU；服务端 = 无头 Rust。三者共用 Core，只在最外层的"壳"上分叉。


---

## 二、仓库与工程结构（指导"软件工程怎么建"）

### 2.1 总体布局：一个 workspace，多个 crate

推荐 **Cargo workspace（单仓多包）**，而不是拆成多个 Git 仓库。理由：Core 各层耦合紧、需要原子提交与统一 CI。

```
bim-core/                          ← 仓库根
├─ Cargo.toml                      ← workspace 清单（见 2.3）
├─ rust-toolchain.toml             ← 钉死工具链版本
├─ .github/workflows/ci.yml        ← 门禁（见 2.6）
├─ xtask/                          ← 构建/代码生成/发布脚本（Rust 写脚本）
├─ crates/
│  ├─ core-kernel/                 ← 第 1 层：内核适配（唯一碰 occt-wasm）
│  ├─ core-model/                  ← 第 2 层：数据模型
│  ├─ core-doc/                    ← 第 3 层：二维图纸 + 关联更新
│  ├─ core-interop/                ← 第 1.5 层：四格式互操作
│  ├─ core-sdk/                    ← 第 4 层：SDK 契约
│  ├─ core-app/                    ← 第 4 层：应用服务
│  └─ core-testkit/                ← 测试夹具与黄金样本
├─ modules/                        ← 二阶段专业模块（一阶段留空 + 一个示例）
│  └─ example-beam/                ← 最小示例模块，用于验证 SDK 接口
├─ apps/
│  ├─ desktop/                     ← 桌面端壳
│  ├─ server/                      ← 服务端壳
│  └─ web/                         ← 浏览器端（TS + npm 包）
└─ testdata/                       ← STEP/DXF/IFC/glTF 测试样本
```

### 2.2 每个 crate 的职责与依赖方向（一张表说清）

| crate | 职责 | 允许依赖 | **禁止依赖** |
|---|---|---|---|
| `core-kernel` | 包装 occt-wasm；句柄、错误、HLR、网格化 | `occt-wasm`、`thiserror` | 其它 core-* |
| `core-model` | 文档/元素/属性/事务（纯数据，不含几何算法） | `serde`、`uuid` | `occt-wasm`、其它 core-* |
| `core-doc` | 二维实体、标注、图纸、关联更新 | `core-model` | `occt-wasm`（通过 kernel 的抽象接口） |
| `core-interop` | 四格式读写 | `core-kernel`、`core-model`、dxf/gltf/ifc 库 | `core-doc` |
| `core-sdk` | trait 定义 + 注册表（**只定义，不实现**） | `core-model` | 具体模块 |
| `core-app` | 用例编排（打开文档、出图、导入导出） | 以上全部 | UI |
| `apps/*` | 界面/进程入口 | `core-app` | — |
| `modules/*` | 专业功能 | `core-sdk`（**只允许这一个**） | 其它 core-* |

> **依赖方向自检法**：画一张箭头图，如果既有"向上"又有"向下"的箭头，就是循环依赖，CI 里用 `cargo deny` 或 `cargo machete` 检查。

### 2.3 Cargo workspace 配置（可直接用）

```toml
# Cargo.toml（仓库根）
[workspace]
resolver = "3"
members = [
    "xtask",
    "crates/core-kernel",
    "crates/core-model",
    "crates/core-doc",
    "crates/core-interop",
    "crates/core-sdk",
    "crates/core-app",
    "crates/core-testkit",
    "modules/example-beam",
    "apps/desktop",
    "apps/server",
]

[workspace.package]
edition = "2024"
rust-version = "1.95"      # occt-wasm 要求 Rust 1.95+（见其 rust-toolchain.toml）
license = "Proprietary"    # 我们平台层闭源；仅内核是 LGPL 的 WASM 产物

[workspace.dependencies]
# 几何内核：仅 core-kernel / core-interop 使用（一阶段唯一内核依赖）
occt-wasm = { version = "4", default-features = false }
# ⚠ 版本订正（2026-09，M0 实测）：本文写作时为 v3；crates.io/镜像实测最新为 **4.0.0**，
#   已按 v4 接入（`crate/src/occt.rs`）。v4 的公开 API 与本文 3.4 记录的签名**逐字一致**，
#   无需改动章节内容；另 v4 依赖 **wasmtime 48**，因此 **release 构建**是实测要求
#   （debug 下 wasmtime 编译 21 MB WASM 慢约 100 倍）。
# cadrum：一阶段【不引入】。若将来需要原生性能快照，再按可选 feature 单独加，
# 且必须走独立合规评审（静态链接 OCCT 的 LGPL 义务更重）。见 1.4 决策二。
# cadrum = { version = "0.8", optional = true }

# 通用
thiserror = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = { version = "1", features = ["v7", "serde"] }
glam = "0.30"              # 向量/矩阵；与 occt-wasm 的 Vec3 各司其职（我们内部统一用 glam）

# 互操作
gltf = "1"
dxf = "0.6"                # dxf-rs
# IFC：纯 Rust、MPL-2.0 —— 二选一，实测后定（见 7.5）
# ifc-lite-core = "1"      # 备选 A：IFC4X3 + IFC5/IFCX
# bimifc-parser = "0.2"    # 备选 B：IFC4(STEP) + IFC5(IFCX)
# 注意：DWG 不在本清单——LibreDWG(GPLv3+) 绝不可链接进主程序，只能包成独立转换服务，
# 经 IPC/HTTP 调用（见 7.8）。

# 二维绘制（出图/投影/无新依赖，见 7.7）
kurbo = "0.11"
lyon = "1"

# 渲染（桌面/服务端可选）
wgpu = "24"

# 桌面 GUI（仅 apps/desktop 使用）——选型见 1.6
# 版本号以实际选用时的最新稳定版为准；关键是 egui + eframe(wgpu) + egui-wgpu 三者版本对齐。
egui = "0.29"
egui-wgpu = "0.29"
eframe = { version = "0.29", default-features = false, features = ["wgpu"] }

[workspace.lints.rust]
unsafe_code = "deny"       # 内核适配层可局部 allow，但需评审
[workspace.lints.clippy]
unwrap_used = "deny"       # 生产代码禁止 unwrap，测试可豁免
panic = "deny"
```

> **为什么 `unsafe_code = "deny"`**：occt-wasm 已经把所有 `unsafe` 封在它自己内部（它的 crate 承诺 FFI 安全）。我们的代码没有理由再碰裸指针。cadrum 的 wasm 初始化需要 `unsafe { __wasm_call_ctors() }`，那部分单独 `#[allow(unsafe_code)]` 并写清 SAFETY 注释即可。

### 2.4 构建流程

| 命令 | 作用 |
|---|---|
| `cargo build` | 常规构建（occt-wasm 自动下载/内嵌 WASM，无需 C++ 工具链） |
| `cargo xtask test` | 跑全部测试（含黄金图对比） |
| `cargo xtask gen-samples` | 生成测试用 STEP/DXF/glTF 样本（用于回归） |
| `cargo xtask bench` | 跑性能基线并与 `benchmarks/baseline.json` 对比 |
| `cargo xtask dist` | 打包三端产物 |

> **借鉴 occt-wasm 的做法**：它用 `xtask`（一个 Rust 二进制）统一编排构建与**代码生成**，并且把"生成的代码"提交进仓库、CI 检查是否漂移（drift）。我们若需要**在自己 fork 上给内核加方法**，也走同样的模式（见 3.8）。

### 2.5 代码规范（团队公约）

- **Rust edition 2024**，与 occt-wasm 对齐；
- 生产代码禁 `unwrap` / `panic` / `unsafe`（lint 强制，测试模块可 `#[allow]`）；
- 所有公开 API 必须有文档注释（`///`），说明**参数单位、坐标系、容差、失败条件**；
- 单位统一 **毫米（mm）**；角度统一 **弧度**（对外接口注意在文档里标注）；坐标系统一 **Z 轴朝上、右手系**；
- 错误类型分层：`kernel` 抛 `KernelError`，`model` 抛 `ModelError`，`app` 汇聚成 `AppError` 并携带**可展示给用户的中文消息 + 可上报的错误码**。

### 2.6 CI 门禁（合并前必过）

```
lint      : cargo fmt --check / clippy -D warnings / 文档链接检查
deps      : cargo deny（许可证白名单 + 禁止循环依赖）
unit      : cargo test（纯逻辑，秒级）
golden    : 出图黄金样本对比（同一模型同一视图，像素/矢量差异在阈值内）
bench     : 性能回归门禁（超阈值失败，参考 occt-wasm 的 bench-check 思路）
license   : 检查发布产物是否携带 LGPL 声明（合规检查点）
```

> **licence 门禁特别说明**：occt-wasm 是 **crate MIT/Apache + 内嵌 WASM LGPL-2.1-only**。我们的**发布产物里必须随附 LGPL 声明，并保证终端用户能替换 WASM 组件**。这条要在 CI 里做成硬门禁（见 10.4 风险 R4）。

---

## 三、内核适配层 `core-kernel`（把 occt-wasm 包成自己的）

### 3.1 为什么要多包一层

直接调 `occt_wasm::OcctKernel` 当然能跑，但会带来三个后果：
1. **换内核**（万一将来 OCCT-WASM 维护断档）要改遍全项目；
2. 上层的业务代码里会散落 `ShapeHandle`、`OcctError` 这些"内核方言"；
3. 内核的**创建慢、调用快**这个特性若不在架构里体现，性能会一塌糊涂。

所以 `core-kernel` 对外只暴露**我们自己的名词**：

```rust
// core-kernel/src/lib.rs —— 对外唯一入口，零 occt_wasm 类型泄漏
pub struct Kernel { /* 内部持有 occt_wasm::OcctKernel */ }
pub struct ShapeId(u64);                 // 我们的句柄（不是 occt 的 u32）
pub struct MeshData { /* positions/normals/indices/face_groups */ }
pub struct Projection { /* 6 组 2D 曲线 */ }
pub enum KernelError { /* 我们自己的错误枚举 */ }

impl Kernel {
    pub fn new() -> Result<Self, KernelError>;
    pub fn make_box(&mut self, dx: f64, dy: f64, dz: f64) -> Result<ShapeId, KernelError>;
    pub fn fuse(&mut self, a: ShapeId, b: ShapeId) -> Result<ShapeId, KernelError>;
    pub fn tessellate(&mut self, s: ShapeId, linear: f64, angular: f64) -> Result<MeshData, KernelError>;
    pub fn project_edges(&mut self, s: ShapeId, view: ViewDef) -> Result<Projection, KernelError>;
    // ...
}
```

### 3.2 内核实例的生命周期（性能关键）

源码事实：`OcctKernel::new()` 会 brotli 解压并 JIT 编译约 21 MB 的 WASM，**耗时约 100–500 ms**；而单次调用很快。occt-wasm 自己的建议原文是：

> "Re-use one `OcctKernel` per worker thread; creation is the slow part, individual calls are cheap."（每个工作线程复用一个 `OcctKernel`；创建是慢的那部分，单次调用很便宜。）

因此设计上：

```rust
// core-kernel/src/pool.rs —— 线程本地内核池
thread_local! {
    static KERNEL: RefCell<Option<Kernel>> = const { RefCell::new(None) };
}

pub fn with_kernel<R>(f: impl FnOnce(&mut Kernel) -> R) -> R {
    KERNEL.with(|k| {
        let mut slot = k.borrow_mut();
        let kernel = slot.get_or_insert_with(|| Kernel::new().expect("kernel init"));
        f(kernel)
    })
}
```

**规矩**：
- 一个线程一个内核实例，**绝不每次操作用完就丢**；
- 大任务用线程池，每个工作线程各自持有一个内核；
- 浏览器端用 `OcctWorker`（Web Worker）把内核挪出主线程，避免卡界面；
- 服务端做"进程级单例 + 请求排队"，或"每 worker 一实例"，由压测决定。

> **M0 实测补充（2026-09，重要）**：上面这条"绝不每次操作都新建内核"**不是性能优化，是硬约束**。
> M0 探针测试最初让 11 个用例各自 `OcctBackend::new()`，在默认的多线程并行测试下**直接把进程撑到 OOM**——
> 报 `memory allocation of 1048576 bytes failed`，退出码 `0xc0000409 (STATUS_STACK_BUFFER_OVERRUN)`。
> 注意这个失败的样子：**测试输出里一行 `FAILED` 也没有**，只有 `error: test failed`，极易被误读成代码回归。
> 改为进程内单实例（`OnceLock<Mutex<OcctBackend>>`）后不 OOM，且耗时 `91.72s → 18.67s`。
> **代价**：这里的 arena **不会自动回收**（内核只提供 `release` / `release_all` / `checkpoint` / `release_since`，
> **没有** `is_alive`），测试里必须用 `release_all` 显式清场，否则 `live_count` 类断言会随用例执行顺序漂移。

### 3.3 句柄体系：会话态 vs 持久态（**这是最重要的设计点**）

问题：occt-wasm 的 `ShapeHandle(u32)` 是**沙箱 arena 里的临时编号**，程序一关就没了。而 BIM 文档要能存盘、能跨会话打开、能版本管理。所以必须把两者**彻底分开**：

| 概念 | 谁管 | 生命周期 | 用途 |
|---|---|---|---|
| `ShapeId`（持久态） | 我们的 `core-model` | 跟文档一样长，可存盘 | 文档里每个构件的"身份证号" |
| `ShapeHandle`（会话态） | occt-wasm arena | 进程内的临时号 | 当前会话里算几何用 |

**对应关系**用一个映射表维护：

```rust
// core-kernel/src/arena.rs
pub struct Kernel {
    inner: occt_wasm::OcctKernel,
    ids: HashMap<ShapeId, occt_wasm::ShapeHandle>,   // 持久ID → 会话句柄
    next: u64,
    refs: HashMap<ShapeId, u32>,                     // 引用计数（见 3.7）
}
```

**存盘时不存几何**（几何以 STEP/BREP 文本块存，见 4.2），打开文档时**按需重建**句柄。原则：
- **惰性重建**：打开大文档不一次性把所有几何都塞进内核，用到哪个构件才把它的 STEP 读进来；
- **句柄可丢**：会话句柄丢了不影响文档，下次用时从持久几何重建；
- **持久ID永不复用**：用 UUID v7（时间有序），避免"删了又建"导致的引用错乱。

### 3.4 HLR 出图接口（能力②的"算"）—— 源码级说明

occt-wasm 的投影方法（Rust crate）签名（源码 `crate/src/kernel_generated.rs`）：

```rust
pub fn project_edges(
    &mut self,
    shape_id: ShapeHandle,
    ox: f64, oy: f64, oz: f64,      // 视点 origin
    dx: f64, dy: f64, dz: f64,      // 视线方向（从物体指向观察者）
    xx: f64, xy: f64, xz: f64,      // 屏幕向右轴（可省略，OCCT 自选）
    has_x_axis: bool,
) -> OcctResult<ProjectionData>;
```

返回的 `ProjectionData`（源码 `crate/src/types.rs`）是**六个 B-Rep 复合体句柄**：

```rust
pub struct ProjectionData {
    pub visible_outline: ShapeHandle,  // 可见·轮廓线（外轮廓，HLR 的 OutLineV）
    pub visible_smooth:  ShapeHandle,  // 可见·光顺边（相切连续，Rg1LineV）
    pub visible_sharp:   ShapeHandle,  // 可见·尖锐边（真实棱边，VCompound）
    pub hidden_outline:  ShapeHandle,  // 隐藏·轮廓线（OutLineH）
    pub hidden_smooth:   ShapeHandle,  // 隐藏·光顺边（Rg1LineH）
    pub hidden_sharp:    ShapeHandle,  // 隐藏·尖锐边（HCompound）
}
```

**为什么这个设计很关键**：OCCT 的 HLR 底层（`facade/generated/kernel.cpp`）用的是：

```cpp
Handle(HLRBRep_Algo) hlr = new HLRBRep_Algo();
hlr->Add(shape, 0);
HLRAlgo_Projector projector(ax2);
hlr->Projector(projector);
hlr->Update();
hlr->Hide();
HLRBRep_HLRToShape hlrShapes(hlr);
result.visibleOutline = storeIfNotNull(hlrShapes.OutLineVCompound());
// ... 其余五类同理
```

也就是说：**它输出的六类结果都是"形状"（可继续查拓扑、提曲线），不是折线、不是位图**。这正是"工程图级 HLR"与"预览级 HLR"的分水岭（对照 cadrum 的网格折线输出，见 3.6）。

**我们的 `Projection` 类型**（`core-kernel`）：

```rust
pub struct Projection {
    pub visible: Vec<ProjectedCurve>,
    pub hidden:  Vec<ProjectedCurve>,
}
pub struct ProjectedCurve {
    pub kind: CurveKind,       // Line / Circle / Arc / Ellipse / BSpline / Other
    pub data: CurveGeometry,   // 解析参数（圆心/半径）或 NURBS 控制点
    pub layer_hint: LineClass, // 轮廓 / 光顺 / 尖锐（决定线型：实线/细线/虚线）
}
```

### 3.5 从 HLR 结果提取"可编辑的二维曲线"（出图的核心工序）

拿到六个 compound 之后，要做四步：

1. **拆边**：用 `get_sub_shapes(handle, "edge")` 把 compound 拆成一条条边；
2. **判型**：对每条边查其底层曲线类型（直线 / 圆 / 椭圆 / B 样条）；
3. **提参**：
   - 直线 → 起点终点；
   - 圆 → 圆心 + 半径（**这样画出来才是圆，才能标半径、才能捕捉圆心**）；
   - B 样条 → NURBS 数据（occt-wasm 的曲线类目提供 NURBS 提取：`degree/knots/poles/weights`）；
4. **打包**成 `ProjectedCurve`，交给 `core-doc` 变成二维实体。

> **前置验证项（必须最先做）**：确认 `project_edges` 输出的边能正确判出 `Circle` 而非退化成 `Other`。若某些曲线类型判不出来，退路是**用 NURBS 数据保真存储**（仍然是精确曲线，不是折线），DXF 导出时再转 SPLINE。**这是出图目标能否成立的第一号技术验证。**

### 3.6 预览级消隐的第二实现（cadrum，**一阶段不引入**）

> **本节只是"如果将来要，怎么接"的备忘，不是一阶段要做的事。**
> 一阶段**默认不依赖 cadrum**（见 1.4 决策二、2.3 已注释掉该依赖）。**没有它，BIM 的六项能力一样齐全**——凡涉及内核的能力，occt-wasm 全包。
> 保留本节的意义有二：① 若将来需要"原生性能的模型快照（PNG/SVG）"，可按下面的 `feature` 开关单独加；② 作为技术对照，说明"为什么它不能当主力"。

对"给评审/LLM 看一张快照"这类需求，我们接入 cadrum 作为第二实现：

```rust
// 仅在 feature = "preview-cadrum" 时编入
use cadrum::{Solid, SceneOption, DVec3};

pub fn preview_svg(solid: &Solid, view: DVec3) -> String {
    let mesh = Solid::mesh([solid], Default::default()).unwrap();
    let scene = mesh.scene(SceneOption { view, hidden_edges: true, ..Default::default() });
    // scene.edges_visible / scene.edges_hidden 是 NaN 分隔的折线
    render_svg(&scene)
}
```

**关键区别（团队必须牢记）**：

| 维度 | occt-wasm `project_edges` | cadrum `Mesh::scene` |
|---|---|---|
| 算法基础 | **B-Rep**（`HLRBRep_Algo`） | **三角网格**（`detect_silhouette_edges`） |
| 输出 | 六类形状（可提精确曲线） | `Vec<DVec2>` 折线（NaN 分隔） |
| 圆的表示 | 精确圆（可标半径） | 离散折线 |
| **边的分类** | **轮廓母线 / 光顺边 / 棱线 三类分开输出** | **只算"轮廓边"一类**，与棱线混在一起，不区分 |
| **可否作标注依据** | **可**（分类 + 精确曲线 + 可追溯） | **不可**（无分类、无精确曲线、无溯源） |
| 拾取 / 选择 API | ❌ 无（见 6.6） | ❌ 无 |
| 用途 | **出图主力** | 预览 / 快照 / 对照 |
| 许可 | WASM 产物 LGPL-2.1-only | 静态链接 OCCT，LGPL 义务更重 |
| 性能 | 有 WASM 开销（约 2×） | 原生，更快 |

> **一句话**：cadrum 的 `scene()` 源码里 `let all_edges = silhouette_edges`——它**只把"轮廓边"一种边**放进 2D 场景（`detect_silhouette_edges` 从三角形邻接按朝向翻转判定），**既不分轮廓/光顺/棱线，也没有 B-Rep 溯源**。作为"看图的快照"够用，作为"标注的依据"不够。

### 3.7 资源释放与引用计数

occt-wasm 的 arena **不会自动回收**（源码 `CLAUDE.md`：IDs are never auto-freed）。TS 侧靠 `Symbol.dispose` + `FinalizationRegistry`。Rust 侧我们要自己做：

- `ShapeId → ShapeHandle` 映射表里维护**引用计数**；
- 文档里某构件的所有引用都断开时，通知内核释放对应句柄（如果 occt-wasm 提供 release；否则记录"冗余句柄"，在做大装配前统一清理）；
- **大装配场景建议按"文档分页"重建内核句柄**，避免 arena 无限膨胀。

### 3.8 如果需要内核没暴露的方法

occt-wasm 的**中央接缝**是代码生成：`xtask/src/codegen/config.rs` 里有一张声明式的 `MethodSpec` 清单，`cargo xtask codegen` 会从它生成四个文件（C++ facade、Embind 绑定、WASI 导出、Rust 生成代码），生成文件是**提交进仓库**的，CI 会检查漂移。

因此路线是：
1. 先查 occt-wasm 是否已有对应方法（170+ 方法，大概率有）；
2. 没有 → 在 **fork** 上加一条 `MethodSpec` → `cargo xtask codegen` → 重建 WASM；
3. 走 upstream PR 争取合并，避免长期维护 fork。

> **注意**：`crate/src/occt-wasm.wasm.br` 是**提交进仓库的产物**，本地重建与 CI 不完全字节一致，重新嵌入必须取 CI 的产物（见其 `CLAUDE.md`）。这是维护 fork 时最容易踩的坑。

---

## 四、数据层 `core-model`（能力① B-Rep 信息存储与管理）

### 4.1 数据模型：文档 → 元素 → 几何/属性

```
Document（一个 .bimdoc 文件）
 ├─ Meta（名称 / 单位 / 坐标系 / 作者 / 版本）
 ├─ Elements（元素字典：id → Element）
 │    ├─ Element { id, category, name, geometry_ref, properties, relations }
 │    │     ├─ geometry_ref → GeometryRecord（指向几何存储）
 │    │     └─ properties  → PropertyMap（参数：标高、材质、截面……）
 │    └─ ...
 ├─ Views（视图：平面/立面/剖面/详图）
 ├─ Sheets（图纸集）
 └─ Relations（关联关系图，供自动更新用）
```

**关键区分**：`Element` 是"语义对象"（一根柱、一段桥架），`GeometryRecord` 是"几何数据"。一个元素可以没有几何（纯标注），也可以引用一段共享几何。

### 4.2 持久化设计：自建容器 + 内嵌标准格式

**不打自研一套 B-Rep 二进制格式**（那是重复造轮子且极易出错）。方案是：

```
xxx.bimdoc（其实是一个 zip / 目录）
 ├─ doc.json          # 文档结构：元素、属性、视图、图纸、关系（人可读、可 diff）
 ├─ geom/
 │    ├─ <shapeid>.brep   # OCCT BREP 文本格式（精确几何）
 │    └─ <shapeid>.glb    # 可选：网格缓存，打开时先显示它，几何后台重建
 ├─ views/<viewid>.json   # 二维视图数据（实体 + 标注）
 └─ assets/               # 字体、图框、符号
```

| 存什么 | 用什么格式 | 理由 |
|---|---|---|
| 结构 / 属性 | JSON | 可读、可 diff、易调试、版本管理友好 |
| 精确几何 | **OCCT BREP**（`Solid::write_brep` / occt-wasm 的 BREP 导出） | OCCT 原生、无损、单文件 |
| 交换几何 | STEP | 对外交付 |
| 显示缓存 | glTF（`.glb`） | 打开即显示，不必等几何重建 |

> occt-wasm 支持 STEP / STL / **BREP（文本+二进制）** 导入导出；cadrum 也支持 `read_brep` / `write_brep`（OCCT `BinTools` 格式）。两者 BREP 都是 OCCT 原生格式，**优先用文本 BREP**（可读、可 diff）。

> **⚠️ 落地实测（2026-09-23，已实现）** —— 容器已按本节落地在 `core-app/src/store.rs`，与设计的三点差异：
>
> 1. **多一个 `manifest.json`**：放格式版本号（`FORMAT_VERSION`），打开时第一个读，用于版本识别与将来迁移。
> 2. **`doc.json` 是唯一权威副本；`geom/<shapeid>.brep` 是必须一致的导出副本**。打开时逐条 `verify_geometry` 比对
>    `GeometryRecord.brep` 与对应文件内容，**不一致或缺失就报错**——不静默采信任何一侧。
> 3. **写盘前必须过滤会话态**：`doc.json` 落的是 `Document::to_persistable()`，它会剔除 `DatumScope::Session` 的基准元素
>    （见 4.6.3 与第 735 行那条纪律）。**这条规则放在数据层（`core-model`），不在 app 层复制。**
>
> 另外提供了 `pack_to_dir` / `open_from_dir` 的**目录形态**：`.bimdoc` 本质是 zip 或目录，目录形态便于调试与 diff。
> `geom/<shapeid>.glb` 网格缓存与 `views/` 视图数据在 M2/M5 落地，容器已预留位置。

### 4.3 事务与撤销

```rust
pub struct Transaction<'d> {
    doc: &'d mut Document,
    undo_log: Vec<Delta>,
}
impl<'d> Transaction<'d> {
    pub fn begin(doc: &'d mut Document) -> Self;
    pub fn add_element(&mut self, e: Element) -> ElementId;   // 记录 delta
    pub fn commit(self) -> Result<(), ModelError>;             // 一次性落盘
    pub fn rollback(self);                                     // 全部丢弃
}
```

- 所有对文档的修改**必须**走事务（CI 里检查 `Document` 的字段私有化即可强制）；
- `Delta` 要**可序列化**，这样"撤销栈"也能存盘（大文档恢复更稳）；
- 每次 commit 产生一个**版本号**（单调递增），供视图/图纸判断"要不要重算"。

> **⚠️ 落地实测（2026-09-23，M2 收口已实现）** —— 第一条纪律原先**没有落实**：`Document` 的 8 个字段
> （`meta`/`elements`/`datums`/`geometry`/`relations`/`views`/`sheets`/`version`）当时**全是 `pub`**，
> 且已实证存在绕过事务的路径（`core-app/src/geometry_service.rs` 直接 `doc.geometry.remove(...)`）。
> 现已按本节要求**全部私有化**：只提供 9 个 `&self` **只读访问器**（`meta()` / `elements()` / `datums()` /
> `geometry()` / `relations()` / `views()` / `sheets()` / `version()` / `geometry_count()`），**不提供任何
> `*_mut` 访问器**；补唯一受控修改口 `remove_geometry()`；并把 12 个文件的调用点迁到访问器（含 `core-doc`、
> `core-interop`、`apps/desktop`）。
>
> **"字段保持私有"由源码级守卫钉住**（`core-model/src/document.rs` 的
> `document_fields_stay_private_source_guard`）：取 `Document` 定义体 → 剥注释 → 去空白归一化 →
> 断言体内**不出现 `pub`**。之所以不用 `compile_fail` doctest：本沙箱跑不了它（rustdoc **非合并**路径要给
> rustc 接**管道 stdin**，`CreateProcess` 报 `ERROR_PIPE_BUSY`/`os error 231`；`no_run` 走**合并**路径故可跑）——
> 真机/CI 上可把该 doctest 加回。**注意守卫不能用"逐字段字面匹配 `pub {field}:`"**：那样会漏掉
> `pub(crate)`（实测打穿过），反而给出假的保证。

### 4.4 属性体系（专业扩展的入口）

用**模式（Schema）+ 实例**两级：

```rust
pub struct PropertySchema {           // 由专业模块注册（见第八章）
    pub type_name: String,            // 如 "POWER_CABLE_TRAY"
    pub fields: Vec<FieldDef>,        // 名称/类型/单位/枚举/默认值
}
pub struct PropertyMap {              // 元素实例上挂的值
    schema: String,
    values: BTreeMap<String, Value>,
}
```

**核心原则**：`core-model` **不认识任何专业字段**。它只知道"有个模式叫 X、它有哪些字段"。强电电气的字段、暖通的风量、电缆隧道的支架间距，**全部由模块注册**。这样 Core 永远不会因为"加了个专业"而要改。

### 4.5 与内核句柄的解耦（重申 3.3）

`core-model` **完全不 import `occt_wasm`**。几何在文档里是"`GeometryRecord`（BREP 文本）"，需要算的时候由 `core-app` 通过 `core-kernel` 现场把 BREP 读成句柄。**数据层是纯数据，零几何知识。**

### 4.6 基准与定位元素（轴网 / 标高 / 参考面 / 参考线）——**必须内置在 Core**

> **一句话结论**：这四类是**全专业共用的"定位地基"**，不是某个专业的功能，所以**必须预制在 Core 里、且是内置元素类型**——不能让它们落到 SDK 模块去注册。

#### 4.6.1 为什么必须内置（四个理由，缺一个都成立）

| # | 理由 | 不内置会怎样 |
|---|---|---|
| 1 | **坐标统一** | 建筑专业与电力专业各自定义轴网 → 两套坐标系拼不到一个模型里，变电站与厂房永远对不齐 |
| 2 | **定位宿主** | 墙、柱、设备、电缆层支架、桥架都靠"**标高 X 上的 Y 轴**"定位。定位关系的"锚"必须由平台给，否则每个模块各造一套，跨模块互认失败 |
| 3 | **视图生成的依赖** | **平面图 = 在某标高做水平剖切 + 投影；立面图 = 沿某轴网方向投影；剖面图 = 垂直于某轴**。没有标高/轴网，就没有"平立剖"视图体系——而视图体系是能力②③的骨架 |
| 4 | **标注的基准** | 轴网标注（`1-2-3` / `A-B-C`）、标高标注（`±0.000`）是出图 P0 条目，**它们直接引用基准元素**，不是自己画出来的文字 |

> **判据（一句话）**：凡是"**别的元素要靠它定位、视图要靠它生成**"的东西，就是基础设施，必须在 Core。

#### 4.6.2 关键设计原则：把"基准"和"几何"分开

**最容易踩的坑**：把轴号文字、标高符号当几何画进模型。结果模型里全是文字图元，改一个轴号要在整图里找，视图联动全废。正确做法是**一个基准元素横跨四层**，各层各归各位：

| 层 | 内容 | 归属 | 说明 |
|---|---|---|---|
| **语义层** | 轴号/名称、标高值、类型、可见性、是否钉住、所属工作集 | `core-model` | **这才是"轴网/标高"的本体** |
| **几何层** | 轴网曲线（直线/圆弧/多段）、标高的水平面与基准点 | `core-kernel` | 内核只见几何，**不需要内核懂"轴网"概念** |
| **显示层** | 轴号圈、标高符号（△ / ±0.000）、延长线、气泡、当前视图的裁切范围 | `core-doc` / UI | 出图时才渲染，**不存进几何** |
| **关系层** | 谁挂在它上（构件定位）、哪些视图引用它、哪些标注以它为基准 | `core-model` 的 Relations | 供 6.4 关联更新使用 |

> **印证内核边界**：occt-wasm 里**没有** grid / level / datum 任何概念，它只有 `make_line` / `make_plane` / `half_space`（剖切用）。**基准元素是纯平台概念，不是内核概念**——这正好说明它该落在 `core-model`，而不是内核适配层。

#### 4.6.3 四类元素的数据模型（草案）

```rust
// core-model：内置基准元素（built-in datum，不走 SDK 注册）
pub enum DatumKind { Grid, Level, RefPlane, RefLine }
pub enum DatumScope { Document, ViewLocal, Session }   // 作用域

pub struct DatumElement {
    pub id: ElementId,
    pub kind: DatumKind,
    pub name: String,            // 轴号 "3" / "A"；标高名 "F2"
    pub scope: DatumScope,       // 文档级 / 视图局部 / 仅会话
    pub geom_ref: GeometryRef,   // 指向内核几何（曲线 / 平面）
    pub locked: bool,            // 钉住（防误拖）
    pub visible_in: ViewFilter,  // 在 3D / 平面 / 立面 哪些可见
    pub extra: BTreeMap<String, Value>,  // 标高值、面层厚度、轴号前缀…
    pub relations: Vec<RelationId>,
}
```

| 元素 | 几何是什么 | 语义（`extra`） | 默认作用域 |
|---|---|---|---|
| **Grid 轴网** | 一条（可折线化的）曲线 + 两端轴号位置 | 轴号、轴号排列规则（数字/字母/混合） | Document |
| **Level 标高** | 一个水平面（供剖切判定）+ 一个基准点 | `elevation` 标高值、（可选）面层厚度 | Document |
| **RefPlane 参考面** | 一个平面 | 名称；"设为当前工作平面"属于**会话状态**（不进文档） | Document / ViewLocal |
| **RefLine 参考线** | 直线 / 曲线 | 名称 | Document / ViewLocal / Session |

- **轴号文字 / 标高符号不存几何**，只存 `name` 与 `elevation`，出图时由 `core-doc` 渲染 → 改轴号只改一个字段，全图所有视图同步。
- **"设为当前工作平面"是会话状态**：跟 3.3 的"持久态 vs 会话态"同一条纪律，`Session` 作用域的东西**不写进 `.bimdoc`**。

#### 4.6.4 内置 ≠ 写死：可配置的部分走约定表

内置的是**元素类型与关系契约**，不是具体取值。以下必须可配置，供不同院标 / 国标覆盖（与 7.7 的 `DraftingStandard` 同思路）：

| 可配置项 | 归属 | 例子 |
|---|---|---|
| 轴号命名规则 | `DatumConvention` | `1,2,3…` 与 `A,B,C…` 如何交替、是否跳过 I/O/Z |
| 标高类型与显示精度 | `DatumConvention` | 建筑标高 vs 结构标高、保留几位小数、`±0.000` 还是 `EL+0.000` |
| 轴网/标高在各类视图的可见性 | `DatumConvention` | 平面图显示哪些轴网、3D 里是否显示 |

> **建筑标高 vs 结构标高（真实坑，必须预留）**：同一楼层有"建筑完成面（FFL）"与"结构面（SSL）"两个标高，差一个面层厚度。平台应支持 **一个 Level 带"面层厚度/偏移"或成对定义**，且构件的"标高归属"要能区分建筑/结构。**不预留这个字段，后期改不动。**

> **⚠️ 落地实测（2026-09-23，M2 收口已实现）** —— 本表第一行"轴号命名规则…是否跳过 I/O/Z"原先**只在类型上
> 占了位、并未真正可配**：`GridNumbering` 只有 `Numeric/Alphabetic/Mixed` 三值，既无"跳过易混字母"的配置项，
> 也没有"序号 → 轴号文本"的能力。现已补齐：`DatumConvention.skip_ambiguous_grid_letters`（`#[serde(default)]`，
> **默认 `true`**，符合国标）+ `GridDirection { Vertical, Horizontal }` + 纯函数
> `grid_label(index, direction)`。字母采用**双射 base-K**（不是 Excel 式溢出）：不跳过时 K=26
> （`1→A … 26→Z, 27→AA`）；跳过 `I/O/Z` 时 K=23（`9→J, 21→W, 23→Y, 24→AA, 46→AY`）。
> `Mixed` 按方向取值：`Horizontal` 出字母、`Vertical` 出数字。**旧 JSON 缺该字段仍可反序列化**（serde default）。
> 面层厚度（`level_with_slab` / `KEY_SLAB_THICKNESS` / `KEY_LEVEL_KIND`）此前已落地。

#### 4.6.5 与 SDK 的边界（哪些能外挂、哪些不能）

| 能力 | 谁能做 |
|---|---|
| 新增属性 schema（给基准元素加自定义参数） | ✅ 模块可注册 |
| 新增标注样式 / 出图规则 | ✅ 模块可注册 |
| 新增元素类型（如专业专用构件） | ✅ 模块可注册（见 8.2）|
| **重定义 `DatumKind`（改轴网/标高的语义）** | ❌ **禁止** —— 这会破坏跨模块定位契约 |
| **专业定位规则**（如"排管相对某轴网偏移 N 米"） | ✅ 留在模块里——那才是专业能力 |

#### 4.6.6 落地节奏

- **M0 不涉及**（M0 只验几何与内核，见 10.1）；
- **M2 数据层落地**：四类内置元素 + Relations + 事务支持；**✅ 已完成（2026-09-23 收口）**——四类内置元素与
  事务此前已落地，本轮补齐了三条**文档明文要求却没做到**的部分：①`Document` 字段私有化以**强制走事务**（见 4.3）；
  ②轴号编号**真可配**（含跳 I/O/Z，见 4.6.4）；③关系图**多跳遍历原语**（`core-model/src/graph.rs`，供 6.4 使用；
  6.4 的 `DependencyGraph` 与增量重绘仍归 M5）。门禁 474 passed / 0 failed；
- **M5 出图 P0 强依赖**：平面视图绑 `Level`、立面/剖面绑 `Grid` 方向，**这一条不先有，视图体系无从谈起**；
- 一阶段建议：**Grid / Level 内置且必须**（`built-in required`），**RefPlane / RefLine 内置可选**。

---

## 五、几何构造层（能力⑤的落地清单）

### 5.1 算子清单（与 occt-wasm 方法的映射）

| 我们的算子 | occt-wasm 方法（Rust） | 说明 |
|---|---|---|
| 长方体/球/柱/锥/环 | `make_box` / `make_sphere` / `make_cylinder` / `make_cone` / `make_torus` | 基元 |
| 拉伸 | `extrude` | 2D → 3D |
| 旋转 | `revolve` | |
| 融合/切除/求交 | `fuse` / `cut` / `common` | 布尔 |
| 布尔管道 | `boolean_pipeline` | 多步布尔一次算 |
| 圆角/倒角 | `fillet` / `chamfer` / `fillet_batch` | |
| 抽壳/偏置 | `shell` / `offset` / `thicken` | |
| 扫掠/放样 | `sweep` / `loft` / `pipe` | |
| 缝合/修复 | `sew` / `fix_shape` / `unify_same_domain` | 导入后常用 |
| 几何查询 | `get_volume` / `get_bounding_box` / 表面积 | |

**配方参数 key 约定（`BuildStep.args`，唯一权威来源）**：`core-app::GeometryService::execute_step`
按下列 key 把 `BuildOp` 分发到内核门面。**长度一律 mm，角度一律 rad**（键名固定为 `angle_rad`，
**不接受** `angle`/度数）。

| `BuildOp` | 参数 key | 输入 |
|---|---|---|
| `MakeBox` | `dx`, `dy`, `dz` | 0 |
| `MakeSphere` | `radius` | 0 |
| `MakeCylinder` | `radius`, `height` | 0 |
| `MakeCone` | `r1`, `r2`, `height` | 0 |
| `MakeTorus` | `major_radius`, `minor_radius` | 0 |
| `Translate` | `x`, `y`, `z` | 1 |
| `Rotate` | `px`, `py`, `pz`, `dx`, `dy`, `dz`, `angle_rad` | 1 |
| `Extrude` | `dx`, `dy`, `dz` | 1 |
| `Revolve` | `px`, `py`, `pz`, `dx`, `dy`, `dz`, `angle_rad` | 1 |
| `Fuse` / `Cut` / `Common` | （无） | 2 |
| `Fillet` | `radius`、`edge_selectors` | 1（+ 选择器） |
| `Chamfer` | `distance`、`edge_selectors` | 1（+ 选择器） |
| `Shell` | `thickness`、`tolerance`(可选)、`face_selectors` | 1（+ 选择器） |
| `Offset` | `distance`、`tolerance`(可选) | 1 |
| `Thicken` | `thickness`、`tolerance`(可选) | 1（面/壳） |
| `Loft` | `is_solid`(可选)、`ruled`(可选) | **≥2**（变长） |
| `Sweep` | `transition_mode`(可选) | 2（`[profile, spine]`） |
| `Pipe` | （无） | 2（`[profile, spine]`） |

> **M5-B6.2 追加说明（八算子）**：上表末 8 行为 **B6.2 规格**新增（`BuildOp` 变体名见方案 §2.7.16.5(a)）。
> `edge_selectors` / `face_selectors` 为**子形状选择器 key**，值为 `Value::Text` 承载的 **JSON 数组**（子形状选择器**不**另立 `Value` 变体、**不**新增 `BuildStep` 字段，理由见方案 §2.7.16.1）。

**子形状选择器 key（M5-B6.2，配合上表八算子）**：

| 选择器 key | 值 | 线格式（`Value::Text`） |
|---|---|---|
| `edge_selectors` | `EdgeSelector` JSON 数组 | `[{"ByIndex":3}]` / `[{"NearPoint":[x,y,z]}]` / `[{"ByTopoName":{…TopoName…}}]` |
| `face_selectors` | `FaceSelector` JSON 数组 | 同上（Face 变体） |

> **硬约束**：本表为 args key **唯一权威来源**；key 命名变更须**同步本表与** `core-app::selector` 编解码 helper（`encode_*`/`parse_*`）。
> 选择器 JSON 解析失败**必须响亮报错**（绝不静默当空选择器——空 `edge_ids` 会让 `fillet`/`chamfer` 静默无效）。

### 5.2 容差体系（数值稳定的命门）

- **长度**类容差（点重合、面贴合、自交判定）统一用 **`1e-6 ~ 1e-8`**（单位 mm）；
- **过松**：把本不相交的两个面判成相交，布尔结果出错；
- **过紧**：退化情形（相切、共面）下布尔直接失败；
- **落地**：容差**集中定义**在一个常量模块，不许各调用点各写各的：

```rust
// core-kernel/src/tolerance.rs
pub const TOL_POINT: f64 = 1e-7;            // 点重合（mm）
pub const TOL_FACE:  f64 = 1e-6;            // 面贴合（mm）
pub const TOL_SELF_INTERSECTION: f64 = 1e-8;// 自交判定（mm）
pub const TOL_ANGLE: f64 = 1e-9;            // 角度（rad）
```

> **⚠ 口径澄清（M3 落地时补齐）**：上面正文那句 **`1e-6 ~ 1e-8` 讲的是"长度类"容差**
> （点/面/自交）；`TOL_ANGLE = 1e-9` 是**角度**容差，单位是 **rad**（≈ 5.7e-8 度），
> **独立于长度容差区间**，因此**不在** `1e-8 ~ 1e-6` 之内并不矛盾——角度与长度是两把不同的尺子。
> 另：`TOL_ANGLE` 必须与**长度量级无关**；角度比较（如 `angles_coincident`）须按 `2π` 周期取模后再比，
> 否则 `0` 与 `2π` 会被误判为不相等。

### 5.3 必须知道的坑：布尔结果是"复合体"

occt-wasm 官方文档明确：布尔运算（`BRepAlgoAPI`）的结果是 `TopoDS_Compound`——**一次布尔可能产出多个互不相连的体**。所以：

```rust
// 错误写法：直接把它当 solid 用（fillet 等会拒绝 compound）
let r = kernel.fuse(a, b)?;
let f = kernel.fillet(r, edges, 2.0)?;   // 可能失败！

// 正确写法：先拆出 solid
let solids = kernel.get_sub_shapes(r, ShapeKind::Solid)?;
let first = solids.first().ok_or(KernelError::EmptyResult)?;
```

**规矩**：任何布尔之后、任何"要求单实体"的算子之前，都要**显式处理复合体**。

### 5.4 演化历史（支撑"关联更新"的关键）

occt-wasm 提供带历史的布尔（`fuse_with_history` / `cut_with_history` 等）和 `EvolutionData`（记录了"哪些面被改了、哪些新生成、哪些被删了"）。**这是"改模型 → 图纸自动更新"能实现的底层依据**：

- 当一根柱子的尺寸变了，通过历史能知道它的哪些面变了；
- 图纸上的标注若绑在这些面上，就知道"这条尺寸需要重算"。

> **设计含义**：出图模块的"关联"必须搭建在**几何历史 + 元素ID + 标注绑定**三者之上，而不是靠"重新投影一遍再比对"（那样无法知道哪条标注对应哪个构件）。

---

## 六、显示与出图层

### 6.1 三维预览管线（能力②的"看"）

```
B-Rep（内核） → tessellate → MeshData（三角网格）
            → wgpu 上传顶点/法线/索引 → 屏幕
```

- 网格只用于**看**，几何真身始终是 B-Rep（立项方案 1.3 第①条）；
- `tessellate(shape, linear_deflection, angular_deflection)` 两个参数控制精度：**线性偏差**（越小越圆滑）、**角度偏差**；
- 大装配用 `mesh_batch` 批量网格化，减少跨边界次数；
- 颜色/名字/装配层级用 **XCAF**（`xcaf_*`）：`xcaf_import_step` 保留颜色名称，`xcaf_export_gltf` 导出带材质。

### 6.2 HLR 出图管线（能力②的"出图"，本方案难点）

> #### 谁负责 HLR？—— 先把这个说清楚（避免误读）
>
> **HLR 算法本身，`occt-wasm` 全包了，你一行都不用写。**
> 前面几节出现的"要自建"，说的都是 **HLR 之外**的事，是四件不同的活，不能混成一件事：
>
> | 环节 | 谁负责 | 要不要自己写 |
> |---|---|---|
> | **HLR 算法**（B-Rep 消隐，算轮廓线 / 判可见性） | **`occt-wasm` 的 `project_edges`**（底层 OCCT `HLRBRep_Algo`） | ❌ **不用写**，直接调 |
> | HLR 结果 → 二维实体（拆边 / 判型 / 提参数） | 我们（`core-kernel` 的"接线"层） | ✅ 写"接线"，**不是写算法** |
> | 三维拾取（鼠标点选点/线/面） | 我们（`core-doc` 的 `Picker`，见 6.6） | ✅ 写交互逻辑 |
> | 标注 + 模型改了自动更新 | 我们（`core-doc` 出图层） | ✅ 出图层的本职 |
> | `cadrum` | **不负责 HLR**（它只有网格级折线"伪消隐"） | — |
>
> **一句话**：HLR = occt-wasm 的活；**"把 HLR 的结果变成可标注的二维图纸" = 我们的活**。前者是买来的，后者是自研的，两件事。
>
> **20 行就能自证**（M0 第一件事）：建一个带圆孔的板 → `project_edges` → 把返回的 `visible_sharp` / `visible_outline` 逐个 `get_sub_shapes(...,"edge")` 看 `curve_type`。**能跑出结果，就说明 HLR 你没写、但能用。**

```
选定构件集合 + 视图定义（视点/视线/屏幕右轴/视图深度）
      │
      ▼
core-kernel::Kernel::project_edges   ← 得到六类 2D 曲线（精确）
      │
      ▼
core-doc：分类（可见/隐藏 × 轮廓/光顺/尖锐）→ 二维实体
      │
      ├─ 可见·尖锐 → 实线（粗）
      ├─ 可见·光顺、可见·轮廓 → 细实线
      └─ 隐藏   → 虚线（可选是否显示）
      │
      ▼
二维视图（View）→ 标注 → 图纸（Sheet）
```

> **别去找"二维投影库"——它不存在，也不需要。** 上面这张图就是"二维投影"的全部：**视图变换矩阵（我们写）+ 已算好的 2D 曲线（`occt-wasm` 的 `project_edges` 给）+ 画线（`kurbo` / `lyon`）**，三件套拼出来，**不引入任何新库**。投影与消隐是同一件事的两半，**消隐算好了，投影就只剩变换与绘制**。真正的工作量不在几何，而在**图纸语义层（图幅 / 比例 / 线型 / 标注样式 / 会签栏）**——那是"规则"不是"格式"，见 7.7。

| 视图类型 | 怎么来 |
|---|---|
| 平面图 | 视点在构件上方俯视，视线向下；按"视图范围/剖切高度"裁剪 |
| 立面图 | 视线水平，指定方向 |
| 剖面图 | **先剖切**（用半空间 `half_space` 切掉前半），再投影 |
| 详图 / 大样 | 对局部区域用更大的比例再投影一次 |

> **剖面图的实现**：OCCT 的 HLR 支持剖切平面（`HLRAlgo_Projector` + 剖面），occt-wasm 的 facade 是否暴露"带剖面的投影"需要核实；**退路**是我们自己在**内核里先做布尔切（`cut` + `half_space`）再投影**——多一步但稳。**这是一项待验证项。**

### 6.3 二维图纸编辑（能力③，最大工作量）

二维实体的数据模型（`core-doc`）：

```rust
pub enum Entity2D {
    Line { a: P2, b: P2 },
    Circle { center: P2, radius: f64 },
    Arc { center: P2, radius: f64, start: f64, end: f64 },   // 弧度
    Ellipse { center: P2, rx: f64, ry: f64, rot: f64 },
    Polyline { pts: Vec<P2> },
    Spline { ctrl: Vec<P2>, degree: u8, knots: Vec<f64> },
}

pub struct DrawItem {
    pub id: ElementId,
    pub geom: Entity2D,
    pub layer: LayerId,
    pub style: LineStyle,        // 线型/线宽/颜色
    pub bind: Option<ModelBinding>,  // 关联到哪个构件的哪个面/边（关联更新用）
}
```

标注体系（对应立项方案附录 B 的 P0）：

```rust
pub enum Annotation {
    Linear { p1: P2, p2: P2, offset: f64 },     // 线性尺寸
    Aligned { ... },                             // 对齐尺寸（沿两点方向）
    Angular { a: P2, b: P2, c: P2 },             // 角度
    Radial { center: P2, target: P2 },           // 半径/直径
    Level { pos: P2, height: f64 },              // 高程点
    Tag { pos: P2, element: ElementId },         // 标记（与构件属性联动）
    Text { pos: P2, content: String },           // 文字
    Leader { from: P2, to: P2 },                 // 引线
}
```

**捕捉（Snap）**：端点、中点、圆心、垂足、切点、交点、最近点。捕捉的目标是**模型几何**（通过 `ModelBinding` 找到三维构件上的面/边，再投影）。
**详图图元**：详图线（只存在于视图、不进模型）、填充区域（图案填充）、遮罩区域（遮挡下方）。

### 6.4 关联更新引擎（**P0 中的 P0**，BIM 出图与"CAD 贴图"的分水岭）

```
模型改动（事务 commit，版本号 +1）
      │
      ▼
找出受影响的面/边（靠几何历史 EvolutionData）
      │
      ▼
沿 Relations 图找到绑定了这些面/边的 DrawItem / Annotation
      │
      ▼
只重算这些项：重新投影其子几何 → 更新二维实体与尺寸数值
      │
      ▼
标记视图/图纸为"脏" → UI 增量重绘
```

数据结构：

```rust
pub struct DependencyGraph {
    // 几何单元 → 依赖它的图元
    geom_to_items: HashMap<GeomKey, Vec<ElementId>>,
    // 图元 → 它所在的视图 → 视图所在的图纸
    item_to_view: HashMap<ElementId, ViewId>,
    view_to_sheet: HashMap<ViewId, SheetId>,
}
```

**验收口径**：改一个构件的尺寸，**不手动改动**的情况下，相关视图的轮廓与尺寸标注自动更新，图纸编号/数量统计同步。**这条不过，就不算 BIM 出图。**

### 6.5 DXF 导出（出图通道）

- 二维实体 → DXF：`Line`→`LINE`，`Circle`→`CIRCLE`（**必须是真圆，不能是折线**），`Arc`→`ARC`，`Spline`→`SPLINE`，标注→`DIMENSION`（或展开成几何 + 文字）；
- 图层、线型、线宽、颜色映射到 DXF 相应字段；
- Rust 侧用现成 DXF 库（如 `dxf-rs`，crates.io 名 `dxf`），**自己只做"我们的实体 → DXF 实体"的翻译层**；
- **验收**：导出的 DXF 在主流 CAD 里打开，圆是圆、标注可编辑、图层正确。

### 6.6 三维交互拾取（3D 界面里点选点/线/面）——**occt-wasm 不提供，必须自建**

> **先说结论（很重要，别踩空）**：occt-wasm **没有**拾取（picking / selection）API。OCCT 原生的三维交互拾取（`SelectMgr`、`Select3D`、`AIS_InteractiveContext`）**被排除在 WASM 构建之外**。occt-wasm 的 README「Scope」一节原文：
>
> > Include OCCT visualization or display modules — **TKV3d, TKHLR (except the HLR facade), and the AIS interactive context are excluded; bring your own renderer**.
>
> cadrum 同样没有交互拾取（只有 `Edge::project` / `Face::project` / `Solid::contains` 这类投影原语）。
>
> **但拾取所需的几何原语，occt-wasm 全部具备**——我们要做的只是"自己写拾取逻辑"，而不是"自己写几何算法"。这是本方案完全可承受的工作量。

#### 6.6.1 可用原语清单（自建拾取的积木）

| 你需要的能力 | occt-wasm API（Rust / TS 同名） |
|---|---|
| 取出所有面 / 边 / 顶点 | `get_sub_shapes(shape, "face"/"edge"/"vertex")` |
| 面 / 边的稳定指纹 | `hash_code(shape, upper_bound)`、`sub_shape_hashes(...)` |
| **三角形 → 属于哪个面** | `mesh_shape(...)` 的 `face_groups`：`[tri_start, tri_count, face_hash]`。⚠ 两处**必须知道**的落地细节（M0 实测）：① `tri_start`/`tri_count` 是**索引**单位、不是三角形单位，换算要 `/3`；② `face_hash` 的**域由 `hash_upper_bound` 决定**——OCCT `HashCode(upper)` 结果落在 `[1, upper]`，网格化器用的是 `i32::MAX`。**自己另取一个 bound 会得到完全不相交的值集**（同一个面：`1_000_000` 下 `326887`、`i32::MAX` 下 `1652326887`），表象是"怎么都对不上"。故已收口成常量 `FACE_HASH_UPPER_BOUND` + 唯一入口 `Kernel::face_hashes`，调用方**不要**再手写 bound |
| **折线 → 是哪条边** | `wireframe(...)` 的 `edge_groups`：`[point_start, point_count, edge_hash]` |
| 边 → 相邻面 | `edge_to_face_map(shape, upper_bound)`、`shared_edges(a,b)`、`adjacent_faces(shape,face)` |
| 点到**面**的最近点 | `project_point_on_face(face, point)` |
| 点到**边**的最近点 + 切向 + 参数 | `project_point_on_edge(edge, point)` → `{point, tangent, parameter}` |
| 顶点坐标 | `vertex_position(vertex)` |
| 点是否在体内 | `contains_point(solid, point, tol)` |
| 曲面参数 / 分类 / 法向 | `uv_from_point`、`classify_point_on_face`、`surface_normal`、`point_on_surface` |
| 剖切（可做"剖切面拾取"） | `section_plane(shape, origin, normal)` |
| 形体间距 | `distance_between(a, b)` |

#### 6.6.2 两条实现路线 + 推荐混合

**路线 A —— 网格级拾取（快，屏幕空间，走主流程）**

渲染时用 `mesh_shape` 建一张 **三角形 → face_hash** 映射表，命中判定二者择一：

- **ID Buffer**：离屏渲染一张"每个面一种颜色 = 该面 hash"的图，鼠标位置读像素 → 直接得到面（GPU 加速，最快）；
- **CPU 射线求交**：鼠标像素 + 相机 → 反投影出射线 → 与三角形求交 → 取最近三角形 → `face_hash`。

边同理走 `wireframe` 折线做屏幕空间最近线；顶点走 `vertex_position` 投影后比距离。

**路线 B —— 内核级投影拾取（准，几何级）**

拿到候选面/边后，用 `project_point_on_face` / `project_point_on_edge` 在内核里算**精确最近点与参数**（边的还能拿到切向），适合"放尺寸定位点、精确贴附到边"。

> **推荐：A 粗筛 → B 精算。** A 回答"点到了谁"，B 回答"点在它的哪个参数位置"。

```
鼠标 (x, y)
   │  A：ID Buffer 读像素 / 射线求交           ← 快，整屏一次
   ▼
命中 (ElementId, face_hash / edge_hash)
   │  B：project_point_on_face / on_edge      ← 只对候选调用，个位数
   ▼
拾取结果 { element, kind: Face|Edge|Vertex, param: (u,v)/t, world_point, normal/tangent }
```

#### 6.6.3 两个必须知道的坑

1. **`hash_code` 不能当持久 ID。** 哈希值只在给定 `upper_bound` 内唯一，跨会话/跨重建不稳定。高亮可以临时用哈希；**做标注绑定、版本管理必须用我们自己的持久 `ShapeId` + 稳定拓扑名**（呼应 3.3 的"持久态 vs 会话态"分离）。
2. **每次内核调用都是一次 WASM 往返。** 别对整栋楼的面逐个调投影——先用网格粗筛把候选压到个位数，再进内核精算。

#### 6.6.4 交付物

`core-doc` 里新增 `Picker` 模块（不属内核、属交互层）：

```rust
pub enum PickKind { Face, Edge, Vertex }
pub struct PickResult {
    pub element: ElementId,
    pub kind: PickKind,
    pub topo_hash: u64,        // 会话内用于高亮
    pub world_point: P3,
    pub param: PickParam,      // Face(u,v) / Edge(t) / Vertex
    pub normal: Option<V3>,    // 面的法向，用于高亮/贴附
}
pub trait Picker {
    /// 屏坐标 + 相机 → 拾取结果（A 粗筛 B 精算在实现里组合）
    fn pick(&self, screen: (f32, f32), camera: &Camera, filter: PickFilter) -> Option<PickResult>;
}
```

> **M0 建议加一项验证**：用 `mesh_shape` 的 `face_groups` 能否正确把三角形映射回 `get_sub_shapes(...,"face")` 的句柄（哈希对齐）。这是拾取链路能否成立的第一个验证点。

#### 6.6.5 两个库谁能胜任？——"拾取"与"标注依据"分开判

| 问题 | occt-wasm | cadrum | 判定 |
|---|---|---|---|
| **有直接拾取/选择 API 吗？** | ❌ 无（AIS/SelectMgr 被排除） | ❌ 无（grep 全库无 `pick`/`select`；只有 `Solid::contains` 走 `BRepClass3d_SolidClassifier`） | **都要自建** |
| **拾取原语够不够？** | ✅ 齐备：`project_point_on_face` / `project_point_on_edge`（带切向+参数）/ `get_sub_shapes` / `hash_code` / `edge_to_face_map` / `shared_edges` / `adjacent_faces` / `uv_from_point` / `classify_point_on_face` / `mesh_shape(face_groups)` / `wireframe(edge_groups)` / `section_plane` | ⚠️ 够用但更薄：`Edge::project` / `Face::project` / `Solid::contains`，网格侧 `Mesh.face_ids`（每三角形一个面 ID）可做三角形→面映射 | occt-wasm 更全 |
| **消隐结果能当"标注依据"吗？** | ✅ **能**：`project_edges` 把**轮廓母线（OutLine）/ 光顺边（Rg1）/ 棱线（V/HCompound）三类分开输出**，且是 B-Rep 精确曲线 | ❌ **不能**：`scene()` 只算"轮廓边"一类（`all_edges = silhouette_edges`），**不分轮廓/光顺/棱线，输出折线，无 B-Rep 溯源** | **只有 occt-wasm 胜任** |
| **能出可编辑的二维曲线（标注的前提）** | ✅ 线/圆/弧/样条（可标线性、半径、直径） | ❌ 只有折线（无法标半径、无法捕捉圆心） | occt-wasm |
| **能支撑"模型改了标注跟着变"** | ✅ 输出是形状，可查拓扑/类型，配合演化历史做关联 | ❌ 折线无拓扑身份 | occt-wasm |

**结论**：

1. **拾取（picking）**：两个库**都要自建**，谁都没有现成接口；但 occt-wasm 的原语更全，**拾取建立在 occt-wasm 之上**。
2. **HLR 作"标注依据"**：**只有 occt-wasm 胜任**。它输出的三类边正好对应工程标注要区分的对象——**轮廓母线**（曲面投影外沿，标外形尺寸）、**棱线**（实体真实棱边，标定位/间距尺寸）、**光顺边**（相切过渡，通常不标或用细线），而且都是**精确可编辑曲线**。
3. cadrum 在这两件事上都**不合格**，它的位置仍是"预览快照 + 原生性能对照"，不进入拾取与出图链路。

> **仍需注意的共同缺口（两库都没有）**：**"这条投影后的二维线，来自模型的哪个面/哪条边"——即溯源（provenance）**。occt-wasm 的 HLR 输出虽是 B-Rep，但 facade **没有暴露**"HLR 结果 ← 原始边"的映射。因此**关联标注不能依赖 HLR 输出反查**，而应：
> - **标注绑定到"模型拓扑"**（`get_sub_shapes` + `hash_code` + 我们自己的持久 `ShapeId`）；
> - **投影自己算**（把模型边投影到视图平面）；
> - **HLR 只用两件事**：① 决定可见性（实线 / 虚线）；② 把投影曲线归类为 轮廓 / 光顺 / 棱线（决定线型与可否标注）。
>
> 这样"关联更新"与"HLR 溯源"解耦，不依赖那个不存在的映射。若将来确需精确溯源，可 fork occt-wasm、按 3.8 的 `MethodSpec` 机制加一个"HLR 输出→原始边"的 facade 方法。

---

## 七、数据互操作层 `core-interop`（能力④）

### 7.1 四格式路由表（缺一条就断链）

| 格式 | 通路角色 | 一阶段目标 | 实现 | 风险 |
|---|---|---|---|---|
| **IFC** | 评审入口 | **解析** | **OCCT 不含**：`ifc-lite-core` / `bimifc-parser`（纯 Rust，MPL-2.0）| **高**（唯一缺口） |
| **STEP** | 交付通道 | 读写（含装配/颜色/名称） | occt-wasm 原生 + XCAF | 低 |
| **DXF** | 出图通道 | 兼容（二维读写） | Rust `dxf-rs` | 中 |
| **glTF** | 查看流转通道 | 解析 | occt-wasm 导出 / Rust `gltf` | 低–中 |

> **这只是"四个基线格式"。** 实际工程还会遇到 **SVG（出口）、DWG（导入，需进程隔离）、GIM（自建，单列模块）** 三种，**处理方式各不相同**，见 **7.8**；而"出图"真正的工作量在**图纸语义层**（图幅/比例/线型/标注样式/会签栏），**不是格式转换**，见 **7.7**。

### 7.2 STEP 读写

```rust
// 读：带装配层级与颜色（用 XCAF 文档）
let doc = kernel.xcaf_import_step(&step_text)?;
for root in kernel.xcaf_get_root_labels(doc)? {
    let info = kernel.xcaf_get_label_info(doc, root)?;   // name / color / is_assembly
    // 递归 walk：getReferredLabel 找到原型，getChildLabels 找子件
}
// 写：保留颜色与名称
let out = kernel.xcaf_export_step(doc)?;
```

### 7.3 glTF 解析

glTF 走"**查看/流转**"定位：拿到 `.glb` → 解析网格/材质/节点树 → 渲染。用 Rust `gltf` crate 解析即可，不进 B-Rep 管线。

### 7.4 DXF 兼容

- **导入**：DXF → 我们的 `Entity2D`（块/图层/图元覆盖度要逐项验证）；
- **导出**：见 6.5；
- **关键约束**：**圆、弧必须是解析实体**，否则标注与捕捉全废。

### 7.5 IFC 解析（最大风险点，给三条路线）

| 路线 | 做法 | 优点 | 缺点 |
|---|---|---|---|
| A. IfcOpenShell 绑定 | 通过 C API / 子进程用 IfcOpenShell | 生态最全、语义最准 | 引入 C++/Python 依赖，跨端（浏览器）难 |
| B. web-ifc / **纯 Rust 解析器** | 用 web-ifc 的 WASM，或用**纯 Rust 的 `ifc-lite-core` / `bimifc-parser`** | 浏览器友好、纯 Rust、跨端 | 语义映射要自己做 |
| C. 自研 IFC-SPF 解析 | Rust 直接解析 IFC 的 SPF 文本，映射到自建模型 | 完全可控、纯 Rust、跨端 | 工作量最大，要覆盖 IFC 子集 |

> **选型更新（2026-09，已核实）**：路线 B 现在有**两个纯 Rust、MPL-2.0** 的现实选项，可直接 `cargo add`，不必自己从零写 SPF 词法：
>
> | 候选 crate | 许可 | 覆盖 | 备注 |
> |---|---|---|---|
> | **`ifc-lite-core`**（ifc-lite） | **MPL-2.0** | IFC4X3（876 实体）+ IFC5 / IFCX | 列式数据结构、零拷贝分词；另有 `ifc-lite-geometry`（三角化）、`@ifc-lite/drawing-2d`（平立剖导航） |
> | **`bimifc-parser`**（bimifc） | **MPL-2.0** | IFC4（STEP）+ IFC5（IFCX / JSON） | 纯 Rust、惰性解码 + SIMD 扫描；含 `bimifc-geometry` 三角化 |
>
> **MPL-2.0 的含义**（对合规很重要）：它是**文件级 copyleft**——**可以静态链接进我们的程序而不"传染"自研代码**，只有当我们**修改了它自己的源文件**时才需要回馈那些改动。这与 LGPL-2.1（`occt-wasm`）、GPLv3+（`LibreDWG`）是三种不同的义务强度。

**建议**：一阶段走 **B（`ifc-lite-core` 或 `bimifc-parser`，二选一实测后定）** 拿到"能读构件、属性、空间层级"的能力，同时把映射层设计成**可替换**（接口对外是"IFC → 我们的 Document"，内部实现可换）。**把 IFC 解析做成独立里程碑，不与几何进度绑定。** 两个候选都**先只做导入**，写回是否做、做到什么覆盖度，**以实测结论为准**（详见 7.7 末的待验证清单）。

### 7.6 能力矩阵与降级策略

| 能力 | 一阶段 | 降级 |
|---|---|---|
| STEP 读 | ✅ 完整 | — |
| STEP 写 | ✅ 完整 | — |
| glTF 读 | ✅ 网格级 | 只读显示 |
| glTF 写 | ✅（occt-wasm 导出） | — |
| DXF 读 | ⚠️ 常见图元 | 未支持图元报警并跳过 |
| DXF 写 | ✅ 二维实体 | — |
| SVG 写 | ✅（`kurbo` / `lyon` 序列化） | — |
| DWG 读 | ⚠️ 经独立转换服务（`LibreDWG`，**进程隔离**） | 服务不可用时该通道整体不可用，**不影响主程序** |
| DWG 写 | ❌ 一阶段不做 | —（GPL 与覆盖度均不支持） |
| IFC 读 | ⚠️ 子集 | 未支持实体报警并跳过，但**不崩** |
| IFC 写 | ❌ 一阶段不做 | 视实测决定是否纳入二阶段 |
| GIM 读 | ❌ 不进本期 Core | 单列电网侧模块 |

> **统一原则**：所有导入都要**"能读多少读多少，读不了报清楚，绝不整包失败"**。评审场景最怕"一个 IFC 打不开就全打不开"。

### 7.7 图纸语义层与制图国标（**自建规则引擎，本能力④最重的一段**）

> **先纠正一个常见误判**：很多人以为"出图 = 把三维投影成二维线"。**投影只是几何**，真正的工作量在**图纸语义层**——图幅、比例、线型线宽、图层、标注样式、会签栏、图纸集。**这些是"规则"，不是"格式"**，**没有任何库能替你写**，必须自建一套可配置的规则引擎。

**分层（谁负责什么）**：

| 层 | 内容 | 谁负责 |
|---|---|---|
| **几何层** | 视图变换矩阵 + HLR 投影结果 → 二维精确曲线（圆仍是圆） | `occt-wasm`（`project_edges`）+ 我们的接线层 |
| **绘制层** | 把二维曲线 / 标注渲染成像素或矢量路径（屏幕、SVG、打印） | `kurbo` / `lyon`（**零自研**，精度优先，`kurbo` 面向工程与科学场景）【54】 |
| **语义层** | 图幅、比例、线型线宽、图层、标注样式、会签栏、图纸集、图纸编号 | **自建规则引擎**（本节重点） |

> **关键澄清：投影与消隐是同一件事的两半。** 不要再去找"二维投影库"——它不存在，也不需要。**投影片 = 视图变换矩阵（我们写）+ HLR 已算好的 2D 曲线（`occt-wasm` 给）+ `kurbo`/`lyon` 画线（库）**，三件套拼出来，**不引入任何新库**。

**规则引擎要能配置的对象（示意）**：

```rust
pub struct DraftingStandard {         // 一套"制图标准"（国标 / 院标 / 项目标）
    pub name: String,
    pub paper: PaperSpec,             // 图幅：A0–A4 + 加长
    pub scales: Vec<f64>,             // 允许的比例：1:50 / 1:100 ...
    pub line_styles: LineStyleTable,  // 线型：实线/虚线/点划线 + 线宽组（粗/中/细）
    pub layers: LayerTable,           // 图层：名称/颜色/线型
    pub dim_styles: DimStyleTable,    // 标注样式：箭头/文字高度/精度/单位
    pub title_block: TitleBlockSpec,  // 图签 / 会签栏字段
}
```

**为什么这块是成本热点**：它会随"制图标准"变化而变（国标、行业标、院标、业主要求），**不能写死**，必须做成**数据驱动的规则表**——这也是它归到能力④（通用性）而非某个专业模块的原因。**验收口径**：换一套标准配置，不改代码就能出符合该标准的图纸。

**待验证清单（M0 / 早期就要实测）**：

| # | 待验证项 | 判据 |
|---|---|---|
| 1 | `ifc-lite-core` vs `bimifc-parser` 的 IFC4X3 覆盖度 | 用真实工程 IFC 跑，统计"能读 / 报警跳过 / 崩" 三类比例 |
| 2 | IFC 写回是否需要 | 若仅评审入口，一阶段可只读 |
| 3 | `occt-wasm` 是否暴露"带剖面的投影" | 见 6.2；否则走"先布尔切再投影"退路 |
| 4 | `kurbo` / `lyon` 的 SVG 输出是否满足出图线型（虚线/点划线）| 导出后在浏览器与 CAD 各验证一次 |

### 7.8 扩展格式：DWG / SVG / GIM（许可与边界）

四格式（7.1）之外，实际工程还会遇到三种"格式"，**三者的处理方式完全不同**，必须分开设计：

| 格式 | 属"格式"还是"规则" | 引入新库？ | 选型 | 许可 / 隔离 | 一阶段范围 |
|---|---|---|---|---|---|
| **SVG** | 格式（矢量出口） | **否** | 用 `kurbo` / `lyon` **序列化**二维实体 | 既有依赖（MIT / Apache-2.0） | ✅ 随出图层落地 |
| **DWG** | 格式（AutoCAD 原生） | **是，且必须隔离** | `LibreDWG`（GNU，C 库）【53】 | **GPLv3+ → 只能进程边界隔离** | ⚠️ **只导入**，且经独立转换服务 |
| **GIM** | **规则（非公开标准）** | **否（自建）** | 国网 GIM **无可用开源解析器**，自建解析器 | 自研资产 | ❌ **不进本期 Core**，单列"电网侧模块"（2–3 人月）|

> **DWG 的部署红线（务必写进合规评审）**：`LibreDWG` 是 **GPLv3+**，**绝不可静态链接进主程序**。正确做法是：**把 LibreDWG 包成一个独立的服务端进程 / 微服务**，主程序通过 **IPC / HTTP** 调它做"DWG → 中间格式"的转换。这样**主程序保持在自有许可之内，GPL 的传染性被进程边界挡住**。一阶段**只做导入（读），不做导出（写）**——GPL 与写入覆盖度（LibreDWG 写出仅 R1.1–R2000）都不支持一阶段做导出。
>
> **关于 ODA（Open Design Alliance）**：它是商业授权、覆盖度最好的 DWG 方案，但**一阶段不引入**——先用 LibreDWG 把"能读"这条通路的架构（转换服务 + 进程隔离）搭对，将来若要提升覆盖度，**只替换转换服务内部实现，不影响主程序**。
>
> **GIM 为什么要单列**：GIM 是**国网自定义、非公开**的数据标准，**不存在可直接 `cargo add` 的解析器**，只能对着标准文档自建。它**不是通用能力，是电网侧专业能力**，因此**不进本期 Core**，按"电网侧专用模块"独立立项排期。

---

## 八、SDK 与模块层 `core-sdk`（能力⑥，复利开关）

### 8.1 设计原则

> **内核不感知任何专业。** Core 不知道"电缆隧道"是什么，只知道"有个模块注册了一个叫 `CABLE_TUNNEL` 的属性模式和几个几何算子"。

### 8.2 插件接口契约

```rust
// core-sdk/src/lib.rs —— 模块开发者的全部接口面
pub trait Module {
    fn meta(&self) -> ModuleMeta;                       // id / 名称 / 版本 / 依赖
    fn register(&self, reg: &mut Registry) -> Result<(), SdkError>;
}

pub struct Registry { /* 模块往这里塞东西 */ }

impl Registry {
    /// 1) 注册几何算子：把参数变成 STEP/BREP
    pub fn add_geometry_op(&mut self, name: &str, op: Box<dyn GeometryOp>);
    /// 2) 注册属性模式：新增一种构件类型
    pub fn add_property_schema(&mut self, schema: PropertySchema);
    /// 3) 注册二维标注规则：新专业的标注长什么样
    pub fn add_annotation_rule(&mut self, rule: Box<dyn AnnotationRule>);
    /// 4) 注册构件创建向导 / 参数面板（可选）
    pub fn add_element_factory(&mut self, f: Box<dyn ElementFactory>);
}

pub trait GeometryOp: Send + Sync {
    fn name(&self) -> &str;
    /// 输入：参数字典；输出：几何（以 BREP 文本返回，由 Core 落库）
    fn build(&self, params: &ParamMap, ctx: &mut KernelCtx) -> Result<GeometryBlob, SdkError>;
}

pub trait AnnotationRule: Send + Sync {
    /// 给定构件，产出它在该视图里应显示的标注
    fn annotate(&self, element: &Element, view: &View, out: &mut Vec<Annotation>);
}
```

### 8.3 模块能做什么、不能做什么

| 能做 | 不能做 |
|---|---|
| 注册新的构件类型与参数 | 直接调 `occt_wasm::` |
| 用 Core 提供的几何算子组装专业构件 | 改 Core 的数据库 schema |
| 定义专业标注规则 | 绕过事务直接改文档 |
| 提供自己的参数界面 | 依赖其它模块的内部实现 |

### 8.4 运行时与加载

- **静态链接**（一阶段）：模块在编译期编进程序，`ModuleHost` 在启动时依次 `register`；
- **动态加载**（二阶段）：用动态库 / 或"模块 = 一段 WASM"（与内核同构，天然沙箱）；
- **注册表冻结**：启动完成后 `Registry` 只读，运行期不再变更。

### 8.5 **一阶段必须做的"接口体检"**

> **用 1–2 个最小示例模块把接口跑通，否则二阶段会推倒重来。**

示例模块 `modules/example-beam`（不属任何专业，只为验证接口）：

```rust
pub struct BeamModule;
impl Module for BeamModule {
    fn meta(&self) -> ModuleMeta { ModuleMeta::new("example-beam", "示例梁", "0.1.0") }
    fn register(&self, reg: &mut Registry) -> Result<(), SdkError> {
        reg.add_property_schema(PropertySchema {
            type_name: "EXAMPLE_BEAM".into(),
            fields: vec![
                FieldDef::length("b", "截面宽").unit("mm").default(300.0),
                FieldDef::length("h", "截面高").unit("mm").default(600.0),
                FieldDef::length("L", "长度").unit("mm").default(6000.0),
            ],
        });
        reg.add_geometry_op("example_beam", Box::new(BeamOp));
        Ok(())
    }
}
struct BeamOp;
impl GeometryOp for BeamOp {
    fn name(&self) -> &str { "example_beam" }
    fn build(&self, p: &ParamMap, ctx: &mut KernelCtx) -> Result<GeometryBlob, SdkError> {
        let (b, h, l) = (p.get_f64("b")?, p.get_f64("h")?, p.get_f64("L")?);
        let profile = ctx.make_rect_profile(b, h)?;      // Core 提供
        let solid = ctx.extrude(profile, l)?;            // Core 提供
        Ok(ctx.to_brep_blob(solid)?)
    }
}
```

**验收**：这个模块能被 Core 加载、能在界面上建出一根梁、能出图、能存盘、能再打开。**跑不通就说明 SDK 接口设计有问题，必须现在改。**

> **✅ 接口体检实测（2026-09-30，M6 收口）**：五环已全部落为自动化断言，用例只放 `apps/desktop`（见 §9.5 依赖终点铁律）。
>
> **mock 腿** `apps/desktop/tests/e2e_mock.rs`（`cargo test -p bim-desktop` 默认腿全绿）：
>
> | 环 | 用例 | 断言数 | 实测结果 |
> |---|---|---|---|
> | 1 加载 | `e2e_load_module_registers_geometry_ops` | 1 | `install_module(ExampleBeam)` 通过（api_version 校验内含），算子已登记 |
> | 2 建模 | `e2e_model_creates_shape` | 2 | `geometry_count()==1` 且返回的 ShapeId 对应几何记录 |
> | 3 出图 | `e2e_drafting_view_yields_entities` | 4 | 图元非空、`up_to_date` 置位、单形状诊断可观测且不写回 |
> | 4 存盘 | `e2e_save_embeds_drawing_container` | 3 | 容器含 `drawing.json` / `views/` / `doc.json` |
> | 5 再打开 | `e2e_reopen_preserves_everything` | 5 | 元素数/品类/几何记录数/视图数/Drawing 深相等五项全过 |
>
> **真实内核腿** `apps/desktop/tests/e2e_occt.rs`（`--features occt --release -- --test-threads=1`）：**8 环已拆为 8 条独立 `#[test]`（M8 时点 2026-10-01 收口，对齐 `e2e_power` 先例"天然拆环、失败可归因"）**——防假腿守卫 → 装载 → 建模 → 出图〔真实视向〕→ 存盘〔容器五段齐全〕→ 重开 `open_real` → rebuild → regenerate，每环一条用例、helper 分层复用装配链（`real_session`/`modeled_session`/`viewed_session`/`reopened_with_view`），每环重跑自己需要的前缀。原单链 17.17s → 8 用例 104.56s（约 6 倍：7 次额外的 occt-wasm 加载），换失败精确归因 + 单环重跑能力；共享态方案不取（`CoreSession` 非 `Sync`，环间顺序依赖与测试执行顺序耦合，脆弱）。
>
> 合计：默认腿 5 passed（e2e_mock）；occt 腿 **6 passed**（0 bin + e2e_mock 5 + e2e_occt 1）。**本节验收"能被 Core 加载、能在界面上建出一根梁、能出图、能存盘、能再打开"在两腿均成立。**

---

## 九、关键流程（端到端）

### 9.1 建模流程

```
新建文档 → 事务开始 → 选构件类型（模块注册的）
        → 填参数（属性面板）→ 调模块的 GeometryOp → 内核产出 BREP
        → 落到 GeometryRecord → 建 Element → 关联计算
        → 事务提交（版本+1）→ 通知视图刷新
```

### 9.2 出图流程

```
选视图类型（平面/立面/剖面/详图）→ 设视点/视图范围（剖面还要设剖切面）
        → Kernel::project_edges → 六类曲线 → 转二维实体（可见/隐藏 × 线型）
        → 进入二维视图 → 标注（捕捉模型几何）→ 放图框、视口、定比例
        → 图纸集（Sheet）→ 导出 DXF / PDF / 打印
```

### 9.3 导入导出流程

```
导入：文件 → core-interop 判类型 → 解析 → 映射到 Document（走事务）→ 落库
导出：Document → core-interop → （STEP 走 XCAF 保颜色；DXF 走二维；glTF 走网格）
```

### 9.4 二次开发流程

```
模块作者 → 依赖 core-sdk → 实现 Module trait → 注册算子/属性/标注
        → 交付模块（静态/动态/WASM）→ Core 加载 → 出现在构件库里
```

### 9.5 端到端闭环（2026-09-23 跑通"建模 → 存盘 → 再打开"；2026-09-30 M6 收口补齐出图环与关联更新）

上表 9.1 的建模链路的**最小可用版本**已落地，入口是顶层门面 `CoreSession`（`core-app/src/session.rs`）：

```
CoreSession::new()            # 装配 DocumentService + GeometryService + InteropService + ModuleHost（默认占位内核）
  → install_module(ExampleBeam)    # 模块只依赖 core-sdk
  → create_element(category, name) -> ElementId
  → build_geometry(element, op_id) # 算子 → 配方(BuildStep 序列) → execute → snapshot → 落盘 → 回填 geometry_ref
  → create_view(name, orientation) -> ViewId   # M6 出图环：建视图 → P0 投影写回 → rebuild_index → sync_drawing_registry
  → save("x.bimdoc")               # 容器 v2：manifest + doc.json(to_persistable) + drawing.json + views/<viewid>.json + geom/*.brep
  → open("x.bimdoc")               # 逐条 verify_geometry；Drawing 随容器恢复（"重开丢图纸"已修复）
  → 断言元素 / 品类 / 几何 / Drawing 四项一致
```

**CoreSession 出图 / 关联更新门面（M6 交付，全部落 core-app）**：`create_view`（建视图 + P0 投影初生成）、`regenerate_view`（全量重投，`(写入数, 警告)` 原样上抛）、`project_shape`（单形状诊断，不写回）、`rebuild_geometry`（修改既有元素几何并触发关联更新）；真腿装配 `with_real_kernel` / `open_real`（`#[cfg(feature = "occt")]` 门控，`new()` 保持占位内核默认；真腿以 `backend_name()=="occt-wasm"` + `backend_kind().is_real()` 守卫防假腿）。

`apps/desktop/src/main.rs` 跑的就是这条链路（退出码 0，**出图环已通**）；自动化断言载体为
`apps/desktop/tests/e2e_mock.rs`（mock 腿 5 环）与 `tests/e2e_occt.rs`（真实内核腿 8 环，含真实视向投影与 rebuild → regenerate 闭环）。
**`rebuild_geometry` 是 `update_after_commit` 的首个生产调用方**：新配方**复用原 `ShapeId`** 收进文档（remove + insert 同 id，构件身份不变），键解析**演化优先、指纹退避**，随后定向重投 + 标脏（`up_to_date=false`），返回 `UpdateReport` 供调用方决定 regenerate（门面不自动重投）。
**注意端到端用例必须放在 `apps/*`**：`core-app` 不被允许依赖 `example-beam`（否则反向依赖违铁律②），`apps/*` 才是依赖终点。

**尚未接线**（属后续里程碑）：视图刷新通知（GUI 侧消费；关联更新已在数据层以 `mark_views_stale` 标脏落地，UI 推送属 GUI 交付物）。
原「尚未接线」注记（2026-09-23）据实更新：~~事务（4.3）~~ 已随 M2 收口（字段私有化 + 受控修改口）；~~`views/` 落盘~~ 已随容器 v2 落地（`drawing.json` + `views/<viewid>.json`）；~~HLR → 二维实体~~ 已随 M5 落地（六类曲线 → 二维实体）。
`BuildOp` 已全部接线真执行（24 变体：`Rotate`/`Extrude`/`Revolve` 随 M3 接线，八算子 Fillet/Chamfer/Shell/Offset/Thicken/Loft/Sweep/Pipe 随 M5-B6.2b 接线真实派发）；后端不具备的能力仍显式返回 `KernelError::Unsupported`（**不假装支持**）。

> **修订记录（2026-09-30，M6 收口）**：§8.5 补五环实测记录；§9.5 端到端链路（容器 v2 / 出图门面 / rebuild 闭环）与注记按 M6 交付据实更新；§10 里程碑表 M6 行补「M6 现状」行（含 **M6b-3 降级去向=M7 候选**：example-beam 第二算子〔布尔 Cut〕＋演化优先分支在 rebuild 场景的替代验证路径）。本次为文档收口更新，未修改任何源码（§1.2 未引用 M6 状态、附录 B 为外部源码索引、附录 C 未列 CoreSession API，均无需改动）。

---

## 十、工程落地指导

### 10.1 里程碑拆解（把立项的"三阶段"细化到可验收）

| 里程碑 | 内容 | 验收标准 | 依赖 |
|---|---|---|---|
| **M0 技术验证（先做，2–3 周量级）** | 内核可行性与五大前置验证 | ① `project_edges` 能返回**精确圆**（不是折线）；② 剖面投影可行（或布尔切+投影可行）；③ WASM 在桌面正确加载；④ IFC 组件能读出一个子集；⑤ **拾取链路可用**：`mesh_shape` 的 `face_groups` 能把三角形哈希映射回 `get_sub_shapes(...,"face")` 的句柄（见 6.6） | 无 |
| **M1 内核适配层** | `core-kernel` 完成 | 能建基元、布尔、网格化、HLR；错误/句柄/容差就绪 | M0 |
| **M2 数据层** | `core-model` 完成 | 文档 CRUD、事务、存盘、再打开一致 | M1 |
| ↑ **M2 现状** | ✅ **已完成（2026-09-23 收口）** | CRUD / 事务 / 存盘往返此前已绿；本轮补齐 §4.3 事务强制（字段私有化）、§4.6.4 轴号编号可配、§6.4 关系图遍历原语。门禁 474 passed / 0 failed / 2 ignored | M1 |
| **M3 几何构造** | 能力⑤算子齐备 | 立项清单算子可用，容差与复合体处理正确 | M1 |
| ↑ **M3 现状** | ✅ **已完成** | 新增 `MakeCone`/`MakeTorus`，接线 `Rotate`/`Extrude`/`Revolve`；容差对齐 §5.2（含角度容差独立澄清）；布尔复合体按 §5.3 提供 `split_solids`/`single_solid`（多体报 `NotSingleSolid`、空报 `EmptyResult`）；补齐 `bounding_box`/`surface_area`。门禁 509 passed / 0 failed / 2 ignored，occt 腿（`core-kernel`）89 passed / 0 failed | M1 |
| **M4 互操作** | 四格式基线 | IFC 可解析、STEP 可读写、DXF 可兼容、glTF 可解析 | M1 |
| **M5 出图 P0 子集** | 能力②③ | "模型→视图→标注→图纸"闭环，**关联更新生效**，DXF 导出圆是真圆 | M2 M3 |
| **M6 SDK + 示例模块** | 能力⑥ | `example-beam` 能加载、建模、出图、存盘 | M2 M5 |
| ↑ **M6 现状** | ✅ **已完成（2026-09-30 收口）** | 实测：默认腿 674 passed / 0 failed / 2 ignored；core-kernel occt 127 lib + 1 doctest；core-app occt 211 lib + 48 集成 + 1 doctest = 260 全 target；bim-desktop occt **6 passed**（第 8 道门禁：e2e_mock 5 + e2e_occt 1，真内核全链 17.17s）。交付：容器 v2（`FORMAT_VERSION` 1→2，`drawing.json` + `views/<viewid>.json`，图纸随容器恢复，修复「重开丢图纸」）；CoreSession 出图门面 `create_view`/`regenerate_view`/`project_shape` + 组合访问器 `with_doc_and_drawing_mut`；真腿装配 `with_real_kernel`/`open_real`（cfg 门控 + 防假腿守卫）；`rebuild_geometry` 为 `update_after_commit` 首个生产调用方（复用原 `ShapeId`、演化优先/指纹退避、定向重投+标脏）。**M6b-3 据实降级为 M7 候选**：example-beam 第二算子（布尔 Cut）＋演化优先分支在 rebuild 场景的替代验证路径——规格内布尔演化只覆盖「毛坯→结果」，在 rebuild「旧几何→新几何」场景数学上无法真实命中、必然指纹退避；演化优先分支真实命中已由既有强钉覆盖（同 id remove+insert 文档模式） | M2 M5 |
| **M7 → 二阶段** | 专业模块 | 电力侧与建筑侧各至少一个可用模块 | M6 |
| ↑ **M7 现状** | ✅ **已完成（2026-09-30）** | M7a 变长算子（`Loft` 截面 N≥2 / `MakeWire` 边 N≥1，`arity_ok` 单一事实源，`core-sdk/src/geometry_op.rs` 有专门 M7a 用例）；M7b example-beam 第二算子（`example.beam.hole` 圆孔板：MakeBox→MakeCylinder→Cut→Translate，M6b-3 候选落地）；M7c 电力侧 `cable-tunnel`（`power.tunnel.rect` 矩形隧道段 + `power.tunnel.ductbank` 电缆排管段：MakeLineEdge×4→MakeWire→Pipe，16 单测）；M7d 建筑侧 `hvac-duct`（`hvac.duct.rect` 矩形风管段 MakeBox→Translate + `hvac.duct.transition` 变径管 11 步配方 Loft(is_solid+ruled) + `hvac.duct.width` 标注，17 单测）——**零 SDK 改动挂接**（立项 P0"接口贴合单一专业"风险持续证伪）。变径管 occt 腿棱台体积 oracle：V=h/3×(S1+S2+√(S1·S2))=373,333,333.33 mm³ ±0.5%、bbox 800×800×1000 ±1e-6，实测命中；mock 腿不硬凑（占位后端不支持 Loft，变径管真实 execute 断言只放 occt 腿）。门禁：fmt 0；clippy 两腿 0；check-deps 合规（`hvac-duct` 仅依赖 `[core-sdk]`）；默认腿 **720 passed / 0 failed / 2 ignored**（M6 基线 674 → +46）；三模块 49 单测（hvac-duct 17 + cable-tunnel 16 + example-beam 16）；bim-desktop occt 腿 **14 passed**（e2e_mock 5 + e2e_occt 1 + e2e_power 5 + e2e_hvac 1 + e2e_hvac_occt 2，其中 e2e_hvac_occt 19.74s）。⚠ 遗留（**已收口，M8 时点 2026-10-01**）：§8.5 注记的 e2e_occt 8 环链**拆环已做**——8 环拆为 8 条独立用例（helper 分层复用装配链），occt 腿 **21 passed / 0 failed** | M6 |
| ↑ **M8 现状（新增起点，里程碑表外）** | ✅ **已完成（2026-10-01）** | 桌面端 GUI 第一版（egui 0.29 + eframe/wgpu，`src/apps/desktop/src/gui.rs` ~900 行，`gui` feature 门控）：三维视口（轨道相机 Z 朝上 mm 单位 + CPU 画家算法单 Mesh + 双面光照 + 地面网格坐标轴）；相机控制**两轴方向实测校准**（拖右＝物体右转、拖下＝相机降低）+ **无限俯仰**（过极点 `(pitch,yaw)→(±π−pitch,yaw+π)` 参数化翻转，画面无跳变，`pitch_drag_sign` 保持拖动方向自洽）；拾取全链（视口相机实现 `core_doc::Camera::ray_through` → `CoreSession::pick_at` 门面 → KernelPicker 粗筛+精算+反查；mock 腿命中后精算必然 Err——"绝不假命中"，日志引导用真内核）；CJK 字体运行时兜底；`CoreSession` 新增 `tessellate`/`pick_at` 两个门面。门禁：fmt 0；clippy default/gui/gui,occt 三腿 `-D warnings` 全 RC=0；默认腿 720 passed；GUI 实机：mock 腿 32m48s / **occt 真内核腿首次 1h51m57s** 均退出码 0 | M7 |
| ↑ **e2e_occt 拆环收口（M7 遗留，M8 时点）** | ✅ **已完成（2026-10-01）** | e2e_occt.rs 从"单链收在一个 `#[test]`"（8 环 20 断言）重写为 **8 条独立用例**（每环一条；对齐 `e2e_power` 先例"天然拆环、失败可归因"）；装配链收进 4 个分层 helper（`real_session`/`modeled_session`/`viewed_session`/`reopened_with_view`），每环重跑自己需要的前缀，**断言语义逐字保留**；共享态方案不取（`CoreSession` 非 `Sync`，环间顺序依赖与测试执行顺序耦合）。门禁：occt 腿全量 **21 passed / 0 failed**（e2e_mock 5 + e2e_occt 8〔104.56s〕+ e2e_power 5 + e2e_hvac 1 + e2e_hvac_occt 2）；代价 7 次额外 occt-wasm 加载（17.17s → 104.56s，约 6 倍），换失败精确归因 + 单环重跑 | M7 |
| **M9（二阶段）GUI 文件链路** | 存盘 / 打开 / 另存 / 新建接通 GUI | `rfd` 原生文件对话框；保存走 `CoreSession::save`、打开按 feature 路由（occt → `open_real`，否则 `open`）；标题栏显示当前文件路径。**验收**：GUI 里保存 → 关闭 → 重开，元素 / 几何 / 图纸 / 拾取全链往返一致；双腿（mock / occt）均可跑 | M8 |
| **M10（二阶段）GUI 关联更新** | 属性编辑 + rebuild / regenerate 在 GUI 落地 | 右栏属性面板可编辑（构件参数），确认后走 `rebuild_geometry`（M6 关联更新首链在 GUI 生效）→ 网格重建 + 拾取即时反映新几何 + stale 视图徽章；"重新出图"动作走 `regenerate_view`。**验收**：GUI 改梁宽 300→500，网格 / 拾取 / stale 提示全部正确 | M9 |
| **M11（二阶段）出图视图面板** | GUI 内 2D 图纸 + DXF 导出 | 新增"图纸"侧面板：视图列表（§6 真实视向：平面绑 `Level`、立面 / 剖面绑 `Grid`）+ `Entity2D` 投影渲染（线段 / **解析圆** / 弧 / 文字）+ DXF 导出按钮（`core-interop` 既有能力）。**验收**：GUI 建俯视 / 立面视图，2D 图元正确显示且圆是真圆；导出 DXF 外部可打开 | M9 M10 |
| **M12（二阶段）拾取增强 + 性能** | 边 / 顶点拾取 + 大装配性能 | `KernelPicker` 扩展边 / 顶点（§6.6 全链，P1 风险"三维拾取需自建"的收尾）；网格**增量**重建（现在 `dirty` 全量重建）+ 拾取 tessellate 结果缓存；性能基线入门禁。**验收**：边 / 顶点拾取命中且高亮与面区分；大装配（千级构件）交互有明确性能边界记录 | M8 M11 |

> **二阶段不规划项**：STEP（上游 `occt-wasm` 4.x 构建形态缺陷，host 侧不可修，见 M4）；DWG（LibreDWG GPLv3+ 绝不可链接主程序，只可独立转换服务，见 §2 依赖注记与 7.8）。

> **M0 是省钱的关键**：用两三周把"技术前提"验掉，而不是等搭了半年才发现"本来以为能精确出图，结果只能出折线"。

> **⚠️ 落地实测订正（2026-09-23，重要）**
>
> M0 的"真实内核可用"这一前置条件**实际未满足**：本机 `occt-wasm` **4.0.0** 的
> `OcctKernel::new()`（`src/kernel.rs:148-161`）用一个**空 `Linker`** 直接
> `linker.instantiate(...)`，而内嵌 WASM 是 Emscripten 产物、声明了 `env::*` host import，
> 于是 wasmtime 48 在实例化阶段报
> `unknown import: env::emscripten_get_preloaded_image_data has not been defined` —— **内核完全装不起来**。
>
> 更严重的是：早期 M0 的 occt 用例都写成 `let Some(b) = try_backend() else { return }`，
> 内核不可用就**静默跳过**，测试框架仍记为 passed，于是"29 passed"是一份**假绿**报告。
> **判据 ①②③⑤ 因此全部属于"未验证"**。详见 `src/README.md` 的「M0 订正」与「M1」两节。
>
> 由此得到一条**工程铁律**（已写入 `rust-workspace-green-gate` skill）：
> **涉及外部依赖可用性的验证，必须有一条"注定会失败"的门禁用例来钉住它**；
> "跳过"与"通过"必须在报告里可区分，否则假绿会把整个里程碑结论带偏。
>
> M1（`core-kernel`）的**工程硬化部分已完成**（槽位 arena、`BackendKind` 可信度、BREP 导入导出、
> `scope` 临时物清理、内核池硬化并修掉一个真实 bug、`core-app` 装载器与落盘闭环）；
> 「用真实后端替换 `MockBackend`」原先受阻于上述上游缺陷。
>
> **✅ 上述 P0 已修复并复验（2026-09-23 晚）**：方案为 **vendor + `[patch.crates-io]`**——
> 把 `occt-wasm` 4.0.0 源码放进 `src/vendor/occt-wasm/`，仅在其 `kernel.rs` 的
> `OcctKernel::new()` 里补一行 `Linker::define_unknown_imports_as_traps(&module)`，
> 并在根 `Cargo.toml` 用 `[patch.crates-io]` 指向该 path 副本（WASM 本体 SHA256 未改）。
> 选 `traps` 而非 `default values`：前者对未实现导入会**显式 trap**，后者会**静默返回 0** 把几何算错。
> 复验（真实内核独立实测）：`make_box(10,20,30)` 体积 **6000.0**；BREP 往返守恒；布尔 cut 差值
> = π·15²·10/4（正确）；`translate` bbox 精确位移；HLR 圆孔板俯视判出**半径 15.0000 的解析圆**。
>
> **✅ 适配层三条残留红亦已修正（2026-09-23 深夜）**：①②⑤ 的根因**都不在内核**，而是适配层的
> "口径"——(1) HLR 空分支用 **id = 0** 占位，`collect_branch` 必须先挡掉再查子形状；
> (2) 内核 `face_groups` 的 `tri_start/tri_count` 是**索引**单位，须 `/3` 换算成三角形单位；
> (3) 判据⑤并非"两个域"，而是**两把尺子**——OCCT `HashCode(upper)` 落在 `[1, upper]`，
> 用例用了 `1_000_000` 而网格化器用 `i32::MAX`。域现由 `FACE_HASH_UPPER_BOUND` 固定，并经
> 新增能力 `KernelBackend::face_hashes` 暴露（见 6.6 与 `src/README.md`）。
>
> **结论：M0 可验判据 ①②③⑤ 全部通过**（④ 属 M4）；`cargo test -p core-kernel --features occt --release`
> = **62 passed / 0 failed / 0 ignored**。
>
> **✅ M2 数据层已收口（2026-09-23）**：M2 的"CRUD / 事务 / 存盘往返"此前已随骨架落地，本轮做的是**逐条对照
> 设计文档收口**——补齐三条"文档明文要求、代码却没做到"的部分：
> (1) **§4.3 事务强制**：`Document` 8 个字段原全 `pub`（且存在绕过事务的真实路径），现全部私有化 +
> 只读访问器 + 受控修改口 `remove_geometry()`，并用源码级守卫钉住"字段保持私有"；
> (2) **§4.6.4 轴号编号可配**：补 `skip_ambiguous_grid_letters` 与 `grid_label()`（双射 base-K，跳 I/O/Z 时 K=23）；
> (3) **§6.4 关系图遍历原语**：新增 `core-model/src/graph.rs`（无向 BFS + 防环，零新依赖）。
> **门禁 474 passed / 0 failed / 2 ignored**（收口前 457，+17 全新用例，无既有断言弱化）。
>
> **一条并列于"假绿"的教训**：**守卫本身也要被攻击**。第一版"字段私有化守卫"用的是逐字段字面匹配
> `pub {field}:`，被 QA 实测打穿（改成 `pub(crate) meta:` 时它仍然绿）——"看起来在守、其实漏报"的守卫
> 比没有守卫更坏，因为它把"漏"伪装成"过"。现改为"剥注释 → 去空白归一化 → 断言体内不含 `pub`"，
> 并对 `pub` / `pub(crate)` / `pub(super)` / 跨行 / 多空格一律报红（三次攻击实测）。

### 10.2 团队分工建议

| 角色 | 负责 | 关键交付 |
|---|---|---|
| 内核工程师 | `core-kernel`、`core-interop` 的几何部分 | 稳定内核抽象、性能基线 |
| 平台/数据工程师 | `core-model`、`core-doc` 的数据与关联 | 文档模型、事务、关联更新 |
| 出图工程师 | `core-doc` 的二维与标注、DXF | Revit 级 P0 子集 |
| 互操作工程师 | `core-interop` 的 IFC/DXF | IFC 解析组件 |
| 平台架构 | `core-sdk`、CI、构建 | SDK 契约、门禁 |

### 10.3 测试策略

| 层级 | 测什么 | 手段 |
|---|---|---|
| 单元 | 数据模型、事务、几何算子 | `cargo test`，秒级 |
| 集成 | 导入→建模→出图→导出全链路 | 用 `testdata/` 样本 |
| **黄金图对比** | 同一模型同一视图的 HLR 输出**稳定** | 与基准向量/位图比对，阈值内通过 |
| 往返一致性 | STEP 读写、BREP 往返、DXF 往返 | "写出去再读回来，几何等价" |
| 性能基线 | WASM 调用、HLR、大装配网格化 | `benchmarks/baseline.json` + 门禁 |
| 合规 | 发布产物含 LGPL 声明、WASM 可替换 | CI 硬检查 |

### 10.4 风险与规避（工程视角）

| 优先级 | 风险 | 工程规避 |
|---|---|---|
| ~~**P0**~~ | ~~**内核实例化失败**（`occt-wasm` 4.0.0 空 `Linker` 未定义 Emscripten `env::*` 导入）~~ | ✅ **已修复（2026-09-23）**：采用 **vendor + `[patch.crates-io]`**，在 `vendor/occt-wasm/src/kernel.rs` 补 `Linker::define_unknown_imports_as_traps(&module)`；WASM 本体未改。门禁用例 `occt_feature_must_actually_instantiate_the_kernel` 保留以持续钉死。**升级 `occt-wasm` 时须重新评估是否可移除本补丁** |
| ~~**P0**~~ | ~~HLR 输出的圆/弧判不出解析类型~~ | ✅ **M0 已先验通过**：圆孔板俯视投影判出**半径 15.0000 的解析圆**（判据①），剖面亦含解析圆边（判据②）。NURBS 保真 + DXF 转 SPLINE 的降级路线**保留备而不用** |
| **P0** | 剖面投影不是 facade 暴露的能力 | 退路：布尔切 + 投影（多一步） |
| **P0** | 关联更新做不到"自动" | 关联建在"几何历史 + 绑定"上，不做全量重算比对 |
| **P0** | IFC 组件跨端不可用 | 接口抽象化，桌面用 IfcOpenShell、浏览器用 web-ifc |
| **P0** | LGPL 在 WASM 分发下的义务 | 合规门禁 + 法务确认；保证终端能替换 WASM |
| P1 | WASM 约 2× 开销 | 桌面端测原生 cadrum 路径；大装配并行分页 |
| P1 | arena 句柄不自动回收 | 引用计数 + 定期清理 + 文档分页 |
| P1 | **三维拾取需自建**（occt-wasm 无拾取 API，OCCT 的 SelectMgr/AIS 被排除） | 属交互层自研、非几何算法，原语齐备；走 6.6 的"A 网格粗筛 + B 内核精算"。**哈希映射已先验通过**（判据⑤）：粗筛改用新增能力 `Kernel::face_hashes`，与 `MeshData::face_groups[].face_hash` **同域同序**——⚠ 域由 `hash_upper_bound` 决定，**不能自己另取一个 bound**（取错会得到完全不相交的值集） |
| P1 | `hash_code` 不稳定，误当持久 ID | 高亮用哈希、**绑定额外用持久 `ShapeId` + 稳定拓扑名**（见 6.6.3） |
| P1 | SDK 接口贴合单一专业 | `example-beam` 体检 + 二阶段前评审接口 |
| P2 | fork upstream 的维护成本 | 优先 upstream PR；只加必要方法 |

### 10.5 第一周起步清单（拿到这份文档就能动手）

1. `git init` 仓库，建 Cargo workspace（照 2.3 抄）；
2. `cargo add occt-wasm`，写一个 20 行 demo：建长方体 → 布尔 → 导出 STEP，**确认 WASM 能在本机加载**；
3. 写 M0 验证一：`project_edges` 一个球（或圆孔板），检查输出边能否判出 `Circle`；
4. 写 M0 验证二：对同一形状做"布尔切 + 投影"，看是否等价于剖面投影；
5. 建 `crates/core-kernel`，把上面 demo 包成 `Kernel::new/make_box/fuse/export_step`；
6. 建 CI：`fmt + clippy + test` 三条先跑起来；
7. 拉一份 `testdata/`（一个 STEP、一个 DXF、一个 glTF、一个 IFC 小样本）。

---

## 附录 A：名词速查（大白话）

| 词 | 一句话 |
|---|---|
| B-Rep | 精确描述立体的"面/边/点"数学表示 |
| HLR | 把立体拍成二维线框图，挡住的不画 |
| 精确 HLR / 工程图级 | 输出的是真圆真弧，能标注；区别于"折线级" |
| 句柄 | 从仓库取东西的"取货单" |
| arena | 内核里的仓库，东西放这，程序一关就清空 |
| 容差 | 判断"算不算同一个点"的尺子 |
| 事务 | 一批改动全成或全不成 |
| 关联更新 | 改模型，图纸自己跟着变 |
| XCAF | OCCT 里管装配树/颜色/名字的模块 |
| WASM | 能跨平台的"通用可执行格式" |
| SDK | 给外人二次开发的"改装接口" |
| 复合体 Compound | 一个"袋子"，可能装一个或多个立体，布尔结果是它 |
| 演化历史 | 记录"这次操作改了哪些面"，关联更新靠它 |

## 附录 B：源码事实索引（逐条可核对）

| # | 事实 | 源码位置 |
|---|---|---|
| 1 | Rust crate 提供 `OcctKernel`，170+ 方法，嵌入 brotli WASM | `occt-wasm/crate/README.md` |
| 2 | `OcctKernel::new()` 解压+JIT，约 100–500 ms，建议每线程复用一个 | `crate/README.md`、`crate/src/kernel.rs` |
| 3 | `ShapeHandle(u32)` 是 arena 句柄，`Drop` 内核即清空 | `crate/src/types.rs`、`crate/src/kernel.rs` |
| 4 | `project_edges(...) -> ProjectionData`（ox/oy/oz, dx/dy/dz, xx/xy/xz, has_x_axis） | `crate/src/kernel_generated.rs` |
| 5 | `ProjectionData` = 可见/隐藏 × 轮廓/光顺/尖锐 六个 ShapeHandle | `crate/src/types.rs` |
| 6 | HLR 底层用 `HLRBRep_Algo` + `HLRAlgo_Projector` + `HLRBRep_HLRToShape`（六类 compound） | `facade/generated/kernel.cpp`（约 4133–4162 行） |
| 7 | TS 侧同方法 `projectEdges(shape, viewOrigin, viewDirection, xAxis?)` | `ts/src/index.ts`（约 1793–1832 行） |
| 8 | 布尔结果是 compound，下钻用 `getSubShapes(fused, "solid")` | `occt-wasm/README.md`（Quick Start 注） |
| 9 | 代码生成中央接缝：`xtask/src/codegen/config.rs` 的 `MethodSpec`；生成文件提交且 CI 查漂移 | `occt-wasm/CLAUDE.md` |
| 10 | 许可证：crate MIT/Apache；内嵌 WASM LGPL-2.1-only，需保证可替换 | `crate/README.md`、`README.md` |
| 11 | cadrum 消隐基于三角网格：`detect_silhouette_edges` + `classify_edges` 中点采样 | `cadrum/src/common/mesh.rs` |
| 12 | cadrum 2D 场景输出折线 `Scene2D { edges_visible: Vec<DVec2>, edges_hidden: Vec<DVec2> }`（NaN 分隔），无解析实体 | `cadrum/src/common/mesh.rs` |
| 13 | cadrum 拓扑边 `Mesh::edges` 注释为 "for glTF export only" | `cadrum/src/common/mesh.rs` |
| 14 | cadrum 公开类型 `Edge` / `Face` / `Solid`；`read_step/write_step/read_brep/write_brep`；`Solid::mesh` → `Mesh` → `scene` | `cadrum/src/lib.rs` |
| 15 | cadrum 静态链接 OCCT，MIT 本体 + LGPL 2.1 内嵌；分发须合规 | `cadrum/README.md` |
| 16 | cadrum 支持 `wasm32-unknown-unknown`，需先调 `__anchor_wasi_stub` + `__wasm_call_ctors` | `cadrum/README.md` |
| 17 | **occt-wasm 不提供拾取/选择 API**：OCCT 的 `TKV3d` / `TKHLR`（除 HLR facade）/ AIS 交互上下文被排除，需自带渲染器与自建拾取 | `occt-wasm/README.md`（Scope 一节） |
| 18 | 拾取原语齐备：`get_sub_shapes` / `hash_code` / `sub_shape_hashes` / `mesh_shape`（`face_groups`）/ `wireframe`（`edge_groups`）/ `edge_to_face_map` / `shared_edges` / `adjacent_faces` / `project_point_on_face` / `project_point_on_edge` / `vertex_position` / `contains_point` / `uv_from_point` / `classify_point_on_face` / `section_plane` | `ts/src/index.ts`（约 1119–1221、1537–1721 行）、`crate/src/types.rs` |
| 19 | **cadrum 也无拾取/选择 API**：仅 `Solid::contains`（OCCT `BRepClass3d_SolidClassifier`）、`Edge::project`、`Face::project`；`Mesh.face_ids` 提供"每三角形一个面 ID"可做三角→面映射 | `cadrum/src/ffi.cpp`（约 613 行）、`cadrum/src/lib.rs`、`cadrum/src/common/mesh.rs` |
| 20 | **cadrum `scene()` 只取"轮廓边"一类**：`let all_edges = silhouette_edges`（`detect_silhouette_edges` 按三角形邻接的两面朝向是否翻转判定），**不区分轮廓/光顺/棱线，输出 `Vec<DVec2>` 折线，无 B-Rep 溯源** | `cadrum/src/common/mesh.rs`（约 298–317 行及 `detect_silhouette_edges`） |

## 附录 C：关键接口速查（团队自查用）

```text
core-kernel:  Kernel{ new, make_*, fuse/cut/common, fillet/chamfer/shell/offset,
                     sweep/loft, tessellate, mesh_batch,
                     project_edges -> Projection,
                     import_step/export_step, xcaf_* }
              ShapeId(u64)  KernelError  MeshData  Projection  ViewDef

core-model:   Document{ elements, views, sheets, relations }
              Element{ id, category, name, geometry_ref, properties, relations }
              GeometryRecord(BREP 文本)  PropertySchema  PropertyMap
              Transaction{ begin, add_element, commit, rollback }

core-doc:     Entity2D{ Line|Circle|Arc|Ellipse|Polyline|Spline }
              DrawItem{ id, geom, layer, style, bind }
              Annotation{ Linear|Aligned|Angular|Radial|Level|Tag|Text|Leader }
              View  Sheet  DependencyGraph{ update_dependents }
              DraftingStandard{ paper, scales, line_styles, layers, dim_styles, title_block }

core-interop: import_step/export_step  import_gltf  import_dxf/export_dxf  export_svg
              import_ifc（子集，ifc-lite-core | bimifc-parser）
              import_dwg（经独立转换服务，LibreDWG 进程隔离）
              import_gim（电网侧模块，自建解析器，单列）
              FormatRouter

core-sdk:     Module{ meta, register }  Registry{ add_geometry_op,
              add_property_schema, add_annotation_rule, add_element_factory }
              GeometryOp  AnnotationRule  ElementFactory  ParamMap  KernelCtx
```

---

*本设计文档依据立项方案 v4/v5 的定位与结论，结合 `occt-wasm`、`cadrum` 两份源码的事实勘察编写。所有源码事实标注于附录 B，可直接核对；IFC / DWG / 二维绘制三类第三方库的选型与许可（`ifc-lite`/`bimifc` MPL-2.0、`LibreDWG` GPLv3+、`kurbo`/`lyon` MIT）已在 7.5–7.8 逐项标注，**凡未验证项均明确标注为"待验证/实测项"，不以假设充事实**。*

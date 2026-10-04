# 轻量化BIM内核平台 GUI 开发计划（v1）

> 立项（2026-10-02）：M8-M12 的 GUI 是"功能验证 UI"——egui 默认样式、6 条模块配方按钮、
> 无参数输入、无类型编辑。本计划把它升级为**产品级建模工作台**。
> **范围基调**（用户定版）：参考市场上常见 BIM 软件的 UI，但不深入专业——
> P0 只聚焦两件事：**① 几何体构建**（原生算子 + 参数表单）、**② 几何体的类型/属性参数自定义能力**。

---

## 1. 设计基准（参考软件取什么 / 不取什么）

| 软件 | 取 | 不取 | 理由 |
|---|---|---|---|
| Revit | 属性面板常驻右侧、项目浏览器树在左 | Ribbon | 属性常驻是参数驱动建模的核心；Ribbon 需要重 UI 框架，egui 无原生支持 |
| Fusion 360 | 工具栏分组、参数驱动建模 | 时间线（操作历史） | 分组工具栏轻量，贴合 BuildOp 七组划分；时间线后补 |
| **SolidWorks** | **草图 → 特征流程**（画二维 → 确认 → 检查 → 生成） | 特征树（后补） | **用户定版：几何建立方式以此为基准**——绝不让用户直接填 dx/dy/dz |
| AutoCAD | 参数即时反馈 | 命令行本身 | 反馈通道已有 logs_panel，不需要命令行交互范式 |
| Blender | 大纲视图 + 属性页签 | 全套快捷键体系 | 元素树 + 属性页签贴合现有面板结构；快捷键可后补 |

**结论**：经典五区布局（菜单栏 / 工具栏 / 左树 / 中视口 / 右属性 / 底状态），不引入新 UI 框架。

## 2. 范围界定

- **P0（本计划）**：视觉基建、布局重组、几何构建工具栏 + 参数表单、草图特征建模（G4）、类型管理器、实例属性面板升级、元素树。
- **不做（本期）**：Ribbon、多文档页签、撤销/重做 UI、专业模块深化、出图深化、可停靠面板——**全部移交后续计划（§9，G6+）**。

### 2.1 SDK API 全覆盖原则（用户定版，全局硬约束）

> **软件所有功能均开放 SDK API 可供外部调用**——**GUI 是薄壳**：GUI 能做的每一件事，
> 必须在 `core-sdk` / `CoreSession` 门面层有对应 API；GUI 只做「交互 → API 调用 → 渲染」，
> 不藏任何只存在于 UI 层的逻辑。

**API 现状（已 API 化，GUI 全部走门面，`core-app/src/session.rs`）**：

| 能力 | API |
|---|---|
| 会话 / 文件 | `new` / `open` / `with_real_kernel` / `open_real` / `save` / `from_parts` |
| 模块安装 | `install_module` |
| 构建几何 | `create_element` + `build_geometry`（配方）+ `rebuild_geometry`（关联更新）+ `carve_geometry` |
| 出图 | `create_view` / `regenerate_view` / `project_shape` |
| 拾取 / 渲染 | `pick_at` / `tessellate`（+ `tessellate_for_pick` 会话缓存） |
| 互操作 | `interop().export_view(Format, &View, &DraftingStandard)` |
| 类型 / 属性 | `Registry::register_schema` / `PropertySchema::validate` |
| 组合访问器 | `document()` / `geometry()` / `interop()` / `modules()` / `summary()` |

**执行规则（G3-G5 起的硬约束）**：
1. **API 先行**：每个新功能**先落 API**（core-sdk / core-app，含单测），GUI 再接线——禁止"先写 UI 后补 API"。
2. **GUI 动作可被纯 API 复现**：每个 GUI 操作必须能用一段不依赖 GUI 的 API 调用序列复现（例：草图矩形拉伸 500mm = wire 组装 API + `Extrude` API；改梁宽 = `rebuild_geometry(element, op_id, params)`）。**G3-G5 每阶段验收新增"API 复现用例"**（集成测试腿）。
3. **G4 草图的 API 化**：草图转特征链（图元 → wire 组装 / 封闭性检查 / 特征执行）全部落 core-app 层 API，GUI 画布只负责收集图元、调 API。

**外部调用形态分期**：
- **P0（本期）**：进程内 Rust API（`core-sdk` + `CoreSession`，库形态）全覆盖——上表即契约。
- **G6-M6（CLI，✅ 2026-10-04）**：bim-server clap 子命令形态落地（demo/new/info/export 四子命令 + cli_probe 5 探针跑真实二进制）。
- **G6-M7（HTTP 桥，已定版）**：axum 暴露 CoreSession 门面为 REST（外部调用形态第二期）。
- **后续**：脚本引擎（rhai/wasm）按需立项；进程内 API 是它们的地基，先立地基。

## 3. 视觉规格（深色专业 CAD 风）

### 3.1 配色板（G1 落为 `gui/theme.rs` 常量）

| 语义 | 色值 | 用途 |
|---|---|---|
| `BG` | `#1e1f22` | 窗口底 |
| `PANEL` | `#2a2b30` | 面板底 |
| `EXTREME` | `#35363c` | 面板标题 / 输入框内 |
| `STROKE` | `#3f4046` | 分隔线 / 边框 |
| `TEXT` | `#d6d9de` | 正文 |
| `WEAK` | `#969ba5` | 弱文本（沿用现状） |
| `ACCENT` | `#4f8cff` | 高亮 / 主按钮 |
| `SELECTED` | `#35507a` | 选中态 |
| `OK` / `WARN` / `ERR` | `#57ad68` / `#e0a34e` / `#e06c6c` | 日志三级 |
| `VIEWPORT_BG` | `#17181a` | 三维视口底 |

构件分色沿用 `base_color()`（品类灰蓝 / 银灰 / 混凝土灰）；拾取高亮沿用面橙 `#ff9d2e`、边/顶点黄 `#ffd52e`。

### 3.2 字体与图标

- CJK 运行时兜底**已有**（`set_fonts`，1697 行），保持不动。
- 数字/坐标用 `FontId::monospace`（对齐输入框）。
- **图标：`egui_phosphor`**（Phosphor icons，MIT）。用法：按钮文本嵌 `format!("{}", PH_ICON_CUBE)`。
  版本选与 egui 0.29 对齐的（实施时核对 crates.io 兼容表），`egui_phosphor::add_to_fonts` 在 `set_fonts` **之后**叠加，验证图标不缺字。

### 3.3 egui 定制点

`ctx.set_visuals` 深色覆写：`panel_bg = PANEL`、`window_fill = PANEL`、`extreme_bg_color = EXTREME`、`selection.bg_fill = SELECTED`、圆角 4px、`spacing.item_spacing = (8, 8)`；工具栏按钮 `min_size (32, 32)` 图标化。

## 4. 布局规格

```
┌──────────────────────────────────────────────────────────────┐
│ 菜单栏:  文件  编辑  视图  帮助                                │
├──────────────────────────────────────────────────────────────┤
│ 工具栏: [基本体▾][线框▾][扫掠放样▾][布尔▾][变换▾][修饰▾][修复▾] │
│         …右侧：拾取模式(面/边/顶点) · 视图预设 · 性能基线        │
├────────────┬──────────────────────────────┬──────────────────┤
│ 左侧页签    │                              │ 右侧页签          │
│ ├─ 元素树   │         三维视口              │ ├─ 属性（页签1）   │
│ └─ 类型管理 │   （VIEWPORT_BG 深底）        │ └─ 图纸（页签2）   │
├────────────┴──────────────────────────────┴──────────────────┤
│ 状态栏: 文件名 · 命中坐标 · 拾取模式 · 网格数                   │
│ 日志（可折叠，默认高 120px）                                    │
└──────────────────────────────────────────────────────────────┘
```

## 5. 功能模块规格（P0 核心）

### 5.1 几何构建工具栏（G3）

BuildOp 30 算子分七组（图标按钮 + 中文 tooltip）：

| 组 | 算子 |
|---|---|
| 基本体（5） | MakeBox / MakeCylinder / MakeSphere / MakeCone / MakeTorus |
| 线框（5） | MakeLineEdge / MakeArcEdge / MakeWire / MakeFace / ExtractWire |
| 扫掠放样（5） | Extrude / Revolve / Loft / Sweep / Pipe |
| 布尔（3） | Fuse / Cut / Common |
| 变换（2） | Translate / Rotate |
| 修饰（5） | Fillet / Chamfer / Shell / Offset / Thicken |
| 修复（4） | Sew / SewAndSolidify / FixShape / UnifySameDomain |

交互：**拉伸 / 旋转扫掠 / 放样三入口走草图流程（§5.6）**，不走参数表单。其余算子：点按钮 → 右侧弹出**参数表单**（5.2）→ 确定 → `build_op` 传参执行。需要输入形状的算子（布尔/变换/修饰）：先在元素树或视口选中输入构件，按钮才可用（否则禁用 + tooltip 提示）。**模块配方按钮（现有 6 条）保留**，移入工具栏"模块配方"分组——与原生算子并列。

### 5.2 算子参数表单（G3 关键决策：单一事实源）

**现在的问题**：算子参数键名散在 `core-app/geometry_service.rs` 的 `execute` match 臂里（`need("dx")` 等），GUI 无法得知每个算子要什么参数。

**决策**：`core-sdk/geometry_op.rs` 的 `BuildOp` 加参数定义表，`execute` 前置校验与 GUI 动态表单**共用**：

```rust
pub enum ParamKind { Length, Angle, Int, Bool, Selectors }
pub struct ParamDef {
    pub key: &'static str,   // args 键名（与 execute 逐臂对照）
    pub label: &'static str, // 中文标签
    pub kind: ParamKind,
    pub default: f64,        // 缺省（与 execute 缺省对齐；Bool 用 0/1）
}
impl BuildOp {
    /// 参数定义表（execute 校验 + GUI 表单共用，单一事实源）。
    pub fn param_defs(&self) -> Vec<ParamDef>;
    /// 该算子需要几个输入形状（MakeBox=0、Fuse=2、Loft=N≥2……）。
    pub fn input_arity(&self) -> usize; // 变长返回最小值
}
```

**键名对照表**（已从 `execute` 逐臂核对，G3 实现时以此为准）：

| 算子 | 参数键 | 缺省 |
|---|---|---|
| MakeBox | dx, dy, dz | — |
| MakeCylinder | radius, height | — |
| MakeSphere | radius | — |
| MakeCone | r1, r2, height | — |
| MakeTorus | major_radius, minor_radius | — |
| Extrude | dx, dy, dz | — |
| Revolve / Rotate | px, py, pz, dx, dy, dz, angle_rad | — |
| Translate | x, y, z | — |
| Fillet | radius + edge_selectors | — |
| Chamfer | distance + edge_selectors | — |
| Shell | thickness + face_selectors | — |
| Offset | distance | — |
| Thicken | thickness | — |
| Loft | is_solid, ruled | true / false |
| Sweep | transition_mode | 0 |
| Sew / SewAndSolidify | tolerance（可选） | `DEFAULT_SEW_TOLERANCE_MM` |
| FixShape / UnifySameDomain / 布尔三件套 / Pipe | 无参数 | — |

GUI 表单按 `ParamKind` 路由控件：Length → `DragValue` + "mm" 后缀；Angle → `DragValue` + "°"（UI 度、传参换算弧度）；Int → 步进；Bool → checkbox；Selectors → 从拾取缓存（M12 边/顶点拾取）取序号。

### 5.3 类型管理器（G4）

- 左侧页签：类型列表（beam / duct / tunnel / 用户自定义）+ 「新建类型」按钮。
- 类型详情：字段表格（名 / 类型 / 单位 / 必填 / 默认值 / 说明）+ 增删改字段行。
- `FieldType` 六种路由编辑控件：Bool / Int / Float / Text / Enum（选项列表编辑）/ Ref。
- 保存 → **`Registry::register_schema`（API 已有，模块侧在用）**；type_name 唯一性校验。
- 约束（诚实提示）：修改已注册 schema 只影响**新实例**——既有实例的 `PropertyMap` 不回写，面板上要说明。

### 5.4 实例属性面板升级（G4）

M10 基础（BTreeMap 平铺编辑）升级为**按 FieldDef 完整路由**：

| FieldDef 能力 | UI 呈现 |
|---|---|
| `unit` | 输入框后缀（"mm" / "kW"） |
| `required` | 缺失标红；确认时 `schema.validate` 失败红字拦截 |
| `Enum(options)` | 下拉选择（现在只是文本框） |
| `default` | 「恢复默认」按钮一键回填 |
| `doc` | 字段名 tooltip |

确认链路：`validate → set → rebuild`（复用 `apply_rebuild`）。

### 5.5 元素树（G5）

品类分组（折叠）→ 节点（图标 + 显示名 + 可见性眼睛 + 删除）；选中联动属性面板与视口高亮（复用 `selected` 链路）。

### 5.6 草图特征建模（G4，用户定版备注）

> **建模易用性原则**（用户定版）：多参考成熟三维建模软件（SolidWorks / Fusion 360）操作流程——
> **选基准 → 画草图 → 确认检查 → 生成特征**，绝不让用户直接填 dx/dy/dz。

三种建立方式（默认参数为用户定版）：

| 方式 | 操作流程 | 默认参数 | 确认时校验 |
|---|---|---|---|
| **拉伸** | ① 选基准面 → ② 草图窗体绘制二维图形 → ③ 点确认 → ④ 检查封闭 | 拉伸高度 **500mm**，方向默认草图平面法向 | 未封闭 → **警告** + 留在草图；封闭 → 生成拉伸体 |
| **旋转扫掠** | ① 指定轴（空间直线，或直接 XYZ 轴）→ ② 草图窗体绘制二维图形（**轴与图形必须共面**）→ ③ 点确认 → ④ 检查封闭 + 共面 | 默认 **360°**，可指定起终点角度（如 **20° ~ 180°**） | 未封闭 → **警告**；轴与图形不共面 → **警告** |
| **放样扫掠** | ① 指定路径（空间曲线，一般样条）→ ② 草图窗体绘制二维图形 → ③ 点确认 → ④ 检查封闭 → ⑤ **路径起点切向 = 草图平面法向** | — | 未封闭 → **警告**；封闭 → 沿路径放样 |

**草图编辑器**（G4 新组件，本计划最大的新增件）：

- **入口**：工具栏「拉伸 / 旋转扫掠 / 放样」三按钮 → 选基准面（XY / XZ / YZ 或轴/路径）→ 打开草图窗体（`egui::Window` 浮窗，二维画布 + 网格 + 捕捉；复用 M11 图纸预览的 mm→px 缩放渲染骨架）。
- **画布**：mm 坐标、网格吸附（可开关）、段内撤销；**图元工具 11 种**（§5.6.1，用户定版清单）。

#### 5.6.1 草图图元工具（G4，用户定版）

| 工具 | 交互（成熟软件惯例） | 转换为内核边 |
|---|---|---|
| 直线 | 两点点击 | `MakeLineEdge` |
| 矩形 | 两角点点击 | 4×`MakeLineEdge` |
| 正多边形·内接 | 点中心 → 点角点（边数参数默认 6） | N×`MakeLineEdge` |
| 正多边形·外切 | 点中心 → 点边中点（边数参数） | N×`MakeLineEdge` |
| 圆 | 点圆心 → 点半径 | **采样折线近似**（内核无整圆边算子，64 段，封闭性天然满足） |
| 弧·圆心起终点 | 圆心 → 起点 → 终点 | **转三点弧**（按圆心/起终点算弧上中点）→ `MakeArcEdge` |
| 弧·三点 | 起点 → 终点 → 第三点 | `MakeArcEdge`（vendor 原生三点式） |
| 倒圆角 | 依次点两条线段 → 半径参数 | 修剪两线段 + 插入弧段（**纯几何**，GUI 内完成） |
| 倒直线角 | 依次点两条线段 → 距离参数 | 修剪两线段 + 插入斜线段（纯几何） |
| 贝塞尔多段线 | 连续点击控制点 + 回车/双击结束 | `make_b_spline_edge`（Bézier 是 B-spline 特例；vendor 已有、**Kernel trait 待接线**，与放样路径同款缺口） |
| 椭圆 | 点中心 → 点长轴端 → 点短轴端 | **采样折线近似**（内核无椭圆边算子，72 段） |

**图元工具注记**：
1. ~~圆 / 椭圆的采样折线近似是 P0 方案（vendor occt-wasm 无 `fn_make_circle_edge` / `fn_make_ellipse_edge`，仅 `fn_make_ellipsoid` 椭球体）~~ **更新（2026-10-02，G4-M13 前置调研实测）**：vendor occt-wasm **现已有** `make_circle_edge`（圆心+法向+半径）/ `make_ellipse_edge`（圆心+法向+长短轴）/ `make_circle_arc` / `make_ellipse_arc`（角度式）/ `make_tangent_arc` / `make_helix_wire`——圆 / 椭圆工具 **P0 直通解析边**，采样折线近似废弃（后续内核补算子时替换为解析边的 P1 项提前兑现）；plan 中"64 段 / 72 段"转换列作历史方案存证。
2. 倒圆角 / 倒直线角是**草图内修剪工具**（先选两条线段、再输入半径/距离），纯几何计算两段修剪 + 插入新段——不依赖内核，双腿一致。
3. 所有图元最终统一为**边序列**（直线边 / 三点弧边 / B 样条边），组装 `MakeWire` → 封闭性检查（§5.6 确认流程）。
4. 每种工具进入时状态栏显示当前交互步骤提示（如"正多边形·内接：第 2 步——点角点"，ESC 取消本次绘制）。

#### 5.6.2 CAD 常用绘制 / 编辑操作（G4 补充，用户追加）

**绘制类补充（6）**——进入 wire 组装链（构造线/射线/点除外）：

| 工具 | 交互 | 转换 |
|---|---|---|
| 多段线（直线） | 连续点击 + 双击/回车结束 | (N−1)×`MakeLineEdge`（PLINE 直线段版，与贝塞尔互补） |
| 样条曲线（拟合点） | 连续拟合点 + 结束 | `make_b_spline_edge`（插值式，与贝塞尔同链路，均待 Kernel trait 接线） |
| 椭圆弧 | 中心 → 长轴端 → 短轴端 → 起终角 | 采样折线（按角度范围采样部分弧） |
| 点 | 单击 | 辅助顶点标记（**不进 wire**） |
| 构造线 | 两点（无限延长） | 辅助定位线（**不进 wire、不进封闭检查**，虚线显示） |
| 射线 | 起点 → 方向点 | 辅助定位线（同上） |

**编辑类补充（8）**——全部**纯几何**（GUI 内顶点变换，与内核无关，双腿一致）：

| 工具 | 交互 | 实现 |
|---|---|---|
| 移动 | 选图元 → 基点 → 目标点 | 顶点平移 |
| 复制 | 选图元 → 基点 → 目标点 | 克隆 + 平移（可连续多份） |
| 旋转 | 选图元 → 基点 → 角度 | 顶点绕基点旋转 |
| 缩放 | 选图元 → 基点 → 比例 | 顶点绕基点缩放 |
| 镜像 | 选图元 → 镜像线两点 | 顶点关于直线反射 |
| 偏移 | 选线段/圆 → 偏移距离 | 平行线 / 同心圆 |
| 修剪 | 选边界 → 点被剪段 | 线段/圆弧求交裁剪 |
| 打断 | 选线段上一点 | 断成两段 |

**辅助类（4）**：

| 工具 | 交互 | 说明 |
|---|---|---|
| 对象捕捉 | 端点 / 中点 / 圆心 / 交点自动吸附 | 在网格捕捉之上叠加，标记显示吸附点类型 |
| 正交模式 | ORTHO 开关（Shift 临时切换） | 水平 / 垂直锁定 |
| 尺寸标注 | 选图元 → 放置位置 | P0 **显示型**（线性/对齐/半径/角度，标注数字）；**尺寸驱动顶点 P1**（约束系统单独立项） |
| 删除 / 撤销 / 重做 | 选中删除 / Ctrl+Z / Ctrl+Y | 段内编辑标配（撤销已有，补删除/重做） |

**范围注记**：绘制类 + 编辑类 + 对象捕捉/正交/显示型标注均 **P0**（编辑类纯几何实现直接）；**尺寸驱动（改标注数值 → 驱动顶点重算）是 P1**——约束系统（SolidWorks 核心）体量大，单独立项不进本计划。

#### 5.6.3 二维图形操作的库选型（零新库，G4 前提）

| 层 | 选型 | 理由 |
|---|---|---|
| **渲染 + 交互** | **egui Painter（现有依赖，零新增）** | M11 已验证同款骨架（mm→px 缩放 / 解析圆 `circle_stroke` / 弧采样 / 文字）；实测 epaint 0.29 自带 `bezier.rs`（`Shape::CubicBezier` / `QuadraticBezier`，贝塞尔多段线**原生渲染**）+ `shape_transform.rs`（`Shape` 平移/旋转/缩放，编辑操作的预览直接复用） |
| **几何类型 / 变换** | **glam（现有依赖）** | `DVec2` / `DMat2` 齐全，零新依赖 |
| **几何算法**（求交 / 修剪 / 偏移 / 镜像 / 封闭检查 / 采样） | **手写 `sketch_geom.rs`**（GUI 模块内新文件） | 全是标准算法：线段求交、点到线距离、圆参数化、平行偏移、de Casteljau 采样——每个 ≤ 20 行，可控可测；**P0 不引入 geo** |
| **内核侧转换** | BuildOp 既有算子 + `make_b_spline_edge`（待接线） | §5.6.1 转换表 |

**不引入的库**：
- `geo` crate（Rust 最成熟几何库，GIS 向）——P0 草图操作（线段/圆的求交、平行偏移）手写可控；若后续做多边形布尔 / 复杂 offset 再引入（P1 备选）。
- cairo / skia——与 egui/wgpu 渲染栈冲突且重。
- 专门 2D CAD 库——Rust 生态无成熟选项（事实注记）。

## 9. 后续计划（G6+，不进本期）

六项暂不做但保留的能力，全部**移交后续计划**（时机按建模功能稳定后的优先级排）：

| 项 | 现状 | 建议时机与备注 |
|---|---|---|
| **撤销 / 重做 UI** | ~~事务引擎已有、UI 未接~~ **已完成（2026-10-03，G6-M1 提交 8489022）**：菜单栏「编辑」组 + Ctrl+Z/Ctrl+Y 已接线 | ✅ 已交付——内核侧零改动（DocumentService 双侧快照 undo/redo 直接转发） |
| **可停靠面板** | ~~egui_dock 待引入~~ **已完成（2026-10-03，G6-M5 提交 118f58a）**：gui.rs 五区布局升级 egui_dock 0.14——面板区（元素/类型/属性/视图/图纸/日志/视口）由 DockState 管理，可拖动重排 / 拖出窗口 / 关闭；菜单栏「视图 · 重置布局」恢复默认布局 | ✅ 已交付——视口不可关闭；日志开关改聚焦语义 |
| **出图深化** | ~~M11 已有基础（视图面板 / 2D 预览 / DXF 导出）~~ **图纸布置已完成（2026-10-03，G6-M3 提交 bc51bde）**：图纸页（Sheet 列表/新建/布置表单/整图预览/DXF+SVG 整图导出）+ edit_drawing 撤销步门面；**视图标题标签已完成（2026-10-03，G6-M4 提交 67707aa）**：sheet_to_2d 每个 placement 下方标注「视图名 1:N」 | ✅ 图纸布置 + 视图标题标签已交付；图框/图幅/标题栏渲染复用既有 sheet_to_2d 链路 |
| **专业模块深化** | M7 三模块已有（beam / duct / tunnel，49 单测） | G6+——GUI 稳定后回到模块侧：模块配方参数化 UI、更多专业模块 |
| **Ribbon** | 不做（egui 无原生支持，需自绘模拟） | G6+——仅当五区布局被验证不够用时评估；自绘 Ribbon 工作量大，优先级最低 |
| **多文档页签** | CoreSession 单文档会话 | G6+——需要多文档同时打开时；涉及会话管理改造（多 CoreSession 或会话池），牵涉面最大放最后 |
| **外部调用形态** | ~~进程内 Rust API（§2.1 契约）全覆盖~~ **CLI 第一期已完成（2026-10-04，G6-M6 提交 a289a99）**：bim-server clap 四子命令 demo/new/info/export 驱动 CoreSession 做建模+出图+导出批处理（cli_probe 5 探针跑真实二进制） | ✅ CLI 已交付——HTTP 桥第二期（G6-M7，axum 暴露 CoreSession 门面为 REST）；脚本引擎（rhai/wasm）按需后续 |
- **确认流程**：收集图元 → 组装 wire（`MakeLineEdge` / `MakeArcEdge` 逐段 + `MakeWire`）→ 封闭性检查（首尾点距离 ≤ 容差，P0 纯几何双腿一致）→ 转特征表单（拉伸高度 / 旋转角度区间 / 路径选择）→ `build_op` 执行 → 失败/取消**回滚回草图编辑，绝不丢草图**。

**内核缺口（诚实注记，G4 前置）**：

1. ~~**样条路径**：`make_b_spline_edge` vendor occt-wasm **已有**（`kernel_generated.rs:1842`），Kernel trait **未接线**~~ **更新（2026-10-02，M13 已完成，提交 746bbdc）**：Kernel trait 已接 `make_bezier_edge(&mut self, points: &[[f64; 3]])`（vendor `make_bezier_edge` 扁平点直通；mock 腿记录型 `EdgeSpline` 占位——首末控制点 + 控制点数入档，中间点不入档）；`BuildOp` 加 `MakeSplineEdge` 变体（30 变体）+ `ParamKind::Points`（`points` 键 JSON 坐标数组必填，`require_spline_points` 解析响亮拒绝）。钉子：core-app 钉 1（mock 记录型 + wire 聚合 extents）/钉 2（坏 JSON 响亮拒绝+回滚）/钉 3（execute_native 直通）+ occt 钉 1（真内核 4 控制点建边 + 包围盒 + 双边聚合 wire，`--release` 实测 9.6s 通过）。贝塞尔多段线工具按 4 点/段分段生成多条 `MakeSplineEdge`；插值式样条（拟合点 → Catmull-Rom 转分段贝塞尔）放 sketch_geom（M14）。
2. **封闭性查询**：`curve_is_closed` vendor **已有**（`kernel_generated.rs:2714`），Kernel trait 未接线——P0 先用草图编辑器内首尾点距离检查（纯几何），内核查询接线为增强项。
3. **revolve 起终点角度**：内核 `revolve` 是单 `angle_rad`（从 0° 起）；用户要"20°~180°"起终点式——**实现方案：轮廓预旋转**（wire 先 `Rotate` 到起始角，再 `revolve`（终止角 − 起始角）），**零内核改动**。

## 6. 实施阶段

| 阶段 | 内容 | 验收 | 依赖 |
|---|---|---|---|
| **G1 视觉基建** | `theme.rs` 配色板 + egui_phosphor 图标 + set_visuals 深色定制 | 双腿编译过；GUI 实机视觉统一；**零逻辑变化** | M12 ✅ |
| ↑ **G1 现状** | ✅ **已完成（2026-10-02）** | `apps/desktop/src/theme.rs`（129 行）：配色板 12 常量（BG/PANEL/EXTREME/STROKE/TEXT/WEAK/ACCENT/SELECTED/OK/WARN/ERR/VIEWPORT_BG）+ `apply(ctx)`（set_fonts + set_visuals + style_mut 间距 8px/按钮 padding）+ `theme_visuals()`（panel_fill/window_fill/extreme_bg/selection/控件三态 + 圆角 4px）；`egui-phosphor = "0.7"`（**0.7.x 对应 egui 0.29，CHANGELOG 实证**），`add_to_fonts` 排在 CJK 兜底之前（图标先于中文命中）；CJK 字体装载从 gui.rs 迁入 theme.rs（逻辑不变）；egui 0.29 实测：`Visuals` 无整体 `rounding` 字段，圆角在 `WidgetVisuals.rounding` 每态设置、面板底是 `panel_fill`。未接线的配色常量（BG/WEAK/OK/WARN/ERR/VIEWPORT_BG）带 `#[allow(dead_code)]` 注明 G2 接线。门禁：fmt 0；clippy 双腿 `-D warnings` 全绿；默认腿测试 180+11 全 passed。提交 9e884de | — |
| **G2 布局重组** | 菜单栏 + 工具栏骨架 + 五区布局 + 状态栏 + 日志折叠 | 现有全部功能（文件/拾取/属性/图纸/预览）在新布局可用 | G1 |
| ↑ **G2 现状** | ✅ **已完成（2026-10-02）** | gui.rs +253/−56：`menu_bar`（文件〔新建/打开/保存/另存/退出 `ViewportCommand::Close`〕/ 编辑〔清空选择〕/ 视图〔适配 + 日志开关〕/ 帮助〔关于→日志〕）+ `toolbar`（适配/清选 phosphor 图标 + 拾取模式三态 + 性能基线 + 日志折叠开关 + **构建分组 G3 预留注释**）+ `left_panel`/`right_panel`（**自制页签** `page_tabs`——egui 无原生 Tabs：左 元素/类型〔G5 占位〕、右 属性/图纸）+ `status_bar`（文件名〔点击=另存〕· 命中坐标 CROSSHAIR · 拾取模式 · 内核 · 统计，自 top_bar 迁移）+ 日志 `show_logs` 可折叠（bottom 面板先 show 贴底：状态栏最底、日志其上）；BimApp 加 `left_page`/`right_page`/`show_logs` 三字段 + `Page` enum；三个面板 heading 去重（页签行充当标题）；头注释布局图更新为五区。门禁：fmt 0；clippy 双腿 `-D warnings` 全绿；默认腿 **231 passed** / 0 failed。提交 9a2f0f7 | — |
| **G3 构建链路** | BuildOp `param_defs()`（core-sdk）+ 七组工具栏 + 参数表单 + build_op 传参 | GUI 点 MakeBox 改参数 → 视口出对应长方体；Fuse 先选两个构件；**API 复现用例**（纯 API 建任意算子构件） | G2 |
| ↑ **G3 现状** | ✅ **已完成（2026-10-02）** | core-sdk `geometry_op.rs`（+340）：`ParamKind`（Length/Angle/Int/Bool/Selectors）+ `ParamDefault`（Float/Int/Bool/Text）+ `ParamDef`（key/label/kind/required/default）+ 三构造器 `required`/`prefilled`/`optional`（const fn，**required 字段显式区分 execute 语义与 GUI 预填**：required=true 时 default 只是表单预填值）+ `BuildOp::param_defs()`（**29 算子全臂**——BuildOp 恰 29 变体=M6 的 24+M7a 的 5，键序与 execute 臂**求值顺序**逐臂对照：块内语句按序、参数从左到右；无参数算子合并臂 `=> vec![]`；tolerance 缺省用 `DEFAULT_SEW_TOLERANCE_MM`=1e-7 与内核缺省一致）+ 钉 1 `param_defs_cover_every_op_with_consistent_kinds`（29 清单+键唯一+label 非空+kind 与 default 类型匹配+Selectors 必填无预填）+ 钉 2 `optional_param_defaults_match_the_execute_fallbacks`（tolerance 五算子=1e-7；Loft is_solid=true/ruled=false；Sweep transition_mode=0）；lib.rs re-export 加 ParamDefault/ParamDef/ParamKind。core-app `geometry_service.rs`（+180）：`execute_native`（**复用 execute_step 全部算子分派**——含 Fillet/Chamfer/Shell 的 single_solid 子选择链路与 Fuse/Cut 带历史布尔；输入是调用方 resolve 好的句柄不是配方步骤；`arity_ok` 前置校验）+ 钉 3 `param_defs_required_keys_align_with_execute_errors`（逐必填键"补前缺后"断言报错含键名 + 失败 `live_count()==0` 回滚）+ 钉 4 `all_declared_keys_satisfy_execute_so_no_missing_param_error`（全声明键补齐→Err 不含缺参字样）+ 钉 5 `execute_native_validates_arity_and_keeps_caller_inputs`（元数错报错且输入句柄存活；正确链路 Translate 平移后体积 1000）。core-app `session.rs`（+250）：`NativeOutcome{element,shape,warnings}`（build/combine 统一返回）+ `create_generic_element`（generic 品类，不走模块工厂）+ `install_executed`（build_geometry ④④′⑤ 共用段：snapshot→insert+geometry_ref 回填→remember_history→释放句柄，warnings 透传**布尔回退说明可观测**）+ `build_native`（0 输入新构件；已有几何拒绝重复构造）+ `modify_native`（1 输入**复用 ShapeId** + `update_after_commit` 关联更新；缓存纪律照 carve_geometry ⑤：**先 forget 旧形状句柄映射再 release**）+ `combine_native`（2 输入主选+副选→**新构件收结果**，名取算子名、品类取主选，原两构件不动；布尔消费语义 G6+ 撤销引擎后完善）+ G3 API 复现用例 2 个（`native_build_modify_combine_reproduce_the_gui_actions`：MakeBox→球→Translate 复用 ShapeId→Fuse 新构件"布尔并"+原构件不动；`native_gates_refuse_wrong_arity_ops`：元数错一律拒绝）。gui.rs（+420）：`build_group` 七组（元数路由启用——0 输入恒可用；1 输入需 selected；2 输入需 selected+secondary；变长 Loft/MakeWire **禁用+tooltip 注明 G4**，`add_enabled`+`on_disabled_hover_text`）+ 模块配方 6 条移入工具栏分组 + `open_op_form`（param_defs default 预填，表单不另立参数表）+ `op_form_window`（egui::Window + `open` 可关 + Grid；ParamKind 路由：Length→DragValue+mm、Angle→DragValue+°、Int→DragValue、Bool→checkbox、Selectors→单行文本 hint JSON）+ `confirm_op_form`（必填 Selectors 空串拒绝——空选择会让 Fillet/Chamfer/Shell 静默无效；**Angle 度→`to_radians()`**；按元数路由三入口；成功关表单、失败保持打开可改后重试）+ `do_native_build`/`do_native_modify`（UpdateReport 关联更新日志照 apply_rebuild 口径）/`do_native_combine`（成功清 secondary）+ `after_geometry_change`（选中/增量网格化单形状/fit_pending）+ `do_pick` 与 elements_panel 均 Ctrl+单击设副选（副选同高亮）+ update() 挂 `op_form_window`。**落地说明：`input_arity()` 未新增**——既有 `arity()` 已是同语义单一事实源（变长返回**下限**，`arity_ok` 已在 M7a 落地），避免重复 API。egui_phosphor 0.7.3 实测：CUBE/SHAPES/PATH/UNITE/INTERSECT/SUBTRACT/STACK/WRENCH/HAMMER/MAGIC_WAND/PACKAGE/CIRCLE/SQUARE/TRIANGLE/SPHERE 存在，CUBES/CURVES/VECTOR_LINE 不存在。门禁：fmt 0；clippy 双腿 `-D warnings` 全绿；默认腿测试全绿（core-sdk **62** / core-app **185**〔含 G3 API 复现 2 例〕/ occt 16 / e2e 各腿全 passed）。提交 f9bcf7d | — |
| **G4 草图特征建模** | 草图编辑器（画布/捕捉/**11 种图元工具** §5.6.1 + **CAD 绘制/编辑操作 18 项** §5.6.2）+ **拉伸（默认 500mm）/ 旋转扫掠（默认 360° 可区间）/ 放样三方式**（§5.6）+ 封闭性检查 + 确认/回滚 | GUI 草图画矩形 → 拉伸 500mm 生成实体；未封闭草图确认出警告；旋转扫掠 20°~180° 命中预旋转方案；倒圆角/倒直线角修剪正确；移动/镜像/偏移/修剪变换正确；**API 复现用例**（纯 API 组装 wire → 拉伸/旋转/放样，不经 GUI） | G3 |
| ↑ **G4-M13 现状** | ✅ **前置已完成（2026-10-02）** | 内核样条接线（提交 746bbdc，7 文件 +309/−20）：①core-kernel——`Kernel` trait 加 `make_bezier_edge(&mut self, points: &[[f64; 3]]) -> KernelResult<ShapeHandle>`（backend.rs 线框生产段，N≥2 响亮校验）+ kernel.rs 薄委托 + **mock 记录型**（`ShapeKind::EdgeSpline`：origin/size 记首末控制点、rotation[0] 记控制点数；tag `edge_spline` 持久化零格式变化；member_points / tessellate 空网格 / project_edges 折线段三臂同步）+ **occt vendor 直通**（扁平点展开 + 逐坐标 finite）；②core-sdk——`ParamKind::Points`（JSON 坐标数组，GUI 文本框）+ `BuildOp::MakeSplineEdge`（30 变体，label"样条边"，arity 0，param_defs `points` 必填无预填）+ 清点钉改名 `build_op_variant_count_is_thirty` + 钉 1 更新（30 清单 + Selectors/Points 同款必填无预填）；③core-app——`require_spline_points`（serde_json 解析 + N≥2 响亮拒绝，照 `require_edge_selectors` 口径）+ execute 臂 + `all_ops()`/`placeholder_value` 30 更新（Points 占位 JSON 三点不共线）+ M13 钉 1/2/3；④gui.rs——ParamKind 三处 match 补 Points 臂（表单 Text 路由）。**实测发现**：vendor 已有 `make_circle_edge`/`make_ellipse_edge`/`make_circle_arc`/`make_ellipse_arc`/`make_tangent_arc`/`make_helix_wire`——§5.6.1 注记 1 已更新（圆/椭圆 P0 直通解析边，采样折线近似废弃）。门禁：fmt 0；clippy 双腿 `-D warnings` 全绿；默认腿测试全绿（core-app **188**〔含 M13 钉 3 例〕/ core-sdk 62）；occt 腿真内核样条钉 `--release` **通过**（9.6s）。G4 剩余：M14 sketch_geom 算法库 → M15 草图编辑器骨架 → M16 图元工具补全 → M17 编辑类 8 项 → M18 三入口特征链路 | G3 |
| ↑ **G4-M14 现状** | ✅ **已完成（2026-10-02）** | 草图纯几何算法库（提交 10fe4c6，2 文件 +594）：`apps/desktop/src/sketch_geom.rs` 16 个 DVec2 纯函数——segment_intersection（参数法求交，平行/共线/延长线 None、t/u∈[0,1]）/ closest_point_on_segment（clamp 投影）/ point_segment_distance / circle_through_three（外接圆，共线 None）/ arc_midpoint（ccw/cw 扫掠方向角平分，同角视为整圆取对径点）/ mirror_point（直线镜像）/ rotate_point / scale_point（绕 pivot）/ offset_segment（左法向平行偏移）/ offset_circle（半径收缩 ≤0 None）/ is_closed_loop（首末点距离≤容差，<2 点不封闭）/ catmull_rom_to_bezier（重复点法切线→分段三次贝塞尔 [起点,控制1,控制2,终点]）/ fillet_corner（切点距顶点 r/tan(φ/2) + 角平分线圆心 r/sin(φ/2)，共线/针尖 None）/ chamfer_corner（切点=distance，边长不足 None）/ trim_segment（keep_near 侧保留，无交点原样返回）/ split_segment（投影打断，端点/段外 None）；22 单测（直线拟合点共线性、直角倒圆角 (-r,r) 圆心、修剪双侧保留、打断投影 clamp 等语义钉）；main.rs 挂 `mod sketch_geom;` **不门控 feature**（纯算法 + glam 非可选依赖，默认腿可测；模块级 `#![allow(dead_code)]` 抑制 bin crate 下 M15 接线前未用告警）；门禁 fmt 0 + clippy 双腿 `-D warnings` 全绿 + 默认腿测试全绿（bim-desktop bin 22 + e2e 1 / core-app 188 / core-sdk 62）+ `cargo xtask check-deps` 合规。M15 草图编辑器直接消费本模块（画布 y 向下，ccw 换算在接线层）。 |
| ↑ **G4-M15 现状** | ✅ **已完成（2026-10-02）** | 解析边直通 + core-app 链路（提交 c6c1171，8 文件 +1105）：①core-kernel——`Kernel` trait 加 `make_circle_edge`（7 参）/`make_circle_arc`（9 参）/`make_ellipse_edge`（8 参，**无主轴方向参数**——vendor 主轴沿法向局部 +x，任意朝向上层"造边+rotate"组合）三直通 + **mock 记录型**（`ShapeKind::EdgeCircle`/`EdgeEllipse`：origin=AABB 最小角点、rotation 记圆心存证；member_points 4 轴向极值点 / project_edges 投**精确圆椭圆曲线**〔CurveKind::Circle/Ellipse，比设计定稿的外接方框保留类型信息，与 Sphere/Cylinder 臂同款〕；make_circle_arc 转三点弧零新 kind）+ **mock rotate 语义修复**（线种类 rotation 是**记录点**非姿态存证——rotate 当点真转不累加旋转向量，否则"造边+rotate"进 wire 后圆心被角度漂移）+ M13 遗留补 EdgeSpline/EdgeCircle/EdgeEllipse 的 sub_shapes matches! 漏臂 + occt vendor 直通（手写零法向校验，occt.rs 无 normalize_axis helper）+ occt 钉（负半径/零法向/短>长 InvalidArgument / 整圆 bbox±50 / 四分之一弧 ccw / 轴对齐椭圆主轴沿 +x / 连通 wire=直线+弧 sub_shape_count==2，`--release` **133 passed 全绿**）；②core-app——`sketch.rs`（SketchEntity 五变体 + normalize_arc_angles ccw 归一）+ `build_sketch_wire`（先造边后 wire、失败 release_many 回滚、成功释放中间边）+ `build_sketch_profile`（**先 wire 后元素**，失败零半成品）+ 11 钉；③session——`build_sketch_profile`（snapshot+edit 照 install_executed 口径）。GUI 骨架（提交 3e35e40，5 文件 +1243）：`sketch_tool.rs`（**不门控**，9 单测默认腿可测，模块级 `#![allow(dead_code)]` 照 sketch_geom 同款）五种工具状态机（折线/矩形/圆/椭圆/弧；重合点击**绝不产出退化图元**、椭圆第三击垂距 clamp 短≤长、弧 ccw 归一 + 0 扫角不推进）+ snap_grid/snap_entities（端点/中点/中心/圆 4 极值点最近）+ orthogonalize；`sketch_editor.rs`（gui 门控）egui 浮窗画布——视图变换（滚轮缩放**视点定标在光标处**/中右键平移/y 翻转）、网格细+主+原点轴、实体渲染（圆真圆/椭圆主轴旋转采样 64 段/弧归一采样）、捕捉优先退回网格、Shift 正交、Ctrl+单击选中、撤销/重做/删除/清空、完成→`build_sketch_profile`；gui.rs 集成（sketch 字段 + sketch_editor_window **take+match** 避开借用冲突〔disjoint 字段借用 session/logs〕 + toolbar 三入口〔拉伸/旋转扫掠/放样草图共用同一编辑器，M18 接特征链路〕+ update 挂载）+ main.rs mod 声明。**实测修复**：egui 0.29 的 `double_clicked` **蕴含** `clicked`（response.rs 源码实证 `self.clicked && ...`）——事件收集改先查双击，否则折线双击结束的第二击被当"单击加点"且 DoubleClick 分支永不可达；backtrack 测试对齐 pop 语义（1→0 点撤点成功、空折线再撤 false）；`NativeOutcome` 加 Debug derive；测试模块 `#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]` 沿用仓库同款（workspace lints `panic/unwrap_used = deny`）。门禁：fmt 0 + clippy 双腿 `-D warnings` 全绿 + 默认腿测试全绿（bim-desktop bin **31**〔含 sketch_tool 9〕/ core-app **201**）+ check-deps 合规。G4 剩余：M16 图元工具补全 → M17 编辑类 8 项 → M18 三入口特征链路 |
| ↑ **G4-M16 现状** | ✅ **已完成（2026-10-02）** | 图元工具补全两提交（①eb00245，4 文件 +236/−4；②b54caf9，3 文件 +1276/−41）。①**数据层（提交 1）**：SketchEntity 三新变体——`Bezier(Vec<Vec<DVec2>>)`（分段贝塞尔，每段控制点序列 ≥2 点一条 `make_bezier_edge` 直通，多段各产一条边）/`Point(DVec2)`（辅助顶点，**不进 wire、不进封闭检查**）/`AuxLine{from,to,ray}`（构造线 ray=false / 射线 ray=true，不进 wire）；`build_sketch_wire` 加空边检查（辅助图元跳过后 edges 为空 → RecipeNotExecutable 明确提示"点/构造线/射线是辅助图元"）；`assemble_sketch_edges` 加 Bezier 臂（每段单句柄、段 <2 控制点报错）+ 辅助跳过臂 + 4 钉（分段建边/辅助跳过/纯辅助拒绝/段控制点不足拒绝）；②**算法**：sketch_geom 加 `bezier_sample`（de Casteljau n 阶取值）+ `bezier_sample_polyline`（均匀参数折线采样，渲染用）+ 3 钉；③**工具（提交 2）**：SketchTool 10 新变体（内接/外切多边形、三点弧、贝塞尔、样条、点、构造线、射线、圆角、倒角）ALL 15；SketchDraft 9 新状态；click 推进（多边形两击中心+顶点/边中点定大小〔边数参数行、<3 视为 3〕；三点弧三击外接圆 + **中间点定方向**——ccw 扫掠含中间点则 s→e 存储，否则反向存储同一几何弧；辅助图元单击/两击；退化不推进）；`finish_polyline` 泛化 `finish_open`（折线 / 贝塞尔〔单条 n 阶段〕/ 样条〔Catmull-Rom n 点→n-1 段三次〕统一收尾）；backtrack 泛化全部草稿（三点弧退中间点、两击类退第一击、倒角退选定）；`polygon_vertices`（内接顶点 / 外切边中点口径，**闭合点显式取首点副本**——cos(TAU)≠1.0 浮点误差会让首末点差 ~1e-14）；`pick_corner_segment`（单段折线过滤最近拾取）+ `pick_corner`（两次选定 → Both、同图元忽略）+ `apply_fillet`/`apply_chamfer`（segment_intersection 求交点 v → fillet/chamfer_corner 切点 → **替换两段+插连接图元**〔圆角弧 / 倒角直线〕，圆角弧方向 `arc_midpoint` 试 ccw 取中点靠近 v 者〔凸向交点侧〕，切点越出原线段 / 平行共线 / 同下标 / 多段折线一律 Err 图元不变）；④**GUI**：状态加 polygon_sides/fillet_radius/chamfer_distance 参数行字段 + `fresh_draft`（工具切换/复位/清空共用，正多边形带边数副本）；工具栏拆两行（15 工具 `horizontal_wrapped` + 操作行按工具显示 DragValue 参数、进行中草稿同步边数）；apply_event 加圆角/倒角拾取分支（**选图元不选点**，成功/失败入日志）；draw_entity 贝塞尔升级精确采样曲线（提交 1 的控制多边形折线退役）；draw_preview 加 9 分支（多边形预览/三点弧整圆预览/贝塞尔 n 阶采样+控制多边形/样条拟合采样/辅助线橡皮筋）；画布实体层圆角/倒角拾取高亮；anchor_of/last_point_of 补新变体。**门禁五道全绿**：fmt 0 + clippy 双腿 `-D warnings` + test --workspace **795 passed**（提交 1 778 + 17）+ check-deps 合规。G4 剩余：M17 编辑类 8 项 + 显示型标注 → M18 三入口特征链路 | G3 |
| ↑ **G4-M17 现状** | ✅ **已完成（2026-10-02）** | CAD 编辑类 8 项 + 显示型尺寸标注（提交 e533ecd，4 文件 +1900/−48）。**数据层**：core-app SketchEntity +3 显示型标注变体——`DimLinear{from,to,pos,linear}`（linear=true 测轴对齐增量 |dx|≥|dy| 取 dx 否则 dy / false 测真实距离、尺寸线平行 from→to 过 pos）/ `DimRadius{center,radius,pos}`（完成时捕获圆心半径）/ `DimAngle{vertex,arm1,arm2,pos}`（两线段交点定角顶点、两臂取距交点远端方向点）——全部**不进 wire**；geometry_service wire 跳过臂扩 3 变体。**工具层（sketch_tool）**：SketchTool +12（编辑 8：Move/Copy/Rotate/Scale/Mirror/Offset/Trim/Break + 标注 4：DimLinear/DimAlign/DimRadius/DimAngle）→ `DRAWING_TOOLS` 15 / `EDITING_TOOLS` 12 / `ALL` 27 分行常量；SketchDraft +12 交互状态（Move/Copy{target 下标,base}、Rotate/Scale{target,base,reference}、Mirror{target,first}、Trim{boundary}、DimLinear/DimAlign{first,second 点}、DimRadius{circle 捕获}、DimAngle{first,second 线段下标}）；`EditOutcome` 四态（Continued / Ignored 草稿保持 / Applied{replace,add} / Failed 日志+复位）+ `edit_click(p, picked, entities)` 十一项统一推进——**复制三击后草稿不复位**（同一基点连续多份，Esc/换工具结束）、Trim 两击保留单击侧（无交点 Failed）、Break 投影打断（端点/段外 Failed，段内 replace+add）、DimRadius 拾圆捕获 (center,radius) 再放位置、DimAngle 三击（两线段交点 → 两臂 → 第三击定角弧半径；平行共线 Failed）；`transform_entity(e, f, flip_handedness)` 点映射底座——折线/贝塞尔/点/构造线全点映射、圆探针（center+(r,0)）还原新半径（平移/旋转/镜像不变、均匀缩放同乘）、椭圆长轴端点探针还原主轴方向与比例（|new_d|=比例×major 即新长半轴）、弧起终点探针还原半径与起终角 + flip（镜像）起终点互换后 ccw 归一（镜像翻手性、几何弧不变扫向反转）、标注位置点全映射；translate/rotate/scale/mirror_entity 四包装（scale 负比例=点反射手性保持）；`offset_entity`（单段折线左法向平行偏移〔正=左侧〕/ 圆同心〔正=向外〕，半径不足/曲线/多段 Err）；backtrack 补编辑草稿臂（Rotate/Scale reference→base→target、Move/Copy base→target、Mirror first→target、Dim second→first）；hint 12 新臂全量阶段文案；`edit_highlight`（圆角/倒角 first + Move/Copy/Rotate/Scale/Mirror target + Trim boundary + DimAngle first，画布拾取反馈单点化）。**GUI**：工具栏两行（绘制 15 / 编辑标注 12 各 `horizontal_wrapped`）+ 偏移距离 DragValue 参数行（负=右侧/向内）；apply_event 三路分派（Fillet/Chamfer 现流 / **Offset 编辑器侧直接应用**——拾取即按参数行距离成新图元、原图元不动 / 其余 11 项走 `edit_click`，picked=`pick_entity_at`；Applied 后草稿复位为 None 则 `fresh_draft` 重启同工具）；draw_entity +3 标注渲染（尺寸线+两端延长线+45° tick+距离数字 / 半径引线+R 数字 / 角弧〔取含标注位置一侧〕+度数；`DIM_COLOR` 青蓝 0x50b8d8 与几何图元区分、选中转黄高亮）；draw_preview +12（Move/Copy 平移幽灵+橡皮筋、Rotate/Scale 参考灰线+当前角/比例变换幽灵、Mirror 虚线镜像线+镜像幽灵、标注实时数值跟随光标）；anchor_of 补编辑草稿（正交参照取最近放置点）、pick_entity_at +3（标注按位置点拾取）、snap_entities 标注跳过。**实测修复**：transform_entity 椭圆探针公式（|new_d| 本身即新长半轴，误乘 major 会得 major²）；Rotate/Scale 预览 match None 臂 ShapeIdx/() 类型对齐；egui Pos2 无加法（mid=qa+(qb−qa)×0.5）；clippy collapsible_if ×5 合并为 edition 2024 let-chains。测试 +13（move 平移复位 / copy 连续两份 / rotate 参考差角+参考点重合 Ignored / scale 2× 圆半径同乘 / mirror 弧起终点互换+镜像线退化 Ignored / 椭圆探针旋转缩放 / offset 线段左侧+圆向内+半径不足+曲线多段 Err / trim 左右双侧+无交点 Failed / break 段内+端点 Failed+落空 Ignored / DimLinear+DimAlign linear 真假+重合 Ignored / DimRadius 捕获+非圆 Ignored / DimAngle 交点两臂+平行 Failed / edit_highlight 阶段跟踪+圆角复用）。**门禁五道全绿**：fmt 0 + clippy 默认腿 + clippy gui 腿 `-D warnings` + test --workspace **808 passed**（795+13）+ check-deps 合规。G4 剩余：M18 三入口特征链路 | G3 |
| ↑ **G4-M18 现状** | ✅ **已完成（2026-10-02）——G4 收官** | 三入口特征链路 + API 复现用例（提交 83a8021，5 文件 +923/−24）。**数据层（core-app sketch.rs，+190）**：`SKETCH_CLOSED_TOL_MM` = 0.01 mm 容差常量（比内核 1e-6 级点重合宽松——草图端点来自点击 + 捕捉，留毫米级余量防"视觉闭合"误报）+ `is_closed_profile(entities, tol)` **单一封闭环判定**（零内核调用）——线状图元（折线 / 贝塞尔首段首点 + 末段末点 / 圆弧归一后起终点）各贡献一对端点、整圆 / 整椭圆自身即环、辅助图元（点 / 构造线 / 射线 / 标注）不参与（与 wire 组装口径一致）；**恰 1 独立环**且无线状 → 封闭；无环且全部端点**两两配对**（每个端点容差内计数含自身 ≥ 2）→ 封闭（多段线 / 多图元拼环均可）；其余（无几何 / 多环 / 圆线混合）→ 未封闭——多环交内核权威拒绝，本检查是特征门面第一步的快速反馈。+8 单测（矩形拼环 / 封闭折线 / 开放 L 形 / 整圆 / 弧+线拼环 / 圆线混合 / 超容差缝隙 / 纯辅助）。**特征门面（core-app session.rs，+524）**：`RevolveAxis`（X/Y/Z + `direction()` / `label()`）+ 三门面照 combine_native 纪律——`build_sketch_extrude(name, entities, height_mm)`（①封闭检查〔未封闭 → RecipeNotExecutable，GUI 警告 + **留在草图**〕②`build_sketch_wire` 造 wire ③`execute_native(Extrude, dx=0, dy=0, dz=height)` **dz 即草图平面法向**）+ `build_sketch_revolve(name, entities, axis, start_deg, end_deg)`（①封闭检查 ②**共面校验**——草图在 z=0 平面 → 轴须在该平面内，Z 轴垂直于草图平面、轮廓绕它只会扫出平面环形 → 拒绝 ③**区间校验**——扫角 = 终角 − 起角 ∈ (0°, 360°] ④wire ⑤**轮廓预旋转方案**（§5.6 行 203：起角非零 → `kernel.rotate(wire, 0,0,0, dir, start_rad)` 先转到起始角再扫终角 − 起角，**零内核改动**；rotate 返回新句柄不消费输入，旧 wire 释放）⑥`execute_native(Revolve, 轴过原点 + 角度弧度)`）+ `build_sketch_sweep(name, entities, path_element)`（①封闭检查 ②路径构件须有几何 ③截面 wire ④`resolve` 懒重建路径句柄（**缓存句柄归 geometry 管理**，失败释放截面 wire）⑤**路径起点切向尽力校验**（§5.6 行 204 ⑤：`tessellate_for_pick(path_shape, 0.1, 0.5)` 与拾取链路同挠度可复用缓存，采样前两点近似切向，|tz| < 0.99 → warnings 警告**不阻断**；**采样不足静默跳过**——真实内核对 wire 无网格化顶点，每次都警告会成噪音，实测发现）⑥`execute_native(Sweep, 输入恒 [profile, spine])`）；共用段 `install_sketch_solid`（`Element::new(generic)` + `install_executed` 落盘——结果句柄由 install 统一释放）；**输入 wire 是本门面临时句柄**（`execute_native` 不释放输入——照 combine_native 纪律，那里的输入是缓存句柄），成功路径**补释放**（失败路径已释放，`live_count` 回 0）；失败一律回滚回草图（GUI 警告 + 留在草图，**绝不丢草图**）。**GUI**：`SketchEntry` 四入口（Profile 通用 / Extrude 拉伸 / Revolve 旋转 / Sweep 放样 + `label()`）+ SketchEditorState 特征参数（`extrude_height` 500 默认 / `revolve_axis` X 默认〔参数行只给 X/Y——Z 轴不共面〕/ `revolve_start_deg` 0 / `revolve_end_deg` 360 / `sweep_path: Option<(ElementId, String)>`）；工具栏**四入口**按钮（通用草图 VECTOR_THREE / 拉伸 FRAME_CORNERS / 旋转 PATH / 放样 SHAPES）——**放样路径先行**（§5.6 ①指定路径 → ②草图：须先选中路径构件，未选中日志响亮提示并不开编辑器）；特征参数行按入口显示（拉伸高度 DragValue / 轴 selectable + 起终角 DragValue / 路径名 label 或 ⚠ 提示）；「完成」按入口分派四门面（失败警告 + **留在草图**，绝不丢草图）+ hover 文案按入口；窗口标题带入口名；`SketchEditorState::new` 收敛为 `new_with_entry`（唯一构造器）。**API 复现用例（mock 腿 8 + occt 真腿 1）**：拉伸端到端（element / geometry_ref 回填 / extrude 形态文本 / live_count 0）+ 未封闭拒绝 + NaN 高度内核拒绝（wire 回滚）+ 旋转 360° 全周 + 20°~180° 预旋转方案（旧 wire 也释放）+ Z 轴共面拒绝 + 区间校验（扫角 ≤0 / 相等 / >360°）+ sweep mock 响亮拒绝（截面 wire 回滚、spine 缓存保留 live_count=1——resolve 懒重建是缓存非泄漏）+ occt 真腿 sweep 端到端（竖直脊线切向 = +Z = 草图平面法向，无警告，`--release`）。**门禁五道全绿**：fmt 0 + clippy 默认腿 + clippy gui 腿 `-D warnings` + test --workspace **824 passed**（808+16）+ check-deps 合规；occt 真腿 core-app **分模块**全绿（session 31〔116s〕/ selector 7〔43s〕/ projection 12〔19s〕）——**全量一起跑存在既有 0xc0000409 fastfail**（OCCT JIT 重建累积压力，非 M18 引入——分模块跑全部通过，记录在案待后续会话拆分重内核测试）。G4 收官（M13 样条接线 → M14 sketch_geom → M15 草图编辑器 → M16 图元工具 → M17 编辑标注 → M18 特征链路全闭环） | G3 |
| ↑ **G5-M19 现状** | ✅ **已完成（2026-10-03）** | 数据层 schema 列出/覆盖 + 三门面 + API 复现用例（提交 911608f，2 文件 +247/−2）。**core-sdk registry.rs（+2 方法 +4 钉）**：`schemas()` 全部属性模式按模式名有序（照 `categories()`/`layers()` 先例，BTreeMap values 天然排序——类型管理器列表数据源）+ `replace_schema()` **覆盖式登记**（存在则覆盖 / 不存在则插入；与 `register_schema` 的差异——后者对同名异内容响亮拒绝〔模块装载语义：两份不同定义撞名须人工裁决，重复装载靠内容幂等容错〕，前者直接覆盖〔类型管理器编辑语义：用户改完字段就该落库，不需要先删再建〕；空名校验沿用 InvalidManifest；⚠ 覆盖**只影响新实例**——既有实例 PropertyMap 是落成时刻快照不回写〔设计文档 3.3 句柄可丢文本为真相〕）+ 4 钉（schemas 按名排序 / 覆盖改内容〔旧字段消失新字段生效 + 覆盖后旧内容再 register_schema 仍 DuplicateSchema〕/ 缺席插入 / 空名拒绝）。**core-app session.rs 三门面（+55）**：`list_schemas() -> Vec<PropertySchema>`（转发 registry().schemas() 克隆）+ `upsert_schema(schema) -> AppResult<()>`（转发 registry_mut().replace_schema；模块装载幂等语义仍走 install_module）+ `validate_element_schema(element: ElementId) -> AppResult<()>`（转发 validate_element；元素不存在 → Model(ElementNotFound)；G5-M21 属性面板确认链路的拦截点）。**API 复现用例（+1 钉 `schema_api_upsert_validate_build_rebuild_end_to_end`）**：纯 API upsert_schema → list_schemas 断言 → create_generic_element + document.edit 挂 schema 属性 → validate Ok → 三类非法值拦截（缺必填 dx / dy 类型 Text 不匹配 / 未知字段 weight）→ 无模式元素 UnknownSchema → install DemoModule 提供 test.demo.box 算子 → build_geometry（元素属性即算子输入，geometry_ref 回填）→ rebuild_geometry（UpdateReport.changed_shapes==[原 ShapeId] 构件身份不变；**元素属性保持 dx=10 不被临时参数回写**——M10 临时参数语义实证，G5-M21 待确认点：属性编辑是否需要回写元素属性）。**门禁五道全绿**：fmt 0 + clippy 双腿 `-D warnings` 0 + test --workspace **829 passed**（M18 824 + 本轮 5：registry 4 + session 1）+ check-deps 合规。G5 剩余：M20 GUI 类型管理器（Types 页）→ M21 属性面板按 FieldDef 升级 + apply_rebuild validate 拦截 + 元素树 | G3 |
| ↑ **G5-M20 现状** | ✅ **已完成（2026-10-03）** | GUI 类型管理器 Types 页（提交 0749577，3 文件 +593/−5）。**分层照 sketch_tool 哲学**：纯逻辑抽到**不门控**模块 `types_editor.rs`（默认腿可单测，8 单测），egui 渲染与交互在 gui.rs（gui 门控）——照 sketch_tool/sketch_editor 同款分层。**types_editor.rs（纯逻辑 +408）**：`FieldRow` 编辑行（GUI 表单 ↔ FieldDef 的桥——name / field_type / unit 文本〔空=无〕/ required / has_default / default_text〔提交时按类型解析〕/ enum_options〔逗号分隔〕/ doc）+ `blank()` 空白行 + `from_def()` 快照（默认值反序列化文本、枚举选项拼接）+ `to_def()` 构造（**校验链**：字段名非空 → Enum 至少一选项〔逗号切分去空白〕→ 默认值按类型解析〔Bool true/false / Int i64 / Float f64 / Text 原文 / Enum 选项内 / **Ref 不支持默认值**〕→ 枚举默认值须在选项内）+ `parse_default` / `default_to_text` / `type_label`（六种中文标签）+ `FIELD_TYPES` 常量（Enum 空选项占位）+ `TypesEdit` 状态（type_name / fields / is_new / message）+ `new_blank()`（一行空白字段直接改）+ `from_schema()` 编辑快照 + `build_schema()` 校验构造（类型名非空 → 至少一字段 → 逐行 to_def → **字段名重复拒绝**）。8 单测：空白行拒绝 / **schema 快照无损往返**（Float+单位+必填+默认 / Text / Enum 三字段 full roundtrip）/ 默认值按类型解析（8 例含 Bool yes 拒绝、Int 4.5 拒绝、Ref None）/ 枚举选项与默认值校验（空选项拒绝 / 逗号切分去空白 / 选项内 Ok / 越界拒绝）/ 字段名重复 / 类型名空与字段表空 / Ref 无默认 / 常量覆盖六种。**gui.rs Types 页（+180）**：Types 占位臂换 `types_panel`——类型列表（`list_schemas` 按名有序，`selectable_label` 点击进入编辑快照 + 编辑中高亮）+ 「新建类型」按钮（`new_blank()`）+ 编辑区（新建/编辑双标题；类型名新建可填 / **编辑只读**〔不做改名语义——改名会插入新条目旧条目残留〕；字段表格 Grid：名〔doc tooltip〕/ **类型下拉**〔`egui::ComboBox::from_id_salt`——**from_id_source 已 deprecated**，egui 0.29.1 源码实证；Enum 高亮 `matches!` 特防 `Enum(vec![]) != Enum([...])`，已是枚举再点不重置选项〕/ 单位 / 必填 checkbox / 默认值路由〔has_default checkbox + 按类型：无 →"—" / Ref →"引用无默认" / 其余文本框〕/ 说明 / 删行）+ Enum 类型追加「选项」行（逗号分隔）+ 加字段 + message 红字 + 保存/取消；**保存动作段照 props_panel「渲染收集动作」**（edit 可变借用圈在渲染段，动作借用结束后执行）：build_schema 校验失败红字拦截 → 新建模式重名校验（`list_schemas` 查重，编辑模式覆盖语义）→ `upsert_schema` 覆盖落库 → 成功关编辑器 + log「✓ 类型已保存」；**约束诚实提示**（§5.3）：编辑模式 ⚠「覆盖保存只影响新实例：既有构件的属性是落成时刻快照，不回写」+「自定义类型是会话态：不随 .bimdoc 持久化，重开文档后丢失」。**实测修复**：E0614 `*row.field_type` 解引用错误（字段访问非引用）→ `row.field_type == *ft`——默认腿不编译 gui.rs 故仅 gui 腿抓出（双跑 clippy 的意义）。**门禁五道全绿**：fmt 0 + clippy 双腿 `-D warnings` 0 + test --workspace **837 passed**（M19 829 + types_editor 8）+ check-deps 合规。G5 剩余：M21 属性面板按 FieldDef 升级 + apply_rebuild validate 拦截 + 元素树 | G3 |
| ↑ **G5-M21 现状** | ✅ **已完成（2026-10-03）——G5 收官** | 属性面板按 FieldDef 升级 + validate 拦截 + 元素树（提交 69dcc9d，2 文件 +387/−49）。**session 三新门面（core-app session.rs）**：`validate_map(schema_name, map)`（校验**编辑中的新值**——与 validate_element_schema 校验元素当前存储属性互补；未注册 → UnknownSchema，G5-M21 确认链路的 **set 前拦截**）+ `update_element_properties(element, properties)`（**回写语义定版**：validate 通过后回写元素 properties——属性与几何一致；M10 现状不回写会导致重选构件后编辑快照变回旧值，语义混乱；自动成为撤销步）+ `delete_element(element)`（**删除构件**：元素 + 级联清关系〔remove_element 语义，撤销步〕+ **删几何记录**〔不残留孤儿几何——has_dangling_geometry 的消除路径〕；⚠ 已知限制诚实：受影响视图的图纸实体不自动清除——不重投，调用方提示用户对相关视图「重新生成」）+ 2 钉（validate_map set 前拦截 + 回写往返〔回写后元素属性读回新值〕；delete_element 元素+几何都不残留 + 元素不存在明确报错）。**props_panel 按 FieldDef 完整路由（gui.rs，§5.4）**：ParamEdit 加 `fields` 模式定义快照（schema 未注册时为空）——**已定义字段按 schema 顺序渲染**（不再是 BTreeMap 字典序）+ `required` 缺失标红（colored_label RED +「（必填）」后缀 + tooltip「必填字段缺失：确认时被校验拦截」）+ `doc` 字段名 tooltip + `unit` 输入框后缀（M10 硬编码 " mm" 改为 def.unit）+ `Enum(options)` **下拉选择**（ComboBox from_id_salt，之前只是文本框）+ `default`「默认」小按钮**一键回填**（set_default 收集 → 借用结束插入）+ **未知字段红字附加**（schema 没有此字段定义——validate 会拦截，渲染附加红字比隐藏诚实）+ 提示文案更新（"非法值会被已注册模式拦截"）。**apply_rebuild 语义定版（§5.4 确认链路）**：**validate → set → rebuild** 三步——①validate（schema 已注册时校验新值；未注册照 M10 现状跳过——参数自由）②set 回写（update_element_properties；失败 log 不 rebuild）③rebuild_geometry（M6 关联更新首链在 GUI 生效——dirty + 增量重建 + stale 标脏 + 警告逐条入日志）。**元素树（elements_panel 升级，§5.5）**：平铺列表 → **品类分组折叠**（BTreeMap 按品类天然有序；组头 CARET_DOWN/RIGHT 折叠开关〔phosphor regular 常量编译实证〕+ 品类名 + 计数）+ 节点（显示名〔无几何标注〕+ 选中 / Ctrl 副选联动保留 + **删除按钮**〔TRASH〕）+ 删除动作段（文档遍历借用结束后执行：删除前读 geometry_ref → delete_element → meshes.remove + 选中态清理〔selected/secondary/edit/op_by_element 归零〕+ dirty 全量重建 + log 提示重新生成视图）+ **可见性眼睛记 P1**（Element 无 visible 字段——会话态显隐，后续交付）。**门禁五道全绿**：fmt 0 + clippy 双腿 `-D warnings` 0 + test --workspace **839 passed**（M20 837 + session 2）+ check-deps 合规。**G5 收官**：M19 数据层 schema 列出/覆盖 + 三门面 + API 复现用例 → M20 GUI 类型管理器 Types 页 → M21 属性面板按 FieldDef 升级 + validate 拦截 + 元素树，全闭环（§5.3/5.4/5.5 三规格全交付；~~余 P1：元素树可见性眼睛 / schema 持久化 / 删除后视图自动重投~~ **P1 三项已于 G6-M2 兑现（提交 11c363e）**） | G3 |
| **G5 类型系统** | 类型管理器 + 实例属性面板升级 + 元素树 | GUI 新建类型 → 造构件挂该类型 → 属性面板按 schema 表单编辑 → validate 拦截非法值；**API 复现用例**（纯 API register_schema → build → rebuild） | G3 |
| ↑ **G6-M1 现状** | ✅ **已完成（2026-10-03）** | 撤销/重做 UI 接线事务引擎到菜单栏与快捷键（提交 8489022，2 文件 +195）。**core-app session.rs（+91）**：CoreSession 新增 6 个撤销/重做转发门面——`can_undo()`/`can_redo()`/`undo_depth()`/`redo_depth()`（只读状态，菜单置灰与深度文案数据源）+ `undo()`/`redo()`（转发 DocumentService 双侧快照历史：**文档与图纸一起回滚/前进**，§M5 语义——撤销后模型与图纸一致，视图/图元/关联索引一并回滚）；**内核侧零改动实证**：`GeometryService::resolve` 未命中缓存时从文档几何记录重新装载（geometry_service.rs 行 349-360）——撤销后记录消失（resolve 失败但无人调用：refresh_meshes 全量重建只处理 elements() 里的 geometry_ref）、重做后记录回来 resolve 恢复可用（缓存命中或从 BREP 文本重装）。**+2 钉**：`undo_redo_facade_moves_both_stacks`（add ×2 = 2 撤销步 → undo 一步元素 -1 / 重做栈 +1 → redo 还原 / 重做栈清空，深度与 can_* 严格对账）+ `undo_redo_keeps_the_geometry_chain_resolvable`（建几何 → tessellate Ok → undo 几何记录消失 → redo 记录回来 → **tessellate 仍 Ok**——重做后几何链路可用性的语义钉）。**gui.rs（+104）**：①菜单栏「编辑」组升级（G2 只有「清空选择」）——撤销/重做菜单项**深度入文案**（「撤销（N 步）」/ 无可撤销只显示「撤销」）+ `add_enabled(can_undo, ...)` **不可用时置灰**（button 无 disabled，egui 惯例 add_enabled）+ 分隔线隔开「清空选择」；②快捷键 Ctrl+Z / Ctrl+Y（update 里跟随 Ctrl+S/Ctrl+O 先例：`modifiers.command && key_pressed`——command 在 Windows=Ctrl、macOS=Cmd 跨平台；egui 0.29 `key_pressed` **含 key-repeat**〔input_state/mod.rs 行 585 实证「Includes key-repeat events」〕——按住连发撤销，CAD 惯例；无可撤销时静默忽略）；③`do_undo`/`do_redo`/`after_history_jump` 三方法（保存动作分组在 save_to 之后）——**撤销后状态清理**：`dirty = true` 全量重建（不设增量集 → refresh_meshes 的 clear 分支清掉已消失几何的孤儿缓存）+ 失效态清理（选中/副选/拾取/参数编辑/图纸视图选中里已不存在的 id 清掉——**先决策后改状态**：文档/图纸只读借用圈在块内收集 5 个清理决策，借用结束后赋值，照 elements_panel 删除动作段先例）+ **op_by_element 不清理**（残留条目对重做友好：重做后元素回来重选即可按原算子编辑；ElementId 只增不复用，残留无语义风险）+ 不自动适配视图（撤销是精细操作，自动 fit 会让视口跳变）+ log「✓ 已撤销（可再撤销 N 步 / 可重做 M 步）」。**门禁五道全绿**：fmt 0 + clippy 双腿 `-D warnings` 0（本里程碑用既有先例 API 无 gui 专属编译问题）+ test --workspace **841 passed**（M21 839 + 本轮 2）+ check-deps 合规。**G6 剩余**：可停靠面板 egui_dock → 出图深化/专业模块深化/Ribbon/多文档页签/外部调用形态（G6+）| G5 |
| ↑ **G6-M2 现状** | ✅ **已完成（2026-10-03）——余 P1 三项打包交付** | 元素眼睛 + schema 持久化 + 删除后视图自动清理（提交 11c363e，7 文件 +728/−81）。**①元素级可见性（元素树眼睛）**：core-model `Element` 加 `visible: bool`（`#[serde(default = "default_visible")]`——旧容器缺字段默认可见，serde 双向兼容：旧程序读新文件忽略未知字段）+ `with_visible` 链式构造器 + 2 钉（默认可见 / visible 随 JSON 往返 + 手工 remove("visible") 模拟旧格式 → 默认可见）；渲染语义定版：某形状的**全部**引用元素都隐藏时才不画（多元素共享同段几何按「任一可见即画」）——viewport 每帧预计算 `visible_shapes: HashSet<ShapeId>` 过滤；gui elements_panel 四元组 (id/name/has_geom/visible) + 三态显示名（原名 /（已隐藏）/（无几何））+ 眼睛按钮（EYE / EYE_SLASH，悬停「隐藏（撤销可回滚）」——`set_element_visibility` 走 document.edit 撤销步）。**②schema 持久化（自定义类型重开不丢）**：store 加**可选段** `SCHEMAS_ENTRY = "schemas.json"`——save/pack 写段、open/from_dir 读段（旧 v2 容器无段恢复空 registry——行为退化为类型管理器为空、validate 不拦截，不报错）；**FORMAT_VERSION 保持 2**（加可选段不是不兼容变更：旧程序按段显式读直接忽略新段、file_count 只供人核对不校验）+ manifest file_count +1；core-sdk `Registry::restore_schemas` **批量覆盖式恢复**（逐个 replace_schema，空名响应拒绝）+ 1 钉（空 registry 恢复 / 同名覆盖不增条目、旧字段消失新字段生效 / 空名拒绝）；CoreSession::save/open 装配；gui types_panel 提示文案更新（会话态 → 随 .bimdoc 持久化；旧容器无该段打开后为空）。**③删除后视图自动清理（G5-M21 已知限制消除）**：`update_service::update_after_removal` + `RemovalReport{removed_shapes, cleaned_views, removed_entities, removed_annotations}` 新类型（不复用 UpdateReport——reprojected_entities 语义偏移）；**清除语义定版**：remove 场景不能走 reproject 链路（几何记录已删，投影装载 BREP 必失败）——孤儿图元留在视图只会误导，只清「绑定了被删形状」的视图（views_bound_to_shapes）；清除后**不动 up_to_date**（该新鲜的新鲜、stale 的保持 stale）；索引用 rebuild_index 重建（删除低频 O(全部图元) 可接受）+ 2 钉（shape_a 清除 / shape_b 逐字节不变 / free 纯手绘图元不动 / up_to_date 不动 / 索引 dependents_of 空；空入参全 0 图纸逐字节不变）；session::delete_element 返回 RemovalReport（几何删除 + 元素删除在 edit 撤销步内，清理在 edit 之后——**undo 文档与图纸一起回滚孤儿图元随图纸回来，redo 前进到清理后快照，双侧快照天然保证一致**）；gui 删除日志改为「已清理孤儿图元 N 个 / 标注 M 条（索引随清理重建）」（不再提示「重新生成」）。**测试 10 钉**（element 2 + registry 1 + store 2 + update_service 2 + session 3：delete_element 清孤儿 + undo/redo 一致性 / 隐藏是撤销步且持久 / schema 随 save+open 往返）+ container_probe **15 处签名适配**（空段 `&[]`）。**门禁五道全绿**：fmt 0 + clippy 双腿 `-D warnings` 0 + test --workspace **851 passed**（G6-M1 收官 841 + 本轮 10 精确对账）+ check-deps 合规。**G6 剩余**：可停靠面板 egui_dock → 出图深化/专业模块深化/Ribbon/多文档页签/外部调用形态（G6+）| G5 |
| ↑ **G6-M3 现状** | ✅ **已完成（2026-10-03）——出图深化·图纸布置** | Sheet 管理 GUI + 撤销步门面（提交 bc51bde，4 文件 +714/−8）。**调研结论**：链路大头已完备（Sheet/Placement/TitleBlockDef/SheetFormat〔GB/T 14689 留装订边〕+ `sheet_to_2d` 布置求值 + 图框 + 标题栏 + `export_sheet` 整图 DXF/SVG + **sheets 随 drawing.json 持久化**〔store 只拆 views，sheets 整体在头部——持久化天然已通〕+ `draw_drawing_2d` 渲染），本里程碑只补门面与 GUI。**core-app DocumentService::edit_drawing 新门面（+64）**：与 `edit` 同款快照语义（doc + drawing **双侧快照**入撤销栈），闭包参数 `&mut Drawing`——**填补「改图纸没有撤销步」的空白**（drawing_mut 本身不产生撤销步，只有随后一次 edit 才把改动前的图纸间接压栈）；闭包 Err 时双侧一并回滚、不留半成品。**core-doc Drawing::remove_sheet（+39）**：删图纸本体并返回它（布置随本体一起返回）；它引用的**视图不动**（视图是"算出来的"投影结果，可摆到别的图纸）。**CoreSession 4 门面（+257）**：`create_sheet`（**先 edit**〔登记 doc 引用并压入"图纸尚无新图纸"的快照〕**后 add_sheet** 加本体——undo 双侧一起回滚，单撤销步）+ `remove_sheet`（反向：edit 撤销 doc 引用压快照 → remove_sheet 删本体）+ `place_view_on_sheet`（走 **edit_drawing**：布置 + 整图纸 `Sheet::validate` 越界/比例 0 当场拦截〔早失败〕，Err 天然回滚；先查视图存在再取可变借用防 E0502）+ `remove_placement`（**按下标精确移除**——同一视图可布置多次〔详图索引〕）。**GUI 右栏三页（+362）**：Page 加 `Sheets`——既有「图纸」页**更名「视图」**（本页实为视图列表，"图纸"让给 Sheet 页，命名与 core-doc 语义对齐）+ 新 `sheets_panel`（图纸列表〔图号·名称·图幅·布置数〕+ 新建〔名称/图号文本 + 图幅下拉 A0~A4x3；标题栏国标默认 180×56 右下角〕+ 选中操作条〔整图预览/导出 DXF/导出 SVG/删除图纸〕+ 放置视图表单〔视图下拉 + scale_series 比例下拉 + X/Y DragValue〕+ 布置列表〔视图名 (x,y) 1:N + 按下标移除〕——**渲染收集动作模式**〔可变借用圈在渲染段，动作借用结束后执行，elements_panel 先例〕；session 借用**先克隆快照**防 E0502）+ `sheet_preview_window`（整图预览浮窗：`sheet_to_2d` + `draw_drawing_2d` 复用——图幅/图框/标题栏/全部布置一起画）+ `export_sheet_file`（整图导出 DXF/SVG）。**+6 钉**：session 4（create/remove 撤销步〔doc 引用与本体一起回滚/前进〕+ place 撤销步与越界/比例 0 拦截〔失败不留半成品〕+ 按下标精确移除 + 图纸布置随 save/open 持久化）+ document_service 1（edit_drawing 撤销步语义：图纸回滚文档不动 + Err 不留半成品不进栈）+ drawing 1（remove_sheet 返回本体视图不动 + SheetNotFound 响亮）。**门禁五道全绿**：fmt 0 + clippy 双腿 `-D warnings` 0 + test --workspace **857 passed**（G6-M2 收官 851 + 本轮 6 精确对账）+ check-deps 合规。**出图深化余 P1**：视图标题标签（placement 下方标注「视图名 1:N」——会动 sheet_to_2d 既有输出顺序，既有钉子依赖 curves[0]）；**G6 剩余**：可停靠面板 egui_dock → 专业模块深化/Ribbon/多文档页签/外部调用形态（G6+）| G5 |
| ↑ **G6-M4 现状** | ✅ **已完成（2026-10-03）——出图深化·视图标题标签** | sheet_to_2d 每个 placement 下方标注「视图名 1:N」（提交 67707aa，1 文件 +178/−4）。**调研结论**：Text2D 结构（at 插入点图纸坐标 mm Y 朝上 / content / height_mm / layer）+ 标签是 texts 不产生 curves——既有钉子依赖的 curves[0] 首条视图实体顺序天然安全（图框在 placements 之后追加）；texts 断言钉子全部在 view_to_2d 不受影响；标题栏字段只写图名/图号/图幅/比例/修订，标签不与标题栏重复。**实现（interop_service.rs）**：加 `VIEW_LABEL_GAP_MM = 4.0` 常量 + sheet_to_2d 的 placements 循环里 append_view_content 之后写标签——位置在内容包围盒底边下方 4mm、左端对齐内容左边缘（bbox 调一次存变量，half_w = (max.x−min.x)×scale×0.5 / half_h 同款）；空视图（content_bbox = None）half 取 0，标签落布置点正下方 4mm；图层走 `require_layer(LAYER_TEXT)`、字高照「标准」样式（map_or 兜底 3.5mm）与标题栏字段口径一致，解析提到循环外不重复查表；标签文本 `format!("{} 1:{}", view.name, scale_denominator)`。**既有钉子语义对齐**：B5.4 钉子 ⑥⑦（oversized / not_bottom_right 跳过标题栏）断言 `flat.texts.is_empty()` 改为「无标题栏字段文字（视图标签照旧）」——标签属于布置输出而非标题栏字段，跳过标题栏只影响字段文字。**+3 钉**：`sheet_export_writes_a_view_label_under_each_placement`（标签内容/位置 (295,191) 精确断言/图层 LAYER_TEXT/字高 3.5 + curves[0] 仍是 1:100 水平线 dx=10）/ `sheet_export_labels_each_placement_individually`（同视图布置两次 1:100+1:50 → 两条标签各自按比例定位 (145,141)/(440,136)，1:50 内容更大标签更低）/ `sheet_export_labels_empty_views_below_the_placement`（空视图标签落布置点正下方 4mm）。**门禁五道全绿**：fmt 0 + clippy 双腿 `-D warnings` 0 + test --workspace **860 passed**（G6-M3 收官 857 + 本轮 3 精确对账）+ check-deps 合规。**出图深化余项**：标注增强/会签栏等更深需求移交后续；**G6 剩余**：可停靠面板 egui_dock → 专业模块深化/Ribbon/多文档页签/外部调用形态（G6+）| G5 |
| ↑ **G6-M5 现状** | ✅ **已完成（2026-10-03）——可停靠面板 egui_dock** | gui.rs 五区布局升级 egui_dock 0.14（提交 118f58a，4 文件 +261/−122）。**依赖**：egui_dock = "0.14"（workspace.dependencies 与 egui 同段；其 [dependencies.egui] version = "0.29" **精确对齐**，MIT；desktop gui feature 门控 optional——cargo info 实证下载后查其 Cargo.toml）。**调研关键结论**：split 的 fraction 恒归 **old** 节点（`split_left(parent, f, tabs)` 把 tabs 放左侧占 f、原内容在右占 1−f）——lib.rs 例子注释对、tree/mod.rs 的 API 文档注释错，**以实现为准**（Node::split 造 Horizontal{fraction} + show/mod.rs 分隔符 midpoint = min + size×fraction）；`set_focused_node_and_surface((surface, node))` 参数是**元组解构**（传 (a,b) 不传 a,b）；`find_tab` 在 DockState 级（跨 surface，返回 (SurfaceIndex, NodeIndex, TabIndex)）。**DockTab 枚举（替换 Page）**：7 变体 Viewport/Elements/Types/Props/Views/Sheets/Logs（Debug derive 供测试断言）。**default_dock() 默认布局照五区语义**：split_left 左栏（元素+类型叠 tab）15% → split_right 右栏（属性/视图/图纸叠 tab）占 22%（old 占 78%）→ split_below 底部日志 20%（old 占 80%）；两处构造（new/open_document）与「重置布局」共用。**BimTabViewer（TabViewer 实现）**：title（图标+中文名）/ ui（match 转发既有 panel 方法）/ closeable（**Viewport=false**——3D 主体不可关，其余可关）。**update() 改造**：菜单栏/工具栏/状态栏照旧 TopBottomPanel → CentralPanel 包 DockArea（**Style::from_egui 继承主题**；**mem::replace 拆借用**——dock 暂移出、viewer 持 &mut self、渲染完还回，避免 self.dock 与 self 双重可变借用）→ 浮窗 × 4 照旧。**退役**：Page/page_tabs/left_page/right_page/show_logs——dock tab 选中态/关闭/重排由 DockState 天然接管。**日志开关改聚焦语义**（focus_or_restore_tab）：find_tab → set_focused_node_and_surface，被关则 push_to_focused_leaf 加回；菜单栏「视图」组加**重置布局**（恢复 default_dock，关闭 tab 后的找回出口）。**+3 钉（gui.rs tests 模块，gui feature 门控——门禁扩展跑 cargo test -p bim-desktop --features gui）**：default_dock_contains_all_seven_tabs（7 tab 全在主 surface）/ default_dock_stacks_tabs_per_region（元素+类型同 node、属性+视图+图纸同 node、日志独立、左右分区）/ default_dock_is_rebuildable（两次构造布局一致）；tests 模块 `#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]` 沿用仓库同款。**门禁六道全绿**：fmt 0 + clippy 双腿 `-D warnings` 0 + test --workspace **860 passed**（默认腿不变，dock 钉在 gui 腿）+ gui 腿测试 **72 passed**（69+3 精确对账）+ check-deps 合规。**实测修复**：focus_or_restore_tab 误放进 impl eframe::App 块（E0407 not a member of trait）→ 挪到 impl BimApp 固有块；set_focused_node_and_surface 两参数调用 E0061 → 传元组。**G6 剩余**：专业模块深化/Ribbon/多文档页签/外部调用形态（G6+）| G5 |

| ↑ **G6-M6 现状** | ✅ **已完成（2026-10-04）——外部调用形态·CLI 第一期** | bim-server 加 clap 子命令驱动 CoreSession（提交 a289a99，5 文件 +621/−25）。**依赖**：clap = "4.6"（derive feature，workspace.dependencies serde_json 之后；bim-server dependencies 加 clap.workspace = true）。**Command 四子命令**：Demo（端到端装配自检，无子命令默认走它）/ New（造构件 + 建几何 + 可选 `--view` 建视图并初生成 + `--save` 保存）/ Info（打开打印摘要 + 元素/图纸清单）/ Export（`--format dxf\|svg`，`--sheet 图号` 与 `--view 视图名` 二选一，`std::fs::write` 落盘 `output.bytes`）。**FormatArg（ValueEnum）**：Dxf/Svg → `Format` 映射薄壳——DXF/SVG 是当前可用出图出口，其余格式（Step/Gltf/Ifc4x3/Dwg/Gim）或需真内核、或只导入、或 GPL 隔离（Dwg），CLI 不暴露。**单一事实源（实测修复）**：`new --category/--op` 默认值直接引用 `example_beam::{CATEGORY, GEOMETRY_OP_ID}` 导出常量（与 desktop 同款）——CATEGORY 实际值为 **BEAM 大写**，曾用 "beam" 小写导致注册表查不到、`create_element` 报错退出（cli_probe 4/5 失败的根因）；GEOMETRY_OP_ID = "example.beam.rect"。**find_sheet_by_number / find_view_by_name**：按图号/视图名查（用户语义，非内部 id），未命中报现有清单。**mod setup**（原 module_host 改名避撞 `core_app::module_host`）：install_example_beam + apply_layers 共用装配。**+5 探针（tests/cli_probe.rs 新建）**：`env!("CARGO_BIN_EXE_bim-server")` 跑**真实二进制**断言退出码与输出——demo 自检 / new→info 闭环（构件名 + 视图数）/ new→export SVG 头 / export 无目标提示二选一 / 未知图号报现有清单；temp_doc 进程 id 隔离。**门禁六道全绿**：fmt 0 + clippy 双腿 `-D warnings` 0 + test --workspace **865 passed**（G6-M5 收官 860 + cli_probe 5 精确对账）+ check-deps 合规 + gui 腿测试 **72 passed**（69+3 不变）。**G6-M7（已定版）**：HTTP 桥——axum 暴露 CoreSession 门面为 REST（外部调用形态第二期） |

**每阶段门禁**：`fmt` + clippy 双腿 `-D warnings` + 默认腿测试全绿 + GUI 实机（mock 腿）走查。

## 7. 与现有代码的映射（gui.rs ~1700 行改造对照）

| 现有 | 目标 |
|---|---|
| `top_bar`（文件组 + 拾取模式 + 性能基线） | 菜单栏（文件组）+ 工具栏右侧（拾取模式/视图预设/性能基线） |
| `elements_panel`（左：6 条配方按钮 + 构件列表） | 左侧页签：元素树（5.5）+ 模块配方移工具栏 |
| `props_panel`（右，M10 平铺编辑） | 右侧属性页签（5.4 升级） |
| `views_panel`（右，M11） | 右侧图纸页签（原样迁移） |
| `logs_panel`（底） | 底部日志 + 可折叠 + 状态栏 |
| `drawing_preview_window` | 保留浮窗 |
| `op_buttons()`（6 条配方） | 工具栏「模块配方」分组 |

## 8. 风险与选型

1. **param_defs 漂移风险**：`param_defs` 与 `execute` 键名必须对齐——G3 加钉子测试：遍历全部算子，`param_defs` 的每个 key 在 `execute` 缺参时必须产生含该 key 的报错信息（防"表单改了、execute 不认"）。
2. **egui_dock 可停靠面板**：Revit 风格可停靠窗口，egui_dock 是成熟 crate——**本计划不做**，固定五区布局；已移交后续计划（§9）。
3. **图标字体加载**：`add_to_fonts` 与 CJK 兜底的叠加顺序要实测（缺字表现为方框）。
4. **mock 腿兼容**：原生算子在 mock 上响亮报错（沿用现风格 `build_op` 的 Err 日志）；**草图转特征在 mock 腿的降级**——`Extrude`/`Revolve`/`Sweep` 在 mock 的能力按现状（`Unsupported` 响亮报错），草图编辑本身（画布/封闭检查）纯 GUI 双腿一致。
5. ~~**schema 持久化**：`register_schema` 是会话态（registry 不随文档容器保存）——GUI 自定义类型的持久化后补（P1），本期诚实提示"自定义类型重开丢失"。~~ **更新（2026-10-03，G6-M2 已完成，提交 11c363e）**：store 加可选段 `schemas.json`（旧 v2 容器无该段恢复空 registry，不报错；FORMAT_VERSION 保持 2）；`Registry::restore_schemas` 批量覆盖式恢复；CoreSession::save/open 装配——自定义类型随 .bimdoc 持久化，重开不丢。
6. **草图编辑器体量**：画布 + 图元 + 捕捉 + 转特征是本计划最大新增件——G4 内部再拆（先拉伸单特征跑通全链，再补旋转/放样），避免一次交付过大。

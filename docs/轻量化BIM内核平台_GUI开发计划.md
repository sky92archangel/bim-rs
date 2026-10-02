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
- **G6+（§9）**：进程外形态按需单独立项——CLI / 脚本引擎 / HTTP 桥；进程内 API 是它们的地基，先立地基。

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
1. 圆 / 椭圆的采样折线近似是 P0 方案（vendor occt-wasm 无 `fn_make_circle_edge` / `fn_make_ellipse_edge`，仅 `fn_make_ellipsoid` 椭球体）——采样段数走常量，后续内核补算子时替换为解析边（P1）。
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
| **撤销 / 重做 UI** | 事务引擎**已有**（`Document` Delta / `update_after_commit`），UI 未接 | G6 首选——撤销栈 = 事务 Delta 的反向应用，内核侧零改动，纯 UI 接线；建模功能稳定后第一件事 |
| **可停靠面板** | egui_dock（成熟 crate，MIT） | G6——五区固定布局运行一段时间、面板需求明确后再引入，避免过早布局抽象 |
| **出图深化** | M11 已有基础（视图面板 / 2D 预览 / DXF 导出） | G6+——图框 / 图幅 / 标注增强 / 图纸布局，与草图编辑器（G4）共用 2D 渲染骨架 |
| **专业模块深化** | M7 三模块已有（beam / duct / tunnel，49 单测） | G6+——GUI 稳定后回到模块侧：模块配方参数化 UI、更多专业模块 |
| **Ribbon** | 不做（egui 无原生支持，需自绘模拟） | G6+——仅当五区布局被验证不够用时评估；自绘 Ribbon 工作量大，优先级最低 |
| **多文档页签** | CoreSession 单文档会话 | G6+——需要多文档同时打开时；涉及会话管理改造（多 CoreSession 或会话池），牵涉面最大放最后 |
| **外部调用形态** | 进程内 Rust API（§2.1 契约）全覆盖 | G6+——CLI / 脚本引擎（rhai/wasm）/ HTTP 桥按需单独立项；进程内 API 是它们的地基，§2.1 全覆盖原则先立地基 |
- **确认流程**：收集图元 → 组装 wire（`MakeLineEdge` / `MakeArcEdge` 逐段 + `MakeWire`）→ 封闭性检查（首尾点距离 ≤ 容差，P0 纯几何双腿一致）→ 转特征表单（拉伸高度 / 旋转角度区间 / 路径选择）→ `build_op` 执行 → 失败/取消**回滚回草图编辑，绝不丢草图**。

**内核缺口（诚实注记，G4 前置）**：

1. **样条路径**：`make_b_spline_edge` vendor occt-wasm **已有**（`kernel_generated.rs:1842`），Kernel trait **未接线**——G4 前置任务：core-kernel 接线 + `BuildOp` 加 `MakeSplineEdge` 变体（mock 腿按现风格响亮 `Unsupported` 或按点折线近似）；P0 若赶进度可先用三点弧近似路径，样条列为 G4 内任务。
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
| **G4 草图特征建模** | 草图编辑器（画布/捕捉/**11 种图元工具** §5.6.1 + **CAD 绘制/编辑操作 18 项** §5.6.2）+ **拉伸（默认 500mm）/ 旋转扫掠（默认 360° 可区间）/ 放样三方式**（§5.6）+ 封闭性检查 + 确认/回滚 | GUI 草图画矩形 → 拉伸 500mm 生成实体；未封闭草图确认出警告；旋转扫掠 20°~180° 命中预旋转方案；倒圆角/倒直线角修剪正确；移动/镜像/偏移/修剪变换正确；**API 复现用例**（纯 API 组装 wire → 拉伸/旋转/放样，不经 GUI） | G3 |
| **G5 类型系统** | 类型管理器 + 实例属性面板升级 + 元素树 | GUI 新建类型 → 造构件挂该类型 → 属性面板按 schema 表单编辑 → validate 拦截非法值；**API 复现用例**（纯 API register_schema → build → rebuild） | G3 |

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
5. **schema 持久化**：`register_schema` 是会话态（registry 不随文档容器保存）——GUI 自定义类型的持久化后补（P1），本期诚实提示"自定义类型重开丢失"。
6. **草图编辑器体量**：画布 + 图元 + 捕捉 + 转特征是本计划最大新增件——G4 内部再拆（先拉伸单特征跑通全链，再补旋转/放样），避免一次交付过大。

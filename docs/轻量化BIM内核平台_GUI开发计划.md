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

- **P0（本计划）**：视觉基建、布局重组、几何构建工具栏 + 参数表单、类型管理器、实例属性面板升级、元素树。
- **不做**：Ribbon、多文档页签、撤销/重做 UI（事务引擎已有，UI 后补）、专业模块深化、出图深化（M11 已有基础）、可停靠面板（egui_dock 后补）。

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
- **画布**：mm 坐标、网格吸附（可开关）、图元工具（线段 / 矩形 / 圆 / 三点弧 / 多段线）、段内撤销。
- **确认流程**：收集图元 → 组装 wire（`MakeLineEdge` / `MakeArcEdge` 逐段 + `MakeWire`）→ 封闭性检查（首尾点距离 ≤ 容差，P0 纯几何双腿一致）→ 转特征表单（拉伸高度 / 旋转角度区间 / 路径选择）→ `build_op` 执行 → 失败/取消**回滚回草图编辑，绝不丢草图**。

**内核缺口（诚实注记，G4 前置）**：

1. **样条路径**：`make_b_spline_edge` vendor occt-wasm **已有**（`kernel_generated.rs:1842`），Kernel trait **未接线**——G4 前置任务：core-kernel 接线 + `BuildOp` 加 `MakeSplineEdge` 变体（mock 腿按现风格响亮 `Unsupported` 或按点折线近似）；P0 若赶进度可先用三点弧近似路径，样条列为 G4 内任务。
2. **封闭性查询**：`curve_is_closed` vendor **已有**（`kernel_generated.rs:2714`），Kernel trait 未接线——P0 先用草图编辑器内首尾点距离检查（纯几何），内核查询接线为增强项。
3. **revolve 起终点角度**：内核 `revolve` 是单 `angle_rad`（从 0° 起）；用户要"20°~180°"起终点式——**实现方案：轮廓预旋转**（wire 先 `Rotate` 到起始角，再 `revolve`（终止角 − 起始角）），**零内核改动**。

## 6. 实施阶段

| 阶段 | 内容 | 验收 | 依赖 |
|---|---|---|---|
| **G1 视觉基建** | `theme.rs` 配色板 + egui_phosphor 图标 + set_visuals 深色定制 | 双腿编译过；GUI 实机视觉统一；**零逻辑变化** | M12 ✅ |
| **G2 布局重组** | 菜单栏 + 工具栏骨架 + 五区布局 + 状态栏 + 日志折叠 | 现有全部功能（文件/拾取/属性/图纸/预览）在新布局可用 | G1 |
| **G3 构建链路** | BuildOp `param_defs()`（core-sdk）+ 七组工具栏 + 参数表单 + build_op 传参 | GUI 点 MakeBox 改参数 → 视口出对应长方体；Fuse 先选两个构件 | G2 |
| **G4 草图特征建模** | 草图编辑器（画布/图元/捕捉）+ **拉伸（默认 500mm）/ 旋转扫掠（默认 360° 可区间）/ 放样三方式**（§5.6）+ 封闭性检查 + 确认/回滚 | GUI 草图画矩形 → 拉伸 500mm 生成实体；未封闭草图确认出警告；旋转扫掠 20°~180° 命中预旋转方案 | G3 |
| **G5 类型系统** | 类型管理器 + 实例属性面板升级 + 元素树 | GUI 新建类型 → 造构件挂该类型 → 属性面板按 schema 表单编辑 → validate 拦截非法值 | G3 |

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
2. **egui_dock 可停靠面板**：Revit 风格可停靠窗口，egui_dock 是成熟 crate——**本计划不做**，固定五区布局；后续可作为 G6。
3. **图标字体加载**：`add_to_fonts` 与 CJK 兜底的叠加顺序要实测（缺字表现为方框）。
4. **mock 腿兼容**：原生算子在 mock 上响亮报错（沿用现风格 `build_op` 的 Err 日志）；**草图转特征在 mock 腿的降级**——`Extrude`/`Revolve`/`Sweep` 在 mock 的能力按现状（`Unsupported` 响亮报错），草图编辑本身（画布/封闭检查）纯 GUI 双腿一致。
5. **schema 持久化**：`register_schema` 是会话态（registry 不随文档容器保存）——GUI 自定义类型的持久化后补（P1），本期诚实提示"自定义类型重开丢失"。
6. **草图编辑器体量**：画布 + 图元 + 捕捉 + 转特征是本计划最大新增件——G4 内部再拆（先拉伸单特征跑通全链，再补旋转/放样），避免一次交付过大。

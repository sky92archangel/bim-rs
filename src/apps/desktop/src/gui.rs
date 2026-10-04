//! # 桌面端图形窗口（`feature = "gui"`，egui 0.29 + eframe/wgpu）
//!
//! 真实 GUI：**视口 / 拾取 / 属性面板 / 文件链路 / 图纸面板**。
//!
//! ```text
//! ┌────────────────────────────────────────────────────────────┐
//! │ 菜单栏: 文件 编辑 视图 帮助                                  │
//! ├────────────────────────────────────────────────────────────┤
//! │ Ribbon: [建模][草图][视图] 页签 + 分组面板（按钮堆叠+小标题）  │
//! ├────────────┬─────────────────────────────┬─────────────────┤
//! │ 元素│类型   │                             │ 属性│视图│图纸    │
//! │ (可停靠)    │        三维视口              │  (可停靠)        │
//! │             ├─────────────────────────────┤                 │
//! │             │ 日志                         │                 │
//! ├────────────┴─────────────────────────────┴─────────────────┤
//! │ 状态栏: 文件名 · 坐标 · 拾取模式 · 内核 · 统计               │
//! └────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## 可停靠面板（G6-M5：egui_dock 0.14）
//!
//! 面板区（元素/类型/属性/视图/图纸/日志/视口）由 [`egui_dock::DockState`] 管理：
//! 每个面板是 [`DockTab`]，用户可拖动重排、拖出为独立窗口、关闭；
//! 菜单栏「视图 · 重置布局」恢复默认五区布局。视口不可关闭（closeable=false）——
//! 它是 3D 主体；其余面板关闭后可从重置布局找回。
//!
//! ## Ribbon（G6-M8：Revit 风格页签化工具区）
//!
//! 单行工具栏升级为三页签 Ribbon：**建模**（构建七组 + 模块配方）/ **草图**
//! （放样路径先行四入口）/ **视图**（适配 / 清选 / 拾取三态 / 基线 / 日志）。
//! 页签行 + 内容行：分组面板按钮竖排堆叠 + 底部小标题 + 竖分隔线。
//! 数据/渲染分离：`RibbonTab` + `RIBBON_TABS` + `build_groups()`（构建分组
//! 清单）供测试断言；`ribbon*` 系列函数只做渲染。
//!
//! ## 构建链路（G3：原生算子门面 + 参数表单）
//!
//! Ribbon 建模页签七组（基本体 / 线框 / 扫掠放样 / 布尔 / 变换 / 修饰 / 修复，计划
//! §5.1）+ 模块配方分组：点算子 → **参数表单浮窗**（[`ParamKind`] 路由控件，
//! `core_sdk::BuildOp::param_defs` 单一事实源）→ 确定 → 按元数路由执行：
//!
//! | 元数 | 路由 | 语义 |
//! |---|---|---|
//! | 0 输入 | `create_generic_element` + `build_native` | 新构件 |
//! | 1 输入（非变长）| `modify_native` | 替换选中构件几何（关联更新）|
//! | 2 输入（非变长）| `combine_native` | 主选 + 副选（Ctrl+单击）→ 新构件收结果 |
//! | 变长 | —（占位）| Loft / MakeWire 走 G4 草图流程 |
//!
//! 参数换算：角度 GUI 用度（提交时 `to_radians()`）、长度 mm、选择器 JSON
//! 文本（必填，空串拒绝提交——空选择会让算子静默无效）。
//! 所有 GUI 动作可被纯 API 复现（session.rs 的 G3 API 复现用例）。
//!
//! ## 渲染路线（为什么不用 wgpu paint callback）
//!
//! 构件网格很小（占位内核盒体 12 三角；真实内核数百三角以内），**CPU 画家算法**
//! （按深度排序后交给 egui 的三角形 mesh）完全够用：零自定义 shader、零 wgpu
//! 生命周期代码，单文件可审。将来网格到万级三角再换 `egui-wgpu` 自定义回调。
//!
//! ## 拾取路线（复用既有链路，不另造轮子）
//!
//! 视口相机实现 [`core_doc::Camera`]（`ray_through`），拾取走
//! [`CoreSession::pick_at`]（网格粗筛 + 内核精算 + 元素反查，设计文档 §6.6
//! 同一条链路）——GUI 只贡献"屏幕像素坐标"。
//!
//! ## 后端
//!
//! 与 e2e 双腿同款：默认腿用占位内核（[`CoreSession::new`]，秒起）；
//! `--features occt` 时用真实内核（[`CoreSession::with_real_kernel`]，首帧
//! JIT 较慢）——两腿共用同一套 GUI 代码，顶栏显示当前内核名。
//!
//! ## 失败口径
//!
//! 与本仓铁律一致：**不 panic、不吞错**。装载 / 建几何 / 网格化 / 拾取的
//! 失败一律写进底栏日志（错误原文），窗口照常可操作。

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use eframe::egui;
use glam::DVec3;

use cable_tunnel::{
    CATEGORY as TUNNEL_CATEGORY, CableTunnel, GEOMETRY_OP_ID_DUCTBANK as OP_TUNNEL_DUCTBANK,
    GEOMETRY_OP_ID_RECT as OP_TUNNEL_RECT,
};
use core_app::CoreSession;
use core_doc::{
    Camera, PickFilter, PickKind, PickParam, PickResult, Ray, SheetFormat, ViewOrientation,
};
use core_interop::{Drawing2D, Format};
use core_model::{ElementId, FieldDef, FieldType, PropertyMap, ShapeId, SheetId, Value, ViewId};
use core_sdk::{BuildOp, Module, ParamDef, ParamDefault, ParamKind};
use example_beam::{
    CATEGORY as BEAM_CATEGORY, ExampleBeam, GEOMETRY_OP_ID as OP_BEAM_RECT,
    GEOMETRY_OP_ID_HOLE as OP_BEAM_HOLE,
};
use hvac_duct::{
    CATEGORY as DUCT_CATEGORY, GEOMETRY_OP_ID_RECT as OP_DUCT_RECT,
    GEOMETRY_OP_ID_TRANSITION as OP_DUCT_TRANSITION, HvacDuct,
};

use crate::sketch_editor::SketchEntry;
use crate::types_editor::{self, TypesEdit};

// ---------------------------------------------------------------------------
// 常量
// ---------------------------------------------------------------------------

/// 视口渲染网格化的线性挠度（mm）——远细于构件尺寸，三角量可忽略。
const VIEW_LINEAR: f64 = 0.5;
/// 视口渲染网格化的角度挠度（rad）。
const VIEW_ANGULAR: f64 = 0.1;
/// 最小视距（mm）。
const MIN_DISTANCE: f64 = 50.0;
/// 最大视距（mm）。
const MAX_DISTANCE: f64 = 500_000.0;
/// 世界"上"方向：内核网格的 Z 轴（`cylinder_mesh` 高度沿 Z，`ViewOrientation::top` 同向）。
const WORLD_UP: DVec3 = DVec3::Z;
/// 视口底色（深灰蓝，CAD 常规）。
const VIEWPORT_BG: egui::Color32 = egui::Color32::from_rgb(0x25, 0x27, 0x2c);
/// 选中构件的高亮混入色（橙）。
const SELECT_TINT: [u8; 3] = [0xff, 0x96, 0x28];
/// 拾取命中面的实色（橙红）。
const PICK_ACCENT: [u8; 3] = [0xff, 0x5a, 0x1e];

// ---------------------------------------------------------------------------
// 轨道相机
// ---------------------------------------------------------------------------

/// 轨道相机：绕目标点旋转，Z 朝上，单位 mm。
struct OrbitCamera {
    /// 注视点（世界坐标，mm）
    target: DVec3,
    /// 视距（mm）
    distance: f64,
    /// 水平角（rad，绕 Z）
    yaw: f64,
    /// 俯仰角（rad，0=水平，±π/2=过顶/过底；归一化到 [−π/2, π/2]）
    pitch: f64,
    /// 俯仰拖动方向（+1 / −1）：每越过一个极点翻转一次，
    /// 使"拖动方向＝物体转动方向"在过极点后依然成立。
    pitch_drag_sign: f64,
    /// 垂直视场角（rad）
    fov_y: f64,
    /// 视口几何（每帧由 GUI 更新；拾取射线换算用）：(中心 x, 中心 y, 高度 px)
    viewport: (f64, f64, f64),
}

impl OrbitCamera {
    fn new() -> Self {
        Self {
            target: DVec3::new(0.0, 0.0, 250.0),
            distance: 6000.0,
            yaw: -0.9,
            pitch: 0.45,
            pitch_drag_sign: 1.0,
            fov_y: 45.0_f64.to_radians(),
            viewport: (0.0, 0.0, 800.0),
        }
    }

    /// 归一化俯仰角到 [−π/2, π/2]（**解除俯仰限位**的关键）。
    ///
    /// 越过极点时做 `(pitch, yaw) → (π−pitch, yaw+π)`（北上）或
    /// `(−π−pitch, yaw+π)`（南下）：两个参数化描述的是**同一个眼点**——
    /// `eye` 公式可验证 `cp′cy′ = cpcy`、`sp′ = sp`，视图基只依赖眼点与目标，
    /// 因此**翻转前后画面完全一致、无跳变**，而拖动方向经
    /// [`Self::pitch_drag_sign`] 翻转后依然满足"拖动方向＝物体转动方向"。
    /// 于是可以无限次绕过正上/正下方继续旋转，不再需要限位。
    /// 阈值留 1e−4 rad（0.006°，不可感知）：避开恰在极点处 `f ∥ up`
    /// 导致视图基退化。
    fn normalize_pitch(&mut self) {
        const HALF_PI: f64 = std::f64::consts::FRAC_PI_2;
        const EPS: f64 = 1.0e-4;
        if self.pitch > HALF_PI - EPS {
            self.pitch = std::f64::consts::PI - self.pitch;
            self.yaw += std::f64::consts::PI;
            self.pitch_drag_sign = -self.pitch_drag_sign;
        } else if self.pitch < -HALF_PI + EPS {
            self.pitch = -std::f64::consts::PI - self.pitch;
            self.yaw += std::f64::consts::PI;
            self.pitch_drag_sign = -self.pitch_drag_sign;
        }
    }

    /// 相机眼点。
    fn eye(&self) -> DVec3 {
        let (sp, cp) = self.pitch.sin_cos();
        let (sy, cy) = self.yaw.sin_cos();
        self.target + self.distance * DVec3::new(cp * cy, cp * sy, sp)
    }

    /// 视图基：前向 f / 右向 r / 上向 u（单位向量）。
    fn basis(&self) -> (DVec3, DVec3, DVec3) {
        let f = (self.target - self.eye()).normalize();
        let r = f.cross(WORLD_UP).normalize_or_zero();
        let u = r.cross(f);
        (f, r, u)
    }

    /// 焦距（px）。
    fn focal(&self, viewport_h: f64) -> f64 {
        (viewport_h.max(1.0) / 2.0) / (self.fov_y / 2.0).tan()
    }

    /// 世界点 → 屏幕点 + 深度（近平面内返回 `None`，调用方跳过该三角形）。
    fn project(&self, p: DVec3, rect: &egui::Rect) -> Option<(egui::Pos2, f64)> {
        let (f, r, u) = self.basis();
        let rel = p - self.eye();
        let depth = rel.dot(f);
        if depth <= 1.0 {
            return None;
        }
        let focal = self.focal(rect.height() as f64);
        let cx = rect.center().x as f64;
        let cy = rect.center().y as f64;
        let x = cx + rel.dot(r) / depth * focal;
        let y = cy - rel.dot(u) / depth * focal; // egui 的 y 轴向下
        Some((egui::Pos2::new(x as f32, y as f32), depth))
    }
}

impl Camera for OrbitCamera {
    fn ray_through(&self, screen: (f32, f32)) -> Ray {
        let (f, r, u) = self.basis();
        let (cx, cy, vh) = self.viewport;
        let focal = self.focal(vh);
        let dir = f + r * ((screen.0 as f64 - cx) / focal) + u * ((cy - screen.1 as f64) / focal);
        Ray {
            origin: self.eye(),
            direction: dir.normalize_or_zero(),
        }
    }
}

// ---------------------------------------------------------------------------
// 渲染缓存与应用状态
// ---------------------------------------------------------------------------

/// 一个形状的渲染缓存（网格 + 基色）。
struct ShapeMesh {
    mesh: core_kernel::MeshData,
    base_color: [u8; 3],
}

/// 一条"新建构件 + 造几何"按钮的静态描述。
struct OpButton {
    category: &'static str,
    op_id: &'static str,
    label: &'static str,
}

/// 六条算子按钮：三个模块 × 各自可用算子（mock 腿不支持的算子点击后日志响亮报错）。
fn op_buttons() -> Vec<OpButton> {
    vec![
        OpButton {
            category: BEAM_CATEGORY,
            op_id: OP_BEAM_RECT,
            label: "梁·矩形截面",
        },
        OpButton {
            category: BEAM_CATEGORY,
            op_id: OP_BEAM_HOLE,
            label: "梁·圆孔板",
        },
        OpButton {
            category: DUCT_CATEGORY,
            op_id: OP_DUCT_RECT,
            label: "风管·矩形段",
        },
        OpButton {
            category: DUCT_CATEGORY,
            op_id: OP_DUCT_TRANSITION,
            label: "风管·变径管（Loft）",
        },
        OpButton {
            category: TUNNEL_CATEGORY,
            op_id: OP_TUNNEL_RECT,
            label: "隧道段·矩形",
        },
        OpButton {
            category: TUNNEL_CATEGORY,
            op_id: OP_TUNNEL_DUCTBANK,
            label: "排管段·线束（Pipe）",
        },
    ]
}

/// 一个"新建视图"的视向预设（M11 图纸面板）。
struct ViewPreset {
    label: &'static str,
    orientation: ViewOrientation,
}

/// 三个内置视向：俯视 / 前视用 [`ViewOrientation`] 内置构造；
/// 右视按 front() 同款模式推导（观察方向 -X、向右 +Y、向上 +Z，
/// `x_axis = view_dir × up = (−X)×Z = +Y`）。
fn view_presets() -> Vec<ViewPreset> {
    vec![
        ViewPreset {
            label: "俯视（平面）",
            orientation: ViewOrientation::top(),
        },
        ViewPreset {
            label: "前视（立面）",
            orientation: ViewOrientation::front(),
        },
        ViewPreset {
            label: "右视（立面）",
            orientation: ViewOrientation::new(DVec3::new(1.0, 0.0, 0.0), DVec3::NEG_X, DVec3::Y),
        },
    ]
}

/// 按品类取基色（构件分色，便于区分模块）。
fn base_color(category: &str) -> [u8; 3] {
    match category {
        BEAM_CATEGORY => [0x8a, 0x93, 0xa6],   // 混凝土灰蓝
        DUCT_CATEGORY => [0xbd, 0xc3, 0xcc],   // 镀锌钢板银灰
        TUNNEL_CATEGORY => [0xa6, 0xa0, 0x96], // 隧道混凝土灰
        _ => [0x99, 0x99, 0x99],
    }
}

/// 两个 RGB 颜色线性混合（t=0 取 a，t=1 取 b）。
fn mix_rgb(a: [u8; 3], b: [u8; 3], t: f32) -> egui::Color32 {
    let ch = |i: usize| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t) as u8;
    egui::Color32::from_rgb(ch(0), ch(1), ch(2))
}

/// 属性值 → 展示文本。
fn format_value(v: &Value) -> String {
    match v {
        Value::Bool(b) => format!("{b}"),
        Value::Int(i) => format!("{i}"),
        Value::Float(f) => format!("{f}"),
        Value::Text(s) => s.clone(),
        Value::Ref(r) => format!("→ {r}"),
    }
}

/// 可停靠面板页签（G6-M5：egui_dock 的 Tab）。
/// 面板内容与标题在 [`BimTabViewer`] 里路由；选中态由 `DockState` 管理。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DockTab {
    /// 视口（中央，3D 主体——不可关闭）
    Viewport,
    /// 元素（构件树）
    Elements,
    /// 类型管理器（G5 交付）
    Types,
    /// 属性
    Props,
    /// 视图列表（G6-M3 更名：本页实为视图列表——Sheet 是"图纸"）
    Views,
    /// 图纸（G6-M3 出图深化：Sheet 布置——图幅 / 标题栏 / 整图预览导出）
    Sheets,
    /// 日志
    Logs,
}

/// Ribbon 页签（G6-M8）：Revit 风格工具区的三大页——数据与渲染分离，
/// 页签清单 [`RIBBON_TABS`] 与分组数据 [`build_groups`] 供测试断言。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum RibbonTab {
    /// 建模：构建七组 + 模块配方
    #[default]
    Model,
    /// 草图：四入口（通用 / 拉伸 / 旋转扫掠 / 放样）
    Sketch,
    /// 视图：适配 / 清选 / 拾取三态 / 基线 / 日志
    View,
}

impl RibbonTab {
    /// 页签显示名。
    fn label(self) -> &'static str {
        match self {
            Self::Model => "建模",
            Self::Sketch => "草图",
            Self::View => "视图",
        }
    }
}

/// Ribbon 页签清单（G6-M8：从左到右）。
const RIBBON_TABS: [RibbonTab; 3] = [RibbonTab::Model, RibbonTab::Sketch, RibbonTab::View];

/// 建模页签的构建分组清单（G3 七组——照旧单行工具栏的分组原样迁移，
/// 数据与渲染分离：Ribbon 面板与测试共用一份）。
fn build_groups() -> Vec<(&'static str, &'static str, Vec<BuildOp>)> {
    use egui_phosphor::regular as ph;
    vec![
        (
            ph::CUBE,
            "基本体",
            vec![
                BuildOp::MakeBox,
                BuildOp::MakeCylinder,
                BuildOp::MakeSphere,
                BuildOp::MakeCone,
                BuildOp::MakeTorus,
            ],
        ),
        (
            ph::SHAPES,
            "线框",
            vec![BuildOp::MakeLineEdge, BuildOp::MakeArcEdge],
        ),
        (
            ph::PATH,
            "扫掠放样",
            vec![BuildOp::Extrude, BuildOp::Revolve],
        ),
        (
            ph::UNITE,
            "布尔",
            vec![BuildOp::Fuse, BuildOp::Cut, BuildOp::Common],
        ),
        (
            ph::ARROWS_OUT_LINE_HORIZONTAL,
            "变换",
            vec![BuildOp::Translate, BuildOp::Rotate],
        ),
        (
            ph::MAGIC_WAND,
            "修饰",
            vec![
                BuildOp::Fillet,
                BuildOp::Chamfer,
                BuildOp::Shell,
                BuildOp::Offset,
                BuildOp::Thicken,
            ],
        ),
        (
            ph::WRENCH,
            "修复",
            vec![
                BuildOp::Sew,
                BuildOp::SewAndSolidify,
                BuildOp::FixShape,
                BuildOp::UnifySameDomain,
            ],
        ),
    ]
}

/// 默认停靠布局（G6-M5）：照五区语义——
/// 左栏（元素/类型叠 tab）15% → 右栏（属性/视图/图纸叠 tab）22% → 底部日志 20%，
/// 剩余为中央视口。两处构造（[`BimApp::new`] / [`BimApp::open_document`]）与
/// 菜单栏「重置布局」共用。
///
/// split 语义（egui_dock 0.14 实现为准）：`split_left(parent, f, tabs)` 把
/// **tabs 放左侧占 f**、原内容（old）在右占 1−f；`split_right` / `split_below` 同理
/// （new 在方向侧占 1−f——注意 fraction 恒属于 old 节点）。
fn default_dock() -> egui_dock::DockState<DockTab> {
    let mut dock = egui_dock::DockState::new(vec![DockTab::Viewport]);
    let surface = dock.main_surface_mut();
    // ① 左栏：元素 + 类型叠 tab，占 15%（old=其余内容在右占 85%）
    let [rest, _left] = surface.split_left(
        egui_dock::NodeIndex::root(),
        0.15,
        vec![DockTab::Elements, DockTab::Types],
    );
    // ② 右栏：属性 / 视图 / 图纸叠 tab，占 22%（old=中央块在左占 78%）
    let [center, _right] = surface.split_right(
        rest,
        0.78,
        vec![DockTab::Props, DockTab::Views, DockTab::Sheets],
    );
    // ③ 底部：日志占 20%（old=视口块在上占 80%）
    let [_viewport, _logs] = surface.split_below(center, 0.80, vec![DockTab::Logs]);
    dock
}

/// 把 [`DockTab`] 的路由进 [`BimApp`] 的对应面板方法（egui_dock `TabViewer`）。
///
/// 借用说明：`update()` 里用 `mem::replace` 把 dock 暂移出去、
/// viewer 持 `&mut BimApp`，渲染完再还回——避免 self.dock 与 self 双重借用。
struct BimTabViewer<'a> {
    app: &'a mut BimApp,
}

impl egui_dock::TabViewer for BimTabViewer<'_> {
    type Tab = DockTab;

    fn title(&mut self, tab: &mut Self::Tab) -> egui::WidgetText {
        use egui_phosphor::regular as ph;
        let (icon, name) = match tab {
            DockTab::Viewport => (ph::CUBE, "视口"),
            DockTab::Elements => (ph::LIST_BULLETS, "元素"),
            DockTab::Types => (ph::GEAR, "类型"),
            DockTab::Props => (ph::GEAR, "属性"),
            DockTab::Views => (ph::FRAME_CORNERS, "视图"),
            DockTab::Sheets => (ph::BROWSER, "图纸"),
            DockTab::Logs => (ph::CARET_DOUBLE_DOWN, "日志"),
        };
        format!("{icon} {name}").into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Self::Tab) {
        match tab {
            DockTab::Viewport => self.app.viewport(ui),
            DockTab::Elements => self.app.elements_panel(ui),
            DockTab::Types => self.app.types_panel(ui),
            DockTab::Props => self.app.props_panel(ui),
            DockTab::Views => self.app.views_panel(ui),
            DockTab::Sheets => self.app.sheets_panel(ui),
            DockTab::Logs => self.app.logs_panel(ui),
        }
    }

    /// 视口不可关闭——它是 3D 主体，关了没地方画；其余面板可关（重置布局找回）。
    fn closeable(&mut self, tab: &mut Self::Tab) -> bool {
        !matches!(tab, DockTab::Viewport)
    }
}

/// GUI 应用状态。
/// 面板区由 [`egui_dock::DockState`] 管理可停靠页签（G6-M5），
/// 菜单栏 / 工具栏 / 状态栏仍是固定 TopBottomPanel。
struct BimApp {
    session: CoreSession,
    camera: OrbitCamera,
    /// 渲染缓存：ShapeId → 网格 + 基色（几何变化后重建）
    meshes: HashMap<ShapeId, ShapeMesh>,
    /// 当前选中构件
    selected: Option<ElementId>,
    /// 最近一次拾取结果
    picked: Option<PickResult>,
    /// 操作日志（底栏显示，新消息在底部）
    logs: Vec<String>,
    /// 几何脏标记：置位后下一帧重建全部网格
    dirty: bool,
    /// 待自动适配视图（首帧或新几何后）
    fit_pending: bool,
    /// 是否真内核（occt feature）：mock 占位内核无真面拓扑，拾取命中后精算必然失败。
    real_kernel: bool,
    /// 当前文档路径（M9 文件链路：None = 未保存过的新文档）
    file_path: Option<PathBuf>,
    /// 上次同步到窗口标题的文字（避免每帧重复 SetWindowText）
    last_title: Option<String>,
    /// 参数编辑暂存（选中构件时快照；"应用并重建"提交，M10）
    edit: Option<ParamEdit>,
    /// ElementId → 建几何用的算子 id（rebuild 需要同款算子；build_op / new 时记录，M10）
    op_by_element: HashMap<ElementId, String>,
    /// 图纸面板选中的视图（M11）
    selected_view: Option<ViewId>,
    /// 浮动图纸预览窗口开关（M11）
    preview_open: bool,
    /// 图纸页选中的图纸（G6-M3）
    selected_sheet: Option<SheetId>,
    /// 浮动整图预览窗口开关（G6-M3：Sheet 级——图幅/图框/标题栏/布置）
    sheet_preview_open: bool,
    /// 新建图纸输入暂存（G6-M3）
    sheet_new_name: String,
    /// 新建图纸图号暂存（G6-M3）
    sheet_new_number: String,
    /// 新建图纸图幅暂存（G6-M3）
    sheet_new_format: SheetFormat,
    /// 放置视图表单：视图暂存（G6-M3）
    place_view_id: Option<ViewId>,
    /// 放置视图表单：比例分母暂存（G6-M3）
    place_scale: u32,
    /// 放置视图表单：位置 X 暂存（mm，G6-M3）
    place_x_mm: f64,
    /// 放置视图表单：位置 Y 暂存（mm，G6-M3）
    place_y_mm: f64,
    /// 拾取模式（面 / 边 / 顶点；M12）
    pick_mode: PickMode,
    /// 增量重建集（M12）：None = 全量重建，Some = 只重建这些形状。
    dirty_shapes: Option<HashSet<ShapeId>>,
    /// 可停靠面板布局（G6-M5：egui_dock——面板选中态/关闭/重排由它管理；
    /// 默认五区布局见 [`default_dock`]）
    dock: egui_dock::DockState<DockTab>,
    /// 待确认的原生算子参数表单（G3：点工具栏算子项 → 表单浮窗 → 确认执行）
    op_form: Option<OpForm>,
    /// 副选构件（G3：布尔 / 扫掠类第二输入；元素列表 / 视口 Ctrl+单击设置）
    secondary: Option<ElementId>,
    /// 草图编辑器状态（G4-M15：Some = 草图浮窗开着；三入口共用一个编辑器）
    sketch: Option<crate::sketch_editor::SketchEditorState>,
    /// 类型编辑器状态（G5-M20：Some = Types 页编辑中——新建 / 编辑 PropertySchema）
    types_edit: Option<TypesEdit>,
    /// 元素树折叠的品类（G5-M21：默认全展开；集合内 = 折叠）
    collapsed_groups: BTreeSet<String>,
    /// Ribbon 当前页签（G6-M8：建模 / 草图 / 视图三页，默认建模）
    ribbon_tab: RibbonTab,
}

/// 原生算子参数表单状态（G3：确认时换算成 PropertyMap → build/modify/combine_native）。
struct OpForm {
    op: BuildOp,
    /// 编辑中的值（键 → 表单值；初始由 param_defs 的 default 预填）
    values: BTreeMap<&'static str, FormValue>,
}

/// 表单里的值（提交时按 ParamKind 换算成 Value：角度度 → 弧度）。
enum FormValue {
    /// 长度 mm / 角度度（Angle 提交时 `to_radians()`）
    Float(f64),
    Int(i64),
    Bool(bool),
    /// 选择器 JSON（`Value::Text`）
    Text(String),
}

/// 由参数定义取表单初始值（default 预填；无 default 落零值——必填选择器落
/// 空串，提交时校验拒绝，空选择会让算子静默无效）。
fn form_value_from_default(def: &ParamDef) -> FormValue {
    match (def.kind, &def.default) {
        (ParamKind::Bool, Some(ParamDefault::Bool(b))) => FormValue::Bool(*b),
        (ParamKind::Bool, _) => FormValue::Bool(false),
        (ParamKind::Int, Some(ParamDefault::Int(i))) => FormValue::Int(*i),
        (ParamKind::Int, _) => FormValue::Int(0),
        (ParamKind::Selectors, _) => FormValue::Text(String::new()),
        (ParamKind::Points, _) => FormValue::Text(String::new()),
        (ParamKind::Length, Some(ParamDefault::Float(f)))
        | (ParamKind::Angle, Some(ParamDefault::Float(f))) => FormValue::Float(*f),
        (_, _) => FormValue::Float(0.0),
    }
}

/// 一次参数编辑的暂存（选中构件时快照；"应用并重建"提交）。
///
/// G5-M21：快照带**模式定义**（`fields`）——渲染按 FieldDef 完整路由
/// （unit 后缀 / required 缺失标红 / Enum 下拉 / default 恢复 / doc tooltip）。
#[derive(Clone)]
struct ParamEdit {
    /// 编辑中的构件
    element: ElementId,
    /// 重建用的算子（= 该构件当初建几何的算子）
    op_id: String,
    /// 属性模式名（重建参数要带上）
    schema: String,
    /// 模式定义快照（schema 未注册时为空——字段按 BTreeMap 字典序渲染）
    fields: Vec<FieldDef>,
    /// 编辑中的参数快照（含未提交修改）
    values: BTreeMap<String, Value>,
}

/// 拾取模式（顶栏三态单选；M12 边 / 顶点拾取的 GUI 入口）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickMode {
    /// 拾取面（默认）
    Face,
    /// 拾取边（点附近的边，内核精算）
    Edge,
    /// 拾取顶点（点附近的顶点，内核精算）
    Vertex,
}

impl PickMode {
    /// 顶栏标签。
    const fn label(self) -> &'static str {
        match self {
            Self::Face => "拾取面",
            Self::Edge => "拾取边",
            Self::Vertex => "拾取顶点",
        }
    }

    /// 对应的拾取过滤器（单选：只请求该种类）。
    const fn filter(self) -> PickFilter {
        match self {
            Self::Face => PickFilter {
                faces: true,
                edges: false,
                vertices: false,
            },
            Self::Edge => PickFilter {
                faces: false,
                edges: true,
                vertices: false,
            },
            Self::Vertex => PickFilter {
                faces: false,
                edges: false,
                vertices: true,
            },
        }
    }
}

/// 装载全部三个专业模块（new / open 共用；失败逐条写日志，不 panic）。
fn install_modules(session: &mut CoreSession, logs: &mut Vec<String>) {
    let modules: [(&'static str, Box<dyn Module>); 3] = [
        ("example.beam", Box::new(ExampleBeam)),
        ("hvac.duct", Box::new(HvacDuct)),
        ("power.tunnel", Box::new(CableTunnel)),
    ];
    for (name, module) in modules {
        if let Err(e) = session.install_module(module) {
            logs.push(format!("✗ 装载模块 {name} 失败：{e}"));
        }
    }
    logs.push(format!(
        "✓ 已装载 {} 个模块（内核：{}）",
        session.modules().module_count(),
        session.geometry().backend_name()
    ));
}

impl BimApp {
    fn new() -> Result<Self, String> {
        // 与 e2e 双腿同款：默认占位内核，occt feature 时真内核（绝不静默降级）。
        #[cfg(feature = "occt")]
        let picked_session = CoreSession::with_real_kernel();
        #[cfg(not(feature = "occt"))]
        let picked_session = CoreSession::new();
        let mut session = picked_session.map_err(|e| format!("会话装配失败：{e}"))?;
        #[cfg(feature = "occt")]
        let real_kernel = true;
        #[cfg(not(feature = "occt"))]
        let real_kernel = false;

        let mut logs: Vec<String> = Vec::new();
        install_modules(&mut session, &mut logs);
        let mut op_by_element: HashMap<ElementId, String> = HashMap::new();

        // 起手造一根梁，让视口不空。
        match session.create_element(BEAM_CATEGORY, "") {
            Ok(id) => match session.build_geometry(id, OP_BEAM_RECT) {
                Ok(shape) => {
                    op_by_element.insert(id, OP_BEAM_RECT.to_string());
                    logs.push(format!("✓ 初始构件：梁已建几何（ShapeId {shape}）"));
                }
                Err(e) => logs.push(format!("✗ 初始梁建几何失败：{e}")),
            },
            Err(e) => logs.push(format!("✗ 初始构件创建失败：{e}")),
        }

        Ok(Self {
            session,
            camera: OrbitCamera::new(),
            meshes: HashMap::new(),
            selected: None,
            picked: None,
            logs,
            dirty: true,
            fit_pending: true,
            real_kernel,
            file_path: None,
            last_title: None,
            edit: None,
            op_by_element,
            selected_view: None,
            preview_open: false,
            selected_sheet: None,
            sheet_preview_open: false,
            sheet_new_name: String::new(),
            sheet_new_number: String::new(),
            sheet_new_format: SheetFormat::A2,
            place_view_id: None,
            place_scale: 100,
            place_x_mm: 100.0,
            place_y_mm: 100.0,
            pick_mode: PickMode::Face,
            dirty_shapes: None,
            dock: default_dock(),
            op_form: None,
            secondary: None,
            sketch: None,
            types_edit: None,
            collapsed_groups: BTreeSet::new(),
            ribbon_tab: RibbonTab::Model,
        })
    }

    /// 打开一个 `.bimdoc` 并包成 GUI 应用（模块重装；打开后渲染缓存/选中态归零）。
    ///
    /// feature 路由与 [`Self::new`] 一致：occt 腿走 [`CoreSession::open_real`]
    /// （真实内核，重开后 rebuild / 投影走真几何），否则 [`CoreSession::open`]
    /// （占位内核，重开后拾取不可用——mock 能力边界，与 M8 口径一致）。
    fn open_document(path: &Path) -> Result<Self, String> {
        #[cfg(feature = "occt")]
        let picked_session = CoreSession::open_real(path);
        #[cfg(not(feature = "occt"))]
        let picked_session = CoreSession::open(path);
        let mut session = picked_session.map_err(|e| format!("打开失败：{e}"))?;
        #[cfg(feature = "occt")]
        let real_kernel = true;
        #[cfg(not(feature = "occt"))]
        let real_kernel = false;

        let mut logs: Vec<String> = Vec::new();
        install_modules(&mut session, &mut logs);
        logs.push(format!(
            "✓ 已打开：{}（{} 个构件 / {} 条几何）",
            path.display(),
            session.summary().elements,
            session.summary().geometry
        ));

        Ok(Self {
            session,
            camera: OrbitCamera::new(),
            meshes: HashMap::new(),
            selected: None,
            picked: None,
            logs,
            dirty: true,
            fit_pending: true,
            real_kernel,
            file_path: Some(path.to_path_buf()),
            last_title: None,
            edit: None,
            // 打开的文档无算子记录（build_op 只在本次会话记录）——
            // 这些构件参数只读、不可重建，GUI 里如实提示。
            op_by_element: HashMap::new(),
            selected_view: None,
            preview_open: false,
            selected_sheet: None,
            sheet_preview_open: false,
            sheet_new_name: String::new(),
            sheet_new_number: String::new(),
            sheet_new_format: SheetFormat::A2,
            place_view_id: None,
            place_scale: 100,
            place_x_mm: 100.0,
            place_y_mm: 100.0,
            pick_mode: PickMode::Face,
            dirty_shapes: None,
            dock: default_dock(),
            op_form: None,
            secondary: None,
            sketch: None,
            types_edit: None,
            collapsed_groups: BTreeSet::new(),
            ribbon_tab: RibbonTab::Model,
        })
    }

    fn log(&mut self, msg: impl Into<String>) {
        self.logs.push(msg.into());
        if self.logs.len() > 200 {
            let overflow = self.logs.len() - 200;
            self.logs.drain(0..overflow);
        }
    }

    // ---- 文件链路（M9：新建 / 打开 / 保存 / 另存） ----

    /// 新建文档：全新会话 + 重装模块（未确认丢弃提示由日志记录，不做模态弹窗）。
    fn do_new(&mut self) {
        match Self::new() {
            Ok(mut app) => {
                app.logs
                    .insert(0, "✓ 已新建文档（未保存更改已丢弃）".to_string());
                *self = app;
            }
            Err(e) => self.log(format!("✗ 新建失败：{e}")),
        }
    }

    /// 打开文档：rfd 原生对话框选路径 → [`Self::open_document`] 整体替换。
    fn do_open(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("BIM 文档", &["bimdoc"])
            .pick_file()
        else {
            self.log("· 打开已取消".to_string());
            return;
        };
        match Self::open_document(&path) {
            Ok(app) => *self = app,
            Err(e) => self.log(e),
        }
    }

    /// 保存（Ctrl+S）：已有路径直接存；从未保存过 → 转另存。
    fn do_save(&mut self) {
        let Some(path) = self.file_path.clone() else {
            self.do_save_as();
            return;
        };
        self.save_to(&path);
    }

    /// 另存为（rfd 原生对话框 → 保存 → 记住路径）。
    fn do_save_as(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("BIM 文档", &["bimdoc"])
            .set_file_name("未命名.bimdoc")
            .save_file()
        else {
            self.log("· 另存已取消".to_string());
            return;
        };
        self.save_to(&path);
    }

    /// 存盘到指定路径（成功记住路径，失败写日志不 panic）。
    fn save_to(&mut self, path: &Path) {
        match self.session.save(path) {
            Ok(()) => {
                self.file_path = Some(path.to_path_buf());
                self.log(format!("✓ 已保存：{}", path.display()));
            }
            Err(e) => self.log(format!("✗ 保存失败：{e}")),
        }
    }

    /// 撤销一步（G6-M1）：文档与图纸一起回滚，网格全量重建 + 失效态清理。
    ///
    /// - `undo()` 无可撤销时返回 false（菜单项已 `add_enabled` 置灰，快捷键
    ///   路径仍可能到达——静默忽略）；
    /// - 回滚后网格缓存**全量重建**（`dirty = true` 且不设增量集——
    ///   refresh_meshes 的 clear 分支自动清掉已消失几何的孤儿缓存）；
    /// - 选中 / 副选 / 拾取 / 参数编辑 / 图纸视图选中里已不存在的 id 清掉
    ///   （回滚后元素与视图可能消失）；
    /// - **op_by_element 不清理**：残留条目对重做友好（重做后元素回来，重选
    ///   即可按原算子编辑）；ElementId 只增不复用，残留无语义风险；
    /// - 不自动适配视图（撤销是精细操作，自动 fit 会让视口跳变）。
    fn do_undo(&mut self) {
        if !self.session.undo() {
            return;
        }
        self.after_history_jump("撤销");
    }

    /// 重做一步（G6-M1）：语义同 [`Self::do_undo`]，方向相反。
    fn do_redo(&mut self) {
        if !self.session.redo() {
            return;
        }
        self.after_history_jump("重做");
    }

    /// 撤销 / 重做共用的状态清理（历史移动后文档与图纸都变了）。
    fn after_history_jump(&mut self, action: &str) {
        self.dirty = true;
        // —— 清理决策（文档 / 图纸只读借用圈在块内；先决策后改状态）——
        let (selected_gone, secondary_gone, edit_gone, picked_gone, view_gone) = {
            let svc = self.session.document();
            let doc = svc.doc();
            let drawing = svc.drawing();
            (
                self.selected.is_some_and(|id| doc.element(id).is_none()),
                self.secondary.is_some_and(|id| doc.element(id).is_none()),
                self.edit
                    .as_ref()
                    .is_some_and(|e| doc.element(e.element).is_none()),
                self.picked
                    .as_ref()
                    .is_some_and(|p| doc.element(p.element).is_none()),
                self.selected_view
                    .is_some_and(|v| drawing.view(v).is_none()),
            )
        };
        if selected_gone {
            self.selected = None;
        }
        if secondary_gone {
            self.secondary = None;
        }
        if edit_gone {
            self.edit = None;
        }
        if picked_gone {
            self.picked = None;
        }
        if view_gone {
            self.selected_view = None;
        }
        self.log(format!(
            "✓ 已{action}（可再撤销 {} 步 / 可重做 {} 步）",
            self.session.undo_depth(),
            self.session.redo_depth()
        ));
    }

    /// 把当前文档状态同步到窗口标题栏（有路径显示路径，否则"未命名"）。
    fn sync_title(&mut self, ctx: &egui::Context) {
        let title = match &self.file_path {
            Some(p) => format!("轻量化 BIM 内核平台 —— {}", p.display()),
            None => "轻量化 BIM 内核平台 —— 未命名".to_string(),
        };
        if self.last_title.as_deref() != Some(title.as_str()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = Some(title);
        }
    }

    // ---- 关联更新（M10：属性可编辑 → rebuild_geometry → stale → regenerate） ----

    /// 提交参数编辑（G5-M21 语义定版）：**validate → set → rebuild**。
    ///
    /// ① validate：schema 已注册时先校验新值（非法值红字拦截——不落库不重建；
    ///    未注册模式照 M10 现状跳过，参数自由）；
    /// ② set：新值**回写元素 properties**（属性与几何一致——M10 现状不回写，
    ///    重选构件后编辑快照变回旧值，语义混乱）；
    /// ③ rebuild：走 `rebuild_geometry`（M6 关联更新首链在 GUI 生效）——
    ///    网格重建（dirty）+ 定向重投 + stale 视图标脏；警告逐条入日志，不被吞。
    fn apply_rebuild(&mut self) {
        let Some(edit) = self.edit.clone() else {
            return;
        };
        let mut params = PropertyMap::new(edit.schema.clone());
        for (k, v) in &edit.values {
            params.set(k.clone(), v.clone());
        }
        // ① validate（schema 已注册时；未注册模式跳过——参数自由，照 M10 现状）
        let registered = self
            .session
            .list_schemas()
            .iter()
            .any(|s| s.type_name == edit.schema);
        if registered && let Err(e) = self.session.validate_map(&edit.schema, &params) {
            self.log(format!("✗ 非法值已拦截：{e}（修正后再点\"应用并重建\"）"));
            return;
        }
        // ② set：新值回写元素 properties（属性与几何一致）
        if let Err(e) = self
            .session
            .update_element_properties(edit.element, params.clone())
        {
            self.log(format!("✗ 属性回写失败：{e}"));
            return;
        }
        // ③ rebuild（M6 关联更新首链在 GUI 生效）
        match self
            .session
            .rebuild_geometry(edit.element, &edit.op_id, params)
        {
            Ok(report) => {
                self.dirty = true;
                // M12 增量重建：只网格化本次被改的形状。
                if !report.changed_shapes.is_empty() {
                    self.dirty_shapes = Some(report.changed_shapes.iter().copied().collect());
                }
                self.log(format!(
                    "✓ 关联更新：演化直连 {} + 指纹回退 {}，重投 {} 图元、重算 {} 标注、{} 视图已标脏",
                    report.evolution_resolved,
                    report.fingerprint_resolved,
                    report.reprojected_entities,
                    report.remeasured_annotations,
                    report.stale_views
                ));
                for w in &report.warnings {
                    self.log(format!("⚠ {w}"));
                }
                // 重建后重新快照（内核可能归一参数；选中不变）。
                self.edit = None;
            }
            Err(e) => self.log(format!("✗ 关联更新失败：{e}")),
        }
    }

    // ---- 性能基线（M12：当前会话实测，日志可见；不写死阈值——不同机器不同） ----

    /// 跑一轮性能基线：全量 tessellate 计时 + 视口中心附近 5 次拾取计时，写日志。
    ///
    /// **只测量、不断言**：性能边界随机器 / 内核腿（mock / occt）变化，
    /// 硬阈值必然假红；实测数字进日志 + README 记录，回归靠人眼比对量级。
    fn run_perf_baseline(&mut self) {
        let shapes: Vec<ShapeId> = self
            .session
            .document()
            .doc()
            .elements()
            .values()
            .filter_map(|e| e.geometry_ref)
            .collect();
        if shapes.is_empty() {
            self.log("· 无几何，性能基线跳过（先建几个构件）");
            return;
        }
        // ① 全量网格化（渲染层挠度）。
        let t0 = std::time::Instant::now();
        let mut ok = 0usize;
        for s in &shapes {
            if self
                .session
                .tessellate(*s, VIEW_LINEAR, VIEW_ANGULAR)
                .is_ok()
            {
                ok += 1;
            }
        }
        let tess_ms = t0.elapsed().as_millis();
        // ② 拾取 ×5（视口中心 ± 横向偏移，覆盖命中与未命中两类路径；
        //    M12 缓存后第 2 次起走 pick_meshes，量的是"缓存热"的拾取代价）。
        let (cx, cy, _) = self.camera.viewport;
        let t1 = std::time::Instant::now();
        let mut hits = 0usize;
        for i in 0..5i32 {
            let off = (i - 2) * 30;
            let screen = ((cx + off as f64) as f32, cy as f32);
            if let Ok(Some(_)) =
                self.session
                    .pick_at(screen, &self.camera, PickFilter::faces_only())
            {
                hits += 1;
            }
        }
        let pick_ms = t1.elapsed().as_millis();
        self.log(format!(
            "⏱ 性能基线：tessellate {} 形状 {tess_ms} ms（成功 {ok}）；拾取 ×5 {pick_ms} ms（命中 {hits}）",
            shapes.len()
        ));
    }

    /// 脏标记置位时重建网格（M12：有增量集只重建集合内形状，否则全量）。
    fn refresh_meshes(&mut self) {
        if !self.dirty {
            return;
        }
        match self.dirty_shapes.take() {
            // 全量重建（首帧 / 打开文档 / 增量集未登记时）。
            None => {
                self.meshes.clear();
                let entries: Vec<(ShapeId, [u8; 3])> = self
                    .session
                    .document()
                    .doc()
                    .elements()
                    .values()
                    .filter_map(|e| e.geometry_ref.map(|s| (s, base_color(e.category.as_str()))))
                    .collect();
                for (shape, base) in entries {
                    self.tessellate_into_cache(shape, base);
                }
            }
            // 增量重建：只碰登记过的形状（其余缓存一动不动）。
            Some(shapes) => {
                for shape in &shapes {
                    let Some(elem) = self
                        .session
                        .document()
                        .doc()
                        .elements()
                        .values()
                        .find(|e| e.geometry_ref == Some(*shape))
                        .cloned()
                    else {
                        // 形状已无元素引用（被删）→ 顺带清渲染缓存。
                        self.meshes.remove(shape);
                        continue;
                    };
                    self.tessellate_into_cache(*shape, base_color(elem.category.as_str()));
                }
            }
        }
        self.dirty = false;
        if self.fit_pending {
            self.fit_view();
            self.fit_pending = false;
        }
    }

    /// 网格化一个形状并写进渲染缓存（失败写日志，不 panic）。
    fn tessellate_into_cache(&mut self, shape: ShapeId, base: [u8; 3]) {
        match self.session.tessellate(shape, VIEW_LINEAR, VIEW_ANGULAR) {
            Ok(mesh) => {
                self.meshes.insert(
                    shape,
                    ShapeMesh {
                        mesh,
                        base_color: base,
                    },
                );
            }
            Err(e) => self.log(format!("✗ 网格化 {shape} 失败：{e}")),
        }
    }

    /// 适配视图：注视点取全部网格包围盒中心，视距取对角线的 1.8 倍。
    fn fit_view(&mut self) {
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        let mut any = false;
        for sm in self.meshes.values() {
            for p in &sm.mesh.positions {
                any = true;
                for i in 0..3 {
                    min[i] = min[i].min(p[i]);
                    max[i] = max[i].max(p[i]);
                }
            }
        }
        if !any {
            return;
        }
        self.camera.target = DVec3::new(
            (min[0] + max[0]) / 2.0,
            (min[1] + max[1]) / 2.0,
            (min[2] + max[2]) / 2.0,
        );
        let diag = DVec3::new(max[0] - min[0], max[1] - min[1], max[2] - min[2]).length();
        self.camera.distance = (diag * 1.8).max(500.0);
    }

    /// 新建构件 + 造几何（失败写日志，不 panic）。
    fn build_op(&mut self, category: &str, op_id: &str, label: &str) {
        match self.session.create_element(category, "") {
            Ok(elem) => match self.session.build_geometry(elem, op_id) {
                Ok(shape) => {
                    self.selected = Some(elem);
                    self.picked = None;
                    self.dirty = true;
                    // M12 增量重建：只网格化新建的形状。
                    self.dirty_shapes = Some([shape].into_iter().collect());
                    self.fit_pending = true;
                    // 记录算子（M10：rebuild 需要同款算子）。
                    self.op_by_element.insert(elem, op_id.to_string());
                    self.log(format!("✓ {label}：{op_id} 已建几何（ShapeId {shape}）"));
                }
                Err(e) => self.log(format!("✗ {label}：{op_id} 建几何失败：{e}")),
            },
            Err(e) => self.log(format!("✗ 创建 {category} 构件失败：{e}")),
        }
    }

    /// 视口拾取（单击）：屏幕坐标 → 图元（面 / 边 / 顶点按当前拾取模式）。
    ///
    /// Ctrl+单击 = 设**副选**（G3：布尔 / 扫掠类第二输入）；普通单击 = 主选。
    fn do_pick(&mut self, screen: (f32, f32), ctrl: bool) {
        let camera = &self.camera;
        let filter = self.pick_mode.filter();
        match self.session.pick_at(screen, camera, filter) {
            Ok(Some(hit)) => {
                if ctrl {
                    self.secondary = Some(hit.element);
                } else {
                    self.selected = Some(hit.element);
                }
                let p = hit.world_point;
                let kind = match hit.kind {
                    PickKind::Face => "面",
                    PickKind::Edge => "边",
                    PickKind::Vertex => "顶点",
                };
                self.log(format!(
                    "✓ 拾取{kind}：命中点 ({:.0}, {:.0}, {:.0})",
                    p.x, p.y, p.z
                ));
                self.picked = Some(hit);
            }
            Ok(None) => {
                self.picked = None;
                self.log("· 未命中任何构件面".to_string());
            }
            Err(e) => {
                self.picked = None;
                if self.real_kernel {
                    self.log(format!("✗ 拾取失败：{e}"));
                } else {
                    self.log(format!(
                        "✗ 拾取失败：{e}（当前是 mock 占位内核，无真面拓扑、命中后精算必然失败；请用真内核运行：cargo run -p bim-desktop --features gui,occt）"
                    ));
                }
            }
        }
    }

    // ---- 面板 ----

    /// 菜单栏（G2 五区布局：文件 / 编辑 / 视图 / 帮助；文件动作与 M9 门面一致）。
    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::menu::bar(ui, |ui| {
            ui.menu_button("文件", |ui| {
                if ui.button("新建").clicked() {
                    self.do_new();
                    ui.close_menu();
                }
                if ui.button("打开").clicked() {
                    self.do_open();
                    ui.close_menu();
                }
                if ui.button("保存").clicked() {
                    self.do_save();
                    ui.close_menu();
                }
                if ui.button("另存为").clicked() {
                    self.do_save_as();
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("退出").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("编辑", |ui| {
                // 撤销 / 重做（G6-M1：双侧快照回滚——文档与图纸一起；深度入文案，
                // 不可用时置灰）。语义见 do_undo / do_redo。
                let undo_label = if self.session.can_undo() {
                    format!("撤销（{} 步）", self.session.undo_depth())
                } else {
                    "撤销".to_string()
                };
                if ui
                    .add_enabled(self.session.can_undo(), egui::Button::new(undo_label))
                    .clicked()
                {
                    self.do_undo();
                    ui.close_menu();
                }
                let redo_label = if self.session.can_redo() {
                    format!("重做（{} 步）", self.session.redo_depth())
                } else {
                    "重做".to_string()
                };
                if ui
                    .add_enabled(self.session.can_redo(), egui::Button::new(redo_label))
                    .clicked()
                {
                    self.do_redo();
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("清空选择").clicked() {
                    self.selected = None;
                    self.picked = None;
                    self.secondary = None;
                    ui.close_menu();
                }
            });
            ui.menu_button("视图", |ui| {
                if ui.button("适配视图").clicked() {
                    self.fit_view();
                    ui.close_menu();
                }
                // G6-M5：日志面板是 dock tab——点这里聚焦它；被关了就加回当前 leaf。
                if ui.button("聚焦日志").clicked() {
                    self.focus_or_restore_tab(DockTab::Logs);
                    ui.close_menu();
                }
                // G6-M5：面板拖动/关闭后一键恢复默认五区布局。
                if ui.button("重置布局").clicked() {
                    self.dock = default_dock();
                    self.log("✓ 已恢复默认面板布局".to_string());
                    ui.close_menu();
                }
            });
            ui.menu_button("帮助", |ui| {
                if ui.button("关于").clicked() {
                    self.log(format!(
                        "轻量化 BIM 内核平台 v{}（内核：{}）",
                        env!("CARGO_PKG_VERSION"),
                        self.session.geometry().backend_name()
                    ));
                    ui.close_menu();
                }
            });
        });
    }

    /// Ribbon（G6-M8）：Revit 风格页签化工具区——页签行（建模/草图/视图）+
    /// 内容行（分组面板：按钮堆叠 + 底部小标题 + 竖分隔线）。替换 G2 单行 toolbar。
    fn ribbon(&mut self, ui: &mut egui::Ui) {
        // 页签行
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            for tab in RIBBON_TABS {
                if ui
                    .selectable_label(self.ribbon_tab == tab, tab.label())
                    .clicked()
                {
                    self.ribbon_tab = tab;
                }
            }
        });
        ui.separator();
        // 内容行（分组面板）
        ui.horizontal(|ui| match self.ribbon_tab {
            RibbonTab::Model => {
                for (icon, name, ops) in build_groups() {
                    self.ribbon_build_group(ui, icon, name, &ops);
                }
                self.ribbon_recipes(ui);
            }
            RibbonTab::Sketch => self.ribbon_sketch_entries(ui),
            RibbonTab::View => self.ribbon_view_group(ui),
        });
    }

    /// 一个 Ribbon 构建分组面板：算子按钮竖排堆叠（元数启用——0 输入恒可用；
    /// 1 输入需选中构件；2 输入需主选 + 副选；变长禁用说明，绝不静默）+
    /// 底部小标题；点击开参数表单浮窗。照旧工具栏时期的元数口径，
    /// 菜单下拉改为面板直铺。
    fn ribbon_build_group(&mut self, ui: &mut egui::Ui, icon: &str, name: &str, ops: &[BuildOp]) {
        ui.vertical(|ui| {
            ui.set_min_width(92.0);
            for &op in ops {
                let (ready, tip) = if op.is_variadic() {
                    (false, "变长输入：草图流程接入后可用")
                } else {
                    match op.arity() {
                        0 => (true, ""),
                        1 => (
                            self.selected.is_some(),
                            "先选中一个构件（视口单击或元素列表）",
                        ),
                        2 => (
                            self.selected.is_some() && self.secondary.is_some(),
                            "先选主选构件，再 Ctrl+单击 副选构件",
                        ),
                        _ => (false, "不支持的元数"),
                    }
                };
                if ui
                    .add_enabled(ready, egui::Button::new(format!("{icon} {}", op.label())))
                    .on_disabled_hover_text(tip)
                    .clicked()
                {
                    self.open_op_form(op);
                }
            }
            ui.vertical_centered(|ui| ui.weak(name));
        });
        ui.separator();
    }

    /// 模块配方面板（计划 §5.1：六条配方按钮——mock 腿不支持的算子点击后
    /// 日志响亮报错）。
    fn ribbon_recipes(&mut self, ui: &mut egui::Ui) {
        use egui_phosphor::regular as ph;
        ui.vertical(|ui| {
            ui.set_min_width(128.0);
            for btn in op_buttons() {
                if ui
                    .button(format!("{} {}", ph::PACKAGE, btn.label))
                    .clicked()
                {
                    self.build_op(btn.category, btn.op_id, btn.label);
                }
            }
            ui.vertical_centered(|ui| ui.weak("模块配方"));
        });
    }

    /// 草图四入口面板（G4-M15 共用编辑器；G4-M18 特征链路——入口决定「完成」
    /// 落盘语义；放样路径先行：先选中路径构件再打开放样草图）。
    fn ribbon_sketch_entries(&mut self, ui: &mut egui::Ui) {
        use egui_phosphor::regular as ph;
        ui.vertical(|ui| {
            for (name, icon, entry) in [
                ("通用草图", ph::VECTOR_THREE, SketchEntry::Profile),
                ("拉伸草图", ph::FRAME_CORNERS, SketchEntry::Extrude),
                ("旋转扫掠草图", ph::PATH, SketchEntry::Revolve),
                ("放样草图", ph::SHAPES, SketchEntry::Sweep),
            ] {
                let tip = match entry {
                    SketchEntry::Extrude => {
                        "封闭轮廓沿草图平面法向拉伸成体（默认 500mm；未封闭警告并留在草图）"
                    }
                    SketchEntry::Revolve => {
                        "封闭轮廓绕轴旋转成体（默认 0°→360°，可区间；轴须与草图共面）"
                    }
                    SketchEntry::Sweep => {
                        "封闭截面沿路径扫掠成体（先选中路径构件，再打开放样草图）"
                    }
                    SketchEntry::Profile => "打开草图编辑器（通用 wire 元素）",
                };
                if ui
                    .button(format!("{icon} {name}"))
                    .on_hover_text(tip)
                    .clicked()
                {
                    // 窗口已开时不重置（避免误点丢草稿）。
                    if self.sketch.is_none() {
                        // 放样路径先行（§5.6 ①指定路径 → ②草图）：须先选中
                        // 路径构件（样条 / 线状）。
                        let sweep_path = if entry == SketchEntry::Sweep {
                            match self.selected {
                                Some(id) => {
                                    let path_name = self
                                        .session
                                        .document()
                                        .doc()
                                        .element(id)
                                        .map(|e| e.name.clone())
                                        .unwrap_or_else(|| "路径".to_string());
                                    Some((id, path_name))
                                }
                                None => {
                                    self.log("✗ 放样：先选中路径构件（样条 / 线状）再打开放样草图");
                                    continue;
                                }
                            }
                        } else {
                            None
                        };
                        self.sketch =
                            Some(crate::sketch_editor::SketchEditorState::new_with_entry(
                                name, entry, sweep_path,
                            ));
                    }
                }
            }
            ui.vertical_centered(|ui| ui.weak("草图入口"));
        });
    }

    /// 视图页签：适配 / 清选 / 拾取三态 / 性能基线 / 日志聚焦（横向排布）。
    fn ribbon_view_group(&mut self, ui: &mut egui::Ui) {
        use egui_phosphor::regular as ph;
        if ui
            .button(format!("{} 适配", ph::ARROWS_OUT))
            .on_hover_text("适配视图")
            .clicked()
        {
            self.fit_view();
        }
        if ui
            .button(format!("{} 清选", ph::BROOM))
            .on_hover_text("清空选择（含副选）")
            .clicked()
        {
            self.selected = None;
            self.picked = None;
            self.secondary = None;
        }
        ui.separator();
        ui.label("拾取：");
        for mode in [PickMode::Face, PickMode::Edge, PickMode::Vertex] {
            if ui
                .selectable_label(self.pick_mode == mode, mode.label())
                .clicked()
            {
                self.pick_mode = mode;
            }
        }
        ui.separator();
        if ui
            .button(format!("{} 基线", ph::TIMER))
            .on_hover_text("性能基线计时")
            .clicked()
        {
            self.run_perf_baseline();
        }
        if ui
            .button(format!("{} 日志", ph::CARET_DOUBLE_DOWN))
            .on_hover_text("聚焦日志面板（关闭后点此恢复）")
            .clicked()
        {
            self.focus_or_restore_tab(DockTab::Logs);
        }
    }

    /// 打开参数表单浮窗：按 [`BuildOp::param_defs`] 的 default 预填（G3 单一
    /// 事实源——表单不另立参数表）。
    fn open_op_form(&mut self, op: BuildOp) {
        let mut values = BTreeMap::new();
        for def in op.param_defs() {
            values.insert(def.key, form_value_from_default(&def));
        }
        self.op_form = Some(OpForm { op, values });
    }

    /// 参数表单浮窗（G3）：[`ParamKind`] 路由控件 + 确定提交。
    ///
    /// 控件路由：Length → DragValue + "mm"；Angle → DragValue + "°"（GUI 用度，
    /// 提交时 `to_radians()`）；Int → DragValue；Bool → checkbox；Selectors →
    /// 单行文本（JSON，必填，空串提交被拒）。
    fn op_form_window(&mut self, ctx: &egui::Context) {
        if self.op_form.is_none() {
            return;
        }
        // 标题与参数定义先取出（窗口闭包内只借 self.op_form 的值槽）。
        let (title, defs) = {
            let form = self.op_form.as_ref().expect("checked above");
            (format!("参数：{}", form.op.label()), form.op.param_defs())
        };
        let mut open = true;
        let mut confirmed = false;
        egui::Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .default_width(340.0)
            .show(ctx, |ui| {
                let form = self.op_form.as_mut().expect("checked above");
                egui::Grid::new("op_form_grid")
                    .striped(true)
                    .show(ui, |ui| {
                        for def in &defs {
                            ui.label(def.label);
                            let v = form
                                .values
                                .get_mut(def.key)
                                .expect("values keyed by param_defs");
                            match v {
                                FormValue::Float(f) => match def.kind {
                                    ParamKind::Angle => {
                                        // 角度 GUI 用度，步幅 10° 便于转整角度。
                                        ui.add(egui::DragValue::new(f).speed(10.0).suffix("°"));
                                    }
                                    ParamKind::Length => {
                                        ui.add(egui::DragValue::new(f).speed(1.0).suffix(" mm"));
                                    }
                                    _ => {
                                        ui.add(egui::DragValue::new(f).speed(0.1));
                                    }
                                },
                                FormValue::Int(i) => {
                                    ui.add(egui::DragValue::new(i).speed(1.0));
                                }
                                FormValue::Bool(b) => {
                                    ui.checkbox(b, "");
                                }
                                FormValue::Text(s) => {
                                    ui.add(
                                        egui::TextEdit::singleline(s)
                                            .hint_text("JSON（必填）")
                                            .desired_width(180.0),
                                    );
                                }
                            }
                            ui.end_row();
                        }
                    });
                if ui.button("确定").clicked() {
                    confirmed = true;
                }
            });
        if !open {
            self.op_form = None;
        }
        if confirmed {
            self.confirm_op_form();
        }
    }

    /// 提交参数表单（G3）：必填校验 → 表单值换算成 [`PropertyMap`]（角度度 →
    /// 弧度）→ 按元数路由 [`Self::do_native_build`] / [`Self::do_native_modify`]
    /// / [`Self::do_native_combine`]。
    ///
    /// 校验失败（必填选择器空串）或执行失败：表单**保持打开**（用户可改后重试）；
    /// 成功才关闭。失败一律写日志，不吞错。
    fn confirm_op_form(&mut self) {
        // 校验 + 换算（借用圈在块内；路由在块外做可变调用）。
        let (op, args) = {
            let Some(form) = self.op_form.as_ref() else {
                return;
            };
            let mut args = PropertyMap::new(format!("native:{}", form.op.label()));
            for def in form.op.param_defs() {
                let Some(v) = form.values.get(def.key) else {
                    continue;
                };
                // 必填选择器空串拒绝：空选择会让 Fillet / Chamfer / Shell 静默无效。
                if def.required
                    && let FormValue::Text(s) = v
                    && s.trim().is_empty()
                {
                    self.log(format!(
                        "✗ {}：{}（{}）为必填，请填选择器 JSON",
                        form.op.label(),
                        def.label,
                        def.key
                    ));
                    return;
                }
                let converted = match (def.kind, v) {
                    (ParamKind::Angle, FormValue::Float(deg)) => Value::Float(deg.to_radians()),
                    (ParamKind::Length, FormValue::Float(mm)) => Value::Float(*mm),
                    (ParamKind::Int, FormValue::Int(i)) => Value::Int(*i),
                    (ParamKind::Bool, FormValue::Bool(b)) => Value::Bool(*b),
                    (ParamKind::Selectors | ParamKind::Points, FormValue::Text(s)) => {
                        Value::Text(s.clone())
                    }
                    // 种类与定义不匹配（不应发生）：落缺，由内核缺参报错兜底。
                    _ => continue,
                };
                args.set(def.key.to_string(), converted);
            }
            (form.op, args)
        };
        // 按元数路由执行；成功关表单，失败保持打开（已写日志，可改后重试）。
        let ok = match op.arity() {
            0 => self.do_native_build(op, args),
            1 => self.do_native_modify(op, args),
            2 => self.do_native_combine(op, args),
            _ => {
                self.log(format!("· {} 是变长输入算子，G4 草图流程接入", op.label()));
                false
            }
        };
        if ok {
            self.op_form = None;
        }
    }

    /// 0 输入算子（G3）：建通用构件 + [`CoreSession::build_native`]
    /// （新构件收几何；失败写日志，不 panic）。
    fn do_native_build(&mut self, op: BuildOp, args: PropertyMap) -> bool {
        let name = op.label();
        match self.session.create_generic_element(name) {
            Ok(id) => match self.session.build_native(id, op, &args) {
                Ok(out) => {
                    self.after_geometry_change(id, out.shape);
                    self.log(format!("✓ {name}：新构件已建几何（ShapeId {}）", out.shape));
                    for w in &out.warnings {
                        self.log(format!("⚠ {w}"));
                    }
                    true
                }
                Err(e) => {
                    self.log(format!("✗ {name} 建几何失败：{e}"));
                    false
                }
            },
            Err(e) => {
                self.log(format!("✗ 创建 {name} 构件失败：{e}"));
                false
            }
        }
    }

    /// 1 输入算子（G3）：[`CoreSession::modify_native`] 替换选中构件几何
    /// （复用 ShapeId + 关联更新，图纸跟着动）。
    fn do_native_modify(&mut self, op: BuildOp, args: PropertyMap) -> bool {
        let Some(id) = self.selected else {
            return false; // 菜单按选中态启用，正常到不了这里
        };
        match self.session.modify_native(id, op, &args) {
            Ok(report) => {
                self.dirty = true;
                // M12 增量重建：只网格化本次被改的形状（modify 复用原 ShapeId）。
                if !report.changed_shapes.is_empty() {
                    self.dirty_shapes = Some(report.changed_shapes.iter().copied().collect());
                }
                self.log(format!(
                    "✓ {}:关联更新：演化直连 {} + 指纹回退 {}，重投 {} 图元、重算 {} 标注、{} 视图已标脏",
                    op.label(),
                    report.evolution_resolved,
                    report.fingerprint_resolved,
                    report.reprojected_entities,
                    report.remeasured_annotations,
                    report.stale_views
                ));
                for w in &report.warnings {
                    self.log(format!("⚠ {w}"));
                }
                true
            }
            Err(e) => {
                self.log(format!("✗ {} 修改失败：{e}", op.label()));
                false
            }
        }
    }

    /// 2 输入算子（G3）：[`CoreSession::combine_native`] 主选 + 副选 →
    /// 新构件收结果（不动原有两构件；布尔消费语义 G6+ 撤销引擎后完善）。
    fn do_native_combine(&mut self, op: BuildOp, args: PropertyMap) -> bool {
        let (Some(primary), Some(sec)) = (self.selected, self.secondary) else {
            return false; // 菜单按选中态启用，正常到不了这里
        };
        match self.session.combine_native(primary, sec, op, &args) {
            Ok(out) => {
                self.after_geometry_change(out.element, out.shape);
                // 布尔完成，副选清空（下一次组合需重新 Ctrl+单击）。
                self.secondary = None;
                self.log(format!(
                    "✓ {}：新构件已收结果（ShapeId {}，原两构件不动）",
                    op.label(),
                    out.shape
                ));
                for w in &out.warnings {
                    self.log(format!("⚠ {w}"));
                }
                true
            }
            Err(e) => {
                self.log(format!("✗ {} 组合失败：{e}", op.label()));
                false
            }
        }
    }

    /// 几何变化后的 GUI 状态收尾（build / combine 共用）：选中新构件、
    /// 增量网格化新形状、下一帧适配视图。
    fn after_geometry_change(&mut self, element: ElementId, shape: ShapeId) {
        self.selected = Some(element);
        self.picked = None;
        self.dirty = true;
        // M12 增量重建：只网格化新建的形状。
        self.dirty_shapes = Some([shape].into_iter().collect());
        self.fit_pending = true;
    }

    /// 草图编辑器浮窗（G4-M15）：take 状态机跑一帧，按结果放回 / 收尾 / 丢弃。
    ///
    /// take + match 模式避开借用冲突：`run_editor` 持 `&mut session` /
    /// `&mut logs`（disjoint 字段借用），期间 `self.sketch` 已被 take 出来，
    /// 三者互不重叠；结束后按 [`EditorFrame`] 归还或丢弃状态。
    fn sketch_editor_window(&mut self, ctx: &egui::Context) {
        let Some(mut st) = self.sketch.take() else {
            return;
        };
        let frame =
            crate::sketch_editor::run_editor(&mut st, &mut self.session, &mut self.logs, ctx);
        match frame {
            crate::sketch_editor::EditorFrame::KeepOpen => self.sketch = Some(st),
            // 提交成功：run_editor 已写日志并关窗——这里只做渲染收尾。
            crate::sketch_editor::EditorFrame::Committed(element, shape) => {
                self.after_geometry_change(element, shape);
            }
            // × 关闭且未提交：草稿丢弃，日志响亮可见。
            crate::sketch_editor::EditorFrame::Closed => {
                self.log("草图编辑器已关闭（未提交，草稿丢弃）");
            }
        }
    }

    /// 状态栏（G2：文件名 · 命中坐标 · 拾取模式 · 内核 · 统计；内容自 top_bar 迁移）。
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        use egui_phosphor::regular as ph;
        ui.horizontal(|ui| {
            let name = self
                .file_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "未命名文档".to_string());
            // 点文件名 = 另存（快速指定路径）。
            if ui.monospace(name).clicked() {
                self.do_save_as();
            }
            ui.separator();
            match &self.picked {
                Some(pk) => {
                    let p = pk.world_point;
                    ui.monospace(format!(
                        "{} ({:.0}, {:.0}, {:.0})",
                        ph::CROSSHAIR,
                        p.x,
                        p.y,
                        p.z
                    ));
                }
                None => {
                    ui.weak("未命中");
                }
            }
            ui.separator();
            ui.label(format!("拾取：{}", self.pick_mode.label()));
            ui.separator();
            ui.monospace(format!("内核：{}", self.session.geometry().backend_name()));
            ui.separator();
            let sum = self.session.summary();
            ui.label(format!(
                "构件 {} · 几何 {} · 模块 {}",
                sum.elements, sum.geometry, sum.modules
            ));
        });
    }

    /// 左栏 · 元素树（G5-M21：§5.5——品类分组折叠 + 节点选中联动 + 删除；
    /// G6-M2：可见性眼睛 + 删除后视图自动清理）。
    ///
    /// 分组：按品类（BTreeMap 天然按品类名排序）；组头折叠开关 + 计数；
    /// 节点：显示名（无几何标注 + 隐藏淡显）+ 选中 / Ctrl 副选联动（与视口
    /// do_pick 同语义）+ 眼睛按钮（`set_element_visibility`：文档级显隐，
    /// 自动成为一个撤销步，随 .bimdoc 持久化）+ 删除按钮（`delete_element`：
    /// 元素 + 级联清关系 + 几何记录 + **受影响视图的孤儿图元 / 标注自动清除**
    /// ——`update_after_removal`，不再需要提示「重新生成」）。
    fn elements_panel(&mut self, ui: &mut egui::Ui) {
        use egui_phosphor::regular as ph;
        // —— 新建构件（模块算子按钮；mock 腿不支持的算子点击后日志响亮报错）——
        ui.label("新建构件：");
        ui.horizontal_wrapped(|ui| {
            for btn in op_buttons() {
                if ui.small_button(btn.label).clicked() {
                    self.build_op(btn.category, btn.op_id, btn.label);
                }
            }
        });
        ui.separator();
        // 先收集再交互（避免边遍历文档边改状态）：按品类分组（BTreeMap 天然有序）。
        let mut groups: BTreeMap<String, Vec<(ElementId, String, bool, bool)>> = BTreeMap::new();
        for (id, e) in self.session.document().doc().elements() {
            groups
                .entry(e.category.as_str().to_string())
                .or_default()
                .push((*id, e.name.clone(), e.geometry_ref.is_some(), e.visible));
        }
        if groups.is_empty() {
            ui.label("（无构件，从上方按钮新建）");
            return;
        }
        let mut delete: Option<(ElementId, String)> = None;
        let mut toggle_visible: Option<(ElementId, bool, String)> = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (category, items) in &groups {
                // 组头：折叠开关 + 品类名 + 计数
                let collapsed = self.collapsed_groups.contains(category);
                let caret = if collapsed {
                    ph::CARET_RIGHT
                } else {
                    ph::CARET_DOWN
                };
                if ui
                    .selectable_label(false, format!("{caret} {category}（{}）", items.len()))
                    .clicked()
                {
                    if collapsed {
                        self.collapsed_groups.remove(category);
                    } else {
                        self.collapsed_groups.insert(category.clone());
                    }
                }
                if collapsed {
                    continue;
                }
                for (id, name, has_geom, visible) in items {
                    let text = match (*has_geom, *visible) {
                        (true, true) => name.clone(),
                        (true, false) => format!("{name}（已隐藏）"),
                        (false, _) => format!("{name}（无几何）"),
                    };
                    // 副选构件同高亮（G3：布尔 / 扫掠类的第二输入）。
                    let is_sel = self.selected == Some(*id) || self.secondary == Some(*id);
                    ui.horizontal(|ui| {
                        if ui.selectable_label(is_sel, text).clicked() {
                            // Ctrl+单击 = 设副选；普通单击 = 主选（与视口 do_pick 同语义）。
                            if ui.input(|i| i.modifiers.ctrl) {
                                self.secondary = Some(*id);
                                self.log(format!("✓ 已设副选：{name}"));
                            } else {
                                self.selected = Some(*id);
                                self.picked = None;
                            }
                        }
                        // 可见性眼睛（G6-M2：文档级显隐，撤销步；动作收集）
                        let (icon, tip) = if *visible {
                            (ph::EYE, "隐藏（撤销可回滚）")
                        } else {
                            (ph::EYE_SLASH, "显示（撤销可回滚）")
                        };
                        if ui.small_button(icon).on_hover_text(tip).clicked() {
                            toggle_visible = Some((*id, !*visible, name.clone()));
                        }
                        // 删除（G5-M21：动作收集，借用结束后执行）
                        if ui.small_button(ph::TRASH).clicked() {
                            delete = Some((*id, name.clone()));
                        }
                    });
                }
            }
        });
        // —— 动作段（文档遍历借用已结束）——
        if let Some((id, visible, name)) = toggle_visible {
            match self.session.set_element_visibility(id, visible) {
                Ok(()) => {
                    self.dirty = true; // 视口过滤按 visible 重算
                    self.log(if visible {
                        format!("✓ 已显示「{name}」（撤销可回滚）")
                    } else {
                        format!("✓ 已隐藏「{name}」（撤销可回滚）")
                    });
                }
                Err(e) => self.log(format!("✗ 显隐失败：{e}")),
            }
        }
        if let Some((id, name)) = delete {
            let shape = self
                .session
                .document()
                .doc()
                .element(id)
                .and_then(|e| e.geometry_ref);
            match self.session.delete_element(id) {
                Ok(report) => {
                    if let Some(s) = shape {
                        self.meshes.remove(&s);
                    }
                    // 选中态清理（删的是选中 / 副选 / 编辑中 → 归零）
                    if self.selected == Some(id) {
                        self.selected = None;
                        self.picked = None;
                    }
                    if self.secondary == Some(id) {
                        self.secondary = None;
                    }
                    if self.edit.as_ref().map(|e| e.element) == Some(id) {
                        self.edit = None;
                    }
                    self.op_by_element.remove(&id);
                    self.dirty = true;
                    self.log(format!(
                        "✓ 已删除构件「{name}」（元素 + 级联关系 + 几何记录；撤销可整体回滚）"
                    ));
                    self.log(format!(
                        "· 已清理相关视图的孤儿图元 {} 个 / 标注 {} 条（索引随清理重建）",
                        report.removed_entities, report.removed_annotations
                    ));
                }
                Err(e) => self.log(format!("✗ 删除失败：{e}")),
            }
        }
    }

    /// 左栏 · 类型管理器（G5-M20：§5.3——类型列表 + 新建 / 编辑 PropertySchema）。
    ///
    /// 编辑语义（`types_editor` 模块文档同款）：新建模式重名校验拒绝（新建不
    /// 该静默覆盖既有类型）；编辑模式 type_name 只读（不做改名语义——改名会
    /// 插入新条目、旧条目残留）、保存走 `upsert_schema` 覆盖。约束（诚实提示）：
    /// 覆盖**只影响新实例**——既有实例 PropertyMap 是落成时刻快照不回写。
    /// schema 持久化（G6-M2）：自定义类型随 `.bimdoc` 的 `schemas.json` 段
    /// 落盘，重开文档不丢；旧容器无该段恢复空 registry。
    fn types_panel(&mut self, ui: &mut egui::Ui) {
        use egui_phosphor::regular as ph;
        ui.horizontal(|ui| {
            ui.label("已注册类型：");
            if ui.small_button(format!("{} 新建类型", ph::PLUS)).clicked() {
                self.types_edit = Some(TypesEdit::new_blank());
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            // —— 类型列表（按名有序；M19 list_schemas）——
            let schemas = self.session.list_schemas();
            if schemas.is_empty() {
                ui.label("（无已注册类型）");
            }
            for schema in &schemas {
                let editing = self
                    .types_edit
                    .as_ref()
                    .is_some_and(|t| !t.is_new && t.type_name == schema.type_name);
                let text = format!("{}（{} 字段）", schema.type_name, schema.fields.len());
                if ui.selectable_label(editing, text).clicked() {
                    self.types_edit = Some(TypesEdit::from_schema(schema));
                }
            }
            // —— 编辑区渲染段（types_edit 可变借用圈在块内；照 props_panel
            //    「渲染收集动作」同款，动作在借用结束后执行）——
            let mut save_clicked = false;
            let mut cancel_clicked = false;
            if let Some(edit) = self.types_edit.as_mut() {
                ui.separator();
                ui.heading(if edit.is_new {
                    "新建类型"
                } else {
                    "编辑类型"
                });
                ui.weak(if edit.is_new {
                    "保存后可在属性面板给构件挂该类型（非法值会被 validate 拦截）"
                } else {
                    "⚠ 覆盖保存只影响新实例：既有构件的属性是落成时刻快照，不回写"
                });
                ui.weak("（自定义类型随 .bimdoc 持久化；旧容器无该段打开后为空）");
                ui.horizontal(|ui| {
                    ui.label("类型名：");
                    if edit.is_new {
                        ui.text_edit_singleline(&mut edit.type_name);
                    } else {
                        ui.monospace(&edit.type_name);
                        ui.weak("（只读；保存将覆盖已注册定义）");
                    }
                });
                // 字段表格（名 / 类型 / 单位 / 必填 / 默认值 / 说明 + 删行）
                let mut remove: Option<usize> = None;
                egui::Grid::new("types_fields_grid")
                    .striped(true)
                    .show(ui, |ui| {
                        for (i, row) in edit.fields.iter_mut().enumerate() {
                            ui.text_edit_singleline(&mut row.name).on_hover_text(
                                if row.doc.is_empty() {
                                    "字段名".to_string()
                                } else {
                                    row.doc.clone()
                                },
                            );
                            egui::ComboBox::from_id_salt(format!("ft_{i}"))
                                .selected_text(types_editor::type_label(&row.field_type))
                                .show_ui(ui, |ui| {
                                    for ft in &types_editor::FIELD_TYPES {
                                        let is_enum = matches!(ft, FieldType::Enum(_))
                                            && matches!(row.field_type, FieldType::Enum(_));
                                        if ui
                                            .selectable_label(
                                                is_enum || row.field_type == *ft,
                                                types_editor::type_label(ft),
                                            )
                                            .clicked()
                                            && !is_enum
                                        {
                                            // 切到枚举清空旧类型；已是枚举时再点不重置选项
                                            row.field_type = ft.clone();
                                        }
                                    }
                                });
                            ui.text_edit_singleline(&mut row.unit)
                                .on_hover_text("单位（空 = 无）");
                            ui.checkbox(&mut row.required, "必填");
                            ui.checkbox(&mut row.has_default, "默认");
                            match (&row.field_type, row.has_default) {
                                (_, false) => {
                                    ui.weak("—");
                                }
                                (FieldType::Ref, true) => {
                                    ui.weak("引用无默认");
                                }
                                _ => {
                                    ui.text_edit_singleline(&mut row.default_text);
                                }
                            }
                            ui.text_edit_singleline(&mut row.doc)
                                .on_hover_text("说明（字段名悬停展示）");
                            if ui.small_button(format!("{} 删", ph::TRASH)).clicked() {
                                remove = Some(i);
                            }
                            ui.end_row();
                            if matches!(row.field_type, FieldType::Enum(_)) {
                                ui.label("选项");
                                ui.text_edit_singleline(&mut row.enum_options)
                                    .on_hover_text("枚举选项（逗号分隔）");
                                ui.end_row();
                            }
                        }
                    });
                if let Some(i) = remove {
                    edit.fields.remove(i);
                }
                ui.horizontal(|ui| {
                    if ui.small_button(format!("{} 加字段", ph::PLUS)).clicked() {
                        edit.fields.push(types_editor::FieldRow::blank());
                    }
                });
                if let Some(msg) = &edit.message {
                    ui.colored_label(egui::Color32::RED, msg.clone());
                }
                ui.horizontal(|ui| {
                    if ui.button("保存").clicked() {
                        save_clicked = true;
                    }
                    if ui.button("取消").clicked() {
                        cancel_clicked = true;
                    }
                });
            }
            // —— 动作段（edit 借用结束；照 props_panel 同款）——
            if cancel_clicked {
                self.types_edit = None;
            }
            if save_clicked {
                let outcome: Result<Option<(String, usize)>, String> =
                    match self.types_edit.as_ref() {
                        Some(e) => match e.build_schema() {
                            Ok(schema) => {
                                let name = schema.type_name.clone();
                                let count = schema.fields.len();
                                // 新建模式重名校验（编辑模式覆盖语义——upsert 直接落库）
                                let exists = self
                                    .session
                                    .list_schemas()
                                    .iter()
                                    .any(|s| s.type_name == name);
                                if e.is_new && exists {
                                    Err(format!("类型名已存在：{name}"))
                                } else {
                                    match self.session.upsert_schema(schema) {
                                        Ok(()) => Ok(Some((name, count))),
                                        Err(err) => Err(err.to_string()),
                                    }
                                }
                            }
                            Err(msg) => Err(msg),
                        },
                        None => Ok(None),
                    };
                match outcome {
                    Ok(Some((name, count))) => {
                        self.types_edit = None;
                        self.log(format!("✓ 类型「{name}」已保存（{count} 个字段）"));
                    }
                    Ok(None) => {}
                    Err(msg) => {
                        if let Some(e) = self.types_edit.as_mut() {
                            e.message = Some(msg);
                        }
                    }
                }
            }
        });
    }

    fn props_panel(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.selected else {
            ui.label("未选中构件");
            ui.label("（列表单击，或视口内单击拾取）");
            self.edit = None;
            return;
        };
        let Some(elem) = self.session.document().doc().element(id).cloned() else {
            ui.label("构件不存在");
            self.edit = None;
            return;
        };
        egui::Grid::new("props_grid").striped(true).show(ui, |ui| {
            ui.label("名称");
            ui.label(elem.name.clone());
            ui.end_row();
            ui.label("品类");
            ui.label(elem.category.as_str());
            ui.end_row();
            ui.label("ID");
            ui.label(format!("{}", elem.id));
            ui.end_row();
            ui.label("几何");
            ui.label(match elem.geometry_ref {
                Some(s) => format!("ShapeId {s}"),
                None => "无".to_string(),
            });
            ui.end_row();
        });

        ui.separator();
        ui.heading("参数");
        // —— M10：选中变化时重置编辑暂存（快照当前参数 + 该构件的算子）——
        // G5-M21：一并快照模式定义（fields，渲染路由用；未注册模式为空）。
        if self.edit.as_ref().map(|e| e.element) != Some(id) {
            let schemas = self.session.list_schemas();
            self.edit = self.op_by_element.get(&id).map(|op| {
                let schema = elem.properties.schema.clone();
                let fields = schemas
                    .iter()
                    .find(|s| s.type_name == schema)
                    .map(|s| s.fields.clone())
                    .unwrap_or_default();
                ParamEdit {
                    element: id,
                    op_id: op.clone(),
                    schema,
                    fields,
                    values: elem.properties.values.clone(),
                }
            });
        }
        // 渲染收集动作（edit 可变借用只圈在渲染段；动作在借用结束后执行）。
        let mut apply_clicked = false;
        let mut revert_clicked = false;
        match self.edit.as_mut() {
            // 无算子记录（打开的文档构件 / 无几何构件）：参数只读展示。
            None => {
                if elem.properties.is_empty() {
                    ui.label("（无属性）");
                } else {
                    ui.label(format!("模式：{}", elem.properties.schema));
                    egui::Grid::new("params_grid").striped(true).show(ui, |ui| {
                        for (k, v) in &elem.properties.values {
                            ui.label(k.clone());
                            ui.label(format_value(v));
                            ui.end_row();
                        }
                    });
                }
                ui.label("（该构件未在本会话造过几何，无算子记录；参数只读）");
            }
            Some(edit) => {
                ui.label(format!(
                    "模式：{}（可编辑，改后点\"应用并重建\"；非法值会被已注册模式拦截）",
                    elem.properties.schema
                ));
                // G5-M21：FieldDef 完整路由——required 缺失标红 + doc tooltip +
                // unit 后缀 + Enum 下拉 + default「默认」一键回填；未定义的
                // 未知字段红字附加（validate 会拦截）。
                let mut set_default: Option<(String, Value)> = None;
                egui::Grid::new("params_edit_grid")
                    .striped(true)
                    .show(ui, |ui| {
                        // ① 已定义字段按 schema 顺序（G5-M21：不再是 BTreeMap 字典序）
                        for def in &edit.fields {
                            let missing = !edit.values.contains_key(&def.name);
                            // 名列：required 缺失标红 + doc tooltip
                            let name_text = if def.required && missing {
                                format!("{}（必填）", def.name)
                            } else {
                                def.name.clone()
                            };
                            let name_label = if def.required && missing {
                                ui.colored_label(egui::Color32::RED, name_text)
                            } else {
                                ui.label(name_text)
                            };
                            let mut tip = def.doc.clone();
                            if def.required && missing {
                                if !tip.is_empty() {
                                    tip.push('\n');
                                }
                                tip.push_str("必填字段缺失：确认时被校验拦截");
                            }
                            if !tip.is_empty() {
                                name_label.on_hover_text(tip);
                            }
                            // 值列：按 FieldDef 路由
                            match edit.values.get_mut(&def.name) {
                                Some(v) => {
                                    match v {
                                        Value::Float(f) => {
                                            let mut dv = egui::DragValue::new(f).speed(1.0);
                                            if let Some(unit) = &def.unit {
                                                dv = dv.suffix(format!(" {unit}"));
                                            }
                                            ui.add(dv);
                                        }
                                        Value::Int(i) => {
                                            ui.add(egui::DragValue::new(i));
                                        }
                                        Value::Text(s) => match &def.field_type {
                                            // Enum 下拉（G5-M21：之前只是文本框）
                                            FieldType::Enum(options) => {
                                                egui::ComboBox::from_id_salt(format!(
                                                    "pe_enum_{}",
                                                    def.name
                                                ))
                                                .selected_text(s.clone())
                                                .show_ui(ui, |ui| {
                                                    for opt in options {
                                                        if ui
                                                            .selectable_label(
                                                                *s == *opt,
                                                                opt.as_str(),
                                                            )
                                                            .clicked()
                                                        {
                                                            *s = opt.clone();
                                                        }
                                                    }
                                                });
                                            }
                                            _ => {
                                                ui.text_edit_singleline(s);
                                            }
                                        },
                                        Value::Bool(b) => {
                                            ui.checkbox(b, "");
                                        }
                                        Value::Ref(r) => {
                                            ui.label(format!("→ {r}"));
                                        }
                                    }
                                }
                                None => {
                                    ui.weak("（缺失）");
                                }
                            }
                            // 恢复默认列（def.default 存在才给按钮）
                            if let Some(d) = &def.default
                                && ui.small_button("默认").clicked()
                            {
                                set_default = Some((def.name.clone(), d.clone()));
                            }
                            ui.end_row();
                        }
                        // ② 未知字段红字附加（schema 没有此字段定义——validate 拦截）
                        for (k, v) in edit.values.iter_mut() {
                            if edit.fields.iter().any(|f| &f.name == k) {
                                continue;
                            }
                            ui.colored_label(egui::Color32::RED, k.clone())
                                .on_hover_text(
                                    "未知字段：该模式没有此字段定义，确认时会被校验拦截",
                                );
                            match v {
                                Value::Float(f) => {
                                    ui.add(egui::DragValue::new(f).speed(1.0));
                                }
                                Value::Int(i) => {
                                    ui.add(egui::DragValue::new(i));
                                }
                                Value::Text(s) => {
                                    ui.text_edit_singleline(s);
                                }
                                Value::Bool(b) => {
                                    ui.checkbox(b, "");
                                }
                                Value::Ref(r) => {
                                    ui.label(format!("→ {r}"));
                                }
                            }
                            ui.end_row();
                        }
                    });
                ui.horizontal(|ui| {
                    if ui.button("应用并重建").clicked() {
                        apply_clicked = true;
                    }
                    if ui.button("还原").clicked() {
                        revert_clicked = true;
                    }
                });
                if let Some((name, d)) = set_default {
                    edit.values.insert(name, d);
                }
            }
        }
        if apply_clicked {
            self.apply_rebuild();
        }
        if revert_clicked && let Some(e) = self.edit.as_mut() {
            e.values = elem.properties.values.clone();
        }

        ui.separator();
        ui.heading("拾取信息");
        match &self.picked {
            None => {
                ui.label("（单击视口拾取）");
            }
            Some(p) => {
                let w = p.world_point;
                egui::Grid::new("pick_grid").striped(true).show(ui, |ui| {
                    ui.label("种类");
                    ui.label(match p.kind {
                        PickKind::Face => "面",
                        PickKind::Edge => "边",
                        PickKind::Vertex => "顶点",
                    });
                    ui.end_row();
                    ui.label("命中点");
                    ui.label(format!("({:.0}, {:.0}, {:.0})", w.x, w.y, w.z));
                    ui.end_row();
                    ui.label("参数");
                    ui.label(match p.param {
                        PickParam::Face { u, v } => format!("u={u:.3}, v={v:.3}"),
                        PickParam::Edge { t } => format!("t={t:.3}"),
                        PickParam::Vertex => "（顶点无参数）".to_string(),
                    });
                    ui.end_row();
                    ui.label("拓扑哈希");
                    ui.label(if p.topo_hash == 0 {
                        "（边/顶点无哈希，高亮走标记点）".to_string()
                    } else {
                        format!("{}", p.topo_hash)
                    });
                    ui.end_row();
                    ui.label("法向");
                    ui.label(match p.normal {
                        Some(n) => format!("({:.3}, {:.3}, {:.3})", n.x, n.y, n.z),
                        None => "（边/顶点或内核未提供）".to_string(),
                    });
                    ui.end_row();
                });
            }
        }

        ui.separator();
        ui.heading("网格统计");
        match elem.geometry_ref.and_then(|s| self.meshes.get(&s)) {
            None => {
                ui.label("（未网格化）");
            }
            Some(sm) => {
                egui::Grid::new("mesh_grid").striped(true).show(ui, |ui| {
                    ui.label("顶点");
                    ui.label(format!("{}", sm.mesh.vertex_count()));
                    ui.end_row();
                    ui.label("三角形");
                    ui.label(format!("{}", sm.mesh.triangle_count()));
                    ui.end_row();
                    ui.label("面");
                    ui.label(format!("{}", sm.mesh.face_groups.len()));
                    ui.end_row();
                });
            }
        }
    }

    fn logs_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("日志");
            if ui.small_button("清空").clicked() {
                self.logs.clear();
            }
        });
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for line in &self.logs {
                    ui.monospace(line.clone());
                }
            });
    }

    // ---- 图纸面板（M11：视图列表 + 新建视图 + 重新出图 + DXF 导出 + 2D 预览） ----

    fn views_panel(&mut self, ui: &mut egui::Ui) {
        // —— 新建视图（三个内置视向预设）——
        ui.label("新建视图：");
        let presets = view_presets();
        ui.horizontal_wrapped(|ui| {
            for p in &presets {
                if ui.small_button(p.label).clicked() {
                    match self.session.create_view(p.label, p.orientation) {
                        Ok(vid) => {
                            self.selected_view = Some(vid);
                            self.log(format!("✓ 视图已创建：{}（ViewId {vid}）", p.label));
                        }
                        Err(e) => self.log(format!("✗ 创建视图失败：{e}")),
                    }
                }
            }
        });
        ui.separator();
        // 视图列表（先收集再交互，避免边遍历边改状态）。
        let items: Vec<(ViewId, String, bool, usize)> = self
            .session
            .document()
            .drawing()
            .views
            .iter()
            .map(|(id, v)| (*id, v.name.clone(), v.up_to_date, v.entities.len()))
            .collect();
        if items.is_empty() {
            ui.label("（无视图，从上方按钮新建）");
            return;
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (id, name, up_to_date, entity_count) in items {
                let text = if up_to_date {
                    name
                } else {
                    format!("{name} ⚠过期")
                };
                if ui
                    .selectable_label(self.selected_view == Some(id), text)
                    .clicked()
                {
                    self.selected_view = Some(id);
                }
                // 选中视图的操作条（M10 stale 徽章 + 重新出图在这里落地）。
                if self.selected_view == Some(id) {
                    ui.horizontal(|ui| {
                        if ui.small_button("重新出图").clicked() {
                            self.regenerate_selected();
                        }
                        if ui.small_button("导出 DXF").clicked() {
                            self.export_dxf(id);
                        }
                        if ui.small_button("预览").clicked() {
                            self.preview_open = true;
                        }
                    });
                    ui.label(format!("图元 {entity_count} 条"));
                    ui.separator();
                }
            }
        });
    }

    /// "重新出图"：走 `regenerate_view`（写回 + up_to_date 翻回 true；警告逐条可见）。
    fn regenerate_selected(&mut self) {
        let Some(vid) = self.selected_view else {
            return;
        };
        let standard = self.session.document().drawing().standard.clone();
        match self.session.regenerate_view(vid, &standard) {
            Ok((written, warnings)) => {
                self.log(format!("✓ 重新出图：写入 {written} 图元"));
                for w in warnings {
                    self.log(format!("⚠ {w}"));
                }
            }
            Err(e) => self.log(format!("✗ 重新出图失败：{e}")),
        }
    }

    /// 导出选中视图为 DXF（`core-interop` 既有能力；rfd 选路径 → 写文件）。
    fn export_dxf(&mut self, vid: ViewId) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("DXF", &["dxf"])
            .set_file_name("视图.dxf")
            .save_file()
        else {
            self.log("· DXF 导出已取消".to_string());
            return;
        };
        let Some(view) = self.session.document().drawing().view(vid).cloned() else {
            self.log("✗ 导出失败：视图不存在".to_string());
            return;
        };
        let standard = self.session.document().drawing().standard.clone();
        match self
            .session
            .interop()
            .export_view(Format::Dxf, &view, &standard)
        {
            Ok(out) => match std::fs::write(&path, &out.bytes) {
                Ok(()) => self.log(format!(
                    "✓ DXF 已导出：{}（{} 字节）",
                    path.display(),
                    out.bytes.len()
                )),
                Err(e) => self.log(format!("✗ DXF 写文件失败：{e}")),
            },
            Err(e) => self.log(format!("✗ DXF 导出失败：{e}")),
        }
    }

    // ---- 图纸页（G6-M3 出图深化：Sheet 布置——图幅 / 标题栏 / 整图预览导出） ----

    /// 图纸页：图纸列表 + 新建（图幅下拉）+ 布置表单 + 整图预览 / 导出。
    ///
    /// Sheet 是"把若干视图按比例摆到一张图幅上，再套图框与标题栏"（core-doc
    /// 语义）；本页只编排布置——摊平 / 图框 / 标题栏渲染全部复用既有
    /// `sheet_to_2d` 链路，`sheets` 随 drawing.json 持久化。
    fn sheets_panel(&mut self, ui: &mut egui::Ui) {
        // —— 新建图纸（标题栏用国标默认 180×56 右下角）——
        ui.label("新建图纸：");
        ui.horizontal(|ui| {
            ui.label("名称");
            ui.add(egui::TextEdit::singleline(&mut self.sheet_new_name).desired_width(110.0));
            ui.label("图号");
            ui.add(egui::TextEdit::singleline(&mut self.sheet_new_number).desired_width(80.0));
        });
        ui.horizontal(|ui| {
            ui.label("图幅");
            egui::ComboBox::from_id_salt("sheet_new_format")
                .selected_text(self.sheet_new_format.code())
                .show_ui(ui, |ui| {
                    for f in [
                        SheetFormat::A0,
                        SheetFormat::A1,
                        SheetFormat::A2,
                        SheetFormat::A3,
                        SheetFormat::A4,
                        SheetFormat::A4Extended,
                    ] {
                        ui.selectable_value(&mut self.sheet_new_format, f, f.code());
                    }
                });
            if ui.button("新建").clicked() {
                let name = self.sheet_new_name.trim();
                let number = self.sheet_new_number.trim();
                let name = if name.is_empty() { "图纸" } else { name };
                let number = if number.is_empty() { "G-01" } else { number };
                match self
                    .session
                    .create_sheet(name, number, self.sheet_new_format)
                {
                    Ok(sid) => {
                        self.selected_sheet = Some(sid);
                        self.log(format!(
                            "✓ 图纸已创建：{number} · {name}（{}，撤销可恢复）",
                            self.sheet_new_format.code()
                        ));
                    }
                    Err(e) => self.log(format!("✗ 创建图纸失败：{e}")),
                }
            }
        });
        ui.separator();
        // 图纸列表（先收集再交互，避免边遍历边改状态）。
        let items: Vec<(SheetId, String, String, String, usize)> = self
            .session
            .document()
            .drawing()
            .sheets
            .iter()
            .map(|(id, s)| {
                (
                    *id,
                    s.number.clone(),
                    s.name.clone(),
                    s.format.code().to_string(),
                    s.placement_count(),
                )
            })
            .collect();
        if items.is_empty() {
            ui.label("（无图纸，从上方新建）");
            return;
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            // 渲染收集动作（elements_panel 先例：可变借用圈在渲染段，动作借用结束后执行）
            let mut do_remove_sheet: Option<SheetId> = None;
            let mut do_remove_pl: Option<(SheetId, usize)> = None;
            let mut do_place: Option<(SheetId, ViewId, glam::DVec2, u32)> = None;
            for (id, number, name, fmt_code, count) in items {
                let text = if count > 0 {
                    format!("{number} · {name}（{fmt_code}，{count} 布置）")
                } else {
                    format!("{number} · {name}（{fmt_code}，空图纸）")
                };
                if ui
                    .selectable_label(self.selected_sheet == Some(id), &text)
                    .clicked()
                {
                    self.selected_sheet = Some(id);
                }
                // 选中图纸的操作条 + 放置表单 + 布置列表
                if self.selected_sheet == Some(id) {
                    ui.horizontal(|ui| {
                        if ui.small_button("整图预览").clicked() {
                            self.sheet_preview_open = true;
                        }
                        if ui.small_button("导出 DXF").clicked() {
                            self.export_sheet_file(Format::Dxf, id);
                        }
                        if ui.small_button("导出 SVG").clicked() {
                            self.export_sheet_file(Format::Svg, id);
                        }
                        if ui.small_button("删除图纸").clicked() {
                            do_remove_sheet = Some(id);
                        }
                    });
                    // —— 放置视图表单（视图下拉 + 比例下拉 + 位置；都先克隆出快照，
                    //    避免 session 不可变借用与暂存字段可变借用并存）——
                    ui.label("放置视图：");
                    let views: Vec<(ViewId, String)> = self
                        .session
                        .document()
                        .drawing()
                        .views
                        .iter()
                        .map(|(vid, v)| (*vid, v.name.clone()))
                        .collect();
                    if self
                        .place_view_id
                        .is_none_or(|vid| !views.iter().any(|(v, _)| *v == vid))
                    {
                        self.place_view_id = views.first().map(|(vid, _)| *vid);
                    }
                    let scales = self
                        .session
                        .document()
                        .drawing()
                        .standard
                        .scale_series
                        .clone();
                    ui.horizontal(|ui| {
                        let pv = self
                            .place_view_id
                            .and_then(|vid| views.iter().find(|(v, _)| *v == vid))
                            .map(|(_, n)| n.clone())
                            .unwrap_or_else(|| "（选视图）".to_string());
                        egui::ComboBox::from_id_salt(format!("place_view_{id}"))
                            .selected_text(pv)
                            .show_ui(ui, |ui| {
                                for (vid, vname) in &views {
                                    ui.selectable_value(&mut self.place_view_id, Some(*vid), vname);
                                }
                            });
                        ui.label("比例");
                        egui::ComboBox::from_id_salt(format!("place_scale_{id}"))
                            .selected_text(format!("1:{}", self.place_scale))
                            .show_ui(ui, |ui| {
                                for d in &scales {
                                    ui.selectable_value(
                                        &mut self.place_scale,
                                        *d,
                                        format!("1:{d}"),
                                    );
                                }
                            });
                    });
                    ui.horizontal(|ui| {
                        ui.label("位置");
                        ui.add(
                            egui::DragValue::new(&mut self.place_x_mm)
                                .prefix("X ")
                                .suffix(" mm")
                                .speed(1.0),
                        );
                        ui.add(
                            egui::DragValue::new(&mut self.place_y_mm)
                                .prefix("Y ")
                                .suffix(" mm")
                                .speed(1.0),
                        );
                        if ui.button("布置").clicked()
                            && let Some(vid) = self.place_view_id
                        {
                            do_place = Some((
                                id,
                                vid,
                                glam::DVec2::new(self.place_x_mm, self.place_y_mm),
                                self.place_scale,
                            ));
                        }
                    });
                    // —— 布置列表（快照 + 视图名解析）——
                    if count > 0 {
                        let placements: Vec<(usize, ViewId, glam::DVec2, u32)> = self
                            .session
                            .document()
                            .drawing()
                            .sheet(id)
                            .map(|s| {
                                s.placements
                                    .iter()
                                    .enumerate()
                                    .map(|(idx, p)| (idx, p.view, p.at, p.scale_denominator))
                                    .collect()
                            })
                            .unwrap_or_default();
                        ui.label(format!("布置（{count}）："));
                        for (idx, pvid, at, scale) in placements {
                            let pname = self
                                .session
                                .document()
                                .drawing()
                                .view(pvid)
                                .map(|v| v.name.clone())
                                .unwrap_or_else(|| "（视图已删）".to_string());
                            ui.horizontal(|ui| {
                                ui.label(format!("· {pname}（{:.0}, {:.0}）1:{scale}", at.x, at.y));
                                if ui.small_button("移除").clicked() {
                                    do_remove_pl = Some((id, idx));
                                }
                            });
                        }
                    }
                    ui.separator();
                }
            }
            // 动作段（渲染段的可变借用已结束）
            if let Some(sid) = do_remove_sheet {
                match self.session.remove_sheet(sid) {
                    Ok(()) => {
                        if self.selected_sheet == Some(sid) {
                            self.selected_sheet = None;
                        }
                        self.log("✓ 图纸已删除（其引用的视图不动；撤销可恢复）".to_string());
                    }
                    Err(e) => self.log(format!("✗ 删除图纸失败：{e}")),
                }
            }
            if let Some((sid, idx)) = do_remove_pl {
                match self.session.remove_placement(sid, idx) {
                    Ok(vid) => self.log(format!("✓ 布置已移除（ViewId {vid}；撤销可恢复）")),
                    Err(e) => self.log(format!("✗ 移除布置失败：{e}")),
                }
            }
            if let Some((sid, vid, at, scale)) = do_place {
                match self.session.place_view_on_sheet(sid, vid, at, scale) {
                    Ok(()) => self.log(format!("✓ 视图已布置到图纸（1:{scale}，撤销可恢复）")),
                    Err(e) => self.log(format!("✗ 布置失败：{e}")),
                }
            }
        });
    }

    /// 导出整张图纸（G6-M3：布置求值 + 图框 + 标题栏一起出，`export_sheet` 既有门面）。
    fn export_sheet_file(&mut self, format: Format, sid: SheetId) {
        let (label, ext) = match format {
            Format::Svg => ("SVG", "svg"),
            _ => ("DXF", "dxf"),
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter(label, &[ext])
            .set_file_name(format!("图纸.{ext}"))
            .save_file()
        else {
            self.log(format!("· {label} 导出已取消"));
            return;
        };
        let drawing = self.session.document().drawing().clone();
        match self.session.interop().export_sheet(format, &drawing, sid) {
            Ok(out) => match std::fs::write(&path, &out.bytes) {
                Ok(()) => self.log(format!(
                    "✓ {label} 已导出：{}（{} 字节）",
                    path.display(),
                    out.bytes.len()
                )),
                Err(e) => self.log(format!("✗ {label} 写文件失败：{e}")),
            },
            Err(e) => self.log(format!("✗ {label} 导出失败：{e}")),
        }
    }

    /// 浮动"整图预览"窗口（G6-M3）：把选中图纸摊平成 Drawing2D 后用 egui 画
    /// （图幅 / 图框 / 标题栏 / 全部布置一起画，`sheet_to_2d` 既有链路）。
    fn sheet_preview_window(&mut self, ctx: &egui::Context) {
        if !self.sheet_preview_open {
            return;
        }
        let mut open = self.sheet_preview_open;
        egui::Window::new("整图预览")
            .open(&mut open)
            .default_width(820.0)
            .default_height(600.0)
            .show(ctx, |ui| {
                let Some(sid) = self.selected_sheet else {
                    ui.label("（图纸页未选中图纸）");
                    return;
                };
                let Some(sheet) = self.session.document().drawing().sheet(sid).cloned() else {
                    ui.label("图纸不存在");
                    return;
                };
                let drawing = self.session.document().drawing().clone();
                match core_app::interop_service::sheet_to_2d(&drawing, &sheet) {
                    Ok(d2) => {
                        let title = format!("{}（{}）", sheet.number, sheet.name);
                        self.draw_drawing_2d(ui, &d2, title);
                    }
                    Err(e) => {
                        ui.label(format!("摊平失败：{e}"));
                    }
                }
            });
        self.sheet_preview_open = open;
    }

    /// 浮动"图纸预览"窗口：把选中视图摊平成 Drawing2D 后用 egui 画。
    fn drawing_preview_window(&mut self, ctx: &egui::Context) {
        if !self.preview_open {
            return;
        }
        let mut open = self.preview_open;
        egui::Window::new("图纸预览")
            .open(&mut open)
            .default_width(720.0)
            .default_height(520.0)
            .show(ctx, |ui| {
                let Some(vid) = self.selected_view else {
                    ui.label("（图纸面板未选中视图）");
                    return;
                };
                let Some(view) = self.session.document().drawing().view(vid).cloned() else {
                    ui.label("视图不存在");
                    return;
                };
                let standard = self.session.document().drawing().standard.clone();
                match core_app::interop_service::view_to_2d(&view, &standard) {
                    Ok(d2) => {
                        self.draw_drawing_2d(ui, &d2, view.name.clone());
                    }
                    Err(e) => {
                        ui.label(format!("投影失败：{e}"));
                    }
                }
            });
        self.preview_open = open;
    }

    /// 画一张 2D 图纸（Drawing2D → egui）。
    ///
    /// 坐标系：图纸 mm、Y 轴朝上；画布 px、Y 轴向下——线性变换 y 取负。
    /// **解析圆走 `painter.circle_stroke`（egui 真圆，不是折线近似）**——
    /// "圆是真圆"是 M11 的验收线；弧 / 椭圆按角度采样（egui 无弧 primitive）。
    fn draw_drawing_2d(&mut self, ui: &mut egui::Ui, d2: &Drawing2D, name: String) {
        ui.label(format!(
            "{name}（图幅 {:.0}×{:.0} mm，{} 曲线 / {} 文字）",
            d2.width_mm,
            d2.height_mm,
            d2.curves.len(),
            d2.texts.len()
        ));
        let (rect, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
        let painter = ui.painter().with_clip_rect(rect);
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(0xf5, 0xf4, 0xf0));
        // 图幅 → 画布：留 16px 边距、等比缩放、居中（mm 域算比例，px 域落点）。
        let avail_w = ((rect.width() - 32.0).max(1.0)) as f64;
        let avail_h = ((rect.height() - 32.0).max(1.0)) as f64;
        let k = (avail_w / d2.width_mm.max(1.0)).min(avail_h / d2.height_mm.max(1.0));
        let ox = rect.left() as f64 + 16.0 + (avail_w - d2.width_mm * k) / 2.0;
        let oy = rect.bottom() as f64 - 16.0 - (avail_h - d2.height_mm * k) / 2.0;
        let to_px = |p: [f64; 2]| egui::Pos2::new((ox + p[0] * k) as f32, (oy - p[1] * k) as f32);
        for c in &d2.curves {
            let color = egui::Color32::from_rgb(c.color[0], c.color[1], c.color[2]);
            let width = ((c.width_mm * k) as f32).clamp(0.5, 3.0);
            let stroke = egui::Stroke::new(width.max(1.0), color);
            match &c.analytic {
                Some(core_interop::Analytic2D::Circle { center, radius }) => {
                    // 解析圆 → egui 真圆。
                    painter.circle_stroke(to_px(*center), (*radius * k).max(0.5) as f32, stroke);
                }
                Some(core_interop::Analytic2D::Arc {
                    center,
                    radius,
                    start_angle_rad,
                    end_angle_rad,
                }) => {
                    // egui 无弧 primitive：按角度采样 64 段折线（屏幕上与真圆不可分）。
                    const SEGMENTS: usize = 64;
                    let r = (*radius * k).max(0.5);
                    let cx = (ox + center[0] * k) as f32;
                    let cy = (oy - center[1] * k) as f32; // y 翻转 → 角度取负
                    let span = end_angle_rad - start_angle_rad;
                    let pts: Vec<egui::Pos2> = (0..=SEGMENTS)
                        .map(|i| {
                            let a = start_angle_rad + span * (i as f64 / SEGMENTS as f64);
                            egui::Pos2::new(cx + (a.cos() * r) as f32, cy - (a.sin() * r) as f32)
                        })
                        .collect();
                    painter.add(egui::Shape::line(pts, stroke));
                }
                _ => {
                    // 折线（含解析椭圆的采样兜底点列）。
                    let pts: Vec<egui::Pos2> = c.points.iter().map(|p| to_px(*p)).collect();
                    if pts.len() >= 2 {
                        painter.add(egui::Shape::line(pts, stroke));
                    }
                }
            }
        }
        for t in &d2.texts {
            let at = to_px(t.at);
            painter.text(
                at,
                egui::Align2::LEFT_TOP,
                t.content.clone(),
                egui::FontId::proportional(((t.height_mm * k) as f32).max(9.0)),
                egui::Color32::from_rgb(t.color[0], t.color[1], t.color[2]),
            );
        }
    }

    // ---- 视口 ----

    fn viewport(&mut self, ui: &mut egui::Ui) {
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        // 视口几何喂给相机（拾取射线换算用）。
        self.camera.viewport = (
            rect.center().x as f64,
            rect.center().y as f64,
            rect.height() as f64,
        );
        let painter = ui.painter().with_clip_rect(rect);
        painter.rect_filled(rect, 0.0, VIEWPORT_BG);

        // —— 旋转（左键拖动；两轴方向均按实测校准：拖右＝物体右转，拖下＝相机降低）——
        if response.dragged_by(egui::PointerButton::Primary) && !ui.input(|i| i.modifiers.shift) {
            let d = response.drag_delta();
            // 水平：拖右＝物体右转＝相机绕目标左移（yaw 取负）。
            self.camera.yaw -= d.x as f64 * 0.01;
            // 垂直：拖下＝相机降低（pitch 取负，与水平轴同语义：拖动方向与参数增量反号）。
            // 无限位——越过极点由 normalize_pitch 翻转参数化（画面无跳变），
            // pitch_drag_sign 在过极点后翻转，方向映射保持自洽。
            self.camera.pitch -= self.camera.pitch_drag_sign * d.y as f64 * 0.01;
            self.camera.normalize_pitch();
        }
        // —— 平移（右键拖动 或 Shift+左键拖动，抓取世界语义）——
        if response.dragged_by(egui::PointerButton::Secondary)
            || (response.dragged_by(egui::PointerButton::Primary)
                && ui.input(|i| i.modifiers.shift))
        {
            let d = response.drag_delta();
            let (_, r, u) = self.camera.basis();
            let k = self.camera.distance * 0.0016;
            self.camera.target -= r * (d.x as f64 * k);
            self.camera.target += u * (d.y as f64 * k);
        }
        // —— 缩放（滚轮）——
        if response.hovered() {
            let scroll = ui.input(|i| i.raw_scroll_delta.y) as f64;
            if scroll != 0.0 {
                self.camera.distance = (self.camera.distance * (-scroll * 0.0022).exp())
                    .clamp(MIN_DISTANCE, MAX_DISTANCE);
            }
        }
        // —— 拾取（单击；Ctrl+单击 = 副选，G3）——
        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let ctrl = ui.input(|i| i.modifiers.ctrl);
            self.do_pick((pos.x, pos.y), ctrl);
        }

        // —— 画：地面网格 → 实体（画家算法）——
        draw_grid(&painter, &rect, &self.camera);
        let selected_shape = self.selected.and_then(|id| {
            self.session
                .document()
                .doc()
                .element(id)
                .and_then(|e| e.geometry_ref)
        });
        self.draw_shapes(&painter, &rect, selected_shape);

        // —— 拾取标记（M12：边 / 顶点命中画黄圈标记，与面高亮橙实色区分）——
        if let Some(pk) = &self.picked
            && !matches!(pk.kind, PickKind::Face)
            && let Some((sp, _)) = self.camera.project(pk.world_point, &rect)
        {
            let marker = egui::Color32::from_rgb(0xff, 0xd5, 0x2e);
            painter.circle_stroke(sp, 7.0, egui::Stroke::new(2.0_f32, marker));
            painter.circle_filled(sp, 2.5, marker);
        }

        painter.text(
            rect.left_bottom() + egui::vec2(8.0, -8.0),
            egui::Align2::LEFT_BOTTOM,
            "左键拖动＝旋转 · 右键/Shift+左键拖动＝平移 · 滚轮＝缩放 · 单击＝拾取面",
            egui::FontId::proportional(12.0),
            egui::Color32::from_rgb(0x96, 0x9b, 0xa5),
        );
    }

    /// 画全部实体：三角形按深度降序（远→近）组装进一个 egui mesh。
    fn draw_shapes(
        &self,
        painter: &egui::Painter,
        rect: &egui::Rect,
        selected_shape: Option<ShapeId>,
    ) {
        if self.meshes.is_empty() {
            return;
        }
        let light = DVec3::new(0.45, -0.5, 0.85).normalize_or_zero();
        struct Tri {
            depth: f64,
            pts: [egui::Pos2; 3],
            color: egui::Color32,
        }
        let mut tris: Vec<Tri> = Vec::new();

        // 元素级显隐过滤（G6-M2 眼睛）：某形状的**全部**引用元素都隐藏时才不画
        // （多元素共享同一段几何按"任一可见即画"）。每帧从元素预计算一次，
        // O(元素数)——视口渲染本身就要遍历全部 mesh，代价可接受。
        let visible_shapes: HashSet<ShapeId> = self
            .session
            .document()
            .doc()
            .elements()
            .values()
            .filter(|e| e.visible)
            .filter_map(|e| e.geometry_ref)
            .collect();
        for (shape_id, sm) in self.meshes.iter() {
            if !visible_shapes.contains(shape_id) {
                continue;
            }
            let mesh = &sm.mesh;
            let tri_count = mesh.triangle_count();
            let is_selected = Some(*shape_id) == selected_shape;

            // 三角形 → 面组映射（面组序号；越界/缺组记 usize::MAX）。
            let mut tri2group = vec![usize::MAX; tri_count];
            for (gi, fg) in mesh.face_groups.iter().enumerate() {
                let start = fg.tri_start as usize;
                let end = start.saturating_add(fg.tri_count as usize).min(tri_count);
                if start < end {
                    tri2group[start..end].fill(gi);
                }
            }
            // 每面组法线（取该组第一个三角形）。
            let face_normal = |gi: usize| -> DVec3 {
                let Some(fg) = mesh.face_groups.get(gi) else {
                    return DVec3::ZERO;
                };
                let base = fg.tri_start as usize * 3;
                let (Some(&ia), Some(&ib), Some(&ic)) = (
                    mesh.indices.get(base),
                    mesh.indices.get(base + 1),
                    mesh.indices.get(base + 2),
                ) else {
                    return DVec3::ZERO;
                };
                let (Some(pa), Some(pb), Some(pc)) = (
                    mesh.positions.get(ia as usize),
                    mesh.positions.get(ib as usize),
                    mesh.positions.get(ic as usize),
                ) else {
                    return DVec3::ZERO;
                };
                let a = DVec3::new(pa[0], pa[1], pa[2]);
                let b = DVec3::new(pb[0], pb[1], pb[2]);
                let c = DVec3::new(pc[0], pc[1], pc[2]);
                (b - a).cross(c - a).normalize_or_zero()
            };

            for (t, &gi) in tri2group.iter().enumerate() {
                let base = t * 3;
                let (Some(&ia), Some(&ib), Some(&ic)) = (
                    mesh.indices.get(base),
                    mesh.indices.get(base + 1),
                    mesh.indices.get(base + 2),
                ) else {
                    continue;
                };
                let (Some(pa), Some(pb), Some(pc)) = (
                    mesh.positions.get(ia as usize),
                    mesh.positions.get(ib as usize),
                    mesh.positions.get(ic as usize),
                ) else {
                    continue;
                };
                // 任一顶点在近平面内 → 跳过该三角形（首版不做裁剪）。
                let (Some(s0), Some(s1), Some(s2)) = (
                    self.camera.project(DVec3::new(pa[0], pa[1], pa[2]), rect),
                    self.camera.project(DVec3::new(pb[0], pb[1], pb[2]), rect),
                    self.camera.project(DVec3::new(pc[0], pc[1], pc[2]), rect),
                ) else {
                    continue;
                };
                let n = if gi == usize::MAX {
                    // 无面组的三角形：用自己的法线。
                    let a = DVec3::new(pa[0], pa[1], pa[2]);
                    let b = DVec3::new(pb[0], pb[1], pb[2]);
                    let c = DVec3::new(pc[0], pc[1], pc[2]);
                    (b - a).cross(c - a).normalize_or_zero()
                } else {
                    face_normal(gi)
                };
                // 双面光照（|n·l|），避免背向面全黑。
                let lit = 0.45 + 0.55 * n.dot(light).abs() as f32;
                let picked_face = gi != usize::MAX
                    && self
                        .picked
                        .as_ref()
                        .is_some_and(|pk| mesh.face_groups[gi].face_hash == pk.topo_hash);
                let color = if picked_face {
                    egui::Color32::from_rgb(PICK_ACCENT[0], PICK_ACCENT[1], PICK_ACCENT[2])
                } else {
                    let shaded = [
                        (sm.base_color[0] as f32 * lit) as u8,
                        (sm.base_color[1] as f32 * lit) as u8,
                        (sm.base_color[2] as f32 * lit) as u8,
                    ];
                    if is_selected {
                        mix_rgb(shaded, SELECT_TINT, 0.35)
                    } else {
                        egui::Color32::from_rgb(shaded[0], shaded[1], shaded[2])
                    }
                };
                tris.push(Tri {
                    depth: (s0.1 + s1.1 + s2.1) / 3.0,
                    pts: [s0.0, s1.0, s2.0],
                    color,
                });
            }
        }

        // 画家算法：按深度降序（远 → 近），后画覆盖先画。
        tris.sort_by(|x, y| {
            y.depth
                .partial_cmp(&x.depth)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        // epaint 0.29 的字体纹理（白色）是 Managed(0)。
        let mut mesh_out = egui::epaint::Mesh {
            texture_id: egui::TextureId::Managed(0),
            ..Default::default()
        };
        for t in &tris {
            let base = mesh_out.vertices.len() as u32;
            for pt in t.pts {
                mesh_out.vertices.push(egui::epaint::Vertex {
                    pos: pt,
                    uv: egui::Pos2::ZERO,
                    color: t.color,
                });
            }
            mesh_out
                .indices
                .extend_from_slice(&[base, base + 1, base + 2]);
        }
        if !mesh_out.indices.is_empty() {
            painter.add(egui::Shape::mesh(mesh_out));
        }
    }

    /// 聚焦一个 dock tab；若它已被关闭，加回当前聚焦的 leaf（G6-M5 日志开关语义）。
    fn focus_or_restore_tab(&mut self, tab: DockTab) {
        if let Some((surface, node, _tab_index)) = self.dock.find_tab(&tab) {
            self.dock.set_focused_node_and_surface((surface, node));
        } else {
            self.dock.push_to_focused_leaf(tab);
        }
    }
}

/// 画地面网格（z=0 平面，±6 m，1 m 间距）与三根坐标轴（X 红 / Y 绿 / Z 蓝）。
///
/// 先画网格后画实体（同一 painter 的 add 顺序），实体盖在网格之上；
/// 视点低于地面时会穿帮（首版接受——网格只是深度参照，不是遮挡求解）。
fn draw_grid(painter: &egui::Painter, rect: &egui::Rect, camera: &OrbitCamera) {
    const HALF: f64 = 6000.0;
    const STEP: f64 = 1000.0;
    let stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(0x3c, 0x41, 0x4c));
    let mut i = -HALF;
    while i <= HALF {
        // X 方向线（y = i）
        if let (Some(a), Some(b)) = (
            camera.project(DVec3::new(-HALF, i, 0.0), rect),
            camera.project(DVec3::new(HALF, i, 0.0), rect),
        ) {
            painter.line_segment([a.0, b.0], stroke);
        }
        // Y 方向线（x = i）
        if let (Some(a), Some(b)) = (
            camera.project(DVec3::new(i, -HALF, 0.0), rect),
            camera.project(DVec3::new(i, HALF, 0.0), rect),
        ) {
            painter.line_segment([a.0, b.0], stroke);
        }
        i += STEP;
    }
    // 坐标轴：X 红 / Y 绿 / Z 蓝（Z 朝上，与内核网格一致）。
    let axis = |a: DVec3, b: DVec3, color: egui::Color32| {
        if let (Some(pa), Some(pb)) = (camera.project(a, rect), camera.project(b, rect)) {
            painter.line_segment([pa.0, pb.0], egui::Stroke::new(2.0_f32, color));
        }
    };
    axis(
        DVec3::new(-HALF, 0.0, 0.0),
        DVec3::new(HALF, 0.0, 0.0),
        egui::Color32::from_rgb(0xc0, 0x50, 0x50),
    );
    axis(
        DVec3::new(0.0, -HALF, 0.0),
        DVec3::new(0.0, HALF, 0.0),
        egui::Color32::from_rgb(0x50, 0xb0, 0x60),
    );
    axis(
        DVec3::new(0.0, 0.0, 0.0),
        DVec3::new(0.0, 0.0, 3000.0),
        egui::Color32::from_rgb(0x55, 0x75, 0xc8),
    );
}

impl eframe::App for BimApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // M9：标题栏同步 + 文件快捷键（Ctrl+S 保存 / Ctrl+O 打开）。
        self.sync_title(ctx);
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S)) {
            self.do_save();
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::O)) {
            self.do_open();
        }
        // G6-M1：撤销 / 重做快捷键（Ctrl+Z / Ctrl+Y；command 在 Windows=Ctrl）。
        // key_pressed 含 key-repeat（按住连发，CAD 惯例）；无可撤销时静默忽略。
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Z)) {
            self.do_undo();
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Y)) {
            self.do_redo();
        }

        self.refresh_meshes();
        // G6-M5 可停靠面板 + G6-M8 Ribbon：菜单栏 → Ribbon → dock（面板区：元素/
        // 类型/属性/视图/图纸/日志/视口，用户可拖动重排 / 拖出窗口 / 关闭）→ 状态栏 → 浮窗。
        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| self.menu_bar(ui));
        egui::TopBottomPanel::top("ribbon").show(ctx, |ui| self.ribbon(ui));
        // bottom 面板先 show 的贴底：状态栏最底。
        egui::TopBottomPanel::bottom("status_bar")
            .exact_height(26.0)
            .show(ctx, |ui| self.status_bar(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                // 借用拆分：dock 暂移出去、viewer 持 &mut self——
                // 避免 self.dock 与 self 双重可变借用；渲染完还回。
                let mut dock = std::mem::replace(&mut self.dock, default_dock());
                {
                    let mut viewer = BimTabViewer { app: self };
                    egui_dock::DockArea::new(&mut dock)
                        .style(egui_dock::Style::from_egui(ui.style().as_ref()))
                        .show_inside(ui, &mut viewer);
                }
                self.dock = dock;
            });
        // M11：浮动图纸预览窗口（"视图"页签选中视图后点"预览"打开）。
        self.drawing_preview_window(ctx);
        // G6-M3：整图预览窗口（"图纸"页签选中图纸后点"整图预览"打开——
        // 图幅 / 图框 / 标题栏 / 全部布置一起画）。
        self.sheet_preview_window(ctx);
        // G3：原生算子参数表单浮窗（工具栏构建七组的算子项点击后打开）。
        self.op_form_window(ctx);
        // G4-M15：草图编辑器浮窗（工具栏三入口点击后打开）。
        self.sketch_editor_window(ctx);
    }
}

// ---------------------------------------------------------------------------
// 入口
// ---------------------------------------------------------------------------

/// 启动图形窗口。
///
/// # Errors
/// 会话装配失败或 eframe 初始化失败时返回错误文本（调用方打印并以非零退出）。
pub fn run_gui() -> Result<(), String> {
    let app = BimApp::new()?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("轻量化 BIM 内核平台 —— 桌面端")
            .with_inner_size([1440.0, 900.0]),
        ..Default::default()
    };
    eframe::run_native(
        "轻量化 BIM 内核平台",
        options,
        Box::new(|cc| {
            // G1：主题应用（Phosphor 图标 + CJK 兜底字体 + 深色 CAD visuals，见 theme.rs）。
            crate::theme::apply(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| format!("GUI 运行错误：{e}"))
}

#[cfg(test)]
mod tests {
    // workspace lints（unwrap_used / panic deny）在测试模块放开——仓库同款。
    #![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

    use super::*;

    /// `find_tab` 的薄包装：断言 tab 在主 surface 并返回其 node。
    fn main_surface_node_of(
        dock: &egui_dock::DockState<DockTab>,
        tab: DockTab,
    ) -> egui_dock::NodeIndex {
        let (surface, node, _tab_index) = dock
            .find_tab(&tab)
            .unwrap_or_else(|| panic!("默认布局缺少 {tab:?} 页签"));
        assert!(surface.is_main(), "{tab:?} 应在主 surface");
        node
    }

    /// 默认布局钉 1：7 个面板全在主 surface，视口与其他面板不同 node。
    #[test]
    fn default_dock_contains_all_seven_tabs() {
        let dock = default_dock();
        let viewport = main_surface_node_of(&dock, DockTab::Viewport);
        for tab in [
            DockTab::Elements,
            DockTab::Types,
            DockTab::Props,
            DockTab::Views,
            DockTab::Sheets,
            DockTab::Logs,
        ] {
            let node = main_surface_node_of(&dock, tab);
            assert_ne!(node, viewport, "{tab:?} 不应与视口同 node");
        }
    }

    /// 默认布局钉 2：元素/类型叠 tab（同 node），属性/视图/图纸叠 tab（同 node）——
    /// 五区布局的"左右栏各一组页签"语义。
    #[test]
    fn default_dock_stacks_tabs_per_region() {
        let dock = default_dock();
        let elements = main_surface_node_of(&dock, DockTab::Elements);
        let types = main_surface_node_of(&dock, DockTab::Types);
        assert_eq!(elements, types, "元素与类型应同 node（左栏叠 tab）");

        let props = main_surface_node_of(&dock, DockTab::Props);
        let views = main_surface_node_of(&dock, DockTab::Views);
        let sheets = main_surface_node_of(&dock, DockTab::Sheets);
        assert_eq!(props, views, "属性与视图应同 node（右栏叠 tab）");
        assert_eq!(views, sheets, "视图与图纸应同 node（右栏叠 tab）");

        let logs = main_surface_node_of(&dock, DockTab::Logs);
        assert_ne!(logs, elements, "日志是独立底部区");
        assert_ne!(props, elements, "左右栏是两个分区");
    }

    /// 默认布局钉 3：布局可重建——mem::replace 的兜底值与默认布局同构。
    #[test]
    fn default_dock_is_rebuildable() {
        let a = default_dock();
        let b = default_dock();
        for tab in [
            DockTab::Viewport,
            DockTab::Elements,
            DockTab::Types,
            DockTab::Props,
            DockTab::Views,
            DockTab::Sheets,
            DockTab::Logs,
        ] {
            assert_eq!(
                main_surface_node_of(&a, tab),
                main_surface_node_of(&b, tab),
                "两次构造的 {tab:?} 布局应一致"
            );
        }
    }

    /// Ribbon 钉 1：默认建模页签 + 三页签顺序与 label（建模/草图/视图）。
    #[test]
    fn ribbon_tabs_default_model_with_labels() {
        assert_eq!(RibbonTab::default(), RibbonTab::Model);
        let labels: Vec<&str> = RIBBON_TABS.iter().map(|t| t.label()).collect();
        assert_eq!(labels, ["建模", "草图", "视图"]);
        for label in &labels {
            assert!(!label.is_empty());
        }
    }

    /// Ribbon 钉 2：构建分组 7 组齐全、合计 23 算子，图标/组名/算子全非空。
    #[test]
    fn ribbon_build_groups_complete() {
        let groups = build_groups();
        assert_eq!(
            groups.len(),
            7,
            "构建七组：基本体/线框/扫掠放样/布尔/变换/修饰/修复"
        );
        let total: usize = groups.iter().map(|(_, _, ops)| ops.len()).sum();
        assert_eq!(total, 23, "七组合计 23 个算子");
        for (icon, name, ops) in &groups {
            assert!(!icon.is_empty(), "分组 {name} 图标非空");
            assert!(!name.is_empty());
            assert!(!ops.is_empty(), "分组 {name} 不应为空");
        }
    }

    /// Ribbon 钉 3：算子无重复——同一 BuildOp 不出现在两个分组。
    #[test]
    fn ribbon_build_ops_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for (_, name, ops) in build_groups() {
            for op in ops {
                assert!(
                    seen.insert(format!("{op:?}")),
                    "算子 {op:?} 在分组 {name} 重复出现"
                );
            }
        }
    }
}

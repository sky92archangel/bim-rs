//! # 桌面端图形窗口（`feature = "gui"`，egui 0.29 + eframe/wgpu）
//!
//! 真实 GUI 的第一版：**视口 / 拾取 / 属性面板** 三件套。
//!
//! ```text
//! ┌─────────┬──────────────────────────────┬──────────┐
//! │ 构件列表 │          三维视口             │  属性面板 │
//! │ (左栏)   │ 轨道相机 + 面着色 + 地面网格   │  (右栏)   │
//! │         │ 左键拖动=旋转 右键=平移 滚轮=缩放│          │
//! ├─────────┴──────────────────────────────┴──────────┤
//! │ 操作日志（底栏，失败响亮可见，不弹窗不吞错）          │
//! └───────────────────────────────────────────────────┘
//! ```
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

use std::collections::HashMap;

use eframe::egui;
use glam::DVec3;

use cable_tunnel::{
    CATEGORY as TUNNEL_CATEGORY, CableTunnel, GEOMETRY_OP_ID_DUCTBANK as OP_TUNNEL_DUCTBANK,
    GEOMETRY_OP_ID_RECT as OP_TUNNEL_RECT,
};
use core_app::CoreSession;
use core_doc::{Camera, PickFilter, PickResult, Ray};
use core_model::{ElementId, ShapeId, Value};
use core_sdk::Module;
use example_beam::{
    CATEGORY as BEAM_CATEGORY, ExampleBeam, GEOMETRY_OP_ID as OP_BEAM_RECT,
    GEOMETRY_OP_ID_HOLE as OP_BEAM_HOLE,
};
use hvac_duct::{
    CATEGORY as DUCT_CATEGORY, GEOMETRY_OP_ID_RECT as OP_DUCT_RECT,
    GEOMETRY_OP_ID_TRANSITION as OP_DUCT_TRANSITION, HvacDuct,
};

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

/// GUI 应用状态。
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

        // 起手造一根梁，让视口不空。
        match session.create_element(BEAM_CATEGORY, "") {
            Ok(id) => match session.build_geometry(id, OP_BEAM_RECT) {
                Ok(shape) => logs.push(format!("✓ 初始构件：梁已建几何（ShapeId {shape}）")),
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
        })
    }

    fn log(&mut self, msg: impl Into<String>) {
        self.logs.push(msg.into());
        if self.logs.len() > 200 {
            let overflow = self.logs.len() - 200;
            self.logs.drain(0..overflow);
        }
    }

    /// 脏标记置位时重建全部网格（占位内核便宜；真实内核单次也只是一次网格化）。
    fn refresh_meshes(&mut self) {
        if !self.dirty {
            return;
        }
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
        self.dirty = false;
        if self.fit_pending {
            self.fit_view();
            self.fit_pending = false;
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
                    self.fit_pending = true;
                    self.log(format!("✓ {label}：{op_id} 已建几何（ShapeId {shape}）"));
                }
                Err(e) => self.log(format!("✗ {label}：{op_id} 建几何失败：{e}")),
            },
            Err(e) => self.log(format!("✗ 创建 {category} 构件失败：{e}")),
        }
    }

    /// 视口拾取（单击）：屏幕坐标 → 面 → 元素。
    fn do_pick(&mut self, screen: (f32, f32)) {
        let camera = &self.camera;
        let filter = PickFilter {
            faces: true,
            edges: false,
            vertices: false,
        };
        match self.session.pick_at(screen, camera, filter) {
            Ok(Some(hit)) => {
                self.selected = Some(hit.element);
                let p = hit.world_point;
                self.log(format!(
                    "✓ 拾取面：命中点 ({:.0}, {:.0}, {:.0})",
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

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("轻量化 BIM 内核平台");
            ui.separator();
            ui.monospace(format!("内核：{}", self.session.geometry().backend_name()));
            ui.separator();
            let sum = self.session.summary();
            ui.label(format!(
                "构件 {} · 几何 {} · 模块 {}",
                sum.elements, sum.geometry, sum.modules
            ));
            ui.separator();
            if ui.button("适配视图").clicked() {
                self.fit_view();
            }
            if ui.button("清空选择").clicked() {
                self.selected = None;
                self.picked = None;
            }
        });
    }

    fn elements_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("构件");
        ui.separator();
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
        // 先收集再交互（避免边遍历文档边改状态）。
        let items: Vec<(ElementId, String, bool)> = self
            .session
            .document()
            .doc()
            .elements()
            .iter()
            .map(|(id, e)| {
                (
                    *id,
                    format!("{} · {}", e.name, e.category.as_str()),
                    e.geometry_ref.is_some(),
                )
            })
            .collect();
        if items.is_empty() {
            ui.label("（无构件，从上方按钮新建）");
            return;
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (id, label, has_geom) in items {
                let text = if has_geom {
                    label
                } else {
                    format!("{label}（无几何）")
                };
                if ui
                    .selectable_label(self.selected == Some(id), text)
                    .clicked()
                {
                    self.selected = Some(id);
                    self.picked = None;
                }
            }
        });
    }

    fn props_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("属性面板");
        ui.separator();
        let Some(id) = self.selected else {
            ui.label("未选中构件");
            ui.label("（列表单击，或视口内单击拾取）");
            return;
        };
        let Some(elem) = self.session.document().doc().element(id).cloned() else {
            ui.label("构件不存在");
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

        ui.separator();
        ui.heading("拾取信息");
        match &self.picked {
            None => {
                ui.label("（单击视口拾取面）");
            }
            Some(p) => {
                let w = p.world_point;
                egui::Grid::new("pick_grid").striped(true).show(ui, |ui| {
                    ui.label("命中点");
                    ui.label(format!("({:.0}, {:.0}, {:.0})", w.x, w.y, w.z));
                    ui.end_row();
                    ui.label("面哈希");
                    ui.label(format!("{}", p.topo_hash));
                    ui.end_row();
                    ui.label("法向");
                    ui.label(match p.normal {
                        Some(n) => format!("({:.3}, {:.3}, {:.3})", n.x, n.y, n.z),
                        None => "（内核未提供）".to_string(),
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
        // —— 拾取（单击）——
        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            self.do_pick((pos.x, pos.y));
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

        for (shape_id, sm) in self.meshes.iter() {
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
        self.refresh_meshes();
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| self.top_bar(ui));
        egui::SidePanel::left("elements_panel")
            .resizable(true)
            .default_width(230.0)
            .show(ctx, |ui| self.elements_panel(ui));
        egui::SidePanel::right("props_panel")
            .resizable(true)
            .default_width(300.0)
            .show(ctx, |ui| self.props_panel(ui));
        egui::TopBottomPanel::bottom("logs_panel")
            .resizable(true)
            .default_height(150.0)
            .show(ctx, |ui| self.logs_panel(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| self.viewport(ui));
    }
}

// ---------------------------------------------------------------------------
// 入口
// ---------------------------------------------------------------------------

/// 装载 CJK 字体（egui 默认字体无中文字形；Windows 取微软雅黑等系统字体兜底）。
fn install_cjk_font(ctx: &egui::Context) {
    let candidates = [
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/simhei.ttf",
        "C:/Windows/Fonts/simsun.ttc",
        "C:/Windows/Fonts/Deng.ttf",
    ];
    for path in candidates {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "cjk_fallback".to_string(),
            egui::FontData::from_owned(bytes),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .push("cjk_fallback".to_string());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push("cjk_fallback".to_string());
        ctx.set_fonts(fonts);
        return;
    }
    // 一个 CJK 字体都没找到：界面中文会显示为方框，但不影响功能（不 panic）。
}

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
            install_cjk_font(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| format!("GUI 运行错误：{e}"))
}

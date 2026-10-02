//! 草图编辑器（G4-M15）：egui 浮窗画布 + 工具 + 段内编辑。
//!
//! 渲染与交互（egui 依赖）在本模块（`#[cfg(feature = "gui")]` 门控）；
//! 工具状态机与捕捉 / 正交纯逻辑在**不门控**的 `sketch_tool`（默认腿可单测）。
//!
//! ## 坐标系
//!
//! 草图纸面 mm、数学坐标（y 向上、角度 ccw 为正）；画布 px、y 向下——线性
//! 变换 y 取负。视图状态（`view_scale` / `view_center`）支撑滚轮缩放（视点
//! 定标在光标处）与中键 / 右键平移；网格捕捉默认开（步长 10 mm），对象捕捉
//! （端点 / 中点 / 中心）优先，Shift 临时正交。
//!
//! ## 交互约定
//!
//! - 单击推进工具状态机（`sketch_tool::SketchDraft::click`）；Ctrl+单击改为
//!   选中附近图元（不画）；圆角 / 倒角工具单击拾取**线段图元**（不走点状态机）；
//! - 回车 / 双击结束开放曲线（折线 / 贝塞尔 / 样条），Backspace 撤点，
//!   Esc 取消进行中，Del 删除选中；
//! - Ctrl+Z 撤销上一个图元，Ctrl+Y 重做（新动作使重做失效）；
//! - 「完成」→ [`core_app::CoreSession::build_sketch_profile`]（造 wire +
//!   建通用元素，失败零半成品）→ 调用方做渲染收尾（选中新构件 + 网格化 + 适配）。

use glam::DVec2;

use core_app::{CoreSession, SketchEntity};

use crate::sketch_geom::{
    bezier_sample_polyline, catmull_rom_to_bezier, circle_through_three, segment_intersection,
};
use crate::sketch_tool::{
    ClickOutcome, CornerPick, DEFAULT_POLYGON_SIDES, DRAWING_TOOLS, EDITING_TOOLS, EditOutcome,
    SketchDraft, SketchTool, apply_chamfer, apply_fillet, mirror_entity, offset_entity,
    orthogonalize, pick_corner_segment, polygon_vertices, rotate_entity, scale_entity,
    snap_entities, snap_grid, translate_entity,
};

/// 网格捕捉步长（mm）。
const GRID_STEP_MM: f64 = 10.0;
/// 主网格（加粗）每格数。
const GRID_MAJOR_EVERY: i64 = 10;
/// 初始 mm→px 比例（px/mm）。
const INITIAL_SCALE: f64 = 2.0;
/// 对象捕捉阈值（px）——换算成 mm 用（mm tol = px tol / scale）。
const SNAP_TOL_PX: f64 = 8.0;
/// 滚轮缩放的上下限（px/mm）。
const SCALE_MIN: f64 = 0.05;
const SCALE_MAX: f64 = 50.0;
/// 尺寸标注底色（显示型标注与几何图元的区分色）。
const DIM_COLOR: egui::Color32 = egui::Color32::from_rgb(0x50, 0xb8, 0xd8);
/// 标注数字字号（px）。
const DIM_FONT_SIZE: f32 = 12.0;

/// 草图编辑器状态（BimApp 字段：`Some` = 编辑器开着）。
pub struct SketchEditorState {
    /// 编辑器开关（浮窗 × 或提交后关闭）
    pub open: bool,
    /// 当前工具
    pub tool: SketchTool,
    /// 进行中的图元
    pub draft: SketchDraft,
    /// 已提交图元（顺序即建边序）
    pub entities: Vec<SketchEntity>,
    /// 撤销栈（Ctrl+Z 弹回）
    pub undo: Vec<SketchEntity>,
    /// 选中图元（Del 删除）
    pub selected: Option<usize>,
    /// 草图名（完成时建元素用）
    pub name: String,
    /// 网格捕捉开关（默认开；对象捕捉恒开，本开关只控制网格捕捉）
    pub snap_enabled: bool,
    /// 视图缩放（px/mm）
    pub view_scale: f64,
    /// 视图中心（mm，画布中心对应的草图纸面坐标）
    pub view_center: DVec2,
    /// 正多边形边数（内接 / 外切工具参数行；建草稿时复制进草稿）
    pub polygon_sides: usize,
    /// 圆角半径（mm，圆角工具参数行）
    pub fillet_radius: f64,
    /// 倒角距离（mm，倒角工具参数行）
    pub chamfer_distance: f64,
    /// 偏移距离（mm，偏移工具参数行；正 = 左侧 / 向外）
    pub offset_distance: f64,
}

impl SketchEditorState {
    /// 新建一个编辑器状态（预填草图名）。
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            open: true,
            tool: SketchTool::Polyline,
            draft: SketchDraft::new(SketchTool::Polyline),
            entities: Vec::new(),
            undo: Vec::new(),
            selected: None,
            name: name.into(),
            snap_enabled: true,
            view_scale: INITIAL_SCALE,
            view_center: DVec2::new(0.0, 0.0),
            polygon_sides: DEFAULT_POLYGON_SIDES,
            fillet_radius: 10.0,
            chamfer_distance: 10.0,
            offset_distance: 10.0,
        }
    }

    /// 按当前工具 + 参数行新建草稿（正多边形边数从参数取，工具切换 /
    /// 复位 / 清空共用）。
    fn fresh_draft(&self) -> SketchDraft {
        SketchDraft::with_params(self.tool, self.polygon_sides)
    }

    /// 段内撤销：弹出最后一个已提交图元进撤销栈。
    fn undo_one(&mut self) {
        if let Some(e) = self.entities.pop() {
            self.undo.push(e);
            self.selected = None;
        }
    }

    /// 段内重做：从撤销栈恢复最后一个图元。
    fn redo_one(&mut self) {
        if let Some(e) = self.undo.pop() {
            self.entities.push(e);
            self.selected = None;
        }
    }

    /// 删除选中图元（Del）。
    fn delete_selected(&mut self) {
        if let Some(i) = self.selected.take()
            && i < self.entities.len()
        {
            self.entities.remove(i);
        }
    }

    /// 提交一个完成图元（清除重做栈——新动作使 redo 失效，与编辑器惯例一致）。
    fn push_entity(&mut self, entity: SketchEntity) {
        self.entities.push(entity);
        self.undo.clear();
        self.selected = None;
    }
}

/// 编辑器一帧的执行结果（调用方据 [`EditorFrame::Committed`] 做渲染收尾）。
pub enum EditorFrame {
    /// 继续编辑（窗口保持打开）
    KeepOpen,
    /// 草图已提交并关闭（返回 (元素 id, 形状 id)）
    Committed(core_model::ElementId, core_model::ShapeId),
    /// 用户关闭且未提交（窗口丢弃）
    Closed,
}

/// 草图编辑器浮窗（BimApp::update 每帧调用）。
///
/// 状态（`st`）、文档会话（`session`）与日志（`logs`）全部经参数显式传入
/// （不经 BimApp——渲染模块不持全量应用状态，字段可见性单点化在调用方）。
#[must_use]
pub fn run_editor(
    st: &mut SketchEditorState,
    session: &mut CoreSession,
    logs: &mut Vec<String>,
    ctx: &egui::Context,
) -> EditorFrame {
    use egui_phosphor::regular as ph;

    let mut outcome = EditorFrame::KeepOpen;
    let mut open = st.open;
    egui::Window::new("草图编辑器")
        .open(&mut open)
        .default_width(860.0)
        .default_height(620.0)
        .show(ctx, |ui| {
            // —— 工具行一（绘制类 15，换行显示）——
            ui.horizontal_wrapped(|ui| {
                for tool in DRAWING_TOOLS {
                    if ui
                        .selectable_label(st.tool == tool, tool.label())
                        .clicked()
                    {
                        st.tool = tool;
                        st.draft = st.fresh_draft();
                        st.selected = None;
                    }
                }
            });
            // —— 工具行二（编辑 + 标注类 12，换行显示）——
            ui.horizontal_wrapped(|ui| {
                for tool in EDITING_TOOLS {
                    if ui
                        .selectable_label(st.tool == tool, tool.label())
                        .clicked()
                    {
                        st.tool = tool;
                        st.draft = st.fresh_draft();
                        st.selected = None;
                    }
                }
            });
            // —— 操作行（捕捉 + 段内操作 + 工具参数 + 完成）——
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(st.snap_enabled, format!("{} 捕捉", ph::CROSSHAIR))
                    .on_hover_text("网格 / 对象捕捉开关（对象捕捉恒开，此处控制网格步长吸附）")
                    .clicked()
                {
                    st.snap_enabled = !st.snap_enabled;
                }
                ui.separator();
                if ui
                    .button(format!("{} 撤销", ph::ARROW_COUNTER_CLOCKWISE))
                    .clicked()
                {
                    st.undo_one();
                }
                if ui.button(format!("{} 重做", ph::ARROW_CLOCKWISE)).clicked() {
                    st.redo_one();
                }
                if ui.button(format!("{} 删除", ph::TRASH)).clicked() {
                    st.delete_selected();
                }
                if ui.button(format!("{} 清空", ph::BROOM)).clicked() {
                    st.entities.clear();
                    st.undo.clear();
                    st.draft = st.fresh_draft();
                    st.selected = None;
                }
                // —— 工具参数（按当前工具显示 DragValue）——
                match st.tool {
                    SketchTool::PolygonIn | SketchTool::PolygonOut => {
                        ui.separator();
                        ui.label("边数");
                        ui.add(
                            egui::DragValue::new(&mut st.polygon_sides)
                                .range(3..=64)
                                .speed(0.5),
                        );
                        // 进行中草稿同步参数副本。
                        if let SketchDraft::Polygon { sides, .. } = &mut st.draft {
                            *sides = st.polygon_sides.max(3);
                        }
                    }
                    SketchTool::Fillet => {
                        ui.separator();
                        ui.label("圆角半径");
                        ui.add(
                            egui::DragValue::new(&mut st.fillet_radius)
                                .range(0.01..=100000.0)
                                .speed(1.0),
                        );
                    }
                    SketchTool::Chamfer => {
                        ui.separator();
                        ui.label("倒角距离");
                        ui.add(
                            egui::DragValue::new(&mut st.chamfer_distance)
                                .range(0.01..=100000.0)
                                .speed(1.0),
                        );
                    }
                    SketchTool::Offset => {
                        ui.separator();
                        ui.label("偏移距离");
                        ui.add(
                            egui::DragValue::new(&mut st.offset_distance)
                                .range(-100000.0..=100000.0)
                                .speed(1.0),
                        );
                    }
                    _ => {}
                }
                ui.separator();
                ui.add(
                    egui::TextEdit::singleline(&mut st.name)
                        .desired_width(110.0)
                        .hint_text("草图名"),
                );
                if ui
                    .button(format!("{} 完成", ph::CHECK))
                    .on_hover_text("建线成 wire 并落成通用草图元素")
                    .clicked()
                {
                    if st.entities.is_empty() {
                        logs.push("✗ 草图为空：至少画 1 个图元再完成".to_string());
                    } else {
                        match session.build_sketch_profile(&st.name, &st.entities) {
                            Ok(o) => {
                                logs.push(format!("✓ 草图「{}」已落成元素（{o:?}）", st.name));
                                outcome = EditorFrame::Committed(o.element, o.shape);
                            }
                            Err(e) => logs.push(format!("✗ 草图构建失败：{e}")),
                        }
                    }
                }
            });
            ui.separator();

            // —— 画布（事件 → 状态机推进）——
            let ev = draw_canvas(st, ui);
            apply_event(st, logs, ev);

            // —— 状态栏（步骤提示 + 图元数 + 光标坐标）——
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(st.draft.hint());
                ui.separator();
                ui.label(format!(
                    "{} 图元 / {} 可重做 · Esc 取消 · Backspace 撤点 · Del 删除 · Ctrl+Z/Y 撤销/重做 · Ctrl+单击选中",
                    st.entities.len(),
                    st.undo.len()
                ));
            });
        });
    st.open = open;
    if matches!(outcome, EditorFrame::Committed(..)) {
        // 提交成功：窗口随之一并关闭（egui 的 .open(&mut) 借用横跨闭包，
        // 闭包内写不进去——这里统一收口）。
        st.open = false;
    }
    if !st.open && matches!(outcome, EditorFrame::KeepOpen) {
        // × 关闭且未提交：丢弃窗口（调用方把字段置 None）。
        return EditorFrame::Closed;
    }
    outcome
}

/// 把画布事件推进到工具状态机（渲染层与纯逻辑层的唯一衔接点）。
fn apply_event(st: &mut SketchEditorState, logs: &mut Vec<String>, ev: CanvasEvent) {
    match ev {
        CanvasEvent::None => {}
        CanvasEvent::Click(p) => {
            // 圆角 / 倒角：单击拾取线段图元（选图元不选点，不走点状态机）。
            if matches!(
                st.draft,
                SketchDraft::Fillet { .. } | SketchDraft::Chamfer { .. }
            ) {
                let tol = SNAP_TOL_PX / st.view_scale;
                let picked = pick_corner_segment(&st.entities, p, tol);
                if let CornerPick::Both(a, b) = st.draft.pick_corner(picked) {
                    let label = st.tool.label();
                    let result = if matches!(st.tool, SketchTool::Fillet) {
                        apply_fillet(&mut st.entities, a, b, st.fillet_radius)
                    } else {
                        apply_chamfer(&mut st.entities, a, b, st.chamfer_distance)
                    };
                    match result {
                        Ok(()) => logs.push(format!("✓ {label} 完成")),
                        Err(e) => logs.push(format!("✗ {label} 失败：{e}")),
                    }
                    st.draft = st.fresh_draft();
                }
            } else if st.draft.is_editing() {
                let tol = SNAP_TOL_PX / st.view_scale;
                let picked = pick_entity_at(&st.entities, p, tol);
                if matches!(st.draft, SketchDraft::Offset) {
                    // 偏移：单击拾取即按参数行距离成新图元（原图元不动）。
                    if let Some(i) = picked {
                        let label = st.tool.label();
                        match offset_entity(&st.entities[i], st.offset_distance) {
                            Ok(e) => {
                                st.push_entity(e);
                                logs.push(format!("✓ {label} 完成"));
                            }
                            Err(e) => logs.push(format!("✗ {label} 失败：{e}")),
                        }
                    }
                } else {
                    // 其余编辑 / 标注：统一点击推进（拾取阶段用通用拾取）。
                    match st.draft.edit_click(p, picked, &st.entities) {
                        EditOutcome::Applied { replace, add } => {
                            if let Some((i, e)) = replace
                                && i < st.entities.len()
                            {
                                st.entities[i] = e;
                                st.undo.clear(); // 新动作使 redo 失效
                                st.selected = None;
                            }
                            for e in add {
                                st.push_entity(e);
                            }
                            // 交互完成草稿已复位为 None：重启同工具继续。
                            if matches!(st.draft, SketchDraft::None) {
                                st.draft = st.fresh_draft();
                            }
                        }
                        EditOutcome::Failed(msg) => {
                            logs.push(format!("✗ {} 失败：{msg}", st.tool.label()));
                            st.draft = st.fresh_draft();
                        }
                        EditOutcome::Continued | EditOutcome::Ignored => {}
                    }
                }
            } else {
                match st.draft.click(p) {
                    ClickOutcome::Finished(entity) => {
                        st.push_entity(entity);
                        st.draft = st.fresh_draft();
                    }
                    ClickOutcome::Continued => {}
                }
            }
        }
        CanvasEvent::DoubleClick => {
            if let Some(entity) = st.draft.finish_open() {
                st.push_entity(entity);
                st.draft = st.fresh_draft();
            }
        }
        CanvasEvent::Escape => {
            st.draft.abort();
        }
        CanvasEvent::Backspace => {
            st.draft.backtrack();
        }
        CanvasEvent::Delete => st.delete_selected(),
        CanvasEvent::Undo => st.undo_one(),
        CanvasEvent::Redo => st.redo_one(),
    }
}

/// 一帧画布事件（mm 坐标已按捕捉 / 正交处理）。
enum CanvasEvent {
    None,
    /// 单击（mm，已捕捉 / 正交）
    Click(DVec2),
    /// 双击（结束折线；egui 的 double_clicked 蕴含 clicked——载荷无独立用途）
    DoubleClick,
    Escape,
    Backspace,
    Delete,
    Undo,
    Redo,
}

/// 画一帧草图画布：网格 → 已提交实体 → 进行中预览；收集交互事件。
///
/// 坐标换算：mm → px 等比缩放 + y 翻转；滚轮缩放（视点定标在光标处）、
/// 中键 / 右键拖动平移。
fn draw_canvas(st: &mut SketchEditorState, ui: &mut egui::Ui) -> CanvasEvent {
    let (rect, response) =
        ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
    let painter = ui.painter().with_clip_rect(rect);

    // —— 视图变换（mm ↔ px；y 翻转；只读 view_center / view_scale）——
    let scale = st.view_scale;
    let center_mm = st.view_center;
    let center = rect.center();
    let to_px = |p: DVec2| {
        egui::Pos2::new(
            (center.x as f64 + (p.x - center_mm.x) * scale) as f32,
            (center.y as f64 - (p.y - center_mm.y) * scale) as f32,
        )
    };
    let to_mm = |q: egui::Pos2| {
        DVec2::new(
            center_mm.x + (q.x as f64 - center.x as f64) / scale,
            center_mm.y - (q.y as f64 - center.y as f64) / scale,
        )
    };

    // —— 平移（中键 / 右键拖动）——
    if response.dragged_by(egui::PointerButton::Middle)
        || response.dragged_by(egui::PointerButton::Secondary)
    {
        let d = response.drag_delta();
        st.view_center.x -= d.x as f64 / scale;
        st.view_center.y += d.y as f64 / scale; // y 翻转：拖右＝纸面左移
    }
    // —— 缩放（滚轮，视点定标在光标处）——
    let raw_mm = to_mm(response.hover_pos().unwrap_or(center));
    if response.hovered() {
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll != 0.0 {
            let anchor = raw_mm;
            st.view_scale =
                (st.view_scale * (f64::from(scroll) * 0.002).exp()).clamp(SCALE_MIN, SCALE_MAX);
            // 缩放后把锚点拉回光标处（视点定标：光标下的纸面点不漂移）。
            let new_mm = to_mm(response.hover_pos().unwrap_or(center));
            st.view_center += anchor - new_mm;
        }
    }

    // —— 网格 ——
    draw_grid_lines(&painter, &rect, st, &to_px);

    // —— 已提交实体（选中高亮 + 编辑 / 标注拾取高亮）——
    let edit_pick = st.draft.edit_highlight();
    for (i, e) in st.entities.iter().enumerate() {
        let highlighted = st.selected == Some(i) || edit_pick == Some(i);
        let color = if highlighted {
            egui::Color32::from_rgb(0xff, 0xd0, 0x40)
        } else if matches!(
            e,
            SketchEntity::DimLinear { .. }
                | SketchEntity::DimRadius { .. }
                | SketchEntity::DimAngle { .. }
        ) {
            // 标注（显示型）：专属青蓝底色，与几何图元区分。
            DIM_COLOR
        } else {
            egui::Color32::from_rgb(0xe0, 0xe0, 0xe0)
        };
        draw_entity(&painter, e, &to_px, scale, color);
    }

    // —— 光标 mm 坐标（捕捉 / 正交处理后的画布输入）——
    let shift = ui.input(|i| i.modifiers.shift);
    let mut mm = raw_mm;
    if st.snap_enabled {
        // 对象捕捉优先（端点 / 中点 / 中心），退回网格捕捉。
        let tol = SNAP_TOL_PX / scale;
        mm = snap_entities(&st.entities, raw_mm, tol)
            .unwrap_or_else(|| snap_grid(raw_mm, GRID_STEP_MM));
    }
    if shift {
        // 正交（Shift 临时）：相对上一个锚点锁 0/90°。
        if let Some(prev) = anchor_of(&st.draft).or_else(|| last_point_of(&st.entities)) {
            mm = orthogonalize(prev, mm);
        }
    }

    // —— 进行中预览 ——
    draw_preview(&painter, &st.draft, &st.entities, mm, &to_px, scale);

    // —— 事件收集（Ctrl+单击 = 选中附近图元，不画）——
    let cmd = ui.input(|i| i.modifiers.command);
    // 双击先于单击判（egui 0.29：double_clicked 蕴含 clicked——若先查单击，
    // 双击的第二击会被当"单击加点"，折线双击结束时多放一个点）。
    let mut ev = if response.double_clicked() {
        CanvasEvent::DoubleClick
    } else if response.clicked() {
        if cmd {
            st.selected = pick_entity_at(&st.entities, mm, SNAP_TOL_PX / scale);
            CanvasEvent::None
        } else {
            CanvasEvent::Click(mm)
        }
    } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        CanvasEvent::Escape
    } else if ui.input(|i| i.key_pressed(egui::Key::Backspace)) {
        CanvasEvent::Backspace
    } else if ui.input(|i| i.key_pressed(egui::Key::Delete)) {
        CanvasEvent::Delete
    } else if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Z)) {
        CanvasEvent::Undo
    } else if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Y)) {
        CanvasEvent::Redo
    } else {
        CanvasEvent::None
    };
    // 键盘事件只认画布获得焦点时的悬停（避免全局热键抢走文档快捷键）。
    if !matches!(ev, CanvasEvent::Click(_) | CanvasEvent::DoubleClick) && !response.hovered() {
        ev = CanvasEvent::None;
    }
    ev
}

/// 画布网格：细格（GRID_STEP_MM）+ 主格（GRID_MAJOR_EVERY 倍）+ 原点轴。
fn draw_grid_lines(
    painter: &egui::Painter,
    rect: &egui::Rect,
    st: &SketchEditorState,
    to_px: &dyn Fn(DVec2) -> egui::Pos2,
) {
    let fine = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(0x38, 0x3d, 0x47));
    let major = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(0x4c, 0x53, 0x60));
    let scale = st.view_scale;
    let half_w = rect.width() as f64 / 2.0 / scale;
    let half_h = rect.height() as f64 / 2.0 / scale;
    let (x0, x1) = (st.view_center.x - half_w, st.view_center.x + half_w);
    let (y0, y1) = (st.view_center.y - half_h, st.view_center.y + half_h);
    let step = GRID_STEP_MM;
    let major_step = GRID_MAJOR_EVERY as f64 * step;
    let mut x = (x0 / step).floor() * step;
    while x <= x1 {
        let stroke = if (x / major_step).round() * major_step == x {
            major
        } else {
            fine
        };
        painter.line_segment([to_px(DVec2::new(x, y0)), to_px(DVec2::new(x, y1))], stroke);
        x += step;
    }
    let mut y = (y0 / step).floor() * step;
    while y <= y1 {
        let stroke = if (y / major_step).round() * major_step == y {
            major
        } else {
            fine
        };
        painter.line_segment([to_px(DVec2::new(x0, y)), to_px(DVec2::new(x1, y))], stroke);
        y += step;
    }
    // 原点轴（X 红 / Y 绿）。
    painter.line_segment(
        [to_px(DVec2::new(x0, 0.0)), to_px(DVec2::new(x1, 0.0))],
        egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(0xb0, 0x58, 0x58)),
    );
    painter.line_segment(
        [to_px(DVec2::new(0.0, y0)), to_px(DVec2::new(0.0, y1))],
        egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(0x58, 0xa8, 0x64)),
    );
}

/// 画一个已提交图元（圆走 egui 真圆；弧 / 椭圆采样 64 段——egui 无弧 primitive）。
fn draw_entity(
    painter: &egui::Painter,
    e: &SketchEntity,
    to_px: &dyn Fn(DVec2) -> egui::Pos2,
    scale: f64,
    color: egui::Color32,
) {
    const SEGMENTS: usize = 64;
    let stroke = egui::Stroke::new(1.5_f32, color);
    match e {
        SketchEntity::Polyline(points) => {
            let pts: Vec<egui::Pos2> = points.iter().map(|p| to_px(*p)).collect();
            if pts.len() >= 2 {
                painter.add(egui::Shape::line(pts, stroke));
            }
        }
        SketchEntity::Circle { center, radius } => {
            // 解析圆走 egui 真圆（circle_stroke）——"圆是真圆"沿用 M11 验收线。
            painter.circle_stroke(to_px(*center), ((*radius * scale).max(0.5)) as f32, stroke);
        }
        SketchEntity::Ellipse {
            center,
            major,
            minor,
            major_angle,
        } => {
            let (c, s) = major_angle.sin_cos();
            let pts: Vec<egui::Pos2> = (0..=SEGMENTS)
                .map(|i| {
                    let t = std::f64::consts::TAU * (i as f64) / (SEGMENTS as f64);
                    let local = DVec2::new(major * t.cos(), minor * t.sin());
                    // 主轴旋转（ccw）：(x·cosθ − y·sinθ, x·sinθ + y·cosθ)。
                    to_px(
                        *center + DVec2::new(local.x * c - local.y * s, local.x * s + local.y * c),
                    )
                })
                .collect();
            painter.add(egui::Shape::line(pts, stroke));
        }
        SketchEntity::Arc {
            center,
            radius,
            start_angle,
            end_angle,
        } => {
            let (sa, ea) = core_app::normalize_arc_angles(*start_angle, *end_angle);
            let pts: Vec<egui::Pos2> = (0..=SEGMENTS)
                .map(|i| {
                    let t = sa + (ea - sa) * (i as f64) / (SEGMENTS as f64);
                    to_px(DVec2::new(
                        center.x + radius * t.cos(),
                        center.y + radius * t.sin(),
                    ))
                })
                .collect();
            painter.add(egui::Shape::line(pts, stroke));
        }
        SketchEntity::Bezier(segments) => {
            // 每段 n 阶贝塞尔精确采样（de Casteljau）——提交 1 的控制多边形
            // 折线显示升级为曲线。
            for seg in segments {
                let pts: Vec<egui::Pos2> = bezier_sample_polyline(seg, 32)
                    .iter()
                    .map(|p| to_px(*p))
                    .collect();
                if pts.len() >= 2 {
                    painter.add(egui::Shape::line(pts, stroke));
                }
            }
        }
        SketchEntity::Point(p) => {
            // 辅助顶点：小实心圆点（不进 wire，只在画布可见）。
            painter.circle_filled(to_px(*p), 3.0_f32, color);
        }
        SketchEntity::AuxLine { from, to, .. } => {
            // 构造线 / 射线：虚线显示（epaint 的 dashed_line——egui crate 无此
            // API，经 egui::Shape re-export 调用）。
            let shapes =
                egui::Shape::dashed_line(&[to_px(*from), to_px(*to)], stroke, 6.0_f32, 4.0_f32);
            painter.extend(shapes);
        }
        SketchEntity::DimLinear {
            from,
            to,
            pos,
            linear,
        } => {
            draw_dim_linear(painter, to_px, *from, *to, *pos, *linear, color);
        }
        SketchEntity::DimRadius {
            center,
            radius,
            pos,
        } => {
            draw_dim_radius(painter, to_px, *center, *radius, *pos, color);
        }
        SketchEntity::DimAngle {
            vertex,
            arm1,
            arm2,
            pos,
        } => {
            draw_dim_angle(painter, to_px, *vertex, *arm1, *arm2, *pos, color);
        }
    }
}

/// 线性 / 对齐标注渲染（G4-M17 显示型）：尺寸线（线性为轴对齐过 pos、对齐
/// 为平行 from→to 过 pos）+ 两端延长线 + 两端 45° tick + 距离数字。
fn draw_dim_linear(
    painter: &egui::Painter,
    to_px: &dyn Fn(DVec2) -> egui::Pos2,
    from: DVec2,
    to: DVec2,
    pos: DVec2,
    linear: bool,
    color: egui::Color32,
) {
    let stroke = egui::Stroke::new(1.2_f32, color);
    let value = if linear {
        // 轴对齐增量：|dx| ≥ |dy| 取 dx，否则 dy。
        if (to.x - from.x).abs() >= (to.y - from.y).abs() {
            to.x - from.x
        } else {
            to.y - from.y
        }
    } else {
        (to - from).length()
    };
    let (p1, p2) = if linear {
        if (to.x - from.x).abs() >= (to.y - from.y).abs() {
            (DVec2::new(from.x, pos.y), DVec2::new(to.x, pos.y))
        } else {
            (DVec2::new(pos.x, from.y), DVec2::new(pos.x, to.y))
        }
    } else {
        // 对齐：尺寸线平行 from→to、过 pos——两端点沿法向平移到尺寸线。
        let d = to - from;
        let len2 = d.length_squared();
        if len2 <= f64::EPSILON {
            (from, to)
        } else {
            let n = DVec2::new(-d.y, d.x) / len2.sqrt();
            let off = n.dot(pos - from);
            let shift = |q: DVec2| q + n * off;
            (shift(from), shift(to))
        }
    };
    // 延长线（几何点 → 尺寸线端点）。
    painter.line_segment([to_px(from), to_px(p1)], stroke);
    painter.line_segment([to_px(to), to_px(p2)], stroke);
    // 尺寸线。
    painter.line_segment([to_px(p1), to_px(p2)], stroke);
    // 两端 45° tick（尺寸方向与左法向的角平分方向，屏面 4px）。
    let qa = to_px(p1);
    let qb = to_px(p2);
    let d = qb - qa;
    let len = d.length();
    let dir = if len > 1e-6 { d / len } else { egui::Vec2::X };
    let bisector = dir + egui::Vec2::new(-dir.y, dir.x);
    let bis_len = bisector.length();
    let bis = if bis_len > 1e-6 {
        bisector / bis_len
    } else {
        egui::Vec2::new(1.0, 1.0)
    };
    for q in [qa, qb] {
        let dv = bis * 4.0_f32;
        painter.line_segment([q - dv, q + dv], stroke);
    }
    // 距离数字（尺寸线中点上方）。
    let text = format!("{:.1}", value.abs());
    let mid = qa + (qb - qa) * 0.5;
    painter.text(
        mid,
        egui::Align2::CENTER_BOTTOM,
        &text,
        egui::FontId::proportional(DIM_FONT_SIZE),
        color,
    );
}

/// 半径标注渲染：引线（标注位置 → 圆周）+ R 数字。
fn draw_dim_radius(
    painter: &egui::Painter,
    to_px: &dyn Fn(DVec2) -> egui::Pos2,
    center: DVec2,
    radius: f64,
    pos: DVec2,
    color: egui::Color32,
) {
    let stroke = egui::Stroke::new(1.2_f32, color);
    // 引线：沿 pos−center 方向从标注位置指向圆周。
    let dir = (pos - center).normalize_or_zero();
    let rim = center + dir * radius;
    painter.line_segment([to_px(pos), to_px(rim)], stroke);
    // 数字：R + 半径（标注位置处）。
    let text = format!("R{:.1}", radius);
    painter.text(
        to_px(pos),
        egui::Align2::LEFT_CENTER,
        &text,
        egui::FontId::proportional(DIM_FONT_SIZE),
        color,
    );
}

/// 角度标注渲染：角弧（取含标注位置一侧）+ 度数数字。
fn draw_dim_angle(
    painter: &egui::Painter,
    to_px: &dyn Fn(DVec2) -> egui::Pos2,
    vertex: DVec2,
    arm1: DVec2,
    arm2: DVec2,
    pos: DVec2,
    color: egui::Color32,
) {
    const SEGMENTS: usize = 48;
    let stroke = egui::Stroke::new(1.2_f32, color);
    // 角弧半径由标注位置定。
    let r = (pos - vertex).length();
    if r <= f64::EPSILON {
        return;
    }
    let a1 = (arm1.y - vertex.y).atan2(arm1.x - vertex.x);
    let a2 = (arm2.y - vertex.y).atan2(arm2.x - vertex.x);
    let a_pos = (pos.y - vertex.y).atan2(pos.x - vertex.x);
    // 角弧取含 pos 一侧：ccw 扫掠（a1→a2）中点与 pos 方向夹角 ≤ π 则取 ccw，
    // 否则取 cw（从 a2 扫回 a1，扫角 = 2π − ccw 扫角）。
    let span_ccw = (a2 - a1).rem_euclid(std::f64::consts::TAU);
    let mid_off = (a_pos - (a1 + span_ccw * 0.5)).rem_euclid(std::f64::consts::TAU);
    let (from_a, span) = if mid_off <= std::f64::consts::PI {
        (a1, span_ccw)
    } else {
        (a2, std::f64::consts::TAU - span_ccw)
    };
    if span <= f64::EPSILON {
        return;
    }
    let pts: Vec<egui::Pos2> = (0..=SEGMENTS)
        .map(|i| {
            let t = from_a + span * (i as f64) / (SEGMENTS as f64);
            to_px(DVec2::new(vertex.x + r * t.cos(), vertex.y + r * t.sin()))
        })
        .collect();
    painter.add(egui::Shape::line(pts, stroke));
    // 度数（标注位置处）。
    let text = format!("{:.1}°", span.to_degrees());
    painter.text(
        to_px(pos),
        egui::Align2::CENTER_CENTER,
        &text,
        egui::FontId::proportional(DIM_FONT_SIZE),
        color,
    );
}

/// 进行中预览（按工具阶段画锚点与橡皮筋；编辑类画变换幽灵，标注画实时数值）。
fn draw_preview(
    painter: &egui::Painter,
    draft: &SketchDraft,
    entities: &[SketchEntity],
    cursor: DVec2,
    to_px: &dyn Fn(DVec2) -> egui::Pos2,
    scale: f64,
) {
    let preview_color = egui::Color32::from_rgb(0x60, 0xa0, 0xff);
    let stroke = egui::Stroke::new(((scale * 0.6) as f32).clamp(1.0, 2.0), preview_color);
    let gray = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(0x55, 0x60, 0x70));
    let dot = |p: DVec2| {
        painter.circle_filled(to_px(p), 3.0, preview_color);
    };
    match draft {
        SketchDraft::None => {}
        SketchDraft::Polyline { points } => {
            for p in points {
                dot(*p);
            }
            if let Some(last) = points.last() {
                painter.line_segment([to_px(*last), to_px(cursor)], stroke);
            }
        }
        SketchDraft::Rect { first } => {
            if let Some(f) = first {
                dot(*f);
                // 对角预览矩形。
                let tl = to_px(*f);
                let br = to_px(cursor);
                let tr = egui::Pos2::new(br.x, tl.y);
                let bl = egui::Pos2::new(tl.x, br.y);
                painter.line_segment([tl, tr], stroke);
                painter.line_segment([tr, br], stroke);
                painter.line_segment([br, bl], stroke);
                painter.line_segment([bl, tl], stroke);
            }
        }
        SketchDraft::Circle { center } => {
            if let Some(c) = center {
                dot(*c);
                painter.line_segment([to_px(*c), to_px(cursor)], stroke);
                let r = (cursor - *c).length();
                let pts: Vec<egui::Pos2> = (0..=48)
                    .map(|i| {
                        let t = std::f64::consts::TAU * (i as f64) / 48.0;
                        to_px(DVec2::new(c.x + r * t.cos(), c.y + r * t.sin()))
                    })
                    .collect();
                painter.add(egui::Shape::line(pts, stroke));
            }
        }
        SketchDraft::Ellipse { center, axis_end } => match (*center, *axis_end) {
            (Some(c), None) => {
                dot(c);
                painter.line_segment([to_px(c), to_px(cursor)], stroke);
            }
            (Some(c), Some(a)) => {
                dot(c);
                dot(a);
                painter.line_segment([to_px(c), to_px(a)], stroke);
                painter.line_segment([to_px(a), to_px(cursor)], stroke);
            }
            _ => {}
        },
        SketchDraft::Arc { center, start } => match (*center, *start) {
            (Some(c), None) => {
                dot(c);
                painter.line_segment([to_px(c), to_px(cursor)], stroke);
            }
            (Some(c), Some(s)) => {
                dot(c);
                dot(s);
                painter.line_segment([to_px(c), to_px(s)], stroke);
                painter.line_segment([to_px(c), to_px(cursor)], stroke);
                let r = (s - c).length();
                let sa = (s.y - c.y).atan2(s.x - c.x);
                let ea = (cursor.y - c.y).atan2(cursor.x - c.x);
                let (sa, ea) = core_app::normalize_arc_angles(sa, ea);
                if (ea - sa).abs() > f64::EPSILON {
                    let pts: Vec<egui::Pos2> = (0..=48)
                        .map(|i| {
                            let t = sa + (ea - sa) * (i as f64) / 48.0;
                            to_px(DVec2::new(c.x + r * t.cos(), c.y + r * t.sin()))
                        })
                        .collect();
                    painter.add(egui::Shape::line(pts, stroke));
                }
            }
            _ => {}
        },
        SketchDraft::Polygon {
            center,
            sides,
            circumscribed,
        } => {
            if let Some(c) = center {
                dot(*c);
                painter.line_segment([to_px(*c), to_px(cursor)], stroke);
                // 预览多边形（含光标的顶点 / 边中点定大小）。
                let verts = polygon_vertices(*c, cursor, *sides, *circumscribed);
                if !verts.is_empty() {
                    let pts: Vec<egui::Pos2> = verts.iter().map(|p| to_px(*p)).collect();
                    painter.add(egui::Shape::line(pts, stroke));
                }
            }
        }
        SketchDraft::Arc3 { start, mid } => match (*start, *mid) {
            (Some(s), None) => {
                dot(s);
                painter.line_segment([to_px(s), to_px(cursor)], stroke);
            }
            (Some(s), Some(m)) => {
                dot(s);
                dot(m);
                painter.line_segment([to_px(m), to_px(cursor)], stroke);
                // 预览弧：过三点的整圆采样（共线时无预览）。
                if let Some((c, r)) = circle_through_three(s, m, cursor) {
                    let pts: Vec<egui::Pos2> = (0..=48)
                        .map(|i| {
                            let t = std::f64::consts::TAU * (i as f64) / 48.0;
                            to_px(DVec2::new(c.x + r * t.cos(), c.y + r * t.sin()))
                        })
                        .collect();
                    painter.add(egui::Shape::line(pts, stroke));
                }
            }
            _ => {}
        },
        SketchDraft::Bezier { points } => {
            for p in points {
                dot(*p);
            }
            // 预览曲线（含光标的 n 阶采样）+ 细控制多边形。
            let mut controls: Vec<DVec2> = points.clone();
            controls.push(cursor);
            if controls.len() >= 2 {
                let pts: Vec<egui::Pos2> = bezier_sample_polyline(&controls, 32)
                    .iter()
                    .map(|p| to_px(*p))
                    .collect();
                painter.add(egui::Shape::line(pts, stroke));
                let ctrl: Vec<egui::Pos2> = controls.iter().map(|p| to_px(*p)).collect();
                painter.add(egui::Shape::line(
                    ctrl,
                    egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(0x55, 0x60, 0x70)),
                ));
            }
        }
        SketchDraft::Spline { points } => {
            for p in points {
                dot(*p);
            }
            // 预览样条（含光标的 Catmull-Rom 拟合采样）。
            let mut fits: Vec<DVec2> = points.clone();
            fits.push(cursor);
            if fits.len() >= 2 {
                for seg in catmull_rom_to_bezier(&fits) {
                    let pts: Vec<egui::Pos2> = bezier_sample_polyline(&seg, 16)
                        .iter()
                        .map(|p| to_px(*p))
                        .collect();
                    painter.add(egui::Shape::line(pts, stroke));
                }
            }
        }
        SketchDraft::Point => {} // 单击即放置，无预览
        SketchDraft::Construction { first } => {
            if let Some(f) = first {
                dot(*f);
                painter.line_segment([to_px(*f), to_px(cursor)], stroke);
            }
        }
        SketchDraft::Ray { start } => {
            if let Some(s) = start {
                dot(*s);
                painter.line_segment([to_px(*s), to_px(cursor)], stroke);
            }
        }
        // 圆角 / 倒角：选图元高亮在画布实体层做（edit_highlight），无橡皮筋预览。
        SketchDraft::Fillet { .. } | SketchDraft::Chamfer { .. } => {}
        // —— 编辑 / 标注类预览（G4-M17）——
        SketchDraft::Move { target, base } | SketchDraft::Copy { target, base } => {
            if let (Some(i), Some(b)) = (*target, *base)
                && let Some(e) = entities.get(i)
            {
                // 橡皮筋（基点 → 光标）+ 平移幽灵。
                painter.line_segment([to_px(b), to_px(cursor)], stroke);
                let ghost = translate_entity(e, cursor - b);
                draw_entity(painter, &ghost, to_px, scale, preview_color);
            }
        }
        SketchDraft::Rotate {
            target,
            base,
            reference,
        }
        | SketchDraft::Scale {
            target,
            base,
            reference,
        } => {
            if let (Some(i), Some(b)) = (*target, *base)
                && let Some(e) = entities.get(i)
            {
                match *reference {
                    None => {
                        painter.line_segment([to_px(b), to_px(cursor)], stroke);
                    }
                    Some(r) => {
                        // 参考线（灰）+ 当前角 / 比例橡皮筋 + 变换幽灵。
                        painter.line_segment([to_px(b), to_px(r)], gray);
                        painter.line_segment([to_px(b), to_px(cursor)], stroke);
                        let is_rotate = matches!(draft, SketchDraft::Rotate { .. });
                        let ghost = if is_rotate {
                            let angle = (cursor - b).y.atan2((cursor - b).x)
                                - (r - b).y.atan2((r - b).x);
                            rotate_entity(e, b, angle)
                        } else {
                            let d_ref = (r - b).length();
                            if d_ref <= f64::EPSILON || (cursor - b).length() <= f64::EPSILON {
                                e.clone()
                            } else {
                                scale_entity(e, b, (cursor - b).length() / d_ref)
                            }
                        };
                        draw_entity(painter, &ghost, to_px, scale, preview_color);
                    }
                }
            }
        }
        SketchDraft::Mirror { target, first } => {
            if let (Some(i), Some(f)) = (*target, *first)
                && let Some(e) = entities.get(i)
            {
                // 镜像线（虚线）+ 镜像幽灵。
                let shapes =
                    egui::Shape::dashed_line(&[to_px(f), to_px(cursor)], stroke, 6.0_f32, 4.0_f32);
                painter.extend(shapes);
                let ghost = mirror_entity(e, f, cursor);
                draw_entity(painter, &ghost, to_px, scale, preview_color);
            }
        }
        // 偏移 / 打断：无状态单击即成，无预览（偏移高亮在实体层）。
        SketchDraft::Offset | SketchDraft::Break => {}
        // 修剪：边界高亮在实体层做。
        SketchDraft::Trim { .. } => {}
        SketchDraft::DimLinear { first, second } => match (*first, *second) {
            (Some(f), None) => {
                dot(f);
                // 实时尺寸预览（to = pos = 光标）。
                draw_dim_linear(painter, to_px, f, cursor, cursor, true, preview_color);
            }
            (Some(f), Some(s)) => {
                dot(f);
                dot(s);
                painter.line_segment([to_px(s), to_px(cursor)], stroke);
                // 尺寸线跟随光标放置。
                draw_dim_linear(painter, to_px, f, s, cursor, true, preview_color);
            }
            _ => {}
        },
        SketchDraft::DimAlign { first, second } => match (*first, *second) {
            (Some(f), None) => {
                dot(f);
                draw_dim_linear(painter, to_px, f, cursor, cursor, false, preview_color);
            }
            (Some(f), Some(s)) => {
                dot(f);
                dot(s);
                painter.line_segment([to_px(s), to_px(cursor)], stroke);
                draw_dim_linear(painter, to_px, f, s, cursor, false, preview_color);
            }
            _ => {}
        },
        SketchDraft::DimRadius { circle } => {
            if let Some((c, r)) = *circle {
                dot(c);
                // 引线 + R 数字跟随光标。
                draw_dim_radius(painter, to_px, c, r, cursor, preview_color);
            }
        }
        SketchDraft::DimAngle { first, second } => {
            if let (Some(i1), Some(i2)) = (*first, *second)
                && let (Some(SketchEntity::Polyline(a)), Some(SketchEntity::Polyline(b))) =
                    (entities.get(i1), entities.get(i2))
                && a.len() == 2
                && b.len() == 2
                && let Some(v) = segment_intersection(a[0], a[1], b[0], b[1])
            {
                // 交点 + 两臂（远端）+ 光标定半径的实时角弧。
                let arm1 = if v.distance(a[0]) >= v.distance(a[1]) {
                    a[0]
                } else {
                    a[1]
                };
                let arm2 = if v.distance(b[0]) >= v.distance(b[1]) {
                    b[0]
                } else {
                    b[1]
                };
                draw_dim_angle(painter, to_px, v, arm1, arm2, cursor, preview_color);
            }
        }
    }
}

/// 画布草稿的当前锚点（正交约束的参照；无锚点 None）。
fn anchor_of(draft: &SketchDraft) -> Option<DVec2> {
    match draft {
        SketchDraft::Polyline { points } => points.last().copied(),
        SketchDraft::Rect { first } => *first,
        SketchDraft::Circle { center } => *center,
        SketchDraft::Ellipse { center, axis_end } => axis_end.or(*center),
        SketchDraft::Arc { center, start } => start.or(*center),
        SketchDraft::Polygon { center, .. } => *center,
        SketchDraft::Arc3 { start, mid } => mid.or(*start),
        SketchDraft::Bezier { points } | SketchDraft::Spline { points } => points.last().copied(),
        SketchDraft::Construction { first } | SketchDraft::Ray { start: first } => *first,
        // 编辑类：正交参照取最近放置的点（基点 / 参考点 / 镜像线点 / 标注点）。
        SketchDraft::Move { base, .. } | SketchDraft::Copy { base, .. } => *base,
        SketchDraft::Rotate {
            base, reference, ..
        }
        | SketchDraft::Scale {
            base, reference, ..
        } => reference.or(*base),
        SketchDraft::Mirror { first, .. } => *first,
        SketchDraft::DimLinear { first, second } | SketchDraft::DimAlign { first, second } => {
            second.or(*first)
        }
        SketchDraft::Offset
        | SketchDraft::Trim { .. }
        | SketchDraft::Break
        | SketchDraft::DimRadius { .. }
        | SketchDraft::DimAngle { .. }
        | SketchDraft::Point
        | SketchDraft::Fillet { .. }
        | SketchDraft::Chamfer { .. }
        | SketchDraft::None => None,
    }
}

/// 已提交图元的最后一个点（正交约束的次级参照）。
fn last_point_of(entities: &[SketchEntity]) -> Option<DVec2> {
    match entities.last() {
        Some(SketchEntity::Polyline(points)) => points.last().copied(),
        Some(SketchEntity::Circle { center, .. })
        | Some(SketchEntity::Ellipse { center, .. })
        | Some(SketchEntity::Arc { center, .. }) => Some(*center),
        Some(SketchEntity::Bezier(segments)) => segments.last().and_then(|seg| seg.last().copied()),
        Some(SketchEntity::Point(p)) => Some(*p),
        Some(SketchEntity::AuxLine { to, .. }) => Some(*to),
        _ => None,
    }
}

/// 选中拾取：mm 点附近（tol）**最近**的图元下标。
fn pick_entity_at(entities: &[SketchEntity], p: DVec2, tol: f64) -> Option<usize> {
    let mut best: Option<(f64, usize)> = None;
    for (i, e) in entities.iter().enumerate() {
        let d = match e {
            SketchEntity::Polyline(points) => points
                .windows(2)
                .map(|w| point_seg_distance(p, w[0], w[1]))
                .chain(points.iter().map(|v| (*v - p).length()))
                .fold(f64::INFINITY, f64::min),
            SketchEntity::Circle { center, radius } => ((p - *center).length() - *radius).abs(),
            SketchEntity::Ellipse { center, .. } | SketchEntity::Arc { center, .. } => {
                (p - *center).length()
            }
            SketchEntity::Bezier(segments) => {
                // 控制点全量取最近（含段端点——端点即锚点，拾取价值最高）。
                segments
                    .iter()
                    .flat_map(|seg| seg.iter())
                    .map(|v| (*v - p).length())
                    .fold(f64::INFINITY, f64::min)
            }
            SketchEntity::Point(v) => (*v - p).length(),
            SketchEntity::AuxLine { from, to, .. } => point_seg_distance(p, *from, *to),
            // 标注（显示型）：按标注位置点（数字处）拾取。
            SketchEntity::DimLinear { pos, .. }
            | SketchEntity::DimRadius { pos, .. }
            | SketchEntity::DimAngle { pos, .. } => (p - *pos).length(),
        };
        if d <= tol && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, i));
        }
    }
    best.map(|(_, i)| i)
}

/// 点到线段距离（本地小助手，与 `sketch_geom::point_segment_distance` 同口径）。
fn point_seg_distance(p: DVec2, a: DVec2, b: DVec2) -> f64 {
    let ab = b - a;
    let len2 = ab.dot(ab);
    if len2 <= f64::EPSILON {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    (a + ab * t - p).length()
}

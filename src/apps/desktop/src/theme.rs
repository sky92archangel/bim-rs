//! 视觉主题（GUI 计划 §3，G1 视觉基建）——深色专业 CAD 风配色板 + egui 定制。
//!
//! **零逻辑**（G1 验收约束）：本模块只管视觉——配色板、`Visuals` 覆写、字体装载
//! （CJK 兜底 + Phosphor 图标叠加），不改任何建模 / 会话逻辑。
//!
//! 字体优先级（Proportional / Monospace 家族的 fallback 顺序）：
//! `默认拉丁字体 → Phosphor 图标 → CJK 兜底`——英文/图标先命中，中文落到系统字体。
//! 一个 CJK 字体都没找到（非 Windows）时中文显示为方框，但不影响功能（不 panic）。

use egui::{Color32, Context, FontData, FontDefinitions, Rounding, Stroke, Visuals};

// ---- 配色板（计划 §3.1，用户定版深色 CAD 风）--------------------------------
// 部分常量在 G1 尚未被 visuals 引用（状态栏/日志着色/视口底色是 G2+ 接线项），
// 统一 #[allow(dead_code)] 并注明接线阶段；clippy -D warnings 门禁下必须显式放行。

/// 窗口底（G2 状态栏 / 菜单栏接线）。
#[allow(dead_code)]
pub const BG: Color32 = Color32::from_rgb(0x1e, 0x1f, 0x22);
/// 面板底。
pub const PANEL: Color32 = Color32::from_rgb(0x2a, 0x2b, 0x30);
/// 面板标题 / 输入框内。
pub const EXTREME: Color32 = Color32::from_rgb(0x35, 0x36, 0x3c);
/// 分隔线 / 边框。
pub const STROKE: Color32 = Color32::from_rgb(0x3f, 0x40, 0x46);
/// 正文。
pub const TEXT: Color32 = Color32::from_rgb(0xd6, 0xd9, 0xde);
/// 弱文本（G2 状态栏 / 提示文本接线）。
#[allow(dead_code)]
pub const WEAK: Color32 = Color32::from_rgb(0x96, 0x9b, 0xa5);
/// 高亮 / 主按钮。
pub const ACCENT: Color32 = Color32::from_rgb(0x4f, 0x8c, 0xff);
/// 选中态。
pub const SELECTED: Color32 = Color32::from_rgb(0x35, 0x50, 0x7a);
/// 日志 · 成功（G2 日志着色接线）。
#[allow(dead_code)]
pub const OK: Color32 = Color32::from_rgb(0x57, 0xad, 0x68);
/// 日志 · 警告（G2 日志着色接线）。
#[allow(dead_code)]
pub const WARN: Color32 = Color32::from_rgb(0xe0, 0xa3, 0x4e);
/// 日志 · 错误（G2 日志着色接线）。
#[allow(dead_code)]
pub const ERR: Color32 = Color32::from_rgb(0xe0, 0x6c, 0x6c);
/// 三维视口底（G2 视口背景接线）。
#[allow(dead_code)]
pub const VIEWPORT_BG: Color32 = Color32::from_rgb(0x17, 0x18, 0x1a);

// ---- 装载入口 ----------------------------------------------------------------

/// 应用主题：字体（Phosphor 图标 + CJK 兜底）+ 深色 visuals + 间距。
pub fn apply(ctx: &Context) {
    ctx.set_fonts(build_fonts());
    ctx.set_visuals(theme_visuals());
    ctx.style_mut(|style| {
        // 计划 §3.3：圆角 4px + 间距 8px。
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
        style.spacing.icon_spacing = 6.0;
        style.spacing.icon_width = 20.0;
    });
}

/// 组装字体表：默认拉丁 → Phosphor 图标（fallback）→ CJK 系统字体（兜底）。
fn build_fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    // Phosphor 图标：加为 Proportional 的 fallback（0.7.3 修复了覆盖拉丁字形的问题，
    // 排在默认字体之后、CJK 之前——图标先于中文命中）。
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    // CJK 兜底：Windows 取微软雅黑等系统字体（egui 默认字体无中文字形）。
    for path in CJK_FONT_CANDIDATES {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        fonts
            .font_data
            .insert("cjk_fallback".to_string(), FontData::from_owned(bytes));
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .push("cjk_fallback".to_string());
        }
        break; // 找到第一个可用字体即止
    }
    fonts
}

/// CJK 字体候选（Windows 系统字体，按优先级）。
const CJK_FONT_CANDIDATES: [&str; 4] = [
    "C:/Windows/Fonts/msyh.ttc",
    "C:/Windows/Fonts/simhei.ttf",
    "C:/Windows/Fonts/simsun.ttc",
    "C:/Windows/Fonts/Deng.ttf",
];

/// 深色 CAD visuals（计划 §3.1 / §3.3）。
fn theme_visuals() -> Visuals {
    let mut v = Visuals::dark();
    v.override_text_color = Some(TEXT);
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = EXTREME;
    v.faint_bg_color = EXTREME.gamma_multiply(0.5_f32);
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = SELECTED;
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    v.window_stroke = Stroke::new(1.0_f32, STROKE);
    v.window_rounding = Rounding::same(4.0);
    v.menu_rounding = Rounding::same(4.0);
    // 控件三态：静止（EXTREME 底 + STROKE 边）、悬停（ACCENT 边）、按下（SELECTED 底）；
    // 圆角 4px 在每个控件态上（egui 0.29 的 Visuals 无整体 rounding 字段）。
    let rounding = Rounding::same(4.0);
    v.widgets.noninteractive.rounding = rounding;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, STROKE);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.inactive.rounding = rounding;
    v.widgets.inactive.bg_fill = EXTREME;
    v.widgets.inactive.weak_bg_fill = PANEL;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, STROKE);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.hovered.rounding = rounding;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    v.widgets.hovered.weak_bg_fill = EXTREME;
    v.widgets.active.rounding = rounding;
    v.widgets.active.bg_fill = SELECTED;
    v.widgets.active.weak_bg_fill = SELECTED;
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    v
}

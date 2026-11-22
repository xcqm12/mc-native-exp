//! 科幻 HUD 主题：全局视觉（深空底/青紫双色/辉光描边）+ 科幻按钮绘制。

use eframe::egui;
use std::sync::OnceLock;

pub const CYAN: egui::Color32 = egui::Color32::from_rgb(95, 232, 255);
pub const VIOLET: egui::Color32 = egui::Color32::from_rgb(183, 148, 255);
pub const MINT: egui::Color32 = egui::Color32::from_rgb(124, 232, 200);
pub const AMBER: egui::Color32 = egui::Color32::from_rgb(255, 200, 97);
pub const RED: egui::Color32 = egui::Color32::from_rgb(240, 140, 140);
pub const TEXT: egui::Color32 = egui::Color32::from_rgb(223, 230, 243);
pub const MUTED: egui::Color32 = egui::Color32::from_rgb(139, 147, 168);
pub const BG: egui::Color32 = egui::Color32::from_rgb(7, 9, 15);
pub const PANEL: egui::Color32 = egui::Color32::from_rgb(13, 18, 30);

/// 全局视觉：只调用一次。
pub fn install(ctx: &egui::Context) {
    static ONCE: OnceLock<()> = OnceLock::new();
    if ONCE.set(()).is_err() {
        return;
    }
    let mut style = (*ctx.style()).clone();
    style.visuals.dark_mode = true;
    style.visuals.panel_fill = PANEL;
    style.visuals.window_fill = PANEL;
    style.visuals.extreme_bg_color = BG;
    style.visuals.faint_bg_color = egui::Color32::from_rgb(17, 24, 40);
    // 圆角与描边：科技感直角微圆
    style.visuals.window_rounding = egui::Rounding::same(6.0);
    // 文字色
    style.visuals.override_text_color = Some(TEXT);
    // 控件描边（青色暗光）
    let w = &mut style.visuals.widgets;
    let stroke = |c: egui::Color32| egui::Stroke::new(1.0, c);
    w.noninteractive.bg_stroke = stroke(egui::Color32::from_rgb(40, 56, 84));
    w.inactive.bg_stroke = stroke(egui::Color32::from_rgb(70, 96, 140));
    w.hovered.bg_stroke = stroke(CYAN.gamma_multiply(0.8));
    w.active.bg_stroke = stroke(CYAN);
    w.inactive.bg_fill = egui::Color32::from_rgb(20, 28, 46);
    w.hovered.bg_fill = egui::Color32::from_rgb(28, 40, 64);
    w.active.bg_fill = egui::Color32::from_rgb(36, 52, 82);
    let rounding = egui::Rounding::same(4.0);
    w.noninteractive.rounding = rounding;
    w.inactive.rounding = rounding;
    w.hovered.rounding = rounding;
    w.active.rounding = rounding;

    // 选择高亮
    style.visuals.selection.bg_fill = CYAN.gamma_multiply(0.25);
    style.visuals.selection.stroke = stroke(CYAN);
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    ctx.set_style(style);
}

/// 科幻按钮：双层描边 + 呼吸辉光 + 悬停点亮。`accent` 决定辉光颜色。
pub fn sci_button(ui: &mut egui::Ui, text: &str, accent: egui::Color32) -> egui::Response {
    let t = ui.ctx().input(|i| i.time);
    let pulse: f32 = 0.55 + 0.45 * (((t * 2.2).sin() * 0.5 + 0.5) as f32);
    let btn = egui::Button::new(egui::RichText::new(text).color(TEXT).strong())
        .fill(egui::Color32::from_rgb(16, 24, 40))
        .stroke(egui::Stroke::new(1.2, accent.gamma_multiply(0.75)))
        .rounding(egui::Rounding::same(5.0))
        .min_size(egui::vec2(150.0, 34.0));
    let resp = ui.add(btn);
    if resp.hovered() {
        // 悬停：外圈辉光（多层半透明描边近似）
        let rect = resp.rect.expand2(egui::vec2(4.0, 4.0));
        ui.painter().rect_stroke(rect, 7.0, egui::Stroke::new(2.0, accent.gamma_multiply(0.35 * pulse)));
        ui.painter().rect_stroke(rect.expand2(egui::vec2(3.0, 3.0)), 9.0, egui::Stroke::new(1.0, accent.gamma_multiply(0.18 * pulse)));
        ui.ctx().request_repaint();
    }
    resp
}

/// 科幻标签：发光文字（描边底色近似）。
pub fn sci_label(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    ui.label(egui::RichText::new(format!("▸ {text}")).color(color).monospace().strong());
}

/// HUD 分隔线（青色渐变近似：两段色）。
pub fn hud_separator(ui: &mut egui::Ui) {
    let avail = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(avail, 2.0), egui::Sense::hover());
    let (a, b) = (rect.left(), rect.right());
    let mid = (a + b) / 2.0;
    ui.painter().line_segment([egui::pos2(a, rect.center().y), egui::pos2(mid, rect.center().y)], egui::Stroke::new(1.0, CYAN.gamma_multiply(0.55)));
    ui.painter().line_segment([egui::pos2(mid, rect.center().y), egui::pos2(b, rect.center().y)], egui::Stroke::new(1.0, VIOLET.gamma_multiply(0.55)));
}

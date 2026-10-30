//! Reusable interface pieces: filter chips, message boxes, progress card, scrollbar.
#![allow(dead_code)]

use super::theme::{self, ACCENT, BG, ERROR, ERROR_BG, LINE, LINE_STRONG, MUTED, ON_ACCENT, PANEL, RAISED, WARN, WARN_BG};
use crate::prelude::egui;
use egui::{pos2, vec2, Align, Align2, FontId, Layout, Margin, Rect, RichText, Rounding, Sense, Stroke};

/// A removable filter, drawn as a pill: `label value ×`. Returns true when the × is pressed.
pub(crate) fn chip(ui: &mut egui::Ui, label: &str, value: &str) -> bool {
    let mut remove = false;
    egui::Frame::none()
        .fill(RAISED)
        .stroke(Stroke::new(1.0, LINE_STRONG))
        .rounding(Rounding::same(16.0))
        .inner_margin(Margin { left: 14.0, right: 4.0, top: 3.0, bottom: 3.0 })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(label);
                ui.label(RichText::new(value).family(theme::bold()));
                let x = egui::Button::new(RichText::new("×").size(16.0).color(MUTED)).frame(false).min_size(vec2(24.0, 24.0));
                if ui.add(x).on_hover_text("Remove filter").clicked() { remove = true; }
            });
        });
    remove
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Severity { Warning, Error }

/// A boxed message: a title, what happened and what to do, with optional buttons.
/// The first action is the main one. Returns the index of the button pressed.
pub(crate) fn message_box(ui: &mut egui::Ui, sev: Severity, title: &str, body: &str, actions: &[&str]) -> Option<usize> {
    let (bg, fg) = match sev { Severity::Error => (ERROR_BG, ERROR), Severity::Warning => (WARN_BG, WARN) };
    let mut pressed = None;
    egui::Frame::none()
        .fill(bg)
        .stroke(Stroke::new(1.0, fg.gamma_multiply(0.45)))
        .rounding(Rounding::same(10.0))
        .inner_margin(Margin::symmetric(16.0, 14.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 11.0, fg);
                ui.painter().text(r.center(), Align2::CENTER_CENTER, "!", FontId::new(13.0, theme::bold()), BG);
                ui.vertical(|ui| {
                    ui.label(RichText::new(title).family(theme::bold()));
                    ui.label(body);
                    if !actions.is_empty() {
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            for (i, a) in actions.iter().enumerate() {
                                let b = if i == 0 { egui::Button::new(RichText::new(*a).color(ON_ACCENT)).fill(ACCENT) } else { egui::Button::new(*a) };
                                if ui.add(b).clicked() { pressed = Some(i); }
                            }
                        });
                    }
                });
            });
        });
    pressed
}

/// A card for a running task: title, a bar (`None` = progress unknown), a detail line and Cancel.
/// Returns true when Cancel is pressed.
pub(crate) fn progress_card(ui: &mut egui::Ui, title: &str, frac: Option<f32>, detail: &str) -> bool {
    let mut cancel = false;
    egui::Frame::none()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, LINE))
        .rounding(Rounding::same(12.0))
        .inner_margin(Margin::symmetric(18.0, 14.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).family(theme::bold()));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| { if ui.button("Cancel").clicked() { cancel = true; } });
            });
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 10.0), Sense::hover());
            let p = ui.painter_at(r);
            p.rect_filled(r, 5.0, LINE);
            match frac {
                Some(f) => { p.rect_filled(Rect::from_min_size(r.min, vec2(r.width() * f.clamp(0.0, 1.0), r.height())), 5.0, ACCENT); }
                None => {
                    let t = (ui.ctx().input(|i| i.time) * 0.7).fract() as f32;
                    let w = r.width() * 0.3;
                    let x = r.left() + (r.width() + w) * t - w;
                    p.rect_filled(Rect::from_min_size(pos2(x, r.top()), vec2(w, r.height())), 5.0, ACCENT);
                    ui.ctx().request_repaint();
                }
            }
            ui.label(RichText::new(detail).color(MUTED));
        });
    cancel
}

/// Thin vertical scrollbar for the virtual table. `top` = first visible row, `max` = largest value of `top`,
/// `vis` = rows on screen. Returns the new `top` while it is dragged or clicked.
pub(crate) fn scrollbar(ui: &mut egui::Ui, top: u64, max: u64, vis: u64) -> Option<u64> {
    let size = vec2(ui.available_width(), ui.available_height());
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    if max == 0 { return None; }
    let total = (max + vis.max(1)) as f32;
    let thumb_h = (rect.height() * vis as f32 / total).clamp(28.0, rect.height());
    let span = (rect.height() - thumb_h).max(1.0);
    let mut out = None;
    if resp.dragged() || resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            let f = ((p.y - rect.top() - thumb_h / 2.0) / span).clamp(0.0, 1.0);
            out = Some((f as f64 * max as f64).round() as u64);
        }
    }
    let f = out.unwrap_or(top).min(max) as f32 / max as f32;
    let w = if resp.hovered() || resp.dragged() { 8.0 } else { 6.0 };
    let x = rect.center().x - w / 2.0;
    let thumb = Rect::from_min_size(pos2(x, rect.top() + f * span), vec2(w, thumb_h));
    let col = if resp.dragged() { ACCENT } else if resp.hovered() { MUTED } else { LINE_STRONG };
    ui.painter().rect_filled(thumb, Rounding::same(w / 2.0), col);
    out
}

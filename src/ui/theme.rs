//! Visual theme: palette, spacing and bundled fonts. `apply` is called once at start-up.
#![allow(dead_code)]

use crate::prelude::egui;
use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId, Rounding, Stroke, TextStyle, Vec2};

pub(crate) const BG: Color32 = Color32::from_rgb(0x1C, 0x26, 0x31); // window background, text fields
pub(crate) const PANEL: Color32 = Color32::from_rgb(0x24, 0x32, 0x41); // cards, tables, dialogs
pub(crate) const RAISED: Color32 = Color32::from_rgb(0x2E, 0x40, 0x54); // buttons, chips
pub(crate) const LINE: Color32 = Color32::from_rgb(0x35, 0x47, 0x5A); // dividers
pub(crate) const LINE_STRONG: Color32 = Color32::from_rgb(0x3F, 0x55, 0x6C); // control outlines
pub(crate) const TEXT: Color32 = Color32::from_rgb(0xE8, 0xEE, 0xF4);
pub(crate) const MUTED: Color32 = Color32::from_rgb(0xA5, 0xB3, 0xC2);
pub(crate) const ACCENT: Color32 = Color32::from_rgb(0x9D, 0xB8, 0xFF);
pub(crate) const ON_ACCENT: Color32 = Color32::from_rgb(0x14, 0x20, 0x2B);
pub(crate) const MATCH_BG: Color32 = Color32::from_rgb(0x5C, 0x4A, 0x1E);
pub(crate) const MATCH_FG: Color32 = Color32::from_rgb(0xFF, 0xE7, 0xA8);
pub(crate) const ERROR: Color32 = Color32::from_rgb(0xFF, 0x9C, 0x8F);
pub(crate) const ERROR_BG: Color32 = Color32::from_rgb(0x3A, 0x2A, 0x2E);
pub(crate) const WARN: Color32 = Color32::from_rgb(0xFF, 0xE7, 0xA8);
pub(crate) const WARN_BG: Color32 = Color32::from_rgb(0x3A, 0x32, 0x20);

pub(crate) fn medium() -> FontFamily { FontFamily::Name("medium".into()) }
pub(crate) fn bold() -> FontFamily { FontFamily::Name("bold".into()) }

fn fonts() -> FontDefinitions {
    let mut f = FontDefinitions::default();
    f.font_data.insert("dmsans-regular".into(), FontData::from_static(include_bytes!("../../assets/fonts/DMSans-Regular.ttf")));
    f.font_data.insert("dmsans-medium".into(), FontData::from_static(include_bytes!("../../assets/fonts/DMSans-Medium.ttf")));
    f.font_data.insert("dmsans-bold".into(), FontData::from_static(include_bytes!("../../assets/fonts/DMSans-Bold.ttf")));
    // Regular replaces the default proportional font; the built-in fonts stay behind it as fallbacks for symbols.
    f.families.entry(FontFamily::Proportional).or_default().insert(0, "dmsans-regular".into());
    for (name, key) in [("medium", "dmsans-medium"), ("bold", "dmsans-bold")] {
        f.families.insert(FontFamily::Name(name.into()), vec![key.into(), "dmsans-regular".into()]);
    }
    f
}

pub(crate) fn apply(ctx: &egui::Context) {
    ctx.set_fonts(fonts());

    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(TEXT);
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.window_stroke = Stroke::new(1.0, LINE);
    v.window_rounding = Rounding::same(12.0);
    v.menu_rounding = Rounding::same(8.0);
    v.extreme_bg_color = BG;
    v.faint_bg_color = Color32::from_rgb(0x28, 0x38, 0x49);
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = Color32::from_rgb(0x3A, 0x55, 0x8F);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.window_shadow.color = Color32::from_black_alpha(140);
    v.popup_shadow.color = Color32::from_black_alpha(140);

    let r = Rounding::same(8.0);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = PANEL;
    w.noninteractive.weak_bg_fill = PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.noninteractive.rounding = r;
    w.inactive.bg_fill = RAISED;
    w.inactive.weak_bg_fill = RAISED;
    w.inactive.bg_stroke = Stroke::new(1.0, LINE_STRONG);
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.inactive.rounding = r;
    w.hovered.bg_fill = Color32::from_rgb(0x38, 0x4D, 0x63);
    w.hovered.weak_bg_fill = Color32::from_rgb(0x38, 0x4D, 0x63);
    w.hovered.bg_stroke = Stroke::new(1.0, MUTED);
    w.hovered.fg_stroke = Stroke::new(1.5, TEXT);
    w.hovered.rounding = r;
    w.active.bg_fill = Color32::from_rgb(0x43, 0x60, 0x7C);
    w.active.weak_bg_fill = Color32::from_rgb(0x43, 0x60, 0x7C);
    w.active.bg_stroke = Stroke::new(1.0, ACCENT);
    w.active.fg_stroke = Stroke::new(1.5, TEXT);
    w.active.rounding = r;
    w.open.bg_fill = RAISED;
    w.open.weak_bg_fill = RAISED;
    w.open.bg_stroke = Stroke::new(1.0, ACCENT);
    w.open.rounding = r;
    ctx.set_visuals(v);

    ctx.style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 6.0);
        s.spacing.button_padding = Vec2::new(10.0, 3.0);
        s.spacing.interact_size = Vec2::new(36.0, 24.0);
        s.spacing.window_margin = egui::Margin::same(12.0);
        s.text_styles = [
            (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(14.0, medium())),
            (TextStyle::Heading, FontId::new(18.0, bold())),
            (TextStyle::Monospace, FontId::new(13.5, FontFamily::Monospace)),
        ]
        .into();
    });
}

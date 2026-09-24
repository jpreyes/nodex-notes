//! Tema claro, fuentes del sistema e íconos.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle, Theme,
};
use std::sync::Arc;

pub const BG_RAIL: Color32 = Color32::from_rgb(240, 240, 237);
pub const BG_SIDE: Color32 = Color32::from_rgb(248, 248, 246);
pub const BG_EDITOR: Color32 = Color32::WHITE;
pub const TEXT: Color32 = Color32::from_rgb(38, 38, 36);
pub const MUTED: Color32 = Color32::from_rgb(135, 135, 130);
pub const BORDER: Color32 = Color32::from_rgb(226, 226, 221);
pub const ACCENT: Color32 = Color32::from_rgb(40, 104, 214);
pub const ACCENT_BG: Color32 = Color32::from_rgb(228, 238, 252);
pub const HOVER: Color32 = Color32::from_rgb(233, 233, 229);
pub const SUCCESS: Color32 = Color32::from_rgb(46, 125, 50);

/// Familia para títulos (negrita del sistema si existe).
pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

pub fn setup(ctx: &egui::Context) {
    setup_fonts(ctx);

    let mut v = egui::Visuals::light();
    v.panel_fill = BG_SIDE;
    v.window_fill = Color32::WHITE;
    v.extreme_bg_color = Color32::WHITE;
    v.faint_bg_color = BG_SIDE;
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = Color32::from_rgb(200, 220, 250);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(8);
    v.text_cursor.stroke = Stroke::new(1.5, TEXT);
    let w = &mut v.widgets;
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.inactive.weak_bg_fill = Color32::TRANSPARENT;
    w.inactive.bg_fill = Color32::WHITE;
    w.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    w.hovered.weak_bg_fill = HOVER;
    w.hovered.bg_fill = HOVER;
    w.hovered.bg_stroke = Stroke::new(1.0, BORDER);
    w.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    w.active.weak_bg_fill = Color32::from_rgb(222, 222, 217);
    w.active.fg_stroke = Stroke::new(1.0, TEXT);
    w.open.weak_bg_fill = HOVER;
    for s in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        s.corner_radius = CornerRadius::same(6);
        s.expansion = 0.0;
    }
    ctx.set_visuals_of(Theme::Light, v);
    ctx.set_theme(egui::ThemePreference::Light);

    ctx.all_styles_mut(|s| {
        s.text_styles.insert(TextStyle::Body, FontId::proportional(14.0));
        s.text_styles.insert(TextStyle::Button, FontId::proportional(14.0));
        s.text_styles.insert(TextStyle::Small, FontId::proportional(12.0));
        s.text_styles.insert(TextStyle::Heading, bold(20.0));
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        s.spacing.button_padding = egui::vec2(8.0, 4.0);
        s.spacing.interact_size.y = 24.0;
    });
}

fn first_file(paths: &[String]) -> Option<Vec<u8>> {
    paths.iter().find_map(|p| std::fs::read(p).ok())
}

fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);

    let (regular, bold_paths): (Vec<String>, Vec<String>) = if cfg!(windows) {
        let dir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into()) + "\\Fonts\\";
        (vec![dir.clone() + "segoeui.ttf"], vec![dir.clone() + "seguisb.ttf", dir + "segoeuib.ttf"])
    } else if cfg!(target_os = "macos") {
        (
            vec!["/System/Library/Fonts/Supplemental/Arial.ttf".into()],
            vec!["/System/Library/Fonts/Supplemental/Arial Bold.ttf".into()],
        )
    } else {
        let d = ["/usr/share/fonts/truetype/noto/", "/usr/share/fonts/noto/", "/usr/share/fonts/google-noto/"];
        let dv = "/usr/share/fonts/truetype/dejavu/";
        (
            d.iter().map(|p| format!("{p}NotoSans-Regular.ttf")).chain([format!("{dv}DejaVuSans.ttf")]).collect(),
            d.iter().map(|p| format!("{p}NotoSans-SemiBold.ttf")).chain([format!("{dv}DejaVuSans-Bold.ttf")]).collect(),
        )
    };

    let mut proportional = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    if let Some(bytes) = first_file(&regular) {
        fonts.font_data.insert("ui".into(), Arc::new(FontData::from_owned(bytes)));
        proportional.insert(0, "ui".into());
    }
    let mut bold_family = proportional.clone();
    if let Some(bytes) = first_file(&bold_paths) {
        fonts.font_data.insert("ui-bold".into(), Arc::new(FontData::from_owned(bytes)));
        bold_family.insert(0, "ui-bold".into());
    }
    fonts.families.insert(FontFamily::Proportional, proportional);
    fonts.families.insert(FontFamily::Name("bold".into()), bold_family);
    ctx.set_fonts(fonts);
}

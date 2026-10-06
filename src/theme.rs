//! Tema (claro u oscuro), fuentes del sistema e íconos.
//!
//! El oscuro es grafito con un toque verdoso, texto claro suave y un acento verde jade. Nada de
//! negro puro, violetas ni neón. Los colores se piden con funciones (`TEXT()`,
//! `ACCENT()`…) que miran el modo actual, así el tema cambia sin reiniciar. Los colores puestos a
//! mano en la interfaz pasan por `c()`, que en oscuro los convierte (los fondos claros quedan
//! oscuros con su mismo tono; los textos oscuros, claros).

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle, Theme,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

static DARK: AtomicBool = AtomicBool::new(false);

/// ¿Está el tema oscuro?
pub fn is_dark() -> bool {
    DARK.load(Ordering::Relaxed)
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

macro_rules! palette {
    ($($name:ident: $light:expr, $dark:expr;)*) => {
        $(
            #[allow(non_snake_case)]
            pub fn $name() -> Color32 {
                if is_dark() { $dark } else { $light }
            }
        )*
    };
}

palette! {
    BG_RAIL: rgb(240, 240, 237), rgb(21, 23, 22);
    BG_SIDE: rgb(248, 248, 246), rgb(27, 29, 28);
    BG_EDITOR: Color32::WHITE, rgb(32, 35, 33);
    TEXT: rgb(38, 38, 36), rgb(226, 230, 224);
    MUTED: rgb(110, 110, 105), rgb(150, 158, 151);
    BORDER: rgb(226, 226, 221), rgb(54, 59, 56);
    ACCENT: rgb(40, 104, 214), rgb(104, 190, 150);
    ACCENT_BG: rgb(228, 238, 252), rgb(35, 56, 47);
    HOVER: rgb(233, 233, 229), rgb(41, 45, 43);
    SUCCESS: rgb(46, 125, 50), rgb(156, 196, 116);
    RED: rgb(198, 40, 40), rgb(229, 123, 108);
    WARN: rgb(176, 104, 0), rgb(232, 145, 90);
}

/// Un color puesto a mano para el tema claro, como se ve en el tema actual: en oscuro, los
/// fondos claros pasan a ser oscuros (con su tono, un poco más claros que la hoja) y los textos
/// oscuros, claros.
pub fn c(light: Color32) -> Color32 {
    if !is_dark() {
        return light;
    }
    let [r, g, b, a] = light.to_array();
    let (h, s, l) = to_hsl(r, g, b);
    let (s2, l2) = if l > 0.5 { (s * 0.45, 0.17 + (1.0 - l) * 0.8) } else { (s * 0.8, (0.92 - l * 0.55).clamp(0.6, 0.9)) };
    let (r, g, b) = from_hsl(h, s2, l2);
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

fn to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    if max == min {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, s, l)
}

fn from_hsl(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    if s == 0.0 {
        let v = (l * 255.0).round() as u8;
        return (v, v, v);
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |mut t: f32| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v * 255.0).round().clamp(0.0, 255.0) as u8
    };
    (f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0))
}

/// Familia para títulos (negrita del sistema si existe).
pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

pub fn setup(ctx: &egui::Context) {
    setup_fonts(ctx);
    apply(ctx, is_dark());

    ctx.all_styles_mut(|s| {
        s.text_styles.insert(TextStyle::Body, FontId::proportional(14.0));
        s.text_styles.insert(TextStyle::Button, FontId::proportional(14.0));
        s.text_styles.insert(TextStyle::Small, FontId::proportional(12.5));
        s.text_styles.insert(TextStyle::Heading, bold(20.0));
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        s.spacing.button_padding = egui::vec2(8.0, 4.0);
        s.spacing.interact_size.y = 24.0;
    });
}

/// Pone el tema claro u oscuro (sin reiniciar).
pub fn apply(ctx: &egui::Context, dark: bool) {
    DARK.store(dark, Ordering::Relaxed);
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    let surface = c(Color32::WHITE);
    v.panel_fill = BG_SIDE();
    v.window_fill = surface;
    v.extreme_bg_color = if dark { rgb(26, 28, 27) } else { Color32::WHITE };
    v.faint_bg_color = BG_SIDE();
    v.hyperlink_color = ACCENT();
    v.selection.bg_fill = if dark { rgb(44, 82, 64) } else { rgb(200, 220, 250) };
    v.selection.stroke = Stroke::new(1.0, ACCENT());
    v.window_stroke = Stroke::new(1.0, BORDER());
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(8);
    v.text_cursor.stroke = Stroke::new(1.5, TEXT());
    v.override_text_color = None;
    let w = &mut v.widgets;
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT());
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER());
    w.noninteractive.bg_fill = BG_SIDE();
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT());
    w.inactive.weak_bg_fill = Color32::TRANSPARENT;
    w.inactive.bg_fill = surface;
    w.inactive.bg_stroke = Stroke::new(1.0, BORDER());
    w.hovered.weak_bg_fill = HOVER();
    w.hovered.bg_fill = HOVER();
    w.hovered.bg_stroke = Stroke::new(1.0, BORDER());
    w.hovered.fg_stroke = Stroke::new(1.0, TEXT());
    w.active.weak_bg_fill = if dark { rgb(48, 54, 51) } else { rgb(222, 222, 217) };
    w.active.bg_fill = w.active.weak_bg_fill;
    w.active.fg_stroke = Stroke::new(1.0, TEXT());
    w.open.weak_bg_fill = HOVER();
    w.open.fg_stroke = Stroke::new(1.0, TEXT());
    for s in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        s.corner_radius = CornerRadius::same(6);
        s.expansion = 0.0;
    }
    let (theme, pref) = if dark { (Theme::Dark, egui::ThemePreference::Dark) } else { (Theme::Light, egui::ThemePreference::Light) };
    ctx.set_visuals_of(theme, v);
    ctx.set_theme(pref);
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

/// Colores de una etiqueta: fondo, contorno, texto y punto.
#[derive(Clone, Copy)]
pub struct TagColors {
    pub bg: Color32,
    pub border: Color32,
    pub text: Color32,
    pub dot: Color32,
}

const TAG_PALETTE: [[u32; 4]; 7] = [
    [0xEEEDFE, 0xAFA9EC, 0x3C3489, 0x7F77DD], // morado
    [0xE1F5EE, 0x5DCAA5, 0x085041, 0x1D9E75], // verde azulado
    [0xFAECE7, 0xF0997B, 0x712B13, 0xD85A30], // coral
    [0xFBEAF0, 0xED93B1, 0x72243E, 0xD4537E], // rosado
    [0xE6F1FB, 0x85B7EB, 0x0C447C, 0x378ADD], // azul
    [0xEAF3DE, 0x97C459, 0x27500A, 0x639922], // verde
    [0xFAEEDA, 0xEF9F27, 0x633806, 0xBA7517], // ámbar
];

fn hex(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Cada etiqueta tiene siempre el mismo color (según su nombre).
pub fn tag_colors(tag: &str) -> TagColors {
    let i = (crate::ai::fnv(&tag.to_lowercase()) % TAG_PALETTE.len() as u64) as usize;
    let [bg, border, text, dot] = TAG_PALETTE[i];
    TagColors { bg: c(hex(bg)), border: c(hex(border)), text: c(hex(text)), dot: hex(dot) }
}

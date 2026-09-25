#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agenda;
mod ai;
mod app;
mod ask;
mod capture;
mod config;
mod gcal;
mod lines;
mod links;
mod organize;
mod tags;
mod theme;
mod vault;

fn main() -> eframe::Result {
    let (cfg, cfg_msg) = config::load();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Notas")
            .with_inner_size([1040.0, 680.0])
            .with_min_inner_size([640.0, 420.0])
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png")).unwrap_or_default()),
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        "Notas",
        options,
        Box::new(move |cc| {
            theme::setup(&cc.egui_ctx);
            Ok(Box::new(app::NotesApp::new(cfg, cfg_msg, cc.egui_ctx.clone())))
        }),
    )
}

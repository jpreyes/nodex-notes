#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Primero: sus macros (t!, tf!) se usan en todos los demás.
#[macro_use]
mod i18n;
mod account;
mod activity;
mod agenda;
mod ai;
mod app;
mod ask;
mod background;
mod calendars;
mod capture;
mod claims;
mod config;
mod conflicts;
mod doubts;
mod dropbox;
mod dups;
mod gcal;
mod history;
mod import;
mod ics;
mod lines;
mod links;
mod mail;
mod mail_ai;
mod merge;
mod organize;
mod shared;
mod spaces;
mod sync;
mod tags;
mod todo;
mod update;
mod theme;
mod vault;

fn main() -> eframe::Result {
    // Recién actualizada: la versión anterior termina de guardar antes de leer nada.
    let after_update = update::wait_for_previous();
    // Una sola Notas: si ya hay una (quizás en segundo plano), se muestra esa.
    let listener = match background::single_instance(after_update) {
        background::Instance::Shown => return Ok(()),
        background::Instance::First(l) => l,
    };
    // Abierta con Windows: empieza escondida, junto al reloj.
    let hidden = cfg!(windows) && std::env::args().any(|a| a == background::HIDDEN_ARG);
    // Antes de leer la configuración, el idioma del sistema (para sus avisos); después, el elegido.
    i18n::set_english(i18n::system_is_english());
    let (cfg, cfg_msg) = config::load();
    // El idioma: el elegido o, si no se eligió, el del sistema.
    i18n::set_english(match cfg.idioma.as_str() {
        "en" => true,
        "es" => false,
        _ => i18n::system_is_english(),
    });
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Notas")
            .with_inner_size([1040.0, 680.0])
            .with_min_inner_size([640.0, 420.0])
            .with_visible(!hidden)
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png")).unwrap_or_default()),
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        "Notas",
        options,
        Box::new(move |cc| {
            theme::setup(&cc.egui_ctx);
            let mut app = app::NotesApp::new(cfg, cfg_msg, cc.egui_ctx.clone());
            app.set_background(background::Background::start(cc, listener));
            Ok(Box::new(app))
        }),
    )
}

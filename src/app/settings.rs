//! Ventana de Configuración: categorías a la izquierda, opciones a la derecha.
//! Cada cambio se guarda en config.toml y se aplica al instante (sin reiniciar).

use super::*;
use std::sync::mpsc::Receiver;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Section {
    General,
    Ai,
    Calendar,
    Shortcuts,
    About,
}

const SECTIONS: [(Section, &str, &str); 5] = [
    (Section::General, icon::FOLDER_SIMPLE, "General"),
    (Section::Ai, icon::SPARKLE, "Inteligencia artificial"),
    (Section::Calendar, icon::CALENDAR_BLANK, "Calendar"),
    (Section::Shortcuts, icon::KEYBOARD, "Atajos"),
    (Section::About, icon::INFO, "Acerca de"),
];

const REPO: &str = "https://github.com/jpreyes/nodex-notes";

pub(super) struct Settings {
    pub section: Section,
    show_key: bool,
    show_secret: bool,
    /// El modelo no está en la lista sugerida: se escribe a mano.
    custom_model: bool,
    // Borradores de texto: se aplican al salir del campo (o al cerrar la ventana).
    key: String,
    model: String,
    client_id: String,
    client_secret: String,
    /// Formulario para agregar un calendario (nombre, enlace).
    cal_form: Option<(String, String)>,
    test: Option<Receiver<Result<u128, String>>>,
    test_result: Option<Result<u128, String>>,
    update: Option<Receiver<Result<Option<String>, String>>>,
    update_result: Option<Result<Option<String>, String>>,
}

fn models_for(provider: &str) -> &'static [&'static str] {
    ai::PROVIDERS.iter().find(|p| p.0 == provider).map_or(&[], |p| p.2)
}

impl Settings {
    pub fn new(section: Section, cfg: &Config) -> Self {
        Settings {
            section,
            show_key: false,
            show_secret: false,
            custom_model: !models_for(&cfg.proveedor).contains(&cfg.modelo.as_str()),
            key: cfg.clave_api.clone(),
            model: cfg.modelo.clone(),
            client_id: cfg.google_client_id.clone(),
            client_secret: cfg.google_client_secret.clone(),
            cal_form: None,
            test: None,
            test_result: None,
            update: None,
            update_result: None,
        }
    }
}

/// Cambios pedidos desde la ventana; se aplican después de dibujarla.
enum Change {
    Folder(PathBuf),
    Provider(String),
    Model(String),
    Key(String),
    AiAuto(bool),
    GoogleCreds(String, String),
    TestConnection,
    CheckUpdate,
    Do(Action),
    OpenUrl(&'static str),
}

/// ¿Hay una versión publicada más nueva que esta? Devuelve la etiqueta si la hay.
fn check_update(ctx: egui::Context) -> Receiver<Result<Option<String>, String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| -> Result<Option<String>, String> {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
            let v: serde_json::Value = rt.block_on(async {
                reqwest::Client::new()
                    .get("https://api.github.com/repos/jpreyes/nodex-notes/releases/latest")
                    .header("User-Agent", "nodex-notes")
                    .send()
                    .await
                    .map_err(|e| format!("sin conexión ({e})"))?
                    .json()
                    .await
                    .map_err(|e| e.to_string())
            })?;
            let tag = v["tag_name"].as_str().ok_or("GitHub no respondió la versión")?.to_string();
            let num = |s: &str| s.trim_start_matches('v').split('.').map(|p| p.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>();
            Ok((num(&tag) > num(env!("CARGO_PKG_VERSION"))).then_some(tag))
        })();
        let _ = tx.send(result);
        ctx.request_repaint();
    });
    rx
}

// ---------- Widgets ----------

/// Acorta un texto a `n` caracteres con "…" (para que un nombre largo no ensanche la ventana).
fn short(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n).collect::<String>()) }
}

/// Interruptor redondo (estilo iOS/macOS).
fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let size = egui::vec2(38.0, 22.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(response.id, *on);
    let off = Color32::from_rgb(205, 205, 200);
    let bg = Color32::from_rgb(
        egui::lerp(off.r() as f32..=ACCENT.r() as f32, t) as u8,
        egui::lerp(off.g() as f32..=ACCENT.g() as f32, t) as u8,
        egui::lerp(off.b() as f32..=ACCENT.b() as f32, t) as u8,
    );
    let r = rect.height() / 2.0;
    ui.painter().rect_filled(rect, r, bg);
    let x = egui::lerp((rect.left() + r)..=(rect.right() - r), t);
    ui.painter().circle_filled(egui::pos2(x, rect.center().y), r - 3.0, Color32::WHITE);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Fila de opción: título (y ayuda) a la izquierda, control a la derecha, línea abajo.
fn row(ui: &mut Ui, title: &str, hint: &str, control: impl FnOnce(&mut Ui)) {
    ui.add_space(10.0);
    // Primero el control (a la derecha); el texto usa solo el ancho que queda.
    ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 0.0), Layout::right_to_left(Align::Center), |ui| {
        control(ui);
        ui.add_space(12.0);
        ui.with_layout(Layout::top_down(Align::Min), |ui| {
            ui.label(RichText::new(title).size(14.0).color(TEXT));
            if !hint.is_empty() {
                ui.label(RichText::new(hint).size(12.0).color(MUTED));
            }
        });
    });
    ui.add_space(10.0);
    ui.separator();
}

fn heading(ui: &mut Ui, title: &str, subtitle: &str) {
    ui.label(RichText::new(title).font(theme::bold(20.0)));
    ui.label(RichText::new(subtitle).size(13.0).color(MUTED));
    ui.add_space(6.0);
    ui.separator();
}

fn chip(ui: &mut Ui, text: &str, ok: bool) {
    let (fg, bg) = if ok {
        (SUCCESS, Color32::from_rgb(230, 244, 231))
    } else {
        (RED, Color32::from_rgb(252, 235, 235))
    };
    // Los mensajes largos (errores con URL) pasan a varias líneas en vez de ensanchar la ventana.
    Frame::new()
        .fill(bg)
        .corner_radius(6)
        .inner_margin(Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width());
            ui.add(egui::Label::new(RichText::new(text).size(12.5).color(fg)).wrap())
        });
}

fn secret_field(ui: &mut Ui, value: &mut String, show: &mut bool, hint: &str) -> Response {
    let eye = if *show { icon::EYE_SLASH } else { icon::EYE };
    if ui.add(egui::Button::new(RichText::new(eye).size(16.0).color(MUTED)).frame(false)).on_hover_text(if *show { "Ocultar" } else { "Mostrar" }).clicked() {
        *show = !*show;
    }
    ui.add(egui::TextEdit::singleline(value).password(!*show).hint_text(hint).desired_width(230.0).margin(Margin::symmetric(8, 4)))
}

impl NotesApp {
    pub(super) fn open_settings(&mut self, section: Section) {
        self.save();
        self.settings = Some(Settings::new(section, &self.cfg));
    }

    /// Dibuja la ventana (si está abierta) y aplica lo que se cambió.
    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
        let Some(mut s) = self.settings.take() else { return };
        if let Some(r) = s.test.as_ref().and_then(|rx| rx.try_recv().ok()) {
            s.test_result = Some(r);
            s.test = None;
        }
        if let Some(r) = s.update.as_ref().and_then(|rx| rx.try_recv().ok()) {
            s.update_result = Some(r);
            s.update = None;
        }

        let mut changes: Vec<Change> = Vec::new();
        let mut close = false;
        let screen = ctx.content_rect();
        let w = (screen.width() - 80.0).clamp(560.0, 820.0);
        let h = (screen.height() - 80.0).clamp(380.0, 540.0);
        let nav_w = 200.0;

        let modal = egui::Modal::new(Id::new("settings"))
            .frame(Frame::new().fill(Color32::WHITE).corner_radius(12).stroke(Stroke::new(1.0, theme::BORDER)))
            .show(ctx, |ui| {
                // Tamaño fijo: ningún contenido puede agrandar la ventana más allá de la pantalla.
                ui.set_width(w);
                ui.set_height(h);
                ui.set_max_size(egui::vec2(w, h));
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    // Categorías
                    ui.allocate_ui_with_layout(egui::vec2(nav_w, h), Layout::top_down(Align::Min), |ui| {
                        let rect = egui::Rect::from_min_size(ui.max_rect().min, egui::vec2(nav_w, h));
                        let radius = egui::CornerRadius { nw: 12, sw: 12, ne: 0, se: 0 };
                        ui.painter().rect_filled(rect, radius, BG_SIDE);
                        ui.painter().vline(rect.right(), rect.y_range(), Stroke::new(1.0, theme::BORDER));
                        Frame::new().inner_margin(Margin::symmetric(10, 16)).show(ui, |ui| {
                            ui.set_width(nav_w - 20.0);
                            ui.label(RichText::new("   Configuración").font(theme::bold(16.0)));
                            ui.add_space(12.0);
                            for (sec, glyph, label) in SECTIONS {
                                if list_row(ui, glyph, label, "", s.section == sec).clicked() {
                                    s.section = sec;
                                }
                            }
                        });
                    });
                    // Contenido
                    ui.allocate_ui_with_layout(egui::vec2(w - nav_w, h), Layout::top_down(Align::Min), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0; // (solo las dos columnas van pegadas)
                        Frame::new().inner_margin(Margin { left: 26, right: 22, top: 16, bottom: 16 }).show(ui, |ui| {
                            ui.set_width(w - nav_w - 48.0);
                            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                                let x = egui::Button::new(RichText::new(icon::X).size(18.0).color(MUTED)).frame(false);
                                if ui.add(x).on_hover_text("Cerrar (Esc)").clicked() {
                                    close = true;
                                }
                            });
                            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match s.section {
                                Section::General => self.section_general(ui, &mut changes),
                                Section::Ai => self.section_ai(ui, &mut s, &mut changes),
                                Section::Calendar => self.section_calendar(ui, &mut s, &mut changes),
                                Section::Shortcuts => section_shortcuts(ui),
                                Section::About => section_about(ui, &mut s, &mut changes),
                            });
                        });
                    });
                });
            });
        if modal.should_close() {
            close = true;
        }
        if close {
            // Lo escrito y no confirmado también se guarda.
            if s.key != self.cfg.clave_api {
                changes.push(Change::Key(s.key.clone()));
            }
            if s.custom_model && s.model.trim() != self.cfg.modelo {
                changes.push(Change::Model(s.model.trim().to_string()));
            }
            if s.client_id != self.cfg.google_client_id || s.client_secret != self.cfg.google_client_secret {
                changes.push(Change::GoogleCreds(s.client_id.clone(), s.client_secret.clone()));
            }
        }
        for c in changes {
            self.apply_setting(c, &mut s);
        }
        if !close {
            self.settings = Some(s);
        }
    }

    fn apply_setting(&mut self, c: Change, s: &mut Settings) {
        match c {
            Change::Folder(p) => self.change_folder(p),
            Change::Provider(id) => {
                let models = models_for(&id);
                if !models.is_empty() && !models.contains(&self.cfg.modelo.as_str()) {
                    self.cfg.modelo = models[0].to_string();
                }
                self.cfg.proveedor = id;
                s.model = self.cfg.modelo.clone();
                s.custom_model = !models_for(&self.cfg.proveedor).contains(&self.cfg.modelo.as_str());
                s.test_result = None;
                self.save_config();
                self.restart_ai();
            }
            Change::Model(m) => {
                self.cfg.modelo = m;
                s.test_result = None;
                self.save_config();
                self.restart_ai();
            }
            Change::Key(k) => {
                self.cfg.clave_api = k.trim().to_string();
                s.test_result = None;
                self.save_config();
                self.restart_ai();
            }
            Change::AiAuto(on) => {
                self.cfg.ia_automatica = on;
                self.ai_auto = on;
                self.save_config();
            }
            Change::GoogleCreds(id, secret) => {
                self.cfg.google_client_id = id.trim().to_string();
                self.cfg.google_client_secret = secret.trim().to_string();
                self.save_config();
                self.restart_gcal();
            }
            Change::TestConnection => {
                s.test_result = None;
                s.test = Some(ai::test_connection(&self.cfg, self.ctx.clone()));
            }
            Change::CheckUpdate => {
                s.update_result = None;
                s.update = Some(check_update(self.ctx.clone()));
            }
            Change::Do(a) => self.apply(a),
            Change::OpenUrl(u) => gcal::open_browser(u),
        }
    }

    fn section_general(&self, ui: &mut Ui, changes: &mut Vec<Change>) {
        heading(ui, "General", "Dónde se guardan tus notas.");
        let folder = self.vault.root.display().to_string();
        row(ui, "Carpeta de notas", &folder, |ui| {
            if ui.button(format!("{}  Elegir carpeta…", icon::FOLDER_OPEN)).clicked() {
                let picked = rfd::FileDialog::new()
                    .set_title("Carpeta de notas")
                    .set_directory(&self.vault.root)
                    .pick_folder();
                if let Some(p) = picked {
                    changes.push(Change::Folder(p));
                }
            }
            if ui.link("Abrir").clicked() {
                changes.push(Change::Do(Action::OpenExternal(self.vault.root.clone())));
            }
        });
        ui.add_space(8.0);
        ui.label(
            RichText::new(
                "Cada nota es un archivo Markdown (.md) que puedes abrir con cualquier editor; cada subcarpeta es un espacio de trabajo. \
                 Si usas Dropbox, elige una carpeta dentro de Dropbox para tener tus notas en todos tus equipos.",
            )
            .size(12.5)
            .color(MUTED),
        );
        ui.add_space(16.0);
        let cfg_path = config::config_path().display().to_string();
        row(ui, "Archivo de configuración", &cfg_path, |ui| {
            if ui.link("Abrir config.toml").clicked() {
                changes.push(Change::Do(Action::OpenExternal(config::config_path())));
            }
        });
    }

    fn section_ai(&self, ui: &mut Ui, s: &mut Settings, changes: &mut Vec<Change>) {
        heading(ui, "Inteligencia artificial", "Organiza tus notas sola: espacio, etiquetas, resumen, tareas y agenda.");

        let mut auto = self.cfg.ia_automatica;
        row(ui, "Organizar automáticamente", "Al dejar una nota o tras 45 s sin escribir", |ui| {
            if toggle(ui, &mut auto).changed() {
                changes.push(Change::AiAuto(auto));
            }
        });

        let current = ai::PROVIDERS.iter().find(|p| p.0 == self.cfg.proveedor);
        row(ui, "Proveedor", "", |ui| {
            let label = current.map_or_else(|| format!("Desconocido: {}", short(&self.cfg.proveedor, 22)), |p| p.1.to_string());
            egui::ComboBox::from_id_salt("proveedor").selected_text(label).width(250.0).show_ui(ui, |ui| {
                for (id, name, _, _) in ai::PROVIDERS {
                    if ui.selectable_label(self.cfg.proveedor == *id, *name).clicked() && self.cfg.proveedor != *id {
                        changes.push(Change::Provider(id.to_string()));
                    }
                }
            });
        });

        let models = models_for(&self.cfg.proveedor);
        row(ui, "Modelo", if models.is_empty() { "Escribe el nombre exacto del modelo" } else { "" }, |ui| {
            if s.custom_model || models.is_empty() {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut s.model)
                        .hint_text("nombre del modelo")
                        .desired_width(if models.is_empty() { 250.0 } else { 150.0 })
                        .margin(Margin::symmetric(8, 4)),
                );
                if r.lost_focus() && s.model.trim() != self.cfg.modelo {
                    changes.push(Change::Model(s.model.trim().to_string()));
                }
            }
            if !models.is_empty() {
                let text = if s.custom_model { "Otro…" } else { self.cfg.modelo.as_str() };
                let width = if s.custom_model { 95.0 } else { 250.0 };
                egui::ComboBox::from_id_salt("modelo").selected_text(short(text, 28)).width(width).show_ui(ui, |ui| {
                    for m in models {
                        if ui.selectable_label(!s.custom_model && self.cfg.modelo == *m, *m).clicked() {
                            s.custom_model = false;
                            s.model = m.to_string();
                            if self.cfg.modelo != *m {
                                changes.push(Change::Model(m.to_string()));
                            }
                        }
                    }
                    ui.separator();
                    if ui.selectable_label(s.custom_model, "Otro…").clicked() {
                        s.custom_model = true;
                    }
                });
            }
        });

        let ollama = self.cfg.proveedor == "ollama";
        row(
            ui,
            "Clave API",
            if ollama { "No hace falta con Ollama" } else { "Se guarda solo en este equipo, nunca en Dropbox" },
            |ui| {
                let r = secret_field(ui, &mut s.key, &mut s.show_key, "pega aquí tu clave");
                if (r.lost_focus() || ui.input(|i| i.key_pressed(Key::Enter))) && s.key.trim() != self.cfg.clave_api {
                    changes.push(Change::Key(s.key.clone()));
                }
            },
        );

        row(ui, "Conexión", "Envía un mensaje corto para comprobar la clave y el modelo", |ui| {
            let b = egui::Button::new(format!("{}  Probar conexión", icon::PLUGS_CONNECTED));
            if ui.add_enabled(s.test.is_none(), b).clicked() {
                changes.push(Change::TestConnection);
            }
            if s.test.is_some() {
                ui.add(egui::Spinner::new().size(14.0));
            }
        });
        // Resultado debajo, a todo el ancho (puede ocupar varias líneas).
        ui.add_space(8.0);
        match (&s.test, &s.test_result) {
            (Some(_), _) => {
                ui.label(RichText::new("Probando…").size(12.5).color(MUTED));
            }
            (None, Some(Ok(ms))) => chip(ui, &format!("{} Conectado · respondió en {:.1} s", icon::CHECK_CIRCLE, *ms as f32 / 1000.0), true),
            (None, Some(Err(e))) => chip(ui, &format!("{} {e}", icon::WARNING_CIRCLE), false),
            (None, None) => {
                if let Err(e) = &self.ai {
                    chip(ui, &format!("{} {e}", icon::WARNING_CIRCLE), false);
                }
            }
        }
        if let Some(url) = current.and_then(|p| p.3) {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("¿No tienes clave?").size(12.5).color(MUTED));
                if ui.link(RichText::new("Crear una en opencode.ai").size(12.5)).clicked() {
                    changes.push(Change::OpenUrl(url));
                }
            });
        }
        if self.cfg.proveedor.starts_with("opencode") {
            ui.add_space(6.0);
            ui.label(
                RichText::new("OpenCode Zen cobra por uso desde tu saldo; OpenCode Go es una suscripción mensual con límite de uso. Ambos usan la misma clave.")
                    .size(12.0)
                    .color(MUTED),
            );
        }
    }

    fn section_calendar(&self, ui: &mut Ui, s: &mut Settings, changes: &mut Vec<Change>) {
        heading(ui, "Calendar", "Agrega tus calendarios con su enlace y verás sus eventos en la Agenda, Hoy e Inicio.");
        ui.add_space(12.0);
        Frame::new()
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(10)
            .inner_margin(Margin::same(16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(format!("{}  Tus calendarios", icon::CALENDAR_BLANK)).font(theme::bold(15.0)));
                ui.label(
                    RichText::new("Google Calendar, Outlook, iCloud o cualquier calendario con enlace ICS. Solo se leen; se actualizan cada 15 minutos.")
                        .size(12.5)
                        .color(MUTED),
                );
                ui.add_space(8.0);
                if let Some(a) = super::calendars_ui::calendars_panel(ui, &self.cfg.calendarios, &self.cals, &mut s.cal_form) {
                    changes.push(Change::Do(a));
                }
            });
        ui.add_space(12.0);
        Frame::new()
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(10)
            .inner_margin(Margin::same(16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{}  Enviar tu agenda a Google Calendar (avanzado)", icon::GOOGLE_LOGO)).font(theme::bold(15.0)));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| match &self.gcal {
                        None => {
                            ui.label(RichText::new("Falta el ID de cliente").size(12.5).color(MUTED));
                        }
                        Some(g) if g.connecting => {
                            ui.label(RichText::new("Esperando tu permiso en el navegador…").size(12.5).color(ACCENT));
                        }
                        Some(g) if g.connected => {
                            if ui.button("Desconectar").clicked() {
                                changes.push(Change::Do(Action::GoogleDisconnect));
                            }
                            if ui.button("Sincronizar ahora").clicked() {
                                changes.push(Change::Do(Action::GoogleSync));
                            }
                            let status = match (&g.last_error, g.last_sync) {
                                (Some(_), _) => "error (ver abajo)".to_string(),
                                (None, Some(t)) => format!("sincronizado {}", t.format("%H:%M")),
                                _ => "conectado".into(),
                            };
                            ui.label(RichText::new(status).size(12.5).color(if g.last_error.is_some() { RED } else { SUCCESS }));
                        }
                        Some(_) => {
                            if ui.button(format!("{}  Conectar", icon::LINK)).clicked() {
                                changes.push(Change::Do(Action::GoogleConnect));
                            }
                        }
                    });
                });
                ui.label(
                    RichText::new("Crea un calendario propio llamado «Notas» y solo toca ese; tus otros calendarios quedan fuera de su alcance.")
                        .size(12.5)
                        .color(MUTED),
                );
                if let Some(e) = self.gcal.as_ref().and_then(|g| g.last_error.as_ref()) {
                    ui.add_space(6.0);
                    chip(ui, &format!("{} {e}", icon::WARNING_CIRCLE), false);
                }
                ui.add_space(6.0);
                row(ui, "ID de cliente", "", |ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut s.client_id)
                            .hint_text("123456-abc.apps.googleusercontent.com")
                            .desired_width(250.0)
                            .margin(Margin::symmetric(8, 4)),
                    );
                    if r.lost_focus() && s.client_id.trim() != self.cfg.google_client_id {
                        changes.push(Change::GoogleCreds(s.client_id.clone(), s.client_secret.clone()));
                    }
                });
                row(ui, "Secreto de cliente", "", |ui| {
                    let r = secret_field(ui, &mut s.client_secret, &mut s.show_secret, "GOCSPX-…");
                    if r.lost_focus() && s.client_secret.trim() != self.cfg.google_client_secret {
                        changes.push(Change::GoogleCreds(s.client_id.clone(), s.client_secret.clone()));
                    }
                });
                ui.add_space(6.0);
                egui::CollapsingHeader::new(RichText::new("¿Cómo obtengo estas credenciales? (una vez, ~5 minutos)").size(13.0))
                    .default_open(self.cfg.google_client_id.is_empty())
                    .show(ui, |ui| {
                        let steps: [(&str, Option<(&str, &'static str)>); 5] = [
                            ("Crea un proyecto en Google Cloud (por ejemplo «Notas»).", Some(("Abrir Google Cloud", "https://console.cloud.google.com/projectcreate"))),
                            ("Habilita la Google Calendar API en ese proyecto.", Some(("Abrir Calendar API", "https://console.cloud.google.com/apis/library/calendar-json.googleapis.com"))),
                            ("En Google Auth Platform → Branding pon el nombre «Notas» y tu correo; en Público elige Externo.", Some(("Abrir Google Auth Platform", "https://console.cloud.google.com/auth/overview"))),
                            ("En Público presiona «Publicar app» (si queda en Prueba, el permiso vence cada 7 días).", None),
                            ("En Clientes → Crear cliente elige «App de escritorio» y pega aquí el ID y el secreto.", Some(("Abrir Clientes", "https://console.cloud.google.com/auth/clients"))),
                        ];
                        for (i, (text, link)) in steps.iter().enumerate() {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(format!("{}. {text}", i + 1)).size(12.5));
                                if let Some((label, url)) = link {
                                    if ui.link(RichText::new(*label).size(12.5)).clicked() {
                                        changes.push(Change::OpenUrl(url));
                                    }
                                }
                            });
                        }
                    });
            });
        ui.add_space(14.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!("Otros calendarios (Outlook, Apple) llegarán más adelante. Mientras, puedes importar {}.", agenda::ICS_FILE))
                    .size(12.5)
                    .color(MUTED),
            );
            if ui.link(RichText::new("Abrir carpeta").size(12.5)).clicked() {
                changes.push(Change::Do(Action::OpenExternal(self.vault.root.clone())));
            }
        });
    }
}

fn section_shortcuts(ui: &mut Ui) {
    heading(ui, "Atajos", "Atajos de teclado (en Mac, Cmd en lugar de Ctrl).");
    ui.add_space(8.0);
    let keys = [
        ("Ctrl + N", "Nueva nota"),
        ("Ctrl + D", "Nota de hoy"),
        ("Ctrl + H", "Hoy: atrasado, hoy y esta semana"),
        ("Ctrl + K", "Preguntar a tus notas"),
        ("Ctrl + T / Ctrl + W", "Nueva pestaña / cerrar pestaña"),
        ("Ctrl + Tab", "Pestaña siguiente (con Shift, la anterior); Ctrl + 1…9 salta a una"),
        ("Ctrl + clic", "Abrir la nota en otra pestaña"),
        ("Ctrl + R", "Nueva reunión"),
        ("Esc", "Cerrar la reunión, la búsqueda o la vista"),
        ("Ctrl + F", "Buscar en todas las notas"),
        ("Ctrl + S", "Guardar ahora (se guarda solo igual)"),
        ("Ctrl + ,", "Configuración"),
        ("Tab", "Una vez: parte de la nota de arriba · dos: ítem de lista"),
        ("Shift + Tab", "Quitar un nivel de sangría"),
        ("Enter", "En una lista, sigue la lista (en un ítem vacío, sale)"),
        ("#palabra", "Etiqueta (se ve como píldora, sin el #)"),
        ("- [ ] / - [x]", "Tarea pendiente / hecha (clic en la casilla)"),
        ("due:2026-09-26", "Fecha de una tarea"),
    ];
    egui::Grid::new("atajos").num_columns(2).spacing([28.0, 10.0]).show(ui, |ui| {
        for (k, what) in keys {
            Frame::new()
                .fill(BG_SIDE)
                .stroke(Stroke::new(1.0, theme::BORDER))
                .corner_radius(5)
                .inner_margin(Margin::symmetric(8, 2))
                .show(ui, |ui| ui.label(RichText::new(k).size(12.5).monospace()));
            ui.label(RichText::new(what).size(13.5));
            ui.end_row();
        }
    });
}

fn section_about(ui: &mut Ui, s: &mut Settings, changes: &mut Vec<Change>) {
    heading(ui, "Acerca de", "Notas rápidas con espacios de trabajo, reuniones e IA que organiza sola.");
    ui.add_space(12.0);
    ui.label(RichText::new(format!("Notas {}", env!("CARGO_PKG_VERSION"))).font(theme::bold(17.0)));
    ui.label(RichText::new("Código abierto (MIT) · hecho en Rust con egui").size(12.5).color(MUTED));
    ui.add_space(10.0);
    if ui.link(RichText::new(format!("{}  {REPO}", icon::GITHUB_LOGO)).size(13.0)).clicked() {
        changes.push(Change::OpenUrl(REPO));
    }
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        let b = egui::Button::new(format!("{}  Buscar actualizaciones", icon::ARROW_CLOCKWISE));
        if ui.add_enabled(s.update.is_none(), b).clicked() {
            changes.push(Change::CheckUpdate);
        }
        ui.add_space(10.0);
        match (&s.update, &s.update_result) {
            (Some(_), _) => {
                ui.add(egui::Spinner::new().size(14.0));
            }
            (None, Some(Ok(None))) => chip(ui, &format!("{} Tienes la última versión", icon::CHECK_CIRCLE), true),
            (None, Some(Ok(Some(tag)))) => {
                ui.label(RichText::new(format!("Hay una versión nueva: {tag}")).size(13.0).color(ACCENT));
                if ui.link("Descargar").clicked() {
                    changes.push(Change::OpenUrl("https://github.com/jpreyes/nodex-notes/releases/latest"));
                }
            }
            (None, Some(Err(e))) => chip(ui, &format!("{} {e}", icon::WARNING_CIRCLE), false),
            (None, None) => {}
        }
    });
}

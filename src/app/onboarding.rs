//! La primera vez que se abre la app: tres pasos cortos para empezar a escribir sin configurar
//! nada. 1) dónde se guardan las notas; 2) la cuenta (con la prueba de 14 días de la IA incluida;
//! se puede saltar); 3) cómo se usa, y a escribir.

use super::*;

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Step {
    Folder,
    Account,
    Ready,
}

pub(super) struct Onboarding {
    pub step: Step,
    email: String,
    code: String,
}

impl Onboarding {
    pub(super) fn new() -> Onboarding {
        Onboarding { step: Step::Folder, email: String::new(), code: String::new() }
    }
}

impl NotesApp {
    /// El asistente de la primera vez (una ventana encima de la app).
    pub(super) fn onboarding_window(&mut self, ctx: &egui::Context) {
        let Some(ob) = self.onboarding.as_mut() else { return };
        let step = ob.step;
        let signed_in = !self.cfg.cuenta.is_empty() && self.cfg.codigo_ia.starts_with("ns-");
        let server = crate::account::server(&self.cfg);
        let busy = self.acct.busy;
        let code_sent = self.acct.code_sent_to.clone();
        let error = self.acct.error.clone();
        let folder = self.vault.root.clone();
        let mut next: Option<Step> = None;
        let mut pick = false;
        let (mut send, mut enter, mut microsoft, mut done) = (false, false, false, false);
        egui::Modal::new(Id::new("primera-vez")).show(ctx, |ui| {
            ui.set_width(480.0);
            let dots = |ui: &mut Ui| {
                ui.horizontal(|ui| {
                    for s in [Step::Folder, Step::Account, Step::Ready] {
                        let c = if s == step { ACCENT } else { theme::BORDER };
                        let (r, _) = ui.allocate_exact_size(egui::vec2(22.0, 5.0), Sense::hover());
                        ui.painter().rect_filled(r, 2.5, c);
                    }
                });
            };
            dots(ui);
            ui.add_space(10.0);
            match step {
                Step::Folder => {
                    ui.label(RichText::new("Te damos la bienvenida a Notas").font(theme::bold(20.0)));
                    ui.add_space(4.0);
                    ui.label(RichText::new("Escribe como en un bloc, una idea por línea. La IA las ordena sola: cada cosa a su espacio, con etiquetas, tareas y fechas.").size(13.5));
                    ui.add_space(12.0);
                    ui.label(RichText::new("Tus notas se guardan aquí, como archivos de texto que siempre son tuyos:").size(13.0).color(MUTED));
                    let path = folder.display().to_string();
                    ui.add(egui::Label::new(RichText::new(format!("{} {path}", icon::FOLDER_SIMPLE)).size(13.0)).truncate()).on_hover_text(&path);
                    if ui.small_button("Elegir otra carpeta…").clicked() {
                        pick = true;
                    }
                    ui.add_space(4.0);
                    let cloud = crate::dropbox::contains(&folder) || folder.components().any(|c| c.as_os_str().to_string_lossy().starts_with("OneDrive"));
                    ui.label(
                        RichText::new(if cloud {
                            "Está en tu nube: tus notas estarán en todos tus equipos."
                        } else {
                            "Si usas Dropbox u OneDrive, elige una carpeta dentro de ella para tener tus notas en todos tus equipos."
                        })
                        .size(12.5)
                        .color(MUTED),
                    );
                    ui.add_space(14.0);
                    if ui.add(egui::Button::new(RichText::new("Seguir").color(Color32::WHITE)).fill(ACCENT)).clicked() {
                        next = Some(if server.is_some() && !signed_in { Step::Account } else { Step::Ready });
                    }
                }
                Step::Account => {
                    ui.label(RichText::new("Tu cuenta").font(theme::bold(20.0)));
                    ui.add_space(4.0);
                    ui.label(RichText::new("Con una cuenta, la IA viene incluida: 14 días gratis, sin claves ni configuración. Y tu configuración te sigue a tus otros equipos.").size(13.5));
                    ui.add_space(12.0);
                    let Some(ob) = self.onboarding.as_mut() else { return };
                    match &code_sent {
                        None => {
                            ui.label(RichText::new("Tu correo (te mandamos un código de 6 dígitos):").size(13.0).color(MUTED));
                            ui.horizontal(|ui| {
                                let r = ui.add(egui::TextEdit::singleline(&mut ob.email).hint_text("tu@correo.cl").desired_width(260.0));
                                let ok = ob.email.contains('@') && !busy;
                                if ui.add_enabled(ok, egui::Button::new("Enviar código")).clicked() || (ok && r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))) {
                                    send = true;
                                }
                            });
                        }
                        Some(to) => {
                            ui.label(RichText::new(format!("Escribe el código que enviamos a {to}:")).size(13.0).color(MUTED));
                            ui.horizontal(|ui| {
                                let r = ui.add(egui::TextEdit::singleline(&mut ob.code).hint_text("123456").desired_width(110.0));
                                if !r.has_focus() && ob.code.is_empty() {
                                    r.request_focus();
                                }
                                let ok = ob.code.trim().len() >= 6 && !busy;
                                if ui.add_enabled(ok, egui::Button::new(RichText::new("Entrar").color(Color32::WHITE)).fill(ACCENT)).clicked()
                                    || (ok && r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)))
                                {
                                    enter = true;
                                }
                            });
                        }
                    }
                    ui.add_space(8.0);
                    if ui.add_enabled(!busy, egui::Button::new(format!("{}  Entrar con Microsoft", icon::WINDOWS_LOGO))).clicked() {
                        microsoft = true;
                    }
                    if busy {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(RichText::new("Un momento…").size(12.5).color(MUTED));
                        });
                    }
                    if let Some(e) = &error {
                        ui.label(RichText::new(format!("{} {e}", icon::WARNING_CIRCLE)).size(12.5).color(RED));
                    }
                    ui.add_space(14.0);
                    if ui.link(RichText::new("Ahora no (puedes usar tu propia clave de IA, o ninguna)").size(12.5)).clicked() {
                        next = Some(Step::Ready);
                    }
                }
                Step::Ready => {
                    ui.label(RichText::new("¡Listo! A escribir").font(theme::bold(20.0)));
                    ui.add_space(8.0);
                    for (k, what) in [
                        ("Una idea por línea", "la IA la lleva a su espacio, con etiquetas, tareas y fechas"),
                        ("Hoy (Ctrl+D)", "la nota del día, para anotar lo que vaya surgiendo"),
                        ("Ctrl+R", "una reunión: cada línea con su hora, y un resumen al cerrarla"),
                        ("Ctrl+K", "pregúntale a la IA sobre todas tus notas"),
                        ("Ctrl+Enter", "la línea se vuelve tarea"),
                    ] {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(k).size(13.5).strong());
                            ui.label(RichText::new(format!("— {what}")).size(13.0).color(MUTED));
                        });
                    }
                    if !signed_in && self.ai.is_err() {
                        ui.add_space(8.0);
                        ui.label(RichText::new("La IA aún no está activa: entra con tu cuenta o pon tu clave en Configuración → Inteligencia artificial.").size(12.5).color(WARN));
                    }
                    ui.add_space(14.0);
                    if ui.add(egui::Button::new(RichText::new("Empezar a escribir").color(Color32::WHITE)).fill(ACCENT)).clicked() {
                        done = true;
                    }
                }
            }
        });
        if pick {
            if let Some(p) = rfd::FileDialog::new().set_title("Carpeta de notas").set_directory(&folder).pick_folder() {
                self.change_folder(p);
            }
        }
        let (email, code) = self.onboarding.as_ref().map(|o| (o.email.clone(), o.code.clone())).unwrap_or_default();
        if send {
            self.account_send_code(email.clone());
        }
        if enter {
            if let Some(to) = code_sent {
                self.account_sign_in(to, code, true);
            }
        }
        if microsoft {
            self.account_sign_in_microsoft(true);
        }
        // Al entrar, se pasa solo al último paso.
        if step == Step::Account && signed_in {
            next = Some(Step::Ready);
        }
        if let (Some(s), Some(ob)) = (next, self.onboarding.as_mut()) {
            ob.step = s;
        }
        if done {
            self.onboarding = None;
            self.focus_editor = true;
        }
    }
}

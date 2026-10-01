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
    login: super::account_ui::LoginForm,
}

impl Onboarding {
    pub(super) fn new() -> Onboarding {
        // La primera vez, lo normal es crear la cuenta.
        let login = super::account_ui::LoginForm { mode: super::account_ui::LoginMode::Register, ..Default::default() };
        Onboarding { step: Step::Folder, login }
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
        let notice = self.acct.notice.clone();
        let mut login: Option<super::account_ui::LoginAction> = None;
        let folder = self.vault.root.clone();
        let mut next: Option<Step> = None;
        let mut pick = false;
        let mut done = false;
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
                    ui.label(RichText::new("Con una cuenta, la IA queda activa al entrar (14 días gratis), sin claves ni configuración, y tu configuración te sigue a tus otros equipos. Las cuentas nuevas se aprueban antes de usarse: te avisamos por correo.").size(13.5));
                    ui.add_space(12.0);
                    let Some(ob) = self.onboarding.as_mut() else { return };
                    login = super::account_ui::login_form(ui, &mut ob.login, busy, server.is_some(), error.as_deref(), notice.as_deref(), code_sent.as_deref());
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
        if let Some(a) = login {
            self.account_login(a);
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

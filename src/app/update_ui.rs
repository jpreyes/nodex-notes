//! Versiones nuevas: se buscan y se bajan solas (si está activado); «Actualizar» en la barra de
//! abajo cierra la app y abre la nueva. Al abrir la nueva, se muestra qué trae.

use super::*;
use crate::update::{self, Install, Release};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;

/// Cada cuánto se busca una versión nueva (la primera vez, un rato después de abrir).
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
const FIRST_CHECK: Duration = Duration::from_secs(30);

pub(super) enum Step {
    Idle,
    Checking(Receiver<Result<(Release, Install), String>>),
    UpToDate,
    Downloading(Release, Arc<AtomicU64>, Receiver<Result<PathBuf, String>>),
    /// Bajada y lista: falta el clic en «Actualizar».
    Ready(Release, PathBuf),
    /// Hay una versión nueva, pero esta copia no se actualiza sola.
    Manual(Release),
    Applying(Receiver<Result<(), String>>),
    Failed(String),
}

pub(super) struct Updater {
    pub step: Step,
    /// Cómo está instalada (se sabe después de la primera búsqueda).
    pub install: Option<Install>,
    last_check: Option<Instant>,
    started: Instant,
    /// La persona pidió buscar: se muestra el resultado aunque no haya nada nuevo o falle.
    asked: bool,
    /// Se acaba de instalar una versión: «Qué hay de nuevo» (una vez).
    pub news: Option<update::Pending>,
}

impl Default for Updater {
    fn default() -> Self {
        Updater { step: Step::Idle, install: None, last_check: None, started: Instant::now(), asked: false, news: None }
    }
}

/// Solo en compilaciones de prueba: busca enseguida y actualiza sin esperar el clic
/// (con «listo», se queda esperando el clic).
fn demo() -> bool {
    cfg!(debug_assertions) && std::env::var_os("NODEX_DEMO_ACTUALIZAR").is_some()
}

fn demo_applies() -> bool {
    demo() && std::env::var("NODEX_DEMO_ACTUALIZAR").is_ok_and(|v| v != "listo")
}

impl NotesApp {
    /// Al abrir: ¿se acaba de actualizar?
    pub(super) fn take_update_news(&mut self) {
        let Some(p) = update::take_pending() else { return };
        if p.installed {
            self.updater.news = Some(p);
        } else {
            self.msg(tf!("No se instaló la versión {new}: sigues con la {cur}. Puedes intentarlo de nuevo desde Configuración → Acerca de.", new = p.tag.trim_start_matches('v'), cur = update::current()));
        }
    }

    /// Buscar una versión nueva (`asked`: lo pidió la persona).
    pub(super) fn check_update(&mut self, asked: bool) {
        if matches!(self.updater.step, Step::Checking(_) | Step::Downloading(..) | Step::Applying(_)) {
            self.updater.asked |= asked;
            return;
        }
        // Ya está bajada: no hace falta buscar de nuevo.
        if matches!(self.updater.step, Step::Ready(..)) {
            return;
        }
        self.updater.asked = asked;
        self.updater.last_check = Some(Instant::now());
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let r = update::fetch_latest().map(|rel| (rel, update::detect()));
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        self.updater.step = Step::Checking(rx);
    }

    /// Desde `poll`: busca sola de vez en cuando y avanza lo que esté en curso.
    pub(super) fn poll_update(&mut self) {
        let mut failed = None;
        let u = &mut self.updater;
        let step = std::mem::replace(&mut u.step, Step::Idle);
        u.step = match step {
            Step::Checking(rx) => match rx.try_recv() {
                Ok(Ok((rel, install))) => {
                    let asset = install.asset_name().and_then(|n| rel.asset(n)).cloned();
                    u.install = Some(install.clone());
                    if !update::is_newer(&rel.tag, update::current()) {
                        Step::UpToDate
                    } else if let Some(asset) = asset {
                        let progress = Arc::new(AtomicU64::new(0));
                        let (tx, rx) = std::sync::mpsc::channel();
                        let (p, ctx, dir) = (progress.clone(), self.ctx.clone(), update::download_dir(&rel.tag));
                        std::thread::spawn(move || {
                            let r = update::download(&asset, &dir, &p).and_then(|f| update::prepare(&install, &f));
                            let _ = tx.send(r);
                            ctx.request_repaint();
                        });
                        Step::Downloading(rel, progress, rx)
                    } else {
                        Step::Manual(rel)
                    }
                }
                Ok(Err(e)) => if u.asked { Step::Failed(e) } else { Step::Idle },
                Err(std::sync::mpsc::TryRecvError::Empty) => Step::Checking(rx),
                Err(_) => Step::Idle,
            },
            Step::Downloading(rel, p, rx) => match rx.try_recv() {
                Ok(Ok(file)) => Step::Ready(rel, file),
                Ok(Err(e)) => if u.asked { Step::Failed(e) } else { Step::Idle },
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    // Mientras baja, el avance se ve en Acerca de.
                    if self.settings.is_some() {
                        self.ctx.request_repaint_after(Duration::from_millis(250));
                    }
                    Step::Downloading(rel, p, rx)
                }
                Err(_) => Step::Idle,
            },
            Step::Applying(rx) => match rx.try_recv() {
                Ok(Ok(())) => {
                    // La nueva ya se está abriendo (y espera a que esta termine de guardar).
                    if let Some(b) = &self.background {
                        b.set_quitting();
                    }
                    self.ctx.send_viewport_cmd(ViewportCommand::Close);
                    Step::Idle
                }
                Ok(Err(e)) => {
                    failed = Some(tf!("No se pudo actualizar: {e}", e = e));
                    Step::Failed(e)
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => Step::Applying(rx),
                Err(_) => Step::Idle,
            },
            other => other,
        };
        if let Some(m) = failed {
            self.msg(m);
        }
        if demo_applies() && matches!(self.updater.step, Step::Ready(..)) {
            self.apply_update();
        }
        // Buscar sola.
        let u = &self.updater;
        let wait = if demo() { Duration::from_secs(2) } else { FIRST_CHECK };
        let due = match u.last_check {
            None => u.started.elapsed() >= wait,
            Some(t) => t.elapsed() >= CHECK_EVERY,
        };
        if (self.cfg.actualizar_sola || demo()) && due && matches!(u.step, Step::Idle | Step::UpToDate | Step::Failed(_) | Step::Manual(_)) {
            self.check_update(false);
        }
    }

    /// «Actualizar»: guarda todo, instala la versión nueva y la abre (esta se cierra).
    pub(super) fn apply_update(&mut self) {
        let Step::Ready(rel, file) = std::mem::replace(&mut self.updater.step, Step::Idle) else { return };
        let Some(install) = self.updater.install.clone() else { return };
        self.save();
        self.save_estado();
        self.vault.save_cache_now();
        update::remember_pending(&rel);
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(update::apply(&install, &file));
            ctx.request_repaint();
        });
        self.updater.step = Step::Applying(rx);
    }

    /// En la barra de abajo: «Actualizar a la X» cuando ya está lista.
    pub(super) fn update_status(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        match &self.updater.step {
            Step::Ready(rel, _) => {
                let in_meeting = self.meeting.is_some();
                let b = egui::Button::new(RichText::new(format!("{} {}", icon::ARROW_CIRCLE_UP, tf!("Actualizar a la {v}", v = rel.version()))).size(12.5).color(theme::c(Color32::WHITE)))
                    .fill(ACCENT());
                let hint = if in_meeting {
                    t!("Al terminar la reunión")
                } else {
                    t!("Notas se cierra y se abre con la versión nueva, en unos segundos. No se pierde nada.")
                };
                if ui.add_enabled(!in_meeting, b).on_hover_text(hint).on_disabled_hover_text(hint).clicked() {
                    action = Some(Action::ApplyUpdate);
                }
                ui.add_space(12.0);
            }
            Step::Manual(rel) if self.updater.install != Some(Install::Store) => {
                let text = RichText::new(format!("{} {}", icon::ARROW_CIRCLE_UP, tf!("Versión nueva: {v}", v = rel.version()))).size(12.5).color(ACCENT());
                if ui.add(egui::Label::new(text).sense(Sense::click())).on_hover_text(t!("Descargarla desde GitHub")).clicked() {
                    gcal::open_browser(update::RELEASES_PAGE);
                }
                ui.add_space(12.0);
            }
            Step::Applying(_) => {
                ui.add(egui::Spinner::new().size(12.0));
                ui.label(RichText::new(t!("Actualizando…")).size(12.5).color(MUTED()));
                ui.add_space(12.0);
            }
            _ => {}
        }
        action
    }

    /// En Configuración → Acerca de.
    pub(super) fn update_settings(&self, ui: &mut Ui, changes: &mut Vec<settings::Change>) {
        use settings::Change;
        ui.horizontal(|ui| {
            let busy = matches!(self.updater.step, Step::Checking(_) | Step::Downloading(..) | Step::Applying(_));
            match &self.updater.step {
                Step::Ready(rel, _) => {
                    let b = egui::Button::new(RichText::new(format!("{}  {}", icon::ARROW_CIRCLE_UP, tf!("Actualizar a la {v}", v = rel.version()))).color(theme::c(Color32::WHITE))).fill(ACCENT());
                    if ui.add_enabled(self.meeting.is_none(), b).on_hover_text(t!("Notas se cierra y se abre con la versión nueva, en unos segundos")).clicked() {
                        changes.push(Change::Do(Action::ApplyUpdate));
                    }
                }
                _ => {
                    let b = egui::Button::new(format!("{}  {}", icon::ARROW_CLOCKWISE, t!("Buscar actualizaciones")));
                    if ui.add_enabled(!busy, b).clicked() {
                        changes.push(Change::Do(Action::CheckUpdate));
                    }
                }
            }
            ui.add_space(10.0);
            match &self.updater.step {
                Step::Checking(_) | Step::Applying(_) => {
                    ui.add(egui::Spinner::new().size(14.0));
                }
                Step::Downloading(rel, p, _) => {
                    let total = rel.assets.iter().map(|a| a.size).max().unwrap_or(0);
                    let size = self.updater.install.as_ref().and_then(|i| i.asset_name()).and_then(|n| rel.asset(n)).map_or(total, |a| a.size);
                    let pct = if size > 0 { p.load(Ordering::Relaxed) * 100 / size } else { 0 };
                    ui.add(egui::Spinner::new().size(14.0));
                    ui.label(RichText::new(tf!("Bajando la {v}… {pct} %", v = rel.version(), pct = pct)).size(13.0).color(MUTED()));
                }
                Step::Ready(..) => {
                    ui.label(RichText::new(t!("Lista para instalar")).size(13.0).color(MUTED()));
                }
                Step::UpToDate if self.updater.asked => settings::chip(ui, &format!("{} {}", icon::CHECK_CIRCLE, t!("Tienes la última versión")), true),
                Step::Manual(rel) => {
                    ui.label(RichText::new(tf!("Hay una versión nueva: {v}", v = rel.version())).size(13.0).color(ACCENT()));
                    if self.updater.install != Some(Install::Store) && ui.link(t!("Descargar")).clicked() {
                        changes.push(Change::OpenUrl(update::RELEASES_PAGE));
                    }
                }
                Step::Failed(e) => settings::chip(ui, &format!("{} {e}", icon::WARNING_CIRCLE), false),
                _ => {}
            }
        });
        if let Some(hint) = self.updater.install.as_ref().and_then(|i| i.describe()) {
            ui.add_space(4.0);
            ui.label(RichText::new(hint).size(12.5).color(MUTED()));
        }
    }

    /// «Notas se actualizó»: qué trae la versión nueva (una vez, al abrirla).
    pub(super) fn update_news_window(&mut self, ctx: &egui::Context) {
        let Some(news) = &self.updater.news else { return };
        let mut close = false;
        let lines = update::notes_lines(&news.notes);
        let modal = egui::Modal::new(Id::new("novedades")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.label(RichText::new(format!("{} {}", icon::SPARKLE, tf!("Notas se actualizó a la {v}", v = update::current()))).font(theme::bold(17.0)));
            ui.add_space(6.0);
            if lines.is_empty() {
                ui.label(RichText::new(t!("Todo sigue donde estaba.")).size(13.5).color(MUTED()));
            } else {
                ui.label(RichText::new(t!("Qué hay de nuevo:")).size(13.0).color(MUTED()));
                ui.add_space(4.0);
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    for l in lines.iter().take(30) {
                        let (text, bullet) = match l.trim_start().strip_prefix("- ").or_else(|| l.trim_start().strip_prefix("* ")) {
                            Some(rest) => (rest.to_string(), true),
                            None => (l.trim_start_matches('#').trim().to_string(), false),
                        };
                        let indent = l.len() - l.trim_start().len();
                        ui.horizontal_wrapped(|ui| {
                            ui.add_space(indent as f32 * 6.0);
                            if bullet {
                                ui.label(RichText::new("•").size(13.5).color(ACCENT()));
                                ui.label(RichText::new(text.replace("**", "")).size(13.5));
                            } else {
                                ui.label(RichText::new(text.replace("**", "")).font(theme::bold(13.5)));
                            }
                        });
                    }
                });
            }
            ui.add_space(10.0);
            if ui.button(t!("Listo")).clicked() {
                close = true;
            }
        });
        if close || modal.should_close() {
            self.updater.news = None;
        }
    }
}

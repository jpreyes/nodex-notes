//! La sincronización propia en la app. Es automática: al entrar con la cuenta, la carpeta de notas
//! se sincroniza sola (se puede apagar en Configuración → Tu cuenta). Si la carpeta está en
//! Dropbox u OneDrive, se pregunta una vez si copiarla afuera y sincronizar esa (un solo modo por
//! carpeta: si no, Dropbox y la cuenta se pelearían el mismo archivo). Abajo, cómo va.

use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;

/// Copiar la carpeta de notas a una fuera de la nube.
pub(super) struct CopyJob {
    dest: PathBuf,
    done: Arc<AtomicUsize>,
    total: usize,
    rx: Receiver<Result<(), String>>,
}

#[derive(Default)]
pub(super) struct SyncUi {
    handle: Option<crate::sync::Handle>,
    /// Hasta qué versión de las notas se avisó al motor.
    told: u64,
    told_at: Option<Instant>,
    pub copy: Option<CopyJob>,
    /// Se cerró la pregunta «¿Sincronizamos…?» en esta sesión (sin elegir).
    offer_closed: bool,
}

/// ¿La carpeta está en una nube que ya sincroniza (Dropbox, OneDrive, iCloud)?
pub(super) fn in_cloud(root: &Path) -> bool {
    crate::dropbox::contains(root)
        || root.components().any(|c| {
            let s = c.as_os_str().to_string_lossy().to_lowercase();
            s.starts_with("onedrive") || s == "icloud drive" || s == "icloudrive" || s == "mobile documents"
        })
}

/// Una carpeta libre fuera de la nube para las notas («Documentos/Notas», «Notas 2»…).
fn outside_folder() -> PathBuf {
    // En Windows, Documentos puede estar dentro de OneDrive: entonces, la carpeta personal.
    let docs = dirs::document_dir().filter(|d| !in_cloud(d)).or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from("."));
    let mut p = docs.join("Notas");
    let mut i = 2;
    while p.exists() && fs::read_dir(&p).is_ok_and(|mut d| d.next().is_some()) {
        p = docs.join(format!("Notas {i}"));
        i += 1;
    }
    p
}

fn copy_tree(from: &Path, to: &Path, done: &AtomicUsize) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)? {
        let e = e?;
        let target = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_tree(&e.path(), &target, done)?;
        } else {
            fs::copy(e.path(), &target)?;
            done.fetch_add(1, Ordering::Relaxed);
        }
    }
    Ok(())
}

fn count_files(dir: &Path) -> usize {
    fs::read_dir(dir).into_iter().flatten().flatten().map(|e| if e.path().is_dir() { count_files(&e.path()) } else { 1 }).sum()
}

impl NotesApp {
    /// Empieza (o detiene) la sincronización según la configuración y la cuenta.
    pub(super) fn restart_sync(&mut self) {
        self.sync.handle = None; // detiene la anterior
        if self.cfg.sin_sincronizar || !self.signed_in() || in_cloud(&self.vault.root) {
            return;
        }
        let Some(base) = crate::account::server(&self.cfg) else { return };
        let ctx = self.ctx.clone();
        self.sync.handle = Some(crate::sync::start(self.vault.root.clone(), base, self.cfg.codigo_ia.clone(), move || ctx.request_repaint()));
        self.sync.told = self.vault.generation;
    }

    /// Desde `poll`: avisar al motor si algo cambió aquí, y la copia afuera de la nube.
    pub(super) fn poll_sync(&mut self) {
        if let Some(h) = &self.sync.handle {
            // Un cambio en las notas: revisar ya (pero no más de una vez por segundo).
            if self.vault.generation != self.sync.told && self.sync.told_at.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
                self.sync.told = self.vault.generation;
                self.sync.told_at = Some(Instant::now());
                h.kick();
            }
        }
        let Some(job) = &self.sync.copy else { return };
        let Ok(r) = job.rx.try_recv() else {
            self.ctx.request_repaint_after(Duration::from_millis(300));
            return;
        };
        let dest = job.dest.clone();
        self.sync.copy = None;
        match r {
            Ok(()) => {
                self.change_folder(dest.clone());
                self.cfg.sin_sincronizar = false;
                self.save_config();
                self.restart_sync();
                self.msg(format!("Tus notas ahora están en {} y se sincronizan con tu cuenta (la carpeta de Dropbox quedó como estaba)", dest.display()));
            }
            Err(e) => self.msg(format!("No se pudieron copiar tus notas: {e}")),
        }
    }

    /// Encender o apagar la sincronización con la cuenta.
    pub(super) fn set_sync(&mut self, on: bool) {
        self.cfg.sin_sincronizar = !on;
        self.save_config();
        self.restart_sync();
        self.msg(if on { "Tus notas se sincronizan con tu cuenta" } else { "Tus notas ya no se sincronizan con tu cuenta (quedan en este equipo)" });
    }

    /// Copiar las notas a una carpeta fuera de la nube y sincronizar esa.
    pub(super) fn copy_out_of_cloud(&mut self) {
        if self.sync.copy.is_some() {
            return;
        }
        self.save();
        let (from, dest) = (self.vault.root.clone(), outside_folder());
        let total = count_files(&from);
        let done = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = std::sync::mpsc::channel();
        let (d2, dest2, ctx) = (done.clone(), dest.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let r = copy_tree(&from, &dest2, &d2).map_err(|e| e.to_string());
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        self.sync.copy = Some(CopyJob { dest, done, total, rx });
    }

    /// Abajo: cómo va la sincronización.
    pub(super) fn sync_status(&self, ui: &mut Ui) {
        let Some(h) = &self.sync.handle else { return };
        let Ok(s) = h.status.lock().map(|s| s.clone()) else { return };
        let (glyph, text, color) = if s.busy && s.pending > 0 {
            (icon::CLOUD_ARROW_UP, format!("Sincronizando ({})", s.pending), MUTED())
        } else if s.busy {
            (icon::CLOUD_ARROW_UP, "Sincronizando…".to_string(), MUTED())
        } else if let Some(e) = &s.error {
            let short = if e.contains("sin conexión") { "Sin conexión: tus cambios quedan aquí".to_string() } else { e.chars().take(60).collect() };
            (icon::CLOUD_SLASH, short, WARN())
        } else if s.synced_at.is_some() {
            (icon::CLOUD_CHECK, "Sincronizado".to_string(), MUTED())
        } else {
            (icon::CLOUD, "Conectando…".to_string(), MUTED())
        };
        let tip = match (&s.error, s.synced_at) {
            (Some(e), _) => format!("{e}. Se reintenta solo; lo que escribas queda en este equipo y se sube al volver."),
            (None, Some(t)) => format!("Tus notas están al día con tu cuenta (a las {})", t.format("%H:%M")),
            _ => "Tus notas se sincronizan con tu cuenta".into(),
        };
        ui.label(RichText::new(format!("{glyph} {text}")).size(12.5).color(color)).on_hover_text(tip);
        ui.add_space(12.0);
    }

    /// Una vez, al entrar con la cuenta y con la carpeta en Dropbox/OneDrive: «¿Sincronizamos tus
    /// notas con tu cuenta?» (copiarlas afuera, o seguir con Dropbox).
    pub(super) fn sync_offer_window(&mut self, ctx: &egui::Context) {
        let show = self.signed_in()
            && !self.cfg.sin_sincronizar
            && !self.sync.offer_closed
            && self.sync.copy.is_none()
            && self.onboarding.is_none()
            && self.settings.is_none()
            && crate::account::server(&self.cfg).is_some()
            && in_cloud(&self.vault.root);
        if !show {
            return;
        }
        let dest = outside_folder();
        let (mut yes, mut keep, mut close) = (false, false, false);
        let cloud = if crate::dropbox::contains(&self.vault.root) { "Dropbox" } else { "OneDrive" };
        let modal = egui::Modal::new(Id::new("ofrecer-sincronizar")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(RichText::new(format!("{} ¿Sincronizamos tus notas con tu cuenta?", icon::CLOUD_ARROW_UP)).font(theme::bold(17.0)));
            ui.add_space(6.0);
            ui.label(RichText::new(format!("Tus notas están en {cloud}. Con tu cuenta, cada cambio llega en segundos a tus otros equipos, sin depender de {cloud}, y si dos equipos cambian la misma nota, se juntan.")).size(13.5));
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("Para que no se peleen las dos sincronizaciones, tus notas se copian a {} y la app pasa a usar esa carpeta. La de {cloud} queda como está.", dest.display()))
                    .size(12.5)
                    .color(MUTED()),
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new("Sí, sincronizar").color(theme::c(Color32::WHITE))).fill(ACCENT())).clicked() {
                    yes = true;
                }
                if ui.button(format!("Seguir con {cloud}")).clicked() {
                    keep = true;
                }
            });
        });
        if modal.should_close() {
            close = true;
        }
        if yes {
            self.copy_out_of_cloud();
        } else if keep {
            self.cfg.sin_sincronizar = true;
            self.save_config();
            self.msg(format!("Tus notas siguen con {cloud}. Puedes cambiarlo en Configuración → Tu cuenta"));
        } else if close {
            self.sync.offer_closed = true;
        }
    }

    /// En Configuración → Tu cuenta.
    pub(super) fn sync_settings(&self, ui: &mut Ui, changes: &mut Vec<settings::Change>) {
        use settings::Change;
        let cloud = in_cloud(&self.vault.root);
        let hint = "Tus notas en todos tus equipos, sin Dropbox: cada cambio sube a tu cuenta y llega al instante a tus otros equipos. Si dos equipos cambian la misma nota, se juntan. Sin conexión, escribes igual. Se enciende sola al entrar.";
        settings::row(ui, "Sincronizar tus notas", hint, |ui| {
            let mut on = !self.cfg.sin_sincronizar && !cloud;
            ui.add_enabled_ui(!cloud && self.sync.copy.is_none(), |ui| {
                if settings::toggle(ui, &mut on).changed() {
                    changes.push(Change::Sync(on));
                }
            });
        });
        if let Some(job) = &self.sync.copy {
            ui.horizontal(|ui| {
                ui.spinner();
                let n = job.done.load(Ordering::Relaxed);
                ui.label(RichText::new(format!("Copiando tus notas a {}… {n} de {}", job.dest.display(), job.total)).size(12.5).color(MUTED()));
            });
        } else if cloud {
            ui.label(
                RichText::new("Tu carpeta de notas está en Dropbox u OneDrive, que ya la sincroniza: con las dos a la vez se pelearían. Para usar tu cuenta, tus notas se copian a una carpeta fuera de la nube (la de Dropbox queda como está).")
                    .size(12.5)
                    .color(MUTED()),
            );
            ui.add_space(4.0);
            let dest = outside_folder();
            if ui.button(format!("{}  Copiar mis notas a {} y sincronizarlas", icon::COPY, dest.display())).clicked() {
                changes.push(Change::CopyOutOfCloud);
            }
        } else if let Some(h) = self.sync.handle.as_ref() {
            if let Ok(s) = h.status.lock() {
                if s.limit > 0 {
                    let mb = |b: u64| b as f64 / 1_048_576.0;
                    ui.label(RichText::new(format!("Espacio usado: {:.1} MB de {:.0} MB", mb(s.used), mb(s.limit))).size(12.5).color(MUTED()));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clouds_are_recognized() {
        assert!(in_cloud(Path::new("C:/Users/ana/OneDrive/Notas")));
        assert!(in_cloud(Path::new("C:/Users/ana/Dropbox/Notas")));
        assert!(!in_cloud(Path::new("C:/Users/ana/Documents/Notas")));
        // La copia nunca queda en otra nube (Documentos puede estar dentro de OneDrive).
        assert!(!in_cloud(&outside_folder()), "{}", outside_folder().display());
    }
}

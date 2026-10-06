//! Importar notas Markdown (Configuración → General): elegir la carpeta, confirmar y ver el
//! avance. El trabajo va en un hilo aparte; al terminar, las notas aparecen en sus espacios.
//! Por defecto la IA no las reorganiza (con miles de notas gastaría mucho); se puede pedir.

use super::*;
use crate::import;
use std::sync::mpsc::{self, Receiver};

pub(super) enum Msg {
    Progress(usize, usize),
    Done(import::Report),
}

pub(super) enum ImportStep {
    Confirm { src: PathBuf, notes: usize, files: usize, organize: bool },
    Running { rx: Receiver<Msg>, done: usize, total: usize, organize: bool, name: String },
}

impl NotesApp {
    /// Elegir la carpeta a importar.
    pub(super) fn pick_import(&mut self) {
        let Some(src) = rfd::FileDialog::new().set_title(t!("Carpeta con notas Markdown (Obsidian, Notion exportado…)")).pick_folder() else { return };
        if src.starts_with(&self.vault.root) || self.vault.root.starts_with(&src) {
            self.msg(t!("Esa carpeta es (o contiene) tu carpeta de notas: elige otra"));
            return;
        }
        let (notes, files) = import::count(&src);
        if notes == 0 {
            self.msg(t!("En esa carpeta no hay notas Markdown (.md)"));
            return;
        }
        self.import = Some(ImportStep::Confirm { src, notes, files, organize: false });
    }

    fn start_import(&mut self, src: PathBuf, organize: bool) {
        let (tx, rx) = mpsc::channel();
        let root = self.vault.root.clone();
        let ctx = self.ctx.clone();
        let name = src.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        std::thread::spawn(move || {
            let mut last = Instant::now();
            let report = import::run(&src, &root, |done, total| {
                if last.elapsed() >= Duration::from_millis(100) {
                    last = Instant::now();
                    let _ = tx.send(Msg::Progress(done, total));
                    ctx.request_repaint();
                }
            });
            let _ = tx.send(Msg::Done(report));
            ctx.request_repaint();
        });
        self.import = Some(ImportStep::Running { rx, done: 0, total: 0, organize, name });
    }

    fn finish_import(&mut self, report: import::Report, organize: bool, name: &str) {
        self.vault.scan();
        // Lo importado no se manda a la IA, salvo que se haya pedido.
        if !organize {
            for p in &report.created {
                if let Ok(t) = vault::read_text(p) {
                    self.analyzed.insert(ai::fnv(&t));
                }
            }
            self.save_analyzed();
        }
        let mut text = tf!("Importé {notes} de «{name}»", notes = plural(report.notes, "nota"), name = name);
        if report.files > 0 {
            text += &tf!(" y {files}", files = plural(report.files, "archivo"));
        }
        if !report.spaces.is_empty() {
            text += &tf!(", en {spaces}", spaces = report.spaces.join(", "));
        }
        if report.skipped > 0 {
            text += &tf!(" ({n} ya estaban)", n = report.skipped);
        }
        if !report.errors.is_empty() {
            text += &tf!(" · {n} no se pudieron: {error}", n = report.errors.len(), error = report.errors[0]);
        }
        self.msg(text);
        if report.notes > 0 {
            self.notes_space = None;
            self.show_in_tab(View::Notes);
        }
    }

    /// La ventana de importar: confirmar y avance.
    pub(super) fn import_window(&mut self, ctx: &egui::Context) {
        let Some(step) = self.import.as_mut() else { return };
        let mut go: Option<(PathBuf, bool)> = None;
        let mut cancel = false;
        let mut finished: Option<(import::Report, bool, String)> = None;
        match step {
            ImportStep::Confirm { src, notes, files, organize } => {
                let modal = egui::Modal::new(Id::new("importar")).show(ctx, |ui| {
                    ui.set_width(460.0);
                    ui.label(RichText::new(format!("{} {}", icon::DOWNLOAD_SIMPLE, t!("Importar notas"))).font(theme::bold(17.0)));
                    ui.add_space(6.0);
                    ui.label(RichText::new(tf!("{path} · {notes} y {files}", path = src.display(), notes = plural(*notes, "nota"), files = plural(*files, "archivo"))).size(13.0));
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(t!("Cada carpeta de primer nivel pasa a ser un espacio. Las imágenes y archivos se copian a Adjuntos, y los enlaces entre páginas quedan como [[enlaces]]. Tu carpeta original no se toca, y si importas dos veces no se repite nada."))
                            .size(12.5)
                            .color(MUTED()),
                    );
                    ui.add_space(8.0);
                    ui.checkbox(organize, t!("Que la IA las organice después (etiquetas, tareas y fechas)"));
                    if *organize && *notes > 200 {
                        ui.label(RichText::new(tf!("Con {notes} usa bastante IA y toma un buen rato.", notes = plural(*notes, "nota"))).size(12.0).color(WARN()));
                    }
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new(RichText::new(t!("Importar")).color(theme::c(Color32::WHITE))).fill(ACCENT())).clicked() {
                            go = Some((src.clone(), *organize));
                        }
                        if ui.button(t!("Cancelar")).clicked() {
                            cancel = true;
                        }
                    });
                });
                if modal.should_close() {
                    cancel = true;
                }
            }
            ImportStep::Running { rx, done, total, organize, name } => {
                for m in rx.try_iter() {
                    match m {
                        Msg::Progress(d, t) => (*done, *total) = (d, t),
                        Msg::Done(r) => finished = Some((r, *organize, name.clone())),
                    }
                }
                let (d, t) = (*done, *total);
                egui::Modal::new(Id::new("importando")).show(ctx, |ui| {
                    ui.set_width(380.0);
                    ui.label(RichText::new(tf!("Importando «{name}»…", name = name)).font(theme::bold(15.0)));
                    ui.add_space(8.0);
                    let frac = if t == 0 { 0.0 } else { d as f32 / t as f32 };
                    ui.add(egui::ProgressBar::new(frac).text(tf!("{d} de {t}", d = d, t = t)));
                });
            }
        }
        if cancel {
            self.import = None;
        } else if let Some((src, organize)) = go {
            self.start_import(src, organize);
        } else if let Some((r, organize, name)) = finished {
            self.import = None;
            self.finish_import(r, organize, &name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lo importado queda en sus espacios y, si no se pidió, la IA no lo reorganiza.
    #[test]
    fn imported_notes_are_not_sent_to_the_ai() {
        let base = std::env::temp_dir().join(format!("nodex-importar-app-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        let (src, dir) = (base.join("Obsidian"), base.join("Notas"));
        fs::create_dir_all(src.join("Obra")).unwrap();
        fs::create_dir_all(dir.join("General")).unwrap();
        fs::write(src.join("Obra").join("Muro.md"), "Revisar el drenaje del muro con el inspector\n").unwrap();
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        let report = import::run(&src, &dir, |_, _| {});
        app.finish_import(report, false, "Obsidian");
        let muro = dir.join("Obra").join("Muro.md");
        assert!(app.vault.get(&muro).is_some() && app.vault.workspaces.contains(&"Obra".to_string()));
        assert!(!app.unorganized().contains(&muro), "no va a la IA");
        assert_eq!(app.view, View::Notes);
        let _ = fs::remove_dir_all(&base);
    }
}

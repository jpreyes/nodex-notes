//! Archivar notas: salen de la vista (listas, búsqueda, Tu día y la IA) sin borrarse. Van a
//! `Archivo/<espacio>/<nota>.md` y se ven en «Archivadas», desde donde vuelven a su espacio.
//! Las notas del día se archivan en `Archivo/Diario/`.

use super::*;
use std::time::SystemTime;

/// Una nota archivada.
pub(super) struct Archived {
    pub path: PathBuf,
    /// De qué espacio era (o «Diario»).
    pub from: String,
    pub modified: Option<SystemTime>,
}

impl NotesApp {
    fn archive_dir(&self) -> PathBuf {
        self.vault.root.join(vault::ARCHIVE)
    }

    /// Las notas archivadas, la más reciente primero.
    pub(super) fn archived(&self) -> Vec<Archived> {
        let mut out = Vec::new();
        for d in fs::read_dir(self.archive_dir()).into_iter().flatten().flatten() {
            if !d.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let from = d.file_name().to_string_lossy().into_owned();
            for e in fs::read_dir(d.path()).into_iter().flatten().flatten() {
                let path = e.path();
                if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("md")) && e.file_type().is_ok_and(|t| t.is_file()) {
                    out.push(Archived { modified: vault::modified(&path), path, from: from.clone() });
                }
            }
        }
        // Por espacio (el que tiene lo más reciente, primero) y, dentro, lo más reciente primero.
        out.sort_by(|a, b| b.modified.cmp(&a.modified));
        let mut order: Vec<String> = Vec::new();
        for a in &out {
            if !order.contains(&a.from) {
                order.push(a.from.clone());
            }
        }
        out.sort_by_key(|a| order.iter().position(|f| *f == a.from));
        out
    }

    /// Ruta libre en una carpeta para ese nombre de archivo ("Muro.md", "Muro 2.md"…).
    fn free_path(dir: &Path, stem: &str) -> PathBuf {
        let mut p = dir.join(format!("{stem}.md"));
        let mut i = 2;
        while p.exists() {
            p = dir.join(format!("{stem} {i}.md"));
            i += 1;
        }
        p
    }

    /// Archiva una nota (con Deshacer).
    pub(super) fn archive_note(&mut self, path: PathBuf) {
        let is_open = path == self.note.path;
        if is_open {
            self.save();
        }
        if !path.is_file() {
            return;
        }
        let from = if vault::in_diary(&path) { vault::DIARY.to_string() } else { workspace_of(&path).unwrap_or_else(|| vault::DEFAULT_WORKSPACE.into()) };
        let dir = self.archive_dir().join(&from);
        let dest = Self::free_path(&dir, &vault::stem(&path));
        if let Err(e) = fs::create_dir_all(&dir).and_then(|_| fs::rename(&path, &dest)) {
            self.msg(format!("No se pudo archivar: {e}"));
            return;
        }
        self.undo = Some(Undo {
            files: Vec::new(),
            renamed: None,
            agenda: self.agenda.snapshot(),
            at: Instant::now(),
            moved: vec![(path.clone(), dest)],
            created_dir: None,
            apart: Vec::new(),
            keep_tasks: Vec::new(),
            relinks: Vec::new(),
        });
        self.undo_entry = None;
        self.msg(format!("«{}» archivada: la encuentras en Archivadas (abajo a la izquierda)", display_title(&vault::stem(&path))));
        if self.meeting.as_ref().is_some_and(|m| m.path == path) {
            self.meeting = None;
        }
        self.touched.remove(&path);
        self.vault.scan();
        if is_open {
            self.note.dirty = false;
            self.note.disk_mtime = None;
            self.note.disk_len = None;
            let ws = self.ws.clone();
            self.select_workspace(ws);
        }
    }

    /// Archiva un espacio completo: sus notas pasan a `Archivo/<espacio>/` y el espacio deja de
    /// verse. Con Deshacer.
    pub(super) fn archive_space(&mut self, ws: String) {
        self.save();
        if self.meeting.as_ref().is_some_and(|m| workspace_of(&m.path).as_deref() == Some(ws.as_str())) {
            self.close_meeting(Local::now());
        }
        let src = self.vault.root.join(&ws);
        let dest = self.archive_dir().join(&ws);
        let count = fs::read_dir(&src).into_iter().flatten().flatten().filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("md"))).count();
        let mut moved = Vec::new();
        if !dest.exists() {
            if let Err(e) = fs::create_dir_all(self.archive_dir()).and_then(|_| fs::rename(&src, &dest)) {
                self.msg(format!("No se pudo archivar el espacio: {e}"));
                return;
            }
            moved.push((src.clone(), dest.clone()));
        } else {
            // Ya había notas archivadas de ese espacio: se suman una por una.
            for e in fs::read_dir(&src).into_iter().flatten().flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("md")) {
                    let to = Self::free_path(&dest, &vault::stem(&p));
                    if fs::rename(&p, &to).is_ok() {
                        moved.push((p, to));
                    }
                }
            }
            let _ = fs::remove_dir(&src);
        }
        self.undo = Some(Undo {
            files: Vec::new(),
            renamed: None,
            agenda: self.agenda.snapshot(),
            at: Instant::now(),
            moved,
            created_dir: None,
            apart: Vec::new(),
            keep_tasks: Vec::new(),
            relinks: Vec::new(),
        });
        self.undo_entry = None;
        self.touched.retain(|p| workspace_of(p).as_deref() != Some(ws.as_str()));
        self.vault.scan();
        if workspace_of(&self.note.path).as_deref() == Some(ws.as_str()) || self.ws == ws {
            self.note.dirty = false;
            let next = self.vault.workspaces.first().cloned().unwrap_or_else(|| vault::DEFAULT_WORKSPACE.into());
            self.select_workspace(next);
        }
        self.msg(format!("«{ws}» archivado ({}): lo encuentras en Archivo, abajo a la izquierda", plural(count, "nota")));
    }

    /// Saca del archivo todas las notas de un espacio (vuelve a ser un espacio).
    pub(super) fn unarchive_space(&mut self, ws: String) {
        let src = self.archive_dir().join(&ws);
        let dest = if vault::is_diary_dir(&ws) {
            self.vault.diary_path("x").parent().map(Path::to_path_buf).unwrap_or_else(|| self.vault.root.join(vault::DIARY))
        } else {
            self.vault.root.join(&ws)
        };
        let mut n = 0;
        if !dest.exists() && fs::rename(&src, &dest).is_ok() {
            n = fs::read_dir(&dest).into_iter().flatten().count();
        } else {
            let _ = fs::create_dir_all(&dest);
            for e in fs::read_dir(&src).into_iter().flatten().flatten() {
                let p = e.path();
                if fs::rename(&p, Self::free_path(&dest, &vault::stem(&p))).is_ok() {
                    n += 1;
                }
            }
            let _ = fs::remove_dir(&src);
        }
        self.vault.scan();
        self.msg(format!("«{ws}» volvió ({})", plural(n, "nota")));
        if !vault::is_diary_dir(&ws) && self.vault.workspaces.contains(&ws) {
            self.select_workspace(ws);
        }
    }

    /// Devuelve una nota archivada a su espacio (si ya no existe, se vuelve a crear) y la abre.
    pub(super) fn unarchive(&mut self, path: PathBuf) {
        let from = path.parent().and_then(|p| p.file_name()).map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let dir = if vault::is_diary_dir(&from) {
            self.vault.diary_path("x").parent().map(Path::to_path_buf).unwrap_or_else(|| self.vault.root.join(vault::DIARY))
        } else if from.is_empty() || vault::is_reserved_dir(&from) {
            self.vault.root.join(&self.ws)
        } else {
            self.vault.root.join(&from)
        };
        let dest = Self::free_path(&dir, &vault::stem(&path));
        if let Err(e) = fs::create_dir_all(&dir).and_then(|_| fs::rename(&path, &dest)) {
            self.msg(format!("No se pudo sacar del archivo: {e}"));
            return;
        }
        let _ = fs::remove_dir(path.parent().unwrap_or(&path)); // si quedó vacía
        self.vault.scan();
        self.msg(format!("«{}» volvió a {}", display_title(&vault::stem(&dest)), if vault::is_diary_dir(&from) { vault::DIARY } else { from.as_str() }));
        self.open_in_tab(dest, None);
    }

    /// «Archivadas»: buscar, ver y sacar del archivo.
    pub(super) fn archive_view(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        let items = self.archived();
        Self::column(ui, "archivadas", |ui, _| {
            ui.label(RichText::new(format!("{} Archivadas", icon::ARCHIVE)).font(theme::bold(24.0)));
            ui.add_space(4.0);
            ui.label(
                RichText::new("Notas que ya no necesitas a la vista: no salen en las listas, la búsqueda, Tu día ni la IA, pero siguen aquí (en la carpeta Archivo). «Sacar del archivo» la devuelve a su espacio.")
                    .size(13.0)
                    .color(MUTED()),
            );
            ui.add_space(10.0);
            if items.is_empty() {
                ui.label(RichText::new("No hay notas archivadas. Para archivar una, clic derecho sobre ella en la lista de notas → Archivar.").color(MUTED()));
                return;
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.archive_filter)
                    .hint_text(format!("{}  Buscar en las archivadas", icon::MAGNIFYING_GLASS))
                    .desired_width(f32::INFINITY)
                    .margin(Margin::symmetric(8, 5)),
            );
            ui.add_space(10.0);
            let want = vault::fold(self.archive_filter.trim());
            let mut shown = 0;
            let mut group = String::new();
            for a in &items {
                let text = vault::read_text(&a.path).unwrap_or_default();
                let title = display_title(&vault::stem(&a.path));
                if !want.is_empty() && !vault::fold(&title).contains(&want) && !vault::fold(&text).contains(&want) {
                    continue;
                }
                shown += 1;
                // Por espacio: su nombre, cuántas tiene y «Sacar todo el espacio».
                if a.from != group {
                    group = a.from.clone();
                    let n = items.iter().filter(|x| x.from == group).count();
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{} {group}", icon::FOLDER_SIMPLE)).font(theme::bold(15.0)));
                        ui.label(RichText::new(plural(n, "nota")).size(12.5).color(MUTED()));
                        if n > 1 && ui.link(RichText::new(format!("{} Sacar todo el espacio", icon::ARROW_COUNTER_CLOCKWISE)).size(12.5)).clicked() {
                            action = Some(Action::UnarchiveSpace(group.clone()));
                        }
                    });
                    ui.add_space(4.0);
                }
                let open = self.archive_open.as_ref() == Some(&a.path);
                Frame::new().stroke(Stroke::new(1.0, theme::BORDER())).corner_radius(10).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{}  {title}", icon::FILE_TEXT)).font(theme::bold(14.5)));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button(RichText::new(format!("{} Sacar del archivo", icon::ARROW_COUNTER_CLOCKWISE)).size(12.5)).clicked() {
                                action = Some(Action::Unarchive(a.path.clone()));
                            }
                            let label = if open { "Ocultar" } else { "Ver" };
                            if ui.button(RichText::new(label).size(12.5)).clicked() {
                                self.archive_open = if open { None } else { Some(a.path.clone()) };
                            }
                        });
                    });
                    let when = a
                        .modified
                        .map(|m| format!(" · editada el {}", long_date(&chrono::DateTime::<Local>::from(m).format("%Y-%m-%d").to_string())))
                        .unwrap_or_default();
                    ui.label(RichText::new(format!("De {}{when}", a.from)).size(12.5).color(MUTED()));
                    if open {
                        ui.add_space(6.0);
                        ui.label(RichText::new(&text).size(13.5).color(TEXT()));
                    } else {
                        let preview: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).take(2).collect();
                        if !preview.is_empty() {
                            let p: String = preview.join(" · ").chars().take(160).collect();
                            ui.label(RichText::new(p).size(12.5).color(TEXT()));
                        }
                    }
                });
                ui.add_space(8.0);
            }
            if shown == 0 {
                ui.label(RichText::new("Ninguna archivada tiene eso.").color(MUTED()));
            }
        });
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Archivar saca la nota de la vista; Deshacer la devuelve; desde Archivadas vuelve a su espacio.
    #[test]
    fn archive_and_bring_back() {
        let dir = std::env::temp_dir().join(format!("nodex-archivo-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        fs::create_dir_all(dir.join("Obra")).unwrap();
        let muro = dir.join("Obra").join("Muro.md");
        fs::write(&muro, "Revisar el muro\n").unwrap();
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());

        app.apply(Action::Archive(muro.clone()));
        let kept = dir.join(vault::ARCHIVE).join("Obra").join("Muro.md");
        assert!(!muro.exists() && kept.exists());
        assert!(app.vault.get(&muro).is_none() && app.vault.all_notes().is_empty(), "no se ve en ninguna parte");
        assert!(!app.vault.workspaces.contains(&vault::ARCHIVE.to_string()), "Archivo no es un espacio");
        assert_eq!(app.archived().len(), 1);

        app.apply(Action::Undo);
        assert!(muro.exists() && !kept.exists());

        app.apply(Action::Archive(muro.clone()));
        fs::remove_dir_all(dir.join("Obra")).unwrap();
        app.vault.scan();
        app.apply(Action::Unarchive(kept.clone()));
        assert_eq!(fs::read_to_string(&muro).unwrap(), "Revisar el muro\n", "vuelve aunque su espacio ya no exista");
        assert_eq!(app.note.path, muro);
        assert!(app.archived().is_empty());

        // Un espacio completo: sale de la vista y vuelve con todas sus notas.
        fs::write(dir.join("Obra").join("Losa.md"), "Losa\n").unwrap();
        app.vault.scan();
        app.apply(Action::ArchiveSpace("Obra".into()));
        assert!(!app.vault.workspaces.contains(&"Obra".to_string()));
        assert_eq!(app.archived().len(), 2);
        app.apply(Action::UnarchiveSpace("Obra".into()));
        assert!(app.vault.workspaces.contains(&"Obra".to_string()));
        assert!(muro.exists() && dir.join("Obra").join("Losa.md").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}

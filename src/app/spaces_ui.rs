//! Sugerencias de espacios nuevos: la tarjeta en Hoy y crear el espacio (moviendo sus notas).

use super::*;
use crate::spaces::{Idea, Ref};

pub(super) enum SpaceReply {
    Create(String, bool),
    Later(String),
    Never(String),
}

impl NotesApp {
    /// ¿La nota (o la nota de adentro) de una sugerencia todavía existe?
    fn ref_exists(&self, r: &Ref) -> bool {
        let path = self.vault.root.join(format!("{}.md", r.note));
        let text = if path == self.note.path { Some(self.note.text.clone()) } else { self.vault.get(&path).map(|n| n.text.clone()) };
        match text {
            Some(t) => r.unit.is_empty() || doubts::find_unit(&t, &r.unit).is_some(),
            None => false,
        }
    }

    /// Quita de las sugerencias las notas que ya no existen o se movieron.
    pub(super) fn prune_ideas(&mut self) {
        let mut ideas = std::mem::take(&mut self.ideas);
        if ideas.prune(|r| self.ref_exists(r)) {
            let _ = ideas.save(&self.vault.root);
        }
        self.ideas = ideas;
    }

    /// Lo aprendido más los espacios descartados, para el prompt.
    pub(super) fn ai_facts(&self) -> Vec<String> {
        let mut facts = doubts::learned(&self.vault.root);
        facts.extend(self.ideas.rejected.iter().map(|n| format!("No crear un espacio para «{n}».")));
        facts
    }

    pub(super) fn ready_ideas(&self) -> Vec<Idea> {
        self.ideas.ready().cloned().collect()
    }

    pub(super) fn handle_space_reply(&mut self, r: SpaceReply) {
        let root = self.vault.root.clone();
        match r {
            SpaceReply::Create(name, move_notes) => self.create_space(&name, move_notes),
            SpaceReply::Later(name) => {
                self.ideas.snooze(&name);
                let _ = self.ideas.save(&root);
                self.msg(format!("Te vuelvo a sugerir «{name}» cuando haya más notas"));
            }
            SpaceReply::Never(name) => {
                self.ideas.reject(&name);
                let _ = self.ideas.save(&root);
                self.msg(format!("No volveré a sugerir el espacio «{name}»"));
            }
        }
    }

    /// Crea el espacio y, si se pidió, mueve ahí sus notas (con sus tareas). Se puede deshacer.
    fn create_space(&mut self, name: &str, move_notes: bool) {
        self.save();
        let root = self.vault.root.clone();
        let existed = self.vault.workspaces.iter().any(|w| w.eq_ignore_ascii_case(name));
        let ws = match self.vault.create_workspace(name) {
            Ok(ws) => ws,
            Err(e) => {
                self.msg(format!("No se pudo crear el espacio: {e}"));
                return;
            }
        };
        let idea = self.ideas.take(name);
        let _ = self.ideas.save(&root);
        let created_dir = (!existed).then(|| root.join(&ws));
        let snapshot = self.agenda.snapshot();
        let mut files: Vec<(PathBuf, Option<String>)> = Vec::new();
        let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
        let mut count = 0;

        if let Some(idea) = idea.filter(|_| move_notes) {
            // Notas completas: el archivo se mueve al espacio nuevo.
            for r in idea.refs.iter().filter(|r| r.unit.is_empty()) {
                let from = root.join(format!("{}.md", r.note));
                if !from.exists() {
                    continue;
                }
                let to = self.vault.unique_path(&ws, &vault::stem(&from));
                if fs::rename(&from, &to).is_ok() {
                    let _ = self.agenda.retarget(Some(&r.note), &[], &self.rel(&to), &ws);
                    if self.note.path == from {
                        self.note.path = to.clone();
                    }
                    moved.push((from, to));
                    count += 1;
                }
            }
            // Notas de adentro: sus líneas pasan a una nota del espacio nuevo.
            let mut sources: Vec<String> = idea.refs.iter().filter(|r| !r.unit.is_empty()).map(|r| r.note.clone()).collect();
            sources.dedup();
            let mut targets: Vec<(PathBuf, Option<String>, String)> = Vec::new(); // (ruta, antes, texto nuevo)
            for src in sources {
                let path = root.join(format!("{src}.md"));
                let Ok(before) = vault::read_text(&path) else { continue };
                let trailing = before.ends_with('\n');
                let mut text = before.clone();
                for r in idea.refs.iter().filter(|r| r.note == src && !r.unit.is_empty()) {
                    let Some(u) = doubts::find_unit(&text, &r.unit) else { continue };
                    let mut ls: Vec<String> = text.lines().map(str::to_string).collect();
                    let chunk: Vec<String> = ls.drain(u.first..=u.last).collect();
                    let title = vault::sanitize(if r.title.trim().is_empty() { name } else { r.title.trim() });
                    let target = self.vault.note_path(&ws, &title);
                    let entry = match targets.iter().position(|(p, _, _)| *p == target) {
                        Some(i) => i,
                        None => {
                            let b = vault::read_text(&target).ok();
                            let t = b.clone().unwrap_or_default();
                            targets.push((target.clone(), b, t));
                            targets.len() - 1
                        }
                    };
                    let current = targets[entry].2.trim_end().to_string();
                    let body = chunk.join("\n");
                    targets[entry].2 = if current.is_empty() { format!("{body}\n") } else { format!("{current}\n{body}\n") };
                    let ids: Vec<String> = chunk.iter().filter_map(|l| lines::id_of(l)).collect();
                    let _ = self.agenda.retarget(None, &ids, &self.rel(&target), &ws);
                    let mut rest = ls.join("\n");
                    if trailing && !rest.is_empty() {
                        rest.push('\n');
                    }
                    text = rest;
                    count += 1;
                }
                if text != before {
                    files.push((path.clone(), Some(before)));
                    if text.trim().is_empty() {
                        let _ = self.vault.trash(&path);
                    } else {
                        let _ = fs::write(&path, &text);
                        self.analyzed.insert(ai::fnv(&text));
                    }
                }
            }
            for (p, before, text) in targets {
                if fs::write(&p, &text).is_ok() {
                    self.analyzed.insert(ai::fnv(&text));
                    files.push((p, before));
                }
            }
            self.gcal_dirty = true;
            self.save_analyzed();
        }

        self.vault.scan();
        if files.iter().any(|(p, _)| *p == self.note.path) {
            self.note = OpenNote::load(self.note.path.clone());
        }
        if !files.is_empty() || !moved.is_empty() || created_dir.is_some() {
            self.undo = Some(Undo { files, renamed: None, agenda: snapshot, at: Instant::now(), moved, created_dir });
        }
        self.prune_doubts();
        self.prune_ideas();
        let what = if count > 0 { format!(" con {}", plural(count, "nota")) } else { String::new() };
        self.select_workspace(ws.clone());
        self.msg(format!("Espacio «{ws}» creado{what}"));
    }

    /// La tarjeta de una sugerencia de espacio.
    pub(super) fn idea_card(&self, ui: &mut Ui, idea: &Idea) -> Option<SpaceReply> {
        let mut reply = None;
        let n = idea.refs.len();
        Frame::new()
            .fill(Color32::from_rgb(244, 250, 246))
            .stroke(Stroke::new(1.0, Color32::from_rgb(190, 225, 200)))
            .corner_radius(10)
            .inner_margin(Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(format!("{} ¿Crear el espacio «{}»?", icon::FOLDER_PLUS, idea.name)).size(14.5).color(TEXT));
                ui.label(RichText::new(format!("Tienes {} sobre {} y no tiene espacio propio:", plural(n, "nota"), idea.name)).size(12.5).color(MUTED));
                for r in idea.refs.iter().take(5) {
                    let text = if r.unit.is_empty() {
                        format!("•  nota «{}»", r.note.replace('/', " / "))
                    } else {
                        format!("•  «{}»  ·  {}", r.unit, r.note.replace('/', " / "))
                    };
                    ui.add(egui::Label::new(RichText::new(text).size(12.5).color(MUTED).italics()).truncate());
                }
                if n > 5 {
                    ui.label(RichText::new(format!("   y {} más", n - 5)).size(12.5).color(MUTED));
                }
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                    let main = egui::Button::new(RichText::new(format!("Crear y mover {}", plural(n, "nota"))).size(13.0).color(Color32::WHITE))
                        .fill(SUCCESS)
                        .corner_radius(8);
                    if ui.add(main).on_hover_text("Se puede deshacer").clicked() {
                        reply = Some(SpaceReply::Create(idea.name.clone(), true));
                    }
                    if ui.add(egui::Button::new(RichText::new("Solo crear").size(13.0)).fill(Color32::WHITE).corner_radius(8)).clicked() {
                        reply = Some(SpaceReply::Create(idea.name.clone(), false));
                    }
                    if ui.add(egui::Button::new(RichText::new("Ahora no").size(13.0)).fill(Color32::WHITE).corner_radius(8)).clicked() {
                        reply = Some(SpaceReply::Later(idea.name.clone()));
                    }
                    if ui.link(RichText::new("No, gracias").size(12.5).color(MUTED)).on_hover_text("No volver a sugerirlo").clicked() {
                        reply = Some(SpaceReply::Never(idea.name.clone()));
                    }
                });
            });
        ui.add_space(8.0);
        reply
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tres notas sobre LaVet sin espacio: se sugiere; "Crear y mover" las lleva con su tarea; Deshacer lo revierte.
    #[test]
    fn suggested_space_is_created_with_its_notes() {
        let dir = std::env::temp_dir().join(format!("nodex-espacios-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        fs::create_dir_all(dir.join("General")).unwrap();
        let daily = dir.join("General").join("2026-09-25.md");
        let original = "Entregar el LaVet la próxima semana\nRevisar planos del LaVet\nComprar pan\n";
        fs::write(&daily, original).unwrap();
        let visita = dir.join("General").join("Visita LaVet.md");
        fs::write(&visita, "Visita a terreno del LaVet\n").unwrap();
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());

        let a: Analysis = serde_json::from_str(
            r#"{"unidades": [{"id": "L1", "espacio_nuevo": "LaVet", "nota": "Entregas"}, {"id": "L2", "espacio_nuevo": "LaVet"}],
                "tareas": [{"texto": "Entregar el LaVet", "fecha": "2026-10-02", "unidad": "L1"}]}"#,
        )
        .unwrap();
        app.apply_analysis(daily.clone(), ai::fnv(original), a);
        assert!(app.ready_ideas().is_empty(), "con 2 notas todavía no");
        let text = fs::read_to_string(&visita).unwrap();
        let b: Analysis = serde_json::from_str(r#"{"espacio_nuevo": "lavet"}"#).unwrap();
        app.apply_analysis(visita.clone(), ai::fnv(&text), b);
        let ready = app.ready_ideas();
        assert_eq!((ready.len(), ready[0].refs.len()), (1, 3));
        assert!(app.ai_facts().is_empty());

        app.handle_space_reply(SpaceReply::Create("LaVet".into(), true));
        assert!(dir.join("LaVet").join("Visita LaVet.md").exists());
        let entregas = fs::read_to_string(dir.join("LaVet").join("Entregas.md")).unwrap();
        assert!(entregas.starts_with("- [ ] Entregar el LaVet la próxima semana due:2026-10-02 ^"), "{entregas}");
        assert_eq!(fs::read_to_string(dir.join("LaVet").join("LaVet.md")).unwrap(), "Revisar planos del LaVet\n");
        assert_eq!(fs::read_to_string(&daily).unwrap(), "Comprar pan\n");
        let t = &app.agenda.tasks()[0];
        assert_eq!((t.note.as_deref(), t.project.as_str()), (Some("LaVet/Entregas"), "LaVet"));
        assert_eq!(app.ws, "LaVet");

        app.undo_ai();
        assert!(visita.exists() && !dir.join("LaVet").exists());
        assert!(fs::read_to_string(&daily).unwrap().contains("Revisar planos del LaVet"));

        // "No, gracias": no se vuelve a sugerir y la IA lo sabe.
        app.ideas.add("Obra Talca", Ref { note: "General/2026-09-25".into(), unit: "Comprar pan".into(), title: String::new() }, &app.vault.workspaces.clone());
        app.handle_space_reply(SpaceReply::Never("Obra Talca".into()));
        assert_eq!(app.ai_facts(), vec!["No crear un espacio para «Obra Talca».".to_string()]);
        let _ = fs::remove_dir_all(&dir);
    }
}

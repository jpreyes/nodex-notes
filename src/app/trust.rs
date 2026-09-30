//! Confianza en la IA:
//!
//! - **«No, gracias»** en lo que hizo sola (organizar una nota, anotar un correo): lo deshace si
//!   todavía se puede y le enseña a no repetirlo (una línea en `aprendido.txt`, con tus palabras
//!   si las escribes). Queda marcado en «Lo que hizo».
//! - **Sugerir antes de aplicar** (opcional): la IA propone y nada cambia hasta que apruebas. Las
//!   propuestas esperan en «Preguntas» (`.nodex/sugerencias.json`).
//! - **Cuánto acierta**: en «Lo que hizo», los cambios del mes y cuántos se deshicieron o no se
//!   quisieron.

use super::*;
use crate::activity::Kind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const SUGGESTIONS: &str = "sugerencias.json";

/// Lo que la IA propone para una nota, esperando tu visto bueno.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Suggestion {
    /// Nota (relativa, sin ".md") y la huella de su texto cuando se analizó.
    pub note: String,
    pub hash: u64,
    pub at: String,
    /// Lo que haría, en palabras ("«Revisar vigas» → Obra/Muro", "Tarea: …").
    pub resumen: Vec<String>,
    pub analysis: ai::Analysis,
}

/// Qué se está rechazando: algo que hizo la IA (entrada de «Lo que hizo») o una sugerencia.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Rejected {
    Done(String),
    Suggested(String),
}

/// Cuánto acertó la IA este mes: (cambios, deshechos o rechazados).
pub(super) fn month_accuracy(log: &crate::activity::Log) -> (usize, usize) {
    let month = Local::now().format("%Y-%m").to_string();
    let mine = log.entries.iter().filter(|e| e.at.starts_with(&month) && matches!(e.kind, Kind::Organizar | Kind::Correo));
    let (mut n, mut bad) = (0, 0);
    for e in mine {
        n += 1;
        if e.undone || e.rechazado {
            bad += 1;
        }
    }
    (n, bad)
}

/// Lo que haría un análisis, en palabras, para mostrarlo antes de aplicarlo.
pub(super) fn describe(a: &ai::Analysis, text: &str, is_capture: bool) -> Vec<String> {
    let units = lines::units(text);
    let all: Vec<&str> = text.lines().collect();
    let unit_text = |id: &str| -> String {
        units
            .iter()
            .find(|u| u.id.eq_ignore_ascii_case(id.trim()))
            .and_then(|u| all.get(u.first))
            .map(|l| crate::doubts::core(l).chars().take(60).collect())
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    if is_capture {
        for u in a.unidades.iter().filter(|u| !u.espacio.trim().is_empty() && !u.nota.trim().is_empty()) {
            out.push(format!("«{}» → {}/{}", unit_text(&u.id), u.espacio.trim(), u.nota.trim()));
        }
    } else {
        if a.confianza.trim().eq_ignore_ascii_case("alta") && !a.espacio.trim().is_empty() {
            out.push(format!("Mover la nota a {}", a.espacio.trim()));
        }
        if !a.titulo.trim().is_empty() {
            out.push(format!("Título: «{}»", a.titulo.trim()));
        }
    }
    let mut tags: Vec<String> = a.unidades.iter().flat_map(|u| u.etiquetas.iter()).map(|t| organize::clean_tag(t)).filter(|t| !t.is_empty()).collect();
    tags.sort();
    tags.dedup();
    if !tags.is_empty() {
        out.push(format!("Etiquetas: {}", tags.join(", ")));
    }
    for t in a.tareas.iter().filter(|t| !t.texto.trim().is_empty()) {
        let when = Some(t.fecha.trim()).filter(|d| agenda::is_date(d)).map(|d| format!(" · {}", long_date(d))).unwrap_or_default();
        out.push(format!("Tarea: {}{when}", t.texto.trim()));
    }
    for e in a.eventos.iter().filter(|e| agenda::is_date(e.fecha.trim()) && !e.titulo.trim().is_empty()) {
        out.push(format!("Evento: {} · {} {}", e.titulo.trim(), long_date(e.fecha.trim()), e.hora.trim()).trim_end().to_string());
    }
    if a.es_reunion || a.unidades.iter().any(|u| u.es_reunion) {
        out.push("Resumen de la reunión, con decisiones y acuerdos".into());
    }
    if out.is_empty() {
        out.push("Ordenar la nota (sin cambios que mostrar)".into());
    }
    out
}

impl NotesApp {
    fn suggestions_file(&self) -> PathBuf {
        self.vault.root.join(".nodex").join(SUGGESTIONS)
    }

    pub(super) fn suggestions(&self) -> BTreeMap<String, Suggestion> {
        vault::read_text(&self.suggestions_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn save_suggestions(&mut self, s: &BTreeMap<String, Suggestion>) {
        let _ = fs::create_dir_all(self.vault.root.join(".nodex"));
        if let Err(e) = fs::write(self.suggestions_file(), serde_json::to_string_pretty(s).unwrap_or_default()) {
            self.msg(format!("No se pudo guardar la sugerencia: {e}"));
        }
        self.suggestion_count = s.len();
    }

    /// En modo «sugerir»: guarda lo que propone la IA en vez de aplicarlo.
    pub(super) fn suggest(&mut self, path: PathBuf, hash: u64, a: ai::Analysis) {
        let Ok(text) = vault::read_text(&path) else { return };
        if ai::fnv(&text) != hash {
            return; // se siguió escribiendo: se volverá a analizar
        }
        let resumen = describe(&a, &text, capture::is_capture(&vault::stem(&path)));
        let mut all = self.suggestions();
        let note = self.rel(&path);
        all.retain(|_, s| s.note != note);
        all.insert(new_task_id(), Suggestion { note: note.clone(), hash, at: Local::now().format("%Y-%m-%d %H:%M").to_string(), resumen, analysis: a });
        self.save_suggestions(&all);
        // No se vuelve a mandar a la IA mientras espera.
        self.analyzed.insert(hash);
        self.save_analyzed();
        self.touched.remove(&path);
        self.msg(format!("La IA tiene una sugerencia para «{}» (en la ventana de la IA → Preguntas)", vault::stem(&path)));
    }

    /// Aplica una sugerencia (si la nota no cambió desde entonces).
    pub(super) fn accept_suggestion(&mut self, id: &str) {
        let mut all = self.suggestions();
        let Some(s) = all.remove(id) else { return };
        self.save_suggestions(&all);
        let path = self.vault.root.join(format!("{}.md", s.note));
        self.save();
        let current = if path == self.note.path { self.note.text.clone() } else { vault::read_text(&path).unwrap_or_default() };
        if ai::fnv(&current) != s.hash {
            self.msg("La nota cambió desde la sugerencia: la IA la vuelve a mirar");
            self.touched.insert(path);
            return;
        }
        self.apply_analysis(path, s.hash, s.analysis);
    }

    /// Pide «No, gracias» (abre la ventana para decir qué debió hacer).
    pub(super) fn ask_reject(&mut self, what: Rejected) {
        self.rejecting = Some((what, String::new()));
    }

    /// «No, gracias»: deshace (si se puede), enseña y lo deja marcado.
    pub(super) fn reject(&mut self, what: Rejected, why: &str) {
        let root = self.vault.root.clone();
        let why = why.trim();
        match what {
            Rejected::Done(id) => {
                let Some(e) = self.activity.entries.iter().find(|e| e.id == id).cloned() else { return };
                let can_undo = self.undo_entry.as_deref() == Some(id.as_str()) && self.undo.as_ref().is_some_and(|u| u.at.elapsed() < UNDO_WINDOW);
                if can_undo {
                    self.undo_ai();
                }
                let fact = if !why.is_empty() {
                    why.to_string()
                } else {
                    match e.kind {
                        Kind::Correo => format!(
                            "No anotes correos como este, no son importantes para mí: «{}».",
                            e.details.first().map(|d| d.chars().take(140).collect::<String>()).unwrap_or_default()
                        ),
                        _ => format!(
                            "Al organizar «{}» no quise esto: {}.",
                            vault::stem(Path::new(&e.note)),
                            e.details.iter().take(4).cloned().collect::<Vec<_>>().join("; ").chars().take(300).collect::<String>()
                        ),
                    }
                };
                let _ = doubts::learn(&root, &fact);
                if let Some(x) = self.activity.entries.iter_mut().find(|x| x.id == id) {
                    x.rechazado = true;
                    x.undone |= can_undo;
                }
                let _ = self.activity.save(&root);
                self.toast = None;
                self.msg(if can_undo {
                    "Deshecho. La IA lo tendrá en cuenta la próxima vez".to_string()
                } else {
                    "Ya no se puede deshacer solo (hubo otros cambios después), pero la IA lo tendrá en cuenta la próxima vez".to_string()
                });
            }
            Rejected::Suggested(id) => {
                let mut all = self.suggestions();
                let Some(s) = all.remove(&id) else { return };
                self.save_suggestions(&all);
                if !why.is_empty() {
                    let _ = doubts::learn(&root, why);
                }
                let entry = crate::activity::Entry {
                    id: new_task_id(),
                    at: Local::now().format("%Y-%m-%d %H:%M").to_string(),
                    kind: Kind::Organizar,
                    note: s.note.clone(),
                    text: format!("No quisiste lo que sugirió para «{}»", vault::stem(Path::new(&s.note))),
                    details: s.resumen.clone(),
                    undone: false,
                    rechazado: true,
                };
                self.activity.add(entry);
                let _ = self.activity.save(&root);
                self.msg("Sugerencia descartada");
            }
        }
    }

    /// La ventana de «No, gracias»: qué debió hacer (opcional).
    pub(super) fn reject_window(&mut self, ctx: &egui::Context) {
        let Some((what, why)) = self.rejecting.as_mut() else { return };
        let is_suggestion = matches!(what, Rejected::Suggested(_));
        let (mut go, mut close) = (false, false);
        let modal = egui::Modal::new(Id::new("no-gracias")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(RichText::new("No, gracias").font(theme::bold(16.0)));
            ui.add_space(4.0);
            let explain = if is_suggestion {
                "La sugerencia se descarta y la nota queda como está."
            } else {
                "La IA deshace esto (si todavía se puede) y lo recuerda para no repetirlo."
            };
            ui.label(RichText::new(explain).size(13.0).color(MUTED));
            ui.add_space(8.0);
            ui.label(RichText::new("¿Qué debería hacer la próxima vez? (opcional)").size(13.0));
            let r = ui.add(egui::TextEdit::multiline(why).hint_text("Por ejemplo: «los correos del banco no son importantes» o «lo de LaVet va en Docencia»").desired_rows(2).desired_width(f32::INFINITY));
            if !r.has_focus() && why.is_empty() {
                r.request_focus();
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let label = if is_suggestion { "Descartar" } else { "Deshacer y enseñar" };
                if ui.add(egui::Button::new(RichText::new(label).color(Color32::WHITE)).fill(ACCENT)).clicked() {
                    go = true;
                }
                if ui.button("Cancelar").clicked() {
                    close = true;
                }
            });
        });
        if modal.should_close() {
            close = true;
        }
        if go {
            if let Some((what, why)) = self.rejecting.take() {
                self.reject(what, &why);
            }
        } else if close {
            self.rejecting = None;
        }
    }

    /// Las sugerencias que esperan, como tarjetas (en Preguntas).
    pub(super) fn suggestions_ui(&mut self, ui: &mut Ui) -> Option<Action> {
        let all = self.suggestions();
        if all.is_empty() {
            return None;
        }
        let mut action = None;
        let mut accept = None;
        ui.label(RichText::new(format!("{} Lo que propone la IA", icon::SPARKLE)).font(theme::bold(15.0)).color(ACCENT));
        ui.label(RichText::new("Estás en «sugerir antes de aplicar»: nada cambia hasta que lo apruebes.").size(12.5).color(MUTED));
        ui.add_space(6.0);
        for (id, s) in &all {
            Frame::new().stroke(Stroke::new(1.0, theme::BORDER)).corner_radius(10).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let path = self.vault.root.join(format!("{}.md", s.note));
                let r = ui.link(RichText::new(format!("{} {}", icon::FILE_TEXT, display_title(&vault::stem(&path)))).font(theme::bold(14.0)));
                if r.clicked() {
                    action = Some(Action::Open(path, None));
                }
                for d in s.resumen.iter().take(10) {
                    ui.label(RichText::new(format!("·  {d}")).size(13.0));
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(RichText::new(format!("{} Aplicar", icon::CHECK)).color(Color32::WHITE)).fill(ACCENT)).clicked() {
                        accept = Some(id.clone());
                    }
                    if ui.button("No, gracias").clicked() {
                        self.rejecting = Some((Rejected::Suggested(id.clone()), String::new()));
                    }
                });
            });
            ui.add_space(8.0);
        }
        if let Some(id) = accept {
            self.accept_suggestion(&id);
        }
        ui.add_space(10.0);
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::Analysis;

    fn app(dir: &Path) -> NotesApp {
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        let cfg = Config { carpeta_notas: dir.to_path_buf(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        NotesApp::new(cfg, None, egui::Context::default())
    }

    fn analysis() -> Analysis {
        serde_json::from_str(
            r#"{"unidades": [{"id": "L1", "espacio": "Obra", "nota": "Muro", "etiquetas": ["vigas"]}],
                "tareas": [{"texto": "Revisar las vigas", "fecha": "2026-10-02", "unidad": "L1"}]}"#,
        )
        .unwrap()
    }

    /// «No, gracias» deshace lo que hizo la IA, le enseña y lo cuenta en el acierto del mes.
    #[test]
    fn no_thanks_undoes_and_teaches() {
        let dir = std::env::temp_dir().join(format!("nodex-no-gracias-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for w in ["General", "Obra", "Diario"] {
            fs::create_dir_all(dir.join(w)).unwrap();
        }
        let hoy = dir.join("Diario").join("2026-09-30.md");
        fs::write(&hoy, "Revisar vigas del muro\n").unwrap();
        let mut app = app(&dir);
        app.apply_analysis(hoy.clone(), ai::fnv("Revisar vigas del muro\n"), analysis());
        assert!(dir.join("Obra").join("Muro.md").exists());
        let id = app.activity.entries.last().unwrap().id.clone();
        assert_eq!(month_accuracy(&app.activity), (1, 0));

        app.reject(Rejected::Done(id.clone()), "lo de las vigas va en General");
        assert!(!dir.join("Obra").join("Muro.md").exists(), "se deshizo");
        assert_eq!(fs::read_to_string(&hoy).unwrap(), "Revisar vigas del muro\n");
        assert!(doubts::learned(&dir).contains(&"lo de las vigas va en General".to_string()));
        let e = app.activity.entries.iter().find(|e| e.id == id).unwrap();
        assert!(e.rechazado && e.undone);
        assert_eq!(month_accuracy(&app.activity), (1, 1));
        let _ = fs::remove_dir_all(&dir);
    }

    /// Con «sugerir antes de aplicar», nada cambia hasta aprobar; y lo descartado se anota.
    #[test]
    fn suggestions_wait_for_approval() {
        let dir = std::env::temp_dir().join(format!("nodex-sugerir-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for w in ["General", "Obra", "Diario"] {
            fs::create_dir_all(dir.join(w)).unwrap();
        }
        let hoy = dir.join("Diario").join("2026-09-30.md");
        let text = "Revisar vigas del muro\n";
        fs::write(&hoy, text).unwrap();
        let mut app = app(&dir);
        app.suggest(hoy.clone(), ai::fnv(text), analysis());
        assert!(!dir.join("Obra").join("Muro.md").exists(), "aún no se aplica");
        let all = app.suggestions();
        let (id, s) = all.iter().next().unwrap();
        assert_eq!(s.resumen, vec!["«Revisar vigas del muro» → Obra/Muro", "Etiquetas: vigas", "Tarea: Revisar las vigas · viernes 2 oct"]);
        assert!(app.analyzed.contains(&ai::fnv(text)), "no se vuelve a mandar a la IA mientras espera");
        app.accept_suggestion(id);
        assert!(dir.join("Obra").join("Muro.md").exists(), "aprobada: se aplica");
        assert!(app.suggestions().is_empty());

        // Otra, descartada.
        let text2 = "Llamar a Pedro\n";
        fs::write(&hoy, text2).unwrap();
        app.suggest(hoy.clone(), ai::fnv(text2), analysis());
        let id2 = app.suggestions().keys().next().unwrap().clone();
        app.reject(Rejected::Suggested(id2), "");
        assert!(app.suggestions().is_empty());
        assert_eq!(fs::read_to_string(&hoy).unwrap(), text2);
        assert!(app.activity.entries.last().unwrap().rechazado);
        let _ = fs::remove_dir_all(&dir);
    }
}

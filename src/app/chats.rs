//! Historial de conversaciones con la IA: cada conversación de «Conversar» se guarda sola en
//! `.nodex/conversaciones/<id>.json` (viaja con la carpeta, así que se ve en los otros equipos),
//! y se puede retomar o borrar.
//!
//! De cada respuesta se guardan solo las notas y tareas que cita (no todas las que se enviaron).

use super::ask_view::{Source, TaskKey, Turn};
use super::*;
use crate::ask::{self, Block, Inline};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const DIR: &str = "conversaciones";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub(super) struct SavedTask {
    pub id: Option<String>,
    pub text: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub(super) struct SavedTurn {
    pub question: String,
    pub answer: String,
    /// Clave citada ("n3") -> (nota relativa, sin ".md"; cómo se muestra).
    pub sources: BTreeMap<String, (String, String)>,
    pub tasks: BTreeMap<String, SavedTask>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub(super) struct Chat {
    pub id: String,
    /// "2026-09-30 17:06"
    pub created: String,
    pub updated: String,
    pub turns: Vec<SavedTurn>,
}

/// Una conversación en la lista (sin cargarla entera).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ChatInfo {
    pub id: String,
    pub title: String,
    pub updated: String,
    pub turns: usize,
}

/// Las claves citadas en una respuesta ("n3", "t4").
fn cited(blocks: &[Block]) -> Vec<String> {
    let mut out = Vec::new();
    for b in blocks {
        let parts = match b {
            Block::Para(p) => p,
            Block::Item { parts, .. } => parts,
        };
        for p in parts {
            if let Inline::Cite(k, _) = p {
                if !out.contains(k) {
                    out.push(k.clone());
                }
            }
        }
    }
    out
}

fn now() -> String {
    Local::now().format("%Y-%m-%d %H:%M").to_string()
}

impl NotesApp {
    fn chats_dir(&self) -> PathBuf {
        self.vault.root.join(".nodex").join(DIR)
    }

    fn chat_file(&self, id: &str) -> PathBuf {
        self.chats_dir().join(format!("{id}.json"))
    }

    /// Una pregunta respondida, como se guarda.
    fn saved_turn(&self, t: &Turn) -> Option<SavedTurn> {
        let Some(Ok(answer)) = &t.answer else { return None };
        let keys = cited(&t.blocks);
        let sources = keys.iter().filter_map(|k| t.sources.get(k).map(|s| (k.clone(), (self.rel(&s.path), s.label.clone())))).collect();
        let tasks = keys
            .iter()
            .filter_map(|k| t.tasks.get(k).map(|x| (k.clone(), SavedTask { id: x.id.clone(), text: x.text.clone(), note: x.note.clone() })))
            .collect();
        Some(SavedTurn { question: t.question.clone(), answer: answer.clone(), sources, tasks })
    }

    /// Guarda la conversación abierta (se llama al terminar cada respuesta).
    pub(super) fn save_chat(&mut self) {
        let turns: Vec<SavedTurn> = self.ask.turns.iter().filter_map(|t| self.saved_turn(t)).collect();
        if turns.is_empty() {
            return;
        }
        let id = self.ask.chat_id.get_or_insert_with(|| format!("{}-{}", Local::now().format("%Y%m%d-%H%M%S"), new_task_id())).clone();
        let created = std::mem::take(&mut self.ask.created);
        let chat = Chat { id: id.clone(), created: if created.is_empty() { now() } else { created }, updated: now(), turns };
        self.ask.created = chat.created.clone();
        let _ = fs::create_dir_all(self.chats_dir());
        if let Ok(json) = serde_json::to_string_pretty(&chat) {
            if let Err(e) = fs::write(self.chat_file(&id), json) {
                self.msg(format!("No se pudo guardar la conversación: {e}"));
            }
        }
        self.ask.list = None;
    }

    /// Las conversaciones guardadas, la más reciente primero (se lee una vez y se recuerda).
    pub(super) fn chat_list(&mut self) -> Vec<ChatInfo> {
        if let Some(l) = &self.ask.list {
            return l.clone();
        }
        let mut list: Vec<ChatInfo> = fs::read_dir(self.chats_dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| serde_json::from_str::<Chat>(&vault::read_text(&e.path()).ok()?).ok())
            .filter(|c| !c.turns.is_empty())
            .map(|c| ChatInfo { title: c.turns[0].question.clone(), updated: c.updated.clone(), turns: c.turns.len(), id: c.id })
            .collect();
        list.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| b.id.cmp(&a.id)));
        self.ask.list = Some(list.clone());
        list
    }

    /// Retoma una conversación guardada.
    pub(super) fn open_chat(&mut self, id: &str) {
        if self.ask.busy() {
            return;
        }
        let Some(chat) = vault::read_text(&self.chat_file(id)).ok().and_then(|t| serde_json::from_str::<Chat>(&t).ok()) else {
            self.msg("No se pudo abrir esa conversación");
            self.ask.list = None;
            return;
        };
        let root = self.vault.root.clone();
        self.ask.turns = chat
            .turns
            .into_iter()
            .map(|t| Turn {
                blocks: ask::parse_answer(&t.answer),
                answer: Some(Ok(t.answer)),
                question: t.question,
                progress: String::new(),
                sources: t.sources.into_iter().map(|(k, (rel, label))| (k, Source { path: root.join(format!("{rel}.md")), label })).collect(),
                tasks: t.tasks.into_iter().map(|(k, x)| (k, TaskKey { id: x.id, text: x.text, note: x.note })).collect(),
            })
            .collect();
        self.ask.chat_id = Some(chat.id);
        self.ask.created = chat.created;
        self.ask.focus = true;
    }

    /// Empieza una conversación nueva (la anterior ya quedó guardada).
    pub(super) fn new_chat(&mut self) {
        if self.ask.busy() {
            return;
        }
        self.ask.turns.clear();
        self.ask.chat_id = None;
        self.ask.created.clear();
        self.ask.focus = true;
    }

    /// Borra una conversación (se puede deshacer desde la barra inferior).
    pub(super) fn delete_chat(&mut self, id: &str) {
        let file = self.chat_file(id);
        let Ok(text) = vault::read_text(&file) else { return };
        if fs::remove_file(&file).is_err() {
            return;
        }
        if self.ask.chat_id.as_deref() == Some(id) {
            self.ask.turns.clear();
            self.ask.chat_id = None;
            self.ask.created.clear();
        }
        self.ask.list = None;
        self.undo = Some(Undo {
            files: vec![(file, Some(text))],
            renamed: None,
            agenda: self.agenda.snapshot(),
            at: Instant::now(),
            moved: Vec::new(),
            created_dir: None,
            apart: Vec::new(),
            relinks: Vec::new(),
        });
        self.undo_entry = None;
        self.msg("Conversación borrada");
    }

    /// La lista de conversaciones (a la izquierda de Conversar).
    pub(super) fn chats_panel(&mut self, ui: &mut Ui) -> Option<Action> {
        let list = self.chat_list();
        let busy = self.ask.busy();
        ui.add_space(12.0);
        let b = egui::Button::new(RichText::new(format!("{}  Nueva conversación", icon::PLUS)).size(13.0)).min_size(egui::vec2(ui.available_width(), 30.0));
        if ui.add_enabled(!busy, b).clicked() {
            self.new_chat();
        }
        ui.add_space(10.0);
        section(ui, "Conversaciones", None);
        if list.is_empty() {
            ui.label(RichText::new("Aquí quedan tus conversaciones con la IA, para retomarlas.").size(12.5).color(MUTED));
        }
        let mut open = None;
        let mut delete = None;
        egui::ScrollArea::vertical().id_salt("conversaciones").auto_shrink([false, false]).show(ui, |ui| {
            let mut last_day = String::new();
            for c in &list {
                let day = c.updated.get(..10).unwrap_or("").to_string();
                if day != last_day {
                    ui.add_space(6.0);
                    let label = match display_title(&day).as_str() {
                        t @ ("Hoy" | "Ayer") => t.to_string(),
                        _ => long_date(&day),
                    };
                    ui.label(RichText::new(label).size(12.0).color(MUTED));
                    last_day = day;
                }
                let selected = self.ask.chat_id.as_deref() == Some(c.id.as_str());
                let r = list_row(ui, icon::CHAT_CIRCLE_TEXT, &c.title, "", selected).on_hover_text(format!("{}\n{}", c.title, plural(c.turns, "pregunta")));
                if row_trash_button(ui, &r, "Borrar la conversación (se puede deshacer)") {
                    delete = Some(c.id.clone());
                } else if r.clicked() && !busy {
                    open = Some(c.id.clone());
                }
            }
        });
        if let Some(id) = open {
            self.open_chat(&id);
        }
        if let Some(id) = delete {
            self.delete_chat(&id);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cada conversación se guarda sola al terminar una respuesta (solo con lo que cita), se
    /// retoma y se borra con Deshacer.
    #[test]
    fn conversations_are_kept_resumed_and_deleted() {
        let dir = std::env::temp_dir().join(format!("nodex-conversaciones-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        fs::create_dir_all(dir.join("Obra")).unwrap();
        fs::write(dir.join("Obra").join("Muro.md"), "Cubicar el muro\n").unwrap();
        fs::write(dir.join("Obra").join("Losa.md"), "Otra cosa\n").unwrap();
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());

        // Una pregunta respondida (como la deja poll_ask).
        let (_, mut turn) = app.build_request("¿Qué falta en el muro?".into(), Vec::new(), None);
        let muro_key = turn.sources.iter().find(|(_, s)| s.path.ends_with("Muro.md")).map(|(k, _)| k.clone()).unwrap();
        let answer = format!("Falta cubicar el muro [{muro_key}:1].");
        turn.blocks = ask::parse_answer(&answer);
        turn.answer = Some(Ok(answer.clone()));
        app.ask.turns.push(turn);
        app.save_chat();

        let list = app.chat_list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].title, "¿Qué falta en el muro?");
        let saved: Chat = serde_json::from_str(&fs::read_to_string(app.chat_file(&list[0].id)).unwrap()).unwrap();
        assert_eq!(saved.turns[0].sources.len(), 1, "solo la nota citada");
        assert_eq!(saved.turns[0].sources[&muro_key].0, "Obra/Muro");

        // Nueva conversación y después retomar la anterior.
        app.new_chat();
        assert!(app.ask.turns.is_empty());
        app.open_chat(&list[0].id);
        assert_eq!(app.ask.turns.len(), 1);
        assert_eq!(app.ask.turns[0].answer, Some(Ok(answer)));
        assert!(app.ask.turns[0].sources[&muro_key].path.ends_with("Muro.md"));
        // Otra pregunta en la misma conversación la actualiza (no crea otra).
        let (_, mut t2) = app.build_request("¿Y la losa?".into(), Vec::new(), None);
        t2.answer = Some(Ok("Nada pendiente.".into()));
        app.ask.turns.push(t2);
        app.save_chat();
        assert_eq!(app.chat_list().len(), 1);
        assert_eq!(app.chat_list()[0].turns, 2);

        // Borrar y deshacer.
        app.delete_chat(&list[0].id);
        assert!(app.chat_list().is_empty() && app.ask.turns.is_empty());
        app.apply(Action::Undo);
        app.ask.list = None;
        assert_eq!(app.chat_list().len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }
}

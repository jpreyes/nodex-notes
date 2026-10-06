//! Microsoft To Do: cuándo sincronizar, qué hacer con lo que llega de To Do y el panel en Tareas.

use super::*;
use crate::todo::{Change, LocalTask};

/// Aunque no cambie nada aquí, se revisa To Do cada tanto para traer lo que se hizo allá.
const TODO_EVERY: Duration = Duration::from_secs(120);

impl NotesApp {
    /// Respuestas del hilo de To Do (cada cuadro).
    pub(super) fn handle_todo(&mut self) {
        let Some(t) = &mut self.todo else { return };
        let (changes, msgs) = t.poll();
        if !changes.is_empty() {
            self.apply_todo_changes(changes);
        }
        for m in msgs {
            self.msg(m);
        }
    }

    /// Manda las tareas a To Do si cambiaron (o si pasó un rato). Se llama una vez por segundo.
    pub(super) fn maybe_sync_todo(&mut self) {
        let Some(t) = &self.todo else { return };
        if !t.connected || t.busy {
            return;
        }
        let text = vault::read_text(&self.vault.root.join(agenda::TASKS_FILE)).unwrap_or_default();
        let changed = ai::fnv(&text) != self.todo_hash;
        let since = self.todo_last.elapsed();
        if !(changed && since >= Duration::from_secs(3) || since >= TODO_EVERY) {
            return;
        }
        // Cada tarea pendiente necesita su identificador para reconocerla allá.
        if self.agenda.ensure_ids(new_task_id).unwrap_or(false) {
            self.gcal_dirty = true;
        }
        let tasks: Vec<LocalTask> = self
            .agenda
            .tasks()
            .into_iter()
            .filter_map(|t| {
                let id = t.id.clone()?;
                let mut body = String::new();
                if !t.project.is_empty() {
                    body += &format!("Espacio: {}\n", t.project);
                }
                if let Some(n) = t.note.as_deref().filter(|n| !n.is_empty()) {
                    body += &format!("Nota: {n}\n");
                }
                body += "Desde la app Notas.";
                Some(LocalTask { id, title: agenda::display_text(&t.text), due: t.due.filter(|d| agenda::is_date(d)), done: t.done, body })
            })
            .collect();
        self.todo_hash = ai::fnv(&vault::read_text(&self.vault.root.join(agenda::TASKS_FILE)).unwrap_or_default());
        self.todo_last = Instant::now();
        if let Some(t) = &mut self.todo {
            t.sync(tasks);
        }
    }

    /// Lo que se hizo en To Do pasa a Notas: hecha/pendiente, fecha y tareas nuevas.
    fn apply_todo_changes(&mut self, changes: Vec<Change>) {
        let today = today();
        for c in changes {
            let result = match &c {
                Change::Done(id, done) => {
                    let r = self.agenda.set_done_by_id(id, *done, &today).map(|_| ());
                    if let Some(note) = self.task_note(id) {
                        self.sync_task_line(&note, id, *done);
                    }
                    r
                }
                Change::Due(id, due) => {
                    let r = match due {
                        Some(d) => self.agenda.set_due_by_id(id, d).map(|_| ()),
                        None => self.agenda.clear_due_by_id(id),
                    };
                    if let Some(note) = self.task_note(id) {
                        let due = due.clone();
                        self.edit_task_line(&note, id, |line| {
                            let bare: String = line.split(' ').filter(|w| !w.starts_with("due:")).collect::<Vec<_>>().join(" ");
                            let new = match &due {
                                Some(d) => lines::set_meta(&bare, Some(d), None),
                                None => bare,
                            };
                            (new != line).then_some(new)
                        });
                    }
                    r
                }
                Change::New(id, title, due) => {
                    // Agregada en To Do: una tarea de General (sin nota), que dice de dónde vino.
                    let ws = self
                        .vault
                        .workspaces
                        .iter()
                        .find(|w| *w == vault::DEFAULT_WORKSPACE)
                        .or(self.vault.workspaces.first())
                        .cloned()
                        .unwrap_or_else(|| vault::DEFAULT_WORKSPACE.into());
                    let mut line = format!("{today} {} {}", title.trim(), agenda::project_token(&ws));
                    if let Some(d) = due {
                        line += &format!(" due:{d}");
                    }
                    line += &format!(" de:todo id:{id}");
                    self.agenda.add_task(line)
                }
            };
            if let Err(e) = result {
                self.msg(format!("No se pudo actualizar tareas.txt: {e}"));
            }
        }
        self.gcal_dirty = true;
        self.todo_hash = ai::fnv(&vault::read_text(&self.vault.root.join(agenda::TASKS_FILE)).unwrap_or_default());
    }

    /// La nota de donde salió la tarea con ese identificador.
    fn task_note(&self, id: &str) -> Option<String> {
        self.agenda.tasks().into_iter().find(|t| t.id.as_deref() == Some(id))?.note.filter(|n| !n.is_empty())
    }

    /// Estado y botones de To Do, arriba en Tareas.
    pub(super) fn todo_panel(&self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        let Some(t) = &self.todo else { return None };
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("{} Microsoft To Do", icon::CHECK_SQUARE_OFFSET)).font(theme::bold(13.5)));
            if t.connecting {
                ui.label(RichText::new("Esperando tu permiso en el navegador…").size(13.0).color(ACCENT()));
            } else if !t.connected {
                ui.label(RichText::new("Tus tareas también en To Do, en una lista «Notas», en los dos sentidos.").size(13.0).color(MUTED()));
                if ui.link(RichText::new("Conectar").size(13.0)).clicked() {
                    action = Some(Action::TodoConnect);
                }
                if ui.link(RichText::new("Más detalles").size(13.0)).clicked() {
                    action = Some(Action::OpenSettings(Section::Tasks));
                }
                if let Some(e) = &t.last_error {
                    ui.label(RichText::new(e).size(13.0).color(RED()));
                }
            } else {
                let status = if t.busy {
                    "sincronizando…".to_string()
                } else if let Some(e) = &t.last_error {
                    e.clone()
                } else if let Some(s) = t.last_sync {
                    format!("sincronizado a las {}", s.format("%H:%M"))
                } else {
                    "conectado".to_string()
                };
                let color = if t.last_error.is_some() { RED() } else { SUCCESS() };
                ui.label(RichText::new(format!("lista «Notas» · {status}")).size(13.0).color(color));
                if ui.link(RichText::new("Sincronizar ahora").size(13.0)).clicked() {
                    action = Some(Action::TodoSync);
                }
                if ui.link(RichText::new("Desconectar").size(13.0)).clicked() {
                    action = Some(Action::TodoDisconnect);
                }
            }
        });
        action
    }
}

#[cfg(test)]
#[path = "todo_ui_tests.rs"]
mod tests;

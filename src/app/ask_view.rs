//! Vista Preguntar (Ctrl+K): una conversación con la IA sobre todas las notas.
//! Cada dato de la respuesta lleva un número que abre la nota en esa línea, y las
//! tareas de la respuesta se pueden marcar ahí mismo.

use super::*;
use crate::ask::{self, Block, Inline};
use std::sync::mpsc::Receiver;

/// A qué apunta una clave de la respuesta.
#[derive(Clone)]
struct Source {
    path: PathBuf,
    label: String,
}

/// Cómo reconocer una tarea aunque cambie su línea en tareas.txt (al marcarla).
#[derive(Clone)]
struct TaskKey {
    id: Option<String>,
    text: String,
    note: Option<String>,
}

pub(super) struct Turn {
    question: String,
    answer: Option<Result<String, String>>,
    blocks: Vec<Block>,
    progress: String,
    sources: HashMap<String, Source>,
    tasks: HashMap<String, TaskKey>,
}

#[derive(Default)]
pub(super) struct AskState {
    turns: Vec<Turn>,
    input: String,
    rx: Option<Receiver<ask::Msg>>,
    pub(super) focus: bool,
}

impl AskState {
    pub(super) fn busy(&self) -> bool {
        self.rx.is_some()
    }
}

/// La respuesta como texto plano: citas como "(Espacio/Título)" y sin casillas.
fn plain(turn: &Turn) -> String {
    let mut out = String::new();
    for b in &turn.blocks {
        let (prefix, parts) = match b {
            Block::Para(p) => (String::new(), p),
            Block::Item { level, parts, .. } => (format!("{}- ", "  ".repeat(*level as usize)), parts),
        };
        out += &prefix;
        let mut last_cite = String::new();
        for p in parts {
            match p {
                Inline::Text(t) => out += t,
                Inline::Cite(k, _) => {
                    if let Some(s) = turn.sources.get(k).filter(|_| *k != last_cite) {
                        out += &format!(" ({})", s.label);
                    }
                    last_cite = k.clone();
                }
            }
        }
        out.push('\n');
    }
    out
}

/// Texto con **negrita**.
fn rich(text: &str, size: f32, color: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    for (i, part) in text.split("**").enumerate() {
        let font = if i % 2 == 1 { theme::bold(size) } else { FontId::proportional(size) };
        job.append(part, 0.0, fmt(font, color));
    }
    job
}

impl NotesApp {
    /// Envía una pregunta con todas las notas, tareas y agenda.
    pub(super) fn ask(&mut self, question: String) {
        let question = question.trim().to_string();
        if question.is_empty() || self.ask.busy() {
            return;
        }
        self.save();
        let mut sources = HashMap::new();
        let docs: Vec<ask::Doc> = self
            .vault
            .all_notes()
            .into_iter()
            .enumerate()
            .map(|(i, n)| {
                let d: DateTime<Local> = n.modified.into();
                let doc = ask::Doc {
                    key: format!("n{}", i + 1),
                    path: n.path.clone(),
                    workspace: n.workspace.clone(),
                    title: n.title.clone(),
                    date: d.format("%Y-%m-%d").to_string(),
                    text: n.text.clone(),
                };
                sources.insert(doc.key.clone(), Source { path: doc.path.clone(), label: doc.label() });
                doc
            })
            .collect();
        let all = self.agenda.tasks();
        let (pending, done): (Vec<_>, Vec<_>) = all.into_iter().partition(|t| !t.done);
        let chosen: Vec<agenda::Task> = pending.into_iter().chain(done.into_iter().rev().take(30)).collect();
        let mut tasks = HashMap::new();
        let task_list: Vec<(String, agenda::Task)> = chosen
            .into_iter()
            .enumerate()
            .map(|(i, t)| {
                let key = format!("t{}", i + 1);
                tasks.insert(key.clone(), TaskKey { id: t.id.clone(), text: t.text.clone(), note: t.note.clone() });
                (key, t)
            })
            .collect();
        let month_ago = (Local::now() - chrono::Duration::days(30)).format("%Y-%m-%d").to_string();
        let events: Vec<agenda::Event> = self.agenda.events().into_iter().filter(|e| e.date >= month_ago).collect();
        let history: Vec<(String, String)> = self
            .ask
            .turns
            .iter()
            .filter_map(|t| match &t.answer {
                Some(Ok(a)) => Some((t.question.clone(), a.clone())),
                _ => None,
            })
            .collect();
        let input = ask::Input { question: question.clone(), history, docs, tasks: task_list, events, today: today() };
        self.ask.rx = Some(ask::start(&self.cfg, input, self.ctx.clone()));
        self.ask.turns.push(Turn {
            question,
            answer: None,
            blocks: Vec::new(),
            progress: "Buscando en tus notas…".into(),
            sources,
            tasks,
        });
    }

    /// Avances y respuesta del hilo de Preguntar.
    pub(super) fn poll_ask(&mut self) {
        let Some(rx) = &self.ask.rx else { return };
        let mut finished = false;
        for m in rx.try_iter() {
            let Some(turn) = self.ask.turns.last_mut() else { continue };
            match m {
                ask::Msg::Progress(p) => turn.progress = p,
                ask::Msg::Done(r) => {
                    if let Ok(a) = &r {
                        turn.blocks = ask::parse_answer(a);
                    }
                    turn.answer = Some(r);
                    finished = true;
                }
            }
        }
        if finished {
            self.ask.rx = None;
        }
    }

    /// Guarda una respuesta como nota nueva en el espacio actual.
    fn save_answer(&mut self, idx: usize) {
        let Some(turn) = self.ask.turns.get(idx) else { return };
        let q: String = turn.question.trim_end_matches('?').trim_start_matches('¿').chars().take(50).collect();
        let text = format!("Pregunta: {}\n\n{}", turn.question, plain(turn));
        let path = self.vault.unique_path(&self.ws, &format!("Respuesta · {}", q.trim()));
        if let Err(e) = fs::write(&path, &text) {
            self.msg(format!("No se pudo guardar: {e}"));
            return;
        }
        // Es un resumen: no hace falta que la IA vuelva a sacar tareas de ella.
        self.analyzed.insert(ai::fnv(&text));
        self.save_analyzed();
        self.vault.scan();
        self.msg(format!("Respuesta guardada en «{}»", vault::stem(&path)));
        self.open(path, Some(0));
    }

    pub(super) fn ask_view(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        let mut save: Option<usize> = None;
        let mut send: Option<String> = None;
        let tasks_now = self.agenda.tasks();
        let busy = self.ask.busy();
        let n_notes = self.vault.all_notes().len();
        let ai_error = self.ai.as_ref().err().cloned();

        egui::ScrollArea::vertical().id_salt("ask").auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
            let w = ui.available_width();
            let col_w = (w - 64.0).clamp(200.0, COLUMN_MAX);
            ui.horizontal_top(|ui| {
                ui.add_space((w - col_w) / 2.0);
                ui.vertical(|ui| {
                    ui.set_width(col_w);
                    ui.add_space(26.0);
                    if view_header(ui, "Preguntar", &format!("Busca en {}, tus tareas y la agenda; cada dato lleva el número de su nota", plural(n_notes, "nota"))) {
                        action = Some(Action::CloseResults);
                    }
                    if let Some(e) = &ai_error {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(format!("IA no disponible: {e}")).color(RED).size(13.0));
                            if ui.link("Configurar").clicked() {
                                action = Some(Action::OpenSettings(Section::Ai));
                            }
                        });
                        ui.add_space(8.0);
                    }

                    if self.ask.turns.is_empty() {
                        ui.label(RichText::new("Pregunta lo que quieras sobre tus notas. Por ejemplo:").color(MUTED));
                        ui.add_space(6.0);
                        let examples = [
                            "¿Cuáles son todas las tareas que me faltan?".to_string(),
                            "¿Qué tengo para esta semana?".to_string(),
                            format!("Resumen de las notas de {}", self.ws),
                            "¿Qué se habló en la última reunión?".to_string(),
                        ];
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                            for ex in examples {
                                let b = egui::Button::new(RichText::new(&ex).size(13.0)).corner_radius(14);
                                if ui.add(b).clicked() {
                                    send = Some(ex);
                                }
                            }
                        });
                        ui.add_space(18.0);
                    }

                    for (ti, turn) in self.ask.turns.iter().enumerate() {
                        // Pregunta, a la derecha.
                        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                            Frame::new().fill(BG_SIDE).corner_radius(12).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
                                ui.set_max_width(col_w * 0.8);
                                ui.label(RichText::new(&turn.question).size(14.5).color(TEXT));
                            });
                        });
                        ui.add_space(10.0);
                        match &turn.answer {
                            None => {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label(RichText::new(&turn.progress).color(MUTED));
                                });
                            }
                            Some(Err(e)) => {
                                ui.label(RichText::new(format!("No se pudo responder: {e}")).color(RED));
                            }
                            Some(Ok(_)) => {
                                if let Some(a) = self.answer_ui(ui, turn, &tasks_now) {
                                    action = Some(a);
                                }
                                ui.add_space(6.0);
                                ui.horizontal(|ui| {
                                    let small = |t: String| egui::Button::new(RichText::new(t).size(12.5).color(MUTED)).frame(false);
                                    if ui.add(small(format!("{} Guardar como nota", icon::FLOPPY_DISK))).clicked() {
                                        save = Some(ti);
                                    }
                                    if ui.add(small(format!("{} Copiar", icon::COPY))).clicked() {
                                        ui.ctx().copy_text(plain(turn));
                                    }
                                });
                            }
                        }
                        ui.add_space(22.0);
                    }

                    // Campo para preguntar: Enter envía, Shift+Enter hace otra línea.
                    let id = Id::new("ask-input");
                    let focused = ui.memory(|m| m.has_focus(id));
                    let enter = focused
                        && !ui.input(|i| i.modifiers.shift)
                        && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
                    let hint = if self.ask.turns.is_empty() { "Escribe tu pregunta…" } else { "Otra pregunta, o sigue con esta…" };
                    let mut submit = false;
                    Frame::new()
                        .fill(Color32::WHITE)
                        .stroke(Stroke::new(1.0, if focused { ACCENT } else { theme::BORDER }))
                        .corner_radius(10)
                        .inner_margin(Margin::symmetric(10, 6))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let r = ui.add(
                                    egui::TextEdit::multiline(&mut self.ask.input)
                                        .id(id)
                                        .frame(Frame::NONE)
                                        .hint_text(hint)
                                        .desired_rows(1)
                                        .desired_width(ui.available_width() - 34.0)
                                        .font(FontId::proportional(14.5)),
                                );
                                if std::mem::take(&mut self.ask.focus) {
                                    r.request_focus();
                                }
                                let color = if busy || self.ask.input.trim().is_empty() { MUTED } else { ACCENT };
                                let b = egui::Button::new(RichText::new(icon::PAPER_PLANE_RIGHT).size(18.0).color(color)).frame(false);
                                if ui.add(b).on_hover_text("Preguntar (Enter)").clicked() {
                                    submit = true;
                                }
                            });
                        });
                    if (enter || submit) && !busy && !self.ask.input.trim().is_empty() {
                        send = Some(std::mem::take(&mut self.ask.input));
                        self.ask.focus = true;
                    }
                    if !self.ask.turns.is_empty() {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("La IA lee tus notas para responder.").size(12.0).color(MUTED));
                            if !busy && ui.link(RichText::new("Nueva conversación").size(12.0)).clicked() {
                                self.ask.turns.clear();
                                self.ask.focus = true;
                            }
                        });
                    }
                    ui.add_space(40.0);
                });
            });
        });
        if let Some(q) = send {
            self.ask(q);
        }
        if let Some(i) = save {
            self.save_answer(i);
        }
        if self.esc(ui) {
            action = Some(Action::CloseResults);
        }
        action
    }

    /// La respuesta: párrafos, listas, casillas de tareas y citas numeradas; al final, las fuentes.
    fn answer_ui(&self, ui: &mut Ui, turn: &Turn, tasks_now: &[agenda::Task]) -> Option<Action> {
        let mut action = None;
        // Número de cada nota citada, en orden de aparición (con la primera línea citada).
        let mut numbers: Vec<(String, Option<usize>)> = Vec::new();
        let find_task = |key: &str| -> Option<&agenda::Task> {
            let k = turn.tasks.get(key)?;
            tasks_now.iter().find(|t| match &k.id {
                Some(id) => t.id.as_ref() == Some(id),
                None => t.text == k.text && t.note == k.note,
            })
        };
        for b in &turn.blocks {
            let (level, check, parts) = match b {
                Block::Para(p) => (None, None, p),
                Block::Item { level, check, parts } => (Some(*level), *check, parts),
            };
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 2.0);
                if let Some(level) = level {
                    ui.add_space(4.0 + level as f32 * 20.0);
                    // Una tarea citada con su clave se puede marcar aquí.
                    let task_key = parts.iter().find_map(|p| match p {
                        Inline::Cite(k, _) if k.starts_with('t') => Some(k.clone()),
                        _ => None,
                    });
                    let task = task_key.as_deref().and_then(find_task);
                    match (check, task) {
                        (_, Some(t)) => {
                            let (glyph, color) = if t.done { (icon::CHECK_SQUARE, ACCENT) } else { (icon::SQUARE, MUTED) };
                            let b = egui::Button::new(RichText::new(glyph).size(17.0).color(color)).frame(false);
                            if ui.add(b).on_hover_text(if t.done { "Marcar pendiente" } else { "Marcar hecha" }).clicked() {
                                action = Some(Action::ToggleTask(t.raw.clone()));
                            }
                        }
                        (Some(done), None) => {
                            ui.label(RichText::new(if done { icon::CHECK_SQUARE } else { icon::SQUARE }).size(17.0).color(MUTED));
                        }
                        (None, None) => {
                            ui.label(RichText::new("•").size(15.0).color(MUTED));
                        }
                    }
                    ui.add_space(8.0);
                }
                let done = level.is_some() && parts.iter().find_map(|p| match p {
                    Inline::Cite(k, _) if k.starts_with('t') => find_task(k).map(|t| t.done),
                    _ => None,
                }) == Some(true);
                let mut prev_note: Option<&str> = None;
                for p in parts {
                    if !matches!(p, Inline::Cite(k, _) if k.starts_with('n')) {
                        prev_note = None;
                    }
                    match p {
                        Inline::Text(t) => {
                            let mut job = rich(t, 14.5, if done { MUTED } else { TEXT });
                            if done {
                                for s in &mut job.sections {
                                    s.format.strikethrough = Stroke::new(1.0, MUTED);
                                }
                            }
                            ui.label(job);
                        }
                        Inline::Cite(k, line) if k.starts_with('n') => {
                            let Some(src) = turn.sources.get(k) else { continue };
                            // "[n1:4][n1:5]" seguidas: un solo número.
                            if prev_note == Some(k.as_str()) {
                                continue;
                            }
                            prev_note = Some(k.as_str());
                            let n = match numbers.iter().position(|x| x.0 == *k) {
                                Some(i) => i + 1,
                                None => {
                                    numbers.push((k.clone(), *line));
                                    numbers.len()
                                }
                            };
                            let tip = match line {
                                Some(l) => format!("{} · línea {l}", src.label),
                                None => src.label.clone(),
                            };
                            ui.add_space(2.0);
                            let chip = egui::Button::new(RichText::new(n.to_string()).size(10.5).color(ACCENT))
                                .fill(ACCENT_BG)
                                .corner_radius(6)
                                .min_size(egui::vec2(16.0, 16.0));
                            if ui.add(chip).on_hover_text(tip).clicked() {
                                action = Some(self.open_at_line(&src.path, *line));
                            }
                        }
                        Inline::Cite(..) => {} // tareas: se ven como casilla
                    }
                }
            });
            ui.add_space(3.0);
        }
        // Fuentes: cada nota citada, con su número.
        if !numbers.is_empty() {
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                for (i, (k, line)) in numbers.iter().enumerate() {
                    let Some(src) = turn.sources.get(k) else { continue };
                    let text = format!("{}  {}", i + 1, src.label);
                    let b = egui::Button::new(RichText::new(text).size(12.0).color(ACCENT)).corner_radius(6);
                    if ui.add(b).on_hover_text("Abrir la nota").clicked() {
                        action = Some(self.open_at_line(&src.path, *line));
                    }
                }
            });
        }
        action
    }

    /// Abrir una nota con el cursor al final de una línea (desde 1).
    fn open_at_line(&self, path: &Path, line: Option<usize>) -> Action {
        let cursor = line.and_then(|l| {
            let text = self.vault.get(path)?.text.clone();
            let mut ci = 0;
            for (i, full) in text.split_inclusive('\n').enumerate() {
                let content = full.trim_end_matches(['\n', '\r']);
                if i + 1 == l {
                    return Some(ci + content.chars().count());
                }
                ci += full.chars().count();
            }
            None
        });
        Action::Open(path.to_path_buf(), cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_answer_names_sources() {
        let mut sources = HashMap::new();
        sources.insert("n1".to_string(), Source { path: PathBuf::from("x.md"), label: "Consorcio/Trincheras".into() });
        let answer = "Quedan 2:\n- [ ] Entregar informe [t1]\n- Revisar taludes [n1:3][n1]";
        let turn = Turn {
            question: "q".into(),
            answer: Some(Ok(answer.into())),
            blocks: ask::parse_answer(answer),
            progress: String::new(),
            sources,
            tasks: HashMap::new(),
        };
        assert_eq!(plain(&turn), "Quedan 2:\n- Entregar informe\n- Revisar taludes (Consorcio/Trincheras)\n");
    }
}

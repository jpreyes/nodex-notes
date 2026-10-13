//! Hábitos: unos pocos hábitos diarios que se marcan día a día, con su racha y la meta del día
//! (hacerlos todos).
//!
//! Se guardan en `.nodex/habitos.txt`, una línea por cambio, para que las copias de otro equipo
//! se junten sumando líneas: «momento TAB habito TAB id TAB nombre», «momento TAB quitar TAB id»
//! y «momento TAB si|no TAB id TAB fecha» (el momento en milisegundos). De cada hábito y de cada
//! día vale lo más reciente.

use super::*;
use std::collections::BTreeSet;

pub(super) const FILE: &str = "habitos.txt";
/// Cuántos días se ven en la cuadrícula de cada hábito.
const GRID_DAYS: i64 = 28;
/// Ideas para empezar (con la lista vacía).
const IDEAS: [&str; 4] = ["Caminar 30 minutos", "Leer 20 minutos", "Tomar 2 litros de agua", "Dormir antes de las 12"];

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Habit {
    pub id: String,
    pub name: String,
    /// El día en que se creó (antes no cuenta para la meta).
    pub since: String,
    pub done: BTreeSet<String>,
}

impl Habit {
    pub fn is_done(&self, day: &str) -> bool {
        self.done.contains(day)
    }

    /// Días seguidos hechos hasta hoy; si hoy aún no, la racha sigue viva desde ayer.
    pub fn streak(&self, today: &str) -> usize {
        let Some(mut d) = parse_day(today) else { return 0 };
        if !self.is_done(today) {
            d = d.pred_opt().unwrap_or(d);
        }
        let mut n = 0;
        while self.done.contains(&d.format("%Y-%m-%d").to_string()) {
            n += 1;
            d = match d.pred_opt() {
                Some(p) => p,
                None => break,
            };
        }
        n
    }

    /// La racha más larga.
    pub fn best(&self) -> usize {
        let (mut best, mut run, mut last) = (0, 0, None::<NaiveDate>);
        for d in self.done.iter().filter_map(|d| parse_day(d)) {
            run = if last.and_then(|l| l.succ_opt()) == Some(d) { run + 1 } else { 1 };
            best = best.max(run);
            last = Some(d);
        }
        best
    }
}

fn parse_day(d: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()
}

fn shift(day: &str, by: i64) -> String {
    parse_day(day).map(|d| (d + chrono::Duration::days(by)).format("%Y-%m-%d").to_string()).unwrap_or_default()
}

fn now_ms() -> i64 {
    Local::now().timestamp_millis()
}

/// Lee los hábitos (en el orden en que se crearon).
pub(super) fn parse(text: &str) -> Vec<Habit> {
    let mut rows: Vec<(i64, Vec<&str>)> = text
        .lines()
        .filter_map(|l| {
            let mut p = l.split('\t');
            let at = p.next()?.trim().parse::<i64>().ok()?;
            Some((at, p.collect()))
        })
        .collect();
    rows.sort_by_key(|r| r.0);
    let mut out: Vec<Habit> = Vec::new();
    let mut gone: HashSet<String> = HashSet::new();
    let mut days: HashMap<(String, String), bool> = HashMap::new();
    for (at, f) in rows {
        match f.as_slice() {
            ["habito", id, name, ..] => {
                gone.remove(*id);
                match out.iter_mut().find(|h| h.id == *id) {
                    Some(h) => h.name = name.to_string(),
                    None => {
                        let since = DateTime::from_timestamp_millis(at).map(|t| t.with_timezone(&Local).format("%Y-%m-%d").to_string()).unwrap_or_default();
                        out.push(Habit { id: id.to_string(), name: name.to_string(), since, done: BTreeSet::new() });
                    }
                }
            }
            ["quitar", id, ..] => {
                gone.insert(id.to_string());
            }
            [state @ ("si" | "no"), id, day, ..] => {
                days.insert((id.to_string(), day.to_string()), *state == "si");
            }
            _ => {}
        }
    }
    out.retain(|h| !gone.contains(&h.id));
    for h in &mut out {
        h.done = days.iter().filter(|((id, _), on)| **on && *id == h.id).map(|((_, d), _)| d.clone()).collect();
        // Si se marcó un día antes de crearlo (otro equipo con otra hora), cuenta desde ahí.
        if let Some(first) = h.done.iter().next().filter(|f| **f < h.since) {
            h.since = first.clone();
        }
    }
    out
}

/// ¿Se cumplió la meta de `day` (todos los hábitos que ya existían, hechos)?
pub(super) fn goal_met(habits: &[Habit], day: &str) -> bool {
    let due: Vec<&Habit> = habits.iter().filter(|h| h.since.as_str() <= day).collect();
    !due.is_empty() && due.iter().all(|h| h.is_done(day))
}

/// Días seguidos con la meta cumplida (si hoy aún no, desde ayer).
pub(super) fn goal_streak(habits: &[Habit], today: &str) -> usize {
    let mut d = if goal_met(habits, today) { today.to_string() } else { shift(today, -1) };
    let mut n = 0;
    while !d.is_empty() && goal_met(habits, &d) {
        n += 1;
        d = shift(&d, -1);
    }
    n
}

/// Lo que se hace en la vista (se aplica después de dibujarla).
enum Op {
    Add(String),
    Toggle(String, String),
    Rename(String, String),
    Remove(String),
    Open,
}

impl NotesApp {
    fn habits_path(&self) -> PathBuf {
        self.vault.root.join(".nodex").join(FILE)
    }

    pub(super) fn habits(&self) -> Vec<Habit> {
        parse(&vault::read_text(&self.habits_path()).unwrap_or_default())
    }

    fn habit_write(&self, line: String) {
        let path = self.habits_path();
        let _ = fs::create_dir_all(self.vault.root.join(".nodex"));
        let mut text = vault::read_text(&path).unwrap_or_default();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text += &format!("{}\t{line}\n", now_ms());
        let _ = fs::write(&path, text);
    }

    pub(super) fn add_habit(&mut self, name: &str) {
        let name = name.replace(['\t', '\n'], " ").trim().to_string();
        if name.is_empty() {
            return;
        }
        let id = format!("h{:x}", now_ms());
        self.habit_write(format!("habito\t{id}\t{name}"));
    }

    /// Marca o desmarca un día; dice si se cumplió la meta.
    pub(super) fn toggle_habit(&mut self, id: &str, day: &str) {
        let before = self.habits();
        let Some(h) = before.iter().find(|h| h.id == id) else { return };
        let on = !h.is_done(day);
        self.habit_write(format!("{}\t{id}\t{day}", if on { "si" } else { "no" }));
        let now = self.habits();
        if on && day == today() && goal_met(&now, day) && !goal_met(&before, day) {
            let n = goal_streak(&now, day);
            self.msg(if n > 1 { tf!("Meta de hoy cumplida: {n} días seguidos", n = n) } else { t!("Meta de hoy cumplida").to_string() });
        }
    }

    fn rename_habit(&mut self, id: &str, name: &str) {
        let name = name.replace(['\t', '\n'], " ").trim().to_string();
        if !name.is_empty() {
            self.habit_write(format!("habito\t{id}\t{name}"));
        }
    }

    fn remove_habit(&mut self, id: &str) {
        let name = self.habits().into_iter().find(|h| h.id == id).map(|h| h.name).unwrap_or_default();
        self.habit_write(format!("quitar\t{id}"));
        self.msg(tf!("Quité el hábito «{name}» (sus días quedan guardados)", name = name));
    }

    /// Hoy en Tu día: cada hábito con su casilla y su racha.
    pub(super) fn habits_today_ui(&mut self, ui: &mut Ui) -> Option<Action> {
        let habits = self.habits();
        if habits.is_empty() {
            return None;
        }
        let today = today();
        let mut op = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
            let met = goal_met(&habits, &today);
            let head = egui::Button::new(RichText::new(format!("{} {}", icon::FLAME, t!("Hábitos"))).font(theme::bold(13.5)).color(if met { SUCCESS() } else { MUTED() })).frame(false);
            if ui.add(head).on_hover_text(t!("Ver tus hábitos y sus rachas")).clicked() {
                op = Some(Op::Open);
            }
            for h in &habits {
                let done = h.is_done(&today);
                let streak = h.streak(&today);
                let mark = if done { icon::CHECK_SQUARE } else { icon::SQUARE };
                let text = if streak > 0 { format!("{mark} {}  {} {streak}", h.name, icon::FLAME) } else { format!("{mark} {}", h.name) };
                let color = if done { SUCCESS() } else { TEXT() };
                let b = egui::Button::new(RichText::new(text).size(13.0).color(color)).corner_radius(12);
                let tip = if done { t!("Hecho hoy (clic para desmarcar)") } else { t!("Marcar como hecho hoy") };
                if ui.add(b).on_hover_text(tip).clicked() {
                    op = Some(Op::Toggle(h.id.clone(), today.clone()));
                }
            }
            if met {
                ui.label(RichText::new(t!("meta de hoy cumplida")).size(12.5).color(SUCCESS()));
            }
        });
        ui.add_space(10.0);
        self.habit_op(op)
    }

    fn habit_op(&mut self, op: Option<Op>) -> Option<Action> {
        match op? {
            Op::Add(name) => self.add_habit(&name),
            Op::Toggle(id, day) => self.toggle_habit(&id, &day),
            Op::Rename(id, name) => self.rename_habit(&id, &name),
            Op::Remove(id) => self.remove_habit(&id),
            Op::Open => return Some(Action::ShowTab(View::Habits)),
        }
        None
    }

    /// La vista de hábitos: agregar, marcar hoy (o un día pasado), la racha y la meta del día.
    pub(super) fn habits_view(&mut self, ui: &mut Ui) -> Option<Action> {
        let habits = self.habits();
        let today = today();
        let mut op = None;
        Self::column(ui, "habitos", |ui, _| {
            let done_today = habits.iter().filter(|h| h.is_done(&today)).count();
            let met = goal_met(&habits, &today);
            let streak = goal_streak(&habits, &today);
            let subtitle = if habits.is_empty() {
                t!("Unos pocos hábitos diarios, marcados día a día. La meta: hacerlos todos.").to_string()
            } else if met {
                tf!("Meta de hoy cumplida · {n} seguidos cumpliéndola", n = plural(streak, "día"))
            } else if streak > 0 {
                tf!("Hoy: {done} de {total} · llevas {n} cumpliendo la meta", done = done_today, total = habits.len(), n = plural(streak, "día"))
            } else {
                tf!("Hoy: {done} de {total}", done = done_today, total = habits.len())
            };
            view_header(ui, t!("Hábitos"), &subtitle);

            // Agregar.
            ui.horizontal(|ui| {
                let field = ui.add(
                    egui::TextEdit::singleline(&mut self.habit_new)
                        .hint_text(format!("{}  {}", icon::PLUS, t!("Nuevo hábito (p. ej. Caminar 30 minutos)")))
                        .desired_width((ui.available_width() - 90.0).max(120.0))
                        .margin(Margin::symmetric(8, 5)),
                );
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                let add = ui.add_enabled(!self.habit_new.trim().is_empty(), egui::Button::new(t!("Agregar"))).clicked();
                if (enter || add) && !self.habit_new.trim().is_empty() {
                    op = Some(Op::Add(std::mem::take(&mut self.habit_new)));
                    field.request_focus();
                }
            });
            ui.add_space(10.0);
            if habits.is_empty() {
                ui.label(RichText::new(t!("Algunas ideas para empezar:")).size(12.5).color(MUTED()));
                ui.horizontal_wrapped(|ui| {
                    for idea in IDEAS {
                        if ui.button(format!("{}  {}", icon::PLUS, crate::i18n::tr(idea))).clicked() {
                            op = Some(Op::Add(crate::i18n::tr(idea).to_string()));
                        }
                    }
                });
                return;
            }

            let first = shift(&today, 1 - GRID_DAYS);
            for h in &habits {
                let done = h.is_done(&today);
                let streak = h.streak(&today);
                Frame::new().stroke(Stroke::new(1.0, theme::BORDER())).corner_radius(10).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let mark = if done { icon::CHECK_SQUARE } else { icon::SQUARE };
                        let b = egui::Button::new(RichText::new(mark).size(22.0).color(if done { SUCCESS() } else { MUTED() })).frame(false);
                        let tip = if done { t!("Hecho hoy (clic para desmarcar)") } else { t!("Marcar como hecho hoy") };
                        if ui.add(b).on_hover_text(tip).clicked() {
                            op = Some(Op::Toggle(h.id.clone(), today.clone()));
                        }
                        let editing = self.habit_edit.as_ref().is_some_and(|(id, _)| *id == h.id);
                        if editing {
                            let (_, text) = self.habit_edit.as_mut().unwrap();
                            let r = ui.add(egui::TextEdit::singleline(text).desired_width(260.0));
                            r.request_focus();
                            if r.lost_focus() {
                                let (id, text) = self.habit_edit.take().unwrap();
                                if !ui.input(|i| i.key_pressed(Key::Escape)) {
                                    op = Some(Op::Rename(id, text));
                                }
                            }
                        } else {
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&h.name).font(theme::bold(15.0)).color(TEXT()));
                                let line = match (streak, h.best()) {
                                    (0, 0) => t!("Aún sin racha").to_string(),
                                    (0, b) => tf!("Sin racha ahora · la mejor: {n}", n = plural(b, "día")),
                                    (s, b) if s >= b => tf!("{flame} {n} seguidos · tu mejor racha", flame = icon::FLAME, n = plural(s, "día")),
                                    (s, b) => tf!("{flame} {n} seguidos · la mejor: {best}", flame = icon::FLAME, n = plural(s, "día"), best = plural(b, "día")),
                                };
                                ui.label(RichText::new(line).size(12.5).color(if streak > 0 { SUCCESS() } else { MUTED() }));
                            });
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let small = |g: &str| egui::Button::new(RichText::new(g).size(15.0).color(MUTED())).frame(false);
                            let r = ui.add(small(icon::DOTS_THREE)).on_hover_text(t!("Más"));
                            egui::Popup::menu(&r).show(|ui| {
                                if ui.button(format!("{}  {}", icon::PENCIL_SIMPLE, t!("Cambiar el nombre"))).clicked() {
                                    self.habit_edit = Some((h.id.clone(), h.name.clone()));
                                    ui.close();
                                }
                                if ui.button(format!("{}  {}", icon::TRASH, t!("Quitar el hábito"))).clicked() {
                                    op = Some(Op::Remove(h.id.clone()));
                                    ui.close();
                                }
                            });
                        });
                    });
                    ui.add_space(6.0);
                    // Los últimos días: un cuadrito por día; clic para corregir uno pasado.
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
                        for i in 0..GRID_DAYS {
                            let day = shift(&first, i);
                            let on = h.is_done(&day);
                            let before = day < h.since;
                            let (rect, r) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), Sense::click());
                            let fill = if on { SUCCESS() } else if before { Color32::TRANSPARENT } else { HOVER() };
                            ui.painter().rect_filled(rect, 3, fill);
                            if day == today {
                                ui.painter().rect_stroke(rect, 3, Stroke::new(1.5, ACCENT()), egui::StrokeKind::Outside);
                            } else if before {
                                ui.painter().rect_stroke(rect, 3, Stroke::new(1.0, theme::BORDER()), egui::StrokeKind::Inside);
                            }
                            let state = if on { t!("hecho") } else { t!("no hecho") };
                            let r = r.on_hover_text(format!("{} · {state}", long_date(&day))).on_hover_cursor(egui::CursorIcon::PointingHand);
                            if r.clicked() {
                                op = Some(Op::Toggle(h.id.clone(), day));
                            }
                        }
                    });
                });
                ui.add_space(8.0);
            }
            ui.label(RichText::new(tf!("Los cuadritos son los últimos {n} días: clic en uno para corregirlo.", n = GRID_DAYS)).size(12.0).color(MUTED()));
        });
        self.habit_op(op)
    }

    /// Para la IA: los hábitos, con su racha y los últimos días.
    pub(super) fn habits_for_ai(&self) -> Option<String> {
        let habits = self.habits();
        if habits.is_empty() {
            return None;
        }
        let today = today();
        let mut out = format!("Meta del día (hacer todos): {}. Días seguidos cumpliéndola: {}.\n", if goal_met(&habits, &today) { "cumplida hoy" } else { "aún no hoy" }, goal_streak(&habits, &today));
        for h in &habits {
            let last: Vec<&str> = h.done.iter().rev().take(14).map(String::as_str).collect();
            out += &format!(
                "- {}: {} hoy; racha {} días (mejor {}); desde {}; últimos días hechos: {}\n",
                h.name,
                if h.is_done(&today) { "hecho" } else { "no hecho" },
                h.streak(&today),
                h.best(),
                h.since,
                if last.is_empty() { "ninguno".into() } else { last.join(", ") }
            );
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn habits_streaks_and_goal() {
        let text = "\
1000\thabito\th1\tLeer
1001\thabito\th2\tCaminar
2000\tsi\th1\t2026-10-08
2001\tsi\th1\t2026-10-09
2002\tsi\th1\t2026-10-10
2003\tsi\th1\t2026-10-11
2004\tsi\th1\t2026-10-12
2005\tno\th1\t2026-10-12
1500\tsi\th2\t2026-10-11
3000\tsi\th2\t2026-10-12
3001\thabito\th2\tCaminar 30 min
";
        let mut hs = parse(text);
        assert_eq!(hs.len(), 2);
        assert_eq!(hs[1].name, "Caminar 30 min", "el nombre más reciente");
        // Hoy (12) desmarcado: la racha de Leer sigue viva desde ayer.
        assert!(!hs[0].is_done("2026-10-12"));
        assert_eq!(hs[0].streak("2026-10-12"), 4);
        assert_eq!(hs[0].best(), 4);
        assert_eq!(hs[1].streak("2026-10-12"), 2);
        // Los hábitos se crearon «en 1970»: la meta cuenta todos los días.
        assert!(goal_met(&hs, "2026-10-11"));
        assert!(!goal_met(&hs, "2026-10-12"));
        assert_eq!(goal_streak(&hs, "2026-10-12"), 1);
        // Un hábito creado hoy no rompe la meta de los días anteriores.
        hs[1].since = "2026-10-11".into();
        hs[0].done.insert("2026-10-12".into());
        assert!(goal_met(&hs, "2026-10-12"));
        assert_eq!(goal_streak(&hs, "2026-10-12"), 5, "del 8 al 12; Caminar cuenta desde el 11");
        // Quitar uno, y las copias de otro equipo se juntan sumando líneas (sin importar el orden).
        let merged = crate::conflicts::merge_lines(text, "1600\tsi\th1\t2026-10-07\n4000\tquitar\th2\n");
        let hs = parse(&merged);
        assert_eq!(hs.len(), 1);
        assert_eq!(hs[0].streak("2026-10-11"), 5);
        assert!(parse("basura\n\t\n").is_empty());
    }

    #[test]
    fn habits_in_the_app() {
        let dir = std::env::temp_dir().join(format!("nodex-habitos-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        fs::create_dir_all(dir.join("General")).unwrap();
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        assert!(app.habits_for_ai().is_none());
        app.add_habit("Leer\t20 minutos");
        std::thread::sleep(Duration::from_millis(3));
        app.add_habit("  ");
        let hs = app.habits();
        assert_eq!(hs.len(), 1);
        assert_eq!(hs[0].name, "Leer 20 minutos");
        assert_eq!(hs[0].since, today());
        app.toggle_habit(&hs[0].id, &today());
        assert!(app.habits()[0].is_done(&today()));
        assert!(app.habits_for_ai().unwrap().contains("Leer 20 minutos: hecho hoy; racha 1"));
        std::thread::sleep(Duration::from_millis(3));
        app.toggle_habit(&hs[0].id, &today());
        assert!(!app.habits()[0].is_done(&today()));
        std::thread::sleep(Duration::from_millis(3));
        app.remove_habit(&hs[0].id);
        assert!(app.habits().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}

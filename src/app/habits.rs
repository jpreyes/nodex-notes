//! Hábitos: unos pocos hábitos que se marcan día a día, con su racha y la meta del día (hacer
//! todo lo que toca hoy). Cada uno tiene su frecuencia: todos los días, ciertos días de la semana
//! (solo cuentan esos días) o N veces por semana, el día que sea (la racha cuenta semanas, y no
//! entra en la meta del día).
//!
//! Se guardan en `.nodex/habitos.txt`, una línea por cambio, para que las copias de otro equipo
//! se junten sumando líneas: «momento TAB habito TAB id TAB nombre», «momento TAB frecuencia TAB
//! id TAB diario|dias:1,3|semana:2», «momento TAB quitar TAB id» y «momento TAB si|no TAB id TAB
//! fecha» (el momento en milisegundos). De cada hábito y de cada día vale lo más reciente.

use super::*;
use std::collections::BTreeSet;

pub(super) const FILE: &str = "habitos.txt";
/// Cuántas semanas completas se ven en la cuadrícula (además de la actual).
const GRID_WEEKS: i64 = 4;
/// Hasta cuántos días atrás se miran las rachas.
const LOOK_BACK: i64 = 1100;
/// Ideas para empezar (con la lista vacía).
const IDEAS: [&str; 4] = ["Caminar 30 minutos", "Leer 20 minutos", "Tomar 2 litros de agua", "Dormir antes de las 12"];

/// Cada cuánto toca un hábito.
#[derive(Clone, Debug, PartialEq, Default)]
pub(super) enum Freq {
    #[default]
    Daily,
    /// Ciertos días de la semana (1 = lunes … 7 = domingo).
    Days(BTreeSet<u32>),
    /// N veces por semana, el día que sea.
    Weekly(u32),
}

impl Freq {
    fn parse(s: &str) -> Freq {
        if let Some(d) = s.strip_prefix("dias:") {
            let days: BTreeSet<u32> = d.split(',').filter_map(|x| x.trim().parse().ok()).filter(|w| (1..=7).contains(w)).collect();
            if !days.is_empty() {
                return Freq::Days(days);
            }
        }
        if let Some(n) = s.strip_prefix("semana:").and_then(|n| n.trim().parse::<u32>().ok()) {
            return Freq::Weekly(n.clamp(1, 7));
        }
        Freq::Daily
    }

    fn encode(&self) -> String {
        match self {
            Freq::Daily => "diario".into(),
            Freq::Days(d) => format!("dias:{}", d.iter().map(u32::to_string).collect::<Vec<_>>().join(",")),
            Freq::Weekly(n) => format!("semana:{n}"),
        }
    }

    /// «Todos los días», «Cada lunes y jueves», «2 veces por semana».
    pub fn describe(&self) -> String {
        match self {
            Freq::Daily => t!("Todos los días").into(),
            Freq::Days(d) if *d == (1..=5).collect::<BTreeSet<u32>>() => t!("De lunes a viernes").into(),
            Freq::Days(d) => {
                let names: Vec<&str> = d.iter().map(|w| crate::i18n::dias()[*w as usize - 1]).collect();
                let days = match names.split_last() {
                    Some((last, rest)) if !rest.is_empty() => format!("{} {} {last}", rest.join(", "), t!("y")),
                    _ => names.join(""),
                };
                tf!("Cada {days}", days = days)
            }
            Freq::Weekly(1) => t!("1 vez por semana").into(),
            Freq::Weekly(n) => tf!("{n} veces por semana", n = n),
        }
    }

    /// Cómo se cuenta la racha: días, veces o semanas.
    fn amount(&self, n: usize) -> String {
        match self {
            Freq::Daily => plural(n, "día"),
            Freq::Days(_) if n == 1 => t!("1 vez").into(),
            Freq::Days(_) => tf!("{n} veces", n = n),
            Freq::Weekly(_) => plural(n, "semana"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Habit {
    pub id: String,
    pub name: String,
    pub freq: Freq,
    /// El día en que se creó (antes no cuenta para la meta).
    pub since: String,
    pub done: BTreeSet<String>,
}

fn parse_day(d: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()
}

fn ymd(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

fn monday_of(d: NaiveDate) -> NaiveDate {
    d - chrono::Duration::days(d.weekday().num_days_from_monday() as i64)
}

fn now_ms() -> i64 {
    Local::now().timestamp_millis()
}

impl Habit {
    pub fn is_done(&self, day: &str) -> bool {
        self.done.contains(day)
    }

    /// ¿Le toca ese día? (Los de N veces por semana no tienen un día).
    pub fn due_on(&self, day: &str) -> bool {
        match &self.freq {
            Freq::Daily => true,
            Freq::Days(d) => parse_day(day).is_some_and(|x| d.contains(&x.weekday().number_from_monday())),
            Freq::Weekly(_) => false,
        }
    }

    /// Cuántas veces se hizo en la semana de `day` (de lunes a domingo).
    pub fn week_count(&self, day: &str) -> usize {
        let Some(m) = parse_day(day).map(monday_of) else { return 0 };
        (0..7).filter(|i| self.done.contains(&ymd(m + chrono::Duration::days(*i)))).count()
    }

    /// Lo que cuenta para la racha, desde que existe hasta hoy: cada día que le toca, o cada
    /// semana. (¿se cumplió?, ¿es el de hoy?)
    fn periods(&self, today: &str) -> Vec<(bool, bool)> {
        let Some(t) = parse_day(today) else { return Vec::new() };
        let start = parse_day(&self.since).unwrap_or(t).max(t - chrono::Duration::days(LOOK_BACK));
        let mut out = Vec::new();
        match self.freq {
            Freq::Weekly(n) => {
                let mut w = monday_of(start);
                while w <= t {
                    out.push((self.week_count(&ymd(w)) >= n as usize, w == monday_of(t)));
                    w += chrono::Duration::days(7);
                }
            }
            _ => {
                let mut d = start;
                while d <= t {
                    let day = ymd(d);
                    if self.due_on(&day) {
                        out.push((self.is_done(&day), d == t));
                    }
                    d += chrono::Duration::days(1);
                }
            }
        }
        out
    }

    /// La racha de ahora: lo que toca hoy (o esta semana) aún no la corta.
    pub fn streak(&self, today: &str) -> usize {
        let p = self.periods(today);
        let mut it = p.iter().rev().peekable();
        if it.peek().is_some_and(|(done, now)| !done && *now) {
            it.next();
        }
        it.take_while(|(done, _)| *done).count()
    }

    /// La racha más larga.
    pub fn best(&self, today: &str) -> usize {
        let (mut best, mut run) = (0, 0);
        for (done, _) in self.periods(today) {
            run = if done { run + 1 } else { 0 };
            best = best.max(run);
        }
        best
    }
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
    let mut freqs: HashMap<String, Freq> = HashMap::new();
    let mut days: HashMap<(String, String), bool> = HashMap::new();
    for (at, f) in rows {
        match f.as_slice() {
            ["habito", id, name, ..] => {
                gone.remove(*id);
                match out.iter_mut().find(|h| h.id == *id) {
                    Some(h) => h.name = name.to_string(),
                    None => {
                        let since = DateTime::from_timestamp_millis(at).map(|t| t.with_timezone(&Local).format("%Y-%m-%d").to_string()).unwrap_or_default();
                        out.push(Habit { id: id.to_string(), name: name.to_string(), freq: Freq::Daily, since, done: BTreeSet::new() });
                    }
                }
            }
            ["frecuencia", id, spec, ..] => {
                freqs.insert(id.to_string(), Freq::parse(spec));
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
        h.freq = freqs.remove(&h.id).unwrap_or_default();
        h.done = days.iter().filter(|((id, _), on)| **on && *id == h.id).map(|((_, d), _)| d.clone()).collect();
        // Si se marcó un día antes de crearlo (otro equipo con otra hora), cuenta desde ahí.
        if let Some(first) = h.done.iter().next().filter(|f| **f < h.since) {
            h.since = first.clone();
        }
    }
    out
}

/// La meta de un día: None si ese día no tocaba ninguno; si no, si se hizo todo lo que tocaba.
pub(super) fn goal(habits: &[Habit], day: &str) -> Option<bool> {
    let due: Vec<&Habit> = habits.iter().filter(|h| h.since.as_str() <= day && h.due_on(day)).collect();
    (!due.is_empty()).then(|| due.iter().all(|h| h.is_done(day)))
}

pub(super) fn goal_met(habits: &[Habit], day: &str) -> bool {
    goal(habits, day) == Some(true)
}

/// Días seguidos con la meta cumplida (hoy, si aún no, no la corta; los días en que no tocaba
/// nada no cuentan ni cortan).
pub(super) fn goal_streak(habits: &[Habit], today: &str) -> usize {
    let Some(mut d) = parse_day(today) else { return 0 };
    let oldest = habits.iter().map(|h| h.since.as_str()).min().unwrap_or(today).to_string();
    let mut n = 0;
    for i in 0..LOOK_BACK {
        let day = ymd(d);
        if day < oldest {
            break;
        }
        match goal(habits, &day) {
            Some(true) => n += 1,
            Some(false) if i == 0 => {}
            Some(false) => break,
            None => {}
        }
        d = d.pred_opt().unwrap_or(d);
    }
    n
}

/// Lo de hoy: (hechos, cuántos tocan), sin los de N veces por semana.
pub(super) fn today_count(habits: &[Habit], today: &str) -> (usize, usize) {
    let due: Vec<&Habit> = habits.iter().filter(|h| h.due_on(today)).collect();
    (due.iter().filter(|h| h.is_done(today)).count(), due.len())
}

/// Elegir la frecuencia; devuelve la nueva si se cambió.
fn freq_menu(ui: &mut Ui, cur: &Freq) -> Option<Freq> {
    let mut out = None;
    if ui.radio(*cur == Freq::Daily, t!("Todos los días")).clicked() {
        out = Some(Freq::Daily);
    }
    ui.add_space(6.0);
    ui.label(RichText::new(t!("Solo ciertos días:")).size(12.5).color(MUTED()));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for w in 1..=7u32 {
            let on = matches!(cur, Freq::Days(d) if d.contains(&w));
            if ui.selectable_label(on, crate::i18n::dias_cortos()[w as usize - 1]).clicked() {
                let mut d = match cur {
                    Freq::Days(d) => d.clone(),
                    _ => BTreeSet::new(),
                };
                if on {
                    d.remove(&w);
                } else {
                    d.insert(w);
                }
                out = Some(if d.is_empty() { Freq::Daily } else { Freq::Days(d) });
            }
        }
    });
    ui.add_space(6.0);
    ui.label(RichText::new(t!("Veces por semana, el día que sea:")).size(12.5).color(MUTED()));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for n in 1..=6u32 {
            if ui.selectable_label(*cur == Freq::Weekly(n), n.to_string()).clicked() {
                out = Some(Freq::Weekly(n));
            }
        }
    });
    out
}

/// Lo que se hace en la vista (se aplica después de dibujarla).
enum Op {
    Add(String, Freq),
    Toggle(String, String),
    Rename(String, String),
    SetFreq(String, Freq),
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

    /// Un hábito nuevo; devuelve su id.
    pub(super) fn add_habit(&mut self, name: &str, freq: Freq) -> Option<String> {
        let name = name.replace(['\t', '\n'], " ").trim().to_string();
        if name.is_empty() {
            return None;
        }
        let id = format!("h{:x}", now_ms());
        self.habit_write(format!("habito\t{id}\t{name}"));
        if freq != Freq::Daily {
            self.habit_write(format!("frecuencia\t{id}\t{}", freq.encode()));
        }
        Some(id)
    }

    fn set_habit_freq(&mut self, id: &str, freq: &Freq) {
        self.habit_write(format!("frecuencia\t{id}\t{}", freq.encode()));
    }

    /// Marca o desmarca un día; avisa si se cumplió la meta del día o la de la semana.
    pub(super) fn toggle_habit(&mut self, id: &str, day: &str) {
        let before = self.habits();
        let Some(h) = before.iter().find(|h| h.id == id) else { return };
        let on = !h.is_done(day);
        self.habit_write(format!("{}\t{id}\t{day}", if on { "si" } else { "no" }));
        if !on {
            return;
        }
        let now = self.habits();
        if let (Freq::Weekly(n), Some(h)) = (&h.freq, now.iter().find(|h| h.id == id)) {
            if h.week_count(day) == *n as usize {
                self.msg(tf!("«{name}»: listo esta semana", name = h.name));
            }
        } else if day == today() && goal_met(&now, day) && !goal_met(&before, day) {
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

    /// Hoy en Tu día: lo que toca hoy, y los de la semana, con su casilla y su racha.
    pub(super) fn habits_today_ui(&mut self, ui: &mut Ui) -> Option<Action> {
        let habits = self.habits();
        let today = today();
        let list: Vec<&Habit> = habits.iter().filter(|h| h.due_on(&today) || matches!(h.freq, Freq::Weekly(_))).collect();
        if list.is_empty() {
            return None;
        }
        let mut op = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
            let met = goal_met(&habits, &today);
            let head = egui::Button::new(RichText::new(format!("{} {}", icon::FLAME, t!("Hábitos"))).font(theme::bold(13.5)).color(if met { SUCCESS() } else { MUTED() })).frame(false);
            if ui.add(head).on_hover_text(t!("Ver tus hábitos y sus rachas")).clicked() {
                op = Some(Op::Open);
            }
            for h in list {
                let done = h.is_done(&today);
                let streak = h.streak(&today);
                let mark = if done { icon::CHECK_SQUARE } else { icon::SQUARE };
                let mut text = format!("{mark} {}", h.name);
                if let Freq::Weekly(n) = h.freq {
                    text += &format!("  {}/{n}", h.week_count(&today));
                }
                if streak > 0 {
                    text += &format!("  {} {streak}", icon::FLAME);
                }
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
            Op::Add(name, freq) => {
                self.add_habit(&name, freq);
            }
            Op::Toggle(id, day) => self.toggle_habit(&id, &day),
            Op::Rename(id, name) => self.rename_habit(&id, &name),
            Op::SetFreq(id, freq) => self.set_habit_freq(&id, &freq),
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
            let (done_today, due_today) = today_count(&habits, &today);
            let met = goal_met(&habits, &today);
            let streak = goal_streak(&habits, &today);
            let subtitle = if habits.is_empty() {
                t!("Unos pocos hábitos, marcados día a día. La meta: hacer todo lo que toca hoy.").to_string()
            } else if due_today == 0 {
                t!("Hoy no toca ninguno de todos los días").to_string()
            } else if met {
                tf!("Meta de hoy cumplida · {n} seguidos cumpliéndola", n = plural(streak, "día"))
            } else if streak > 0 {
                tf!("Hoy: {done} de {total} · llevas {n} cumpliendo la meta", done = done_today, total = due_today, n = plural(streak, "día"))
            } else {
                tf!("Hoy: {done} de {total}", done = done_today, total = due_today)
            };
            view_header(ui, t!("Hábitos"), &subtitle);

            // Agregar, con su frecuencia.
            ui.horizontal(|ui| {
                let field = ui.add(
                    egui::TextEdit::singleline(&mut self.habit_new)
                        .hint_text(format!("{}  {}", icon::PLUS, t!("Nuevo hábito (p. ej. Caminar 30 minutos)")))
                        .desired_width((ui.available_width() - 260.0).max(120.0))
                        .margin(Margin::symmetric(8, 5)),
                );
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                ui.menu_button(format!("{}  {}", icon::REPEAT, self.habit_new_freq.describe()), |ui| {
                    if let Some(f) = freq_menu(ui, &self.habit_new_freq) {
                        self.habit_new_freq = f;
                    }
                })
                .response
                .on_hover_text(t!("Cada cuánto"));
                let add = ui.add_enabled(!self.habit_new.trim().is_empty(), egui::Button::new(t!("Agregar"))).clicked();
                if (enter || add) && !self.habit_new.trim().is_empty() {
                    op = Some(Op::Add(std::mem::take(&mut self.habit_new), std::mem::take(&mut self.habit_new_freq)));
                    field.request_focus();
                }
            });
            ui.add_space(10.0);
            if habits.is_empty() {
                ui.label(RichText::new(t!("Algunas ideas para empezar:")).size(12.5).color(MUTED()));
                ui.horizontal_wrapped(|ui| {
                    for idea in IDEAS {
                        if ui.button(format!("{}  {}", icon::PLUS, crate::i18n::tr(idea))).clicked() {
                            op = Some(Op::Add(crate::i18n::tr(idea).to_string(), Freq::Daily));
                        }
                    }
                });
                return;
            }

            // La cuadrícula empieza un lunes: las últimas semanas completas y la actual.
            let first = parse_day(&today).map(|t| monday_of(t) - chrono::Duration::days(7 * GRID_WEEKS)).unwrap_or_default();
            let last = parse_day(&today).unwrap_or_default();
            for h in &habits {
                let done = h.is_done(&today);
                let due = h.due_on(&today) || matches!(h.freq, Freq::Weekly(_));
                let streak = h.streak(&today);
                Frame::new().stroke(Stroke::new(1.0, theme::BORDER())).corner_radius(10).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let mark = if done { icon::CHECK_SQUARE } else { icon::SQUARE };
                        if due {
                            let b = egui::Button::new(RichText::new(mark).size(22.0).color(if done { SUCCESS() } else { MUTED() })).frame(false);
                            let tip = if done { t!("Hecho hoy (clic para desmarcar)") } else { t!("Marcar como hecho hoy") };
                            if ui.add(b).on_hover_text(tip).clicked() {
                                op = Some(Op::Toggle(h.id.clone(), today.clone()));
                            }
                        } else {
                            ui.label(RichText::new(icon::MOON).size(22.0).color(theme::BORDER())).on_hover_text(t!("Hoy no toca"));
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
                                let mut how = h.freq.describe();
                                if let Freq::Weekly(n) = h.freq {
                                    how += &format!(" · {}", tf!("{done} de {n} esta semana", done = h.week_count(&today), n = n));
                                }
                                let line = match (streak, h.best(&today)) {
                                    (0, 0) => t!("Aún sin racha").to_string(),
                                    (0, b) => tf!("Sin racha ahora · la mejor: {n}", n = h.freq.amount(b)),
                                    (s, b) if s >= b => tf!("{flame} Racha: {n} · tu mejor racha", flame = icon::FLAME, n = h.freq.amount(s)),
                                    (s, b) => tf!("{flame} Racha: {n} · la mejor: {best}", flame = icon::FLAME, n = h.freq.amount(s), best = h.freq.amount(b)),
                                };
                                ui.label(RichText::new(how).size(12.5).color(MUTED()));
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
                                ui.menu_button(format!("{}  {}", icon::REPEAT, t!("Cada cuánto")), |ui| {
                                    if let Some(f) = freq_menu(ui, &h.freq) {
                                        op = Some(Op::SetFreq(h.id.clone(), f));
                                    }
                                });
                                if ui.button(format!("{}  {}", icon::TRASH, t!("Quitar el hábito"))).clicked() {
                                    op = Some(Op::Remove(h.id.clone()));
                                    ui.close();
                                }
                            });
                        });
                    });
                    ui.add_space(6.0);
                    // Las últimas semanas: un cuadrito por día (los días que no tocan, en gris
                    // claro); clic para corregir uno pasado.
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
                        let mut d = first;
                        while d <= last {
                            let day = ymd(d);
                            let on = h.is_done(&day);
                            let before = day < h.since;
                            let counts = h.due_on(&day) || matches!(h.freq, Freq::Weekly(_));
                            let (rect, r) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), if counts { Sense::click() } else { Sense::hover() });
                            r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, counts, on, format!("{} · {}", h.name, long_date(&day))));
                            if counts {
                                let fill = if on { SUCCESS() } else if before { Color32::TRANSPARENT } else { HOVER() };
                                ui.painter().rect_filled(rect, 3, fill);
                                if before && !on {
                                    ui.painter().rect_stroke(rect, 3, Stroke::new(1.0, theme::BORDER()), egui::StrokeKind::Inside);
                                }
                            } else {
                                ui.painter().circle_filled(rect.center(), 2.0, theme::BORDER());
                            }
                            if day == today {
                                ui.painter().rect_stroke(rect, 3, Stroke::new(1.5, ACCENT()), egui::StrokeKind::Outside);
                            }
                            let state = if !counts { t!("no toca") } else if on { t!("hecho") } else { t!("no hecho") };
                            let r = r.on_hover_text(format!("{} · {state}", long_date(&day)));
                            if counts && r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                op = Some(Op::Toggle(h.id.clone(), day));
                            }
                            // Un espacio entre semanas.
                            if d.weekday() == chrono::Weekday::Sun {
                                ui.add_space(6.0);
                            }
                            d += chrono::Duration::days(1);
                        }
                    });
                });
                ui.add_space(8.0);
            }
            ui.label(RichText::new(tf!("Los cuadritos son las últimas {n} semanas, de lunes a domingo: clic en uno para corregirlo.", n = GRID_WEEKS)).size(12.0).color(MUTED()));
        });
        self.habit_op(op)
    }

    /// Para la IA: los hábitos, con su frecuencia, su racha y los últimos días.
    pub(super) fn habits_for_ai(&self) -> Option<String> {
        let habits = self.habits();
        if habits.is_empty() {
            return None;
        }
        let today = today();
        let goal_now = match goal(&habits, &today) {
            None => "hoy no toca ninguno diario",
            Some(true) => "cumplida hoy",
            Some(false) => "aún no hoy",
        };
        let mut out = format!("Meta del día (hacer todo lo que toca hoy): {goal_now}. Días seguidos cumpliéndola: {}.\n", goal_streak(&habits, &today));
        for h in &habits {
            let last: Vec<&str> = h.done.iter().rev().take(14).map(String::as_str).collect();
            let unit = match h.freq {
                Freq::Weekly(_) => "semanas",
                Freq::Days(_) => "veces",
                Freq::Daily => "días",
            };
            let week = match h.freq {
                Freq::Weekly(n) => format!(" ({} de {n} esta semana)", h.week_count(&today)),
                _ => String::new(),
            };
            out += &format!(
                "- {} [{}{week}]: {} hoy; racha {} {unit} (mejor {}); desde {}; últimos días hechos: {}\n",
                h.name,
                h.freq.describe(),
                if h.is_done(&today) { "hecho" } else { "no hecho" },
                h.streak(&today),
                h.best(&today),
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
        assert_eq!(hs[0].freq, Freq::Daily);
        // Hoy (12) desmarcado: la racha de Leer sigue viva desde ayer.
        assert!(!hs[0].is_done("2026-10-12"));
        assert_eq!(hs[0].streak("2026-10-12"), 4);
        assert_eq!(hs[0].best("2026-10-12"), 4);
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

    /// Los que tocan ciertos días solo cuentan esos días; los de N por semana cuentan semanas y
    /// no entran en la meta del día.
    #[test]
    fn habits_that_are_not_daily() {
        // Octubre de 2026: el lunes 5, el 12 y el 19; hoy es el miércoles 21.
        let text = "\
1000\thabito\tlun\tRevisar el presupuesto
1001\tfrecuencia\tlun\tdias:1
1002\thabito\tgym\tGimnasio
1003\tfrecuencia\tgym\tsemana:2
1004\thabito\tleer\tLeer
2000\tsi\tlun\t2026-09-28
2001\tsi\tlun\t2026-10-05
2002\tsi\tlun\t2026-10-12
2003\tsi\tlun\t2026-10-19
2004\tsi\tlun\t2026-10-20
2010\tsi\tgym\t2026-10-06
2011\tsi\tgym\t2026-10-09
2012\tsi\tgym\t2026-10-13
2013\tsi\tgym\t2026-10-17
2014\tsi\tgym\t2026-10-21
2020\tsi\tleer\t2026-10-20
2021\tsi\tleer\t2026-10-21
";
        let hs = parse(text);
        let (lunes, gym, leer) = (&hs[0], &hs[1], &hs[2]);
        assert_eq!(lunes.freq, Freq::Days([1].into()));
        assert_eq!(gym.freq, Freq::Weekly(2));
        let today = "2026-10-21";
        // Lunes: cuatro lunes seguidos; que el martes 20 esté marcado no cambia nada.
        assert!(!lunes.due_on(today) && lunes.due_on("2026-10-19"));
        assert_eq!(lunes.streak(today), 4);
        // Gimnasio: dos semanas completas (5 y 12 de oct.); esta semana va 1 de 2, sin cortarla.
        assert_eq!(gym.week_count(today), 1);
        assert_eq!(gym.week_count("2026-10-18"), 2, "el domingo es de la semana del lunes 12");
        assert_eq!(gym.streak(today), 2);
        assert_eq!(gym.best(today), 2);
        // La meta de hoy (miércoles): solo Leer; ni el de los lunes ni el gimnasio.
        assert_eq!(today_count(&hs, today), (1, 1));
        assert!(goal_met(&hs, today));
        // El lunes 19 tocaban Leer y el presupuesto: Leer no se hizo, así que la meta se cortó.
        assert_eq!(goal(&hs, "2026-10-19"), Some(false));
        assert_eq!(goal_streak(&hs, today), 2, "martes y miércoles");
        // Si el lunes que viene no se revisa, la racha se corta.
        assert_eq!(lunes.streak("2026-10-26"), 4, "hoy es lunes: aún no la corta");
        assert_eq!(lunes.streak("2026-10-27"), 0, "pasó el lunes sin hacerlo");
        // Textos.
        assert_eq!(Freq::parse("dias:1,3").encode(), "dias:1,3");
        assert_eq!(Freq::parse("dias:9"), Freq::Daily);
        assert_eq!(Freq::parse("semana:0"), Freq::Weekly(1));
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
        let id = app.add_habit("Leer\t20 minutos", Freq::Daily).unwrap();
        std::thread::sleep(Duration::from_millis(3));
        assert!(app.add_habit("  ", Freq::Daily).is_none());
        let gym = app.add_habit("Gimnasio", Freq::Weekly(2)).unwrap();
        let hs = app.habits();
        assert_eq!(hs.len(), 2);
        assert_eq!(hs[0].name, "Leer 20 minutos");
        assert_eq!(hs[0].since, today());
        assert_eq!(hs[1].freq, Freq::Weekly(2));
        app.toggle_habit(&id, &today());
        assert!(app.habits()[0].is_done(&today()));
        let ai = app.habits_for_ai().unwrap();
        assert!(ai.contains("Leer 20 minutos [Todos los días]: hecho hoy; racha 1 días"), "{ai}");
        assert!(ai.contains("Gimnasio [2 veces por semana (0 de 2 esta semana)]"), "{ai}");
        std::thread::sleep(Duration::from_millis(3));
        app.set_habit_freq(&gym, &Freq::Days([1, 4].into()));
        assert_eq!(app.habits()[1].freq, Freq::Days([1, 4].into()));
        std::thread::sleep(Duration::from_millis(3));
        app.toggle_habit(&id, &today());
        assert!(!app.habits()[0].is_done(&today()));
        std::thread::sleep(Duration::from_millis(3));
        app.remove_habit(&id);
        assert_eq!(app.habits().len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }
}

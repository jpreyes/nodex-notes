//! Los hábitos: leerlos de `.nodex/habitos.txt` y calcular sus rachas y la meta del día. Los usan
//! la app y el servidor (la app del celular). Ver la descripción del formato en la app
//! (`src/app/habits.rs`).

use chrono::{DateTime, Datelike, Local, NaiveDate};
use std::collections::{BTreeSet, HashMap, HashSet};

pub const FILE: &str = "habitos.txt";
/// Hasta cuántos días atrás se miran las rachas.
pub const LOOK_BACK: i64 = 1100;

/// Cada cuánto toca un hábito.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Freq {
    #[default]
    Daily,
    /// Ciertos días de la semana (1 = lunes … 7 = domingo).
    Days(BTreeSet<u32>),
    /// N veces por semana, el día que sea.
    Weekly(u32),
}

impl Freq {
    pub fn parse(s: &str) -> Freq {
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

    pub fn encode(&self) -> String {
        match self {
            Freq::Daily => "diario".into(),
            Freq::Days(d) => format!("dias:{}", d.iter().map(u32::to_string).collect::<Vec<_>>().join(",")),
            Freq::Weekly(n) => format!("semana:{n}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Habit {
    pub id: String,
    pub name: String,
    pub freq: Freq,
    /// El día en que se creó (antes no cuenta para la meta).
    pub since: String,
    pub done: BTreeSet<String>,
}

pub fn parse_day(d: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()
}

pub fn ymd(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

pub fn monday_of(d: NaiveDate) -> NaiveDate {
    d - chrono::Duration::days(d.weekday().num_days_from_monday() as i64)
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
    pub fn periods(&self, today: &str) -> Vec<(bool, bool)> {
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
pub fn parse(text: &str) -> Vec<Habit> {
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
pub fn goal(habits: &[Habit], day: &str) -> Option<bool> {
    let due: Vec<&Habit> = habits.iter().filter(|h| h.since.as_str() <= day && h.due_on(day)).collect();
    (!due.is_empty()).then(|| due.iter().all(|h| h.is_done(day)))
}

pub fn goal_met(habits: &[Habit], day: &str) -> bool {
    goal(habits, day) == Some(true)
}

/// Días seguidos con la meta cumplida (hoy, si aún no, no la corta; los días en que no tocaba
/// nada no cuentan ni cortan).
pub fn goal_streak(habits: &[Habit], today: &str) -> usize {
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
pub fn today_count(habits: &[Habit], today: &str) -> (usize, usize) {
    let due: Vec<&Habit> = habits.iter().filter(|h| h.due_on(today)).collect();
    (due.iter().filter(|h| h.is_done(today)).count(), due.len())
}


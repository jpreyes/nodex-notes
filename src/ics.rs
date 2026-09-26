//! Lector de calendarios ICS (iCalendar): el enlace que dan Google Calendar ("Dirección secreta
//! en formato iCal"), Outlook ("Publicar calendario → ICS"), iCloud y casi cualquier app.
//!
//! Entiende eventos de día completo y con hora (en UTC o en hora local), los repetidos
//! (RRULE diario, semanal con días, mensual y anual, con INTERVAL, COUNT y UNTIL), las
//! excepciones (EXDATE), los cambios a una repetición (RECURRENCE-ID) y los cancelados.
//! Las horas con zona horaria (TZID) se toman como hora local, que suele ser la de uno.

use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc, Weekday};

/// Un evento ya ubicado en un día.
#[derive(Debug, Clone, PartialEq)]
pub struct Occurrence {
    pub date: NaiveDate,
    /// `None` = todo el día.
    pub time: Option<NaiveTime>,
    pub end: Option<NaiveTime>,
    pub title: String,
    pub location: String,
}

#[derive(Debug, Clone, Default)]
struct Rule {
    freq: String,
    interval: i64,
    count: Option<usize>,
    until: Option<NaiveDateTime>,
    by_day: Vec<Weekday>,
}

#[derive(Debug, Clone)]
struct Event {
    uid: String,
    title: String,
    location: String,
    start: NaiveDateTime,
    all_day: bool,
    end: Option<NaiveDateTime>,
    rule: Option<Rule>,
    exdates: Vec<NaiveDateTime>,
    recurrence_id: Option<NaiveDateTime>,
    cancelled: bool,
}

/// Une las líneas partidas (las que empiezan con espacio o tabulación siguen a la anterior).
fn unfold(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if (line.starts_with(' ') || line.starts_with('\t')) && !out.is_empty() {
            out.last_mut().unwrap().push_str(&line[1..]);
        } else {
            out.push(line.to_string());
        }
    }
    out
}

fn unescape(v: &str) -> String {
    let mut out = String::new();
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push(' '),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out.trim().to_string()
}

/// "20260926" (día completo), "20260926T100000Z" (UTC) o "20260926T100000" (hora local).
fn parse_when(v: &str) -> Option<(NaiveDateTime, bool)> {
    let v = v.trim();
    if v.len() == 8 {
        let d = NaiveDate::parse_from_str(v, "%Y%m%d").ok()?;
        return Some((d.and_hms_opt(0, 0, 0)?, true));
    }
    if let Some(utc) = v.strip_suffix('Z') {
        let n = NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S").ok()?;
        let local = Utc.from_utc_datetime(&n).with_timezone(&Local).naive_local();
        return Some((local, false));
    }
    Some((NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S").ok()?, false))
}

fn weekday(code: &str) -> Option<Weekday> {
    // "MO", "1MO", "-1FR": solo cuenta el día.
    let code = code.trim_start_matches(|c: char| c == '-' || c == '+' || c.is_ascii_digit());
    Some(match code {
        "MO" => Weekday::Mon,
        "TU" => Weekday::Tue,
        "WE" => Weekday::Wed,
        "TH" => Weekday::Thu,
        "FR" => Weekday::Fri,
        "SA" => Weekday::Sat,
        "SU" => Weekday::Sun,
        _ => return None,
    })
}

fn parse_rule(v: &str) -> Rule {
    let mut r = Rule { interval: 1, ..Rule::default() };
    for part in v.split(';') {
        let Some((k, val)) = part.split_once('=') else { continue };
        match k {
            "FREQ" => r.freq = val.to_string(),
            "INTERVAL" => r.interval = val.parse().unwrap_or(1).max(1),
            "COUNT" => r.count = val.parse().ok(),
            "UNTIL" => r.until = parse_when(val).map(|(d, all_day)| if all_day { d + Duration::hours(23) + Duration::minutes(59) } else { d }),
            "BYDAY" => r.by_day = val.split(',').filter_map(weekday).collect(),
            _ => {}
        }
    }
    r
}

fn parse_events(text: &str) -> Vec<Event> {
    let mut out = Vec::new();
    let mut cur: Option<Event> = None;
    let mut depth = 0; // dentro de un VALARM u otro bloque anidado
    for line in unfold(text) {
        let (head, value) = match line.split_once(':') {
            Some(x) => x,
            None => continue,
        };
        let name = head.split(';').next().unwrap_or("").to_uppercase();
        match (name.as_str(), value.trim()) {
            ("BEGIN", "VEVENT") => {
                cur = Some(Event {
                    uid: String::new(),
                    title: String::new(),
                    location: String::new(),
                    start: NaiveDateTime::MIN,
                    all_day: false,
                    end: None,
                    rule: None,
                    exdates: Vec::new(),
                    recurrence_id: None,
                    cancelled: false,
                });
                depth = 0;
            }
            ("BEGIN", _) if cur.is_some() => depth += 1,
            ("END", "VEVENT") => {
                if let Some(e) = cur.take().filter(|e| e.start != NaiveDateTime::MIN) {
                    out.push(e);
                }
            }
            ("END", _) if cur.is_some() => depth -= 1,
            _ => {}
        }
        let Some(e) = cur.as_mut().filter(|_| depth == 0) else { continue };
        match name.as_str() {
            "UID" => e.uid = value.trim().to_string(),
            "SUMMARY" => e.title = unescape(value),
            "LOCATION" => e.location = unescape(value),
            "DTSTART" => {
                if let Some((d, all_day)) = parse_when(value) {
                    e.start = d;
                    e.all_day = all_day;
                }
            }
            "DTEND" => e.end = parse_when(value).map(|x| x.0),
            "RRULE" => e.rule = Some(parse_rule(value.trim())),
            "EXDATE" => e.exdates.extend(value.split(',').filter_map(parse_when).map(|x| x.0)),
            "RECURRENCE-ID" => e.recurrence_id = parse_when(value).map(|x| x.0),
            "STATUS" => e.cancelled = value.trim().eq_ignore_ascii_case("CANCELLED"),
            _ => {}
        }
    }
    out
}

fn add_months(d: NaiveDateTime, months: i64) -> Option<NaiveDateTime> {
    let total = d.year() as i64 * 12 + d.month0() as i64 + months;
    let (y, m) = ((total / 12) as i32, (total % 12) as u32 + 1);
    NaiveDate::from_ymd_opt(y, m, d.day()).map(|nd| nd.and_time(d.time()))
}

/// Los inicios de un evento (repetido o no) hasta `to`.
fn starts(e: &Event, to: NaiveDateTime) -> Vec<NaiveDateTime> {
    let Some(r) = &e.rule else { return vec![e.start] };
    let mut out = Vec::new();
    let limit = r.until.map_or(to, |u| u.min(to));
    let push = |d: NaiveDateTime, out: &mut Vec<NaiveDateTime>| -> bool {
        if d > limit || r.count.is_some_and(|c| out.len() >= c) {
            return false;
        }
        if d >= e.start {
            out.push(d);
        }
        true
    };
    match r.freq.as_str() {
        "DAILY" => {
            let mut d = e.start;
            for _ in 0..5000 {
                if !push(d, &mut out) {
                    break;
                }
                d += Duration::days(r.interval);
            }
        }
        "WEEKLY" => {
            let days: Vec<Weekday> = if r.by_day.is_empty() { vec![e.start.weekday()] } else { r.by_day.clone() };
            let week0 = e.start.date() - Duration::days(e.start.weekday().num_days_from_monday() as i64);
            'weeks: for w in 0..2000 {
                let monday = week0 + Duration::weeks(w * r.interval);
                let mut dates: Vec<NaiveDateTime> =
                    days.iter().map(|wd| (monday + Duration::days(wd.num_days_from_monday() as i64)).and_time(e.start.time())).collect();
                dates.sort();
                for d in dates {
                    if !push(d, &mut out) {
                        break 'weeks;
                    }
                }
            }
        }
        "MONTHLY" | "YEARLY" => {
            let step = if r.freq == "MONTHLY" { r.interval } else { r.interval * 12 };
            for i in 0..2000 {
                let Some(d) = add_months(e.start, i * step) else { continue };
                if !push(d, &mut out) {
                    break;
                }
            }
        }
        _ => out.push(e.start),
    }
    out
}

/// Todos los eventos del calendario entre dos días (inclusive), ordenados.
pub fn occurrences(text: &str, from: NaiveDate, to: NaiveDate) -> Vec<Occurrence> {
    let events = parse_events(text);
    // Repeticiones cambiadas: la original se omite en esa fecha (la versión cambiada viene aparte).
    let moved: Vec<(String, NaiveDateTime)> =
        events.iter().filter_map(|e| Some((e.uid.clone(), e.recurrence_id?))).collect();
    let end = to.and_hms_opt(23, 59, 59).unwrap_or(NaiveDateTime::MAX);
    let mut out = Vec::new();
    for e in events.iter().filter(|e| !e.cancelled) {
        let length = e.end.map(|x| x - e.start);
        for s in starts(e, end) {
            if s.date() < from || s.date() > to {
                continue;
            }
            if e.exdates.iter().any(|x| *x == s || (x.date() == s.date() && e.all_day)) {
                continue;
            }
            if e.rule.is_some() && moved.iter().any(|(uid, when)| *uid == e.uid && *when == s) {
                continue;
            }
            out.push(Occurrence {
                date: s.date(),
                time: (!e.all_day).then(|| s.time()),
                end: length.filter(|_| !e.all_day).map(|l| (s + l).time()),
                title: if e.title.is_empty() { "(sin título)".into() } else { e.title.clone() },
                location: e.location.clone(),
            });
        }
    }
    out.sort_by(|a, b| (a.date, a.time).cmp(&(b.date, b.time)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAL: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:a\r\nDTSTART;VALUE=DATE:20260928\r\nDTEND;VALUE=DATE:20260929\r\nSUMMARY:Feriado\\, día libre\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:b\r\nDTSTART;TZID=America/Santiago:20260921T100000\r\nDTEND;TZID=America/Santiago:20260921T110000\r\nRRULE:FREQ=WEEKLY;BYDAY=MO,WE;COUNT=5\r\nEXDATE;TZID=America/Santiago:20260923T100000\r\nSUMMARY:Reunión de\r\n  equipo\r\nLOCATION:Sala 2\r\nBEGIN:VALARM\r\nSUMMARY:no es el título\r\nEND:VALARM\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:b\r\nRECURRENCE-ID;TZID=America/Santiago:20260928T100000\r\nDTSTART;TZID=America/Santiago:20260928T150000\r\nSUMMARY:Reunión de equipo (movida)\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:c\r\nDTSTART:20260930T120000\r\nSUMMARY:Cancelada\r\nSTATUS:CANCELLED\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:d\r\nDTSTART:20250115T090000\r\nRRULE:FREQ=MONTHLY;INTERVAL=1\r\nSUMMARY:Pago mensual\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";

    #[test]
    fn reads_events_repeats_and_exceptions() {
        let d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        let t = |s: &str| NaiveTime::parse_from_str(s, "%H:%M").ok();
        let occ = occurrences(CAL, d("2026-09-21"), d("2026-10-15"));
        let list: Vec<(String, Option<NaiveTime>, String)> = occ.iter().map(|o| (o.date.to_string(), o.time, o.title.clone())).collect();
        assert_eq!(
            list,
            vec![
                ("2026-09-21".into(), t("10:00"), "Reunión de equipo".into()),
                ("2026-09-28".into(), None, "Feriado, día libre".into()),
                ("2026-09-28".into(), t("15:00"), "Reunión de equipo (movida)".into()),
                ("2026-09-30".into(), t("10:00"), "Reunión de equipo".into()),
                ("2026-10-05".into(), t("10:00"), "Reunión de equipo".into()),
                ("2026-10-15".into(), t("09:00"), "Pago mensual".into()),
            ]
        );
        assert_eq!((occ[0].end, occ[0].location.as_str()), (t("11:00"), "Sala 2"));
    }

    /// Un calendario real descargado de Google: `NODEX_ICS=archivo.ics cargo test -- --ignored ics_real`
    #[test]
    #[ignore]
    fn ics_real() {
        let path = std::env::var("NODEX_ICS").expect("NODEX_ICS");
        let text = std::fs::read_to_string(path).unwrap();
        let from = Local::now().date_naive();
        let occ = occurrences(&text, from, from + Duration::days(120));
        for o in &occ {
            println!("{} {:?} {}", o.date, o.time, o.title);
        }
        assert!(!occ.is_empty());
    }

    #[test]
    fn utc_times_become_local() {
        let cal = "BEGIN:VEVENT\nUID:x\nDTSTART:20260926T150000Z\nSUMMARY:UTC\nEND:VEVENT\n";
        let day = NaiveDate::from_ymd_opt(2026, 9, 25).unwrap();
        let occ = occurrences(cal, day, day + Duration::days(2));
        let expected = Utc.with_ymd_and_hms(2026, 9, 26, 15, 0, 0).unwrap().with_timezone(&Local).naive_local();
        assert_eq!((occ[0].date, occ[0].time), (expected.date(), Some(expected.time())));
    }
}

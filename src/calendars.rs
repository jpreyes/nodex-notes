//! Calendarios agregados con su enlace ICS (Google, Outlook, iCloud…): se descargan en un hilo
//! aparte al abrir y cada 15 minutos, y sus eventos se muestran en Agenda, Hoy, Inicio y Semana.
//! Los enlaces quedan en config.toml (solo en este equipo); la última copia de cada calendario,
//! junto a él, para verlos sin internet.

use crate::ics::{self, Occurrence};
use chrono::{Duration, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};

/// Cada cuánto se vuelven a descargar.
pub const REFRESH: std::time::Duration = std::time::Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Subscription {
    pub nombre: String,
    pub url: String,
}

/// "webcal://…" es lo mismo que "https://…".
pub fn normalize(url: &str) -> String {
    let u = url.trim();
    match u.strip_prefix("webcal://") {
        Some(rest) => format!("https://{rest}"),
        None => u.to_string(),
    }
}

/// Un evento de un calendario agregado.
#[derive(Debug, Clone, PartialEq)]
pub struct External {
    pub calendar: String,
    pub occ: Occurrence,
}

struct Fetched {
    url: String,
    result: Result<String, String>,
}

pub struct Calendars {
    /// Texto ICS de cada calendario (por URL).
    texts: HashMap<String, String>,
    /// Error de la última descarga (por URL).
    pub errors: HashMap<String, String>,
    rx: Option<Receiver<Fetched>>,
    pending: usize,
    pub last: Option<std::time::Instant>,
}

fn cache_path() -> std::path::PathBuf {
    crate::config::config_path().with_file_name("calendarios-cache.json")
}

impl Calendars {
    /// Con la copia guardada de la última vez.
    pub fn load() -> Calendars {
        let texts = crate::vault::read_text(&cache_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Calendars { texts, errors: HashMap::new(), rx: None, pending: 0, last: None }
    }

    pub fn busy(&self) -> bool {
        self.pending > 0
    }

    /// Descarga todos (o uno) en un hilo aparte.
    pub fn refresh(&mut self, subs: &[Subscription], ctx: eframe::egui::Context) {
        if subs.is_empty() || self.busy() {
            return;
        }
        let urls: Vec<String> = subs.iter().map(|s| normalize(&s.url)).collect();
        let (tx, rx) = mpsc::channel();
        self.pending = urls.len();
        self.rx = Some(rx);
        self.last = Some(std::time::Instant::now());
        std::thread::spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
            let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build();
            for url in urls {
                let result = match &client {
                    Ok(c) => rt.block_on(async {
                        let r = c.get(&url).header("User-Agent", format!("nodex-notes/{}", env!("CARGO_PKG_VERSION"))).send().await.map_err(|e| e.to_string())?;
                        if !r.status().is_success() {
                            return Err(format!("el servidor respondió {}", r.status()));
                        }
                        let text = r.text().await.map_err(|e| e.to_string())?;
                        if !text.contains("BEGIN:VCALENDAR") {
                            return Err("el enlace no es un calendario ICS".into());
                        }
                        Ok(text)
                    }),
                    Err(e) => Err(e.to_string()),
                };
                let _ = tx.send(Fetched { url, result });
                ctx.request_repaint();
            }
        });
    }

    /// Recibe lo descargado. Devuelve true si algo cambió.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        let mut changed = false;
        for f in rx.try_iter() {
            self.pending = self.pending.saturating_sub(1);
            match f.result {
                Ok(t) => {
                    self.errors.remove(&f.url);
                    self.texts.insert(f.url, t);
                }
                Err(e) => {
                    self.errors.insert(f.url, e);
                }
            }
            changed = true;
        }
        if self.pending == 0 {
            self.rx = None;
            if changed {
                let _ = std::fs::write(cache_path(), serde_json::to_string(&self.texts).unwrap_or_default());
            }
        }
        changed
    }

    /// ¿Ya se descargó bien alguna vez?
    pub fn loaded(&self, url: &str) -> bool {
        self.texts.contains_key(&normalize(url))
    }

    /// Eventos de todos los calendarios entre dos días.
    pub fn events(&self, subs: &[Subscription], from: NaiveDate, to: NaiveDate) -> Vec<External> {
        let mut out = Vec::new();
        for s in subs {
            if let Some(t) = self.texts.get(&normalize(&s.url)) {
                out.extend(ics::occurrences(t, from, to).into_iter().map(|occ| External { calendar: s.nombre.clone(), occ }));
            }
        }
        out.sort_by(|a, b| (a.occ.date, a.occ.time).cmp(&(b.occ.date, b.occ.time)));
        out
    }

    /// Los eventos como eventos de la agenda (para Hoy, Inicio, Semana y Preguntar): del mes pasado a 3 meses.
    pub fn as_agenda(&self, subs: &[Subscription]) -> Vec<crate::agenda::Event> {
        let today = Local::now().date_naive();
        self.events(subs, today - Duration::days(31), today + Duration::days(92))
            .into_iter()
            .map(|e| crate::agenda::Event {
                date: e.occ.date.format("%Y-%m-%d").to_string(),
                time: e.occ.time.map(|t| t.format("%H:%M").to_string()),
                title: e.occ.title,
                project: e.calendar,
                note: None,
                mail: None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webcal_links_work() {
        assert_eq!(normalize(" webcal://p01-caldav.icloud.com/x.ics "), "https://p01-caldav.icloud.com/x.ics");
        assert_eq!(normalize("https://calendar.google.com/calendar/ical/x/basic.ics"), "https://calendar.google.com/calendar/ical/x/basic.ics");
    }

    /// Un servidor local sirve un calendario: se descarga, se lee y queda en la copia.
    #[test]
    fn downloads_a_calendar() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let today = Local::now().format("%Y%m%d").to_string();
        let body = format!("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:1\r\nDTSTART:{today}T100000\r\nSUMMARY:Visita a terreno\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n");
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/calendar\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
        });
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        let _ = std::fs::create_dir_all(crate::config::config_path().parent().unwrap());
        let subs = vec![Subscription { nombre: "Trabajo".into(), url: format!("http://127.0.0.1:{port}/cal.ics") }];
        let mut c = Calendars { texts: HashMap::new(), errors: HashMap::new(), rx: None, pending: 0, last: None };
        c.refresh(&subs, eframe::egui::Context::default());
        let start = std::time::Instant::now();
        while c.busy() && start.elapsed().as_secs() < 20 {
            c.poll();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(c.loaded(&subs[0].url), "{:?}", c.errors);
        let ev = c.as_agenda(&subs);
        assert_eq!((ev[0].title.as_str(), ev[0].time.as_deref(), ev[0].project.as_str()), ("Visita a terreno", Some("10:00"), "Trabajo"));
    }
}

//! Correo por IMAP (Gmail, Outlook, iCloud… con una contraseña de aplicación): se leen la
//! bandeja de entrada y los enviados de los últimos días, sin marcarlos como leídos.
//! Los correos quedan solo en este equipo (`correos.json`, junto a config.toml).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Días hacia atrás que se leen la primera vez.
pub const DAYS: i64 = 7;
/// Máximo de correos nuevos por carpeta en cada revisión.
const MAX_PER_FOLDER: usize = 60;
/// Bytes que se leen de cada correo (el texto va al principio; los adjuntos, al final).
const MAX_BYTES: usize = 60_000;
/// Caracteres del cuerpo que se guardan.
const MAX_BODY: usize = 6_000;

/// Una cuenta de correo: la dirección, la contraseña de aplicación y (opcional) el servidor.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Account {
    pub correo: String,
    pub clave: String,
    /// "imap.ejemplo.com" (o "imap.ejemplo.com:993"); vacío = se deduce del correo.
    pub servidor: String,
}

/// Servidor IMAP de los correos más comunes.
pub fn server_for(account: &Account) -> (String, u16) {
    let custom = account.servidor.trim();
    if !custom.is_empty() {
        return match custom.rsplit_once(':') {
            Some((h, p)) if p.parse::<u16>().is_ok() => (h.to_string(), p.parse().unwrap_or(993)),
            _ => (custom.to_string(), 993),
        };
    }
    let domain = account.correo.rsplit('@').next().unwrap_or("").trim().to_lowercase();
    let host = match domain.as_str() {
        "gmail.com" | "googlemail.com" => "imap.gmail.com".to_string(),
        "outlook.com" | "hotmail.com" | "live.com" | "msn.com" | "outlook.es" | "hotmail.es" | "live.cl" => "outlook.office365.com".to_string(),
        "icloud.com" | "me.com" | "mac.com" => "imap.mail.me.com".to_string(),
        "yahoo.com" | "yahoo.es" => "imap.mail.yahoo.com".to_string(),
        d => format!("imap.{d}"),
    };
    (host, 993)
}

/// Un correo leído (y, después, lo que entendió la IA).
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Mail {
    /// "cuenta:carpeta:uid"
    pub id: String,
    pub account: String,
    /// Enviado por la persona (carpeta de enviados).
    pub sent: bool,
    pub from: String,
    pub to: String,
    pub subject: String,
    /// "2026-09-26 09:12"
    pub date: String,
    pub message_id: String,
    pub body: String,
    /// Boletín o aviso automático (no lo lee la IA).
    pub bulk: bool,
    // Lo que entendió la IA.
    pub analyzed: bool,
    pub summary: String,
    pub important: bool,
    pub workspace: String,
    /// Lo que se agregó a la agenda desde este correo (para mostrarlo y poder quitarlo).
    pub items: Vec<String>,
    pub task_ids: Vec<String>,
    pub event_lines: Vec<String>,
    /// Tareas pendientes que este correo parece cumplir (id, texto) y si ya se respondió.
    pub fulfills: Vec<Fulfill>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Fulfill {
    pub task_id: String,
    pub task_text: String,
    pub resolved: bool,
}

/// Correos guardados y hasta dónde se leyó cada carpeta.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Store {
    pub mails: Vec<Mail>,
    /// "cuenta:carpeta" -> último UID leído.
    pub last_uid: HashMap<String, u32>,
    /// Día de la última revisión diaria (AAAA-MM-DD).
    pub last_daily: String,
}

fn store_path() -> std::path::PathBuf {
    crate::config::config_path().with_file_name("correos.json")
}

impl Store {
    pub fn load() -> Store {
        crate::vault::read_text(&store_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = std::fs::write(store_path(), serde_json::to_string(self).unwrap_or_default());
    }

    /// Agrega los nuevos (sin repetir) y descarta los de hace más de 60 días.
    pub fn merge(&mut self, new: Vec<Mail>) {
        for m in new {
            if !self.mails.iter().any(|x| x.id == m.id || (!m.message_id.is_empty() && x.message_id == m.message_id && x.sent == m.sent)) {
                self.mails.push(m);
            }
        }
        let cutoff = (chrono::Local::now() - chrono::Duration::days(60)).format("%Y-%m-%d").to_string();
        self.mails.retain(|m| m.date >= cutoff);
        self.mails.sort_by(|a, b| b.date.cmp(&a.date));
    }
}

/// "Juan Pérez <juan@x.cl>" o la dirección sola.
fn addr_text(a: Option<&mail_parser::Address>) -> String {
    let Some(a) = a else { return String::new() };
    a.iter()
        .map(|x| match (x.name(), x.address()) {
            (Some(n), Some(e)) => format!("{n} <{e}>"),
            (Some(n), None) => n.to_string(),
            (None, Some(e)) => e.to_string(),
            _ => String::new(),
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Convierte un correo (bytes RFC 822) en `Mail`.
pub fn parse(raw: &[u8], id: String, account: &str, sent: bool) -> Option<Mail> {
    let msg = mail_parser::MessageParser::default().parse(raw)?;
    let date = msg
        .date()
        .and_then(|d| chrono::DateTime::parse_from_rfc3339(&d.to_rfc3339()).ok())
        .map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default();
    let body: String = msg.body_text(0).map(|b| b.to_string()).unwrap_or_default();
    // Sin las líneas citadas de correos anteriores ("> …") y sin espacios de más.
    let mut clean = String::new();
    for l in body.lines() {
        let t = l.trim_end();
        if t.trim_start().starts_with('>') {
            continue;
        }
        if t.is_empty() && clean.ends_with("\n\n") {
            continue;
        }
        clean += t;
        clean.push('\n');
    }
    let bulk = msg.header("List-Unsubscribe").is_some()
        || msg.header("List-Id").is_some()
        || msg.header("Precedence").and_then(|h| h.as_text()).is_some_and(|p| matches!(p.to_lowercase().as_str(), "bulk" | "list" | "junk"))
        || msg.header("Auto-Submitted").and_then(|h| h.as_text()).is_some_and(|a| !a.eq_ignore_ascii_case("no"));
    Some(Mail {
        id,
        account: account.to_string(),
        sent,
        from: addr_text(msg.from()),
        to: addr_text(msg.to()),
        subject: msg.subject().unwrap_or("(sin asunto)").to_string(),
        date,
        message_id: msg.message_id().unwrap_or("").to_string(),
        body: clean.trim().chars().take(MAX_BODY).collect(),
        bulk,
        ..Mail::default()
    })
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

fn imap_date(d: chrono::NaiveDate) -> String {
    use chrono::Datelike;
    format!("{}-{}-{}", d.day(), MONTHS[d.month0() as usize], d.year())
}

/// La carpeta de enviados: la marcada como \Sent, o por su nombre.
fn sent_folder<T: Read + Write>(s: &mut imap::Session<T>) -> Option<String> {
    let names = s.list(Some(""), Some("*")).ok()?;
    let mut by_name = None;
    for n in names.iter() {
        if n.attributes().iter().any(|a| format!("{a:?}").contains("Sent")) {
            return Some(n.name().to_string());
        }
        let lower = n.name().to_lowercase();
        if by_name.is_none() && ["sent", "enviados", "enviado", "sent items", "sent messages", "elementos enviados"].iter().any(|x| lower.ends_with(x)) {
            by_name = Some(n.name().to_string());
        }
    }
    by_name
}

/// Lee lo nuevo de la bandeja de entrada y de enviados en una sesión ya abierta.
pub fn read_session<T: Read + Write>(s: &mut imap::Session<T>, account: &str, last_uid: &HashMap<String, u32>) -> Result<(Vec<Mail>, HashMap<String, u32>), String> {
    let mut out = Vec::new();
    let mut uids = HashMap::new();
    let mut folders = vec![("INBOX".to_string(), false)];
    if let Some(sent) = sent_folder(s) {
        folders.push((sent, true));
    }
    for (folder, sent) in folders {
        s.examine(&folder).map_err(|e| format!("{folder}: {e}"))?;
        let key = format!("{account}:{folder}");
        let query = match last_uid.get(&key) {
            Some(u) => format!("UID {}:*", u + 1),
            None => format!("SINCE {}", imap_date(chrono::Local::now().date_naive() - chrono::Duration::days(DAYS))),
        };
        let mut found: Vec<u32> = s.uid_search(&query).map_err(|e| format!("{folder}: {e}"))?.into_iter().collect();
        found.retain(|u| last_uid.get(&key).is_none_or(|last| u > last));
        found.sort_unstable();
        let newest: Vec<u32> = found.iter().rev().take(MAX_PER_FOLDER).copied().collect();
        if let Some(max) = found.last() {
            uids.insert(key.clone(), *max);
        } else if let Some(u) = last_uid.get(&key) {
            uids.insert(key.clone(), *u);
        }
        if newest.is_empty() {
            continue;
        }
        let set = newest.iter().map(|u| u.to_string()).collect::<Vec<_>>().join(",");
        let fetches = s.uid_fetch(&set, format!("(UID BODY.PEEK[]<0.{MAX_BYTES}>)")).map_err(|e| format!("{folder}: {e}"))?;
        for f in fetches.iter() {
            let (Some(uid), Some(body)) = (f.uid, f.body()) else { continue };
            if let Some(m) = parse(body, format!("{key}:{uid}"), account, sent) {
                out.push(m);
            }
        }
    }
    Ok((out, uids))
}

/// La conexión cifrada, envuelta para poder ponerle tiempo de espera (lo necesita IDLE).
pub struct Tls(rustls::StreamOwned<rustls::ClientConnection, TcpStream>);

impl Read for Tls {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for Tls {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

impl imap::extensions::idle::SetReadTimeout for Tls {
    fn set_read_timeout(&mut self, timeout: Option<Duration>) -> imap::Result<()> {
        self.0.sock.set_read_timeout(timeout).map_err(imap::Error::Io)
    }
}

/// Conecta (IMAP sobre TLS) e inicia sesión.
fn connect(account: &Account) -> Result<imap::Session<Tls>, String> {
    let (host, port) = server_for(account);
    let addr = (host.as_str(), port).to_socket_addrs().map_err(|e| format!("no se encontró {host}: {e}"))?.next().ok_or(format!("no se encontró {host}"))?;
    let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(20)).map_err(|e| format!("no se pudo conectar a {host}: {e}"))?;
    tcp.set_read_timeout(Some(Duration::from_secs(60))).ok();
    let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(host.clone()).map_err(|e| e.to_string())?;
    let conn = rustls::ClientConnection::new(Arc::new(config), name).map_err(|e| e.to_string())?;
    let tls = Tls(rustls::StreamOwned::new(conn, tcp));
    let mut client = imap::Client::new(tls);
    client.read_greeting().map_err(|e| format!("{host}: {e}"))?;
    client.login(account.correo.trim(), account.clave.trim()).map_err(|(e, _)| {
        let e = e.to_string();
        if e.to_lowercase().contains("auth") || e.to_lowercase().contains("credential") || e.to_lowercase().contains("login") {
            format!("el correo o la contraseña de aplicación no son correctos ({e})")
        } else {
            e
        }
    })
}

/// Cada cuánto se renueva la espera (los servidores cortan a los 29 minutos; así también
/// se nota antes si hay que dejar de esperar).
const IDLE_ROUND: Duration = Duration::from_secs(10 * 60);

/// Espera avisos de correo nuevo en la bandeja de entrada (IMAP IDLE) hasta que `stop` se active.
/// Llama `notify` cada vez que llega algo.
pub fn idle_loop<T: Read + Write + imap::extensions::idle::SetReadTimeout>(
    s: &mut imap::Session<T>,
    stop: &AtomicBool,
    notify: &mut dyn FnMut(),
) -> Result<(), String> {
    s.examine("INBOX").map_err(|e| e.to_string())?;
    while !stop.load(Ordering::Relaxed) {
        let outcome = s
            .idle()
            .timeout(IDLE_ROUND)
            .keepalive(false)
            .wait_while(|r| !matches!(r, imap::types::UnsolicitedResponse::Exists(_)))
            .map_err(|e| e.to_string())?;
        if outcome == imap::extensions::idle::WaitOutcome::MailboxChanged {
            notify();
        }
    }
    Ok(())
}

/// Hilo que avisa al llegar un correo: se reconecta solo si se corta (cada minuto, hasta `stop`).
pub fn watch(account: Account, stop: Arc<AtomicBool>, mut notify: impl FnMut(Result<(), String>)) {
    while !stop.load(Ordering::Relaxed) {
        let result = connect(&account).and_then(|mut s| {
            let r = idle_loop(&mut s, &stop, &mut || notify(Ok(())));
            let _ = s.logout();
            r
        });
        if let Err(e) = result {
            notify(Err(e));
            for _ in 0..60 {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

/// Solo prueba que se pueda entrar.
pub fn test_login(account: &Account) -> Result<(), String> {
    let mut s = connect(account)?;
    let _ = s.logout();
    Ok(())
}

/// Conecta, inicia sesión y lee lo nuevo.
pub fn fetch(account: &Account, last_uid: &HashMap<String, u32>) -> Result<(Vec<Mail>, HashMap<String, u32>), String> {
    let mut s = connect(account)?;
    let r = read_session(&mut s, &account.correo, last_uid);
    let _ = s.logout();
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servers_and_parsing() {
        let a = |c: &str, s: &str| Account { correo: c.into(), clave: String::new(), servidor: s.into() };
        assert_eq!(server_for(&a("jp@gmail.com", "")), ("imap.gmail.com".into(), 993));
        assert_eq!(server_for(&a("jp@hotmail.com", "")), ("outlook.office365.com".into(), 993));
        assert_eq!(server_for(&a("jp@empresa.cl", "")), ("imap.empresa.cl".into(), 993));
        assert_eq!(server_for(&a("jp@empresa.cl", "mail.empresa.cl:1993")), ("mail.empresa.cl".into(), 1993));

        let raw = "From: Juan Pérez <juan@obra.cl>\r\nTo: jp@gmail.com\r\nSubject: =?UTF-8?Q?Planos_corregidos_muro_norte?=\r\nDate: Sat, 26 Sep 2026 09:12:00 -0300\r\nMessage-ID: <abc@obra.cl>\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHola JP,\r\nte adjunto los planos rev. B.\r\n\r\n> correo anterior citado\r\nSaludos\r\n";
        let m = parse(raw.as_bytes(), "x:INBOX:7".into(), "jp@gmail.com", false).unwrap();
        assert_eq!(m.from, "Juan Pérez <juan@obra.cl>");
        assert_eq!(m.subject, "Planos corregidos muro norte");
        assert!(m.date.starts_with("2026-09-26"), "{}", m.date);
        assert_eq!(m.body, "Hola JP,\nte adjunto los planos rev. B.\n\nSaludos");
        assert!(!m.bulk);
        let news = "From: Tienda <news@t.cl>\r\nList-Unsubscribe: <mailto:x@t.cl>\r\nSubject: Ofertas\r\n\r\nCompra ya";
        assert!(parse(news.as_bytes(), "x".into(), "a", false).unwrap().bulk);
        assert_eq!(imap_date(chrono::NaiveDate::from_ymd_opt(2026, 9, 12).unwrap()), "12-Sep-2026");
    }

    /// Un servidor IMAP falso (sin TLS) responde como Gmail: se leen la bandeja y enviados.
    #[test]
    fn reads_inbox_and_sent_from_a_fake_server() {
        use std::io::{BufRead, BufReader};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let mail = "From: María <maria@cic.cl>\r\nTo: jp@gmail.com\r\nSubject: Visita a obra\r\nDate: Fri, 25 Sep 2026 08:40:00 -0300\r\nMessage-ID: <m1@cic.cl>\r\n\r\nNecesito la cubicación antes del martes.\r\n";
        let sent = "From: jp@gmail.com\r\nTo: rodrigo@utalca.cl\r\nSubject: Re: Informe\r\nDate: Fri, 25 Sep 2026 18:05:00 -0300\r\nMessage-ID: <s1@gmail.com>\r\n\r\nTe mando el informe final el lunes.\r\n";
        let server = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut w = s.try_clone().unwrap();
            let mut r = BufReader::new(s);
            w.write_all(b"* OK Gimap ready\r\n").unwrap();
            let mut folder = String::new();
            loop {
                let mut line = String::new();
                if r.read_line(&mut line).unwrap() == 0 {
                    break;
                }
                let (tag, cmd) = line.trim_end().split_once(' ').unwrap();
                let up = cmd.to_uppercase();
                let reply = if up.starts_with("LOGIN") {
                    format!("{tag} OK logged in\r\n")
                } else if up.starts_with("LIST") {
                    format!("* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n* LIST (\\HasNoChildren \\Sent) \"/\" \"[Gmail]/Enviados\"\r\n{tag} OK done\r\n")
                } else if up.starts_with("EXAMINE") {
                    folder = cmd.to_string();
                    format!("* 1 EXISTS\r\n* OK [UIDVALIDITY 1] ok\r\n{tag} OK [READ-ONLY] done\r\n")
                } else if up.starts_with("UID SEARCH") {
                    let uid = if folder.contains("Enviados") { 5 } else { 3 };
                    format!("* SEARCH {uid}\r\n{tag} OK done\r\n")
                } else if up.starts_with("UID FETCH") {
                    let (uid, body) = if folder.contains("Enviados") { (5, sent) } else { (3, mail) };
                    format!("* 1 FETCH (UID {uid} BODY[]<0> {{{}}}\r\n{body})\r\n{tag} OK done\r\n", body.len())
                } else if up.starts_with("LOGOUT") {
                    w.write_all(format!("* BYE\r\n{tag} OK bye\r\n").as_bytes()).unwrap();
                    break;
                } else {
                    format!("{tag} OK\r\n")
                };
                w.write_all(reply.as_bytes()).unwrap();
            }
        });
        let tcp = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut client = imap::Client::new(tcp);
        client.read_greeting().unwrap();
        let mut s = client.login("jp@gmail.com", "clave").map_err(|(e, _)| e).unwrap();
        let (mails, uids) = read_session(&mut s, "jp@gmail.com", &HashMap::new()).unwrap();
        let _ = s.logout();
        server.join().unwrap();
        assert_eq!(mails.len(), 2, "{mails:?}");
        assert_eq!((mails[0].subject.as_str(), mails[0].sent), ("Visita a obra", false));
        assert_eq!((mails[1].subject.as_str(), mails[1].sent), ("Re: Informe", true));
        assert_eq!(uids.get("jp@gmail.com:INBOX"), Some(&3));
        assert_eq!(uids.get("jp@gmail.com:[Gmail]/Enviados"), Some(&5));
    }

    /// IDLE: el servidor avisa "* 4 EXISTS" y la app se entera al instante.
    #[test]
    fn idle_notices_new_mail() {
        use std::io::{BufRead, BufReader};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut w = s.try_clone().unwrap();
            let mut r = BufReader::new(s);
            w.write_all(b"* OK ready\r\n").unwrap();
            let mut idle_tag = String::new();
            loop {
                let mut line = String::new();
                if r.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let line = line.trim_end().to_string();
                if line == "DONE" {
                    w.write_all(format!("{idle_tag} OK idle done\r\n").as_bytes()).unwrap();
                    continue;
                }
                let (tag, cmd) = line.split_once(' ').unwrap();
                let up = cmd.to_uppercase();
                if up.starts_with("IDLE") {
                    idle_tag = tag.to_string();
                    w.write_all(b"+ idling\r\n* 4 EXISTS\r\n").unwrap();
                } else if up.starts_with("EXAMINE") {
                    w.write_all(format!("* 3 EXISTS\r\n{tag} OK [READ-ONLY] done\r\n").as_bytes()).unwrap();
                } else if up.starts_with("LOGOUT") {
                    w.write_all(format!("* BYE\r\n{tag} OK bye\r\n").as_bytes()).unwrap();
                    break;
                } else {
                    w.write_all(format!("{tag} OK\r\n").as_bytes()).unwrap();
                }
            }
        });
        let tcp = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut client = imap::Client::new(tcp);
        client.read_greeting().unwrap();
        let mut s = client.login("jp@gmail.com", "clave").map_err(|(e, _)| e).unwrap();
        let stop = AtomicBool::new(false);
        let mut count = 0;
        idle_loop(&mut s, &stop, &mut || {
            count += 1;
            stop.store(true, Ordering::Relaxed);
        })
        .unwrap();
        let _ = s.logout();
        assert_eq!(count, 1);
    }
}

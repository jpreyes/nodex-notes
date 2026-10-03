//! Pruebas de To Do contra un Microsoft falso (sin red ni cuenta).

use super::*;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

/// Tareas del To Do falso: (id, título, estado, fecha).
type Store = Arc<Mutex<Vec<(String, String, String, Value)>>>;

/// Como To Do guarda las fechas: solo el día, que devuelve como la medianoche UTC.
fn as_todo_keeps_it(due: &Value) -> Value {
    match due["dateTime"].as_str() {
        Some(d) => json!({ "dateTime": format!("{}T00:00:00.0000000", &d[..10]), "timeZone": "UTC" }),
        None => Value::Null,
    }
}

/// Un Microsoft falso: token, listas y tareas (lo justo para la app).
fn fake_microsoft(store: Store) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let mut next = 0;
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            // Cabeceras y cuerpo (según Content-Length).
            loop {
                let n = s.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buf).to_string();
                if let Some(end) = text.find("\r\n\r\n") {
                    let len = text[..end]
                        .lines()
                        .find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                        .unwrap_or(0);
                    if buf.len() >= end + 4 + len {
                        break;
                    }
                }
            }
            let text = String::from_utf8_lossy(&buf).to_string();
            let first = text.lines().next().unwrap_or("").to_string();
            let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
            let mut parts = first.split_whitespace();
            let (method, path) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
            let path = path.split('?').next().unwrap_or("").to_string();
            let mut tasks = store.lock().unwrap();
            let reply: Value = match (method.as_str(), path.as_str()) {
                ("POST", "/token") => json!({ "access_token": "AT", "refresh_token": "RT", "expires_in": 3600 }),
                ("GET", "/me/todo/lists") => json!({ "value": [] }),
                ("POST", "/me/todo/lists") => json!({ "id": "L1", "displayName": "Notas" }),
                ("GET", "/me/todo/lists/L1/tasks") => json!({
                    "value": tasks.iter().map(|(id, t, st, due)| json!({ "id": id, "title": t, "status": st, "dueDateTime": due })).collect::<Vec<_>>()
                }),
                ("POST", "/me/todo/lists/L1/tasks") => {
                    let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                    next += 1;
                    let id = format!("R{next}");
                    tasks.push((id.clone(), v["title"].as_str().unwrap_or("").into(), "notStarted".into(), as_todo_keeps_it(&v["dueDateTime"])));
                    json!({ "id": id })
                }
                ("PATCH", p) => {
                    let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                    let id = p.rsplit('/').next().unwrap_or("");
                    if let Some(t) = tasks.iter_mut().find(|t| t.0 == id) {
                        if let Some(st) = v["status"].as_str() {
                            t.2 = st.into();
                        }
                        if v.get("dueDateTime").is_some() {
                            t.3 = as_todo_keeps_it(&v["dueDateTime"]);
                        }
                    }
                    json!({})
                }
                ("DELETE", p) => {
                    let id = p.rsplit('/').next().unwrap_or("").to_string();
                    tasks.retain(|t| t.0 != id);
                    Value::Null
                }
                _ => json!({}),
            };
            drop(tasks);
            let out = reply.to_string();
            let head = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", out.len());
            let _ = s.write_all((head + &out).as_bytes());
        }
    });
    port
}

fn wait(app: &mut NotesApp, what: &str, done: impl Fn(&NotesApp) -> bool) {
    let start = Instant::now();
    while !done(app) {
        app.handle_todo();
        assert!(start.elapsed() < Duration::from_secs(15), "se esperaba: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn sync(app: &mut NotesApp) {
    app.todo_last = long_ago();
    app.maybe_sync_todo();
    wait(app, "sincronizado", |a| a.todo.as_ref().is_some_and(|t| !t.busy));
}

/// Conectar, mandar las tareas, traer las de To Do y marcar hechas en los dos sentidos.
#[test]
fn syncs_with_microsoft_todo_both_ways() {
    let store: Store = Arc::new(Mutex::new(Vec::new()));
    let port = fake_microsoft(store.clone());
    let dir = std::env::temp_dir().join(format!("nodex-todo-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("General")).unwrap();
    fs::write(dir.join("General").join("x.md"), "- [ ] Enviar planos due:2026-10-02 ^pl001\n").unwrap();
    fs::write(
        dir.join("tareas.txt"),
        "2026-09-27 Enviar planos +General due:2026-10-02 nota:General/x id:pl001\n2026-09-27 Llamar a Pedro +General\nx 2026-09-26 2026-09-20 Vieja +General\n",
    )
    .unwrap();
    unsafe {
        std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id())));
        std::env::set_var("NODEX_MS_API", format!("http://127.0.0.1:{port}"));
    }
    let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
    let mut app = NotesApp::new(cfg, None, egui::Context::default());
    unsafe { std::env::remove_var("NODEX_MS_API") };
    app.apply(Action::TodoConnect);
    wait(&mut app, "conectado", |a| a.todo.as_ref().is_some_and(|t| t.connected && !t.busy));

    // Primera vuelta: las dos pendientes van a To Do (la sin id recibe uno); la hecha no.
    store.lock().unwrap().push(("P1".into(), "Comprar pan".into(), "notStarted".into(), json!(null)));
    sync(&mut app);
    let titles: Vec<String> = store.lock().unwrap().iter().map(|t| t.1.clone()).collect();
    assert_eq!(titles, vec!["Comprar pan", "Enviar planos", "Llamar a Pedro"]);
    let planos = store.lock().unwrap().iter().find(|t| t.1 == "Enviar planos").unwrap().3.clone();
    assert_eq!(crate::todo::tests_due(&planos).as_deref(), Some("2026-10-02"));
    // Otra vuelta: la fecha sigue en su día (antes, cada vuelta la corría un día antes).
    sync(&mut app);
    assert_eq!(app.agenda.tasks().iter().find(|t| t.id.as_deref() == Some("pl001")).unwrap().due.as_deref(), Some("2026-10-02"));
    // Lo agregado en To Do llegó a Tareas.
    let tasks = app.agenda.tasks();
    assert!(tasks.iter().any(|t| t.text == "Comprar pan" && t.id.is_some() && t.note.is_none() && t.from.as_deref() == Some("todo")), "{tasks:?}");
    assert!(tasks.iter().all(|t| t.done || t.id.is_some()));

    // Hecha en To Do -> hecha en Notas (y su casilla en la nota).
    for t in store.lock().unwrap().iter_mut().filter(|t| t.1 == "Enviar planos") {
        t.2 = "completed".into();
    }
    sync(&mut app);
    assert!(app.agenda.tasks().iter().any(|t| t.id.as_deref() == Some("pl001") && t.done));
    assert!(fs::read_to_string(dir.join("General").join("x.md")).unwrap().starts_with("- [x] Enviar planos"));

    // Hecha en Notas -> hecha en To Do.
    let pedro = app.agenda.tasks().into_iter().find(|t| t.text == "Llamar a Pedro").unwrap().id.unwrap();
    app.agenda.set_done_by_id(&pedro, true, "2026-09-27").unwrap();
    sync(&mut app);
    assert_eq!(store.lock().unwrap().iter().find(|t| t.1 == "Llamar a Pedro").unwrap().2, "completed");

    app.apply(Action::TodoDisconnect);
    wait(&mut app, "desconectado", |a| a.todo.as_ref().is_some_and(|t| !t.connected));
    let _ = fs::remove_dir_all(&dir);
}

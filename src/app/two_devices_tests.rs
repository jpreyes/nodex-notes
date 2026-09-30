//! Prueba de punta a punta de la sincronización (M2): dos equipos con la app abierta, cada uno
//! con su copia de la carpeta, y un Dropbox simulado que copia los archivos entre ellos. Si los
//! dos cambiaron el mismo archivo entre dos sincronizaciones, deja una «copia en conflicto»,
//! como el de verdad. Al final las dos carpetas tienen que quedar iguales y sin perder nada.

use super::*;
use std::collections::BTreeSet;

/// Un Dropbox de mentira entre dos carpetas.
struct FakeDropbox {
    a: PathBuf,
    b: PathBuf,
    /// Cómo quedó cada archivo en la última sincronización.
    synced: HashMap<String, Option<Vec<u8>>>,
}

fn files(root: &Path) -> BTreeSet<String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeSet<String>) {
        for e in fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(root, &p, out);
            } else if let Ok(rel) = p.strip_prefix(root) {
                out.insert(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(root, root, &mut out);
    out
}

fn put(root: &Path, rel: &str, data: &Option<Vec<u8>>) {
    let p = root.join(rel);
    match data {
        Some(d) => {
            let _ = fs::create_dir_all(p.parent().unwrap());
            fs::write(&p, d).unwrap();
        }
        None => {
            let _ = fs::remove_file(&p);
        }
    }
}

impl FakeDropbox {
    fn new(a: &Path, b: &Path) -> FakeDropbox {
        let mut d = FakeDropbox { a: a.to_path_buf(), b: b.to_path_buf(), synced: HashMap::new() };
        d.sync();
        d
    }

    /// Una vuelta de sincronización. Si los dos cambiaron lo mismo, gana el primero (A) y lo de B
    /// queda como "Nombre (copia en conflicto de pc-b 2026-09-30).ext" en las dos carpetas.
    fn sync(&mut self) {
        // Dropbox tarda un momento en traer lo del otro equipo (y Windows guarda la fecha de
        // modificación con una precisión de ~16 ms).
        std::thread::sleep(Duration::from_millis(25));
        let mut all = files(&self.a);
        all.extend(files(&self.b));
        all.extend(self.synced.keys().cloned());
        for rel in all {
            let (ca, cb) = (fs::read(self.a.join(&rel)).ok(), fs::read(self.b.join(&rel)).ok());
            let base = self.synced.get(&rel).cloned().flatten();
            if ca == cb {
                self.synced.insert(rel, ca);
                continue;
            }
            match (ca != base, cb != base) {
                (true, false) => {
                    put(&self.b, &rel, &ca);
                    self.synced.insert(rel, ca);
                }
                (false, true) => {
                    put(&self.a, &rel, &cb);
                    self.synced.insert(rel, cb);
                }
                _ if ca.is_none() => {
                    put(&self.a, &rel, &cb);
                    self.synced.insert(rel, cb);
                }
                _ if cb.is_none() => {
                    put(&self.b, &rel, &ca);
                    self.synced.insert(rel, ca);
                }
                _ => {
                    let (stem, ext) = rel.rsplit_once('.').map(|(s, e)| (s.to_string(), format!(".{e}"))).unwrap_or((rel.clone(), String::new()));
                    let copy = format!("{stem} (copia en conflicto de pc-b 2026-09-30){ext}");
                    put(&self.b, &rel, &ca);
                    put(&self.a, &copy, &cb);
                    put(&self.b, &copy, &cb);
                    self.synced.insert(rel, ca);
                    self.synced.insert(copy, cb);
                }
            }
        }
    }
}

/// Lo que hace un equipo cuando le llegan cambios (lo mismo que su revisión de cada segundo,
/// sin esperas).
fn catch_up(app: &mut NotesApp) {
    app.vault.scan();
    app.poll();
    app.sync_stores();
    app.resolve_conflicts(true);
    app.reconcile_tasks();
    app.save();
}

fn type_line(app: &mut NotesApp, line: &str) {
    app.note.text = format!("{}{line}\n", app.note.text);
    app.note.dirty = true;
    app.save();
}

#[test]
fn two_devices_converge_without_losing_anything() {
    let base = std::env::temp_dir().join(format!("nodex-dos-pc-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let (dir_a, dir_b) = (base.join("pc-a"), base.join("pc-b"));
    fs::create_dir_all(dir_a.join("General")).unwrap();
    fs::write(dir_a.join("General").join("Obra.md"), "Inicio\n- [ ] Enviar planos ^pl001\n").unwrap();
    fs::write(dir_a.join("tareas.txt"), "2026-09-28 Enviar planos +General nota:General/Obra id:pl001\n").unwrap();
    let mut dropbox = FakeDropbox::new(&dir_a, &dir_b);
    unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
    let cfg = |dir: &Path| Config { carpeta_notas: dir.to_path_buf(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
    let mut a = NotesApp::new(cfg(&dir_a), None, egui::Context::default());
    let mut b = NotesApp::new(cfg(&dir_b), None, egui::Context::default());
    a.machine = "pc-a".into();
    b.machine = "pc-b".into();
    let (obra_a, obra_b) = (dir_a.join("General").join("Obra.md"), dir_b.join("General").join("Obra.md"));
    a.open(obra_a.clone(), None);
    b.open(obra_b.clone(), None);

    // 1. Con conexión: lo que escribe cada uno llega al otro.
    type_line(&mut a, "A1");
    dropbox.sync();
    catch_up(&mut b);
    type_line(&mut b, "B1");
    dropbox.sync();
    catch_up(&mut a);
    assert_eq!(a.note.text, "Inicio\n- [ ] Enviar planos ^pl001\nA1\nB1\n");

    // 2. Una tarea marcada hecha en A queda hecha en B, en Tareas y en su casilla.
    let raw = a.agenda.tasks()[0].raw.clone();
    a.apply(Action::ToggleTask(raw));
    dropbox.sync();
    catch_up(&mut b);
    assert!(b.agenda.tasks()[0].done);
    assert!(b.note.text.contains("- [x] Enviar planos"), "{}", b.note.text);

    // 3. Sin conexión: los dos escriben en la misma nota y anotan en «Lo que hizo».
    type_line(&mut a, "A2");
    a.log_ai(crate::activity::Kind::Organizar, "General/Obra", "en A".into(), vec![], false);
    type_line(&mut b, "B2");
    b.log_ai(crate::activity::Kind::Organizar, "General/Obra", "en B".into(), vec![], false);
    // Vuelve la conexión: Dropbox deja copias en conflicto; A las junta y todo se reparte.
    dropbox.sync();
    assert!(files(&dir_a).iter().any(|f| f.contains("copia en conflicto")));
    catch_up(&mut a);
    dropbox.sync();
    catch_up(&mut b);
    dropbox.sync();
    catch_up(&mut a);
    dropbox.sync();

    // Nada se perdió, cada línea está una sola vez, y las dos carpetas quedaron iguales.
    let text = fs::read_to_string(&obra_a).unwrap();
    for l in ["Inicio", "A1", "B1", "A2", "B2"] {
        assert_eq!(text.lines().filter(|x| *x == l).count(), 1, "«{l}» en:\n{text}");
    }
    assert!(text.contains("- [x] Enviar planos ^pl001"), "{text}");
    assert_eq!(a.note.text, text);
    assert_eq!(b.note.text, text);
    assert!(!files(&dir_a).iter().any(|f| !f.starts_with(".papelera") && f.contains("copia en conflicto")), "{:?}", files(&dir_a));
    let entries = |app: &NotesApp| app.activity.entries.iter().map(|e| e.text.clone()).collect::<BTreeSet<_>>();
    let expected = ["en A", "en B", "Se juntó una copia en conflicto de «Obra»"].map(String::from);
    assert_eq!(entries(&a), BTreeSet::from(expected), "lo de los dos equipos, más el aviso de que se juntó la copia");
    assert_eq!(entries(&b), entries(&a));
    let (fa, fb) = (files(&dir_a), files(&dir_b));
    assert_eq!(fa, fb);
    for f in &fa {
        assert_eq!(fs::read(dir_a.join(f)).unwrap(), fs::read(dir_b.join(f)).unwrap(), "{f} distinto en los dos equipos");
    }
    let _ = fs::remove_dir_all(&base);
}

//! Banco de pruebas de rendimiento (R1 del roadmap).
//!
//! Genera carpetas de notas realistas (siempre las mismas, para comparar antes y después) y
//! mide la app de verdad, sin abrir ventana: cargar, dibujar Inicio y una nota, la revisión
//! de la carpeta que se hace cada segundo, buscar y la RAM.
//!
//! ```text
//! cargo test --release rendimiento -- --ignored --nocapture
//! NODEX_BENCH=1000,10000 cargo test --release rendimiento -- --ignored --nocapture
//! ```
//!
//! Las carpetas quedan en la carpeta temporal (`nodex-bench/`) y se reutilizan. El resultado se
//! imprime y se agrega a `target/rendimiento.md`.

use super::*;

/// Cambiar si cambia el generador (así se vuelven a crear las carpetas).
const GENERATOR: &str = "v1";
const SPACES: [&str; 12] = [
    "General", "Consorcio", "Docencia", "Obra Talca", "Proyecto LAV", "Clientes", "Personal", "Investigación", "Finanzas", "Casa",
    "Lecturas", "Viajes",
];
const WORDS: &str = "revisar planos muro cubicación informe entrega cliente reunión acuerdo presupuesto obra hormigón \
    acero viga losa fundación talud trinchera inspección visita terreno memoria cálculo norma sismo modelo análisis \
    curso clase alumnos prueba nota evaluación proyecto avance plazo contrato factura pago cotización proveedor \
    material bodega llamar enviar pedir confirmar coordinar preparar documento correo planilla idea libro artículo \
    resumen lista compras viaje vuelo hotel médico";
const TAGS: [&str; 10] = ["informe", "planos", "urgente", "idea", "lectura", "pago", "reunión", "cliente", "clase", "obra"];
const PEOPLE: [&str; 6] = ["Juan", "Ana", "Pedro", "María", "Luis", "Carla"];

/// Números pseudoaleatorios repetibles (xorshift).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn range(&mut self, a: usize, b: usize) -> usize {
        a + self.below(b - a + 1)
    }
    fn chance(&mut self, p: f64) -> bool {
        (self.next() % 10_000) as f64 / 10_000.0 < p
    }
}

/// Una frase de entre `lo` y `hi` palabras.
fn sentence(rng: &mut Rng, words: &[&str], lo: usize, hi: usize) -> String {
    let k = rng.range(lo, hi);
    let s: Vec<&str> = (0..k).map(|_| words[rng.below(words.len())]).collect();
    let s = s.join(" ");
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn line(rng: &mut Rng, words: &[&str], today: NaiveDate) -> String {
    let r = rng.below(100);
    let mut s = sentence(rng, words, 4, 14);
    if r < 12 {
        let d = today + chrono::Duration::days(rng.range(0, 120) as i64 - 60);
        let id: String = (0..5).map(|_| char::from(b"abcdefghijkmnpqrstuvwxyz0123456789"[rng.below(34)])).collect();
        let check = if rng.chance(0.4) { "x" } else { " " };
        return format!("- [{check}] {s} due:{d} ^{id}");
    }
    if r < 30 {
        s += &format!(" #{}", TAGS[rng.below(TAGS.len())]);
    }
    match r {
        30..40 => format!("  {s}"),
        40..47 => format!("  - {s}"),
        _ => s,
    }
}

/// Crea (si no existe ya) una carpeta con `n` notas: notas del día, reuniones y notas con título.
fn generate(n: usize, root: &Path) -> f64 {
    let marker = root.join(".banco");
    let tag = format!("{GENERATOR} {n}");
    if vault::read_text(&marker).is_ok_and(|t| t.trim() == tag) {
        return folder_mb(root);
    }
    let _ = fs::remove_dir_all(root);
    let words: Vec<&str> = WORDS.split_whitespace().collect();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ n as u64);
    let today = NaiveDate::from_ymd_opt(2026, 9, 30).expect("fecha");
    let mut used = HashSet::new();
    for i in 0..n {
        let ws = SPACES[rng.below(SPACES.len())];
        let kind = rng.below(100);
        let (mut title, body): (String, Vec<String>) = if kind < 35 {
            let d = today - chrono::Duration::days(rng.below(3650) as i64);
            let k = rng.range(2, 20);
            (d.to_string(), (0..k).map(|_| line(&mut rng, &words, today)).collect())
        } else if kind < 45 {
            let d = today - chrono::Duration::days(rng.below(1500) as i64);
            let title = format!("Reunión {} {}", PEOPLE[rng.below(PEOPLE.len())], sentence(&mut rng, &words, 2, 2).to_lowercase());
            let mut body = vec![format!("## {title} · {d} 10:00")];
            let k = rng.range(4, 25);
            for j in 0..k {
                body.push(format!("- {:02}:{:02} {}", 10 + j / 6, (j * 7) % 60, sentence(&mut rng, &words, 5, 12)));
            }
            body.push("## fin · 11:10".into());
            (title, body)
        } else {
            let title = sentence(&mut rng, &words, 2, 5);
            let k = rng.range(3, 60);
            (title, (0..k).map(|_| line(&mut rng, &words, today)).collect())
        };
        if !used.insert((ws, title.clone())) {
            title = format!("{title} {i}");
            used.insert((ws, title.clone()));
        }
        let dir = root.join(ws);
        fs::create_dir_all(&dir).expect("carpeta");
        fs::write(dir.join(format!("{title}.md")), body.join("\n") + "\n").expect("nota");
    }
    fs::write(&marker, tag).expect("marca");
    folder_mb(root)
}

fn folder_mb(root: &Path) -> f64 {
    fn walk(p: &Path) -> u64 {
        fs::read_dir(p)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| match e.file_type() {
                Ok(t) if t.is_dir() => walk(&e.path()),
                _ => e.metadata().map(|m| m.len()).unwrap_or(0),
            })
            .sum()
    }
    walk(root) as f64 / 1_048_576.0
}

fn ram_mb() -> f64 {
    memory_stats::memory_stats().map_or(0.0, |m| m.physical_mem as f64 / 1_048_576.0)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Un cuadro completo, como en la ventana (1280×800), incluido armar la geometría.
fn frame(ctx: &egui::Context, app: &mut NotesApp) -> Duration {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0))),
        ..Default::default()
    };
    let t = Instant::now();
    let mut out = ctx.run_ui(input, |ui| app.frame(ui));
    let _ = ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
    let took = t.elapsed();
    out.drop_without_applying_deltas();
    took
}

/// Promedio de varios cuadros seguidos (lo que cuesta cada repintado).
fn frames(ctx: &egui::Context, app: &mut NotesApp, k: usize) -> f64 {
    (0..k).map(|_| ms(frame(ctx, app))).sum::<f64>() / k as f64
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Row {
    notes: usize,
    mb: f64,
    load: f64,
    home_first: f64,
    home: f64,
    note_first: f64,
    note: f64,
    rescan: f64,
    search: f64,
    ram: f64,
}

fn measure(n: usize, base: &Path) -> Row {
    let root = base.join(format!("v{n}"));
    let mb = generate(n, &root);
    let cfg_dir = base.join(format!("cfg{n}"));
    let _ = fs::remove_dir_all(&cfg_dir);
    unsafe { std::env::set_var("NODEX_CONFIG_DIR", &cfg_dir) };
    let cfg = Config { carpeta_notas: root.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
    let ctx = egui::Context::default();
    theme::setup(&ctx);

    // Cargar: leer todas las notas y preparar la app.
    let t = Instant::now();
    let mut app = NotesApp::new(cfg, None, ctx.clone());
    let load = ms(t.elapsed());

    // Inicio (es lo que se abre la primera vez cada día).
    app.apply(Action::ShowTab(View::Home));
    let home_first = ms(frame(&ctx, &mut app));
    let home = frames(&ctx, &mut app, 3);

    // Una nota con título del espacio General.
    let note = app.vault.notes_in("General").iter().find(|n| !agenda::is_date(&n.title)).map(|n| n.path.clone()).expect("una nota");
    app.apply(Action::Open(note, None));
    let note_first = ms(frame(&ctx, &mut app));
    let note_ms = frames(&ctx, &mut app, 3);

    // La revisión de la carpeta que se hace cada segundo.
    let rescan = (0..3)
        .map(|_| {
            let t = Instant::now();
            app.poll();
            ms(t.elapsed())
        })
        .sum::<f64>()
        / 3.0;

    // RAM con todo cargado (antes de buscar, que arma muchas líneas de resultados).
    let ram = ram_mb();

    // Buscar una palabra en todas las notas (el cuadro con los resultados).
    app.search = "cubicación".into();
    let search = ms(frame(&ctx, &mut app));
    app.search.clear();

    drop(app);
    let _ = fs::remove_dir_all(&cfg_dir);
    Row { notes: n, mb, load, home_first, home, note_first, note: note_ms, rescan, search, ram }
}

fn seconds(ms: f64) -> String {
    if ms >= 1000.0 { format!("{:.1} s", ms / 1000.0) } else { format!("{ms:.0} ms") }
}

fn table(rows: &[Row]) -> String {
    let mut out = String::from(
        "| Notas | Tamaño | Cargar | 1er cuadro Inicio | Cuadro Inicio | 1er cuadro nota | Cuadro nota | Revisar carpeta | Quieta en Inicio | Quieta en nota | Buscar | RAM (sin ventana) |\n\
         |---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for r in rows {
        // La app repinta y revisa la carpeta una vez por segundo: eso es lo que gasta quieta.
        let idle = |frame: f64| format!("{:.1} %", (frame + r.rescan) / 10.0);
        out += &format!(
            "| {} | {:.1} MB | {} | {} | {} | {} | {} | {} | {} | {} | {} | {:.0} MB |\n",
            r.notes,
            r.mb,
            seconds(r.load),
            seconds(r.home_first),
            seconds(r.home),
            seconds(r.note_first),
            seconds(r.note),
            seconds(r.rescan),
            idle(r.home),
            idle(r.note),
            seconds(r.search),
            r.ram
        );
    }
    out
}

/// Mide un tamaño en este proceso (lo llama `rendimiento` en un proceso aparte por tamaño,
/// para que la RAM y la carga no arrastren lo de las mediciones anteriores).
fn measure_alone(n: usize, base: &Path) -> Row {
    // Calentamiento: lo que cuesta solo la primera vez en un proceso (hilos, fuentes, TLS…).
    let warm = base.join("v0");
    let _ = fs::create_dir_all(warm.join("General"));
    let _ = measure_warmup(&warm, base);
    measure(n, base)
}

fn measure_warmup(root: &Path, base: &Path) -> Option<()> {
    let cfg_dir = base.join("cfg0");
    unsafe { std::env::set_var("NODEX_CONFIG_DIR", &cfg_dir) };
    let cfg = Config { carpeta_notas: root.to_path_buf(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
    let ctx = egui::Context::default();
    theme::setup(&ctx);
    let mut app = NotesApp::new(cfg, None, ctx.clone());
    frame(&ctx, &mut app);
    drop(app);
    let _ = fs::remove_dir_all(&cfg_dir);
    Some(())
}

#[test]
#[ignore = "banco de pruebas: cargo test --release rendimiento -- --ignored --nocapture"]
fn rendimiento() {
    let base = std::env::temp_dir().join("nodex-bench");
    // Proceso hijo: mide un solo tamaño y devuelve la fila.
    if let Some(n) = std::env::var("NODEX_BENCH_ONE").ok().and_then(|v| v.parse().ok()) {
        let row = measure_alone(n, &base);
        println!("FILA {}", serde_json::to_string(&row).expect("fila"));
        return;
    }
    let sizes: Vec<usize> = std::env::var("NODEX_BENCH")
        .unwrap_or_else(|_| "1000,5000,10000,50000".into())
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    let exe = std::env::current_exe().expect("ejecutable de pruebas");
    let mut rows = Vec::new();
    for n in sizes {
        eprintln!("Midiendo {n} notas…");
        let out = std::process::Command::new(&exe)
            .args(["app::rendimiento::rendimiento", "--exact", "--ignored", "--nocapture"])
            .env("NODEX_BENCH_ONE", n.to_string())
            .output()
            .expect("proceso de medición");
        let text = String::from_utf8_lossy(&out.stdout);
        let Some(row) = text.lines().find_map(|l| l.strip_prefix("FILA ")).and_then(|j| serde_json::from_str::<Row>(j).ok()) else {
            panic!("la medición de {n} notas falló:
{text}
{}", String::from_utf8_lossy(&out.stderr));
        };
        rows.push(row);
        eprintln!("{}", table(&rows[rows.len() - 1..]));
    }
    let optimized = if cfg!(debug_assertions) { "sin optimizar (usar --release)" } else { "--release" };
    let report = format!(
        "\n## {} · v{} · {optimized}\n\n{}\n«Quieta» = lo que cuesta un repintado más la revisión de la carpeta, que la app hace una vez por segundo (en % de un núcleo).\n",
        Local::now().format("%Y-%m-%d %H:%M"),
        env!("CARGO_PKG_VERSION"),
        table(&rows)
    );
    println!("{report}");
    let target = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("rendimiento.md");
    let old = fs::read_to_string(&target).unwrap_or_else(|_| "# Rendimiento de Notas\n".into());
    let _ = fs::write(&target, old + &report);
}

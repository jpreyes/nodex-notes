//! El editor de la nota. El archivo sigue siendo texto plano; aquí solo cambia cómo se ve:
//!
//! - cada nota (línea sin sangría o bloque "##") lleva su número a la izquierda;
//! - las etiquetas son píldoras de color sin el '#';
//! - la sangría y las listas se dibujan con viñetas; las tareas, con casilla;
//! - "due:2026-09-26" se ve como una fecha ("mañana") y "^k3f9a" no se ve;
//! - en las reuniones, la hora de cada línea, el inicio y el fin se ven como etiquetas;
//! - las rutas y direcciones web se abren con un clic;
//! - las tablas (`| a | b |`) se ven alineadas, con bordes; Tab pasa de celda y Enter agrega una fila;
//! - `[[Nota]]` es un enlace a otra nota y `[informe.pdf](../Adjuntos/informe.pdf)`, un archivo
//!   adjunto: se ve solo el nombre (con su ícono) y se abren con un clic.
//!
//! En la línea donde está el cursor se ve el texto tal cual, para poder editarlo.
//! Tab / Shift+Tab cambian la sangría; Enter la mantiene.

use super::*;
use crate::lines::{self, Token};
use crate::links::{self, Target};
use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

/// Tamaño de letra de lo que no se ve ("#", "due:…", sangrías).
const HIDDEN: f32 = 0.5;
/// Espacio antes del nombre de una etiqueta (el punto de color) y después.
const PILL_LEFT: f32 = 15.0;
const PILL_RIGHT: f32 = 9.0;
/// Dónde empieza el texto según el nivel de sangría.
const LEVEL_X: [f32; 5] = [0.0, 22.0, 42.0, 62.0, 82.0];
const CHECK_W: f32 = 25.0;
/// Espacio para el ícono de un archivo adjunto.
const FILE_ICON_W: f32 = 19.0;
/// Margen a cada lado del texto de una celda, y ancho mínimo de una columna.
const CELL_PAD: f32 = 10.0;
const MIN_CELL: f32 = 24.0;
const LABEL_SIZE: f32 = 12.5;

/// Algo que se dibuja sobre el texto; `chars` son posiciones en el texto completo.
#[derive(Clone)]
enum Kind {
    Pill(String),
    /// Sangría con viñeta o casilla; `check` = estado de la casilla.
    Prefix { level: u8, check: Option<bool> },
    Date { label: String, fg: Color32, bg: Color32 },
    Link(Target),
    /// Una línea que es una imagen: su ruta (como está escrita), ancho y alto en pantalla.
    Image(String, f32, f32),
    /// `[[Nota]]`: a qué nota apunta (como está escrito) y si existe.
    NoteLink(String, bool),
    /// `[texto](archivo)`: el archivo, su ícono y si existe.
    FileLink(Target, &'static str, bool),
    /// Una fila de tabla: dónde están sus «|» (caracteres del texto), si es el encabezado y si
    /// es la primera fila.
    TableRow { pipes: Vec<usize>, header: bool, first: bool },
}

/// Cómo se ve una fila de tabla (todas las de una tabla comparten los anchos).
struct TableRow {
    widths: Rc<Vec<f32>>,
    header: bool,
    rule: bool,
    first: bool,
}

/// Las tablas del texto: cada fila con los anchos de sus columnas. Si una tabla no cabe en
/// `width`, no se alinea (se ve como texto).
fn table_layout(text: &str, measure: &dyn Fn(&str, &FontId) -> f32, width: f32) -> HashMap<usize, TableRow> {
    let all: Vec<&str> = text.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    let mut out = HashMap::new();
    let mut i = 0;
    while i < all.len() {
        if !lines::is_table_row(all[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < all.len() && lines::is_table_row(all[i]) {
            i += 1;
        }
        let has_rule = i - start > 1 && lines::is_table_rule(all[start + 1]);
        let mut widths: Vec<f32> = Vec::new();
        for (k, l) in all[start..i].iter().enumerate() {
            if lines::is_table_rule(l) {
                continue;
            }
            let font = if has_rule && k == 0 { theme::bold(EDITOR_SIZE) } else { FontId::proportional(EDITOR_SIZE) };
            for (c, &(a, b)) in lines::table_cells(l).1.iter().enumerate() {
                if widths.len() <= c {
                    widths.push(MIN_CELL);
                }
                widths[c] = widths[c].max(measure(&l[a..b], &font));
            }
        }
        let total: f32 = widths.iter().map(|w| w + 2.0 * CELL_PAD).sum();
        if widths.is_empty() || total > width - 8.0 {
            continue;
        }
        let widths = Rc::new(widths);
        for li in start..i {
            out.insert(li, TableRow { widths: widths.clone(), header: has_rule && li == start, rule: lines::is_table_rule(all[li]), first: li == start });
        }
    }
    out
}

/// Qué hacer con un clic sobre el texto.
enum Click {
    Check,
    Open(Target),
    Note(String),
}

/// Cómo se ve un enlace escrito (`[[Nota]]` o `[texto](destino)`), desde la nota `note`.
fn link_kind(link: &lines::Link, note: &Path, note_exists: &dyn Fn(&str) -> bool) -> Kind {
    match link {
        lines::Link::Note(t) => Kind::NoteLink(t.clone(), note_exists(t)),
        lines::Link::Markdown(dest) if dest.starts_with("http://") || dest.starts_with("https://") || dest.starts_with("www.") => {
            let url = if dest.starts_with("www.") { format!("https://{dest}") } else { dest.clone() };
            Kind::Link(Target::Url(url))
        }
        lines::Link::Markdown(dest) => {
            let path = super::images::resolve(note, dest);
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let exists = path.exists();
            Kind::FileLink(Target::Path(path), super::attachments::file_icon(&name), exists)
        }
    }
}

#[derive(Clone)]
struct Deco {
    kind: Kind,
    chars: Range<usize>,
    line: usize,
    /// Espacio reservado antes del primer carácter.
    lead: f32,
}

/// Dónde quedó dibujado cada carácter (relativo a la esquina del texto).
#[derive(Clone, Copy)]
struct CharBox {
    x: f32,
    w: f32,
    top: f32,
    h: f32,
    row_top: f32,
    row_h: f32,
    row: usize,
}

fn char_boxes(g: &egui::Galley) -> Vec<CharBox> {
    let mut out = Vec::new();
    for (ri, pr) in g.rows.iter().enumerate() {
        let (row_top, row_h) = (pr.pos.y, pr.row.size.y);
        for gl in &pr.row.glyphs {
            out.push(CharBox {
                x: pr.pos.x + gl.pos.x,
                w: gl.advance_width,
                top: pr.pos.y + gl.pos.y - gl.font_ascent,
                h: gl.font_height,
                row_top,
                row_h,
                row: ri,
            });
        }
        if pr.ends_with_newline {
            let x = pr.pos.x + pr.row.size.x;
            out.push(CharBox { x, w: 0.0, top: row_top, h: row_h, row_top, row_h, row: ri });
        }
    }
    out
}

/// Centro vertical del texto normal en la fila de la caja `i`.
fn text_center(boxes: &[CharBox], i: usize) -> f32 {
    let Some(b) = boxes.get(i) else { return 0.0 };
    boxes
        .iter()
        .filter(|o| o.row == b.row && o.h > 4.0 && o.w > 0.0)
        .max_by(|a, b| a.h.total_cmp(&b.h))
        .map_or(b.row_top + b.row_h / 2.0, |o| o.top + o.h / 2.0)
}

/// "hoy", "mañana", "vie 26", "3 oct", "venció 24 sep", con sus colores.
fn due_label(date: &str, done: bool) -> (String, Color32, Color32) {
    const DIAS_CORTOS: [&str; 7] = ["lun", "mar", "mié", "jue", "vie", "sáb", "dom"];
    let gray = (Color32::from_rgb(95, 94, 90), Color32::from_rgb(241, 239, 232));
    let Ok(d) = NaiveDate::parse_from_str(date, "%Y-%m-%d") else {
        return (date.to_string(), gray.0, gray.1);
    };
    let today = Local::now().date_naive();
    let days = (d - today).num_days();
    let short = format!("{} {}", d.day(), MESES[d.month0() as usize]);
    let (text, (fg, bg)) = match days {
        _ if done => (short, gray),
        n if n < 0 => (format!("venció {short}"), (Color32::from_rgb(163, 45, 45), Color32::from_rgb(252, 235, 235))),
        0 => ("hoy".to_string(), (Color32::from_rgb(133, 79, 11), Color32::from_rgb(250, 238, 218))),
        1 => ("mañana".to_string(), (Color32::from_rgb(133, 79, 11), Color32::from_rgb(250, 238, 218))),
        n if n < 7 => (
            format!("{} {}", DIAS_CORTOS[d.weekday().num_days_from_monday() as usize], d.day()),
            (Color32::from_rgb(12, 68, 124), Color32::from_rgb(230, 241, 251)),
        ),
        _ => (short, gray),
    };
    (format!("{} {text}", icon::CALENDAR_BLANK), fg, bg)
}

/// "## Reunión CIC · 2026-09-24 10:00" -> "Reunión · mié 24 sep · 10:00" (en la primera línea el
/// título ya está arriba); "## fin · 10:40" -> "Fin · 10:40". Otras líneas "##": `None`.
fn meeting_label(line: &str, first: bool) -> Option<String> {
    const DIAS_CORTOS: [&str; 7] = ["lun", "mar", "mié", "jue", "vie", "sáb", "dom"];
    let body = line.trim().trim_start_matches('#').trim();
    if lines::is_block_end(line) {
        let time = body.rsplit_once('·').map(|(_, t)| t.trim()).filter(|t| agenda::is_time(t));
        return Some(match time {
            Some(t) => format!("{} Fin · {t}", icon::FLAG_CHECKERED),
            None => format!("{} Fin de la reunión", icon::FLAG_CHECKERED),
        });
    }
    let (title, when) = body.rsplit_once(" · ")?;
    let (date, time) = when.trim().split_once(' ').unwrap_or((when.trim(), ""));
    let d = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    let day = format!("{} {} {}", DIAS_CORTOS[d.weekday().num_days_from_monday() as usize], d.day(), MESES[d.month0() as usize]);
    let name = if first { "Reunión".to_string() } else { title.trim().to_string() };
    let time = if agenda::is_time(time.trim()) { format!(" · {}", time.trim()) } else { String::new() };
    Some(format!("{} {name} · {day}{time}", icon::USERS))
}

/// Enlaces de cada línea, guardados para no buscar en disco en cada cuadro.
#[derive(Default)]
pub(super) struct LinkCache {
    bases: Vec<PathBuf>,
    found: HashMap<String, Vec<(usize, usize, Target)>>,
}

impl LinkCache {
    pub(super) fn new(root: &Path) -> Self {
        LinkCache { bases: links::bases(root), found: HashMap::new() }
    }

    fn get(&mut self, line: &str) -> Vec<(usize, usize, Target)> {
        // Solo las líneas que parecen tener una ruta o una web.
        if !line.contains('/') && !line.contains('\\') {
            return Vec::new();
        }
        if self.found.len() > 2000 {
            self.found.clear();
        }
        let bases = &self.bases;
        self.found.entry(line.to_string()).or_insert_with(|| links::find(line, bases)).clone()
    }
}

/// Lo que `build` necesita saber de afuera.
struct Env<'a> {
    /// Ancho de un texto en una letra.
    measure: &'a dyn Fn(&str, &FontId) -> f32,
    /// Ancho y alto con que se ve una imagen (por la ruta escrita en la nota).
    image: &'a dyn Fn(&str) -> Option<(f32, f32)>,
    /// Cómo se ve un enlace escrito.
    link: &'a dyn Fn(&lines::Link) -> Kind,
    /// Ancho del texto.
    width: f32,
}

/// `build_in` con lo mínimo (para las pruebas).
#[cfg(test)]
fn build(
    text: &str,
    active: Option<usize>,
    cache: &mut LinkCache,
    measure: &dyn Fn(&str) -> f32,
    image: &dyn Fn(&str) -> Option<(f32, f32)>,
    link: &dyn Fn(&lines::Link) -> Kind,
) -> (LayoutJob, Vec<Deco>) {
    build_in(text, active, cache, &Env { measure: &|s, _| measure(s), image, link, width: f32::INFINITY })
}

/// Arma el texto con formato y la lista de cosas a dibujar.
fn build_in(text: &str, active: Option<usize>, cache: &mut LinkCache, env: &Env) -> (LayoutJob, Vec<Deco>) {
    let (measure, image, link) = (env.measure, env.image, env.link);
    let size = EDITOR_SIZE;
    let mut job = LayoutJob::default();
    let mut decos = Vec::new();
    let hidden = fmt(FontId::proportional(HIDDEN), Color32::TRANSPARENT);
    let tables = table_layout(text, measure, env.width);
    let mut ci = 0; // carácter donde empieza la línea
    for (li, full) in text.split_inclusive('\n').enumerate() {
        let line = full.trim_end_matches(['\n', '\r']);
        let ending = &full[line.len()..];
        let n_chars = |a: usize, b: usize| line[a..b].chars().count();
        let is_active = active == Some(li);
        let info = lines::parse(line);

        // Una fila de tabla (salvo la que se edita): cada celda en su columna.
        if let Some(row) = tables.get(&li).filter(|_| !is_active) {
            let lh = Some(size * 1.85);
            let mut fh = hidden.clone();
            if row.rule {
                // La fila «|---|---|» no se ve.
                fh.line_height = Some(3.0);
                job.append(line, 0.0, fh.clone());
                job.append(ending, 0.0, fh);
                ci += full.chars().count();
                continue;
            }
            fh.line_height = lh;
            let mut ft = fmt(if row.header { theme::bold(size) } else { FontId::proportional(size) }, TEXT);
            ft.line_height = lh;
            let col_x = |c: usize| row.widths.iter().take(c).map(|w| w + 2.0 * CELL_PAD).sum::<f32>();
            let (pipes, cells) = lines::table_cells(line);
            let (mut pos, mut x) = (0, 0.0);
            for (c, &(a, b)) in cells.iter().enumerate().take(row.widths.len()) {
                // Desde la «|» hasta el texto, oculto: la «|» queda en el borde de la columna.
                job.append(&line[pos..a], (col_x(c) - x).max(0.0), fh.clone());
                x = col_x(c);
                if b > a {
                    job.append(&line[a..b], CELL_PAD, ft.clone());
                    x += CELL_PAD + measure(&line[a..b], &ft.font_id);
                }
                pos = b.max(a);
            }
            job.append(&line[pos..], (col_x(row.widths.len()) - x).max(0.0), fh.clone());
            job.append(ending, 0.0, fh);
            let pipes = pipes.iter().map(|&p| ci + n_chars(0, p)).collect();
            decos.push(Deco { kind: Kind::TableRow { pipes, header: row.header, first: row.first }, chars: ci..ci + n_chars(0, line.len()), line: li, lead: 0.0 });
            ci += full.chars().count();
            continue;
        }

        // Una imagen (salvo en la línea que se edita): su línea queda del alto de la imagen.
        if let Some((w, h)) = lines::image_of(line).filter(|_| !is_active).and_then(|(_, rel)| Some((rel, image(rel)?))).map(|(rel, (w, h))| {
            decos.push(Deco { kind: Kind::Image(rel.to_string(), w, h), chars: ci..ci + n_chars(0, line.len()), line: li, lead: 0.0 });
            (w, h)
        }) {
            let _ = w;
            let mut f = hidden.clone();
            f.line_height = Some(h + 12.0);
            job.append(line, 0.0, f);
            job.append(ending, 0.0, fmt(FontId::proportional(size), TEXT));
            ci += full.chars().count();
            continue;
        }

        if lines::is_heading(line) || lines::is_block_start(line) || lines::is_block_end(line) {
            let hsize = if line.trim_start().starts_with("# ") { 23.0 } else if line.trim_start().starts_with("## ") { 19.0 } else { 17.0 };
            let bold = fmt(theme::bold(hsize), TEXT);
            if is_active {
                job.append(full, 0.0, bold);
            } else if let Some(label) = meeting_label(line, li == 0) {
                // Inicio y fin de una reunión: una etiqueta con la fecha y la hora.
                let w = measure(&label, &FontId::proportional(LABEL_SIZE)) + 16.0;
                let mut f = hidden.clone();
                f.line_height = Some(28.0);
                job.append(line, w, f);
                let (fg, bg) = (Color32::from_rgb(85, 84, 80), Color32::from_rgb(241, 239, 232));
                decos.push(Deco { kind: Kind::Date { label, fg, bg }, chars: ci..ci + n_chars(0, line.len()), line: li, lead: w });
                job.append(ending, 0.0, fmt(FontId::proportional(size), TEXT));
            } else {
                // Los "#" del título no se ven (salvo en la línea que se edita).
                let t = line.trim_start();
                let mut marker = line.len() - t.len() + t.chars().take_while(|c| *c == '#').count();
                if line[marker..].starts_with(' ') {
                    marker += 1;
                }
                job.append(&line[..marker], 0.0, hidden.clone());
                job.append(&line[marker..], 0.0, bold.clone());
                job.append(ending, 0.0, bold);
            }
            ci += full.chars().count();
            continue;
        }

        let done = info.check == Some(true);
        let mut body = fmt(FontId::proportional(size), if done { MUTED } else { TEXT });
        if done {
            body.strikethrough = Stroke::new(1.0, MUTED);
        }
        let mut pos = 0;
        let mut lead = 0.0;

        // Sangría, viñeta y casilla: no se ven como texto, se dibujan.
        if info.level >= 1 || info.check.is_some() {
            let w = LEVEL_X[info.level as usize] + if info.check.is_some() { CHECK_W } else { 0.0 };
            job.append(&line[..info.prefix], w, hidden.clone());
            decos.push(Deco {
                kind: Kind::Prefix { level: info.level, check: info.check },
                chars: ci..ci + n_chars(0, info.prefix),
                line: li,
                lead: w,
            });
            pos = info.prefix;
        } else if line.len() >= 8 && line.starts_with("- ") && agenda::is_time(&line[2..7]) && line[7..].starts_with(' ') {
            // "- 15:03 " de las reuniones: una etiqueta con la hora (tal cual en la línea que se edita).
            if is_active {
                job.append(&line[..8], 0.0, fmt(FontId::proportional(size), MUTED));
            } else {
                let label = line[2..7].to_string();
                let w = measure(&label, &FontId::proportional(LABEL_SIZE)) + 16.0;
                job.append(&line[..8], w, hidden.clone());
                let (fg, bg) = (Color32::from_rgb(95, 94, 90), Color32::from_rgb(241, 239, 232));
                decos.push(Deco { kind: Kind::Date { label, fg, bg }, chars: ci..ci + 8, line: li, lead: w });
            }
            pos = 8;
        }
        // Seguimiento ("↳ 2026-10-01: …"): la fecha, como etiqueta verde.
        if !is_active {
            if let Some((date, at)) = lines::follow_up(&line[pos..]) {
                let day = if date.is_empty() { "Seguimiento".to_string() } else { super::tracking::short_day(date) };
                let label = format!("{} {day}", icon::ARROW_ELBOW_DOWN_RIGHT);
                let w = measure(&label, &FontId::proportional(LABEL_SIZE)) + 16.0;
                job.append(&line[pos..pos + at], w, hidden.clone());
                let (fg, bg) = (Color32::from_rgb(46, 98, 56), Color32::from_rgb(228, 243, 230));
                decos.push(Deco { kind: Kind::Date { label, fg, bg }, chars: ci + n_chars(0, pos)..ci + n_chars(0, pos + at), line: li, lead: w });
                pos += at;
            }
        }

        // Enlaces escritos ([[Nota]], [texto](archivo)), rutas y webs, etiquetas, fechas e identificadores.
        enum Span {
            Written((usize, usize), Kind),
            Found(Target),
            Tok(Token),
        }
        let mut spans: Vec<(usize, usize, Span)> =
            lines::links(line).into_iter().filter(|(a, ..)| *a >= pos).map(|(a, b, shown, l)| (a, b, Span::Written(shown, link(&l)))).collect();
        let overlaps = |spans: &[(usize, usize, Span)], a: usize, b: usize| spans.iter().any(|(x, y, _)| a < *y && b > *x);
        for (a, b, t) in cache.get(line) {
            if a >= pos && !overlaps(&spans, a, b) {
                spans.push((a, b, Span::Found(t)));
            }
        }
        for (a, b, t) in lines::tokens(line) {
            if a >= pos && !overlaps(&spans, a, b) {
                spans.push((a, b, Span::Tok(t)));
            }
        }
        spans.sort_by_key(|s| s.0);

        for (a, b, span) in spans {
            if a < pos {
                continue;
            }
            if a > pos {
                job.append(&line[pos..a], lead, body.clone());
                lead = 0.0;
            }
            if matches!(span, Span::Tok(_)) {
                // Tachado solo en el texto de una tarea hecha, no entre sus etiquetas.
                body.strikethrough = Stroke::NONE;
            }
            let chars = ci + n_chars(0, a)..ci + n_chars(0, b);
            match span {
                Span::Written((s0, s1), kind) => {
                    if is_active {
                        job.append(&line[a..b], lead, fmt(FontId::proportional(size), ACCENT));
                    } else {
                        // Solo se ve el nombre (con el ícono, si es un archivo); lo demás, oculto.
                        let icon_w = if matches!(kind, Kind::FileLink(..)) { FILE_ICON_W } else { 0.0 };
                        let color = match &kind {
                            Kind::NoteLink(_, false) | Kind::FileLink(_, _, false) => MUTED,
                            _ => ACCENT,
                        };
                        let mut f = fmt(FontId::proportional(size), color);
                        f.underline = Stroke::new(1.0, color.gamma_multiply(0.45));
                        job.append(&line[a..s0], lead + icon_w, hidden.clone());
                        job.append(&line[s0..s1], 0.0, f);
                        job.append(&line[s1..b], 0.0, hidden.clone());
                        decos.push(Deco { kind, chars: ci + n_chars(0, s0)..ci + n_chars(0, s1), line: li, lead: icon_w });
                    }
                }
                Span::Found(t) => {
                    job.append(&line[a..b], lead, fmt(FontId::proportional(size), ACCENT));
                    if !is_active {
                        decos.push(Deco { kind: Kind::Link(t), chars, line: li, lead: 0.0 });
                    }
                }
                Span::Tok(Token::Tag(tag)) => {
                    let c = theme::tag_colors(&tag);
                    if is_active {
                        job.append(&line[a..b], lead, fmt(FontId::proportional(size), c.text));
                    } else {
                        job.append(&line[a..a + 1], lead + PILL_LEFT, hidden.clone());
                        job.append(&line[a + 1..b], 0.0, fmt(FontId::proportional(size - 1.5), c.text));
                        decos.push(Deco { kind: Kind::Pill(tag), chars, line: li, lead: PILL_LEFT });
                        lead = PILL_RIGHT;
                        pos = b;
                        continue;
                    }
                }
                Span::Tok(Token::Due(d)) => {
                    if is_active {
                        job.append(&line[a..b], lead, fmt(FontId::proportional(size - 2.0), MUTED));
                    } else {
                        let (label, fg, bg) = due_label(&d, done);
                        let w = measure(&label, &FontId::proportional(LABEL_SIZE)) + 16.0;
                        job.append(&line[a..b], lead + w, hidden.clone());
                        decos.push(Deco { kind: Kind::Date { label, fg, bg }, chars, line: li, lead: w });
                    }
                }
                Span::Tok(Token::Id) => {
                    let f = if is_active { fmt(FontId::proportional(size - 2.5), MUTED) } else { hidden.clone() };
                    job.append(&line[a..b], lead, f);
                }
            }
            lead = 0.0;
            pos = b;
        }
        job.append(&line[pos..], lead, body.clone());
        if !ending.is_empty() {
            job.append(ending, 0.0, body);
        }
        ci += full.chars().count();
    }
    if text.is_empty() || text.ends_with('\n') {
        job.append("", 0.0, fmt(FontId::proportional(size), TEXT));
    }
    (job, decos)
}

#[derive(Clone, Copy)]
enum TableKey {
    Next,
    Prev,
    Enter,
}

/// Tab (`Next`), Shift+Tab (`Prev`) o Enter en la fila `l` de una tabla, con el cursor en la
/// columna `col` (en caracteres): el texto nuevo y dónde queda el cursor.
/// Tab pasa a la celda siguiente (al final, a la fila de abajo, o agrega una); Enter agrega una
/// fila debajo, salvo en una fila vacía, que se borra (sale de la tabla).
fn table_edit(text: &str, l: usize, col: usize, key: TableKey) -> (String, usize) {
    let raw: Vec<&str> = text.split('\n').collect();
    let ls: Vec<&str> = raw.iter().map(|x| x.trim_end_matches('\r')).collect();
    let cr = if raw[l].ends_with('\r') { "\r" } else { "" };
    let line = ls[l];
    let (pipes, cells) = lines::table_cells(line);
    let n = cells.len().max(1);
    let at = byte_index(line, col);
    let cur = pipes.iter().filter(|&&p| p < at).count().saturating_sub(1);
    // Dónde poner el cursor en la celda `k` de la fila `row` (en una vacía, entre sus espacios).
    let cell_start = |row: &str, k: usize| -> usize {
        let (pipes, cells) = lines::table_cells(row);
        match cells.get(k) {
            Some(&(a, b)) if a < b => a,
            Some(_) => (pipes[k] + 2).min(pipes.get(k + 1).copied().unwrap_or(row.len())).min(row.len()),
            None => row.len(),
        }
    };
    let is_row = |i: usize| ls.get(i).is_some_and(|x| lines::is_table_row(x) && !lines::is_table_rule(x));
    let mut new_lines: Vec<String> = raw.iter().map(|x| x.to_string()).collect();
    let new_row = format!("{}{cr}", lines::empty_table_row(n));
    // (fila, byte) donde queda el cursor
    let target: (usize, usize) = match key {
        TableKey::Next if cur + 1 < cells.len() => (l, cell_start(line, cur + 1)),
        TableKey::Next => {
            let next = if ls.get(l + 1).is_some_and(|x| lines::is_table_rule(x)) { l + 2 } else { l + 1 };
            if is_row(next) {
                (next, cell_start(ls[next], 0))
            } else {
                new_lines.insert(l + 1, new_row);
                (l + 1, 2)
            }
        }
        TableKey::Prev if cur > 0 => (l, cell_start(line, cur - 1)),
        TableKey::Prev => {
            let prev = if l >= 2 && lines::is_table_rule(ls[l - 1]) { Some(l - 2) } else { l.checked_sub(1) };
            match prev.filter(|&p| is_row(p)) {
                Some(p) => (p, cell_start(ls[p], lines::table_cells(ls[p]).1.len().saturating_sub(1))),
                None => (l, cell_start(line, 0)),
            }
        }
        TableKey::Enter if cells.iter().all(|&(a, b)| a == b) && l > 0 && lines::is_table_row(ls[l - 1]) => {
            new_lines[l] = cr.to_string();
            (l, 0)
        }
        TableKey::Enter => {
            new_lines.insert(l + 1, new_row);
            (l + 1, 2)
        }
    };
    let new = new_lines.join("\n");
    let row = new.split('\n').nth(target.0).unwrap_or("");
    let ci = line_starts(&new)[target.0] + row[..target.1.min(row.len())].chars().count();
    (new, ci)
}

/// Los rectángulos de los caracteres `chars` en pantalla.
fn char_rects(boxes: &[CharBox], o: egui::Pos2, chars: Range<usize>) -> Vec<egui::Rect> {
    chars.filter_map(|i| boxes.get(i)).map(|b| egui::Rect::from_min_size(egui::pos2(o.x + b.x, o.y + b.top), egui::vec2(b.w, b.h))).collect()
}

/// Líneas del texto con su posición (en caracteres) de inicio.
fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    let mut ci = 0;
    for c in text.chars() {
        ci += 1;
        if c == '\n' {
            starts.push(ci);
        }
    }
    starts
}

fn line_at(starts: &[usize], ci: usize) -> usize {
    starts.partition_point(|&s| s <= ci).saturating_sub(1)
}

/// Reemplaza una línea (sin su salto de línea) por otra.
pub(super) fn replace_line(text: &str, idx: usize, new: &str) -> String {
    let mut out = String::with_capacity(text.len() + new.len());
    for (i, full) in text.split_inclusive('\n').enumerate() {
        if i == idx {
            let line = full.trim_end_matches(['\n', '\r']);
            out += new;
            out += &full[line.len()..];
        } else {
            out += full;
        }
    }
    out
}

fn nth_line(text: &str, idx: usize) -> &str {
    text.split('\n').nth(idx).unwrap_or("").trim_end_matches('\r')
}

impl NotesApp {
    pub(super) fn editor(&mut self, ui: &mut Ui) {
        let viewport_h = ui.available_height();
        let ctx = ui.ctx().clone();
        let mut reply = None;
        let mut followup = false;
        let mut move_to: Option<String> = None;
        let mut follow: Option<String> = None;
        let mut open_backlink: Option<PathBuf> = None;
        let mut show_history = false;
        let mut attach_now = false;
        let mut table_now = false;
        let mut template_now = false;
        let mut follow_now: Option<usize> = None;
        let backlinks = self.backlinks();
        Self::column(ui, "editor", |ui, col_w| {
            // Título = nombre del archivo; las notas del día muestran su fecha ("Hoy, domingo 27 sep").
            if let Some(h) = day_heading(&vault::stem(&self.note.path)) {
                ui.label(RichText::new(h).font(theme::bold(26.0)));
            } else {
                let title = ui.add(
                    egui::TextEdit::singleline(&mut self.note.title)
                        .id(Id::new("title"))
                        .font(theme::bold(26.0))
                        .frame(Frame::NONE)
                        .hint_text("Sin título")
                        .desired_width(f32::INFINITY),
                );
                if std::mem::take(&mut self.focus_title) {
                    title.request_focus();
                    if let Some(mut st) = TextEditState::load(&ctx, title.id) {
                        let n = self.note.title.chars().count();
                        st.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(0), CCursor::new(n))));
                        st.store(&ctx, title.id);
                    }
                }
                if title.lost_focus() {
                    self.commit_title();
                    if ui.input(|i| i.key_pressed(Key::Enter)) {
                        self.focus_editor = true;
                    }
                }
            }

            let in_meeting = self.meeting.as_ref().is_some_and(|m| m.path == self.note.path);
            let when = match self.note.disk_mtime {
                Some(m) => {
                    let d: DateTime<Local> = m.into();
                    format!("editada {} {} {}", d.day(), MESES[d.month0() as usize], d.format("%H:%M"))
                }
                None => "sin guardar".into(),
            };
            let count = lines::units(&self.note.text).len();
            ui.horizontal(|ui| {
                let notes = if count > 1 { format!("   ·   {}", plural(count, "nota")) } else { String::new() };
                let gap = ui.spacing().item_spacing.x;
                ui.spacing_mut().item_spacing.x = 0.0;
                if vault::in_diary(&self.note.path) {
                    ui.label(RichText::new(format!("{} Diario · la IA lleva cada cosa a su espacio", icon::SUN)).size(12.5).color(MUTED));
                } else if vault::in_templates(&self.note.path) {
                    ui.label(RichText::new(format!("{} Plantilla · se usa desde el + de las pestañas", icon::FILE_DASHED)).size(12.5).color(MUTED))
                        .on_hover_text("Al crear una nota con ella, {{fecha}}, {{hoy}}, {{hora}} y {{titulo}} se cambian por la fecha, el día, la hora y el nombre de la nota");
                } else {
                    // El espacio de la nota: un clic permite moverla a otro.
                    let here = workspace_of(&self.note.path).unwrap_or_else(|| self.ws.clone());
                    let r = ui
                        .add(egui::Label::new(RichText::new(format!("{} {here} {}", icon::FOLDER_SIMPLE, icon::CARET_DOWN)).size(12.5).color(MUTED)).sense(Sense::click()))
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text("Mover la nota a otro espacio");
                    egui::Popup::menu(&r).show(|ui| {
                        ui.label(RichText::new("Mover a").size(12.0).color(MUTED));
                        for w in self.vault.workspaces.iter().filter(|w| **w != here) {
                            if ui.button(format!("{}  {w}", icon::FOLDER_SIMPLE)).clicked() {
                                move_to = Some(w.clone());
                                ui.close();
                            }
                        }
                    });
                }
                ui.label(RichText::new(format!("{notes}   ·   {when}")).size(12.5).color(MUTED));
                // Versiones anteriores de la nota.
                if self.note.disk_mtime.is_some() && !vault::in_templates(&self.note.path) {
                    ui.label(RichText::new("   ·   ").size(12.5).color(MUTED));
                    let r = ui
                        .add(egui::Label::new(RichText::new(format!("{} Historial", icon::CLOCK_COUNTER_CLOCKWISE)).size(12.5).color(MUTED)).sense(Sense::click()))
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text("Ver y recuperar cómo estaba la nota antes");
                    if r.clicked() {
                        show_history = true;
                    }
                }
                // Las notas que enlazan a esta.
                if !backlinks.is_empty() {
                    ui.label(RichText::new("   ·   ").size(12.5).color(MUTED));
                    let r = ui
                        .add(egui::Label::new(RichText::new(format!("{} Enlazada desde {}", icon::LINK_SIMPLE, plural(backlinks.len(), "nota"))).size(12.5).color(ACCENT)).sense(Sense::click()))
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    egui::Popup::menu(&r).show(|ui| {
                        for p in &backlinks {
                            let ws = workspace_of(p).unwrap_or_else(|| vault::DIARY.to_string());
                            if ui.button(format!("{}  {}   ·  {ws}", icon::FILE_TEXT, display_title(&vault::stem(p)))).clicked() {
                                open_backlink = Some(p.clone());
                                ui.close();
                            }
                        }
                    });
                }
                ui.spacing_mut().item_spacing.x = gap;
                if in_meeting {
                    ui.label(RichText::new(format!("  {} Reunión en curso", icon::RECORD)).size(12.5).color(SUCCESS));
                } else if is_meeting(&self.note.text) && self.ai.is_ok() {
                    ui.add_space(8.0);
                    let r = ui.link(RichText::new(format!("{} Correo de seguimiento", icon::ENVELOPE_SIMPLE)).size(12.5));
                    if r.on_hover_text("La IA redacta un correo con el resumen, las decisiones y los acuerdos").clicked() {
                        followup = true;
                    }
                }
            });
            ui.add_space(14.0);

            // Preguntas de la IA sobre esta nota.
            let rel = self.rel(&self.note.path);
            let mine: Vec<(crate::doubts::Doubt, usize)> = self
                .doubts
                .for_note(&rel)
                .filter_map(|d| Some((d.clone(), Self::doubt_unit_number(&self.note.text, d)?)))
                .collect();
            for (d, n) in mine {
                if let Some(r) = self.doubt_card(ui, &d, n, None) {
                    reply = Some(r);
                }
            }

            // Texto
            let id = Id::new(("editor", &self.note.path));
            if let Some(c) = self.pending_cursor.take() {
                let n = self.note.text.chars().count();
                let mut st = TextEditState::load(&ctx, id).unwrap_or_default();
                st.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(c.min(n)))));
                st.store(&ctx, id);
            }
            let focused = ui.memory(|m| m.has_focus(id));
            if focused && !self.link_pick_keys(ui, id) {
                self.edit_keys(ui, id);
            }
            let before = TextEditState::load(&ctx, id).and_then(|s| s.cursor.char_range());
            let starts = line_starts(&self.note.text);
            let active = before.filter(|_| focused).map(|r| line_at(&starts, r.primary.index.0));

            // Las imágenes de la nota: se cargan una vez; cada una, del ancho de la columna.
            self.images.prepare(&ctx, &self.note.path, &self.note.text);
            let image_sizes: HashMap<String, (f32, f32)> = self
                .note
                .text
                .lines()
                .filter_map(lines::image_of)
                .filter_map(|(_, rel)| Some((rel.to_string(), self.images.shown_size(&super::images::resolve(&self.note.path, rel), col_w - 8.0)?)))
                .collect();
            // Ctrl+V con una imagen en el portapapeles (sin texto: egui no la pega).
            let ctrl_v = self.images.ctrl_v_pressed(ui) && !ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Paste(_))));
            if focused && ctrl_v {
                self.paste_image(active);
            }
            // Cómo se ve cada enlace escrito de la nota (si la nota o el archivo existen).
            let mut kinds: HashMap<lines::Link, Kind> = HashMap::new();
            let written: Vec<lines::Link> =
                self.note.text.lines().filter(|l| l.contains('[')).flat_map(|l| lines::links(l).into_iter().map(|(.., k)| k)).collect();
            for l in written {
                if !kinds.contains_key(&l) {
                    let exists = match &l {
                        lines::Link::Note(t) => self.link_target(t).is_some(),
                        _ => false,
                    };
                    kinds.insert(l.clone(), link_kind(&l, &self.note.path, &|_| exists));
                }
            }
            let note_path = self.note.path.clone();
            let link_of = |l: &lines::Link| kinds.get(l).cloned().unwrap_or_else(|| link_kind(l, &note_path, &|_| false));
            let decos: RefCell<Vec<Deco>> = RefCell::new(Vec::new());
            let links = &mut self.links;
            let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
                let measure = |s: &str, font: &FontId| ui.fonts_mut(|f| f.layout_no_wrap(s.to_string(), font.clone(), TEXT).size().x);
                let env = Env { measure: &measure, image: &|rel| image_sizes.get(rel).copied(), link: &link_of, width: wrap };
                let (mut job, d) = build_in(buf.as_str(), active, links, &env);
                *decos.borrow_mut() = d;
                job.wrap.max_width = wrap;
                ui.fonts_mut(|f| f.layout_job(job))
            };
            let hint = if in_meeting {
                "Escribe lo que se va diciendo; cada Enter agrega la hora…"
            } else if vault::in_templates(&self.note.path) {
                "Escribe cómo empieza cada nota de este tipo…  {{fecha}}, {{hoy}}, {{hora}} y {{titulo}} se completan solos"
            } else {
                "Escribe una idea por línea…  Tab la une a la de arriba · #etiqueta · «el viernes» le pone fecha · Ctrl+R reunión"
            };
            let under = ui.painter().add(egui::Shape::Noop);
            let out = egui::TextEdit::multiline(&mut self.note.text)
                .id(id)
                .frame(Frame::NONE)
                .hint_text(hint)
                .desired_width(f32::INFINITY)
                .min_size(egui::vec2(col_w, (viewport_h - 120.0).max(120.0)))
                .lock_focus(true)
                .layouter(&mut layouter)
                .show(ui);
            let decos = decos.into_inner();

            // Clic derecho en una línea: convertirla en tarea (o marcarla hecha).
            if out.response.secondary_clicked() {
                self.menu_line = out.response.interact_pointer_pos().map(|p| line_at(&line_starts(&self.note.text), out.galley.cursor_from_pos(p - out.galley_pos).index.0));
            }
            let mut to_task = None;
            let mut paste_now = false;
            out.response.context_menu(|ui| {
                if ui.button(format!("{}  Pegar imagen del portapapeles", icon::IMAGE)).clicked() {
                    paste_now = true;
                    ui.close();
                }
                if ui.button(format!("{}  Adjuntar archivo…", icon::PAPERCLIP)).on_hover_text("También puedes arrastrar archivos a la ventana").clicked() {
                    attach_now = true;
                    ui.close();
                }
                if ui.button(format!("{}  Insertar tabla", icon::TABLE)).on_hover_text("Tab pasa a la celda siguiente; Enter agrega una fila").clicked() {
                    table_now = true;
                    ui.close();
                }
                if !vault::in_templates(&self.note.path) && ui.button(format!("{}  Guardar como plantilla", icon::FILE_DASHED)).on_hover_text("Para crear notas que empiecen igual (desde el + de las pestañas)").clicked() {
                    template_now = true;
                    ui.close();
                }
                let Some(l) = self.menu_line else { return };
                let line = nth_line(&self.note.text, l);
                if line.trim().is_empty() || lines::is_heading(line) {
                    ui.label(RichText::new("Haz clic derecho en una línea con texto").size(12.5).color(MUTED));
                    return;
                }
                let label = match lines::parse(line).check {
                    None => format!("{}  Convertir en tarea   Ctrl+Enter", icon::CHECK_SQUARE),
                    Some(false) => format!("{}  Marcar hecha   Ctrl+Enter", icon::CHECK_SQUARE),
                    Some(true) => format!("{}  Marcar pendiente   Ctrl+Enter", icon::SQUARE),
                };
                if ui.button(label).clicked() {
                    to_task = Some(l);
                    ui.close();
                }
                if !lines::is_follow_up(line) && ui.button(format!("{}  Seguimiento   Ctrl+Shift+Enter", icon::ARROW_ELBOW_DOWN_RIGHT)).on_hover_text("Anotar debajo qué se hizo, con la fecha (la IA lo tiene en cuenta)").clicked() {
                    follow_now = Some(l);
                    ui.close();
                }
            });
            if paste_now {
                self.paste_image(self.menu_line);
            }
            if attach_now {
                self.pick_attachments(self.menu_line);
            }
            if template_now {
                self.save_as_template();
            }
            if let Some(l) = follow_now {
                self.start_follow_up_line(l);
            }
            if table_now {
                // El cursor queda en la primera celda.
                let first = self.insert_lines_after(self.menu_line, lines::NEW_TABLE.iter().map(|l| l.to_string()).collect());
                self.pending_cursor = line_starts(&self.note.text).get(first).map(|s| s + 2);
                self.focus_editor = true;
            }
            if let Some(l) = to_task {
                self.line_to_task(l);
            }

            if out.response.changed() {
                self.note.dirty = true;
                self.note.last_edit = Instant::now();
                if in_meeting {
                    if let Some(m) = &mut self.meeting {
                        m.last_activity = Instant::now();
                        m.last_time = Local::now();
                    }
                    if ui.input(|i| i.key_pressed(Key::Enter)) {
                        if let Some(cr) = out.cursor_range {
                            self.stamp_new_line(&ctx, id, out.state.clone(), cr.primary.index.0);
                        }
                    }
                }
            }
            follow = self.paint_decorations(ui, &out, &decos, active, under, before);
            // Escribiendo «[[…»: la lista de notas para enlazar.
            self.update_link_pick(&out, out.response.has_focus());
            self.link_pick_ui(&ctx, id);
            self.keep_cursor_out_of_prefix(&ctx, id, before);
            if std::mem::take(&mut self.focus_editor) {
                out.response.request_focus();
            }
            // Al cambiar de línea, la anterior vuelve a verse con sus píldoras.
            let now_line = out.cursor_range.map(|r| line_at(&line_starts(&self.note.text), r.primary.index.0));
            if now_line != active && ui.memory(|m| m.has_focus(id)) {
                ctx.request_repaint();
            }
        });
        if let Some(r) = reply {
            self.handle_reply(r);
        }
        if let Some(ws) = move_to {
            let p = self.note.path.clone();
            self.move_note(p, ws);
        }
        if followup {
            self.start_followup();
        }
        if let Some(t) = follow {
            self.follow_note_link(&t);
        }
        if let Some(p) = open_backlink {
            self.open_in_tab(p, None);
        }
        if show_history {
            self.open_history();
        }
    }

    /// Números, barras, píldoras, casillas, fechas y enlaces; y sus clics.
    fn paint_decorations(
        &mut self,
        ui: &Ui,
        out: &egui::text_edit::TextEditOutput,
        decos: &[Deco],
        active: Option<usize>,
        under: egui::layers::ShapeIdx,
        before: Option<CCursorRange>,
    ) -> Option<String> {
        let boxes = char_boxes(&out.galley);
        if boxes.is_empty() {
            return None;
        }
        let o = out.galley_pos;
        let painter = ui.painter();
        let mut bg: Vec<egui::Shape> = Vec::new();
        let hover = out.response.hover_pos();
        let mut clicked: Option<(usize, Click)> = None;
        let text = self.note.text.clone();
        let starts = line_starts(&text);

        // Números de cada nota y barra que une sus líneas.
        for (n, u) in lines::units(&text).iter().enumerate() {
            let Some(&first) = starts.get(u.first) else { continue };
            let Some(b) = boxes.get(first) else { continue };
            let cy = o.y + text_center(&boxes, first);
            let here = active.is_some_and(|l| (u.first..=u.last).contains(&l));
            painter.text(
                egui::pos2(o.x - 14.0, cy),
                Align2::RIGHT_CENTER,
                (n + 1).to_string(),
                FontId::proportional(12.0),
                if here { ACCENT } else { Color32::from_rgb(170, 170, 164) },
            );
            if u.last > u.first {
                let end_char = starts.get(u.last + 1).map_or(boxes.len() - 1, |s| s - 1).min(boxes.len() - 1);
                let e = &boxes[end_char];
                let top = o.y + b.row_top + b.row_h;
                let bottom = o.y + e.row_top + e.row_h - 4.0;
                if bottom > top {
                    painter.line_segment(
                        [egui::pos2(o.x - 6.0, top + 2.0), egui::pos2(o.x - 6.0, bottom)],
                        Stroke::new(2.0, theme::BORDER),
                    );
                }
            }
        }

        for d in decos {
            let Some(first) = boxes.get(d.chars.start) else { continue };
            match &d.kind {
                Kind::Pill(tag) => {
                    let Some(last) = boxes.get(d.chars.end.saturating_sub(1)) else { continue };
                    if last.row != first.row {
                        continue;
                    }
                    let c = theme::tag_colors(tag);
                    let name = boxes.get(d.chars.start + 1).unwrap_or(last);
                    let cy = o.y + name.top + name.h / 2.0;
                    let h = name.h + 4.0;
                    let rect = egui::Rect::from_min_max(
                        egui::pos2(o.x + first.x - d.lead + 2.0, cy - h / 2.0),
                        egui::pos2(o.x + last.x + last.w + PILL_RIGHT - 2.0, cy + h / 2.0),
                    );
                    bg.push(egui::Shape::rect_filled(rect, h / 2.0, c.bg));
                    bg.push(egui::Shape::rect_stroke(rect, h / 2.0, Stroke::new(1.0, c.border), egui::StrokeKind::Inside));
                    bg.push(egui::Shape::circle_filled(egui::pos2(rect.left() + 7.5, cy), 2.8, c.dot));
                }
                Kind::Prefix { level, check } => {
                    let x0 = o.x + first.x - d.lead;
                    let cy = o.y + text_center(&boxes, d.chars.start);
                    let base = x0 + LEVEL_X[*level as usize];
                    if *level >= 2 {
                        let center = egui::pos2(base - 11.0, cy);
                        if *level % 2 == 0 {
                            painter.circle_filled(center, 2.6, MUTED);
                        } else {
                            painter.circle_stroke(center, 2.6, Stroke::new(1.2, MUTED));
                        }
                    }
                    if let Some(done) = check {
                        let r = egui::Rect::from_center_size(egui::pos2(base + 8.5, cy), egui::vec2(15.0, 15.0));
                        let hovered = hover.is_some_and(|p| r.expand(3.0).contains(p));
                        if *done {
                            bg.push(egui::Shape::rect_filled(r, 4.0, ACCENT));
                            let pts = [
                                egui::pos2(r.left() + 3.5, r.center().y + 0.5),
                                egui::pos2(r.left() + 6.5, r.bottom() - 4.0),
                                egui::pos2(r.right() - 3.5, r.top() + 4.0),
                            ];
                            painter.line(pts.to_vec(), Stroke::new(1.8, Color32::WHITE));
                        } else {
                            let stroke = if hovered { ACCENT } else { Color32::from_rgb(150, 150, 144) };
                            bg.push(egui::Shape::rect_filled(r, 4.0, Color32::WHITE));
                            bg.push(egui::Shape::rect_stroke(r, 4.0, Stroke::new(1.5, stroke), egui::StrokeKind::Inside));
                        }
                        if hovered {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            if out.response.clicked() {
                                clicked = Some((d.line, Click::Check));
                            }
                        }
                    }
                }
                Kind::Date { label, fg, bg: fill } => {
                    let cy = o.y + text_center(&boxes, d.chars.start);
                    let right = o.x + first.x - 4.0;
                    let rect = egui::Rect::from_min_max(egui::pos2(right - d.lead + 8.0, cy - 10.0), egui::pos2(right, cy + 10.0));
                    bg.push(egui::Shape::rect_filled(rect, 5.0, *fill));
                    painter.text(
                        egui::pos2(rect.left() + 6.0, cy),
                        Align2::LEFT_CENTER,
                        label,
                        FontId::proportional(LABEL_SIZE),
                        *fg,
                    );
                }
                Kind::Image(rel, w, h) => {
                    let file = super::images::resolve(&self.note.path, rel);
                    let Some(tex) = self.images.texture(&file) else { continue };
                    let rect = egui::Rect::from_min_size(egui::pos2(o.x + first.x, o.y + first.row_top + 6.0), egui::vec2(*w, *h));
                    painter.image(tex, rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                    painter.rect_stroke(rect, 4.0, Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Outside);
                    if hover.is_some_and(|p| rect.contains(p)) {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), Id::new("imagen-tip"), egui::PopupAnchor::Pointer)
                            .show(|ui| ui.label(RichText::new("Abrir en tamaño real").size(12.5)));
                        if out.response.clicked() {
                            clicked = Some((d.line, Click::Open(Target::Path(file))));
                        }
                    }
                }
                Kind::Link(target) => {
                    let rects: Vec<egui::Rect> = d
                        .chars
                        .clone()
                        .filter_map(|i| boxes.get(i))
                        .map(|b| egui::Rect::from_min_size(egui::pos2(o.x + b.x, o.y + b.top), egui::vec2(b.w, b.h)))
                        .collect();
                    if hover.is_some_and(|p| rects.iter().any(|r| r.contains(p))) {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        let tip = match target {
                            Target::Url(u) => format!("Abrir {u}"),
                            Target::Path(p) => format!("Abrir {}", p.display()),
                        };
                        egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), Id::new("link-tip"), egui::PopupAnchor::Pointer)
                            .show(|ui| ui.label(RichText::new(tip).size(12.5)));
                        if out.response.clicked() {
                            clicked = Some((d.line, Click::Open(target.clone())));
                        }
                    }
                }
                Kind::TableRow { pipes, header, first: is_first } => {
                    let Some(b0) = boxes.get(d.chars.start) else { continue };
                    let (top, bottom) = (o.y + b0.row_top, o.y + b0.row_top + b0.row_h);
                    let mut xs: Vec<f32> = pipes.iter().filter_map(|&i| boxes.get(i)).map(|b| o.x + b.x).collect();
                    // Sin «|» al final: el borde derecho es donde termina la fila.
                    if let Some(end) = d.chars.end.checked_sub(1).and_then(|i| boxes.get(i)) {
                        let r = o.x + end.x + end.w;
                        if xs.last().is_none_or(|&x| r > x + 4.0) {
                            xs.push(r);
                        }
                    }
                    let (Some(&l), Some(&r)) = (xs.first(), xs.last()) else { continue };
                    let line = Stroke::new(1.0, theme::BORDER);
                    if *header {
                        bg.push(egui::Shape::rect_filled(egui::Rect::from_min_max(egui::pos2(l, top), egui::pos2(r, bottom)), 0.0, BG_SIDE));
                    }
                    // Arriba solo en la primera fila; las demás tapan la fila «|---|» oculta.
                    let from = if *is_first { top } else { top - 4.0 };
                    if *is_first {
                        painter.hline(l..=r, top, line);
                    }
                    painter.hline(l..=r, bottom, if *header { Stroke::new(1.5, Color32::from_rgb(200, 198, 190)) } else { line });
                    for x in xs {
                        painter.vline(x, from..=bottom, line);
                    }
                }
                Kind::NoteLink(target, exists) => {
                    let rects = char_rects(&boxes, o, d.chars.clone());
                    if hover.is_some_and(|p| rects.iter().any(|r| r.contains(p))) {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        let name = target.rsplit('/').next().unwrap_or(target);
                        let tip = if *exists { format!("Abrir «{name}»") } else { format!("Crear la nota «{name}»") };
                        egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), Id::new("link-tip"), egui::PopupAnchor::Pointer)
                            .show(|ui| ui.label(RichText::new(tip).size(12.5)));
                        if out.response.clicked() {
                            clicked = Some((d.line, Click::Note(target.clone())));
                        }
                    }
                }
                Kind::FileLink(target, glyph, exists) => {
                    let cy = o.y + text_center(&boxes, d.chars.start);
                    let color = if *exists { ACCENT } else { MUTED };
                    let icon_rect = egui::Rect::from_center_size(egui::pos2(o.x + first.x - d.lead + 8.0, cy), egui::vec2(16.0, 18.0));
                    painter.text(icon_rect.center(), Align2::CENTER_CENTER, *glyph, FontId::proportional(15.0), color);
                    let mut rects = char_rects(&boxes, o, d.chars.clone());
                    rects.push(icon_rect);
                    if hover.is_some_and(|p| rects.iter().any(|r| r.contains(p))) {
                        let Target::Path(path) = target else { continue };
                        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        let tip = if *exists { format!("Abrir {name}") } else { format!("No se encuentra {}", path.display()) };
                        if *exists {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), Id::new("link-tip"), egui::PopupAnchor::Pointer)
                            .show(|ui| ui.label(RichText::new(tip).size(12.5)));
                        if *exists && out.response.clicked() {
                            clicked = Some((d.line, Click::Open(target.clone())));
                        }
                    }
                }
            }
        }
        painter.set(under, egui::Shape::Vec(bg));

        // Un clic en una casilla o un enlace no mueve el cursor.
        if let Some((line, target)) = clicked {
            if let Some(mut st) = TextEditState::load(ui.ctx(), out.response.id) {
                st.cursor.set_char_range(before);
                st.store(ui.ctx(), out.response.id);
            }
            match target {
                Click::Open(t) => links::open(&t),
                Click::Check => self.toggle_line_check(line),
                Click::Note(t) => return Some(t),
            }
        }
        None
    }

    /// Marca o desmarca la casilla de una línea de la nota abierta (y su tarea en tareas.txt).
    pub(super) fn toggle_line_check(&mut self, line_idx: usize) {
        let old = self.note.text.clone();
        let line = nth_line(&old, line_idx).to_string();
        let new_line = lines::toggle_check(&line);
        if new_line == line {
            return;
        }
        self.note.text = replace_line(&old, line_idx, &new_line);
        self.note.dirty = true;
        self.note.last_edit = Instant::now();
        // Marcar una casilla no cambia el contenido: no hace falta volver a analizarla.
        if self.analyzed.contains(&ai::fnv(&old)) {
            self.analyzed.insert(ai::fnv(&self.note.text));
            self.save_analyzed();
        }
        if let Some(id) = lines::id_of(&new_line) {
            let done = lines::parse(&new_line).check == Some(true);
            match self.agenda.set_done_by_id(&id, done, &today()) {
                Ok(true) => self.gcal_dirty = true,
                Ok(false) => {}
                Err(e) => self.msg(format!("No se pudo actualizar tareas.txt: {e}")),
            }
        }
        self.save();
        // Recién hecha: ¿qué se hizo? (fuera de las plantillas)
        if lines::parse(&new_line).check == Some(true) && !vault::in_templates(&self.note.path) {
            let target = super::tracking::Target::Line { note: self.note.path.clone(), id: lines::id_of(&new_line), text: new_line.clone() };
            self.ask_follow_up(super::tracking::line_title(&new_line), target, true);
        }
    }

    /// Convierte una línea en tarea: casilla, identificador y su tarea en Tareas (en el espacio
    /// de la nota). Si ya era tarea, la marca o desmarca.
    pub(super) fn line_to_task(&mut self, line_idx: usize) {
        let old = self.note.text.clone();
        let line = nth_line(&old, line_idx).to_string();
        let info = lines::parse(&line);
        if line[info.prefix..].trim().is_empty() || lines::is_heading(&line) {
            return;
        }
        if info.check.is_some() {
            self.toggle_line_check(line_idx);
            return;
        }
        // En una plantilla, solo la casilla: la tarea se crea en cada nota hecha con ella.
        if vault::in_templates(&self.note.path) {
            self.note.text = replace_line(&old, line_idx, &lines::make_task(&line));
            self.note.dirty = true;
            self.note.last_edit = Instant::now();
            return;
        }
        let id = new_task_id();
        let new_line = lines::set_meta(&lines::make_task(&line), None, Some(&id));
        self.note.text = replace_line(&old, line_idx, &new_line);
        self.note.dirty = true;
        self.note.last_edit = Instant::now();
        // Poner la casilla no es contenido nuevo: la IA no la vuelve a organizar por esto.
        if self.analyzed.contains(&ai::fnv(&old)) {
            self.analyzed.insert(ai::fnv(&self.note.text));
            self.save_analyzed();
        }
        // El texto de la tarea: las etiquetas quedan como palabras ("#planos" -> "planos").
        let text = new_line[lines::parse(&new_line).prefix..]
            .split_whitespace()
            .filter(|w| !w.starts_with("due:") && !(w.starts_with('^') && w.len() > 3))
            .map(|w| w.trim_start_matches(['#', '+']))
            .filter(|w| !w.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let due = lines::due_of(&new_line).filter(|d| agenda::is_date(d));
        let (rel, ws) = (self.rel(&self.note.path), self.space_of(&self.note.path));
        if let Err(e) = self.agenda.add_task(agenda::format_task(&today(), &text, &ws, due.as_deref(), &rel, Some(&id))) {
            self.msg(format!("No se pudo escribir tareas.txt: {e}"));
        }
        self.gcal_dirty = true;
        self.save();
        self.msg(format!("Tarea: «{text}» (Ctrl+Enter la marca hecha)"));
    }

    /// Tab, Shift+Tab, Enter y Retroceso en líneas con sangría o casilla; Ctrl+Enter, tarea.
    fn edit_keys(&mut self, ui: &Ui, id: Id) {
        let ctx = ui.ctx();
        let Some(mut st) = TextEditState::load(ctx, id) else { return };
        let Some(range) = st.cursor.char_range() else { return };
        let text = self.note.text.clone();
        let starts = line_starts(&text);
        let (lo, hi) = {
            let (a, b) = (range.primary.index.0, range.secondary.index.0);
            (a.min(b), a.max(b))
        };
        let (la, lb) = (line_at(&starts, lo), line_at(&starts, hi));
        // Ctrl+Shift+Enter: un seguimiento debajo de la línea.
        if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Enter)) {
            self.start_follow_up_line(la);
            return;
        }
        // Ctrl+Enter: la línea se vuelve tarea (o, si ya lo es, se marca hecha o pendiente).
        if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter)) {
            let col = range.primary.index.0 - starts[la];
            let prefix = |l: &str| l[..lines::parse(l).prefix].chars().count();
            let before = prefix(nth_line(&text, la));
            self.line_to_task(la);
            let new_line = nth_line(&self.note.text, la).to_string();
            // El cursor queda en el mismo lugar del texto (la casilla se agregó adelante).
            let ci = starts[la] + (col + prefix(&new_line)).saturating_sub(before).min(new_line.chars().count());
            st.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(ci))));
            st.store(ctx, id);
            return;
        }
        // En una tabla: Tab y Shift+Tab pasan de celda; Enter agrega una fila.
        if lo == hi && lines::is_table_row(nth_line(&text, la)) && self.table_keys(ui, id, &text, la, lo - starts[la], st.clone()) {
            return;
        }
        let untab = ui.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::Tab));
        let tab = !untab && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Tab));

        // Posición (línea, columna dentro del texto sin prefijo) para reubicar el cursor.
        let locate = |ci: usize| {
            let l = line_at(&starts, ci);
            let prefix = nth_line(&text, l)[..lines::parse(nth_line(&text, l)).prefix].chars().count();
            (l, (ci - starts[l]).saturating_sub(prefix))
        };
        let place = |t: &str, (l, col): (usize, usize)| {
            let s = line_starts(t);
            let line = nth_line(t, l);
            let prefix = line[..lines::parse(line).prefix].chars().count();
            (s[l.min(s.len() - 1)] + prefix + col).min(s[l.min(s.len() - 1)] + line.chars().count())
        };

        if tab || untab {
            let (p, s) = (locate(range.primary.index.0), locate(range.secondary.index.0));
            let mut new = text.clone();
            for l in la..=lb {
                let line = nth_line(&new, l).to_string();
                if line.trim().is_empty() && la != lb {
                    continue;
                }
                let level = lines::parse(&line).level;
                let target = if tab { (level + 1).min(lines::MAX_LEVEL) } else { level.saturating_sub(1) };
                if target != level {
                    new = replace_line(&new, l, &lines::with_level(&line, target));
                }
            }
            if new != text {
                st.cursor.set_char_range(Some(CCursorRange {
                    primary: CCursor::new(place(&new, p)),
                    secondary: CCursor::new(place(&new, s)),
                    h_pos: None,
                }));
                self.set_text(ctx, id, st, new);
            }
            return;
        }
        if lo != hi {
            return;
        }
        let l = la;
        let line = nth_line(&text, l).to_string();
        let info = lines::parse(&line);
        let col = lo - starts[l];
        let prefix_chars = line[..info.prefix].chars().count();

        // Enter en una línea con sangría: la siguiente sigue igual (una vacía, en cambio, sale un nivel).
        if info.level >= 1 && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
            if line[info.prefix..].trim().is_empty() {
                let new_line = lines::with_level(&line, info.level - 1);
                let new = replace_line(&text, l, &new_line);
                let ci = starts[l] + new_line.chars().count();
                st.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(ci))));
                self.set_text(ctx, id, st, new);
            } else {
                let at = byte_index(&line, col.max(prefix_chars));
                let next = lines::prefix_for(info.level, info.check.map(|_| false)) + line[at..].trim_start();
                let joined = format!("{}\n{}", line[..at].trim_end(), next);
                let new = replace_line(&text, l, &joined);
                let ci = starts[l] + line[..at].trim_end().chars().count() + 1 + lines::prefix_for(info.level, info.check.map(|_| false)).chars().count();
                st.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(ci))));
                self.set_text(ctx, id, st, new);
            }
            return;
        }
        // Retroceso al inicio del texto: primero quita la casilla, después un nivel de sangría.
        let has_prefix = info.level >= 1 || info.check.is_some();
        if has_prefix && col == prefix_chars && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Backspace)) {
            let content = &line[info.prefix..];
            let new_line = match info.check {
                Some(_) if info.level == 0 => content.to_string(),
                Some(_) => lines::prefix_for(info.level, None) + content,
                None => lines::with_level(&line, info.level - 1),
            };
            let new = replace_line(&text, l, &new_line);
            let ci = starts[l] + new_line[..lines::parse(&new_line).prefix].chars().count();
            st.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(ci))));
            self.set_text(ctx, id, st, new);
        }
    }

    /// Tab, Shift+Tab y Enter en la fila `l` de una tabla, con el cursor en la columna `col`.
    fn table_keys(&mut self, ui: &Ui, id: Id, text: &str, l: usize, col: usize, mut st: TextEditState) -> bool {
        let key = if ui.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::Tab)) {
            TableKey::Prev
        } else if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Tab)) {
            TableKey::Next
        } else if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
            TableKey::Enter
        } else {
            return false;
        };
        let (new, ci) = table_edit(text, l, col, key);
        st.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(ci))));
        if new == text {
            st.store(ui.ctx(), id);
            ui.ctx().request_repaint();
        } else {
            self.set_text(ui.ctx(), id, st, new);
        }
        true
    }

    fn set_text(&mut self, ctx: &egui::Context, id: Id, st: TextEditState, text: String) {
        st.store(ctx, id);
        self.note.text = text;
        self.note.dirty = true;
        self.note.last_edit = Instant::now();
        ctx.request_repaint();
    }

    /// El cursor no se queda dentro de la sangría invisible: salta al inicio del texto
    /// (o a la línea anterior, si venía de ahí con la flecha izquierda).
    fn keep_cursor_out_of_prefix(&mut self, ctx: &egui::Context, id: Id, before: Option<CCursorRange>) {
        let Some(mut st) = TextEditState::load(ctx, id) else { return };
        let Some(r) = st.cursor.char_range() else { return };
        if r.primary != r.secondary {
            return;
        }
        let ci = r.primary.index.0;
        let starts = line_starts(&self.note.text);
        let l = line_at(&starts, ci);
        let line = nth_line(&self.note.text, l);
        let info = lines::parse(line);
        if info.level == 0 && info.check.is_none() {
            return;
        }
        let content = starts[l] + line[..info.prefix].chars().count();
        if ci >= content {
            return;
        }
        let from_content = before.is_some_and(|b| b.primary == b.secondary && b.primary.index.0 == content);
        let target = if from_content && l > 0 { starts[l] - 1 } else { content };
        st.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(target))));
        st.store(ctx, id);
        ctx.request_repaint();
    }

    /// En una reunión, cada línea nueva empieza con la hora ("- 15:03 ").
    /// Un Enter sobre una línea que solo tiene la hora la deja en blanco.
    fn stamp_new_line(&mut self, ctx: &egui::Context, id: Id, mut st: TextEditState, ci: usize) {
        let text = &mut self.note.text;
        let bi = byte_index(text, ci);
        if bi == 0 || !text[..bi].ends_with('\n') {
            return;
        }
        let prev_end = bi - 1;
        let prev_start = text[..prev_end].rfind('\n').map_or(0, |p| p + 1);
        let new_ci = if is_empty_stamp(&text[prev_start..prev_end]) {
            let removed = text[prev_start..prev_end].chars().count();
            text.replace_range(prev_start..prev_end, "");
            ci - removed
        } else {
            let stamp = format!("- {} ", Local::now().format("%H:%M"));
            text.insert_str(bi, &stamp);
            ci + stamp.chars().count()
        };
        st.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(new_ci))));
        st.store(ctx, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_links(l: &lines::Link) -> Kind {
        link_kind(l, Path::new("C:/Notas/Obra/Muro.md"), &|t| t == "Planos")
    }

    /// `[[Nota]]` y `[archivo](ruta)`: se ve solo el nombre; el resto queda oculto.
    #[test]
    fn note_and_file_links_show_only_their_name() {
        let mut cache = LinkCache::default();
        let text = "Ver [[Planos]] y [[Otra|esta]] con [acta.pdf](../Adjuntos/acta%201.pdf)\n";
        let (job, decos) = build(text, None, &mut cache, &|_| 10.0, &|_| None, &test_links);
        let shown: Vec<&str> = job.sections.iter().filter(|s| s.format.color != Color32::TRANSPARENT).map(|s| &job.text[s.byte_range.start.0..s.byte_range.end.0]).collect();
        assert_eq!(shown, vec!["Ver ", "Planos", " y ", "esta", " con ", "acta.pdf", "\n"]);
        let kinds: Vec<String> = decos
            .iter()
            .map(|d| match &d.kind {
                Kind::NoteLink(t, e) => format!("nota {t} {e}"),
                Kind::FileLink(Target::Path(p), _, e) => format!("archivo {} {e}", p.file_name().unwrap().to_string_lossy()),
                _ => "otro".into(),
            })
            .collect();
        assert_eq!(kinds, vec!["nota Planos true", "nota Otra false", "archivo acta 1.pdf false"]);
        // En la línea que se edita, se ve el texto tal cual.
        let (job, decos) = build(text, Some(0), &mut cache, &|_| 10.0, &|_| None, &test_links);
        assert!(decos.is_empty());
        assert!(job.text.contains("[[Otra|esta]]"));
    }

    /// Tab y Enter en una tabla.
    #[test]
    fn tab_and_enter_in_a_table() {
        let t = "| Material | Kg |\n|---|---|\n| Acero | 120 |\nFin";
        let at = |text: &str, ci: usize| -> (usize, usize) {
            let starts = line_starts(text);
            let l = line_at(&starts, ci);
            (l, ci - starts[l])
        };
        // Tab en «Material» → «Kg».
        let (s1, ci) = table_edit(t, 0, 3, TableKey::Next);
        assert_eq!((s1.as_str(), at(t, ci)), (t, (0, 13)));
        // Tab en la última celda del encabezado → primera de la fila de abajo (salta |---|).
        let (_, ci) = table_edit(t, 0, 14, TableKey::Next);
        assert_eq!(at(t, ci), (2, 2));
        // Tab en la última celda de la tabla → una fila nueva.
        let (s2, ci) = table_edit(t, 2, 11, TableKey::Next);
        assert_eq!(s2, "| Material | Kg |\n|---|---|\n| Acero | 120 |\n|  |  |\nFin");
        assert_eq!(at(&s2, ci), (3, 2));
        // Shift+Tab en la primera celda de una fila → última de la de arriba.
        let (_, ci) = table_edit(t, 2, 3, TableKey::Prev);
        assert_eq!(at(t, ci), (0, 13));
        // Enter en una fila vacía: sale de la tabla.
        let (s3, ci) = table_edit(&s2, 3, 2, TableKey::Enter);
        assert_eq!(s3, "| Material | Kg |\n|---|---|\n| Acero | 120 |\n\nFin");
        assert_eq!(at(&s3, ci), (3, 0));
        // Con finales de línea de Windows, se respetan.
        let w = "| a | b |\r\n| c | d |\r\n";
        let (s4, _) = table_edit(w, 1, 9, TableKey::Enter);
        assert_eq!(s4, "| a | b |\r\n| c | d |\r\n|  |  |\r\n");
    }

    /// Las celdas de una tabla quedan en columnas: cada «|» en el borde de su columna.
    #[test]
    fn table_cells_line_up() {
        let mut cache = LinkCache::default();
        let text = "| Material | Kg |\n|---|---|\n| Acero | 120 |\n";
        // Cada letra mide 7 (en negrita, 8).
        let measure = |s: &str, f: &FontId| s.chars().count() as f32 * if f.family == FontId::proportional(1.0).family { 7.0 } else { 8.0 };
        let env = Env { measure: &measure, image: &|_| None, link: &test_links, width: 500.0 };
        let (job, decos) = build_in(text, None, &mut cache, &env);
        let rows: Vec<(bool, bool)> = decos
            .iter()
            .filter_map(|d| match &d.kind {
                Kind::TableRow { header, first, .. } => Some((*header, *first)),
                _ => None,
            })
            .collect();
        assert_eq!(rows, vec![(true, true), (false, false)], "la fila |---| no se dibuja");
        // La segunda celda de cada fila empieza en la misma columna: ancho de la primera + márgenes.
        let lead_of = |cell: &str| job.sections.iter().find(|s| &job.text[s.byte_range.start.0..s.byte_range.end.0] == cell).map(|s| s.leading_space);
        assert_eq!(lead_of("Material"), Some(CELL_PAD));
        assert_eq!(lead_of("Acero"), Some(CELL_PAD));
        // Si no cabe, se ve como texto.
        let narrow = Env { width: 60.0, ..env };
        let (_, decos) = build_in(text, None, &mut cache, &narrow);
        assert!(decos.iter().all(|d| !matches!(d.kind, Kind::TableRow { .. })));
        // La que se edita, tal cual.
        let (_, decos) = build_in(text, Some(2), &mut cache, &env);
        assert_eq!(decos.iter().filter(|d| matches!(d.kind, Kind::TableRow { .. })).count(), 1);
    }

    /// Una línea de imagen se ve como la imagen (su fila, del alto de la imagen), salvo al editarla.
    #[test]
    fn image_lines_become_images() {
        let text = "Antes\n![Captura](../Adjuntos/c.png)\nDespués\n";
        let mut cache = LinkCache::default();
        let size = |rel: &str| (rel == "../Adjuntos/c.png").then_some((300.0, 150.0));
        let (job, decos) = build(text, None, &mut cache, &|_| 10.0, &size, &test_links);
        assert_eq!(job.text, text);
        assert!(decos.iter().any(|d| matches!(&d.kind, Kind::Image(r, w, h) if r == "../Adjuntos/c.png" && *w == 300.0 && *h == 150.0) && d.line == 1));
        let (_, decos) = build(text, Some(1), &mut cache, &|_| 10.0, &size, &test_links);
        assert!(!decos.iter().any(|d| matches!(d.kind, Kind::Image(..))), "en la línea que se edita se ve el texto");
    }

    #[test]
    fn build_hides_hash_due_and_id_except_on_active_line() {
        let text = "- [ ] Entregar #informe due:2026-09-26 ^k3f9a\n  ver /no/existe\n";
        let mut cache = LinkCache::default();
        let (job, decos) = build(text, None, &mut cache, &|s| s.chars().count() as f32 * 7.0, &|_| None, &test_links);
        assert_eq!(job.text, text, "el texto no cambia, solo su formato");
        let kinds: Vec<&str> = decos
            .iter()
            .map(|d| match d.kind {
                Kind::Pill(_) => "pill",
                Kind::Prefix { .. } => "prefix",
                Kind::Date { .. } => "date",
                Kind::Link(_) => "link",
                Kind::Image(..) => "image",
                Kind::NoteLink(..) | Kind::FileLink(..) => "enlace",
                Kind::TableRow { .. } => "tabla",
            })
            .collect();
        assert_eq!(kinds, vec!["prefix", "pill", "date", "prefix"]);
        // El '#' no se ve.
        let hash = text.find('#').unwrap();
        let sec = job.sections.iter().find(|s| s.byte_range.start.0 == hash).unwrap();
        assert_eq!(sec.format.color, Color32::TRANSPARENT);
        // En la línea activa sí.
        let (job, decos) = build(text, Some(0), &mut cache, &|_| 10.0, &|_| None, &test_links);
        let sec = job.sections.iter().find(|s| s.byte_range.start.0 == hash).unwrap();
        assert_ne!(sec.format.color, Color32::TRANSPARENT);
        assert!(!decos.iter().any(|d| matches!(d.kind, Kind::Pill(_))));
    }

    #[test]
    fn headings_hide_their_marks_and_meetings_become_labels() {
        let text = "## Reunión CIC · 2026-09-24 10:00\n- 10:02 hola\n## fin · 10:40\n### Resumen\n";
        let mut cache = LinkCache::default();
        let (job, decos) = build(text, None, &mut cache, &|s| s.chars().count() as f32 * 7.0, &|_| None, &test_links);
        assert_eq!(job.text, text);
        let labels: Vec<String> = decos
            .iter()
            .filter_map(|d| match &d.kind {
                Kind::Date { label, .. } => Some(label.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            labels,
            vec![format!("{} Reunión · jue 24 sep · 10:00", icon::USERS), "10:02".to_string(), format!("{} Fin · 10:40", icon::FLAG_CHECKERED)]
        );
        let hashes = text.find("### ").unwrap();
        let sec = job.sections.iter().find(|s| s.byte_range.start.0 == hashes).unwrap();
        assert_eq!(sec.format.color, Color32::TRANSPARENT, "los ### no se ven");
        assert_eq!(meeting_label("## Visita · 2026-09-24 15:00", false).unwrap(), format!("{} Visita · jue 24 sep · 15:00", icon::USERS));
        assert!(meeting_label("## Ideas para el curso", false).is_none());
        // En la línea que se edita, tal cual.
        let (job, _) = build(text, Some(3), &mut cache, &|_| 10.0, &|_| None, &test_links);
        let sec = job.sections.iter().find(|s| s.byte_range.start.0 == hashes).unwrap();
        assert_ne!(sec.format.color, Color32::TRANSPARENT);
    }

    /// Teclas de verdad en un editor sin ventana: Tab, Enter, Shift+Tab y Retroceso.
    #[test]
    fn keys_change_indent_levels() {
        let dir = std::env::temp_dir().join(format!("nodex-teclas-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("General")).unwrap();
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let ctx = egui::Context::default();
        theme::setup(&ctx);
        let mut app = NotesApp::new(cfg, None, ctx.clone());
        app.view = View::Editor;
        app.focus_editor = true;
        app.note.text = "Uno
dos".into();
        let key = |k: Key, modifiers: Modifiers| egui::Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers };
        let frame = |app: &mut NotesApp, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0))),
                ..Default::default()
            };
            ctx.run_ui(input, |ui| app.editor(ui)).drop_without_applying_deltas();
        };
        frame(&mut app, vec![]);
        frame(&mut app, vec![]);
        let steps: [(Key, Modifiers, &str); 7] = [
            (Key::Tab, Modifiers::NONE, "Uno
  dos"),
            (Key::Tab, Modifiers::NONE, "Uno
  - dos"),
            (Key::Enter, Modifiers::NONE, "Uno
  - dos
  - "),
            (Key::Enter, Modifiers::NONE, "Uno
  - dos
  "),
            (Key::Enter, Modifiers::NONE, "Uno
  - dos
"),
            (Key::Tab, Modifiers::SHIFT, "Uno
  - dos
"),
            (Key::Backspace, Modifiers::NONE, "Uno
  - dos"),
        ];
        for (k, m, want) in steps {
            frame(&mut app, vec![key(k, m)]);
            assert_eq!(app.note.text, want, "tras {k:?} {m:?}");
        }
        // Retroceso al inicio del texto de un ítem: sube un nivel en vez de borrar.
        app.note.text = "Uno
  - dos".into();
        let id = Id::new(("editor", &app.note.path));
        let mut st = TextEditState::load(&ctx, id).unwrap();
        st.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(8))));
        st.store(&ctx, id);
        frame(&mut app, vec![key(Key::Backspace, Modifiers::NONE)]);
        assert_eq!(app.note.text, "Uno
  dos");
        frame(&mut app, vec![key(Key::Backspace, Modifiers::NONE)]);
        assert_eq!(app.note.text, "Uno
dos");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replace_line_keeps_endings() {
        assert_eq!(replace_line("a\r\nb\nc", 1, "B"), "a\r\nB\nc");
        assert_eq!(replace_line("a\nb", 1, "x\ny"), "a\nx\ny");
        assert_eq!(line_at(&line_starts("ab\ncd\n"), 3), 1);
    }
}

//! El editor de la nota. El archivo sigue siendo texto plano; aquí solo cambia cómo se ve:
//!
//! - cada nota (línea sin sangría o bloque "##") lleva su número a la izquierda;
//! - las etiquetas son píldoras de color sin el '#';
//! - la sangría y las listas se dibujan con viñetas; las tareas, con casilla;
//! - "due:2026-09-26" se ve como una fecha ("mañana") y "^k3f9a" no se ve;
//! - las rutas y direcciones web se abren con un clic.
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

/// Tamaño de letra de lo que no se ve ("#", "due:…", sangrías).
const HIDDEN: f32 = 0.5;
/// Espacio antes del nombre de una etiqueta (el punto de color) y después.
const PILL_LEFT: f32 = 15.0;
const PILL_RIGHT: f32 = 9.0;
/// Dónde empieza el texto según el nivel de sangría.
const LEVEL_X: [f32; 5] = [0.0, 22.0, 42.0, 62.0, 82.0];
const CHECK_W: f32 = 25.0;
const LABEL_SIZE: f32 = 12.5;

/// Algo que se dibuja sobre el texto; `chars` son posiciones en el texto completo.
#[derive(Clone)]
enum Kind {
    Pill(String),
    /// Sangría con viñeta o casilla; `check` = estado de la casilla.
    Prefix { level: u8, check: Option<bool> },
    Date { label: String, fg: Color32, bg: Color32 },
    Link(Target),
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

/// Arma el texto con formato y la lista de cosas a dibujar.
fn build(
    text: &str,
    active: Option<usize>,
    cache: &mut LinkCache,
    measure: &dyn Fn(&str) -> f32,
) -> (LayoutJob, Vec<Deco>) {
    let size = EDITOR_SIZE;
    let mut job = LayoutJob::default();
    let mut decos = Vec::new();
    let hidden = fmt(FontId::proportional(HIDDEN), Color32::TRANSPARENT);
    let mut ci = 0; // carácter donde empieza la línea
    for (li, full) in text.split_inclusive('\n').enumerate() {
        let line = full.trim_end_matches(['\n', '\r']);
        let ending = &full[line.len()..];
        let n_chars = |a: usize, b: usize| line[a..b].chars().count();
        let is_active = active == Some(li);
        let info = lines::parse(line);

        if lines::is_heading(line) || lines::is_block_start(line) || lines::is_block_end(line) {
            let hsize = if line.trim_start().starts_with("# ") { 23.0 } else if line.trim_start().starts_with("## ") { 19.0 } else { 17.0 };
            job.append(full, 0.0, fmt(theme::bold(hsize), TEXT));
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
        } else if line.len() >= 8 && line.starts_with("- ") && agenda::is_time(&line[2..7]) {
            // "- 15:03 " de las reuniones, en gris.
            job.append(&line[..8], 0.0, fmt(FontId::proportional(size), MUTED));
            pos = 8;
        }

        // Etiquetas, fechas, identificadores y enlaces.
        let found = cache.get(line);
        let mut spans: Vec<(usize, usize, Option<Token>, Option<Target>)> = found
            .into_iter()
            .filter(|(a, _, _)| *a >= pos)
            .map(|(a, b, t)| (a, b, None, Some(t)))
            .collect();
        for (a, b, t) in lines::tokens(line) {
            if a >= pos && !spans.iter().any(|(x, y, _, _)| a < *y && b > *x) {
                spans.push((a, b, Some(t), None));
            }
        }
        spans.sort_by_key(|s| s.0);

        for (a, b, token, target) in spans {
            if a < pos {
                continue;
            }
            if a > pos {
                job.append(&line[pos..a], lead, body.clone());
                lead = 0.0;
            }
            if token.is_some() {
                // Tachado solo en el texto de una tarea hecha, no entre sus etiquetas.
                body.strikethrough = Stroke::NONE;
            }
            let chars = ci + n_chars(0, a)..ci + n_chars(0, b);
            match (token, target) {
                (_, Some(t)) => {
                    job.append(&line[a..b], lead, fmt(FontId::proportional(size), ACCENT));
                    if !is_active {
                        decos.push(Deco { kind: Kind::Link(t), chars, line: li, lead: 0.0 });
                    }
                }
                (Some(Token::Tag(tag)), _) => {
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
                (Some(Token::Due(d)), _) => {
                    if is_active {
                        job.append(&line[a..b], lead, fmt(FontId::proportional(size - 2.0), MUTED));
                    } else {
                        let (label, fg, bg) = due_label(&d, done);
                        let w = measure(&label) + 16.0;
                        job.append(&line[a..b], lead + w, hidden.clone());
                        decos.push(Deco { kind: Kind::Date { label, fg, bg }, chars, line: li, lead: w });
                    }
                }
                (Some(Token::Id), _) => {
                    let f = if is_active { fmt(FontId::proportional(size - 2.5), MUTED) } else { hidden.clone() };
                    job.append(&line[a..b], lead, f);
                }
                (None, None) => job.append(&line[a..b], lead, body.clone()),
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
        Self::column(ui, "editor", |ui, col_w| {
            // Título = nombre del archivo
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
                ui.label(RichText::new(format!("{} {}{notes}   ·   {when}", icon::FOLDER_SIMPLE, self.ws)).size(12.5).color(MUTED));
                if in_meeting {
                    ui.label(RichText::new(format!("  {} Reunión en curso", icon::RECORD)).size(12.5).color(SUCCESS));
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
            if focused {
                self.edit_keys(ui, id);
            }
            let before = TextEditState::load(&ctx, id).and_then(|s| s.cursor.char_range());
            let starts = line_starts(&self.note.text);
            let active = before.filter(|_| focused).map(|r| line_at(&starts, r.primary.index.0));

            let decos: RefCell<Vec<Deco>> = RefCell::new(Vec::new());
            let links = &mut self.links;
            let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
                let measure = |s: &str| {
                    ui.fonts_mut(|f| f.layout_no_wrap(s.to_string(), FontId::proportional(LABEL_SIZE), TEXT).size().x)
                };
                let (mut job, d) = build(buf.as_str(), active, links, &measure);
                *decos.borrow_mut() = d;
                job.wrap.max_width = wrap;
                ui.fonts_mut(|f| f.layout_job(job))
            };
            let hint = if in_meeting {
                "Escribe lo que se va diciendo; cada Enter agrega la hora…"
            } else {
                "Empieza a escribir…  Cada línea es una nota · Tab la une a la de arriba · #etiqueta"
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
            self.paint_decorations(ui, &out, &decos, active, under, before);
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
    ) {
        let boxes = char_boxes(&out.galley);
        if boxes.is_empty() {
            return;
        }
        let o = out.galley_pos;
        let painter = ui.painter();
        let mut bg: Vec<egui::Shape> = Vec::new();
        let hover = out.response.hover_pos();
        let mut clicked: Option<(usize, Option<Target>)> = None; // (línea, enlace) — sin enlace = casilla
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
                                clicked = Some((d.line, None));
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
                            .show(|ui| ui.label(RichText::new(tip).size(12.0)));
                        if out.response.clicked() {
                            clicked = Some((d.line, Some(target.clone())));
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
                Some(t) => links::open(&t),
                None => self.toggle_line_check(line),
            }
        }
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
    }

    /// Tab, Shift+Tab, Enter y Retroceso en líneas con sangría o casilla.
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

    #[test]
    fn build_hides_hash_due_and_id_except_on_active_line() {
        let text = "- [ ] Entregar #informe due:2026-09-26 ^k3f9a\n  ver /no/existe\n";
        let mut cache = LinkCache::default();
        let (job, decos) = build(text, None, &mut cache, &|s| s.chars().count() as f32 * 7.0);
        assert_eq!(job.text, text, "el texto no cambia, solo su formato");
        let kinds: Vec<&str> = decos
            .iter()
            .map(|d| match d.kind {
                Kind::Pill(_) => "pill",
                Kind::Prefix { .. } => "prefix",
                Kind::Date { .. } => "date",
                Kind::Link(_) => "link",
            })
            .collect();
        assert_eq!(kinds, vec!["prefix", "pill", "date", "prefix"]);
        // El '#' no se ve.
        let hash = text.find('#').unwrap();
        let sec = job.sections.iter().find(|s| s.byte_range.start.0 == hash).unwrap();
        assert_eq!(sec.format.color, Color32::TRANSPARENT);
        // En la línea activa sí.
        let (job, decos) = build(text, Some(0), &mut cache, &|_| 10.0);
        let sec = job.sections.iter().find(|s| s.byte_range.start.0 == hash).unwrap();
        assert_ne!(sec.format.color, Color32::TRANSPARENT);
        assert!(!decos.iter().any(|d| matches!(d.kind, Kind::Pill(_))));
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

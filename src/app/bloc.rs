//! Bloc: un lugar para pegar y escribir cualquier cosa (código, ideas sueltas, claves, tokens) tal
//! cual, sin que se vuelva tareas, etiquetas ni eventos. Tiene páginas: cada una es un archivo en
//! la carpeta `Bloc/` de las notas (no es un espacio; viaja con la carpeta y guarda versiones).
//!
//! - Letra de ancho fijo; Tab escribe una tabulación; nada se transforma.
//! - Los bloques (separados por una línea en blanco) tienen «Copiar» al pasar el mouse.
//! - La IA no lo toca sola. «Ordenar» (a pedido) agrupa los bloques por tipo con títulos, sin
//!   cambiar una letra (si su respuesta no calza, no se aplica), y se puede deshacer.
//! - Lo que parece una clave o un token no se le manda a la IA: va como «[clave 1]».

use super::*;
use egui::text::{CCursor, CCursorRange};
use std::sync::mpsc::Receiver;

/// La carpeta de las páginas, la primera página y el archivo de antes (una sola página).
pub(super) const DIR: &str = "Bloc";
const FIRST: &str = "General";
const OLD_FILE: &str = "Bloc.md";
/// Cuánto de cada bloque ve la IA para ordenarlo (basta para saber de qué es).
const PEEK: usize = 600;

const ORDER_SYSTEM: &str = r#"Ordenas un bloc de notas libre: código, comandos, enlaces, claves, ideas sueltas. Recibes sus bloques numerados (B1, B2…). Agrúpalos por tipo o tema, con títulos cortos en español (por ejemplo «Código», «Comandos», «Claves y tokens», «Enlaces», «Ideas», o el tema concreto si varios bloques tratan de lo mismo). «[clave N]» es una clave oculta. Cada bloque va en exactamente un grupo; no cambies ni resumas los bloques. Responde solo JSON: {"grupos": [{"titulo": "…", "bloques": ["B3", "B1"]}]}"#;

#[derive(Default)]
pub(super) struct Bloc {
    loaded: bool,
    text: String,
    /// Como estaba en disco al leerlo (para juntar si otro equipo lo cambió).
    base: String,
    disk: Option<(SystemTime, u64)>,
    dirty: bool,
    last_edit: Option<Instant>,
    /// Antes de «Ordenar» (para deshacer).
    undo: Option<String>,
    ordering: Option<Receiver<Result<String, String>>>,
    error: Option<String>,
    focus: bool,
    /// El bloque recién copiado (para decir «Copiado»).
    copied: Option<(usize, Instant)>,
    /// Cuándo se guardó la última versión en el historial.
    kept: Option<DateTime<Local>>,
    /// La página abierta ("" = la primera).
    page: String,
    /// Cambiando el nombre de una página: (cuál, nombre nuevo).
    renaming: Option<(String, String)>,
}

/// Lo que se pidió con las páginas.
enum PageDo {
    Open(String),
    New,
    StartRename(String),
    Rename(String, String),
    Delete(String),
}

/// Los bloques del texto (separados por líneas en blanco): primera y última línea, y su texto.
pub(super) fn blocks(text: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let ls: Vec<&str> = text.lines().collect();
    for (i, l) in ls.iter().enumerate() {
        match (l.trim().is_empty(), start) {
            (false, None) => start = Some(i),
            (true, Some(s)) => {
                out.push((s, i - 1, ls[s..i].join("\n")));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push((s, ls.len() - 1, ls[s..].join("\n")));
    }
    out
}

/// ¿Es un título puesto por «Ordenar»? (un bloque de una sola línea «## …»)
fn is_heading_block(b: &str) -> bool {
    !b.contains('\n') && b.starts_with("## ")
}

/// ¿Parece una clave o un token? (prefijos conocidos, o una cadena larga sin espacios que mezcla
/// letras y números)
pub(super) fn looks_secret(word: &str) -> bool {
    let w = word.trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | ',' | ';' | '(' | ')' | '<' | '>'));
    const PREFIXES: [&str; 12] = ["sk-", "sk_", "pk_", "rk_", "ghp_", "gho_", "github_pat_", "xox", "AKIA", "AIza", "eyJ", "re_"];
    if w.starts_with("http://") || w.starts_with("https://") || w.contains('/') && w.contains('.') && !w.contains('=') {
        return false; // un enlace o una ruta
    }
    if PREFIXES.iter().any(|p| w.starts_with(p)) && w.len() >= 16 {
        return true;
    }
    let letters = w.chars().filter(|c| c.is_ascii_alphabetic()).count();
    let digits = w.chars().filter(|c| c.is_ascii_digit()).count();
    let allowed = w.chars().all(|c| c.is_ascii_alphanumeric() || "-_.=+/".contains(c));
    allowed && w.len() >= 24 && letters >= 4 && digits >= 2
}

/// El texto con las claves cambiadas por «[clave N]» (N sigue a partir de `known`, y la misma
/// clave tiene siempre el mismo número).
pub(super) fn mask(text: &str, known: &mut Vec<String>) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let mut first = true;
        for part in line.split(' ') {
            if !first {
                out.push(' ');
            }
            first = false;
            // «clave=valor» o «clave: valor»: se mira el valor.
            let (head, value) = match part.split_once('=') {
                Some((h, v)) if !v.is_empty() => (format!("{h}="), v),
                _ => (String::new(), part),
            };
            if looks_secret(value) {
                let n = match known.iter().position(|k| k == value) {
                    Some(n) => n + 1,
                    None => {
                        known.push(value.to_string());
                        known.len()
                    }
                };
                out += &format!("{head}[clave {n}]");
            } else {
                out += part;
            }
        }
    }
    out
}

/// El bloc ordenado según `groups` (títulos y números de bloque, desde 1). Los bloques que la IA
/// no puso van al final en «Otros». `None` si no calza (para no perder nada).
pub(super) fn arrange(text: &str, groups: &[(String, Vec<usize>)]) -> Option<String> {
    let items: Vec<String> = blocks(text).into_iter().map(|(.., b)| b).filter(|b| !is_heading_block(b)).collect();
    if items.is_empty() {
        return None;
    }
    let mut used = vec![false; items.len()];
    let mut out: Vec<String> = Vec::new();
    for (title, ids) in groups {
        let mine: Vec<usize> = ids.iter().filter_map(|&n| n.checked_sub(1)).filter(|&i| i < items.len() && !std::mem::replace(&mut used[i], true)).collect();
        if mine.is_empty() {
            continue;
        }
        out.push(format!("## {}", title.trim().replace('\n', " ")));
        out.extend(mine.into_iter().map(|i| items[i].clone()));
    }
    let rest: Vec<usize> = (0..items.len()).filter(|&i| !used[i]).collect();
    if !rest.is_empty() {
        out.push("## Otros".into());
        out.extend(rest.into_iter().map(|i| items[i].clone()));
    }
    let new = out.join("\n\n") + "\n";
    // Comprobación: los mismos bloques, ni uno más ni uno menos.
    let mut a: Vec<String> = blocks(&new).into_iter().map(|(.., b)| b).filter(|b| !is_heading_block(b)).collect();
    let mut b = items;
    a.sort();
    b.sort();
    (a == b).then_some(new)
}

/// Lee la respuesta de la IA: {"grupos": [{"titulo", "bloques": ["B1", …]}]}.
fn parse_groups(reply: &str) -> Option<Vec<(String, Vec<usize>)>> {
    let json = &reply[reply.find('{')?..=reply.rfind('}')?];
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let groups = v["grupos"].as_array()?;
    Some(
        groups
            .iter()
            .filter_map(|g| {
                let title = g["titulo"].as_str()?.to_string();
                let ids = g["bloques"].as_array()?.iter().filter_map(|b| b.as_str()?.trim().trim_start_matches(['B', 'b']).parse().ok()).collect();
                Some((title, ids))
            })
            .collect(),
    )
}

impl NotesApp {
    fn bloc_dir(&self) -> PathBuf {
        self.vault.root.join(DIR)
    }

    /// La página abierta.
    fn bloc_page(&self) -> String {
        if self.bloc.page.is_empty() { FIRST.to_string() } else { self.bloc.page.clone() }
    }

    /// El archivo de la página abierta.
    pub(super) fn bloc_path(&self) -> PathBuf {
        self.bloc_dir().join(format!("{}.md", self.bloc_page()))
    }

    /// ¿Es una página del Bloc?
    pub(super) fn is_bloc_page(&self, path: &Path) -> bool {
        path.parent().is_some_and(|p| p == self.bloc_dir())
    }

    /// Las páginas, por nombre (la primera, primero).
    pub(super) fn bloc_pages(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(self.bloc_dir())
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("md")) && crate::conflicts::original_of(&p.file_name().unwrap_or_default().to_string_lossy()).is_none())
            .map(|p| vault::stem(&p))
            .collect();
        if !v.iter().any(|p| p == FIRST) {
            v.push(FIRST.to_string());
        }
        v.sort_by_key(|p| (p != FIRST, p.to_lowercase()));
        v
    }

    /// El Bloc de antes (`Bloc.md`, una sola página) pasa a ser la página «General».
    fn migrate_old_bloc(&self) {
        let old = self.vault.root.join(OLD_FILE);
        let first = self.bloc_dir().join(format!("{FIRST}.md"));
        if old.is_file() && !first.exists() {
            let _ = fs::create_dir_all(self.bloc_dir());
            let _ = fs::rename(&old, &first);
        }
    }

    /// Abre otra página (guardando la que estaba).
    fn open_bloc_page(&mut self, name: &str) {
        self.save_bloc();
        self.bloc.page = name.to_string();
        self.bloc.loaded = false;
        self.bloc.undo = None;
        self.bloc.error = None;
        self.bloc.kept = None;
        self.load_bloc();
        self.bloc.focus = true;
    }

    fn new_bloc_page(&mut self) {
        let pages = self.bloc_pages();
        let name = (2..).map(|i| format!("Página {i}")).find(|n| !pages.contains(n)).unwrap_or_else(|| "Página".into());
        let _ = fs::create_dir_all(self.bloc_dir());
        let _ = fs::write(self.bloc_dir().join(format!("{name}.md")), "");
        self.open_bloc_page(&name);
    }

    fn rename_bloc_page(&mut self, old: &str, new: &str) {
        let new = vault::sanitize(new);
        if new == old || new.trim().is_empty() {
            return;
        }
        if self.bloc_pages().iter().any(|p| p.eq_ignore_ascii_case(&new)) {
            self.msg(format!("Ya hay una página «{new}»"));
            return;
        }
        if old == self.bloc_page() {
            self.save_bloc();
        }
        let (from, to) = (self.bloc_dir().join(format!("{old}.md")), self.bloc_dir().join(format!("{new}.md")));
        let _ = fs::create_dir_all(self.bloc_dir());
        if from.exists() {
            if let Err(e) = fs::rename(&from, &to) {
                self.msg(format!("No se pudo cambiar el nombre: {e}"));
                return;
            }
        } else {
            let _ = fs::write(&to, "");
        }
        crate::history::relink(&self.vault.root, &format!("{DIR}/{old}"), &format!("{DIR}/{new}"));
        if old == self.bloc_page() {
            self.bloc.page = new;
            self.bloc.disk = self.bloc_disk();
        }
    }

    fn delete_bloc_page(&mut self, name: &str) {
        if self.bloc_pages().len() <= 1 {
            self.msg("El Bloc necesita al menos una página");
            return;
        }
        if name == self.bloc_page() {
            self.save_bloc();
        }
        let path = self.bloc_dir().join(format!("{name}.md"));
        if path.exists() {
            if let Err(e) = self.vault.trash(&path) {
                self.msg(format!("No se pudo borrar la página: {e}"));
                return;
            }
        }
        self.msg(format!("Página «{name}» a la papelera"));
        if name == self.bloc_page() {
            let first = self.bloc_pages().into_iter().next().unwrap_or_else(|| FIRST.into());
            self.bloc.dirty = false;
            self.open_bloc_page(&first);
        }
    }

    fn bloc_disk(&self) -> Option<(SystemTime, u64)> {
        fs::metadata(self.bloc_path()).ok().and_then(|m| Some((m.modified().ok()?, m.len())))
    }

    /// Lee el bloc (la primera vez, o si cambió en disco y aquí no hay nada sin guardar).
    fn load_bloc(&mut self) {
        self.migrate_old_bloc();
        let disk = self.bloc_disk();
        if self.bloc.loaded && (self.bloc.dirty || disk == self.bloc.disk) {
            return;
        }
        let text = vault::read_text(&self.bloc_path()).unwrap_or_default();
        self.bloc.text = text.clone();
        self.bloc.base = text;
        self.bloc.disk = disk;
        self.bloc.loaded = true;
    }

    /// Guarda el bloc si hay algo nuevo (juntando con lo que haya llegado de otro equipo).
    pub(super) fn save_bloc(&mut self) {
        if !self.bloc.dirty {
            return;
        }
        let path = self.bloc_path();
        let _ = fs::create_dir_all(self.bloc_dir());
        let disk_now = self.bloc_disk();
        if disk_now != self.bloc.disk {
            if let Ok(disk) = vault::read_text(&path) {
                if disk != self.bloc.base && disk != self.bloc.text {
                    self.bloc.text = crate::merge::merge3(&self.bloc.base, &self.bloc.text, &disk).text;
                }
            }
        }
        // La versión anterior, al historial (como las notas).
        let now = Local::now();
        let rel = format!("{DIR}/{}", self.bloc_page());
        let last = self.bloc.kept.or_else(|| crate::history::last_kept(&self.vault.root, &rel));
        if crate::history::should_keep(&self.bloc.base, &self.bloc.text, last.map(|t| (now - t).to_std().unwrap_or_default())) {
            if crate::history::keep(&self.vault.root, &rel, &self.bloc.base, now).is_ok() {
                self.bloc.kept = Some(now);
            }
        }
        match fs::write(&path, &self.bloc.text) {
            Ok(()) => {
                self.bloc.dirty = false;
                self.bloc.base = self.bloc.text.clone();
                self.bloc.disk = self.bloc_disk();
            }
            Err(e) => self.msg(format!("No se pudo guardar el Bloc: {e}")),
        }
    }

    /// Desde `poll`/cada cuadro: guardar al dejar de escribir.
    pub(super) fn maybe_save_bloc(&mut self) {
        if self.bloc.dirty && self.bloc.last_edit.is_some_and(|t| t.elapsed() >= Duration::from_millis(1200)) {
            self.save_bloc();
        }
    }

    /// Las páginas del Bloc para la IA (para «Preguntar»), con las claves ocultas: (archivo,
    /// página, texto).
    pub(super) fn bloc_for_ai(&mut self) -> Vec<(PathBuf, String, String)> {
        self.load_bloc();
        self.save_bloc();
        let mut known = Vec::new();
        let mut budget = 20_000usize;
        let mut out = Vec::new();
        for page in self.bloc_pages() {
            let path = self.bloc_dir().join(format!("{page}.md"));
            let text = vault::read_text(&path).unwrap_or_default();
            let t: String = text.trim().chars().take(budget).collect();
            if t.is_empty() {
                continue;
            }
            budget = budget.saturating_sub(t.chars().count());
            out.push((path, page, mask(&t, &mut known)));
            if budget == 0 {
                break;
            }
        }
        out
    }

    /// «Ordenar»: la IA agrupa los bloques (sin ver las claves).
    fn order_bloc(&mut self) {
        self.save_bloc();
        let text = self.bloc.text.clone();
        let items: Vec<String> = blocks(&text).into_iter().map(|(.., b)| b).filter(|b| !is_heading_block(b)).collect();
        if items.len() < 2 {
            self.msg("Hay poco que ordenar todavía");
            return;
        }
        let mut known = Vec::new();
        let user: String = items
            .iter()
            .enumerate()
            .map(|(i, b)| format!("B{}:\n<<<\n{}\n>>>\n", i + 1, mask(&b.chars().take(PEEK).collect::<String>(), &mut known)))
            .collect();
        let (cfg, ctx) = (self.cfg.clone(), self.ctx.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let r = ai::complete(&cfg, ORDER_SYSTEM, &user).and_then(|reply| {
                let groups = parse_groups(&reply).ok_or("la IA no respondió con los grupos")?;
                arrange(&text, &groups).ok_or_else(|| "la respuesta de la IA no calzaba con el Bloc; no se cambió nada".to_string())
            });
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        self.bloc.error = None;
        self.bloc.ordering = Some(rx);
    }

    pub(super) fn bloc_view(&mut self, ui: &mut Ui) -> Option<Action> {
        self.load_bloc();
        // La respuesta de «Ordenar».
        if let Some(r) = self.bloc.ordering.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.bloc.ordering = None;
            match r {
                Ok(new) if new != self.bloc.text => {
                    self.bloc.undo = Some(std::mem::replace(&mut self.bloc.text, new));
                    self.bloc.dirty = true;
                    self.bloc.last_edit = Some(Instant::now() - Duration::from_secs(5));
                    self.save_bloc();
                    self.msg("Bloc ordenado (no cambió ninguna letra)");
                }
                Ok(_) => self.msg("El Bloc ya estaba ordenado"),
                Err(e) => self.bloc.error = Some(e),
            }
        }
        let ctx = ui.ctx().clone();
        let page = self.bloc_page();
        let id = Id::new(("bloc", page.clone()));
        let mut copy: Option<(usize, String)> = None;
        let pages = self.bloc_pages();
        let mut page_do: Option<PageDo> = None;
        Self::column(ui, "bloc-vista", |ui, col_w| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Bloc").font(theme::bold(26.0)));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let busy = self.bloc.ordering.is_some();
                    if busy {
                        ui.add(egui::Spinner::new().size(16.0));
                    }
                    let b = egui::Button::new(RichText::new(format!("{}  Ordenar", icon::SPARKLE)));
                    let tip = if self.ai.is_ok() {
                        "La IA agrupa los bloques por tipo, con títulos, sin cambiar una letra (las claves no las ve)"
                    } else {
                        "Configura la IA para ordenar el Bloc"
                    };
                    if ui.add_enabled(!busy && self.ai.is_ok(), b).on_hover_text(tip).on_disabled_hover_text(tip).clicked() {
                        self.order_bloc();
                    }
                    if self.bloc.undo.is_some() {
                        let u = egui::Button::new(RichText::new(format!("{}  Deshacer el orden", icon::ARROW_COUNTER_CLOCKWISE)));
                        if ui.add(u).clicked() {
                            if let Some(old) = self.bloc.undo.take() {
                                self.bloc.text = old;
                                self.bloc.dirty = true;
                                self.save_bloc();
                            }
                        }
                    }
                });
            });
            ui.label(
                RichText::new("Pega o escribe lo que sea: código, ideas, claves. Se guarda solo y queda tal cual; la IA no lo toca salvo que aprietes «Ordenar». Separa las cosas con una línea en blanco.")
                    .size(12.5)
                    .color(MUTED()),
            );
            if let Some(e) = &self.bloc.error {
                ui.label(RichText::new(format!("{} {e}", icon::WARNING_CIRCLE)).size(12.5).color(RED()));
            }
            ui.add_space(8.0);
            // Las páginas: un clic abre; doble clic o clic derecho, cambiar el nombre o borrar.
            let busy = self.bloc.ordering.is_some();
            ui.horizontal_wrapped(|ui| {
                for p in &pages {
                    if let Some((old, new)) = self.bloc.renaming.as_mut().filter(|(o, _)| o == p) {
                        let r = ui.add(egui::TextEdit::singleline(new).desired_width(120.0).margin(Margin::symmetric(6, 3)));
                        if !r.has_focus() && !r.lost_focus() {
                            r.request_focus();
                        }
                        if r.lost_focus() {
                            page_do = Some(if ui.input(|i| i.key_pressed(Key::Escape)) { PageDo::Rename(old.clone(), old.clone()) } else { PageDo::Rename(old.clone(), new.clone()) });
                        }
                        continue;
                    }
                    let label = RichText::new(format!("{}  {p}", icon::NOTEPAD)).size(13.0);
                    let r = ui.add_enabled(!busy || *p == page, egui::Button::selectable(*p == page, label));
                    if r.clicked() && *p != page {
                        page_do = Some(PageDo::Open(p.clone()));
                    }
                    if r.double_clicked() {
                        page_do = Some(PageDo::StartRename(p.clone()));
                    }
                    r.context_menu(|ui| {
                        if ui.button(format!("{}  Cambiar nombre", icon::PENCIL_SIMPLE)).clicked() {
                            page_do = Some(PageDo::StartRename(p.clone()));
                            ui.close();
                        }
                        if ui.button(format!("{}  Borrar página", icon::TRASH)).clicked() {
                            page_do = Some(PageDo::Delete(p.clone()));
                            ui.close();
                        }
                    });
                }
                if ui.add_enabled(!busy, egui::Button::new(RichText::new(icon::PLUS).size(14.0)).frame(false)).on_hover_text("Página nueva").clicked() {
                    page_do = Some(PageDo::New);
                }
            });
            ui.add_space(8.0);
            let out = egui::TextEdit::multiline(&mut self.bloc.text)
                .id(id)
                .font(FontId::monospace(14.0))
                .code_editor()
                .frame(Frame::NONE)
                .hint_text("Pega aquí…")
                .desired_width(f32::INFINITY)
                .min_size(egui::vec2(col_w, (ui.available_height() - 40.0).max(200.0)))
                .lock_focus(true)
                .show(ui);
            if out.response.changed() {
                self.bloc.dirty = true;
                self.bloc.last_edit = Some(Instant::now());
            }
            if std::mem::take(&mut self.bloc.focus) {
                out.response.request_focus();
            }
            // «Copiar» en el bloque que está bajo el mouse.
            let Some(pointer) = ctx.pointer_hover_pos().filter(|p| out.response.rect.contains(*p)) else { return };
            let starts: Vec<usize> = std::iter::once(0).chain(self.bloc.text.match_indices('\n').map(|(i, _)| i + 1)).map(|b| self.bloc.text[..b].chars().count()).collect();
            for (k, (first, last, text)) in blocks(&self.bloc.text).into_iter().enumerate() {
                if is_heading_block(&text) {
                    continue;
                }
                let top = out.galley.pos_from_cursor(CCursor::new(starts[first])).top();
                let end_char = starts.get(last + 1).map_or(self.bloc.text.chars().count(), |s| s.saturating_sub(1));
                let bottom = out.galley.pos_from_cursor(CCursor::new(end_char)).bottom();
                let (top, bottom) = (out.galley_pos.y + top, out.galley_pos.y + bottom);
                if pointer.y < top - 2.0 || pointer.y > bottom + 2.0 {
                    continue;
                }
                let just = self.bloc.copied.is_some_and(|(b, t)| b == k && t.elapsed() < Duration::from_millis(1500));
                let label = if just { format!("{} Copiado", icon::CHECK) } else { format!("{} Copiar", icon::COPY) };
                let rect = egui::Rect::from_min_size(egui::pos2(out.response.rect.right() - 88.0, top), egui::vec2(84.0, 22.0));
                let b = egui::Button::new(RichText::new(label).size(12.5)).fill(theme::c(Color32::WHITE)).stroke(Stroke::new(1.0, theme::BORDER())).corner_radius(6);
                if ui.put(rect, b).on_hover_text("Copiar este bloque").clicked() {
                    copy = Some((k, text));
                }
                ui.painter().vline(out.response.rect.left() - 10.0, top..=bottom, Stroke::new(2.0, ACCENT_BG()));
                break;
            }
        });
        match page_do {
            Some(PageDo::Open(p)) => self.open_bloc_page(&p),
            Some(PageDo::New) => self.new_bloc_page(),
            Some(PageDo::StartRename(p)) => self.bloc.renaming = Some((p.clone(), p)),
            Some(PageDo::Rename(old, new)) => {
                self.bloc.renaming = None;
                self.rename_bloc_page(&old, &new);
            }
            Some(PageDo::Delete(p)) => self.delete_bloc_page(&p),
            None => {}
        }
        if let Some((k, text)) = copy {
            ctx.copy_text(text);
            self.bloc.copied = Some((k, Instant::now()));
        }
        if self.bloc.copied.is_some_and(|(_, t)| t.elapsed() < Duration::from_millis(1600)) {
            ctx.request_repaint_after(Duration::from_millis(300));
        }
        if self.bloc.ordering.is_some() {
            ctx.request_repaint_after(Duration::from_millis(300));
        }
        None
    }

    /// Ctrl+B o el botón del costado: el Bloc, con el cursor al final, listo para pegar.
    pub(super) fn open_bloc(&mut self) {
        self.load_bloc();
        self.bloc.focus = true;
        let n = self.bloc.text.chars().count();
        let id = Id::new(("bloc", self.bloc_page()));
        let mut st = egui::text_edit::TextEditState::load(&self.ctx, id).unwrap_or_default();
        st.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(n))));
        st.store(&self.ctx, id);
    }

    /// Abre una página del Bloc por su archivo (desde una cita de Preguntar).
    pub(super) fn open_bloc_file(&mut self, path: &Path) {
        let page = vault::stem(path);
        if page != self.bloc_page() {
            self.open_bloc_page(&page);
        }
        self.open_bloc();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_hidden_from_the_ai() {
        let mut known = Vec::new();
        let t = "token de resend: re_8fK2mQ9xLp3Vz7Ty1Bn4Wc6\nOPENAI=sk-proj-abc123def456ghi789\nver https://github.com/jpreyes/nodex-notes\nuna idea suelta";
        let m = mask(t, &mut known);
        assert_eq!(m, "token de resend: [clave 1]\nOPENAI=[clave 2]\nver https://github.com/jpreyes/nodex-notes\nuna idea suelta");
        assert_eq!(known.len(), 2);
        // La misma clave, el mismo número.
        assert_eq!(mask("otra vez re_8fK2mQ9xLp3Vz7Ty1Bn4Wc6", &mut known), "otra vez [clave 1]");
        assert!(!looks_secret("transformaciones"), "una palabra larga no es una clave");
        assert!(looks_secret("a1b2c3d4e5f6a7b8c9d0e1f2a3b4"));
    }

    #[test]
    fn ordering_never_loses_a_letter() {
        let t = "fn main() {\n    println!(\"hola\");\n}\n\nComprar cables\n\nre_8fK2mQ9xLp3Vz7Ty1Bn4Wc6\n";
        let groups = vec![("Claves".to_string(), vec![3]), ("Código".to_string(), vec![1])];
        let new = arrange(t, &groups).unwrap();
        assert_eq!(new, "## Claves\n\nre_8fK2mQ9xLp3Vz7Ty1Bn4Wc6\n\n## Código\n\nfn main() {\n    println!(\"hola\");\n}\n\n## Otros\n\nComprar cables\n");
        // Ordenar de nuevo no repite los títulos.
        let again = arrange(&new, &[("Todo".to_string(), vec![1, 2, 3])]).unwrap();
        assert_eq!(again.matches("## ").count(), 1);
        // Números que no existen o repetidos: se ignoran.
        assert!(arrange(t, &[("X".into(), vec![9, 1, 1])]).is_some());
        let g = parse_groups("Aquí va: {\"grupos\": [{\"titulo\": \"Código\", \"bloques\": [\"B2\", \"b1\"]}]}").unwrap();
        assert_eq!(g, vec![("Código".to_string(), vec![2, 1])]);
    }

    /// Con la app: el Bloc de antes pasa a ser «General»; páginas nuevas, con otro nombre o borradas.
    #[test]
    fn pages() {
        let dir = std::env::temp_dir().join(format!("nodex-bloc-paginas-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("General")).unwrap();
        fs::write(dir.join(OLD_FILE), "lo de antes
").unwrap();
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        app.load_bloc();
        assert_eq!(app.bloc.text, "lo de antes
");
        assert!(dir.join("Bloc").join("General.md").exists() && !dir.join(OLD_FILE).exists());
        app.new_bloc_page();
        assert_eq!(app.bloc_pages(), vec!["General".to_string(), "Página 2".to_string()]);
        app.bloc.text = "sk-proj-abc123def456ghi789 token de prueba
".into();
        app.bloc.dirty = true;
        app.save_bloc();
        app.rename_bloc_page("Página 2", "Claves");
        assert_eq!(app.bloc_page(), "Claves");
        assert_eq!(fs::read_to_string(dir.join("Bloc").join("Claves.md")).unwrap(), "sk-proj-abc123def456ghi789 token de prueba
");
        // Preguntar ve todas las páginas, sin las claves.
        let ai = app.bloc_for_ai();
        assert_eq!(ai.iter().map(|(_, p, _)| p.as_str()).collect::<Vec<_>>(), vec!["General", "Claves"]);
        assert!(ai[1].2.contains("[clave 1]") && !ai[1].2.contains("sk-proj"));
        app.delete_bloc_page("Claves");
        assert_eq!(app.bloc_pages(), vec!["General".to_string()]);
        assert_eq!(app.bloc_page(), "General");
        assert!(!app.vault.workspaces.iter().any(|w| w == "Bloc"), "no es un espacio");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn blocks_are_split_by_blank_lines() {
        let b = blocks("uno\ndos\n\n\ntres\n");
        assert_eq!(b, vec![(0, 1, "uno\ndos".to_string()), (4, 4, "tres".to_string())]);
    }
}

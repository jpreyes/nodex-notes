//! Enlaces entre notas: `[[Muro]]`, `[[Obra/Muro]]` (si hay dos con el mismo nombre) o
//! `[[Muro|el muro]]` (se ve «el muro»). Es el mismo formato de Obsidian.
//!
//! - Un clic abre la nota; si no existe, la crea (en el espacio de la nota donde está el enlace).
//! - Al escribir `[[` aparecen las notas que coinciden; Enter o Tab elige.
//! - Arriba de cada nota, «Enlazada desde N notas».
//! - Al renombrar o mover una nota, sus enlaces se actualizan solos.

use super::*;
use crate::lines::Link;
use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;

/// Nombre de una nota tal como se busca: sin «.md», sin «#sección», sin tildes ni mayúsculas.
fn key(target: &str) -> String {
    let t = target.trim();
    let t = t.split('#').next().unwrap_or(t).trim();
    vault::fold(t.strip_suffix(".md").unwrap_or(t))
}

/// Las notas por nombre, rehecho cuando cambia alguna.
#[derive(Default)]
pub(super) struct NoteIndex {
    generation: Option<u64>,
    root: PathBuf,
    by_name: HashMap<String, Vec<PathBuf>>,
    by_rel: HashMap<String, PathBuf>,
    /// (título, espacio o «Diario», ruta), las más recientes primero (para sugerir).
    all: Vec<(String, String, PathBuf)>,
    /// Las notas que enlazan a una, por (versión del índice, nota).
    backlinks: Option<(u64, PathBuf, Vec<PathBuf>)>,
}

impl NoteIndex {
    fn refresh(&mut self, v: &vault::Vault) {
        if self.generation == Some(v.generation) && self.root == v.root {
            return;
        }
        self.generation = Some(v.generation);
        self.root = v.root.clone();
        self.by_name.clear();
        self.by_rel.clear();
        self.all.clear();
        for n in v.all_notes() {
            let title = vault::stem(&n.path);
            let rel = n.path.strip_prefix(&v.root).unwrap_or(&n.path).with_extension("").to_string_lossy().replace('\\', "/");
            self.by_name.entry(vault::fold(&title)).or_default().push(n.path.clone());
            self.by_rel.insert(vault::fold(&rel), n.path.clone());
            let ws = workspace_of(&n.path).unwrap_or_else(|| vault::DIARY.to_string());
            self.all.push((title, ws, n.path.clone()));
        }
        for list in self.by_name.values_mut() {
            list.sort();
        }
        self.backlinks = None;
    }

    /// La nota a la que apunta `target`, visto desde una nota del espacio `from_ws` (si hay dos
    /// con el mismo nombre, gana la del mismo espacio).
    pub(super) fn resolve(&self, target: &str, from_ws: Option<&str>) -> Option<PathBuf> {
        let k = key(target);
        if k.contains('/') {
            return self.by_rel.get(&k).cloned();
        }
        let list = self.by_name.get(&k)?;
        list.iter().find(|p| from_ws.is_some() && workspace_of(p).as_deref() == from_ws).or(list.first()).cloned()
    }

    /// Notas cuyo nombre contiene `query` (primero las que empiezan así), sin `except`.
    fn suggest(&self, query: &str, except: &Path, max: usize) -> Vec<(String, String, PathBuf)> {
        let q = vault::fold(query.trim());
        let mut starts = Vec::new();
        let mut contains = Vec::new();
        for (title, ws, path) in &self.all {
            if path == except {
                continue;
            }
            let f = vault::fold(title);
            if f.starts_with(&q) {
                starts.push((title.clone(), ws.clone(), path.clone()));
            } else if f.contains(&q) {
                contains.push((title.clone(), ws.clone(), path.clone()));
            }
            if starts.len() >= max {
                break;
            }
        }
        starts.extend(contains);
        starts.truncate(max);
        starts
    }

    /// Cómo escribir el enlace a `path`: su nombre, o «Espacio/Nombre» si hay otra con ese nombre.
    fn link_text(&self, title: &str, path: &Path) -> String {
        if self.by_name.get(&vault::fold(title)).is_some_and(|l| l.len() > 1) {
            path.strip_prefix(&self.root).unwrap_or(path).with_extension("").to_string_lossy().replace('\\', "/")
        } else {
            title.to_string()
        }
    }
}

/// `text` con los enlaces a `old` («Obra/Muro», o un espacio «Obra») cambiados a `new`. Con
/// `bare`, también los que solo dicen el nombre (`[[Muro]]`). `None` si no cambió nada.
pub(super) fn rewrite(text: &str, old: &str, new: &str, bare: bool) -> Option<String> {
    if !text.contains("[[") {
        return None;
    }
    let (old_k, old_name) = (vault::fold(old), vault::fold(old.rsplit('/').next().unwrap_or(old)));
    let new_name = new.rsplit('/').next().unwrap_or(new);
    let mut changed = false;
    let mut out = String::with_capacity(text.len());
    for full in text.split_inclusive('\n') {
        let mut line = full.to_string();
        // De atrás hacia adelante, para que no se muevan las posiciones.
        for (a, b, _, link) in lines::links(full).into_iter().rev() {
            let Link::Note(target) = link else { continue };
            let k = key(&target);
            // «#Sección» se conserva.
            let anchor = target.find('#').map_or("", |i| &target[i..]);
            let replacement = if k == old_k {
                Some(format!("{new}{anchor}"))
            } else if k.starts_with(&format!("{old_k}/")) {
                // Una nota dentro del espacio que cambió de nombre.
                Some(format!("{new}{}", target.chars().skip(old.chars().count()).collect::<String>()))
            } else if bare && !k.contains('/') && k == old_name {
                Some(format!("{new_name}{anchor}"))
            } else {
                None
            };
            let Some(r) = replacement else { continue };
            let body = &full[a + 2..b - 2];
            let rest = body.find('|').map_or("", |i| &body[i..]);
            line.replace_range(a..b, &format!("[[{r}{rest}]]"));
            changed = true;
        }
        out += &line;
    }
    changed.then_some(out)
}

/// Sugerencias mientras se escribe `[[…`.
pub(super) struct LinkPick {
    /// Dónde empieza lo escrito después de «[[» (en caracteres del texto).
    start: usize,
    query: String,
    /// (texto para el enlace, título, espacio)
    items: Vec<(String, String, String)>,
    sel: usize,
    /// Dónde dibujar la lista (debajo del cursor) y dónde quedó.
    at: egui::Pos2,
    rect: Option<egui::Rect>,
}

/// Si el cursor está justo después de «[[algo», dónde empieza «algo» y qué dice.
fn typing_link(text: &str, ci: usize) -> Option<(usize, String)> {
    let before: String = text.chars().take(ci).collect();
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let line = &before[line_start..];
    let open = line.rfind("[[")?;
    let query = &line[open + 2..];
    if query.contains(']') || query.contains('[') || query.chars().count() > 60 {
        return None;
    }
    let start = before[..line_start + open + 2].chars().count();
    Some((start, query.to_string()))
}

impl NotesApp {
    pub(super) fn note_index(&mut self) -> &NoteIndex {
        self.note_index.refresh(&self.vault);
        &self.note_index
    }

    /// ¿Existe la nota de un enlace? (desde la nota abierta)
    pub(super) fn link_target(&mut self, target: &str) -> Option<PathBuf> {
        let ws = workspace_of(&self.note.path);
        self.note_index().resolve(target, ws.as_deref())
    }

    /// Clic en un enlace: abre la nota o, si no existe, la crea.
    pub(super) fn follow_note_link(&mut self, target: &str) {
        if let Some(p) = self.link_target(target) {
            self.open_in_tab(p, None);
            return;
        }
        let t = target.split('#').next().unwrap_or(target).trim();
        let here = workspace_of(&self.note.path).unwrap_or_else(|| self.home_ws());
        let (ws, title) = match t.split_once('/') {
            Some((w, name)) if self.vault.workspaces.iter().any(|x| x.eq_ignore_ascii_case(w.trim())) => {
                (self.vault.workspaces.iter().find(|x| x.eq_ignore_ascii_case(w.trim())).cloned().unwrap_or(here), name.trim())
            }
            _ => (here, t.rsplit('/').next().unwrap_or(t).trim()),
        };
        let path = self.vault.note_path(&ws, title);
        self.open_in_tab(path, None);
        self.msg(format!("Nota nueva «{}» en {ws}: se guarda al escribir", vault::sanitize(title)));
    }

    /// Las notas que enlazan a la nota abierta.
    pub(super) fn backlinks(&mut self) -> Vec<PathBuf> {
        self.note_index.refresh(&self.vault);
        let me = self.note.path.clone();
        let generation = self.note_index.generation.unwrap_or(0);
        if let Some((g, p, list)) = &self.note_index.backlinks {
            if *g == generation && *p == me {
                return list.clone();
            }
        }
        let idx = &self.note_index;
        let mut out = Vec::new();
        for n in self.vault.all_notes() {
            if n.path == me || !n.text.contains("[[") {
                continue;
            }
            let ws = workspace_of(&n.path);
            let links_here = n.text.lines().filter(|l| l.contains("[[")).any(|l| {
                lines::links(l).iter().any(|(_, _, _, link)| matches!(link, Link::Note(t) if idx.resolve(t, ws.as_deref()).as_deref() == Some(me.as_path())))
            });
            if links_here {
                out.push(n.path.clone());
            }
        }
        self.note_index.backlinks = Some((generation, me, out.clone()));
        out
    }

    /// Cambia los enlaces a `old` por `new` en todas las notas (ver `rewrite`). Devuelve en cuántas.
    pub(super) fn update_links(&mut self, old: &str, new: &str, bare: bool) -> usize {
        let mut n = 0;
        if let Some(t) = rewrite(&self.note.text, old, new, bare) {
            self.note.text = t;
            self.note.dirty = true;
            self.note.last_edit = Instant::now();
            n += 1;
        }
        let open = self.note.path.clone();
        let changes: Vec<(PathBuf, String)> = self
            .vault
            .all_notes()
            .into_iter()
            .filter(|note| note.path != open)
            .filter_map(|note| Some((note.path.clone(), rewrite(&note.text, old, new, bare)?)))
            .collect();
        for (path, text) in changes {
            if fs::write(&path, &text).is_ok() {
                let m = vault::modified(&path).unwrap_or_else(SystemTime::now);
                self.vault.upsert(path, text, m);
                n += 1;
            }
        }
        n
    }

    /// Después de dibujar el editor: ¿se está escribiendo «[[…»? Prepara la lista.
    pub(super) fn update_link_pick(&mut self, out: &egui::text_edit::TextEditOutput, focused: bool) {
        let cursor = out.cursor_range.filter(|r| r.primary == r.secondary && focused).map(|r| r.primary.index.0);
        // Un clic en la lista le quita el foco al editor: mientras el mouse está encima, sigue.
        let pointer = out.response.ctx.pointer_latest_pos();
        if cursor.is_none() && self.link_pick.as_ref().and_then(|p| p.rect).zip(pointer).is_some_and(|(r, p)| r.contains(p)) {
            return;
        }
        let Some((start, query)) = cursor.and_then(|ci| typing_link(&self.note.text, ci)) else {
            self.link_pick = None;
            self.link_pick_closed = None;
            return;
        };
        if self.link_pick_closed == Some(start) {
            return;
        }
        let ci = cursor.unwrap_or(0);
        let r = out.galley.pos_from_cursor(CCursor::new(ci));
        let at = out.galley_pos + egui::vec2(r.left(), r.bottom() + 4.0);
        if self.link_pick.as_ref().is_some_and(|p| p.start == start && p.query == query) {
            if let Some(p) = &mut self.link_pick {
                p.at = at;
            }
            return;
        }
        let me = self.note.path.clone();
        self.note_index.refresh(&self.vault);
        let idx = &self.note_index;
        let mut items: Vec<(String, String, String)> =
            idx.suggest(&query, &me, 8).into_iter().map(|(title, ws, path)| (idx.link_text(&title, &path), title, ws)).collect();
        let exact = items.iter().any(|(_, t, _)| vault::fold(t) == vault::fold(query.trim()));
        if !query.trim().is_empty() && !exact {
            items.push((query.trim().to_string(), query.trim().to_string(), String::new()));
        }
        self.link_pick = (!items.is_empty()).then_some(LinkPick { start, query, items, sel: 0, at, rect: None });
    }

    /// Antes de dibujar el editor: las teclas de la lista (↑ ↓ Enter Tab Esc).
    pub(super) fn link_pick_keys(&mut self, ui: &Ui, id: Id) -> bool {
        let Some(p) = &mut self.link_pick else { return false };
        let n = p.items.len();
        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown)) {
            p.sel = (p.sel + 1) % n;
            return true;
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp)) {
            p.sel = (p.sel + n - 1) % n;
            return true;
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.link_pick_closed = Some(p.start);
            self.link_pick = None;
            return true;
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter) || i.consume_key(Modifiers::NONE, Key::Tab)) {
            let sel = p.sel;
            self.pick_link(ui.ctx(), id, sel);
            return true;
        }
        false
    }

    /// Escribe el enlace elegido y deja el cursor después de «]]».
    fn pick_link(&mut self, ctx: &egui::Context, id: Id, sel: usize) {
        let Some(p) = self.link_pick.take() else { return };
        let Some((text, _, _)) = p.items.get(sel) else { return };
        let start = byte_index(&self.note.text, p.start);
        let end = byte_index(&self.note.text, p.start + p.query.chars().count());
        let closes = self.note.text[end..].starts_with("]]");
        let insert = if closes { text.clone() } else { format!("{text}]]") };
        self.note.text.replace_range(start..end, &insert);
        let ci = p.start + text.chars().count() + 2;
        let mut st = TextEditState::load(ctx, id).unwrap_or_default();
        st.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(ci))));
        st.store(ctx, id);
        self.note.dirty = true;
        self.note.last_edit = Instant::now();
        self.link_pick_closed = None;
        ctx.memory_mut(|m| m.request_focus(id));
        ctx.request_repaint();
    }

    /// La lista de notas para el enlace que se está escribiendo.
    pub(super) fn link_pick_ui(&mut self, ctx: &egui::Context, id: Id) {
        let Some(p) = &self.link_pick else { return };
        let mut chosen = None;
        let area = egui::Area::new(Id::new("enlace-notas")).order(egui::Order::Foreground).fixed_pos(p.at).show(ctx, |ui| {
            Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(260.0);
                ui.label(RichText::new("Enlazar a una nota  ·  ↑ ↓ y Enter").size(11.5).color(MUTED()));
                for (i, (_, title, ws)) in p.items.iter().enumerate() {
                    let mut label = egui::text::LayoutJob::default();
                    let font = FontId::proportional(13.5);
                    if ws.is_empty() {
                        label.append(&format!("{}  Nota nueva «{title}»", icon::FILE_PLUS), 0.0, egui::TextFormat::simple(font, TEXT()));
                    } else {
                        label.append(&format!("{}  {}", icon::FILE_TEXT, display_title(title)), 0.0, egui::TextFormat::simple(font.clone(), TEXT()));
                        label.append(&format!("   {ws}"), 0.0, egui::TextFormat::simple(FontId::proportional(12.0), MUTED()));
                    }
                    if ui.add(egui::Button::selectable(i == p.sel, label)).clicked() {
                        chosen = Some(i);
                    }
                }
            });
        });
        if let Some(p) = &mut self.link_pick {
            p.rect = Some(area.response.rect);
        }
        if let Some(i) = chosen {
            self.pick_link(ctx, id, i);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_links_when_a_note_moves_or_is_renamed() {
        let t = "Ver [[Muro]] y [[Obra/Muro|el muro]]\notra [[Planos]] y [[obra/muro#Fotos]]\n";
        // Se renombró: los que dicen solo el nombre también cambian.
        let r = rewrite(t, "Obra/Muro", "Obra/Muro sur", true).unwrap();
        assert_eq!(r, "Ver [[Muro sur]] y [[Obra/Muro sur|el muro]]\notra [[Planos]] y [[Obra/Muro sur#Fotos]]\n");
        // Se movió de espacio (el nombre sigue igual): solo los que dicen el espacio.
        let r = rewrite(t, "Obra/Muro", "Talca/Muro", false).unwrap();
        assert_eq!(r, "Ver [[Muro]] y [[Talca/Muro|el muro]]\notra [[Planos]] y [[Talca/Muro#Fotos]]\n");
        // Cambió el nombre del espacio.
        let r = rewrite(t, "Obra", "Obra Talca", false).unwrap();
        assert!(r.contains("[[Obra Talca/Muro|el muro]]") && r.contains("[[Obra Talca/muro#Fotos]]"), "{r}");
        assert_eq!(rewrite("sin enlaces", "Obra/Muro", "x", true), None);
    }

    /// Con la app: renombrar una nota cambia los enlaces de las demás; moverla, los que dicen el espacio.
    #[test]
    fn links_follow_renames_and_moves() {
        let dir = std::env::temp_dir().join(format!("nodex-enlaces-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Obra")).unwrap();
        fs::create_dir_all(dir.join("General")).unwrap();
        let muro = dir.join("Obra").join("Muro.md");
        let ideas = dir.join("General").join("Ideas.md");
        fs::write(&muro, "Revisar armado\n").unwrap();
        fs::write(&ideas, "Ver [[Muro]] y [[Obra/Muro|el muro]]\n").unwrap();
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        app.open(muro.clone(), None);
        assert_eq!(app.backlinks(), vec![ideas.clone()]);
        assert_eq!(app.link_target("muro"), Some(muro.clone()), "sin importar mayúsculas");

        app.note.title = "Muro sur".into();
        app.commit_title();
        assert_eq!(fs::read_to_string(&ideas).unwrap(), "Ver [[Muro sur]] y [[Obra/Muro sur|el muro]]\n");

        let sur = dir.join("Obra").join("Muro sur.md");
        app.move_note(sur.clone(), "General".into());
        assert_eq!(fs::read_to_string(&ideas).unwrap(), "Ver [[Muro sur]] y [[General/Muro sur|el muro]]\n");
        assert_eq!(app.link_target("Muro sur"), Some(dir.join("General").join("Muro sur.md")));

        // Un enlace a una nota que no existe la crea (en el espacio de la nota abierta).
        app.open(ideas.clone(), None);
        app.follow_note_link("Reunión con mandante");
        assert_eq!(app.note.path, dir.join("General").join("Reunión con mandante.md"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn knows_when_a_link_is_being_typed() {
        let t = "hola [[Mu";
        assert_eq!(typing_link(t, t.chars().count()), Some((7, "Mu".into())));
        assert_eq!(typing_link("ya [[Muro]] listo", 17), None);
        assert_eq!(typing_link("[[", 2), Some((2, String::new())));
        assert_eq!(typing_link("[[a\nb", 5), None);
    }
}

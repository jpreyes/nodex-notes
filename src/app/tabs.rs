//! Pestañas, como en un navegador: cada una muestra una nota o una vista (Inicio, IA,
//! Tareas…). La pestaña activa sigue lo que se está viendo; cambiar de pestaña vuelve
//! a lo que mostraba. Se recuerdan entre sesiones (estado.toml).

use super::*;

#[derive(Clone, PartialEq, Debug)]
pub(super) enum Tab {
    Note(PathBuf),
    View(View),
}

pub(super) struct Tabs {
    pub(super) list: Vec<Tab>,
    pub(super) active: usize,
}

impl Tabs {
    /// Mueve la pestaña `from` para que quede antes de la que estaba en `to` (`to` = cantidad:
    /// al final). La activa sigue siendo la misma.
    pub(super) fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.list.len() || to > self.list.len() || to == from || to == from + 1 {
            return;
        }
        let active = self.list.get(self.active).cloned();
        let tab = self.list.remove(from);
        let at = if to > from { to - 1 } else { to };
        self.list.insert(at, tab);
        if let Some(a) = active {
            self.active = self.list.iter().position(|t| *t == a).unwrap_or(0);
        }
    }
}

impl Default for Tabs {
    fn default() -> Self {
        Tabs { list: vec![Tab::View(View::Home)], active: 0 }
    }
}

/// "nota:General/Notas generales", "vista:hoy", "vista:#informe".
pub(super) fn encode(tab: &Tab, root: &Path) -> String {
    match tab {
        Tab::Note(p) => {
            let rel = p.strip_prefix(root).unwrap_or(p).with_extension("");
            format!("nota:{}", rel.to_string_lossy().replace('\\', "/"))
        }
        Tab::View(v) => format!(
            "vista:{}",
            match v {
                View::Home => "inicio".to_string(),
                View::Mail => "correos".into(),
                View::Ai => "ia".into(),
                View::Week => "semana".into(),
                View::Tasks => "tareas".into(),
                View::Agenda => "agenda".into(),
                View::Trash => "papelera".into(),
                View::Bloc => "bloc".into(),
                View::Archive => "archivadas".into(),
                View::Notes => "notas".into(),
                View::Tag(t) => format!("#{t}"),
                View::Editor => "inicio".into(),
            }
        ),
    }
}

pub(super) fn decode(s: &str, root: &Path) -> Option<Tab> {
    if let Some(rel) = s.strip_prefix("nota:") {
        return Some(Tab::Note(root.join(format!("{rel}.md"))));
    }
    let v = match s.strip_prefix("vista:")? {
        "inicio" => View::Home,
        "correos" => View::Mail,
        "hoy" => View::Home, // la vista Hoy ahora es parte de Inicio
        "ia" | "preguntar" => View::Ai,
        "semana" => View::Week,
        "tareas" => View::Tasks,
        "agenda" => View::Agenda,
        "papelera" => View::Trash,
        "bloc" => View::Bloc,
        "archivadas" => View::Archive,
        "notas" => View::Notes,
        t => View::Tag(t.strip_prefix('#')?.to_string()),
    };
    Some(Tab::View(v))
}

fn view_label(v: &View) -> (&'static str, String) {
    match v {
        View::Home | View::Editor => (icon::HOUSE, "Inicio".into()),
        View::Mail => (icon::ENVELOPE_SIMPLE, "Correos".into()),
        View::Ai => (icon::SPARKLE, "IA".into()),
        View::Week => (icon::CALENDAR_CHECK, "Semana".into()),
        View::Tasks => (icon::CHECK_SQUARE, "Tareas".into()),
        View::Agenda => (icon::CALENDAR_BLANK, "Agenda".into()),
        View::Trash => (icon::TRASH, "Papelera".into()),
        View::Bloc => (icon::NOTEPAD, "Bloc".into()),
        View::Archive => (icon::ARCHIVE, "Archivadas".into()),
        View::Notes => (icon::FILE_TEXT, "Notas".into()),
        View::Tag(t) => (icon::HASH, t.clone()),
    }
}

impl NotesApp {
    /// Lo que se está viendo, como pestaña.
    pub(super) fn current_tab(&self) -> Tab {
        match &self.view {
            View::Editor => Tab::Note(self.note.path.clone()),
            v => Tab::View(v.clone()),
        }
    }

    /// La pestaña activa sigue a lo que se está viendo.
    pub(super) fn sync_tab(&mut self) {
        let cur = self.current_tab();
        if self.tabs.active >= self.tabs.list.len() {
            self.tabs.active = self.tabs.list.len().saturating_sub(1);
        }
        match self.tabs.list.get_mut(self.tabs.active) {
            Some(t) if *t != cur => *t = cur.clone(),
            Some(_) => {}
            None => self.tabs.list.push(cur.clone()),
        }
        // Nunca dos pestañas con lo mismo: se queda la activa.
        let active = self.tabs.active.min(self.tabs.list.len() - 1);
        let before = self.tabs.list.len();
        let mut i = 0;
        let mut removed_before = 0;
        self.tabs.list.retain(|t| {
            let keep = i == active || *t != cur;
            if !keep && i < active {
                removed_before += 1;
            }
            i += 1;
            keep
        });
        if self.tabs.list.len() != before {
            self.tabs.active = active - removed_before;
        }
    }

    /// Abre una nota como en un navegador: si ya tiene pestaña, va a esa; si se está viendo una
    /// vista (Inicio, Tareas, IA…), la abre en una pestaña nueva; si no, la muestra en la pestaña activa.
    pub(super) fn open_in_tab(&mut self, path: PathBuf, cursor: Option<usize>) {
        self.sync_tab();
        let tab = Tab::Note(path.clone());
        match self.tabs.list.iter().position(|t| *t == tab) {
            Some(i) if i != self.tabs.active => self.activate_tab(i),
            None if self.view != View::Editor => {
                self.save();
                self.note = OpenNote::load(path.clone());
                self.new_tab(tab);
            }
            _ => {}
        }
        self.open(path, cursor);
    }

    pub(super) fn activate_tab(&mut self, i: usize) {
        let Some(tab) = self.tabs.list.get(i).cloned() else { return };
        self.save();
        self.search.clear();
        self.tabs.active = i;
        match tab {
            Tab::Note(p) => {
                // Si la nota se movió o se renombró, se busca por su nombre.
                let found = if p.exists() || p == self.note.path {
                    Some(p)
                } else {
                    let stem = vault::stem(&p);
                    self.vault.all_notes().iter().find(|n| n.title == stem).map(|n| n.path.clone())
                };
                match found {
                    Some(p) => self.open(p, None),
                    None => {
                        self.view = View::Home;
                        self.tabs.list[i] = Tab::View(View::Home);
                    }
                }
            }
            Tab::View(v) => {
                self.ask.focus = v == View::Ai && self.ai_tab == ai_view::AiTab::Chat;
                self.view = v;
            }
        }
        self.save_estado();
    }

    pub(super) fn new_tab(&mut self, tab: Tab) {
        self.sync_tab();
        let at = (self.tabs.active + 1).min(self.tabs.list.len());
        self.tabs.list.insert(at, tab);
        self.activate_tab(at);
    }

    /// Activa la pestaña que ya muestra esa vista, o abre una nueva.
    pub(super) fn show_in_tab(&mut self, v: View) {
        self.sync_tab();
        match self.tabs.list.iter().position(|t| *t == Tab::View(v.clone())) {
            Some(i) => self.activate_tab(i),
            None => self.new_tab(Tab::View(v)),
        }
    }

    pub(super) fn close_tab(&mut self, i: usize) {
        self.sync_tab();
        if i >= self.tabs.list.len() {
            return;
        }
        if self.tabs.list.len() == 1 {
            self.tabs.list[0] = Tab::View(View::Home);
            self.activate_tab(0);
            return;
        }
        self.tabs.list.remove(i);
        let next = if self.tabs.active > i || self.tabs.active >= self.tabs.list.len() {
            self.tabs.active.saturating_sub(1)
        } else {
            self.tabs.active
        };
        self.activate_tab(next.min(self.tabs.list.len() - 1));
    }

    fn close_others(&mut self, i: usize) {
        self.sync_tab();
        if let Some(t) = self.tabs.list.get(i).cloned() {
            self.tabs.list = vec![t];
            self.activate_tab(0);
        }
    }

    fn tab_label(&self, tab: &Tab) -> (&'static str, String) {
        match tab {
            Tab::Note(p) => {
                let meeting = self.vault.get(p).is_some_and(|n| n.meeting(is_meeting));
                let title = if *p == self.note.path { self.note.title.clone() } else { vault::stem(p) };
                let glyph = if meeting { icon::USERS } else if agenda::is_date(&title) { icon::SUN } else { icon::FILE_TEXT };
                (glyph, display_title(&title))
            }
            Tab::View(v) => view_label(v),
        }
    }

    /// La barra de pestañas (arriba del área principal).
    pub(super) fn tab_bar(&mut self, ui: &mut Ui) {
        self.sync_tab();
        enum Do {
            Activate(usize),
            Close(usize),
            CloseOthers(usize),
            Move(usize, usize),
            Recurring,
            New,
            NewNote,
            NewMeeting,
            FromTemplate(PathBuf),
            EditTemplate(PathBuf),
            NewTemplate,
        }
        let mut todo = None;
        let n = self.tabs.list.len();
        // Arrastrar una pestaña la cambia de lugar: (cuál, dónde se soltaría).
        let mut rects: Vec<egui::Rect> = Vec::with_capacity(n);
        let mut dragging: Option<usize> = None;
        let mut dropped: Option<usize> = None;
        let avail = ui.available_width() - 36.0;
        let w = (avail / n as f32).clamp(90.0, 200.0);
        let bar = ui.max_rect();
        ui.painter().hline(bar.x_range(), bar.bottom() - 0.5, Stroke::new(1.0, theme::BORDER));
        egui::ScrollArea::horizontal().id_salt("pestanas").auto_shrink([false, true]).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for i in 0..n {
                    let tab = self.tabs.list[i].clone();
                    let (glyph, title) = self.tab_label(&tab);
                    let active = i == self.tabs.active;
                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 31.0), Sense::click_and_drag());
                    rects.push(rect);
                    if resp.dragged() {
                        dragging = Some(i);
                    }
                    if resp.drag_stopped() {
                        dropped = Some(i);
                    }
                    let p = ui.painter();
                    let hovered = resp.hovered();
                    if active {
                        p.rect_filled(rect, egui::CornerRadius { nw: 7, ne: 7, sw: 0, se: 0 }, BG_EDITOR);
                        p.rect_stroke(rect.expand2(egui::vec2(0.0, 1.0)), egui::CornerRadius { nw: 7, ne: 7, sw: 0, se: 0 }, Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
                        p.hline(rect.x_range().shrink(1.0), rect.bottom(), Stroke::new(2.0, BG_EDITOR));
                        p.hline(rect.x_range().shrink(8.0), rect.top() + 1.0, Stroke::new(2.0, ACCENT));
                    } else if hovered {
                        p.rect_filled(rect.shrink2(egui::vec2(0.0, 2.0)), 6, HOVER);
                    }
                    let color = if active { TEXT } else { MUTED };
                    p.text(egui::pos2(rect.left() + 10.0, rect.center().y), Align2::LEFT_CENTER, glyph, FontId::proportional(14.0), color);
                    let mut job = LayoutJob::simple_singleline(title.clone(), FontId::proportional(13.0), color);
                    job.wrap = egui::text::TextWrapping { max_width: w - 52.0, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                    let g = p.layout_job(job);
                    p.galley(egui::pos2(rect.left() + 30.0, rect.center().y - g.size().y / 2.0), g, color);
                    // Cerrar (visible en la activa o al pasar el mouse).
                    let x_rect = egui::Rect::from_center_size(egui::pos2(rect.right() - 14.0, rect.center().y), egui::vec2(18.0, 18.0));
                    let over_x = ui.input(|inp| inp.pointer.hover_pos()).is_some_and(|pos| x_rect.contains(pos));
                    if active || hovered {
                        if over_x {
                            p.rect_filled(x_rect, 4, HOVER);
                        }
                        p.text(x_rect.center(), Align2::CENTER_CENTER, icon::X, FontId::proportional(12.0), MUTED);
                    }
                    let resp = resp.on_hover_text(match &tab {
                        Tab::Note(path) => self.rel(path),
                        Tab::View(_) => title.clone(),
                    });
                    if resp.clicked() {
                        todo = Some(if over_x { Do::Close(i) } else { Do::Activate(i) });
                    }
                    if resp.middle_clicked() {
                        todo = Some(Do::Close(i));
                    }
                    resp.context_menu(|ui| {
                        if ui.button("Cerrar pestaña").clicked() {
                            todo = Some(Do::Close(i));
                            ui.close();
                        }
                        if ui.button("Cerrar las demás").clicked() {
                            todo = Some(Do::CloseOthers(i));
                            ui.close();
                        }
                    });
                }
                // Dónde quedaría la pestaña arrastrada: una línea en ese lugar.
                let from = dragging.or(dropped);
                let target = from.and_then(|_| ui.ctx().pointer_interact_pos()).map(|p| rects.iter().position(|r| p.x < r.center().x).unwrap_or(n));
                if let (Some(i), Some(to)) = (dragging, target) {
                    if to != i && to != i + 1 {
                        let x = rects.get(to).map_or_else(|| rects[n - 1].right() + 1.0, |r| r.left() - 1.0);
                        ui.painter().vline(x, rects[0].y_range(), Stroke::new(2.0, ACCENT));
                    }
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                }
                if let (Some(i), Some(to)) = (dropped, target) {
                    todo = Some(Do::Move(i, to));
                }
                let ws = self.ws.clone();
                let plus = ui.menu_button(RichText::new(icon::PLUS).size(15.0).color(MUTED), |ui| {
                    if ui.button(format!("{}  Nota nueva en {ws}   Ctrl+N", icon::NOTE_PENCIL)).clicked() {
                        todo = Some(Do::NewNote);
                        ui.close();
                    }
                    if ui.button(format!("{}  Reunión nueva   Ctrl+R", icon::USERS)).clicked() {
                        todo = Some(Do::NewMeeting);
                        ui.close();
                    }
                    if ui.button(format!("{}  Reunión o nota que se repite…", icon::ARROWS_CLOCKWISE)).clicked() {
                        todo = Some(Do::Recurring);
                        ui.close();
                    }
                    if ui.button(format!("{}  Inicio   Ctrl+T", icon::HOUSE)).clicked() {
                        todo = Some(Do::New);
                        ui.close();
                    }
                    ui.separator();
                    let templates = self.templates();
                    if !templates.is_empty() {
                        ui.label(RichText::new(format!("Desde una plantilla, en {ws}")).size(12.0).color(MUTED));
                        for t in templates {
                            ui.horizontal(|ui| {
                                if ui.button(format!("{}  {}", icon::FILE_DASHED, vault::stem(&t))).clicked() {
                                    todo = Some(Do::FromTemplate(t.clone()));
                                    ui.close();
                                }
                                let edit = egui::Button::new(RichText::new(icon::PENCIL_SIMPLE).size(13.0).color(MUTED)).frame(false);
                                if ui.add(edit).on_hover_text("Editar la plantilla").clicked() {
                                    todo = Some(Do::EditTemplate(t.clone()));
                                    ui.close();
                                }
                            });
                        }
                    }
                    if ui.button(format!("{}  Plantilla nueva…", icon::FILE_PLUS)).on_hover_text("Una nota que sirve de punto de partida para otras (visita a obra, acta…)").clicked() {
                        todo = Some(Do::NewTemplate);
                        ui.close();
                    }
                });
                plus.response.on_hover_text("Nueva pestaña");
            });
        });
        match todo {
            Some(Do::Activate(i)) if i != self.tabs.active => self.activate_tab(i),
            Some(Do::Close(i)) => self.close_tab(i),
            Some(Do::CloseOthers(i)) => self.close_others(i),
            Some(Do::Move(from, to)) => {
                self.sync_tab();
                self.tabs.move_tab(from, to);
                self.save_estado();
            }
            Some(Do::New) => self.new_tab(Tab::View(View::Home)),
            Some(Do::NewNote) => {
                // Siempre en una pestaña nueva.
                self.sync_tab();
                self.save();
                let path = self.vault.unique_path(&self.ws, "Sin título");
                self.note = OpenNote::load(path.clone());
                self.new_tab(Tab::Note(path));
                self.search.clear();
                self.focus_title = true;
            }
            Some(Do::NewMeeting) => self.start_meeting(),
            Some(Do::Recurring) => self.open_recurring(),
            Some(Do::FromTemplate(t)) => self.new_from_template(&t),
            Some(Do::EditTemplate(t)) => self.open_in_tab(t, None),
            Some(Do::NewTemplate) => self.new_template(),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_move_and_keep_the_active_one() {
        let t = |s: &str| Tab::Note(PathBuf::from(s));
        let mut tabs = Tabs { list: vec![t("a"), t("b"), t("c"), t("d")], active: 1 };
        tabs.move_tab(0, 3); // «a» antes de «d»
        assert_eq!(tabs.list, vec![t("b"), t("c"), t("a"), t("d")]);
        assert_eq!(tabs.list[tabs.active], t("b"));
        tabs.move_tab(3, 0); // «d» al comienzo
        assert_eq!(tabs.list, vec![t("d"), t("b"), t("c"), t("a")]);
        tabs.move_tab(1, 4); // «b» al final
        assert_eq!(tabs.list, vec![t("d"), t("c"), t("a"), t("b")]);
        assert_eq!(tabs.list[tabs.active], t("b"));
        tabs.move_tab(2, 3); // al mismo lugar: nada
        assert_eq!(tabs.list, vec![t("d"), t("c"), t("a"), t("b")]);
    }

    #[test]
    fn tabs_round_trip() {
        let root = PathBuf::from("C:/Notas");
        for t in [
            Tab::Note(root.join("General").join("Notas generales.md")),
            Tab::View(View::Home),
            Tab::View(View::Ai),
            Tab::View(View::Tag("informe".into())),
        ] {
            let s = encode(&t, &root);
            let back = decode(&s, &root).unwrap();
            assert_eq!(encode(&back, &root), s, "{s}");
        }
        assert_eq!(encode(&Tab::Note(root.join("General").join("x.md")), &root), "nota:General/x");
        assert!(decode("otra cosa", &root).is_none());
        assert_eq!(decode("vista:preguntar", &root), Some(Tab::View(View::Ai)), "pestañas guardadas antes de la v0.15");
    }

    /// Abrir una nota que ya tiene pestaña va a esa pestaña; desde Inicio se abre en otra
    /// (Inicio no se transforma), y nunca quedan dos pestañas con lo mismo.
    #[test]
    fn notes_open_in_their_own_tab() {
        let dir = std::env::temp_dir().join(format!("nodex-tabs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        fs::create_dir_all(dir.join("General")).unwrap();
        let general = dir.join("General").join("Notas generales.md");
        let other = dir.join("General").join("Otra.md");
        fs::write(&general, "hola\n").unwrap();
        fs::write(&other, "otra\n").unwrap();
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        app.tabs = Tabs { list: vec![Tab::View(View::Home), Tab::Note(general.clone())], active: 1 };
        app.activate_tab(1);

        // Inicio, y de vuelta a Notas generales desde la barra lateral: se vuelve a su pestaña.
        app.apply(Action::ShowTab(View::Home));
        assert_eq!(app.tabs.active, 0);
        app.apply(Action::Open(general.clone(), None));
        app.sync_tab();
        assert_eq!(app.tabs.list, vec![Tab::View(View::Home), Tab::Note(general.clone())]);
        assert_eq!((app.tabs.active, app.view.clone()), (1, View::Editor));

        // Desde Inicio, otra nota: pestaña nueva; Inicio sigue ahí.
        app.apply(Action::ShowTab(View::Home));
        app.apply(Action::Open(other.clone(), None));
        app.sync_tab();
        assert_eq!(app.tabs.list, vec![Tab::View(View::Home), Tab::Note(other.clone()), Tab::Note(general.clone())]);
        assert_eq!(app.tabs.active, 1);

        // Si igual quedaran repetidas, se juntan en la activa.
        app.tabs.list = vec![Tab::Note(general.clone()), Tab::View(View::Home), Tab::Note(general.clone()), Tab::Note(general.clone())];
        app.tabs.active = 2;
        app.note = OpenNote::load(general.clone());
        app.view = View::Editor;
        app.sync_tab();
        assert_eq!(app.tabs.list, vec![Tab::View(View::Home), Tab::Note(general.clone())]);
        assert_eq!(app.tabs.active, 1);

        // La ventana de la IA se abre en la sección pedida, en su propia pestaña.
        app.apply(Action::ShowAi(super::super::ai_view::AiTab::Log));
        app.sync_tab();
        assert_eq!(app.tabs.list.iter().filter(|t| **t == Tab::View(View::Ai)).count(), 1);
        assert_eq!((app.view.clone(), app.ai_tab), (View::Ai, super::super::ai_view::AiTab::Log));
        let _ = fs::remove_dir_all(&dir);
    }
}

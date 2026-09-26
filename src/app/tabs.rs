//! Pestañas, como en un navegador: cada una muestra una nota o una vista (Inicio, Hoy,
//! Preguntar…). La pestaña activa sigue lo que se está viendo; cambiar de pestaña vuelve
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
                View::Today => "hoy".into(),
                View::Ask => "preguntar".into(),
                View::Week => "semana".into(),
                View::Tasks => "tareas".into(),
                View::Agenda => "agenda".into(),
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
        "hoy" => View::Today,
        "preguntar" => View::Ask,
        "semana" => View::Week,
        "tareas" => View::Tasks,
        "agenda" => View::Agenda,
        t => View::Tag(t.strip_prefix('#')?.to_string()),
    };
    Some(Tab::View(v))
}

fn view_label(v: &View) -> (&'static str, String) {
    match v {
        View::Home | View::Editor => (icon::HOUSE, "Inicio".into()),
        View::Today => (icon::TRAY, "Hoy".into()),
        View::Ask => (icon::CHAT_CIRCLE_TEXT, "Preguntar".into()),
        View::Week => (icon::CALENDAR_CHECK, "Semana".into()),
        View::Tasks => (icon::CHECK_SQUARE, "Tareas".into()),
        View::Agenda => (icon::CALENDAR_BLANK, "Agenda".into()),
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
            Some(t) if *t != cur => *t = cur,
            Some(_) => {}
            None => self.tabs.list.push(cur),
        }
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
                self.ask.focus = v == View::Ask;
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
                let meeting = self.vault.get(p).is_some_and(|n| is_meeting(&n.text));
                let title = if *p == self.note.path { self.note.title.clone() } else { vault::stem(p) };
                (if meeting { icon::USERS } else { icon::FILE_TEXT }, title)
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
            New,
        }
        let mut todo = None;
        let n = self.tabs.list.len();
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
                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 31.0), Sense::click());
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
                let plus = egui::Button::new(RichText::new(icon::PLUS).size(15.0).color(MUTED)).frame(false).min_size(egui::vec2(28.0, 28.0));
                if ui.add(plus).on_hover_text("Nueva pestaña (Ctrl+T)").clicked() {
                    todo = Some(Do::New);
                }
            });
        });
        match todo {
            Some(Do::Activate(i)) if i != self.tabs.active => self.activate_tab(i),
            Some(Do::Close(i)) => self.close_tab(i),
            Some(Do::CloseOthers(i)) => self.close_others(i),
            Some(Do::New) => self.new_tab(Tab::View(View::Home)),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_round_trip() {
        let root = PathBuf::from("C:/Notas");
        for t in [
            Tab::Note(root.join("General").join("Notas generales.md")),
            Tab::View(View::Home),
            Tab::View(View::Ask),
            Tab::View(View::Tag("informe".into())),
        ] {
            let s = encode(&t, &root);
            let back = decode(&s, &root).unwrap();
            assert_eq!(encode(&back, &root), s, "{s}");
        }
        assert_eq!(encode(&Tab::Note(root.join("General").join("x.md")), &root), "nota:General/x");
        assert!(decode("otra cosa", &root).is_none());
    }
}

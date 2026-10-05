//! «Notas»: todas las notas de todos los espacios en un solo lugar, la más reciente primero,
//! con buscador y filtro por espacio. Un clic la abre; clic derecho, lo mismo que en la lista.

use super::*;

/// Cuántas se muestran de una vez (las demás, buscando).
const SHOW: usize = 300;

impl NotesApp {
    pub(super) fn notes_view(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        let spaces = self.vault.workspaces.clone();
        let want = vault::fold(self.notes_filter.trim());
        let only = self.notes_space.clone();
        let notes: Vec<(PathBuf, String, String, Option<SystemTime>, String)> = self
            .vault
            .all_notes()
            .into_iter()
            .filter(|n| only.as_ref().is_none_or(|w| n.workspace == *w))
            .filter(|n| want.is_empty() || vault::fold(&n.title).contains(&want) || n.folded().contains(&want))
            .map(|n| {
                let preview = tracking::preview(&n.text, 2, 150);
                let ws = if n.workspace == vault::DIARY { vault::DIARY.to_string() } else { n.workspace.clone() };
                (n.path.clone(), display_title(&n.title), ws, Some(n.modified), preview)
            })
            .collect();
        let total = self.vault.all_notes().len();
        Self::column(ui, "todas-las-notas", |ui, _| {
            view_header(ui, "Notas", &format!("{} en {}, la más reciente primero", plural(total, "nota"), plural(spaces.len(), "espacio")));
            ui.add(
                egui::TextEdit::singleline(&mut self.notes_filter)
                    .hint_text(format!("{}  Buscar en las notas", icon::MAGNIFYING_GLASS))
                    .desired_width(f32::INFINITY)
                    .margin(Margin::symmetric(8, 5)),
            );
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                if ui.selectable_label(self.notes_space.is_none(), RichText::new("Todas").size(12.5)).clicked() {
                    self.notes_space = None;
                }
                for w in spaces.iter().map(String::as_str).chain([vault::DIARY]) {
                    let on = self.notes_space.as_deref() == Some(w);
                    if ui.selectable_label(on, RichText::new(format!("{} {w}", icon::FOLDER_SIMPLE)).size(12.5)).clicked() {
                        self.notes_space = if on { None } else { Some(w.to_string()) };
                    }
                }
            });
            ui.add_space(12.0);
            if notes.is_empty() {
                ui.label(RichText::new("Ninguna nota tiene eso.").color(MUTED));
            }
            for (path, title, ws, modified, preview) in notes.iter().take(SHOW) {
                let when = modified.map(|m| long_date(&chrono::DateTime::<Local>::from(m).format("%Y-%m-%d").to_string())).unwrap_or_default();
                let r = Frame::new()
                    .stroke(Stroke::new(1.0, theme::BORDER))
                    .corner_radius(10)
                    .inner_margin(Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let mut job = LayoutJob::default();
                        job.append(&format!("{}  ", icon::FILE_TEXT), 0.0, fmt(FontId::proportional(14.0), MUTED));
                        job.append(title, 0.0, fmt(theme::bold(14.5), TEXT));
                        job.append(&format!("   {ws} · {when}"), 0.0, fmt(FontId::proportional(12.5), MUTED));
                        ui.label(job);
                        if !preview.is_empty() {
                            ui.label(RichText::new(preview).size(12.5).color(MUTED));
                        }
                    })
                    .response
                    .interact(Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if r.clicked() {
                    action = Some(Action::Open(path.clone(), None));
                }
                r.context_menu(|ui| {
                    if ui.button(format!("{}  Abrir en otra pestaña", icon::PLUS)).clicked() {
                        action = Some(Action::OpenNewTab(path.clone()));
                        ui.close();
                    }
                    if ui.button(format!("{}  Archivar", icon::ARCHIVE)).clicked() {
                        action = Some(Action::Archive(path.clone()));
                        ui.close();
                    }
                    if ui.button(format!("{}  Mover a la papelera", icon::TRASH)).clicked() {
                        action = Some(Action::Trash(path.clone()));
                        ui.close();
                    }
                });
                ui.add_space(6.0);
            }
            if notes.len() > SHOW {
                ui.label(RichText::new(format!("y {} más: busca para encontrarlas", notes.len() - SHOW)).size(12.5).color(MUTED));
            }
        });
        action
    }
}

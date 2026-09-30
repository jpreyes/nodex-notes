//! La papelera: las notas y espacios borrados, para restaurarlos con un clic o borrarlos para
//! siempre (con una confirmación explícita, a propósito).

use super::*;
use crate::vault::Trashed;

/// El día de una nota del día en la papelera ("2026-09-30" o "2026-09-30 2", si se repitió el nombre).
fn day_of(name: &str) -> Option<&str> {
    let base = match name.rsplit_once(' ') {
        Some((b, n)) if n.chars().all(|c| c.is_ascii_digit()) => b,
        _ => name,
    };
    agenda::is_date(base).then_some(base)
}

/// Qué se quiere borrar para siempre (se confirma en una ventana).
pub(super) enum Forever {
    One(Trashed),
    All,
}

impl NotesApp {
    pub(super) fn trash_view(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        let items = self.vault.trashed();
        Self::column(ui, "papelera", |ui, _| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{} Papelera", icon::TRASH)).font(theme::bold(24.0)));
                if !items.is_empty() {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let b = egui::Button::new(RichText::new(format!("{} Vaciar la papelera", icon::TRASH)).color(RED)).stroke(Stroke::new(1.0, RED));
                        if ui.add(b).on_hover_text("Borra para siempre todo lo que hay aquí").clicked() {
                            self.forever = Some((Forever::All, false));
                        }
                    });
                }
            });
            ui.add_space(4.0);
            ui.label(
                RichText::new("Lo que borras queda aquí (en la carpeta .papelera). «Restaurar» lo devuelve a su lugar; «Borrar para siempre» no se puede deshacer.")
                    .size(13.0)
                    .color(MUTED),
            );
            ui.add_space(14.0);
            if items.is_empty() {
                ui.label(RichText::new("La papelera está vacía.").color(MUTED));
                return;
            }
            for t in &items {
                Frame::new().stroke(Stroke::new(1.0, theme::BORDER)).corner_radius(10).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let glyph = if t.is_dir { icon::FOLDER_SIMPLE } else { icon::FILE_TEXT };
                        let stem = vault::stem(&t.path);
                        let title = match day_of(&stem) {
                            _ if t.is_dir => t.name.clone(),
                            Some(d) => format!("Nota del día · {}", long_date(d)),
                            None => stem.clone(),
                        };
                        ui.label(RichText::new(format!("{glyph}  {title}")).font(theme::bold(14.5)));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let b = egui::Button::new(RichText::new("Borrar para siempre").size(12.5).color(RED));
                            if ui.add(b).clicked() {
                                self.forever = Some((Forever::One(t.clone()), false));
                            }
                            if ui.button(RichText::new(format!("{} Restaurar", icon::ARROW_COUNTER_CLOCKWISE)).size(12.5)).clicked() {
                                action = Some(Action::Restore(t.clone()));
                            }
                        });
                    });
                    let from = match (&t.from, t.is_dir) {
                        (_, true) => "Un espacio con sus notas".to_string(),
                        (Some(f), false) => match f.rsplit_once('/') {
                            Some((dir, _)) if vault::is_diary_dir(dir) => "Era una nota del Diario".to_string(),
                            Some((dir, _)) => format!("Estaba en {dir}"),
                            None => "Estaba en la carpeta de notas".to_string(),
                        },
                        (None, false) if day_of(&vault::stem(&t.path)).is_some() => "Vuelve al Diario".to_string(),
                        (None, false) => "No se sabe de qué espacio era: vuelve al espacio actual".to_string(),
                    };
                    let when = if t.when.is_empty() { String::new() } else { format!(" · borrada el {}", long_date(&t.when[..10.min(t.when.len())])) };
                    ui.label(RichText::new(format!("{from}{when}")).size(12.5).color(MUTED));
                    // El comienzo de la nota, para reconocerla.
                    if !t.is_dir && t.path.extension().is_some_and(|e| e.eq_ignore_ascii_case("md")) {
                        let text = vault::read_text(&t.path).unwrap_or_default();
                        let preview: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).take(2).collect();
                        if !preview.is_empty() {
                            let p: String = preview.join(" · ").chars().take(160).collect();
                            ui.label(RichText::new(p).size(12.5).color(TEXT));
                        }
                    }
                });
                ui.add_space(8.0);
            }
        });
        action
    }

    /// Devuelve algo de la papelera a su lugar y lo abre.
    pub(super) fn restore(&mut self, t: Trashed) {
        let ws = self.ws.clone();
        match self.vault.restore(&t, &ws) {
            Ok(to) => {
                let place = if t.is_dir { "como espacio".to_string() } else { format!("en {}", self.rel(to.parent().unwrap_or(&to))) };
                self.msg(format!("«{}» restaurada {place}", if t.is_dir { t.name.clone() } else { vault::stem(&to) }));
                if !t.is_dir && self.vault.get(&to).is_some() {
                    self.open_in_tab(to, None);
                }
            }
            Err(e) => self.msg(format!("No se pudo restaurar: {e}")),
        }
    }

    /// Confirmación para borrar para siempre: hay que marcar que se entiende que no se recupera.
    pub(super) fn forever_window(&mut self, ctx: &egui::Context) {
        let Some((what, sure)) = self.forever.as_mut() else { return };
        let (title, detail) = match what {
            Forever::One(t) => (format!("¿Borrar para siempre «{}»?", if t.is_dir { t.name.clone() } else { vault::stem(&t.path) }), if t.is_dir { "Se borra el espacio con todas sus notas.".to_string() } else { String::new() }),
            Forever::All => ("¿Vaciar la papelera?".to_string(), format!("Se borra para siempre todo lo que hay en ella ({}).", plural(self.vault.trashed().len(), "elemento"))),
        };
        let mut go = false;
        let mut close = false;
        let modal = egui::Modal::new(Id::new("borrar-para-siempre")).show(ctx, |ui| {
            ui.set_width(400.0);
            ui.label(RichText::new(title).font(theme::bold(16.0)));
            ui.add_space(4.0);
            ui.label(RichText::new(format!("{detail} No se puede deshacer, ni desde la app ni desde la carpeta.").trim()).size(13.0).color(MUTED));
            ui.add_space(8.0);
            ui.checkbox(sure, "Entiendo que no se puede recuperar");
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let b = egui::Button::new(RichText::new(format!("{} Borrar para siempre", icon::TRASH)).color(Color32::WHITE)).fill(RED);
                if ui.add_enabled(*sure, b).clicked() {
                    go = true;
                }
                if ui.button("Cancelar").clicked() {
                    close = true;
                }
            });
        });
        if modal.should_close() {
            close = true;
        }
        if go {
            let Some((what, _)) = self.forever.take() else { return };
            let items = match what {
                Forever::One(t) => vec![t],
                Forever::All => self.vault.trashed(),
            };
            let mut gone = 0;
            for t in &items {
                match self.vault.delete_forever(t) {
                    Ok(()) => gone += 1,
                    Err(e) => self.msg(format!("No se pudo borrar «{}»: {e}", t.name)),
                }
            }
            if gone > 0 {
                self.msg(format!("{} borrado para siempre", plural(gone, "elemento")));
            }
        } else if close {
            self.forever = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lo borrado vuelve a su lugar, aunque su espacio ya no exista; y se borra para siempre
    /// solo lo que está en la papelera.
    #[test]
    fn restore_and_delete_forever() {
        let dir = std::env::temp_dir().join(format!("nodex-vista-papelera-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        for ws in ["Obra", "General"] {
            fs::create_dir_all(dir.join(ws)).unwrap();
        }
        let muro = dir.join("Obra").join("Muro.md");
        fs::write(&muro, "Revisar el muro\n").unwrap();
        fs::write(dir.join("General").join("Otra.md"), "Hola\n").unwrap();
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());

        // Una nota y después su espacio completo.
        app.apply(Action::Trash(muro.clone()));
        fs::write(dir.join("Obra").join("Losa.md"), "Losa\n").unwrap();
        app.vault.scan();
        app.trash_workspace("Obra".into());
        let t = app.vault.trashed();
        assert_eq!(t.len(), 2);
        let note = t.iter().find(|t| !t.is_dir).unwrap().clone();
        assert_eq!(note.from.as_deref(), Some("Obra/Muro.md"));

        // La nota vuelve a su lugar (se vuelve a crear la carpeta del espacio).
        app.apply(Action::Restore(note));
        assert_eq!(fs::read_to_string(&muro).unwrap(), "Revisar el muro\n");
        assert_eq!(app.note.path, muro);
        // El espacio vuelve con otro nombre, porque ya hay un «Obra».
        let space = app.vault.trashed().into_iter().find(|t| t.is_dir).unwrap();
        app.apply(Action::Restore(space));
        assert!(dir.join("Obra 2").join("Losa.md").exists());
        assert!(app.vault.trashed().is_empty());

        // Borrar para siempre.
        app.apply(Action::Trash(muro.clone()));
        let t = app.vault.trashed().remove(0);
        app.vault.delete_forever(&t).unwrap();
        assert!(app.vault.trashed().is_empty() && !t.path.exists());
        // Nada fuera de la papelera.
        let outside = Trashed { path: dir.join("General").join("Otra.md"), name: "Otra.md".into(), from: None, when: String::new(), is_dir: false };
        assert!(app.vault.delete_forever(&outside).is_err());
        assert!(dir.join("General").join("Otra.md").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}

//! Anterior y siguiente, como en un navegador: ← vuelve a lo que se estaba viendo (una nota o
//! una vista) y → avanza otra vez. También con Alt+← / Alt+→ y los botones laterales del mouse.

use super::*;
use tabs::Tab;

/// Cuánto se recuerda hacia atrás.
const KEEP: usize = 100;

#[derive(Default)]
pub(super) struct History {
    back: Vec<Tab>,
    forward: Vec<Tab>,
    last: Option<Tab>,
    /// Se está yendo atrás o adelante: no cuenta como un paso nuevo.
    jumping: bool,
}

impl NotesApp {
    /// Anota lo que se está viendo (se llama una vez por cuadro).
    pub(super) fn track_history(&mut self) {
        let cur = self.current_tab();
        let h = &mut self.nav;
        if h.last.as_ref() == Some(&cur) {
            return;
        }
        if let Some(prev) = h.last.take() {
            if !h.jumping {
                h.back.push(prev);
                if h.back.len() > KEEP {
                    h.back.remove(0);
                }
                h.forward.clear();
            }
        }
        h.jumping = false;
        h.last = Some(cur);
    }

    fn go_to(&mut self, tab: Tab) {
        self.nav.jumping = true;
        match tab {
            Tab::Note(p) if p.is_file() => self.open_in_tab(p, None),
            Tab::Note(_) => self.nav.jumping = false,
            Tab::View(v) => self.show_in_tab(v),
        }
    }

    pub(super) fn go_back(&mut self) {
        // Lo que ya no existe (una nota borrada) se salta.
        while let Some(t) = self.nav.back.pop() {
            if matches!(&t, Tab::Note(p) if !p.is_file()) {
                continue;
            }
            if let Some(cur) = self.nav.last.clone() {
                self.nav.forward.push(cur);
            }
            self.go_to(t);
            return;
        }
    }

    pub(super) fn go_forward(&mut self) {
        while let Some(t) = self.nav.forward.pop() {
            if matches!(&t, Tab::Note(p) if !p.is_file()) {
                continue;
            }
            if let Some(cur) = self.nav.last.clone() {
                self.nav.back.push(cur);
            }
            self.go_to(t);
            return;
        }
    }

    /// Las flechas, a la izquierda de las pestañas; y los atajos.
    pub(super) fn nav_arrows(&mut self, ui: &mut Ui) {
        let (alt_left, alt_right, mouse_back, mouse_fwd) = ui.input(|i| {
            (
                i.modifiers.alt && i.key_pressed(Key::ArrowLeft),
                i.modifiers.alt && i.key_pressed(Key::ArrowRight),
                i.pointer.button_pressed(egui::PointerButton::Extra1),
                i.pointer.button_pressed(egui::PointerButton::Extra2),
            )
        });
        let can_back = !self.nav.back.is_empty();
        let can_fwd = !self.nav.forward.is_empty();
        let arrow = |ui: &mut Ui, glyph: &str, on: bool, tip: &str| {
            let color = if on { TEXT() } else { theme::BORDER() };
            ui.add_enabled(on, egui::Button::new(RichText::new(glyph).size(15.0).color(color)).frame(false).min_size(egui::vec2(24.0, 28.0)))
                .on_hover_text(tip)
                .clicked()
        };
        let back = arrow(ui, icon::ARROW_LEFT, can_back, "Anterior (Alt+←)");
        let fwd = arrow(ui, icon::ARROW_RIGHT, can_fwd, "Siguiente (Alt+→)");
        if (back || alt_left || mouse_back) && can_back {
            self.go_back();
        } else if (fwd || alt_right || mouse_fwd) && can_fwd {
            self.go_forward();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Atrás y adelante recorren lo que se vio, como un navegador.
    #[test]
    fn back_and_forward() {
        let dir = std::env::temp_dir().join(format!("nodex-nav-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        fs::create_dir_all(dir.join("Obra")).unwrap();
        let (a, b) = (dir.join("Obra").join("A.md"), dir.join("Obra").join("B.md"));
        fs::write(&a, "Nota A\n").unwrap();
        fs::write(&b, "Nota B\n").unwrap();
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        app.track_history();
        app.open_in_tab(a.clone(), None);
        app.track_history();
        app.show_in_tab(View::Tasks);
        app.track_history();
        app.open_in_tab(b.clone(), None);
        app.track_history();

        app.go_back();
        app.track_history();
        assert_eq!(app.view, View::Tasks);
        app.go_back();
        app.track_history();
        assert_eq!((app.view.clone(), app.note.path.clone()), (View::Editor, a.clone()));
        app.go_forward();
        app.track_history();
        assert_eq!(app.view, View::Tasks);
        // Ir a otra parte borra lo que había adelante.
        app.show_in_tab(View::Agenda);
        app.track_history();
        assert!(app.nav.forward.is_empty());
        app.go_back();
        app.track_history();
        assert_eq!(app.view, View::Tasks);
        let _ = fs::remove_dir_all(&dir);
    }
}

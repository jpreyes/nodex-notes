//! «Guardar una página web»: desde el + de las pestañas, o con clic derecho en una línea que
//! tiene una dirección. La página se baja aparte y queda como nota nueva en el espacio actual,
//! con su fuente arriba. No pasa por la IA (es lectura guardada, no algo que organizar).

use super::*;
use std::sync::mpsc;

pub(super) struct WebClip {
    url: String,
    rx: Option<mpsc::Receiver<Result<crate::webclip::Clip, String>>>,
    error: Option<String>,
}

impl NotesApp {
    pub(super) fn open_web_clip(&mut self, url: Option<String>) {
        self.web_clip = Some(WebClip { url: url.unwrap_or_default(), rx: None, error: None });
    }

    fn start_web_clip(&mut self) {
        let Some(c) = self.web_clip.as_mut() else { return };
        let Some(url) = crate::webclip::normalize(&c.url) else {
            c.error = Some(t!("Eso no parece una dirección web").into());
            return;
        };
        c.url = url.clone();
        c.error = None;
        let (tx, rx) = mpsc::channel();
        c.rx = Some(rx);
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let r = crate::webclip::fetch(&url).and_then(|html| crate::webclip::clip(&html, &url));
            let _ = tx.send(r);
            ctx.request_repaint();
        });
    }

    /// Crea la nota con la página; devuelve dónde quedó.
    pub(super) fn save_clip(&mut self, url: &str, clip: &crate::webclip::Clip) -> Option<PathBuf> {
        let ws = if self.vault.workspaces.contains(&self.ws) { self.ws.clone() } else { self.home_ws() };
        let path = self.vault.unique_path(&ws, &clip.title);
        let source = tf!("Fuente: [{title}]({url}) · guardada el {date}", title = clip.title.replace(['[', ']'], ""), url = url, date = long_date(&today()));
        let body = format!("{source}\n\n{}", clip.body);
        if let Err(e) = fs::create_dir_all(self.vault.root.join(&ws)).and_then(|_| fs::write(&path, &body)) {
            self.msg(tf!("No se pudo crear la nota: {e}", e = e));
            return None;
        }
        // Es lectura guardada: la IA no la reorganiza.
        self.analyzed.insert(ai::fnv(&body));
        self.save_analyzed();
        self.vault.upsert(path.clone(), body, vault::modified(&path).unwrap_or_else(SystemTime::now));
        Some(path)
    }

    pub(super) fn web_clip_window(&mut self, ctx: &egui::Context) {
        let Some(c) = self.web_clip.as_mut() else { return };
        let mut done = None;
        if let Some(rx) = &c.rx {
            if let Ok(r) = rx.try_recv() {
                c.rx = None;
                match r {
                    Ok(clip) => done = Some((c.url.clone(), clip)),
                    Err(e) => c.error = Some(tf!("No se pudo guardar la página: {e}", e = e)),
                }
            }
        }
        let busy = c.rx.is_some();
        let (mut go, mut close) = (false, false);
        let modal = egui::Modal::new(Id::new("pagina-web")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(RichText::new(format!("{} {}", icon::GLOBE, t!("Guardar una página web"))).font(theme::bold(17.0)));
            ui.add_space(4.0);
            ui.label(RichText::new(t!("Queda como nota en este espacio: solo el artículo, sin menús ni avisos, con sus imágenes y la dirección de dónde salió.")).size(12.5).color(MUTED()));
            ui.add_space(10.0);
            let field = ui.add_enabled(
                !busy,
                egui::TextEdit::singleline(&mut c.url).hint_text("https://…").desired_width(f32::INFINITY).margin(Margin::symmetric(8, 6)),
            );
            if c.url.is_empty() && !busy {
                field.request_focus();
            }
            if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                go = true;
            }
            if let Some(e) = &c.error {
                ui.add_space(6.0);
                ui.label(RichText::new(e).size(12.5).color(RED()));
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if busy {
                    ui.spinner();
                    ui.label(RichText::new(t!("Bajando la página…")).size(13.0).color(MUTED()));
                } else if ui.add(egui::Button::new(RichText::new(t!("Guardar")).color(theme::c(Color32::WHITE))).fill(ACCENT())).clicked() {
                    go = true;
                }
                if ui.button(t!("Cancelar")).clicked() {
                    close = true;
                }
            });
        });
        if modal.should_close() {
            close = true;
        }
        if go && !busy {
            self.start_web_clip();
        }
        if let Some((url, clip)) = done {
            self.web_clip = None;
            if let Some(path) = self.save_clip(&url, &clip) {
                self.open_in_tab(path, None);
                self.msg(tf!("Página guardada: «{title}»", title = clip.title));
            }
        } else if close {
            self.web_clip = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_a_page_as_a_note() {
        let dir = std::env::temp_dir().join(format!("nodex-web-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        fs::create_dir_all(dir.join("Obra")).unwrap();
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        app.ws = "Obra".into();
        let clip = crate::webclip::Clip { title: "Curado: guía / 2026".into(), body: "El curado mantiene la humedad.\n".into() };
        let path = app.save_clip("https://obra.cl/curado", &clip).unwrap();
        assert!(path.starts_with(dir.join("Obra")));
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("Fuente: [Curado: guía / 2026](https://obra.cl/curado) · guardada el "), "{text}");
        assert!(text.ends_with("El curado mantiene la humedad.\n"));
        assert!(app.analyzed.contains(&ai::fnv(&text)), "la IA no la reorganiza");
        // Otra vez la misma: otra nota, sin pisar la primera.
        let again = app.save_clip("https://obra.cl/curado", &clip).unwrap();
        assert_ne!(again, path);
        let _ = fs::remove_dir_all(&dir);
    }
}

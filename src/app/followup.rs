//! Correo de seguimiento de una reunión: la IA redacta un borrador con el resumen, las
//! decisiones y los acuerdos; se puede editar, copiar o abrir en el programa de correo.

use super::*;
use std::sync::mpsc::{self, Receiver};

#[derive(Default)]
pub(super) struct FollowUp {
    pub(super) open: bool,
    title: String,
    text: String,
    error: Option<String>,
    rx: Option<Receiver<Result<String, String>>>,
}

const SYSTEM: &str = "Redactas correos de seguimiento de reuniones, en español, cordiales, claros y breves. Usa solo lo que está en la nota; no inventes nombres, fechas ni acuerdos. Formato: la primera línea es \"Asunto: …\", después una línea en blanco, un saludo, un resumen de 2 o 3 frases, las decisiones (lista con guiones), los acuerdos con responsable y fecha (lista con guiones), los próximos pasos si hay, y termina con \"Saludos,\". Texto plano, sin asteriscos ni otros símbolos de formato.";

/// Codifica un texto para un enlace "mailto:".
fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out += &format!("%{b:02X}"),
        }
    }
    out
}

/// Separa "Asunto: …" del cuerpo.
fn split_subject(text: &str) -> (String, String) {
    let t = text.trim();
    match t.split_once('\n') {
        Some((first, rest)) if first.trim().to_lowercase().starts_with("asunto:") => {
            (first.trim()["asunto:".len()..].trim().to_string(), rest.trim_start_matches(['\r', '\n']).to_string())
        }
        _ => (String::new(), t.to_string()),
    }
}

impl NotesApp {
    /// Pide a la IA el borrador del correo para la nota abierta.
    pub(super) fn start_followup(&mut self) {
        self.save();
        let title = self.note.title.clone();
        let user = format!("Nota de la reunión «{title}»:\n<<<\n{}\n>>>", self.note.text);
        let cfg = self.cfg.clone();
        let ctx = self.ctx.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(ai::complete(&cfg, SYSTEM, &user));
            ctx.request_repaint();
        });
        self.followup = FollowUp { open: true, title, text: String::new(), error: None, rx: Some(rx) };
    }

    /// La ventana del borrador (si está abierta).
    pub(super) fn followup_window(&mut self, ctx: &egui::Context) {
        if !self.followup.open {
            return;
        }
        if let Some(r) = self.followup.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            match r {
                Ok(t) => self.followup.text = t,
                Err(e) => self.followup.error = Some(e),
            }
            self.followup.rx = None;
        }
        let mut close = false;
        let mut retry = false;
        let modal = egui::Modal::new(Id::new("seguimiento")).show(ctx, |ui| {
            ui.set_width(560.0_f32.min(ctx.content_rect().width() - 80.0));
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{} Correo de seguimiento", icon::ENVELOPE_SIMPLE)).font(theme::bold(17.0)));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(egui::Button::new(RichText::new(icon::X).size(16.0)).frame(false)).on_hover_text("Cerrar (Esc)").clicked() {
                        close = true;
                    }
                });
            });
            ui.label(RichText::new(format!("Borrador para «{}». Puedes editarlo antes de enviarlo.", self.followup.title)).size(12.5).color(MUTED));
            ui.add_space(8.0);
            if self.followup.rx.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Redactando…").color(MUTED));
                });
                return;
            }
            if let Some(e) = &self.followup.error {
                ui.label(RichText::new(format!("No se pudo redactar: {e}")).color(RED));
                if ui.button("Intentar de nuevo").clicked() {
                    retry = true;
                }
                return;
            }
            egui::ScrollArea::vertical().max_height(ctx.content_rect().height() * 0.55).show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(&mut self.followup.text).desired_width(f32::INFINITY).desired_rows(14).font(FontId::proportional(14.0)));
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(format!("{} Copiar", icon::COPY)).clicked() {
                    ui.ctx().copy_text(self.followup.text.clone());
                    self.msg("Correo copiado");
                }
                if ui.button(format!("{} Abrir en el correo", icon::PAPER_PLANE_RIGHT)).on_hover_text("Abre tu programa de correo con el asunto y el texto").clicked() {
                    let (subject, body) = split_subject(&self.followup.text);
                    let url = format!("mailto:?subject={}&body={}", url_encode(&subject), url_encode(&body.replace('\n', "\r\n")));
                    gcal::open_browser(&url);
                }
                if ui.button(format!("{} Rehacer", icon::ARROW_CLOCKWISE)).clicked() {
                    retry = true;
                }
            });
        });
        if close || modal.should_close() {
            self.followup.open = false;
        }
        if retry {
            self.start_followup();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subject_and_mailto_encoding() {
        let (s, b) = split_subject("Asunto: Reunión CIC\n\nHola a todos,\nSaludos,");
        assert_eq!((s.as_str(), b.as_str()), ("Reunión CIC", "Hola a todos,\nSaludos,"));
        assert_eq!(split_subject("Hola").0, "");
        assert_eq!(url_encode("a b&ñ"), "a%20b%26%C3%B1");
    }
}

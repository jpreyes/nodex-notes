//! La ventana «Historial» de una nota: sus versiones anteriores, qué cambió desde cada una,
//! recuperar una línea o restaurar la versión entera (la de ahora queda en el historial).

use super::*;
use crate::history::{self, Version};

/// Una línea al comparar una versión con la nota de ahora.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Diff {
    Same(String),
    /// Estaba en la versión y ya no; `after` = la línea de ahora tras la que iba.
    Gone { line: String, after: Option<usize> },
    /// Está ahora y no estaba en la versión.
    Added(String),
}

/// Qué cambió de `old` a `now`, línea por línea.
pub(super) fn diff(old: &str, now: &str) -> Vec<Diff> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = now.lines().collect();
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    let mut last_b: Option<usize> = None;
    for (pi, pj) in crate::merge::common(&a, &b).into_iter().chain(std::iter::once((a.len(), b.len()))) {
        while i < pi {
            out.push(Diff::Gone { line: a[i].to_string(), after: last_b });
            i += 1;
        }
        while j < pj {
            out.push(Diff::Added(b[j].to_string()));
            last_b = Some(j);
            j += 1;
        }
        if pi < a.len() {
            out.push(Diff::Same(a[pi].to_string()));
            last_b = Some(pj);
            i = pi + 1;
            j = pj + 1;
        }
    }
    out
}

/// `text` con `line` agregada después de la línea `after` (o al comienzo).
pub(super) fn insert_line(text: &str, after: Option<usize>, line: &str) -> String {
    let mut ls: Vec<&str> = text.split('\n').collect();
    let at = after.map_or(0, |a| (a + 1).min(ls.len()));
    ls.insert(at, line);
    let mut out = ls.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// «Hoy», «Ayer» o «mar 29 sep».
fn day_label(when: DateTime<Local>) -> String {
    let today = Local::now().date_naive();
    let d = when.date_naive();
    match (today - d).num_days() {
        0 => "Hoy".into(),
        1 => "Ayer".into(),
        _ => {
            let year = if d.year() != today.year() { format!(" {}", d.year()) } else { String::new() };
            format!("{} {} {}{year}", DIAS_CORTOS[d.weekday().num_days_from_monday() as usize], d.day(), MESES[d.month0() as usize])
        }
    }
}

pub(super) struct HistoryView {
    path: PathBuf,
    versions: Vec<Version>,
    sel: usize,
    /// El texto de la versión elegida y lo que cambió (para qué versión y qué texto de ahora).
    text: String,
    diff: Vec<Diff>,
    diff_of: Option<(usize, u64)>,
    /// Ver el texto completo de la versión en vez de los cambios.
    full: bool,
}

enum Do {
    Select(usize),
    Restore,
    Recover(String, Option<usize>),
    Full(bool),
    Close,
}

impl NotesApp {
    /// Abre el historial de la nota abierta.
    pub(super) fn open_history(&mut self) {
        self.save();
        let rel = history::rel_of(&self.vault.root, &self.note.path);
        let versions = history::list(&self.vault.root, &rel);
        self.history = Some(HistoryView { path: self.note.path.clone(), versions, sel: 0, text: String::new(), diff: Vec::new(), diff_of: None, full: false });
    }

    pub(super) fn history_window(&mut self, ctx: &egui::Context) {
        let Some(mut h) = self.history.take() else { return };
        if h.path != self.note.path {
            return; // se abrió otra nota
        }
        let now_hash = ai::fnv(&self.note.text);
        if !h.versions.is_empty() && h.diff_of != Some((h.sel, now_hash)) {
            h.text = history::read(&h.versions[h.sel]);
            h.diff = diff(&h.text, &self.note.text);
            h.diff_of = Some((h.sel, now_hash));
        }
        let mut todo: Option<Do> = None;
        let screen = ctx.content_rect();
        let w = (screen.width() - 80.0).clamp(560.0, 920.0);
        let hgt = (screen.height() - 80.0).clamp(380.0, 600.0);
        let modal = egui::Modal::new(Id::new("historial"))
            .frame(Frame::new().fill(theme::c(Color32::WHITE)).corner_radius(12).stroke(Stroke::new(1.0, theme::BORDER())).inner_margin(Margin::same(18)))
            .show(ctx, |ui| {
                ui.set_width(w);
                ui.set_height(hgt);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{} Historial de «{}»", icon::CLOCK_COUNTER_CLOCKWISE, display_title(&self.note.title))).font(theme::bold(18.0)));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(egui::Button::new(RichText::new(icon::X).size(18.0).color(MUTED())).frame(false)).on_hover_text("Cerrar (Esc)").clicked() {
                            todo = Some(Do::Close);
                        }
                    });
                });
                ui.label(
                    RichText::new("Se guarda una versión cada vez que la nota cambia (como mucho cada 10 minutos, y siempre antes de un cambio grande), también la que llega de otro equipo.")
                        .size(12.5)
                        .color(MUTED()),
                );
                ui.add_space(8.0);
                if h.versions.is_empty() {
                    ui.add_space(30.0);
                    ui.label(RichText::new("Todavía no hay versiones anteriores de esta nota.").size(14.0));
                    ui.label(RichText::new("Aparecen solas a medida que la nota cambia.").size(13.0).color(MUTED()));
                    return;
                }
                ui.separator();
                let list_w = 190.0;
                let body_h = hgt - 110.0;
                ui.horizontal_top(|ui| {
                    // Las versiones, por día.
                    ui.allocate_ui_with_layout(egui::vec2(list_w, body_h), Layout::top_down(Align::Min), |ui| {
                        egui::ScrollArea::vertical().id_salt("versiones").auto_shrink([false, false]).show(ui, |ui| {
                            ui.set_width(list_w - 12.0);
                            let mut day = String::new();
                            for (i, v) in h.versions.iter().enumerate() {
                                let d = day_label(v.when);
                                if d != day {
                                    ui.add_space(6.0);
                                    ui.label(RichText::new(&d).size(12.0).color(MUTED()));
                                    day = d;
                                }
                                let label = RichText::new(format!("{}  {}", icon::CLOCK, v.when.format("%H:%M"))).size(13.5);
                                if ui.add(egui::Button::selectable(i == h.sel, label).min_size(egui::vec2(list_w - 16.0, 0.0))).clicked() {
                                    todo = Some(Do::Select(i));
                                }
                            }
                        });
                    });
                    ui.separator();
                    // La versión elegida.
                    ui.vertical(|ui| {
                        let v = &h.versions[h.sel];
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("{}, {}", day_label(v.when), v.when.format("%H:%M"))).font(theme::bold(14.5)));
                            ui.add_space(12.0);
                            if ui.selectable_label(!h.full, "Qué cambió").on_hover_text("Comparada con la nota de ahora").clicked() {
                                todo = Some(Do::Full(false));
                            }
                            if ui.selectable_label(h.full, "Texto completo").clicked() {
                                todo = Some(Do::Full(true));
                            }
                        });
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical().id_salt("version").auto_shrink([false, false]).max_height(body_h - 40.0).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            if h.full {
                                ui.add(egui::Label::new(RichText::new(&h.text).size(14.0)).wrap().selectable(true));
                            } else {
                                diff_ui(ui, &h.diff, &mut todo);
                            }
                        });
                    });
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let b = egui::Button::new(RichText::new(format!("{}  Restaurar esta versión", icon::ARROW_COUNTER_CLOCKWISE)).color(theme::c(Color32::WHITE))).fill(ACCENT());
                    if ui.add(b).on_hover_text("La nota vuelve a como estaba; la de ahora queda en el historial").clicked() {
                        todo = Some(Do::Restore);
                    }
                    if ui.button("Cerrar").clicked() {
                        todo = Some(Do::Close);
                    }
                });
            });
        if modal.should_close() {
            todo = Some(Do::Close);
        }
        match todo {
            Some(Do::Close) => return,
            Some(Do::Select(i)) => h.sel = i,
            Some(Do::Full(f)) => h.full = f,
            Some(Do::Recover(line, after)) => {
                self.note.text = insert_line(&self.note.text, after, &line);
                self.note.dirty = true;
                self.note.last_edit = Instant::now();
                self.save();
                self.msg("Línea recuperada");
            }
            Some(Do::Restore) => {
                let (root, rel) = (self.vault.root.clone(), history::rel_of(&self.vault.root, &self.note.path));
                // La de ahora queda en el historial.
                let _ = history::keep(&root, &rel, &self.note.text, Local::now());
                let when = h.versions[h.sel].when;
                self.note.text = h.text.clone();
                self.note.dirty = true;
                self.note.last_edit = Instant::now();
                self.save();
                self.msg(format!("Se restauró la versión de {} a las {}; la que había quedó en el historial", day_label(when).to_lowercase(), when.format("%H:%M")));
                h.versions = history::list(&root, &rel);
                h.sel = 0;
                h.diff_of = None;
                return;
            }
            None => {}
        }
        self.history = Some(h);
    }
}

/// «… 3 líneas iguales»
fn same_lines(ui: &mut Ui, n: usize) {
    let text = if n == 1 { "1 línea igual".to_string() } else { format!("{n} líneas iguales") };
    ui.label(RichText::new(format!("   … {text}")).size(12.0).color(MUTED()));
}

/// Los cambios: lo que se fue (con «Recuperar»), lo nuevo y, alrededor, un poco de lo igual.
fn diff_ui(ui: &mut Ui, diff: &[Diff], todo: &mut Option<Do>) {
    const AROUND: usize = 2;
    if diff.iter().all(|d| matches!(d, Diff::Same(_))) {
        ui.label(RichText::new("Es igual a la nota de ahora.").size(13.5).color(MUTED()));
        return;
    }
    let near: Vec<bool> = (0..diff.len())
        .map(|i| {
            let lo = i.saturating_sub(AROUND);
            let hi = (i + AROUND + 1).min(diff.len());
            diff[lo..hi].iter().any(|d| !matches!(d, Diff::Same(_)))
        })
        .collect();
    let mut skipped = 0;
    for (i, d) in diff.iter().enumerate() {
        if matches!(d, Diff::Same(_)) && !near[i] {
            skipped += 1;
            continue;
        }
        if skipped > 0 {
            same_lines(ui, skipped);
            skipped = 0;
        }
        match d {
            Diff::Same(l) => {
                ui.label(RichText::new(format!("   {l}")).size(13.5).color(MUTED()));
            }
            Diff::Gone { line, after } => {
                Frame::new().fill(theme::c(Color32::from_rgb(252, 235, 235))).corner_radius(4).inner_margin(Margin::symmetric(6, 2)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("−").size(13.5).color(RED()));
                        ui.add(egui::Label::new(RichText::new(line).size(13.5)).wrap());
                        if !line.trim().is_empty() {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.small_button("Recuperar").on_hover_text("Volver a ponerla en la nota, donde estaba").clicked() {
                                    *todo = Some(Do::Recover(line.clone(), *after));
                                }
                            });
                        }
                    });
                });
            }
            Diff::Added(l) => {
                Frame::new().fill(theme::c(Color32::from_rgb(230, 244, 231))).corner_radius(4).inner_margin(Margin::symmetric(6, 2)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("+").size(13.5).color(SUCCESS()));
                        ui.add(egui::Label::new(RichText::new(l).size(13.5)).wrap());
                    });
                });
            }
        }
    }
    if skipped > 0 {
        same_lines(ui, skipped);
    }
    ui.add_space(6.0);
    ui.label(RichText::new("En rojo, lo que había en esta versión y ya no está; en verde, lo que se agregó después.").size(12.0).color(MUTED()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_changed_since_a_version() {
        let old = "Uno\nDos\nTres\n";
        let now = "Uno\nTres\nCuatro\n";
        assert_eq!(
            diff(old, now),
            vec![
                Diff::Same("Uno".into()),
                Diff::Gone { line: "Dos".into(), after: Some(0) },
                Diff::Same("Tres".into()),
                Diff::Added("Cuatro".into()),
            ]
        );
        // Recuperar «Dos» la pone donde estaba.
        assert_eq!(insert_line(now, Some(0), "Dos"), "Uno\nDos\nTres\nCuatro\n");
        assert_eq!(insert_line("A\n", None, "Primera"), "Primera\nA\n");
    }

    /// Con la app: cambiar una nota guarda la versión anterior; restaurarla deja la de ahora.
    #[test]
    fn versions_are_kept_and_restored() {
        let dir = std::env::temp_dir().join(format!("nodex-versiones-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Obra")).unwrap();
        let muro = dir.join("Obra").join("Muro.md");
        fs::write(&muro, "Revisar armado\nPedir acero\nLlamar a Juan\n").unwrap();
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        app.open(muro.clone(), None);
        // Se borran dos líneas de golpe (como si la IA las llevara a otra nota).
        app.note.text = "Revisar armado\n".into();
        app.note.dirty = true;
        app.save();
        let v = history::list(&dir, "Obra/Muro");
        assert_eq!(v.len(), 1);
        assert_eq!(history::read(&v[0]), "Revisar armado\nPedir acero\nLlamar a Juan\n");
        // Restaurar.
        app.open_history();
        let mut h = app.history.take().unwrap();
        h.text = history::read(&h.versions[0]);
        app.history = Some(h);
        let text = app.history.as_ref().unwrap().text.clone();
        let _ = history::keep(&dir, "Obra/Muro", &app.note.text, Local::now());
        app.note.text = text;
        app.note.dirty = true;
        app.save();
        assert_eq!(fs::read_to_string(&muro).unwrap(), "Revisar armado\nPedir acero\nLlamar a Juan\n");
        assert_eq!(history::list(&dir, "Obra/Muro").len(), 2, "la de antes de restaurar también quedó");
        // Al moverla, el historial la sigue.
        fs::create_dir_all(dir.join("General")).unwrap();
        app.vault.scan();
        app.move_note(muro.clone(), "General".into());
        assert_eq!(history::list(&dir, "General/Muro").len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }
}

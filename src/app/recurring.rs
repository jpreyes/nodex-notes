//! Reuniones y notas recurrentes: «Reunión de equipo, todos los lunes a las 9:00».
//!
//! A esa hora (con la app abierta ese día) se crea sola su nota del día, con los acuerdos que
//! quedaron pendientes de la anterior al comienzo (líneas «↻ …», que la IA no vuelve a
//! convertir en tareas). Aparecen en la Agenda y en «Tu día»; en una reunión, «Tomar notas» la
//! empieza. Se guardan en `.nodex/recurrentes.json` (viajan con la carpeta).

use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const FILE: &str = "recurrentes.json";
/// Cuántos días hacia adelante se muestran en la Agenda.
const AHEAD_DAYS: i64 = 30;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub(super) struct Recurring {
    pub titulo: String,
    pub espacio: String,
    /// Días de la semana: 0 = lunes … 6 = domingo.
    pub dias: Vec<u32>,
    /// "09:00"
    pub hora: String,
    /// Reunión (con hora en cada línea y resumen al cerrar) o nota.
    pub reunion: bool,
    /// Último día en que se creó, y su nota (relativa, sin ".md").
    pub ultima: String,
    pub ultima_nota: String,
}

impl Recurring {
    /// "todos los lunes y jueves a las 9:00", "todos los días a las 8:30"
    pub(super) fn describe(&self) -> String {
        let mut d = self.dias.clone();
        d.sort();
        d.dedup();
        let days = match d.len() {
            7 => "todos los días".to_string(),
            5 if d == [0, 1, 2, 3, 4] => "de lunes a viernes".to_string(),
            _ => {
                let names: Vec<String> = d.iter().filter_map(|i| DIAS.get(*i as usize)).map(|n| if n.ends_with('s') { n.to_string() } else { format!("{n}s") }).collect();
                match names.split_last() {
                    Some((last, rest)) if !rest.is_empty() => format!("todos los {} y {last}", rest.join(", ")),
                    Some((last, _)) => format!("todos los {last}"),
                    None => "ningún día".to_string(),
                }
            }
        };
        format!("{days} a las {}", self.hora)
    }

    /// ¿Toca este día?
    fn on(&self, day: NaiveDate) -> bool {
        self.dias.contains(&day.weekday().num_days_from_monday())
    }
}

/// Las recurrentes, por identificador (así dos equipos pueden agregar sin pisarse).
pub(super) type Store = BTreeMap<String, Recurring>;

/// El formulario para crear una (nombre, espacio, días, hora, reunión).
#[derive(Clone)]
pub(super) struct Form {
    pub titulo: String,
    pub espacio: String,
    pub dias: [bool; 7],
    pub hora: String,
    pub reunion: bool,
}

impl NotesApp {
    fn recurring_file(&self) -> PathBuf {
        self.vault.root.join(".nodex").join(FILE)
    }

    pub(super) fn recurring(&self) -> Store {
        vault::read_text(&self.recurring_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn save_recurring(&mut self, s: &Store) {
        let _ = fs::create_dir_all(self.vault.root.join(".nodex"));
        if let Ok(json) = serde_json::to_string_pretty(s) {
            if let Err(e) = fs::write(self.recurring_file(), json) {
                self.msg(format!("No se pudo guardar recurrentes.json: {e}"));
            }
        }
    }

    pub(super) fn add_recurring(&mut self, f: Form) {
        let titulo = vault::sanitize(f.titulo.trim());
        let dias: Vec<u32> = (0..7).filter(|i| f.dias[*i as usize]).collect();
        let hora = f.hora.trim().to_string();
        if f.titulo.trim().is_empty() || dias.is_empty() || !agenda::is_time(&hora) {
            self.msg("Falta el nombre, algún día o la hora (HH:MM)");
            return;
        }
        let mut s = self.recurring();
        let r = Recurring { titulo: titulo.clone(), espacio: f.espacio, dias, hora, reunion: f.reunion, ..Default::default() };
        let what = r.describe();
        s.insert(new_task_id(), r);
        self.save_recurring(&s);
        self.msg(format!("«{titulo}», {what}"));
    }

    pub(super) fn remove_recurring(&mut self, id: &str) {
        let mut s = self.recurring();
        if let Some(r) = s.remove(id) {
            self.save_recurring(&s);
            self.msg(format!("«{}» ya no se repite (sus notas quedan)", r.titulo));
        }
    }

    /// Las próximas: como eventos de la Agenda (hoy y los próximos días).
    pub(super) fn recurring_events(&self) -> Vec<agenda::Event> {
        let today = Local::now().date_naive();
        let mut out = Vec::new();
        for r in self.recurring().values() {
            for k in 0..AHEAD_DAYS {
                let day = today + chrono::Duration::days(k);
                if r.on(day) {
                    out.push(agenda::Event {
                        date: day.format("%Y-%m-%d").to_string(),
                        time: Some(r.hora.clone()),
                        title: r.titulo.clone(),
                        project: r.espacio.clone(),
                        note: None,
                        mail: None,
                    });
                }
            }
        }
        out
    }

    /// La nota de un día de una recurrente.
    fn occurrence_path(&self, r: &Recurring, day: &str) -> PathBuf {
        let ws = if self.vault.workspaces.contains(&r.espacio) { r.espacio.clone() } else { self.home_ws() };
        self.vault.note_path(&ws, &format!("{} {day}", r.titulo))
    }

    /// Crea la nota de hoy de una recurrente (si no existe), con los pendientes de la anterior.
    fn create_occurrence(&mut self, id: &str) -> Option<PathBuf> {
        let mut store = self.recurring();
        let r = store.get(id)?.clone();
        let day = today();
        let path = self.occurrence_path(&r, &day);
        let rel = self.rel(&path);
        if !path.exists() {
            // Un solo equipo la crea.
            if !crate::claims::take_note(&self.vault.root, &rel, &self.machine, CLAIM_TTL) {
                return None;
            }
            let prev = Some(r.ultima_nota.as_str()).filter(|p| !p.is_empty() && *p != rel);
            let pending: Vec<String> = prev
                .map(|p| {
                    self.agenda
                        .tasks()
                        .into_iter()
                        .filter(|t| !t.done && t.note.as_deref() == Some(p))
                        .map(|t| {
                            let due = t.due.as_deref().filter(|d| agenda::is_date(d)).map(|d| format!(" · {}", display_title(d).to_lowercase())).unwrap_or_default();
                            format!("↻ {}{due}", agenda::display_text(&t.text))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let mut text = if r.reunion { format!("## {} · {day} {}\n", r.titulo, r.hora) } else { format!("# {}\n", r.titulo) };
            if !pending.is_empty() {
                let since = r.ultima.get(..10).map(|d| format!(" ({})", display_title(d).to_lowercase())).unwrap_or_default();
                text += &format!("Pendiente de la anterior{since}:\n");
                for l in &pending {
                    text += &format!("{l}\n");
                }
            }
            if let Some(dir) = path.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let written = fs::write(&path, &text);
            crate::claims::release_note(&self.vault.root, &rel, &self.machine);
            if let Err(e) = written {
                self.msg(format!("No se pudo crear «{}»: {e}", r.titulo));
                return None;
            }
            // Lo que ya trae no es nuevo: la IA la organiza cuando se escriba en ella.
            self.analyzed.insert(ai::fnv(&text));
            self.save_analyzed();
            if let Some(m) = vault::modified(&path) {
                self.vault.upsert(path.clone(), text, m);
            }
        }
        if let Some(x) = store.get_mut(id) {
            if x.ultima != day {
                x.ultima = day;
                x.ultima_nota = rel;
                self.save_recurring(&store);
            }
        }
        Some(path)
    }

    /// Revisa si a alguna le toca crearse (se llama cada segundo; trabaja una vez por minuto).
    pub(super) fn maybe_create_recurring(&mut self) {
        let now = Local::now();
        let minute = now.format("%Y-%m-%d %H:%M").to_string();
        if self.recurring_checked == minute {
            return;
        }
        self.recurring_checked = minute;
        let (day, time) = (now.format("%Y-%m-%d").to_string(), now.format("%H:%M").to_string());
        let due: Vec<(String, Recurring)> =
            self.recurring().into_iter().filter(|(_, r)| r.on(now.date_naive()) && r.ultima != day && time >= r.hora).collect();
        for (id, r) in due {
            if self.create_occurrence(&id).is_some() {
                let how = if r.reunion { "en Inicio, «Tomar notas» la empieza" } else { "ya está en su espacio" };
                self.msg(format!("«{}» de hoy está lista: {how}", r.titulo));
            }
        }
    }

    /// «Tomar notas» en una recurrente de hoy: abre su nota del día y empieza la reunión.
    /// Devuelve false si ese título no es de una recurrente de hoy.
    pub(super) fn start_recurring(&mut self, title: &str) -> bool {
        let today = Local::now().date_naive();
        let Some((id, r)) = self.recurring().into_iter().find(|(_, r)| r.titulo == title && r.on(today)) else { return false };
        let Some(path) = self.create_occurrence(&id) else { return false };
        if !r.reunion {
            self.open_in_tab(path, Some(usize::MAX));
            return true;
        }
        self.close_meeting(Local::now());
        self.open_in_tab(path.clone(), None);
        let now = Local::now();
        let t = self.note.text.trim_end().to_string();
        // Si se había cerrado, se retoma: se quita el «## fin» del final.
        let t = match t.rfind("\n## fin") {
            Some(i) if !t[i + 1..].contains('\n') => t[..i].to_string(),
            _ => t,
        };
        self.note.text = format!("{t}\n- {} ", now.format("%H:%M"));
        self.note.dirty = true;
        self.save();
        self.meeting = Some(Meeting { path, title: r.titulo.clone(), started: now, last_activity: Instant::now(), last_time: now });
        self.pending_cursor = Some(usize::MAX);
        self.focus_editor = true;
        self.msg("Reunión iniciada: cada línea lleva su hora. Esc la cierra.");
        true
    }

    /// Ventana para crear y quitar recurrentes.
    pub(super) fn recurring_window(&mut self, ctx: &egui::Context) {
        let Some(form) = self.recurring_form.as_mut() else { return };
        let list = {
            let mut l: Vec<(String, Recurring)> = vault::read_text(&self.vault.root.join(".nodex").join(FILE))
                .ok()
                .and_then(|t| serde_json::from_str::<Store>(&t).ok())
                .unwrap_or_default()
                .into_iter()
                .collect();
            l.sort_by(|a, b| a.1.titulo.cmp(&b.1.titulo));
            l
        };
        let spaces = self.vault.workspaces.clone();
        let (mut add, mut remove, mut close) = (false, None, false);
        let modal = egui::Modal::new(Id::new("recurrentes")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.label(RichText::new(format!("{} Reuniones y notas que se repiten", icon::ARROWS_CLOCKWISE)).font(theme::bold(16.0)));
            ui.add_space(4.0);
            ui.label(RichText::new("A esa hora se crea sola su nota del día, con los acuerdos que quedaron pendientes de la anterior.").size(12.5).color(MUTED));
            ui.add_space(8.0);
            for (id, r) in &list {
                ui.horizontal(|ui| {
                    let glyph = if r.reunion { icon::USERS } else { icon::FILE_TEXT };
                    ui.label(RichText::new(format!("{glyph}  {}", r.titulo)).size(13.5));
                    ui.label(RichText::new(format!("{} · {}", r.describe(), r.espacio)).size(12.0).color(MUTED));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button(icon::TRASH).on_hover_text("Que ya no se repita (sus notas quedan)").clicked() {
                            remove = Some(id.clone());
                        }
                    });
                });
            }
            if !list.is_empty() {
                ui.separator();
            }
            ui.label(RichText::new("Nueva").font(theme::bold(13.5)));
            ui.add(egui::TextEdit::singleline(&mut form.titulo).hint_text("Nombre: Reunión de equipo").desired_width(f32::INFINITY));
            ui.horizontal(|ui| {
                ui.radio_value(&mut form.reunion, true, "Reunión");
                ui.radio_value(&mut form.reunion, false, "Nota");
                ui.add_space(12.0);
                ui.label("en");
                egui::ComboBox::from_id_salt("recurrente-espacio").selected_text(form.espacio.clone()).show_ui(ui, |ui| {
                    for w in &spaces {
                        ui.selectable_value(&mut form.espacio, w.clone(), w);
                    }
                });
            });
            ui.horizontal(|ui| {
                for (i, d) in DIAS_CORTOS.iter().enumerate() {
                    ui.toggle_value(&mut form.dias[i], *d);
                }
                ui.add_space(8.0);
                ui.label("a las");
                ui.add(egui::TextEdit::singleline(&mut form.hora).hint_text("09:00").desired_width(48.0));
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let ok = !form.titulo.trim().is_empty() && form.dias.iter().any(|d| *d) && agenda::is_time(form.hora.trim());
                if ui.add_enabled(ok, egui::Button::new(RichText::new("Agregar").color(Color32::WHITE)).fill(ACCENT)).clicked() {
                    add = true;
                }
                if ui.button("Cerrar").clicked() {
                    close = true;
                }
            });
        });
        if modal.should_close() {
            close = true;
        }
        if let Some(id) = remove {
            self.remove_recurring(&id);
        }
        if add {
            if let Some(f) = self.recurring_form.clone() {
                let ws = f.espacio.clone();
                self.add_recurring(f);
                self.recurring_form = Some(Form { titulo: String::new(), espacio: ws, dias: [false; 7], hora: "09:00".into(), reunion: true });
            }
        }
        if close {
            self.recurring_form = None;
        }
    }

    /// Abre la ventana de recurrentes.
    pub(super) fn open_recurring(&mut self) {
        self.recurring_form = Some(Form { titulo: String::new(), espacio: self.ws.clone(), dias: [false; 7], hora: "09:00".into(), reunion: true });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_the_days() {
        let r = |dias: Vec<u32>| Recurring { dias, hora: "09:00".into(), ..Default::default() };
        assert_eq!(r(vec![0]).describe(), "todos los lunes a las 09:00");
        assert_eq!(r(vec![0, 3]).describe(), "todos los lunes y jueves a las 09:00");
        assert_eq!(r(vec![0, 1, 2, 3, 4]).describe(), "de lunes a viernes a las 09:00");
        assert_eq!(r((0..7).collect()).describe(), "todos los días a las 09:00");
    }

    /// Se crea sola a su hora, con los acuerdos pendientes de la anterior; «Tomar notas» la
    /// empieza como reunión.
    #[test]
    fn a_weekly_meeting_is_created_with_what_was_left_pending() {
        let dir = std::env::temp_dir().join(format!("nodex-recurrentes-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        fs::create_dir_all(dir.join("Obra")).unwrap();
        fs::write(dir.join("Obra").join("Reunión de equipo 2026-09-28.md"), "## Reunión de equipo · 2026-09-28 09:00\n## fin · 09:40\n").unwrap();
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        // La tarea apunta a la reunión anterior (nota con espacios en su nombre).
        let prev = "Obra/Reunión de equipo 2026-09-28";
        fs::write(dir.join("tareas.txt"), format!("2026-09-28 Enviar planos @Juan +Obra due:2026-10-02 nota:{} id:a1\n", agenda::encode_note(prev))).unwrap();

        // Todos los días a las 00:00, para que toque hoy y ya sea la hora.
        app.add_recurring(Form { titulo: "Reunión de equipo".into(), espacio: "Obra".into(), dias: [true; 7], hora: "00:00".into(), reunion: true });
        let mut s = app.recurring();
        let id = s.keys().next().unwrap().clone();
        s.get_mut(&id).unwrap().ultima_nota = prev.into();
        s.get_mut(&id).unwrap().ultima = "2026-09-28".into();
        app.save_recurring(&s);

        app.maybe_create_recurring();
        let path = dir.join("Obra").join(format!("Reunión de equipo {}.md", today()));
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with(&format!("## Reunión de equipo · {} 00:00\nPendiente de la anterior", today())), "{text}");
        assert!(text.contains("↻ Juan: Enviar planos · "), "{text}");
        assert_eq!(app.recurring()[&id].ultima, today());
        // No se crea dos veces.
        app.recurring_checked.clear();
        app.maybe_create_recurring();
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
        // Aparece en la agenda de hoy, y «Tomar notas» empieza la reunión en su nota.
        assert!(app.all_events().iter().any(|e| e.date == today() && e.title == "Reunión de equipo"));
        app.apply(Action::StartMeetingNamed("Reunión de equipo".into()));
        assert_eq!(app.note.path, path);
        assert_eq!(app.meeting.as_ref().map(|m| m.path.clone()), Some(path.clone()));
        let _ = fs::remove_dir_all(&dir);
    }
}

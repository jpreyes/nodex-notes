//! Plantillas: notas en `Plantillas/` que sirven de punto de partida («Visita a obra», «Acta»).
//! Desde el «+» de las pestañas, «Desde una plantilla» crea una nota con su texto, en el espacio
//! actual; `{{fecha}}`, `{{hoy}}`, `{{hora}}` y `{{titulo}}` se completan. Una nota se puede
//! guardar como plantilla (clic derecho). La carpeta no es un espacio: sus notas no se
//! organizan, no se buscan y sus casillas no son tareas.

use super::*;

/// El texto de una plantilla para una nota nueva llamada `title`, escrita en `now`.
pub(super) fn fill(text: &str, title: &str, now: DateTime<Local>) -> String {
    let hoy = format!("{} {} {}", DIAS[now.weekday().num_days_from_monday() as usize], now.day(), MESES[now.month0() as usize]);
    text.replace("{{fecha}}", &now.format("%Y-%m-%d").to_string())
        .replace("{{hoy}}", &hoy)
        .replace("{{hora}}", &now.format("%H:%M").to_string())
        .replace("{{titulo}}", title)
}

/// Una nota para guardarla como plantilla: las tareas sin marcar, sin fechas ni identificadores
/// (serían de la nota de origen).
pub(super) fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for full in text.split_inclusive('\n') {
        let line = full.trim_end_matches(['\n', '\r']);
        let mut l = if lines::parse(line).check == Some(true) { lines::toggle_check(line) } else { line.to_string() };
        if lines::due_of(&l).is_some() || lines::id_of(&l).is_some() {
            let lead = &l[..l.len() - l.trim_start().len()];
            let words: Vec<&str> = l.split_whitespace().filter(|w| !(w.starts_with("due:") || (w.starts_with('^') && w.len() > 3))).collect();
            l = format!("{lead}{}", words.join(" "));
        }
        out += &l;
        out += &full[line.len()..];
    }
    out
}

impl NotesApp {
    fn templates_dir(&self) -> PathBuf {
        self.vault.root.join(vault::TEMPLATES)
    }

    /// Las plantillas, por nombre.
    pub(super) fn templates(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = fs::read_dir(self.templates_dir())
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("md")))
            .collect();
        v.sort_by_key(|p| vault::stem(p).to_lowercase());
        v
    }

    /// Una nota nueva con el texto de la plantilla, en el espacio actual.
    pub(super) fn new_from_template(&mut self, tpl: &Path) {
        let Ok(text) = vault::read_text(tpl) else {
            self.msg(format!("No se pudo leer la plantilla «{}»", vault::stem(tpl)));
            return;
        };
        let ws = if self.vault.workspaces.contains(&self.ws) { self.ws.clone() } else { self.home_ws() };
        let now = Local::now();
        let path = self.vault.unique_path(&ws, &format!("{} {}", vault::stem(tpl), now.format("%Y-%m-%d")));
        let body = fill(&text, &vault::stem(&path), now);
        if let Err(e) = fs::create_dir_all(self.vault.root.join(&ws)).and_then(|_| fs::write(&path, &body)) {
            self.msg(format!("No se pudo crear la nota: {e}"));
            return;
        }
        self.vault.upsert(path.clone(), body, vault::modified(&path).unwrap_or_else(SystemTime::now));
        self.open_in_tab(path, None);
        self.focus_title = true;
        self.msg(format!("Nota nueva desde «{}»: cámbiale el nombre si quieres", vault::stem(tpl)));
    }

    /// Una plantilla nueva, vacía, para escribirla.
    pub(super) fn new_template(&mut self) {
        let path = self.vault.unique_path(vault::TEMPLATES, "Plantilla nueva");
        self.open_in_tab(path, None);
        self.focus_title = true;
    }

    /// Guarda la nota abierta como plantilla (con su nombre).
    pub(super) fn save_as_template(&mut self) {
        self.save();
        let name = display_title(&self.note.title);
        let path = self.vault.unique_path(vault::TEMPLATES, &name);
        let r = fs::create_dir_all(self.templates_dir()).and_then(|_| fs::write(&path, clean(&self.note.text)));
        match r {
            Ok(()) => self.msg(format!("Guardada como plantilla «{}»: úsala desde el + de las pestañas", vault::stem(&path))),
            Err(e) => self.msg(format!("No se pudo guardar la plantilla: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_fill_in_the_date_and_forget_their_tasks() {
        use chrono::TimeZone;
        let now = Local.with_ymd_and_hms(2026, 10, 1, 9, 30, 0).unwrap();
        assert_eq!(fill("# {{titulo}}\nVisita del {{hoy}} ({{fecha}}, {{hora}})", "Visita", now), "# Visita\nVisita del jueves 1 oct (2026-10-01, 09:30)");
        let t = "Revisar:\n- [x] Armado muro due:2026-09-30 ^k3f9a\n- [ ] Moldaje ^ab12c\nNotas #obra\n";
        assert_eq!(clean(t), "Revisar:\n- [ ] Armado muro\n- [ ] Moldaje\nNotas #obra\n");
    }

    /// Con la app: la plantilla no es un espacio, y una nota desde ella queda en el espacio actual.
    #[test]
    fn notes_from_templates() {
        let dir = std::env::temp_dir().join(format!("nodex-plantillas-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Obra")).unwrap();
        fs::create_dir_all(dir.join(vault::TEMPLATES)).unwrap();
        fs::write(dir.join(vault::TEMPLATES).join("Visita a obra.md"), "Visita del {{fecha}}\n- [ ] Fotos\n").unwrap();
        fs::write(dir.join("Obra").join("Muro.md"), "x\n").unwrap();
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        assert!(!app.vault.workspaces.contains(&vault::TEMPLATES.to_string()));
        app.open(dir.join("Obra").join("Muro.md"), None);
        let tpls = app.templates();
        assert_eq!(tpls.len(), 1);
        app.new_from_template(&tpls[0]);
        let day = Local::now().format("%Y-%m-%d").to_string();
        assert_eq!(app.note.path, dir.join("Obra").join(format!("Visita a obra {day}.md")));
        assert_eq!(app.note.text, format!("Visita del {day}\n- [ ] Fotos\n"));
        // Editar la plantilla no la vuelve una nota más (ni la manda a la IA).
        app.open(tpls[0].clone(), None);
        app.note.text.push_str("- [ ] Planos\n");
        app.note.dirty = true;
        app.save();
        assert!(app.vault.get(&tpls[0]).is_none());
        assert!(!app.touched.contains(&tpls[0]));
        assert_eq!(workspace_of(&tpls[0]), None);
        let _ = fs::remove_dir_all(&dir);
    }
}

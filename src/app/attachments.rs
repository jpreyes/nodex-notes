//! Adjuntar archivos a una nota: se arrastran a la ventana (o clic derecho → «Adjuntar
//! archivo…»), se copian a `Adjuntos/` y la nota gana una línea por archivo:
//! `![foto](../Adjuntos/foto.jpg)` si es una imagen (se ve en la nota) o
//! `[informe.pdf](../Adjuntos/informe.pdf)` si no (un clic lo abre con su programa).

use super::*;

/// Extensiones que se muestran como imagen dentro de la nota.
pub(super) const IMAGE_EXT: [&str; 7] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "jfif"];

pub(super) fn is_image(path: &Path) -> bool {
    path.extension().is_some_and(|e| IMAGE_EXT.contains(&e.to_string_lossy().to_lowercase().as_str()))
}

/// El ícono de un archivo según su tipo.
pub(super) fn file_icon(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "pdf" => icon::FILE_PDF,
        "doc" | "docx" | "odt" | "rtf" => icon::FILE_DOC,
        "xls" | "xlsx" | "ods" => icon::FILE_XLS,
        "csv" => icon::FILE_CSV,
        "ppt" | "pptx" | "odp" => icon::FILE_PPT,
        "zip" | "rar" | "7z" | "tar" | "gz" => icon::FILE_ZIP,
        "mp4" | "mov" | "avi" | "mkv" | "webm" => icon::FILE_VIDEO,
        "mp3" | "wav" | "m4a" | "ogg" | "flac" => icon::FILE_AUDIO,
        "txt" | "md" => icon::FILE_TXT,
        "dwg" | "dxf" | "skp" | "ifc" | "rvt" => icon::FILE_DASHED,
        _ if IMAGE_EXT.contains(&ext.as_str()) => icon::FILE_IMAGE,
        _ => icon::PAPERCLIP,
    }
}

/// Un nombre libre en `dir` («informe.pdf», «informe (2).pdf», …).
fn free_name(dir: &Path, name: &str) -> String {
    if !dir.join(name).exists() {
        return name.to_string();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    (2..).map(|i| format!("{stem} ({i}){ext}")).find(|n| !dir.join(n).exists()).unwrap_or_else(|| name.to_string())
}

/// La línea de la nota para un archivo que ya está en `Adjuntos/` con ese nombre.
pub(super) fn attachment_line(name: &str) -> String {
    let rel = format!("../{}/{}", vault::ATTACHMENTS, lines::encode_path(name));
    if is_image(Path::new(name)) {
        let alt = name.rsplit_once('.').map_or(name, |(s, _)| s);
        format!("![{}]({rel})", alt.replace(['[', ']'], ""))
    } else {
        format!("[{}]({rel})", name.replace(['[', ']'], ""))
    }
}

impl NotesApp {
    /// Agrega líneas a la nota abierta después de la línea `line` (si está vacía, la usa; sin
    /// línea, al final) y la guarda.
    pub(super) fn insert_lines_after(&mut self, line: Option<usize>, new: Vec<String>) {
        let mut ls: Vec<String> = self.note.text.split('\n').map(str::to_string).collect();
        let mut cur = line.unwrap_or(ls.len() - 1).min(ls.len() - 1);
        for (i, md) in new.into_iter().enumerate() {
            if i == 0 && ls[cur].trim().is_empty() {
                ls[cur] = md;
            } else {
                cur += 1;
                ls.insert(cur, md);
            }
        }
        let mut text = ls.join("\n");
        if !text.ends_with('\n') {
            text.push('\n');
        }
        self.note.text = text;
        self.note.dirty = true;
        self.note.last_edit = Instant::now();
        self.save();
    }

    /// Copia los archivos a `Adjuntos/` y los agrega a la nota abierta, después de `line`.
    pub(super) fn attach_files(&mut self, files: Vec<PathBuf>, line: Option<usize>) {
        let dir = self.vault.root.join(vault::ATTACHMENTS);
        let mut added = Vec::new();
        let mut names = Vec::new();
        for f in files {
            if f.is_dir() {
                self.msg(format!("«{}» es una carpeta: se adjuntan solo archivos", vault::stem(&f)));
                continue;
            }
            let Some(name) = f.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
            // Ya está en Adjuntos: no se copia de nuevo.
            let name = if f.parent().is_some_and(|p| p == dir) {
                name
            } else {
                let target = free_name(&dir, &name);
                if let Err(e) = fs::create_dir_all(&dir).and_then(|_| fs::copy(&f, dir.join(&target))) {
                    self.msg(format!("No se pudo adjuntar «{name}»: {e}"));
                    continue;
                }
                target
            };
            added.push(attachment_line(&name));
            names.push(name);
        }
        if added.is_empty() {
            return;
        }
        self.insert_lines_after(line, added);
        self.msg(match names.as_slice() {
            [one] => format!("Adjuntado «{one}» (en {})", vault::ATTACHMENTS),
            many => format!("{} adjuntados (en {})", many.len(), vault::ATTACHMENTS),
        });
    }

    /// «Adjuntar archivo…»: elegirlos con la ventana del sistema.
    pub(super) fn pick_attachments(&mut self, line: Option<usize>) {
        if let Some(files) = rfd::FileDialog::new().set_title("Adjuntar a la nota").pick_files() {
            self.attach_files(files, line);
        }
    }

    /// Archivos soltados sobre la ventana: van a la nota abierta. Mientras se arrastran, un aviso.
    pub(super) fn handle_dropped_files(&mut self, ctx: &egui::Context, cursor_line: Option<usize>) {
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        if hovering {
            let rect = ctx.content_rect();
            let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("soltar-archivos")));
            p.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(255, 255, 255, 215));
            let text = if self.view == View::Editor {
                format!("{}  Suelta para adjuntar a «{}»", icon::PAPERCLIP, display_title(&self.note.title))
            } else {
                format!("{}  Abre una nota para adjuntarle archivos", icon::PAPERCLIP)
            };
            p.text(rect.center(), Align2::CENTER_CENTER, text, theme::bold(20.0), ACCENT);
        }
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if dropped.is_empty() {
            return;
        }
        if self.view == View::Editor {
            self.attach_files(dropped, cursor_line);
        } else {
            self.msg("Abre una nota para adjuntarle archivos");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_for_attachments() {
        assert_eq!(attachment_line("Informe final.pdf"), "[Informe final.pdf](../Adjuntos/Informe%20final.pdf)");
        assert_eq!(attachment_line("foto obra.JPG"), "![foto obra](../Adjuntos/foto%20obra.JPG)");
        assert_eq!(file_icon("planos.dwg"), icon::FILE_DASHED);
        let dir = std::env::temp_dir().join(format!("nodex-adjuntos-nombre-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.pdf"), "x").unwrap();
        fs::write(dir.join("a (2).pdf"), "x").unwrap();
        assert_eq!(free_name(&dir, "a.pdf"), "a (3).pdf");
        assert_eq!(free_name(&dir, "b.pdf"), "b.pdf");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Con la app: el archivo se copia a Adjuntos y la nota gana su línea, debajo del cursor.
    #[test]
    fn attaching_copies_the_file_and_adds_its_line() {
        let dir = std::env::temp_dir().join(format!("nodex-adjuntar-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Obra")).unwrap();
        let outside = std::env::temp_dir().join(format!("nodex-afuera-{}", std::process::id()));
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("Acta 30 sep.pdf"), "pdf").unwrap();
        fs::write(outside.join("foto.jpg"), "jpg").unwrap();
        let muro = dir.join("Obra").join("Muro.md");
        fs::write(&muro, "Uno\nDos\n").unwrap();
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        app.open(muro.clone(), None);
        app.attach_files(vec![outside.join("Acta 30 sep.pdf"), outside.join("foto.jpg")], Some(0));
        assert_eq!(
            fs::read_to_string(&muro).unwrap(),
            "Uno\n[Acta 30 sep.pdf](../Adjuntos/Acta%2030%20sep.pdf)\n![foto](../Adjuntos/foto.jpg)\nDos\n"
        );
        assert_eq!(fs::read_to_string(dir.join("Adjuntos").join("Acta 30 sep.pdf")).unwrap(), "pdf");
        assert!(!app.vault.workspaces.contains(&"Adjuntos".to_string()));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&outside);
    }
}

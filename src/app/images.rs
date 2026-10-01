//! Capturas de pantalla pegadas en las notas.
//!
//! Ctrl+V con una imagen en el portapapeles (o clic derecho → «Pegar imagen») la guarda en
//! tamaño real, como PNG, en `Adjuntos/` y agrega a la nota una línea
//! `![Captura 30 sep 17:06](../Adjuntos/captura-2026-09-30-170632.png)`. La ruta es relativa a
//! la nota, y como todas las notas están a un nivel, sirve aunque la IA lleve la línea a otra.
//! En el editor la línea se ve como la imagen; un clic la abre en tamaño real.

use super::*;
use std::collections::HashMap;

/// Alto máximo con que se muestra una imagen en la nota.
const MAX_H: f32 = 420.0;

/// Las imágenes ya cargadas (o que no se pudieron leer), por archivo.
#[derive(Default)]
pub(super) struct Images {
    loaded: HashMap<PathBuf, Option<(egui::TextureHandle, [usize; 2])>>,
    /// La tecla V estaba presionada en el cuadro anterior (para notar Ctrl+V).
    v_down: bool,
}

/// El archivo de una imagen de la nota (la ruta de la línea es relativa a la nota).
pub(super) fn resolve(note: &Path, rel: &str) -> PathBuf {
    let dir = note.parent().unwrap_or(Path::new(""));
    let mut out = dir.to_path_buf();
    for part in rel.replace('\\', "/").split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            p => out.push(lines::decode_path(p)),
        }
    }
    out
}

/// ¿Está apretada la tecla V? (Windows, Mac y Linux con X11; en Wayland no se puede saber:
/// ahí se pega con clic derecho → «Pegar imagen del portapapeles».)
#[cfg(windows)]
fn v_key_down() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_V};
    unsafe { GetAsyncKeyState(VK_V as i32) < 0 }
}

#[cfg(target_os = "macos")]
fn v_key_down() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventSourceKeyState(state: i32, key: u16) -> bool;
    }
    // kCGEventSourceStateCombinedSessionState = 0; la V es la tecla 9 (kVK_ANSI_V).
    unsafe { CGEventSourceKeyState(0, 0x09) }
}

#[cfg(target_os = "linux")]
fn v_key_down() -> bool {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::ConnectionExt;
    use x11rb::rust_connection::RustConnection;
    // Una conexión con X11 (la primera vez) y el código de la tecla de la «v».
    static X: std::sync::OnceLock<Option<(RustConnection, u8)>> = std::sync::OnceLock::new();
    let Some((conn, code)) = X.get_or_init(|| {
        let (conn, _) = x11rb::connect(None).ok()?;
        let (min, max) = (conn.setup().min_keycode, conn.setup().max_keycode);
        let map = conn.get_keyboard_mapping(min, max - min + 1).ok()?.reply().ok()?;
        let per = map.keysyms_per_keycode.max(1) as usize;
        // XK_v = 0x76, XK_V = 0x56
        let i = map.keysyms.chunks(per).position(|k| k.contains(&0x76) || k.contains(&0x56))?;
        Some((conn, min + i as u8))
    }) else {
        return false;
    };
    let Ok(Ok(keys)) = conn.query_keymap().map(|c| c.reply()) else { return false };
    keys.keys.get(*code as usize / 8).is_some_and(|b| b & (1 << (code % 8)) != 0)
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn v_key_down() -> bool {
    false
}

/// Lado máximo con que se carga una imagen para verla en la nota (el clic abre el original).
const MAX_SIDE: u32 = 2048;

/// Lee una imagen (PNG, JPG, GIF, WebP o BMP), derecha según cómo se tomó la foto.
fn decode_image(bytes: &[u8]) -> Option<egui::ColorImage> {
    use image::ImageDecoder;
    let mut dec = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?.into_decoder().ok()?;
    let orientation = dec.orientation().ok();
    let mut img = image::DynamicImage::from_decoder(dec).ok()?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    if img.width() > MAX_SIDE || img.height() > MAX_SIDE {
        img = img.thumbnail(MAX_SIDE, MAX_SIDE);
    }
    let rgba = img.to_rgba8();
    Some(egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
}

/// Guarda una imagen RGBA como PNG.
pub(super) fn encode_png(w: usize, h: usize, rgba: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().ok()?;
        wr.write_image_data(rgba).ok()?;
    }
    Some(out)
}

impl Images {
    /// Carga las imágenes de la nota que aún no están (una vez cada una).
    pub(super) fn prepare(&mut self, ctx: &egui::Context, note: &Path, text: &str) {
        for line in text.lines().filter(|l| l.contains("![")) {
            let Some((_, rel)) = lines::image_of(line) else { continue };
            let path = resolve(note, rel);
            if self.loaded.contains_key(&path) {
                continue;
            }
            let img = fs::read(&path).ok().and_then(|b| decode_image(&b)).map(|img| {
                let size = img.size;
                (ctx.load_texture(path.to_string_lossy(), img, egui::TextureOptions::LINEAR), size)
            });
            self.loaded.insert(path, img);
        }
    }

    /// Cómo se muestra (ancho y alto) una imagen cargada, en una columna de ancho `max_w`.
    pub(super) fn shown_size(&self, path: &Path, max_w: f32) -> Option<(f32, f32)> {
        let (_, [w, h]) = self.loaded.get(path)?.as_ref()?;
        let (w, h) = (*w as f32, *h as f32);
        let scale = (max_w / w).min(MAX_H / h).min(1.0);
        Some((w * scale, h * scale))
    }

    pub(super) fn texture(&self, path: &Path) -> Option<egui::TextureId> {
        self.loaded.get(path)?.as_ref().map(|(t, _)| t.id())
    }

    /// ¿Se presionó Ctrl+V (Cmd+V en Mac) recién? egui no avisa si el portapapeles solo tiene
    /// una imagen, así que se mira el estado de la tecla V en el sistema.
    pub(super) fn ctrl_v_pressed(&mut self, ui: &Ui) -> bool {
        let down = ui.input(|i| i.modifiers.command) && v_key_down();
        let pressed = down && !self.v_down;
        self.v_down = down;
        pressed
    }
}

impl NotesApp {
    /// Pega la imagen del portapapeles en la nota abierta, después de la línea `line`.
    /// Devuelve false si el portapapeles no tiene una imagen.
    pub(super) fn paste_image(&mut self, line: Option<usize>) -> bool {
        let Ok(img) = arboard::Clipboard::new().and_then(|mut c| c.get_image()) else { return false };
        let mut rgba = img.bytes.into_owned();
        // Algunas capturas llegan con el canal alfa en cero: se ven invisibles.
        if rgba.chunks(4).all(|p| p.get(3) == Some(&0)) {
            for p in rgba.chunks_mut(4) {
                p[3] = 255;
            }
        }
        let Some(png) = encode_png(img.width, img.height, &rgba) else {
            self.msg("No se pudo convertir la imagen");
            return true;
        };
        let now = Local::now();
        let dir = self.vault.root.join(vault::ATTACHMENTS);
        let mut name = format!("captura-{}.png", now.format("%Y-%m-%d-%H%M%S"));
        let mut i = 2;
        while dir.join(&name).exists() {
            name = format!("captura-{}-{i}.png", now.format("%Y-%m-%d-%H%M%S"));
            i += 1;
        }
        if let Err(e) = fs::create_dir_all(&dir).and_then(|_| fs::write(dir.join(&name), &png)) {
            self.msg(format!("No se pudo guardar la imagen: {e}"));
            return true;
        }
        let md = format!("![Captura {} {} {}](../{}/{name})", now.day(), MESES[now.month0() as usize], now.format("%H:%M"), vault::ATTACHMENTS);
        // Va en la línea del cursor si está vacía, o en una nueva debajo (sin cursor: al final).
        let _ = self.insert_lines_after(line, vec![md]);
        self.msg(format!("Captura pegada ({}×{}), guardada en {}/{name}", img.width, img.height, vault::ATTACHMENTS));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_lines_and_paths() {
        assert_eq!(lines::image_of("![Captura 30 sep](../Adjuntos/c.png)"), Some(("Captura 30 sep", "../Adjuntos/c.png")));
        assert_eq!(lines::image_of("texto ![x](a.png)"), None);
        // Con etiquetas que le puso la IA, sigue siendo una imagen.
        assert_eq!(lines::image_of("![x](a.png) #planos"), Some(("x", "a.png")));
        assert_eq!(lines::image_of("![x](a.png) y más texto"), None);
        let note = Path::new("C:/Notas/Obra/Muro.md");
        assert_eq!(resolve(note, "../Adjuntos/c.png"), PathBuf::from("C:/Notas/Adjuntos/c.png"));
        // Una imagen pequeña ida y vuelta.
        let rgba: Vec<u8> = (0..4 * 3 * 2).map(|i| i as u8).collect();
        let png = encode_png(3, 2, &rgba).unwrap();
        let img = decode_image(&png).unwrap();
        assert_eq!(img.size, [3, 2]);
    }
}

#[cfg(test)]
mod clipboard_tests {
    use super::*;

    /// Con el portapapeles de verdad (se deja como estaba): cargo test pegar_captura -- --ignored
    #[test]
    #[ignore]
    fn pegar_captura_del_portapapeles() {
        let mut cb = arboard::Clipboard::new().unwrap();
        let saved_text = cb.get_text().ok();
        let saved_img = cb.get_image().ok().map(|i| i.to_owned_img());
        // Una imagen de 4×3 con el alfa en cero, como llegan algunas capturas.
        let bytes: Vec<u8> = (0..12).flat_map(|i| [i as u8 * 20, 100, 200, 0]).collect();
        cb.set_image(arboard::ImageData { width: 4, height: 3, bytes: bytes.into() }).unwrap();

        let dir = std::env::temp_dir().join(format!("nodex-captura-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        fs::create_dir_all(dir.join("Obra")).unwrap();
        let muro = dir.join("Obra").join("Muro.md");
        fs::write(&muro, "Línea uno\nLínea dos\n").unwrap();
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        app.open(muro.clone(), None);
        let ok = app.paste_image(Some(0));

        match (saved_text, saved_img) {
            (Some(t), _) => cb.set_text(t).unwrap(),
            (None, Some(i)) => cb.set_image(i).unwrap(),
            _ => cb.clear().unwrap(),
        }
        assert!(ok);
        let text = fs::read_to_string(&muro).unwrap();
        let line = text.lines().nth(1).unwrap();
        let (_, rel) = lines::image_of(line).expect("la segunda línea es la imagen");
        let file = resolve(&muro, rel);
        assert!(file.starts_with(dir.join("Adjuntos")), "{file:?}");
        let img = decode_image(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(img.size, [4, 3]);
        assert_eq!(img.pixels[0].a(), 255, "el alfa se arregla");
        assert!(text.ends_with("Línea dos\n"));
        assert!(!app.vault.workspaces.contains(&"Adjuntos".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }
}

//! Capturas de pantalla pegadas en las notas.
//!
//! Ctrl+V con una imagen en el portapapeles (o clic derecho → «Pegar imagen») la guarda en
//! tamaño real, como PNG, en `Adjuntos/` y agrega a la nota una línea
//! `![Captura 30 sep 17:06](../Adjuntos/captura-2026-09-30-170632.png)`. La ruta es relativa a
//! la nota, y como todas las notas están a un nivel, sirve aunque la IA lleve la línea a otra.
//! En el editor la línea se ve como la imagen; un clic la abre en tamaño real.
//!
//! También se ven las imágenes de la web (`![](https://…)`, guardadas en una caché local) y los
//! PDF (una línea `[Informe](../Adjuntos/informe.pdf)` muestra sus páginas). El ancho se puede
//! elegir: «![Plano|300](…)» (arrastrando la esquina o con el clic derecho); `|0` en un PDF lo
//! deja como enlace.

use super::*;
use std::collections::HashMap;
use std::sync::mpsc;

/// Alto máximo con que se muestra una imagen en la nota.
const MAX_H: f32 = 420.0;
/// Ancho con que se muestra un PDF (si no se eligió otro).
const PDF_W: f32 = 520.0;
/// Lo más grande que se baja de la web.
const MAX_DOWNLOAD: usize = 25 * 1024 * 1024;

/// Una imagen o página que terminó de cargarse aparte.
struct Loaded {
    key: PathBuf,
    /// El PDF y cuántas páginas tiene.
    pdf: Option<(PathBuf, usize)>,
    image: Option<egui::ColorImage>,
}

/// Las imágenes ya cargadas (o que no se pudieron leer), por archivo.
pub(super) struct Images {
    loaded: HashMap<PathBuf, Option<(egui::TextureHandle, [usize; 2])>>,
    /// La tecla V estaba presionada en el cuadro anterior (para notar Ctrl+V).
    v_down: bool,
    /// Lo que se está cargando aparte (de la web, o una página de un PDF).
    pending: HashSet<PathBuf>,
    tx: mpsc::Sender<Loaded>,
    rx: mpsc::Receiver<Loaded>,
    /// De cada PDF: cuántas páginas tiene, cuál se ve y la última que se alcanzó a mostrar.
    pdf_pages: HashMap<PathBuf, usize>,
    pdf_page: HashMap<PathBuf, usize>,
    pdf_last: HashMap<PathBuf, PathBuf>,
    /// Arrastrando la esquina de una imagen: (ruta escrita, ancho).
    pub resizing: Option<(String, f32)>,
}

impl Default for Images {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Images {
            loaded: HashMap::new(),
            v_down: false,
            pending: HashSet::new(),
            tx,
            rx,
            pdf_pages: HashMap::new(),
            pdf_page: HashMap::new(),
            pdf_last: HashMap::new(),
            resizing: None,
        }
    }
}

pub(super) fn is_url(rel: &str) -> bool {
    rel.starts_with("http://") || rel.starts_with("https://")
}

pub(super) fn is_pdf(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

/// Dónde está lo que dice una línea: el archivo (relativo a la nota) o la dirección web.
pub(super) fn locate(note: &Path, rel: &str) -> PathBuf {
    if is_url(rel) { PathBuf::from(rel) } else { resolve(note, rel) }
}

/// Dónde se guarda una imagen bajada de la web.
pub(super) fn web_cache(url: &str) -> PathBuf {
    use sha2::Digest;
    let hash: String = sha2::Sha256::digest(url.as_bytes()).iter().take(12).map(|b| format!("{b:02x}")).collect();
    crate::config::config_path().with_file_name("imagenes-web").join(hash)
}

/// Si lo pegado es la dirección de una imagen, la línea que la muestra.
pub(super) fn pasted_image_url(text: &str) -> Option<String> {
    let t = text.trim();
    if !is_url(t) || t.contains(char::is_whitespace) || t.contains(['(', ')']) {
        return None;
    }
    let path = t.split(['?', '#']).next().unwrap_or(t).to_lowercase();
    super::attachments::IMAGE_EXT.iter().any(|e| path.ends_with(&format!(".{e}"))).then(|| format!("![]({t})"))
}

/// La clave de una página de un PDF.
fn page_key(pdf: &Path, page: usize) -> PathBuf {
    PathBuf::from(format!("{}#pagina{page}", pdf.display()))
}

/// Dibuja una página de un PDF (unos 1100 px de ancho, para que se lea nítida).
pub(super) fn render_pdf_page(bytes: Vec<u8>, page: usize) -> Option<(egui::ColorImage, usize)> {
    use hayro::hayro_interpret::InterpreterSettings;
    use hayro::hayro_syntax::Pdf;
    let pdf = Pdf::new(bytes).ok()?;
    let pages = pdf.pages();
    let p = pages.get(page)?;
    let (w, h) = p.render_dimensions();
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let scale = (1100.0 / w).clamp(0.5, 3.0).min(4000.0 / h);
    let settings = hayro::PixmapSettings { x_scale: scale, y_scale: scale, bg_color: hayro::vello_cpu::color::palette::css::WHITE };
    let pix = hayro::render(p, &hayro::RenderCache::new(), &InterpreterSettings::default(), &hayro::RenderSettings::default(), &settings);
    let size = [pix.width() as usize, pix.height() as usize];
    Some((egui::ColorImage::from_rgba_premultiplied(size, pix.data_as_u8_slice()), pages.len()))
}

/// Baja una imagen de la web a la caché.
fn download(url: &str) -> Result<Vec<u8>, String> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    let client = reqwest::Client::builder().timeout(Duration::from_secs(30)).build().map_err(|e| e.to_string())?;
    rt.block_on(async {
        let r = client.get(url).header("User-Agent", format!("nodex-notes/{}", env!("CARGO_PKG_VERSION"))).send().await.map_err(|e| e.to_string())?;
        if !r.status().is_success() {
            return Err(r.status().to_string());
        }
        if r.content_length().is_some_and(|n| n as usize > MAX_DOWNLOAD) {
            return Err("muy grande".into());
        }
        let bytes = r.bytes().await.map_err(|e| e.to_string())?;
        if bytes.len() > MAX_DOWNLOAD {
            return Err("muy grande".into());
        }
        Ok(bytes.to_vec())
    })
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
    fn store(&mut self, ctx: &egui::Context, key: PathBuf, img: Option<egui::ColorImage>) {
        let img = img.map(|img| {
            let size = img.size;
            (ctx.load_texture(key.to_string_lossy(), img, egui::TextureOptions::LINEAR), size)
        });
        self.loaded.insert(key, img);
    }

    /// Carga las imágenes y los PDF de la nota que aún no están (una vez cada uno). Los de la
    /// web y las páginas de PDF se cargan aparte, sin detener la ventana.
    pub(super) fn prepare(&mut self, ctx: &egui::Context, note: &Path, text: &str) {
        while let Ok(l) = self.rx.try_recv() {
            self.pending.remove(&l.key);
            if let Some((pdf, n)) = l.pdf {
                self.pdf_pages.insert(pdf.clone(), n);
                if l.image.is_some() {
                    self.pdf_last.insert(pdf, l.key.clone());
                }
            }
            self.store(ctx, l.key, l.image);
        }
        for line in text.lines().filter(|l| l.contains("](")) {
            if let Some((_, rel)) = lines::image_of(line) {
                let path = locate(note, rel);
                if self.loaded.contains_key(&path) || self.pending.contains(&path) {
                    continue;
                }
                if !is_url(rel) {
                    let img = fs::read(&path).ok().and_then(|b| decode_image(&b));
                    self.store(ctx, path, img);
                    continue;
                }
                // De la web: de la caché, o se baja.
                let (url, tx, ctx) = (rel.to_string(), self.tx.clone(), ctx.clone());
                self.pending.insert(path.clone());
                std::thread::spawn(move || {
                    let cache = web_cache(&url);
                    let bytes = fs::read(&cache).ok().or_else(|| {
                        let b = download(&url).ok()?;
                        let _ = fs::create_dir_all(cache.parent()?);
                        let _ = fs::write(&cache, &b);
                        Some(b)
                    });
                    let _ = tx.send(Loaded { key: path, pdf: None, image: bytes.and_then(|b| decode_image(&b)) });
                    ctx.request_repaint();
                });
            } else if let Some((alt, rel)) = lines::pdf_of(line) {
                if lines::shown_width(alt).1 == Some(0) {
                    continue;
                }
                let pdf = locate(note, rel);
                let key = page_key(&pdf, self.page(&pdf));
                if self.loaded.contains_key(&key) || self.pending.contains(&key) {
                    continue;
                }
                let (tx, ctx, page) = (self.tx.clone(), ctx.clone(), self.page(&pdf));
                self.pending.insert(key.clone());
                std::thread::spawn(move || {
                    let done = fs::read(&pdf).ok().and_then(|b| render_pdf_page(b, page));
                    let n = done.as_ref().map(|d| d.1).unwrap_or(0);
                    let _ = tx.send(Loaded { key, pdf: Some((pdf, n)), image: done.map(|d| d.0) });
                    ctx.request_repaint();
                });
            }
        }
    }

    /// La página de un PDF que se ve (desde 0).
    pub(super) fn page(&self, pdf: &Path) -> usize {
        self.pdf_page.get(pdf).copied().unwrap_or(0)
    }

    pub(super) fn pages(&self, pdf: &Path) -> usize {
        self.pdf_pages.get(pdf).copied().unwrap_or(0)
    }

    pub(super) fn set_page(&mut self, pdf: &Path, page: usize) {
        let n = self.pages(pdf);
        if n > 0 {
            self.pdf_page.insert(pdf.to_path_buf(), page.min(n - 1));
        }
    }

    /// La textura que se muestra: la imagen, o la página del PDF (o la anterior mientras carga).
    fn shown(&self, path: &Path) -> Option<&(egui::TextureHandle, [usize; 2])> {
        if !is_pdf(path) {
            return self.loaded.get(path)?.as_ref();
        }
        let key = page_key(path, self.page(path));
        match self.loaded.get(&key) {
            Some(Some(t)) => Some(t),
            _ => self.loaded.get(self.pdf_last.get(path)?)?.as_ref(),
        }
    }

    /// Cómo se muestra (ancho y alto) una imagen o un PDF, en una columna de ancho `max_w`;
    /// `want` = el ancho elegido.
    pub(super) fn shown_size(&self, path: &Path, max_w: f32, want: Option<f32>) -> Option<(f32, f32)> {
        let (_, [w, h]) = self.shown(path)?;
        let (w, h) = (*w as f32, *h as f32);
        let scale = match want {
            Some(want) => want.clamp(48.0, max_w.max(48.0)) / w,
            None if is_pdf(path) => PDF_W.min(max_w) / w,
            None => (max_w / w).min(MAX_H / h).min(1.0),
        };
        Some((w * scale, h * scale))
    }

    pub(super) fn texture(&self, path: &Path) -> Option<egui::TextureId> {
        self.shown(path).map(|(t, _)| t.id())
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
    /// Cambia el ancho con que se ve la imagen o el PDF de la línea `idx` (None = el de siempre).
    pub(super) fn set_shown_width(&mut self, idx: usize, width: Option<u32>) {
        let Some(line) = self.note.text.split('\n').nth(idx).map(|l| l.trim_end_matches('\r').to_string()) else { return };
        let new = lines::with_width(&line, width);
        if new != line {
            self.note.text = editor::replace_line(&self.note.text, idx, &new);
            self.note.dirty = true;
            self.save();
        }
    }

    /// Guarda en Adjuntos la imagen de la web de la línea `idx` (para que no dependa del sitio).
    pub(super) fn keep_web_image(&mut self, idx: usize) {
        let Some(line) = self.note.text.split('\n').nth(idx).map(|l| l.trim_end_matches('\r').to_string()) else { return };
        let Some((_, url)) = lines::image_of(&line).filter(|(_, r)| is_url(r)) else { return };
        let url = url.to_string();
        let Ok(bytes) = fs::read(web_cache(&url)) else {
            self.msg(t!("La imagen aún no se ha bajado"));
            return;
        };
        let ext = match image::guess_format(&bytes) {
            Ok(image::ImageFormat::Jpeg) => "jpg",
            Ok(image::ImageFormat::Gif) => "gif",
            Ok(image::ImageFormat::WebP) => "webp",
            Ok(image::ImageFormat::Bmp) => "bmp",
            _ => "png",
        };
        let last = url.split(['?', '#']).next().unwrap_or("").rsplit('/').next().unwrap_or("");
        let stem = last.rsplit_once('.').map(|(b, _)| b).unwrap_or(last);
        let base: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).take(60).collect();
        let base = if base.trim_matches('-').is_empty() { format!("imagen-web-{}", Local::now().format("%Y-%m-%d-%H%M%S")) } else { base };
        let dir = self.vault.root.join(vault::ATTACHMENTS);
        let mut name = format!("{base}.{ext}");
        let mut i = 2;
        while dir.join(&name).exists() {
            name = format!("{base}-{i}.{ext}");
            i += 1;
        }
        if let Err(e) = fs::create_dir_all(&dir).and_then(|_| fs::write(dir.join(&name), &bytes)) {
            self.msg(tf!("No se pudo guardar la imagen: {e}", e = e));
            return;
        }
        let new = line.replacen(&format!("]({url})"), &format!("](../{}/{name})", vault::ATTACHMENTS), 1);
        self.note.text = editor::replace_line(&self.note.text, idx, &new);
        self.note.dirty = true;
        self.save();
        self.msg(tf!("Imagen guardada en {dir}/{name}", dir = vault::ATTACHMENTS, name = name));
    }

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
            self.msg(t!("No se pudo convertir la imagen"));
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
            self.msg(tf!("No se pudo guardar la imagen: {e}", e = e));
            return true;
        }
        let md = format!("![Captura {} {} {}](../{}/{name})", now.day(), crate::i18n::meses()[now.month0() as usize], now.format("%H:%M"), vault::ATTACHMENTS);
        // Va en la línea del cursor si está vacía, o en una nueva debajo (sin cursor: al final).
        let _ = self.insert_lines_after(line, vec![md]);
        self.msg(tf!("Captura pegada ({w}×{h}), guardada en {dir}/{name}", w = img.width, h = img.height, dir = vault::ATTACHMENTS, name = name));
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
        // Direcciones de imágenes pegadas.
        assert_eq!(pasted_image_url(" https://x.cl/a/foto.JPG?w=2 ").as_deref(), Some("![](https://x.cl/a/foto.JPG?w=2)"));
        assert_eq!(pasted_image_url("https://x.cl/pagina"), None);
        assert_eq!(pasted_image_url("mira https://x.cl/a.png"), None);
        assert_eq!(locate(note, "https://x.cl/a.png"), PathBuf::from("https://x.cl/a.png"));
    }

    /// Un PDF mínimo de dos páginas (la primera, con un cuadrado negro) se dibuja.
    #[test]
    fn renders_pdf_pages() {
        let pdf = tiny_pdf();
        let (img, pages) = render_pdf_page(pdf.clone(), 0).expect("se dibuja");
        assert_eq!(pages, 2);
        assert!(img.size[0] >= 1000 && img.size[1] > img.size[0], "{:?}", img.size);
        assert!(img.pixels.iter().any(|p| p.r() < 40), "tiene algo negro");
        assert!(img.pixels.iter().any(|p| p.r() > 240), "y fondo blanco");
        assert!(render_pdf_page(pdf, 5).is_none());
        assert!(render_pdf_page(b"no es un pdf".to_vec(), 0).is_none());
    }

    /// Arma un PDF de dos páginas carta, con un cuadrado relleno en la primera.
    pub(in crate::app) fn tiny_pdf() -> Vec<u8> {
        let content = "0 0 0 rg 100 100 200 200 re f";
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_string(),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).bytes());
        for off in offsets {
            out.extend(format!("{off:010} 00000 n \n").bytes());
        }
        out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).bytes());
        out
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

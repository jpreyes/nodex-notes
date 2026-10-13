//! Guardar una página web como nota: se baja, se queda solo con el artículo (sin menús, avisos
//! ni comentarios) y se pasa a Markdown, con sus imágenes como `![](https://…)`.

use std::time::Duration;

/// Lo más grande que se baja.
const MAX_PAGE: usize = 10 * 1024 * 1024;

/// Una página lista para guardar.
#[derive(Debug, PartialEq)]
pub struct Clip {
    pub title: String,
    /// El artículo en Markdown.
    pub body: String,
}

/// La dirección escrita, completa («www.x.cl» -> «https://www.x.cl»); None si no parece una.
pub fn normalize(url: &str) -> Option<String> {
    let u = url.trim();
    if u.is_empty() || u.contains(char::is_whitespace) {
        return None;
    }
    let full = if u.starts_with("http://") || u.starts_with("https://") { u.to_string() } else { format!("https://{u}") };
    let host = full.split("://").nth(1)?.split(['/', '?', '#']).next()?;
    host.contains('.').then_some(full)
}

/// La primera dirección web de una línea (para ofrecer guardarla).
pub fn url_in(line: &str) -> Option<String> {
    line.split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == '<' || c == '>')
        .find(|w| w.starts_with("http://") || w.starts_with("https://") || w.starts_with("www."))
        .and_then(normalize)
}

/// Saca el artículo de una página.
pub fn clip(html: &str, url: &str) -> Result<Clip, String> {
    let mut r = dom_smoothie::Readability::new(html, Some(url), None).map_err(|e| e.to_string())?;
    let a = r.parse().map_err(|e| e.to_string())?;
    let md = htmd::convert(&a.content).map_err(|e| e.to_string())?;
    let title = a.title.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = if title.is_empty() { url.split("://").nth(1).unwrap_or(url).split('/').next().unwrap_or(url).to_string() } else { title };
    Ok(Clip { title, body: tidy(&fix_links(&md)) })
}

/// Enlaces que la nota sabe mostrar: la imagen enlazada queda como imagen; sin el «título» de
/// los enlaces, con los paréntesis de la dirección codificados y sin las anclas de la misma
/// página (las citas «[2]» se van).
fn fix_links(md: &str) -> String {
    use regex::Regex;
    use std::sync::LazyLock;
    static LINKED_IMAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\s*(!\[[^\]]*\]\([^)\s]*\))\s*\]\([^)]*\)").unwrap());
    static DEST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\]\(((?:[^)\s\\]|\\.)*)(?:\s+"[^"]*")?\)"#).unwrap());
    static CITE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\\\[[^\]]*\]\]\(#[^)]*\)").unwrap());
    static ANCHOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([^\]]*)\]\(#[^)]*\)").unwrap());
    let s = LINKED_IMAGE.replace_all(md, "$1");
    let s = DEST.replace_all(&s, |c: &regex::Captures| format!("]({})", c[1].replace("\\(", "%28").replace("\\)", "%29")));
    let s = CITE.replace_all(&s, "");
    ANCHOR.replace_all(&s, "$1").into_owned()
}

/// Markdown más limpio: sin espacios al final, sin líneas en blanco de más y con cada imagen en
/// su línea (así se ve en la nota).
fn tidy(md: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for raw in md.lines() {
        let line = raw.trim_end();
        if !line.contains("![") || crate::lines::image_of(line).is_some() {
            out.push(line.to_string());
            continue;
        }
        // Una imagen metida en un párrafo va a su propia línea.
        let mut rest = line;
        while let Some(i) = rest.find("![") {
            let Some(end) = rest[i..].find(')').map(|e| i + e + 1) else { break };
            let before = rest[..i].trim();
            if !before.is_empty() {
                out.push(before.to_string());
            }
            out.push(rest[i..end].to_string());
            rest = rest[end..].trim_start();
        }
        if !rest.trim().is_empty() {
            out.push(rest.to_string());
        }
    }
    let mut text = String::new();
    let mut blank = true;
    for l in out {
        if l.trim().is_empty() {
            if !blank {
                text.push('\n');
            }
            blank = true;
        } else {
            text += &l;
            text.push('\n');
            blank = false;
        }
    }
    text.trim_end().to_string() + "\n"
}

/// Baja la página.
pub fn fetch(url: &str) -> Result<String, String> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    let client = reqwest::Client::builder().timeout(Duration::from_secs(30)).build().map_err(|e| e.to_string())?;
    rt.block_on(async {
        let r = client
            .get(url)
            // Algunos sitios no responden sin un navegador conocido.
            .header("User-Agent", format!("Mozilla/5.0 (compatible; nodex-notes/{})", env!("CARGO_PKG_VERSION")))
            .header("Accept", "text/html,application/xhtml+xml")
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !r.status().is_success() {
            return Err(tf!("el sitio respondió {status}", status = r.status()));
        }
        let kind = r.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_lowercase();
        if !kind.is_empty() && !kind.contains("html") {
            return Err(tf!("no es una página web ({kind})", kind = kind));
        }
        let bytes = r.bytes().await.map_err(|e| e.to_string())?;
        if bytes.len() > MAX_PAGE {
            return Err(t!("la página es muy grande").into());
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_article() {
        let html = r#"<!doctype html><html><head><title>Cómo curar el hormigón | Blog de Obra</title></head><body>
            <nav><a href="/">Inicio</a> <a href="/blog">Blog</a> <a href="/contacto">Contacto</a></nav>
            <div class="ads">Compre ahora, oferta limitada</div>
            <article>
              <h1>Cómo curar el hormigón</h1>
              <p>El curado mantiene la humedad del hormigón durante los primeros días, cuando gana la mayor parte de su resistencia. Sin curado, la superficie se seca, se fisura y queda más débil de lo calculado.</p>
              <p>Hay tres formas comunes: mojar con agua, cubrir con láminas o aplicar una membrana de curado. <img src="/img/curado.jpg" alt="Losa cubierta"> La elección depende del clima y del tipo de elemento.</p>
              <h2>Cuánto tiempo</h2>
              <ul><li>Siete días para cemento corriente.</li><li>Catorce días si hace calor o viento.</li></ul>
              <p>Más detalles en la <a href="https://www.ich.cl/norma">norma chilena</a>, que fija los mínimos para cada caso y explica cómo comprobar que el curado fue suficiente.</p>
            </article>
            <footer>© 2026 Blog de Obra · Todos los derechos reservados</footer>
        </body></html>"#;
        let c = clip(html, "https://obra.cl/blog/curado").unwrap();
        assert!(c.title.starts_with("Cómo curar el hormigón"), "{}", c.title);
        assert!(c.body.contains("El curado mantiene la humedad"), "{}", c.body);
        assert!(c.body.contains("Cuánto tiempo"));
        assert!(c.body.contains("Siete días"));
        assert!(c.body.contains("[norma chilena](https://www.ich.cl/norma)"));
        assert!(!c.body.contains("Compre ahora") && !c.body.contains("Todos los derechos"), "{}", c.body);
        // La imagen, con su dirección completa y en su propia línea.
        assert!(c.body.lines().any(|l| l.trim() == "![Losa cubierta](https://obra.cl/img/curado.jpg)"), "{}", c.body);
        assert!(!c.body.contains("\n\n\n"));
    }

    #[test]
    fn addresses() {
        assert_eq!(normalize("www.obra.cl/x").as_deref(), Some("https://www.obra.cl/x"));
        assert_eq!(normalize("https://obra.cl").as_deref(), Some("https://obra.cl"));
        assert_eq!(normalize("hola"), None);
        assert_eq!(normalize("dos palabras.cl"), None);
        assert_eq!(url_in("Leer esto (https://obra.cl/a) mañana").as_deref(), Some("https://obra.cl/a"));
        assert_eq!(url_in("[Blog](https://obra.cl/a)").as_deref(), Some("https://obra.cl/a"));
        assert_eq!(url_in("nada aquí"), None);
        assert_eq!(tidy("a  \n\n\n\nb ![x](u.png) c\n"), "a\n\nb\n![x](u.png)\nc\n");
        // Lo que deja Wikipedia.
        let md = r#"[
![](https://up.org/a.jpg?x=1)
](https://es.org/Archivo:a.jpg)

El **hormigón**[\[2\]](#cite_note-2) del [arco](https://es.org/Arco_\(arquitectura\) "Arco (arquitectura)") y [ver abajo](#usos)."#;
        assert_eq!(
            fix_links(md),
            "![](https://up.org/a.jpg?x=1)\n\nEl **hormigón** del [arco](https://es.org/Arco_%28arquitectura%29) y ver abajo."
        );
    }
}



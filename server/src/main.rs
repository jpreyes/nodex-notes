//! `nodex-ia`: el servidor de la IA incluida de Notas.
//!
//! ```text
//! nodex-ia servir                       atiende en NOTAS_IA_DIRECCION:NOTAS_IA_PUERTO (127.0.0.1:8080)
//! nodex-ia nuevo "Ana Pérez" [tokens]   crea una persona y muestra su código (una sola vez)
//! nodex-ia lista                        uso del mes de cada persona y costo estimado
//! nodex-ia desactivar <huella>          deja sin IA a una persona (las primeras letras de su huella)
//! nodex-ia plan <correo> <plan> [días]   cambia el plan de una cuenta (prueba, pro, fundador, gratis)
//! ```
//!
//! Configuración (variables de entorno): NOTAS_IA_KEY (clave de la IA, obligatoria para servir),
//! NOTAS_IA_UPSTREAM, NOTAS_IA_MODEL, NOTAS_IA_DATA (carpeta de datos), NOTAS_IA_LIMITE (tokens al
//! mes por persona), NOTAS_IA_PRECIO_ENTRADA y NOTAS_IA_PRECIO_SALIDA (USD por millón de tokens).

use nodex_ia::{Settings, Store, report, router, state};

fn usage() -> ! {
    eprintln!("uso: nodex-ia servir | nuevo \"Nombre\" [tokens al mes] | lista | desactivar <huella> | plan <correo> <plan> [días]");
    std::process::exit(2)
}

#[tokio::main]
async fn main() {
    let settings = Settings::from_env();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("servir") => {
            if settings.key.is_empty() {
                eprintln!("Falta NOTAS_IA_KEY (la clave de la IA del servicio)");
                std::process::exit(1);
            }
            let port = std::env::var("NOTAS_IA_PUERTO").ok().and_then(|p| p.parse::<u16>().ok()).unwrap_or(8080);
            // Detrás de Caddy (que pone el HTTPS) basta con escuchar en este mismo equipo.
            let host = std::env::var("NOTAS_IA_DIRECCION").ok().filter(|h| !h.is_empty()).unwrap_or_else(|| "127.0.0.1".into());
            let listener = tokio::net::TcpListener::bind((host.as_str(), port)).await.expect("no se pudo abrir el puerto");
            println!("nodex-ia en el puerto {port} · modelo {} · datos en {}", settings.model, settings.data.display());
            axum::serve(listener, router(state(settings)))
                .with_graceful_shutdown(async {
                    let _ = tokio::signal::ctrl_c().await;
                })
                .await
                .expect("el servidor se detuvo con un error");
        }
        Some("nuevo") => {
            let Some(name) = args.get(1) else { usage() };
            let limit = args.get(2).and_then(|l| l.parse().ok()).unwrap_or(0);
            let mut store = Store::load(&settings.data);
            let code = store.add_user(name, limit);
            store.save_users(&settings.data).expect("no se pudo guardar usuarios.json");
            println!("{name}: {code}");
            println!("(El código se muestra solo esta vez. En la app: Configuración → Inteligencia artificial → IA incluida.)");
        }
        Some("lista") => print!("{}", report(&settings)),
        Some("plan") => {
            // nodex-ia plan ana@correo.cl pro   (prueba, pro, fundador o gratis)
            let (Some(correo), Some(plan)) = (args.get(1), args.get(2)) else { usage() };
            if !["prueba", "pro", "fundador", "gratis"].contains(&plan.as_str()) {
                eprintln!("Plan desconocido «{plan}»: usa prueba, pro, fundador o gratis");
                std::process::exit(1);
            }
            let mut accounts = nodex_ia::accounts::Accounts::load(&settings.data);
            let Some(id) = accounts.by_email(correo).map(|a| a.id.clone()) else {
                eprintln!("No hay una cuenta con el correo {correo}");
                std::process::exit(1);
            };
            if let Some(a) = accounts.cuentas.get_mut(&id) {
                a.plan = plan.clone();
                if plan == "prueba" {
                    let days = args.get(3).and_then(|d| d.parse::<i64>().ok()).unwrap_or(settings.trial_days);
                    a.prueba_hasta = (chrono::Local::now().date_naive() + chrono::Duration::days(days)).format("%Y-%m-%d").to_string();
                }
                println!("{correo}: plan {plan}");
            }
            accounts.save(&settings.data).expect("no se pudo guardar cuentas.json");
        }
        Some("desactivar") => {
            let Some(prefix) = args.get(1) else { usage() };
            let mut store = Store::load(&settings.data);
            let matches: Vec<String> = store.users.keys().filter(|k| k.starts_with(prefix.as_str())).cloned().collect();
            if matches.len() != 1 {
                eprintln!("La huella «{prefix}» calza con {} personas; usa más letras", matches.len());
                std::process::exit(1);
            }
            if let Some(u) = store.users.get_mut(&matches[0]) {
                u.activo = false;
                println!("{} ya no tiene IA incluida", u.nombre);
            }
            store.save_users(&settings.data).expect("no se pudo guardar usuarios.json");
        }
        _ => usage(),
    }
}

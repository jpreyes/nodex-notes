//! Notas en segundo plano (Windows). Al cerrar la ventana, Notas sigue funcionando con su ícono
//! junto al reloj: sincroniza tus notas y sigue con lo suyo. Un clic en el ícono la vuelve a
//! mostrar; «Salir de Notas» (clic derecho en el ícono) la cierra de verdad. Puede abrirse sola
//! al iniciar Windows, ya en segundo plano.
//!
//! Hay una sola Notas a la vez: la primera escucha en un puerto local (solo de este equipo); si se
//! abre otra, le pide que se muestre y no abre una segunda.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

/// Lo que responde una Notas cuando otra le pide que se muestre.
const HELLO: &str = "notas";
/// Lo que se agrega al abrirse con Windows: empieza escondida.
pub const HIDDEN_ARG: &str = "--segundo-plano";

/// El puerto de esta Notas. Depende de la carpeta de configuración, para que una de prueba
/// (NODEX_CONFIG_DIR) no se confunda con la de verdad.
fn port() -> u16 {
    let key = crate::config::config_path().to_string_lossy().to_lowercase();
    20000 + (crate::ai::fnv(&key) % 20000) as u16
}

pub enum Instance {
    /// Esta es la primera (con su puerto, si se pudo abrir).
    First(Option<TcpListener>),
    /// Ya había una: se le pidió que se mostrara.
    Shown,
}

/// ¿Ya hay una Notas abierta? Si la hay, se le pide que se muestre. Recién actualizada, la
/// anterior puede tardar unos segundos en soltar el puerto: se espera.
pub fn single_instance(after_update: bool) -> Instance {
    let until = Instant::now() + Duration::from_secs(if after_update { 15 } else { 0 });
    loop {
        if let Ok(l) = TcpListener::bind(("127.0.0.1", port())) {
            return Instance::First(Some(l));
        }
        if !after_update && ask_to_show() {
            return Instance::Shown;
        }
        if Instant::now() >= until {
            // Otro programa usa ese puerto: se sigue sin esto.
            return Instance::First(None);
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// Le pide a la Notas abierta que se muestre. `true` si respondió.
fn ask_to_show() -> bool {
    let Ok(mut s) = TcpStream::connect_timeout(&([127, 0, 0, 1], port()).into(), Duration::from_secs(1)) else { return false };
    let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
    if s.write_all(b"mostrar\n").is_err() {
        return false;
    }
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).is_ok() && line.trim() == HELLO
}

/// Atiende a las Notas que se abren después: se muestra esta.
fn listen(listener: TcpListener, show: impl Fn() + Send + 'static) {
    std::thread::Builder::new()
        .name("una-sola-notas".into())
        .spawn(move || {
            for s in listener.incoming().flatten() {
                let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
                let mut line = String::new();
                let mut r = BufReader::new(&s);
                if r.read_line(&mut line).is_ok() && line.trim() == "mostrar" {
                    show();
                    let _ = (&s).write_all(format!("{HELLO}\n").as_bytes());
                }
            }
        })
        .ok();
}

#[cfg(windows)]
pub use win::Background;

#[cfg(windows)]
mod win {
    use super::*;
    use eframe::egui;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, PostMessageW, SW_HIDE, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindow, WM_CLOSE};

    pub struct Background {
        hwnd: isize,
        _tray: Option<TrayIcon>,
        quit: Arc<AtomicBool>,
    }

    fn show(hwnd: isize, ctx: &egui::Context) {
        let h = hwnd as windows_sys::Win32::Foundation::HWND;
        unsafe {
            ShowWindow(h, if IsIconic(h) != 0 { SW_RESTORE } else { SW_SHOW });
            SetForegroundWindow(h);
        }
        ctx.request_repaint();
    }

    fn icon() -> Option<tray_icon::Icon> {
        let img = image::load_from_memory(include_bytes!("../assets/icon-256.png")).ok()?;
        let small = img.resize_exact(32, 32, image::imageops::FilterType::Lanczos3).to_rgba8();
        tray_icon::Icon::from_rgba(small.into_raw(), 32, 32).ok()
    }

    impl Background {
        /// Pone el ícono junto al reloj y atiende a las Notas que se abran después.
        pub fn start(cc: &eframe::CreationContext<'_>, listener: Option<TcpListener>) -> Option<Background> {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            let RawWindowHandle::Win32(h) = cc.window_handle().ok()?.as_raw() else { return None };
            let hwnd = h.hwnd.get();
            let ctx = cc.egui_ctx.clone();
            let quit = Arc::new(AtomicBool::new(false));
            if let Some(l) = listener {
                let ctx = ctx.clone();
                listen(l, move || show(hwnd, &ctx));
            }
            let open = MenuItem::new("Abrir Notas", true, None);
            let exit = MenuItem::new("Salir de Notas", true, None);
            let menu = Menu::new();
            let _ = menu.append(&open);
            let _ = menu.append(&PredefinedMenuItem::separator());
            let _ = menu.append(&exit);
            let tray = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_menu_on_left_click(false)
                .with_tooltip("Notas · sigue sincronizando tus notas")
                .with_icon(icon()?)
                .build()
                .ok();
            {
                let ctx = ctx.clone();
                TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| match e {
                    TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } | TrayIconEvent::DoubleClick { .. } => {
                        show(hwnd, &ctx)
                    }
                    _ => {}
                }));
            }
            {
                let (open, exit, quit) = (open.id().clone(), exit.id().clone(), quit.clone());
                MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                    if e.id == open {
                        show(hwnd, &ctx);
                    } else if e.id == exit {
                        // Se muestra para que termine de guardar y se cierre como siempre.
                        quit.store(true, Ordering::SeqCst);
                        show(hwnd, &ctx);
                        unsafe { PostMessageW(hwnd as windows_sys::Win32::Foundation::HWND, WM_CLOSE, 0, 0) };
                    }
                }));
            }
            Some(Background { hwnd, _tray: tray, quit })
        }

        /// Esconde la ventana (Notas sigue funcionando).
        pub fn hide(&self) {
            unsafe { ShowWindow(self.hwnd as windows_sys::Win32::Foundation::HWND, SW_HIDE) };
        }

        /// Se pidió salir de verdad (desde el ícono o para actualizarse).
        pub fn quitting(&self) -> bool {
            self.quit.load(Ordering::SeqCst)
        }

        pub fn set_quitting(&self) {
            self.quit.store(true, Ordering::SeqCst);
        }
    }
}

/// En Mac y Linux no hay segundo plano todavía: al cerrar, se cierra.
#[cfg(not(windows))]
pub struct Background;

#[cfg(not(windows))]
#[allow(dead_code)]
impl Background {
    pub fn start(cc: &eframe::CreationContext<'_>, listener: Option<TcpListener>) -> Option<Background> {
        let ctx = cc.egui_ctx.clone();
        if let Some(l) = listener {
            listen(l, move || ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Focus));
        }
        None
    }
    pub fn hide(&self) {}
    pub fn quitting(&self) -> bool {
        true
    }
    pub fn set_quitting(&self) {}
}

/// ¿Notas se abre sola al iniciar Windows?
#[cfg(windows)]
pub fn starts_with_windows() -> bool {
    run_key(&["query", RUN_KEY, "/v", "Notas"]).is_some_and(|out| out.contains(HIDDEN_ARG))
}

/// Que Notas se abra (o no) sola al iniciar Windows, ya en segundo plano.
#[cfg(windows)]
pub fn set_start_with_windows(on: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let ok = if on {
        let value = format!("\"{}\" {HIDDEN_ARG}", exe.display());
        run_key(&["add", RUN_KEY, "/v", "Notas", "/t", "REG_SZ", "/d", &value, "/f"]).is_some()
    } else {
        run_key(&["delete", RUN_KEY, "/v", "Notas", "/f"]).is_some() || !starts_with_windows()
    };
    if ok { Ok(()) } else { Err("Windows no lo permitió".into()) }
}

#[cfg(windows)]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

/// `reg.exe` sin ventana; la salida si terminó bien.
#[cfg(windows)]
fn run_key(args: &[&str]) -> Option<String> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("reg").args(args).creation_flags(0x0800_0000).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(not(windows))]
pub fn starts_with_windows() -> bool {
    false
}

#[cfg(not(windows))]
pub fn set_start_with_windows(_on: bool) -> Result<(), String> {
    Err("solo en Windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Una sola Notas: la segunda le pide a la primera que se muestre y no abre otra.
    #[test]
    fn second_instance_shows_the_first() {
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        let Instance::First(Some(l)) = single_instance(false) else { panic!("la primera abre el puerto") };
        let (tx, rx) = std::sync::mpsc::channel();
        listen(l, move || {
            let _ = tx.send(());
        });
        assert!(matches!(single_instance(false), Instance::Shown));
        assert!(rx.recv_timeout(Duration::from_secs(3)).is_ok(), "la primera se mostró");
    }
}

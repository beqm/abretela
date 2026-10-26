//! Ícone na bandeja do sistema e indicador de "ao vivo" (bandeja + barra de tarefas).

use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, Wry,
};

const TRAY_ID: &str = "main";

pub struct Tray {
    open: MenuItem<Wry>,
    quit: MenuItem<Wry>,
    pub close_to_tray: AtomicBool,
    live: AtomicBool,
    idle_tooltip: std::sync::Mutex<(String, String)>, // (normal, ao vivo)
}

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Abrir AbreTela", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Sair", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(app.default_window_icon().cloned().expect("ícone padrão"))
        .tooltip("AbreTela")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;

    app.manage(Tray {
        open,
        quit,
        close_to_tray: AtomicBool::new(true),
        live: AtomicBool::new(false),
        idle_tooltip: std::sync::Mutex::new(("AbreTela".into(), "AbreTela".into())),
    });
    Ok(())
}

/// Textos traduzidos do menu e das dicas da bandeja.
pub fn set_texts(app: &AppHandle, open: &str, quit: &str, tooltip: &str, live_tooltip: &str) {
    let tray = app.state::<Tray>();
    let _ = tray.open.set_text(open);
    let _ = tray.quit.set_text(quit);
    *tray.idle_tooltip.lock().unwrap() = (tooltip.into(), live_tooltip.into());
    refresh_tooltip(app);
}

fn refresh_tooltip(app: &AppHandle) {
    let tray = app.state::<Tray>();
    let (idle, live) = tray.idle_tooltip.lock().unwrap().clone();
    let text = if tray.live.load(Ordering::Relaxed) { live } else { idle };
    if let Some(icon) = app.tray_by_id(TRAY_ID) {
        let _ = icon.set_tooltip(Some(text));
    }
}

/// Liga/desliga o indicador de transmissão: bolinha vermelha no ícone da
/// bandeja e selo sobre o ícone na barra de tarefas.
pub fn set_live(app: &AppHandle, live: bool) {
    let tray = app.state::<Tray>();
    tray.live.store(live, Ordering::Relaxed);

    let base = app.default_window_icon().cloned().expect("ícone padrão");
    if let Some(icon) = app.tray_by_id(TRAY_ID) {
        let image = if live { with_live_dot(&base) } else { base };
        let _ = icon.set_icon(Some(image));
    }
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_overlay_icon(live.then(live_badge));
    }
    refresh_tooltip(app);
}

const RED: [u8; 3] = [0xFF, 0x4D, 0x6D];

/// Desenha um círculo vermelho com borda branca (anti-aliasing simples).
fn draw_dot(rgba: &mut [u8], w: u32, h: u32, cx: f32, cy: f32, r: f32, ring: f32) {
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            let outer = (r + ring + 0.5 - d).clamp(0.0, 1.0);
            if outer <= 0.0 {
                continue;
            }
            let inner = (r + 0.5 - d).clamp(0.0, 1.0);
            let color = [
                RED[0] as f32 * inner + 255.0 * (1.0 - inner),
                RED[1] as f32 * inner + 255.0 * (1.0 - inner),
                RED[2] as f32 * inner + 255.0 * (1.0 - inner),
            ];
            let i = ((y * w + x) * 4) as usize;
            let a = outer;
            for c in 0..3 {
                rgba[i + c] = (color[c] * a + rgba[i + c] as f32 * (1.0 - a)) as u8;
            }
            rgba[i + 3] = (255.0 * a + rgba[i + 3] as f32 * (1.0 - a)) as u8;
        }
    }
}

fn with_live_dot(base: &Image<'_>) -> Image<'static> {
    let (w, h) = (base.width(), base.height());
    let mut rgba = base.rgba().to_vec();
    let r = w as f32 * 0.2;
    draw_dot(&mut rgba, w, h, w as f32 - r - w as f32 * 0.06, h as f32 - r - h as f32 * 0.06, r, w as f32 * 0.06);
    Image::new_owned(rgba, w, h)
}

fn live_badge() -> Image<'static> {
    let size = 32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    draw_dot(&mut rgba, size, size, 16.0, 16.0, 11.0, 3.0);
    Image::new_owned(rgba, size, size)
}

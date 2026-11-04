//! Abertura no estilo do Discord: uma janelinha (splash) mostra o mascote e
//! procura atualizações enquanto a janela principal carrega escondida. A
//! principal só aparece quando as duas coisas terminam.
//!
//! Aberto junto com o Windows (`--minimized`), a janelinha roda escondida: a
//! atualização acontece do mesmo jeito e o app continua só na bandeja.

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

const SPLASH: &str = "splash";
/// Se a janelinha não terminar nesse tempo (e não estiver baixando uma
/// atualização), o app abre mesmo assim.
const SPLASH_TIMEOUT: Duration = Duration::from_secs(45);
/// Depois da janelinha terminar, quanto esperar a principal ficar pronta.
const MAIN_TIMEOUT: Duration = Duration::from_secs(15);

struct Startup {
    minimized: bool,
    splash_done: AtomicBool,
    main_ready: AtomicBool,
    updating: AtomicBool,
    finished: AtomicBool,
}

pub fn setup(app: &AppHandle, minimized: bool) -> tauri::Result<()> {
    app.manage(Startup {
        minimized,
        splash_done: AtomicBool::new(false),
        main_ready: AtomicBool::new(false),
        updating: AtomicBool::new(false),
        finished: AtomicBool::new(false),
    });

    let mut splash = WebviewWindowBuilder::new(app, SPLASH, WebviewUrl::App("splash.html".into()));
    // o WebView2 é compartilhado e recusa janelas com opções de inicialização diferentes
    if let Some(args) = app.config().app.windows.iter().find_map(|w| w.additional_browser_args.clone()) {
        splash = splash.additional_browser_args(&args);
    }
    splash
        .title("AbreTela")
        .inner_size(240.0, 240.0)
        .resizable(false)
        .maximizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .center()
        .visible(!minimized)
        .build()?;

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(SPLASH_TIMEOUT).await;
        let state = app.state::<Startup>();
        if !state.updating.load(Ordering::Relaxed) && !state.finished.load(Ordering::Relaxed) {
            log_warn!("app", "The splash screen took too long, opening the app anyway");
            finish(&app);
        }
    });
    Ok(())
}

/// A janelinha está baixando uma atualização (não fecha por tempo).
#[tauri::command]
pub fn splash_status(app: AppHandle, updating: bool) {
    app.state::<Startup>().updating.store(updating, Ordering::Relaxed);
}

/// A janelinha terminou (sem atualização, ou ela falhou).
#[tauri::command]
pub fn splash_done(app: AppHandle) {
    app.state::<Startup>().splash_done.store(true, Ordering::Relaxed);
    try_finish(&app);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(MAIN_TIMEOUT).await;
        if !app.state::<Startup>().finished.load(Ordering::Relaxed) {
            log_warn!("app", "The main window took too long to load, showing it anyway");
            finish(&app);
        }
    });
}

/// A janela principal carregou.
#[tauri::command]
pub fn app_ready(app: AppHandle) {
    app.state::<Startup>().main_ready.store(true, Ordering::Relaxed);
    try_finish(&app);
}

fn try_finish(app: &AppHandle) {
    let state = app.state::<Startup>();
    if state.splash_done.load(Ordering::Relaxed) && state.main_ready.load(Ordering::Relaxed) {
        finish(app);
    }
}

fn finish(app: &AppHandle) {
    let state = app.state::<Startup>();
    if state.finished.swap(true, Ordering::Relaxed) {
        return;
    }
    log_info!("app", "Startup finished{}", if state.minimized { " (staying in the tray)" } else { ", showing the app" });
    if !state.minimized {
        super::tray::show_main(app);
    }
    if let Some(splash) = app.get_webview_window(SPLASH) {
        let _ = splash.close();
    }
}

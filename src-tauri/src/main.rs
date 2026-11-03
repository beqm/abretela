#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[macro_use]
mod logger;
mod capture;
mod server;
mod tray;
mod tunnel;
#[cfg(test)]
mod e2e_test;

use serde::Serialize;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Manager, RunEvent, State, WindowEvent};

struct Room {
    stop: tokio::sync::watch::Sender<bool>,
    tunnel: tokio::process::Child,
}

#[derive(Default)]
struct AppState {
    room: Mutex<Option<Room>>,
}

impl AppState {
    fn close(&self) {
        if let Some(mut room) = self.room.lock().unwrap().take() {
            let _ = room.stop.send(true);
            let _ = room.tunnel.start_kill();
        }
    }
}

#[derive(Serialize)]
struct RoomInfo {
    port: u16,
    code: String,
    password: String,
}

/// Senha numérica de 6 dígitos.
fn random_password() -> String {
    use std::hash::{BuildHasher, RandomState};
    let n = RandomState::new().hash_one(std::time::SystemTime::now()) % 1_000_000;
    format!("{n:06}")
}

#[tauri::command]
async fn create_room(
    state: State<'_, AppState>,
    settings: server::RoomSettings,
    password: Option<String>,
) -> Result<RoomInfo, String> {
    state.close();

    let password = password
        .map(|p| p.trim().chars().take(32).collect::<String>())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(random_password);
    log_info!(
        "room",
        "Creating a room (quality limits: {}p-{}p, {}-{} fps)",
        settings.min_height,
        settings.max_height,
        settings.min_fps,
        settings.max_fps
    );
    let server = server::start(settings, password.clone()).await.map_err(|e| {
        log_error!("room", "Could not start the local room server: {e}");
        format!("falha ao iniciar servidor: {e}")
    })?;
    let (tunnel, url) = match tunnel::start(server.port).await {
        Ok(t) => t,
        Err(e) => {
            log_error!("room", "Could not open the internet tunnel: {e}");
            let _ = server.stop.send(true);
            return Err(e);
        }
    };
    log_info!("room", "Room is online at {url}");

    let code = url
        .trim_start_matches("https://")
        .trim_end_matches(".trycloudflare.com")
        .to_string();
    *state.room.lock().unwrap() = Some(Room { stop: server.stop, tunnel });
    Ok(RoomInfo { port: server.port, code, password })
}

#[tauri::command]
fn close_room(state: State<'_, AppState>) {
    log_info!("room", "Closed the room");
    state.close();
}

/// Linhas de log vindas do frontend.
#[tauri::command]
fn log(level: String, area: String, message: String) {
    logger::write(&level, &area, &message);
}

#[tauri::command]
async fn list_sources() -> Vec<capture::Source> {
    tokio::task::spawn_blocking(capture::list_sources).await.unwrap_or_default()
}

#[tauri::command]
async fn thumbnail(id: String) -> Option<String> {
    tokio::task::spawn_blocking(move || capture::thumbnail(&id)).await.ok().flatten()
}

/// Retorna a URL do WebSocket local que entrega os frames.
#[tauri::command]
async fn start_capture(id: String, height: u32, fps: u32, audio: bool) -> Result<String, String> {
    tokio::task::spawn_blocking(move || capture::start(&id, height, fps, audio))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn stop_capture(token: Option<String>) {
    let _ = tokio::task::spawn_blocking(move || capture::stop_if(token.as_deref())).await;
}

#[tauri::command]
fn set_close_to_tray(app: AppHandle, enabled: bool) {
    app.state::<tray::Tray>().close_to_tray.store(enabled, Ordering::Relaxed);
}

#[tauri::command]
fn set_live(app: AppHandle, live: bool) {
    tray::set_live(&app, live);
}

#[tauri::command]
fn set_tray_texts(app: AppHandle, open: String, quit: String, tooltip: String, live_tooltip: String) {
    tray::set_texts(&app, &open, &quit, &tooltip, &live_tooltip);
}

fn main() {
    tauri::Builder::default()
        // abrir o app de novo só traz a janela que já está aberta (ou na bandeja)
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| tray::show_main(app)))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .setup(|app| {
            let fallback = app.path().app_log_dir().unwrap_or_else(|_| std::env::temp_dir().join("AbreTela"));
            logger::init(
                fallback,
                &format!(
                    "AbreTela v{} started · {} · WebView2 {}",
                    app.package_info().version,
                    logger::windows_version(),
                    tauri::webview_version().unwrap_or_else(|_| "unknown".into()),
                ),
            );
            tauri::async_runtime::spawn(async {
                if let Err(e) = capture::serve().await {
                    log_error!("capture", "The local capture server stopped: {e}");
                }
            });
            tray::setup(app.handle())?;
            // iniciado junto com o Windows: começa escondido na bandeja
            if std::env::args().any(|a| a == "--minimized") {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.state::<tray::Tray>().close_to_tray.load(Ordering::Relaxed) {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            create_room,
            close_room,
            list_sources,
            thumbnail,
            start_capture,
            stop_capture,
            set_close_to_tray,
            set_live,
            set_tray_texts,
            log
        ])
        .build(tauri::generate_context!())
        .expect("erro ao iniciar o app")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                log_info!("app", "App closed");
                capture::stop();
                app.state::<AppState>().close();
            }
        });
}

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod server;
mod tunnel;
#[cfg(test)]
mod e2e_test;

use serde::Serialize;
use std::sync::Mutex;
use tauri::{Manager, RunEvent, State};

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
    let server = server::start(settings, password.clone())
        .await
        .map_err(|e| format!("falha ao iniciar servidor: {e}"))?;
    let (tunnel, url) = match tunnel::start(server.port).await {
        Ok(t) => t,
        Err(e) => {
            let _ = server.stop.send(true);
            return Err(e);
        }
    };

    let code = url
        .trim_start_matches("https://")
        .trim_end_matches(".trycloudflare.com")
        .to_string();
    *state.room.lock().unwrap() = Some(Room { stop: server.stop, tunnel });
    Ok(RoomInfo { port: server.port, code, password })
}

#[tauri::command]
fn close_room(state: State<'_, AppState>) {
    state.close();
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

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .setup(|_| {
            tauri::async_runtime::spawn(async {
                if let Err(e) = capture::serve().await {
                    eprintln!("servidor de captura falhou: {e}");
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            create_room,
            close_room,
            list_sources,
            thumbnail,
            start_capture,
            stop_capture
        ])
        .build(tauri::generate_context!())
        .expect("erro ao iniciar o app")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                capture::stop();
                app.state::<AppState>().close();
            }
        });
}

//! Comandos do app para a sala: criar (servidor local + túnel) e fechar.

use super::{invite_page::InvitePage, server};
use crate::tunnel;
use serde::Serialize;
use std::sync::Mutex;
use tauri::State;

/// A sala aberta por este PC (no máximo uma).
struct OpenRoom {
    stop: tokio::sync::watch::Sender<bool>,
    tunnel: Box<dyn tunnel::Tunnel>,
}

#[derive(Default)]
pub struct RoomState {
    room: Mutex<Option<OpenRoom>>,
}

impl RoomState {
    /// Fecha a sala aberta, se houver (servidor e túnel).
    pub fn close(&self) {
        if let Some(mut room) = self.room.lock().unwrap().take() {
            let _ = room.stop.send(true);
            room.tunnel.close();
        }
    }
}

#[derive(Serialize)]
pub struct RoomInfo {
    port: u16,
    /// URL https pública da sala (o convite é ela + "#senha").
    url: String,
    password: String,
}

/// Senha numérica de 6 dígitos.
fn random_password() -> String {
    use std::hash::{BuildHasher, RandomState};
    let n = RandomState::new().hash_one(std::time::SystemTime::now()) % 1_000_000;
    format!("{n:06}")
}

#[tauri::command]
pub async fn create_room(
    state: State<'_, RoomState>,
    settings: server::RoomSettings,
    password: Option<String>,
    host_name: Option<String>,
    lang: Option<String>,
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
    let page = InvitePage {
        host_name: host_name.unwrap_or_default().trim().chars().take(32).collect(),
        lang: lang.unwrap_or_default(),
    };
    let server = server::start(settings, password.clone(), page).await.map_err(|e| {
        log_error!("room", "Could not start the local room server: {e}");
        format!("falha ao iniciar servidor: {e}")
    })?;
    let tunnel = match tunnel::open(server.port).await {
        Ok(t) => t,
        Err(e) => {
            log_error!("room", "Could not open the internet tunnel: {e}");
            let _ = server.stop.send(true);
            return Err(e);
        }
    };
    let url = tunnel.public_url().to_string();
    log_info!("room", "Room is online at {url}");
    *state.room.lock().unwrap() = Some(OpenRoom { stop: server.stop, tunnel });
    Ok(RoomInfo { port: server.port, url, password })
}

#[tauri::command]
pub fn close_room(state: State<'_, RoomState>) {
    log_info!("room", "Closed the room");
    state.close();
}

//! Comandos do app para o envio nativo. As mensagens para a sala saem como o
//! evento `native-send` ({ viewer, kind, ... }); o app (`transport/native.ts`)
//! as manda pela sinalização como se fossem do WebRTC do WebView.

use super::{Outgoing, Signals};
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

struct AppSignals(AppHandle);

#[derive(Serialize, Clone)]
struct Event<'a> {
    viewer: &'a str,
    #[serde(flatten)]
    msg: Outgoing,
}

impl Signals for AppSignals {
    fn send(&self, viewer: &str, msg: Outgoing) {
        let _ = self.0.emit("native-send", Event { viewer, msg });
    }
}

/// Alguém pediu para assistir: cria a conexão (a oferta sai pelo evento).
#[tauri::command]
pub async fn native_viewer_open(app: AppHandle, viewer: String, name: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || super::open_viewer(&viewer, &name, Arc::new(AppSignals(app))))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn native_viewer_answer(viewer: String, sdp: String) -> Result<(), String> {
    super::answer(&viewer, sdp)
}

#[tauri::command]
pub fn native_viewer_candidate(viewer: String, candidate: String) -> Result<(), String> {
    super::remote_candidate(&viewer, candidate)
}

#[tauri::command]
pub fn native_viewer_close(viewer: String) {
    super::close_viewer(&viewer);
}

#[tauri::command]
pub fn native_send_stop() {
    super::stop();
}

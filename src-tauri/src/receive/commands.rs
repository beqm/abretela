//! Comandos do app para a recepção nativa. As mensagens para a sala saem como
//! o evento `native-watch` ({ sharer, kind, ... }); o app
//! (`transport/nativeWatch.ts`) as manda pela sinalização como se fossem do
//! WebRTC do WebView. A mídia vai pelo WebSocket local (`bridge.rs`).

use super::{bridge::WebViewSink, Outgoing, Signals};
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

struct AppSignals(AppHandle);

#[derive(Serialize, Clone)]
struct Event<'a> {
    sharer: &'a str,
    #[serde(flatten)]
    msg: Outgoing,
}

impl Signals for AppSignals {
    fn send(&self, sharer: &str, msg: Outgoing) {
        let _ = self.0.emit("native-watch", Event { sharer, msg });
    }
}

/// Chegou a oferta de quem transmite: responde (a resposta sai pelo evento)
/// e devolve a URL do WebSocket local por onde a mídia vai chegar.
#[tauri::command]
pub async fn native_watch_open(app: AppHandle, sharer: String, name: String, sdp: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let port = crate::capture::frames_port().ok_or("servidor local não iniciado")?;
        let token = crate::capture::random_token();
        let sink = WebViewSink::open(&token);
        if let Err(e) = super::open(&sharer, &name, &sdp, Arc::new(AppSignals(app)), sink) {
            WebViewSink::discard(&token);
            return Err(e);
        }
        Ok(format!("ws://127.0.0.1:{port}/watch/{token}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn native_watch_candidate(sharer: String, candidate: String) -> Result<(), String> {
    super::remote_candidate(&sharer, candidate)
}

#[tauri::command]
pub fn native_watch_close(sharer: String) {
    super::close(&sharer);
}

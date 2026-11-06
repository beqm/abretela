//! Comandos do app para a captura (rodam fora da thread do Tauri: a captura
//! do Windows bloqueia enquanto inicia).

use super::Source;

#[tauri::command]
pub async fn list_sources() -> Vec<Source> {
    tokio::task::spawn_blocking(super::list_sources).await.unwrap_or_default()
}

#[tauri::command]
pub async fn thumbnail(id: String) -> Option<String> {
    tokio::task::spawn_blocking(move || super::thumbnail(&id)).await.ok().flatten()
}

/// Retorna a URL do WebSocket local que entrega os frames. Com
/// `native_send` (bitrate máximo), os frames também vão para o envio nativo.
#[tauri::command]
pub async fn start_capture(
    id: String,
    height: u32,
    fps: u32,
    audio: bool,
    gpu: bool,
    native_send: Option<u32>,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || super::start(&id, height, fps, audio, gpu, native_send))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn stop_capture(token: Option<String>) {
    let _ = tokio::task::spawn_blocking(move || super::stop_if(token.as_deref())).await;
}

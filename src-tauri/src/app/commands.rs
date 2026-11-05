//! Comandos do app em si: bandeja e logs vindos do frontend.

use super::tray;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Manager};

/// Linhas de log vindas do frontend.
#[tauri::command]
pub fn log(level: String, area: String, message: String) {
    crate::logging::write(&level, &area, &message);
}

/// Escreve as placas de rede no log (chamado ao entrar numa sala).
#[tauri::command]
pub async fn log_network(why: String) {
    // GetAdaptersAddresses pode demorar um pouco: fora da thread principal
    let _ = tauri::async_runtime::spawn_blocking(move || crate::logging::network::log_adapters(&why)).await;
}

#[tauri::command]
pub fn set_close_to_tray(app: AppHandle, enabled: bool) {
    app.state::<tray::Tray>().close_to_tray.store(enabled, Ordering::Relaxed);
}

#[tauri::command]
pub fn set_live(app: AppHandle, live: bool) {
    tray::set_live(&app, live);
}

#[tauri::command]
pub fn set_tray_texts(app: AppHandle, open: String, quit: String, tooltip: String, live_tooltip: String) {
    tray::set_texts(&app, &open, &quit, &tooltip, &live_tooltip);
}

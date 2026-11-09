#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Só liga as peças: plugins do Tauri, estado, inicialização e comandos.
//! Cada área mora na própria pasta (app, capture, room, tunnel, logging).

#[macro_use]
mod logging;
mod app;
mod capture;
mod encode;
mod room;
mod send;
mod tunnel;
#[cfg(test)]
mod e2e_test;

use app::{startup, tray};
use room::commands::RoomState;
use std::sync::atomic::Ordering;
use tauri::{Manager, RunEvent, WindowEvent};

fn main() {
    tauri::Builder::default()
        // abrir o app de novo só traz a janela que já está aberta (ou na bandeja)
        // links abretela:// que abrem uma segunda instância chegam ao app já aberto
        // pelo deep-link (recurso "deep-link" do single-instance)
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| tray::show_main(app)))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_opener::init())
        .manage(RoomState::default())
        .setup(|app| {
            let fallback = app.path().app_log_dir().unwrap_or_else(|_| std::env::temp_dir().join("AbreTela"));
            logging::init(
                fallback,
                &format!(
                    "AbreTela v{} started · {} · WebView2 {}",
                    app.package_info().version,
                    logging::windows_version(),
                    tauri::webview_version().unwrap_or_else(|_| "unknown".into()),
                ),
            );
            std::thread::spawn(|| logging::network::log_adapters("at startup"));
            tauri::async_runtime::spawn(async {
                if let Err(e) = capture::serve().await {
                    log_error!("capture", "The local capture server stopped: {e}");
                }
            });
            tray::setup(app.handle())?;
            // instalado, o próprio instalador registra o abretela://; em
            // desenvolvimento, registra aqui
            #[cfg(debug_assertions)]
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                if let Err(e) = app.deep_link().register_all() {
                    log_warn!("app", "Could not register the abretela:// links: {e}");
                }
            }
            // a janela principal nasce escondida; a janelinha de abertura mostra
            // ela quando tudo carregar (iniciado junto com o Windows: fica na bandeja)
            let minimized = std::env::args().any(|a| a == "--minimized");
            startup::setup(app.handle(), minimized)?;
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
            room::commands::create_room,
            room::commands::close_room,
            capture::commands::list_sources,
            capture::commands::thumbnail,
            capture::commands::start_capture,
            capture::commands::stop_capture,
            app::commands::set_close_to_tray,
            app::commands::set_live,
            app::commands::set_tray_texts,
            app::commands::log,
            send::commands::native_viewer_open,
            send::commands::native_viewer_answer,
            send::commands::native_viewer_candidate,
            send::commands::native_viewer_view,
            send::commands::native_viewer_close,
            send::commands::native_send_stop,
            app::commands::log_network,
            startup::splash_status,
            startup::splash_done,
            startup::app_ready,
        ])
        .build(tauri::generate_context!())
        .expect("erro ao iniciar o app")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                log_info!("app", "App closed");
                capture::stop();
                app.state::<RoomState>().close();
            }
        });
}

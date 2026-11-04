//! Destino dos pacotes hoje: um WebSocket local que o app lê
//! (`src/lib/capture/native.ts`) e transforma em faixas para o WebRTC.

use super::contracts::FrameSink;
use super::packet::VideoPacket;
use axum::{
    body::Bytes,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use futures_util::SinkExt;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

type Receivers = (mpsc::Receiver<Bytes>, mpsc::Receiver<Bytes>);

static PORT: OnceLock<u16> = OnceLock::new();
/// Filas da captura atual, esperando o app conectar com o token certo.
static WAITING: Mutex<Option<(String, Receivers)>> = Mutex::new(None);

pub struct WebSocketSink {
    latest: Mutex<Option<Bytes>>,
    fresh: AtomicBool,
    audio: mpsc::Sender<Bytes>,
}

impl FrameSink for WebSocketSink {
    fn video(&self, packet: VideoPacket) {
        *self.latest.lock().unwrap() = Some(packet.into_bytes());
        self.fresh.store(true, Ordering::Release);
    }

    fn audio(&self, packet: Bytes) {
        let _ = self.audio.try_send(packet);
    }

    fn is_closed(&self) -> bool {
        self.audio.is_closed()
    }
}

impl WebSocketSink {
    /// Prepara o destino de uma captura: devolve o destino e a URL que o app
    /// deve abrir. Os frames saem no ritmo de `fps` até `stop`.
    pub fn open(token: &str, fps: u32, stop: Arc<AtomicBool>) -> Result<(Arc<Self>, String), String> {
        let port = *PORT.get().ok_or("servidor de captura não iniciado")?;
        let (video_tx, video_rx) = mpsc::channel::<Bytes>(2);
        let (audio_tx, audio_rx) = mpsc::channel::<Bytes>(200);
        let sink = Arc::new(Self { latest: Mutex::new(None), fresh: AtomicBool::new(false), audio: audio_tx });
        {
            let sink = sink.clone();
            thread::spawn(move || sink.pace(fps, stop, video_tx));
        }
        *WAITING.lock().unwrap() = Some((token.to_string(), (video_rx, audio_rx)));
        Ok((sink, format!("ws://127.0.0.1:{port}/frames/{token}")))
    }

    /// Esquece as filas de uma captura que parou sem o app ter conectado.
    pub fn discard(token: &str) {
        let mut waiting = WAITING.lock().unwrap();
        if waiting.as_ref().is_some_and(|(t, _)| t == token) {
            *waiting = None;
        }
    }

    /// Envia frames no ritmo do FPS escolhido; se a imagem ficar parada,
    /// reenvia o último frame a cada 500ms para quem entrar depois receber imagem.
    fn pace(&self, fps: u32, stop: Arc<AtomicBool>, tx: mpsc::Sender<Bytes>) {
        let interval = Duration::from_micros(1_000_000 / fps as u64);
        let mut next = Instant::now();
        let mut last_sent = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            next += interval;
            if let Some(wait) = next.checked_duration_since(Instant::now()) {
                thread::sleep(wait);
            } else {
                next = Instant::now();
            }
            let is_fresh = self.fresh.swap(false, Ordering::Acquire);
            if !is_fresh && last_sent.elapsed() < Duration::from_millis(500) {
                continue;
            }
            let Some(frame) = self.latest.lock().unwrap().clone() else { continue };
            if tx.is_closed() {
                break;
            }
            let _ = tx.try_send(frame); // descarta se o frontend estiver atrasado
            last_sent = Instant::now();
        }
    }
}

/// Sobe o servidor local dos frames (uma vez, ao abrir o app).
pub async fn serve() -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let _ = PORT.set(listener.local_addr()?.port());
    let app = Router::new().route("/frames/{token}", get(frames_handler));
    axum::serve(listener, app).await
}

async fn frames_handler(ws: WebSocketUpgrade, Path(token): Path<String>) -> impl IntoResponse {
    let channels = {
        let mut waiting = WAITING.lock().unwrap();
        match waiting.take() {
            Some((t, channels)) if t == token => Some(channels),
            other => {
                *waiting = other;
                None
            }
        }
    };
    ws.on_upgrade(move |socket| async move {
        if let Some((video, audio)) = channels {
            pump(socket, video, audio).await;
        }
    })
}

async fn pump(mut socket: WebSocket, mut video: mpsc::Receiver<Bytes>, mut audio: mpsc::Receiver<Bytes>) {
    let mut video_open = true;
    let mut audio_open = true;
    while video_open {
        let msg = tokio::select! {
            m = video.recv() => match m { Some(m) => m, None => { video_open = false; continue } },
            m = audio.recv(), if audio_open => match m { Some(m) => m, None => { audio_open = false; continue } },
        };
        if socket.send(Message::Binary(msg)).await.is_err() {
            break;
        }
    }
    let _ = socket.close().await;
}

/// Só para testes: pega as filas direto, sem WebSocket.
#[cfg(test)]
pub fn take_waiting(token: &str) -> Option<Receivers> {
    let mut waiting = WAITING.lock().unwrap();
    match waiting.take() {
        Some((t, channels)) if t == token => Some(channels),
        other => {
            *waiting = other;
            None
        }
    }
}

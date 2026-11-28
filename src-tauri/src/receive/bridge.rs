//! A ponte da recepção nativa para o WebView: a mídia recebida, ainda
//! codificada, vai em pacotes (`capture/packet.rs`) pelo mesmo servidor
//! WebSocket local da captura, numa rota própria (`/watch/<token>`). O
//! WebView decodifica e mostra.
//!
//! Se o WebView ficar para trás, a fila enche. Vídeo comprimido não pode
//! perder um frame no meio (os seguintes dependem dele), então, quando um
//! frame não cabe, os próximos são descartados até chegar um quadro-chave.

use super::{MediaSink, Received};
use crate::capture::packet::{encoded_packet, KIND_ENCODED_AUDIO, KIND_ENCODED_VIDEO};
use axum::{
    body::Bytes,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path,
    },
    response::IntoResponse,
};
use futures_util::SinkExt;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use tokio::sync::mpsc;

/// Quantos pacotes podem esperar o WebView (uns 5 s de vídeo e som).
const QUEUE: usize = 512;

/// Filas esperando o WebView conectar, por token.
static WAITING: Mutex<Vec<(String, mpsc::Receiver<Bytes>)>> = Mutex::new(Vec::new());

pub struct WebViewSink {
    tx: mpsc::Sender<Bytes>,
    /// Um frame não coube: esperando um quadro-chave para voltar.
    behind: AtomicBool,
    dropped: AtomicU64,
}

impl WebViewSink {
    /// Prepara o destino de uma recepção; o WebView busca a fila pelo `token`.
    pub fn open(token: &str) -> Arc<Self> {
        Self::with_queue(token, QUEUE)
    }

    fn with_queue(token: &str, size: usize) -> Arc<Self> {
        let (tx, rx) = mpsc::channel(size);
        WAITING.lock().unwrap().push((token.to_string(), rx));
        Arc::new(Self { tx, behind: AtomicBool::new(false), dropped: AtomicU64::new(0) })
    }

    /// Esquece a fila de uma recepção que parou sem o WebView ter conectado.
    pub fn discard(token: &str) {
        WAITING.lock().unwrap().retain(|(t, _)| t != token);
    }

    /// Frames descartados porque o WebView ficou para trás.
    #[cfg(test)]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl MediaSink for WebViewSink {
    fn video(&self, frame: Received) {
        if self.behind.load(Ordering::Relaxed) && !frame.keyframe {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let packet = encoded_packet(KIND_ENCODED_VIDEO, frame.keyframe, frame.timestamp, &frame.data);
        let sent = self.tx.try_send(packet).is_ok();
        let was_behind = self.behind.swap(!sent, Ordering::Relaxed);
        if !sent {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        } else if was_behind {
            crate::log_warn!(
                "watching",
                "The WebView fell behind the video being received; back at a keyframe ({} frames skipped so far)",
                self.dropped.load(Ordering::Relaxed)
            );
        }
    }

    fn audio(&self, packet: Received) {
        // um pacote de som perdido é só um estalo: não precisa esperar nada
        let _ = self.tx.try_send(encoded_packet(KIND_ENCODED_AUDIO, packet.keyframe, packet.timestamp, &packet.data));
    }
}

fn take(token: &str) -> Option<mpsc::Receiver<Bytes>> {
    let mut waiting = WAITING.lock().unwrap();
    let index = waiting.iter().position(|(t, _)| t == token)?;
    Some(waiting.remove(index).1)
}

/// Rota `/watch/<token>` do servidor local: entrega a fila ao WebView.
pub async fn handler(ws: WebSocketUpgrade, Path(token): Path<String>) -> impl IntoResponse {
    let queue = take(&token);
    ws.on_upgrade(move |socket| async move {
        if let Some(queue) = queue {
            pump(socket, queue).await;
        }
    })
}

async fn pump(mut socket: WebSocket, mut queue: mpsc::Receiver<Bytes>) {
    while let Some(packet) = queue.recv().await {
        if socket.send(Message::Binary(packet)).await.is_err() {
            break;
        }
    }
    let _ = socket.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::packet::HEADER;
    use std::time::Duration;

    fn frame(keyframe: bool, ms: u64) -> Received {
        Received { data: Arc::from(vec![ms as u8; 4]), keyframe, timestamp: Duration::from_millis(ms) }
    }

    /// (tipo, é quadro-chave, primeiro byte dos dados) do próximo pacote da fila.
    fn next(queue: &mut mpsc::Receiver<Bytes>) -> Option<(u8, bool, u8)> {
        queue.try_recv().ok().map(|p| (p[0], p[1] == 1, p[HEADER]))
    }

    #[test]
    fn media_reaches_the_webview_queue_in_order() {
        let sink = WebViewSink::open("order");
        let mut queue = take("order").expect("the queue waits for the WebView under its token");
        sink.video(frame(true, 0));
        sink.audio(frame(true, 20));
        sink.video(frame(false, 33));
        assert_eq!(next(&mut queue), Some((KIND_ENCODED_VIDEO, true, 0)));
        assert_eq!(next(&mut queue), Some((KIND_ENCODED_AUDIO, true, 20)));
        assert_eq!(next(&mut queue), Some((KIND_ENCODED_VIDEO, false, 33)));
        assert_eq!(next(&mut queue), None);
        assert!(take("order").is_none(), "a queue is handed out only once");
    }

    #[test]
    fn a_webview_left_behind_comes_back_at_a_keyframe() {
        let sink = WebViewSink::with_queue("behind", 2);
        let mut queue = take("behind").unwrap();
        sink.video(frame(true, 1));
        sink.video(frame(false, 2));
        sink.video(frame(false, 3)); // não cabe: daqui em diante, só de um quadro-chave
        assert_eq!(next(&mut queue), Some((KIND_ENCODED_VIDEO, true, 1)));
        assert_eq!(next(&mut queue), Some((KIND_ENCODED_VIDEO, false, 2)));
        sink.video(frame(false, 4)); // há lugar, mas depende do 3, que se perdeu
        assert_eq!(next(&mut queue), None);
        sink.video(frame(true, 5));
        sink.video(frame(false, 6));
        assert_eq!(next(&mut queue), Some((KIND_ENCODED_VIDEO, true, 5)));
        assert_eq!(next(&mut queue), Some((KIND_ENCODED_VIDEO, false, 6)));
        assert_eq!(sink.dropped(), 2);
    }

    #[test]
    fn a_reception_that_never_reached_the_webview_leaves_nothing_behind() {
        let _sink = WebViewSink::open("gone");
        WebViewSink::discard("gone");
        assert!(take("gone").is_none());
    }
}

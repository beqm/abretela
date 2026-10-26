//! Servidor de sinalização: guarda quem está na sala e repassa as mensagens
//! WebRTC entre os participantes. Vídeo e áudio nunca passam por aqui.

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::{mpsc, watch, Mutex};

#[derive(Clone, Serialize)]
struct PeerInfo {
    id: String,
    name: String,
    sharing: bool,
    /// Ex.: "720p 60fps"
    label: Option<String>,
}

/// Limites de qualidade escolhidos por quem criou a sala.
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomSettings {
    pub min_height: u32,
    pub max_height: u32,
    pub min_fps: u32,
    pub max_fps: u32,
}

struct Peer {
    info: PeerInfo,
    tx: mpsc::UnboundedSender<Message>,
}

struct Hub {
    peers: Mutex<HashMap<String, Peer>>,
    next_id: AtomicU64,
    shutdown: watch::Receiver<bool>,
    settings: RoomSettings,
    password: String,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum ClientMsg {
    Join {
        name: String,
        #[serde(default)]
        password: String,
    },
    Sharing { sharing: bool, label: Option<String> },
    Signal { to: String, data: Value },
}

pub struct RunningServer {
    pub port: u16,
    pub stop: watch::Sender<bool>,
}

pub async fn start(settings: RoomSettings, password: String) -> std::io::Result<RunningServer> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let (stop, stop_rx) = watch::channel(false);

    let hub = Arc::new(Hub {
        peers: Mutex::new(HashMap::new()),
        next_id: AtomicU64::new(1),
        shutdown: stop_rx.clone(),
        settings,
        password,
    });
    let app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/health", get(|| async { "ok" }))
        .with_state(hub);

    let mut rx = stop_rx;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = rx.wait_for(|stopped| *stopped).await;
            })
            .await;
    });

    Ok(RunningServer { port, stop })
}

async fn ws_handler(ws: WebSocketUpgrade, State(hub): State<Arc<Hub>>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, hub))
}

fn send(tx: &mpsc::UnboundedSender<Message>, value: Value) {
    let _ = tx.send(Message::Text(value.to_string().into()));
}

fn broadcast(peers: &HashMap<String, Peer>, value: Value) {
    let text = value.to_string();
    for peer in peers.values() {
        let _ = peer.tx.send(Message::Text(text.clone().into()));
    }
}

async fn handle_socket(socket: WebSocket, hub: Arc<Hub>) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

    // Escritor: envia as mensagens da fila e um ping periódico para o
    // Cloudflare não derrubar a conexão por inatividade.
    let writer = tokio::spawn(async move {
        let mut ping = tokio::time::interval(Duration::from_secs(20));
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    Some(m) => if sink.send(m).await.is_err() { break },
                    None => break,
                },
                _ = ping.tick() => {
                    if sink.send(Message::Ping(Default::default())).await.is_err() { break }
                }
            }
        }
        let _ = sink.close().await;
    });

    let mut shutdown = hub.shutdown.clone();
    let mut my_id: Option<String> = None;

    loop {
        let msg = tokio::select! {
            msg = stream.next() => msg,
            _ = shutdown.wait_for(|stopped| *stopped) => break,
        };
        let text = match msg {
            Some(Ok(Message::Text(t))) => t,
            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
            Some(Ok(_)) => continue,
        };
        let Ok(cmsg) = serde_json::from_str::<ClientMsg>(&text) else {
            continue;
        };

        match cmsg {
            ClientMsg::Join { name, password } => {
                if my_id.is_some() {
                    continue;
                }
                if password.trim() != hub.password {
                    // atraso para dificultar tentativa e erro; depois desconecta
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    send(&tx, json!({ "type": "error", "reason": "wrong-password" }));
                    break;
                }
                let id = hub.next_id.fetch_add(1, Ordering::Relaxed).to_string();
                let mut name: String = name.trim().chars().take(32).collect();
                if name.is_empty() {
                    name = format!("Convidado {id}");
                }
                let info = PeerInfo { id: id.clone(), name, sharing: false, label: None };

                let mut peers = hub.peers.lock().await;
                let others: Vec<PeerInfo> = peers.values().map(|p| p.info.clone()).collect();
                send(
                    &tx,
                    json!({ "type": "welcome", "id": id, "peers": others, "settings": hub.settings }),
                );
                broadcast(&peers, json!({ "type": "peer-joined", "peer": info }));
                peers.insert(id.clone(), Peer { info, tx: tx.clone() });
                my_id = Some(id);
            }
            ClientMsg::Sharing { sharing, label } => {
                let Some(id) = &my_id else { continue };
                let label = label.filter(|_| sharing).map(|l| l.chars().take(48).collect::<String>());
                let mut peers = hub.peers.lock().await;
                if let Some(me) = peers.get_mut(id) {
                    me.info.sharing = sharing;
                    me.info.label = label.clone();
                }
                broadcast(
                    &peers,
                    json!({ "type": "peer-updated", "id": id, "sharing": sharing, "label": label }),
                );
            }
            ClientMsg::Signal { to, data } => {
                let Some(id) = &my_id else { continue };
                let peers = hub.peers.lock().await;
                if let Some(target) = peers.get(&to) {
                    send(&target.tx, json!({ "type": "signal", "from": id, "data": data }));
                }
            }
        }
    }

    if let Some(id) = my_id {
        let mut peers = hub.peers.lock().await;
        peers.remove(&id);
        broadcast(&peers, json!({ "type": "peer-left", "id": id }));
    }
    drop(tx);
    let _ = writer.await;
}

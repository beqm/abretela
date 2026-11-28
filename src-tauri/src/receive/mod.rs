//! Recepção nativa: assistir a tela de alguém com o WebRTC no Rust (str0m),
//! para os sistemas em que o WebView não tem WebRTC (Linux, com o WebKitGTK).
//!
//! É o espelho do envio nativo (`send/`): aqui o app responde à oferta de quem
//! transmite, recebe o H.264 e o Opus e os entrega, ainda codificados, a um
//! destino ([`MediaSink`]). No app o destino é o WebView, que decodifica e
//! mostra. As mensagens da sala (offer/answer/ice) são as mesmas do WebRTC do
//! WebView, então quem transmite não percebe diferença.
//!
//! Compila em todos os sistemas, para os testes rodarem em qualquer um.

pub mod bridge;
pub mod commands;
mod opus;
mod watcher;

#[cfg(test)]
mod tests;

use serde::Serialize;
use std::{
    net::IpAddr,
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};
use watcher::WatchMsg;

/// Mensagens para a sala (o app as manda pela sinalização a quem transmite).
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Outgoing {
    Answer { sdp: String },
    Ice { candidate: String, mid: String },
    /// "connected", "disconnected", "failed", "closed" (para o app e os logs).
    State { state: String },
}

/// Para onde vão as mensagens de cada transmissão assistida (o app, ou um teste).
pub trait Signals: Send + Sync {
    fn send(&self, sharer: &str, msg: Outgoing);
}

/// Um frame de vídeo (H.264 em Annex B, uma unidade de acesso inteira) ou um
/// pacote de som (Opus), como chegou: ainda codificado.
pub struct Received {
    pub data: Arc<[u8]>,
    /// Quadro-chave: dá para começar a decodificar por ele (no som, sempre).
    pub keyframe: bool,
    /// Tempo desde o começo do que foi recebido, o mesmo relógio para o vídeo
    /// e o som.
    pub timestamp: Duration,
}

/// Para onde vai a mídia recebida.
pub trait MediaSink: Send + Sync {
    fn video(&self, frame: Received);
    fn audio(&self, packet: Received);
}

/// Quem estou assistindo: o id da pessoa e o canal da thread da conexão.
static WATCHING: Mutex<Vec<(String, mpsc::Sender<WatchMsg>)>> = Mutex::new(Vec::new());

/// Responde à oferta de `id` e começa a receber. A resposta e as rotas saem
/// por `signals`; a mídia, por `sink`.
pub fn open(
    id: &str,
    name: &str,
    offer_sdp: &str,
    signals: Arc<dyn Signals>,
    sink: Arc<dyn MediaSink>,
) -> Result<(), String> {
    open_on(id, name, offer_sdp, crate::logging::network::usable_addresses(), signals, sink)
}

/// Como [`open`], escutando só em `addresses` (os testes usam 127.0.0.1).
pub(crate) fn open_on(
    id: &str,
    name: &str,
    offer_sdp: &str,
    addresses: Vec<IpAddr>,
    signals: Arc<dyn Signals>,
    sink: Arc<dyn MediaSink>,
) -> Result<(), String> {
    close(id);
    let (tx, rx) = mpsc::channel();
    watcher::spawn(id.to_string(), name.to_string(), offer_sdp, addresses, signals, sink, tx.clone(), rx)?;
    WATCHING.lock().unwrap().push((id.to_string(), tx));
    Ok(())
}

/// Uma rota que quem transmite mandou depois da oferta.
pub fn remote_candidate(id: &str, candidate: String) -> Result<(), String> {
    let watching = WATCHING.lock().unwrap();
    let (_, tx) = watching.iter().find(|(who, _)| who == id).ok_or("not watching this person")?;
    tx.send(WatchMsg::RemoteCandidate(candidate)).map_err(|_| "the connection already closed".to_string())
}

/// Para de assistir `id` (se estava).
pub fn close(id: &str) {
    let mut watching = WATCHING.lock().unwrap();
    watching.retain(|(who, tx)| {
        if who == id {
            let _ = tx.send(WatchMsg::Close);
        }
        who != id
    });
}

//! Envio nativo (ROADMAP, seção 2, parte 3b): quem transmite codifica a tela
//! UMA vez (placa de vídeo, `encode/`) e manda por WebRTC (str0m) para cada
//! espectador. Quem assiste continua usando o WebRTC do WebView, então as
//! mensagens da sala (offer/answer/ice) são as mesmas de sempre.
//!
//! - A captura entrega os frames por [`frame_sink`] (junto com a prévia local).
//! - Uma thread de codificação pega sempre o frame mais novo, codifica só se
//!   alguém estiver conectado e manda o resultado a todos.
//! - Cada espectador é uma thread com o próprio `Rtc` do str0m e sockets UDP
//!   (`viewer.rs`); as mensagens para a sala saem por [`Signals`].
//!
//! Por enquanto só vídeo (o áudio é a parte 3c).

pub mod commands;
pub mod stun;
mod viewer;

use crate::capture::{contracts::FrameSink, packet::VideoPacket};
use crate::encode::{self, EncodedFrame, EncoderConfig, VideoEncoder};
use axum::body::Bytes;
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Condvar, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use viewer::ViewerMsg;

/// No máximo um quadro-chave por esse intervalo, juntando os pedidos (sem
/// isso um espectador com perda pede dezenas por segundo; ver o protótipo).
const KEYFRAME_MIN_INTERVAL: Duration = Duration::from_secs(1);
/// A captura só entrega frame quando a tela muda. Parada, o último frame é
/// codificado de novo nesse ritmo (e na hora, se alguém pedir quadro-chave),
/// para quem acabou de entrar ver alguma coisa.
const REPEAT_STILL_FRAME: Duration = Duration::from_secs(1);
const STILL_CHECK: Duration = Duration::from_millis(100);
/// Menor bitrate que o controle de banda pode pedir.
const MIN_BITRATE: u32 = 150_000;

/// Mensagens para a sala (o app as manda pela sinalização).
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Outgoing {
    Offer { sdp: String },
    Ice { candidate: String, mid: String },
    /// "connected", "disconnected", "failed", "closed" (para o app e os logs).
    State { state: String },
}

/// Para onde vão as mensagens de cada espectador (o app, ou um teste).
pub trait Signals: Send + Sync {
    fn send(&self, viewer: &str, msg: Outgoing);
}

/// Um frame codificado, compartilhado por todos.
pub struct Encoded {
    pub data: Arc<[u8]>,
    pub keyframe: bool,
    /// Tempo desde o começo da transmissão.
    pub timestamp: Duration,
}

struct ViewerEntry {
    tx: mpsc::Sender<ViewerMsg>,
    /// Qual conexão é (uma nova pode substituir a antiga com o mesmo id).
    session: u64,
    name: String,
    connected: bool,
    /// Banda estimada até essa pessoa (bps).
    estimate: Option<u64>,
}

pub struct Sender {
    running: AtomicBool,
    latest: Mutex<Option<(VideoPacket, Instant)>>,
    frame_ready: Condvar,
    viewers: Mutex<HashMap<String, ViewerEntry>>,
    keyframe: AtomicBool,
    config: Mutex<SendConfig>,
    started: Instant,
    next_session: std::sync::atomic::AtomicU64,
}

#[derive(Clone, Copy, Debug)]
struct SendConfig {
    fps: u32,
    max_bitrate: u32,
}

static SENDER: Mutex<Option<Arc<Sender>>> = Mutex::new(None);

fn current() -> Option<Arc<Sender>> {
    SENDER.lock().unwrap().clone()
}

/// Liga (ou reaproveita, ao trocar de tela) o envio nativo e devolve o
/// destino dos frames da captura.
pub fn start(fps: u32, max_bitrate: u32) -> Arc<dyn FrameSink> {
    let mut slot = SENDER.lock().unwrap();
    if let Some(s) = slot.as_ref().filter(|s| s.running.load(Ordering::Relaxed)) {
        *s.config.lock().unwrap() = SendConfig { fps, max_bitrate };
        s.keyframe.store(true, Ordering::Relaxed);
        log_info!("sharing", "Native sending: new capture at {fps} fps, up to {} kbps", max_bitrate / 1000);
        return s.clone();
    }
    let sender = Arc::new(Sender {
        running: AtomicBool::new(true),
        latest: Mutex::new(None),
        frame_ready: Condvar::new(),
        viewers: Mutex::new(HashMap::new()),
        keyframe: AtomicBool::new(true),
        config: Mutex::new(SendConfig { fps, max_bitrate }),
        started: Instant::now(),
        next_session: Default::default(),
    });
    let worker = sender.clone();
    thread::Builder::new()
        .name("native-encoder".into())
        .spawn(move || worker.encode_loop())
        .expect("encoder thread");
    log_info!("sharing", "Native sending started at {fps} fps, up to {} kbps (video only for now)", max_bitrate / 1000);
    *slot = Some(sender.clone());
    sender
}

/// Para de transmitir: fecha todos os espectadores e o codificador.
pub fn stop() {
    let Some(sender) = SENDER.lock().unwrap().take() else { return };
    sender.running.store(false, Ordering::Relaxed);
    sender.frame_ready.notify_all();
    let names: Vec<String> = sender
        .viewers
        .lock()
        .unwrap()
        .drain()
        .map(|(_, v)| {
            let _ = v.tx.send(ViewerMsg::Close);
            v.name
        })
        .collect();
    log_info!("sharing", "Native sending stopped (closing {})", if names.is_empty() { "nobody".into() } else { names.join(", ") });
}

/// Alguém quer assistir: cria a conexão e manda a oferta.
pub fn open_viewer(id: &str, name: &str, signals: Arc<dyn Signals>) -> Result<(), String> {
    let sender = current().ok_or("not sharing")?;
    if let Some(old) = sender.viewers.lock().unwrap().remove(id) {
        let _ = old.tx.send(ViewerMsg::Close);
    }
    let bitrate = sender.config.lock().unwrap().max_bitrate;
    let session = sender.next_session.fetch_add(1, Ordering::Relaxed);
    // registra antes de começar, para os avisos da thread acharem a entrada
    let (tx, rx) = mpsc::channel();
    sender.viewers.lock().unwrap().insert(
        id.to_string(),
        ViewerEntry { tx: tx.clone(), session, name: name.to_string(), connected: false, estimate: None },
    );
    if let Err(e) = viewer::spawn(id.to_string(), name.to_string(), session, bitrate, sender.clone(), signals, tx, rx) {
        sender.forget(id, session);
        return Err(e);
    }
    Ok(())
}

fn to_viewer(id: &str, msg: ViewerMsg) -> Result<(), String> {
    let sender = current().ok_or("not sharing")?;
    let viewers = sender.viewers.lock().unwrap();
    let v = viewers.get(id).ok_or("no connection with this viewer")?;
    v.tx.send(msg).map_err(|_| "the connection already closed".to_string())
}

pub fn answer(id: &str, sdp: String) -> Result<(), String> {
    to_viewer(id, ViewerMsg::Answer(sdp))
}

pub fn remote_candidate(id: &str, candidate: String) -> Result<(), String> {
    to_viewer(id, ViewerMsg::RemoteCandidate(candidate))
}

pub fn close_viewer(id: &str) {
    if let Some(sender) = current() {
        if let Some(v) = sender.viewers.lock().unwrap().remove(id) {
            let _ = v.tx.send(ViewerMsg::Close);
        }
    }
}

impl Sender {
    // ---------- chamado pelos espectadores ----------

    fn request_keyframe(&self) {
        self.keyframe.store(true, Ordering::Relaxed);
    }

    fn set_connected(&self, id: &str, session: u64, connected: bool) {
        if let Some(v) = self.viewers.lock().unwrap().get_mut(id).filter(|v| v.session == session) {
            v.connected = connected;
        }
        if connected {
            self.request_keyframe();
        }
    }

    fn set_estimate(&self, id: &str, session: u64, bps: u64) {
        if let Some(v) = self.viewers.lock().unwrap().get_mut(id).filter(|v| v.session == session) {
            v.estimate = Some(bps);
        }
    }

    /// A thread do espectador terminou.
    fn forget(&self, id: &str, session: u64) {
        let mut viewers = self.viewers.lock().unwrap();
        if viewers.get(id).is_some_and(|v| v.session == session) {
            viewers.remove(id);
        }
    }

    // ---------- codificação ----------

    fn encode_loop(self: Arc<Self>) {
        let mut encoder: Option<Box<dyn VideoEncoder>> = None;
        let mut software_only = false;
        let mut failures = 0u32;
        let mut last_keyframe: Option<Instant> = None;
        let mut bitrate = 0u32;
        let mut last_rate_check = Instant::now();
        let mut stats = (0u64, Duration::ZERO, Instant::now()); // frames, tempo, desde
        let mut last_frame: Option<VideoPacket> = None;
        let mut last_encode = Instant::now();

        while self.running.load(Ordering::Relaxed) {
            let next = {
                let latest = self.latest.lock().unwrap();
                let (mut latest, _) = self.frame_ready.wait_timeout_while(latest, STILL_CHECK, |l| l.is_none()).unwrap();
                latest.take()
            };
            let (frame, captured) = match next {
                Some((frame, at)) => {
                    last_frame = Some(frame.clone());
                    (frame, at)
                }
                // tela parada: repete o último frame se alguém precisa
                None => match &last_frame {
                    Some(f)
                        if self.keyframe.load(Ordering::Relaxed) || last_encode.elapsed() >= REPEAT_STILL_FRAME =>
                    {
                        (f.clone(), Instant::now())
                    }
                    _ => continue,
                },
            };

            // ninguém conectado: não gasta a placa
            let targets: Vec<mpsc::Sender<ViewerMsg>> = {
                let viewers = self.viewers.lock().unwrap();
                viewers.values().filter(|v| v.connected).map(|v| v.tx.clone()).collect()
            };
            if targets.is_empty() {
                continue;
            }

            let config = *self.config.lock().unwrap();
            let size = (frame.width() as u32, frame.height() as u32);
            if encoder.as_ref().is_none_or(|e| e.size() != size) {
                bitrate = config.max_bitrate;
                let enc_config = EncoderConfig {
                    width: size.0,
                    height: size.1,
                    fps: config.fps,
                    bitrate,
                    software_only,
                };
                let me = Arc::downgrade(&self);
                let on_encoded: encode::OnEncoded = Arc::new(move |f| {
                    if let Some(sender) = me.upgrade() {
                        sender.broadcast(f);
                    }
                });
                match encode::open(enc_config, on_encoded) {
                    Ok(e) => {
                        log_info!("sharing", "Native sending encodes {}x{} with {}", size.0, size.1, e.describe());
                        if !e.hardware() && !software_only {
                            log_warn!("sharing", "No graphics card encoder worked here, encoding on the processor");
                        }
                        encoder = Some(e);
                        self.keyframe.store(true, Ordering::Relaxed);
                    }
                    Err(e) => {
                        log_error!("sharing", "Could not start a video encoder: {e}");
                        thread::sleep(Duration::from_secs(1));
                        continue;
                    }
                }
            }
            let enc = encoder.as_mut().unwrap();

            // banda: a menor estimativa entre quem está conectado, com folga
            if last_rate_check.elapsed() >= Duration::from_secs(1) {
                last_rate_check = Instant::now();
                let lowest = self.viewers.lock().unwrap().values().filter_map(|v| v.estimate).min();
                if let Some(est) = lowest {
                    let target = ((est * 9 / 10) as u32).clamp(MIN_BITRATE, config.max_bitrate.max(MIN_BITRATE));
                    if target.abs_diff(bitrate) as u64 * 10 > bitrate as u64 {
                        log_info!(
                            "sharing",
                            "Native sending: bitrate {} -> {} kbps (slowest viewer can take ~{} kbps)",
                            bitrate / 1000,
                            target / 1000,
                            est / 1000
                        );
                        bitrate = target;
                        enc.set_bitrate(bitrate);
                    }
                }
            }

            let keyframe = self.keyframe.load(Ordering::Relaxed)
                && last_keyframe.is_none_or(|t| t.elapsed() >= KEYFRAME_MIN_INTERVAL);
            if keyframe {
                self.keyframe.store(false, Ordering::Relaxed);
                last_keyframe = Some(Instant::now());
            }

            let t0 = Instant::now();
            last_encode = t0;
            let timestamp = captured.duration_since(self.started);
            match enc.encode(&frame, timestamp, keyframe) {
                Ok(()) => {
                    failures = 0;
                    stats.0 += 1;
                    stats.1 += t0.elapsed();
                }
                Err(e) => {
                    failures += 1;
                    log_error!("sharing", "The video encoder failed ({failures}x): {e}");
                    encoder = None;
                    if failures >= 3 && !software_only {
                        software_only = true;
                        log_warn!("sharing", "Switching native sending to the processor encoder");
                    }
                }
            }

            if stats.2.elapsed() >= Duration::from_secs(30) {
                let avg = stats.1.as_secs_f64() * 1000.0 / stats.0.max(1) as f64;
                log_info!(
                    "sharing",
                    "Native sending: {} frames encoded in the last 30s ({:.0} fps), {avg:.1} ms to hand each \
                     to the encoder, {} kbps, {} viewers connected",
                    stats.0,
                    stats.0 as f64 / 30.0,
                    bitrate / 1000,
                    targets.len()
                );
                if let Some(t) = encoder.as_mut().and_then(|e| e.take_timings()) {
                    log_info!("sharing", "Native sending, per frame: {t}");
                }
                stats = (0, Duration::ZERO, Instant::now());
            }
        }
        log_info!("sharing", "Native encoder thread finished");
    }
}

impl Sender {
    /// Um frame codificado: o mesmo para todos os conectados.
    fn broadcast(&self, f: EncodedFrame) {
        let shared = Arc::new(Encoded { data: f.data.into(), keyframe: f.keyframe, timestamp: f.timestamp });
        for v in self.viewers.lock().unwrap().values().filter(|v| v.connected) {
            let _ = v.tx.send(ViewerMsg::Frame(shared.clone()));
        }
    }
}

impl FrameSink for Sender {
    fn video(&self, packet: VideoPacket) {
        // fica só o mais novo: se o codificador atrasar, pula frames
        *self.latest.lock().unwrap() = Some((packet, Instant::now()));
        self.frame_ready.notify_one();
    }

    fn audio(&self, _packet: Bytes) {}

    fn is_closed(&self) -> bool {
        !self.running.load(Ordering::Relaxed)
    }
}

/// Manda os frames da captura para dois destinos (a prévia local pelo
/// WebSocket e o envio nativo). O áudio e o fim seguem o primeiro.
pub struct Tee(pub Arc<dyn FrameSink>, pub Arc<dyn FrameSink>);

impl FrameSink for Tee {
    fn video(&self, packet: VideoPacket) {
        // a prévia local recebe uma cópia; o envio nativo, o original
        self.0.video(packet.clone());
        self.1.video(packet);
    }
    fn audio(&self, packet: Bytes) {
        self.0.audio(packet);
    }
    fn is_closed(&self) -> bool {
        self.0.is_closed()
    }
}

#[cfg(test)]
mod tests;

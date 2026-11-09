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
//! - O som da captura vira Opus (`audio.rs`), também uma vez para todos.

mod audio;
pub mod commands;
pub mod stun;
mod viewer;

use crate::capture::{
    contracts::FrameSink,
    packet::{VideoPacket, HEADER},
};
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
    /// A captura tem som (decide se a oferta leva uma faixa de áudio).
    has_audio: AtomicBool,
    audio: Mutex<Option<audio::AudioEncoder>>,
}

#[derive(Clone, Copy, Debug)]
struct SendConfig {
    fps: u32,
    max_bitrate: u32,
    /// "Aceleração de hardware" ligada: codifica pela placa de vídeo.
    gpu: bool,
}

static SENDER: Mutex<Option<Arc<Sender>>> = Mutex::new(None);

fn current() -> Option<Arc<Sender>> {
    SENDER.lock().unwrap().clone()
}

/// Liga (ou reaproveita, ao trocar de tela) o envio nativo e devolve o
/// destino dos frames da captura.
pub fn start(fps: u32, max_bitrate: u32, has_audio: bool, gpu: bool) -> Arc<dyn FrameSink> {
    let mut slot = SENDER.lock().unwrap();
    let audio_text = if has_audio { "with audio" } else { "no audio" };
    if let Some(s) = slot.as_ref().filter(|s| s.running.load(Ordering::Relaxed)) {
        *s.config.lock().unwrap() = SendConfig { fps, max_bitrate, gpu };
        s.keyframe.store(true, Ordering::Relaxed);
        if s.has_audio.swap(has_audio, Ordering::Relaxed) != has_audio {
            // quem já está conectado continua com as faixas que combinou
            log_info!("sharing", "Native sending: the new capture {}; only new viewers get the change", audio_text);
        }
        log_info!("sharing", "Native sending: new capture at {fps} fps, up to {} kbps, {audio_text}", max_bitrate / 1000);
        return s.clone();
    }
    let sender = Arc::new(Sender {
        running: AtomicBool::new(true),
        latest: Mutex::new(None),
        frame_ready: Condvar::new(),
        viewers: Mutex::new(HashMap::new()),
        keyframe: AtomicBool::new(true),
        config: Mutex::new(SendConfig { fps, max_bitrate, gpu }),
        started: Instant::now(),
        next_session: Default::default(),
        has_audio: AtomicBool::new(has_audio),
        audio: Mutex::new(None),
    });
    let worker = sender.clone();
    thread::Builder::new()
        .name("native-encoder".into())
        .spawn(move || worker.encode_loop())
        .expect("encoder thread");
    log_info!("sharing", "Native sending started at {fps} fps, up to {} kbps, {audio_text}", max_bitrate / 1000);
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
        // a placa falhou 3 vezes seguidas: fica no processador
        let mut gpu_failed = false;
        // com o que o codificador atual foi aberto
        let mut opened_software_only = false;
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
            let software_only = gpu_failed || !config.gpu;
            if encoder.as_ref().is_none_or(|e| e.size() != size) || opened_software_only != software_only {
                encoder = None;
                opened_software_only = software_only;
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
                        if software_only && !config.gpu {
                            log_info!("sharing", "Hardware acceleration is off, so native sending encodes on the processor");
                        } else if !e.hardware() && !software_only {
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
                        gpu_failed = true;
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
        self.to_connected(|| ViewerMsg::Frame(shared.clone()));
    }

    fn to_connected(&self, msg: impl Fn() -> ViewerMsg) {
        for v in self.viewers.lock().unwrap().values().filter(|v| v.connected) {
            let _ = v.tx.send(msg());
        }
    }

    fn anyone_connected(&self) -> bool {
        self.viewers.lock().unwrap().values().any(|v| v.connected)
    }

    /// Um pedaço de som da captura (10 ms).
    fn encode_audio(&self, packet: &[u8]) {
        let Some(samples) = packet.get(HEADER..) else { return };
        let mut slot = self.audio.lock().unwrap();
        if !self.anyone_connected() {
            if let Some(a) = slot.as_mut() {
                a.reset();
            }
            return;
        }
        if slot.is_none() {
            match audio::AudioEncoder::new(self.started) {
                Ok(a) => {
                    log_info!("sharing", "Native sending encodes the audio with Opus (48 kHz stereo, 20 ms packets)");
                    *slot = Some(a);
                }
                Err(e) => {
                    log_error!("sharing", "Could not start the audio encoder: {e}");
                    self.has_audio.store(false, Ordering::Relaxed);
                    return;
                }
            }
        }
        let enc = slot.as_mut().unwrap();
        let resyncs = enc.resyncs;
        let ready = match enc.push(samples, Instant::now()) {
            Ok(r) => r,
            Err(e) => {
                log_error!("sharing", "Audio encoding failed, restarting it: {e}");
                *slot = None;
                return;
            }
        };
        if enc.resyncs > resyncs && enc.resyncs.is_power_of_two() {
            log_info!("sharing", "Native audio resumed after a pause (silence) {} times so far", enc.resyncs);
        }
        drop(slot);
        for p in ready {
            let p = Arc::new(p);
            self.to_connected(|| ViewerMsg::Audio(p.clone()));
        }
    }
}

impl FrameSink for Sender {
    fn video(&self, packet: VideoPacket) {
        // fica só o mais novo: se o codificador atrasar, pula frames
        *self.latest.lock().unwrap() = Some((packet, Instant::now()));
        self.frame_ready.notify_one();
    }

    fn audio(&self, packet: Bytes) {
        if self.has_audio.load(Ordering::Relaxed) {
            self.encode_audio(&packet);
        }
    }

    fn is_closed(&self) -> bool {
        !self.running.load(Ordering::Relaxed)
    }
}

/// No envio nativo a prévia local (a sua própria tela, no app) só serve para
/// você ver: vai em no máximo esse fps, em vez do fps da transmissão.
pub const PREVIEW_FPS: u32 = 15;

/// Manda os frames da captura para dois destinos: a prévia local (WebSocket,
/// no máximo [`PREVIEW_FPS`]) e o envio nativo (todos). O fim segue a prévia.
pub struct Tee {
    preview: Arc<dyn FrameSink>,
    native: Arc<dyn FrameSink>,
    preview_every: Duration,
    last_preview: Mutex<Option<Instant>>,
}

impl Tee {
    pub fn new(preview: Arc<dyn FrameSink>, native: Arc<dyn FrameSink>, preview_fps: u32) -> Self {
        let preview_every = Duration::from_micros(1_000_000 / preview_fps.max(1) as u64);
        Self { preview, native, preview_every, last_preview: Mutex::new(None) }
    }
}

impl FrameSink for Tee {
    fn video(&self, packet: VideoPacket) {
        // a prévia só recebe (e só copia) os frames que vai mostrar; uma
        // pequena folga para o ritmo da captura não pular um a cada dois
        let due = {
            let mut last = self.last_preview.lock().unwrap();
            let due = last.is_none_or(|t| t.elapsed() + self.preview_every / 8 >= self.preview_every);
            if due {
                *last = Some(Instant::now());
            }
            due
        };
        if due {
            self.preview.video(packet.clone());
        }
        self.native.video(packet);
    }
    fn audio(&self, packet: Bytes) {
        self.preview.audio(packet.clone());
        self.native.audio(packet);
    }
    fn is_closed(&self) -> bool {
        self.preview.is_closed()
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod tee_tests {
    use super::*;
    use crate::capture::packet::PixelFormat;
    use std::sync::atomic::AtomicU32;

    #[derive(Default)]
    struct Count(AtomicU32, AtomicU32);
    impl FrameSink for Count {
        fn video(&self, _: VideoPacket) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
        fn audio(&self, _: Bytes) {
            self.1.fetch_add(1, Ordering::Relaxed);
        }
        fn is_closed(&self) -> bool {
            false
        }
    }

    #[test]
    fn preview_gets_fewer_frames_native_gets_all() {
        let (preview, native) = (Arc::new(Count::default()), Arc::new(Count::default()));
        let tee = Tee::new(preview.clone(), native.clone(), PREVIEW_FPS);
        let start = Instant::now();
        for n in 0..60u32 {
            thread::sleep((Duration::from_secs(1) * n / 60).saturating_sub(start.elapsed()));
            tee.video(VideoPacket::new(PixelFormat::Nv12, 16, 16));
            tee.audio(Bytes::from_static(b"x"));
        }
        let shown = preview.0.load(Ordering::Relaxed);
        assert_eq!(native.0.load(Ordering::Relaxed), 60);
        assert!((13..=17).contains(&shown), "preview got {shown} frames in 1 s");
        assert_eq!((preview.1.load(Ordering::Relaxed), native.1.load(Ordering::Relaxed)), (60, 60));
    }
}

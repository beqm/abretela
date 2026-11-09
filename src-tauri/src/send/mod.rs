//! Envio nativo (ROADMAP, seção 2, parte 3b): quem transmite codifica a tela
//! UMA vez (placa de vídeo, `encode/`) e manda por WebRTC (str0m) para cada
//! espectador. Quem assiste continua usando o WebRTC do WebView, então as
//! mensagens da sala (offer/answer/ice) são as mesmas de sempre.
//!
//! - A captura entrega os frames por [`frame_sink`] (junto com a prévia local).
//! - Uma thread de codificação pega sempre o frame mais novo e codifica uma vez
//!   por **camada** de qualidade em uso (`layers.rs`: a da transmissão e as
//!   menores, para quem pediu menos ou tem conexão lenta), não por pessoa.
//! - Cada espectador é uma thread com o próprio `Rtc` do str0m e sockets UDP
//!   (`viewer.rs`); as mensagens para a sala saem por [`Signals`].
//!
//! - O som da captura vira Opus (`audio.rs`), também uma vez para todos.

mod audio;
pub mod commands;
mod layers;
mod scale;
pub mod stun;
mod viewer;

use crate::capture::{
    contracts::FrameSink,
    packet::{VideoPacket, HEADER},
};
use crate::encode::{self, EncodedFrame, EncoderConfig, VideoEncoder};
use axum::body::Bytes;
use serde::Serialize;
use layers::{Plan, NO_VIDEO};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
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
/// codificado de novo na hora em que alguém pede quadro-chave (quem acabou de
/// entrar, ou trocou de camada) e, fora isso, só nesse ritmo: com a tela
/// parada quase não se gasta banda.
const REPEAT_STILL_FRAME: Duration = Duration::from_secs(5);
const STILL_CHECK: Duration = Duration::from_millis(100);
/// De quanto em quanto revê as camadas e o bitrate.
const CHECK_EVERY: Duration = Duration::from_secs(1);

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

/// Um frame codificado, compartilhado por todos da mesma camada.
pub struct Encoded {
    pub data: Arc<[u8]>,
    pub keyframe: bool,
    /// Tempo desde o começo da transmissão.
    pub timestamp: Duration,
    /// Altura da camada (vídeo); 0 no áudio.
    pub layer: u32,
}

struct ViewerEntry {
    tx: mpsc::Sender<ViewerMsg>,
    /// Qual conexão é (uma nova pode substituir a antiga com o mesmo id).
    session: u64,
    name: String,
    connected: bool,
    /// Banda estimada até essa pessoa (bps).
    estimate: Option<u64>,
    /// A altura que pediu ("view"; `None` = ainda não pediu: a transmissão inteira).
    wants: Option<u32>,
    /// A camada que está recebendo (`None` = ainda nenhuma).
    layer: Option<u32>,
    /// Desde quando está sem banda para a camada atual / com folga para a de cima.
    timers: layers::Timers,
    /// Só nos testes: finge uma conexão mais lenta do que a real.
    estimate_cap: Option<u64>,
}

pub struct Sender {
    running: AtomicBool,
    latest: Mutex<Option<(VideoPacket, Instant)>>,
    frame_ready: Condvar,
    viewers: Mutex<HashMap<String, ViewerEntry>>,
    /// Camadas que precisam de um quadro-chave.
    keyframes: Mutex<HashSet<u32>>,
    /// Alguém entrou, saiu ou pediu outro tamanho: rever as camadas já.
    reassign: AtomicBool,
    /// A transmissão atual (altura, fps, bitrate), para as contas de banda.
    plan: Mutex<Option<Plan>>,
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
        s.request_all_keyframes();
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
        keyframes: Mutex::new(HashSet::new()),
        reassign: AtomicBool::new(true),
        plan: Mutex::new(None),
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
        ViewerEntry {
            tx: tx.clone(),
            session,
            name: name.to_string(),
            connected: false,
            estimate: None,
            wants: None,
            layer: None,
            timers: Default::default(),
            estimate_cap: None,
        },
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

/// O tamanho em que alguém está vendo (a mensagem "view"; 0 = app escondido).
pub fn viewer_view(id: &str, height: u32) -> Result<(), String> {
    let sender = current().ok_or("not sharing")?;
    let mut viewers = sender.viewers.lock().unwrap();
    let v = viewers.get_mut(id).ok_or("no connection with this viewer")?;
    v.wants = Some(height);
    sender.reassign.store(true, Ordering::Relaxed);
    Ok(())
}

/// Só nos testes: a banda até `id` passa a ser no máximo `bps` (conexão lenta de mentira).
#[cfg(test)]
pub fn cap_estimate(id: &str, bps: u64) -> Result<(), String> {
    let sender = current().ok_or("not sharing")?;
    let mut viewers = sender.viewers.lock().unwrap();
    let v = viewers.get_mut(id).ok_or("no connection with this viewer")?;
    v.estimate_cap = Some(bps);
    v.estimate = Some(v.estimate.map_or(bps, |e| e.min(bps)));
    Ok(())
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

    /// Um espectador precisa de quadro-chave (entrou, perdeu pacotes, trocou
    /// de camada): pede na camada dele.
    fn request_keyframe(&self, id: &str) {
        let layer = self.viewers.lock().unwrap().get(id).and_then(|v| v.layer);
        if let Some(l) = layer.filter(|&l| l != NO_VIDEO) {
            self.keyframes.lock().unwrap().insert(l);
        }
    }

    /// Captura nova: todas as camadas em uso começam com quadro-chave.
    fn request_all_keyframes(&self) {
        let layers: Vec<u32> = self.viewers.lock().unwrap().values().filter_map(|v| v.layer).collect();
        self.keyframes.lock().unwrap().extend(layers.into_iter().filter(|&l| l != NO_VIDEO));
    }

    fn set_connected(&self, id: &str, session: u64, connected: bool) {
        if let Some(v) = self.viewers.lock().unwrap().get_mut(id).filter(|v| v.session == session) {
            v.connected = connected;
            if !connected {
                v.layer = None;
            }
        }
        // a camada (e o quadro-chave dela) sai na próxima volta do laço
        self.reassign.store(true, Ordering::Relaxed);
    }

    fn set_estimate(&self, id: &str, session: u64, bps: u64) {
        if let Some(v) = self.viewers.lock().unwrap().get_mut(id).filter(|v| v.session == session) {
            v.estimate = Some(v.estimate_cap.map_or(bps, |cap| bps.min(cap)));
        }
    }

    /// A thread do espectador terminou.
    fn forget(&self, id: &str, session: u64) {
        let mut viewers = self.viewers.lock().unwrap();
        if viewers.get(id).is_some_and(|v| v.session == session) {
            viewers.remove(id);
        }
    }

    // ---------- camadas ----------

    /// Banda de vídeo até alguém: a estimada, menos o som.
    fn video_estimate(&self, estimate: Option<u64>) -> Option<u64> {
        let audio = if self.has_audio.load(Ordering::Relaxed) { audio::BITRATE as u64 } else { 0 };
        estimate.map(|e| e.saturating_sub(audio))
    }

    /// O que o controle de banda do str0m deve fazer até `id`: (mínimo a
    /// sustentar, desejado, sondar). O str0m completa com enchimento até o
    /// mínimo (sem passar da estimativa): sem isso a estimativa fica presa no
    /// que a mídia manda (às vezes bem menos, com a tela parada) e cai, e o
    /// espectador desceria de camada à toa. Na camada pedida, o mínimo é o
    /// bitrate dela; abaixo, o da camada de cima, e `sondar` pede sondagens
    /// até o desejado para a estimativa poder crescer e ele subir.
    fn bwe_wish(&self, id: &str) -> Option<(u64, u64, bool)> {
        let plan = (*self.plan.lock().unwrap())?;
        let viewers = self.viewers.lock().unwrap();
        let v = viewers.get(id)?;
        let layer = v.layer?;
        let audio = if self.has_audio.load(Ordering::Relaxed) { audio::BITRATE as u64 } else { 0 };
        let wanted = plan.wanted(v.wants);
        if wanted == NO_VIDEO || layer == NO_VIDEO {
            return Some((0, audio.max(layers::MIN_BITRATE as u64), false));
        }
        let up = plan.layers().into_iter().find(|&l| l > layer && l <= wanted);
        let minimum = plan.rate(up.unwrap_or(layer)) as u64 + audio;
        Some((minimum, plan.rate(wanted) as u64 + audio, up.is_some()))
    }

    /// Revê a camada de cada um e anota o porquê de cada troca.
    fn assign(&self, plan: &Plan, now: Instant) {
        let mut keyframes = Vec::new();
        {
            let mut viewers = self.viewers.lock().unwrap();
            for v in viewers.values_mut().filter(|v| v.connected) {
                let est = self.video_estimate(v.estimate);
                let next = plan.choose(v.wants, est, v.layer, &mut v.timers, now);
                if v.layer == Some(next) {
                    continue;
                }
                let why = layer_change(plan, v.wants, est, v.layer, next);
                log_info!("sharing", "Native sending: {} {why}", v.name);
                v.layer = Some(next);
                if next != NO_VIDEO {
                    keyframes.push(next);
                }
            }
        }
        self.keyframes.lock().unwrap().extend(keyframes);
    }

    // ---------- codificação ----------

    fn encode_loop(self: Arc<Self>) {
        let mut layers: BTreeMap<u32, Layer> = BTreeMap::new();
        let mut scaler = scale::Scaler::default();
        // a placa falhou 3 vezes seguidas: fica no processador
        let mut gpu_failed = false;
        let mut failures = 0u32;
        let mut last_check: Option<Instant> = None;
        let mut stats_since = Instant::now();
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
                        if !self.keyframes.lock().unwrap().is_empty()
                            || last_encode.elapsed() >= REPEAT_STILL_FRAME =>
                    {
                        (f.clone(), Instant::now())
                    }
                    _ => continue,
                },
            };

            // ninguém conectado: não gasta a placa
            if !self.anyone_connected() {
                continue;
            }

            let config = *self.config.lock().unwrap();
            let plan = Plan { top: frame.height() as u32, fps: config.fps, max_bitrate: config.max_bitrate };
            *self.plan.lock().unwrap() = Some(plan);
            let now = Instant::now();
            let check =
                self.reassign.swap(false, Ordering::Relaxed) || last_check.is_none_or(|t| now - t >= CHECK_EVERY);
            if check {
                last_check = Some(now);
                self.assign(&plan, now);
            }

            // quem recebe cada camada, e a banda de cada um
            let mut active: BTreeMap<u32, Vec<u64>> = BTreeMap::new();
            for v in self.viewers.lock().unwrap().values().filter(|v| v.connected) {
                if let Some(l) = v.layer.filter(|&l| l != NO_VIDEO) {
                    let list = active.entry(l).or_default();
                    list.extend(self.video_estimate(v.estimate));
                }
            }
            layers.retain(|h, _| {
                let keep = active.contains_key(h);
                if !keep {
                    log_info!("sharing", "Native sending stopped the {}p layer (nobody gets it now)", h);
                }
                keep
            });
            self.keyframes.lock().unwrap().retain(|h| active.contains_key(h));

            let software_only = gpu_failed || !config.gpu;
            let timestamp = captured.duration_since(self.started);
            last_encode = now;
            for (&h, estimates) in &active {
                let layer = layers.entry(h).or_default();
                let want_key = self.keyframes.lock().unwrap().contains(&h);
                // camada a 30 fps numa transmissão a 60: um frame sim, um não
                if plan.fps(h) < plan.fps && !want_key {
                    let skip = layer.skip_next;
                    layer.skip_next = !skip;
                    if skip {
                        continue;
                    }
                }
                let scaled;
                let input = if h >= plan.top {
                    &frame
                } else {
                    match scaler.scale(&frame, h) {
                        Ok(f) => {
                            scaled = f;
                            &scaled
                        }
                        Err(e) => {
                            log_error!("sharing", "Could not shrink the frame to {}p: {e}", h);
                            continue;
                        }
                    }
                };
                let size = (input.width() as u32, input.height() as u32, software_only);
                if layer.encoder.is_none() || layer.opened != size {
                    layer.encoder = None;
                    layer.bitrate = plan.layer_bitrate(h, estimates.iter().copied());
                    let enc_config = EncoderConfig {
                        width: size.0,
                        height: size.1,
                        fps: plan.fps(h),
                        bitrate: layer.bitrate,
                        software_only,
                    };
                    let me = Arc::downgrade(&self);
                    let on_encoded: encode::OnEncoded = Arc::new(move |f| {
                        if let Some(sender) = me.upgrade() {
                            sender.broadcast(h, f);
                        }
                    });
                    match encode::open(enc_config, on_encoded) {
                        Ok(e) => {
                            log_info!(
                                "sharing",
                                "Native sending: {}p layer ({}x{} at {} fps, {} kbps) encodes with {}",
                                h,
                                size.0,
                                size.1,
                                plan.fps(h),
                                layer.bitrate / 1000,
                                e.describe()
                            );
                            if software_only && !config.gpu {
                                log_info!("sharing", "Hardware acceleration is off, so native sending encodes on the processor");
                            } else if !e.hardware() && !software_only {
                                log_warn!("sharing", "No graphics card encoder worked here, encoding on the processor");
                            }
                            layer.encoder = Some(e);
                            layer.opened = size;
                            self.keyframes.lock().unwrap().insert(h);
                        }
                        Err(e) => {
                            log_error!("sharing", "Could not start a video encoder for {}p: {e}", h);
                            failures += 1;
                            if failures >= 3 && !software_only {
                                gpu_failed = true;
                            }
                            continue;
                        }
                    }
                }
                let enc = layer.encoder.as_mut().unwrap();

                // banda: o mais lento da camada, com folga, nos limites dela
                if check {
                    let target = plan.layer_bitrate(h, estimates.iter().copied());
                    if target.abs_diff(layer.bitrate) as u64 * 10 > layer.bitrate as u64 {
                        log_info!(
                            "sharing",
                            "Native sending: {}p layer bitrate {} -> {} kbps (slowest viewer there can take ~{} kbps)",
                            h,
                            layer.bitrate / 1000,
                            target / 1000,
                            estimates.iter().min().copied().unwrap_or(0) / 1000
                        );
                        layer.bitrate = target;
                        enc.set_bitrate(target);
                    }
                }

                let want_key = self.keyframes.lock().unwrap().contains(&h);
                let keyframe = want_key && layer.last_keyframe.is_none_or(|t| t.elapsed() >= KEYFRAME_MIN_INTERVAL);
                if keyframe {
                    self.keyframes.lock().unwrap().remove(&h);
                    layer.last_keyframe = Some(now);
                }
                let t0 = Instant::now();
                match enc.encode(input, timestamp, keyframe) {
                    Ok(()) => {
                        failures = 0;
                        layer.frames += 1;
                        layer.busy += t0.elapsed();
                    }
                    Err(e) => {
                        failures += 1;
                        log_error!("sharing", "The {}p video encoder failed ({failures}x): {e}", h);
                        layer.encoder = None;
                        if failures >= 3 && !software_only {
                            gpu_failed = true;
                            log_warn!("sharing", "Switching native sending to the processor encoder");
                        }
                    }
                }
            }

            if stats_since.elapsed() >= Duration::from_secs(30) {
                let secs = stats_since.elapsed().as_secs_f64();
                stats_since = Instant::now();
                let viewers = self.viewers.lock().unwrap();
                for (&h, layer) in layers.iter_mut() {
                    let names: Vec<&str> =
                        viewers.values().filter(|v| v.connected && v.layer == Some(h)).map(|v| v.name.as_str()).collect();
                    log_info!(
                        "sharing",
                        "Native sending, {}p layer: {} frames handed to the encoder in the last 30s ({:.0} fps), \
                         {:.1} ms each, {} kbps, for {}",
                        h,
                        layer.frames,
                        layer.frames as f64 / secs,
                        layer.busy.as_secs_f64() * 1000.0 / layer.frames.max(1) as f64,
                        layer.bitrate / 1000,
                        names.join(", ")
                    );
                    if let Some(t) = layer.encoder.as_mut().and_then(|e| e.take_timings()) {
                        log_info!("sharing", "Native sending, {}p layer, per frame: {t}", h);
                    }
                    layer.frames = 0;
                    layer.busy = Duration::ZERO;
                }
                let hidden: Vec<&str> =
                    viewers.values().filter(|v| v.connected && v.layer == Some(NO_VIDEO)).map(|v| v.name.as_str()).collect();
                if !hidden.is_empty() {
                    log_info!("sharing", "Native sending: only audio for {} (app hidden)", hidden.join(", "));
                }
            }
        }
        log_info!("sharing", "Native encoder thread finished");
    }
}

impl Sender {
    /// Um frame codificado de uma camada: o mesmo para todos que a recebem.
    fn broadcast(&self, layer: u32, f: EncodedFrame) {
        let shared =
            Arc::new(Encoded { data: f.data.into(), keyframe: f.keyframe, timestamp: f.timestamp, layer });
        for v in self.viewers.lock().unwrap().values().filter(|v| v.connected && v.layer == Some(layer)) {
            let _ = v.tx.send(ViewerMsg::Frame(shared.clone()));
        }
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

/// Uma camada no laço de codificação.
#[derive(Default)]
struct Layer {
    encoder: Option<Box<dyn VideoEncoder>>,
    /// Largura, altura e "só processador" com que o codificador foi aberto.
    opened: (u32, u32, bool),
    bitrate: u32,
    last_keyframe: Option<Instant>,
    /// Camada a 30 fps numa transmissão a 60: pula o próximo frame.
    skip_next: bool,
    frames: u64,
    busy: Duration,
}

fn label(layer: u32) -> String {
    if layer == NO_VIDEO {
        "no video".into()
    } else {
        format!("{layer}p")
    }
}

/// O porquê de uma troca de camada, para o log.
fn layer_change(plan: &Plan, wants: Option<u32>, est: Option<u64>, from: Option<u32>, to: u32) -> String {
    let kbps = |b: u64| b / 1000;
    let conn = est.map(|e| format!("its connection takes ~{} kbps", kbps(e))).unwrap_or_default();
    let wanted = plan.wanted(wants);
    if to == NO_VIDEO {
        return "isn't looking at my screen (app hidden): sending only audio".into();
    }
    match from {
        None | Some(NO_VIDEO) if to == wanted => format!("gets {}", label(to)),
        None | Some(NO_VIDEO) => format!("gets {} instead of {} ({conn})", label(to), label(wanted)),
        Some(f) if to < f && wanted < f => format!("asked for {} (a smaller view)", label(to)),
        Some(f) if to < f => format!(
            "moved down to {}: {conn}, {} needs ~{} kbps",
            label(to),
            label(f),
            kbps(plan.rate(f) as u64)
        ),
        Some(_) => format!("moved up to {} ({conn})", label(to)),
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

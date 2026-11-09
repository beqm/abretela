//! Uma conexão do envio nativo com um espectador: o `Rtc` do str0m, um
//! socket UDP por placa de rede (rotas "rede local"), STUN em cada um (rotas
//! "public address") e a thread que junta tudo.
//!
//! Tudo o que chega (rede, frames, som, resposta e rotas do outro lado) entra
//! por um canal só, e a thread processa em ordem. Vídeo e áudio saem no mesmo
//! stream ("abretela"), então o navegador os junta numa `MediaStream` só e
//! sincroniza pelo tempo de cada um.

use super::{audio::SAMPLE_RATE, stun, Encoded, Outgoing, Sender, Signals};
use crate::encode::has_idr;
use std::{
    net::{SocketAddr, ToSocketAddrs, UdpSocket},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};
use str0m::{
    bwe::{Bitrate, BweKind},
    change::{SdpAnswer, SdpPendingOffer},
    format::Codec,
    media::{Direction, Frequency, MediaKind, MediaTime, Mid, Pt},
    net::{Protocol, Receive},
    Candidate, Event, IceConnectionState, Input, Output, Rtc,
};

/// Sem rota funcionando por esse tempo, desiste (quem assiste tenta de novo).
const GIVE_UP_AFTER: Duration = Duration::from_secs(30);
const HEALTH_EVERY: Duration = Duration::from_secs(30);
/// Abaixo da camada pedida, sonda a banda nesse ritmo.
const PROBE_EVERY: Duration = Duration::from_secs(5);

pub enum ViewerMsg {
    Net { local: SocketAddr, source: SocketAddr, data: Vec<u8> },
    /// Resposta do STUN: `base` (socket local) é visto como `public`.
    PublicAddress { base: SocketAddr, public: SocketAddr, server: String },
    Frame(Arc<Encoded>),
    /// Um pacote Opus (20 ms).
    Audio(Arc<Encoded>),
    Answer(String),
    RemoteCandidate(String),
    Close,
}

struct Socket {
    socket: Arc<UdpSocket>,
    local: SocketAddr,
}

/// Abre os sockets, cria a oferta e começa a thread. Devolve o canal dela.
#[allow(clippy::too_many_arguments)]
pub fn spawn(
    id: String,
    name: String,
    session: u64,
    bitrate: u32,
    sender: Arc<Sender>,
    signals: Arc<dyn Signals>,
    tx: mpsc::Sender<ViewerMsg>,
    rx: mpsc::Receiver<ViewerMsg>,
) -> Result<(), String> {
    let closed = Arc::new(AtomicBool::new(false));
    let who = format!("to {name}");

    let mut rtc = Rtc::builder()
        .clear_codecs()
        .enable_h264(true)
        .enable_opus(true, false)
        .enable_bwe(Some(Bitrate::bps(bitrate as u64)))
        .set_stats_interval(Some(HEALTH_EVERY))
        .build(Instant::now());

    // um socket por endereço da máquina: assim sabemos por onde cada pacote chegou
    let mut sockets = Vec::new();
    for ip in crate::logging::network::usable_addresses() {
        let Ok(socket) = UdpSocket::bind(SocketAddr::new(ip, 0)) else { continue };
        let Ok(local) = socket.local_addr() else { continue };
        let Ok(candidate) = Candidate::host(local, "udp") else { continue };
        if rtc.add_local_candidate(candidate).is_none() {
            continue;
        }
        log_info!("sharing", "Native route on this PC {who}: local network {local} over UDP");
        let socket = Arc::new(socket);
        spawn_reader(socket.clone(), local, tx.clone(), closed.clone());
        sockets.push(Socket { socket, local });
    }
    if sockets.is_empty() {
        return Err("no network address to listen on".into());
    }

    let mut api = rtc.sdp_api();
    let mid = api.add_media(MediaKind::Video, Direction::SendOnly, Some("abretela".into()), Some("screen".into()), None);
    let audio_mid = sender.has_audio.load(Ordering::Relaxed).then(|| {
        api.add_media(MediaKind::Audio, Direction::SendOnly, Some("abretela".into()), Some("sound".into()), None)
    });
    let (offer, pending) = api.apply().ok_or("could not create the offer")?;
    signals.send(&id, Outgoing::Offer { sdp: offer.to_sdp_string() });
    log_info!(
        "sharing",
        "Sent the native offer {who} ({}, {} routes inside)",
        if audio_mid.is_some() { "H.264 video + Opus audio" } else { "H.264 video, no audio" },
        sockets.len()
    );

    // rotas públicas: pergunta aos servidores STUN por cada socket IPv4/IPv6
    for s in &sockets {
        for server in stun::SERVERS {
            let Ok(mut addrs) = server.to_socket_addrs() else { continue };
            if let Some(addr) = addrs.find(|a| a.is_ipv4() == s.local.is_ipv4()) {
                let _ = s.socket.send_to(&stun::binding_request(stun::new_transaction()), addr);
            }
        }
    }

    let session = Session {
        id,
        session,
        who,
        rtc,
        sockets,
        mid,
        audio_mid,
        pending: Some(pending),
        pt: None,
        audio_pt: None,
        audio_sent: 0,
        sender,
        signals,
        closed,
        started: Instant::now(),
        connected: false,
        waiting_keyframe: true,
        layer: None,
        bwe_wish: None,
        bwe_checked: Instant::now(),
        last_probe: Instant::now(),
        disconnected_since: None,
        frames_sent: 0,
        frames_skipped: 0,
        last_estimate: 0,
        public_seen: Vec::new(),
    };
    thread::Builder::new()
        .name("native-viewer".into())
        .spawn(move || session.run(rx))
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn spawn_reader(socket: Arc<UdpSocket>, local: SocketAddr, tx: mpsc::Sender<ViewerMsg>, closed: Arc<AtomicBool>) {
    let _ = socket.set_read_timeout(Some(Duration::from_millis(250)));
    thread::spawn(move || {
        let mut buf = vec![0u8; 2048];
        while !closed.load(Ordering::Relaxed) {
            match socket.recv_from(&mut buf) {
                Ok((n, source)) => {
                    let data = &buf[..n];
                    let msg = match stun::parse_our_response(data) {
                        Some(public) => ViewerMsg::PublicAddress { base: local, public, server: source.to_string() },
                        None => ViewerMsg::Net { local, source, data: data.to_vec() },
                    };
                    if tx.send(msg).is_err() {
                        return;
                    }
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                // o Windows avisa "porta inalcançável" (ICMP) num recv: não é fatal
                Err(_) => thread::sleep(Duration::from_millis(5)),
            }
        }
    });
}

struct Session {
    id: String,
    session: u64,
    who: String,
    rtc: Rtc,
    sockets: Vec<Socket>,
    mid: Mid,
    audio_mid: Option<Mid>,
    pending: Option<SdpPendingOffer>,
    pt: Option<Pt>,
    audio_pt: Option<Pt>,
    audio_sent: u64,
    sender: Arc<Sender>,
    signals: Arc<dyn Signals>,
    closed: Arc<AtomicBool>,
    started: Instant,
    connected: bool,
    waiting_keyframe: bool,
    /// A camada dos frames que está recebendo.
    layer: Option<u32>,
    /// O último (mínimo, desejado, sondando) passado ao controle de banda, e quando.
    bwe_wish: Option<(u64, u64, bool)>,
    bwe_checked: Instant,
    last_probe: Instant,
    disconnected_since: Option<Instant>,
    frames_sent: u64,
    frames_skipped: u64,
    last_estimate: u64,
    public_seen: Vec<SocketAddr>,
}

impl Session {
    fn since(&self) -> String {
        format!("(+{:.1}s)", self.started.elapsed().as_secs_f64())
    }

    fn run(mut self, rx: mpsc::Receiver<ViewerMsg>) {
        let reason = loop {
            // tudo o que o str0m quer fazer agora (mandar pacotes, avisar coisas)
            let timeout = match self.drain_output() {
                Ok(t) => t,
                Err(e) => break format!("connection error: {e}"),
            };
            if !self.rtc.is_alive() {
                break "the connection closed".into();
            }
            if let Some(t) = self.disconnected_since {
                if t.elapsed() > GIVE_UP_AFTER {
                    self.signals.send(&self.id, Outgoing::State { state: "failed".into() });
                    break format!("no working route for {}s", GIVE_UP_AFTER.as_secs());
                }
            }
            let wait = timeout.saturating_duration_since(Instant::now());
            let msg = match rx.recv_timeout(wait) {
                Ok(m) => Some(m),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => break "stopped".into(),
            };
            let result = match msg {
                None => self.rtc.handle_input(Input::Timeout(Instant::now())).map_err(|e| e.to_string()),
                Some(ViewerMsg::Close) => break "closed by the app".into(),
                Some(m) => self.handle(m),
            };
            if let Err(e) = result {
                log_warn!("sharing", "Native connection {}: {e}", self.who);
            }
        };
        self.closed.store(true, Ordering::Relaxed);
        self.rtc.disconnect();
        self.sender.set_connected(&self.id, self.session, false);
        self.sender.forget(&self.id, self.session);
        self.signals.send(&self.id, Outgoing::State { state: "closed".into() });
        log_info!(
            "sharing",
            "Closed the native connection {} {} ({reason}; {} frames and {} audio packets sent)",
            self.who,
            self.since(),
            self.frames_sent,
            self.audio_sent
        );
    }

    fn handle(&mut self, msg: ViewerMsg) -> Result<(), String> {
        match msg {
            ViewerMsg::Net { local, source, data } => {
                let contents = data.as_slice().try_into().map_err(|e| format!("bad packet: {e:?}"))?;
                let input = Input::Receive(Instant::now(), Receive { proto: Protocol::Udp, source, destination: local, contents });
                // pacotes que o str0m não reconhece (ex.: respostas STUN atrasadas) não são erro
                if self.rtc.accepts(&input) {
                    self.rtc.handle_input(input).map_err(|e| e.to_string())?;
                }
            }
            ViewerMsg::PublicAddress { base, public, server } => {
                if self.public_seen.contains(&public) {
                    return Ok(());
                }
                self.public_seen.push(public);
                let candidate = Candidate::server_reflexive(public, base, "udp").map_err(|e| e.to_string())?;
                if let Some(c) = self.rtc.add_local_candidate(candidate) {
                    let line = c.to_sdp_string();
                    log_info!(
                        "sharing",
                        "Native route on this PC {}: public address (STUN) {public} behind {base}, from {server} {}",
                        self.who,
                        self.since()
                    );
                    self.signals.send(&self.id, Outgoing::Ice { candidate: line, mid: self.mid.to_string() });
                }
            }
            ViewerMsg::Answer(sdp) => {
                let pending = self.pending.take().ok_or("got a second answer")?;
                let answer = SdpAnswer::from_sdp_string(&sdp).map_err(|e| format!("unreadable answer: {e}"))?;
                self.rtc.sdp_api().accept_answer(pending, answer).map_err(|e| format!("answer refused: {e}"))?;
                self.pt = self.h264_pt();
                self.audio_pt = self.opus_pt();
                let audio = match (self.audio_mid, self.audio_pt) {
                    (None, _) => "no audio".to_string(),
                    (Some(_), Some(pt)) => format!("Opus payload {pt}"),
                    (Some(_), None) => "the audio was refused".to_string(),
                };
                match self.pt {
                    Some(pt) => {
                        log_info!("sharing", "Got the answer {}: H.264 payload {pt}, {audio} {}", self.who, self.since())
                    }
                    None => log_error!("sharing", "The answer {} has no H.264 we can send! ({audio})", self.who),
                }
            }
            ViewerMsg::RemoteCandidate(line) => {
                let line = line.trim().trim_start_matches("a=");
                if line.is_empty() {
                    return Ok(());
                }
                if line.contains(".local ") {
                    // o navegador esconde o IP local com um nome mDNS que não dá
                    // para resolver aqui; a rota vira "descoberta" quando chegar a checagem
                    log_info!("sharing", "Skipped a hidden (mDNS) route from the other PC {}", self.who);
                    return Ok(());
                }
                let c = Candidate::from_sdp_string(line).map_err(|e| format!("unreadable route \"{line}\": {e}"))?;
                log_info!(
                    "sharing",
                    "Native route from the other PC {}: {} {} over UDP {}",
                    self.who,
                    c.kind(),
                    c.addr(),
                    self.since()
                );
                self.rtc.add_remote_candidate(c);
            }
            ViewerMsg::Frame(frame) => {
                self.update_bwe();
                self.send_frame(&frame)?
            }
            ViewerMsg::Audio(packet) => self.send_audio(&packet)?,
            ViewerMsg::Close => {}
        }
        Ok(())
    }

    fn send_frame(&mut self, frame: &Encoded) -> Result<(), String> {
        let Some(pt) = self.pt else { return Ok(()) };
        if !self.connected {
            return Ok(());
        }
        if self.layer != Some(frame.layer) {
            // trocou de camada: outra resolução, só dá para seguir de um quadro-chave
            self.layer = Some(frame.layer);
            self.waiting_keyframe = true;
        }
        if self.waiting_keyframe {
            // só começa num quadro-chave: sem ele o navegador não decodifica nada
            if !(frame.keyframe || has_idr(&frame.data)) {
                self.frames_skipped += 1;
                self.sender.request_keyframe(&self.id);
                return Ok(());
            }
            self.waiting_keyframe = false;
            log_info!("sharing", "First native video frame out {} {}", self.who, self.since());
        }
        let wallclock = self.sender.started + frame.timestamp;
        let Some(writer) = self.rtc.writer(self.mid) else { return Ok(()) };
        let rtp_time = MediaTime::new((frame.timestamp.as_micros() as u64) * 90 / 1000, Frequency::NINETY_KHZ);
        writer
            .write(pt, wallclock, rtp_time, frame.data.clone())
            .map_err(|e| format!("could not send a frame: {e}"))?;
        self.frames_sent += 1;
        Ok(())
    }

    /// Conta ao controle de banda quanto sustentar e até onde sondar (ver
    /// `Sender::bwe_wish`), no máximo uma vez por segundo.
    fn update_bwe(&mut self) {
        if self.bwe_checked.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.bwe_checked = Instant::now();
        let Some(wish) = self.sender.bwe_wish(&self.id) else { return };
        let (minimum, desired, probing) = wish;
        if self.bwe_wish == Some(wish) {
            // o str0m só sonda quando o desejado sobe: abaixo da camada pedida,
            // baixa e sobe de novo de tempos em tempos
            if probing && self.last_probe.elapsed() >= PROBE_EVERY {
                self.last_probe = Instant::now();
                let mut bwe = self.rtc.bwe();
                bwe.set_bitrate(Bitrate::bps(minimum), Bitrate::bps(desired / 2));
                bwe.set_bitrate(Bitrate::bps(minimum), Bitrate::bps(desired));
            }
            return;
        }
        if probing && !self.bwe_wish.is_some_and(|w| w.2) {
            log_info!(
                "sharing",
                "Native sending {}: probing whether the connection takes ~{} kbps (to go back up a layer) {}",
                self.who,
                minimum / 1000,
                self.since()
            );
        }
        self.bwe_wish = Some(wish);
        self.last_probe = Instant::now();
        self.rtc.bwe().set_bitrate(Bitrate::bps(minimum), Bitrate::bps(desired));
    }

    /// Um pacote Opus. O tempo é do mesmo relógio do vídeo (desde o começo da
    /// transmissão), e o `wallclock` igual nos dois deixa o navegador
    /// sincronizar som e imagem.
    fn send_audio(&mut self, packet: &Encoded) -> Result<(), String> {
        let (Some(mid), Some(pt)) = (self.audio_mid, self.audio_pt) else { return Ok(()) };
        if !self.connected {
            return Ok(());
        }
        let wallclock = self.sender.started + packet.timestamp;
        let Some(writer) = self.rtc.writer(mid) else { return Ok(()) };
        let samples = packet.timestamp.as_micros() as u64 * SAMPLE_RATE as u64 / 1_000_000;
        writer
            .write(pt, wallclock, MediaTime::new(samples, Frequency::FORTY_EIGHT_KHZ), packet.data.clone())
            .map_err(|e| format!("could not send audio: {e}"))?;
        if self.audio_sent == 0 {
            log_info!("sharing", "First native audio packet out {} {}", self.who, self.since());
        }
        self.audio_sent += 1;
        Ok(())
    }

    /// O Opus que o outro lado aceitou.
    fn opus_pt(&mut self) -> Option<Pt> {
        let writer = self.rtc.writer(self.audio_mid?)?;
        let pt = writer.payload_params().find(|p| p.spec().codec == Codec::Opus).map(|p| p.pt());
        pt
    }

    /// O H.264 Constrained Baseline com packetization-mode 1 que o outro lado aceitou.
    fn h264_pt(&mut self) -> Option<Pt> {
        let writer = self.rtc.writer(self.mid)?;
        let params: Vec<_> = writer.payload_params().cloned().collect();
        let h264: Vec<_> = params.iter().filter(|p| p.spec().codec == Codec::H264).collect();
        h264.iter()
            .find(|p| {
                let f = p.spec().format;
                f.packetization_mode == Some(1) && f.profile_level_id.map(|id| id >> 8) == Some(0x42e0)
            })
            .or_else(|| h264.iter().find(|p| p.spec().format.packetization_mode == Some(1)))
            .map(|p| p.pt())
    }

    /// Processa a saída do str0m até ele pedir para esperar; devolve até quando.
    fn drain_output(&mut self) -> Result<Instant, String> {
        loop {
            match self.rtc.poll_output().map_err(|e| e.to_string())? {
                Output::Timeout(t) => return Ok(t),
                Output::Transmit(t) => {
                    if let Some(s) = self.sockets.iter().find(|s| s.local == t.source) {
                        let _ = s.socket.send_to(&t.contents, t.destination);
                    }
                }
                Output::Event(e) => self.event(e),
            }
        }
    }

    fn event(&mut self, e: Event) {
        match e {
            Event::IceConnectionStateChange(state) => {
                log_info!("sharing", "Native route checks (ICE) {}: {state:?} {}", self.who, self.since());
                match state {
                    IceConnectionState::Disconnected => {
                        self.disconnected_since.get_or_insert_with(Instant::now);
                        if self.connected {
                            log_warn!("sharing", "Native connection {} dropped, trying to recover", self.who);
                            self.signals.send(&self.id, Outgoing::State { state: "disconnected".into() });
                        }
                    }
                    IceConnectionState::Connected | IceConnectionState::Completed => {
                        self.disconnected_since = None;
                    }
                    _ => {}
                }
            }
            Event::Connected => {
                self.connected = true;
                self.waiting_keyframe = true;
                self.sender.set_connected(&self.id, self.session, true);
                self.signals.send(&self.id, Outgoing::State { state: "connected".into() });
                log_info!("sharing", "Native connection {} is up (routes and encryption ready) {}", self.who, self.since());
            }
            Event::KeyframeRequest(r) => {
                self.sender.request_keyframe(&self.id);
                if self.frames_sent < 5 {
                    log_info!("sharing", "{} asked for a keyframe ({:?})", self.who, r.kind);
                }
            }
            Event::EgressBitrateEstimate(BweKind::Twcc { estimate, .. }) => {
                let bps = estimate.as_u64();
                self.sender.set_estimate(&self.id, self.session, bps);
                if bps.abs_diff(self.last_estimate) * 4 > self.last_estimate {
                    log_info!("sharing", "Native bandwidth estimate {}: {} kbps {}", self.who, bps / 1000, self.since());
                    self.last_estimate = bps;
                }
            }
            // uma linha só por espectador: a do vídeo (o áudio entra na contagem)
            Event::MediaEgressStats(s) if s.mid == self.mid => {
                let loss = s.loss.map(|l| format!("{:.1}% lost", l * 100.0)).unwrap_or_else(|| "loss ?".into());
                let rtt = s.rtt.map(|r| format!("{} ms", r.as_millis())).unwrap_or_else(|| "?".into());
                log_info!(
                    "sharing",
                    "Native health {} {}: {} frames and {} audio packets sent, {} KB of video, {loss}, \
                     round trip {rtt}, {} resend requests, {} keyframe requests",
                    self.who,
                    self.since(),
                    self.frames_sent,
                    self.audio_sent,
                    s.bytes / 1024,
                    s.nacks,
                    s.plis + s.firs,
                );
            }
            _ => {}
        }
    }
}

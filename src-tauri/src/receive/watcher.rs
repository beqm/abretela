//! Uma conexão da recepção nativa com quem transmite: o `Rtc` do str0m, um
//! socket UDP por placa de rede (rotas "rede local"), STUN em cada um (rotas
//! "public address") e a thread que junta tudo. A mesma forma de
//! `send/viewer.rs`, no sentido contrário.

use super::{opus::Rejoin, MediaSink, Outgoing, Received, Signals};
use crate::encode::has_idr;
use crate::send::stun;
use std::{
    net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};
use str0m::{
    change::SdpOffer,
    format::Codec,
    media::{KeyframeRequestKind, MediaData, MediaKind, MediaTime, Mid},
    net::{Protocol, Receive},
    Candidate, Event, IceConnectionState, Input, Output, Rtc,
};

/// Sem rota funcionando por esse tempo, desiste (o app tenta de novo).
const GIVE_UP_AFTER: Duration = Duration::from_secs(30);
/// No máximo um pedido de quadro-chave por esse intervalo.
const KEYFRAME_RETRY: Duration = Duration::from_secs(1);

pub enum WatchMsg {
    Net { local: SocketAddr, source: SocketAddr, data: Vec<u8> },
    /// Resposta do STUN: `base` (socket local) é visto como `public`.
    PublicAddress { base: SocketAddr, public: SocketAddr, server: String },
    RemoteCandidate(String),
    Close,
}

struct Socket {
    socket: Arc<UdpSocket>,
    local: SocketAddr,
}

/// A oferta sem as rotas que o navegador escondeu atrás de um nome mDNS
/// (`algo.local`): não dá para resolver esses nomes aqui, e a rota aparece
/// sozinha quando a checagem do outro lado chegar.
pub(super) fn without_hidden_routes(sdp: &str) -> String {
    sdp.split_inclusive('\n').filter(|line| !(line.starts_with("a=candidate:") && line.contains(".local "))).collect()
}

/// O `mid` da primeira mídia da oferta, que acompanha as rotas na sinalização.
pub(super) fn first_mid(sdp: &str) -> String {
    sdp.lines().find_map(|line| line.trim_end().strip_prefix("a=mid:")).unwrap_or("0").to_string()
}

/// Abre os sockets, responde à oferta e começa a thread.
#[allow(clippy::too_many_arguments)]
pub fn spawn(
    id: String,
    name: String,
    offer_sdp: &str,
    addresses: Vec<IpAddr>,
    signals: Arc<dyn Signals>,
    sink: Arc<dyn MediaSink>,
    tx: mpsc::Sender<WatchMsg>,
    rx: mpsc::Receiver<WatchMsg>,
) -> Result<(), String> {
    let closed = Arc::new(AtomicBool::new(false));
    let who = format!("from {name}");

    let mut rtc = Rtc::builder().clear_codecs().enable_h264(true).enable_opus(true, false).build(Instant::now());

    // um socket por endereço da máquina: assim sabemos por onde cada pacote chegou
    let mut sockets = Vec::new();
    for ip in addresses {
        let Ok(socket) = UdpSocket::bind(SocketAddr::new(ip, 0)) else { continue };
        let Ok(local) = socket.local_addr() else { continue };
        let Ok(candidate) = Candidate::host(local, "udp") else { continue };
        if rtc.add_local_candidate(candidate).is_none() {
            continue;
        }
        log_info!("watching", "Native route on this PC {who}: local network {local} over UDP");
        let socket = Arc::new(socket);
        spawn_reader(socket.clone(), local, tx.clone(), closed.clone());
        sockets.push(Socket { socket, local });
    }
    if sockets.is_empty() {
        return Err("no network address to listen on".into());
    }

    let offer = SdpOffer::from_sdp_string(&without_hidden_routes(offer_sdp)).map_err(|e| format!("unreadable offer: {e}"))?;
    let answer = match rtc.sdp_api().accept_offer(offer) {
        Ok(answer) => answer,
        Err(e) => {
            closed.store(true, Ordering::Relaxed);
            return Err(format!("offer refused: {e}"));
        }
    };
    signals.send(&id, Outgoing::Answer { sdp: answer.to_sdp_string() });
    log_info!("watching", "Sent the native answer {who} ({} routes inside)", sockets.len());

    // rotas públicas: pergunta aos servidores STUN por cada socket IPv4/IPv6
    for s in sockets.iter().filter(|s| !s.local.ip().is_loopback()) {
        for server in stun::SERVERS {
            let Ok(mut addrs) = server.to_socket_addrs() else { continue };
            if let Some(addr) = addrs.find(|a| a.is_ipv4() == s.local.is_ipv4()) {
                let _ = s.socket.send_to(&stun::binding_request(stun::new_transaction()), addr);
            }
        }
    }

    let session = Session {
        id,
        who,
        rtc,
        sockets,
        mid: first_mid(offer_sdp),
        signals,
        sink,
        closed,
        started: Instant::now(),
        connected: false,
        disconnected_since: Some(Instant::now()),
        public_seen: Vec::new(),
        video_mid: None,
        waiting_keyframe: true,
        asked_keyframe: None,
        first_media: None,
        video_clock: None,
        audio_clock: None,
        audio_rejoin: Rejoin::default(),
        frames: 0,
        frames_skipped: 0,
        audio_packets: 0,
    };
    thread::Builder::new().name("native-watcher".into()).spawn(move || session.run(rx)).map_err(|e| e.to_string())?;
    Ok(())
}

fn spawn_reader(socket: Arc<UdpSocket>, local: SocketAddr, tx: mpsc::Sender<WatchMsg>, closed: Arc<AtomicBool>) {
    let _ = socket.set_read_timeout(Some(Duration::from_millis(250)));
    thread::spawn(move || {
        let mut buf = vec![0u8; 2048];
        while !closed.load(Ordering::Relaxed) {
            match socket.recv_from(&mut buf) {
                Ok((n, source)) => {
                    let data = &buf[..n];
                    let msg = match stun::parse_our_response(data) {
                        Some(public) => WatchMsg::PublicAddress { base: local, public, server: source.to_string() },
                        None => WatchMsg::Net { local, source, data: data.to_vec() },
                    };
                    if tx.send(msg).is_err() {
                        return;
                    }
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                // "porta inalcançável" (ICMP) num recv não é fatal
                Err(_) => thread::sleep(Duration::from_millis(5)),
            }
        }
    });
}

/// De onde contar o tempo de uma faixa: o tempo de mídia do primeiro pedaço
/// e quanto depois do começo de tudo ele chegou (para o vídeo e o som
/// ficarem no mesmo relógio).
#[derive(Clone, Copy)]
struct Clock {
    first: MediaTime,
    offset: Duration,
}

impl Clock {
    fn at(&self, time: MediaTime) -> Duration {
        self.offset + Duration::from_micros(time.saturating_sub(self.first).as_micros())
    }
}

struct Session {
    id: String,
    who: String,
    rtc: Rtc,
    sockets: Vec<Socket>,
    /// O `mid` que acompanha as rotas na sinalização.
    mid: String,
    signals: Arc<dyn Signals>,
    sink: Arc<dyn MediaSink>,
    closed: Arc<AtomicBool>,
    started: Instant,
    connected: bool,
    disconnected_since: Option<Instant>,
    public_seen: Vec<SocketAddr>,
    video_mid: Option<Mid>,
    /// Só dá para começar (ou voltar, depois de perder um pedaço) por um quadro-chave.
    waiting_keyframe: bool,
    asked_keyframe: Option<Instant>,
    /// Quando chegou o primeiro pedaço de mídia.
    first_media: Option<Instant>,
    video_clock: Option<Clock>,
    audio_clock: Option<Clock>,
    audio_rejoin: Rejoin,
    frames: u64,
    frames_skipped: u64,
    audio_packets: u64,
}

impl Session {
    fn since(&self) -> String {
        format!("(+{:.1}s)", self.started.elapsed().as_secs_f64())
    }

    fn run(mut self, rx: mpsc::Receiver<WatchMsg>) {
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
                Some(WatchMsg::Close) => break "closed by the app".into(),
                Some(m) => self.handle(m),
            };
            if let Err(e) = result {
                log_warn!("watching", "Native connection {}: {e}", self.who);
            }
        };
        self.closed.store(true, Ordering::Relaxed);
        self.rtc.disconnect();
        self.signals.send(&self.id, Outgoing::State { state: "closed".into() });
        log_info!(
            "watching",
            "Closed the native connection {} {} ({reason}; {} frames and {} audio packets received, {} frames skipped waiting for a keyframe, {} audio packets that came split in two)",
            self.who,
            self.since(),
            self.frames,
            self.audio_packets,
            self.frames_skipped,
            self.audio_rejoin.joined
        );
    }

    fn handle(&mut self, msg: WatchMsg) -> Result<(), String> {
        match msg {
            WatchMsg::Net { local, source, data } => {
                let contents = data.as_slice().try_into().map_err(|e| format!("bad packet: {e:?}"))?;
                let input = Input::Receive(Instant::now(), Receive { proto: Protocol::Udp, source, destination: local, contents });
                // pacotes que o str0m não reconhece (ex.: respostas STUN atrasadas) não são erro
                if self.rtc.accepts(&input) {
                    self.rtc.handle_input(input).map_err(|e| e.to_string())?;
                }
            }
            WatchMsg::PublicAddress { base, public, server } => {
                if self.public_seen.contains(&public) {
                    return Ok(());
                }
                self.public_seen.push(public);
                let candidate = Candidate::server_reflexive(public, base, "udp").map_err(|e| e.to_string())?;
                if let Some(c) = self.rtc.add_local_candidate(candidate) {
                    let line = c.to_sdp_string();
                    log_info!(
                        "watching",
                        "Native route on this PC {}: public address (STUN) {public} behind {base}, from {server} {}",
                        self.who,
                        self.since()
                    );
                    self.signals.send(&self.id, Outgoing::Ice { candidate: line, mid: self.mid.clone() });
                }
            }
            WatchMsg::RemoteCandidate(line) => {
                let line = line.trim().trim_start_matches("a=");
                if line.is_empty() {
                    return Ok(());
                }
                if line.contains(".local ") {
                    // o navegador esconde o IP local com um nome mDNS que não dá
                    // para resolver aqui; a rota vira "descoberta" quando chegar a checagem
                    log_info!("watching", "Skipped a hidden (mDNS) route from the other PC {}", self.who);
                    return Ok(());
                }
                let c = Candidate::from_sdp_string(line).map_err(|e| format!("unreadable route \"{line}\": {e}"))?;
                log_info!(
                    "watching",
                    "Native route from the other PC {}: {} {} over UDP {}",
                    self.who,
                    c.kind(),
                    c.addr(),
                    self.since()
                );
                self.rtc.add_remote_candidate(c);
            }
            WatchMsg::Close => {}
        }
        Ok(())
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
                log_info!("watching", "Native route checks (ICE) {}: {state:?} {}", self.who, self.since());
                match state {
                    IceConnectionState::Disconnected => {
                        self.disconnected_since.get_or_insert_with(Instant::now);
                        if self.connected {
                            log_warn!("watching", "Native connection {} dropped, trying to recover", self.who);
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
                self.disconnected_since = None;
                log_info!("watching", "Native connection {} is up {}", self.who, self.since());
                self.signals.send(&self.id, Outgoing::State { state: "connected".into() });
                self.ask_keyframe();
            }
            Event::MediaAdded(media) if media.kind == MediaKind::Video => self.video_mid = Some(media.mid),
            Event::MediaData(data) => self.media(data),
            _ => {}
        }
    }

    fn media(&mut self, data: MediaData) {
        let arrived = *self.first_media.get_or_insert_with(Instant::now);
        let clock = |slot: &mut Option<Clock>| *slot.get_or_insert(Clock { first: data.time, offset: arrived.elapsed() });
        match data.params.spec().codec {
            Codec::H264 => {
                self.video_mid = Some(data.mid);
                let keyframe = data.is_keyframe() || has_idr(&data.data);
                if !data.contiguous {
                    // perdeu um pedaço do caminho: o que vem depois depende dele
                    self.waiting_keyframe = true;
                }
                if self.waiting_keyframe && !keyframe {
                    self.frames_skipped += 1;
                    self.ask_keyframe();
                    return;
                }
                self.waiting_keyframe = false;
                if self.frames == 0 {
                    log_info!("watching", "First video frame arrived {} ({} bytes) {}", self.who, data.data.len(), self.since());
                }
                self.frames += 1;
                let timestamp = clock(&mut self.video_clock).at(data.time);
                self.sink.video(Received { data: data.data, keyframe, timestamp });
            }
            Codec::Opus => {
                let timestamp = clock(&mut self.audio_clock).at(data.time);
                // quem envia às vezes parte um pacote grande em dois
                for (timestamp, data) in self.audio_rejoin.push(timestamp, &data.data) {
                    self.audio_packets += 1;
                    self.sink.audio(Received { data, keyframe: true, timestamp });
                }
            }
            _ => {}
        }
    }

    /// Pede um quadro-chave a quem transmite (no começo, e quando faltou um pedaço).
    fn ask_keyframe(&mut self) {
        if self.asked_keyframe.is_some_and(|t| t.elapsed() < KEYFRAME_RETRY) {
            return;
        }
        let Some(mid) = self.video_mid else { return };
        if let Some(mut writer) = self.rtc.writer(mid) {
            if writer.request_keyframe(None, KeyframeRequestKind::Pli).is_ok() {
                self.asked_keyframe = Some(Instant::now());
            }
        }
    }
}
